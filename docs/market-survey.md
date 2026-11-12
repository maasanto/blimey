# Fossile — DMX lighting control: market survey & build options

Nov 12, 2026 · @Antoine

**Bottom line:** no free or cheap tool covers all six requirements (pixel mapping, 3D view, open fixture library, simple UI, scene templates, agent-friendly), and none is built to be driven by Reaper. But the hard pieces (fixture formats, 3D viewers, DMX output) now exist as open source, so building your own is realistic if the scope stays tight.

## How past setups failed

- **DoReMIDI MTD-1024 (MIDI→DMX hardware):** crashed mid-show; no pixel mapping.
- **Show Buddy Active:** plugin-in-Reaper sync was the appeal, but it never ran the lights.
- **ENTTEC EMU:** went furthest, but:
  - Oscillators run on a free clock and don't restart on scene recall, so timed swells drift.
  - Fixture attribute overrides make faders snap back unexpectedly.
  - Plugin ↔ app "handshake" — why scene switching fails on the M1 but works on the M2.

All three put a black box between Reaper and the DMX interface. That is the thing to avoid.

## What's out there (Nov 2026)

### Lighting engines

- **QLC+** — free, open source (Apache 2.0), actively developed. 4.13 is stable; version 5 (with 3D view) is still beta — last announced release is Beta 3 (end 2023), recommended for controlled environments only. Basic pixel effects (RGB matrix), dated UI.
- **ChamSys MagicQ** — best free option on paper: free 3D visualizer and 2D/3D pixel mapper. Deal-breaker: on PC/Mac, MIDI only works when fully unlocked by ChamSys hardware, so Reaper can't drive it in demo mode.
- **BlinderKitten** — French dev, fixture-oriented, no paid unlocks, MVR import. No built-in 3D (users borrow grandMA3 onPC's free visualizer). Pixel-mapping maturity unverified.
- **Chataigne** — open source (GPL), Mac, drives an Enttec DMX Pro directly, real-time + timeline: timed swells are easy. No fixture model, no pixel mapping, no 3D — a glue tool, not a desk.
- **Captivate** — open source, synth-like; hundreds of channels reduced to a few parameters. Great for improvised music-reactive shows, poor for programmed shows (per-song setups + MIDI mappings).
- **Lightkey 5** — polished, Mac-native, closed source, yearly licence ($119 for 512 channels). 2D preview only.

### 3D viewers (open source)

- **BlenderDMX** — Blender add-on; imports GDTF, imports/exports MVR, receives Art-Net/sACN; pixel control since 1.0.
- **Beatline Beam** — newer; real-time 3D sandbox driven by Art-Net (Three.js + Electron).
- **ASLS Studio** — web-based control + 3D view, still maintained.

### Open fixture formats ("bank of lights")

- **Open Fixture Library** — JSON definitions, exportable to several formats including QLC+.
- **GDTF + MVR** — industry standards (fixture description + whole-rig scene). Official C++ library; Python libs used by BlenderDMX.

### Agent-friendly control

MCP servers for lighting exist but are early: LacyLights (theatre, depends on OpenAI API), an MA3 MCP explicitly not for production. Nothing usable for Fossile.

## Fit against requirements

|  | Pixel mapping | 3D view | Open fixtures | Simple UI | Driven by Reaper | Agent-friendly |
| --- | --- | --- | --- | --- | --- | --- |
| QLC+ 4.13 | basic | ✗ (v5 beta only) | ✓ | \~ | ✓ MIDI | \~ (XML) |
| MagicQ demo | ✓ | ✓ | \~ | ✗ | ✗ without hardware | ✗ |
| BlinderKitten | ? | external | MVR | \~ | ✓ | \~ |
| Chataigne | ✗ | ✗ | ✗ | \~ | ✓ | \~ |
| Captivate | \~ | ✗ | ✗ | ✓ | MIDI only | ✗ |
| Lightkey | ✓ | 2D | own library | ✓ | ✓ MIDI | ✗ |

## If building: don't start with the plugin

The plugin is the riskiest part, and where EMU and Show Buddy both failed. A plugin owning the USB device and handshaking with a separate app is the fragile pattern.

1. **Reaper stays the timeline.** Keep the MIDI note language and the JSFX script; Reaper sends notes, markers or OSC.
2. **A separate headless engine** owns the Enttec box (simple, documented serial protocol). It handles fixtures (OFL/GDTF), frame-accurate timed swells, and pixel effects over fixture segments.
3. **Shows and scene templates as plain text** (YAML/JSON): agent-writable, git-diffable. Thin MCP server on top.
4. **3D: reuse BlenderDMX or Beam** via an Art-Net copy of the output. Building a viewer is weeks of work for no added value.
5. **Plugin only later, if needed** — and only to relay MIDI/OSC, never to own the hardware.

**Effort:** a first version for the four WildWash fixtures (MIDI in, swells, simple pixel chases, Enttec out) is a few weekends. The "simple intuitive interface with lots of templates" is where the real time goes — that part is the product.

## Open question

The pain is mostly EMU's software, not its approach. If the M1 scene-switching bug were fixed, would EMU + Reaper automation for swells cover the next 12 months of gigs? If yes, building is a long-term project, not an urgent one.

## Sources

- [QLC+ news](https://www.qlcplus.org/news) · [QLC+ 5 Beta 3](https://mail.qlcplus.org/news/qlc-5-0-0-beta-3-release) · [GitHub dmx topic](https://github.com/topics/dmx)
- [MagicQ features](https://chamsysusa.helpscoutdocs.com/article/354-magicq-software-features) · [MagicQ MIDI docs](https://docs.chamsys.co.uk/magicq/midi/MIDI.html)
- [BlinderKitten](http://blinderkitten.lighting) · [Chataigne docs](https://bkuperberg.gitbook.io/chataigne-docs/getting-started-1/getting-started) · [Captivate](https://github.com/spensbot/captivate)
- [Lightkey pricing](https://sites.fastspring.com/lightkey/product/all)
- [BlenderDMX](https://extensions.blender.org/add-ons/open-stage-blender-dmx) · [Beatline Beam](https://github.com/dinther/Beam) · [ASLS Studio](https://github.com/ASLS-org/studio)
- [Open Fixture Library](https://open-fixture-library.org) · [GDTF & MVR](https://gdtf-share.com/help/)
- [LacyLights MCP](https://glama.ai/mcp/servers/bbernstein/lacylights-mcp) · [MA3-MCP](https://lobehub.com/mcp/pahegi-ma3-mcp)
