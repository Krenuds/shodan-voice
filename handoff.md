# Handoff: grow TitoBot's lexicon

Next session has one goal: **take the robot language from 4 words to a vocabulary large enough to
talk with**. `CLAUDE.md` ("TitoBot and speech") describes how the pieces fit. Nothing below is
decided unless it says so.

## Where things stand

- The speed work is done but **uncommitted** (10 modified files on `master`). Commit it first
  (`/sendit`) so the lexicon work starts from a clean tree.
- TitoBot now recognises while the person is talking (`speech::Hearing`): a word is sent as soon
  as another word follows it, two passes end on it, or the voice has been quiet for 80 ms. The
  user tried it live in the release build and called it "perfect". Decided: late is worse than
  wrong.
- Not measured: pass time with the game on the GPU. The strip shows it; if it goes well past
  120 ms, raise `PASS_INTERVAL` in `src/speech/mod.rs`.

## The lexicon today

`src/lexicon.rs`: `WORDS` is a static table, one line per word, each a fixed motif of `Note`s
(semitones above the key, optional slide, length in beats). Four words: `yes`, `no`, `good`,
`bad`. `lookup` matches one whitespace token, ignoring case and surrounding punctuation. The
module plays a motif as a sine-to-square tone with a short envelope (`src/modules/titobot.rs`).

Words travel as `u32` indices into `WORDS`, so the table can grow freely; the order of entries
is not persisted anywhere.

## What "greatly expanding" runs into

1. **Hand-writing motifs does not scale.** Four were designed one by one with a comment each. A
   few hundred need a system: either rules that generate a motif from the word (its meaning
   class, syllables, or sounds), or a small set of building blocks combined by hand. The user
   wants listeners to be able to learn the language, so the same word must always sound the
   same, and related words should probably sound related (`yes`/`no` are mirrors today).
2. **Motif space.** With only pitch and rhythm on one tone, several hundred distinct motifs of
   3–5 beats get hard to tell apart. More dimensions would help: tone colour per word class,
   rests inside a motif, octave, vibrato, two-tone chords. That touches the synth in
   `titobot.rs`, and the parked idea of TitoBot getting its own sound bank.
3. **Word forms.** `lookup` is exact: "goods", "better", "yeah", "nope", "don't" all miss. A big
   lexicon needs a decision on stemming, synonyms mapping to one motif (`yeah` → `yes`), and
   contractions. Whisper also writes numbers as digits.
4. **Multi-word entries.** "thank you", "I don't know". `lookup` is per token, and
   `settled_words` in `src/speech/mod.rs` settles and counts single tokens (`Stream::said`), so
   phrases need a change in both. Streaming makes this harder: "thank" may already be said when
   "you" arrives.
5. **Wrong words get more likely.** Streaming can send a half-said word. With four words that is
   rare; with hundreds, short words that begin longer ones ("no" / "not" / "nobody", "in" /
   "into") will fire early. If it becomes a problem, require two passes to agree on every word
   (about 120 ms slower), or only for words that are a prefix of another entry.
6. **Whisper says "you" for noise.** `transcripts.tsv` has 14 lone "you" utterances from
   breaths or clicks. If `you` gets a motif the robot will say it at random; filter lone "you"
   (and similar: "oh", "thank you") or raise the gate before adding such words.
7. **Speed of speech.** Motifs are 330–550 ms and a fast speaker says 3–4 words a second. With
   most words known instead of almost none, the queue will back up even with the catch-up
   (`HURRY`, up to 2x). Shorter motifs for common words, or skipping function words, may be
   needed.

## Where the words come from

`%APPDATA%\shodan-voice\config\transcripts.tsv` (time, text, unknown words) was meant to be the
source. It has 122 lines, nearly all test utterances of the four words; the most frequent
unknown words are `you` 14, `oh` 7, `cable` 5, `is` 4. **It is too small to choose a vocabulary
from.** Options: a standard frequency list (a few hundred most common spoken English words), a
list the user writes for what they say in game, or have the user talk for a while first and
mine the log.

## Questions for the user

- How many words, roughly: 100, 500, everything common?
- Which words matter: general conversation, or game callouts and names?
- Should every known word be spoken, or should the robot skip filler ("the", "a", "is") to keep up?
- Generated motifs from rules, or hand-tuned ones for a core set and generated for the rest?
- Do synonyms share a motif (`yeah`, `yep` → `yes`)?
- May the four existing motifs change if a system needs them to?

## Working without a mic

```powershell
# Known words straight to the robot
cargo run --release -- --render samples\input_tts.wav samples\out_say.wav --rack titobot --say "yes no good bad"
# Through the recogniser, same code as live; prints when each word reaches the robot
cargo run --release -- --render samples\out_words_in.wav samples\out_stream.wav --rack titobot --hear
```

`samples\out_words_in.wav` (synthesised "Yes. No. Good. Bad. Maybe. Yes, that is good.") is
gitignored; recreate it, or a longer test sentence, with Windows `System.Speech`. The release exe
prints nothing when run directly from some shells; `cargo run` or redirecting its output works.

Tests to keep green: `lexicon::tests` (unique, playable words) and `speech::tests`
(`settled_words`).

## Environment gotchas

- The default build needs the CUDA toolkit (13.4). A shell without `CUDA_PATH` /
  `CUDA_PATH_V13_4` fails in CMake, and the exe needs `%CUDA_PATH%\bin` and `bin\x64` on `PATH`.
  Both can be read from the machine environment and set for the session.
- The model is `%APPDATA%\shodan-voice\config\models\ggml-base.en.bin`, outside the repo.
- A running `shodan-voice.exe` locks the exe; close the app before building that profile.
- The user wants rendered audio opened for them (`Invoke-Item`) before a turn ends.

## Parked

Judging motifs for learnability with real listeners; TitoBot's own sound bank; two TitoBots in
one rack doubling every word; whole-rack presets; drag-and-drop reordering; click-testing with a
real CLAP plugin; keyword spotting as a fast path beside Whisper.
