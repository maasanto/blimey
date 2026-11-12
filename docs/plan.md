# Fossile lighting engine — plan & spec (v1)

Nov 12, 2026 · @Antoine

Working name: **Strata** (placeholder). A small, headless lighting engine driven by Reaper, replacing ENTTEC EMU for Fossile's show. Companion to the market survey doc.

**Scope and pace:** version 1 includes the full scene builder (pixel scenes, swells in musical time, pattern sequencer). With an agent writing the code, 8 weeks is feasible. What the agent can't speed up is time with the real lights: checking looks by eye, hardware tests on the M1, reprogramming songs, rehearsals. The plan limits rig time to one bench session and one rehearsal (section 15), validates everything else on the DMX output, and keeps a go/no-go rehearsal with EMU as the fallback.

## 1. Problems this must fix

| Past problem | Requirement in Strata |
| --- | --- |
| MTD-1024 crashed mid-show | Engine survives 4 h soak; watchdog restart in about 1 s; lights hold last look meanwhile |
| Show Buddy / EMU plugin-to-app link unreliable (M1 scene switching) | No plugin. Reaper sends MIDI to a macOS virtual port; the engine alone owns the DMX interface |
| EMU swells drift (free-running oscillators) | Every cue and effect starts its clock at the triggering note. Same MIDI in, same DMX out, every time |
| EMU doesn't reliably save | Shows are plain text files in git; atomic writes; engine never writes show files while in show mode |
| EMU resets intensities unexpectedly | No hidden state: output is a pure function of show file + MIDI received since song start |
| Cumbersome programming | Named cues in a readable file, hot reload, live 2D preview |

## 2. Decisions taken

- **Audience:** Fossile first; open-source once it has survived gigs.
- **Authoring:** timing in Reaper MIDI items; a scene builder in the web UI (pixel scenes, swells, pattern sequencer); everything it makes is saved as text files.
- **Pixel mapping:** native from v1, generative effects only (no image/video, no audio-reactive). Must cover the 4 WildWash plus pixel fixtures to come.
- **Seeking:** event-driven. After a jump in Reaper, lights catch up at the next cue.
- **Reaper link:** virtual MIDI port (macOS IAC bus).
- **Stack:** Rust engine, web UI served by the engine.
- **Failure mode:** hold last look, auto-restart.
- **Cue language:** legacy raw notes stay as they are; new work uses named cues on a second MIDI channel.
- **Preview:** 2D stage plan in the web UI; Art-Net mirror so BlenderDMX gives 3D for free.
- **Deadline:** first gig within about 2 months.

## 3. Non-goals for v1

Moving heads, image/video mapping, audio-reactive effects, built-in 3D, a DAW plugin, a standalone timeline, Windows/Linux packaging, exact state after seeking.

## 4. Architecture

```
Reaper (timeline, MIDI items)
   │  MIDI over IAC virtual port
   ▼
Strata engine (one Rust process)
   ├─ MIDI input thread
   ├─ render loop, fixed 40 Hz
   │    layers: legacy raw → cues/scenes → pixel effects → master dimmer
   ├─ output: Enttec DMX USB Pro (serial)   ← sole owner of the device
   ├─ output: Art-Net mirror (localhost)    → BlenderDMX 3D
   └─ HTTP + WebSocket on localhost         → web UI, CLI, later MCP
Show files (YAML, git)  ← edited by hand, by the UI, or by an agent
```

Principles:

- **One process, one owner of the hardware.** Nothing else opens the Enttec box.
- **Deterministic render.** Each frame is computed from the show file, the events received and a monotonic clock. No free-running oscillators; random effects use a fixed seed per cue.
- **Show mode vs edit mode.** In show mode the engine is read-only on disk and ignores UI edits.
- **Suggested crates:** `midir` (MIDI), `serialport` (Enttec), `axum` + `tokio` (HTTP/WebSocket), `serde` + YAML, JSON Schema for validation. To be confirmed during the week-1 spike.

## 5. MIDI protocol (Reaper → engine)

