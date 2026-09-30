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

/// Start a thread that listens to mono audio at `sr` pushed into the returned ring, and says
/// what it hears on the word bus. `gate_db` is polled for the level that counts as speech.
/// The thread ends when the producer is dropped.
pub fn listen(sr: f32, app: Arc<Shared>, gate_db: impl Fn() -> f32 + Send + 'static) -> rtrb::Producer<f32> {
    let (tx, mut rx) = rtrb::RingBuffer::new(sr as usize * RING_SECONDS);
    let run = move || {
        let status = &app.speech;
        status.state.store(LOADING, Ordering::Relaxed);
        let mut recognizer = match Recognizer::new() {
            Ok(r) => r,
            Err(e) => {
                status.set_text(&e);
                status.state.store(FAILED, Ordering::Relaxed);
                return;
            }
        };
        status.set_text("");
        status.state.store(LISTENING, Ordering::Relaxed);

        let mut segmenter = Segmenter::new(sr);
        let mut block = Vec::with_capacity(4096);
        let mut utterances: Vec<Utterance> = Vec::new();
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
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            segmenter.push(&block, gate_db(), |u| utterances.push(u));
            for u in utterances.drain(..) {
                let started = Instant::now();
                match recognizer.transcribe(&u.audio) {
                    Ok(text) => {
                        let unknown = lexicon::say(&text, &app.words);
                        status.ms.set(started.elapsed().as_secs_f32() * 1000.0);
                        if !text.is_empty() {
                            status.set_text(&text);
                            log_transcript(&text, &unknown);
                        }
                    }
                    Err(e) => status.set_text(&e),
                }
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
fn log_transcript(text: &str, unknown: &[String]) {
    let Some(dir) = crate::presets::config_dir() else { return };
    let time = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("transcripts.tsv")) {
        let _ = writeln!(file, "{time}\t{text}\t{}", unknown.join(" "));
    }
}
