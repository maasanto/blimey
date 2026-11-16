//! The one write path for show files (plan §14). Content arrives as JSON, is checked against
//! the file's JSON Schema and by the engine's own loader, put in canonical YAML form, then
//! written atomically.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde_json::{Map, Value, json};

use crate::Show;
use crate::rig::{self, Rig};
use crate::show::{PROFILES_DIR, RIG_FILE, SONGS_DIR};

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

    /// Turns schema-valid content into canonical YAML, refusing what the loader would refuse with
    /// a message that names the file at fault.
    fn canonical(self, show_dir: &Path, content: Value) -> Result<String, String> {
        match self {
            Writable::Rig => {
                let in_rig = |error: String| format!("{RIG_FILE}: {error}");
                let yaml = rig::canonical_yaml(content).map_err(in_rig)?;
                let rig = Rig::parse(&yaml, &show_dir.join(PROFILES_DIR)).map_err(in_rig)?;
                load_as_written(show_dir, Some(rig), None)?;
                Ok(yaml)
            }
            Writable::Song(name) => canonical_song(show_dir, name, content),
        }
    }
}

fn canonical_song(show_dir: &Path, name: &str, content: Value) -> Result<String, String> {
    let schema: Value = serde_json::from_str(SONG_SCHEMA).expect("schema/ files are JSON");
    let yaml = to_yaml(&song_in_schema_order(content, &schema));
    load_as_written(show_dir, None, Some((name, &yaml)))?;
    Ok(yaml)
}

/// Loads the show as it would be after the write, the same checks as at load: a rig the songs
/// no longer load against is refused, and a broken song on disk can be fixed through a write.
fn load_as_written(
    show_dir: &Path,
    rig: Option<Rig>,
    song: Option<(&str, &str)>,
) -> Result<(), String> {
    Show::load_as_written(show_dir, rig, song)
        .map(drop)
        .map_err(|error| {
            let show_folder = show_dir.join("").display().to_string();
            error.to_string().replace(&show_folder, "")
        })
}

/// Case-insensitive file systems, such as macOS's, would write `Demo-Song.yaml` over `demo-song.yaml`.
fn refuse_case_twin(show_dir: &Path, file: &str, name: &str) -> io::Result<()> {
    let twin = fs::read_dir(show_dir.join(SONGS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            Some(file_name.strip_suffix(".yaml")?.to_owned())
        })
        .find(|other| other != name && other.eq_ignore_ascii_case(name));
    match twin {
        Some(twin) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{file}: the show already has song {twin:?}; song names must differ by more than case"
            ),
        )),
        None => Ok(()),
    }
}

/// Keys in the order the schema lists them, each cue's by its type's definition, each section's
/// and block's by theirs, and cues by note number.
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
                let mut cue = in_order(cue, &schema["$defs"][&cue_type]["properties"]);
                match cue_type.as_str() {
                    "pixel" => (note, pixel_layers_in_order(cue, schema)),
                    "pattern" => {
                        hits_in_order(&mut cue, schema);
                        (note, cue)
                    }
                    _ => (note, cue),
                }
            })
            .collect();
    }
    if let Some(Value::Object(sections)) = song.get_mut("sections") {
        for section in sections.values_mut() {
            *section = in_order(section.take(), &schema["$defs"]["section"]["properties"]);
            if let Some(Value::Array(blocks)) = section.get_mut("blocks") {
                for block in blocks {
                    *block = in_order(block.take(), &schema["$defs"]["block"]["properties"]);
                }
            }
        }
    }
    if let Some(audio) = song.get_mut("audio") {
        *audio = in_order(audio.take(), &schema["properties"]["audio"]["properties"]);
    }
    if let Some(Value::Array(markers)) = song.get_mut("markers") {
        for marker in markers {
            let marker_properties = &schema["properties"]["markers"]["items"]["properties"];
            *marker = in_order(marker.take(), marker_properties);
        }
    }
    in_order(Value::Object(song), &schema["properties"])
}

/// A pixel cue's `params`, its `layer_2` and each of its `layers`, with their `params`, `area`,
/// `swell` and `pattern`, in schema order too, so a file doesn't depend on the order the builder
/// set them in.
fn pixel_layers_in_order(mut cue: Value, schema: &Value) -> Value {
    let params_in_order = |layer: &mut Value| {
        if let Some(params) = layer.get_mut("params") {
            *params = in_order(params.take(), &schema["$defs"]["pixelParams"]["properties"]);
        }
    };
    params_in_order(&mut cue);
    if let Some(layer_2) = cue.get_mut("layer_2") {
        let layer_2_properties = &schema["$defs"]["pixel"]["properties"]["layer_2"]["properties"];
        *layer_2 = in_order(layer_2.take(), layer_2_properties);
        params_in_order(layer_2);
    }
    if let Some(Value::Array(layers)) = cue.get_mut("layers") {
        let layer_properties = &schema["$defs"]["pixelLayer"]["properties"];
        for layer in layers {
            *layer = in_order(layer.take(), layer_properties);
            for key in ["area", "swell", "pattern"] {
                if let Some(value) = layer.get_mut(key) {
                    *value = in_order(value.take(), &layer_properties[key]["properties"]);
                }
            }
            if let Some(pattern) = layer.get_mut("pattern") {
                hits_in_order(pattern, schema);
            }
            params_in_order(layer);
        }
    }
    cue
}

