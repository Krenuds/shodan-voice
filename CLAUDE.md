# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`shodan-voice`: a Windows desktop app (Rust, edition 2024, eframe/egui) that turns a live microphone into the SHODAN voice from System Shock in real time, and sends it to an output device (typically a virtual audio cable) plus an optional headphone monitor. The sound is made by a reorderable **rack of modules**: the built-in SHODAN stages, any number of CLAP plugins, and other native modules such as TitoBot, which listens with Whisper and answers in a robot language instead of the voice.

## Commands

```powershell
cargo run                      # GUI (dev profile: own code opt-level 1, deps opt-level 3, so DSP is usable in debug)
cargo build --release          # release build has no console window (windows_subsystem = "windows")
cargo build --no-default-features   # without the CLAP host and without speech (no clack, no whisper.cpp, no CUDA needed)
cargo build --no-default-features --features clap-host,speech   # Whisper on the CPU: ~1.1 s per utterance instead of ~25 ms

cargo test                     # all unit tests (they live in #[cfg(test)] modules next to the code)
cargo test robot_flattens      # single test by name substring
cargo test jitter_buffer -- --nocapture   # several tests println! measured values worth reading

# Tests that need a real CLAP plugin are #[ignore]d:
$env:CLAP_PATH = "<dir containing a .clap>"; cargo test -- --ignored clap
```

Offline render — runs the exact same `Engine` over a WAV file, deterministic for a given seed. This is the way to listen to or diff a DSP change without a mic:

```powershell
cargo run -- --render samples\input_tts.wav samples\out_ss1.wav --preset ss1 --seed 1 --set robot=1 --set note=57
```

It renders the default rack (SHODAN Core → Metal → Lo-fi, the first three entries of `modules::NATIVE`) unless `--rack id,id,...` names another chain. `--say "text"` sends a transcript to the word bus (`Shared::words`); the TitoBot module plays the words that are in `src/lexicon.rs` and stays silent for the rest, e.g. `--rack titobot --say "yes no good bad"`. `--hear` (needs the `speech` feature and the model) instead runs the recogniser over the input WAV, prints what it heard, and delivers each utterance's words a fixed 50 ms after it ends, so the render stays reproducible. `--preset` matches built-in names loosely (case/space/dash-insensitive prefix: `ss1`, `ss2`, `subtle`, `glitchstorm`). `--set` uses parameter `key` strings (an unknown key prints the full list). `samples/out_*.wav` is gitignored; `samples/input_tts.wav` is the committed test input. The default rack is sample-identical to the pre-rack engine, so hashing renders before and after a refactor is a cheap regression check.

Set `SHODAN_CONFIG_DIR` to a scratch folder to run the GUI without reading or overwriting the real `settings.json`.

## Architecture

### Threads and the real-time rule

- **GUI thread** (`src/gui`): owns `App`, settings, device selection, the `Rack` model, and the main-thread half of every CLAP plugin.
- **Audio threads** (cpal callbacks in `src/audio/io.rs`): the mic capture callback runs the `Engine`; each output device has its own callback.

Everything reachable from `Engine::process` must not allocate, lock, or free. Cross-thread state is only atomics: `Arc<Shared>` (`src/shared.rs`: I/O params and meters) and one `Arc<ModuleShared>` per module instance (its params, enabled, mix). Anything that isn't a plain value crosses via `rtrb` SPSC rings.

### Audio I/O (`src/audio/io.rs`)

The engine runs *inside the input callback* at the microphone's sample rate, so that rate is the engine's and every module's rate. Processed stereo is pushed into one ring per output (main, monitor). Each output callback pulls through a `Resampler` that does cubic interpolation and nudges its ratio to keep the ring's minimum fill just above one mic block plus a small adaptive margin — this handles both sample-rate mismatch and clock drift at minimum latency. Latency figures shown in the GUI come from `Meters::{input_ms, dsp_ms, output_ms}`.

