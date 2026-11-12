//! Enttec DMX USB Pro output, per Enttec's "DMX USB Pro Widget API Specification".

#[cfg(test)]
mod emulator;

use std::io::{self, Write};
use std::time::Duration;

use serialport::SerialPort;
use strata_engine::{DmxSink, Frame};

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
pub struct EnttecPro {
    path: String,
    port: Option<Box<dyn SerialPort>>,
    is_missing_reported: bool,
}

impl EnttecPro {
    /// Opens nothing yet: the first frame opens the port, and every later frame retries while it is missing.
    pub fn new(path: String) -> Self {
        Self {
            path,
            port: None,
            is_missing_reported: false,
        }
    }

    fn connected_port(&mut self) -> Option<&mut Box<dyn SerialPort>> {
        if self.port.is_none() {
            match serialport::new(&self.path, BAUD_RATE)
                .timeout(WRITE_TIMEOUT)
                .open()
            {
                Ok(port) => {
                    eprintln!("enttec: connected to {}", self.path);
                    self.port = Some(port);
                    self.is_missing_reported = false;
                }
                Err(error) if !self.is_missing_reported => {
                    eprintln!(
                        "enttec: cannot open {}: {error}; retrying every frame (check the USB cable, or run `strata devices`)",
                        self.path
                    );
                    self.is_missing_reported = true;
                }
                Err(_) => {}
            }
        }
        self.port.as_mut()
    }
}

impl DmxSink for EnttecPro {
    /// Never fails: losing the box is reported on stderr and the render loop keeps going.
    fn send(&mut self, _at: Duration, frame: &Frame) -> io::Result<()> {
        let Some(port) = self.connected_port() else {
            return Ok(());
        };
        if let Err(error) = port.write_all(&send_dmx_packet(frame)) {
            eprintln!("enttec: lost {}: {error}; reconnecting", self.path);
            self.port = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use strata_engine::{DMX_CHANNELS, FakeClock, MidiFile, Show, replay, run};

    use super::emulator::EmulatedEnttec;
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
    fn driver_delivers_the_same_frames_as_fake_dmx() {
        let show = Show::load(&Path::new(FIXTURES).join("shows/legacy")).unwrap();
        let midi = || MidiFile::open(&Path::new(FIXTURES).join("midi/legacy-song.mid")).unwrap();
        let expected: Vec<Frame> = replay(&show, midi())
            .iter()
            .map(|frame| frame.values)
            .collect();
        let device = EmulatedEnttec::plug_in();
        let mut driver = EnttecPro::new(device.path());

        let frame_count = u32::try_from(expected.len()).unwrap();
        run(
            &mut FakeClock::default(),
            &mut midi(),
            &show,
            &mut driver,
            frame_count,
        )
        .unwrap();

        assert_eq!(device.wait_for_frames(expected.len(), PATIENCE), expected);
    }

    #[test]
    fn driver_reconnects_after_the_port_is_lost() {
        let mut device = EmulatedEnttec::plug_in();
        let mut driver = EnttecPro::new(device.path());
        driver.send(Duration::ZERO, &frame_with(6, 1)).unwrap();
        device.wait_for_frames(1, PATIENCE);

        device.unplug();
        driver.send(Duration::ZERO, &frame_with(6, 2)).unwrap();
        device.plug_back_in();
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
