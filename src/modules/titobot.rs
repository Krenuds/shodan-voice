//! TitoBot: speaks the robot language instead of the voice. Words arrive on the app's word bus
//! and are played one after another as their motifs from the lexicon; between words it is silent.

use super::ModuleKind;
use crate::audio::module::{Module, ModuleCtx, ModuleShared, SUB_BLOCK, Smoothed};
use crate::lexicon::{WORDS, Word};
use crate::shared::Shared;
use crate::speech;
use std::sync::Arc;
use std::sync::atomic::Ordering;

crate::params::define_params! {
    TitoKey   => ("tito_key", "Key", "TitoBot", 48.0, 96.0, 76.0, "note", Stepped, "The note the words are built on."),
    TitoTempo => ("tito_tempo", "Tempo", "TitoBot", 40.0, 300.0, 110.0, "ms", Continuous, "Length of one beat."),
    TitoTone  => ("tito_tone", "Tone", "TitoBot", 0.0, 1.0, 0.35, "", Continuous, "0 = pure whistle, 1 = buzzy."),
    TitoLevel => ("tito_level", "Level", "TitoBot", 0.0, 1.0, 0.5, "", Continuous, "Loudness of the robot."),
    TitoGate  => ("tito_gate", "Gate", "TitoBot", -70.0, -10.0, -40.0, "dB", Continuous, "Mic level that counts as speech for the recogniser."),
}

pub const KIND: ModuleKind = ModuleKind {
    id: "titobot",
    name: "TitoBot",
    defs: DEFS,
    make: |sr, _seed, shared, app| {
        let tap = app.speech.enabled.load(Ordering::Relaxed).then(|| {
            let shared = shared.clone();
            speech::listen(sr, app.clone(), move || shared.params.get(P::TitoGate))
        });
        Box::new(TitoBot {
            tap,
            s: Smoothed::new(sr, &shared.params),
            shared: shared.clone(),
            seen: app.words.head(),
            app: app.clone(),
            sr,
            queue: [0; QUEUE],
            queue_start: 0,
            queue_len: 0,
            word: None,
            note: 0,
            pos: 0.0,
            rest: 0.0,
            phase: 0.0,
            hurry: 1.0,
        })
    },
};

/// Words that can wait their turn; more than this are dropped.
const QUEUE: usize = 32;
/// Silence between two words, in beats.
const WORD_GAP: f32 = 1.0;
/// A word with others waiting behind it is played this much faster per waiting word, up to
/// `MAX_HURRY` times, so the robot catches up with the speaker.
const HURRY: f32 = 0.5;
const MAX_HURRY: f32 = 2.0;
const ATTACK_S: f32 = 0.004;
const RELEASE_S: f32 = 0.012;

struct TitoBot {
    shared: Arc<ModuleShared>,
    app: Arc<Shared>,
    /// The mic, on its way to the recogniser thread. None when rendering offline.
    tap: Option<rtrb::Producer<f32>>,
    s: Smoothed,
    sr: f32,
    /// Read position on the word bus.
    seen: u32,
    queue: [u32; QUEUE],
    queue_start: usize,
    queue_len: usize,
    word: Option<&'static Word>,
    /// Index of the note being played and how far into it we are, in beats.
    note: usize,
    pos: f32,
    /// Beats of silence left before the next word may start.
    rest: f32,
    phase: f32,
    /// Speed of the word being played, 1 = at the tempo.
    hurry: f32,
}

impl TitoBot {
    fn next_word(&mut self) -> Option<&'static Word> {
        if self.queue_len == 0 {
            return None;
        }
        let word = self.queue[self.queue_start];
        self.queue_start = (self.queue_start + 1) % QUEUE;
        self.queue_len -= 1;
        WORDS.get(word as usize)
    }
}

