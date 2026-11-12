mod enttec;
mod midi_input;
mod stream;

use std::error::Error;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serialport::SerialPortType;
use strata_engine::look::{Look, LookSettings};
use strata_engine::ofl::import_ofl;
use strata_engine::{
    DmxSink, FakeClock, FakeDmx, Frame, MidiEvent, MidiFile, MidiSource, MonotonicClock, Show,
    first_difference, format_dump, parse_dump,
};

use crate::enttec::EnttecPro;
use crate::midi_input::MidiInput;
use crate::stream::StreamSink;

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
    /// Run a show live: MIDI in from a port such as the IAC bus, DMX out to an Enttec DMX USB Pro.
    Run {
        show_dir: PathBuf,
        /// MIDI input to listen on: the first port whose name contains this text.
        #[arg(long, default_value = "IAC")]
        midi_port: String,
        /// Serial device of the Enttec box, e.g. /dev/cu.usbserial-EN123456 (see `strata devices`).
        #[arg(long)]
        dmx_port: String,
    },
    /// List MIDI input ports and serial ports.
    Devices,
    /// Set a look by fixture or group name. Prints it as a one-frame dump, or with --dmx-port
    /// or --listen holds it live until stopped.
    Fire {
        show_dir: PathBuf,
        /// A fixture or group, then its settings: `--set all-wash color=deep-blue dimmer=60%`.
        /// Keys: color (palette name or #rrggbb), dimmer (percentage), rgbw (saturated or
        /// white-rich). Repeatable; later sets win.
        #[arg(long, required = true, num_args = 2.., value_names = ["TARGET", "KEY=VALUE"])]
        set: Vec<String>,
        /// Output to this Enttec DMX USB Pro, as `strata run` does.
        #[arg(long)]
        dmx_port: Option<String>,
        /// Stream engine state over WebSocket at ws://<LISTEN>/ws (see docs/api.md).
        /// Port 0 picks a free port.
        #[arg(long)]
        listen: Option<SocketAddr>,
    },
    /// Manage fixture profiles.
    #[command(subcommand)]
    Fixture(FixtureCommand),
}

#[derive(Subcommand)]
enum FixtureCommand {
    /// Convert an Open Fixture Library JSON file into a Strata profile (YAML).
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
        Command::Run {
            show_dir,
            midi_port,
            dmx_port,
        } => run(&show_dir, &midi_port, dmx_port),
        Command::Devices => devices(),
        Command::Fire {
            show_dir,
            set,
            dmx_port,
            listen,
        } => fire(&show_dir, &set, dmx_port, listen),
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
    let frames = strata_engine::replay(&show, MidiFile::open(midi)?);
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

fn run(show_dir: &Path, midi_port: &str, dmx_port: String) -> Result<ExitCode, Box<dyn Error>> {
    let show = Show::load(show_dir)?;
    let mut clock = MonotonicClock::default();
    let mut midi = MidiInput::connect(midi_port, clock)?;
    let mut dmx = EnttecPro::new(dmx_port);
    eprintln!("strata: running {}; Ctrl-C to stop", show_dir.display());
    // u32::MAX frames at 40 Hz is over three years: in practice, until the process is stopped.
    strata_engine::run(
        &mut clock,
        &mut midi,
        &show,
        &Look::default(),
        &mut dmx,
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
    dmx_port: Option<String>,
    listen: Option<SocketAddr>,
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
    if dmx_port.is_none() && listen.is_none() {
        let mut sink = FakeDmx::default();
        strata_engine::run(
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
    let mut output = LiveOutput {
        dmx: dmx_port.map(EnttecPro::new),
        stream: listen
            .map(|address| StreamSink::start(address, &show.rig))
            .transpose()?,
    };
    eprintln!("strata: holding the look; Ctrl-C to stop");
    strata_engine::run(
        &mut MonotonicClock::default(),
        &mut NoMidi,
        &show,
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

/// A live engine's outputs: the Enttec box, the WebSocket stream, or both.
struct LiveOutput {
    dmx: Option<EnttecPro>,
    stream: Option<StreamSink>,
}

impl DmxSink for LiveOutput {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        if let Some(dmx) = &mut self.dmx {
            dmx.send(at, frame)?;
        }
        if let Some(stream) = &mut self.stream {
            stream.send(at, frame)?;
        }
        Ok(())
    }
}

fn import_fixture(ofl_json: &Path, out: Option<&Path>) -> Result<ExitCode, Box<dyn Error>> {
    let in_file = |error: String| format!("{}: {error}", ofl_json.display());
    let json = fs::read_to_string(ofl_json).map_err(|error| in_file(error.to_string()))?;
    let profile = import_ofl(&json).map_err(in_file)?;
    let source_name = ofl_json.file_name().unwrap_or_default().to_string_lossy();
    let yaml = format!(
        "# Generated by `strata fixture import {source_name}`. Fix the OFL file and re-import rather than editing by hand.\n{}",
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
