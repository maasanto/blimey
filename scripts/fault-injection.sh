#!/bin/sh
# Fault injection against the real binary (plan §9, §15), on an emulated Enttec box (a pty) and
# a virtual CoreMIDI port standing in for Reaper. Each scenario checks the engine recovers:
#   kill    kill -9 the engine mid-song, restart it: first frame and next cue within 1 s,
#           song and master restored
#   midi    drop the MIDI port and bring it back: the engine reconnects
#   serial  unplug the emulated box and plug it back: output resumes within 1 s
# Usage: scripts/fault-injection.sh [kill|midi|serial]   (no argument runs all three)
# macOS only. Changes nothing outside the system temp folder.
set -eu
cd "$(dirname "$0")/.."
exec cargo test -p blimey --test faults -- --ignored --test-threads=1 --nocapture ${1:+"$1"}
