# Strata — agent guide

Strata is a headless DMX lighting engine driven by Reaper over MIDI, replacing ENTTEC EMU for Fossile's show. The spec is [docs/plan.md](docs/plan.md); section 14 defines the work packages (C0–C13) and section 15 the validation strategy. Read both before starting a chunk.

## Repo layout

- `engine/` — Rust library, all logic
- `strata/` — Rust binary: CLI, HTTP/WebSocket, device drivers
- `ui/` — Svelte + Vite, built to static files embedded in the binary
- `schema/` — JSON Schema for show files
- `shows/fossile/` — the real show
- `tests/fixtures/` — MIDI files and reference frame dumps
- `docs/` — spec, API contract (`docs/api.md`), decisions

## Ground rules

- No hardware in tests. The engine takes an injectable clock, a `MidiSource` and a `DmxSink`. Tests use a MIDI-file source and the `FakeDmx` recorder; real drivers (IAC, Enttec, Art-Net) sit behind the same traits.
- Replay tests are the acceptance format: MIDI file + show folder in, frame dump out, compared to a stored reference. A chunk that changes output on purpose regenerates its references and says so in the PR.
- Deterministic render: no free-running oscillators, no wall clock in the engine, random effects use a fixed seed per cue.
- Contracts first. A chunk that adds an HTTP/WebSocket endpoint or a show-file field updates `docs/api.md` and `schema/` in the same PR. UI chunks code against the contract, not engine internals.
- Show files written by the builder go through one engine endpoint that validates, writes atomically and keeps canonical formatting. The UI never writes files itself.
- CI must stay green: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, schema validation of `shows/`, UI Playwright tests.

## Workflow

- One chunk = one branch (`c<N>-<slug>`, e.g. `c3-named-cues`) = one PR against `main`.
- Branch from an up-to-date `main`. Squash before the first push; after that, review fixes are new commits, never force-push.
- Keep the diff to the chunk's scope. Unrelated fixes get their own PR.
- PR description: what the chunk delivers, how acceptance is tested, any regenerated references, then the chunk's human check from `docs/plan.md` as a checklist for Antoine (`- [ ] ...`, or "Human check: none").
