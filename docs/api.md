# Blimey API contract

What the engine exposes to the web UI, the CLI and agents. UI chunks code against this file, not against engine internals. A chunk that changes an endpoint or a show-file field updates this file and `schema/` in the same PR.

## Show files

One folder per show. Each file has a JSON Schema in `schema/`. The engine checks the rest at load and fails with the file name and the problem. Every file is optional: a missing one loads as empty.

| File | Schema | Holds |
| --- | --- | --- |
| `legacy-map.yaml` | `schema/legacy-map.schema.json` | Channel-1 note ranges → DMX channels |
| `rig.yaml` | `schema/rig.schema.json` | Fixtures, DMX addresses, canvas positions, groups |
| `palettes.yaml` | `schema/palettes.schema.json` | Named colours |
| `fixtures/<profile>.yaml` | `schema/fixture.schema.json` | One fixture profile: channels per mode |
| `songs/<name>.yaml` | `schema/song.schema.json` | One song: the program that selects it, its safe look and its cues |

### rig.yaml

```yaml
fixtures:
  - name: wash-1                                # unique, no "="
    profile: stairville-wild-wash-pro-648-rgbw  # fixtures/<profile>.yaml
    mode: 16ch                                  # a mode name from the profile
    address: 1                                  # 1-based DMX start address
    canvas: { x: 0.1, y: 0.2, width: 0.125 }
groups:
  all-wash: [wash-1, wash-2, wash-3, wash-4]
```

The engine refuses to load a rig where:

- two fixtures' channel ranges overlap (`fixtures "wash-a" (DMX 1-16) and "wash-b" (DMX 16-31) overlap`),
- a fixture runs past channel 512,
- a fixture name appears twice, a group lists an unknown fixture, or a group shares a fixture's name,
- the profile or mode doesn't exist,
- a pixel grid doesn't fit its output (see below).

**Canvas.** Fixtures sit on a normalised 2D canvas, 0–1 on both axes, which is the stage as the audience sees it: a front view, not a plan from above. `x` runs from the left edge (0) to the right edge (1) as seen from the audience: x = 0 is stage right, x = 1 is stage left. `y` runs from the floor (0) up to the top of the stage picture (1). A fixture's pixels sit on one row at height `y`, spread evenly across `width` starting at `x`, in pixel order: pixel *i* of *n* is at `x + width × (i + 0.5) / n`. A single-pixel fixture can omit `width`.

**Pixel grids.** Any RGB or RGBW grid is a fixture of `type: pixel-grid`, with no profile. A column is a grid with one column.

```yaml
fixtures:
  - name: back-grid
    type: pixel-grid
    columns: 16
    rows: 8
    colour_order: RGB          # each pixel's emitters in DMX order: R, G, B and optional W, e.g. GRB, RGBW, WRGB
    wiring: serpentine         # optional: row-by-row (default) or serpentine
    start_corner: bottom-left  # optional: top-left (default), top-right, bottom-left, bottom-right
    output: artnet             # optional: enttec (default) or artnet
    universe: 2                # Art-Net port-address of the first pixel; only, and required, for artnet
    address: 1                 # 1-based DMX start address in that universe
    canvas: { x: 0.2, y: 0.5, width: 0.6, height: 0.4 }
```

- Positions are in the front view from the audience: x 0 is stage right and 1 stage left; y 0 is the floor and 1 the top. `canvas` is the rectangle the grid fills, its bottom-left corner at (`x`, `y`). Pixels sit at the centres of `columns` × `rows` equal cells.
- `start_corner` is where the first pixel is, as seen from the audience. The chain runs along rows: `row-by-row` starts every row on the start corner's side, `serpentine` runs each row back the way the previous one came. With `start_corner: bottom-left`, row 1 of the wiring is the bottom row.
- One universe holds 170 RGB or 128 RGBW pixels. A pixel that doesn't fit in the rest of its universe starts the next Art-Net universe at channel 1. An `enttec` grid must fit in the Enttec universe, shared with the other fixtures: anything bigger needs `output: artnet`.
- Only pixel cues drive grids. Looks, scenes, swells and patterns can't name them yet, and groups can't list them.
- The engine refuses a grid that overflows the Enttec universe or Art-Net universe 32767, an `artnet` grid without `universe`, an `enttec` grid with one, a bad colour order, and channels that overlap another fixture or grid in the same universe (`fixtures "a" (Art-Net universe 4, DMX 1-6) and "b" (Art-Net universe 4, DMX 4-9) overlap`).

