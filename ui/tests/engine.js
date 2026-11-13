// Starts real engines for the UI tests: `blimey fire --listen` with no DMX device, so frames go
// only to the stream. Each test gets a scratch copy of a show to write to: four-washes unless it
// sets the `showSource` option.

import { spawn } from 'node:child_process';
import { cpSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import { test as base } from '@playwright/test';

const ROOT = path.resolve(import.meta.dirname, '../..');
export const BINARY = process.env.BLIMEY_BIN ?? path.join(ROOT, 'target/debug/blimey');

/** wash-1 saturated red; wash-4 orange at 60% with its white channels in use; wash-2 and 3 dark. */
const LOOK = [
  '--set', 'wash-1', 'color=#ff0000',
  '--set', 'wash-4', 'color=#ff8040', 'dimmer=60%', 'rgbw=white-rich',
];

function start(show, { listen = '127.0.0.1:0', edit = true, look = LOOK } = {}) {
  const args = ['fire', show, ...look, '--listen', listen, ...(edit ? ['--edit'] : [])];
  const child = spawn(BINARY, args, { stdio: ['ignore', 'pipe', 'inherit'] });
  const exited = new Promise((resolve) => child.once('exit', resolve));
  const address = new Promise((resolve, reject) => {
    child.once('error', (error) => reject(new Error(`${BINARY}: ${error.message}; run \`cargo build\` first`)));
    exited.then((code) => reject(new Error(`blimey exited with ${code} before listening`)));
    readline.createInterface({ input: child.stdout }).on('line', (line) => {
      const match = line.match(/^streaming on ws:\/\/(\S+)\/ws$/);
      if (match) resolve(match[1]);
    });
  });
  const stop = () => {
    child.kill();
    return exited;
  };
  return address.then((address) => ({ address, url: `http://${address}/`, stop }));
}

/** The DMX output of the next frame the engine at `address` streams. */
export function nextDmx(address) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(`ws://${address}/ws`);
    socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.type !== 'frame') return;
      socket.close();
      resolve(message.dmx);
    };
    socket.onerror = () => reject(new Error(`no stream at ws://${address}/ws`));
  });
}

export const test = base.extend({
  /** The show each test copies, from the repository root. */
  showSource: ['tests/fixtures/shows/four-washes', { option: true }],
  show: async ({ showSource }, use) => {
    const show = mkdtempSync(path.join(tmpdir(), 'blimey-ui-'));
    cpSync(path.join(ROOT, showSource), show, { recursive: true });
    await use(show);
    rmSync(show, { recursive: true });
  },
  /** `startEngine(options)` runs an engine on the test's show; every one is stopped after the test. */
  startEngine: async ({ show }, use) => {
    const engines = [];
    await use(async (options) => {
      const engine = await start(show, options);
      engines.push(engine);
      return engine;
    });
    await Promise.all(engines.map((engine) => engine.stop()));
  },
});

export { expect } from '@playwright/test';
