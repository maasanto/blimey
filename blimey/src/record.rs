//! `blimey record`: captures what the DAW sends on the MIDI port to a Standard MIDI File for
//! replay tests (plan §9), and optionally a controller's Art-Net output, such as EMU's, on the
//! same timeline for parity checks (plan §15).

use std::error::Error;
use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Write};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use blimey_engine::{
    ArtNetFrames, Clock, DMX_CHANNELS, FRAME_PERIOD, MidiEvent, MidiFile, MidiSource,
    MonotonicClock, RecordedFrame, Universe, Universes, format_dump,
};
use tokio::signal::unix::{SignalKind, signal};

use crate::artnet;
use crate::midi_input::MidiInput;
use crate::state::write_atomically;

const START: u8 = 0xFA;
const CONTINUE: u8 = 0xFB;
const STOP: u8 = 0xFC;
/// Events are stamped as they arrive, so polling this often only bounds how late a stop lands.
const POLL_INTERVAL: Duration = Duration::from_millis(1);
/// How much of a take a crash or `kill -9` can lose: the files are saved this often.
const SAVE_INTERVAL: Duration = Duration::from_secs(5);

pub struct Recording<'a> {
    pub midi_port: &'a str,
    pub until_stop: bool,
    pub artnet: Option<ArtNetRecording<'a>>,
}

/// What to keep of the Art-Net received alongside the MIDI, and where.
pub struct ArtNetRecording<'a> {
    pub dump: &'a Path,
    /// The universe that stands for the Enttec box, as `run` would mirror it
    /// (`artnet::enttec_universe`); `None` when a pixel grid has universe 0.
    pub enttec_universe: Option<u16>,
    /// The show's pixel-grid universes. Packets for any other universe are ignored.
    pub grid_universes: Vec<u16>,
}

/// Records until Ctrl-C, SIGTERM or SIGHUP (or the first MIDI Stop after a Start, with
/// `until_stop`), saving to `out` every few seconds and at the end. Time 0 is the first MIDI
/// message, in both files.
pub fn record(out: &Path, recording: &Recording) -> Result<(), Box<dyn Error>> {
    let stopped = stop_signals()?;
    let clock = MonotonicClock::default();
    let artnet = recording
        .artnet
        .as_ref()
        .map(|artnet| ArtNetTake::start(artnet, clock))
        .transpose()?;
    let mut midi = MidiInput::connect(recording.midi_port, clock)?;
    eprintln!(
        "record: recording to {}; Ctrl-C to stop{}",
        out.display(),
        if recording.until_stop {
            ", or press stop in the DAW"
        } else {
            ""
        }
    );
    let mut take = Take {
        out,
        events: Vec::new(),
        artnet,
    };
    take.record(&mut midi, &clock, &stopped, recording.until_stop)?;
    if let Some(artnet) = &mut take.artnet {
        artnet.take_packets();
        artnet.capture.end();
    }
    take.save()?;
    let (Some(first), Some(last)) = (take.events.first(), take.events.last()) else {
        return Err(format!(
            "no MIDI received on the port matching {:?}; nothing written",
            recording.midi_port
        )
        .into());
    };
    eprintln!(
        "record: {} MIDI events over {:.1} s written to {}",
        take.events.len(),
        (last.at - first.at).as_secs_f64(),
        out.display()
    );
    if let Some(artnet) = &take.artnet {
        eprintln!(
            "record: {} Art-Net frames written to {}",
            artnet.frames_written,
            artnet.dump.display()
        );
        if let Some(failure) = artnet.failure.lock().unwrap().as_ref() {
            return Err(format!(
                "the Art-Net capture in {} is INCOMPLETE: it stopped early: {failure}",
                artnet.dump.display()
            )
            .into());
        }
    }
    Ok(())
}

struct Take<'a> {
    out: &'a Path,
    /// Stamped on the recording's clock; saved timed from the first one.
    events: Vec<MidiEvent>,
    artnet: Option<ArtNetTake<'a>>,
}