### palettes.yaml

```yaml
deep-blue: { rgb: "#0028ff", lee: "Lee 120 Deep Blue" }
```

`rgb` is what the engine uses. `lee` names the gel the colour approximates, for humans.

### Fixture profiles

```yaml
name: Wild Wash Pro 648 RGBW
source: https://…/manual.pdf
modes:
  - name: 16ch
    channels:            # DMX order
      - name: Red 1
        attribute: red   # dimmer | red | green | blue | white
        pixel: 1         # colour attributes only; defaults to 1
      - name: Shutter
        default: 255     # channels without an attribute hold this value (default 0)
```

Profiles are generated from Open Fixture Library JSON with `blimey fixture import`; fix the OFL file and re-import rather than editing the YAML. The importer maps OFL `Intensity` to `dimmer` and `ColorIntensity` red/green/blue/white to the colour attributes; matrix `templateChannels` become one channel per pixel, numbered in `matrix.pixelKeys` order. Other channels keep their name and hold their OFL `defaultValue`.

### songs/&lt;name&gt;.yaml

The song's name is its file name (`songs/tell-me.yaml` is `tell-me`).

```yaml
program: 3          # Program Change value on channel 2, 0-127, unique across songs
bpm: 92             # optional: tempo for beats and bars when no MIDI clock arrives; default 120
time_signature: 4/4 # optional: beats per bar over the note value; default 4/4
safe_look:          # optional: what panic shows; without it panic goes dark
  all-wash: { color: warm-white, dimmer: 30% }
cues:
  36:               # channel-2 note number
    name: verse-blue
    type: scene
    fade_in: 2s     # optional, default 0
    release: 500ms  # optional, default 0
    values:         # fixture or group → look settings, in order: a later entry wins
      all-wash: { color: deep-blue, dimmer: 60% }
  38:
    name: chorus-swell
    type: swell
    target: { all-wash: { dimmer: 100% } }  # look settings, like a scene's values
    duration: 4 bars
    curve: ease-in  # optional, default ramp-up
    release: 500ms  # optional, default 0
  42:
    name: verse-pulse
    type: pattern
    length: 1 bar                 # 1, 2, 3 or 4 bars
    target: { all-wash: dimmer }  # or look settings: { all-wash: { color: amber } }
    release: 1 beat               # optional, default 0
    hits:                         # at: beat of the pattern from 1, in quarter beats
      - { at: 1,   shape: ramp-up, length: 0.5 beats, level: 100% }
      - { at: 2,   shape: sine,    length: 0.5 beats, level: 80% }
      - { at: 3.5, shape: ramp-up, length: 0.5 beats, level: 100% }
  40:
    name: bridge-chase
    type: pixel
    shape: bar           # optional, default bar
    effect: chase        # optional, default static
    params: { color: amber, width: 0.2, speed: 1 bar, direction: left-to-right }
    seed: 7              # optional, default 0
    rgbw: white-rich     # optional, default saturated
    fade_in: 500ms       # optional, default 0
    release: 1s          # optional, default 0
    layer_2:             # optional
      { shape: circle, effect: pulse, blend: add, params: { color: white, width: 0.1 } }
```

