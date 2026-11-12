//! WebSocket stream of engine state and output values, at `ws://<address>/ws`.
//! The message format is the contract in docs/api.md.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use serde_json::{Value, json};
use strata_engine::rig::Rig;
use strata_engine::{DmxSink, Frame};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::watch;

const PROTOCOL_VERSION: u32 = 1;

/// A `DmxSink` that publishes each frame, with its pixel preview colours, to the stream.
pub struct StreamSink {
    rig: Rig,
    frames: watch::Sender<Arc<str>>,
    /// Serves the stream; dropping the sink stops it.
    _server: Runtime,
}

impl StreamSink {
    /// Binds `address`, prints `streaming on ws://<address>/ws`, then serves in the
    /// background: the `rig` message on connect, then the latest `frame` message each time
    /// one is published. Slow clients skip frames rather than lag.
    pub fn start(address: SocketAddr, rig: &Rig) -> io::Result<StreamSink> {
        let server = Runtime::new()?;
        let listener = server.block_on(TcpListener::bind(address))?;
        println!("streaming on ws://{}/ws", listener.local_addr()?);
        let (frames, receiver) = watch::channel(Arc::from(""));
        let stream = Stream {
            rig: rig_message(rig).to_string().into(),
            frames: receiver,
        };
        let app = Router::new().route("/ws", get(upgrade)).with_state(stream);
        server.spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                eprintln!("stream: stopped serving: {error}");
            }
        });
        Ok(StreamSink {
            rig: rig.clone(),
            frames,
            _server: server,
        })
    }
}

impl DmxSink for StreamSink {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        self.frames
            .send_replace(frame_message(&self.rig, at, frame).to_string().into());
        Ok(())
    }
}

#[derive(Clone)]
struct Stream {
    rig: Arc<str>,
    frames: watch::Receiver<Arc<str>>,
}

async fn upgrade(socket: WebSocketUpgrade, State(stream): State<Stream>) -> Response {
    socket.on_upgrade(|socket| send_stream(socket, stream))
}

async fn send_stream(mut socket: WebSocket, mut stream: Stream) {
    if socket
        .send(Message::Text(stream.rig.as_ref().into()))
        .await
        .is_err()
    {
        return;
    }
    while stream.frames.changed().await.is_ok() {
        let message = stream.frames.borrow_and_update().clone();
        if socket
            .send(Message::Text(message.as_ref().into()))
            .await
            .is_err()
        {
            return;
        }
    }
}

fn rig_message(rig: &Rig) -> Value {
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
        "fixtures": fixtures,
        "groups": rig.groups,
    })
}

fn frame_message(rig: &Rig, at: Duration, frame: &Frame) -> Value {
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
    })
}
