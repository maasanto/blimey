//! The one write path for show files (plan §14). Content arrives as JSON, is checked against
//! the file's JSON Schema and by the engine's own loader, put in canonical YAML form, then
//! written atomically.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde_json::{Map, Value, json};

use crate::Show;
use crate::rig::{self, Rig};
use crate::show::{PROFILES_DIR, RIG_FILE, SONGS_DIR};
use crate::song::Song;

const RIG_SCHEMA: &str = include_str!("../../schema/rig.schema.json");
const SONG_SCHEMA: &str = include_str!("../../schema/song.schema.json");

/// Serialises writes, so two of them never share the temporary file and a cue edit's read and
/// write see no other write in between.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> MutexGuard<'static, ()> {
    WRITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A show file the API reads and writes.
#[derive(Clone, Copy)]
enum Writable<'file> {
    Rig,
    /// `songs/<name>.yaml`, by name.
    Song(&'file str),
}

fn writable(file: &str) -> io::Result<Writable<'_>> {
    if file == RIG_FILE {
        return Ok(Writable::Rig);
    }
    let song = file
        .strip_prefix(SONGS_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_suffix(".yaml"))
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
    song.map(Writable::Song).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{file:?} is not a show file the API reads or writes; it can: {RIG_FILE}, \
                 {SONGS_DIR}/<name>.yaml with a name of letters, digits, - and _"
            ),
        )
    })
}

impl Writable<'_> {
    fn schema(self) -> &'static str {
        match self {
            Writable::Rig => RIG_SCHEMA,
            Writable::Song(_) => SONG_SCHEMA,
        }
    }

    /// Turns schema-valid content into canonical YAML, refusing what the loader would refuse.
    fn canonical(self, show_dir: &Path, content: Value) -> Result<String, String> {
        match self {
            Writable::Rig => {
                let yaml = rig::canonical_yaml(content)?;
                Rig::parse(&yaml, &show_dir.join(PROFILES_DIR))?;
                Ok(yaml)
            }
            Writable::Song(name) => canonical_song(show_dir, name, content),
        }
    }
}

/// Checks a song against the rest of the show as it is on disk, the same checks as at load.
fn canonical_song(show_dir: &Path, name: &str, content: Value) -> Result<String, String> {
    let schema: Value = serde_json::from_str(SONG_SCHEMA).expect("schema/ files are JSON");
    let yaml = to_yaml(&song_in_schema_order(content, &schema));
    let show = Show::load(show_dir).map_err(|error| error.to_string())?;
    let song = Song::parse(name, &yaml, &show)?;
    if let Some(other) = show
        .songs
        .iter()
        .find(|other| other.program == song.program && other.name != name)
    {
        return Err(format!(
            "program {} already selects song {:?}",
            song.program, other.name
        ));
    }
    Ok(yaml)
}

/// Keys in the order the schema lists them, each cue's by its type's definition, and cues by
/// note number.
fn song_in_schema_order(content: Value, schema: &Value) -> Value {
    let Value::Object(mut song) = content else {
        return content;
    };
    if let Some(Value::Object(cues)) = song.get_mut("cues") {
        let mut by_note: Vec<(String, Value)> = std::mem::take(cues).into_iter().collect();
        by_note.sort_by_key(|(note, _)| note.parse::<u8>().ok());
        *cues = by_note
            .into_iter()
            .map(|(note, cue)| {
                let cue_type = cue["type"].as_str().unwrap_or_default().to_owned();
                (
                    note,
                    in_order(cue, &schema["$defs"][cue_type]["properties"]),
                )
            })
            .collect();
    }
    in_order(Value::Object(song), &schema["properties"])
}

/// An object's keys in the order of `properties`; keys it doesn't list follow as given.
fn in_order(value: Value, properties: &Value) -> Value {
    match (value, properties.as_object()) {
        (Value::Object(object), Some(properties)) => {
            let rank = |key: &str| properties.keys().position(|listed| listed == key);
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by_key(|(key, _)| rank(key).unwrap_or(usize::MAX));
            Value::Object(entries.into_iter().collect())
        }
        (value, _) => value,
    }
}

/// Block-style YAML with two-space indents, where a mapping or sequence of plain values sits on
/// one line in flow style: `all-wash: { color: deep-blue, dimmer: 60% }`.
fn to_yaml(value: &Value) -> String {
    let mut yaml = String::new();
    write_block(&mut yaml, value, 0);
    yaml
}

