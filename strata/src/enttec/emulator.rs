//! An emulated DMX USB Pro on a pseudo terminal (plan §15): it parses the Enttec protocol and
//! records every DMX frame it receives, so the real driver is tested without the box.

use std::io::{ErrorKind, Read};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::{fs, process};

use serialport::{SerialPort, TTYPort};
use strata_engine::{DMX_CHANNELS, Frame};

use super::{END_OF_MESSAGE, SEND_DMX_LABEL, START_OF_MESSAGE};

const HEADER_LENGTH: usize = 4;
const DMX_PAYLOAD_LENGTH: usize = DMX_CHANNELS + 1;

pub struct EmulatedEnttec {
    /// Stable device path, like `/dev/cu.usbserial-EN…`, pointing at the current pty.
    link: PathBuf,
    frames: Arc<Mutex<Vec<Frame>>>,
    connection: Option<Connection>,
}

struct Connection {
    stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
    /// Held open so reading the master never fails for lack of a slave before the driver connects.
    _slave: TTYPort,
}

impl EmulatedEnttec {
    pub fn plug_in() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut device = Self {
            link: std::env::temp_dir().join(format!("strata-enttec-{}-{id}", process::id())),
            frames: Arc::default(),
            connection: None,
        };
        device.plug_back_in();
        device
    }

    pub fn path(&self) -> String {
        self.link.to_string_lossy().into_owned()
    }

    pub fn frames(&self) -> Vec<Frame> {
        self.frames.lock().unwrap().clone()
    }

    pub fn wait_for_frames(&self, count: usize, patience: Duration) -> Vec<Frame> {
        let deadline = Instant::now() + patience;
        while self.frames.lock().unwrap().len() < count && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        self.frames()
    }

    /// Like pulling the USB cable: the pty and its device path disappear.
    pub fn unplug(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.stop.store(true, Ordering::Relaxed);
            connection.reader.join().unwrap();
        }
        let _ = fs::remove_file(&self.link);
    }

    /// A fresh pty behind the same device path, as when the box is replugged.
    pub fn plug_back_in(&mut self) {
        let (mut master, slave) = TTYPort::pair().unwrap();
        master.set_timeout(Duration::from_millis(10)).unwrap();
        let _ = fs::remove_file(&self.link);
        symlink(slave.name().unwrap(), &self.link).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let reader = thread::spawn({
            let (stop, frames) = (stop.clone(), self.frames.clone());
            move || record_frames(&mut master, &stop, &frames)
        });
        self.connection = Some(Connection {
            stop,
            reader,
            _slave: slave,
        });
    }
}

impl Drop for EmulatedEnttec {
    fn drop(&mut self) {
        self.unplug();
    }
}

fn record_frames(master: &mut TTYPort, stop: &AtomicBool, frames: &Mutex<Vec<Frame>>) {
    let mut received = Vec::new();
    let mut chunk = [0; 4096];
    while !stop.load(Ordering::Relaxed) {
        match master.read(&mut chunk) {
            Ok(count) => received.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == ErrorKind::TimedOut => continue,
            Err(error) => panic!("emulated Enttec read failed: {error}"),
        }
        frames.lock().unwrap().extend(take_frames(&mut received));
    }
}

/// Removes every complete packet from `received` and returns the DMX frames among them.
/// Bytes before a start delimiter, and packets without the end delimiter, are skipped as noise.
fn take_frames(received: &mut Vec<u8>) -> Vec<Frame> {
    let mut frames = Vec::new();
    loop {
        let Some(start) = received.iter().position(|byte| *byte == START_OF_MESSAGE) else {
            received.clear();
            return frames;
        };
        received.drain(..start);
        let Some(&[_, label, length_lsb, length_msb]) = received.get(..HEADER_LENGTH) else {
            return frames;
        };
        let length = usize::from(u16::from_le_bytes([length_lsb, length_msb]));
        let Some(&end) = received.get(HEADER_LENGTH + length) else {
            return frames;
        };
        if end != END_OF_MESSAGE {
            received.drain(..1);
            continue;
        }
        let payload = &received[HEADER_LENGTH..HEADER_LENGTH + length];
        if label == SEND_DMX_LABEL && length == DMX_PAYLOAD_LENGTH && payload[0] == 0 {
            frames.push(payload[1..].try_into().unwrap());
        }
        received.drain(..=HEADER_LENGTH + length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enttec::send_dmx_packet;

    #[test]
    fn parses_packets_split_across_reads_and_skips_noise() {
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
}
