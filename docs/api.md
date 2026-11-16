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
- All the grids together may use at most 32 Art-Net universes (about 5,400 RGB pixels), four times the plan's 8-universe load target, so a typo in a grid's size can't stall the 40 Hz render: `the pixel grids use 48 Art-Net universes; pixel grids may use at most 32 in all, so the render keeps up at 40 Hz`.
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

The song's name is its file name (`songs/demo-song.yaml` is `demo-song`).

```yaml
program: 3          # Program Change value on channel 2, 0-127, unique across songs
bpm: 92             # optional: tempo for beats and bars when no MIDI clock arrives; default 120
time_signature: 4/4 # optional: beats per bar over the note value; default 4/4
audio: { file: audio/tell-me.mp3, offset: 1.25s }  # optional: see Audio below
markers:            # optional: see Markers below
  - { name: Chorus, at: 42.5s }
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
  43:
    name: chorus-build
    type: pixel
    release: 1s
    layers:              # bottom first, instead of shape, effect, params and layer_2
      - area: { x: 0, y: 0, width: 0.5, height: 1 }
        shape: fill
        params: { color: warm-white }
      - area: { x: 0.5, y: 0, width: 0.5, height: 1 }
        shape: bar
        effect: chase
        params: { color: amber, width: 0.25, speed: 1 bar }
        blend: over
      - area: { x: 0, y: 0.65, width: 1, height: 0.35 }
        shape: fill
        effect: sparkle
        params: { color: steel-blue, width: 0.4, speed: 2 beats }
        level: 60%
        blend: add
        mute: [inner-wash]
sections:                # optional: the song's running order, see below
  verse:
    bars: 16
    blocks:
      - { scene: 36, bar: 1, length: 16 bars }
      - { scene: 38, bar: 9, length: 8 bars, level: 80% }
  bridge:
    bars: 4
    time_signature: 3/4  # optional, default the song's
    blocks:
      - { scene: 40, bar: 1, beat: 2.5, length: 6 beats }
arrangement: [verse, bridge, verse]
```

