//! Export files: a song, a section with the scenes it plays, or a scene, written to carry to
//! another song or show, and imported through the one write path. The format is in docs/api.md
//! and schema/export.schema.json.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::Show;
use crate::show::SONGS_DIR;
use crate::show_file::{self, Save};
use crate::song::MIDI_DATA_MAX;

const EXPORT_SCHEMA: &str = include_str!("../../schema/export.schema.json");

/// The song file, with `kind: song` and its name on top.
pub fn export_song(show_dir: &Path, song: &str) -> io::Result<String> {
    let content = show_file::read(show_dir, &song_file(song))?;
    Ok(export_file("song", Some(song), content))
}

/// The section, with the scenes its blocks play and the pixel scenes they link to.
pub fn export_section(show_dir: &Path, song: &str, section: &str) -> io::Result<String> {
    let file = song_file(song);
    let content = show_file::read(show_dir, &file)?;
    let found = content["sections"].get(section).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{file}: no section {section:?}"),
        )
    })?;
    let notes = found["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["scene"].as_u64())
        .map(|note| note.to_string())
        .collect();
    let part = json!({ "cues": with_linked(&content, &notes), "sections": { section: found } });
    Ok(export_file("section", None, part))
}

/// The scene, with the pixel scenes it links to.
pub fn export_scene(show_dir: &Path, song: &str, note: u8) -> io::Result<String> {
    let file = song_file(song);
    let content = show_file::read(show_dir, &file)?;
    if content["cues"].get(note.to_string()).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{file}: no scene {note}"),
        ));
    }
    let notes = BTreeSet::from([note.to_string()]);
    let part = json!({ "cues": with_linked(&content, &notes) });
    Ok(export_file("scene", None, part))
}

fn song_file(song: &str) -> String {
    format!("{SONGS_DIR}/{song}.yaml")
}

/// The song's cues on `notes`, and the pixel cues they drive the intensity of: the engine
/// refuses a link to a pixel cue the song doesn't have.
fn with_linked(song: &Value, notes: &BTreeSet<String>) -> Map<String, Value> {
    let cues = song["cues"].as_object().cloned().unwrap_or_default();
    let linked: BTreeSet<String> = cues
        .iter()
        .filter(|(note, _)| notes.contains(*note))
        .filter_map(|(_, cue)| cue["target"].as_object())
        .flatten()
        .filter(|(_, driven)| *driven == "intensity")
        .map(|(name, _)| name.clone())
        .collect();
    cues.into_iter()
        .filter(|(note, cue)| {
            let is_linked = cue["type"] == "pixel"
                && cue["name"]
                    .as_str()
                    .is_some_and(|name| linked.contains(name));
            notes.contains(note) || is_linked
        })
        .collect()
}

/// `content`, a song or part of one, in canonical form with `kind` and `name` on top.
fn export_file(kind: &str, name: Option<&str>, content: Value) -> String {
    let mut file = Map::from_iter([("kind".to_owned(), json!(kind))]);
    if let Some(name) = name {
        file.insert("name".into(), name.into());
    }
    if let Value::Object(content) = show_file::song_in_schema_order(content) {
        file.extend(content);
    }
    show_file::to_yaml(&Value::Object(file))
}

/// Imports an export file, YAML or JSON. A song is added to the show; a section or scene is
/// added to `into`. Nothing already in the show is overwritten: a name or number taken by
/// something else gets the next free one, and the summary returned says which, as
/// `{ "song", "program" }` for a song or `{ "song", "scenes": { old: new }, "sections": { old:
/// new } }` for a section or scene.
pub fn import(show_dir: &Path, into: Option<&str>, text: &str) -> io::Result<(Save, Value)> {
    let file: Value = serde_yaml_ng::from_str(text)
        .map_err(|error| refused(format!("the import file is not YAML or JSON: {error}")))?;
    let errors = show_file::schema_errors(EXPORT_SCHEMA, &file);
    if !errors.is_empty() {
        return Err(refused(format!(
            "not a Blimey export file: {}",
            errors.join("; ")
        )));
    }
    let Value::Object(mut file) = file else {
        unreachable!("the schema takes only a mapping")
    };
    let text_of = |value: Option<Value>| value.as_ref().and_then(Value::as_str).map(str::to_owned);
    let kind = text_of(file.remove("kind")).unwrap_or_default();
    if kind == "song" {
        let name = text_of(file.remove("name")).unwrap_or_default();
        return import_song(show_dir, &name, file);
    }
    let into =
        into.ok_or_else(|| refused(format!("a {kind} is imported into a song: name the song")))?;
    import_into(show_dir, into, file)
}