- `safe_look`, `values` and `target` use the [look](#looks) keys.
- Durations are a number then `ms`, `s`, `beat(s)` or `bar(s)`: `500ms`, `1.5s`, `0.5 beats`, `4 bars`. A beat is a quarter note, as in MIDI clock and Reaper's BPM, so a bar of 6/8 is 3 beats. Beats and bars follow the MIDI clock while they run, tempo changes included; without a clock they run at the song's `bpm`. `fade_in`, `release` and `duration` all accept every unit.
- Cue types:
  - `scene` holds a look: it fades in over `fade_in` from note-on and out over `release` from note-off.
  - `swell` reaches its `target` look along `curve` over `duration` from note-on, then holds where the curve ends until note-off, and fades out over `release`. Curves: `ramp-up`, `ramp-down`, `ease-in` (slow start), `sine` (up and back down), `hold` (the target at once), or custom points `{ points: [[0, 0], [0.5, 1], [1, 0.3]] }`: `[time, level]` pairs, both 0–1, times rising, joined by straight lines. The level is how much of the target shows over the layers below, as a scene's fade does.
  - `pattern` plays `hits` in a loop of `length` bars until note-off, then fades out over `release` from where it was at note-off. Each hit plays a `shape` (any swell curve) scaled to `level` over its `length` (beats or bars, at most the pattern's length), from beat `at` of the pattern: 1 to the beats in `length`, in quarter beats. A hit running past the end of the loop carries on into the next pass, and where hits overlap the highest one wins.
    - Start: the loop starts on the next bar line after note-on, so a note just before a bar line waits for it. A note up to one MIDI clock tick (1/24 beat) after a bar line starts it at once, in step with that bar line. That window absorbs MIDI jitter and a note placed on the bar but counted a tick late; it is 18–27 ms at 140–92 BPM, under one frame, so the first hit loses at most a frame of light. Bar lines follow Reaper's bar position from the MIDI clock, or count from the song select without one.
    - `target: { all-wash: dimmer }` scales the intensity the layers below give those fixtures by the pattern's level (their dimmer channel, or their colour channels without one): dark between hits, so put a scene under it for colour. Velocity sets how deep it cuts: 127 goes dark between hits, 64 halfway. On release the fixtures fade back to the layers below.
    - `target: { all-wash: { color: amber } }` takes look settings, as a swell's `target`: the level is how much of that look shows over the layers below. Every entry of one target is either `dimmer` or look settings.
  - `pixel` draws a preset `shape` on the canvas, animated by an `effect`, at every pixel of every fixture and grid. It fades in over `fade_in` from note-on and out over `release` from note-off. See [pixel cues](#pixel-cues).

### Pixel cues

The effect is worked out in RGB at each pixel's canvas position, then converted per fixture: RGBW pixels follow `rgbw` (`saturated` or `white-rich`, as in looks). Velocity is the cue's intensity: it goes on the dimmer channel of fixtures that have one, and scales the colour of the others. A pixel cue writes only what it drives: the colour emitters of fixtures and grids, and the dimmer channel of a fixture whose pixels it draws. Fixtures without colour emitters, and other channels such as a shutter or strobe, keep the layers below. A frame depends only on the time and beats since the note, and on `seed`.

`params`, all optional:

| Key | Value | Default |
| --- | --- | --- |
| `color` | Palette name or `#rrggbb`: the lit colour | white |
| `background` | Palette name or `#rrggbb`: where the shape isn't lit | black |
| `width` | Above 0, up to 1, of the canvas: a bar's or ring's thickness, a chase's step (each split bar is `width` wide, so a split chase steps half as many times) | 1 |
| `speed` | Duration of one cycle, any unit; beats and bars follow the tempo | `1 bar` |
| `direction` | `left-to-right` (as seen from the audience: stage right to stage left), `right-to-left`, `bottom-to-top` (floor up), `top-to-bottom` | `left-to-right` |
| `phase` | Offset in cycles: shifts a moving effect in time, a static shape along its travel | 0 |
| `curve` | A pulse's level over one cycle, any swell curve | `sine` |

Shapes, each with a position along its travel from 0 to 1:

| Shape | Lit |
| --- | --- |
| `bar` | A band `width` wide across the direction, centred on the position |
| `sweep` | Everything from the start edge up to the position: a wipe |
| `circle` | A ring `width` thick around the canvas centre, at the position as a fraction of the distance to the corners: growing for `left-to-right` and `bottom-to-top`, shrinking for the other two |
| `split` | The canvas split into halves mirrored about the centre line across the direction, with a bar `width` wide in each: the two bars travel from the centre out to the edges for `left-to-right` and `bottom-to-top`, from the edges in for the other two |
| `gradient` | A ramp from `background` to `color` along the direction, scrolled by the position |
| `one-by-one` | One cell at a time, in order of where its pixels sit along the direction. A profiled fixture is one cell; a grid is one cell per line across the direction: per column for `left-to-right` and `right-to-left`, per row for the vertical directions. A column grid so steps pixel by pixel going up or down. Cells at the same position along the direction, a fixture's at its centre, light together: two grids hung one above the other step left to right as one |

Effects, each repeating once per `speed`, counted from the note:

| Effect | Does |
| --- | --- |
| `static` | Holds the shape mid-travel; `phase` moves it |
| `move` | Glides the shape along its travel |
| `chase` | Steps the shape along its travel in 1 / `width` steps; `one-by-one` steps a cell at a time, whatever `width` |
| `pulse` | Holds the shape mid-travel, its level following `curve` |
| `sparkle` | Holds the shape mid-travel; each of its pixels flashes once per cycle at a random moment, fading over a quarter cycle. `seed` fixes the pattern |

Plan §7's other effect names map onto these: `solid` is `static` with the default width of 1, `wave` is `gradient` with `move`, and `strobe` is `pulse` with a square curve such as `{ points: [[0, 1], [0.2, 1], [0.21, 0]] }`. `noise` has no equivalent; `sparkle` is the closest.

`layer_2` takes its own `shape`, `effect` and `params`, plus `blend`: `max` (the brighter of the two per emitter, the default), `add` (the sum, clipped at full) or `mask` (layer 1 shows only where layer 2's shape is lit, as much as it is). Layer 2's sparkle differs from layer 1's under the same seed.

The engine refuses a song whose looks name an unknown fixture, group or colour, or whose program another song already uses. Every load error names the file and the line, for example `songs/tell-me.yaml: line 9: cue 36: all-wash: no colour "teal" in palettes.yaml, and not a #rrggbb value`.

## Playback

What the MIDI does (plan §5). Output depends only on the show and the MIDI received since the last song select.

| MIDI | Effect |
| --- | --- |
| Channel 2 Program Change | Selects the song with that `program` and drops every active cue. Another value selects no song. Before the first one, no song is selected and channel-2 notes do nothing. |
| Channel 2 note-on | Starts the cue in that slot of the current song. Velocity scales its dimmers (127 = as written). A cue already running on that note fades out under the new one. |
| Channel 2 note-off (or velocity 0) | Releases the cue: it fades out over its `release`. |
| Channel 2 CC 7 | Master dimmer, 0-127 → 0-100%. Starts at 100%. |
| Channel 2 CC 20-27 | Live parameters 1-8, stored and streamed. Nothing uses them yet. |
| Channel 16 note 127 | Panic: clears the legacy layer and every cue, then shows the song's `safe_look` until the next song select. Master and live parameters are kept. |
| MIDI clock (24 per beat), Start, Stop, Continue, Song Position | Tempo and bar position for beat and bar durations. Each song select starts the song's musical time afresh. |

Fades run from the note's timestamp, not from the next frame.

**Musical time.** Beats come from counting clock ticks, smoothed through jitter. Without a clock, or 250 ms after it goes silent, they run at the song's `bpm`; an edited `bpm` sets the pace from the next frame, without moving musical time (the builder's metronome). When ticks come back they catch up with the clock over about a beat, never jumping or running backwards, so fades stay smooth. After Reaper stops, running swells finish at the song's `bpm`. The engine logs each change of tempo source on stderr, e.g. `blimey: tell-me: no MIDI clock: using the song's 92 BPM` or `blimey: tell-me: following the MIDI clock`. In a MIDI file for `blimey replay`, clock and transport messages are stored as SMF escape events (`F7 01 F8`). A scene crossfades each channel from the layers below it to its own value, linearly. Cues stack in the order they were fired, later ones on top, except that pixel cues always draw above the others.

Layers, lowest first (plan §5): legacy notes, the safe look after panic, scene, swell and pattern cues, pixel cues, then the master dimmer. Within the cue and pixel layers, a cue fired later draws over earlier ones. The master scales dimmer channels, the colour channels of fixtures without a dimmer channel, and channels no profiled fixture uses (legacy-only, and pixel grids, on every Art-Net universe too). Other channels, such as a shutter, keep their value.

## Looks

A look sets colour, dimmer and RGBW mode per fixture, by fixture or group name. Show files write it as `{ color: deep-blue, dimmer: 60% }`; the CLI as `color=deep-blue dimmer=60%`.

| Key | Value | Unset means |
| --- | --- | --- |
| `color` | Palette name, or `#rrggbb` | Black for a fixture not yet set; otherwise unchanged |
| `dimmer` | Percentage, `0%`–`100%` | 100% for a fixture not yet set; otherwise unchanged |
| `rgbw` | `saturated` (white off) or `white-rich` (the part common to R, G and B goes to white) | `saturated` |

The look renders above the legacy layer (plan §5): every channel of a fixture in the look comes from the look, with channels it doesn't drive at their profile default. Fixtures the look doesn't mention keep the legacy values. A fixture with a dimmer channel gets the look's dimmer there; a fixture without one, like the Wild Wash in 16ch, has its colour channels scaled by it.

## CLI

```sh
blimey validate <show>                                              # load the show; print the first error with file and line
blimey run <show> --dmx-port /dev/cu.usbserial-… [--midi-port IAC]  # live: MIDI in, DMX out, hot reload
blimey run <show> --listen 127.0.0.1:7340 [--dmx-port …] [--edit]   # and the web UI
blimey fire <show> --set <fixture-or-group> key=value… [--set …]   # print the look as a one-frame dump
blimey fire <show> --set … --dmx-port /dev/cu.usbserial-…           # hold it on the Enttec box
blimey fire <show> --set … --listen 127.0.0.1:7340 [--edit]         # hold it and serve the web UI
blimey fire <show> --set … --artnet [<ip>[:port]] [--artnet-universe <n>]   # hold it and mirror it over Art-Net
blimey run <show> --dmx-port … --artnet [<ip>[:port]] [--artnet-universe <n>]
blimey run <show> … [--state-dir <folder>]                         # where state and event log live
blimey service install [--dry-run] <run arguments>                  # keep `blimey run <run arguments>` going under launchd
blimey service uninstall
blimey fixture import <ofl.json> [--out <show>/fixtures/<name>.yaml]
```

`validate` runs the same checks as loading the show: it exits 0 and prints `<show>: ok, N fixtures, N songs`, or exits 1 with the error.

`run` needs at least one of `--dmx-port`, `--listen` and `--artnet`. In edit mode it reloads the show whenever a file in its folder changes, keeping the selected song and active cues, which take their new definitions. A file that fails to load is reported on stderr and the previous show keeps running.

`run` survives losing its devices: a MIDI port that is missing at start, disappears or comes back as a new port with the same name is reconnected within about half a second, and an Enttec box that is unplugged is reopened on every frame until it is back. Both are reported on stderr.

`fire` goes through the same render loop as `blimey run`, with no MIDI port; with `--listen` in edit mode, MIDI posted to `POST /api/midi` still plays. Without `--dmx-port`, `--listen` or `--artnet` it prints a [frame dump](frame-dump.md) with a single frame at 0 ms. With any of them, it renders at 40 Hz until killed, reloading the show in edit mode; the look keeps applying to the fixtures it names.

`--listen` serves the web UI at `http://<address>/`, the WebSocket stream at `/ws` and the HTTP API at `/api`. Once bound it prints `streaming on ws://<address>/ws`, then `UI on http://<address>/ in <mode> mode`; port 0 picks a free port.

**Modes.** Show mode is the default, for gigs: the engine reads the show once at start and never again, never writes to the show folder, and the API refuses every change with `403`: saves, cue edits, undo, look previews, posted MIDI and fixture tests. Reads still work. `--edit` starts it in edit mode instead: the show is hot reloaded and the UI may do all of these. Art-Net is off in both unless `--artnet` is given. A browser build of the UI comes embedded in the binary; build it with `pnpm --dir ui install && pnpm --dir ui build` before `cargo build`, or the binary serves a page saying so.

`--artnet` mirrors every frame as one ArtDmx packet (Art-Net 4, protocol version 14) to an IPv4 node, unicast or broadcast; the port defaults to 6454 and `--artnet` alone sends to 127.0.0.1. `--artnet-universe` is the 15-bit port-address (Net, Sub-Net, Universe), from 0, default 0. Art-Net is off unless asked. A send error is reported once on stderr and never stops the render loop. The same node also gets every Art-Net universe the rig's pixel grids use, one packet each per frame, blackout included; each universe numbers its packets on its own. If a grid uses universe 0 and `--artnet-universe` isn't given, the grid keeps universe 0 and the Enttec frame isn't mirrored (the engine says so on stderr): give `--artnet-universe` to mirror it elsewhere. An `--artnet-universe` that is also a grid's universe is refused. Without `--artnet`, a rig with Art-Net grids starts with a warning that they get no output. See [blenderdmx.md](blenderdmx.md) for a 3D preview.

## State, event log and service

`run` keeps two files in its state folder, `~/Library/Application Support/Blimey/` unless `--state-dir` says otherwise. Nothing goes in the show folder. Both are diagnostics, written on a background thread: a state folder the engine can't create or write is reported on stderr, and the show runs without them.

- `state.json`, `{"song": "tell-me", "master": 0.5}`: the selected song (or `null`) and the master, 0–1, rewritten atomically whenever either changes. The background thread saves a change a few milliseconds after the frame that shows it; a crash inside that window resumes the state from before the change. On start the engine resumes them, as if it had received the song's Program Change and the master's CC 7, then waits for the next cue. A saved song the show no longer has is dropped; an unreadable file gives a fresh start. Both are reported on stderr.
- `events.log`: one JSON object per line, for post-gig diagnosis. Each line is written unbuffered as soon as the writer gets it, so a crash loses at most the lines still queued. Past 10 MB it rotates to `events.log.1`, up to `events.log.4`; if rotating fails, logging carries on in `events.log`.

```json
{"master":0.5,"mode":"show","show":"shows/fossile","song":"tell-me","time_ms":1794470400000,"type":"start"}
{"at_ms":1250,"message":[145,36,127],"time_ms":1794470401250,"type":"midi"}
{"action":"start","at_ms":1250,"name":"verse-blue","note":36,"song":"tell-me","time_ms":1794470401250,"type":"cue"}
{"action":"release","at_ms":4000,"name":"verse-blue","note":36,"song":"tell-me","time_ms":1794470404000,"type":"cue"}
```

- `start`: the engine started, with the song and master it resumed.
- `midi`: every MIDI message taken in, as in the stream's `midi` message, clock ticks and active sensing left out. The resumed song and master follow `start` as `midi` lines at `at_ms` 0.
- `cue`: a cue started (note-on) or released (note-off, or a retrigger fading out the previous instance). Panic and song select drop cues without a `release` line.
- `at_ms` is engine time, as in the stream; `time_ms` is wall-clock time, in milliseconds since 1970. `jq '.time_ms / 1000 | todate'` reads it.

`blimey service install <run arguments>` makes the checks `run` makes at start (an output is given, the show loads), so launchd never keeps restarting an engine that exits at once. It then writes the launch agent `~/Library/LaunchAgents/app.blimey.engine.plist` and starts it. launchd starts the engine at login and restarts it within a second whenever it exits. Relative paths are taken from the folder the command ran in, and the engine's stdout and stderr go to `engine.log` in the state folder. Installing again replaces the agent; `--dry-run` prints the plist instead. `blimey service uninstall` stops and removes it, even if it wasn't running.

`scripts/fault-injection.sh [kill|midi|serial|state]` (macOS) checks recovery against the real binary, an emulated Enttec box on a pty and a virtual CoreMIDI port: killing the engine mid-song, dropping the MIDI port, unplugging the box, starting with an unusable state folder.

## HTTP API

Errors come back as `{ "error": "<what went wrong>" }` with `404` (unknown file, song, cue or fixture), `403` (show mode), `409` (conflict: a note already taken, a song name that differs from another only by case, a cue changed since it was loaded, or a file changed since the save to undo), `422` (content refused, or a request body not in the shape documented) or `500`.

**One write path.** Every endpoint that changes a show file, `PUT /api/show/<file>` and the song endpoints below alike, goes through the engine's one writer: it validates, writes atomically and keeps canonical formatting, as described for `PUT /api/show/<file>`. Each save can be undone.

### Show summary: `GET /api/show`

What the builder needs to offer choices: palette colours as `#rrggbb`, and the songs with the Program Change that selects each.

```json
{ "palettes": { "deep-blue": "#0028ff" }, "songs": [{ "name": "tell-me", "program": 3 }] }
```

### Show files: `GET` and `PUT /api/show/<file>`

Reads and replaces a whole show file. `<file>` is a path in the show folder: `rig.yaml` or `songs/<name>.yaml`, a name of letters, digits, `-` and `_`. The body of both is the file's content as JSON, in the shape of its schema, keys in file order (a look's entries apply in order).

`PUT` replaces the file, or creates a song (`204` on success):

1. The content is checked against the file's JSON Schema in `schema/`, then by the engine's own loader. A song is judged by loading the whole show as it would be with the new content in place, the same checks as at start-up, so a song broken on disk can be fixed through this endpoint. Either refusal is a `422` naming the problem and the file at fault, such as `rig.yaml: /fixtures/0/canvas/x: 1.5 is greater than the maximum of 1` or `songs/tell-me.yaml: program 3 already selects song "other"`, and leaves the file untouched. A new song whose name differs from an existing one only by case is refused with `409`: macOS would write it over the other.
2. The file is written in canonical form. Its leading comment block is kept; other comments are not. Writing back what `GET` returned changes nothing for a file already in canonical form, so an edit changes only the lines it touches.
   - `rig.yaml`: block-style YAML with fields in a fixed order.
   - Songs: block-style YAML, two-space indents, keys in the order of the schema's `properties` (each cue's by its type's definition) and cues by note number. A mapping or list of plain values sits on one line: `all-wash: { color: deep-blue, dimmer: 60% }`, `- [0.5, 1]`.
3. The write is atomic: a crash leaves the old file or the new one, never half of one.
4. Hot reload picks the file up on the next frame: a readdressed fixture moves on every output, and stream clients get the new `rig` message. A `204` means the file is written, not yet rendered.

To move a fixture, `GET` the file, change the fixture's `canvas`, `PUT` it back.

### Cues: `PUT` and `DELETE /api/songs/<song>/cues/<note>`

Each reads, edits and writes `songs/<song>.yaml` under one lock, so no other save lands in between, then writes it as `PUT /api/show/<file>` does.

`PUT` saves one cue on a channel-2 note (`204`):

```json
{ "cue": { "name": "verse-blue", "type": "scene", "fade_in": "2s", "values": { "all-wash": { "color": "deep-blue" } } },
  "replaces": { "note": 37, "cue": { "name": "verse", "type": "scene", "values": {} } } }
```

- `cue` is the cue as in the song file.
- `replaces`, when editing a cue, is its note and the cue as it was loaded; the cue moves from that note to `<note>`. Omit it, or send `null`, for a new cue.
- Refused, leaving the file untouched:
  - a note that another cue already uses, with `409`: `songs/tell-me.yaml: note 36 already fires cue "verse-blue"; pick a free note, or edit that cue`. The engine never merges or overwrites a cue it wasn't asked to replace;
  - a replaced cue that changed since it was loaded, with `409`, or is no longer there, with `404`, so a stale editor never overwrites someone else's change.

`DELETE` removes the cue on `<note>` (`204`; `404` if there is none).

### Metronome tempo: `PUT /api/songs/<song>/bpm`

Sets the song's `bpm`, the tempo for beats and bars when no MIDI clock comes in, leaving the rest of the file as it is (`204`). The body is `{ "bpm": 92 }`. It goes under the same lock as the cue endpoints and takes effect from the next frame.

### Undo: `POST /api/undo`

Undoes the last save made through the API since the engine started, whichever file it was: `PUT /api/show/<file>` or any song endpoint above. The previous content goes back byte for byte, comments and formatting included, after the same checks as a write; a file the save created is removed. Returns `{ "file": "songs/tell-me.yaml" }`.

One level only: a second undo, or one before any save, is a `404`. If the file has changed since the save (by hand, say), undo is refused with `409` rather than lose that change. A refused undo can be tried again, once the file is put back as the save left it.

### Look preview: `PUT` and `DELETE /api/preview`

`PUT` shows a look on every output while the builder edits it (`204`). The body is a look in the shape of a scene's `values`, `{ "all-wash": { "color": "deep-blue", "dimmer": "60%" } }`, resolved like one: an unknown fixture, group or colour is a `422`. The preview draws above every layer, the master dimmer included, on the fixtures it names; the others keep the show's output. Nothing is saved.

It lasts 5 s of engine time from the first frame drawn after the latest `PUT`, so the builder sends it again every few seconds while it shows; a builder that closed or crashed leaves no look on stage. `DELETE`, or a channel-2 note-on posted to `POST /api/midi` (an audition, which should show the cue), clears it at once.

### MIDI: `POST /api/midi`

Plays one MIDI message as if it came in from the MIDI port, to audition cues without Reaper (`204`):

```json
{ "message": [145, 36, 127] }
```

`message` is the raw bytes, status byte first, in the same shape as the stream's `midi` message. It acts on the next frame, stamped with that frame's time, and is streamed as a `midi` message. Anything but one well-formed message is a `422`. A channel-2 note-on also clears the look preview. To fire a cue: Program Change for its song on channel 2 (`[193, <program>]`) unless the song is already selected, then note-on (`[145, <note>, 127]`); release it with note-off (`[129, <note>, 0]`).

### Fixture test: `POST /api/fixtures/<name>/test`

Lights the fixture full white for one second, above every other layer, on every output (`204`). Edit mode only.

## WebSocket stream: `GET /ws`

Server to client only, one JSON object per text message. Every message has a `type`. Clients ignore types and fields they don't know, so later chunks can add them without breaking the UI.

### `rig`: sent first, on connect, and again whenever the rig the engine renders changes

```json
{
  "type": "rig",
  "protocol": 1,
  "mode": "edit",
  "fixtures": [
    {
      "name": "wash-1",
      "profile": "stairville-wild-wash-pro-648-rgbw",
      "mode": "16ch",
      "address": 1,
      "channels": ["Red 1", "Green 1", "Blue 1", "White 1", "…"],
      "pixels": [{ "x": 0.115625, "y": 0.2 }, "…"]
    }
  ],
  "grids": [
    { "name": "back-grid", "pixels": [{ "x": 0.21875, "y": 0.525 }, "…"] }
  ],
  "groups": { "all-wash": ["wash-1", "wash-2", "wash-3", "wash-4"] }
}
```

- `protocol` is bumped only by a breaking change to this stream.
- `mode`: `edit` or `show` (see [Modes](#cli)).
- `channels` lists channel names in DMX order, starting at `address`.
- `pixels` are canvas positions, in pixel order; for a grid, in wiring order.
- `grids`: the rig's [pixel grids](#rigyaml), apart from `fixtures` since they have no profile, mode or single address.

### `frame`: the output, after each rendered frame

```json
{
  "type": "frame",
  "at_ms": 1250,
  "dmx": [0, 10, 255, "… 512 values"],
  "pixels": { "wash-1": ["#000aff", "#000aff", "#000aff", "#000aff"] },
  "song": "tell-me",
  "cues": [{ "note": 36, "name": "verse-blue", "intensity": 1.0, "released": false }],
  "master": 1.0,
  "live_parameters": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}
```

- `at_ms`: engine time when the frame was output, in milliseconds.
- `dmx`: the universe as output, 512 values of 0–255. Index 0 is DMX channel 1.
- `pixels`: per fixture and per grid, the colour each pixel shows, in the same order as the `rig` message. It is computed from the output (white added to R, G and B, then the dimmer channel applied), so the preview always matches it: from `dmx`, or for a grid on Art-Net from that universe, which the stream doesn't carry.
- `song`: the selected song's name, or `null`.
- `cues`: the active cues, lowest layer first. `intensity` is the note-on velocity as 0–1; `released` is true while the cue fades out after its note-off.
- `master`: the master dimmer, 0–1.
- `live_parameters`: CC 20–27 on channel 2, as 0–1.

The engine renders at 40 Hz. A slow client skips frames and always gets the latest one.

### `midi`: each MIDI message the engine takes in

```json
{ "type": "midi", "at_ms": 1250, "message": [145, 36, 127] }
```

- `at_ms`: engine time the message arrived, on the same clock as `frame`.
- `message`: the raw bytes, status byte first.

Sent for MIDI from the port (`run`) and from `POST /api/midi`, in arrival order, before the frame they affect. MIDI clock ticks (`F8`) and active sensing (`FE`) are left out; Start, Stop, Continue and Song Position are sent. A client more than 256 messages behind skips the oldest.