| Channel | Message | Meaning |
| --- | --- | --- |
| 1 | Note on/off | **Legacy.** Note = DMX channel via a mapping table, velocity × 2 = level, note-off = 0. Reproduces the current sessions unchanged (e.g. notes 37–52 → channels 6–21) |
| 2 | Program Change | Select song (loads that song's cue table; preloaded, switch is instant) |
| 2 | Note on | Fire named cue: note number = cue slot in the current song, velocity = intensity scale |
| 2 | Note off | Release the cue (held cues fade out with their release time; one-shot cues ignore it) |
| 2 | CC 7 | Master dimmer |
| 2 | CC 20–27 | Eight free live parameters (e.g. effect speed), assignable per song |
| 16 | Note 127 | Panic: clear all layers, go to the song's safe look |
| any | MIDI clock | Tempo for beat-length durations; without it, durations use the song's BPM from the show file |

Layer priority: pixel effects and cues override legacy raw values on the channels they touch; master dimmer scales everything last.

## 6. Show files

One folder per show, in git:

```
show/
  rig.yaml          # fixtures, addresses, positions on the 2D stage plan
  legacy-map.yaml   # channel-1 note ranges → DMX start addresses
  palettes.yaml     # named colours (incl. Lee gel references)
  songs/tell-me.yaml
```

Song example:

```yaml
song: tell-me
program: 3          # MIDI program number
bpm: 92
safe_look: warm-wash-30
cues:
  36:               # note C1 on channel 2
    name: verse-blue
    type: scene
    fade_in: 2s
    values: { all-wash: { color: deep-blue, dimmer: 60% } }
  38:
    name: chorus-swell
    type: swell
    target: { all-wash: { dimmer: 100% } }
    duration: 4 bars
    curve: ease-in
    release: 500ms
  40:
    name: bridge-chase
    type: pixel
    effect: chase
    params: { color: amber, width: 0.2, speed: 1 bar, direction: left-to-right }
    seed: 7
```

A pattern cue, as saved by the sequencer grid:

```yaml
  42:
    name: verse-pulse
    type: pattern
    length: 1 bar
    target: { all-wash: dimmer }
    hits:
      - { at: 1,   shape: swell-up, length: 0.5 beats, level: 100% }
      - { at: 2,   shape: sine,     length: 0.5 beats, level: 80% }
      - { at: 3.5, shape: swell-up, length: 0.5 beats, level: 100% }
```

- Durations accept `ms`, `s`, `beats`, `bars`.
- A JSON Schema ships with the engine; `strata validate` checks a show before it is loaded.
- Hot reload in edit mode: save the file, the look updates.

## 7. Fixtures and pixel mapping

- **Fixture definitions:** imported from Open Fixture Library JSON (GDTF import later). The WildWash Pro 648 16-channel profile must be verified channel by channel, given the attribute errors seen in EMU.
- **Pixels:** a fixture exposes one or more pixels (a whole wash = 1 pixel; a bar = N pixels). Each pixel has a position on a normalised 2D canvas (0–1 on both axes), set in `rig.yaml` or by dragging in the web UI.
- **Rendering:** each frame, the active effect is evaluated at every pixel position and converted to that fixture's channels.
- **v1 effects:** solid, gradient, chase, wave, noise, sparkle, strobe. Common parameters: colours, speed, width, direction, phase offset. Up to 2 effect layers with blend (max, add, mask).
- **Colour:** effects work in RGB; per-fixture conversion to RGBW with two modes, *saturated* (W = 0) and *white-rich*, selectable per cue.
- **Scale target:** 512 channels, a few hundred pixels at 40 Hz with large margin.

**Pixel grids and columns.** The pixel fixtures to come aren't chosen yet, so the engine supports any RGB or RGBW grid as a generic fixture type, with no fixture profile needed:

```yaml
- name: back-grid
  type: pixel-grid
  columns: 16
  rows: 8
  colour_order: RGB        # RGB, GRB, RGBW, ...
  wiring: serpentine       # or row-by-row; plus start corner
  start_corner: bottom-left
  output: artnet           # or enttec
  universe: 2
  address: 1
  canvas: { x: 0.2, y: 0.5, width: 0.6, height: 0.4 }
```

A column is a grid with one column. One universe holds 170 RGB or 128 RGBW pixels; a larger grid continues on the next universe. The Enttec box carries a single universe shared with the washes, so bigger grids go out over Art-Net to a node.

## 8. Scene builder and web UI

The builder is version 1's main interface. Everything it makes is saved into the song's YAML file, and changes show live on the stage preview and the real lights.

### Scene builder

1. **Pixel scenes.** Pick a preset shape (bar, sweep, circle, split, gradient, fixtures one by one), an effect that animates it (static, move, pulse, chase, sparkle), colours from the palettes and a speed in beats. Up to 2 layers.
2. **Swells.** An envelope with a shape (ramp up, ramp down, sine, hold, or custom points) and a length in beats or bars. Target: a group's dimmer or colour, or the intensity of a pixel scene.
3. **Pattern sequencer.** A grid of 1 to 4 bars, resolution down to a quarter beat. Place hits on steps; each hit has its own shape, length and level. Example: swell-up on beat 1, sine on beat 2, swell-up on beat 3.5. The pattern loops until the cue is released and restarts on the bar when triggered.
4. **Static looks.** Colour picker and dimmer per fixture or group, saved as a scene.
5. **Cue assignment.** Name, note number, fade-in and release times, and an audition button that fires the cue without Reaper (with a built-in metronome tempo when Reaper isn't playing).

### Utility pages

- **Monitor:** engine health, MIDI in log, current song and active cues, DMX output values.
- **Stage plan:** 2D live preview of every pixel; drag to position fixtures.
- **Patch:** fixtures, addresses, modes; test a fixture.

Not in v1: a shared template library beyond the built-in presets, more than 2 effect layers, custom-drawn shapes.

## 9. Reliability

- **Watchdog:** engine runs under launchd with auto-restart; target restart under 1 s. On start it reloads the show and the last selected song, then waits for the next cue.
- **Hold last look:** the Enttec DMX USB Pro should keep transmitting its last frame while the engine restarts. To be verified in the week-1 spike; if it doesn't, a tiny separate keeper process owns the port instead.
- **No surprises on stage:** show mode locks files, disables hot reload and UI edits.
- **Logging:** every MIDI event and cue start with timestamps, for post-gig diagnosis.
- **Tests:**
  - Replay tests: recorded MIDI of a song in, DMX frames out, compared against a stored reference.
  - Swell timing: measured duration within one frame (25 ms) of the programmed one, 100 runs.
  - Soak: full set looped for 4 h on the M1 with the complete Reaper session running.
  - Fault injection: kill the engine mid-song, unplug/replug USB, close and reopen Reaper.

## 10. Agent-friendly surface

- Text show files with a published schema are the main interface.
- CLI: `strata validate`, `strata render song.yaml --midi take.mid` (offline render to a frame dump or preview), `strata fire <cue>`.
- HTTP API mirrors the CLI. An MCP server wrapping it comes after the first gig.

## 11. Plan (8 weeks)

| Week | Milestone | Done when |
| --- | --- | --- |
| 1 | **Spike** on the M1: IAC MIDI in → legacy notes → Enttec out; follow Reaper's MIDI clock | An existing legacy song runs from Reaper with EMU closed; hold-last-look behaviour measured |
| 2 | Engine core: fixtures, show files, named cues, envelopes in musical time, pattern engine, song select, panic | Swell timing test passes; the beat 1 / 2 / 3.5 pattern runs from a file |
| 3 | Builder part 1: swell editor, sequencer grid, static looks, cue assignment | You program a song section without opening a file |
| 4 | Pixel canvas, preset shapes, effects, RGBW conversion, stage preview, Art-Net mirror | Shapes and effects run from a file on the real lights; BlenderDMX shows them |
| 5 | Builder part 2: pixel scene editor | One EMU pixel section rebuilt in the builder |
| 6 | Watchdog, show mode, fault injection; start migrating songs | Fault-injection tests pass |
| 7 | Migrate the set; soak test; **go/no-go rehearsal** | Full set, twice, no intervention |
| 8 | Buffer, fixes, gig | — |

**Go/no-go rule:** if the week-7 rehearsal isn't clean, the gig runs on EMU and Strata's first gig moves to the next date. Decide this in advance, not on the day.

**Cut order if late:** second effect layer → custom envelope points → Art-Net mirror → stage-plan dragging → pixel scene editor (pixel songs then stay on EMU for the first gig). Never cut: legacy notes, swells and sequencer, watchdog, soak test.

**After the first gig (v1.1+):** scene template library, GDTF import, MCP server, exact state after seek, built-in 3D, public release.

## 12. Risks

- **Timeline vs scope** (high): engine plus a full scene builder in 8 weeks; the limit is your hours with the real lights, not coding time, part of it over the holidays. Mitigation: cut order and go/no-go rule above.
- **Rust for a first project** (medium): agent-written code you still need to debug at a rehearsal. Mitigation: small codebase, heavy replay tests, clear logs.
- **Pixel fixtures not chosen yet** (medium): covered by generic RGB(W) grid support; a grid beyond one universe needs an Art-Net node, which is extra hardware to buy and test.
- **Reprogramming pixel songs** (medium): EMU pixel sections must be rebuilt by eye; no import.
- **Enttec behaviour on host restart** (low, tested week 1).

## 13. Open questions

1. Pixel fixtures: not chosen yet, expected to be an RGB(W) pixel grid or column. Generic grid support (section 7) covers it; the exact model only matters when patching.
2. Does the WildWash 648 offer a segmented mode worth using as several pixels, or is it one pixel per wash?
3. Song selection: Program Change per song as specified, or one Reaper project per song with the song named some other way?
4. Which EMU pixel sections exist today (song, section, effect)? This is the acceptance list for weeks 4–5.
5. Licence for the later public release.

## 14. Work packages for agents

Fourteen vertical chunks. Each one delivers a behaviour end to end (input → engine → output, plus UI where relevant), has automated acceptance tests that run without hardware, and is validated by agents on the DMX output (section 15). Checks that need you are kept to a minimum, and most need only the laptop.

### Ground rules for every chunk

- **Repo layout:** `engine/` (Rust library: all logic), `strata/` (Rust binary: CLI, HTTP, device drivers), `ui/` (Svelte + Vite, built to static files and embedded in the binary), `schema/` (JSON Schema for show files), `shows/fossile/` (the real show), `tests/fixtures/` (MIDI files and reference frame dumps), `docs/` (API contract, decisions).
- **No hardware in tests.** The engine takes an injectable clock, a MIDI event source and a DMX sink. Tests use a MIDI-file source and a `FakeDmx` sink that records frames. Real drivers (IAC, Enttec, Art-Net) sit behind the same traits.
- **Replay tests are the acceptance format:** MIDI file + show folder in, frame dump out, compared to a stored reference. A chunk that changes output on purpose regenerates its references and says so in the PR.
- **One chunk = one branch = one PR.** CI runs `cargo test`, `cargo clippy`, schema validation of `shows/`, and UI tests (Playwright against the engine with `FakeDmx`).
- **Contracts first.** A chunk that adds an HTTP/WebSocket endpoint or a show-file field updates `docs/api.md` and `schema/` in the same PR. UI chunks code against the contract, not engine internals.
- **Files written by the builder** go through one engine endpoint that validates, writes atomically and keeps canonical formatting. The UI never writes files itself.
- **Each PR description ends with the human check** below, as a checklist for Antoine.

### Overview

| # | Chunk | Depends on | Week | Can run in parallel with |
| --- | --- | --- | --- | --- |
| C0 | Skeleton and test harness | — | 1 | — |
| C1 | Legacy notes to real lights | C0 | 1 | C2 |
| C2 | Rig, fixtures, colour | C0 | 1 | C1 |
| C3 | Named cues and scenes | C2 | 2 | C4, C6, C11 |
| C4 | Musical time and swells | C2 | 2 | C3, C6, C11 |
| C5 | Pattern sequencer engine | C4 | 2 | C6, C7 |
| C6 | UI shell: monitor, patch, stage plan | C2 | 2 | C3, C4, C5 |
| C7 | Builder: static looks, cue assignment, audition | C3, C6 | 3 | C5, C9 |
| C8 | Builder: swell editor and sequencer grid | C4, C5, C7 | 3 | C9, C12 |
| C9 | Pixel engine: canvas, shapes, effects | C2, C4 | 4 | C8, C11, C12 |
| C10 | Builder: pixel scene editor | C7, C9 | 5 | C12 |
| C11 | Art-Net mirror and BlenderDMX setup | C1 | 2–4 | anything |
| C12 | Reliability: watchdog, show mode, resume | C3 | 4–6 | C9, C10 |
| C13 | Migration tooling and soak runner | C1, C3 | 6 | C10, C12 |

### C0 — Skeleton and test harness

- **Goal:** an empty engine that renders 512 zeros at 40 Hz and can be tested by replay.
- **Build:** Cargo workspace; render loop with injectable clock; `MidiSource` and `DmxSink` traits; MIDI-file source; `FakeDmx` recorder; frame-dump format and comparison tool; CI.
- **Acceptance:** a replay test with an empty show produces the expected all-zero dump; frame timing with the fake clock is exact; CI green.
- **Human check:** none.

### C1 — Legacy notes to real lights

- **Goal:** today's sessions run unchanged with EMU closed.
- **Build:** IAC (CoreMIDI) input via `midir`; `legacy-map.yaml` (channel-1 note ranges → DMX addresses, velocity × 2, note-off = 0); Enttec DMX USB Pro driver; `strata run <show>`; `strata devices` to list ports.
- **Acceptance:** replay of a recorded legacy MIDI file matches the reference dump; unit tests for the Enttec packet framing; driver reconnects after a simulated port loss.
- **Human check:** none. Agents test the real driver against the emulated Enttec device and compare legacy songs with the EMU parity captures (section 15). Hold-last-look is measured in the bench session.

### C2 — Rig, fixtures, colour

- **Goal:** address lights by name and colour, not by channel.
- **Build:** `rig.yaml` (fixtures, modes, addresses, groups, canvas positions); Open Fixture Library JSON import (`strata fixture import`); fixture model with pixels; `palettes.yaml`; RGB → RGBW conversion (saturated and white-rich modes); `strata fire --set all-wash color=deep-blue dimmer=60%`; WebSocket stream of engine state and output values.
- **Acceptance:** the WildWash profile maps each attribute to the right channel (table-driven test); colour conversion tests; setting a look by name yields the expected dump; overlapping addresses are rejected at load.
- **Human check:** none. Agents check the profile against the manufacturer's channel table; one wash is confirmed in the bench session.

### C3 — Named cues and scenes

- **Goal:** a note on channel 2 fires a named scene with fades.
- **Build:** song files and JSON Schema; `strata validate`; Program Change song select (songs preloaded); note on/off → cue start/release; scene cue type with fade-in and release; layer priority (legacy < cues < pixel < master); CC 7 master dimmer; CC 20–27 live parameters; panic note and safe look; hot reload in edit mode.
- **Acceptance:** replay tests for fade timing (within one frame), priority between legacy and cue layers, song switch mid-stream, panic; invalid files produce clear errors with line numbers.
- **Human check:** none. Covered by replay tests and an automated hot-reload test.

### C4 — Musical time and swells

- **Goal:** a swell lasts exactly N beats or bars at Reaper's tempo and starts on its note.
- **Build:** MIDI clock follower (tempo estimate, start/stop/continue, bar position from the song's time signature); fallback to the song's BPM; duration parser (`ms`, `s`, `beats`, `bars`); envelope shapes (ramp up, ramp down, sine, hold, custom points); swell cue type targeting dimmer, colour or a scene's intensity.
- **Acceptance:** with a synthetic clock at 92 and 140 BPM, a 4-bar swell ends within one frame of the bar line, 100 runs; tempo change mid-swell is followed; no clock → fallback BPM used and logged.
- **Human check:** none. Timing is asserted on the DMX output; optionally watch the rendered preview video against the audio.

### C5 — Pattern sequencer engine

- **Goal:** a looping pattern of envelope hits on chosen beats.
- **Build:** pattern cue type (`length`, `target`, `hits` with `at`, `shape`, `length`, `level`); resolution to a quarter beat; loop until release; bar-aligned start (starts at the next bar line, or immediately if the note lands within a small window after it); patterns can target a group or a pixel scene's intensity.
- **Acceptance:** the beat 1 / 2 / 3.5 example produces the expected dump at two tempos; loop boundaries are seamless; release mid-hit follows the release time; overlapping hits take the highest value.
- **Human check:** none. Same as C4.

### C6 — UI shell: monitor, patch, stage plan

- **Goal:** a browser page that shows what the engine is doing and where the lights are.
- **Build:** Svelte app served by the engine; WebSocket client; Monitor (health, MIDI log, song, active cues, output values); Patch (fixtures, addresses, test button); Stage plan (2D live preview from pixel positions, drag to move, saved to `rig.yaml` in edit mode); edit/show mode indicator.
- **Acceptance:** Playwright tests against the engine with `FakeDmx`: preview colours match output; dragging a fixture changes `rig.yaml`; the page reconnects after an engine restart.
- **Human check:** position the four washes on the plan (laptop only, no lights).

### C7 — Builder: static looks, cue assignment, audition

- **Goal:** create and save a scene without opening a file.
- **Build:** show-file write endpoint (validate, atomic write, canonical format); looks editor (colour picker, palette swatches, dimmer per fixture or group, live on output); cue list per song (name, note number, type, fade-in, release); audition button; built-in metronome tempo when no MIDI clock; undo of the last save.
- **Acceptance:** Playwright: build a look, save as cue 36, reload the engine, fire note 36, dump matches; duplicate note numbers are refused; in show mode all editing is disabled.
- **Human check:** try the builder at the laptop against the preview; note anything that feels slow.

### C8 — Builder: swell editor and sequencer grid

- **Goal:** draw swells and beat patterns visually.
- **Build:** envelope editor (shape presets, custom points, length in beats or bars, target picker); sequencer grid (1–4 bars, quarter-beat steps, click to place a hit, per-hit shape, length and level, copy bar); live looped audition at the metronome or Reaper tempo; saves swell and pattern cues.
- **Acceptance:** Playwright: build the 1 / 2 / 3.5 pattern in the grid; the saved YAML equals the reference from C5; editing an existing pattern round-trips without changes to untouched hits.
- **Human check:** rebuild one swell-heavy section at the laptop against the preview.

### C9 — Pixel engine: canvas, shapes, effects

- **Goal:** generative pixel scenes rendered deterministically.
- **Build:** normalised 2D canvas; generic RGB and RGBW pixel grids and columns of any size, spanning universes; shape presets (bar, sweep, circle, split, gradient, one-by-one); effects (static, move, pulse, chase, sparkle) with speed in beats, width, direction, phase; 2 layers with max / add / mask; fixed seed per cue; pixel cue type; intensity input so swells and patterns can drive it.
- **Acceptance:** dump tests per shape × effect on the 4-wash rig, an 8×8 RGB grid, a 1×32 RGBW column and a 16×16 RGB grid across two universes, including serpentine wiring and every colour order; same seed → identical frames; 512 channels render in under 1 ms per frame (benchmark in CI).
- **Human check:** none. Pick defaults from the generated preview sheet of every shape and effect.

### C10 — Builder: pixel scene editor

- **Goal:** build a pixel scene by picking a shape and an effect.
- **Build:** shape and effect pickers with thumbnails; parameter sliders; palette colours; layer 2 toggle and blend mode; live on stage plan and output; link a swell or pattern to the scene's intensity; save as pixel cue.
- **Acceptance:** Playwright: create a two-layer scene, save, reload, fire, dump matches the C9 reference for the same parameters.
- **Human check:** rebuild one EMU pixel section at the laptop against the preview.

### C11 — Art-Net mirror and BlenderDMX setup

- **Goal:** 3D preview with no extra code in the UI.
- **Build:** Art-Net output for any number of universes, used both as a preview mirror and as the real output for pixel grids that don't fit the Enttec universe; `docs/blenderdmx.md` with the setup steps; a starter Blender scene for the 4-wash rig if a suitable GDTF file exists.
- **Acceptance:** packet-format unit tests; an integration test receives the mirrored frames on localhost.
- **Human check:** none. BlenderDMX is optional.

### C12 — Reliability: watchdog, show mode, resume

- **Goal:** the engine can die on stage and nobody notices for long.
- **Build:** launchd service with auto-restart; start-up under 1 s; persisted last song and master level; show mode (read-only files, no hot reload, no UI edits, Art-Net off unless enabled); structured log of every MIDI event and cue; fault-injection scripts (kill engine, drop MIDI port, drop serial port); the keeper process from section 9 only if C1's human check showed the Enttec box doesn't hold.
- **Acceptance:** automated kill-and-restart test: output resumes at the next cue within the target time; show mode refuses writes; logs contain every event of a replay.
- **Human check:** none. Fault injection runs against the emulated devices; a real USB unplug is tried once in the bench session.

### C13 — Migration tooling and soak runner

- **Goal:** move the set over and prove it runs for hours.
- **Build:** `strata record` (capture the MIDI of a Reaper run to a file for replay tests); `strata soak <setlist> --hours 4` (loops recorded songs, reports dropped frames, timing drift, memory); a per-song migration checklist in `docs/`; replay references for every song in `shows/fossile/`.
- **Acceptance:** soak run of 4 h on CI hardware with zero dropped frames and flat memory; every song in the show has a passing replay test.
- **Human check:** record each song's MIDI from Reaper (laptop only); start the unattended soak on the M1; then the go/no-go rehearsal, the only session with the full rig.

### What only you can do

1. One bench session and the go/no-go rehearsal (section 15), plus short laptop-only reviews of previews.
2. Rebuilding pixel sections and swells by eye in weeks 5–7.
3. The list of EMU pixel sections (section 13, question 4), before C10.

## 15. Validation without the rig

Agents validate on the DMX output; the rig comes out twice. Every check below runs unattended and reports pass/fail with evidence (frame dumps, timing histograms, preview images).

### What agents check

| Layer | How | Catches |
| --- | --- | --- |
| Engine logic | Replay tests: MIDI file + show in, DMX frames out, compared to references | Wrong values, wrong timing, regressions |
| Real Enttec driver | An emulated DMX USB Pro on a pseudo serial port that parses the Enttec protocol and records what it receives | Framing errors, reconnect bugs, frame rate drops, without the box |
| Real MIDI path | A virtual CoreMIDI port on a macOS runner; a test player sends recorded songs in real time | Dropped or reordered events, latency and jitter (measured, p99 reported) |
| Art-Net | Loopback receiver decoding the packets per universe | Packet format, universe mapping for grids |
| Parity with EMU | Record EMU's Art-Net output while Reaper plays each existing song (laptop only), then compare with Strata's output for the same MIDI | Legacy songs behaving differently from today |
| Fixture profiles | Channel table extracted from the manufacturer's manual, compared with the profile | Mis-tagged attributes like those seen in EMU |
| Pixel grids | Synthetic grids of several sizes, wirings and colour orders; each pixel's address and colour asserted | Mapping errors, off-by-one, universe overflow |
| Invariants (property tests) | Random shows and MIDI: values always 0–255, swells monotonic where they should be, no jump above a threshold between frames during fades, released cues reach zero, same input gives same output | Flicker, stuck channels, non-determinism |
| Musical timing | Synthetic clocks at many tempos, tempo changes, clock dropouts; cue ends asserted against bar lines | Drifting swells |
| Looks | Headless render of the stage plan to an image per cue and a short video per song; a contact sheet for all shapes and effects | Obviously wrong looks, reviewed by an agent and skimmable by you |
| UI | Playwright against the engine with emulated devices | Builder saving wrong data, broken round-trips |

### Stress tests

- **MIDI flood:** 10,000 events per second, all notes on all channels, random controller sweeps, malformed and truncated messages. Pass: no crash, frame rate holds, output valid.
- **Fuzzed show files:** random and corrupted YAML. Pass: clear error, engine keeps running the previous show.
- **Fault injection:** kill the engine, drop and restore the serial port, drop the MIDI port, fill the disk, change tempo wildly, switch songs every 100 ms. Pass: recovery within the target time, no stuck output.
- **Accelerated soak:** 100 hours of show simulated on the fake clock in minutes, checking for drift and memory growth.
- **Real-time soak:** 4 hours on the M1 with Reaper playing the full session and emulated devices, unattended. Pass: zero dropped frames, flat memory, timing jitter within limits.
- **Load:** 8 universes of pixel grids at 40 Hz with two effect layers. Pass: render time under 5 ms per frame.

### What still needs you

1. **Bench session (about 30 minutes, once, around week 2):** the Enttec box and one wash on a table. Confirms what emulation can't: the box holds its last frame when the engine dies, one wash matches its profile, a real USB unplug recovers.
2. **Go/no-go rehearsal (week 7):** the full rig, the full set, twice.
3. **Laptop only, no lights:** record EMU's output and each song's MIDI once; lay out the stage plan; build looks in the builder against the preview.

### What this can't catch

How a look reads on real LEDs (colour rendering, dimming curve at low levels, brightness in a room) and electrical problems. The preview is an approximation, so expect some colour and level touch-ups at the rehearsal.
