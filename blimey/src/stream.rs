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
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use blimey_engine::colour::Rgb;
use blimey_engine::pixel;
use blimey_engine::rig::Rig;
use blimey_engine::{
    ArtNetFrames, DmxSink, Frame, MidiEvent, MidiSource, Show, ShowSource, Status, Universes,
};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
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
    rig: StreamedRig,
    /// The engine state behind the next frame.
    status: Status,
    /// The Art-Net universes of the next frame, for the grids' preview colours.
    artnet: ArtNetFrames,
    frames: watch::Sender<Utf8Bytes>,
    midi: broadcast::Sender<Utf8Bytes>,
    #[cfg(test)]
    address: SocketAddr,
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
        let rig = StreamedRig::new(rig, mode);
        let frames = watch::Sender::new(Utf8Bytes::default());
        let (midi, _) = broadcast::channel(MIDI_BACKLOG);
        let test_flash = TimedLook::new(TEST_FLASH_LENGTH);
        let preview = TimedLook::new(PREVIEW_LIFETIME);
        let (inject_midi, injected_midi) = mpsc::channel();
        let stream = Stream {
            rig_messages: rig.message.clone(),
            frames: frames.clone(),
            midi: midi.clone(),
        };
        let app = Router::new()
            .route("/ws", get(upgrade))
            .with_state(stream)
            .merge(api::routes(Api {
                show_dir: show_dir.to_path_buf(),
                mode,
                rig: rig.rig.clone(),
                test_flash: test_flash.clone(),
                preview: preview.clone(),
                midi: inject_midi,
                history: Default::default(),
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
            artnet: ArtNetFrames::new(),
            frames,
            midi,
            #[cfg(test)]
            address: bound,
            injected_midi: Arc::new(Mutex::new(injected_midi)),
            preview,
            test_flash,
            _server: server,
        })
    }

    /// The `frame` messages published from now on, as a client sees them.
    #[cfg(test)]
    pub fn frames(&self) -> watch::Receiver<Utf8Bytes> {
        self.frames.subscribe()
    }

    /// Draws what the UI puts above every layer over `out`: the builder's look or pixel scene,
    /// then a fixture test if one is running.
    pub fn apply_overlays(&self, at: Duration, out: &mut Universes) {
        let rig = self.rig.rig.borrow();
        self.preview.apply(&rig, at, out);
        self.test_flash.apply(&rig, at, out);
    }

    /// Whether a client is connected: without one, frame messages would go nowhere.
    fn is_watched(&self) -> bool {
        self.frames.receiver_count() > 0
    }
}

impl DmxSink for StreamSink {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        if !self.is_watched() {
            return Ok(());
        }
        let rig = self.rig.rig.borrow();
        let message = frame_message(&rig, at, frame, &self.artnet, &self.status, &self.preview);
        self.frames.send_replace(message.into());
        Ok(())
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) -> io::Result<()> {
        if self.is_watched() {
            self.artnet.clone_from(universes);
        }
        Ok(())
    }

    fn status(&mut self, status: &Status) {
        if self.is_watched() {
            self.status.clone_from(status);
        }
    }
}

/// The rig the render loop draws with, for the API and the frames' preview colours, and its
/// `rig` message, built once for every client.
#[derive(Clone)]
struct StreamedRig {
    rig: Arc<watch::Sender<Rig>>,
    message: watch::Sender<Utf8Bytes>,
    mode: Mode,
}

impl StreamedRig {
    fn new(rig: &Rig, mode: Mode) -> Self {
        StreamedRig {
            rig: Arc::new(watch::Sender::new(rig.clone())),
            message: watch::Sender::new(rig_message(rig, mode).to_string().into()),
            mode,
        }
    }

    /// Streams `rig` if it differs from the one streamed.
    fn publish(&self, rig: &Rig) {
        let is_changed = self.rig.send_if_modified(|streamed| {
            let is_changed = streamed != rig;
            if is_changed {
                streamed.clone_from(rig);
            }
            is_changed
        });
        if is_changed {
            let message = rig_message(rig, self.mode).to_string();
            self.message.send_replace(message.into());
        }
    }
}

/// The render loop's show, with its rig mirrored to the stream: a hot reload or a save reaches
/// the UI, and preview colours use the rig each frame was drawn with.
pub struct StreamedShow<S> {
    shows: S,
    rig: Option<StreamedRig>,
    /// The source's load count when its rig was last streamed.
    streamed_load: Option<u64>,
}

impl<S> StreamedShow<S> {
    pub fn new(shows: S, stream: Option<&StreamSink>) -> Self {
        StreamedShow {
            shows,
            rig: stream.map(|stream| stream.rig.clone()),
            streamed_load: None,
        }
    }
}

impl<S: ShowSource> ShowSource for StreamedShow<S> {
    fn current(&mut self) -> &Show {
        self.current_with_loads().0
    }

    /// Streams the rig of each show the source loads, once.
    fn current_with_loads(&mut self) -> (&Show, u64) {
        let (show, loads) = self.shows.current_with_loads();
        if let Some(rig) = &self.rig
            && self.streamed_load != Some(loads)
        {
            rig.publish(&show.rig);
            self.streamed_load = Some(loads);
        }
        (show, loads)
    }
}

