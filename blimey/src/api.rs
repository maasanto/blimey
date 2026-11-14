//! HTTP API under `/api`, next to the WebSocket stream. The contract is docs/api.md.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use blimey_engine::clock_follower::ClockFollower;
use blimey_engine::colour::Rgb;
use blimey_engine::cue::resolve_look;
use blimey_engine::envelope::Envelope;
use blimey_engine::look::{self, FixtureLook, Look, VirtualDimmers};
use blimey_engine::pattern::Pattern;
use blimey_engine::pixel::{self, PixelScene, RhythmTime};
use blimey_engine::rig::Rig;
use blimey_engine::show_file::{self, History, Save};
use blimey_engine::{Show, Universes};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use serde_yaml_ng::Mapping;
use tokio::sync::watch;

pub const TEST_FLASH_LENGTH: Duration = Duration::from_secs(1);
/// How long a look preview lasts after its last `PUT`: the builder refreshes it more often, so
/// a builder that closed or crashed leaves no look on stage.
pub const PREVIEW_LIFETIME: Duration = Duration::from_secs(5);
/// Channel-2 note-on: fires a cue.
const CUE_NOTE_ON: u8 = 0x91;

/// Edit mode lets the UI change show files and test fixtures; show mode is read-only (plan §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Edit,
    Show,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Edit => "edit",
            Mode::Show => "show",
        }
    }
}

#[derive(Clone)]
pub struct Api {
    pub show_dir: PathBuf,
    pub mode: Mode,
    /// The rig the render loop draws with.
    pub rig: Arc<watch::Sender<Rig>>,
    /// The Patch page's test button: one fixture full white for `TEST_FLASH_LENGTH`.
    pub test_flash: TimedLook,
    /// The builder's look while it is edited, lasting `PREVIEW_LIFETIME` from its last `PUT`.
    pub preview: TimedLook,
    /// MIDI messages for the render loop, as if from the input port.
    pub midi: Sender<Vec<u8>>,
    /// The saves made through the API, to undo and redo.
    pub history: Arc<Mutex<History>>,
}

#[derive(Deserialize)]
struct NewSong {
    name: String,
    /// The song file's content: `program`, and optionally `bpm` and `time_signature`.
    #[serde(flatten)]
    content: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct CueSave {
    cue: Value,
    replaces: Option<ReplacedCue>,
}

/// The cue being edited, as the editor loaded it.
#[derive(Deserialize)]
struct ReplacedCue {
    note: u8,
    cue: Value,
}

#[derive(Deserialize)]
struct Tempo {
    bpm: serde_json::Number,
}

/// A pixel cue to preview, animated at its song's tempo.
#[derive(Deserialize)]
struct PixelPreview {
    song: String,
    cue: Value,
}

#[derive(Deserialize)]
struct PostedMidi {
    message: Vec<u8>,
}

/// The most levels `POST /api/envelope` returns: more than any editor is pixels wide.
const MAX_ENVELOPE_SAMPLES: usize = 1001;

#[derive(Deserialize)]
struct EnvelopeSampling {
    curve: Envelope,
    samples: usize,
}

/// A request body in the shape `T`, or an `InvalidData` error (a `422`) saying what was expected.
fn body<T: DeserializeOwned>(content: Value, expected: &str) -> io::Result<T> {
    serde_json::from_value(content).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("{expected} ({error})"))
    })
}

pub fn routes(api: Api) -> Router {
    Router::new()
        .route("/api/show", get(show_summary))
        .route(
            "/api/show/{*file}",
            get(read_show_file).put(write_show_file),
        )
        .route(
            "/api/songs/{song}/cues/{note}",
            put(save_cue).delete(delete_cue),
        )
        .route("/api/songs", post(create_song))
        .route("/api/songs/{song}/bpm", put(set_bpm))
        .route("/api/history", get(history))
        .route("/api/undo", post(undo))
        .route("/api/redo", post(redo))
        .route("/api/preview", put(set_preview).delete(clear_preview))
        .route("/api/preview/pixel", put(set_pixel_preview))
        .route("/api/pixel-thumbnail", post(render_thumbnail))
        .route("/api/midi", post(inject_midi))
        .route("/api/envelope", post(sample_envelope))
        .route("/api/fixtures/{name}/test", post(test_fixture))
        .with_state(api)
}

