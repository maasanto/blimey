//! An emulated DMX USB Pro on a pseudo terminal (plan §15): it parses the Enttec protocol and
//! records every DMX frame it receives, so the real driver is tested without the box.
//!
//! Self-contained, so the fault-injection tests (`tests/faults.rs`) include it too. It reads
//! the protocol from the specification, not from the driver it tests.

use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use std::{fs, process};

use blimey_engine::{DMX_CHANNELS, Frame};
use serialport::{SerialPort, TTYPort};

const START_OF_MESSAGE: u8 = 0x7E;
const END_OF_MESSAGE: u8 = 0xE7;
const SEND_DMX_LABEL: u8 = 6;
const HEADER_LENGTH: usize = 4;
const DMX_PAYLOAD_LENGTH: usize = DMX_CHANNELS + 1;

pub struct EmulatedEnttec {
    /// Stable device path, like `/dev/cu.usbserial-EN…`, pointing at the current pty.
    link: PathBuf,
    frames: Arc<Mutex<Vec<Frame>>>,
    /// Why the device stopped reading, if it did.
    failure: Arc<Mutex<Option<String>>>,
    connection: Option<Connection>,
}

struct Connection {
    stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
    /// Held open so reading the master never fails for lack of a slave before the driver connects.
    _slave: TTYPort,
}

impl EmulatedEnttec {
    pub fn plug_in() -> io::Result<Self> {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut device = Self {
            link: std::env::temp_dir().join(format!("blimey-enttec-{}-{id}", process::id())),
            frames: Arc::default(),
            failure: Arc::default(),
            connection: None,
        };
        device.plug_back_in()?;
        Ok(device)
    }

    /// Why the device stopped reading frames, if it did: a pty read error.
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }

    pub fn path(&self) -> String {
        self.link.to_string_lossy().into_owned()
    }

    pub fn frames(&self) -> Vec<Frame> {
        self.frames.lock().unwrap().clone()
    }

    /// The frames received since the last call, forgotten by the device: for long runs.
    pub fn take_received(&self) -> Vec<Frame> {
        std::mem::take(&mut *self.frames.lock().unwrap())
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
            let _ = connection.reader.join();
        }
        let _ = fs::remove_file(&self.link);
    }

    /// A fresh pty behind the same device path, as when the box is replugged.
    pub fn plug_back_in(&mut self) -> io::Result<()> {
        let in_pty = |error: serialport::Error| {
            io::Error::other(format!(
                "cannot open a pty for the emulated Enttec: {error}"
            ))
        };
        let (mut master, slave) = TTYPort::pair().map_err(in_pty)?;
        master
            .set_timeout(Duration::from_millis(10))
            .map_err(in_pty)?;
        let slave_name = slave
            .name()
            .ok_or_else(|| io::Error::other("the emulated Enttec's pty has no name"))?;
        let _ = fs::remove_file(&self.link);
        symlink(slave_name, &self.link)?;
        let stop = Arc::new(AtomicBool::new(false));
        let reader = thread::spawn({
            let (stop, frames, failure) = (stop.clone(), self.frames.clone(), self.failure.clone());
            move || {
                if let Err(error) = record_frames(&mut master, &stop, &frames) {
                    eprintln!("emulated Enttec: read failed: {error}; no more frames");
                    *failure.lock().unwrap() = Some(error.to_string());
                }
            }
        });
        self.connection = Some(Connection {
            stop,
            reader,
            _slave: slave,
        });
        Ok(())
    }
}

impl Drop for EmulatedEnttec {
    fn drop(&mut self) {
        self.unplug();
    }
}

fn record_frames(
    master: &mut TTYPort,
    stop: &AtomicBool,
    frames: &Mutex<Vec<Frame>>,
) -> io::Result<()> {
    let mut received = Vec::new();
    let mut chunk = [0; 4096];
    while !stop.load(Ordering::Relaxed) {
        match master.read(&mut chunk) {
            Ok(count) => received.extend_from_slice(&chunk[..count]),
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                continue;
            }
            Err(error) => return Err(error),
        }
        frames.lock().unwrap().extend(take_frames(&mut received));
    }
    Ok(())
}

/// Removes every complete packet from `received` and returns the DMX frames among them.
/// Bytes before a start delimiter, and packets without the end delimiter, are skipped as noise.
pub fn take_frames(received: &mut Vec<u8>) -> Vec<Frame> {
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