/// The engine's MIDI input plus MIDI posted to the API, with every event it delivers sent to
/// the stream as a `midi` message.
pub struct StreamedMidi<M> {
    midi: M,
    log: Option<broadcast::Sender<Utf8Bytes>>,
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
        let log = self.log.as_ref().filter(|log| log.receiver_count() > 0);
        if let Some(log) = log.filter(|_| !is_ticking(&event.message)) {
            // Fails only when the last client left since the check.
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
    rig_messages: watch::Sender<Utf8Bytes>,
    /// Subscribed to by each client, so the render loop knows when nobody is watching.
    frames: watch::Sender<Utf8Bytes>,
    midi: broadcast::Sender<Utf8Bytes>,
}

async fn upgrade(socket: WebSocketUpgrade, State(stream): State<Stream>) -> Response {
    socket.on_upgrade(|socket| send_stream(socket, stream))
}

/// Messages are shared between clients: sending one to a client copies no text.
async fn send_stream(mut socket: WebSocket, stream: Stream) {
    let mut rig = stream.rig_messages.subscribe();
    rig.mark_changed();
    let mut midi = stream.midi.subscribe();
    let mut frames = stream.frames.subscribe();
    loop {
        // Biased, so a client gets the rig before any frame drawn with it, and MIDI events
        // are never starved by frames. A client that lags on MIDI skips the events it missed.
        let message = tokio::select! {
            biased;
            Ok(()) = rig.changed() => rig.borrow_and_update().clone(),
            Ok(event) = midi.recv() => event,
            Ok(()) = frames.changed() => frames.borrow_and_update().clone(),
            else => return,
        };
        if socket.send(Message::Text(message)).await.is_err() {
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
                "colour": pixel::has_emitters(fixture),
            })
        })
        .collect();
    let grids: Vec<Value> = rig
        .grids
        .iter()
        .map(|grid| {
            json!({
                "name": grid.name,
                "pixels": grid.pixels.iter().map(|pixel| json!({ "x": pixel.x, "y": pixel.y })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "type": "rig",
        "protocol": PROTOCOL_VERSION,
        "mode": mode.name(),
        "fixtures": fixtures,
        "grids": grids,
        "groups": rig.groups,
    })
}

/// `{ "<fixture or grid>": ["#rrggbb", …] }`: the colour each pixel shows, in rig order.
pub fn pixel_colours(rig: &Rig, frame: &Frame, artnet: &ArtNetFrames) -> Value {
    serde_json::to_value(PixelColours { rig, frame, artnet }).expect("pixel colours serialize")
}

/// Serialized straight to text, without a `Value` per pixel: it goes out every frame.
#[derive(Serialize)]
#[serde(tag = "type", rename = "frame")]
struct FrameMessage<'frame> {
    at_ms: u64,
    dmx: &'frame [u8],
    pixels: PixelColours<'frame>,
    /// How long the builder's pixel scene preview has animated, so it draws its shapes in step.
    preview_ms: Option<f64>,
    #[serde(flatten)]
    status: &'frame Status,
}

/// `{ "<fixture or grid>": ["#rrggbb", …] }`, written without allocating per pixel.
struct PixelColours<'frame> {
    rig: &'frame Rig,
    frame: &'frame Frame,
    artnet: &'frame ArtNetFrames,
}

impl Serialize for PixelColours<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Self { rig, frame, artnet } = *self;
        let mut map = serializer.serialize_map(Some(rig.fixtures.len() + rig.grids.len()))?;
        for fixture in &rig.fixtures {
            map.serialize_entry(&fixture.name, &HexColours(fixture.pixel_colours(frame)))?;
        }
        for grid in &rig.grids {
            map.serialize_entry(&grid.name, &HexColours(grid.pixel_colours(frame, artnet)))?;
        }
        map.end()
    }
}

struct HexColours<Colours>(Colours);

impl<Colours: ExactSizeIterator<Item = Rgb> + Clone> Serialize for HexColours<Colours> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for colour in self.0.clone() {
            seq.serialize_element(colour.hex().as_str())?;
        }
        seq.end()
    }
}

