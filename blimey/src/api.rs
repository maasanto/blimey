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
use serde_json::{Value, json};
use serde_yaml_ng::Mapping;
use tokio::sync::watch;

const TEST_FLASH_LENGTH: Duration = Duration::from_secs(1);

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
    pub test_flash: TestFlash,
    /// The builder's look, drawn above every layer while it is edited.
    pub preview: Preview,
    /// MIDI messages for the render loop, as if from the input port.
    pub midi: Sender<Vec<u8>>,
    /// The last save made through the API, for undo.
    pub last_save: Arc<Mutex<Option<Save>>>,
}

pub type Preview = Arc<Mutex<Look>>;

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
    Json(mut body): Json<Value>,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let Ok(replaces) = serde_json::from_value::<Option<u8>>(body["replaces"].take()) else {
        return unprocessable("replaces must be the note the cue had before, 0-127, or null");
    };
    let cue = body["cue"].take();
    saved(
        &api,
        show_file::save_cue(&api.show_dir, &song, note, replaces, cue),
    )
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
    let Some(save) = lock(&api.last_save).take() else {
        return error_response(&io::Error::new(
            io::ErrorKind::NotFound,
            "nothing to undo: no save since the engine started, or it was already undone",
        ));
    };
    let file = save.file().to_owned();
    match save.undo(&api.show_dir) {
        Ok(()) => Json(json!({ "file": file })).into_response(),
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
            *lock(&api.preview) = look;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => error_response(&error),
    }
}

async fn clear_preview(State(api): State<Api>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    *lock(&api.preview) = Look::default();
    StatusCode::NO_CONTENT.into_response()
}

async fn inject_midi(State(api): State<Api>, Json(mut body): Json<Value>) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let message = serde_json::from_value::<Vec<u8>>(body["message"].take()).unwrap_or_default();
    let is_message = matches!(message.split_first(), Some((status, data))
        if *status >= 0x80 && data.len() <= 2 && data.iter().all(|byte| *byte < 0x80));
    if !is_message {
        return unprocessable(
            "message must be one MIDI message as bytes, status first, e.g. [145, 36, 127]",
        );
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
    api.test_flash.start(name);
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

/// The Patch page's test button: one fixture at full white for `TEST_FLASH_LENGTH`, above
/// every other layer.
#[derive(Clone, Default)]
pub struct TestFlash(Arc<Mutex<Option<Flash>>>);

struct Flash {
    fixture: String,
    /// Set by the first frame drawn, so the flash runs on engine time.
    until: Option<Duration>,
}

impl TestFlash {
    fn start(&self, fixture: String) {
        *self.lock() = Some(Flash {
            fixture,
            until: None,
        });
    }

    pub fn apply(&self, rig: &Rig, at: Duration, frame: &mut Frame) {
        let mut flash = self.lock();
        let Some(active) = flash.as_mut() else {
            return;
        };
        if at >= *active.until.get_or_insert(at + TEST_FLASH_LENGTH) {
            *flash = None;
            return;
        }
        let white = FixtureLook {
            color: Rgb {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
            },
            ..FixtureLook::default()
        };
        let look = Look {
            fixtures: BTreeMap::from([(active.fixture.clone(), white)]),
        };
        look::render(rig, &look, frame);
    }

    fn lock(&self) -> MutexGuard<'_, Option<Flash>> {
        lock(&self.0)
    }
}