fn import_song(
    show_dir: &Path,
    name: &str,
    mut content: Map<String, Value>,
) -> io::Result<(Save, Value)> {
    let show = Show::load(show_dir)?;
    let name = free_name(name, |candidate| {
        show.songs
            .iter()
            .any(|song| song.name.eq_ignore_ascii_case(candidate))
    });
    let programs: BTreeSet<u8> = show.songs.iter().map(|song| song.program).collect();
    let program = content["program"]
        .as_u64()
        .and_then(|program| u8::try_from(program).ok())
        .filter(|program| !programs.contains(program))
        .or_else(|| free_number(|number| programs.contains(&number), 1))
        .ok_or_else(|| refused("every song number is taken: delete a song first".into()))?;
    content.insert("program".into(), program.into());
    let save = show_file::create_song(show_dir, &name, content.into())?;
    Ok((save, json!({ "song": name, "program": program })))
}

/// Adds the scenes and sections of `part` to `song`.
fn import_into(
    show_dir: &Path,
    song: &str,
    mut part: Map<String, Value>,
) -> io::Result<(Save, Value)> {
    let mut imported = take_mapping(&mut part, "cues");
    let sections = take_mapping(&mut part, "sections");
    let mut numbers = Map::new();
    let mut names = Map::new();
    let save = show_file::edit_song(show_dir, song, |file, content| {
        let cues = show_file::mapping(content, "cues", file)?;
        rename_clashing_pixel_cues(&mut imported, cues);
        numbers = place_scenes(imported, cues, file)?;
        if !sections.is_empty() {
            let existing = show_file::mapping(content, "sections", file)?;
            names = add_sections(sections, &numbers, existing);
        }
        Ok(())
    })?;
    Ok((
        save,
        json!({ "song": song, "scenes": numbers, "sections": names }),
    ))
}

/// Renames an imported pixel cue that would share its name with another pixel cue of the song,
/// and the intensity links to it: a link drives every pixel cue of its name, so the import would
/// otherwise change how the song's own scenes play.
fn rename_clashing_pixel_cues(imported: &mut Map<String, Value>, cues: &Map<String, Value>) {
    let pixel_name = |cue: &Value| {
        (cue["type"] == "pixel")
            .then(|| cue["name"].as_str())
            .flatten()
            .map(str::to_owned)
    };
    let mut taken: BTreeSet<String> = cues.values().filter_map(pixel_name).collect();
    let mut renamed = BTreeMap::new();
    for (note, cue) in imported.iter_mut() {
        let Some(name) = pixel_name(cue) else {
            continue;
        };
        if cues.get(note) == Some(cue) || !taken.contains(&name) {
            continue;
        }
        let new_name = renamed.get(&name).cloned().unwrap_or_else(|| {
            let free = free_name(&name, |candidate| taken.contains(candidate));
            taken.insert(free.clone());
            free
        });
        cue["name"] = json!(new_name);
        renamed.insert(name, new_name);
    }
    for cue in imported.values_mut() {
        if let Some(Value::Object(target)) = cue.get_mut("target") {
            *target = std::mem::take(target)
                .into_iter()
                .map(|(name, driven)| (renamed.get(&name).cloned().unwrap_or(name), driven))
                .collect();
        }
    }
}

/// Adds `imported` to `cues`: a scene keeps its number if that is free or holds the same scene,
/// and takes the next free one otherwise. Returns the number each took.
fn place_scenes(
    imported: Map<String, Value>,
    cues: &mut Map<String, Value>,
    file: &str,
) -> io::Result<Map<String, Value>> {
    let mut numbers = Map::new();
    // Scenes that fit first, so a clash moves only the scenes that clash.
    let (fit, clash): (Vec<_>, Vec<_>) = imported
        .into_iter()
        .partition(|(note, cue)| cues.get(note).is_none_or(|taken| taken == cue));
    for (note, cue) in fit {
        numbers.insert(note.clone(), json!(note.parse::<u8>().unwrap_or_default()));
        cues.insert(note, cue);
    }
    for (note, cue) in clash {
        let wanted: u8 = note.parse().unwrap_or_default();
        let is_taken = |number: u8| cues.contains_key(&number.to_string());
        let free = free_number(is_taken, wanted.saturating_add(1))
            .or_else(|| free_number(is_taken, 1))
            .ok_or_else(|| {
                refused(format!(
                    "{file}: no free number for scene {note}; delete a scene first"
                ))
            })?;
        numbers.insert(note, free.into());
        cues.insert(free.to_string(), cue);
    }
    Ok(numbers)
}

