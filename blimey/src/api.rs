//! HTTP API under `/api`, next to the WebSocket stream. The contract is docs/api.md.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use blimey_engine::colour::Rgb;
use blimey_engine::cue::resolve_look;
use blimey_engine::look::{self, FixtureLook, Look};
use blimey_engine::rig::Rig;
use blimey_engine::show_file::{self, Save};
use blimey_engine::{Frame, Show};
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
    /// The last save made through the API, for undo.
    pub last_save: Arc<Mutex<Option<Save>>>,
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

#[derive(Deserialize)]
struct PostedMidi {
    message: Vec<u8>,
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
        .route("/api/songs/{song}/bpm", put(set_bpm))
        .route("/api/undo", post(undo))
        .route("/api/preview", put(set_preview).delete(clear_preview))
        .route("/api/midi", post(inject_midi))
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

async fn write_show_file(
    State(api): State<Api>,
    Path(file): Path<String>,
    Json(content): Json<Value>,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    // Hot reload picks the file up on the next frame, and the stream then sends the new rig.
    saved(&api, show_file::write(&api.show_dir, &file, content))
}

async fn save_cue(
    State(api): State<Api>,
    Path((song, note)): Path<(String, u8)>,
    Json(content): Json<Value>,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let save: CueSave = match body(
        content,
        "send { cue, replaces }, replaces being null or { note, cue } as loaded",
    ) {
        Ok(save) => save,
        Err(error) => return error_response(&error),
    };
    let replaces = save.replaces.map(|replaced| (replaced.note, replaced.cue));
    saved(
        &api,
        show_file::save_cue(&api.show_dir, &song, note, replaces, save.cue),
    )
}

async fn set_bpm(
    State(api): State<Api>,
    Path(song): Path<String>,
    Json(content): Json<Value>,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    match body::<Tempo>(content, "send { bpm } with bpm a number") {
        Ok(tempo) => saved(&api, show_file::set_bpm(&api.show_dir, &song, tempo.bpm)),
        Err(error) => error_response(&error),
    }
}

async fn delete_cue(State(api): State<Api>, Path((song, note)): Path<(String, u8)>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    saved(&api, show_file::delete_cue(&api.show_dir, &song, note))
}

fn saved(api: &Api, save: io::Result<Save>) -> Response {
    match save {
        Ok(save) => {
            *lock(&api.last_save) = Some(save);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(&error),
    }
}

async fn undo(State(api): State<Api>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let mut last_save = lock(&api.last_save);
    let Some(save) = last_save.as_ref() else {
        return error_response(&io::Error::new(
            io::ErrorKind::NotFound,
            "nothing to undo: no save since the engine started, or it was already undone",
        ));
    };
    // A refused undo keeps the save, to try again once the file is put right.
    match save.undo(&api.show_dir) {
        Ok(()) => {
            let file = save.file().to_owned();
            *last_save = None;
            Json(json!({ "file": file })).into_response()
        }
        Err(error) => error_response(&error),
    }
}

async fn set_preview(State(api): State<Api>, Json(values): Json<Value>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
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
            api.preview.show(look);
            StatusCode::NO_CONTENT.into_response()
        }
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

async fn inject_midi(State(api): State<Api>, Json(content): Json<Value>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
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
        color: Rgb {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        },
        ..FixtureLook::default()
    };
    api.test_flash.show(Look {
        fixtures: BTreeMap::from([(name, white)]),
    });
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

/// A look the UI puts above every layer for `lifetime`, counted on engine time from the first
/// frame drawn after it is shown.
#[derive(Clone)]
pub struct TimedLook {
    lifetime: Duration,
    shown: Arc<Mutex<Option<Shown>>>,
}

struct Shown {
    look: Look,
    until: Option<Duration>,
}

impl TimedLook {
    pub fn new(lifetime: Duration) -> Self {
        TimedLook {
            lifetime,
            shown: Arc::default(),
        }
    }

    /// Shows `look`, or shows it for another `lifetime`.
    fn show(&self, look: Look) {
        *lock(&self.shown) = Some(Shown { look, until: None });
    }

    fn clear(&self) {
        *lock(&self.shown) = None;
    }

    pub fn apply(&self, rig: &Rig, at: Duration, frame: &mut Frame) {
        let mut shown = lock(&self.shown);
        let Some(active) = shown.as_mut() else {
            return;
        };
        if at >= *active.until.get_or_insert(at + self.lifetime) {
            *shown = None;
            return;
        }
        look::render(rig, &active.look, frame);
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
            let mut frame = [0; 512];
            preview.apply(&show.rig, at, &mut frame);
            frame[4]
        };
        let second = Duration::from_secs(1);

        preview.show(look.clone());
        assert_eq!(red_at(10 * second), 255, "timed from the first frame drawn");
        assert_eq!(red_at(14 * second), 255);
        assert_eq!(red_at(15 * second), 0, "lapsed after PREVIEW_LIFETIME");
        assert_eq!(red_at(16 * second), 0);

        preview.show(look);
        assert_eq!(red_at(17 * second), 255, "a new PUT shows it again");
        preview.clear();
        assert_eq!(red_at(17 * second), 0);
    }
}
