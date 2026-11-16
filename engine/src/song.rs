//! Songs (plan §6): `songs/<name>.yaml`, each holding the Program Change that selects it and
//! the cue table its channel-2 notes fire.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::Show;
use crate::arrangement::{self, Section, SectionFile};
use crate::clock_follower::BarGrid;
use crate::cue::{Cue, CueEntry, PixelCues, resolve_look};
use crate::duration::{Length, TimeSignature};
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
    audio: Option<AudioFile>,
    #[serde(default)]
    safe_look: Mapping,
    #[serde(default)]
    cues: BTreeMap<u8, CueEntry>,
    #[serde(default)]
    sections: BTreeMap<String, SectionFile>,
    #[serde(default)]
    arrangement: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioFile {
    file: String,
    offset: Option<Length>,
}

/// A bounce of the song, for the builder's timeline to play along with the lights.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    /// Relative to the show folder, under `audio/`.
    pub file: PathBuf,
    /// Where bar 1 falls in the file.
    pub offset: Duration,
}

impl AudioFile {
    fn resolve(self) -> Result<Audio, String> {
        let file = PathBuf::from(&self.file);
        let parts: Vec<Component> = file.components().collect();
        let is_under_audio = parts.len() > 1
            && parts[0] == Component::Normal("audio".as_ref())
            && parts
                .iter()
                .all(|part| matches!(part, Component::Normal(_)));
        if !is_under_audio {
            return Err(format!(
                "audio: file {:?} must be a path under audio/, e.g. audio/tell-me.mp3",
                self.file
            ));
        }
        let offset = match self.offset {
            None => Duration::ZERO,
            Some(Length::Time(offset)) => offset,
            Some(_) => return Err("audio: offset must be in ms or s, e.g. 1.25s".into()),
        };
        Ok(Audio { file, offset })
    }
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
    pub audio: Option<Audio>,
    /// What panic shows. An empty look leaves everything dark.
    pub safe_look: Look,
    /// Channel-2 note number → the cue it fires.
    pub cues: BTreeMap<u8, Cue>,
    /// By name; the arrangement places them.
    pub sections: BTreeMap<String, Section>,
    /// Section names in playing order; empty for a song without an arrangement.
    pub arrangement: Vec<String>,
    /// Bar lines of the arrangement's sections, then of `time_signature`.
    pub bar_grid: BarGrid,
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
        let audio = file
            .audio
            .map(AudioFile::resolve)
            .transpose()
            .map_err(|error| located("audio", error))?;
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
        let mut sections = BTreeMap::new();
        for (section_name, section) in file.sections {
            let section = section
                .resolve(time_signature, |note| cues.contains_key(&note))
                .map_err(|(block, error)| {
                    let message = format!("section {section_name}: {error}");
                    match line_in_section(yaml, &section_name, block) {
                        Some(line) => format!("line {line}: {message}"),
                        None => message,
                    }
                })?;
            sections.insert(section_name, section);
        }
        if let Some(unknown) = file
            .arrangement
            .iter()
            .find(|placed| !sections.contains_key(*placed))
        {
            return Err(located(
                "arrangement",
                format!("arrangement: no section {unknown:?} in sections"),
            ));
        }
        let bar_grid = arrangement::bar_grid(&sections, &file.arrangement, time_signature);
        Ok(Song {
            name: name.into(),
            program: file.program,
            bpm: file.bpm,
            time_signature,
            audio,
            safe_look,
            cues,
            sections,
            arrangement: file.arrangement,
            bar_grid,
        })
    }
}

