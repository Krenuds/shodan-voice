//! Hearing: mic audio → utterances → text → the words the robot knows, on the word bus.
//!
//! None of this runs on the audio thread. A module hands its audio to [`listen`] through a ring.

pub mod segment;
#[cfg(feature = "speech")]
mod whisper;

use crate::lexicon;
use crate::shared::Shared;
use segment::{Segmenter, Utterance};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "speech")]
pub use whisper::Recognizer;

/// Stand-in for builds without Whisper: it can never be created.
#[cfg(not(feature = "speech"))]
pub struct Recognizer;

#[cfg(not(feature = "speech"))]
impl Recognizer {
    pub fn new() -> Result<Self, String> {
        Err("this build has no speech recognition (build with --features speech)".into())
    }

    pub fn transcribe(&mut self, _audio: &[f32]) -> Result<String, String> {
        unreachable!()
    }
}

/// What the recogniser is doing, for the GUI. Values of `Speech::state`.
pub const OFF: u32 = 0;
pub const LOADING: u32 = 1;
pub const LISTENING: u32 = 2;
pub const FAILED: u32 = 3;

/// Mic audio the listener has not got to yet; more than this is dropped.
const RING_SECONDS: usize = 10;
/// How much an utterance has to grow before it is recognised again while it is being spoken.
const PASS_INTERVAL: usize = segment::RATE * 12 / 100;
/// After this many quiet frames the utterance is taken as it stands, long before the pause
/// that ends it.
const SETTLE_FRAMES: usize = 8;