// ponytail: handlers load the show from disk per request; mirror it from the render loop if
// preview edits ever feel slow.
async fn show_summary(State(api): State<Api>) -> Response {
    match Show::load(&api.show_dir) {
        Ok(show) => Json(json!({
            "palettes": show.palettes.iter().map(|(name, colour)| (name.clone(), json!(colour.to_hex()))).collect::<serde_json::Map<_, _>>(),
            "songs": show.songs.iter().map(|song| json!({ "name": song.name, "program": song.program })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => error_response(&error),
    }
}

async fn read_show_file(State(api): State<Api>, Path(file): Path<String>) -> Response {
    match show_file::read(&api.show_dir, &file) {
        Ok(content) => Json(content).into_response(),
        Err(error) => error_response(&error),
    }
}

/// A JSON request body, its rejection included: handlers refuse show mode (403) before looking
/// at the body, and a body that is not JSON gets the JSON error, not axum's plain text.
type JsonBody = Result<Json<Value>, JsonRejection>;

fn json_body(content: JsonBody) -> io::Result<Value> {
    content
        .map(|Json(content)| content)
        .map_err(|rejection| io::Error::new(io::ErrorKind::InvalidData, rejection.body_text()))
}

async fn write_show_file(
    State(api): State<Api>,
    Path(file): Path<String>,
    content: JsonBody,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    // Hot reload picks the file up on the next frame, and the stream then sends the new rig.
    saved(&api, || show_file::write(&api.show_dir, &file, content))
}

async fn save_cue(
    State(api): State<Api>,
    Path((song, note)): Path<(String, u8)>,
    content: JsonBody,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    let save: CueSave = match body(
        content,
        "send { cue, replaces }, replaces being null or { note, cue } as loaded",
    ) {
        Ok(save) => save,
        Err(error) => return error_response(&error),
    };
    let replaces = save.replaces.map(|replaced| (replaced.note, replaced.cue));
    saved(&api, || {
        show_file::save_cue(&api.show_dir, &song, note, replaces, save.cue)
    })
}

async fn set_bpm(State(api): State<Api>, Path(song): Path<String>, content: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    match body::<Tempo>(content, "send { bpm } with bpm a number") {
        Ok(tempo) => saved(&api, || show_file::set_bpm(&api.show_dir, &song, tempo.bpm)),
        Err(error) => error_response(&error),
    }
}

async fn delete_cue(State(api): State<Api>, Path((song, note)): Path<(String, u8)>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    saved(&api, || show_file::delete_cue(&api.show_dir, &song, note))
}

async fn create_song(State(api): State<Api>, content: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    match body::<NewSong>(
        content,
        "send { name, program } and optionally bpm, time_signature",
    ) {
        Ok(song) => saved(&api, || {
            show_file::create_song(&api.show_dir, &song.name, song.content.into())
        }),
        Err(error) => error_response(&error),
    }
}

/// Makes a save and records it, holding the history throughout so the history lists saves in
/// the order they reached the disk, as undo needs.
fn saved(api: &Api, save: impl FnOnce() -> io::Result<Save>) -> Response {
    let mut history = lock(&api.history);
    match save() {
        Ok(save) => {
            history.record(save);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(&error),
    }
}

/// How many undos and redos are available, as `{ "undo": 3, "redo": 0 }`.
fn steps(history: &History) -> Value {
    let (undo, redo) = history.steps();
    json!({ "undo": undo, "redo": redo })
}

async fn history(State(api): State<Api>) -> Response {
    Json(steps(&lock(&api.history))).into_response()
}

async fn undo(State(api): State<Api>) -> Response {
    step(&api, History::undo)
}

async fn redo(State(api): State<Api>) -> Response {
    step(&api, History::redo)
}

/// Undoes or redoes one save; returns its file and the steps left, as `{ file, undo, redo }`.
fn step(api: &Api, action: fn(&mut History, &std::path::Path) -> io::Result<String>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let mut history = lock(&api.history);
    match action(&mut history, &api.show_dir) {
        Ok(file) => {
            let mut done = steps(&history);
            done["file"] = file.into();
            Json(done).into_response()
        }
        Err(error) => error_response(&error),
    }
}

async fn set_preview(State(api): State<Api>, values: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let values = match json_body(values) {
        Ok(values) => values,
        Err(error) => return error_response(&error),
    };
    let Ok(values) = serde_json::from_value::<Mapping>(values) else {
        return unprocessable(
            "the preview is a mapping of fixture or group names to look settings",
        );
    };
    let look = Show::load(&api.show_dir).and_then(|show| {
        resolve_look(&show, &values)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    });
    match look {
        Ok(look) => {
            api.preview.show(Overlay::Look(look));
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(&error),
    }
}

async fn set_pixel_preview(State(api): State<Api>, content: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let preview = json_body(content)
        .and_then(|content| body::<PixelPreview>(content, "send { song, cue }, cue a pixel cue"))
        .and_then(|preview| {
            let show = Show::load(&api.show_dir)?;
            let Some(song) = show.songs.iter().find(|song| song.name == preview.song) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no song {:?} in the show", preview.song),
                ));
            };
            let scene = PixelScene::parse(&show, song.time_signature, preview.cue)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            Ok(Overlay::Pixel {
                scene: Box::new(scene),
                clock: ClockFollower::new(song.tempo(), song.time_signature),
            })
        });
    match preview {
        Ok(overlay) => {
            api.preview.show(overlay);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(&error),
    }
}

/// Works in show mode too: it renders on a sample grid and changes nothing.
async fn render_thumbnail(State(api): State<Api>, content: JsonBody) -> Response {
    let frames = json_body(content).and_then(|cue| {
        let show = Show::load(&api.show_dir)?;
        pixel::thumbnail(&show, cue)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    });
    match frames {
        Ok(frames) => Json(json!({
            "columns": pixel::THUMBNAIL_SIDE,
            "rows": pixel::THUMBNAIL_SIDE,
            "frames": frames
                .iter()
                .map(|colours| colours.iter().map(|colour| colour.to_hex()).collect())
                .collect::<Vec<Vec<String>>>(),
        }))
        .into_response(),
        Err(error) => error_response(&error),
    }
}

async fn clear_preview(State(api): State<Api>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    api.preview.clear();
    StatusCode::NO_CONTENT.into_response()
}

async fn inject_midi(State(api): State<Api>, content: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    let expected = "message must be one MIDI message as bytes, status first, e.g. [145, 36, 127]";
    let message = match body::<PostedMidi>(content, expected) {
        Ok(posted) => posted.message,
        Err(error) => return error_response(&error),
    };
    let is_message = matches!(message.split_first(), Some((status, data))
        if *status >= 0x80 && data.len() <= 2 && data.iter().all(|byte| *byte < 0x80));
    if !is_message {
        return unprocessable(expected);
    }
    // An audition shows the cue, not the look being edited above it.
    if matches!(*message, [CUE_NOTE_ON, _, velocity] if velocity > 0) {
        api.preview.clear();
    }
    match api.midi.send(message) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error_response(&io::Error::other("the render loop no longer takes MIDI")),
    }
}

/// A curve's levels as the engine plays them, for the builder to draw. Changes nothing, so it
/// works in show mode too.
async fn sample_envelope(content: JsonBody) -> Response {
    match json_body(content).and_then(envelope_levels) {
        Ok(levels) => Json(levels).into_response(),
        Err(error) => error_response(&error),
    }
}

/// `{ "levels": [...] }`: the curve's level at `samples` evenly spaced points from 0 to 1.
fn envelope_levels(content: Value) -> io::Result<Value> {
    let expected = format!(
        "send {{ curve, samples }}: curve as in a song file, samples from 2 to {MAX_ENVELOPE_SAMPLES}"
    );
    let sampling: EnvelopeSampling = body(content, &expected)?;
    if !(2..=MAX_ENVELOPE_SAMPLES).contains(&sampling.samples) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, expected));
    }
    let last = (sampling.samples - 1) as f64;
    let levels: Vec<f64> = (0..sampling.samples)
        .map(|index| sampling.curve.level(index as f64 / last))
        .collect();
    Ok(json!({ "levels": levels }))
}

