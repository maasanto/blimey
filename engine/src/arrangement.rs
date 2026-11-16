//! A song's running order (plan §17): sections of bars holding blocks, each a scene held from
//! one point to another, and the arrangement, the order the sections play in. `events` lists
//! what plays it, for the MIDI file Reaper plays and for the builder alike.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clock_follower::{BarGrid, BarStretch};
use crate::duration::{Length, TimeSignature};
use crate::look::Level;
use crate::Show;
use crate::midi_file::MidiFile;
use crate::show_file::write_atomically;
use crate::song::Song;

/// The show folder's subfolder for exported MIDI files, kept out of git.
pub const EXPORT_DIR: &str = "export";
/// The grid events are placed on, and the resolution of the exported file.
pub(crate) const TICKS_PER_BEAT: u16 = 960;
const MIDI_DATA_MAX: f32 = 127.0;
/// Room for rounding when comparing beats worked out along different paths.
const BEAT_EPSILON: f64 = 1e-9;
/// Far longer than any section, and short enough that bar numbers never overflow.
pub const MAX_BARS: u32 = 9999;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SectionFile {
    bars: u32,
    time_signature: Option<String>,
    #[serde(default)]
    blocks: Vec<BlockFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockFile {
    scene: u8,
    bar: u32,
    beat: Option<f64>,
    length: Length,
    level: Option<Level>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub bars: u32,
    /// The section's own, or the song's.
    pub time_signature: TimeSignature,
    pub blocks: Vec<Block>,
}

/// A scene held over part of a section, in beats from the section's first beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub scene: u8,
    pub start: f64,
    pub end: f64,
    /// 0–1, played as the note's velocity.
    pub level: f32,
}

impl Section {
    pub fn beats(&self) -> f64 {
        f64::from(self.bars) * self.time_signature.beats_per_bar()
    }
}

/// Why a section is refused: the message, and the index of the block at fault, if one is.
pub(crate) type SectionError = (Option<usize>, String);

impl SectionFile {
    /// Checks the section against its song: `song_signature` stands in for a signature of its
    /// own, and `is_cue` says whether a note number has a cue.
    pub(crate) fn resolve(
        self,
        song_signature: TimeSignature,
        is_cue: impl Fn(u8) -> bool,
    ) -> Result<Section, SectionError> {
        let time_signature = self
            .time_signature
            .map(TimeSignature::try_from)
            .transpose()
            .map_err(|error| (None, error))?
            .unwrap_or(song_signature);
        if !(1..=MAX_BARS).contains(&self.bars) {
            return Err((None, format!("bars must be from 1 to {MAX_BARS}")));
        }
        let mut section = Section {
            bars: self.bars,
            time_signature,
            blocks: Vec::with_capacity(self.blocks.len()),
        };
        for (index, block) in self.blocks.into_iter().enumerate() {
            let in_block = |error: String| (Some(index), format!("block {}: {error}", index + 1));
            section
                .blocks
                .push(block.resolve(&section, &is_cue).map_err(in_block)?);
        }
        refuse_overlaps(&section.blocks)?;
        Ok(section)
    }
}

impl BlockFile {
    fn resolve(self, section: &Section, is_cue: impl Fn(u8) -> bool) -> Result<Block, String> {
        let beats_per_bar = section.time_signature.beats_per_bar();
        if !is_cue(self.scene) {
            return Err(format!("scene {} is not in cues", self.scene));
        }
        let beat = self.beat.unwrap_or(1.0);
        if self.bar == 0 || !(1.0..beats_per_bar + 1.0).contains(&beat) {
            return Err(format!(
                "bar {} beat {beat} is not in a bar: bars count from 1, beats from 1 to under {}",
                self.bar,
                beats_per_bar + 1.0
            ));
        }
        let length = match self.length {
            Length::Beats(beats) => beats,
            Length::Bars(bars) => bars * beats_per_bar,
            Length::Time(_) => {
                return Err("length must be in beats or bars, not ms or s".into());
            }
        };
        if length <= 0.0 {
            return Err("length must be above 0".into());
        }
        let start = f64::from(self.bar - 1) * beats_per_bar + beat - 1.0;
        let end = start + length;
        if end > section.beats() + BEAT_EPSILON {
            return Err(format!(
                "runs {} beats past the end of the section",
                end - section.beats()
            ));
        }
        // Its note on and off would share a tick, where the off plays first: the scene would stick.
        if tick(start) == tick(end) {
            return Err(format!(
                "length {length} beats is too short to play: make it at least 1/{TICKS_PER_BEAT} beat"
            ));
        }
        Ok(Block {
            scene: self.scene,
            start,
            end,
            level: self.level.map_or(1.0, |level| level.0),
        })
    }
}

