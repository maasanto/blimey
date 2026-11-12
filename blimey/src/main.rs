mod api;
mod artnet;
mod enttec;
mod hot_reload;
mod midi_input;
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
    DmxSink, FakeClock, FakeDmx, Frame, MidiEvent, MidiFile, MidiSource, MonotonicClock, Show,
    ShowSource, Status, first_difference, format_dump, parse_dump,
};
use clap::{Args, Parser, Subcommand};
use serialport::SerialPortType;

use crate::api::Mode;
use crate::artnet::ArtNet;
use crate::enttec::EnttecPro;
use crate::hot_reload::HotReload;
use crate::midi_input::MidiInput;
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
    /// the web UI or Art-Net. Saving a show file reloads it; a file that fails to load leaves
    /// the running show as is.
    Run {
        show_dir: PathBuf,
        /// MIDI input to listen on: the first port whose name contains this text.
        #[arg(long, default_value = "IAC")]
        midi_port: String,
        #[command(flatten)]
        outputs: Outputs,
    },
    /// List MIDI input ports and serial ports.
    Devices,
    /// Set a look by fixture or group name. Prints it as a one-frame dump, or with --dmx-port,
    /// --listen or --artnet holds it live, reloading the show like `run`, until stopped.
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
    /// Let the web UI save show files and test fixtures. Without it the engine runs in show
    /// mode: the UI is read-only.
    #[arg(long, requires = "listen")]
    edit: bool,
    #[command(flatten)]
    artnet: ArtNetMirror,
}

impl Outputs {
    fn any(&self) -> bool {
        self.dmx_port.is_some() || self.listen.is_some() || self.artnet.artnet.is_some()
    }

    fn start(&self, show_dir: &Path, rig: &Rig) -> io::Result<LiveOutput> {
        let mode = if self.edit { Mode::Edit } else { Mode::Show };
        Ok(LiveOutput {
            dmx: self.dmx_port.clone().map(EnttecPro::new),
            stream: self
                .listen
                .map(|address| StreamSink::start(address, show_dir, rig, mode))
                .transpose()?,
            artnet: self.artnet.start()?,
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
    /// Art-Net universe the mirror sends on: the 15-bit port-address, from 0.
    #[arg(
        long,
        default_value_t = 0,
        value_parser = clap::value_parser!(u16).range(..=i64::from(artnet::MAX_UNIVERSE)),
        requires = "artnet",
    )]
    artnet_universe: u16,
}

impl ArtNetMirror {
    fn start(&self) -> io::Result<Option<ArtNet>> {
        self.artnet
            .map(|node| ArtNet::new(node, self.artnet_universe))
            .transpose()
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
        } => run(&show_dir, &midi_port, &outputs),
        Command::Devices => devices(),
        Command::Fire {
            show_dir,
            set,
            outputs,
        } => fire(&show_dir, &set, &outputs),
        Command::Fixture(FixtureCommand::Import { ofl_json, out }) => {
            import_fixture(&ofl_json, out.as_deref())
        }
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

fn run(show_dir: &Path, midi_port: &str, outputs: &Outputs) -> Result<ExitCode, Box<dyn Error>> {
    if !outputs.any() {
        return Err("run needs somewhere to send frames: --dmx-port, --listen or --artnet".into());
    }
    let mut shows = HotReload::load(show_dir)?;
    let mut clock = MonotonicClock::default();
    let midi = MidiInput::connect(midi_port, clock)?;
    let mut output = outputs.start(show_dir, &shows.current().rig)?;
    let mut midi = StreamedMidi::new(midi, output.stream.as_ref());
    eprintln!("blimey: running {}; Ctrl-C to stop", show_dir.display());
    // u32::MAX frames at 40 Hz is over three years: in practice, until the process is stopped.
    blimey_engine::run(
        &mut clock,
        &mut midi,
        StreamedShow::new(shows, output.stream.as_ref()),
        &Look::default(),
        &mut output,
        u32::MAX,
    )?;
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
    let shows = StreamedShow::new(HotReload::load(show_dir)?, output.stream.as_ref());
    eprintln!("blimey: holding the look; Ctrl-C to stop");
    blimey_engine::run(
        &mut MonotonicClock::default(),
        &mut NoMidi,
        shows,
        &look,
        &mut output,
        u32::MAX,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// `fire` listens to no MIDI: the look alone drives the output.
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
}

impl DmxSink for LiveOutput {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        let mut frame = *frame;
        if let Some(stream) = &self.stream {
            stream.apply_test_flash(at, &mut frame);
        }
        if let Some(dmx) = &mut self.dmx {
            dmx.send(at, &frame)?;
        }
        if let Some(stream) = &mut self.stream {
            stream.send(at, &frame)?;
        }
        if let Some(artnet) = &mut self.artnet {
            artnet.send(at, &frame)?;
        }
        Ok(())
    }

    fn status(&mut self, status: &Status) {
        if let Some(stream) = &mut self.stream {
            stream.status(status);
        }
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