impl Take<'_> {
    fn record(
        &mut self,
        midi: &mut MidiInput,
        clock: &MonotonicClock,
        stopped: &AtomicBool,
        until_stop: bool,
    ) -> Result<(), String> {
        let mut is_playing = false;
        let mut saved = Instant::now();
        while !stopped.load(Ordering::Relaxed) {
            if let Some(artnet) = &mut self.artnet {
                artnet.take_packets();
            }
            while let Some(event) = midi.poll(clock.now()) {
                if let Some(artnet) = &mut self.artnet {
                    artnet.capture.start.get_or_insert(event.at);
                }
                let is_stop = event.message == [STOP];
                is_playing |= event.message == [START] || event.message == [CONTINUE];
                self.events.push(event);
                // The DAW may send Stop before it starts playing, e.g. when seeking.
                if until_stop && is_playing && is_stop {
                    return Ok(());
                }
            }
            if saved.elapsed() >= SAVE_INTERVAL {
                self.save()?;
                saved = Instant::now();
            }
            thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    /// Rewrites the MIDI file with the whole take so far, atomically, and appends the Art-Net
    /// frames finished since the last save, so a crash or kill leaves the take as of the last
    /// save. Writes nothing before the first MIDI message.
    fn save(&mut self) -> Result<(), String> {
        let Some(start) = self.events.first().map(|event| event.at) else {
            return Ok(());
        };
        let events: Vec<MidiEvent> = self
            .events
            .iter()
            .map(|event| MidiEvent {
                at: event.at - start,
                message: event.message.clone(),
            })
            .collect();
        let in_out = |error: io::Error| format!("{}: {error}", self.out.display());
        write_atomically(self.out, MidiFile::encode(&events).map_err(in_out)?).map_err(in_out)?;
        if let Some(artnet) = &mut self.artnet {
            artnet
                .save()
                .map_err(|error| format!("{}: {error}", artnet.dump.display()))?;
        }
        Ok(())
    }
}

/// Set on Ctrl-C, SIGTERM or SIGHUP (the terminal closing), so the take is saved rather than
/// lost. The handlers are in place when this returns.
fn stop_signals() -> io::Result<Arc<AtomicBool>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut signals = {
        let _context = runtime.enter();
        [
            signal(SignalKind::interrupt())?,
            signal(SignalKind::terminate())?,
            signal(SignalKind::hangup())?,
        ]
    };
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    thread::spawn(move || {
        let [interrupt, terminate, hang_up] = &mut signals;
        runtime.block_on(async {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
                _ = hang_up.recv() => {}
            }
        });
        flag.store(true, Ordering::Relaxed);
    });
    Ok(stopped)
}

/// One ArtDmx packet as received, stamped on the recording's clock.
struct Packet {
    at: Duration,
    universe: u16,
    channels: Vec<u8>,
}

/// The Art-Net side of a take: a thread receives packets into `received`, which the recording
/// loop swaps out and turns into frames, so the receiver never waits on frames or files.
struct ArtNetTake<'a> {
    dump: &'a Path,
    received: Arc<Mutex<Vec<Packet>>>,
    /// Why the receiver stopped before the end of the take, if it did.
    failure: Arc<Mutex<Option<String>>>,
    capture: Capture,
    /// Whether a save wrote the dump's header; later saves append.
    is_dump_started: bool,
    frames_written: usize,
}

