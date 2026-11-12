//! `blimey soak`: loops a setlist of recorded songs through the render loop for hours and checks
//! it holds up (plan §15): no dropped frames, no drift, low jitter, flat memory.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use blimey_engine::look::Look;
use blimey_engine::{
    Clock, DmxSink, FRAME_PERIOD, Frame, MidiEvent, MidiFile, MidiSource, ShowSource, Status,
};
use serde::Deserialize;

use crate::enttec::emulator::EmulatedEnttec;

/// Silence between two songs, as on stage.
const SONG_GAP: Duration = Duration::from_secs(2);
/// Each pass is checked against the second one (0-based 1). The first starts from a fresh
/// engine; later ones start from whatever the last song left, such as legacy levels or master.
const REFERENCE_PASS: u32 = 1;
/// One pass to warm up, one as the reference, one to compare with it.
const MIN_PASSES: u32 = 3;
/// A fifth of a frame: a frame that late still goes out well within its slot.
const JITTER_LIMIT: Duration = Duration::from_millis(5);
/// RSS may grow this much after the first pass. The engine allocates nothing that outlives a
/// frame, so allocator noise stays well below it; a leak of 8 bytes a frame exceeds it within
/// an hour, and of 1 byte a frame within 8 hours.
const MEMORY_GROWTH_LIMIT_KIB: u64 = 1024;
/// RSS samples spread over the run, besides the one at the start of the reference pass.
const RSS_SAMPLES: u32 = 20;
const LATENESS_BUCKET: Duration = Duration::from_micros(100);
/// Lateness from 0 to 100 ms in 0.1 ms steps; later frames share the last bucket.
const LATENESS_BUCKETS: usize = 1000;
const REPORT_DIR: &str = "claude-artifacts/soak";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetlistFile {
    show: PathBuf,
    songs: Vec<PathBuf>,
}

/// A show folder and the recorded songs to play against it, in order; paths in the file are
/// relative to it.
pub struct Setlist {
    path: PathBuf,
    pub show_dir: PathBuf,
    songs: Vec<SetlistSong>,
    /// Frames in one pass of the setlist, gaps included.
    pass_frames: u32,
}

struct SetlistSong {
    path: PathBuf,
    midi: Vec<u8>,
    /// Frame the song starts on, from the start of the pass.
    first_frame: u32,
}

/// Where a frame falls: which pass, which song, and the frame that song started on.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Slot {
    pass: u32,
    song: usize,
    first_frame: u32,
}

impl Setlist {
    pub fn load(path: &Path) -> Result<Self, String> {
        let in_file = |error: String| format!("{}: {error}", path.display());
        let text = fs::read_to_string(path).map_err(|error| in_file(error.to_string()))?;
        let file: SetlistFile =
            serde_yaml_ng::from_str(&text).map_err(|error| in_file(error.to_string()))?;
        if file.songs.is_empty() {
            return Err(in_file("`songs` lists no MIDI files".into()));
        }
        let folder = path.parent().unwrap_or(Path::new(""));
        let mut songs = Vec::new();
        let mut pass_frames = 0;
        for song in file.songs {
            let song_path = folder.join(song);
            let in_song = |error: io::Error| format!("{}: {error}", song_path.display());
            let midi = fs::read(&song_path).map_err(in_song)?;
            let duration = MidiFile::parse(&midi).map_err(in_song)?.duration();
            songs.push(SetlistSong {
                path: song_path,
                midi,
                first_frame: pass_frames,
            });
            // Starting each song on a frame keeps its frames comparable from pass to pass.
            pass_frames += frames_in(duration + SONG_GAP);
        }
        Ok(Self {
            path: path.to_path_buf(),
            show_dir: folder.join(file.show),
            songs,
            pass_frames,
        })
    }

    fn slot(&self, frame: u32) -> Slot {
        let pass = frame / self.pass_frames;
        let within = frame % self.pass_frames;
        let song = self
            .songs
            .partition_point(|song| song.first_frame <= within)
            - 1;
        Slot {
            pass,
            song,
            first_frame: pass * self.pass_frames + self.songs[song].first_frame,
        }
    }
}

