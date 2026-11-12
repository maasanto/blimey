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
- the profile or mode doesn't exist.

**Canvas.** The stage plan is a normalised 2D canvas, 0–1 on both axes: x from left to right as seen from the audience, y from upstage (0, top of the plan) to downstage (1). A fixture's pixels sit on one row at height `y`, spread evenly across `width` starting at `x`, in pixel order: pixel *i* of *n* is at `x + width × (i + 0.5) / n`. A single-pixel fixture can omit `width`.

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

**Musical time.** Beats come from counting clock ticks, smoothed through jitter. Without a clock, or 250 ms after it goes silent, they run at the song's `bpm`. When ticks come back they catch up with the clock over about a beat, never jumping or running backwards, so fades stay smooth. After Reaper stops, running swells finish at the song's `bpm`. The engine logs each change of tempo source on stderr, e.g. `blimey: tell-me: no MIDI clock: using the song's 92 BPM` or `blimey: tell-me: following the MIDI clock`. In a MIDI file for `blimey replay`, clock and transport messages are stored as SMF escape events (`F7 01 F8`). A scene crossfades each channel from the layers below it to its own value, linearly. Cues stack in the order they were fired, later ones on top.

Layers, lowest first: legacy notes, the safe look after panic, cues, then the master dimmer. The master scales dimmer channels, the colour channels of fixtures without a dimmer channel, and channels no fixture uses (legacy-only). Other channels, such as a shutter, keep their value.

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
blimey fire <show> --set <fixture-or-group> key=value… [--set …]   # print the look as a one-frame dump
blimey fire <show> --set … --dmx-port /dev/cu.usbserial-…           # hold it on the Enttec box
blimey fire <show> --set … --listen 127.0.0.1:7340                  # hold it and stream it over WebSocket
blimey fire <show> --set … --artnet [<ip>[:port]] [--artnet-universe <n>]   # hold it and mirror it over Art-Net
blimey run <show> --dmx-port … --artnet [<ip>[:port]] [--artnet-universe <n>]
blimey fixture import <ofl.json> [--out <show>/fixtures/<name>.yaml]
```

`validate` runs the same checks as loading the show: it exits 0 and prints `<show>: ok, N fixtures, N songs`, or exits 1 with the error.

`run` is in edit mode: it reloads the show whenever a file in its folder changes, keeping the selected song and active cues, which take their new definitions. A file that fails to load is reported on stderr and the previous show keeps running.

`fire` goes through the same render loop as `blimey run`, with no MIDI input. Without `--dmx-port`, `--listen` or `--artnet` it prints a [frame dump](frame-dump.md) with a single frame at 0 ms. With any of them, it renders at 40 Hz until killed. `--listen` prints `streaming on ws://<address>/ws` once bound; port 0 picks a free port.

`--artnet` mirrors every frame as one ArtDmx packet (Art-Net 4, protocol version 14) to an IPv4 node, unicast or broadcast; the port defaults to 6454 and `--artnet` alone sends to 127.0.0.1. `--artnet-universe` is the 15-bit port-address (Net, Sub-Net, Universe), from 0, default 0. Art-Net is off unless asked. A send error is reported once on stderr and never stops the render loop. See [blenderdmx.md](blenderdmx.md) for a 3D preview.

## WebSocket stream: `GET /ws`

Server to client only, one JSON object per text message. Every message has a `type`. Clients ignore types and fields they don't know, so later chunks can add them without breaking the UI.

### `rig`: sent first, on connect

```json
{
  "type": "rig",
  "protocol": 1,
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
  "groups": { "all-wash": ["wash-1", "wash-2", "wash-3", "wash-4"] }
}
```

- `protocol` is bumped only by a breaking change to this stream.
- `channels` lists channel names in DMX order, starting at `address`.
- `pixels` are canvas positions, in pixel order.

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
- `pixels`: per fixture, the colour each pixel shows, in the same order as the `rig` message. It is computed from `dmx` (white added to R, G and B, then the dimmer channel applied), so the preview always matches the output.
- `song`: the selected song's name, or `null`.
- `cues`: the active cues, lowest layer first. `intensity` is the note-on velocity as 0–1; `released` is true while the cue fades out after its note-off.
- `master`: the master dimmer, 0–1.
- `live_parameters`: CC 20–27 on channel 2, as 0–1.

The engine renders at 40 Hz. A slow client skips frames and always gets the latest one.
