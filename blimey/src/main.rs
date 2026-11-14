mod api;
mod artnet;
mod enttec;
mod event_log;
mod hot_reload;
mod midi_input;
mod record;
mod recorder;
mod service;
mod soak;
mod state;
mod stream;
mod ui;

use std::error::Error;
use std::fs;
use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use blimey_engine::look::{Look, LookSettings};
use blimey_engine::ofl::import_ofl;
use blimey_engine::rig::Rig;
use blimey_engine::{
    ArtNetFrames, CueEvent, DmxSink, FakeClock, FakeDmx, Frame, MidiEvent, MidiFile, MidiSource,
    MonotonicClock, Show, ShowSource, Status, Universes, first_difference, format_dump, parse_dump,
};
use clap::{Args, Parser, Subcommand};
use serialport::SerialPortType;

use crate::api::Mode;
use crate::artnet::ArtNet;
use crate::enttec::EnttecPro;
use crate::enttec::emulator::EmulatedEnttec;
use crate::hot_reload::HotReload;
use crate::midi_input::MidiInput;
use crate::recorder::Recorder;
use crate::soak::{Setlist, Soak};
use crate::state::Resumed;
use crate::stream::{StreamSink, StreamedMidi, StreamedShow};

#[derive(Parser)]
#[command(version, about = "Headless DMX lighting engine driven by MIDI")]
enum Command {
    /// Render a MIDI file against a show offline, then write or check the frame dump.
    Replay {
        show_dir: PathBuf,
        #[arg(long)]
        midi: PathBuf,
        /// Write the frame dump to this file. Without --out or --compare it goes to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Reference dump to compare against; exits with status 1 at the first difference.
        #[arg(long)]
        compare: Option<PathBuf>,
    },
    /// Check a show folder: every file parses and refers to fixtures, groups and colours that
    /// exist. Errors name the file and line.
    Validate { show_dir: PathBuf },
    /// Run a show live: MIDI in from a port such as the IAC bus, out to an Enttec DMX USB Pro,
    /// the web UI or Art-Net. It resumes the last selected song and master level, and logs
    /// every MIDI event and cue to the state folder.
    Run {
        show_dir: PathBuf,
        /// MIDI input to listen on: the first port whose name contains this text.
        #[arg(long, default_value = "IAC")]
        midi_port: String,
        #[command(flatten)]
        outputs: Outputs,
        /// Where to keep the saved song and master (state.json) and the event log
        /// (events.log). Default: ~/Library/Application Support/Blimey
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Record what arrives on a MIDI port, such as Reaper playing a song over the IAC bus, to a
    /// Standard MIDI File for `blimey replay`, clock, Start and Stop included. Time 0 is the
    /// first message. Stops on Ctrl-C.
    Record {
        out: PathBuf,
        /// MIDI input to record: the first port whose name contains this text.
        #[arg(long, default_value = "IAC")]
        midi_port: String,
        /// Also stop at the first MIDI Stop after a Start or Continue: when Reaper stops.
        #[arg(long)]
        until_stop: bool,
        /// Also capture the Art-Net received on UDP 6454, such as EMU's output, to this frame
        /// dump on the same timeline as the MIDI, on the 25 ms frame grid (docs/migration.md).
        /// Keeps only the universes --show outputs on.
        #[arg(long, value_name = "DUMP", requires = "show")]
        artnet: Option<PathBuf>,
        /// The show the Art-Net is captured for: its pixel grids' universes are kept as
        /// Art-Net, any other universe but the Enttec one is ignored.
        #[arg(long, value_name = "SHOW_DIR", requires = "artnet")]
        show: Option<PathBuf>,
        /// The Art-Net universe that stands for the Enttec box in the dump: the 15-bit
        /// port-address. Defaults to 0, unless a pixel grid uses 0: then the grid keeps it.
        #[arg(
            long,
            value_parser = clap::value_parser!(u16).range(..=i64::from(artnet::MAX_UNIVERSE)),
            requires = "artnet",
        )]
        artnet_universe: Option<u16>,
    },
    /// Loop a setlist of recorded songs against its show for hours, then report dropped
    /// frames, drift, frame-time jitter and memory. Exits with status 1 if a check fails.
    Soak {
        /// YAML file: `show: <folder>` and `songs: [<file.mid>, …]`, relative to the file.
        setlist: PathBuf,
        #[arg(long, default_value_t = 4.0)]
        hours: f64,
        /// Write the report to this file. Default: a new
        /// soak-<setlist>-<mode>-<unix time>-<process id>.txt in the current folder.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Simulate on a fake clock as fast as the machine renders, with no outputs: 100 hours
        /// of show in minutes.
        #[arg(long, conflicts_with_all = ["emulated_enttec", "dmx_port", "listen", "artnet"])]
        accelerated: bool,
        /// Send DMX through the real driver to an emulated Enttec box on a pseudo terminal,
        /// and count the frames it receives.
        #[arg(long, conflicts_with = "dmx_port")]
        emulated_enttec: bool,
        #[command(flatten)]
        outputs: Outputs,
    },
    /// List MIDI input ports and serial ports.
    Devices,
    /// Set a look by fixture or group name. Prints it as a one-frame dump, or with --dmx-port,
    /// --listen or --artnet holds it live until stopped, reloading the show with --edit.
    Fire {
        show_dir: PathBuf,
        /// A fixture or group, then its settings: `--set all-wash color=deep-blue dimmer=60%`.
        /// Keys: color (palette name or #rrggbb), dimmer (percentage), rgbw (saturated or
        /// white-rich). Repeatable; later sets win.
        #[arg(long, required = true, num_args = 2.., value_names = ["TARGET", "KEY=VALUE"])]
        set: Vec<String>,
        #[command(flatten)]
        outputs: Outputs,
    },
    /// Manage fixture profiles.
    #[command(subcommand)]
    Fixture(FixtureCommand),
    /// Keep `blimey run` going under launchd (macOS): started at login, restarted within a
    /// second whenever it exits.
    #[command(subcommand)]
    Service(ServiceCommand),
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install the launch agent for `blimey run <RUN_ARGS>` and start it; replaces a previous
    /// install. Relative paths are taken from the current folder.
    Install {
        /// Print the launch agent's plist instead of installing it.
        #[arg(long)]
        dry_run: bool,
        /// The arguments of `blimey run`, e.g. `shows/fossile --dmx-port /dev/cu.usbserial-EN123456`.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        run_args: Vec<String>,
    },
    /// Stop the launch agent and remove it.
    Uninstall,
}

