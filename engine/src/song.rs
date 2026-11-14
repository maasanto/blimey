//! Songs (plan §6): `songs/<name>.yaml`, each holding the Program Change that selects it and
//! the cue table its channel-2 notes fire.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::Show;
use crate::cue::{Cue, CueEntry, PixelCues, resolve_look};
use crate::duration::TimeSignature;
use crate::look::Look;

const MIDI_DATA_MAX: u8 = 127;
/// Reaper's default tempo, for a song without `bpm` when no MIDI clock arrives.
const DEFAULT_BPM: f32 = 120.0;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SongFile {
    program: u8,
    bpm: Option<f32>,
    time_signature: Option<String>,
    #[serde(default)]
    safe_look: Mapping,
    #[serde(default)]
    cues: BTreeMap<u8, CueEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    /// The file name without `.yaml`.
    pub name: String,
    /// The channel-2 Program Change value, 0–127, that selects the song.
    pub program: u8,
    /// Tempo for beat durations when Reaper sends no MIDI clock.
    pub bpm: Option<f32>,
    /// How many beats make a bar, for bar durations and bar positions.
    pub time_signature: TimeSignature,
    /// What panic shows. An empty look leaves everything dark.
    pub safe_look: Look,
    /// Channel-2 note number → the cue it fires.
    pub cues: BTreeMap<u8, Cue>,
}

impl Song {
    /// Beats per minute when no MIDI clock arrives: `bpm`, or Reaper's default.
    pub fn tempo(&self) -> f64 {
        f64::from(self.bpm.unwrap_or(DEFAULT_BPM))
    }

    /// Parses a song file. `show` holds the rig and palettes its looks name.
    pub fn parse(name: &str, yaml: &str, show: &Show) -> Result<Song, String> {
        let file: SongFile = serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())?;
        // Checks against the rig run after parsing, so serde can't point at the line: find it.
        let located = |key: &str, error: String| match line_of_key(yaml, key) {
            Some(line) => format!("line {line}: {error}"),
            None => error,
        };
        if file.program > MIDI_DATA_MAX {
            return Err(located("program", "program must be 0-127".into()));
        }
        if file.bpm.is_some_and(|bpm| !(bpm > 0.0 && bpm.is_finite())) {
            return Err(located("bpm", "bpm must be a number above 0".into()));
        }
        let time_signature = file
            .time_signature
            .map(TimeSignature::try_from)
            .transpose()
            .map_err(|error| located("time_signature", error))?
            .unwrap_or_default();
        let safe_look = resolve_look(show, &file.safe_look)
            .map_err(|error| located("safe_look", format!("safe_look: {error}")))?;
        // Swells and patterns may name any pixel cue of the song, before or after them.
        let mut pixel_cues = PixelCues::new();
        for (slot, entry) in &file.cues {
            if let Some(name) = entry.pixel_name() {
                pixel_cues.entry(name.into()).or_default().insert(*slot);
            }
        }
        let mut cues = BTreeMap::new();
        for (slot, entry) in file.cues {
            let in_cue = |error: String| located(&slot.to_string(), format!("cue {slot}: {error}"));
            if slot > MIDI_DATA_MAX {
                return Err(in_cue("cue numbers are note numbers, 0-127".into()));
            }
            let cue = entry.resolve(show, time_signature, &pixel_cues);
            cues.insert(slot, cue.map_err(in_cue)?);
        }
        Ok(Song {
            name: name.into(),
            program: file.program,
            bpm: file.bpm,
            time_signature,
            safe_look,
            cues,
        })
    }
}

/// The 1-based line that starts with `key:`. Finds keys of block-style YAML only.
fn line_of_key(yaml: &str, key: &str) -> Option<usize> {
    yaml.lines()
        .position(|line| {
            line.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
        })
        .map(|index| index + 1)
}

