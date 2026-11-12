//! The engine's HTTP server: WebSocket stream of engine state and output values at
//! `ws://<address>/ws`, the API under `/api` and the web UI at `/`.
//! The contract is docs/api.md.

use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use blimey_engine::rig::Rig;
use blimey_engine::{DmxSink, Frame, MidiEvent, MidiSource, Show, ShowSource, Status};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, watch};

use crate::api::{self, Api, Mode, PREVIEW_LIFETIME, TEST_FLASH_LENGTH, TimedLook};
use crate::ui;

/// MIDI posted to the API, waiting for the render loop.
type InjectedMidi = Arc<Mutex<Receiver<Vec<u8>>>>;

const PROTOCOL_VERSION: u32 = 1;
/// MIDI messages a slow client may fall behind by before it skips some.
const MIDI_BACKLOG: usize = 256;
/// Status bytes left out of the `midi` stream: they arrive dozens of times a second.
const MIDI_TIMING_CLOCK: u8 = 0xF8;
const MIDI_ACTIVE_SENSING: u8 = 0xFE;

/// A `DmxSink` that publishes each frame, with its pixel preview colours, to the stream.
pub struct StreamSink {
    /// The rig the render loop draws with, kept current by `StreamedShow`.
    rig: Arc<watch::Sender<Rig>>,
    /// The engine state behind the next frame.
    status: Status,
    frames: watch::Sender<Arc<str>>,
    midi: broadcast::Sender<Arc<str>>,
    injected_midi: InjectedMidi,
    preview: TimedLook,
    test_flash: TimedLook,
    /// Serves the stream; dropping the sink stops it.
    _server: Runtime,
}

impl StreamSink {
    /// Binds `address`, prints `streaming on ws://<address>/ws`, then serves in the
    /// background: the `rig` message on connect and whenever the rig changes, every `midi`
    /// message, and the latest `frame` message each time one is published. Slow clients skip
    /// frames rather than lag.
    pub fn start(
        address: SocketAddr,
        show_dir: &Path,
        rig: &Rig,
        mode: Mode,
    ) -> io::Result<StreamSink> {
        let server = Runtime::new()?;
        let listener = server.block_on(TcpListener::bind(address))?;
        let bound = listener.local_addr()?;
        // A closed stdout (a reader that went away, a detached service) must not kill the
        // engine, so these banners ignore write errors instead of panicking like println!.
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "streaming on ws://{bound}/ws");
        let _ = writeln!(stdout, "UI on http://{bound}/ in {} mode", mode.name());
        drop(stdout);
        let rig = Arc::new(watch::Sender::new(rig.clone()));
        let (frames, receiver) = watch::channel(Arc::from(""));
        let (midi, _) = broadcast::channel(MIDI_BACKLOG);
        let test_flash = TimedLook::new(TEST_FLASH_LENGTH);
        let preview = TimedLook::new(PREVIEW_LIFETIME);
        let (inject_midi, injected_midi) = mpsc::channel();
        let stream = Stream {
            rig: rig.clone(),
            mode,
            frames: receiver,
            midi: midi.clone(),
        };
        let app = Router::new()
            .route("/ws", get(upgrade))
            .with_state(stream)
            .merge(api::routes(Api {
                show_dir: show_dir.to_path_buf(),
                mode,
                rig: rig.clone(),
                test_flash: test_flash.clone(),
                preview: preview.clone(),
                midi: inject_midi,
                last_save: Default::default(),
            }))
            .fallback(ui::serve);
        server.spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                eprintln!("stream: stopped serving: {error}");
            }
        });
        Ok(StreamSink {
            rig,
            status: Status::default(),
            frames,
            midi,
            injected_midi: Arc::new(Mutex::new(injected_midi)),
            preview,
            test_flash,
            _server: server,
        })
    }

    /// Draws what the UI puts above every layer over `frame`: the builder's look, then a
    /// fixture test if one is running.
    pub fn apply_overlays(&self, at: Duration, frame: &mut Frame) {
        let rig = self.rig.borrow();
        self.preview.apply(&rig, at, frame);
        self.test_flash.apply(&rig, at, frame);
    }
}

impl DmxSink for StreamSink {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        let message = frame_message(&self.rig.borrow(), at, frame, &self.status);
        self.frames.send_replace(message.to_string().into());
        Ok(())
    }

    fn status(&mut self, status: &Status) {
        self.status = status.clone();
    }
}

/// The render loop's show, with its rig mirrored to the stream: a hot reload or a save reaches
/// the UI, and preview colours use the rig each frame was drawn with.
pub struct StreamedShow<S> {
    shows: S,
    rig: Option<Arc<watch::Sender<Rig>>>,
}

impl<S> StreamedShow<S> {
    pub fn new(shows: S, stream: Option<&StreamSink>) -> Self {
        StreamedShow {
            shows,
            rig: stream.map(|stream| stream.rig.clone()),
        }
    }
}

