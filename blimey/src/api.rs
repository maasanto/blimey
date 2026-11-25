//! HTTP API under `/api`, next to the WebSocket stream. The contract is docs/api.md.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use blimey_engine::arrangement;
use blimey_engine::clock_follower::ClockFollower;
use blimey_engine::colour::Rgb;
use blimey_engine::cue::{Cue, CueTiming};
use blimey_engine::duration::TimeSignature;
use blimey_engine::envelope::Envelope;
use blimey_engine::export;
use blimey_engine::look::{self, FixtureLook, Look, VirtualDimmers};
use blimey_engine::pattern::Pattern;
use blimey_engine::pixel::{self, PixelScene, Rhythm};
use blimey_engine::rig::Rig;
use blimey_engine::show_file::{self, History, Save};
use blimey_engine::song::{Audio, Marker, Song};
use blimey_engine::wav;
use blimey_engine::{ArtNetFrames, DMX_CHANNELS, Show, Universes};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, SemaphorePermit, watch};

use crate::stream;

pub const TEST_FLASH_LENGTH: Duration = Duration::from_secs(1);
/// How long a pixel scene preview lasts after its last `PUT`: the builder refreshes it more
/// often, so a builder that closed or crashed leaves nothing on stage.
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

#[derive(Deserialize)]
struct Arrangement {
    sections: Value,
    arrangement: Value,
}

/// The song a section or scene is imported into.
#[derive(Deserialize)]
struct ImportTarget {
    song: Option<String>,
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
        .route("/api/songs/{song}/cues/{note}/export", get(export_scene))
        .route("/api/songs/{song}/export", get(export_song))
        .route(
            "/api/songs/{song}/sections/{section}/export",
            get(export_section),
        )
        .route("/api/import", post(import))
        .route("/api/songs/{song}/thumbnails", get(song_thumbnails))
        .route("/api/songs", post(create_song))
        .route("/api/songs/{song}/bpm", put(set_bpm))
        .route("/api/songs/{song}/arrangement", put(set_arrangement))
        .route(
            "/api/songs/{song}/arrangement/events",
            get(arrangement_events),
        )
        .route("/api/songs/{song}/arrangement.mid", get(arrangement_midi))
        .route("/api/songs/{song}/audio", get(song_audio).put(upload_audio))
        .route("/api/songs/{song}/audio/offset", put(set_audio_offset))
        .route("/api/songs/{song}/audio/markers", get(audio_markers))
        .route("/api/songs/{song}/markers", put(set_markers))
        .route("/api/history", get(history))
        .route("/api/undo", post(undo))
        .route("/api/redo", post(redo))
        .route("/api/preview", delete(clear_preview))
        .route("/api/preview/pixel", put(set_pixel_preview))
        .route("/api/pixel-thumbnail", post(render_thumbnail))
        .route("/api/midi", post(inject_midi))
        .route("/api/envelope", post(sample_envelope))
        .route("/api/fixtures/{name}/test", post(test_fixture))
        .with_state(api)
}

/// How many requests may load the show and render from it at once, so a flood of them, as
/// from a misbehaving client, can't take the CPU from the render loop. The rest wait.
// ponytail: waiters queue without limit, each holding its request; answer 503 past a queue
// length if a flood ever grows memory.
static SHOW_LOADS: Semaphore = Semaphore::const_new(2);

async fn show_load_turn() -> SemaphorePermit<'static> {
    SHOW_LOADS
        .acquire()
        .await
        .expect("SHOW_LOADS is never closed")
}

// ponytail: handlers load the show from disk per request; mirror it from the render loop if
// preview edits ever feel slow.
async fn show_summary(State(api): State<Api>) -> Response {
    let _turn = show_load_turn().await;
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

async fn set_arrangement(
    State(api): State<Api>,
    Path(song): Path<String>,
    content: JsonBody,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    match body::<Arrangement>(content, "send { sections, arrangement }") {
        Ok(new) => saved(&api, || {
            show_file::set_arrangement(&api.show_dir, &song, new.sections, new.arrangement)
        }),
        Err(error) => error_response(&error),
    }
}

/// Works in show mode too: it reads the song and changes nothing.
async fn arrangement_events(State(api): State<Api>, Path(song): Path<String>) -> Response {
    let _turn = show_load_turn().await;
    let events = Show::load(&api.show_dir).and_then(|show| {
        let song = song_named(&show, &song)?;
        Ok(json!({ "bpm": song.tempo(), "events": arrangement::events(song) }))
    });
    match events {
        Ok(events) => Json(events).into_response(),
        Err(error) => error_response(&error),
    }
}

/// Works in show mode too: it builds the file in memory and writes nothing.
async fn arrangement_midi(State(api): State<Api>, Path(song): Path<String>) -> Response {
    let _turn = show_load_turn().await;
    let file = Show::load(&api.show_dir).and_then(|show| {
        let song = song_named(&show, &song)?;
        if song.arrangement.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("song {:?} has no arrangement to export", song.name),
            ));
        }
        arrangement::midi_file(song)
    });
    match file {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "audio/midi".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    attachment(&format!("{song}.mid")),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// The song's audio, for the builder's timeline to play, with single-range requests. Works in
/// show mode too. `404` for a song without `audio`, or whose file isn't there.
// ponytail: reads the asked range into memory; stream it if bounces ever outgrow a few hundred MB.
async fn song_audio(
    State(api): State<Api>,
    Path(song): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (mut file, content_type) = match open_song_audio(&api.show_dir, &song) {
        Ok(audio) => audio,
        Err(error) => return error_response(&error),
    };
    let length = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => return error_response(&error),
    };
    let asked = headers
        .get(header::RANGE)
        .and_then(|range| range.to_str().ok());
    let range = match asked.map(|range| byte_range(range, length)) {
        None => None,
        Some(Ok(range)) => range,
        Some(Err(())) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{length}"))],
            )
                .into_response();
        }
    };
    let (first, last) = range.unwrap_or((0, length.saturating_sub(1)));
    let mut bytes = Vec::new();
    let read = file
        .seek(io::SeekFrom::Start(first))
        .and_then(|_| (&mut file).take(last + 1 - first).read_to_end(&mut bytes));
    if let Err(error) = read {
        return error_response(&error);
    }
    let headers = [
        (header::CONTENT_TYPE, content_type.to_owned()),
        (header::ACCEPT_RANGES, "bytes".to_owned()),
    ];
    match range {
        None => (headers, bytes).into_response(),
        Some(_) => (
            StatusCode::PARTIAL_CONTENT,
            headers,
            [(
                header::CONTENT_RANGE,
                format!("bytes {first}-{last}/{length}"),
            )],
            bytes,
        )
            .into_response(),
    }
}