/// Where a live engine sends its frames, and whether the web UI may edit.
#[derive(Args)]
struct Outputs {
    /// Serial device of the Enttec DMX USB Pro, e.g. /dev/cu.usbserial-EN123456
    /// (see `blimey devices`).
    #[arg(long)]
    dmx_port: Option<String>,
    /// Serve the web UI and the WebSocket stream at http://<LISTEN>/ (see docs/api.md).
    /// Port 0 picks a free port.
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Edit mode: reload the show whenever one of its files changes, and let the web UI save
    /// show files and test fixtures. Without it the engine runs in show mode: the show is read
    /// once at start and the UI is read-only.
    #[arg(long)]
    edit: bool,
    #[command(flatten)]
    artnet: ArtNetMirror,
}

impl Outputs {
    fn any(&self) -> bool {
        self.dmx_port.is_some() || self.listen.is_some() || self.artnet.artnet.is_some()
    }

    fn mode(&self) -> Mode {
        if self.edit { Mode::Edit } else { Mode::Show }
    }

    /// The show the render loop plays: hot reloaded in edit mode, fixed in show mode.
    fn shows(&self, show_dir: &Path) -> io::Result<Box<dyn ShowSource>> {
        Ok(match self.mode() {
            Mode::Edit => Box::new(HotReload::load(show_dir)?),
            Mode::Show => Box::new(Show::load(show_dir)?),
        })
    }

    fn start(&self, show_dir: &Path, rig: &Rig) -> io::Result<LiveOutput> {
        let mode = self.mode();
        Ok(LiveOutput {
            dmx: self.dmx_port.clone().map(EnttecPro::new),
            stream: self
                .listen
                .map(|address| StreamSink::start(address, show_dir, rig, mode))
                .transpose()?,
            artnet: self.artnet.start(rig)?,
            artnet_frames: ArtNetFrames::new(),
            artnet_error: None,
        })
    }
}

