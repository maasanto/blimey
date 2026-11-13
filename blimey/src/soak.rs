//! `blimey soak`: loops a setlist of recorded songs through the render loop for hours and checks
//! it holds up (plan §15): no dropped frames, no drift, low jitter, flat memory.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use blimey_engine::look::Look;
use blimey_engine::{
    ArtNetFrames, Clock, DmxSink, FRAME_PERIOD, Frame, MidiEvent, MidiFile, MidiSource, ShowSource,
    Status,
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
    midi: MidiFile,
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
            let midi = MidiFile::open(&song_path).map_err(|error| error.to_string())?;
            let song_frames = frames_in(midi.duration() + SONG_GAP);
            songs.push(SetlistSong {
                path: song_path,
                midi,
                first_frame: pass_frames,
            });
            // Starting each song on a frame keeps its frames comparable from pass to pass.
            pass_frames += song_frames;
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
            self.playing = Some((slot, self.setlist.songs[slot.song].midi.clone()));
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
    /// Where to write the report; by default a new `soak-….txt` in the current folder.
    pub report: Option<PathBuf>,
}

impl<S: DmxSink> Soak<'_, S> {
    /// Runs the soak, prints the report and writes it. Returns whether every check passed.
    pub fn run(
        self,
        clock: &mut impl Clock,
        shows: impl ShowSource,
    ) -> Result<bool, Box<dyn Error>> {
        let frame_count = self.frame_count()?;
        let path = self.report.clone().unwrap_or_else(|| self.default_report());
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
        let mut sink = Measured::new(self.setlist, self.output, self.device, frame_count);
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
        sink.end();
        let (report, passed) = sink.report(mode, elapsed);
        print!("{report}");
        if let Some(folder) = path
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
        {
            fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        }
        fs::write(&path, &report).map_err(|error| format!("{}: {error}", path.display()))?;
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

    /// `soak-<setlist>-<mode>-<unix time>-<process>.txt`: the process id keeps two soaks
    /// started in the same second apart.
    fn default_report(&self) -> PathBuf {
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
        PathBuf::from(format!(
            "soak-{name}-{mode}-{since_epoch}-{}.txt",
            std::process::id()
        ))
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
    /// Fingerprint of the Art-Net universes of the frame about to be sent.
    artnet: u64,
    /// Frames whose Art-Net universes failed to send, and the first error.
    artnet_failures: u64,
    first_artnet_failure: Option<String>,
    drift: DriftCheck,
    memory: MemoryCheck,
    sample_every: u32,
}

impl<'a, S> Measured<'a, S> {
    fn new(
        setlist: &'a Setlist,
        output: S,
        device: Option<&'a EmulatedEnttec>,
        frame_count: u32,
    ) -> Self {
        Self {
            setlist,
            output,
            device,
            sent: 0,
            received: 0,
            lateness: Lateness::default(),
            artnet: 0,
            artnet_failures: 0,
            first_artnet_failure: None,
            drift: DriftCheck::default(),
            memory: MemoryCheck::new(setlist.pass_frames * REFERENCE_PASS),
            sample_every: (frame_count / RSS_SAMPLES).max(1),
        }
    }
}

impl<S: DmxSink> DmxSink for Measured<'_, S> {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        let index = self.sent;
        self.lateness
            .record(at.saturating_sub(FRAME_PERIOD * index));
        // Every frame, so the frames the box keeps never weigh in the memory samples.
        if let Some(device) = self.device {
            self.received += device.take_received().len();
        }
        let slot = self.setlist.slot(index);
        if index == self.memory.baseline_frame || index.is_multiple_of(self.sample_every) {
            self.memory.sample(index);
        }
        self.drift
            .frame(slot, frame, std::mem::take(&mut self.artnet));
        self.sent += 1;
        self.output.send(at, frame)
    }

    /// The render loop calls it before each `send`; the universes count towards drift too, and
    /// a frame whose universes fail to send counts as dropped.
    // ponytail: the universes are stashed until `send` because the trait splits a frame in two
    // calls; `DmxSink::send(&Universes)` would remove the stash, a cross-crate change.
    fn send_artnet(&mut self, universes: &ArtNetFrames) -> io::Result<()> {
        let mut hash = DefaultHasher::new();
        universes.hash(&mut hash);
        self.artnet = hash.finish();
        if let Err(error) = self.output.send_artnet(universes) {
            self.artnet_failures += 1;
            self.first_artnet_failure
                .get_or_insert_with(|| error.to_string());
        }
        Ok(())
    }

    fn status(&mut self, status: &Status) {
        self.output.status(status);
    }
}

