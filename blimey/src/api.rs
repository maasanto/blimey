//! HTTP API under `/api`, next to the WebSocket stream. The contract is docs/api.md.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use blimey_engine::colour::Rgb;
use blimey_engine::look::{self, FixtureLook, Look};
use blimey_engine::rig::Rig;
use blimey_engine::{Frame, show_file};
use serde_json::{Value, json};
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
}

pub fn routes(api: Api) -> Router {
    Router::new()
        .route("/api/show/{file}", get(read_show_file).put(write_show_file))
        .route("/api/fixtures/{name}/test", post(test_fixture))
        .with_state(api)
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
    match show_file::write(&api.show_dir, &file, content) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(&error),
    }
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Flash>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
