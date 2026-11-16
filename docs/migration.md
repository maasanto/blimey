# Migrating a set from EMU

Each song moves over the same way: record what the DAW sends, port the cues, check against EMU and the preview, then lock it in with a replay reference. Everything here runs on the laptop, with no lights; the rig only comes out for the go/no-go rehearsal (plan §11, §15). Commands are in [api.md](api.md#recording-and-soak). `<show>` is your show folder, kept outside this repo; [tests/fixtures/shows/demo](../tests/fixtures/shows/demo) is a complete example.

## Once, before the first song

- Enable the IAC Driver in Audio MIDI Setup, and point the DAW's MIDI output at it. `blimey devices` should list it.
- For the parity capture, set EMU's output to Art-Net, sent to `127.0.0.1` (or the laptop's address) on universe 0. EMU and Blimey must never both open the Enttec box; while capturing, the box isn't needed at all.
- Quit BlenderDMX and anything else listening on UDP 6454 while capturing.

## Per song

1. **Record the MIDI and EMU's output.** In the DAW, go to the start of the song with EMU running, then:

   ```sh
   mkdir -p <show>/recordings <show>/references
   blimey record <show>/recordings/<song>.mid --until-stop --artnet <show>/references/emu-<song>.dump --show <show>
   ```

   Press play and let the song run to the end; recording stops when the DAW stops (or Ctrl-C). Time 0 is the first MIDI message, in both files. Check the summary line: a few thousand events for a song with MIDI clock.

2. **Port the cues.** Legacy-only songs need nothing: channel-1 notes go through `legacy-map.yaml` unchanged. For named cues, write `<show>/songs/<song>.yaml` (by hand or in the builder) and make sure the DAW session sends the song's Program Change on channel 2 at the start. Re-record if the session changed.

3. **Compare with EMU (parity, plan §15).** Render Blimey's output for the same MIDI:

   ```sh
   blimey replay <show> --midi <show>/recordings/<song>.mid --out "$TMPDIR/<song>.dump"
   ```

   and compare it with `<show>/references/emu-<song>.dump`. Both are on the same 25 ms frame grid, with only the show's universes in the capture, so the same line in each is the same moment: compare a few moments (`grep '^12[0-9][0-9][0-9] '` around 12 s in both), channel by channel. There is no automated comparison yet; EMU sends at its own rate, so the capture has no line for a frame EMU sent nothing in. Legacy channels should match exactly; differences on ported cues are expected and reviewed by eye on the preview.

4. **Check on the preview.** Run the show with the web UI and play the song from the DAW:

   ```sh
   blimey run <show> --listen 127.0.0.1:7340
   ```

   Watch the stage plan at <http://127.0.0.1:7340/> (and BlenderDMX if set up, [blenderdmx.md](blenderdmx.md)). Note anything to touch up at the rehearsal.

5. **Keep a replay reference.** Once the song looks right, store its render next to the show:

   ```sh
   blimey replay <show> --midi <show>/recordings/<song>.mid --out <show>/references/<song>.dump
   ```

   After a Blimey upgrade or a show edit, replay again to `$TMPDIR` and `diff` against it: any difference is a change in the lights to review. Add the recording to `<show>/setlist.yaml` (in set order) for the soak.

## Synthetic recordings

`tests/fixtures/shows/demo/recordings/demo-song.synthetic.mid` was written by hand, not recorded, so the replay test and the soak have a song to play: Start, MIDI clock at 92 BPM, Program Change 3 on channel 2, then cues 36 (beat 4), 42 (beat 20), 37 (beat 36), 39 (beat 44), master to 100 (beat 50), all released by beat 60, cue 38 (beat 64), master back to full (beat 66), Stop at beat 72.

## Soak

With every song recorded and in `setlist.yaml`:

```sh
blimey soak <show>/setlist.yaml --hours 100 --accelerated      # minutes, any machine
blimey soak <show>/setlist.yaml --hours 4 --emulated-enttec    # on the show machine, unattended
```

The real-time run takes the full 4 hours; add `--listen 127.0.0.1:7340` to watch it, or replace `--emulated-enttec` with `--dmx-port …` to drive the real box. Each run writes its report to a new `soak-….txt` in the current folder (or `--report <file>`); attach it to the go/no-go decision.
