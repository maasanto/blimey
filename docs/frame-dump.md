# Frame dump format

A frame dump is the recorded DMX output of a replay: one line per frame, plain text, so a change in output shows up as a readable `git diff`. Reference dumps live in `tests/fixtures/dumps/`.

```
# blimey frame dump v1
0
25
50 6=254 7=128
75 6=254 7=128 512=255
```

- Lines starting with `#` are comments. The first line names the format version.
- Each other line is one frame: its time in whole milliseconds since the start of the replay, then the channels that are not zero, as `channel=value`.
- Channels are 1-based (1–512), as on a lighting desk. Values are 0–255. A channel that isn't listed is 0, so an all-zero frame is just its time.
- Channels are listed in ascending order. Fields are separated by single spaces.
- Frames are 25 ms apart (40 Hz): frame N is at N × 25 ms. A replay renders every frame from 0 up to and including the time of the MIDI file's last event.

## Producing and checking dumps

```sh
blimey replay <show-dir> --midi <file.mid> --out <dump>        # write a dump
blimey replay <show-dir> --midi <file.mid> --compare <dump>    # check against a reference
```

Without `--out` or `--compare` the dump goes to stdout. `--compare` exits with status 1 and names the first difference, for example:

```
replay differs from ref.dump: frame 12 (300 ms), channel 7: expected 128, got 0
```

A difference in frame time or frame count is reported the same way. A chunk that changes output on purpose regenerates its references with `--out` and says so in its PR.