fn write_block(yaml: &mut String, value: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    let entries: Vec<(String, &Value)> = match value {
        Value::Object(object) => object
            .iter()
            .map(|(key, child)| (format!("{pad}{}:", yaml_key(key)), child))
            .collect(),
        Value::Array(items) => items.iter().map(|item| (format!("{pad}-"), item)).collect(),
        _ => unreachable!("the schemas make show files mappings, and scalars are never blocks"),
    };
    for (lead, child) in entries {
        if is_flat(child) {
            yaml.push_str(&format!("{lead} {}\n", flow(child)));
        } else {
            yaml.push_str(&format!("{lead}\n"));
            write_block(yaml, child, indent + 2);
        }
    }
}

fn is_flat(value: &Value) -> bool {
    let is_scalar = |value: &Value| !(value.is_object() || value.is_array());
    match value {
        Value::Object(object) => object.values().all(is_scalar),
        Value::Array(items) => items.iter().all(is_scalar),
        _ => true,
    }
}

fn flow(value: &Value) -> String {
    match value {
        Value::Object(object) if object.is_empty() => "{}".into(),
        Value::Object(object) => {
            let entries: Vec<String> = object
                .iter()
                .map(|(key, child)| format!("{}: {}", yaml_key(key), flow(child)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(flow).collect();
            format!("[{}]", items.join(", "))
        }
        Value::String(text) => yaml_string(text),
        other => other.to_string(),
    }
}

/// Note numbers stay numbers, as song files write cue keys.
fn yaml_key(key: &str) -> String {
    if !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_digit()) {
        key.into()
    } else {
        yaml_string(key)
    }
}

/// Plain when that reads back as the same string, in block and flow context; quoted otherwise.
fn yaml_string(text: &str) -> String {
    let is_plain = !text
        .starts_with(|c: char| c.is_whitespace() || "-?:,[]{}#&*!|>'\"%@`".contains(c))
        && !text.ends_with(|c: char| c.is_whitespace() || c == ':')
        && !text.contains(|c: char| c.is_control() || ",[]{}".contains(c))
        && !text.contains(": ")
        && !text.contains(" #")
        && serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text).ok()
            == Some(serde_yaml_ng::Value::String(text.into()));
    if is_plain {
        text.into()
    } else {
        serde_json::to_string(text).expect("strings serialise")
    }
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

/// Replaces a show file with `content`, given as JSON, creating it if needed. Content that
/// fails the file's schema or the engine's checks is refused with `InvalidData`, and the file is
/// left untouched.
///
/// The file's leading comment block is kept; other comments are not.
pub fn write(show_dir: &Path, file: &str, content: Value) -> io::Result<Save> {
    let _writing = writing();
    write_locked(show_dir, file, content)
}

fn write_locked(show_dir: &Path, file: &str, content: Value) -> io::Result<Save> {
    let yaml = checked(show_dir, file, content)?;
    let path = show_dir.join(file);
    let before = existing(&path)?;
    let after = format!(
        "{}{yaml}",
        leading_comment(before.as_deref().unwrap_or_default())
    );
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|error| in_path(&path, error))?;
    }
    write_atomically(&path, &after).map_err(|error| in_path(&path, error))?;
    Ok(Save {
        file: file.into(),
        before,
        after,
    })
}

/// `content` as the canonical YAML to write, or why it is refused.
fn checked(show_dir: &Path, file: &str, content: Value) -> io::Result<String> {
    let writable = writable(file)?;
    let refused =
        |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{file}: {error}"));
    let schema = serde_json::from_str(writable.schema()).expect("schema/ files are JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema/ files are JSON Schemas");
    let errors: Vec<String> = validator
        .iter_errors(&content)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    if !errors.is_empty() {
        return Err(refused(errors.join("; ")));
    }
    writable.canonical(show_dir, content).map_err(refused)
}

/// Saves `cue` on `note` in `songs/<song>.yaml`, in place of the cue on `replaces` when editing
/// one (renumbered or not), else as a new cue. A note another cue already uses is refused with
/// `AlreadyExists`: two cues can't share a note.
pub fn save_cue(
    show_dir: &Path,
    song: &str,
    note: u8,
    replaces: Option<u8>,
    cue: Value,
) -> io::Result<Save> {
    edit_cues(show_dir, song, |file, cues| {
        if replaces != Some(note)
            && let Some(taken) = cues.get(&note.to_string())
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{file}: note {note} already fires cue {}; pick a free note, or edit that cue",
                    taken["name"]
                ),
            ));
        }
        if let Some(replaced) = replaces {
            cues.remove(&replaced.to_string());
        }
        cues.insert(note.to_string(), cue);
        Ok(())
    })
}