- `safe_look`, `values` and `target` use the [look](#looks) keys.
- Durations are a number then `ms`, `s`, `beat(s)` or `bar(s)`: `500ms`, `1.5s`, `0.5 beats`, `4 bars`. A beat is a quarter note, as in MIDI clock and a DAW's BPM, so a bar of 6/8 is 3 beats. Beats and bars follow the MIDI clock while they run, tempo changes included; without a clock they run at the song's `bpm`. `fade_in`, `release` and `duration` all accept every unit.
- Cue types:
  - `scene` holds a look: it fades in over `fade_in` from note-on and out over `release` from note-off.
  - `swell` reaches its `target` look along `curve` over `duration` from note-on, then holds where the curve ends until note-off, and fades out over `release`. Curves: `ramp-up`, `ramp-down`, `ease-in` (slow start), `sine` (up and back down), `hold` (the target at once), or custom points `{ points: [[0, 0], [0.5, 1], [1, 0.3]] }`: `[time, level]` pairs, both 0–1, times rising, joined by straight lines. The level is how much of the target shows over the layers below, as a scene's fade does. `target: { bridge-chase: intensity }` drives a pixel cue's intensity instead: see [intensity links](#intensity-links).
  - `pattern` plays `hits` in a loop of `length` bars until note-off, then fades out over `release` from where it was at note-off. Each hit plays a `shape` (any swell curve) scaled to `level` over its `length` (beats or bars, at most the pattern's length), from beat `at` of the pattern: 1 to the beats in `length`, in quarter beats. A hit running past the end of the loop carries on into the next pass, and where hits overlap the highest one wins.
    - Start: the loop starts on the next bar line after note-on, so a note just before a bar line waits for it. A note up to one MIDI clock tick (1/24 beat) after a bar line starts it at once, in step with that bar line. That window absorbs MIDI jitter and a note placed on the bar but counted a tick late; it is 18–27 ms at 140–92 BPM, under one frame, so the first hit loses at most a frame of light. Bar lines follow the DAW's bar position from the MIDI clock, or count from the song select without one.
    - `target: { all-wash: dimmer }` scales the intensity the layers below give those fixtures by the pattern's level (their dimmer channel, or their colour channels without one): dark between hits, so put a scene under it for colour. Velocity sets how deep it cuts: 127 goes dark between hits, 64 halfway. On release the fixtures fade back to the layers below.
    - `target: { all-wash: { color: amber } }` takes look settings, as a swell's `target`: the level is how much of that look shows over the layers below. Every entry of one target is either `dimmer` or look settings.
    - `target: { bridge-chase: intensity }` drives the intensity of the song's pixel cues of that name, as `dimmer` drives fixtures: see [intensity links](#intensity-links).
  - `pixel` draws layers of preset `shape`s, each in an area of the canvas and animated by an `effect`, at every pixel of every fixture and grid. It fades in over `fade_in` from note-on and out over `release` from note-off. See [pixel cues](#pixel-cues).

### Pixel cues

The effect is worked out in RGB at each pixel's canvas position, then converted per fixture: RGBW pixels follow `rgbw` (`saturated` or `white-rich`, as in looks). Velocity is the cue's intensity, scaled by any [intensity link](#intensity-links): it goes on the dimmer channel of fixtures that have one, and scales the colour of the others. A pixel cue writes only what it drives: the colour emitters of fixtures and grids, and the dimmer channel of a fixture whose pixels it draws. Fixtures without colour emitters, and other channels such as a shutter or strobe, keep the layers below. A frame depends only on the time and beats since the note, and on `seed`.

`params`, all optional:

| Key | Value | Default |
| --- | --- | --- |
| `color` | Palette name or `#rrggbb`: the lit colour | white |
| `background` | Palette name or `#rrggbb`: where the shape isn't lit | black |
| `width` | Above 0, up to 1, of the layer's area: a bar's or ring's thickness, a chase's step (each split bar is `width` wide, so a split chase steps half as many times) | 1 |
| `speed` | Duration of one cycle, any unit; beats and bars follow the tempo | `1 bar` |
| `direction` | `left-to-right` (as seen from the audience: stage right to stage left), `right-to-left`, `bottom-to-top` (floor up), `top-to-bottom` | `left-to-right` |
| `phase` | Offset in cycles: shifts a moving effect in time, a static shape along its travel | 0 |
| `curve` | A pulse's level over one cycle, any swell curve | `sine` |

Shapes, each with a position along its travel from 0 to 1, in the layer's area as if it were the whole canvas:

| Shape | Lit |
| --- | --- |
| `bar` | A band `width` wide across the direction, centred on the position |
| `sweep` | Everything from the start edge up to the position: a wipe |
| `circle` | A ring `width` thick around the area's centre, at the position as a fraction of the distance to the corners: growing for `left-to-right` and `bottom-to-top`, shrinking for the other two |
| `split` | The area split into halves mirrored about the centre line across the direction, with a bar `width` wide in each: the two bars travel from the centre out to the edges for `left-to-right` and `bottom-to-top`, from the edges in for the other two |
| `gradient` | A ramp from `background` to `color` along the direction, scrolled by the position |
| `one-by-one` | One cell at a time, in order of where its pixels sit along the direction. A profiled fixture is one cell; a grid is one cell per line across the direction: per column for `left-to-right` and `right-to-left`, per row for the vertical directions. A column grid so steps pixel by pixel going up or down. Cells at the same position along the direction, a fixture's at its centre, light together: two grids hung one above the other step left to right as one. It steps through the cells with a pixel in the layer's area, muted ones and fixtures without colour emitters left out |
| `fill` | The whole area |

Effects, each repeating once per `speed`, counted from the note:

| Effect | Does |
| --- | --- |
| `static` | Holds the shape mid-travel; `phase` moves it |
| `move` | Glides the shape along its travel |
| `chase` | Steps the shape along its travel in 1 / `width` steps; `one-by-one` steps a cell at a time, whatever `width` |
| `pulse` | Holds the shape mid-travel, its level following `curve` |
| `sparkle` | Holds the shape mid-travel; each of its pixels flashes once per cycle at a random moment, fading over a quarter cycle. `seed` fixes the pattern |

Plan §7's other effect names map onto these: `solid` is `static` with the default width of 1, `wave` is `gradient` with `move`, and `strobe` is `pulse` with a square curve such as `{ points: [[0, 1], [0.2, 1], [0.21, 0]] }`. `noise` has no equivalent; `sparkle` is the closest.

**Layers.** `layers` stacks 1 to 8 layers, bottom first. Each takes its own `shape`, `effect` and `params`, plus:

| Key | Value | Default |
| --- | --- | --- |
| `area` | `{ x, y, width, height }` in canvas units, as a grid's `canvas`: (`x`, `y`) is the bottom-left corner, `y` 0 the floor. The layer draws on the pixels inside it, its left and bottom edges included and its right and top edges only where they are the canvas's, so a pixel on an edge two areas share is in one of them; the shape runs in the area's own 0–1 space, so `width` and `direction` are relative to it | the whole canvas |
| `level` | A percentage, how much the layer shows: it dims a `max` or `add` layer, as a scene's `dimmer`, and fades an `over` or `mask` layer into the ones below. At `0%` the layer changes nothing | `100%` |
| `blend` | How the layer meets the ones below it, inside its area: `max` (the brighter per emitter), `add` (the sum, clipped at full), `mask` (the layers below show only where this layer's shape is lit, as much as it is), or `over` (this layer covers the ones below as much as it is lit, so a lit pixel replaces them and a dark one lets them show; with a `background`, it covers its whole area) | `max` |
| `mute` | Fixtures, grids or groups the layer leaves to the layers below | none |
| `swell` | The layer's own rhythm: `{ duration: 4 bars, curve: ease-in }`, the `duration` and `curve` of a swell cue | none |
| `pattern` | The layer's own rhythm: `{ length: 1 bar, hits: [{ at: 1, shape: sine, length: 1 beat, level: 100% }] }`, the `length` and `hits` of a pattern cue | none |

A pixel no layer draws, outside every area or muted on every layer whose area holds it, keeps the layers below the cue, as a fixture without colour emitters does; a fixture none of whose pixels is drawn keeps its dimmer channel too. A pixel some layer draws is driven, dark where the layers leave it dark. Each layer's sparkle differs from the others' under the same `seed`. Outside its area, and on what it mutes, a layer changes nothing, a `mask` included.

**Layer rhythms.** A layer takes a `swell` or a `pattern`, not both, timed as the cue types of the same name: a swell runs along its curve from note-on, then holds; a pattern's loop starts on the bar line after note-on and repeats, dark between hits. The rhythm's level scales the layer as `level` does, so at 0 an `over` or `mask` layer lets the layers below show, and multiplies with `level`, with the cue's velocity and fades, and with any [intensity link](#intensity-links) on the cue. Each layer keeps its own rhythm. Specifics:

- Until its loop starts, a pattern leaves the layer as it is, as a pattern linked to the cue's intensity does. A one-layer cue with a rhythm so renders the same frames as that cue without it plus the same swell or pattern linked to its intensity, fired with it at velocity 127: the rhythm goes on the cue's intensity, so on the dimmer channel of fixtures that have one. With more layers, each rhythm scales its layer's colour.
- On note-off the rhythms stop where they are and the cue fades out from there over its own `release`: a release mid-hit fades over the cue's release time rather than finishing the hit.

The cue is refused at load, with its line, for an area with no width or height (`layer 1: area: width 0 and height 1 must both be above 0`) or past the canvas (`layer 2: area: canvas { x: 0.75, y: 0, width: 0.5, height: 1 } leaves the 0-1 stage canvas`), a `mute` name the rig doesn't have (`layer 1: mute: "inner-wash" is neither a fixture nor a group in the rig`), a layer with both rhythms (`layer 1: a layer has a swell or a pattern, not both`), a pattern a pattern cue couldn't play (`layer 2: pattern: length must be 1, 2, 3 or 4 bars`), more than 8 layers, and `layers` next to `shape`, `effect`, `params` or `layer_2`.

**The older form.** Instead of `layers`, a cue can give one layer's `shape`, `effect` and `params` at its top and an optional `layer_2` with its own, plus `blend`: the same two layers over the whole canvas, rendering the same frames.

### Intensity links

A swell or pattern can drive the intensity of a pixel cue in the same song instead of a look, by naming it with `intensity`:

```yaml
  38: { name: chase-in, type: swell, target: { bridge-chase: intensity }, duration: 4 bars }
  42:
    name: chase-pulse
    type: pattern
    length: 1 bar
    target: { bridge-chase: intensity }
    hits:
      - { at: 1, shape: ramp-up, length: 1 beat, level: 100% }
```

- The keys are pixel cue names; every pixel cue of that name follows. A name no pixel cue has is refused at load: `line 9: cue 42: bridge-chase: no pixel cue of that name in this song`. Every entry of a target that names pixel cues is `intensity`.
- While the link plays, the pixel cue's intensity is its velocity times the link's level: a swell's curve, a pattern's hits, dark between them. The link's velocity sets how deep it cuts, as for a `dimmer` pattern: 127 follows the level fully, 64 goes halfway. A pattern leaves the pixel cue as it is until its loop starts.
- On release the link fades out over its `release` and the pixel cue returns to its own intensity. A link with no pixel cue playing does nothing; a pixel cue fired later still follows it. Several links on one pixel cue multiply.
- The link draws nothing itself, on any fixture.

### Sections and arrangement

A song's running order (plan §17), built in Blimey and played by the DAW from an exported MIDI file.

- `sections` is keyed by name; `arrangement` lists section names in playing order, each as often as it plays. Editing a section changes every place it plays.
- `bars` is the section's length, a whole number from 1 to 9999; `time_signature` overrides the song's for that section's bar lines and its blocks' bars.
- A block holds `scene`, the note number of a cue of the song, from `bar` (from 1) and `beat` (from 1 to under the beats per bar plus one, in quarter beats, fractions allowed; default 1) for `length`, in `beats` or `bars` of the section's signature. `level` scales the scene as the note's velocity does: `round(level × 127)`, at least 1; default 100%.
- A block ends at or before its section's end. Different scenes may overlap and combine as they do live; two blocks of one scene may not overlap, though one may start where another ends.
- Refused at load, each naming its line: an unknown section in `arrangement` (`line 40: arrangement: no section "outro" in sections`), a scene not in `cues`, a block outside its bar or section, overlapping blocks of one scene (`line 31: section verse: block 2: overlaps block 1 of scene 36; a scene plays once at a time`), a `length` in `ms` or `s` or under 1/960 beat, `bars` outside 1-9999.
- **Bar grid.** With an arrangement, bar lines and bar positions follow each placed section's signature from its first beat, and the song's after the arrangement ends: a pattern fired on the first beat of a 3/4 section starts at once, on that section's bar line. Durations inside a cue (`length: 1 bar` on a pattern, `4 bars` on a swell) keep the song's signature. A song without an arrangement behaves as before.
- **Export.** `<show>/export/<song>.mid`, kept out of git, is written for each song with an arrangement whenever the show loads in edit mode, so every save is exported, and by `blimey export`; edit mode writes only the files whose content changed. The `.mid` files of songs no longer arranged (cleared, renamed or removed) are deleted from `export/` at the same time. Show mode writes nothing. A format 0 file at 960 ticks per beat: the song's tempo (its `bpm`, 120 without), a time signature at bar 1 and wherever a placed section's differs from the one before, a marker naming each placed section, a channel-2 Program Change `program` at bar 1, then a channel-2 note on per block (note = scene, velocity = level) and a note off at its end. At one tick, the order is: time signature, marker, Program Change, note offs, note ons. When one placement ends with a block the next starts with (same scene, same level, end and start on the boundary), the two are one held note, so the scene doesn't restart. In the DAW, insert the file at the song's first bar, and again after a change.

### Audio

A bounce of the song, which the builder's timeline plays along with the lights (plan §17); the engine itself never plays it.

- `file` is relative to the show folder and under `audio/`, which is kept out of git: put the bounce there and name it here. Any format the browser decodes: WAV, MP3, AAC, Ogg or FLAC.
- `offset` is where bar 1 falls in the file, in `ms` or `s`; default `0s`. The timeline sets it as the waveform is dragged.
- A missing file is not an error: the song plays silent, with a click. Refused at load: a `file` outside `audio/` (`line 4: audio: file "../a.mp3" must be a path under audio/, e.g. audio/tell-me.mp3`) and an `offset` in beats or bars.
- The timeline uploads the file through [`PUT /api/songs/<song>/audio`](#song-audio-upload-put-apisongssongaudio), which names it `audio/<song>.<ext>`.

### Markers

Named points in the song's audio, as set in Reaper, which the timeline draws as flags to find one's way: `markers: [{ name, at }]`, in time order.

- `at` is a time from the start of the audio file, in `ms` or `s`, not from bar 1, so a marker stays on the music when the offset changes. The timeline places it at `at - offset` on the song's tempo and bar grid; a marker before bar 1 isn't drawn.
- Refused at load: an empty `name` (`line 6: markers: a marker needs a name`) and an `at` in beats or bars.

The engine refuses a song whose looks name an unknown fixture, group or colour, or whose program another song already uses, and a `.yml` file in `songs/`, which would otherwise be skipped. Every load error names the file and the line, for example `songs/demo-song.yaml: line 9: cue 36: all-wash: no colour "teal" in palettes.yaml, and not a #rrggbb value`.

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

**Musical time.** Beats come from counting clock ticks, smoothed through jitter. Without a clock, or 250 ms after it goes silent, they run at the song's `bpm`; an edited `bpm` sets the pace from the next frame, without moving musical time (the builder's metronome). When ticks come back they catch up with the clock over about a beat, never jumping or running backwards, so fades stay smooth. After the DAW stops, running swells finish at the song's `bpm`. The engine logs each change of tempo source on stderr, e.g. `blimey: demo-song: no MIDI clock: using the song's 92 BPM` or `blimey: demo-song: following the MIDI clock`. In a MIDI file for `blimey replay`, clock and transport messages are stored as SMF escape events (`F7 01 F8`). A scene crossfades each channel from the layers below it to its own value, linearly. Cues stack in the order they were fired, later ones on top, except that pixel cues always draw above the others.

Layers, lowest first (plan §5): legacy notes, the safe look after panic, scene, swell and pattern cues, pixel cues, then the master dimmer. Within the cue and pixel layers, a cue fired later draws over earlier ones. The master scales dimmer channels, the colour channels of fixtures without a dimmer channel, and channels no profiled fixture uses (legacy-only, and pixel grids, on every Art-Net universe too). Other channels, such as a shutter, keep their value.

## Looks

A look sets colour, dimmer and RGBW mode per fixture, by fixture or group name. Show files write it as `{ color: deep-blue, dimmer: 60% }`; the CLI as `color=deep-blue dimmer=60%`.

| Key | Value | Unset means |
| --- | --- | --- |
| `color` | Palette name, or `#rrggbb` | The colour from the layers below, for a fixture not yet set; otherwise unchanged |
| `dimmer` | Percentage, `0%`–`100%` | 100% for a fixture not yet set; otherwise unchanged |
| `rgbw` | `saturated` (white off) or `white-rich` (the part common to R, G and B goes to white) | `saturated` |

The look renders above the layers below it (plan §5). Fixtures it doesn't mention keep their values from below.

Every fixture has a colour and an intensity. A fixture with a dimmer channel shows the intensity there and the colour on its colour channels; a fixture without one, like the Wild Wash in 16ch, has a **virtual dimmer**: its colour channels show the colour scaled by the intensity, and the engine keeps the colour at full beside them. So a dimmer means the same on both kinds of fixture.

- A fixture given a colour takes every channel from the look, with channels the look doesn't drive at their profile default: the colour as written, at the look's dimmer. A colour at 100% shows exactly as written, a dim palette colour included.
- A fixture given only a dimmer keeps everything below but its intensity: the colour from below, at full, at the look's dimmer. `deep-blue` at 60% under `{ dimmer: 100% }` becomes `deep-blue` at full, on a dimmer channel or a virtual one. A `{ dimmer: 0% }` blackout keeps that colour, so a dimmer-only swell after it brings the scene back up. With nothing below, there is no colour to light and the fixture stays dark. Legacy notes and other raw DMX below count as a colour at full, at intensity 100%. The colour below is already converted, so `rgbw` without a `color` (from this entry or an earlier one of the same look) is refused: `all-wash: rgbw needs a color for "wash-1": …`.

This applies wherever looks are used: scene `values`, swell `target`, a pattern's look target, the `safe_look` and `fire --set`. A scene or swell fades from the layers below toward that look, so `chorus-swell`'s `{ all-wash: { dimmer: 100% } }` brings the scene under it to full. Velocity scales the look's dimmer, as for any look.

## CLI

```sh
blimey validate <show>                                              # load the show; print the first error with file and line
blimey export <show> [song]                                         # write <show>/export/<song>.mid for the DAW, for each song with an arrangement
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
blimey record <out.mid> [--midi-port IAC] [--until-stop] [--artnet <emu.dump> --show <show> [--artnet-universe <n>]]
blimey soak <setlist.yaml> [--hours 4] [--accelerated | --emulated-enttec | <run outputs>]
```

`validate` runs the same checks as loading the show: it exits 0 and prints `<show>: ok, N fixtures, N songs`, or exits 1 with the error.

`export` loads the show and writes the [MIDI file](#sections-and-arrangement) of `song`, or of every song with an arrangement, printing each path written; without `song` it also deletes the files of songs no longer arranged. A named song without an arrangement, or not in the show, exits 1 with the error.

`run` needs at least one of `--dmx-port`, `--listen` and `--artnet`. In edit mode it reloads the show whenever a file in its folder changes, keeping the selected song and active cues, which take their new definitions. A file that fails to load is reported on stderr and the previous show keeps running.

`run` survives losing its devices: a MIDI port that is missing at start, disappears or comes back as a new port with the same name is reconnected within about half a second, and an Enttec box that is unplugged or stops taking frames is reopened on every frame until it is back, on a thread of its own so the render loop never waits for it. Both are reported on stderr, once when a device goes and once when it is back.

`run` also survives its own bugs and a broken show. A cue that fails to draw (a panic) is dropped until it is fired again, with a line on stderr naming the song and cue; the rest of the frame renders. A panic on a MIDI thread, or the Enttec writer stopping, exits the engine with the reason, for launchd to restart it; a panic on the thread writing `state.json` and `events.log` only stops those, as the light doesn't need them. A show that fails to load at start is reported, and the last show that loaded stands in (see the state folder below); without one, `run` exits with the error. After a stall, frames more than 4 periods late are skipped, and output picks up at the frame due.

`fire` goes through the same render loop as `blimey run`, with no MIDI port; with `--listen` in edit mode, MIDI posted to `POST /api/midi` still plays. Without `--dmx-port`, `--listen` or `--artnet` it prints a [frame dump](frame-dump.md) with a single frame at 0 ms. With any of them, it renders at 40 Hz until killed, reloading the show in edit mode; the look keeps applying to the fixtures it names.

`--listen` serves the web UI at `http://<address>/`, the WebSocket stream at `/ws` and the HTTP API at `/api`. Once bound it prints `streaming on ws://<address>/ws`, then `UI on http://<address>/ in <mode> mode`; port 0 picks a free port. An address that can't be bound, such as a port already taken, is reported on stderr and the engine runs without the UI, DMX and MIDI included, unless the UI is its only output: then it exits with the error.

**Modes.** Show mode is the default, for gigs: the engine reads the show once at start and never again, never writes to the show folder, and the API refuses every change with `403`: saves, new songs, cue edits, undo and redo, previews, posted MIDI and fixture tests. Reads still work. `--edit` starts it in edit mode instead: the show is hot reloaded and the UI may do all of these. Art-Net is off in both unless `--artnet` is given. A browser build of the UI comes embedded in the binary; build it with `pnpm --dir ui install && pnpm --dir ui build` before `cargo build`, or the binary serves a page saying so.

`--artnet` mirrors every frame as one ArtDmx packet (Art-Net 4, protocol version 14) to an IPv4 node, unicast or broadcast; the port defaults to 6454 and `--artnet` alone sends to 127.0.0.1. `--artnet-universe` is the 15-bit port-address (Net, Sub-Net, Universe), from 0, default 0. Art-Net is off unless asked. A send error is reported once on stderr and never stops the render loop. The same node also gets every Art-Net universe the rig's pixel grids use, one packet each per frame, blackout included; each universe numbers its packets on its own. If a grid uses universe 0 and `--artnet-universe` isn't given, the grid keeps universe 0 and the Enttec frame isn't mirrored (the engine says so on stderr): give `--artnet-universe` to mirror it elsewhere. An `--artnet-universe` that is also a grid's universe is refused. Without `--artnet`, a rig with Art-Net grids starts with a warning that they get no output. See [blenderdmx.md](blenderdmx.md) for a 3D preview.

## State, event log and service

`run` keeps its files in its state folder, `~/Library/Application Support/Blimey/` unless `--state-dir` says otherwise. Nothing goes in the show folder. `state.json` and `events.log` are diagnostics, written on a background thread: a state folder the engine can't create or write is reported on stderr, and the show runs without them.

- `last-good-show.json`: the show folder and the YAML files of the last show that loaded, by path, as read just before it loaded, rewritten atomically each time the show loads, at start and on every hot reload. If the show fails to load at start, `run` unpacks them to `last-good-show/` and plays that, saying so on stderr; a copy of another show folder is never played, and the show's own error stands. Saves still go to the show folder, and in edit mode the show folder reloads over the copy once it loads again.

- `state.json`, `{"song": "demo-song", "master": 0.5}`: the selected song (or `null`) and the master, 0–1, rewritten atomically whenever either changes. The background thread saves a change a few milliseconds after the frame that shows it; a crash inside that window resumes the state from before the change. On start the engine resumes them, as if it had received the song's Program Change and the master's CC 7, then waits for the next cue. A saved song the show no longer has is dropped; an unreadable file gives a fresh start. Both are reported on stderr.
- `events.log`: one JSON object per line, for post-gig diagnosis. Each line is written unbuffered as soon as the writer gets it, so a crash loses at most the lines still queued. Past 10 MB it rotates to `events.log.1`, up to `events.log.4`; if rotating fails, logging carries on in `events.log`.

```json
{"master":0.5,"mode":"show","show":"tests/fixtures/shows/demo","song":"demo-song","time_ms":1794470400000,"type":"start"}
{"at_ms":1250,"message":[145,36,127],"time_ms":1794470401250,"type":"midi"}
{"action":"start","at_ms":1250,"name":"verse-blue","note":36,"song":"demo-song","time_ms":1794470401250,"type":"cue"}
{"action":"release","at_ms":4000,"name":"verse-blue","note":36,"song":"demo-song","time_ms":1794470404000,"type":"cue"}
```

- `start`: the engine started, with the song and master it resumed.
- `midi`: every MIDI message taken in, as in the stream's `midi` message, clock ticks and active sensing left out. The resumed song and master follow `start` as `midi` lines at `at_ms` 0.
- `cue`: a cue started (note-on), released (note-off, or a retrigger fading out the previous instance) or, with `"action":"fail"`, dropped for failing (a panic, a bug) until fired again. Panic and song select drop cues without a `release` line.
- `skipped`: the render loop fell behind and skipped `frames` frames; output picked up at the frame due at `at_ms`.
- `at_ms` is engine time, as in the stream; `time_ms` is wall-clock time, in milliseconds since 1970. `jq '.time_ms / 1000 | todate'` reads it.

`blimey service install <run arguments>` makes the checks `run` makes at start (an output is given, the show loads), so launchd never keeps restarting an engine that exits at once. It then writes the launch agent `~/Library/LaunchAgents/app.blimey.engine.plist` and starts it. launchd starts the engine at login and restarts it within a second whenever it exits. Relative paths are taken from the folder the command ran in, and the engine's stdout and stderr go to `engine.log` in the state folder. Installing again replaces the agent; `--dry-run` prints the plist instead. `blimey service uninstall` stops and removes it, even if it wasn't running.

`scripts/fault-injection.sh [kill|midi|serial|state]` (macOS) checks recovery against the real binary, an emulated Enttec box on a pty and a virtual CoreMIDI port: killing the engine mid-song, dropping the MIDI port, unplugging the box, starting with an unusable state folder.

## Recording and soak

`blimey record <out.mid>` writes everything that arrives on the MIDI port, chosen as `run` chooses it, to a format 0 Standard MIDI File: channel messages as MIDI events, clock, Start, Stop and anything else as SMF escapes. Time 0 is the first message; times are kept to 20 µs (the file runs at 120 BPM, 25,000 ticks per beat), rounded up, so every event replays in the frame it arrived in. It stops on Ctrl-C, SIGTERM or SIGHUP (the terminal closing), or with `--until-stop` at the first Stop after a Start or Continue. The files are saved every 5 s and at the end (the MIDI file rewritten atomically, the dump appended to), so a crash or `kill -9` loses at most the last 5 s. `--artnet <dump> --show <show>` also records the ArtDmx packets received on UDP 6454 to a [frame dump](frame-dump.md) on the same timeline and 25 ms frame grid as `blimey replay`'s, laid out like it, so the two compare line by line:

- the Enttec stand-in, `--artnet-universe` (15-bit port-address), is written as plain channels. It is chosen as `run --artnet` chooses its mirror: universe 0 by default, unless one of the show's pixel grids uses 0, in which case the grid keeps it. Naming a grid's universe is refused.
- the show's pixel-grid universes are written as `<universe>:<channel>=<value>`;
- any other universe is ignored, such as a desk elsewhere on the network.

Frame N holds the latest values received by N × 25 ms, a short packet leaving the rest of its universe at 0. A frame with no packet gets no line. Packets before the first MIDI message only set the values the first frame starts from. If the capture fails mid-take, the MIDI keeps recording, and `record` reports the dump as incomplete and exits 1.

`blimey soak <setlist>` plays a setlist's songs back to back, 2 s apart and each starting on a frame, looped for `--hours` (default 4), through the same render loop as `run`. A setlist is a YAML file, paths relative to it:

```yaml
show: .                                  # the show folder
songs:                                   # played in this order, then again
  - recordings/demo-song.mid
```

In real time it sends to the outputs given as for `run` (`--dmx-port`, `--listen`, `--artnet`, `--edit`), or with `--emulated-enttec` through the real driver to an emulated box on a pseudo terminal. `--accelerated` runs on a fake clock with no outputs, as fast as the machine renders. It prints a report, writes it to `--report <file>` (by default a new `soak-<setlist>-<mode>-<unix time>-<process id>.txt` in the current folder) and exits 1 if a check fails:

| Check | Measured | Passes when |
| --- | --- | --- |
| frames | Frames sent a frame period (25 ms) or more after their deadline, frames skipped after a stall (the render loop skips any more than 4 periods late), plus frames the emulated box never received; fails too if the box stopped reading, naming the error | None |
| jitter | How late each frame went out after its deadline: p50, p99 and max, in 0.1 ms steps | p99 under 5 ms |
| drift | Each song's frames, from the third pass on, against the same song in the second pass (the first starts from a fresh engine); the last song counts if it played to its end | None differ |
| memory | Process RSS, read from the kernel about 20 times over the run | Peak after the first pass is at most 1 MiB above RSS at its end |

It needs at least three passes. Memory is flat when it stays within 1 MiB: the engine keeps nothing from one frame to the next, so what remains is allocator noise, while a leak of 8 bytes a frame passes 1 MiB within an hour and of 1 byte a frame within 8 hours.

## HTTP API

Errors come back as `{ "error": "<what went wrong>" }` with `404` (unknown file, song, cue or fixture), `403` (show mode), `409` (conflict: a note already taken, a song name the show already has, a cue changed since it was loaded, or a file changed since the save to undo or redo), `422` (content refused, or a request body not in the shape documented) or `500`.

Requests that load the show to answer, `GET /api/show`, the thumbnails and the pixel preview, run two at a time; the rest wait their turn, so a flood of them can't take the CPU from the render loop.

**One write path.** Every endpoint that changes a show file, `PUT /api/show/<file>` and the song endpoints below alike, goes through the engine's one writer: it validates, writes atomically and keeps canonical formatting, as described for `PUT /api/show/<file>`. Each save can be undone and redone.

### Show summary: `GET /api/show`

What the builder needs to offer choices: palette colours as `#rrggbb`, and the songs with the Program Change that selects each, which the builder shows as the song's number.

```json
{ "palettes": { "deep-blue": "#0028ff" }, "songs": [{ "name": "demo-song", "program": 3 }] }
```

### Show files: `GET` and `PUT /api/show/<file>`

Reads and replaces a whole show file. `<file>` is a path in the show folder: `rig.yaml` or `songs/<name>.yaml`, a name of letters, digits, `-` and `_`. The body of both is the file's content as JSON, in the shape of its schema, keys in file order (a look's entries apply in order).

`PUT` replaces the file, or creates a song (`204` on success):

1. The content is checked against the file's JSON Schema in `schema/`, then by the engine's own loader. A song is judged by loading the whole show as it would be with the new content in place, the same checks as at start-up, so a song broken on disk can be fixed through this endpoint. Either refusal is a `422` naming the problem and the file at fault, such as `rig.yaml: /fixtures/0/canvas/x: 1.5 is greater than the maximum of 1` or `songs/other.yaml: program 3 already selects song "demo-song"`, and leaves the file untouched. A new song whose name differs from an existing one only by case is refused with `409`: macOS would write it over the other.
2. The file is written in canonical form. Its leading comment block is kept; other comments are not. Writing back what `GET` returned changes nothing for a file already in canonical form, so an edit changes only the lines it touches.
   - `rig.yaml`: block-style YAML with fields in a fixed order.
   - Songs: block-style YAML, two-space indents, keys in the order of the schema's `properties` (each cue's by its type's definition, a pixel cue's `params`, `layer_2` and `layers`, with each layer's `area`, too) and cues by note number. A mapping or list of plain values sits on one line: `all-wash: { color: deep-blue, dimmer: 60% }`, `- [0.5, 1]`.3. The write is atomic: a crash leaves the old file or the new one, never half of one.
4. Hot reload picks the file up on the next frame: a readdressed fixture moves on every output, and stream clients get the new `rig` message. A `204` means the file is written, not yet rendered.

To move a fixture, `GET` the file, change the fixture's `canvas`, `PUT` it back.

### Scenes and songs in the builder

The builder calls each cue a scene and goes by numbers, never notes or programs. A scene is stored as a cue entry under the song's `cues:`, and its number is that entry's key: scene 36 is `cues: { 36: … }`, saved and deleted through `/api/songs/<song>/cues/36`. A song's number is its `program`. A swell or pattern cue is a swell or pattern scene; a `type: scene` cue the builder can't open on the stage stays a "fixed look". Valid numbers are what the endpoints take.

Until a later chunk designs control from the DAW, the mapping is the engine's: scene N fires on channel-2 note N and song N is selected by channel-2 Program Change N. The builder's auditions send exactly that through `POST /api/midi`.

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
  - a note that another cue already uses, with `409`: `songs/demo-song.yaml: note 36 already fires cue "verse-blue"; pick a free note, or edit that cue`. The engine never merges or overwrites a cue it wasn't asked to replace;
  - a replaced cue that changed since it was loaded, with `409`, or is no longer there, with `404`, so a stale editor never overwrites someone else's change.

`DELETE` removes the cue on `<note>` (`204`; `404` if there is none).

### Metronome tempo: `PUT /api/songs/<song>/bpm`

Sets the song's `bpm`, the tempo for beats and bars when no MIDI clock comes in, leaving the rest of the file as it is (`204`). The body is `{ "bpm": 92 }`. It goes under the same lock as the cue endpoints and takes effect from the next frame.

### Arrangement: `PUT /api/songs/<song>/arrangement`

Sets the song's [`sections` and `arrangement`](#sections-and-arrangement), leaving the rest of the file as it is (`204`). The body holds both, as in the song file:

```json
{ "sections": { "verse": { "bars": 16, "blocks": [{ "scene": 36, "bar": 1, "length": "16 bars" }] } },
  "arrangement": ["verse", "verse"] }
```

An empty `sections` or `arrangement` is left out of the file. The song is checked and written like `PUT /api/show/<file>`, under the same lock as the cue endpoints, and recorded for undo. Refused with `422`, leaving the file untouched: anything the loader refuses, such as an unknown section in `arrangement` or overlapping blocks of one scene, with the message naming the line it would be on. Hot reload picks the change up from the next frame and writes the song's MIDI file to `<show>/export/`.

### Arrangement events: `GET /api/songs/<song>/arrangement/events`

What the song's arrangement plays, the same list the MIDI export writes, so the browser needn't redo the musical-time arithmetic. Works in show mode too; `404` for an unknown song.

```json
{ "bpm": 120,
  "events": [
    { "beats": 0, "seconds": 0, "type": "time_signature", "numerator": 4, "denominator": 4 },
    { "beats": 0, "seconds": 0, "type": "marker", "section": "verse" },
    { "beats": 0, "seconds": 0, "type": "program_change", "program": 1 },
    { "beats": 0, "seconds": 0, "type": "note_on", "scene": 36, "velocity": 127 },
    { "beats": 8, "seconds": 4, "type": "note_off", "scene": 36 }
  ] }
```

- `beats` count quarter notes from the top of the song, on a grid of 1/960 beat; `seconds` are `beats` at `bpm`.
- In time order, and at one beat in the order of the [export](#sections-and-arrangement). `type` is `time_signature`, `marker`, `program_change`, `note_off` or `note_on`; a held scene across sections is one `note_on` and one `note_off`.
- Empty for a song without an arrangement.

### Arrangement MIDI file: `GET /api/songs/<song>/arrangement.mid`

The song's [exported MIDI file](#sections-and-arrangement), as `audio/midi`, downloaded as `<song>.mid`. Works in show mode too, writing nothing. `404` for an unknown song or one without an arrangement.

### Song audio: `GET /api/songs/<song>/audio`

The song's [audio](#audio) file, with its type from the extension (`audio/wav`, `audio/mpeg`, …) and `Accept-Ranges: bytes`. A `Range` header of one range (`bytes=0-1023`, `bytes=1024-`, `bytes=-1024`) gets `206` and that part, with `Content-Range`; a range past the end gets `416`; a list of ranges gets the whole file. `404` for an unknown song, a song without `audio`, or a file that isn't there, which the timeline plays as silence. Works in show mode too.

### Audio offset: `PUT /api/songs/<song>/audio/offset`

Sets where bar 1 falls in the song's audio, leaving the rest of the file as it is (`204`). The body is `{ "offset": "1.25s" }`. It goes under the same lock as the cue endpoints, is checked and written like `PUT /api/show/<file>`, and is recorded for undo. Refused with `422`, leaving the file untouched: a song without `audio`, or an offset the loader refuses.

### Song audio upload: `PUT /api/songs/<song>/audio`

Stores the request body, the audio file itself, as the song's audio. Its type is read from its content, whatever the request's headers say: WAV (`RIFF`/`WAVE`), MP3 (an ID3 tag or an MPEG frame) or FLAC (`fLaC`). The body is streamed to a temporary file in `audio/`, then renamed to `audio/<song>.<ext>`, so a reader never sees half a file. The song file's `audio.file` then names it, keeping `offset`, through the show-file writer and recorded for undo; undo puts back the song file, not a replaced audio file. The song's previous upload, `audio/<song>.<other ext>`, is removed; a file named by hand is left alone.

Answers `200` with the file and the [markers](#markers) found in it, for the timeline to offer, without changing the song's `markers`:

```json
{ "file": "audio/tell-me.wav", "markers": [{ "name": "Verse", "at": "12.5s" }, { "name": "Marker 2", "at": "40s" }] }
```

Markers come from a WAV's `cue ` points, at their sample position over the file's sample rate, named by their `labl` (or a region's `ltxt` text); an unnamed one is `Marker N`, N counting in time order. A region is a marker at its start. MP3 and FLAC give none.

Refused, writing nothing: show mode (`403`), an unknown song, its name spelled in another case included (`404`), a body over 1 GB (`413`, from `Content-Length` or as it arrives), a file that is not WAV, MP3 or FLAC (`415`), a damaged or truncated WAV (`422`, naming what is wrong).

### Audio markers: `GET /api/songs/<song>/audio/markers`

Reads the markers of the song's audio file as it is now, as `{ "markers": [...] }` in the upload's form, for "Import markers from audio". Works in show mode too. `404` without audio or file, `422` for a damaged WAV.

### Markers: `PUT /api/songs/<song>/markers`

Replaces the song's `markers`, leaving the rest of the file as it is (`204`). The body is `{ "markers": [{ "name": "Verse", "at": "12.5s" }] }`; an empty list removes the key. Checked and written like `PUT /api/show/<file>`, and recorded for undo. Refused with `422`, leaving the file untouched: a negative or non-time `at`, an empty `name`.

### New song: `POST /api/songs`

Creates `songs/<name>.yaml` (`204`). The body is the name and the start of the song file:

```json
{ "name": "encore", "program": 7, "bpm": 100, "time_signature": "6/8" }
```

`bpm` and `time_signature` are optional. The song is checked and written like `PUT /api/show/<file>`, and hot reload makes it selectable from the next frame. Refused, with nothing written: a name the show already has, in any case (`409`); a name that isn't letters, digits, `-` and `_`, or a `program` another song uses (`422`). Unlike `PUT /api/show/<file>`, it never replaces a song.

### Undo and redo: `POST /api/undo`, `POST /api/redo`, `GET /api/history`

The engine keeps the last 50 saves made through the API, across all show files: `PUT /api/show/<file>` and every song endpoint above. The history lives in memory only: a restart clears it.

- `POST /api/undo` undoes the latest save still in the history: the file goes back to what it held before, byte for byte, comments and formatting included, after the same checks as a write. A song the save created is removed.
- `POST /api/redo` makes the latest undone save again, byte for byte. A new save clears what could be redone.
- Both return the file and the steps left: `{ "file": "songs/demo-song.yaml", "undo": 2, "redo": 1 }`. With nothing to undo or redo, they return `404`.
- `GET /api/history` returns the steps alone, `{ "undo": 3, "redo": 0 }`. It works in show mode, where the history is always empty.

If the file has changed since the save or undo (by hand, say), the step is refused with `409` rather than lose that change; it stays in the history, to try again once the file is put back. A step whose earlier content no longer passes the checks, such as an undo back to a song that was broken before a fix, never could be made: it is refused with `422` and dropped, so it doesn't block the steps before it.

### Pixel scene preview: `PUT /api/preview/pixel`

Shows a pixel cue on every output while the builder edits it (`204`). The body is the song and the cue as in its file:

```json
{ "song": "demo-song", "cue": { "name": "bridge-chase", "type": "pixel", "effect": "chase", "params": { "color": "amber", "width": 0.25 } } }
```

The cue is checked like a saved one (`422`, for an unknown colour say); an unknown song is a `404`. It draws above every layer, on the frame as output after the master dimmer, on every fixture and grid, Art-Net ones included, at full intensity, and animates from the first frame drawn at the song's `bpm`, whatever the MIDI clock does. Layer rhythms play too: a swell from that first frame, a pattern's loop from the next bar line counted from engine start, in the song's `time_signature`. A `PUT` of another pixel cue keeps the animation going where it is, so moving a slider doesn't restart it, unless a layer's `swell` or `pattern` changed: then it starts again, so the edited rhythm plays from its start. Nothing is saved.

It lasts 5 s of engine time from the first frame drawn after the latest `PUT`, so the builder sends it again every few seconds while it shows; a builder that closed or crashed leaves nothing on stage. `DELETE /api/preview`, or a channel-2 note-on posted to `POST /api/midi` (an audition, which should show the cue), clears it at once.

### Pixel thumbnail: `POST /api/pixel-thumbnail`

Renders a pixel cue, as in its song file, on a sample grid, for the builder's shape and effect pickers. The engine renders it as a song would, so the pickers never re-implement an effect. Colours come from the show's palettes; nothing is saved, and it works in show mode.

```json
{ "columns": 12, "rows": 12, "frames": [["#000000", "#ff0000", "… 144 colours"], "… 8 frames"] }
```

- The grid is RGB and fills the canvas.
- `?song=demo-song` gives the song whose `time_signature` the cue's bars and its layers' patterns are read in, as when it is saved there; without it, 4/4. An unknown song is a `404`.
- `frames` are 8 moments evenly over the longest of the moving layers' `speed` and the layers' `swell` durations and `pattern` lengths (1 bar when there are none), from the note, with beats at 120 BPM. Layer rhythms play from the note too, a pattern's loop starting at once.
- Either form of pixel cue is accepted. Areas sit on the sample grid as on the stage; `mute` names are checked against the show's rig but mute nothing, since the sample grid holds none of its fixtures.
- Each frame lists the grid's colours row by row from the top left, as the audience sees it, at full intensity.
- A cue that isn't a pixel cue, or that a song would refuse, is a `422`.

### Cue thumbnails: `GET /api/songs/<song>/thumbnails`

What each saved cue of the song looks like, for the pads of the builder's scenes home, by note (the scene's number). The engine loads the show once for the request and renders every cue from the song file, aside from the output: nothing is saved or shown, and it works in show mode. An unknown song is a `404`; a cue that can't be rendered, such as a pixel cue whose `speed` is too long to show a cycle of, is a `422` naming it.

```json
{ "36": { "frames": ["…"] }, "39": { "levels": ["…"] } }
```

A scene or pixel cue's thumbnail is `frames`, each the colour every pixel of the rig shows, at the rig's own pixel positions, in the shape of the stream's `frame.pixels`:

```json
{ "frames": [{ "wash-1": ["#0028ff", "#0028ff", "#0028ff", "#0028ff"], "back-grid": ["#000000", "…"] }] }
```

- A scene is one frame: the cue drawn as playback draws it, over a dark stage with nothing below. A fixture given only a dimmer stays dark.
- A pixel cue is 8 frames, timed as in the [pixel thumbnail](#pixel-thumbnail-post-apipixel-thumbnail), with beats at 120 BPM.
- Fade-in, release and velocity are left out.

A swell or pattern cue's thumbnail is its envelope, `levels`: 129 levels, 0–1, evenly spaced from start to end.

```json
{ "levels": [0, 0.0001, "…", 1] }
```

- A swell's is its `curve` over its `duration`.
- A pattern's is one pass of its loop, from beat 1 to the end of its `length`, as it plays once it has looped: a hit running over the loop's end shows at its start.

### MIDI: `POST /api/midi`

Plays one MIDI message as if it came in from the MIDI port, to audition cues without the DAW (`204`):

```json
{ "message": [145, 36, 127] }
```

`message` is the raw bytes, status byte first, in the same shape as the stream's `midi` message. It acts on the next frame, stamped with that frame's time, and is streamed as a `midi` message. Anything but one well-formed message is a `422`. A channel-2 note-on also clears the pixel scene preview. To fire a cue: Program Change for its song on channel 2 (`[193, <program>]`) unless the song is already selected, then note-on (`[145, <note>, 127]`); release it with note-off (`[129, <note>, 0]`).

### Curve levels: `POST /api/envelope`

The levels a curve plays, for the builder to draw what the engine will play. The body is a curve as in a song file (a swell's `curve`, a hit's `shape`) and how many levels to return, from 2 to 1001:

```json
{ "curve": { "points": [[0, 0], [0.5, 1], [1, 0.3]] }, "samples": 5 }
```

It returns `{ "levels": [0, 0.5, 1, 0.65, 0.3] }`: the level, 0–1, at `samples` evenly spaced points from the start of the curve to its end. A curve the engine would refuse at load is a `422` with the same message. It changes nothing, so it works in show mode too.

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
      "pixels": [{ "x": 0.115625, "y": 0.2 }, "…"],
      "colour": true
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
- `colour`: whether the fixture has colour emitters. A pixel cue lights only those, so the builder keeps a `type: scene` cue that targets a fixture without them a fixed look.
- `pixels` are canvas positions, in pixel order; for a grid, in wiring order.
- `grids`: the rig's [pixel grids](#rigyaml), apart from `fixtures` since they have no profile, mode or single address.

### `frame`: the output, after each rendered frame

```json
{
  "type": "frame",
  "at_ms": 1250,
  "dmx": [0, 10, 255, "… 512 values"],
  "pixels": { "wash-1": ["#000aff", "#000aff", "#000aff", "#000aff"] },
  "song": "demo-song",
  "cues": [{ "note": 36, "name": "verse-blue", "intensity": 1.0, "released": false, "beats": 2.5 }],
  "master": 1.0,
  "live_parameters": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}
```

- `at_ms`: engine time when the frame was output, in milliseconds.
- `dmx`: the universe as output, 512 values of 0–255. Index 0 is DMX channel 1.
- `pixels`: per fixture and per grid, the colour each pixel shows, in the same order as the `rig` message. It is computed from the output (white added to R, G and B, then the dimmer channel applied), so the preview always matches it: from `dmx`, or for a grid on Art-Net from that universe, which the stream doesn't carry.
- `song`: the selected song's name, or `null`.
- `cues`: the active cues, lowest layer first. `intensity` is the note-on velocity as 0–1; `released` is true while the cue fades out after its note-off. `beats` is the musical time since the cue started, at the frame's time and tempo; for a pattern it counts from the bar line its loop starts on, so it is negative while the loop waits for it. A pixel cue with a `pattern` on a layer also has `loop_beats`, counted the same way from the bar line its layers' loops start on.
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