fn frames_in(duration: Duration) -> u32 {
    let frames = duration.as_nanos().div_ceil(FRAME_PERIOD.as_nanos());
    u32::try_from(frames).expect("a song shorter than three years")
}

/// The setlist's songs back to back, looped, on the render loop's timeline.
struct SetlistMidi<'a> {
    setlist: &'a Setlist,
    playing: Option<(Slot, MidiFile)>,
}

impl MidiSource for SetlistMidi<'_> {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        let frame = u32::try_from(until.as_nanos() / FRAME_PERIOD.as_nanos()).ok()?;
        let slot = self.setlist.slot(frame);
        if self
            .playing
            .as_ref()
            .is_none_or(|(playing, _)| *playing != slot)
        {
            let midi = MidiFile::parse(&self.setlist.songs[slot.song].midi)
                .expect("parsed when the setlist loaded");
            self.playing = Some((slot, midi));
        }
        let (slot, midi) = self.playing.as_mut()?;
        let start = FRAME_PERIOD * slot.first_frame;
        let mut event = midi.poll(until - start)?;
        event.at += start;
        Some(event)
    }
}

pub struct Soak<'a, S> {
    pub setlist: &'a Setlist,
    pub hours: f64,
    /// On a fake clock as fast as the machine renders, rather than in real time.
    pub accelerated: bool,
    pub output: S,
    /// The emulated box `output` sends to, if any: frames it misses count as dropped.
    pub device: Option<&'a EmulatedEnttec>,
}

impl<S: DmxSink> Soak<'_, S> {
    /// Runs the soak, writes the report under `claude-artifacts/soak/` and prints it.
    /// Returns whether every check passed.
    pub fn run(
        self,
        clock: &mut impl Clock,
        shows: impl ShowSource,
    ) -> Result<bool, Box<dyn Error>> {
        let frame_count = self.frame_count()?;
        let path = self.report_path();
        let mut sink = Measured {
            setlist: self.setlist,
            output: self.output,
            device: self.device,
            sent: 0,
            received: 0,
            lateness: Lateness::default(),
            song: None,
            song_hash: DefaultHasher::new(),
            reference: Vec::new(),
            songs_compared: 0,
            drifted: 0,
            first_drift: None,
            memory: MemorySampler::start(self.accelerated),
            sample_every: (frame_count / RSS_SAMPLES).max(1),
        };
        let mode = if self.accelerated {
            "accelerated (fake clock)"
        } else {
            "real time"
        };
        eprintln!(
            "soak: {:.1} h of {} in {mode}",
            self.hours,
            self.setlist.path.display()
        );
        let started = Instant::now();
        blimey_engine::run(
            clock,
            &mut SetlistMidi {
                setlist: self.setlist,
                playing: None,
            },
            shows,
            &Look::default(),
            &mut sink,
            frame_count,
        )?;
        let elapsed = started.elapsed();
        sink.memory.sample(frame_count);
        sink.collect_last_frames();
        let (report, passed) = sink.report(mode, elapsed);
        print!("{report}");
        fs::create_dir_all(REPORT_DIR)
            .and_then(|()| fs::write(&path, &report))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        eprintln!("soak: report written to {}", path.display());
        Ok(passed)
    }

    fn frame_count(&self) -> Result<u32, String> {
        let frames = (self.hours * 3600.0 / FRAME_PERIOD.as_secs_f64()).round();
        if !(1.0..=f64::from(u32::MAX)).contains(&frames) {
            return Err(format!(
                "--hours {} is out of range: from one frame to three years",
                self.hours
            ));
        }
        let frames = frames as u32;
        let pass_frames = self.setlist.pass_frames;
        if u64::from(frames) < u64::from(MIN_PASSES) * u64::from(pass_frames) {
            return Err(format!(
                "one pass of {} lasts {:.0} s, so --hours {} covers fewer than the {MIN_PASSES} passes drift is checked over",
                self.setlist.path.display(),
                (FRAME_PERIOD * pass_frames).as_secs_f64(),
                self.hours
            ));
        }
        Ok(frames)
    }

    fn report_path(&self) -> PathBuf {
        let name = self
            .setlist
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let mode = if self.accelerated {
            "accelerated"
        } else {
            "real-time"
        };
        let since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Path::new(REPORT_DIR).join(format!("{name}-{mode}-{since_epoch}.txt"))
    }
}