/// The 1-based line of section `name` under `sections:`, or of its block `block` when given
/// and found. Finds keys and items of block-style YAML only, each nested deeper than its parent.
fn line_in_section(yaml: &str, name: &str, block: Option<usize>) -> Option<usize> {
    let lines: Vec<&str> = yaml.lines().collect();
    let lines = &lines[..];
    let indent = |line: &str| line.len() - line.trim_start().len();
    let is_key = |line: &str, key: &str| {
        line.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.starts_with(':'))
    };
    // The lines one level under `parent`: nested deeper than it, at the depth of the first.
    let children = |parent: usize| {
        let parent_indent = indent(lines[parent]);
        let nested = lines[parent + 1..]
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .take_while(move |(_, line)| indent(line) > parent_indent);
        let child_indent = nested.clone().next().map(|(_, line)| indent(line));
        nested
            .filter(move |(_, line)| Some(indent(line)) == child_indent)
            .map(move |(offset, line)| (parent + 1 + offset, *line))
    };
    let sections = lines
        .iter()
        .position(|line| indent(line) == 0 && is_key(line, "sections"))?;
    let (section, _) = children(sections).find(|(_, line)| is_key(line, name))?;
    let block_line = block.and_then(|block| {
        let (blocks, _) = children(section).find(|(_, line)| is_key(line, "blocks"))?;
        children(blocks)
            .filter(|(_, line)| line.trim_start().starts_with('-'))
            .nth(block)
    });
    Some(block_line.map_or(section, |(line, _)| line) + 1)
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
    use crate::cue::{SwellTarget, SwellTiming};
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
        assert_eq!(swell.timing.duration, Length::Bars(4.0));
        assert_eq!(swell.timing.curve, Envelope::EaseIn);
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
            Some(Rhythm::Swell(SwellTiming {
                duration: Length::Bars(4.0),
                curve: Envelope::RampUp
            })),
            "a swell cue's default curve"
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
            "unknown field `tempo`, expected one of `program`, `bpm`, `time_signature`, `audio`, `safe_look`, `cues`, `sections`, `arrangement` at line 2 column 1"
        );
        assert_eq!(
            error("program: 3\ntime_signature: 4/3\n"),
            "line 2: time signature \"4/3\" must be beats/note value with a power of two up to 32 below, e.g. 4/4 or 6/8"
        );
    }

    #[test]
    fn audio_is_a_file_under_audio_with_bar_one_at_its_offset() {
        let song = Song::parse(
            "song",
            "program: 3\naudio: { file: audio/tell-me.mp3, offset: 1.25s }\n",
            &show(),
        )
        .unwrap();
        assert_eq!(
            song.audio,
            Some(Audio {
                file: "audio/tell-me.mp3".into(),
                offset: Duration::from_millis(1250)
            })
        );
        let song = Song::parse(
            "song",
            "program: 3\naudio: { file: audio/a.wav }\n",
            &show(),
        );
        assert_eq!(song.unwrap().audio.unwrap().offset, Duration::ZERO);

        for file in [
            "../secret.mp3",
            "audio/../rig.yaml",
            "/etc/passwd",
            "audio",
            "a.mp3",
        ] {
            assert_eq!(
                error(&format!("program: 3\naudio: {{ file: {file:?} }}\n")),
                format!(
                    "line 2: audio: file {file:?} must be a path under audio/, e.g. audio/tell-me.mp3"
                )
            );
        }
        assert_eq!(
            error("program: 3\naudio: { file: audio/a.wav, offset: 1 bar }\n"),
            "line 2: audio: offset must be in ms or s, e.g. 1.25s"
        );
    }

    const ARRANGED: &str = "program: 3\ncues:\n  36: { name: red, type: scene, values: {} }\n  37: { name: blue, type: scene, values: {} }\nsections:\n  verse:\n    bars: 2\n    blocks:\n      - { scene: 36, bar: 1, length: 2 bars }\n      - { scene: 37, bar: 2, beat: 3.5, length: 1.5 beats, level: 50% }\n  bridge:\n    bars: 1\n    time_signature: 3/4\n    blocks:\n      - { scene: 36, bar: 1, length: 3 beats }\narrangement: [verse, bridge, verse]\n";

    #[test]
    fn an_arrangement_places_its_sections_and_their_blocks_in_beats() {
        let song = Song::parse("song", ARRANGED, &show()).unwrap();

        assert_eq!(song.arrangement, ["verse", "bridge", "verse"]);
        let verse = &song.sections["verse"];
        assert_eq!(verse.time_signature, TimeSignature::default());
        assert_eq!(
            (
                verse.blocks[1].start,
                verse.blocks[1].end,
                verse.blocks[1].level
            ),
            (6.5, 8.0, 0.5)
        );
        assert_eq!(song.sections["bridge"].beats(), 3.0);
        let bar_and_beat = |beats| {
            let position = song.bar_grid.position(beats);
            (position.bar, position.beat)
        };
        assert_eq!(bar_and_beat(9.0), (3, 2.0), "the bridge's 3/4 bar");
        assert_eq!(
            bar_and_beat(11.0),
            (4, 1.0),
            "the verse again, from beat 11"
        );
        assert_eq!(bar_and_beat(20.0), (6, 2.0), "4/4 after the arrangement");
    }

    #[test]
    fn arrangement_errors_name_their_line() {
        let refused = [
            (
                ("[verse, bridge, verse]", "[verse, outro]"),
                "line 16: arrangement: no section \"outro\" in sections",
            ),
            (
                ("scene: 37", "scene: 50"),
                "line 10: section verse: block 2: scene 50 is not in cues",
            ),
            (
                ("length: 3 beats", "length: 4 beats"),
                "line 15: section bridge: block 1: runs 1 beats past the end of the section",
            ),
            (
                ("scene: 37", "scene: 36"),
                "line 10: section verse: block 2: overlaps block 1 of scene 36; a scene plays once at a time",
            ),
            (
                ("length: 2 bars", "length: 4s"),
                "line 9: section verse: block 1: length must be in beats or bars, not ms or s",
            ),
            (
                ("beat: 3.5", "beat: 5"),
                "line 10: section verse: block 2: bar 2 beat 5 is not in a bar: bars count from 1, beats from 1 to under 5",
            ),
            (
                ("bars: 1", "bars: 0"),
                "line 11: section bridge: bars must be from 1 to 9999",
            ),
            (
                ("bars: 1", "bars: 4294967295"),
                "line 11: section bridge: bars must be from 1 to 9999",
            ),
            (
                ("length: 1.5 beats", "length: 0.0001 beats"),
                "line 10: section verse: block 2: length 0.0001 beats is too short to play: make it at least 1/960 beat",
            ),
        ];
        for ((from, to), expected) in refused {
            assert_eq!(error(&ARRANGED.replacen(from, to, 1)), expected, "{to}");
        }
        assert_eq!(
            error(
                &ARRANGED
                    .replace("bridge", "bars")
                    .replacen("bars: 1", "bars: 0", 1)
            ),
            "line 11: section bars: bars must be from 1 to 9999",
            "a section named like a key is found as a section"
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