/// Loads every `*.yaml` in `dir`, in file-name order. A missing folder means no songs.
/// `replaced`, `(name, yaml)`, stands in for `<name>.yaml`, whether or not it exists.
pub(crate) fn load_songs(
    dir: &Path,
    show: &Show,
    replaced: Option<(&str, &str)>,
) -> io::Result<Vec<Song>> {
    let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("{}: {error}", dir.display()),
            ));
        }
    };
    // A song saved as .yml would otherwise vanish, and its Program Change select nothing.
    if let Some(yml) = paths
        .iter()
        .find(|path| path.extension() == Some("yml".as_ref()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: rename it to .yaml to load it as a song", yml.display()),
        ));
    }
    paths.retain(|path| path.extension() == Some("yaml".as_ref()));
    if let Some((name, _)) = replaced {
        let path = dir.join(format!("{name}.yaml"));
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    let mut songs: Vec<Song> = Vec::new();
    for path in paths {
        let in_file = |error: String| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        };
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let yaml = match replaced {
            Some((replaced_name, yaml)) if replaced_name == name => yaml.to_owned(),
            _ => fs::read_to_string(&path).map_err(|error| in_file(error.to_string()))?,
        };
        let song = Song::parse(&name, &yaml, show).map_err(in_file)?;
        if let Some(other) = songs.iter().find(|other| other.program == song.program) {
            return Err(in_file(format!(
                "program {} already selects song {:?}",
                song.program, other.name
            )));
        }
        songs.push(song);
    }
    Ok(songs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::SwellTarget;
    use crate::duration::Length;
    use crate::envelope::Envelope;
    use crate::pixel::Rhythm;
    use std::time::Duration;

    fn show() -> Show {
        Show::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/cues"
        )))
        .unwrap()
    }

    fn error(yaml: &str) -> String {
        Song::parse("song", yaml, &show()).unwrap_err()
    }

    const SCENE: &str = "program: 3\ncues:\n  36:\n    name: verse\n    type: scene\n";

    #[test]
    fn a_scene_reads_its_fades_and_resolves_its_look() {
        let song = Song::parse(
            "song",
            &format!(
                "{SCENE}    fade_in: 2s\n    values: {{ both: {{ color: red, dimmer: 60% }} }}\n"
            ),
            &show(),
        )
        .unwrap();

        let Cue::Scene(scene) = &song.cues[&36] else {
            panic!("cue 36 is a scene");
        };
        assert_eq!(scene.name, "verse");
        assert_eq!(scene.fade_in, Length::Time(Duration::from_secs(2)));
        assert_eq!(scene.release, Length::Time(Duration::ZERO));
        assert_eq!(scene.look.fixtures.len(), 2);
        assert_eq!(scene.look.fixtures["rgb-par"].dimmer, 0.6);
        assert_eq!(song.time_signature, TimeSignature::default());
    }

    #[test]
    fn a_swell_reads_its_musical_duration_and_curve() {
        let song = Song::parse(
            "song",
            "program: 3\nbpm: 92\ntime_signature: 6/8\ncues:\n  38:\n    name: chorus-swell\n    type: swell\n    target: { both: { dimmer: 100% } }\n    duration: 4 bars\n    curve: ease-in\n    release: 0.5 beats\n",
            &show(),
        )
        .unwrap();

        let Cue::Swell(swell) = &song.cues[&38] else {
            panic!("cue 38 is a swell");
        };
        assert_eq!(swell.duration, Length::Bars(4.0));
        assert_eq!(swell.curve, Envelope::EaseIn);
        assert_eq!(swell.release, Length::Beats(0.5));
        let SwellTarget::Look(target) = &swell.target else {
            panic!("cue 38 targets a look");
        };
        assert_eq!(target.fixtures.len(), 2);
        assert_eq!(song.time_signature.beats_per_bar(), 3.0);
    }

    #[test]
    fn a_swell_needs_a_duration_and_well_ordered_points() {
        assert_eq!(
            error("program: 3\ncues:\n  38: { name: s, type: swell, target: {} }\n"),
            "cues: missing field `duration` at line 3 column 3"
        );
        assert_eq!(
            error(
                "program: 3\ncues:\n  38:\n    name: s\n    type: swell\n    target: {}\n    duration: 1 bar\n    curve: { points: [[0.5, 1], [0.2, 0]] }\n"
            ),
            "cues: points: times must rise from one point to the next at line 3 column 3"
        );
    }

    #[test]
    fn a_pixel_cue_reads_its_layers_and_refuses_params_it_cannot_play() {
        const PIXEL: &str =
            "program: 3\nbpm: 92\ncues:\n  40:\n    name: bridge-chase\n    type: pixel\n";
        let song = Song::parse(
            "song",
            &format!(
                "{PIXEL}    effect: chase\n    params: {{ color: red, width: 0.2, speed: 1 bar, direction: left-to-right }}\n    seed: 7\n    layer_2: {{ shape: circle, blend: mask }}\n"
            ),
            &show(),
        )
        .unwrap();

        let Cue::Pixel(scene) = &song.cues[&40] else {
            panic!("cue 40 is a pixel cue");
        };
        let [layer_1, layer_2] = &scene.layers[..] else {
            panic!("two layers: {:?}", scene.layers);
        };
        assert_eq!(layer_1.effect, crate::pixel::Effect::Chase);
        assert_eq!(layer_1.speed, Length::Bars(1.0));
        assert_eq!(scene.seed, 7);
        assert_eq!(layer_2.blend, crate::pixel::Blend::Mask);
        assert_eq!(layer_2.shape, crate::pixel::Shape::Circle);
        assert_eq!(layer_2.width, 1.0, "defaults to the whole canvas");

        assert_eq!(
            error(&format!("{PIXEL}    params: {{ width: 0 }}\n")),
            "line 4: cue 40: width 0 must be above 0 and at most 1"
        );
        assert_eq!(
            error(&format!(
                "{PIXEL}    layer_2: {{ params: {{ color: teal }} }}\n"
            )),
            "line 4: cue 40: layer_2: no colour \"teal\" in palettes.yaml, and not a #rrggbb value"
        );
        assert_eq!(
            error(&format!("{PIXEL}    params: {{ speed: 0 beats }}\n")),
            "line 4: cue 40: speed must be above 0"
        );
    }

    #[test]
    fn a_layered_pixel_cue_reads_its_areas_and_mutes_and_refuses_what_it_cannot_play() {
        const PIXEL: &str = "program: 3\ncues:\n  40:\n    name: halves\n    type: pixel\n";
        let layers = |layers: &str| format!("{PIXEL}    layers:\n{layers}");
        let song = Song::parse(
            "song",
            &layers(
                "      - { area: { x: 0, y: 0, width: 0.5, height: 1 }, shape: fill, level: 60% }\n      - { blend: over, mute: [both] }\n",
            ),
            &show(),
        )
        .unwrap();

        let Cue::Pixel(scene) = &song.cues[&40] else {
            panic!("cue 40 is a pixel cue");
        };
        let [left, whole] = &scene.layers[..] else {
            panic!("two layers: {:?}", scene.layers);
        };
        assert_eq!((left.area.width, left.level), (0.5, 0.6));
        assert_eq!(left.shape, crate::pixel::Shape::Fill);
        assert_eq!((whole.area.width, whole.area.height), (1.0, 1.0));
        assert_eq!(whole.blend, crate::pixel::Blend::Over);
        assert_eq!(
            whole.mute.iter().collect::<Vec<_>>(),
            ["dimmer-par", "rgb-par"],
            "a group mutes its fixtures"
        );

        let refused = [
            (
                "      - { area: { x: 0.5, y: 0, width: 0, height: 1 } }\n",
                "layer 1: area: width 0 and height 1 must both be above 0",
            ),
            (
                "      - {}\n      - { area: { x: 0.75, y: 0, width: 0.5, height: 1 } }\n",
                "layer 2: area: canvas { x: 0.75, y: 0, width: 0.5, height: 1 } leaves the 0-1 stage canvas",
            ),
            (
                "      - { mute: [inner-wash] }\n",
                "layer 1: mute: \"inner-wash\" is neither a fixture nor a group in the rig",
            ),
            (
                &"      - {}\n".repeat(9),
                "9 layers; a pixel cue has 1 to 8",
            ),
            ("      []\n", "0 layers; a pixel cue has 1 to 8"),
        ];
        for (yaml, expected) in refused {
            assert_eq!(error(&layers(yaml)), format!("line 3: cue 40: {expected}"));
        }
        assert_eq!(
            error(&format!(
                "{PIXEL}    effect: chase\n    layers:\n      - {{}}\n"
            )),
            "line 3: cue 40: a pixel cue has either layers or shape, effect, params and layer_2, not both"
        );
    }

    #[test]
    fn a_layer_reads_its_swell_or_its_pattern_in_the_song_time_signature() {
        const PIXEL: &str =
            "program: 3\ntime_signature: 6/8\ncues:\n  40:\n    name: halves\n    type: pixel\n";
        let layers = |layers: &str| format!("{PIXEL}    layers:\n{layers}");
        let song = Song::parse(
            "song",
            &layers(
                "      - { swell: { duration: 4 bars } }\n      - { pattern: { length: 2 bars, hits: [{ at: 6.75, shape: sine, length: 1 beat, level: 50% }] } }\n",
            ),
            &show(),
        )
        .unwrap();

        let Cue::Pixel(scene) = &song.cues[&40] else {
            panic!("cue 40 is a pixel cue");
        };
        assert_eq!(
            scene.layers[0].rhythm,
            Some(Rhythm::Swell {
                duration: Length::Bars(4.0),
                curve: Envelope::RampUp
            })
        );
        let Some(Rhythm::Pattern(hits)) = &scene.layers[1].rhythm else {
            panic!("layer 2 has a pattern: {:?}", scene.layers[1].rhythm);
        };
        assert_eq!(
            (hits.beats, hits.hits[0].offset),
            (6.0, 5.75),
            "two bars of 6/8"
        );

        let refused = [
            (
                "      - { swell: { duration: 1 bar }, pattern: { length: 1 bar, hits: [] } }\n",
                "layer 1: a layer has a swell or a pattern, not both",
            ),
            (
                "      - {}\n      - { pattern: { length: 8 beats, hits: [] } }\n",
                "layer 2: pattern: length must be 1, 2, 3 or 4 bars",
            ),
        ];
        for (yaml, expected) in refused {
            assert_eq!(error(&layers(yaml)), format!("line 4: cue 40: {expected}"));
        }
    }

    #[test]
    fn a_swell_or_pattern_names_the_pixel_cues_whose_intensity_it_drives() {
        const PIXELS: &str = "program: 3\ncues:\n  40: { name: chase, type: pixel }\n  41: { name: chase, type: pixel }\n";
        let song = Song::parse(
            "song",
            &format!(
                "{PIXELS}  38: {{ name: rise, type: swell, target: {{ chase: intensity }}, duration: 1 bar }}\n"
            ),
            &show(),
        )
        .unwrap();

        assert_eq!(
            song.cues[&38].intensity_targets(),
            Some(&[40, 41].into()),
            "every pixel cue of that name, before or after it"
        );
        assert_eq!(
            error(&format!(
                "{PIXELS}  42:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: {{ chaser: intensity }}\n    hits: []\n"
            )),
            "line 5: cue 42: chaser: no pixel cue of that name in this song"
        );
    }

    #[test]
    fn checks_against_the_rig_name_the_line_of_the_cue() {
        assert_eq!(
            error(&format!(
                "{SCENE}    values:\n      both: {{ color: teal }}\n"
            )),
            "line 3: cue 36: both: no colour \"teal\" in palettes.yaml, and not a #rrggbb value"
        );
        assert_eq!(
            error(&format!(
                "{SCENE}    values:\n      back-wash: {{ color: red }}\n"
            )),
            "line 3: cue 36: back-wash: \"back-wash\" is neither a fixture nor a group in the rig"
        );
        assert_eq!(
            error("program: 3\nsafe_look:\n  both: { dimmer: 120% }\n"),
            "line 2: safe_look: both: level \"120%\" must be a percentage from 0% to 100%"
        );
    }

    #[test]
    fn file_format_errors_carry_serde_line_numbers() {
        assert_eq!(
            error(&format!(
                "{SCENE}    fade_in: 2 seconds\n    values: {{}}\n"
            )),
            "cues: duration \"2 seconds\" must be a number then ms, s, beats or bars, e.g. 500ms, 2s, 0.5 beats or 4 bars at line 3 column 3"
        );
        assert_eq!(
            error("program: 3\ncues:\n  36:\n    name: verse\n    type: scen\n"),
            "cues.36.type: unknown variant `scen`, expected one of `scene`, `swell`, `pattern`, `pixel` at line 5 column 11"
        );
        assert_eq!(
            error("program: 3\ntempo: 92\n"),
            "unknown field `tempo`, expected one of `program`, `bpm`, `time_signature`, `safe_look`, `cues` at line 2 column 1"
        );
        assert_eq!(
            error("program: 3\ntime_signature: 4/3\n"),
            "line 2: time signature \"4/3\" must be beats/note value with a power of two up to 32 below, e.g. 4/4 or 6/8"
        );
    }

    #[test]
    fn rejects_numbers_outside_midi() {
        assert_eq!(error("program: 128\n"), "line 1: program must be 0-127");
        for bpm in ["0", ".nan", ".inf"] {
            assert_eq!(
                error(&format!("program: 3\nbpm: {bpm}\n")),
                "line 2: bpm must be a number above 0",
                "bpm: {bpm}"
            );
        }
        assert_eq!(
            error("program: 3\ncues:\n  128: { name: x, type: scene, values: {} }\n"),
            "line 3: cue 128: cue numbers are note numbers, 0-127"
        );
    }

    #[test]
    fn two_songs_on_one_program_are_refused() {
        let dir =
            std::env::temp_dir().join(format!("blimey-shared-program-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.yaml"), "program: 3\n").unwrap();
        fs::write(dir.join("b.yaml"), "program: 3\n").unwrap();

        let message = load_songs(&dir, &show(), None).unwrap_err().to_string();

        assert!(
            message.ends_with("b.yaml: program 3 already selects song \"a\""),
            "{message}"
        );
    }

    #[test]
    fn a_song_saved_as_yml_is_reported_not_skipped() {
        let dir = std::env::temp_dir().join(format!("blimey-yml-song-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.yaml"), "program: 3\n").unwrap();
        fs::write(dir.join("intro.yml"), "program: 4\n").unwrap();
        fs::write(dir.join(".DS_Store"), "").unwrap();

        let message = load_songs(&dir, &show(), None).unwrap_err().to_string();

        assert!(
            message.ends_with("intro.yml: rename it to .yaml to load it as a song"),
            "{message}"
        );
    }
}