impl<'a> ArtNetTake<'a> {
    fn start(recording: &ArtNetRecording<'a>, clock: MonotonicClock) -> Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, artnet::PORT)).map_err(|error| {
            format!(
                "cannot listen for Art-Net on UDP {}: {error}; quit BlenderDMX or any other Art-Net receiver",
                artnet::PORT
            )
        })?;
        let take = Self {
            dump: recording.dump,
            received: Arc::default(),
            failure: Arc::default(),
            capture: Capture::new(recording.enttec_universe, &recording.grid_universes),
            is_dump_started: false,
            frames_written: 0,
        };
        let (received, failure) = (take.received.clone(), take.failure.clone());
        thread::spawn(move || {
            let mut packet = [0; 1024];
            loop {
                let size = match socket.recv(&mut packet) {
                    Ok(size) => size,
                    // ICMP errors from earlier sends and signals: the socket is still good.
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::Interrupted
                                | ErrorKind::ConnectionRefused
                                | ErrorKind::ConnectionReset
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => {
                        eprintln!(
                            "record: Art-Net capture STOPPED: {error}; the dump ends here, MIDI keeps recording"
                        );
                        *failure.lock().unwrap() = Some(error.to_string());
                        return;
                    }
                };
                if let Some((universe, channels)) = artnet::parse_art_dmx(&packet[..size]) {
                    let at = clock.now();
                    received.lock().unwrap().push(Packet {
                        at,
                        universe,
                        channels: channels.to_vec(),
                    });
                }
            }
        });
        Ok(take)
    }

    fn take_packets(&mut self) {
        let packets = std::mem::take(&mut *self.received.lock().unwrap());
        for packet in packets {
            self.capture
                .packet(packet.at, packet.universe, &packet.channels);
        }
    }

    /// Writes the dump with its header on the first save, then appends the frames finished
    /// since.
    fn save(&mut self) -> io::Result<()> {
        let frames = self.capture.take_frames();
        let dump = format_dump(&frames);
        if self.is_dump_started {
            let lines = dump.split_once('\n').map_or("", |(_, lines)| lines);
            OpenOptions::new()
                .append(true)
                .open(self.dump)?
                .write_all(lines.as_bytes())?;
        } else {
            write_atomically(self.dump, &dump)?;
            self.is_dump_started = true;
        }
        self.frames_written += frames.len();
        Ok(())
    }
}

/// Turns Art-Net packets into frames on the render loop's 25 ms grid, in Blimey's own layout:
/// the Enttec stand-in's universe as the Enttec frame, the grids' universes as Art-Net, any
/// other universe ignored. Frame N holds the latest values received by N × 25 ms, as Blimey's
/// frame N holds the MIDI received by then; frames without packets get no line.
struct Capture {
    enttec_universe: Option<u16>,
    grid_universes: Vec<u16>,
    /// The take's time 0 on the recording's clock: its first MIDI message. Packets before it
    /// only set the values the first frame starts from.
    start: Option<Duration>,
    latest: Universes,
    /// The frame the latest packets fall in, until a packet for a later one finishes it.
    open_frame: Option<u32>,
    finished: Vec<RecordedFrame>,
}

impl Capture {
    fn new(enttec_universe: Option<u16>, grid_universes: &[u16]) -> Self {
        Self {
            enttec_universe,
            grid_universes: grid_universes.to_vec(),
            start: None,
            latest: Universes {
                enttec: [0; DMX_CHANNELS],
                artnet: ArtNetFrames::new(),
            },
            open_frame: None,
            finished: Vec::new(),
        }
    }

    /// One packet received at `at`, on the recording's clock. A packet shorter than the last
    /// one for its universe leaves the channels after it at 0, as a receiver would.
    fn packet(&mut self, at: Duration, universe: u16, channels: &[u8]) {
        let target = if self.enttec_universe == Some(universe) {
            Universe::Enttec
        } else if self.grid_universes.contains(&universe) {
            Universe::ArtNet(universe)
        } else {
            return;
        };
        if let Some(since_start) = self.start.and_then(|start| at.checked_sub(start)) {
            let frame = since_start.as_nanos().div_ceil(FRAME_PERIOD.as_nanos());
            let frame = u32::try_from(frame).unwrap_or(u32::MAX);
            if self.open_frame.is_some_and(|open| open != frame) {
                self.finish_frame();
            }
            self.open_frame = Some(frame);
        }
        let values = self.latest.frame_mut(target);
        let length = channels.len().min(DMX_CHANNELS);
        values[..length].copy_from_slice(&channels[..length]);
        values[length..].fill(0);
    }