/// The audio file `songs/<song>.yaml` names, opened, and its type. Reads only the song file, so
/// a request for a range of the audio costs no show load.
fn open_song_audio(
    show_dir: &std::path::Path,
    song: &str,
) -> io::Result<(std::fs::File, &'static str)> {
    let content = show_file::read(show_dir, &format!("songs/{song}.yaml"))?;
    let audio = Audio::of_song(&content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("song {song:?} has no audio"),
            )
        })?;
    let file = std::fs::File::open(show_dir.join(&audio.file)).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", audio.file.display()))
    })?;
    Ok((file, content_type(&audio.file)))
}

/// The first and last byte a `Range` header asks of a file of `length` bytes; `Ok(None)` for
/// the whole file, as for a list of ranges or a header that isn't one, and `Err` when the file
/// holds none of it.
fn byte_range(range: &str, length: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some((first, last)) = range
        .strip_prefix("bytes=")
        .filter(|ranges| !ranges.contains(','))
        .and_then(|range| range.trim().split_once('-'))
    else {
        return Ok(None);
    };
    let (first, last) = match (first.parse::<u64>(), last.parse::<u64>()) {
        (Ok(first), Ok(last)) => (first, last.min(length.saturating_sub(1))),
        (Ok(first), Err(_)) if last.is_empty() => (first, length.saturating_sub(1)),
        (Err(_), Ok(suffix)) if first.is_empty() && suffix > 0 => {
            (length.saturating_sub(suffix), length.saturating_sub(1))
        }
        (Err(_), Ok(_)) if first.is_empty() => return Err(()),
        _ => return Ok(None),
    };
    if first >= length || first > last {
        return Err(());
    }
    Ok(Some((first, last)))
}

fn content_type(file: &std::path::Path) -> &'static str {
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("m4a" | "aac" | "mp4") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

#[derive(Deserialize)]
struct AudioOffset {
    offset: String,
}

async fn set_audio_offset(
    State(api): State<Api>,
    Path(song): Path<String>,
    content: JsonBody,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    match body::<AudioOffset>(
        content,
        "send { offset } with offset in ms or s, as \"1.25s\"",
    ) {
        Ok(new) => saved(&api, || {
            show_file::set_audio_offset(&api.show_dir, &song, new.offset)
        }),
        Err(error) => error_response(&error),
    }
}

/// The largest song audio an upload may hold: a song's bounce, even as 96 kHz 24-bit stereo
/// WAV, takes a fraction of it.
const MAX_AUDIO_UPLOAD: u64 = 1 << 30;
/// Enough of a file's start to tell WAV, MP3 and FLAC apart.
const SNIFFED_BYTES: usize = 12;

/// Takes the song's audio as the request body, stores it as `audio/<song>.<ext>`, names it in
/// the song file, keeping the offset, and removes the song's previous upload. Answers the
/// markers found in the file.
async fn upload_audio(
    State(api): State<Api>,
    Path(song): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_AUDIO_UPLOAD) {
        return error_response(&too_large());
    }
    // Before writing anything: the song must exist under exactly this name, which keeps the name
    // inside the show and, on a case-insensitive disk, off another spelling's audio file.
    if let Err(error) = song_named_exactly(&api.show_dir, &song) {
        return error_response(&error);
    }
    let stored = async {
        let (temporary, file) = TemporaryUpload::create(&api.show_dir.join("audio"), &song)?;
        let head = receive_upload(body, file).await?;
        let extension = audio_extension(&head).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "the file is not WAV, MP3 or FLAC audio; export the song in one of those",
            )
        })?;
        let markers = match extension {
            "wav" => wav_markers(std::fs::File::open(&temporary.path)?).await?,
            _ => Vec::new(),
        };
        let file = store_upload(&api, &song, extension, &temporary.path)?;
        Ok::<_, io::Error>((file, markers))
    }
    .await;
    match stored {
        Ok((file, markers)) => {
            Json(json!({ "file": file, "markers": markers_json(&markers) })).into_response()
        }
        Err(error) => error_response(&error),
    }
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        format!(
            "audio files are limited to {} MB; bounce the song at a lower rate or as MP3",
            MAX_AUDIO_UPLOAD >> 20
        ),
    )
}

/// The `audio` of `songs/<song>.yaml`, reading only that file.
fn song_file_audio(show_dir: &std::path::Path, song: &str) -> io::Result<Option<Audio>> {
    let content = show_file::read(show_dir, &format!("songs/{song}.yaml"))?;
    Audio::of_song(&content).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Refused (`NotFound`) unless `songs/<song>.yaml` is there under exactly that name.
fn song_named_exactly(show_dir: &std::path::Path, song: &str) -> io::Result<()> {
    // Refuses names that aren't a song's, such as `..`, and songs that aren't there at all.
    show_file::read(show_dir, &format!("songs/{song}.yaml"))?;
    let file_name = format!("{song}.yaml");
    let is_exact = std::fs::read_dir(show_dir.join("songs"))?
        .flatten()
        .any(|entry| entry.file_name() == file_name.as_str());
    if is_exact {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no song {song:?}: song names are case-sensitive"),
    ))
}

/// Counts uploads, so concurrent ones never share a temporary file.
static UPLOADS: AtomicU64 = AtomicU64::new(0);

/// An upload as it arrives, removed when dropped: a refused upload, or one whose client went
/// away, leaves nothing behind. Once stored it has been renamed, and there is nothing to remove.
struct TemporaryUpload {
    path: PathBuf,
}

