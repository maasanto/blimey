//! Live MIDI input from a CoreMIDI port such as the IAC bus the DAW sends to.

use std::error::Error;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use blimey_engine::{Clock, MidiEvent, MidiSource, MonotonicClock};
use midir::{MidiInputConnection, MidiInputPort};

const CLIENT_NAME: &str = "blimey";
/// How often a background thread checks that the port is still the one connected.
const PORT_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// Follows the port by name: if it disappears, or comes back as a new port with the same
/// name (a restarted virtual port, a replugged interface), the input reconnects to it. Port
/// checks and reconnects run on a background thread; the render loop only reads a channel.
pub struct MidiInput {
    events: Receiver<MidiEvent>,
    /// An event received after the deadline being polled, kept for the next frame.
    early: Option<MidiEvent>,
}

impl MidiInput {
    /// Connects to the first input port whose name contains `port_name`, stamping each event
    /// on `clock` as it arrives. A port that is not there yet is reported on stderr and
    /// waited for.
    pub fn connect(port_name: &str, clock: MonotonicClock) -> Result<Self, Box<dyn Error>> {
        #[cfg(target_os = "macos")]
        follow_port_changes()?;
        let (sender, events) = mpsc::channel();
        let mut watcher = PortWatcher {
            port_name: port_name.to_string(),
            clock,
            sender,
            scanner: midir::MidiInput::new(CLIENT_NAME)?,
            connection: None,
            is_missing_reported: false,
            is_failure_reported: false,
        };
        // Before the first frame, so MIDI sent as soon as the engine is up is not lost.
        watcher.check();
        crate::light_thread("midi-watcher", move || {
            loop {
                thread::sleep(PORT_CHECK_INTERVAL);
                watcher.check();
            }
        })?;
        Ok(Self {
            events,
            early: None,
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

struct PortWatcher {
    port_name: String,
    clock: MonotonicClock,
    /// Every connection sends its events here, so the render loop keeps one receiver.
    sender: Sender<MidiEvent>,
    /// Lists ports; kept for the engine's life, as CoreMIDI clients are not meant to churn.
    scanner: midir::MidiInput,
    connection: Option<(MidiInputPort, MidiInputConnection<()>)>,
    is_missing_reported: bool,
    is_failure_reported: bool,
}

impl PortWatcher {
    /// A failure is reported once, until a check succeeds again.
    fn check(&mut self) {
        match self.reconnect_if_needed() {
            Ok(()) => self.is_failure_reported = false,
            Err(error) if !self.is_failure_reported => {
                eprintln!("midi: {error}; retrying every {PORT_CHECK_INTERVAL:?}");
                self.is_failure_reported = true;
            }
            Err(_) => {}
        }
    }

    /// Reconnects if the port the name matches is not the one connected.
    fn reconnect_if_needed(&mut self) -> Result<(), Box<dyn Error>> {
        let ports = self.scanner.ports();
        let names: Vec<String> = ports
            .iter()
            .map(|port| self.scanner.port_name(port).unwrap_or_default())
            .collect();
        let Some(index) = names.iter().position(|name| name.contains(&self.port_name)) else {
            if self.connection.take().is_some() || !self.is_missing_reported {
                eprintln!(
                    "midi: no input port matching {:?}; available: {names:?}; waiting for it. On macOS, enable the IAC Driver in Audio MIDI Setup",
                    self.port_name
                );
                self.is_missing_reported = true;
            }
            return Ok(());
        };
        let port = &ports[index];
        if self
            .connection
            .as_ref()
            .is_some_and(|(connected, _)| connected == port)
        {
            return Ok(());
        }
        self.connection = None;
        let (sender, clock) = (self.sender.clone(), self.clock);
        let full_name = &names[index];
        let connection = midir::MidiInput::new(CLIENT_NAME)?
            .connect(
                port,
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
        self.is_missing_reported = false;
        self.connection = Some((port.clone(), connection));
        Ok(())
    }
}

/// CoreMIDI updates a process's port list on the run loop of the thread that created the
/// process's first client; without a running loop, ports that appear later are never seen.
/// The render loop has none, so a thread creates that first client and runs its loop.
/// Call before any other CoreMIDI client is created.
#[cfg(target_os = "macos")]
fn follow_port_changes() -> Result<(), Box<dyn Error>> {
    let (ready, created) = mpsc::channel();
    crate::light_thread("midi-ports", move || {
        let client = midir::MidiInput::new(CLIENT_NAME);
        let _ = ready.send(client.as_ref().map(|_| ()).map_err(ToString::to_string));
        while client.is_ok() {
            core_foundation::runloop::CFRunLoop::run_current();
            // Only reached if the loop has nothing to wait on.
            thread::sleep(PORT_CHECK_INTERVAL);
        }
    })?;
    created.recv()?.map_err(Into::into)
}

pub fn port_names() -> Result<Vec<String>, Box<dyn Error>> {
    let input = midir::MidiInput::new(CLIENT_NAME)?;
    Ok(input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok())
        .collect())
}
