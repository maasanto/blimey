//! The one write path for show files (plan §14). Content arrives as JSON, is checked against
//! the file's JSON Schema and by the engine's own loader, put in canonical YAML form, then
//! written atomically.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::Show;
use crate::rig::{self, Rig};
use crate::show::{PROFILES_DIR, RIG_FILE};

/// Serialises writes, so two of them never share the temporary file.
static WRITING: Mutex<()> = Mutex::new(());

/// A show file the API reads and writes.
struct Writable {
    schema: &'static str,
    /// Turns schema-valid content into canonical YAML, refusing what the loader would refuse.
    canonical: fn(&Path, Value) -> Result<String, String>,
}

fn writable(file: &str) -> io::Result<Writable> {
    match file {
        RIG_FILE => Ok(Writable {
            schema: include_str!("../../schema/rig.schema.json"),
            canonical: canonical_rig,
        }),
        _ => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{file:?} is not a show file the API reads or writes; it can: {RIG_FILE}"),
        )),
    }
}

/// Refuses a rig the songs no longer load against, as the next start would.
fn canonical_rig(show_dir: &Path, content: Value) -> Result<String, String> {
    let yaml = rig::canonical_yaml(content)?;
    let rig = Rig::parse(&yaml, &show_dir.join(PROFILES_DIR))?;
    Show::load_with_rig(show_dir, rig).map_err(|error| error.to_string())?;
    Ok(yaml)
}

/// A show file's content, as JSON.
pub fn read(show_dir: &Path, file: &str) -> io::Result<Value> {
    writable(file)?;
    let path = show_dir.join(file);
    let in_file =
        |kind, error: String| io::Error::new(kind, format!("{}: {error}", path.display()));
    let yaml =
        fs::read_to_string(&path).map_err(|error| in_file(error.kind(), error.to_string()))?;
    serde_yaml_ng::from_str(&yaml)
        .map_err(|error| in_file(io::ErrorKind::InvalidData, error.to_string()))
}

/// Replaces a show file with `content`, given as JSON. Content that fails the file's schema or
/// the engine's checks is refused with `InvalidData`, and the file is left untouched.
///
/// The file's leading comment block is kept; other comments are not.
pub fn write(show_dir: &Path, file: &str, content: Value) -> io::Result<()> {
    let writable = writable(file)?;
    let refused =
        |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{file}: {error}"));
    let schema = serde_json::from_str(writable.schema).expect("schema/ files are JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema/ files are JSON Schemas");
    let errors: Vec<String> = validator
        .iter_errors(&content)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    if !errors.is_empty() {
        return Err(refused(errors.join("; ")));
    }
    let yaml = (writable.canonical)(show_dir, content).map_err(refused)?;

    let _writing = WRITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = show_dir.join(file);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    write_atomically(&path, &format!("{}{yaml}", leading_comment(&existing)))
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}

/// The comment lines at the top of a file, such as a note on what still needs checking.
fn leading_comment(text: &str) -> &str {
    let length: usize = text
        .split_inclusive('\n')
        .take_while(|line| line.starts_with('#'))
        .map(str::len)
        .sum();
    &text[..length]
}

/// Readers see the old file or the new one, never half of one, even if the engine dies mid-write.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("yaml.tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch copy of the Fossile show.
    fn fossile_copy(test: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../shows/fossile");
        let copy = std::env::temp_dir().join(format!("blimey-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&copy);
        fs::create_dir_all(&copy).unwrap();
        for file in [RIG_FILE, "palettes.yaml"] {
            fs::copy(source.join(file), copy.join(file)).unwrap();
        }
        for folder in [PROFILES_DIR, "songs"] {
            fs::create_dir_all(copy.join(folder)).unwrap();
            for file in fs::read_dir(source.join(folder)).unwrap() {
                let file = file.unwrap().path();
                fs::copy(&file, copy.join(folder).join(file.file_name().unwrap())).unwrap();
            }
        }
        copy
    }

    #[test]
    fn moving_a_fixture_changes_only_its_canvas_line() {
        let show = fossile_copy("move");
        let before = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let mut content = read(&show, RIG_FILE).unwrap();

        write(&show, RIG_FILE, content.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(RIG_FILE)).unwrap(),
            before,
            "shows/fossile/rig.yaml is in canonical form and keeps its header"
        );

        content["fixtures"][1]["canvas"]["x"] = 0.4.into();
        write(&show, RIG_FILE, content).unwrap();
        let after = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let changed: Vec<(&str, &str)> = before
            .lines()
            .zip(after.lines())
            .filter(|(old, new)| old != new)
            .collect();
        assert_eq!(changed, [("    x: 0.325", "    x: 0.4")]);
    }

    #[test]
    fn refused_content_is_named_and_leaves_the_file_untouched() {
        let show = fossile_copy("refuse");
        let before = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let content = read(&show, RIG_FILE).unwrap();

        let mut off_stage = content.clone();
        off_stage["fixtures"][0]["canvas"]["x"] = 1.5.into();
        let error = write(&show, RIG_FILE, off_stage).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .starts_with("rig.yaml: /fixtures/0/canvas/x: 1.5 is greater than"),
            "{error}"
        );

        let mut overlapping = content;
        overlapping["fixtures"][1]["address"] = 10.into();
        let error = write(&show, RIG_FILE, overlapping).unwrap_err();
        assert!(error.to_string().contains("overlap"), "{error}");

        assert_eq!(fs::read_to_string(show.join(RIG_FILE)).unwrap(), before);
    }

    #[test]
    fn a_rig_the_songs_no_longer_load_against_is_refused() {
        let show = fossile_copy("songs");
        let before = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let mut content = read(&show, RIG_FILE).unwrap();
        content["groups"]
            .as_object_mut()
            .unwrap()
            .remove("outer-wash");

        let error = write(&show, RIG_FILE, content).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("tell-me.yaml: line 15: cue 37"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(show.join(RIG_FILE)).unwrap(), before);
        assert!(Show::load(&show).is_ok(), "the show still starts");
    }

    #[test]
    fn only_known_show_files_are_served() {
        let show = fossile_copy("unknown");

        let error = read(&show, "../secrets.yaml").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            write(&show, "palettes.yaml", Value::Null)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