/// Removes the cue on `note` from `songs/<song>.yaml`.
pub fn delete_cue(show_dir: &Path, song: &str, note: u8) -> io::Result<Save> {
    edit_cues(show_dir, song, |file, cues| {
        cues.remove(&note.to_string()).map(drop).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{file}: no cue on note {note}"),
            )
        })
    })
}

fn edit_cues(
    show_dir: &Path,
    song: &str,
    edit: impl FnOnce(&str, &mut Map<String, Value>) -> io::Result<()>,
) -> io::Result<Save> {
    let file = format!("{SONGS_DIR}/{song}.yaml");
    let _writing = writing();
    let mut content = read(show_dir, &file)?;
    let cues = content
        .as_object_mut()
        .map(|song| song.entry("cues").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{file}: the song and its cues must be mappings"),
            )
        })?;
    edit(&file, cues)?;
    write_locked(show_dir, &file, content)
}

/// A write, with what the file held before it so that it can be undone.
#[derive(Debug)]
pub struct Save {
    file: String,
    /// `None` when the write created the file.
    before: Option<String>,
    after: String,
}

impl Save {
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Puts back what the file held before this save, byte for byte, after the same checks as a
    /// write; removes it if the save created it. Refused with `ResourceBusy` if the file has
    /// changed since, as undoing would lose that change.
    pub fn undo(self, show_dir: &Path) -> io::Result<()> {
        let _writing = writing();
        let path = show_dir.join(&self.file);
        if existing(&path)?.as_deref() != Some(self.after.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!(
                    "{}: changed since the save; undoing it would lose that change",
                    self.file
                ),
            ));
        }
        let Some(before) = self.before else {
            return fs::remove_file(&path).map_err(|error| in_path(&path, error));
        };
        let content = serde_yaml_ng::from_str(&before).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", self.file),
            )
        })?;
        checked(show_dir, &self.file, content)?;
        write_atomically(&path, &before).map_err(|error| in_path(&path, error))
    }
}

/// The file's text, or `None` if there is no such file.
fn existing(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(in_path(path, error)),
    }
}

