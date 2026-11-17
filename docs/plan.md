# Fossile lighting engine — plan & spec (v1)

Nov 12, 2026 · @Antoine

Name: **Blimey** (formerly the placeholder Strata). A small, headless lighting engine driven by Reaper, replacing ENTTEC EMU for Fossile's show. Companion to the market survey doc.

**Scope and pace:** version 1 includes the full scene builder (pixel scenes, swells in musical time, pattern sequencer). With an agent writing the code, 8 weeks is feasible. What the agent can't speed up is time with the real lights: checking looks by eye, hardware tests on the M1, reprogramming songs, rehearsals. The plan limits rig time to one bench session and one rehearsal (section 15), validates everything else on the DMX output, and keeps a go/no-go rehearsal with EMU as the fallback.

## 1. Problems this must fix

| Past problem | Requirement in Blimey |
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
Blimey engine (one Rust process)
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
      - { at: 1,   shape: ramp-up, length: 0.5 beats, level: 100% }
      - { at: 2,   shape: sine,    length: 0.5 beats, level: 80% }
      - { at: 3.5, shape: ramp-up, length: 0.5 beats, level: 100% }
```

- Durations accept `ms`, `s`, `beats`, `bars`.
- A JSON Schema ships with the engine; `blimey validate` checks a show before it is loaded.
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

Section 16 replaces this builder with areas placed on the front view; what follows is the record of what C7, C8 and C10 built.

### Scene builder

1. **Pixel scenes.** Pick a preset shape (bar, sweep, circle, split, gradient, fixtures one by one), an effect that animates it (static, move, pulse, chase, sparkle), colours from the palettes and a speed in beats. Up to 2 layers.
2. **Swells.** An envelope with a shape (ramp up, ramp down, sine, hold, or custom points) and a length in beats or bars. Target: a group's dimmer or colour, or the intensity of a pixel scene.
3. **Pattern sequencer.** A grid of 1 to 4 bars, resolution down to a quarter beat. Place hits on steps; each hit has its own shape, length and level. Example: ramp-up on beat 1, sine on beat 2, ramp-up on beat 3.5. The pattern loops until the cue is released and restarts on the bar when triggered.
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
- CLI: `blimey validate`, `blimey render song.yaml --midi take.mid` (offline render to a frame dump or preview), `blimey fire <cue>`.
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

**Go/no-go rule:** if the week-7 rehearsal isn't clean, the gig runs on EMU and Blimey's first gig moves to the next date. Decide this in advance, not on the day.

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

- **Repo layout:** `engine/` (Rust library: all logic), `blimey/` (Rust binary: CLI, HTTP, device drivers), `ui/` (Svelte + Vite, built to static files and embedded in the binary), `schema/` (JSON Schema for show files), `shows/fossile/` (the real show), `tests/fixtures/` (MIDI files and reference frame dumps), `docs/` (API contract, decisions).
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
- **Build:** IAC (CoreMIDI) input via `midir`; `legacy-map.yaml` (channel-1 note ranges → DMX addresses, velocity × 2, note-off = 0); Enttec DMX USB Pro driver; `blimey run <show>`; `blimey devices` to list ports.
- **Acceptance:** replay of a recorded legacy MIDI file matches the reference dump; unit tests for the Enttec packet framing; driver reconnects after a simulated port loss.
- **Human check:** none. Agents test the real driver against the emulated Enttec device and compare legacy songs with the EMU parity captures (section 15). Hold-last-look is measured in the bench session.

### C2 — Rig, fixtures, colour

- **Goal:** address lights by name and colour, not by channel.
- **Build:** `rig.yaml` (fixtures, modes, addresses, groups, canvas positions); Open Fixture Library JSON import (`blimey fixture import`); fixture model with pixels; `palettes.yaml`; RGB → RGBW conversion (saturated and white-rich modes); `blimey fire --set all-wash color=deep-blue dimmer=60%`; WebSocket stream of engine state and output values.
- **Acceptance:** the WildWash profile maps each attribute to the right channel (table-driven test); colour conversion tests; setting a look by name yields the expected dump; overlapping addresses are rejected at load.
- **Human check:** none. Agents check the profile against the manufacturer's channel table; one wash is confirmed in the bench session.

### C3 — Named cues and scenes

- **Goal:** a note on channel 2 fires a named scene with fades.
- **Build:** song files and JSON Schema; `blimey validate`; Program Change song select (songs preloaded); note on/off → cue start/release; scene cue type with fade-in and release; layer priority (legacy < cues < pixel < master); CC 7 master dimmer; CC 20–27 live parameters; panic note and safe look; hot reload in edit mode.
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
- **Build:** `blimey record` (capture the MIDI of a Reaper run to a file for replay tests); `blimey soak <setlist> --hours 4` (loops recorded songs, reports dropped frames, timing drift, memory); a per-song migration checklist in `docs/`; replay references for every song in `shows/fossile/`.
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
| Parity with EMU | Record EMU's Art-Net output while Reaper plays each existing song (laptop only), then compare with Blimey's output for the same MIDI | Legacy songs behaving differently from today |
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

## 16. Builder v2: areas on the stage

Nov 14, 2026 · draft. Replaces the scene builder of section 8 and lifts the two-layer limit of section 7. Sections 1–15 stay as the record of v1; the ground rules of section 14 and the validation of section 15 apply to the chunks below.

### Why

The v1 builder works, but it is four forms. You choose a cue type from a list before you see anything, the front view only shows the result, and a rhythm is tied to a pixel scene through a second cue and a dropdown. The rig will also gain height, so a look has to be composed on the front view, with pixel mapping as the one model for every light.

### The model

- **A look is a stack of areas.** An area is a rectangle of the front view with a recipe: colour, shape, motion, rhythm. Example: a swell over the left half, a chase over the right half, a sparkle across the top row, laid over both.
- **One model, no cue type to choose.** A still look is an area that is filled and stands still; a pixel effect is an area with a shape that moves; a swell or a beat pattern is an area's rhythm. The builder saves pixel cues only.
- **Mute instead of odd-shaped areas.** An area can skip fixtures or groups. "The outer washes" is a full-width area with the inner washes muted.
- **One cue is the whole picture.** Everything a look needs sits in one cue, on one note. Two pixel cues held together do not mix: the later one replaces the earlier one, as today.
- **The song is a bank of pads.** One pad per note, each showing what its cue looks like. A pad opens its cue on the stage.

### In the show file

A pixel cue gains `layers`, bottom first:

```yaml
  43:
    name: chorus-build
    type: pixel
    release: 1s
    layers:
      - area: { x: 0, y: 0, width: 0.5, height: 1 }
        shape: fill
        params: { color: warm-white }
        swell: { duration: 4 bars, curve: ease-in }
      - area: { x: 0.5, y: 0, width: 0.5, height: 1 }
        shape: bar
        effect: chase
        params: { color: amber, width: 0.25, speed: 1 bar }
        blend: over
      - area: { x: 0, y: 0.65, width: 1, height: 0.35 }
        shape: fill
        effect: sparkle
        params: { color: steel-blue, width: 0.4, speed: 2 beats }
        blend: add
        mute: [inner-wash]