/// Adds `sections` to `existing`, their blocks following the scenes that moved; one whose name
/// the song has for a different section takes the next free name. Returns the name each took.
fn add_sections(
    sections: Map<String, Value>,
    numbers: &Map<String, Value>,
    existing: &mut Map<String, Value>,
) -> Map<String, Value> {
    let mut names = Map::new();
    for (name, mut section) in sections {
        let blocks = section.get_mut("blocks").and_then(Value::as_array_mut);
        for block in blocks.into_iter().flatten() {
            if let Some(scene) = block.get_mut("scene")
                && let Some(number) = numbers.get(&scene.to_string())
            {
                *scene = number.clone();
            }
        }
        let free = free_name(&name, |candidate| {
            existing
                .get(candidate)
                .is_some_and(|taken| *taken != section)
        });
        existing.insert(free.clone(), section);
        names.insert(name, free.into());
    }
    names
}

fn take_mapping(part: &mut Map<String, Value>, key: &str) -> Map<String, Value> {
    match part.remove(key) {
        Some(Value::Object(entries)) => entries,
        _ => Map::new(),
    }
}

/// `name`, or `name-2`, `name-3`… the first that isn't taken.
fn free_name(name: &str, is_taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|count| match count {
            1 => name.to_owned(),
            _ => format!("{name}-{count}"),
        })
        .find(|candidate| !is_taken(candidate))
        .expect("a name past every taken one")
}

/// The lowest number from `from` up to 127 that isn't taken.
fn free_number(is_taken: impl Fn(u8) -> bool, from: u8) -> Option<u8> {
    (from..=MIDI_DATA_MAX).find(|&number| !is_taken(number))
}