Changing devices tears everything down and rebuilds it (`App::restart_audio`): `io::start` calls back into `Rack::build` with the new sample rate, so the fresh `Engine` starts with every module already in place.

### The rack (`src/audio/engine.rs`, `src/audio/module.rs`)

`Engine` is only a host: mic → input gain → modules in order → output gain → bypass crossfade → limiter + soft clip. Input gain, output gain and bypass are the `io` parameter table on `Shared`, not a module.

A module implements `Module`: `process(&ModuleCtx, left, right)` works in place on a stereo block of at most `MAX_BLOCK` frames, and `ctx.dry` is the mic after input gain, for modules that key off or replace the voice. The engine applies each module's enable/mix crossfade itself and skips modules that are fully off (their state goes stale, by design). With mix at 100 % the module processes in place with no blend, which is what keeps the default rack bit-exact.

The rack is edited live through `Command::{Add, Remove, Order}` on a ring; storage is preallocated to `MAX_MODULES`. Added modules fade in. Removed ones are `stop()`ped and sent back on the garbage ring so they (and their `Arc<ModuleShared>`) are dropped on the GUI thread. `Order` carries the full id list rather than index moves, because the GUI list can contain items the engine doesn't have (a plugin that failed to activate).

### Native modules (`src/modules`)

Each module kind is one file exporting a `KIND: ModuleKind` (stable `id`, display name, parameter table, `make` fn) and is listed in `modules::NATIVE`. **Adding a native module = one file + one entry in `NATIVE`**; the Rack window's Add button, its knobs, persistence and `--set` all come from the table. Kinds today:

- `shodan.rs` — SHODAN Core: DC block / high-pass → noise gate → **LPC analyzer** (whitens to the vocal-fold residual, tracks f0) → **tape** (stutter / reverse / warp glitches fired at syllable onsets, then skip or speed back to live) → **voices** (granular pitch shifter with random jumps, up to 4 detuned copies, robot pitch flattening) → **LPC synthesizer** (puts the formants back, optionally shifted). Mono in (sums L+R), stereo out.
- `metal.rs` (flanger) and `lofi.rs`: thin stereo wrappers over `src/dsp`.

The DSP itself lives in `src/dsp` and knows nothing about modules. The key idea in the core is the source/filter split (`src/dsp/lpc.rs`): glitches and pitch moves happen on the residual, so formants stay put and are controlled independently. The synthesizer reads the analyzer's frame for the tape position the shifter is *currently playing* (`tape.position() - grain_delay`), not the live one — keep that alignment when touching tape, pitch or LPC timing.

Native modules smooth their continuous parameters with `Smoothed`, stepped once per `SUB_BLOCK` (32) samples.

Invariant enforced by `every_preset_stays_finite_and_below_full_scale`: output is always finite, ≤ 0 dBFS, and free of denormals for every built-in preset.

### Parameters, presets, settings

`define_params!` (`src/params.rs`) generates a `P` enum and `DEFS` table (key, label, group, range, default, unit, kind, help) in the module that invokes it; a `Params` is one instance's atomic values over such a table. Adding a knob means adding one line to a kind's table and reading it with `self.s.get(P::X)`.

`key` is the persistence identity. It must be unique across *all* tables (tested) and stay stable; reordering or adding params is safe.

Presets (`src/presets.rs`) are flat key → value maps. They act on the I/O table plus the **first module of each kind** in the rack (`presets::targets`) and never change which modules are in the rack. Built-in presets list only values that differ from defaults; `bypass` is deliberately never touched. `Values::apply` contains a migration shim for presets saved before `formant` existed — follow that pattern when a new parameter changes the meaning of old presets.

`settings.json` (in the `directories` config dir for `shodan-voice`) stores the rack as an ordered list of `RackItemSettings`, each native item with its own values. Files from before the rack have no `rack`; `Settings::rack_items` then builds the default chain from the old flat `knobs` and appends the old two plugin `slots`.

