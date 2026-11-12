mod enttec;
mod midi_input;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use serialport::SerialPortType;
use strata_engine::{MidiFile, MonotonicClock, Show, first_difference, format_dump, parse_dump};

use crate::enttec::EnttecPro;
use crate::midi_input::MidiInput;

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
    strata_engine::run(&mut clock, &mut midi, &show, &mut dmx, u32::MAX)?;
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