fn unprocessable(message: &str) -> Response {
    error_response(&io::Error::new(io::ErrorKind::InvalidData, message))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn test_fixture(State(api): State<Api>, Path(name): Path<String>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    if api.rig.borrow().fixture(&name).is_none() {
        return error_response(&io::Error::new(
            io::ErrorKind::NotFound,
            format!("no fixture {name:?} in the rig"),
        ));
    }
    let white = FixtureLook {
        color: Some(Rgb {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        }),
        ..FixtureLook::default()
    };
    api.test_flash.show(Overlay::Look(Look {
        fixtures: BTreeMap::from([(name, white)]),
    }));
    StatusCode::NO_CONTENT.into_response()
}

fn refused_in_show_mode(mode: Mode) -> Option<Response> {
    (mode == Mode::Show).then(|| {
        let message = "the engine is in show mode, which is read-only; restart it with --edit";
        (StatusCode::FORBIDDEN, Json(json!({ "error": message }))).into_response()
    })
}

fn error_response(error: &io::Error) -> Response {
    let status = match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        io::ErrorKind::InvalidData => StatusCode::UNPROCESSABLE_ENTITY,
        io::ErrorKind::AlreadyExists | io::ErrorKind::ResourceBusy => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "error": error.to_string() }))).into_response()
}