/// Each of a pattern's `hits`, a pattern cue's or a pixel layer's, in schema order.
fn hits_in_order(pattern: &mut Value, schema: &Value) {
    let hit_properties = &schema["$defs"]["pattern"]["properties"]["hits"]["items"]["properties"];
    if let Some(Value::Array(hits)) = pattern.get_mut("hits") {
        for hit in hits {
            *hit = in_order(hit.take(), hit_properties);
        }
    }
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
        after: Some(after),
    })
}

/// Creates `songs/<name>.yaml` from `content`, through the same checks and canonical write as
/// `write`. Refused with `AlreadyExists` if the show has a song of that name, in any case, so a
/// new song never replaces one.
pub fn create_song(show_dir: &Path, name: &str, content: Value) -> io::Result<Save> {
    let file = format!("{SONGS_DIR}/{name}.yaml");
    if writable(&file).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("song name {name:?} must be letters, digits, - and _"),
        ));
    }
    let _writing = writing();
    // Before the existence check, which a case-insensitive file system would answer for the twin.
    refuse_case_twin(show_dir, &file, name)?;
    if existing(&show_dir.join(&file))?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{file}: the show already has song {name:?}; pick another name"),
        ));
    }
    write_locked(show_dir, &file, content)
}

/// `content` as the canonical YAML to write, or why it is refused.
fn checked(show_dir: &Path, file: &str, content: Value) -> io::Result<String> {
    let writable = writable(file)?;
    if let Writable::Song(name) = writable {
        refuse_case_twin(show_dir, file, name)?;
    }
    let refused = |error: String| io::Error::new(io::ErrorKind::InvalidData, error);
    let schema = serde_json::from_str(writable.schema()).expect("schema/ files are JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema/ files are JSON Schemas");
    let errors: Vec<String> = validator
        .iter_errors(&content)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    if !errors.is_empty() {
        return Err(refused(format!("{file}: {}", errors.join("; "))));
    }
    writable.canonical(show_dir, content).map_err(refused)
}

/// Saves `cue` on `note` in `songs/<song>.yaml`, as a new cue or, when editing one, in place of
/// the cue `replaces` names: its note and the cue as the editor loaded it. That cue may move to
/// another note.
///
/// Refused, leaving the file untouched: a note another cue already uses (`AlreadyExists`, two
/// cues can't share a note); a replaced cue that changed since it was loaded (`ResourceBusy`) or
/// is gone (`NotFound`), so a stale editor never overwrites someone else's change.
pub fn save_cue(
    show_dir: &Path,
    song: &str,
    note: u8,
    replaces: Option<(u8, Value)>,
    cue: Value,
) -> io::Result<Save> {
    edit_cues(show_dir, song, |file, cues| {
        if let Some((replaced, loaded)) = &replaces {
            match cues.get(&replaced.to_string()) {
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("{file}: no cue on note {replaced} any more; reload the song"),
                    ));
                }
                Some(current) if current != loaded => {
                    return Err(io::Error::new(
                        io::ErrorKind::ResourceBusy,
                        format!(
                            "{file}: cue {replaced} changed since it was loaded; reload the song and edit it again"
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
        let replaces = replaces.as_ref().map(|(replaced, _)| *replaced);
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

/// Sets the tempo of `songs/<song>.yaml` without a MIDI clock, leaving the rest of it as it is.
pub fn set_bpm(show_dir: &Path, song: &str, bpm: serde_json::Number) -> io::Result<Save> {
    edit_song(show_dir, song, |_, song| {
        song.insert("bpm".into(), bpm.into());
        Ok(())
    })
}

/// Sets the `sections` and `arrangement` of `songs/<song>.yaml`, leaving the rest of it as it
/// is. An empty one is left out of the file.
pub fn set_arrangement(
    show_dir: &Path,
    song: &str,
    sections: Value,
    arrangement: Value,
) -> io::Result<Save> {
    edit_song(show_dir, song, |_, song| {
        for (key, value) in [("sections", sections), ("arrangement", arrangement)] {
            let is_empty = value.as_object().is_some_and(Map::is_empty)
                || value.as_array().is_some_and(Vec::is_empty);
            if is_empty {
                song.remove(key);
            } else {
                song.insert(key.into(), value);
            }
        }
        Ok(())
    })
}

/// Sets where bar 1 falls in the audio of `songs/<song>.yaml`, leaving the rest of it as it is.
/// Refused (`InvalidData`) for a song without `audio`.
pub fn set_audio_offset(show_dir: &Path, song: &str, offset: String) -> io::Result<Save> {
    edit_song(show_dir, song, |file, song| {
        let audio = song
            .get_mut("audio")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{file}: no audio to set the offset of; name its file under audio: first"
                    ),
                )
            })?;
        audio.insert("offset".into(), offset.into());
        Ok(())
    })
}

/// Names `file` as the audio of `songs/<song>.yaml`, keeping its offset.
pub fn set_audio_file(show_dir: &Path, song: &str, file: &str) -> io::Result<Save> {
    edit_song(show_dir, song, |name, song| {
        let audio = song
            .entry("audio")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{name}: audio must be a mapping"),
                )
            })?;
        audio.insert("file".into(), file.into());
        Ok(())
    })
}