impl TemporaryUpload {
    fn create(audio_dir: &std::path::Path, song: &str) -> io::Result<(Self, std::fs::File)> {
        std::fs::create_dir_all(audio_dir)?;
        let upload = UPLOADS.fetch_add(1, Ordering::Relaxed);
        let path = audio_dir.join(format!(".{song}.{}.{upload}.upload", std::process::id()));
        let file = std::fs::File::create_new(&path)?;
        Ok((Self { path }, file))
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// How many received chunks may wait for the disk before the upload waits in turn.
const UPLOAD_QUEUE_CHUNKS: usize = 16;

/// Streams the body into `file`, on a blocking thread and without holding it in memory, and
/// returns its first bytes.
async fn receive_upload(mut body: Body, file: std::fs::File) -> io::Result<Vec<u8>> {
    let (chunks, mut to_write) = tokio::sync::mpsc::channel::<Bytes>(UPLOAD_QUEUE_CHUNKS);
    let writer = tokio::task::spawn_blocking(move || {
        let mut file = io::BufWriter::new(file);
        while let Some(chunk) = to_write.blocking_recv() {
            file.write_all(&chunk)?;
        }
        file.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()
    });
    let streamed = stream_body(&mut body, &chunks).await;
    drop(chunks);
    // A write error first: it is why the stream stopped, if it did.
    writer.await.map_err(io::Error::other)??;
    streamed
}

async fn stream_body(
    body: &mut Body,
    chunks: &tokio::sync::mpsc::Sender<Bytes>,
) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(SNIFFED_BYTES);
    let mut received = 0;
    while let Some(frame) =
        std::future::poll_fn(|context| std::pin::Pin::new(&mut *body).poll_frame(context)).await
    {
        let Ok(data) = frame.map_err(io::Error::other)?.into_data() else {
            continue;
        };
        received += data.len() as u64;
        if received > MAX_AUDIO_UPLOAD {
            return Err(too_large());
        }
        head.extend(data.iter().take(SNIFFED_BYTES - head.len()));
        chunks
            .send(data)
            .await
            .map_err(|_| io::Error::other("the upload stopped being written"))?;
    }
    Ok(head)
}

/// The markers of a WAV file, read on a blocking thread.
async fn wav_markers(file: std::fs::File) -> io::Result<Vec<Marker>> {
    tokio::task::spawn_blocking(move || wav::markers(&mut io::BufReader::new(file)))
        .await
        .map_err(io::Error::other)?
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// What the file is, by its content rather than by what the request says.
fn audio_extension(head: &[u8]) -> Option<&'static str> {
    match head {
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'A',
            b'V',
            b'E',
            ..,
        ] => Some("wav"),
        [b'f', b'L', b'a', b'C', ..] => Some("flac"),
        [b'I', b'D', b'3', ..] => Some("mp3"),
        // An MPEG audio frame: 11 sync bits, then a layer, which AAC's ADTS leaves at 0.
        [0xFF, second, ..] if second & 0xE0 == 0xE0 && second & 0x06 != 0 => Some("mp3"),
        _ => None,
    }
}

/// Names the received file in the song file, then moves it into place, as one step of the
/// history; the song's previous upload, under another extension, is removed. Returns the file.
fn store_upload(
    api: &Api,
    song: &str,
    extension: &str,
    temporary: &std::path::Path,
) -> io::Result<String> {
    let file = format!("audio/{song}.{extension}");
    let mut history = lock(&api.history);
    // Under the lock, so no other save changes it between this read and the write.
    let previous = song_file_audio(&api.show_dir, song)?;
    // The song file first: if it is refused, no audio file has been touched.
    let save = show_file::set_audio_file(&api.show_dir, song, &file)?;
    if let Err(error) = std::fs::rename(temporary, api.show_dir.join(&file)) {
        if let Err(undo) = save.undo(&api.show_dir) {
            eprintln!("blimey: songs/{song}.yaml names {file}, which couldn't be stored: {undo}");
        }
        return Err(error);
    }
    history.record(save);
    if let Some(previous) = previous.filter(|audio| is_other_upload(audio, song, &file)) {
        // The upload succeeded: a leftover is reported, not a failure.
        match std::fs::remove_file(api.show_dir.join(&previous.file)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => eprintln!(
                "blimey: uploaded {file}, but couldn't remove the previous upload {}: {error}; remove it by hand",
                previous.file.display()
            ),
            _ => {}
        }
    }
    Ok(file)
}

/// Whether `audio` is an upload of `song` other than `file`: named `audio/<song>.<ext>` by an
/// upload, not by hand, and not `file` in another case, which a case-insensitive disk would
/// take for the same file.
fn is_other_upload(audio: &Audio, song: &str, file: &str) -> bool {
    audio.file.parent() == Some("audio".as_ref())
        && audio.file.file_stem() == Some(song.as_ref())
        && !audio.file.to_string_lossy().eq_ignore_ascii_case(file)
}

fn markers_json(markers: &[Marker]) -> Value {
    let seconds = |at: Duration| format!("{}s", (at.as_secs_f64() * 1000.0).round() / 1000.0);
    markers
        .iter()
        .map(|marker| json!({ "name": marker.name, "at": seconds(marker.at) }))
        .collect()
}

/// The markers in the song's audio file as it is now; none for MP3 or FLAC. Works in show mode
/// too.
async fn audio_markers(State(api): State<Api>, Path(song): Path<String>) -> Response {
    let markers = match open_song_audio(&api.show_dir, &song) {
        Ok((file, "audio/wav")) => wav_markers(file).await,
        Ok(_) => Ok(Vec::new()),
        Err(error) => Err(error),
    };
    match markers {
        Ok(markers) => Json(json!({ "markers": markers_json(&markers) })).into_response(),
        Err(error) => error_response(&error),
    }
}

#[derive(Deserialize)]
struct Markers {
    markers: Vec<Value>,
}

async fn set_markers(
    State(api): State<Api>,
    Path(song): Path<String>,
    content: JsonBody,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let content = match json_body(content) {
        Ok(content) => content,
        Err(error) => return error_response(&error),
    };
    match body::<Markers>(
        content,
        "send { markers: [{ name, at }] } with at in ms or s",
    ) {
        Ok(new) => saved(&api, || {
            show_file::set_markers(&api.show_dir, &song, new.markers)
        }),
        Err(error) => error_response(&error),
    }
}