```

- `area` is in canvas units, as in `rig.yaml`: `x`, `y` is the bottom-left corner, `y: 0` the floor. It defaults to the whole canvas.
- The shape runs inside its area: `width` and `direction` are relative to the area, not to the canvas.
- `shape: fill` lights the whole area.
- `level` scales the layer, like a scene's `dimmer`: `level: 60%`.
- `mute` lists fixtures or groups the layer leaves alone.
- `swell` or `pattern`, one at most, is the layer's own rhythm, with the fields and the timing of the cue types of the same name. A swell runs from note-on and then holds; a pattern starts on the bar line and loops, dark between hits.
- `blend` says how the layer meets what is below it. `over` is new: the layer covers what is below by how lit it is, so a lit pixel replaces and a dark one lets the layer below show (with a `background`, the layer is opaque over its whole area). `max`, `add` and `mask` work as today, a mask now acting inside its own area only. The default stays `max`; the builder writes `over` on new areas, so an amber area over a magenta one shows amber, not a mix.
- Up to 8 layers. Sparkle gets a different seed per layer.

### What stays

- **Old pixel cues.** `shape`, `effect`, `params` and `layer_2` at the top of the cue keep loading, as one or two layers over the whole canvas, and render the same frames: no reference is regenerated. A cue has one form or the other.
- **Scene, swell and pattern cues** keep playing and stay in the schema. A swell or pattern cue can still target a pixel cue's intensity; it scales the whole cue, every layer at once.
- **Playback order.** Legacy, then scenes, swells and patterns, then pixel cues, then the master dimmer.

### Not in this version

Areas that aren't rectangles (mute covers the need), held cues that mix, rotated fixtures, custom-drawn shapes, tapping a rhythm in, generated variations of a look, control from Reaper by scene and song number (open question 5).

### Chunks

| # | Chunk | Depends on | Can run in parallel with |
| --- | --- | --- | --- |
| C14 | Pixel layers: areas, mute, blend | C9 | C16 |
| C15 | Per-layer swell and pattern | C14 | C16 |
| C16 | Builder: pad bank | C10 | C14, C15 |
| C17 | Builder: stage composer | C14, C16 | C15 |
| C18 | Builder: rhythm per area, and the old cue types | C15, C17 | — |
| C19 | Builder: scenes home, scenes by number | C18 | — |

### C14 — Pixel layers: areas, mute, blend

- **Goal:** one pixel cue composes up to eight layers, each confined to an area of the canvas.
- **Build:** `layers` on the pixel cue, the old form parsed into the same structure; `area`, with shape, `width` and `direction` evaluated in the area's own 0–1 space; `mute` by fixture or group; `level`; the `fill` shape; the `over` blend, and `mask` limited to the masking layer's area; a sparkle seed per layer; the pixel preview and thumbnail endpoints take the new form; load errors for an area that is empty or outside the canvas, an unknown name in `mute`, and a cue mixing both forms; a three-row wash rig in `tests/fixtures/`.
- **Acceptance:** every existing pixel reference dump is unchanged; a dump test of the cue above, without its swell, on the three-row rig and on an 8×8 grid; a property test that a shape in an area gives the same frames as that shape on the full canvas with the rig squeezed into the area; table tests for the four blends and for mute; 8 layers over 8 universes render in under 5 ms per frame (benchmark in CI).
- **Human check:** none. Skim the preview sheet of the example cue.

### C15 — Per-layer swell and pattern

- **Goal:** each area has its own rhythm inside one cue.
- **Build:** `swell` or `pattern` on a layer, with the timing rules of the cue types; multiplied with the layer's `level`, the cue's velocity and fades, and any swell or pattern cue that targets the cue's intensity.
- **Acceptance:** a one-layer cue with a `pattern` gives the same dump as that cue without it plus a pattern cue targeting its intensity, fired on the same tick, and the same for a swell; at 92 and 140 BPM a layer's 4-bar swell ends within one frame of the bar line; two layers with different rhythms don't affect each other; a release mid-hit follows the cue's release time.
- **Human check:** none. Timing is asserted on the DMX output.

### C16 — Builder: pad bank

- **Goal:** the builder opens on the song as pads, each showing what its cue looks like.
- **Build:** a thumbnail endpoint for any cue, rendered at the rig's pixel positions (a swell or pattern cue returns its envelope); pads by note in banks of 16 aligned on note 36, with a bank switcher that marks the banks holding cues; a pad shows a live thumbnail, the name and the note; a click opens the cue in its editor (the v1 editors until C17); press and hold auditions it through the engine; new cue on an empty pad, duplicate to the next free pad, move by dragging to an empty pad, delete; the song bar (song, new song, tempo, undo and redo) carried over. Replaces the cue table.
- **Acceptance:** Playwright against the engine with `FakeDmx`: the pads match the song file after a load, a save and an undo; duplicate and move write the expected YAML and refuse an occupied note; hold sends note-on and letting go sends note-off; show mode is read-only.
- **Human check:** browse `tell-me` from the pads at the laptop; note anything slow or unclear.

### C17 — Builder: stage composer

- **Goal:** build a look by placing areas on the front view, without choosing a cue type.
- **Build:** the front view edits a `layers` pixel cue. A tray of ingredients (Wash, Chase, Sweep, Sparkle, Breathe: recipe defaults kept in the UI) to drag onto the stage, or click for a full-stage area. Move and resize with snapping to fixture edges; a click selects, a second click goes one area deeper; the selected area outlines the pixels it catches and says so when it catches none. A layer list: order, hide while editing (every layer is still saved), delete. An inspector: colour and background from the palettes, shape and motion as live thumbnails, direction, width, speed, level, blend, and the fixtures the area catches as mute toggles, crossed out on the stage when muted. Live on the front view and the output through the pixel preview. Old-form pixel cues open as layers and save in the new form. One sentence reads the look back ("warm-white wash on the left, amber chase on the right").
- **Acceptance:** Playwright: build the cue above, without its swell, by drag, resize and inspector; the saved YAML equals the C14 reference and the fired note's dump matches it; an old-form cue opened and saved untouched renders the same dump.
- **Human check:** rebuild the example and one real look from the set at the laptop; note what feels fiddly.

### C18 — Builder: rhythm per area, and the old cue types

- **Goal:** a rhythm is part of an area's recipe, and the v1 forms go.
- **Build:** grooves in the inspector as thumbnails (on the beat, off-beat, heartbeat, backbeat, swell); the swell editor and the sequencer grid reused inside the inspector for a custom rhythm, writing the layer's `swell` or `pattern`, with the looped audition and playhead of C8; the Swell and Hits ingredients. Scene cues open in the composer as `fill` areas with `mute` lists and save as pixel cues. Swell and pattern cues keep their editors, reached from their pad. The look list, the cue form with its type list and the two-layer pixel editor are removed.
- **Acceptance:** Playwright: the 1 / 2 / 3.5 pattern built on one area saves the reference YAML and its dump matches C15's; each scene of `tell-me`, converted and fired alone, matches the original scene's dump within one DMX step per channel; editing one area's rhythm leaves the other layers' YAML untouched.
- **Human check:** rebuild one song section end to end from the pads at the laptop.

### C19 — Builder: scenes home, scenes by number

- **Goal:** the builder opens on the song's scenes, edits one scene on a screen of its own, and speaks of scenes and numbers rather than cues and MIDI notes.
- **Build:** two screens in the Builder tab. The scenes home: the song bar and a full-width grid of pads, one per scene in number order with its live thumbnail, name and number, then a New scene pad; holding a pad auditions it, a click opens it, and each pad has an Audition/Release button for the keyboard. The scene editor, at `#builder/<song>/<number>` so the browser's Back returns home: the stage composer across the screen width, or the swell or pattern editor, with the scene's name, number, fade in, release, Save, Audition, Duplicate and Delete; leaving it with unsaved edits asks first. "Cue" becomes "scene" in the builder, and a v1 scene it keeps as it is a "fixed look". A song shows as "Song N", N its `program`, and a new one takes the next free number; a new scene takes the lowest free number from 1, Duplicate the next free one above, and the Number field renumbers on save. The bank switcher, drag-to-move and every mention of notes, programs, MIDI and velocity leave the builder. The show-file format, the API and the engine don't change: scene N still fires on note N and song N on program N (docs/api.md).
- **Acceptance:** Playwright against the engine with `FakeDmx`: the home lists the song's scenes in number order and nothing note-related; a click opens the editor, and its link and the browser's Back return home; leaving with unsaved edits asks first; a new scene takes the lowest free number and saves under it; renumbering onto a taken number is refused and the file is untouched; a new song takes the next free number; holding a pad and the keyboard audition send note-on and note-off; show mode is read-only.
- **Human check:** browse `tell-me` from the scenes home at the laptop, open a scene, change it and go back; note anything unclear.