/// What [`Hearing`] reports.
pub enum Heard<'a> {
    /// A word the robot knows (index into `lexicon::WORDS`), to be spoken now. `after_ms` is
    /// how long the voice had been quiet when it was recognised; near zero while talking.
    Word { word: u32, after_ms: f32 },
    /// The utterance so far, after every pass of the recogniser.
    Text(&'a str),
    /// The utterance is over; this is all of it.
    Utterance(&'a str),
}

/// Mic audio in, words out, while they are being said: the utterance in progress is recognised
/// again and again as it grows. A word is spoken as soon as another word follows it, or two
/// passes in a row end on it, or the voice goes quiet. Spoken words cannot be taken back, so
/// this trades the odd wrong word for not waiting until the end of the sentence.
pub struct Hearing {
    segmenter: Segmenter,
    ended: Vec<Utterance>,
    stream: Stream,
}

/// The recogniser and what it has made of the current utterance.
struct Stream {
    recognizer: Recognizer,
    /// Text of the last pass.
    text: String,
    /// Words of this utterance the robot has been given.
    said: usize,
    /// Length and loud frames of the utterance at the last pass.
    passed_len: usize,
    passed_loud: usize,
    /// The last pass released every word.
    settled: bool,
    pass_ms: f32,
}

impl Hearing {
    /// `sr` is the rate of the audio that will be pushed.
    pub fn new(sr: f32) -> Result<Self, String> {
        let stream = Stream { recognizer: Recognizer::new()?, text: String::new(), said: 0, passed_len: 0, passed_loud: 0, settled: false, pass_ms: 0.0 };
        Ok(Self { segmenter: Segmenter::new(sr), ended: Vec::new(), stream })
    }

    /// How long the last pass of the recogniser took.
    pub fn pass_ms(&self) -> f32 {
        self.stream.pass_ms
    }

    pub fn push(&mut self, block: &[f32], gate_db: f32, mut out: impl FnMut(Heard)) -> Result<(), String> {
        let ended = &mut self.ended;
        self.segmenter.push(block, gate_db, |u| ended.push(u));
        self.end_utterances(&mut out)?;
        if let Some(p) = self.segmenter.partial()
            && p.loud >= segment::MIN_LOUD_FRAMES
        {
            let stream = &mut self.stream;
            let settle = p.quiet >= SETTLE_FRAMES;
            let due = if settle {
                !stream.up_to_date(p.loud)
            } else {
                p.loud > stream.passed_loud && p.audio.len() >= stream.passed_len + PASS_INTERVAL
            };
            if due {
                stream.pass(p.audio, p.loud, p.quiet, settle, &mut out)?;
            }
        }
        Ok(())
    }

    /// The input is over: whatever is being said ends here.
    pub fn finish(&mut self, mut out: impl FnMut(Heard)) -> Result<(), String> {
        let ended = &mut self.ended;
        self.segmenter.finish(|u| ended.push(u));
        self.end_utterances(&mut out)
    }

    fn end_utterances(&mut self, out: &mut impl FnMut(Heard)) -> Result<(), String> {
        let stream = &mut self.stream;
        for u in self.ended.drain(..) {
            let result = if stream.up_to_date(u.loud) { Ok(()) } else { stream.pass(&u.audio, u.loud, u.quiet, true, out) };
            out(Heard::Utterance(&stream.text));
            stream.text.clear();
            stream.said = 0;
            stream.passed_len = 0;
            stream.passed_loud = 0;
            stream.settled = false;
            result?;
        }
        Ok(())
    }
}

impl Stream {
    /// Nothing was said since a pass that released every word.
    fn up_to_date(&self, loud: usize) -> bool {
        self.settled && self.passed_loud == loud
    }

    /// Recognise the utterance as it stands and give out the words that are new and will not
    /// change any more; with `all`, even the last one.
    fn pass(&mut self, audio: &[f32], loud: usize, quiet: usize, all: bool, out: &mut impl FnMut(Heard)) -> Result<(), String> {
        let started = Instant::now();
        let text = self.recognizer.transcribe(audio)?;
        self.pass_ms = started.elapsed().as_secs_f32() * 1000.0;
        let after_ms = quiet as f32 * segment::FRAME_MS + self.pass_ms;

        for word in settled_words(&text, &self.text, all).into_iter().skip(self.said) {
            self.said += 1;
            out(Heard::Word { word, after_ms });
        }
        if !text.is_empty() {
            out(Heard::Text(&text));
        }
        self.text = text;
        self.passed_len = audio.len();
        self.passed_loud = loud;
        self.settled = all;
        Ok(())
    }
}

/// The words of `text` the robot knows and that will not change any more. The last word may
/// still be half said, unless `before`, the pass before this one, had it in the same place or
/// `all` says the voice has stopped. Words the robot does not know are silent, so they are
/// skipped and stay free to change. A phrase is settled when its last word is. A lone "you" is
/// what the recogniser makes of a breath, so it is never spoken.
fn settled_words(text: &str, before: &str, all: bool) -> Vec<u32> {
    if lexicon::is_noise(text) {
        return Vec::new();
    }
    let count = text.split_whitespace().count();
    let same = |i: usize| before.split_whitespace().nth(i).zip(text.split_whitespace().nth(i)).is_some_and(|(b, t)| lexicon::same(b, t));
    lexicon::spans(text).iter().take_while(|s| all || s.last + 1 < count || same(s.last)).filter_map(|s| s.word).collect()
}

/// Start a thread that listens to mono audio at `sr` pushed into the returned ring, and says
/// what it hears on the word bus. `gate_db` is polled for the level that counts as speech.
/// The thread ends when the producer is dropped.
pub fn listen(sr: f32, app: Arc<Shared>, gate_db: impl Fn() -> f32 + Send + 'static) -> rtrb::Producer<f32> {
    let (tx, mut rx) = rtrb::RingBuffer::new(sr as usize * RING_SECONDS);
    let run = move || {
        let status = &app.speech;
        status.state.store(LOADING, Ordering::Relaxed);
        let mut hearing = match Hearing::new(sr) {
            Ok(h) => h,
            Err(e) => {
                status.set_text(&e);
                status.state.store(FAILED, Ordering::Relaxed);
                return;
            }
        };
        status.set_text("");
        status.state.store(LISTENING, Ordering::Relaxed);

        let mut block = Vec::with_capacity(4096);
        loop {
            block.clear();
            while block.len() < block.capacity()
                && let Ok(x) = rx.pop()
            {
                block.push(x);
            }
            if block.is_empty() {
                if rx.is_abandoned() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            // Audio still waiting in the ring is delay the recogniser does not know about.
            let behind_ms = rx.slots() as f32 * 1000.0 / sr;
            let result = hearing.push(&block, gate_db(), |heard| match heard {
                Heard::Word { word, after_ms } => {
                    app.words.push(word);
                    status.delay_ms.set(after_ms + behind_ms);
                }
                Heard::Text(text) => status.set_text(text),
                Heard::Utterance(text) => log_transcript(text),
            });
            status.ms.set(hearing.pass_ms());
            if let Err(e) = result {
                status.set_text(&e);
            }
        }
        status.state.store(OFF, Ordering::Relaxed);
    };
    if let Err(e) = std::thread::Builder::new().name("speech".into()).spawn(run) {
        eprintln!("speech thread: {e}");
    }
    tx
}

/// Append what was heard to `transcripts.tsv` in the config folder: time, text, and the words
/// the lexicon does not have yet. This is the raw material for growing the language.
fn log_transcript(text: &str) {
    if text.is_empty() {
        return;
    }
    let unknown: Vec<&str> = lexicon::spans(text).iter().filter(|s| s.word.is_none()).map(|s| s.token).collect();
    let Some(dir) = crate::presets::config_dir() else { return };
    let time = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("transcripts.tsv")) {
        let _ = writeln!(file, "{time}\t{text}\t{}", unknown.join(" "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(text: &str, before: &str, all: bool) -> Vec<&'static str> {
        settled_words(text, before, all).into_iter().map(|w| lexicon::WORDS[w as usize].text).collect()
    }

    #[test]
    fn the_last_word_waits_for_a_second_pass_or_the_pause() {
        assert_eq!(settled("Yes", "", false), [""; 0]);
        assert_eq!(settled("Yes.", "yes", false), ["yes"]);
        assert_eq!(settled("Yes", "", true), ["yes"]);
        // A different word in that place is not agreement.
        assert_eq!(settled("No", "Yes", false), [""; 0]);
    }

    #[test]
    fn a_word_is_settled_once_another_follows() {
        assert_eq!(settled("Yes, that is good", "Yes", false), ["yes"]);
        assert_eq!(settled("Yes, that is good.", "Yes, that is good", false), ["yes", "good"]);
        assert_eq!(settled("Cable bad no", "", false), ["bad"]);
    }

    #[test]
    fn a_phrase_is_one_word_however_it_arrives() {
        assert_eq!(settled("Thank", "Thank", false), ["thanks"]);
        // Already spoken after the pass before, and counted, so holding it back here is harmless.
        assert_eq!(settled("Thank you", "Thank", false), [""; 0]);
        assert_eq!(settled("Thank you.", "Thank you", false), ["thanks"]);
        assert_eq!(settled("I don't", "I", false), ["me"]);
        assert_eq!(settled("I don't know", "I don't", false), ["me"]);
        assert_eq!(settled("I don't know.", "I don't", true), ["me", "don't know"]);
    }

    #[test]
    fn a_lone_you_is_a_breath() {
        assert_eq!(settled("You.", "You", true), [""; 0]);
        assert_eq!(settled("You go", "You", true), ["you", "go"]);
    }
}