/// Passes frames on to the soak's output and measures them.
struct Measured<'a, S> {
    setlist: &'a Setlist,
    output: S,
    device: Option<&'a EmulatedEnttec>,
    sent: u32,
    /// Frames the emulated box received.
    received: usize,
    lateness: Lateness,
    song: Option<Slot>,
    song_hash: DefaultHasher,
    /// Fingerprint of each song's frames in the reference pass.
    reference: Vec<u64>,
    songs_compared: u32,
    drifted: u32,
    first_drift: Option<String>,
    memory: MemorySampler,
    sample_every: u32,
}

impl<S: DmxSink> DmxSink for Measured<'_, S> {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        let index = self.sent;
        self.lateness
            .record(at.saturating_sub(FRAME_PERIOD * index));
        let slot = self.setlist.slot(index);
        if self.song != Some(slot) {
            self.finish_song();
            self.song = Some(slot);
            if slot.pass == REFERENCE_PASS && slot.song == 0 {
                self.memory.sample(index);
            }
            if let Some(device) = self.device {
                self.received += device.take_received().len();
            }
        }
        if index.is_multiple_of(self.sample_every) {
            self.memory.sample(index);
        }
        frame.hash(&mut self.song_hash);
        self.sent += 1;
        self.output.send(at, frame)
    }

    fn status(&mut self, status: &Status) {
        self.output.status(status);
    }
}