impl<S: ShowSource> ShowSource for StreamedShow<S> {
    fn current(&mut self) -> &Show {
        let show = self.shows.current();
        if let Some(rig) = &self.rig {
            rig.send_if_modified(|streamed| {
                let changed = *streamed != show.rig;
                if changed {
                    *streamed = show.rig.clone();
                }
                changed
            });
        }
        show
    }
}

/// The engine's MIDI input plus MIDI posted to the API, with every event it delivers sent to
/// the stream as a `midi` message.
pub struct StreamedMidi<M> {
    midi: M,
    log: Option<broadcast::Sender<Arc<str>>>,
    injected: Option<InjectedMidi>,
}

impl<M> StreamedMidi<M> {
    pub fn new(midi: M, stream: Option<&StreamSink>) -> Self {
        StreamedMidi {
            midi,
            log: stream.map(|stream| stream.midi.clone()),
            injected: stream.map(|stream| stream.injected_midi.clone()),
        }
    }

    /// A posted message, stamped `until`: it acts on the frame that picks it up.
    fn injected(&self, until: Duration) -> Option<MidiEvent> {
        let injected = self.injected.as_ref()?;
        let message = injected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_recv()
            .ok()?;
        Some(MidiEvent { at: until, message })
    }
}

impl<M: MidiSource> MidiSource for StreamedMidi<M> {
    /// Input events come first, so events stay in time order.
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        let event = self.midi.poll(until).or_else(|| self.injected(until))?;
        if let Some(log) = self.log.as_ref().filter(|_| !is_ticking(&event.message)) {
            // Fails only when no client is connected.
            let _ = log.send(midi_message(&event).to_string().into());
        }
        Some(event)
    }
}

/// MIDI clock ticks and active sensing, which arrive dozens of times a second.
pub fn is_ticking(message: &[u8]) -> bool {
    matches!(
        message.first(),
        Some(&(MIDI_TIMING_CLOCK | MIDI_ACTIVE_SENSING))
    )
}

#[derive(Clone)]
struct Stream {
    rig: Arc<watch::Sender<Rig>>,
    mode: Mode,
    frames: watch::Receiver<Arc<str>>,
    midi: broadcast::Sender<Arc<str>>,
}

async fn upgrade(socket: WebSocketUpgrade, State(stream): State<Stream>) -> Response {
    socket.on_upgrade(|socket| send_stream(socket, stream))
}

async fn send_stream(mut socket: WebSocket, mut stream: Stream) {
    let mut rig = stream.rig.subscribe();
    rig.mark_changed();
    let mut midi = stream.midi.subscribe();
    loop {
        // Biased, so a client gets the rig before any frame drawn with it, and MIDI events
        // are never starved by frames. A client that lags on MIDI skips the events it missed.
        let message = tokio::select! {
            biased;
            Ok(()) = rig.changed() => rig_message(&rig.borrow_and_update(), stream.mode).to_string(),
            Ok(event) = midi.recv() => event.to_string(),
            Ok(()) = stream.frames.changed() => stream.frames.borrow_and_update().to_string(),
            else => return,
        };
        if socket.send(Message::Text(message.into())).await.is_err() {
            return;
        }
    }
}

