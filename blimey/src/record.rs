//! `blimey record`: captures what Reaper sends on the MIDI port to a Standard MIDI File for
//! replay tests (plan §9), and optionally a controller's Art-Net output, such as EMU's, on the
//! same timeline for parity checks (plan §15).

use std::error::Error;
use std::fs;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use blimey_engine::{
    Clock, DMX_CHANNELS, MidiEvent, MidiFile, MidiSource, MonotonicClock, RecordedFrame,
    format_dump,
};

use crate::artnet;
use crate::midi_input::MidiInput;

const START: u8 = 0xFA;
const CONTINUE: u8 = 0xFB;
const STOP: u8 = 0xFC;
/// Events are stamped as they arrive, so polling this often only bounds how late Ctrl-C lands.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

pub struct Recording<'a> {
    pub midi_port: &'a str,
    pub until_stop: bool,
    /// Where to write the Art-Net capture, and which universe to keep.
    pub artnet: Option<(&'a Path, u16)>,
}

/// Records until Ctrl-C (or the first MIDI Stop after a Start, with `until_stop`), then writes
/// `out`. Time 0 is the first MIDI message, in both files.
pub fn record(out: &Path, recording: &Recording) -> Result<(), Box<dyn Error>> {
    let interrupted = ctrl_c()?;
    let clock = MonotonicClock::default();
    let capture = recording
        .artnet
        .map(|(_, universe)| ArtNetCapture::start(universe, clock))
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
    let events = take_events(&mut midi, &clock, &interrupted, recording.until_stop);
    let Some(start) = events.first().map(|event| event.at) else {
        return Err(format!(
            "no MIDI received on the port matching {:?}; nothing written",
            recording.midi_port
        )
        .into());
    };
    let events: Vec<MidiEvent> = events
        .into_iter()
        .map(|event| MidiEvent {
            at: event.at - start,
            message: event.message,
        })
        .collect();
    let in_out = |error: std::io::Error| format!("{}: {error}", out.display());
    fs::write(out, MidiFile::encode(&events).map_err(in_out)?).map_err(in_out)?;
    let length = events.last().map_or(Duration::ZERO, |event| event.at);
    eprintln!(
        "record: {} MIDI events over {:.1} s written to {}",
        events.len(),
        length.as_secs_f64(),
        out.display()
    );
    if let (Some(capture), Some((dump, _))) = (capture, recording.artnet) {
        let frames = capture.since(start);
        fs::write(dump, format_dump(&frames))
            .map_err(|error| format!("{}: {error}", dump.display()))?;
        eprintln!(
            "record: {} Art-Net frames written to {}",
            frames.len(),
            dump.display()
        );
    }
    Ok(())
}

fn take_events(
    midi: &mut MidiInput,
    clock: &MonotonicClock,
    interrupted: &AtomicBool,
    until_stop: bool,
) -> Vec<MidiEvent> {
    let mut events = Vec::new();
    let mut is_playing = false;
    while !interrupted.load(Ordering::Relaxed) {
        while let Some(event) = midi.poll(clock.now()) {
            let is_stop = event.message == [STOP];
            is_playing |= event.message == [START] || event.message == [CONTINUE];
            events.push(event);
            // Reaper may send Stop before it starts playing, e.g. when seeking.
            if until_stop && is_playing && is_stop {
                return events;
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    events
}

/// Set once Ctrl-C is pressed, instead of the process being killed.
fn ctrl_c() -> std::io::Result<Arc<AtomicBool>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    thread::spawn(move || {
        if runtime.block_on(tokio::signal::ctrl_c()).is_ok() {
            flag.store(true, Ordering::Relaxed);
        }
    });
    Ok(interrupted)
}

/// Every ArtDmx packet for one universe received on UDP 6454, stamped on the MIDI's clock.
struct ArtNetCapture {
    frames: Arc<Mutex<Vec<RecordedFrame>>>,
}

impl ArtNetCapture {
    fn start(universe: u16, clock: MonotonicClock) -> Result<Self, String> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, artnet::PORT)).map_err(|error| {
            format!(
                "cannot listen for Art-Net on UDP {}: {error}; quit BlenderDMX or any other Art-Net receiver",
                artnet::PORT
            )
        })?;
        let frames = Arc::<Mutex<Vec<RecordedFrame>>>::default();
        let received = frames.clone();
        thread::spawn(move || {
            let mut packet = [0; 1024];
            while let Ok(size) = socket.recv(&mut packet) {
                let Some((packet_universe, channels)) = artnet::parse_art_dmx(&packet[..size])
                else {
                    continue;
                };
                if packet_universe == universe {
                    let mut values = [0; DMX_CHANNELS];
                    values[..channels.len()].copy_from_slice(channels);
                    let at = clock.now();
                    received.lock().unwrap().push(RecordedFrame { at, values });
                }
            }
        });
        Ok(Self { frames })
    }

    /// The frames from `start` on, timed from it.
    fn since(self, start: Duration) -> Vec<RecordedFrame> {
        let frames = std::mem::take(&mut *self.frames.lock().unwrap());
        frames
            .into_iter()
            .filter_map(|frame| {
                Some(RecordedFrame {
                    at: frame.at.checked_sub(start)?,
                    values: frame.values,
                })
            })
            .collect()
    }
}