### Open questions

1. A converted scene becomes a pixel cue, which sits above scenes in the playback order. Songs built in the composer don't notice; a song mixing old scenes and converted ones might.
2. A bar hung vertically needs a rotation on its `canvas` entry, which a fixture doesn't have today. Wait for the fixture.
3. A light with no colour pixels (a plain dimmer) is outside pixel cues and still needs a scene cue. Not the case of any fixture in the rig.
4. Should the pixels a cue doesn't cover be see-through, so that held cues mix? Left out; revisit if the Reaper sessions want to stack looks.
5. How Reaper selects a song and fires a scene, now that the builder knows them only by number, is designed in a later chunk. Until then scene N is channel-2 note N and song N is channel-2 Program Change N. Section 17 answers it: Reaper plays a MIDI file that Blimey exports.

## 17. Song timeline

Nov 15, 2026 · draft. Builds on section 16; the ground rules of section 14 and the validation of section 15 apply. Lifts the "standalone timeline" non-goal of section 3 for building the show only: Reaper stays the clock on stage.

### Why

Scenes are built in Blimey, then placed by hand in Reaper as channel-2 notes. The piano roll shows note numbers, not looks, and the running order is arranged without seeing the lights. The song's running order should be built in Blimey, against the lights and the audio, and handed to Reaper ready to play.