/// Sets the `markers` of `songs/<song>.yaml`, leaving the rest of it as it is. No markers are
/// left out of the file.
pub fn set_markers(show_dir: &Path, song: &str, markers: Vec<Value>) -> io::Result<Save> {
    edit_song(show_dir, song, |_, song| {
        if markers.is_empty() {
            song.remove("markers");
        } else {
            song.insert("markers".into(), markers.into());
        }
        Ok(())
    })
}

fn edit_cues(
    show_dir: &Path,
    song: &str,
    edit: impl FnOnce(&str, &mut Map<String, Value>) -> io::Result<()>,
) -> io::Result<Save> {
    edit_song(show_dir, song, |file, song| {
        let cues = song
            .entry("cues")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{file}: cues must be a mapping"),
                )
            })?;
        edit(file, cues)
    })
}

/// Reads, edits and writes a song under one lock, so no other write lands in between.
fn edit_song(
    show_dir: &Path,
    song: &str,
    edit: impl FnOnce(&str, &mut Map<String, Value>) -> io::Result<()>,
) -> io::Result<Save> {
    let file = format!("{SONGS_DIR}/{song}.yaml");
    let _writing = writing();
    let mut content = read(show_dir, &file)?;
    let song = content.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{file}: a song must be a mapping"),
        )
    })?;
    edit(&file, song)?;
    write_locked(show_dir, &file, content)
}

/// A change to one show file: its text before and after, `None` where there was no file.
#[derive(Debug, Clone)]
pub struct Save {
    file: String,
    before: Option<String>,
    after: Option<String>,
}

impl Save {
    /// Puts back what the file held before this save, byte for byte, after the same checks as a
    /// write; removes it if the save created it. Returns the change it made, whose undo redoes
    /// the save. Refused with `ResourceBusy` if the file has changed since, as this would lose
    /// that change, or with `InvalidData` if the earlier content no longer passes the checks.
    pub fn undo(&self, show_dir: &Path) -> io::Result<Save> {
        let _writing = writing();
        let path = show_dir.join(&self.file);
        if existing(&path)? != self.after {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!(
                    "{}: changed since the save; this step would lose that change",
                    self.file
                ),
            ));
        }
        match &self.before {
            None => fs::remove_file(&path).map_err(|error| in_path(&path, error))?,
            Some(before) => {
                let content = serde_yaml_ng::from_str(before).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: {error}", self.file),
                    )
                })?;
                checked(show_dir, &self.file, content)?;
                write_atomically(&path, before).map_err(|error| in_path(&path, error))?;
            }
        }
        Ok(Save {
            file: self.file.clone(),
            before: self.after.clone(),
            after: self.before.clone(),
        })
    }
}

/// How many saves `History` keeps to undo.
pub const HISTORY_LENGTH: usize = 50;

/// The saves made through the API since the engine started, to undo one at a time, newest
/// first, and redo what was undone. Kept in memory only: a restart clears it. A new save clears
/// what could be redone.
#[derive(Debug, Default)]
pub struct History {
    /// Oldest first, at most `HISTORY_LENGTH`.
    undo: VecDeque<Save>,
    /// The changes undo made, oldest first; undoing one redoes its save.
    redo: VecDeque<Save>,
}

impl History {
    pub fn record(&mut self, save: Save) {
        self.redo.clear();
        push_bounded(&mut self.undo, save);
    }

    /// Undoes the latest save and returns its file.
    pub fn undo(&mut self, show_dir: &Path) -> io::Result<String> {
        step(&mut self.undo, &mut self.redo, show_dir, "undo")
    }