impl<S> Measured<'_, S> {
    /// After the last frame: takes the last memory sample, finishes the last song if it played
    /// to its end, and waits up to a second for the emulated box to read the last frames.
    fn end(&mut self) {
        self.memory.sample(self.sent);
        self.drift.end(self.setlist, self.sent);
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
        let device_failure = self.device.and_then(EmulatedEnttec::failure);
        let missed = self
            .device
            .map_or(0, |_| (self.sent as usize).saturating_sub(self.received));
        let dropped = late + missed as u64 + self.artnet_failures;
        let mut device_note = match (self.device, &device_failure) {
            (Some(_), Some(error)) => format!(
                "; the emulated Enttec received {} of them, then stopped reading: {error}",
                self.received
            ),
            (Some(_), None) => format!("; the emulated Enttec received {} of them", self.received),
            (None, _) => String::new(),
        };
        if let Some(error) = &self.first_artnet_failure {
            write!(
                device_note,
                "; Art-Net failed to send on {} of them: {error}",
                self.artnet_failures
            )
            .unwrap();
        }
        let p99 = self.lateness.percentile(0.99);
        let drift = &self.drift;
        let first_drift = drift.first.map_or_else(String::new, |(pass, song)| {
            format!(
                " (first: pass {}, {})",
                pass + 1,
                setlist.songs[song].path.display()
            )
        });
        let checks = [
            (
                "frames",
                dropped == 0 && device_failure.is_none(),
                format!(
                    "{} rendered, {dropped} dropped (sent a frame period or more late, never received, or failed on Art-Net){device_note}",
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
                drift.drifted == 0 && drift.compared > 0,
                format!(
                    "{} songs compared with their render in pass {}, {} differ{first_drift}",
                    drift.compared,
                    REFERENCE_PASS + 1,
                    drift.drifted,
                ),
                "none differ".to_string(),
            ),
            (
                "memory",
                self.memory.is_flat(),
                self.memory.summary(),
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
        writeln!(report, "rss      {}", self.memory.series()).unwrap();
        (report, passed)
    }
}

fn verdict(passed: bool) -> &'static str {
    if passed { "PASS" } else { "FAIL" }
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// Fingerprints each song's frames, and compares each song after the reference pass with the
/// same song in it.
#[derive(Default)]
struct DriftCheck {
    song: Option<Slot>,
    hash: DefaultHasher,
    /// Fingerprint of each song in the reference pass.
    reference: Vec<u64>,
    compared: u32,
    drifted: u32,
    /// Pass and song of the first song that differed.
    first: Option<(u32, usize)>,
}

impl DriftCheck {
    /// One frame: the Enttec universe, and the fingerprint of its Art-Net universes.
    fn frame(&mut self, slot: Slot, frame: &Frame, artnet: u64) {
        if self.song != Some(slot) {
            self.finish_song();
            self.song = Some(slot);
        }
        frame.hash(&mut self.hash);
        artnet.hash(&mut self.hash);
    }

    /// The run stopped after `frames` frames: the last song counts only if it played to its
    /// end, i.e. the next frame would have started another song.
    fn end(&mut self, setlist: &Setlist, frames: u32) {
        if self.song.is_some_and(|slot| setlist.slot(frames) != slot) {
            self.finish_song();
        }
        self.song = None;
    }

    fn finish_song(&mut self) {
        let Some(slot) = self.song else {
            return;
        };
        let fingerprint = std::mem::take(&mut self.hash).finish();
        if slot.pass == REFERENCE_PASS {
            self.reference.push(fingerprint);
        } else if slot.pass > REFERENCE_PASS {
            self.compared += 1;
            if self.reference.get(slot.song) != Some(&fingerprint) {
                self.drifted += 1;
                self.first.get_or_insert((slot.pass, slot.song));
            }
        }
    }
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

/// RSS over the run, against its level at the start of the reference pass.
struct MemoryCheck {
    /// The frame each sample was taken at, and RSS in KiB.
    samples: Vec<(u32, u64)>,
    error: Option<String>,
    baseline_frame: u32,
}

impl MemoryCheck {
    fn new(baseline_frame: u32) -> Self {
        Self {
            samples: Vec::new(),
            error: None,
            baseline_frame,
        }
    }

    fn sample(&mut self, frame: u32) {
        match rss_kib() {
            Ok(kib) => self.samples.push((frame, kib)),
            Err(error) => {
                self.error.get_or_insert(error);
            }
        }
    }

    /// RSS at the baseline and the most it reached after, in KiB.
    fn baseline_and_peak(&self) -> Option<(u64, u64)> {
        let at = self
            .samples
            .iter()
            .position(|(frame, _)| *frame >= self.baseline_frame)?;
        let peak = self.samples[at..].iter().map(|(_, kib)| *kib).max()?;
        Some((self.samples[at].1, peak))
    }

    fn is_flat(&self) -> bool {
        self.error.is_none()
            && self.baseline_and_peak().is_some_and(|(baseline, peak)| {
                peak.saturating_sub(baseline) <= MEMORY_GROWTH_LIMIT_KIB
            })
    }

    fn summary(&self) -> String {
        match (&self.error, self.baseline_and_peak()) {
            (Some(error), _) => format!("not measured: {error}"),
            (None, None) => "not measured: no sample after the first pass".into(),
            (None, Some((baseline, peak))) => format!(
                "RSS {} after pass {REFERENCE_PASS}, at most {} after that, growth {}",
                mib(baseline),
                mib(peak),
                mib(peak.saturating_sub(baseline))
            ),
        }
    }

    /// Every sample as `hours: MiB`.
    fn series(&self) -> String {
        self.samples
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

/// This process's resident memory in KiB, from the kernel: cheap enough to sample on the render
/// loop.
#[cfg(target_os = "macos")]
fn rss_kib() -> Result<u64, String> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::zeroed();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: task_info writes at most `count` words into `info`, which is exactly that size.
    // libc suggests the mach2 crate for mach_task_self; it is the same call.
    #[allow(deprecated)]
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return Err(format!("task_info failed with kern_return_t {result}"));
    }
    // SAFETY: task_info succeeded, so it filled `info`.
    Ok(unsafe { info.assume_init() }.resident_size / 1024)
}

#[cfg(target_os = "linux")]
fn rss_kib() -> Result<u64, String> {
    let statm = fs::read_to_string("/proc/self/statm")
        .map_err(|error| format!("cannot read /proc/self/statm: {error}"))?;
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|pages| pages.parse().ok())
        .ok_or_else(|| format!("/proc/self/statm has no resident size: {statm:?}"))?;
    // SAFETY: sysconf only reads a system setting.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Ok(pages * page_size as u64 / 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rss_kib() -> Result<u64, String> {
    Err("reading RSS is only implemented on macOS and Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence() -> MidiFile {
        MidiFile::parse(&MidiFile::encode(&[]).unwrap()).unwrap()
    }

    fn setlist(first_frames: &[u32], pass_frames: u32) -> Setlist {
        Setlist {
            path: PathBuf::new(),
            show_dir: PathBuf::new(),
            songs: first_frames
                .iter()
                .map(|&first_frame| SetlistSong {
                    path: PathBuf::new(),
                    midi: silence(),
                    first_frame,
                })
                .collect(),
            pass_frames,
        }
    }

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
        let setlist = setlist(&[0, 100], 180);
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

    /// Plays `passes` passes of a one-song setlist, the frame value given by `value`.
    fn drift_over(passes: u32, value: impl Fn(u32) -> u8) -> DriftCheck {
        let setlist = setlist(&[0], 4);
        let mut drift = DriftCheck::default();
        let frames = passes * setlist.pass_frames;
        for index in 0..frames {
            drift.frame(setlist.slot(index), &[value(index); 512], 0);
        }
        drift.end(&setlist, frames);
        drift
    }

    #[test]
    fn a_one_song_setlist_over_exactly_three_passes_compares_its_last_pass() {
        let drift = drift_over(3, |_| 7);

        assert_eq!((drift.compared, drift.drifted), (1, 0));
    }

    #[test]
    fn a_song_rendering_differently_from_the_reference_pass_is_drift() {
        let drift = drift_over(4, |index| u8::from(index == 13));

        assert_eq!((drift.compared, drift.drifted), (2, 1));
        assert_eq!(drift.first, Some((3, 0)), "pass 4, the only song");
    }

    #[test]
    fn art_net_universes_reach_the_output_and_count_towards_drift() {
        let setlist = setlist(&[0], 4);
        let mut sink = Measured::new(&setlist, blimey_engine::FakeDmx::default(), None, 16);

        for index in 0..16 {
            let universe = [u8::from(index == 13); 512];
            sink.send_artnet(&ArtNetFrames::from([(3, universe)]))
                .unwrap();
            sink.send(FRAME_PERIOD * index, &[0; 512]).unwrap();
        }
        sink.drift.end(&setlist, 16);

        assert_eq!(sink.output.frames[13].artnet[&3][0], 1);
        assert_eq!(
            (sink.drift.compared, sink.drift.drifted),
            (2, 1),
            "only Art-Net universe 3 differs, in pass 4"
        );
    }

    /// An Art-Net node that is gone: every send of the grids' universes fails.
    struct UnpluggedNode;

    impl DmxSink for UnpluggedNode {
        fn send(&mut self, _at: Duration, _frame: &Frame) -> io::Result<()> {
            Ok(())
        }

        fn send_artnet(&mut self, _universes: &ArtNetFrames) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::HostUnreachable,
                "no route to host",
            ))
        }
    }

    #[test]
    fn frames_whose_art_net_fails_to_send_are_dropped() {
        let setlist = setlist(&[0], 4);
        let mut sink = Measured::new(&setlist, UnpluggedNode, None, 12);

        for index in 0..12 {
            sink.send_artnet(&ArtNetFrames::from([(3, [0; 512])]))
                .unwrap();
            sink.send(FRAME_PERIOD * index, &[0; 512]).unwrap();
        }
        sink.end();
        let (report, passed) = sink.report("accelerated", Duration::ZERO);

        assert!(!passed, "{report}");
        assert!(
            report.contains("12 dropped") && report.contains("no route to host"),
            "{report}"
        );
    }

    #[test]
    fn a_song_cut_short_by_the_end_of_the_run_is_not_compared() {
        let setlist = setlist(&[0], 4);
        let mut drift = DriftCheck::default();
        for index in 0..10 {
            drift.frame(setlist.slot(index), &[0; 512], 0);
        }
        drift.end(&setlist, 10);

        assert_eq!(drift.compared, 0, "pass 3 stopped after 2 of its 4 frames");
    }

    #[test]
    fn rss_follows_memory_the_process_touches() {
        const TOUCHED: usize = 64 << 20;
        let before = rss_kib().unwrap();

        let touched = vec![1u8; TOUCHED];
        let after = rss_kib().unwrap();

        assert!(
            after >= before + (TOUCHED as u64 / 1024) / 2,
            "{before} KiB, then {after} KiB with 64 MiB touched"
        );
        drop(touched);
    }

    #[test]
    fn the_soak_counts_frames_as_the_emulated_box_reads_them_not_at_song_ends() {
        let device = EmulatedEnttec::plug_in().unwrap();
        let output = crate::enttec::EnttecPro::new(device.path());
        let setlist = setlist(&[0], 1_000_000);
        let mut sink = Measured::new(&setlist, output, Some(&device), 1_000_000);

        for _ in 0..100 {
            sink.send(Duration::ZERO, &[0; 512]).unwrap();
            thread::sleep(Duration::from_millis(2));
        }

        assert!(
            sink.received >= 50,
            "received {} of 100 within the song",
            sink.received
        );
    }
}