### The model

- **Reaper plays, Blimey writes.** Blimey exports the running order as a MIDI file on the channel-2 protocol of section 5, and Reaper plays it as a normal item. The engine still renders only from the MIDI it receives; after a seek, the lights catch up at the next scene, as today.
- **Sections are defined once.** A song has sections (intro, verse, chorus) and an arrangement, the order they play in. Editing a section changes every place it plays. A chorus that differs the last time round is a section of its own: duplicate, then edit.
- **A section is bars and blocks.** A number of bars, its own time signature if it differs from the song's, and blocks: a scene held from one point to another, at a level.
- **Playback with audio in Blimey.** The browser plays a bounce of the song and drives the engine through the audition path, so the front view and the lights follow the music. For building the show, in edit mode only.
- **One tempo per song.**

### In the show file

```yaml
time_signature: 4/4
audio: { file: audio/tell-me.mp3, offset: 1.25s }
sections:
  verse:
    bars: 16
    blocks:
      - { scene: 36, bar: 1, length: 16 bars }
      - { scene: 42, bar: 9, length: 8 bars, level: 80% }
  chorus:
    bars: 8
    blocks:
      - { scene: 37, bar: 1, length: 8 bars }
      - { scene: 39, bar: 5, beat: 3, length: 6 beats }
  bridge:
    bars: 4
    time_signature: 3/4
    blocks:
      - { scene: 40, bar: 1, length: 4 bars }
arrangement: [verse, chorus, verse, chorus, bridge, chorus]
```