impl Module for TitoBot {
    fn process(&mut self, ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        if let Some(tap) = &mut self.tap {
            // A full ring means the recogniser is far behind; that audio is simply not heard.
            for &x in ctx.dry {
                if tap.push(x).is_err() {
                    break;
                }
            }
        }
        while let Some(word) = self.app.words.next(&mut self.seen) {
            if self.queue_len < QUEUE {
                self.queue[(self.queue_start + self.queue_len) % QUEUE] = word;
                self.queue_len += 1;
            }
        }

        let n = left.len();
        let mut s0 = 0;
        while s0 < n {
            let s1 = (s0 + SUB_BLOCK).min(n);
            self.s.update(&self.shared.params);
            let key = self.s.get(P::TitoKey);
            let beat_s = self.s.get(P::TitoTempo) * 0.001;
            let tone = self.s.get(P::TitoTone);
            let level = self.s.get(P::TitoLevel);

            for i in s0..s1 {
                if self.word.is_none() {
                    if self.rest > 0.0 {
                        self.rest -= self.hurry / (beat_s * self.sr);
                    } else {
                        self.word = self.next_word();
                        self.note = 0;
                        self.pos = 0.0;
                        self.hurry = (1.0 + HURRY * self.queue_len as f32).min(MAX_HURRY);
                    }
                }
                let beat_s = beat_s / self.hurry;
                let beats_per_sample = 1.0 / (beat_s * self.sr);
                let mut y = 0.0;
                if let Some(word) = self.word {
                    let note = &word.notes[self.note];
                    let semi = note.from + (note.to - note.from) * (self.pos / note.beats);
                    let hz = 440.0 * 2f32.powf((key - 69.0 + semi) / 12.0);
                    self.phase = (self.phase + hz / self.sr).fract();
                    let sine = (self.phase * std::f32::consts::TAU).sin();
                    // Towards a square for the buzzy end of Tone.
                    let voice = sine + tone * ((sine * 4.0).clamp(-1.0, 1.0) - sine);
                    let since = self.pos * beat_s;
                    let left_s = (note.beats - self.pos) * beat_s;
                    let env = (since / ATTACK_S).min(left_s / RELEASE_S).clamp(0.0, 1.0);
                    if !note.rest {
                        y = voice * env * level;
                    }

                    self.pos += beats_per_sample;
                    if self.pos >= note.beats {
                        self.pos = 0.0;
                        self.note += 1;
                        if self.note == word.notes.len() {
                            self.word = None;
                            self.rest = WORD_GAP;
                        }
                    }
                }
                left[i] = y;
                right[i] = y;
            }
            s0 = s1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::engine::Engine;
    use crate::lexicon;
    use crate::modules::{self, Instance};

    /// Peak of each 100 ms of output after saying `text` over a noisy mic.
    fn speak(text: &str) -> Vec<f32> {
        let sr = 48000.0;
        let app = Arc::new(Shared::default());
        let rack = [Instance::new(&KIND)];
        let mut engine = Engine::new(sr, app.clone(), modules::build(&rack, sr, 1, &app), None);
        lexicon::say(text, &app.words);
        let n = 480;
        let input: Vec<f32> = (0..n).map(|i| 0.3 * (i as f32 * 0.1).sin()).collect();
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        let mut peaks = Vec::new();
        for block in 0..300 {
            engine.process(&input, &mut l, &mut r);
            if block % 10 == 0 {
                peaks.push(0.0);
            }
            for &y in l.iter().chain(&r) {
                assert!(y.is_finite() && y.abs() <= 1.0);
                *peaks.last_mut().unwrap() = y.abs().max(*peaks.last().unwrap());
            }
        }
        peaks
    }

    #[test]
    fn known_words_sound_and_then_it_is_silent() {
        let peaks = speak("yes no");
        println!("{peaks:.2?}");
        assert!(peaks[0] > 0.2, "nothing played");
        // yes (3 beats, hurried by the waiting no) + gap (1) + no (3) at 110 ms.
        assert!(peaks[8..].iter().all(|&p| p == 0.0), "still sounding after the words");
    }

    #[test]
    fn unknown_words_and_the_voice_are_silent() {
        assert!(speak("cable later").iter().all(|&p| p == 0.0));
    }
}