fn frame_message(
    rig: &Rig,
    at: Duration,
    frame: &Frame,
    artnet: &ArtNetFrames,
    status: &Status,
    preview: &TimedLook,
) -> String {
    let message = FrameMessage {
        at_ms: at.as_millis() as u64,
        dmx: frame,
        pixels: PixelColours { rig, frame, artnet },
        preview_ms: preview
            .animated(at)
            .map(|animated| animated.as_secs_f64() * 1000.0),
        status,
    };
    serde_json::to_string(&message).expect("a frame message serializes")
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
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    use blimey_engine::look::{Look, LookSettings};
    use blimey_engine::{CueStatus, FakeClock, FakeDmx, show_file};
    use futures_util::StreamExt;
    use tokio_tungstenite::connect_async;

    use crate::hot_reload::HotReload;

    #[test]
    fn frames_are_built_only_while_a_websocket_client_is_connected() {
        let rig = Show::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/four-washes"
        )))
        .unwrap()
        .rig;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let mut sink = StreamSink::start(address, &std::env::temp_dir(), &rig, Mode::Show).unwrap();

        sink.send(Duration::ZERO, &[0; 512]).unwrap();
        assert_eq!(
            sink.frames.borrow().as_str(),
            "",
            "no client: no frame is built"
        );

        let client = Runtime::new().unwrap();
        let url = format!("ws://{}/ws", sink.address);
        let (mut socket, _) = client.block_on(connect_async(url)).unwrap();
        let mut next = || -> Value {
            let message = client.block_on(socket.next()).unwrap().unwrap();
            serde_json::from_str(message.to_text().unwrap()).unwrap()
        };
        assert_eq!(
            next()["fixtures"][3]["name"],
            "wash-4",
            "the rig comes first"
        );
        assert!(
            sink.is_watched(),
            "a connected client holds a frame receiver"
        );
        sink.send(Duration::from_millis(25), &[7; 512]).unwrap();
        let frame = next();
        assert_eq!(
            (&frame["type"], &frame["at_ms"], &frame["dmx"][0]),
            (&json!("frame"), &json!(25), &json!(7))
        );
    }

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
        let streamed_rig = StreamedRig::new(&Rig::default(), Mode::Edit);
        let shows = StreamedShow {
            shows: HotReload::load(&show_dir).unwrap(),
            rig: Some(streamed_rig.clone()),
            streamed_load: None,
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
            streamed_rig.rig.borrow().fixture("wash-4").unwrap().address,
            201
        );
        let message: Value = serde_json::from_str(&streamed_rig.message.borrow()).unwrap();
        assert_eq!(message["fixtures"][3]["address"], 201);
        fs::remove_dir_all(show_dir).unwrap();
    }

    #[test]
    fn frames_carry_the_song_active_cues_master_and_live_parameters() {
        let mut live_parameters = [0.0; 8];
        live_parameters[2] = 0.5;
        let status = Status {
            song: Some("demo-song".into()),
            cues: vec![
                CueStatus {
                    note: 36,
                    name: "verse-blue".into(),
                    intensity: 1.0,
                    released: false,
                    beats: 2.5,
                    loop_beats: None,
                },
                CueStatus {
                    note: 40,
                    name: "hits".into(),
                    intensity: 1.0,
                    released: false,
                    beats: 2.5,
                    loop_beats: Some(-0.5),
                },
            ],
            master: 0.25,
            live_parameters,
        };

        let message: Value = serde_json::from_str(&frame_message(
            &Rig::default(),
            Duration::ZERO,
            &[0; 512],
            &ArtNetFrames::new(),
            &status,
            &TimedLook::new(PREVIEW_LIFETIME),
        ))
        .unwrap();

        assert_eq!(message["song"], "demo-song");
        assert_eq!(
            message["cues"],
            json!([
                { "note": 36, "name": "verse-blue", "intensity": 1.0, "released": false, "beats": 2.5 },
                { "note": 40, "name": "hits", "intensity": 1.0, "released": false, "beats": 2.5, "loop_beats": -0.5 },
            ])
        );
        assert_eq!(message["master"], 0.25);
        assert_eq!(message["live_parameters"][2], 0.5);
    }

    #[test]
    fn grids_come_with_their_pixel_positions_and_art_net_colours() {
        let rig = Rig::parse(
            "fixtures:\n  - { name: wall, type: pixel-grid, columns: 2, rows: 1, colour_order: GRB, output: artnet, universe: 3, address: 1, canvas: { x: 0, y: 0.5, width: 1 } }\n",
            Path::new("."),
        )
        .unwrap();
        let mut universe = [0; 512];
        universe[4] = 255; // GRB: the second pixel's red

        let rig_message = rig_message(&rig, Mode::Show);
        let frame: Value = serde_json::from_str(&frame_message(
            &rig,
            Duration::ZERO,
            &[0; 512],
            &ArtNetFrames::from([(3, universe)]),
            &Status::default(),
            &TimedLook::new(PREVIEW_LIFETIME),
        ))
        .unwrap();

        assert_eq!(rig_message["fixtures"], json!([]));
        assert_eq!(
            rig_message["grids"],
            json!([{ "name": "wall", "pixels": [{ "x": 0.25, "y": 0.5 }, { "x": 0.75, "y": 0.5 }] }])
        );
        assert_eq!(frame["pixels"]["wall"], json!(["#000000", "#ff0000"]));
    }

    #[test]
    fn fixtures_say_whether_they_have_colour_for_pixel_cues_to_light() {
        let show = Show::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/pixel-layers"
        )))
        .unwrap();

        let rig_message = rig_message(&show.rig, Mode::Show);

        let colour = |name: &str| {
            let fixtures = rig_message["fixtures"].as_array().unwrap();
            fixtures
                .iter()
                .find(|fixture| fixture["name"] == name)
                .unwrap()["colour"]
                .clone()
        };
        assert_eq!([colour("smoke-dimmer"), colour("par")], [false, true]);
    }
}