- `sections` is keyed by name; `arrangement` lists names in playing order, each as often as needed.
- `bars` is the section's length; `time_signature` overrides the song's for that section.
- A block holds `scene` (its number, the key in `cues`) from `bar` and `beat` (default 1, fractions allowed) for `length`, in `beats` or `bars` of the section's signature. `level` scales the scene like the note's velocity; default 100%.
- A block ends at or before its section's end. When one placement ends with a block that the next placement starts with (same scene, same level, end and start on the boundary), the export joins them into one held note, so a scene held across sections doesn't restart.
- Different scenes may overlap; they combine as they do live. Two blocks of the same scene may not overlap.
- `audio.file` is relative to the show folder, under `audio/`, kept out of git. `offset` is where bar 1 falls in the file. A missing file is not an error: the song plays silent.
- Load errors: an unknown section in `arrangement`, an unknown scene, a block outside its section, overlapping blocks of one scene, a `length` in `ms` or `s`.

### Export

- `<show>/export/<song>.mid`, written when a song with an arrangement is saved or loaded in edit mode, and by `blimey export <show> [song]`; `export/` is kept out of git. The timeline also downloads it.
- Contents: the song's tempo, the time signature with a change at each section that has its own, a marker per placed section, Program Change `program` on channel 2 at bar 1, then a note on per block (note = scene number, velocity = level × 127) and a note off at its end. A note off and a note on on the same tick go off first.
- In Reaper: insert the file at the song's first bar; re-insert after a change.

### Engine

- The bar grid follows the arrangement: bar lines and bar positions use each section's time signature, so a pattern fired at the start of a 3/4 section starts on that section's bar line. Durations inside a scene (`length: 1 bar` on a pattern) keep the song's signature. A song without an arrangement behaves as today.

### Playback in Blimey