/// A `Content-Disposition` downloading as `file_name`, whatever it holds: an ASCII stand-in
/// for old clients, and the name itself percent-encoded (RFC 6266, RFC 5987).
fn attachment(file_name: &str) -> String {
    let is_safe = |byte: u8| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
    let ascii: String = file_name
        .chars()
        .map(|c| match u8::try_from(c) {
            Ok(byte) if is_safe(byte) => c,
            _ => '_',
        })
        .collect();
    let encoded: String = file_name
        .bytes()
        .map(|byte| {
            if is_safe(byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
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

/// Works in show mode too: it reads the song and changes nothing.
async fn export_song(State(api): State<Api>, Path(song): Path<String>) -> Response {
    let file = export::export_song(&api.show_dir, &song);
    export_file(file, &format!("{song}.yaml"))
}

async fn export_section(
    State(api): State<Api>,
    Path((song, section)): Path<(String, String)>,
) -> Response {
    let file = export::export_section(&api.show_dir, &song, &section);
    export_file(file, &format!("{song}-{section}.yaml"))
}

async fn export_scene(State(api): State<Api>, Path((song, note)): Path<(String, u8)>) -> Response {
    let file = export::export_scene(&api.show_dir, &song, note);
    export_file(file, &format!("{song}-scene-{note}.yaml"))
}

fn export_file(file: io::Result<String>, file_name: &str) -> Response {
    match file {
        Ok(yaml) => (
            [
                (header::CONTENT_TYPE, "application/yaml".to_owned()),
                (header::CONTENT_DISPOSITION, attachment(file_name)),
            ],
            yaml,
        )
            .into_response(),
        Err(error) => error_response(&error),
    }
}

/// The body is the export file itself, as text.
async fn import(
    State(api): State<Api>,
    Query(target): Query<ImportTarget>,
    file: Bytes,
) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let mut history = lock(&api.history);
    let imported = std::str::from_utf8(&file)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the import file must be text"))
        .and_then(|text| export::import(&api.show_dir, target.song.as_deref(), text));
    match imported {
        Ok((save, summary)) => {
            history.record(save);
            Json(summary).into_response()
        }
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

async fn set_pixel_preview(State(api): State<Api>, content: JsonBody) -> Response {
    if let Some(refused) = refused_in_show_mode(api.mode) {
        return refused;
    }
    let _turn = show_load_turn().await;
    let preview = json_body(content)
        .and_then(|content| body::<PixelPreview>(content, "send { song, cue }, cue a pixel cue"))
        .and_then(|preview| {
            let show = Show::load(&api.show_dir)?;
            let song = song_named(&show, &preview.song)?;
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

fn song_named<'show>(show: &'show Show, name: &str) -> io::Result<&'show Song> {
    show.songs
        .iter()
        .find(|song| song.name == name)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no song {name:?} in the show"),
            )
        })
}

#[derive(Deserialize)]
struct ThumbnailQuery {
    /// The song whose time signature places the cue's bars; 4/4 without one.
    song: Option<String>,
    /// Columns and rows of the sample grid; `pixel::THUMBNAIL_SIDE` without one.
    side: Option<usize>,
}

/// Works in show mode too: it renders on a sample grid and changes nothing.
async fn render_thumbnail(
    State(api): State<Api>,
    Query(query): Query<ThumbnailQuery>,
    content: JsonBody,
) -> Response {
    let _turn = show_load_turn().await;
    let side = query
        .side
        .unwrap_or(pixel::THUMBNAIL_SIDE)
        .clamp(1, pixel::MAX_THUMBNAIL_SIDE);
    // Blocking work off the async runtime: a 64 × 64 thumbnail's frames would otherwise hold a
    // runtime thread, and on a two-core machine two of them stall every other request.
    let thumbnail = tokio::task::spawn_blocking(move || {
        json_body(content).and_then(|cue| {
            let show = Show::load(&api.show_dir)?;
            let (bpm, time_signature) = match &query.song {
                Some(name) => {
                    let song = song_named(&show, name)?;
                    (song.tempo(), song.time_signature)
                }
                None => (pixel::THUMBNAIL_BPM, TimeSignature::default()),
            };
            let (cycle, frames) = pixel::thumbnail(&show, bpm, time_signature, cue, side)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            Ok(json!({
                "columns": side,
                "rows": side,
                "cycle_ms": cycle.as_secs_f64() * 1000.0,
                "frames": frames
                    .iter()
                    .map(|colours| colours.iter().map(|colour| colour.to_hex()).collect())
                    .collect::<Vec<Vec<String>>>(),
            }))
        })
    })
    .await
    .unwrap_or_else(|failure| Err(io::Error::other(failure.to_string())));
    match thumbnail {
        Ok(thumbnail) => Json(thumbnail).into_response(),
        Err(error) => error_response(&error),
    }
}

/// Levels in a swell or pattern cue's thumbnail: two per quarter beat of a 4-bar pattern.
const CUE_THUMBNAIL_LEVELS: usize = 129;

/// Works in show mode too: it renders the saved cues aside from the output and changes nothing.
async fn song_thumbnails(State(api): State<Api>, Path(song): Path<String>) -> Response {
    // One load and render for the whole song, as blocking work off the async runtime.
    let _turn = show_load_turn().await;
    let thumbnails = tokio::task::spawn_blocking(move || {
        Show::load(&api.show_dir).and_then(|show| saved_cue_thumbnails(&show, &song))
    })
    .await
    .unwrap_or_else(|failure| Err(io::Error::other(failure.to_string())));
    match thumbnails {
        Ok(thumbnails) => Json(thumbnails).into_response(),
        Err(error) => error_response(&error),
    }
}

/// `{ "<note>": <thumbnail>, … }` for every cue of `song`.
fn saved_cue_thumbnails(show: &Show, song: &str) -> io::Result<Value> {
    let song = show
        .songs
        .iter()
        .find(|entry| entry.name == song)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no song {song:?} in the show"),
            )
        })?;
    let mut thumbnails = serde_json::Map::new();
    for (note, cue) in &song.cues {
        let thumbnail = cue_thumbnail(&show.rig, song, cue).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("songs/{}.yaml: cue {note}: {error}", song.name),
            )
        })?;
        thumbnails.insert(note.to_string(), thumbnail);
    }
    Ok(Value::Object(thumbnails))
}

