// The engine as the UI sees it: the WebSocket stream and the HTTP API of docs/api.md.

const RECONNECT_DELAY_MS = 1000;
const MIDI_LOG_LENGTH = 100;

class Engine {
  connected = $state(false);
  /** The last `rig` message. */
  rig = $state.raw(null);
  /** The last `frame` message. */
  frame = $state.raw(null);
  /** The latest `midi` messages, newest first. */
  midi = $state.raw([]);
  /** Frames received in the last second; below 40 when this page skips frames. */
  framesPerSecond = $state(0);
}

export const engine = new Engine();

let frameTimes = [];

function connect() {
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  const socket = new WebSocket(`${scheme}://${location.host}/ws`);
  socket.onopen = () => (engine.connected = true);
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'rig') engine.rig = message;
    if (message.type === 'midi') engine.midi = [message, ...engine.midi].slice(0, MIDI_LOG_LENGTH);
    if (message.type === 'frame') {
      engine.frame = message;
      countFrame();
    }
  };
  // Fires when a connection drops and when one can't be made, so this retries until the engine is back.
  socket.onclose = () => {
    engine.connected = false;
    frameTimes = [];
    engine.framesPerSecond = 0;
    setTimeout(connect, RECONNECT_DELAY_MS);
  };
}

function countFrame() {
  const now = performance.now();
  frameTimes.push(now);
  while (frameTimes[0] < now - 1000) frameTimes.shift();
  engine.framesPerSecond = frameTimes.length;
}

connect();

/** Calls the HTTP API; throws the engine's error message on failure. */
export async function request(method, path, body) {
  const response = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const refusal = await response.json().catch(() => null);
    throw new Error(refusal?.error ?? `${method} ${path} failed with HTTP ${response.status}`);
  }
  return response.status === 204 ? null : response.json();
}
