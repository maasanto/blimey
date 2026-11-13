//! `blimey record`: captures what Reaper sends on the MIDI port to a Standard MIDI File for
//! replay tests (plan §9), and optionally a controller's Art-Net output, such as EMU's, on the
//! same timeline for parity checks (plan §15).

use std::error::Error;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use blimey_engine::{
    ArtNetFrames, Clock, DMX_CHANNELS, MidiEvent, MidiFile, MidiSource, MonotonicClock,
    RecordedFrame, format_dump,
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
/// How much of a take a crash or `kill -9` can lose: the files are rewritten this often.
const SAVE_INTERVAL: Duration = Duration::from_secs(5);

pub struct Recording<'a> {
    pub midi_port: &'a str,
    pub until_stop: bool,
    /// Where to write the Art-Net capture, and which universe to keep.
    pub artnet: Option<(&'a Path, u16)>,
}

/// Records until Ctrl-C, SIGTERM or SIGHUP (or the first MIDI Stop after a Start, with
/// `until_stop`), saving to `out` every few seconds and at the end. Time 0 is the first MIDI
/// message, in both files.
pub fn record(out: &Path, recording: &Recording) -> Result<(), Box<dyn Error>> {
    let stopped = stop_signals()?;
    let clock = MonotonicClock::default();
    let artnet = recording
        .artnet
        .map(|(dump, universe)| Ok::<_, String>((dump, ArtNetCapture::start(universe, clock)?)))
        .transpose()?;
    let mut midi = MidiInput::connect(recording.midi_port, clock)?;
    eprintln!(
        "record: recording to {}; Ctrl-C to stop{}",
        out.display(),
        if recording.until_stop {
            ", or press stop in Reaper"
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
    if let Some((dump, capture)) = &take.artnet {
        let frames = capture.frames.lock().unwrap().len();
        eprintln!(
            "record: {frames} Art-Net frames written to {}",
            dump.display()
        );
        if let Some(failure) = capture.failure.lock().unwrap().as_ref() {
            return Err(format!(
                "the Art-Net capture in {} is INCOMPLETE: it stopped early: {failure}",
                dump.display()
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
    artnet: Option<(&'a Path, ArtNetCapture)>,
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
            while let Some(event) = midi.poll(clock.now()) {
                let is_stop = event.message == [STOP];
                is_playing |= event.message == [START] || event.message == [CONTINUE];
                self.events.push(event);
                // Reaper may send Stop before it starts playing, e.g. when seeking.
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

    /// Rewrites each file with the whole take so far, atomically, so a crash or kill leaves the
    /// take as of the last save. Writes nothing before the first MIDI message.
    fn save(&self) -> Result<(), String> {
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
        if let Some((dump, capture)) = &self.artnet {
            write_atomically(dump, format_dump(&capture.since(start)))
                .map_err(|error| format!("{}: {error}", dump.display()))?;
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

/// The ArtDmx packets received on UDP 6454, stamped on the MIDI's clock, as frames in Blimey's
/// own layout: `enttec_universe` stands for the Enttec box, every other universe is Art-Net.
/// Each packet gives a frame holding the latest values of every universe seen so far.
struct ArtNetCapture {
    frames: Arc<Mutex<Vec<RecordedFrame>>>,
    /// Why the capture stopped before the end of the take, if it did.
    failure: Arc<Mutex<Option<String>>>,
}

impl ArtNetCapture {
    fn start(enttec_universe: u16, clock: MonotonicClock) -> Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, artnet::PORT)).map_err(|error| {
            format!(
                "cannot listen for Art-Net on UDP {}: {error}; quit BlenderDMX or any other Art-Net receiver",
                artnet::PORT
            )
        })?;
        let capture = Self {
            frames: Arc::default(),
            failure: Arc::default(),
        };
        let (frames, failure) = (capture.frames.clone(), capture.failure.clone());
        thread::spawn(move || {
            let mut packet = [0; 1024];
            let mut latest = RecordedFrame {
                at: Duration::ZERO,
                values: [0; DMX_CHANNELS],
                artnet: ArtNetFrames::new(),
            };
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
                let Some((packet_universe, channels)) = artnet::parse_art_dmx(&packet[..size])
                else {
                    continue;
                };
                let values = if packet_universe == enttec_universe {
                    &mut latest.values
                } else {
                    latest
                        .artnet
                        .entry(packet_universe)
                        .or_insert([0; DMX_CHANNELS])
                };
                values[..channels.len()].copy_from_slice(channels);
                values[channels.len()..].fill(0);
                latest.at = clock.now();
                frames.lock().unwrap().push(latest.clone());
            }
        });
        Ok(capture)
    }

    /// The frames from `start` on, timed from it.
    fn since(&self, start: Duration) -> Vec<RecordedFrame> {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter_map(|frame| {
                Some(RecordedFrame {
                    at: frame.at.checked_sub(start)?,
                    ..frame.clone()
                })
            })
            .collect()
    }
}
