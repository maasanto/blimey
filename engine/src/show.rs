use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::LegacyMap;
use crate::colour::{Rgb, parse_palettes};
use crate::rig::Rig;
use crate::song::{Song, load_songs};

const LEGACY_MAP_FILE: &str = "legacy-map.yaml";
pub(crate) const RIG_FILE: &str = "rig.yaml";
const PALETTES_FILE: &str = "palettes.yaml";
pub(crate) const PROFILES_DIR: &str = "fixtures";
const SONGS_DIR: &str = "songs";

/// A show folder (plan §6), loaded and validated.
#[derive(Debug, Default)]
pub struct Show {
    /// Empty when the show has no `legacy-map.yaml`.
    pub legacy: LegacyMap,
    /// Empty when the show has no `rig.yaml`.
    pub rig: Rig,
    /// Colour name → colour; empty when the show has no `palettes.yaml`.
    pub palettes: BTreeMap<String, Rgb>,
    /// One per file in `songs/`, in file-name order.
    pub songs: Vec<Song>,
}

impl Show {
    pub fn load(dir: &Path) -> io::Result<Self> {
        if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "show folder {} does not exist or is not a directory",
                    dir.display()
                ),
            ));
        }
        let mut show = Self {
            legacy: load_optional(&dir.join(LEGACY_MAP_FILE), |yaml| {
                LegacyMap::parse(yaml).map_err(|error| error.to_string())
            })?,
            rig: load_optional(&dir.join(RIG_FILE), |yaml| {
                Rig::parse(yaml, &dir.join(PROFILES_DIR))
            })?,
            palettes: load_optional(&dir.join(PALETTES_FILE), parse_palettes)?,
            songs: Vec::new(),
        };
        show.songs = load_songs(&dir.join(SONGS_DIR), &show)?;
        Ok(show)
    }
}

/// Parses the file at `path`, or gives the empty value if there is no such file.
fn load_optional<T: Default>(
    path: &Path,
    parse: impl FnOnce(&str) -> Result<T, String>,
) -> io::Result<T> {
    let in_file =
        |kind, error: String| io::Error::new(kind, format!("{}: {error}", path.display()));
    match fs::read_to_string(path) {
        Ok(yaml) => parse(&yaml).map_err(|error| in_file(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(in_file(error.kind(), error.to_string())),
    }
}
