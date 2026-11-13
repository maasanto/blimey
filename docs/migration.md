# Migrating the set from EMU

Each song moves over the same way: record what Reaper sends, port the cues, check against EMU and the preview, then lock it in with a replay reference. Everything here runs on the laptop, with no lights; the rig only comes out for the go/no-go rehearsal (plan §11, §15). Commands are in [api.md](api.md#recording-and-soak).

## Once, before the first song

- Enable the IAC Driver in Audio MIDI Setup, and point Reaper's MIDI output at it. `blimey devices` should list it.
- For the parity capture, set EMU's output to Art-Net, sent to `127.0.0.1` (or the laptop's address) on universe 0. EMU and Blimey must never both open the Enttec box; while capturing, the box isn't needed at all.
- Quit BlenderDMX and anything else listening on UDP 6454 while capturing.

## Per song

Copy this list into the song's PR or an issue, one per song.

1. **Record the MIDI and EMU's output.** In Reaper, go to the start of the song with EMU running, then:

   ```sh
   blimey record shows/fossile/recordings/<song>.mid --until-stop --artnet tests/fixtures/dumps/emu-fossile-<song>.dump
   ```

   Press play and let the song run to the end; recording stops when Reaper stops (or Ctrl-C). Time 0 is the first MIDI message, in both files. Check the summary line: a few thousand events for a song with MIDI clock.

2. **Port the cues.** Legacy-only songs need nothing: channel-1 notes go through `legacy-map.yaml` unchanged. For named cues, write `shows/fossile/songs/<song>.yaml` (by hand or in the builder) and make sure the Reaper session sends the song's Program Change on channel 2 at the start. Re-record if the session changed.

3. **Compare with EMU (parity, plan §15).** Render Blimey's output for the same MIDI:

   ```sh
   blimey replay shows/fossile --midi shows/fossile/recordings/<song>.mid --out "$TMPDIR/<song>.dump"
   ```

   and compare it with `tests/fixtures/dumps/emu-fossile-<song>.dump`. There is no automated comparison yet: EMU sends at its own rate, so its lines don't fall on Blimey's 25 ms grid. Compare a few moments by time (`grep '^12[0-9][0-9][0-9] '` around 12 s in both), channel by channel. Legacy channels should match exactly; differences on ported cues are expected and reviewed by eye on the preview.

4. **Check on the preview.** Run the show with the web UI and play the song from Reaper:

   ```sh
   blimey run shows/fossile --listen 127.0.0.1:7340
   ```

   Watch the stage plan at <http://127.0.0.1:7340/> (and BlenderDMX if set up, [blenderdmx.md](blenderdmx.md)). Note anything to touch up at the rehearsal.

5. **Add the replay reference.**

   ```sh
   blimey replay shows/fossile --midi shows/fossile/recordings/<song>.mid --out tests/fixtures/dumps/fossile-<song>.dump
   cargo test -p blimey-engine --test song_recordings
   ```

   The test finds every `recordings/<name>.mid` and checks it against `tests/fixtures/dumps/fossile-<name>.dump`. Delete the song's `<song>.synthetic.mid` and its dump if there is one, and replace it in `shows/fossile/setlist.yaml` (in set order).

6. **Sign off.** Tick the song in the table below in the same PR.

| Song | MIDI recorded | EMU captured | Cues ported | Parity checked | Preview checked | Reference added | Signed off |
| --- | --- | --- | --- | --- | --- | --- | --- |
| tell-me | synthetic only | | example cues | | | synthetic | |

## Synthetic recordings

`shows/fossile/recordings/tell-me.synthetic.mid` was written by hand, not recorded, so the replay test and the soak have a song of the real show until the first recording: Start, MIDI clock at 92 BPM, Program Change 3 on channel 2, then cues 36 (beat 4), 42 (beat 20), 37 (beat 36), 39 (beat 44), master to 100 (beat 50), all released by beat 60, cue 38 (beat 64), master back to full (beat 66), Stop at beat 72.

## Soak

With every song recorded and in `setlist.yaml`:

```sh
blimey soak shows/fossile/setlist.yaml --hours 100 --accelerated      # minutes, any machine
blimey soak shows/fossile/setlist.yaml --hours 4 --emulated-enttec    # on the M1, unattended
```

The real-time run takes the full 4 hours; add `--listen 127.0.0.1:7340` to watch it, or replace `--emulated-enttec` with `--dmx-port …` to drive the real box. Each run writes its report to a new `soak-….txt` in the current folder (or `--report <file>`); attach the M1 one to the go/no-go decision.
