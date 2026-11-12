use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use strata_engine::{MidiFile, first_difference, format_dump, parse_dump};

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
}

fn main() -> ExitCode {
    let Command::Replay {
        show_dir,
        midi,
        out,
        compare,
    } = Command::parse();
    replay(&show_dir, &midi, out.as_deref(), compare.as_deref()).unwrap_or_else(|error| {
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
    if !show_dir.is_dir() {
        return Err(format!(
            "show folder {} does not exist or is not a directory",
            show_dir.display()
        )
        .into());
    }
    let frames = strata_engine::replay(MidiFile::open(midi)?);
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