/// `{ "frames": [...] }`, the pixels' colours, for a cue that draws; `{ "levels": [...] }`, its
/// envelope, for a swell or pattern.
fn cue_thumbnail(rig: &Rig, song: &Song, cue: &Cue) -> Result<Value, String> {
    let colours = |out: &Universes| stream::pixel_colours(rig, &out.enttec, &out.artnet);
    let levels = |level: &dyn Fn(f64) -> f64| {
        let last = (CUE_THUMBNAIL_LEVELS - 1) as f64;
        (0..CUE_THUMBNAIL_LEVELS)
            .map(|index| level(index as f64 / last))
            .collect::<Vec<f64>>()
    };
    Ok(match cue {
        // Drawn as playback draws it, over a dark stage, at full velocity.
        Cue::Scene(_) => {
            let clock = ClockFollower::new(song.tempo(), song.time_signature);
            let mut out = Universes {
                enttec: [0; DMX_CHANNELS],
                artnet: ArtNetFrames::new(),
            };
            let mut dimmers = VirtualDimmers::of(&out.enttec);
            let started = clock.moment(Duration::ZERO);
            let timing = CueTiming {
                started,
                bar_line: None,
                released: None,
            };
            cue.draw(
                rig,
                &clock,
                timing,
                Duration::ZERO,
                1.0,
                &mut out,
                &mut dimmers,
            );
            json!({ "frames": [colours(&out)] })
        }
        Cue::Pixel(scene) => {
            let (_, frames) = pixel::cycle(
                scene,
                rig,
                pixel::THUMBNAIL_BPM,
                song.time_signature,
                pixel::PAD_THUMBNAIL_FRAMES,
            )?;
            json!({ "frames": frames.iter().map(colours).collect::<Vec<_>>() })
        }
        Cue::Swell(swell) => json!({ "levels": levels(&|time| swell.timing.curve.level(time)) }),
        // The second pass, so a hit running over the loop's end shows at its start.
        Cue::Pattern(pattern) => {
            let rhythm = &pattern.rhythm;
            json!({ "levels": levels(&|time| rhythm.level_after((1.0 + time) * rhythm.beats)) })
        }
    })
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
        io::ErrorKind::FileTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        io::ErrorKind::Unsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "error": error.to_string() }))).into_response()
}

/// What the UI puts above every layer: a look, as a fixture test's flash, or a pixel scene
/// animated from the first frame it is drawn on, at its song's tempo.
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
        // A swell that has run would show no edit to its curve or length: a new rhythm starts
        // the preview again, any other edit carries on where it is.
        let started = match (&*shown, &overlay) {
            (
                Some(Shown {
                    overlay: Overlay::Pixel { scene: shown, .. },
                    started,
                    ..
                }),
                Overlay::Pixel { scene, .. },
            ) if rhythms(shown).eq(rhythms(scene)) => *started,
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

    /// How long the pixel scene shown has animated at `at`, once a frame has drawn it.
    pub fn animated(&self, at: Duration) -> Option<Duration> {
        let shown = lock(&self.shown);
        match shown.as_ref()? {
            Shown {
                overlay: Overlay::Pixel { .. },
                started: Some(started),
                ..
            } => Some(at.saturating_sub(*started)),
            _ => None,
        }
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
                let timing = CueTiming {
                    started: clock.moment(note),
                    bar_line: Some(Pattern::start(clock, note)),
                    released: None,
                };
                scene.draw(rig, clock, timing, at, 1.0, out);
            }
        }
    }
}