fn rig_message(rig: &Rig, mode: Mode) -> Value {
    let fixtures: Vec<Value> = rig
        .fixtures
        .iter()
        .map(|fixture| {
            json!({
                "name": fixture.name,
                "profile": fixture.profile,
                "mode": fixture.mode,
                "address": fixture.address,
                "channels": fixture.channels.iter().map(|channel| &channel.name).collect::<Vec<_>>(),
                "pixels": fixture.pixels.iter().map(|pixel| json!({ "x": pixel.x, "y": pixel.y })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "type": "rig",
        "protocol": PROTOCOL_VERSION,
        "mode": mode.name(),
        "fixtures": fixtures,
        "groups": rig.groups,
    })
}

fn frame_message(rig: &Rig, at: Duration, frame: &Frame, status: &Status) -> Value {
    let pixels: serde_json::Map<String, Value> = rig
        .pixel_colours(frame)
        .into_iter()
        .map(|(name, colours)| {
            let hex: Vec<String> = colours.into_iter().map(|colour| colour.to_hex()).collect();
            (name.to_string(), json!(hex))
        })
        .collect();
    json!({
        "type": "frame",
        "at_ms": at.as_millis() as u64,
        "dmx": frame.as_slice(),
        "pixels": pixels,
        "song": status.song,
        "cues": status.cues,
        "master": status.master,
        "live_parameters": status.live_parameters,
    })
}

fn midi_message(event: &MidiEvent) -> Value {
    json!({
        "type": "midi",
        "at_ms": event.at.as_millis() as u64,
        "message": event.message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use blimey_engine::look::{Look, LookSettings};
    use blimey_engine::{CueStatus, FakeClock, FakeDmx, show_file};

    use crate::hot_reload::HotReload;

    /// Delivers its events in order, all due at once.
    struct Events(Vec<MidiEvent>);

    impl MidiSource for Events {
        fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
            (!self.0.is_empty()).then(|| self.0.remove(0))
        }
    }

    #[test]
    fn midi_events_and_posted_midi_are_streamed_except_clock_ticks() {
        let (log, mut streamed) = broadcast::channel(MIDI_BACKLOG);
        let (post, posted) = mpsc::channel();
        let event = |ms, message: &[u8]| MidiEvent {
            at: Duration::from_millis(ms),
            message: message.to_vec(),
        };
        let received = vec![
            event(1000, &[MIDI_TIMING_CLOCK]),
            event(1250, &[0x91, 36, 127]),
            event(1260, &[0xFA]),
        ];
        post.send(vec![0x81, 36, 0]).unwrap();
        let mut midi = StreamedMidi {
            midi: Events(received.clone()),
            log: Some(log),
            injected: Some(Arc::new(Mutex::new(posted))),
        };

        let taken: Vec<MidiEvent> =
            std::iter::from_fn(|| midi.poll(Duration::from_secs(2))).collect();
        let mut expected = received;
        expected.push(event(2000, &[0x81, 36, 0]));
        assert_eq!(
            taken, expected,
            "the engine gets every event, posted ones after the input's, at the frame that takes them"
        );

        let messages: Vec<Value> = std::iter::from_fn(|| streamed.try_recv().ok())
            .map(|message| serde_json::from_str(&message).unwrap())
            .collect();
        assert_eq!(
            messages,
            [
                json!({ "type": "midi", "at_ms": 1250, "message": [0x91, 36, 127] }),
                json!({ "type": "midi", "at_ms": 1260, "message": [0xFA] }),
                json!({ "type": "midi", "at_ms": 2000, "message": [0x81, 36, 0] }),
            ]
        );
    }

    /// Receives no MIDI, but saves `rig` through the show-file write path at `at`, as
    /// `PUT /api/show/rig.yaml` does while the show runs.
    struct SaveWhilePlaying {
        at: Duration,
        show_dir: PathBuf,
        rig: Option<Value>,
    }

    impl MidiSource for SaveWhilePlaying {
        fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
            if until >= self.at
                && let Some(rig) = self.rig.take()
            {
                show_file::write(&self.show_dir, "rig.yaml", rig).unwrap();
            }
            None
        }
    }

    fn four_washes_copy() -> PathBuf {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/shows/four-washes");
        let copy = std::env::temp_dir().join(format!("blimey-readdress-{}", std::process::id()));
        let _ = fs::remove_dir_all(&copy);
        fs::create_dir_all(copy.join("fixtures")).unwrap();
        for file in [
            "rig.yaml",
            "palettes.yaml",
            "fixtures/stairville-wild-wash-pro-648-rgbw.yaml",
        ] {
            fs::copy(source.join(file), copy.join(file)).unwrap();
        }
        copy
    }

    #[test]
    fn readdressing_a_fixture_through_the_write_path_moves_its_output_and_streamed_rig() {
        let show_dir = four_washes_copy();
        let mut look = Look::default();
        let red = LookSettings::from_pairs(&["color=#ff0000".into()]).unwrap();
        look.set(&Show::load(&show_dir).unwrap(), "wash-4", &red)
            .unwrap();
        let mut rig = show_file::read(&show_dir, "rig.yaml").unwrap();
        rig["fixtures"][3]["address"] = 201.into();
        let streamed_rig = Arc::new(watch::Sender::new(Rig::default()));
        let shows = StreamedShow {
            shows: HotReload::load(&show_dir).unwrap(),
            rig: Some(streamed_rig.clone()),
        };
        let mut midi = SaveWhilePlaying {
            at: Duration::from_millis(50),
            show_dir: show_dir.clone(),
            rig: Some(rig),
        };
        let mut sink = FakeDmx::default();

        blimey_engine::run(
            &mut FakeClock::default(),
            &mut midi,
            shows,
            &look,
            &mut sink,
            6,
        )
        .unwrap();

        let wash_4_red: Vec<(u8, u8)> = sink
            .frames
            .iter()
            .map(|frame| (frame.values[100], frame.values[200]))
            .collect();
        assert_eq!(
            wash_4_red,
            [(255, 0), (255, 0), (255, 0), (0, 255), (0, 255), (0, 255)],
            "DMX 101 before the save at 50 ms, DMX 201 from the next frame"
        );
        assert_eq!(
            streamed_rig.borrow().fixture("wash-4").unwrap().address,
            201
        );
        fs::remove_dir_all(show_dir).unwrap();
    }

    #[test]
    fn frames_carry_the_song_active_cues_master_and_live_parameters() {
        let mut live_parameters = [0.0; 8];
        live_parameters[2] = 0.5;
        let status = Status {
            song: Some("tell-me".into()),
            cues: vec![CueStatus {
                note: 36,
                name: "verse-blue".into(),
                intensity: 1.0,
                released: false,
            }],
            master: 0.25,
            live_parameters,
        };

        let message = frame_message(&Rig::default(), Duration::ZERO, &[0; 512], &status);

        assert_eq!(message["song"], "tell-me");
        assert_eq!(
            message["cues"],
            json!([{ "note": 36, "name": "verse-blue", "intensity": 1.0, "released": false }])
        );
        assert_eq!(message["master"], 0.25);
        assert_eq!(message["live_parameters"][2], 0.5);
    }
}