    fn finish_frame(&mut self) {
        if let Some(frame) = self.open_frame.take() {
            self.finished.push(RecordedFrame {
                at: FRAME_PERIOD * frame,
                values: self.latest.enttec,
                artnet: self.latest.artnet.clone(),
            });
        }
    }

    /// The take is over: the frame still filling is finished too.
    fn end(&mut self) {
        self.finish_frame();
    }

    /// The frames finished since the last call.
    fn take_frames(&mut self) -> Vec<RecordedFrame> {
        std::mem::take(&mut self.finished)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);
    /// The take starts at 1 s on the recording's clock.
    const START: Duration = Duration::from_secs(1);

    fn capture(enttec_universe: Option<u16>, grid_universes: &[u16]) -> Capture {
        let mut capture = Capture::new(enttec_universe, grid_universes);
        capture.start = Some(START);
        capture
    }

    fn frames(mut capture: Capture) -> Vec<RecordedFrame> {
        capture.end();
        capture.take_frames()
    }

    #[test]
    fn a_non_zero_stand_in_universe_is_the_enttec_frame() {
        let mut capture = capture(Some(5), &[]);

        capture.packet(START + 10 * MS, 5, &[1, 2]);

        let frames = frames(capture);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].at, 25 * MS, "10 ms in: frame 1, due at 25 ms");
        assert_eq!(frames[0].values[..3], [1, 2, 0]);
        assert!(frames[0].artnet.is_empty());
    }

    #[test]
    fn a_shorter_repeat_packet_zero_fills_the_rest() {
        let mut capture = capture(Some(0), &[]);

        capture.packet(START + 10 * MS, 0, &[9; 512]);
        capture.packet(START + 30 * MS, 0, &[7, 7]);

        let frames = frames(capture);
        assert_eq!(frames[0].values[511], 9);
        assert_eq!(frames[1].values[..3], [7, 7, 0]);
        assert_eq!(frames[1].values[511], 0);
    }

    #[test]
    fn packets_in_one_frame_give_one_line_with_the_latest_of_each_universe() {
        let mut capture = capture(Some(0), &[2]);

        capture.packet(START + 26 * MS, 0, &[1]);
        capture.packet(START + 30 * MS, 2, &[3]);
        capture.packet(START + 49 * MS, 0, &[4]);

        let frames = frames(capture);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].at, 50 * MS);
        assert_eq!(frames[0].values[0], 4);
        assert_eq!(frames[0].artnet[&2][0], 3);
    }

    #[test]
    fn universes_the_show_does_not_use_are_ignored() {
        let mut capture = capture(Some(0), &[2]);

        capture.packet(START + 10 * MS, 7, &[255]);

        assert!(frames(capture).is_empty(), "a desk on universe 7");
    }

    #[test]
    fn a_grid_on_universe_0_keeps_it_when_the_stand_in_gives_way() {
        let enttec = artnet::enttec_universe(None, &[0]).unwrap();
        let mut capture = capture(enttec, &[0]);

        capture.packet(START + 10 * MS, 0, &[8]);

        let frames = frames(capture);
        assert_eq!(frames[0].values[0], 0, "not written as Enttec channels");
        assert_eq!(frames[0].artnet[&0][0], 8);
        assert!(artnet::enttec_universe(Some(0), &[0]).is_err());
    }

    #[test]
    fn packets_before_the_first_midi_only_set_the_starting_values() {
        let mut capture = Capture::new(Some(0), &[]);
        capture.packet(Duration::ZERO, 0, &[6]);
        capture.start = Some(START);

        capture.packet(START + 30 * MS, 0, &[6, 1]);

        let frames = frames(capture);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].values[..2], [6, 1]);
    }
}
