# Handoff: TitoBot voice module

`CLAUDE.md` has the architecture ("TitoBot and speech"). This file is what is decided, what is
done, and what is next.

## Decided

- The robot is a **delayed translation**: it speaks after an utterance is recognised, and only
  words in the lexicon. Everything else is silent, so fragments come through until the lexicon grows.
- It is a **language players can learn**: each word is always the same motif (`src/lexicon.rs`).
- Whisper runs **in the app** (`whisper-rs` / whisper.cpp), target backend **CUDA**.
- The lexicon grows from **transcripts**: every utterance heard live is appended to
  `transcripts.tsv` in the config folder with the words the lexicon lacks.

## Done (the MVP)

- Lexicon with `yes`, `no`, `good`, `bad`; word bus; TitoBot module (synthesised tones; knobs Key,
  Tempo, Tone, Level, Gate); `--render --rack / --say / --hear`.
- Utterance segmenter (tested) and the Whisper recogniser behind the `speech` feature; live listener
  thread fed from the module; status line in the Rack strip.
- CUDA toolkit 13.4 installed; `speech-cuda` is in the default features. Recognition takes about
  25 ms per utterance on the RTX 4070 SUPER (1.1 s on CPU).
- Verified offline with `base.en`: `samples\input_tts.wav` and a synthesised
  "Yes. No. Good. Bad. Maybe. Yes, that is good." are transcribed correctly, the four words play,
  "maybe / that / is" stay silent. Default SHODAN render hash unchanged.
- Model downloaded to `%APPDATA%\shodan-voice\config\models\ggml-base.en.bin`.

## Not done

- The live mic path was tried by hand once in the GUI and accepted as the MVP; it has no
  automated test, and nothing was measured (delay, misheard words, Gate setting for the mic).
- The robot answers about 0.4 s after you stop: recognition is ~25 ms on the GPU, the rest is the
  350 ms pause the segmenter waits for (`HANG_FRAMES` in `src/speech/segment.rs`). Shortening it
  splits slow speech into more utterances.
- The motifs are first guesses and nobody has judged them by ear for learnability.
- TitoBot's own sound bank is still not located (not in `~/hrrp/[peds]/titobot3`); the voice is a
  synthesised tone.
- Two TitoBots in one rack would both listen and each word would be spoken twice by each.
- A tool to summarise `transcripts.tsv` (most frequent unknown words) would make choosing the
  next words easy.

## Not done in the rack

Whole-rack presets, drag-and-drop reordering, and click-testing add/move/remove in the Rack window
with audio running and with a real CLAP plugin loaded.