/// Two blocks of one scene may not overlap; touching is fine.
fn refuse_overlaps(blocks: &[Block]) -> Result<(), SectionError> {
    for (later, block) in blocks.iter().enumerate() {
        let overlapped = blocks[..later].iter().position(|earlier| {
            earlier.scene == block.scene
                && earlier.start < block.end - BEAT_EPSILON
                && block.start < earlier.end - BEAT_EPSILON
        });
        if let Some(earlier) = overlapped {
            return Err((
                Some(later),
                format!(
                    "block {}: overlaps block {} of scene {}; a scene plays once at a time",
                    later + 1,
                    earlier + 1,
                    block.scene
                ),
            ));
        }
    }
    Ok(())
}

/// A section where it plays: from `beats` and bar `bar` of the song.
struct Placement<'song> {
    name: &'song str,
    section: &'song Section,
    beats: f64,
    bar: u32,
}

/// The arrangement's sections in playing order. Every name must be in `sections`.
fn placements<'song>(
    sections: &'song BTreeMap<String, Section>,
    arrangement: &'song [String],
) -> Vec<Placement<'song>> {
    let (mut beats, mut bar) = (0.0, 1);
    arrangement
        .iter()
        .map(|name| {
            let section = &sections[name];
            let placement = Placement {
                name,
                section,
                beats,
                bar,
            };
            beats += section.beats();
            bar = bar.saturating_add(section.bars);
            placement
        })
        .collect()
}

/// Bar lines in each placed section's signature, then the song's after the arrangement ends.
pub(crate) fn bar_grid(
    sections: &BTreeMap<String, Section>,
    arrangement: &[String],
    song_signature: TimeSignature,
) -> BarGrid {
    let placed = placements(sections, arrangement);
    let stretch = |beats, bar, time_signature: TimeSignature| BarStretch {
        beats,
        bar,
        beats_per_bar: time_signature.beats_per_bar(),
    };
    let mut stretches: Vec<BarStretch> = placed
        .iter()
        .map(|placement| {
            stretch(
                placement.beats,
                placement.bar,
                placement.section.time_signature,
            )
        })
        .collect();
    let (beats, bar) = placed.last().map_or((0.0, 1), |last| {
        (
            last.beats + last.section.beats(),
            last.bar.saturating_add(last.section.bars),
        )
    });
    stretches.push(stretch(beats, bar, song_signature));
    BarGrid::new(stretches)
}

/// One thing that happens as the arrangement plays, at a beat from the top of the song and
/// the second the song's tempo puts it at.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArrangementEvent {
    pub beats: f64,
    pub seconds: f64,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// In the order they play when they share a beat.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    TimeSignature { numerator: u8, denominator: u8 },
    Marker { section: String },
    ProgramChange { program: u8 },
    NoteOff { scene: u8 },
    NoteOn { scene: u8, velocity: u8 },
}

impl EventKind {
    fn rank(&self) -> u8 {
        match self {
            EventKind::TimeSignature { .. } => 0,
            EventKind::Marker { .. } => 1,
            EventKind::ProgramChange { .. } => 2,
            EventKind::NoteOff { .. } => 3,
            EventKind::NoteOn { .. } => 4,
        }
    }
}

/// A block's note, in ticks from the top of the song.
struct Note {
    scene: u8,
    velocity: u8,
    start: u64,
    end: u64,
}

fn tick(beats: f64) -> u64 {
    (beats * f64::from(TICKS_PER_BEAT)).round() as u64
}