#[derive(Args)]
struct ArtNetMirror {
    /// Mirror the output over Art-Net to this node: an IPv4 address, port 6454 unless given.
    /// `--artnet` alone sends to 127.0.0.1, e.g. for BlenderDMX (docs/blenderdmx.md).
    #[arg(
        long,
        value_name = "ADDR",
        num_args = 0..=1,
        default_missing_value = "127.0.0.1",
        value_parser = artnet::parse_node,
    )]
    artnet: Option<SocketAddrV4>,
    /// Art-Net universe the mirror sends on: the 15-bit port-address, from 0. Defaults to 0,
    /// unless a pixel grid uses 0: then the grid keeps it and the output isn't mirrored.
    #[arg(
        long,
        value_parser = clap::value_parser!(u16).range(..=i64::from(artnet::MAX_UNIVERSE)),
        requires = "artnet",
    )]
    artnet_universe: Option<u16>,
}

impl ArtNetMirror {
    /// Opens the Art-Net output, which also carries `rig`'s pixel grids on Art-Net universes.
    // ponytail: checked against the rig at start only; a hot reload that adds grids isn't rechecked.
    fn start(&self, rig: &Rig) -> io::Result<Option<ArtNet>> {
        let grid_universes = &rig.artnet_universes;
        let Some(node) = self.artnet else {
            if !grid_universes.is_empty() {
                eprintln!(
                    "blimey: pixel grids use Art-Net universes {grid_universes:?} but --artnet is not set: they get no output"
                );
            }
            return Ok(None);
        };
        let mirror = self
            .mirror_universe(grid_universes)
            .map_err(io::Error::other)?;
        if mirror.is_none() {
            eprintln!(
                "blimey: not mirroring the output: a pixel grid uses Art-Net universe {}; pass --artnet-universe to mirror on another",
                artnet::DEFAULT_ENTTEC_UNIVERSE
            );
        }
        ArtNet::new(node, mirror).map(Some)
    }

    /// Where the output is mirrored, given the universes the grids use.
    fn mirror_universe(&self, grid_universes: &[u16]) -> Result<Option<u16>, String> {
        artnet::enttec_universe(self.artnet_universe, grid_universes)
    }
}