/// What the UI puts above every layer: a look, or a pixel scene animated from the first frame
/// it is drawn on, at its song's tempo.
pub enum Overlay {
    Look(Look),
    Pixel {
        scene: Box<PixelScene>,
        clock: ClockFollower,
    },
}

/// An overlay the UI puts above every layer for `lifetime`, counted on engine time from the
/// first frame drawn after it is shown.
#[derive(Clone)]
pub struct TimedLook {
    lifetime: Duration,
    shown: Arc<Mutex<Option<Shown>>>,
}

struct Shown {
    overlay: Overlay,
    until: Option<Duration>,
    /// When a pixel scene started animating: kept while pixel scenes replace each other, so
    /// moving a slider or the builder's refresh doesn't restart the animation.
    started: Option<Duration>,
}

impl TimedLook {
    pub fn new(lifetime: Duration) -> Self {
        TimedLook {
            lifetime,
            shown: Arc::default(),
        }
    }

    /// Shows `overlay`, or shows it for another `lifetime`.
    fn show(&self, overlay: Overlay) {
        let mut shown = lock(&self.shown);
        let started = match (&*shown, &overlay) {
            (
                Some(Shown {
                    overlay: Overlay::Pixel { .. },
                    started,
                    ..
                }),
                Overlay::Pixel { .. },
            ) => *started,
            _ => None,
        };
        *shown = Some(Shown {
            overlay,
            until: None,
            started,
        });
    }

    fn clear(&self) {
        *lock(&self.shown) = None;
    }