    /// Redoes the latest undo and returns its file.
    pub fn redo(&mut self, show_dir: &Path) -> io::Result<String> {
        step(&mut self.redo, &mut self.undo, show_dir, "redo")
    }

    /// How many undos and redos are available.
    pub fn steps(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

/// Undoes the latest change in `from`, and keeps the change that made in `to`, so it can be
/// taken back in turn. A step refused because the file changed since stays, to try again once
/// the file is put back; one whose earlier content no longer passes the checks never could, so
/// it is dropped rather than block every step before it.
fn step(
    from: &mut VecDeque<Save>,
    to: &mut VecDeque<Save>,
    show_dir: &Path,
    action: &str,
) -> io::Result<String> {
    let change = from
        .back()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("nothing to {action}")))?;
    match change.undo(show_dir) {
        Ok(inverse) => {
            from.pop_back();
            let file = inverse.file.clone();
            push_bounded(to, inverse);
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            from.pop_back();
            Err(io::Error::new(
                error.kind(),
                format!("{error}; that step can't be made and is dropped from the history"),
            ))
        }
        Err(error) => Err(error),
    }
}

fn push_bounded(changes: &mut VecDeque<Save>, change: Save) {
    if changes.len() == HISTORY_LENGTH {
        changes.pop_front();
    }
    changes.push_back(change);
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
pub(crate) fn write_atomically(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(content.as_ref())?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DEMO_SONG: &str = "songs/demo-song.yaml";

    /// A scratch copy of the demo show.
    fn demo_copy(test: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/shows/demo");
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
        let show = demo_copy("song-round-trip");
        let before = fs::read_to_string(show.join(DEMO_SONG)).unwrap();
        let mut content = read(&show, DEMO_SONG).unwrap();

        write(&show, DEMO_SONG, content.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(DEMO_SONG)).unwrap(),
            before,
            "tests/fixtures/shows/demo/songs/demo-song.yaml is in canonical form and keeps its header"
        );

        content["cues"]["37"]["release"] = "2 beats".into();
        write(&show, DEMO_SONG, content).unwrap();
        assert_eq!(
            changed_lines(&before, &fs::read_to_string(show.join(DEMO_SONG)).unwrap()),
            [("    release: 500ms".into(), "    release: 2 beats".into())]
        );
    }

    #[test]
    fn songs_are_written_in_schema_order_with_flat_looks_on_one_line() {
        let show = demo_copy("song-canonical");
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
    fn a_pixel_cues_params_and_layer_2_are_written_in_schema_order_whatever_order_they_came_in() {
        let show = demo_copy("pixel-order");
        let params = json!({ "speed": "1 bar", "width": 0.5, "color": "amber" });
        let cue = json!({
            "seed": 7,
            "layer_2": { "blend": "add", "params": params, "shape": "bar" },
            "params": params,
            "type": "pixel",
            "name": "two-layer",
        });

        save_cue(&show, "demo-song", 50, None, cue).unwrap();

        let written = fs::read_to_string(show.join(DEMO_SONG)).unwrap();
        assert!(
            written.ends_with(
                "  50:\n    name: two-layer\n    type: pixel\n    params: { color: amber, width: 0.5, speed: 1 bar }\n    layer_2:\n      shape: bar\n      params: { color: amber, width: 0.5, speed: 1 bar }\n      blend: add\n    seed: 7\n"
            ),
            "{written}"
        );
    }

    #[test]
    fn a_pixel_cues_layers_are_written_in_schema_order() {
        let show = demo_copy("pixel-layers-order");
        let cue = json!({
            "layers": [
                {
                    "swell": { "curve": "ease-in", "duration": "4 bars" },
                    "params": { "speed": "1 bar", "color": "amber" },
                    "shape": "fill",
                    "area": { "height": 1, "width": 0.5, "y": 0, "x": 0 },
                },
                {
                    "mute": ["inner-wash"],
                    "blend": "over",
                    "pattern": {
                        "hits": [{ "level": "100%", "length": "1 beat", "shape": "sine", "at": 1 }],
                        "length": "1 bar",
                    },
                    "level": "60%",
                },
            ],
            "type": "pixel",
            "name": "layered",
        });

        save_cue(&show, "demo-song", 50, None, cue).unwrap();

        let written = fs::read_to_string(show.join(DEMO_SONG)).unwrap();
        assert!(
            written.ends_with(
                "  50:\n    name: layered\n    type: pixel\n    layers:\n      -\n        area: { x: 0, y: 0, width: 0.5, height: 1 }\n        shape: fill\n        params: { color: amber, speed: 1 bar }\n        swell: { duration: 4 bars, curve: ease-in }\n      -\n        level: 60%\n        pattern:\n          length: 1 bar\n          hits:\n            - { at: 1, shape: sine, length: 1 beat, level: 100% }\n        blend: over\n        mute: [inner-wash]\n"
            ),
            "{written}"
        );
    }

    #[test]
    fn a_cue_on_a_taken_note_is_refused_and_a_renumbered_one_moves() {
        let show = demo_copy("cue-notes");
        let before = fs::read_to_string(show.join(DEMO_SONG)).unwrap();
        let blackout = read(&show, DEMO_SONG).unwrap()["cues"]["38"].clone();

        let error = save_cue(&show, "demo-song", 36, None, blackout.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            error.to_string(),
            "songs/demo-song.yaml: note 36 already fires cue \"verse-blue\"; pick a free note, or edit that cue"
        );
        let from_38 = Some((38, blackout.clone()));
        let error =
            save_cue(&show, "demo-song", 36, from_38.clone(), blackout.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(show.join(DEMO_SONG)).unwrap(), before);

        save_cue(&show, "demo-song", 48, from_38, blackout).unwrap();
        let cues = read(&show, DEMO_SONG).unwrap()["cues"].clone();
        let notes: Vec<&String> = cues.as_object().unwrap().keys().collect();
        assert_eq!(notes, ["36", "37", "39", "40", "42", "48"]);
        assert_eq!(cues["48"]["name"], "blackout");

        delete_cue(&show, "demo-song", 48).unwrap();
        assert_eq!(
            delete_cue(&show, "demo-song", 48).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn undo_puts_back_the_previous_file_byte_for_byte() {
        let show = demo_copy("undo");
        let hand_written = "# By hand.\nprogram: 3\n\ncues:\n  36: { name: verse, type: scene, values: { all-wash: { color: amber } } }  # keep\n";
        fs::write(show.join(DEMO_SONG), hand_written).unwrap();
        let cue = json!({ "name": "new", "type": "scene", "values": {} });

        let save = save_cue(&show, "demo-song", 40, None, cue.clone()).unwrap();
        assert_ne!(
            fs::read_to_string(show.join(DEMO_SONG)).unwrap(),
            hand_written
        );
        assert_eq!(save.file, DEMO_SONG);
        save.undo(&show).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(DEMO_SONG)).unwrap(),
            hand_written
        );

        let save = save_cue(&show, "demo-song", 40, None, cue).unwrap();
        fs::write(
            show.join(DEMO_SONG),
            hand_written.replace("amber", "magenta"),
        )
        .unwrap();
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
        let show = demo_copy("song-program");

        let error = write(&show, "songs/another.yaml", json!({ "program": 3 })).unwrap_err();

        assert_eq!(
            error.to_string(),
            "songs/demo-song.yaml: program 3 already selects song \"another\""
        );
    }

    fn text(show: &Path, file: &str) -> Option<String> {
        fs::read_to_string(show.join(file)).ok()
    }

    #[test]
    fn the_history_undoes_and_redoes_saves_across_files_newest_first() {
        let show = demo_copy("history");
        let (song_0, rig_0) = (text(&show, DEMO_SONG), text(&show, RIG_FILE));
        let mut history = History::default();
        let cue = json!({ "name": "new", "type": "scene", "values": {} });

        history.record(create_song(&show, "encore", json!({ "program": 9 })).unwrap());
        let encore = text(&show, "songs/encore.yaml");
        history.record(save_cue(&show, "demo-song", 50, None, cue).unwrap());
        let song_1 = text(&show, DEMO_SONG);
        let mut rig = read(&show, RIG_FILE).unwrap();
        rig["fixtures"][0]["canvas"]["x"] = 0.4.into();
        history.record(write(&show, RIG_FILE, rig).unwrap());
        assert_eq!(history.steps(), (3, 0));

        assert_eq!(history.undo(&show).unwrap(), RIG_FILE);
        assert_eq!(text(&show, RIG_FILE), rig_0);
        assert_eq!(history.undo(&show).unwrap(), DEMO_SONG);
        assert_eq!(text(&show, DEMO_SONG), song_0);
        assert_eq!(history.undo(&show).unwrap(), "songs/encore.yaml");
        assert_eq!(text(&show, "songs/encore.yaml"), None);
        assert_eq!(history.steps(), (0, 3));
        assert_eq!(
            history.undo(&show).unwrap_err().to_string(),
            "nothing to undo"
        );

        assert_eq!(history.redo(&show).unwrap(), "songs/encore.yaml");
        assert_eq!(text(&show, "songs/encore.yaml"), encore);
        assert_eq!(history.redo(&show).unwrap(), DEMO_SONG);
        assert_eq!(text(&show, DEMO_SONG), song_1);
        assert_eq!(history.steps(), (2, 1));

        history.record(set_bpm(&show, "demo-song", 100.into()).unwrap());
        assert_eq!(
            history.steps(),
            (3, 0),
            "a new save drops what could be redone"
        );
    }

    #[test]
    fn a_refused_undo_stays_in_the_history() {
        let show = demo_copy("history-refused");
        let before = text(&show, DEMO_SONG).unwrap();
        let mut history = History::default();
        history.record(set_bpm(&show, "demo-song", 100.into()).unwrap());
        let saved = text(&show, DEMO_SONG).unwrap();

        fs::write(show.join(DEMO_SONG), format!("{saved}# by hand\n")).unwrap();
        let error = history.undo(&show).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert_eq!(history.steps(), (1, 0));

        fs::write(show.join(DEMO_SONG), saved).unwrap();
        history.undo(&show).unwrap();
        assert_eq!(text(&show, DEMO_SONG).unwrap(), before);
    }

    #[test]
    fn an_undo_back_to_content_that_no_longer_loads_is_dropped() {
        let show = demo_copy("history-broken");
        let fixed = read(&show, DEMO_SONG).unwrap();
        let broken = text(&show, DEMO_SONG).unwrap().replace("deep-blue", "teal");
        fs::write(show.join(DEMO_SONG), &broken).unwrap();
        let mut history = History::default();
        history.record(write(&show, DEMO_SONG, fixed).unwrap());
        let saved = text(&show, DEMO_SONG);

        let error = history.undo(&show).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .ends_with("that step can't be made and is dropped from the history"),
            "{error}"
        );
        assert_eq!(history.steps(), (0, 0));
        assert_eq!(text(&show, DEMO_SONG), saved, "the fixed song stays");
    }

    #[test]
    fn the_history_keeps_the_latest_saves_only() {
        let show = demo_copy("history-bound");
        let mut history = History::default();

        for bpm in 1..=HISTORY_LENGTH + 2 {
            history.record(set_bpm(&show, "demo-song", bpm.into()).unwrap());
        }
        for _ in 0..HISTORY_LENGTH {
            history.undo(&show).unwrap();
        }

        assert_eq!(history.steps(), (0, HISTORY_LENGTH));
        assert_eq!(
            read(&show, DEMO_SONG).unwrap()["bpm"],
            2,
            "the oldest two saves were dropped"
        );
    }

    #[test]
    fn a_new_song_is_refused_on_a_taken_name_or_program_and_never_replaces_one() {
        let show = demo_copy("create-song");
        let before = text(&show, DEMO_SONG);
        let refusal = |name: &str, program: u8| {
            let error = create_song(&show, name, json!({ "program": program })).unwrap_err();
            (error.kind(), error.to_string())
        };

        assert_eq!(
            refusal("demo-song", 9),
            (
                io::ErrorKind::AlreadyExists,
                "songs/demo-song.yaml: the show already has song \"demo-song\"; pick another name"
                    .into()
            )
        );
        assert_eq!(refusal("Demo-Song", 9).0, io::ErrorKind::AlreadyExists);
        assert_eq!(
            refusal("encore", 3),
            (
                io::ErrorKind::InvalidData,
                "songs/encore.yaml: program 3 already selects song \"demo-song\"".into()
            )
        );
        assert_eq!(
            refusal("en core", 9),
            (
                io::ErrorKind::InvalidData,
                "song name \"en core\" must be letters, digits, - and _".into()
            )
        );
        assert_eq!(text(&show, DEMO_SONG), before);
        assert_eq!(text(&show, "songs/encore.yaml"), None);

        create_song(
            &show,
            "encore",
            json!({ "program": 9, "bpm": 100, "time_signature": "6/8" }),
        )
        .unwrap();
        assert_eq!(
            text(&show, "songs/encore.yaml").unwrap(),
            "program: 9\nbpm: 100\ntime_signature: 6/8\n"
        );
    }

    #[test]
    fn a_pattern_hit_with_a_custom_curve_round_trips_byte_for_byte() {
        let show = demo_copy("custom-hit-round-trip");
        let song = "program: 9\n\
                    cues:\n  \
                      42:\n    \
                        name: pulse\n    \
                        type: pattern\n    \
                        length: 1 bar\n    \
                        target: { all-wash: dimmer }\n    \
                        hits:\n      \
                          -\n        \
                            at: 1\n        \
                            shape:\n          \
                              points:\n            \
                                - [0, 0]\n            \
                                - [0.5, 1]\n        \
                            length: 1 beat\n        \
                            level: 100%\n      \
                          - { at: 3, shape: sine, length: 1 beat, level: 50% }\n";
        fs::write(show.join("songs/pattern.yaml"), song).unwrap();

        write(
            &show,
            "songs/pattern.yaml",
            read(&show, "songs/pattern.yaml").unwrap(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(show.join("songs/pattern.yaml")).unwrap(),
            song
        );
    }

    #[test]
    fn a_two_layer_pixel_cue_round_trips_byte_for_byte() {
        let show = demo_copy("pixel-round-trip");
        let song = "program: 9\n\
                    cues:\n  \
                      41:\n    \
                        name: split-sparkle\n    \
                        type: pixel\n    \
                        fade_in: 1 beat\n    \
                        release: 2s\n    \
                        shape: split\n    \
                        effect: move\n    \
                        params: { color: amber, background: \"#102030\", width: 0.5, speed: 2 bars, direction: right-to-left, phase: 0.25 }\n    \
                        layer_2:\n      \
                          shape: one-by-one\n      \
                          effect: sparkle\n      \
                          params: { color: white, width: 0.1 }\n      \
                          blend: add\n    \
                        rgbw: white-rich\n    \
                        seed: 7\n";
        fs::write(show.join("songs/pixel.yaml"), song).unwrap();

        write(
            &show,
            "songs/pixel.yaml",
            read(&show, "songs/pixel.yaml").unwrap(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(show.join("songs/pixel.yaml")).unwrap(),
            song
        );
    }

    #[test]
    fn a_broken_song_on_disk_can_be_fixed_through_the_write_path() {
        let show = demo_copy("fix-broken");
        let fixed = read(&show, DEMO_SONG).unwrap();
        let broken = fs::read_to_string(show.join(DEMO_SONG))
            .unwrap()
            .replace("deep-blue", "teal");
        fs::write(show.join(DEMO_SONG), broken).unwrap();

        write(&show, DEMO_SONG, fixed).unwrap();

        Show::load(&show).unwrap();
    }

    #[test]
    fn a_song_name_differing_only_by_case_is_refused() {
        let show = demo_copy("song-case");
        let before = fs::read_to_string(show.join(DEMO_SONG)).unwrap();

        let error = write(&show, "songs/Demo-Song.yaml", json!({ "program": 9 })).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            error.to_string(),
            "songs/Demo-Song.yaml: the show already has song \"demo-song\"; song names must differ by more than case"
        );
        assert_eq!(fs::read_to_string(show.join(DEMO_SONG)).unwrap(), before);
        assert!(
            !fs::read_dir(show.join(SONGS_DIR))
                .unwrap()
                .any(|entry| entry.unwrap().file_name() == "Demo-Song.yaml")
        );
    }

    #[test]
    fn replacing_a_cue_that_changed_or_went_since_it_was_loaded_is_refused() {
        let show = demo_copy("stale-replaces");
        let loaded = read(&show, DEMO_SONG).unwrap()["cues"]["37"].clone();
        let mut edited = loaded.clone();
        edited["name"] = "edited".into();
        let mut changed_since = loaded.clone();
        changed_since["release"] = "2s".into();
        save_cue(
            &show,
            "demo-song",
            37,
            Some((37, loaded.clone())),
            changed_since,
        )
        .unwrap();
        let error = save_cue(
            &show,
            "demo-song",
            37,
            Some((37, loaded.clone())),
            edited.clone(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert_eq!(
            error.to_string(),
            "songs/demo-song.yaml: cue 37 changed since it was loaded; reload the song and edit it again"
        );

        delete_cue(&show, "demo-song", 37).unwrap();
        let error = save_cue(&show, "demo-song", 40, Some((37, loaded)), edited).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            error.to_string(),
            "songs/demo-song.yaml: no cue on note 37 any more; reload the song"
        );
    }

    #[test]
    fn setting_the_bpm_changes_only_its_line() {
        let show = demo_copy("set-bpm");
        let before = fs::read_to_string(show.join(DEMO_SONG)).unwrap();

        set_bpm(
            &show,
            "demo-song",
            serde_json::Number::from_f64(120.5).unwrap(),
        )
        .unwrap();

        assert_eq!(
            changed_lines(&before, &fs::read_to_string(show.join(DEMO_SONG)).unwrap()),
            [("bpm: 92".into(), "bpm: 120.5".into())]
        );
    }

    #[test]
    fn the_audio_offset_changes_only_its_line_and_needs_audio() {
        let show = demo_copy("set-audio-offset");
        let error = set_audio_offset(&show, "demo-song", "1s".into()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "songs/demo-song.yaml: no audio to set the offset of; name its file under audio: first"
        );

        let path = show.join(DEMO_SONG);
        let with_audio = fs::read_to_string(&path).unwrap().replace(
            "bpm: 92\n",
            "bpm: 92\naudio: { file: audio/demo-song.wav }\n",
        );
        fs::write(&path, &with_audio).unwrap();
        set_audio_offset(&show, "demo-song", "1.25s".into()).unwrap();
        assert_eq!(
            changed_lines(&with_audio, &fs::read_to_string(&path).unwrap()),
            [(
                "audio: { file: audio/demo-song.wav }".into(),
                "audio: { file: audio/demo-song.wav, offset: 1.25s }".into()
            )]
        );

        let refused = set_audio_offset(&show, "demo-song", "2 bars".into()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
        assert!(fs::read_to_string(&path).unwrap().contains("offset: 1.25s"));
    }

    #[test]
    fn markers_round_trip_undo_and_a_refused_set_leaves_the_file_untouched() {
        let show = demo_copy("set-markers");
        let path = show.join(DEMO_SONG);
        let before = fs::read_to_string(&path).unwrap();
        let markers = vec![
            json!({ "at": "12.5s", "name": "Verse" }),
            json!({ "name": "Chorus 2", "at": "61.25s" }),
        ];
        let save = set_markers(&show, "demo-song", markers.clone()).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.ends_with(
            "markers:\n  - { name: Verse, at: 12.5s }\n  - { name: Chorus 2, at: 61.25s }\n"
        ));
        let content = read(&show, DEMO_SONG).unwrap();
        assert_eq!(content["markers"][1], markers[1]);

        for refused in [
            json!({ "name": "Verse", "at": "-1s" }),
            json!({ "name": "", "at": "1s" }),
            json!({ "name": "Verse", "at": "2 bars" }),
        ] {
            let error = set_markers(&show, "demo-song", vec![refused.clone()]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{refused}");
            assert_eq!(fs::read_to_string(&path).unwrap(), after, "{refused}");
        }

        save.undo(&show).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn the_audio_file_is_named_keeping_its_offset() {
        let show = demo_copy("set-audio-file");
        let path = show.join(DEMO_SONG);
        set_audio_file(&show, "demo-song", "audio/demo-song.wav").unwrap();
        set_audio_offset(&show, "demo-song", "1.5s".into()).unwrap();
        set_audio_file(&show, "demo-song", "audio/demo-song.flac").unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\naudio: { file: audio/demo-song.flac, offset: 1.5s }\n")
        );
    }

    #[test]
    fn keys_read_back_as_names_except_note_numbers() {
        use serde_yaml_ng::Value as Yaml;
        let names = ["007", "true", "1e3", "0x1F", "-1", "+5"];
        let mut keys = json!({ "36": 0, "0": 0 });
        for name in names {
            keys[name] = 0.into();
        }

        let yaml = to_yaml(&keys);

        // How the loader sees them, as a look's targets are read.
        let read: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&yaml).unwrap();
        let read_keys: Vec<&Yaml> = read.keys().collect();
        let mut expected = vec![Yaml::from(36), Yaml::from(0)];
        expected.extend(names.map(Yaml::from));
        assert_eq!(read_keys, expected.iter().collect::<Vec<_>>(), "{yaml}");
    }

    #[test]
    fn moving_a_fixture_changes_only_its_canvas_line() {
        let show = demo_copy("move");
        let before = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let mut content = read(&show, RIG_FILE).unwrap();

        write(&show, RIG_FILE, content.clone()).unwrap();
        assert_eq!(
            fs::read_to_string(show.join(RIG_FILE)).unwrap(),
            before,
            "tests/fixtures/shows/demo/rig.yaml is in canonical form and keeps its header"
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
        let show = demo_copy("refuse");
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
        let show = demo_copy("songs");
        let before = fs::read_to_string(show.join(RIG_FILE)).unwrap();
        let mut content = read(&show, RIG_FILE).unwrap();
        content["groups"]
            .as_object_mut()
            .unwrap()
            .remove("outer-wash");

        let error = write(&show, RIG_FILE, content).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .starts_with("songs/demo-song.yaml: line 15: cue 37"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(show.join(RIG_FILE)).unwrap(), before);
        assert!(Show::load(&show).is_ok(), "the show still starts");
    }

    #[test]
    fn an_unreadable_file_is_reported_rather_than_losing_its_header() {
        let show = demo_copy("unreadable");
        let content = read(&show, RIG_FILE).unwrap();
        let mut before = b"# Notes \xff on the rig\n".to_vec();
        before.extend(fs::read(show.join(RIG_FILE)).unwrap());
        fs::write(show.join(RIG_FILE), &before).unwrap();

        let error = write(&show, RIG_FILE, content).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(show.join(RIG_FILE)).unwrap(), before);
    }

    #[test]
    fn only_known_show_files_are_served() {
        let show = demo_copy("unknown");

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
