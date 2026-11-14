//! Enttec DMX USB Pro output, per Enttec's "DMX USB Pro Widget API Specification".

/// `blimey soak --emulated-enttec` drives it; the tests use the rest.
#[allow(dead_code)]
pub mod emulator;

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use blimey_engine::{DmxSink, Frame};
use serialport::SerialPort;

const START_OF_MESSAGE: u8 = 0x7E;
const END_OF_MESSAGE: u8 = 0xE7;
/// "Output Only Send DMX Packet Request".
const SEND_DMX_LABEL: u8 = 6;
const DMX_START_CODE: u8 = 0;
/// The box is a USB FIFO and ignores the baud rate. On macOS, 0 makes serialport skip the
/// IOSSIOSPEED ioctl, which ptys (the emulated box) reject; on Linux, 0 would mean hang up.
const BAUD_RATE: u32 = if cfg!(target_os = "macos") {
    0
} else {
    115_200
};
/// A box that stops draining for this long is treated as lost.
const WRITE_TIMEOUT: Duration = Duration::from_millis(100);

/// One label 6 packet carrying a full 512-channel universe.
pub fn send_dmx_packet(frame: &Frame) -> Vec<u8> {
    let length = u16::try_from(frame.len() + 1).expect("a DMX frame is 512 channels");
    let mut packet = Vec::with_capacity(frame.len() + 6);
    packet.extend([START_OF_MESSAGE, SEND_DMX_LABEL]);
    packet.extend(length.to_le_bytes());
    packet.push(DMX_START_CODE);
    packet.extend(frame);
    packet.push(END_OF_MESSAGE);
    packet
}

/// The box keeps outputting the last frame it got, so a frame missed while reconnecting holds the previous look.
// ponytail: an engine restart also relies on the box holding its last frame; the bench session
// measures it. If it goes dark instead, the fallback is plan §9's keeper: a tiny process that owns
// this port and repeats the last frame it got from the engine over a local socket.
pub struct EnttecPro {
    /// The frame for the thread that writes the port, so a box that stops draining never holds
    /// up the render loop. Latest wins: a box that drains again gets the current look.
    latest: Arc<Latest>,
    writer: JoinHandle<()>,
}

impl EnttecPro {
    /// Opens nothing yet: the first frame opens the port, and every later frame retries while it is missing.
    pub fn new(path: String) -> Self {
        let latest = Arc::new(Latest::default());
        let mut port = Port {
            path,
            port: None,
            is_working: None,
        };
        let writer = thread::spawn({
            let latest = latest.clone();
            move || {
                while let Some(frame) = latest.take() {
                    if let Some(change) = port.write(&frame) {
                        eprintln!("{change}");
                    }
                }
            }
        });
        Self { latest, writer }
    }
}

impl Drop for EnttecPro {
    /// Lets the writer finish the frame waiting, if any, then stop.
    fn drop(&mut self) {
        self.latest.slot().is_closed = true;
        self.latest.ready.notify_one();
    }
}

/// A one-frame slot between the render loop and the writer.
#[derive(Default)]
struct Latest {
    slot: Mutex<Slot>,
    ready: Condvar,
}

#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    is_closed: bool,
}

impl Latest {
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the frame waiting, if any.
    fn put(&self, frame: Frame) {
        self.slot().frame = Some(frame);
        self.ready.notify_one();
    }

    /// Waits for a frame; `None` once closed with none left.
    fn take(&self) -> Option<Frame> {
        let waiting = |slot: &mut Slot| slot.frame.is_none() && !slot.is_closed;
        self.ready
            .wait_while(self.slot(), waiting)
            .unwrap_or_else(PoisonError::into_inner)
            .frame
            .take()
    }
}

struct Port {
    path: String,
    port: Option<Box<dyn SerialPort>>,
    /// Whether the last frame reached the box; `None` before the first.
    is_working: Option<bool>,
}