    pub fn apply(&self, rig: &Rig, at: Duration, out: &mut Universes) {
        let mut shown = lock(&self.shown);
        let Some(active) = shown.as_mut() else {
            return;
        };
        if at >= *active.until.get_or_insert(at + self.lifetime) {
            *shown = None;
            return;
        }
        match &active.overlay {
            // Above the master, on the frame as output: the colour below is that frame's DMX.
            Overlay::Look(look) => {
                let mut below = VirtualDimmers::of(&out.enttec);
                look::render(rig, look, &mut out.enttec, &mut below);
            }
            Overlay::Pixel { scene, clock } => {
                let note = *active.started.get_or_insert(at);
                let rhythm_time = RhythmTime {
                    bar_line: Pattern::start(clock, note),
                    at,
                };
                scene.draw(rig, clock, clock.moment(note), at, rhythm_time, 1.0, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blimey_engine::look::LookSettings;

    #[test]
    fn a_timed_look_lapses_unless_shown_again() {
        let show = Show::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/cues"
        )))
        .unwrap();
        let mut look = Look::default();
        let red = LookSettings::from_pairs(&["color=#ff0000".into()]).unwrap();
        look.set(&show, "rgb-par", &red).unwrap();
        let preview = TimedLook::new(PREVIEW_LIFETIME);
        let red_at = |at: Duration| {
            let mut out = Universes {
                enttec: [0; 512],
                artnet: Default::default(),
            };
            preview.apply(&show.rig, at, &mut out);
            out.enttec[4]
        };
        let second = Duration::from_secs(1);

        preview.show(Overlay::Look(look.clone()));
        assert_eq!(red_at(10 * second), 255, "timed from the first frame drawn");
        assert_eq!(red_at(14 * second), 255);
        assert_eq!(red_at(15 * second), 0, "lapsed after PREVIEW_LIFETIME");
        assert_eq!(red_at(16 * second), 0);

        preview.show(Overlay::Look(look));
        assert_eq!(red_at(17 * second), 255, "a new PUT shows it again");
        preview.clear();
        assert_eq!(red_at(17 * second), 0);
    }

    /// `POST /api/pixel-thumbnail` with `cue`: its status and JSON body.
    fn thumbnail(cue: Value) -> (StatusCode, Value) {
        let api = Api {
            show_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues").into(),
            mode: Mode::Show,
            rig: Arc::new(watch::Sender::new(Rig::default())),
            test_flash: TimedLook::new(TEST_FLASH_LENGTH),
            preview: TimedLook::new(PREVIEW_LIFETIME),
            midi: std::sync::mpsc::channel().0,
            history: Arc::default(),
        };
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let response = render_thumbnail(State(api), Ok(Json(cue))).await;
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, serde_json::from_slice(&body).unwrap())
        })
    }

    #[test]
    fn a_thumbnail_renders_one_cycle_on_the_sample_grid_with_the_shows_palette() {
        let cue = |effect: &str| json!({ "name": "t", "type": "pixel", "effect": effect, "params": { "color": "red", "width": 0.25, "speed": "1 bar" } });

        let (status, held) = thumbnail(cue("static"));
        assert_eq!(status, StatusCode::OK, "{held}");
        assert_eq!(
            (held["columns"].as_u64(), held["rows"].as_u64()),
            (Some(12), Some(12))
        );
        let frames = held["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 8);
        let red = |frame: &Value, index: usize| frame[index] == "#ff0000";
        let top_row: Vec<usize> = (0..12).filter(|column| red(&frames[0], *column)).collect();
        assert_eq!(
            top_row,
            [4, 5, 6, 7],
            "a quarter-canvas bar mid-way, in the palette's red"
        );
        assert!(
            frames.iter().all(|frame| frame == &frames[0]),
            "static holds still"
        );

        let (_, chased) = thumbnail(cue("chase"));
        let first_lit = |frame: &Value| (0..12).find(|column| red(frame, *column));
        let steps: Vec<Option<usize>> = chased["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(first_lit)
            .collect();
        assert_eq!(
            steps,
            [
                Some(0),
                Some(0),
                Some(3),
                Some(3),
                Some(6),
                Some(6),
                Some(9),
                Some(9)
            ],
            "four steps of a quarter over the cycle"
        );

        let (status, layered) = thumbnail(json!({ "name": "t", "type": "pixel", "layers": [
            { "area": { "x": 0, "y": 0, "width": 0.5, "height": 1 }, "shape": "fill", "params": { "color": "red" } },
        ] }));
        assert_eq!(status, StatusCode::OK, "{layered}");
        let lit: Vec<bool> = (0..12)
            .map(|column| red(&layered["frames"][0], column))
            .collect();
        assert_eq!(
            lit,
            [[true; 6], [false; 6]].concat(),
            "a layer fills its area"
        );

        let (status, refused) = thumbnail(json!({ "name": "t", "type": "scene", "values": {} }));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused["error"], "expected a pixel cue, with type pixel");
        let (status, refused) = thumbnail(
            json!({ "name": "t", "type": "pixel", "effect": "move", "params": { "speed": "100000000000000000000 bars" } }),
        );
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused["error"], "speed is too long to render a cycle of");
    }

    #[test]
    fn a_pixel_preview_draws_on_art_net_grids_and_keeps_moving_when_shown_again() {
        let rig = Rig::parse(
            "fixtures:\n  - { name: wall, type: pixel-grid, columns: 4, rows: 1, colour_order: RGB, output: artnet, universe: 3, address: 1, canvas: { x: 0, y: 0.5, width: 1 } }\n",
            std::path::Path::new("."),
        )
        .unwrap();
        // A bar a column wide, gliding left to right once a second.
        let overlay = || {
            Overlay::Pixel {
            scene: Box::new(
                PixelScene::parse(
                    &Show::default(),
                    Default::default(),
                    json!({ "name": "glide", "type": "pixel", "effect": "move", "params": { "color": "#ff0000", "width": 0.25, "speed": "1s" } }),
                )
                .unwrap(),
            ),
            clock: ClockFollower::new(120.0, Default::default()),
        }
        };
        let preview = TimedLook::new(PREVIEW_LIFETIME);
        let lit_column = |at: Duration| {
            let mut out = Universes {
                enttec: [0; 512],
                artnet: rig.artnet_frames(),
            };
            preview.apply(&rig, at, &mut out);
            (0..4)
                .filter(|column| out.artnet[&3][column * 3] > 0)
                .collect::<Vec<_>>()
        };
        let millis = Duration::from_millis;

        preview.show(overlay());
        assert_eq!(lit_column(millis(10_000)), [0], "starts on its first frame");
        assert_eq!(lit_column(millis(10_375)), [1]);
        preview.show(overlay());
        assert_eq!(
            lit_column(millis(10_625)),
            [2],
            "a new PUT doesn't restart it"
        );
    }

    #[test]
    fn envelope_levels_are_the_engines_own() {
        let points = json!({ "points": [[0.2, 0.1], [0.6, 1], [1, 0.3]] });
        for (curve, envelope) in [
            (json!("ease-in"), Envelope::EaseIn),
            (json!("sine"), Envelope::Sine),
            (
                points,
                Envelope::Points(vec![(0.2, 0.1), (0.6, 1.0), (1.0, 0.3)]),
            ),
        ] {
            let levels = envelope_levels(json!({ "curve": curve, "samples": 11 })).unwrap();

            let expected: Vec<f64> = (0..=10)
                .map(|step| envelope.level(f64::from(step) / 10.0))
                .collect();
            assert_eq!(levels, json!({ "levels": expected }), "{curve}");
        }
    }

    #[test]
    fn envelope_sampling_refuses_unknown_curves_and_sample_counts_out_of_range() {
        let refusal = |content: Value| {
            let error = envelope_levels(content).unwrap_err();
            (error.kind(), error.to_string())
        };

        let (kind, message) = refusal(json!({ "curve": "swell-up", "samples": 11 }));
        assert_eq!(kind, io::ErrorKind::InvalidData);
        assert!(message.contains("a curve: ramp-up, ramp-down"), "{message}");
        let (_, message) =
            refusal(json!({ "curve": { "points": [[0.5, 0], [0.5, 1]] }, "samples": 11 }));
        assert!(message.contains("times must rise"), "{message}");
        for samples in [1, MAX_ENVELOPE_SAMPLES + 1] {
            assert_eq!(
                refusal(json!({ "curve": "sine", "samples": samples })),
                (
                    io::ErrorKind::InvalidData,
                    "send { curve, samples }: curve as in a song file, samples from 2 to 1001"
                        .into()
                )
            );
        }
    }
}