fn in_path(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
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

    const TELL_ME: &str = "songs/tell-me.yaml";

    /// A scratch copy of the Fossile show.
    fn fossile_copy(test: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../shows/fossile");
        let copy = std::env::temp_dir().join(format!("blimey-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&copy);
        for folder in [PROFILES_DIR, SONGS_DIR] {
            fs::create_dir_all(copy.join(folder)).unwrap();
            for file in fs::read_dir(source.join(folder)).unwrap() {
                let file = file.unwrap().path();
                fs::copy(&file, copy.join(folder).join(file.file_name().unwrap())).unwrap();
            }
        }
        for file in [RIG_FILE, "palettes.yaml"] {
            fs::copy(source.join(file), copy.join(file)).unwrap();
        }
        copy
    }

    fn changed_lines(before: &str, after: &str) -> Vec<(String, String)> {
        before
            .lines()
            .zip(after.lines())
            .filter(|(old, new)| old != new)
            .map(|(old, new)| (old.into(), new.into()))
            .collect()
    }

    #[test]
    fn editing_one_cue_leaves_the_rest_of_the_song_byte_identical() {
        let show = fossile_copy("song-round-trip");
        let before = fs::read_to_string(show.join(TELL_ME)).unwrap();
        let mut content = read(&show, TELL_ME).unwrap();

        write(&show, TELL_ME, content.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(TELL_ME)).unwrap(),
            before,
            "shows/fossile/songs/tell-me.yaml is in canonical form and keeps its header"
        );

        content["cues"]["37"]["release"] = "2 beats".into();
        write(&show, TELL_ME, content).unwrap();
        assert_eq!(
            changed_lines(&before, &fs::read_to_string(show.join(TELL_ME)).unwrap()),
            [("    release: 500ms".into(), "    release: 2 beats".into())]
        );
    }

    #[test]
    fn songs_are_written_in_schema_order_with_flat_looks_on_one_line() {
        let show = fossile_copy("song-canonical");
        let song = json!({
            "cues": {
                "40": {
                    "values": { "wash-1": { "color": "#ff8000" }, "all-wash": { "dimmer": "50%" } },
                    "type": "scene",
                    "name": "warm: front",
                },
                "9": {
                    "curve": { "points": [[0, 0], [0.5, 1]] },
                    "duration": "1 bar",
                    "target": { "all-wash": { "dimmer": "100%" } },
                    "type": "swell",
                    "name": "rise",
                },
            },
            "program": 9,
        });

        write(&show, "songs/new.yaml", song).unwrap();

        assert_eq!(
            fs::read_to_string(show.join("songs/new.yaml")).unwrap(),
            "program: 9\n\
             cues:\n  \
               9:\n    \
                 name: rise\n    \
                 type: swell\n    \
                 target:\n      \
                   all-wash: { dimmer: 100% }\n    \
                 duration: 1 bar\n    \
                 curve:\n      \
                   points:\n        \
                     - [0, 0]\n        \
                     - [0.5, 1]\n  \
               40:\n    \
                 name: \"warm: front\"\n    \
                 type: scene\n    \
                 values:\n      \
                   wash-1: { color: \"#ff8000\" }\n      \
                   all-wash: { dimmer: 50% }\n",
            "look entries keep their order: a later one wins"
        );
    }

    #[test]
    fn a_cue_on_a_taken_note_is_refused_and_a_renumbered_one_moves() {
        let show = fossile_copy("cue-notes");
        let before = fs::read_to_string(show.join(TELL_ME)).unwrap();
        let blackout = read(&show, TELL_ME).unwrap()["cues"]["38"].clone();

        let error = save_cue(&show, "tell-me", 36, None, blackout.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            error.to_string(),
            "songs/tell-me.yaml: note 36 already fires cue \"verse-blue\"; pick a free note, or edit that cue"
        );
        let error = save_cue(&show, "tell-me", 36, Some(38), blackout.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(show.join(TELL_ME)).unwrap(), before);

        save_cue(&show, "tell-me", 48, Some(38), blackout).unwrap();
        let cues = read(&show, TELL_ME).unwrap()["cues"].clone();
        let notes: Vec<&String> = cues.as_object().unwrap().keys().collect();
        assert_eq!(notes, ["36", "37", "39", "42", "48"]);
        assert_eq!(cues["48"]["name"], "blackout");

        delete_cue(&show, "tell-me", 48).unwrap();
        assert_eq!(
            delete_cue(&show, "tell-me", 48).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn undo_puts_back_the_previous_file_byte_for_byte() {
        let show = fossile_copy("undo");
        let hand_written = "# By hand.\nprogram: 3\n\ncues:\n  36: { name: verse, type: scene, values: { all-wash: { color: amber } } }  # keep\n";
        fs::write(show.join(TELL_ME), hand_written).unwrap();
        let cue = json!({ "name": "new", "type": "scene", "values": {} });

        let save = save_cue(&show, "tell-me", 40, None, cue.clone()).unwrap();
        assert_ne!(
            fs::read_to_string(show.join(TELL_ME)).unwrap(),
            hand_written
        );
        assert_eq!(save.file(), TELL_ME);
        save.undo(&show).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(TELL_ME)).unwrap(),
            hand_written
        );

        let save = save_cue(&show, "tell-me", 40, None, cue).unwrap();
        fs::write(show.join(TELL_ME), hand_written.replace("amber", "magenta")).unwrap();
        let error = save.undo(&show).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert!(
            error.to_string().contains("changed since the save"),
            "{error}"
        );

        let created = write(&show, "songs/new.yaml", json!({ "program": 9 })).unwrap();
        created.undo(&show).unwrap();
        assert!(!show.join("songs/new.yaml").exists());
    }

    #[test]
    fn a_song_sharing_another_songs_program_is_refused() {
        let show = fossile_copy("song-program");

        let error = write(&show, "songs/other.yaml", json!({ "program": 3 })).unwrap_err();

        assert_eq!(
            error.to_string(),
            "songs/other.yaml: program 3 already selects song \"tell-me\""
        );
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
        assert_eq!(
            changed_lines(&before, &fs::read_to_string(show.join(RIG_FILE)).unwrap()),
            [("    x: 0.325".into(), "    x: 0.4".into())]
        );
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
    fn only_known_show_files_are_served() {
        let show = fossile_copy("unknown");

        for file in [
            "../secrets.yaml",
            "songs/../rig.yaml",
            "songs/.yaml",
            "songs/a/b.yaml",
        ] {
            assert_eq!(
                read(&show, file).unwrap_err().kind(),
                io::ErrorKind::NotFound,
                "{file}"
            );
        }
        assert_eq!(
            write(&show, "palettes.yaml", Value::Null)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
