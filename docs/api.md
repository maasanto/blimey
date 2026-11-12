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
blimey fire <show> --set <fixture-or-group> key=value… [--set …]   # print the look as a one-frame dump
blimey fire <show> --set … --dmx-port /dev/cu.usbserial-…           # hold it on the Enttec box
blimey fire <show> --set … --listen 127.0.0.1:7340                  # hold it and stream it over WebSocket
blimey fire <show> --set … --artnet [<ip>[:port]] [--artnet-universe <n>]   # hold it and mirror it over Art-Net
blimey run <show> --dmx-port … --artnet [<ip>[:port]] [--artnet-universe <n>]
blimey fixture import <ofl.json> [--out <show>/fixtures/<name>.yaml]
```

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
  "pixels": { "wash-1": ["#000aff", "#000aff", "#000aff", "#000aff"] }
}
```

- `at_ms`: engine time when the frame was output, in milliseconds.
- `dmx`: the universe as output, 512 values of 0–255. Index 0 is DMX channel 1.
- `pixels`: per fixture, the colour each pixel shows, in the same order as the `rig` message. It is computed from `dmx` (white added to R, G and B, then the dimmer channel applied), so the preview always matches the output.

The engine renders at 40 Hz. A slow client skips frames and always gets the latest one.
