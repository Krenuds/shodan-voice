# Handoff: TitoBot's lexicon after the first 32 words

`CLAUDE.md` ("TitoBot and speech") describes how the pieces fit. Nothing below is decided unless
it says so.

## Where things stand

- `src/lexicon.rs` has 32 words in ten classes, with synonyms (`also`) and two-word phrases.
  Motifs are hand-designed from per-class building blocks; rests inside a motif are new.
- Checked offline only: every word through `--say`, and a synthesised sentence through `--hear`
  (phrases arrive as one word, 80–240 ms after the voice). **Not yet tried live with a mic.**
- Decided: late is worse than wrong; synonyms share a motif; filler words stay silent; a lone
  "you" is treated as noise and never spoken.
- User presets now save the whole rack; the Voice window has an **Update** button that re-saves
  the loaded preset. `tito` and `rashbot` in the real settings were saved before that (no `rack`)
  and need an Update to pick up their rack.

## Open

1. **Can people tell the words apart?** Nobody has listened to the 32 motifs yet. Likely
   confusions: `ok` / `now` / `here` (all two short notes), `what` / `where` (same ending by
   design), `me` / `you` (one tone each, told apart only by pitch).
2. **Whisper's noise words.** Only a lone "you" is filtered. `transcripts.tsv` also showed "oh"
   (not in the lexicon) and Whisper is known to write "Thank you." for silence; `thanks` would
   then be spoken at random. If that happens, extend `lexicon::is_noise` or raise the gate.
3. **Half-said words.** Streaming can send a word that turns out to be the start of a longer
   one. None of the 32 is a prefix of another entry, but growing the table will bring such
   pairs ("no" / "not" / "nobody"). Then require two passes to agree for those words.
4. **Keeping up.** Most words are 2–3 beats, and `HURRY` plays up to 2x. Untested with a fast
   speaker now that many common words are known.
5. **Growing past 32.** Hand-writing stops scaling at some point; the classes are the start of
   a rule system (tone colour or octave per class would add room). Word forms are listed by
   hand in `also`; there is no stemming, and Whisper writes numbers as digits.
6. Not measured: recogniser pass time with the game on the GPU. The strip shows it; if it goes
   well past 120 ms, raise `PASS_INTERVAL` in `src/speech/mod.rs`.

## Working without a mic

```powershell
# Known words straight to the robot
cargo run --release -- --render samples\input_tts.wav samples\out_say.wav --rack titobot --say "hello, thank you, follow me"
# Through the recogniser, same code as live; prints when each word reaches the robot
cargo run --release -- --render samples\out_phrases_in.wav samples\out_phrases.wav --rack titobot --hear
```

`samples\out_phrases_in.wav` (synthesised "Hello. Thank you. I don't know. Wait here. Follow me.
Where you go now?") is gitignored; recreate it with Windows `System.Speech`. With many words
queued at once `--say` plays them hurried; say a few at a time to hear them at the tempo.

Tests to keep green: `lexicon::tests` and `speech::tests`.

## Environment gotchas

- The default build needs the CUDA toolkit (13.4). A shell without `CUDA_PATH` /
  `CUDA_PATH_V13_4` fails in CMake, and the exe needs `%CUDA_PATH%\bin` and `bin\x64` on `PATH`.
  Both can be read from the machine environment and set for the session.
- The model is `%APPDATA%\shodan-voice\config\models\ggml-base.en.bin`, outside the repo.
- A running `shodan-voice.exe` locks the exe; close the app before building that profile.
- The user wants rendered audio opened for them (`Invoke-Item`) before a turn ends.

## Parked

TitoBot's own sound bank; two TitoBots in one rack doubling every word;
drag-and-drop reordering; click-testing with a real CLAP plugin; keyword spotting as a fast path
beside Whisper.