impl<S> Measured<'_, S> {
    /// Compares the song that just ended with the same song in the reference pass. The last
    /// song of the run is cut short, so it is never finished.
    fn finish_song(&mut self) {
        let Some(slot) = self.song else {
            return;
        };
        let fingerprint = std::mem::take(&mut self.song_hash).finish();
        if slot.pass == REFERENCE_PASS {
            self.reference.push(fingerprint);
        } else if slot.pass > REFERENCE_PASS {
            self.songs_compared += 1;
            if self.reference.get(slot.song) != Some(&fingerprint) {
                self.drifted += 1;
                self.first_drift.get_or_insert_with(|| {
                    format!(
                        "first: pass {}, {}",
                        slot.pass + 1,
                        self.setlist.songs[slot.song].path.display()
                    )
                });
            }
        }
    }

    /// Waits up to a second for the emulated box to read the last frames sent.
    fn collect_last_frames(&mut self) {
        let Some(device) = self.device else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            self.received += device.take_received().len();
            if self.received >= self.sent as usize || Instant::now() > deadline {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn report(self, mode: &str, elapsed: Duration) -> (String, bool) {
        let setlist = self.setlist;
        let late = self.lateness.at_least(FRAME_PERIOD);
        let missed = self
            .device
            .map_or(0, |_| (self.sent as usize).saturating_sub(self.received));
        let dropped = late + missed as u64;
        let device_note = match self.device {
            Some(_) => format!("; the emulated Enttec received {} of them", self.received),
            None => String::new(),
        };
        let p99 = self.lateness.percentile(0.99);
        let memory = MemoryCheck::from(self.memory.finish(), setlist.pass_frames * REFERENCE_PASS);
        let checks = [
            (
                "frames",
                dropped == 0,
                format!(
                    "{} rendered, {dropped} dropped (sent a frame period or more late, or never received){device_note}",
                    self.sent
                ),
                "none dropped".to_string(),
            ),
            (
                "jitter",
                p99 < JITTER_LIMIT,
                format!(
                    "frame lateness p50 {}, p99 {}, max {}",
                    millis(self.lateness.percentile(0.5)),
                    millis(p99),
                    millis(self.lateness.max)
                ),
                format!("p99 under {}", millis(JITTER_LIMIT)),
            ),
            (
                "drift",
                self.drifted == 0 && self.songs_compared > 0,
                format!(
                    "{} songs compared with their render in pass {}, {} differ{}",
                    self.songs_compared,
                    REFERENCE_PASS + 1,
                    self.drifted,
                    self.first_drift
                        .map(|first| format!(" ({first})"))
                        .unwrap_or_default()
                ),
                "none differ".to_string(),
            ),
            (
                "memory",
                memory.is_flat(),
                memory.summary(),
                format!(
                    "RSS grows at most {} MiB after pass {}",
                    MEMORY_GROWTH_LIMIT_KIB / 1024,
                    REFERENCE_PASS
                ),
            ),
        ];
        let passed = checks.iter().all(|(_, ok, ..)| *ok);
        let mut report = format!(
            "Blimey soak: {}\nsetlist  {}: {} songs, one pass {:.0} s, show {}\nmode     {mode}: {:.1} h of show in {:.0} s\n",
            verdict(passed),
            setlist.path.display(),
            setlist.songs.len(),
            (FRAME_PERIOD * setlist.pass_frames).as_secs_f64(),
            setlist.show_dir.display(),
            (FRAME_PERIOD * self.sent).as_secs_f64() / 3600.0,
            elapsed.as_secs_f64(),
        );
        for (name, ok, detail, limit) in checks {
            writeln!(
                report,
                "{name:<8} {}  {detail}  [limit: {limit}]",
                verdict(ok)
            )
            .unwrap();
        }
        writeln!(report, "rss      {}", memory.series()).unwrap();
        (report, passed)
    }
}

fn verdict(passed: bool) -> &'static str {
    if passed { "PASS" } else { "FAIL" }
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// How late each frame went out, in 0.1 ms buckets, so hours of frames take constant memory.
struct Lateness {
    counts: Vec<u64>,
    max: Duration,
}

impl Default for Lateness {
    fn default() -> Self {
        Self {
            counts: vec![0; LATENESS_BUCKETS],
            max: Duration::ZERO,
        }
    }
}

impl Lateness {
    fn bucket(lateness: Duration) -> usize {
        let bucket = lateness.as_nanos() / LATENESS_BUCKET.as_nanos();
        usize::try_from(bucket).map_or(LATENESS_BUCKETS - 1, |bucket| {
            bucket.min(LATENESS_BUCKETS - 1)
        })
    }

    fn record(&mut self, lateness: Duration) {
        self.counts[Self::bucket(lateness)] += 1;
        self.max = self.max.max(lateness);
    }

    /// The lateness `fraction` of frames are within, rounded down to 0.1 ms.
    fn percentile(&self, fraction: f64) -> Duration {
        let total: u64 = self.counts.iter().sum();
        let rank = ((total as f64 * fraction).ceil() as u64).max(1);
        let mut seen = 0;
        let bucket = self
            .counts
            .iter()
            .position(|count| {
                seen += count;
                seen >= rank
            })
            .unwrap_or(0);
        LATENESS_BUCKET * bucket as u32
    }

    fn at_least(&self, lateness: Duration) -> u64 {
        self.counts[Self::bucket(lateness)..].iter().sum()
    }
}

/// Samples the process's RSS on its own thread. In real time the render loop goes on
/// meanwhile, so measuring never delays a frame; on the fake clock it waits, or it would race
/// ahead of the samples.
struct MemorySampler {
    requests: Sender<u32>,
    sampled: Receiver<()>,
    is_waiting: bool,
    sampler: JoinHandle<Result<Vec<(u32, u64)>, String>>,
}

impl MemorySampler {
    fn start(is_waiting: bool) -> Self {
        let (requests, frames) = mpsc::channel();
        let (done, sampled) = mpsc::channel();
        let sampler = thread::spawn(move || {
            let mut samples = Vec::new();
            for frame in frames {
                samples.push((frame, rss_kib()?));
                let _ = done.send(());
            }
            Ok(samples)
        });
        Self {
            requests,
            sampled,
            is_waiting,
            sampler,
        }
    }

    fn sample(&self, frame: u32) {
        // Both fail only once sampling has failed, which `finish` reports.
        if self.requests.send(frame).is_ok() && self.is_waiting {
            let _ = self.sampled.recv();
        }
    }

    /// Each sample: the frame it was asked at, and RSS in KiB.
    fn finish(self) -> Result<Vec<(u32, u64)>, String> {
        drop(self.requests);
        self.sampler.join().expect("the RSS sampler never panics")
    }
}

/// RSS in KiB, from `ps`: the same command on macOS and Linux, and no unsafe code.
fn rss_kib() -> Result<u64, String> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .map_err(|error| format!("cannot run ps to measure memory: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse().map_err(|_| {
        format!(
            "ps gave no RSS: {:?} {}",
            text.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })
}

struct MemoryCheck {
    samples: Result<Vec<(u32, u64)>, String>,
    /// Frame of the baseline: the start of the reference pass.
    baseline_frame: u32,
}

impl MemoryCheck {
    fn from(samples: Result<Vec<(u32, u64)>, String>, baseline_frame: u32) -> Self {
        Self {
            samples,
            baseline_frame,
        }
    }

    /// RSS at the baseline and the most it reached after, in KiB.
    fn baseline_and_peak(&self) -> Option<(u64, u64)> {
        let samples = self.samples.as_ref().ok()?;
        let at = samples
            .iter()
            .position(|(frame, _)| *frame >= self.baseline_frame)?;
        let peak = samples[at..].iter().map(|(_, kib)| *kib).max()?;
        Some((samples[at].1, peak))
    }

    fn is_flat(&self) -> bool {
        self.baseline_and_peak().is_some_and(|(baseline, peak)| {
            peak.saturating_sub(baseline) <= MEMORY_GROWTH_LIMIT_KIB
        })
    }

    fn summary(&self) -> String {
        match (&self.samples, self.baseline_and_peak()) {
            (Err(error), _) => format!("not measured: {error}"),
            (Ok(_), None) => "not measured: no sample after the first pass".into(),
            (Ok(_), Some((baseline, peak))) => format!(
                "RSS {} after pass {REFERENCE_PASS}, at most {} after that, growth {}",
                mib(baseline),
                mib(peak),
                mib(peak.saturating_sub(baseline))
            ),
        }
    }

    /// Every sample as `hours: MiB`.
    fn series(&self) -> String {
        let Ok(samples) = &self.samples else {
            return "not measured".into();
        };
        samples
            .iter()
            .map(|(frame, kib)| {
                let hours = (FRAME_PERIOD * *frame).as_secs_f64() / 3600.0;
                format!("{hours:.2} h: {:.1}", *kib as f64 / 1024.0)
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

fn mib(kib: u64) -> String {
    format!("{:.2} MiB", kib as f64 / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_come_from_the_buckets_and_late_frames_are_counted() {
        let mut lateness = Lateness::default();
        for _ in 0..98 {
            lateness.record(Duration::from_micros(250));
        }
        lateness.record(Duration::from_millis(4));
        lateness.record(Duration::from_millis(30));

        assert_eq!(lateness.percentile(0.5), Duration::from_micros(200));
        assert_eq!(lateness.percentile(0.99), Duration::from_millis(4));
        assert_eq!(lateness.max, Duration::from_millis(30));
        assert_eq!(lateness.at_least(FRAME_PERIOD), 1);
    }

    #[test]
    fn songs_start_on_frames_with_a_gap_and_passes_loop() {
        let setlist = Setlist {
            path: PathBuf::new(),
            show_dir: PathBuf::new(),
            songs: vec![
                SetlistSong {
                    path: PathBuf::new(),
                    midi: Vec::new(),
                    first_frame: 0,
                },
                SetlistSong {
                    path: PathBuf::new(),
                    midi: Vec::new(),
                    first_frame: 100,
                },
            ],
            pass_frames: 180,
        };
        let slot = |frame| {
            let slot = setlist.slot(frame);
            (slot.pass, slot.song, slot.first_frame)
        };

        assert_eq!(slot(0), (0, 0, 0));
        assert_eq!(slot(99), (0, 0, 0));
        assert_eq!(slot(100), (0, 1, 100));
        assert_eq!(slot(179), (0, 1, 100));
        assert_eq!(slot(180), (1, 0, 180));
        assert_eq!(slot(385), (2, 0, 360));
        assert_eq!(frames_in(Duration::from_millis(1001)), 41);
    }
}