### GUI (`src/gui`)

Two OS windows from one eframe app. The **Voice** window (`app.rs`) has routing, meters, presets, I/O gain and the knobs of the first SHODAN Core. The **Rack** window is an egui immediate viewport (it needs `&mut App`, and `LoadedPlugin` is not `Send`) drawing `rack.rs`: one strip per module with reorder / remove / On / Blend and knobs generated from the kind's table (`param_ui.rs`).

`Rack` is the GUI-side list of items: native, CLAP, or `Missing` (a saved module that could not be recreated, kept so saving doesn't silently drop it). Each item knows whether the engine currently holds its audio half (`live`).

Only use glyphs egui's bundled fonts contain (see the list in egui's `lib.rs` docs); others render as empty boxes.

### CLAP plugins (`src/plugins`, feature `clap-host`, on by default)

Uses `clack-host` / `clack-extensions` pinned to a git rev. The engine only knows the `Module` trait, so everything plugin-related is behind `#[cfg(feature = "clap-host")]` and the crate must keep compiling without it.

Lifecycle is split across threads: `LoadedPlugin` (instance, params, editor window) lives on the GUI thread; `activate()` produces a boxed `Module` for the engine. A removed plugin waits in `Rack::pending` until its processor has come back over the garbage ring and been dropped; only then does deactivation succeed. Plugin parameter changes go GUI → audio through a per-plugin ring. Plugin output is sanitized (NaN/inf → 0, clamped) inside the processor.

Plugin editors are hosted in a plain Win32 top-level window (`src/plugins/window.rs`) created on the GUI thread so eframe's message loop pumps it. Plugin discovery (`scan.rs`) covers the standard Windows CLAP directories plus `CLAP_PATH`.

### TitoBot and speech (`src/lexicon.rs`, `src/speech`, `src/modules/titobot.rs`, features `speech` / `speech-cuda`, the latter on by default)

The default build compiles whisper.cpp with CUDA, so it needs cmake, clang and the NVIDIA CUDA toolkit (13.4 here; `CUDA_PATH` and `CUDA_PATH_V13_4` must be set, which a shell opened before the install lacks), and the exe needs the toolkit's DLLs on `PATH`. The first build takes about five minutes.

TitoBot replaces the voice with a robot language: a delayed translation that speaks only the words in `lexicon::WORDS` (each a fixed motif, so listeners can learn it) and is silent for everything else. Adding a word is one line in that table.

Words travel as lexicon indices on `Shared::words` (`WordBus`: atomics, one writer, each module reads with its own position). `lexicon::say(text, bus)` is the only way text gets in; it returns the words it had no motif for.

Live, TitoBot's `make` starts a `speech::listen` thread (only when `Shared::speech.enabled`, which the GUI sets and `--render` does not) and `process` pushes `ctx.dry` to it over a ring. The thread runs `segment::Segmenter` (resample to 16 kHz, level gate from the `tito_gate` knob, utterance = speech up to a 350 ms pause) and hands each utterance to `Recognizer` (Whisper via `whisper-rs`), then calls `lexicon::say` and appends `time, text, unknown words` to `transcripts.tsv` in the config folder — the material for growing the lexicon. The thread ends when the module (its ring producer) is dropped. Without the `speech` feature `Recognizer::new` always fails, so the module still works from `--say` and the strip shows why it is not listening.

The model is `models/ggml-base.en.bin` in the config folder, or `SHODAN_WHISPER_MODEL`; it is loaded once per process. Do not shrink Whisper's `audio_ctx` to speed up short utterances: the decoder then loops on the same phrase.

### Platform

Windows-only in practice: Win32 editor windows, `AttachConsole` so `--render` can print from a release build, and an F8 global hotkey for bypass. On first run the output defaults to a virtual cable device if one is installed.