fn rhythms(scene: &PixelScene) -> impl Iterator<Item = &Rhythm> {
    scene
        .layers
        .iter()
        .filter_map(|layer| layer.rhythm.as_ref())
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
        thumbnail_in(None, cue)
    }

    /// The API of an engine playing the cues show in show mode.
    fn cues_show_api() -> Api {
        Api {
            show_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues").into(),
            mode: Mode::Show,
            rig: Arc::new(watch::Sender::new(Rig::default())),
            test_flash: TimedLook::new(TEST_FLASH_LENGTH),
            preview: TimedLook::new(PREVIEW_LIFETIME),
            midi: std::sync::mpsc::channel().0,
            history: Arc::default(),
        }
    }

    /// A response's status and body.
    async fn read(response: Response) -> (StatusCode, Vec<u8>) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, body.to_vec())
    }

    const ARRANGED_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/shows/arranged"
    );

    /// A scratch copy of the arranged fixture's rig, for `test` to put songs in.
    fn arranged_rig_copy(test: &str) -> PathBuf {
        let fixture = std::path::Path::new(ARRANGED_FIXTURE);
        let dir = std::env::temp_dir().join(format!("blimey-{test}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("fixtures")).unwrap();
        std::fs::create_dir_all(dir.join("songs")).unwrap();
        for file in [
            "rig.yaml",
            "palettes.yaml",
            "fixtures/dimmer-par.yaml",
            "fixtures/rgb-par.yaml",
        ] {
            std::fs::copy(fixture.join(file), dir.join(file)).unwrap();
        }
        dir
    }

    #[test]
    fn an_upload_replaces_the_songs_audio_and_lists_its_markers() {
        let dir = arranged_rig_copy("upload");
        let song_file = dir.join("songs/song.yaml");
        let original = "program: 1\naudio: { file: audio/song.mp3, offset: 1s }\n";
        std::fs::write(&song_file, original).unwrap();
        std::fs::create_dir(dir.join("audio")).unwrap();
        std::fs::write(dir.join("audio/song.mp3"), b"ID3 old bounce").unwrap();
        let api = Api {
            show_dir: dir.clone(),
            mode: Mode::Edit,
            ..cues_show_api()
        };
        let upload = |api: &Api, song: &str, length: Option<u64>, bytes: Vec<u8>| {
            let mut headers = HeaderMap::new();
            if let Some(length) = length {
                headers.insert(header::CONTENT_LENGTH, length.into());
            }
            upload_audio(
                State(api.clone()),
                Path(song.into()),
                headers,
                Body::from(bytes),
            )
        };
        let wav = wav::build::wav(
            48_000,
            &[
                wav::build::cue_points(&[(1, 24_000)]),
                wav::build::adtl(&[wav::build::label(1, "Verse")]),
            ],
        );

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let show_mode = Api {
                mode: Mode::Show,
                ..api.clone()
            };
            let refused = upload(&show_mode, "song", None, wav.clone()).await;
            assert_eq!(refused.status(), StatusCode::FORBIDDEN);
            let refused = upload(&api, "song", Some(MAX_AUDIO_UPLOAD + 1), wav.clone()).await;
            assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
            let refused = upload(&api, "song", None, b"not audio at all".to_vec()).await;
            assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let refused = upload(&api, "song", None, wav[..wav.len() - 4].to_vec()).await;
            assert_eq!(
                refused.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "cut short"
            );
            // SONG: on a case-insensitive disk, songs/SONG.yaml is song's, and audio/SONG.wav its audio.
            for outside in ["..", "../songs/song", "nope", "SONG"] {
                let refused = upload(&api, outside, None, wav.clone()).await;
                assert_eq!(refused.status(), StatusCode::NOT_FOUND, "{outside}");
            }
            assert_eq!(std::fs::read_to_string(&song_file).unwrap(), original);
            assert_eq!(
                std::fs::read_dir(dir.join("audio")).unwrap().count(),
                1,
                "only the old bounce"
            );

            let (status, body) = read(upload(&api, "song", None, wav.clone()).await).await;
            let listed = json!({ "markers": [{ "name": "Verse", "at": "0.5s" }] });
            assert_eq!(status, StatusCode::OK);
            let mut uploaded = listed.clone();
            uploaded["file"] = "audio/song.wav".into();
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), uploaded);
            assert_eq!(std::fs::read(dir.join("audio/song.wav")).unwrap(), wav);
            assert!(
                !dir.join("audio/song.mp3").exists(),
                "the old bounce is gone"
            );
            assert_eq!(
                std::fs::read_dir(dir.join("audio")).unwrap().count(),
                1,
                "no temporary file left"
            );
            assert_eq!(
                std::fs::read_to_string(&song_file).unwrap(),
                "program: 1\naudio: { file: audio/song.wav, offset: 1s }\n"
            );
            let (status, body) =
                read(audio_markers(State(api.clone()), Path("song".into())).await).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), listed);
        });

        lock(&api.history).undo(&dir).unwrap();
        assert_eq!(
            std::fs::read_to_string(&song_file).unwrap(),
            original,
            "undo restores the song file, not the replaced bounce"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_upload_whose_client_goes_away_leaves_no_temporary_file() {
        use futures_util::{FutureExt, StreamExt, stream};
        let dir = arranged_rig_copy("upload-dropped");
        std::fs::write(dir.join("songs/song.yaml"), "program: 1\n").unwrap();
        let api = Api {
            show_dir: dir.clone(),
            mode: Mode::Edit,
            ..cues_show_api()
        };
        let first_bytes = stream::once(async { Ok::<_, io::Error>(Bytes::from_static(b"RIFF")) });
        let body = Body::from_stream(first_bytes.chain(stream::pending()));

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let upload = upload_audio(State(api), Path("song".into()), HeaderMap::new(), body);
            assert!(
                upload.now_or_never().is_none(),
                "still receiving when dropped"
            );
        });
        assert_eq!(std::fs::read_dir(dir.join("audio")).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_another_upload_of_the_song_is_removed_as_its_previous_one() {
        let audio = |file: &str| Audio {
            file: file.into(),
            offset: Duration::ZERO,
        };
        assert!(is_other_upload(
            &audio("audio/song.mp3"),
            "song",
            "audio/song.wav"
        ));
        for kept in [
            "audio/song.WAV",
            "audio/song.wav",
            "audio/song-final.mp3",
            "audio/live/song.mp3",
        ] {
            assert!(
                !is_other_upload(&audio(kept), "song", "audio/song.wav"),
                "{kept}"
            );
        }
    }

    #[test]
    fn audio_is_served_whole_or_by_range_and_a_missing_file_is_not_found() {
        let dir = arranged_rig_copy("audio");
        std::fs::write(
            dir.join("songs/song.yaml"),
            "program: 1\naudio: { file: audio/song.wav, offset: 1s }\n",
        )
        .unwrap();
        std::fs::write(dir.join("songs/silent.yaml"), "program: 2\n").unwrap();
        let api = Api {
            show_dir: dir.clone(),
            ..cues_show_api()
        };
        let get = |song: &str, range: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(range) = range {
                headers.insert(header::RANGE, range.parse().unwrap());
            }
            song_audio(State(api.clone()), Path(song.into()), headers)
        };

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            assert_eq!(
                get("song", None).await.status(),
                StatusCode::NOT_FOUND,
                "no file yet"
            );
            assert_eq!(
                get("silent", None).await.status(),
                StatusCode::NOT_FOUND,
                "no audio"
            );
            std::fs::create_dir(dir.join("audio")).unwrap();
            std::fs::write(dir.join("audio/song.wav"), b"0123456789").unwrap();

            let whole = get("song", None).await;
            assert_eq!(whole.headers()[header::ACCEPT_RANGES], "bytes");
            assert_eq!(whole.headers()[header::CONTENT_TYPE], "audio/wav");
            assert_eq!(read(whole).await, (StatusCode::OK, b"0123456789".to_vec()));

            for (range, part, content_range) in [
                ("bytes=2-5", &b"2345"[..], "bytes 2-5/10"),
                ("bytes=7-", b"789", "bytes 7-9/10"),
                ("bytes=-3", b"789", "bytes 7-9/10"),
                ("bytes=8-99", b"89", "bytes 8-9/10"),
            ] {
                let response = get("song", Some(range)).await;
                assert_eq!(response.headers()[header::CONTENT_RANGE], content_range);
                assert_eq!(
                    read(response).await,
                    (StatusCode::PARTIAL_CONTENT, part.to_vec()),
                    "{range}"
                );
            }
            let beyond = get("song", Some("bytes=10-")).await;
            assert_eq!(beyond.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(beyond.headers()[header::CONTENT_RANGE], "bytes */10");
            let (status, body) = read(get("song", Some("bytes=0-1,4-5")).await).await;
            assert_eq!(
                (status, body.len()),
                (StatusCode::OK, 10),
                "a list: all of it"
            );
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_arrangement_saves_round_trip_and_an_invalid_one_leaves_the_file_untouched() {
        let fixture = std::path::Path::new(ARRANGED_FIXTURE);
        let dir = arranged_rig_copy("arrangement");
        let song_file = dir.join("songs/arranged.yaml");
        let arranged = std::fs::read_to_string(fixture.join("songs/arranged.yaml")).unwrap();
        let unarranged = &arranged[..arranged.find("sections:").unwrap()];
        std::fs::write(&song_file, unarranged).unwrap();
        let content: Value = serde_yaml_ng::from_str(&arranged).unwrap();
        let api = Api {
            show_dir: dir.clone(),
            mode: Mode::Edit,
            ..cues_show_api()
        };
        let put = |body: Value| {
            set_arrangement(State(api.clone()), Path("arranged".into()), Ok(Json(body)))
        };

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let saved = put(json!({ "sections": content["sections"], "arrangement": content["arrangement"] })).await;
            assert_eq!(saved.status(), StatusCode::NO_CONTENT);
            assert_eq!(std::fs::read_to_string(&song_file).unwrap(), arranged);

            let (status, error) = read(
                put(json!({ "sections": content["sections"], "arrangement": ["verse", "outro"] })).await,
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                String::from_utf8_lossy(&error).contains(r#"arrangement: no section \"outro\" in sections"#),
                "{}",
                String::from_utf8_lossy(&error)
            );
            assert_eq!(std::fs::read_to_string(&song_file).unwrap(), arranged);

            let (status, events) =
                read(arrangement_events(State(api.clone()), Path("arranged".into())).await).await;
            assert_eq!(status, StatusCode::OK);
            let events: Value = serde_json::from_slice(&events).unwrap();
            assert_eq!(events["bpm"], 120.0);
            assert_eq!(
                events["events"][4],
                json!({ "beats": 6.0, "seconds": 3.0, "type": "note_on", "scene": 37, "velocity": 64 })
            );

            let (status, file) =
                read(arrangement_midi(State(api.clone()), Path("arranged".into())).await).await;
            assert_eq!(status, StatusCode::OK);
            let show = Show::load(&dir).unwrap();
            assert_eq!(file, arrangement::midi_file(&show.songs[0]).unwrap());

            let named = "café \"live\"";
            std::fs::write(
                dir.join(format!("songs/{named}.yaml")),
                arranged.replace("program: 1", "program: 2"),
            )
            .unwrap();
            let download = arrangement_midi(State(api.clone()), Path(named.into())).await;
            assert_eq!(download.status(), StatusCode::OK);
            assert_eq!(
                download.headers()[header::CONTENT_DISPOSITION],
                "attachment; filename=\"caf___live_.mid\"; filename*=UTF-8''caf%C3%A9%20%22live%22.mid"
            );
            std::fs::remove_file(dir.join(format!("songs/{named}.yaml"))).unwrap();

            assert_eq!(undo(State(api.clone())).await.status(), StatusCode::OK);
            assert_eq!(std::fs::read_to_string(&song_file).unwrap(), unarranged);
            let (status, _) =
                read(arrangement_midi(State(api.clone()), Path("arranged".into())).await).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "no arrangement once undone");
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn show_loads_past_the_limit_wait_their_turn() {
        let cue = json!({ "name": "t", "type": "pixel" });
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let every_turn = SHOW_LOADS.acquire_many(2).await.unwrap();
            let query = Query(ThumbnailQuery {
                song: None,
                side: None,
            });
            let request = render_thumbnail(State(cues_show_api()), query, Ok(Json(cue)));
            tokio::pin!(request);

            let ran = tokio::select! {
                biased;
                _ = &mut request => true,
                () = tokio::task::yield_now() => false,
            };
            drop(every_turn);

            assert!(!ran, "rendered while two others held every turn");
            assert_eq!(request.await.status(), StatusCode::OK);
        });
    }

    /// As `thumbnail`, with `?song=` when `song` is given.
    fn thumbnail_in(song: Option<&str>, cue: Value) -> (StatusCode, Value) {
        let api = cues_show_api();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let query = ThumbnailQuery {
                song: song.map(str::to_owned),
                side: None,
            };
            let response = render_thumbnail(State(api), Query(query), Ok(Json(cue))).await;
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, serde_json::from_slice(&body).unwrap())
        })
    }

    #[test]
    fn a_thumbnail_is_rendered_at_the_side_asked_for_capped_at_the_finest_grid() {
        let render = |side: usize| {
            let cue = json!({ "name": "t", "type": "pixel", "shape": "circle", "params": { "color": "red" } });
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let query = ThumbnailQuery {
                    song: None,
                    side: Some(side),
                };
                let response =
                    render_thumbnail(State(cues_show_api()), Query(query), Ok(Json(cue))).await;
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, serde_json::from_slice::<Value>(&body).unwrap())
            })
        };

        let (status, finest) = render(pixel::MAX_THUMBNAIL_SIDE);
        assert_eq!(status, StatusCode::OK, "{finest}");
        assert_eq!(finest["columns"], 64);
        assert_eq!(finest["frames"][0].as_array().unwrap().len(), 64 * 64);

        let (status, capped) = render(1000);
        assert_eq!(status, StatusCode::OK, "{capped}");
        assert_eq!(
            capped["columns"], 64,
            "a finer grid than the engine renders is capped"
        );
    }

    #[test]
    fn a_thumbnail_renders_one_cycle_on_the_sample_grid_with_the_shows_palette() {
        let cue = |effect: &str| json!({ "name": "t", "type": "pixel", "effect": effect, "params": { "color": "red", "width": 0.25, "speed": "1 bar" } });

        let (status, held) = thumbnail(cue("static"));
        assert_eq!(status, StatusCode::OK, "{held}");
        assert_eq!(
            (held["columns"].as_u64(), held["rows"].as_u64()),
            (Some(32), Some(32))
        );
        let frames = held["frames"].as_array().unwrap();
        assert_eq!(frames.len(), pixel::THUMBNAIL_FRAMES);
        assert_eq!(held["cycle_ms"], 2000.0, "1 bar at 120 BPM, to play it at");
        let red = |frame: &Value, index: usize| frame[index] == "#ff0000";
        let top_row: Vec<usize> = (0..32).filter(|column| red(&frames[0], *column)).collect();
        assert_eq!(
            top_row,
            (12..20).collect::<Vec<_>>(),
            "a quarter-canvas bar mid-way, in the palette's red"
        );
        assert!(
            frames.iter().all(|frame| frame == &frames[0]),
            "static holds still"
        );
        assert_eq!(held["cycle_ms"], 2000.0, "a bar at 120 BPM in 4/4");
        let (_, in_5_4) = thumbnail_in(Some("second"), cue("move"));
        assert_eq!(in_5_4["cycle_ms"], 2500.0, "a bar in the song's 5/4");

        let (_, chased) = thumbnail(cue("chase"));
        let first_lit = |frame: &Value| (0..32).find(|column| red(frame, *column));
        let steps: Vec<Option<usize>> = chased["frames"]
            .as_array()
            .unwrap()
            .iter()
            .step_by(4)
            .map(first_lit)
            .collect();
        assert_eq!(
            steps,
            [
                Some(0),
                Some(0),
                Some(8),
                Some(8),
                Some(16),
                Some(16),
                Some(24),
                Some(24)
            ],
            "four steps of a quarter over the cycle"
        );

        let (status, layered) = thumbnail(json!({ "name": "t", "type": "pixel", "layers": [
            { "area": { "x": 0, "y": 0, "width": 0.5, "height": 1 }, "shape": "fill", "params": { "color": "red" } },
        ] }));
        assert_eq!(status, StatusCode::OK, "{layered}");
        let lit: Vec<bool> = (0..32)
            .map(|column| red(&layered["frames"][0], column))
            .collect();
        assert_eq!(
            lit,
            [[true; 16], [false; 16]].concat(),
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
    fn a_songs_thumbnails_draw_each_cue_on_the_rig_or_return_its_envelope() {
        let mut show = Show::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/four-washes"
        )))
        .unwrap();
        let song = "program: 3\ncues:\n\
            \x20 37: { name: outer-blue, type: scene, values: { all-wash: { color: warm-white }, outer-wash: { color: deep-blue } } }\n\
            \x20 39: { name: rise, type: swell, target: { all-wash: { dimmer: 100% } }, duration: 4 bars, curve: ease-in }\n\
            \x20 40: { name: chase, type: pixel, effect: chase, params: { color: deep-blue, width: 0.2 } }\n\
            \x20 42:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { all-wash: dimmer }\n    hits:\n\
            \x20     - { at: 1, shape: ramp-up, length: 0.5 beats, level: 100% }\n\
            \x20     - { at: 2, shape: sine, length: 0.5 beats, level: 80% }\n";
        let song = Song::parse("demo", song, &show).unwrap();
        show.songs.push(song);
        let thumbnails = saved_cue_thumbnails(&show, "demo").unwrap();
        assert_eq!(
            thumbnails.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["37", "39", "40", "42"]
        );
        let thumbnail = |note: u8| thumbnails[note.to_string()].clone();

        let outer_blue = thumbnail(37);
        let frames = outer_blue["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 1, "a scene holds still");
        let first_pixel = |fixture: &str| frames[0][fixture][0].as_str().unwrap().to_string();
        assert_eq!(first_pixel("wash-1"), "#0028ff");
        assert_eq!(first_pixel("wash-4"), "#0028ff");
        assert_eq!(first_pixel("wash-2"), "#ffd2a0");
        assert_eq!(frames[0]["wash-3"].as_array().unwrap().len(), 4);

        let chase = thumbnail(40);
        let frames = chase["frames"].as_array().unwrap();
        assert_eq!(frames.len(), pixel::PAD_THUMBNAIL_FRAMES);
        assert_ne!(frames[0], frames[4], "the chase moves over its cycle");

        let swell = thumbnail(39);
        let levels = swell["levels"].as_array().unwrap();
        assert_eq!(levels.len(), CUE_THUMBNAIL_LEVELS);
        assert_eq!(
            [&levels[0], &levels[64], &levels[128]],
            [0.0, 0.25, 1.0],
            "ease-in"
        );

        let pattern = thumbnail(42);
        let level = |beat: f64| pattern["levels"][(beat / 4.0 * 128.0) as usize].as_f64();
        assert_eq!(level(0.25), Some(0.5), "halfway up the first ramp");
        assert_eq!(level(0.75), Some(0.0), "dark between hits");
        assert_eq!(level(1.25), Some(f64::from(0.8_f32)), "the sine's peak");

        let error = saved_cue_thumbnails(&show, "encore").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[test]
    fn a_thumbnail_reads_a_layer_pattern_in_its_songs_time_signature() {
        // Beat 5 is in a bar of 5/4, the second song's, and past a bar of 4/4.
        let cue = json!({ "name": "t", "type": "pixel", "layers": [
            { "shape": "fill", "pattern": { "length": "1 bar", "hits": [{ "at": 5, "shape": "hold", "length": "1 beat", "level": "100%" }] } },
        ] });

        let (status, in_5_4) = thumbnail_in(Some("second"), cue.clone());
        assert_eq!(status, StatusCode::OK, "{in_5_4}");
        let (status, in_4_4) = thumbnail(cue.clone());
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{in_4_4}");
        let (status, _) = thumbnail_in(Some("third"), cue);
        assert_eq!(status, StatusCode::NOT_FOUND);
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
        assert_eq!(
            preview.animated(millis(10_700)),
            Some(millis(700)),
            "the stream's preview_ms, for the builder's shapes to follow"
        );
        preview.clear();
        assert_eq!(preview.animated(millis(10_700)), None);
    }

    #[test]
    fn a_pixel_preview_starts_again_when_a_layer_rhythm_changes() {
        let rig = Rig::parse(
            "fixtures:\n  - { name: dot, type: pixel-grid, columns: 1, rows: 1, colour_order: RGB, address: 1, canvas: { x: 0.5, y: 0.5 } }\n",
            std::path::Path::new("."),
        )
        .unwrap();
        let overlay = |color: &str, duration: &str| {
            Overlay::Pixel {
            scene: Box::new(
                PixelScene::parse(
                    &Show::default(),
                    Default::default(),
                    json!({ "name": "rise", "type": "pixel", "layers": [
                        { "shape": "fill", "params": { "color": color }, "swell": { "duration": duration } },
                    ] }),
                )
                .unwrap(),
            ),
            clock: ClockFollower::new(120.0, Default::default()),
        }
        };
        let preview = TimedLook::new(PREVIEW_LIFETIME);
        let lit = |seconds: u64| {
            let mut out = Universes {
                enttec: [0; 512],
                artnet: rig.artnet_frames(),
            };
            preview.apply(&rig, Duration::from_secs(seconds), &mut out);
            out.enttec[0..3].to_vec()
        };

        preview.show(overlay("#ff0000", "2s"));
        assert_eq!(lit(10), [0, 0, 0], "the swell starts on the first frame");
        assert_eq!(lit(12), [255, 0, 0], "and ends 2 s later");
        preview.show(overlay("#00ff00", "2s"));
        assert_eq!(
            lit(13),
            [0, 255, 0],
            "a new colour keeps the swell where it is"
        );
        preview.show(overlay("#00ff00", "4s"));
        assert_eq!(lit(14), [0, 0, 0], "a new swell starts again");
        assert_eq!(lit(16), [0, 128, 0]);
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
