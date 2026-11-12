use std::fs;
use std::io;
use std::path::Path;

use crate::LegacyMap;

const LEGACY_MAP_FILE: &str = "legacy-map.yaml";

/// A show folder (plan §6), loaded and validated.
#[derive(Debug, Default)]
pub struct Show {
    /// Empty when the show has no `legacy-map.yaml`.
    pub legacy: LegacyMap,
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
        let legacy_path = dir.join(LEGACY_MAP_FILE);
        let legacy = match fs::read_to_string(&legacy_path) {
            Ok(yaml) => LegacyMap::parse(&yaml),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LegacyMap::default()),
            Err(error) => Err(error),
        }
        .map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", legacy_path.display()))
        })?;
        Ok(Self { legacy })
    }
}