/// What plays the song's arrangement, in order, on a grid of `TICKS_PER_BEAT`: the time
/// signature wherever it changes, a marker per placed section, the song's Program Change, then
/// a note per block. A scene held to the end of one section and from the start of the next, at
/// the same level, is one note. Empty for a song without an arrangement.
pub fn events(song: &Song) -> Vec<ArrangementEvent> {
    let placed = placements(&song.sections, &song.arrangement);
    if placed.is_empty() {
        return Vec::new();
    }
    let mut timed = vec![(
        0,
        EventKind::ProgramChange {
            program: song.program,
        },
    )];
    let mut signature = None;
    let mut notes: Vec<Note> = Vec::new();
    for placement in &placed {
        let at = tick(placement.beats);
        let section = placement.section;
        if signature != Some(section.time_signature) {
            signature = Some(section.time_signature);
            timed.push((
                at,
                EventKind::TimeSignature {
                    numerator: section.time_signature.numerator,
                    denominator: section.time_signature.denominator,
                },
            ));
        }
        timed.push((
            at,
            EventKind::Marker {
                section: placement.name.into(),
            },
        ));
        for block in &section.blocks {
            let velocity = (block.level * MIDI_DATA_MAX).round().max(1.0) as u8;
            let (start, end) = (
                tick(placement.beats + block.start),
                tick(placement.beats + block.end),
            );
            let held = notes.iter_mut().find(|note| {
                start == at
                    && note.end == at
                    && note.scene == block.scene
                    && note.velocity == velocity
            });
            match held {
                Some(note) => note.end = end,
                None => notes.push(Note {
                    scene: block.scene,
                    velocity,
                    start,
                    end,
                }),
            }
        }
    }
    for note in notes {
        timed.push((
            note.start,
            EventKind::NoteOn {
                scene: note.scene,
                velocity: note.velocity,
            },
        ));
        timed.push((note.end, EventKind::NoteOff { scene: note.scene }));
    }
    timed.sort_by_key(|(tick, kind)| (*tick, kind.rank()));
    let seconds_per_beat = 60.0 / song.tempo();
    timed
        .into_iter()
        .map(|(tick, kind)| {
            let beats = tick as f64 / f64::from(TICKS_PER_BEAT);
            ArrangementEvent {
                beats,
                seconds: beats * seconds_per_beat,
                kind,
            }
        })
        .collect()
}

/// The arrangement as a Standard MIDI File for Reaper: the song's tempo, then `events`.
pub fn midi_file(song: &Song) -> io::Result<Vec<u8>> {
    MidiFile::encode_arrangement(song.tempo(), &events(song))
}

/// Writes `<show_dir>/export/<song>.mid` for a song with an arrangement, and returns its path;
/// `None` for a song without one.
pub fn export(show_dir: &Path, song: &Song) -> io::Result<Option<PathBuf>> {
    if song.arrangement.is_empty() {
        return Ok(None);
    }
    write_export(show_dir, &song.name, &midi_file(song)?).map(Some)
}

/// Writes `bytes` as the exported file of song `name`, atomically, so Reaper never reads half
/// of one; returns its path.
pub fn write_export(show_dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let folder = show_dir.join(EXPORT_DIR);
    let path = folder.join(format!("{name}.mid"));
    fs::create_dir_all(&folder)
        .and_then(|()| write_atomically(&path, bytes))
        .map_err(|error| in_path(&path, error))?;
    Ok(path)
}

/// Removes the exported files of songs `show` doesn't arrange: an arrangement cleared, or a
/// song renamed or gone.
pub fn remove_stale_exports(show_dir: &Path, show: &Show) -> io::Result<()> {
    let folder = show_dir.join(EXPORT_DIR);
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(in_path(&folder, error)),
    };
    for entry in entries {
        let path = entry.map_err(|error| in_path(&folder, error))?.path();
        let is_arranged = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|name| {
                show.songs
                    .iter()
                    .any(|song| song.name == name && !song.arrangement.is_empty())
            });
        if path.extension() == Some("mid".as_ref()) && !is_arranged {
            fs::remove_file(&path).map_err(|error| in_path(&path, error))?;
        }
    }
    Ok(())
}

fn in_path(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}