impl Port {
    /// Writes `frame`, reconnecting first if the box was lost. Returns a line for the log when
    /// the box starts or stops taking frames, so a box that hangs every frame logs once.
    fn write(&mut self, frame: &Frame) -> Option<String> {
        let written = self
            .connected_port()
            .and_then(|port| port.write_all(&send_dmx_packet(frame)));
        let is_working = written.is_ok();
        if written.is_err() {
            self.port = None;
        }
        if self.is_working.replace(is_working) == Some(is_working) {
            return None;
        }
        Some(match written {
            Ok(()) => format!("enttec: sending to {}", self.path),
            Err(error) => format!(
                "enttec: {}: {error}; retrying every frame (check the USB cable, or run `blimey devices`)",
                self.path
            ),
        })
    }

    fn connected_port(&mut self) -> io::Result<&mut Box<dyn SerialPort>> {
        let port = match self.port.take() {
            Some(port) => port,
            None => serialport::new(&self.path, BAUD_RATE)
                .timeout(WRITE_TIMEOUT)
                .open()?,
        };
        Ok(self.port.insert(port))
    }
}

impl DmxSink for EnttecPro {
    /// Never blocks. Fails only if the writer thread died, so the engine exits and launchd
    /// restarts it rather than the light freezing unnoticed.
    fn send(&mut self, _at: Duration, frame: &Frame) -> io::Result<()> {
        if self.writer.is_finished() {
            return Err(io::Error::other(
                "enttec: the thread writing the box stopped",
            ));
        }
        self.latest.put(*frame);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::path::Path;

    use blimey_engine::{DMX_CHANNELS, MidiFile, Show, replay};

    use super::emulator::{EmulatedEnttec, take_frames};
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");
    const PATIENCE: Duration = Duration::from_secs(5);

    fn frame_with(channel: usize, value: u8) -> Frame {
        let mut frame = [0; DMX_CHANNELS];
        frame[channel - 1] = value;
        frame
    }

    #[test]
    fn packet_is_label_6_with_start_code_and_512_channels() {
        let packet = send_dmx_packet(&frame_with(1, 0xAB));

        assert_eq!(packet.len(), 518);
        assert_eq!(packet[..4], [0x7E, 6, 0x01, 0x02], "length 513, LSB first");
        assert_eq!(packet[4], 0, "DMX start code");
        assert_eq!(packet[5], 0xAB, "channel 1");
        assert_eq!(packet[517], 0xE7);
    }

    #[test]
    fn emulator_parses_packets_split_across_reads_and_skips_noise() {
        let mut first = [0; DMX_CHANNELS];
        first[0] = 7;
        let second = [255; DMX_CHANNELS];
        let mut stream = vec![0x00, 0x13];
        stream.extend(send_dmx_packet(&first));
        stream.extend(send_dmx_packet(&second));
        let (head, tail) = stream.split_at(300);

        let mut received = head.to_vec();
        let before_tail = take_frames(&mut received);
        received.extend_from_slice(tail);
        let after_tail = take_frames(&mut received);

        assert!(before_tail.is_empty());
        assert_eq!(after_tail, [first, second]);
        assert!(received.is_empty());
    }

    #[test]
    fn driver_delivers_the_same_frames_as_fake_dmx() {
        let show = Show::load(&Path::new(FIXTURES).join("shows/legacy")).unwrap();
        let midi = MidiFile::open(&Path::new(FIXTURES).join("midi/legacy-song.mid")).unwrap();
        let expected: Vec<Frame> = replay(&show, midi)
            .iter()
            .map(|frame| frame.values)
            .collect();
        let device = EmulatedEnttec::plug_in().unwrap();
        let mut driver = EnttecPro::new(device.path());

        // At the pace of a box, not of a fake clock: the driver drops frames a box can't take.
        for (sent, frame) in (1..).zip(&expected) {
            driver.send(Duration::ZERO, frame).unwrap();
            device.wait_for_frames(sent, PATIENCE);
        }

        assert_eq!(device.frames(), expected);
    }

    /// A box still plugged in that stops reading, as a hung widget: the render loop, and the
    /// Art-Net and MIDI it handles, must not wait on it.
    #[test]
    fn a_box_that_stops_draining_never_holds_up_the_render_loop() {
        let (_never_read, port) = serialport::TTYPort::pair().unwrap();
        let link = std::env::temp_dir().join(format!("blimey-enttec-hung-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(port.name().unwrap(), &link).unwrap();
        let mut driver = EnttecPro::new(link.to_string_lossy().into_owned());

        let started = std::time::Instant::now();
        for _ in 0..40 {
            driver.send(Duration::ZERO, &[0; DMX_CHANNELS]).unwrap();
        }

        // The writer thread outlives the test: it must not open the pty's name once another
        // test reuses it.
        std::fs::remove_file(&link).unwrap();
        assert!(
            started.elapsed() < WRITE_TIMEOUT,
            "a second of frames took {:?} to hand over",
            started.elapsed()
        );
    }

    #[test]
    fn a_box_that_drains_again_gets_the_latest_frame_not_a_stale_one() {
        let (mut box_end, port) = serialport::TTYPort::pair().unwrap();
        box_end.set_timeout(Duration::from_millis(10)).unwrap();
        let link = std::env::temp_dir().join(format!("blimey-enttec-slow-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(port.name().unwrap(), &link).unwrap();
        let mut driver = EnttecPro::new(link.to_string_lossy().into_owned());

        // Unread, the pty fills within a few frames and the writer waits on it.
        for value in 1..=20 {
            driver.send(Duration::ZERO, &frame_with(1, value)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        for value in [101, 102, 103] {
            driver.send(Duration::ZERO, &frame_with(1, value)).unwrap();
        }
        let mut received = Vec::new();
        let draining = std::time::Instant::now();
        while draining.elapsed() < Duration::from_millis(500) {
            let mut chunk = [0; 4096];
            if let Ok(count) = box_end.read(&mut chunk) {
                received.extend_from_slice(&chunk[..count]);
            }
        }
        std::fs::remove_file(&link).unwrap();

        let frames = take_frames(&mut received);
        assert_eq!(
            frames.last(),
            Some(&frame_with(1, 103)),
            "{} frames",
            frames.len()
        );
    }

    #[test]
    fn a_box_is_logged_when_it_hangs_or_comes_back_not_every_frame() {
        let port_on = |path: String| Port {
            path,
            port: None,
            is_working: None,
        };
        let logged = |port: &mut Port, frames: usize| -> Vec<String> {
            (0..frames)
                .filter_map(|_| port.write(&[0; DMX_CHANNELS]))
                .collect()
        };
        let (_never_read, hung) = serialport::TTYPort::pair().unwrap();
        let mut device = EmulatedEnttec::plug_in().unwrap();
        let mut replugged = port_on(device.path());

        let hanging = logged(&mut port_on(hung.name().unwrap()), 8);
        logged(&mut replugged, 2);
        device.unplug();
        logged(&mut replugged, 2);
        device.plug_back_in().unwrap();
        let back = logged(&mut replugged, 3);

        assert!(hanging.len() <= 2, "{hanging:#?}");
        assert!(
            hanging.last().unwrap().contains("timed out"),
            "{hanging:#?}"
        );
        assert_eq!(back, [format!("enttec: sending to {}", device.path())]);
    }

    #[test]
    fn driver_reconnects_after_the_port_is_lost() {
        let mut device = EmulatedEnttec::plug_in().unwrap();
        let mut driver = EnttecPro::new(device.path());
        driver.send(Duration::ZERO, &frame_with(6, 1)).unwrap();
        device.wait_for_frames(1, PATIENCE);

        device.unplug();
        driver.send(Duration::ZERO, &frame_with(6, 2)).unwrap();
        device.plug_back_in().unwrap();
        let deadline = std::time::Instant::now() + PATIENCE;
        while device.frames().len() < 2 && std::time::Instant::now() < deadline {
            driver.send(Duration::ZERO, &frame_with(6, 3)).unwrap();
            std::thread::sleep(Duration::from_millis(25));
        }

        let frames = device.frames();
        assert_eq!(frames[0], frame_with(6, 1));
        assert_eq!(
            frames.get(1),
            Some(&frame_with(6, 3)),
            "no frame after replug"
        );
    }
}
