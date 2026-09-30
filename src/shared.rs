//! State shared between the GUI thread and the audio thread (all lock-free atomics, apart from
//! the recogniser's status text, which the audio thread never touches).

use crate::params::{Params, io};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
    /// Keep the maximum; used for peak meters the GUI resets after reading.
    pub fn max(&self, v: f32) {
        let _ = self.0.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
            (v > f32::from_bits(old)).then_some(v.to_bits())
        });
    }
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

#[derive(Default)]
pub struct Meters {
    pub input_peak: AtomicF32,
    pub output_peak: AtomicF32,
    /// Semitone offset of the main voice.
    pub pitch: AtomicF32,
    /// How far the tape head is behind live, in ms.
    pub lag_ms: AtomicF32,
    /// Incremented for every glitch fired, so the GUI can flash.
    pub glitches: AtomicU32,
    /// Audio callbacks that had to pad with silence (input ran dry).
    pub underruns: AtomicU32,
    /// Age of the oldest mic sample when its callback ran (driver capture delay), ms.
    pub input_ms: AtomicF32,
    /// Delay added by the rack modules, ms.
    pub dsp_ms: AtomicF32,
    /// Buffered + device playback delay for [main output, monitor], ms.
    pub output_ms: [AtomicF32; 2],
}

const WORD_SLOTS: usize = 64;

/// Recognised words (indices into `lexicon::WORDS`) on their way to the modules that speak them.
/// One writer at a time, any number of readers; a reader that falls a full lap behind loses the
/// oldest words.
pub struct WordBus {
    slots: [AtomicU32; WORD_SLOTS],
    head: AtomicU32,
}

impl Default for WordBus {
    fn default() -> Self {
        Self { slots: std::array::from_fn(|_| AtomicU32::new(0)), head: AtomicU32::new(0) }
    }
}

impl WordBus {
    pub fn push(&self, word: u32) {
        let head = self.head.load(Ordering::Relaxed);
        self.slots[head as usize % WORD_SLOTS].store(word, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// Where a new reader starts so that it only hears words pushed from now on.
    pub fn head(&self) -> u32 {
        self.head.load(Ordering::Acquire)
    }

    /// The next word after the reader's position `seen`, advancing it.
    pub fn next(&self, seen: &mut u32) -> Option<u32> {
        let head = self.head();
        if *seen == head {
            return None;
        }
        if head.wrapping_sub(*seen) > WORD_SLOTS as u32 {
            *seen = head.wrapping_sub(WORD_SLOTS as u32);
        }
        let word = self.slots[*seen as usize % WORD_SLOTS].load(Ordering::Relaxed);
        *seen = seen.wrapping_add(1);
        Some(word)
    }
}

/// The speech recogniser, as the GUI sees it. The audio thread only ever reads `enabled`.
#[derive(Default)]
pub struct Speech {
    /// Modules may start a recogniser. Off for offline renders, which must stay deterministic.
    pub enabled: AtomicBool,
    /// One of the `speech::OFF`.. constants.
    pub state: AtomicU32,
    /// How long the last pass of the recogniser took, ms.
    pub ms: AtomicF32,
    /// How long after the voice went quiet the last word was sent to the robot, ms. About the
    /// pass time for a word sent while still talking.
    pub delay_ms: AtomicF32,
    /// The last thing heard, or why the recogniser failed.
    text: Mutex<String>,
}

impl Speech {
    pub fn set_text(&self, text: &str) {
        if let Ok(mut t) = self.text.lock() {
            t.clear();
            t.push_str(text);
        }
    }

    pub fn text(&self) -> String {
        self.text.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

pub struct Shared {
    /// Input gain, output gain and bypass: the fixed stage around the rack.
    pub io: Params,
    pub meters: Meters,
    pub words: WordBus,
    pub speech: Speech,
}

impl Default for Shared {
    fn default() -> Self {
        Self { io: Params::new(io::DEFS), meters: Meters::default(), words: WordBus::default(), speech: Speech::default() }
    }
}