- The engine lists the arrangement's events in beats and seconds, the same list the export writes, so the browser doesn't redo the musical-time arithmetic. It serves the song's audio with range requests.
- The browser plays the audio and is the clock: it schedules each event against the audio position and sends it through `POST /api/midi`. Play sends the Program Change first. Seeking sends a Song Position Pointer, releases the blocks it started and re-fires the blocks active at the new position, so a swell restarts from the seek point. Pause releases them.
- Without audio, playback runs on the browser's clock at the song's tempo, with a click.
- Timing is good enough to build with (a few ms of jitter); the gig runs from Reaper.

### Not in this version

Tempo changes within a song, per-placement overrides of a section, legacy channel-1 notes and CC automation on the timeline (they stay in Reaper and play alongside the exported item), recording a Reaper run into the timeline, exact state after a seek on stage.

### Chunks

| # | Chunk | Depends on | Can run in parallel with |
| --- | --- | --- | --- |
| C20 | Arrangement, bar grid and MIDI export | C19 | — |
| C21 | Builder: song timeline | C20 | — |
| C22 | Builder: playback with audio | C21 | — |
| C23 | Builder: audio upload and markers | C22 | — |
| C24 | Builder: stage view on the timeline | C22 | — |

### C20 — Arrangement, bar grid and MIDI export

- **Goal:** a song's running order lives in its file, and Reaper plays it from an exported MIDI file.
- **Build:** `sections` and `arrangement` in the song model, schema and `docs/api.md`, with the load errors above; the bar grid per section; the event list (one engine function used by the export and by the API); the `.mid` writer on `MidiFile::encode`; `export/` written on save and edit-mode load, and kept out of git; `blimey export`; `GET /api/songs/{song}/arrangement/events` and `GET /api/songs/{song}/arrangement.mid`; `PUT /api/songs/{song}/arrangement` through the show-file writer (validated, atomic, undoable). An arranged song in `tests/fixtures/`, with a 3/4 section and a scene held across two sections.
- **Acceptance:** the fixture's exported file, replayed with its show, matches a stored reference dump; the event list puts each block on the expected beat and second across the 3/4 section; the held scene is one note; a pattern fired at the start of the 3/4 section starts at once rather than at the next 4/4 line; every existing reference dump is unchanged; each load error names its line; the save endpoint round-trips, and an invalid arrangement is refused with the file untouched; the exported file parses back with its tempo, signatures and markers.
- **Human check:** insert one exported song into Reaper at the laptop and check the scenes fire where expected; try re-inserting it after an edit.

### C21 — Builder: song timeline

- **Goal:** build a song's running order by placing scenes on sections, without Reaper.
- **Build:** a Timeline screen in the Builder tab at `#builder/<song>/timeline`, opened from the song bar of the scenes home. A section library: new, rename, duplicate, delete (refused while placed), bars, time signature. An arrangement strip: sections as tiles in playing order; add, drag to reorder, remove; the song's length in bars and minutes. A section editor: a bar ruler with beat ticks, one lane per scene, scenes dragged on from a strip of live thumbnails, blocks moved and resized with beat snapping, level, delete, hold a block to audition its scene; the other placements of the open section are highlighted. An overview of the whole song on one ruler; a click opens a section. "Export for Reaper" downloads the file. Saves through `PUT /api/songs/{song}/arrangement`, with undo and redo from the song bar. Scenes are named and numbered as in C19; no notes or MIDI in the wording.
- **Acceptance:** Playwright against the engine with `FakeDmx`: the C20 fixture arrangement built from empty by drag, resize and level saves the fixture's YAML; editing a block in a reused section changes every placement on the overview; reordering the strip writes the new order; deleting a placed section is refused; an edit the engine refuses shows its error and leaves the file untouched; holding a block sends note-on and letting go note-off; the export downloads a file equal to C20's; show mode is read-only.
- **Human check:** lay out `tell-me` from its real structure at the laptop; note what feels fiddly.

### C22 — Builder: playback with audio

