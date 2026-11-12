use std::fs;
use std::path::{Path, PathBuf};

use blimey_engine::Show;
use serde_json::Value;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

fn yaml_as_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    serde_yaml_ng::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Validates `file` against `schema_name` if the file exists; returns whether it did.
fn check(schema_name: &str, file: &Path) -> bool {
    if !file.exists() {
        return false;
    }
    let schema = fs::read_to_string(Path::new(ROOT).join("schema").join(schema_name)).unwrap();
    let validator = jsonschema::validator_for(&serde_json::from_str(&schema).unwrap()).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&yaml_as_json(file))
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{} fails {schema_name}: {errors:#?}",
        file.display()
    );
    true
}

fn show_dirs() -> Vec<PathBuf> {
    ["shows", "tests/fixtures/shows"]
        .iter()
        .flat_map(|parent| fs::read_dir(Path::new(ROOT).join(parent)).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect()
}

#[test]
fn every_show_file_matches_its_schema() {
    let mut checked = 0;
    for show in show_dirs() {
        checked += usize::from(check("rig.schema.json", &show.join("rig.yaml")));
        checked += usize::from(check("palettes.schema.json", &show.join("palettes.yaml")));
        checked += usize::from(check(
            "legacy-map.schema.json",
            &show.join("legacy-map.yaml"),
        ));
        for (folder, schema) in [
            ("fixtures", "fixture.schema.json"),
            ("songs", "song.schema.json"),
        ] {
            let Ok(files) = fs::read_dir(show.join(folder)) else {
                continue;
            };
            for file in files {
                let path = file.unwrap().path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "yaml")
                {
                    checked += usize::from(check(schema, &path));
                }
            }
        }
    }
    assert!(checked >= 15, "only {checked} show files found");
}

#[test]
fn the_rig_schema_rejects_a_misspelled_field() {
    let schema = fs::read_to_string(Path::new(ROOT).join("schema/rig.schema.json")).unwrap();
    let validator = jsonschema::validator_for(&serde_json::from_str(&schema).unwrap()).unwrap();
    let rig = serde_json::json!({ "fixtures": [{
        "name": "wash-1", "profile": "p", "mode": "16ch", "adress": 1, "canvas": { "x": 0, "y": 0 }
    }] });

    assert!(!validator.is_valid(&rig));
}

#[test]
fn the_rig_schema_ties_universe_to_art_net_grids() {
    let schema = fs::read_to_string(Path::new(ROOT).join("schema/rig.schema.json")).unwrap();
    let validator = jsonschema::validator_for(&serde_json::from_str(&schema).unwrap()).unwrap();
    let grid = |extra: serde_json::Value| {
        let mut grid = serde_json::json!({
            "name": "g", "type": "pixel-grid", "columns": 2, "rows": 2, "colour_order": "RGB",
            "address": 1, "canvas": { "x": 0, "y": 0 }
        });
        grid.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        validator.is_valid(&serde_json::json!({ "fixtures": [grid] }))
    };

    assert!(grid(serde_json::json!({})));
    assert!(grid(
        serde_json::json!({ "output": "artnet", "universe": 2 })
    ));
    assert!(!grid(serde_json::json!({ "output": "artnet" })));
    assert!(!grid(serde_json::json!({ "universe": 2 })));
}

#[test]
fn the_fossile_show_loads() {
    let show = Show::load(&Path::new(ROOT).join("shows/fossile")).unwrap();

    assert_eq!(show.rig.resolve("all-wash").unwrap().len(), 4);
    assert!(show.palettes.contains_key("deep-blue"));
    assert!(show.rig.fixtures.iter().all(|wash| wash.pixels.len() == 4));
    assert!(show.songs.iter().any(|song| song.name == "tell-me"));
}
