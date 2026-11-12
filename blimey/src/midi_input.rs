//! Live MIDI input from a CoreMIDI port such as the IAC bus Reaper sends to.

use std::error::Error;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use blimey_engine::{Clock, MidiEvent, MidiSource, MonotonicClock};
use midir::{MidiInputConnection, MidiInputPort};

const CLIENT_NAME: &str = "blimey";

pub struct MidiInput {
    events: Receiver<MidiEvent>,
    /// An event received after the deadline being polled, kept for the next frame.
    early: Option<MidiEvent>,
    _connection: MidiInputConnection<()>,
}

impl MidiInput {
    /// Connects to the first input port whose name contains `port_name`, stamping each event
    /// on `clock` as it arrives.
    pub fn connect(port_name: &str, clock: MonotonicClock) -> Result<Self, Box<dyn Error>> {
        let input = midir::MidiInput::new(CLIENT_NAME)?;
        let port = find_port(&input, port_name)?;
        let full_name = input.port_name(&port)?;
        let (sender, events) = mpsc::channel();
        let connection = input
            .connect(
                &port,
                CLIENT_NAME,
                move |_, message, ()| {
                    let event = MidiEvent {
                        at: clock.now(),
                        message: message.to_vec(),
                    };
                    // Only fails once the engine has dropped the receiver, i.e. while shutting down.
                    let _ = sender.send(event);
                },
                (),
            )
            .map_err(|error| format!("cannot connect to MIDI port {full_name:?}: {error}"))?;
        eprintln!("midi: listening on {full_name:?}");
        Ok(Self {
            events,
            early: None,
            _connection: connection,
        })
    }
}

impl MidiSource for MidiInput {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        let event = self.early.take().or_else(|| self.events.try_recv().ok())?;
        if event.at <= until {
            Some(event)
        } else {
            self.early = Some(event);
            None
        }
    }
}

pub fn port_names() -> Result<Vec<String>, Box<dyn Error>> {
    let input = midir::MidiInput::new(CLIENT_NAME)?;
    Ok(input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok())
        .collect())
}

fn find_port(input: &midir::MidiInput, port_name: &str) -> Result<MidiInputPort, Box<dyn Error>> {
    let ports = input.ports();
    let names: Vec<String> = ports
        .iter()
        .map(|port| input.port_name(port).unwrap_or_default())
        .collect();
    let index = names
        .iter()
        .position(|name| name.contains(port_name))
        .ok_or_else(|| {
            format!(
                "no MIDI input port matching {port_name:?}; available: {names:?}. On macOS, enable the IAC Driver in Audio MIDI Setup"
            )
        })?;
    Ok(ports[index].clone())
}