- **Goal:** play the song in Blimey with its audio and watch the lights follow.
- **Build:** `audio` in the song model, schema and `docs/api.md`; `audio/` kept out of git; `GET /api/songs/{song}/audio` with range requests. On the timeline: play, pause, back to start, space bar; a playhead on the overview and the section editor; click the ruler to seek; a waveform under the overview ruler, decoded in the browser; drag the waveform against the bar grid to set `offset`, saved to the file. A look-ahead scheduler on the audio clock sending Program Change, notes and Song Position Pointer as above; a click track when there is no audio; edits made while playing take effect from the next event.
- **Acceptance:** scheduler tests on a fake audio clock: every event sent within one frame of its time, a seek mid-block re-fires it, releases the others and sends the position, pause releases all. Playwright with a short generated audio file and `FakeDmx`: playing from bar 1 sends the C20 events in order, each within 50 ms; a missing audio file plays silently with the click; show mode disables playback. An HTTP test for range requests.
- **Human check:** bounce `tell-me` from Reaper, set its offset, play it in Blimey with the front view and judge whether the lights land with the music.

### C23 — Builder: audio upload and markers

- **Goal:** bring a song's track into Blimey from the browser, with the markers set in Reaper, and edit those markers on the timeline.
- **Build:** `PUT /api/songs/{song}/audio` takes the file as the body (WAV, MP3 or FLAC, up to 1 GB), writes it atomically to `audio/<song>.<ext>`, removes that song's previous audio file, sets `audio.file` and keeps `offset`; refused in show mode. The response lists the markers found in the file. Marker import from WAV: cue points with their `labl` names (unnamed ones become "Marker N"), and `ltxt` regions as a marker at their start, converted to seconds with the file's sample rate; a hand-written RIFF reader in the engine, no new crate, and a damaged file gives an error, never a panic. `GET /api/songs/{song}/audio/markers` re-reads them. `markers` in the song model, schema and `docs/api.md`: a list of `{ name, at }`, with `at` a time in the audio file, so markers stay on the music when the offset changes. `PUT /api/songs/{song}/markers` goes through the show-file writer (validated, atomic, undoable). On the timeline: an upload button and drop zone; markers drawn as flags on the overview and the section editor, placed with the offset and the tempo (markers before bar 1 aren't drawn); on upload, a song with no markers takes the file's markers, and one with markers asks before replacing them; "Import markers from audio" does the same later; add a marker at the playhead (M key) or by double-clicking the ruler, rename, drag to move, delete. Undo restores the song file, not a replaced audio file.
- **Acceptance:** Rust tests on WAVs generated in the test: named and unnamed cue points, a region, odd-sized chunks with their padding byte, 48 kHz, a truncated file and a non-WAV file. Upload: atomic write, a previous file with another extension removed, `audio.file` set with the offset kept, show mode, an unsupported type and an oversized body refused, a song name that can't reach outside the show. Markers: round trip, refusal of a negative `at` or an empty name with the file untouched, undo. Playwright with a generated WAV holding two markers: upload, the markers drawn on the expected bars, playback of the uploaded file; add, rename, move and delete each write the expected YAML; replacing existing markers asks first; show mode is read-only.
- **Human check:** render `tell-me` from Reaper with markers in the WAV, upload it, and check the markers land on the song's sections.

### C24 — Builder: stage view on the timeline

- **Goal:** watch the lights follow the music while building a song, without leaving the timeline.
- **Build:** the front view, driven by the engine's output stream as on the Front view tab, pinned on the Timeline screen so it stays in sight while playing and editing: it shows playback, held blocks and scene pads. It reuses the front view and its locked-rig preview from the scenes home, so fixtures don't move there; the user can enlarge, shrink or hide it. No engine, API or schema change.
- **Acceptance:** Playwright against the engine with `FakeDmx` and the steady audio clock: the stage view is on the timeline; playing from bar 1 lights it with the colours the engine sends for the first block's scene; holding a block or a scene pad shows that scene on it; it stays mounted across section switches and during playback; show mode still shows it.
- **Human check:** play a real song on the timeline with its audio, edit a section meanwhile, and judge whether the stage view is big enough to read the look without getting in the way.

### Open questions

1. The engine reads a Song Position Pointer as beats from the top of the song. If one Reaper project holds the whole set and sends positions from the project start, the bar grid is off on stage. Check how the session sends clock before relying on bar-aligned patterns in sections with their own signature.
2. Whether a Reaper item can reference the exported file on disk and pick up changes, instead of being re-inserted.