#[derive(Subcommand)]
enum FixtureCommand {
    /// Convert an Open Fixture Library JSON file into a Blimey profile (YAML).
    Import {
        ofl_json: PathBuf,
        /// Write the profile to this file, typically <show>/fixtures/<name>.yaml.
        /// Without it the profile goes to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let result = match Command::parse() {
        Command::Replay {
            show_dir,
            midi,
            out,
            compare,
        } => replay(&show_dir, &midi, out.as_deref(), compare.as_deref()),
        Command::Validate { show_dir } => validate(&show_dir),
        Command::Run {
            show_dir,
            midi_port,
            outputs,
            state_dir,
        } => run(&show_dir, &midi_port, &outputs, state_dir),
        Command::Record {
            out,
            midi_port,
            until_stop,
            artnet,
            show,
            artnet_universe,
        } => record(
            &out,
            &midi_port,
            until_stop,
            artnet.as_deref().zip(show.as_deref()),
            artnet_universe,
        ),
        Command::Soak {
            setlist,
            hours,
            report,
            accelerated,
            emulated_enttec,
            outputs,
        } => soak(
            &setlist,
            hours,
            report,
            accelerated,
            emulated_enttec,
            &outputs,
        ),
        Command::Devices => devices(),
        Command::Fire {
            show_dir,
            set,
            outputs,
        } => fire(&show_dir, &set, &outputs),
        Command::Fixture(FixtureCommand::Import { ofl_json, out }) => {
            import_fixture(&ofl_json, out.as_deref())
        }
        Command::Service(ServiceCommand::Install { dry_run, run_args }) => {
            install_service(&run_args, dry_run)
        }
        Command::Service(ServiceCommand::Uninstall) => service::uninstall()
            .map(|()| ExitCode::SUCCESS)
            .map_err(Into::into),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}

fn replay(
    show_dir: &Path,
    midi: &Path,
    out: Option<&Path>,
    compare: Option<&Path>,
) -> Result<ExitCode, Box<dyn Error>> {
    let show = Show::load(show_dir)?;
    let frames = blimey_engine::replay(&show, MidiFile::open(midi)?);
    let dump = format_dump(&frames);
    match (out, compare) {
        (Some(path), _) => {
            fs::write(path, &dump).map_err(|error| format!("{}: {error}", path.display()))?
        }
        (None, None) => print!("{dump}"),
        (None, Some(_)) => {}
    }
    if let Some(reference) = compare {
        let in_reference = |error: String| format!("{}: {error}", reference.display());
        let text =
            fs::read_to_string(reference).map_err(|error| in_reference(error.to_string()))?;
        let expected = parse_dump(&text).map_err(in_reference)?;
        if let Some(difference) = first_difference(&frames, &expected) {
            eprintln!("replay differs from {}: {difference}", reference.display());
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn validate(show_dir: &Path) -> Result<ExitCode, Box<dyn Error>> {
    let show = Show::load(show_dir)?;
    println!(
        "{}: ok, {} fixtures, {} songs",
        show_dir.display(),
        show.rig.fixtures.len(),
        show.songs.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn run(
    show_dir: &Path,
    midi_port: &str,
    outputs: &Outputs,
    state_dir: Option<PathBuf>,
) -> Result<ExitCode, Box<dyn Error>> {
    let mut shows = check_run(show_dir, outputs)?;
    let state_dir = state_dir.or_else(|| {
        state::default_dir()
            .map_err(|error| eprintln!("state: {error}; running without saved state or event log"))
            .ok()
    });
    let recorder = Recorder::start(state_dir.as_deref(), show_dir, outputs.mode().name());
    let resume = recorder.saved().resume_events(shows.current());
    let mut clock = MonotonicClock::default();
    let midi = MidiInput::connect(midi_port, clock)?;
    let output = outputs.start(show_dir, &shows.current().rig)?;
    let mut midi = StreamedMidi::new(Resumed::new(resume, midi), output.stream.as_ref());
    let shows = StreamedShow::new(shows, output.stream.as_ref());
    eprintln!(
        "blimey: running {} in {} mode; Ctrl-C to stop",
        show_dir.display(),
        outputs.mode().name()
    );
    // u32::MAX frames at 40 Hz is over three years: in practice, until the process is stopped.
    blimey_engine::run(
        &mut clock,
        &mut midi,
        shows,
        &Look::default(),
        &mut Recorded { output, recorder },
        u32::MAX,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// `artnet` is the dump to capture Art-Net to, and the show it is captured for.
fn record(
    out: &Path,
    midi_port: &str,
    until_stop: bool,
    artnet: Option<(&Path, &Path)>,
    artnet_universe: Option<u16>,
) -> Result<ExitCode, Box<dyn Error>> {
    let artnet = artnet
        .map(|(dump, show_dir)| {
            let grid_universes = Show::load(show_dir)?.rig.artnet_universes;
            let enttec_universe = artnet::enttec_universe(artnet_universe, &grid_universes)?;
            Ok::<_, Box<dyn Error>>(record::ArtNetRecording {
                dump,
                enttec_universe,
                grid_universes,
            })
        })
        .transpose()?;
    record::record(
        out,
        &record::Recording {
            midi_port,
            until_stop,
            artnet,
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

fn soak(
    setlist: &Path,
    hours: f64,
    report: Option<PathBuf>,
    accelerated: bool,
    emulated_enttec: bool,
    outputs: &Outputs,
) -> Result<ExitCode, Box<dyn Error>> {
    let setlist = Setlist::load(setlist)?;
    let mut shows = outputs.shows(&setlist.show_dir)?;
    let device = emulated_enttec.then(EmulatedEnttec::plug_in).transpose()?;
    let mut output = outputs.start(&setlist.show_dir, &shows.current().rig)?;
    if let Some(device) = &device {
        output.dmx = Some(EnttecPro::new(device.path()));
    }
    let soak = Soak {
        setlist: &setlist,
        hours,
        accelerated,
        output,
        device: device.as_ref(),
        report,
    };
    let passed = if accelerated {
        soak.run(&mut FakeClock::default(), shows)?
    } else {
        soak.run(&mut MonotonicClock::default(), shows)?
    };
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// What `run` refuses to start without: somewhere to send frames, and a show that loads.
/// Returns the show to play.
fn check_run(show_dir: &Path, outputs: &Outputs) -> Result<Box<dyn ShowSource>, Box<dyn Error>> {
    if !outputs.any() {
        return Err("run needs somewhere to send frames: --dmx-port, --listen or --artnet".into());
    }
    Ok(outputs.shows(show_dir)?)
}

/// Checks `run_args` as `blimey run` would at start, so launchd never restarts an engine that
/// exits at once, then installs the launch agent for them.
fn install_service(run_args: &[String], dry_run: bool) -> Result<ExitCode, Box<dyn Error>> {
    let command = ["blimey", "run"]
        .into_iter()
        .chain(run_args.iter().map(String::as_str));
    let parsed = Command::try_parse_from(command).unwrap_or_else(|error| error.exit());
    let Command::Run {
        show_dir,
        outputs,
        state_dir,
        ..
    } = parsed
    else {
        unreachable!("parsed as `run`");
    };
    check_run(&show_dir, &outputs)?;
    let state_dir = std::path::absolute(state_dir.map_or_else(state::default_dir, Ok)?)?;
    let plist = service::plist(
        &std::env::current_exe()?,
        run_args,
        &std::env::current_dir()?,
        &state_dir.join("engine.log"),
    );
    if dry_run {
        print!("{plist}");
    } else {
        // launchd opens the log there before the engine could create the folder.
        fs::create_dir_all(&state_dir)?;
        service::install(&plist)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn devices() -> Result<ExitCode, Box<dyn Error>> {
    println!("MIDI inputs:");
    for name in midi_input::port_names()? {
        println!("  {name}");
    }
    println!("Serial ports:");
    for port in serialport::available_ports()? {
        match port.port_type {
            SerialPortType::UsbPort(usb) => println!(
                "  {}  USB {:04x}:{:04x} {} {}",
                port.port_name,
                usb.vid,
                usb.pid,
                usb.product.unwrap_or_default(),
                usb.serial_number.unwrap_or_default()
            ),
            _ => println!("  {}", port.port_name),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn fire(
    show_dir: &Path,
    set_values: &[String],
    outputs: &Outputs,
) -> Result<ExitCode, Box<dyn Error>> {
    let show = Show::load(show_dir)?;
    let mut look = Look::default();
    // clap merges every --set into one list; each value without `=` starts a new target.
    for set in set_values.chunk_by(|_, value| value.contains('=')) {
        let (target, pairs) = set.split_first().expect("chunks are never empty");
        if pairs.is_empty() {
            return Err(format!(
                "--set {target}: give at least one key=value, e.g. --set {target} color=red"
            )
            .into());
        }
        let settings =
            LookSettings::from_pairs(pairs).map_err(|error| format!("--set {target}: {error}"))?;
        look.set(&show, target, &settings)
            .map_err(|error| format!("--set {target}: {error}"))?;
    }
    if !outputs.any() {
        let mut sink = FakeDmx::default();
        blimey_engine::run(
            &mut FakeClock::default(),
            &mut NoMidi,
            &show,
            &look,
            &mut sink,
            1,
        )?;
        print!("{}", format_dump(&sink.frames));
        return Ok(ExitCode::SUCCESS);
    }
    let mut output = outputs.start(show_dir, &show.rig)?;
    let shows = StreamedShow::new(outputs.shows(show_dir)?, output.stream.as_ref());
    eprintln!("blimey: holding the look; Ctrl-C to stop");
    blimey_engine::run(
        &mut MonotonicClock::default(),
        &mut StreamedMidi::new(NoMidi, output.stream.as_ref()),
        shows,
        &look,
        &mut output,
        u32::MAX,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// `fire` listens to no MIDI port; with `--listen`, MIDI posted to the API still plays.
struct NoMidi;

impl MidiSource for NoMidi {
    fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
        None
    }
}

/// A live engine's outputs: any of the Enttec box, the WebSocket stream and web UI, and the
/// Art-Net mirror.
struct LiveOutput {
    dmx: Option<EnttecPro>,
    stream: Option<StreamSink>,
    artnet: Option<ArtNet>,
    /// The Art-Net universes of the next frame, held until `send` so the UI's overlays,
    /// a pixel scene preview among them, draw on them too.
    artnet_frames: ArtNetFrames,
    /// The last Art-Net send error, returned by the next `send_artnet` for a wrapping sink to
    /// count.
    // ponytail: reported a frame late, and the last frame's never; `DmxSink::send(&Universes)`
    // would report it on time, a cross-crate change.
    artnet_error: Option<io::Error>,
}

impl DmxSink for LiveOutput {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        let mut out = Universes {
            enttec: *frame,
            artnet: std::mem::take(&mut self.artnet_frames),
        };
        if let Some(stream) = &self.stream {
            stream.apply_overlays(at, &mut out);
        }
        if let Some(stream) = &mut self.stream {
            stream.send_artnet(&out.artnet)?;
        }
        if let Some(artnet) = &mut self.artnet {
            self.artnet_error = artnet.send_artnet(&out.artnet).err();
        }
        if let Some(dmx) = &mut self.dmx {
            dmx.send(at, &out.enttec)?;
        }
        if let Some(stream) = &mut self.stream {
            stream.send(at, &out.enttec)?;
        }
        if let Some(artnet) = &mut self.artnet {
            artnet.send(at, &out.enttec)?;
        }
        Ok(())
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) -> io::Result<()> {
        self.artnet_frames = universes.clone();
        self.artnet_error.take().map_or(Ok(()), Err)
    }

    fn status(&mut self, status: &Status) {
        if let Some(stream) = &mut self.stream {
            stream.status(status);
        }
    }
}

/// `run`'s output: the live outputs, plus the event log and the state to resume after a
/// restart.
struct Recorded {
    output: LiveOutput,
    recorder: Recorder,
}

impl DmxSink for Recorded {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        self.output.send(at, frame)
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) -> io::Result<()> {
        self.output.send_artnet(universes)
    }

    fn status(&mut self, status: &Status) {
        self.recorder.status(status);
        self.output.status(status);
    }

    fn midi(&mut self, event: &MidiEvent) {
        self.recorder.midi(event);
    }

    fn cue(&mut self, event: &CueEvent) {
        self.recorder.cue(event);
    }
}

fn import_fixture(ofl_json: &Path, out: Option<&Path>) -> Result<ExitCode, Box<dyn Error>> {
    let in_file = |error: String| format!("{}: {error}", ofl_json.display());
    let json = fs::read_to_string(ofl_json).map_err(|error| in_file(error.to_string()))?;
    let profile = import_ofl(&json).map_err(in_file)?;
    let source_name = ofl_json.file_name().unwrap_or_default().to_string_lossy();
    let yaml = format!(
        "# Generated by `blimey fixture import {source_name}`. Fix the OFL file and re-import rather than editing by hand.\n{}",
        serde_yaml_ng::to_string(&profile)?
    );
    match out {
        Some(path) => {
            fs::write(path, yaml).map_err(|error| format!("{}: {error}", path.display()))?
        }
        None => print!("{yaml}"),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn record_takes_art_net_universes_up_to_15_bits_like_run() {
        let record = |universe: &str| {
            Command::try_parse_from([
                "blimey",
                "record",
                "take.mid",
                "--artnet",
                "emu.dump",
                "--show",
                "shows/fossile",
                "--artnet-universe",
                universe,
            ])
        };

        assert!(record("32767").is_ok());
        assert!(record("32768").is_err());
        assert!(
            Command::try_parse_from(["blimey", "record", "take.mid", "--artnet", "emu.dump"])
                .is_err(),
            "the capture needs the show whose universes it keeps"
        );
    }

    const WALL_ON_ART_NET_3: &str = "fixtures:\n  - { name: wall, type: pixel-grid, columns: 1, rows: 1, colour_order: RGB, output: artnet, universe: 3, address: 1, canvas: { x: 0.5, y: 0.5 } }\n";

    #[test]
    fn the_live_stream_previews_grids_on_art_net() {
        let rig = Rig::parse(WALL_ON_ART_NET_3, Path::new(".")).unwrap();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let stream = StreamSink::start(address, &std::env::temp_dir(), &rig, Mode::Show).unwrap();
        let frames = stream.frames();
        let mut output = LiveOutput {
            dmx: None,
            stream: Some(stream),
            artnet: None,
            artnet_frames: ArtNetFrames::new(),
            artnet_error: None,
        };
        let mut universe = [0; 512];
        universe[0] = 255;

        output
            .send_artnet(&ArtNetFrames::from([(3, universe)]))
            .unwrap();
        output.send(Duration::ZERO, &[0; 512]).unwrap();

        let frame: serde_json::Value = serde_json::from_str(&frames.borrow()).unwrap();
        assert_eq!(frame["pixels"]["wall"], serde_json::json!(["#ff0000"]));
    }

    #[test]
    fn grids_keep_universe_0_unless_the_mirror_is_put_there_on_purpose() {
        let mirror = |universe: Option<u16>| ArtNetMirror {
            artnet: Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, artnet::PORT)),
            artnet_universe: universe,
        };

        assert_eq!(mirror(None).mirror_universe(&[]), Ok(Some(0)));
        assert_eq!(mirror(None).mirror_universe(&[0, 1]), Ok(None));
        assert_eq!(mirror(Some(5)).mirror_universe(&[0, 1]), Ok(Some(5)));
        assert!(mirror(Some(1)).mirror_universe(&[0, 1]).is_err());
    }
}