fn refused(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show_file::tests::demo_copy;

    const DEMO_SONG: &str = "songs/demo-song.yaml";

    /// The demo show with a `verse` section playing scene 36 and scene 41, a swell driving the
    /// intensity of pixel scene 40.
    fn arranged_demo(test: &str) -> std::path::PathBuf {
        let show = demo_copy(test);
        let link = json!({ "name": "lift", "type": "swell", "target": { "bridge-chase": "intensity" }, "duration": "1 bar" });
        show_file::save_cue(&show, "demo-song", 41, None, link).unwrap();
        let sections = json!({ "verse": { "bars": 4, "blocks": [
            { "scene": 36, "bar": 1, "length": "4 bars" },
            { "scene": 41, "bar": 3, "length": "2 bars" },
        ] } });
        show_file::set_arrangement(&show, "demo-song", sections, json!(["verse"])).unwrap();
        show
    }

    fn song(show: &Path, name: &str) -> Value {
        show_file::read(show, &format!("{SONGS_DIR}/{name}.yaml")).unwrap()
    }

    #[test]
    fn a_section_exports_with_its_scenes_and_the_pixel_scenes_they_link_to() {
        let show = arranged_demo("export-section");

        let file = export_section(&show, "demo-song", "verse").unwrap();

        let file: Value = serde_yaml_ng::from_str(&file).unwrap();
        assert_eq!(file["kind"], "section");
        let notes: Vec<&String> = file["cues"].as_object().unwrap().keys().collect();
        assert_eq!(notes, ["36", "40", "41"]);
        assert_eq!(
            file["sections"]["verse"],
            song(&show, "demo-song")["sections"]["verse"]
        );
    }

    #[test]
    fn a_section_imported_over_different_scenes_moves_them_and_renames_itself() {
        let show = arranged_demo("import-section");
        let file = export_section(&show, "demo-song", "verse").unwrap();

        // Into its own song unchanged: every scene and the section are already there.
        let (_, summary) = import(&show, Some("demo-song"), &file).unwrap();
        assert_eq!(
            summary,
            json!({ "song": "demo-song", "scenes": { "36": 36, "40": 40, "41": 41 }, "sections": { "verse": "verse" } })
        );

        show_file::save_cue(
            &show,
            "demo-song",
            36,
            Some((36, song(&show, "demo-song")["cues"]["36"].clone())),
            json!({ "name": "other", "type": "scene", "values": {} }),
        )
        .unwrap();
        let (_, summary) = import(&show, Some("demo-song"), &file).unwrap();

        assert_eq!(
            summary,
            json!({ "song": "demo-song", "scenes": { "40": 40, "41": 41, "36": 43 }, "sections": { "verse": "verse-2" } })
        );
        let after = song(&show, "demo-song");
        assert_eq!(
            after["cues"]["36"]["name"], "other",
            "nothing is overwritten"
        );
        assert_eq!(after["cues"]["43"]["name"], "verse-blue");
        assert_eq!(after["sections"]["verse-2"]["blocks"][0]["scene"], 43);
        assert_eq!(after["sections"]["verse"]["blocks"][0]["scene"], 36);
        Show::load(&show).unwrap();
    }

    #[test]
    fn an_imported_pixel_scene_named_as_a_different_one_of_the_song_is_renamed_with_its_links() {
        let show = arranged_demo("import-pixel-name");
        let file = export_section(&show, "demo-song", "verse").unwrap();
        let mut chase = song(&show, "demo-song")["cues"]["40"].clone();
        let loaded = (40, chase.clone());
        chase["seed"] = 8.into();
        show_file::save_cue(&show, "demo-song", 40, Some(loaded), chase).unwrap();

        let (_, summary) = import(&show, Some("demo-song"), &file).unwrap();

        assert_eq!(summary["scenes"], json!({ "36": 36, "40": 43, "41": 44 }));
        let after = song(&show, "demo-song");
        assert_eq!(after["cues"]["43"]["name"], "bridge-chase-2");
        assert_eq!(after["cues"]["43"]["seed"], 7);
        assert_eq!(
            after["cues"]["44"]["target"],
            json!({ "bridge-chase-2": "intensity" })
        );
        assert_eq!(
            after["cues"]["41"]["target"],
            json!({ "bridge-chase": "intensity" }),
            "the song's own link is untouched"
        );
        assert_eq!(after["sections"]["verse-2"]["blocks"][1]["scene"], 44);
    }

    #[test]
    fn a_scene_imports_into_another_song_with_its_linked_pixel_scene() {
        let show = arranged_demo("import-scene");
        show_file::create_song(&show, "encore", json!({ "program": 9 })).unwrap();

        let file = export_scene(&show, "demo-song", 41).unwrap();
        import(&show, Some("encore"), &file).unwrap();

        let encore = song(&show, "encore");
        assert_eq!(encore["cues"]["41"]["name"], "lift");
        assert_eq!(encore["cues"]["40"]["name"], "bridge-chase");
    }

    #[test]
    fn a_song_imports_under_a_free_name_and_number() {
        let show = arranged_demo("import-song");
        let file = export_song(&show, "demo-song").unwrap();
        assert!(file.starts_with("kind: song\nname: demo-song\nprogram: 3\n"));

        let (_, summary) = import(&show, None, &file).unwrap();
        assert_eq!(summary, json!({ "song": "demo-song-2", "program": 1 }));
        let (_, summary) = import(&show, None, &file).unwrap();
        assert_eq!(summary, json!({ "song": "demo-song-3", "program": 2 }));

        let mut copy = song(&show, "demo-song-3");
        copy["program"] = 3.into();
        assert_eq!(copy, song(&show, "demo-song"));
    }

    #[test]
    fn an_import_that_isnt_an_export_file_or_has_no_song_to_go_to_is_refused() {
        let show = arranged_demo("import-refused");
        let before = std::fs::read_to_string(show.join(DEMO_SONG)).unwrap();
        let scene = export_scene(&show, "demo-song", 36).unwrap();

        for (into, text, expected) in [
            (
                Some("demo-song"),
                "program: 3\n",
                "not a Blimey export file",
            ),
            (
                Some("demo-song"),
                "kind: section\ncues: {}\nsections: { v: 5 }\n",
                "not a Blimey export file",
            ),
            (
                Some("demo-song"),
                "kind: section\ncues: {}\nsections: { v: { bars: 1, blocks: [3] } }\n",
                "not a Blimey export file",
            ),
            (
                Some("demo-song"),
                "kind: scene\ncues: {}\n",
                "not a Blimey export file",
            ),
            (
                Some("demo-song"),
                "kind: scene\nprogram: 4\ncues: { 50: { name: x, type: scene, values: {} } }\n",
                "not a Blimey export file",
            ),
            (None, scene.as_str(), "a scene is imported into a song"),
            (
                Some("demo-song"),
                "kind: scene\ncues: { 50: { name: x, type: scene, values: { nowhere: { dimmer: 50% } } } }\n",
                "nowhere",
            ),
        ] {
            let error = import(&show, into, text).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{text}");
            assert!(error.to_string().contains(expected), "{text}: {error}");
        }
        assert_eq!(
            std::fs::read_to_string(show.join(DEMO_SONG)).unwrap(),
            before
        );
    }
}
