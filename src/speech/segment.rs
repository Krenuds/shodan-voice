//! Cuts a live mono signal into utterances for the recogniser: resampled to 16 kHz, each one
//! running from where the level rises above the gate to the first pause. Silence never reaches
//! the recogniser, which would otherwise invent words for it.

use crate::dsp::filters::{Biquad, cubic};
use crate::params::db_to_gain;
use std::collections::VecDeque;

/// Sample rate of the utterances, as Whisper wants it.
pub const RATE: usize = 16000;
/// The level is judged once per this many samples (10 ms).
const FRAME: usize = RATE / 100;
/// Audio kept from before the gate opened, so soft word starts are not cut off.
const PRE_ROLL: usize = 15 * FRAME;
/// This many quiet frames in a row end the utterance.
const HANG_FRAMES: usize = 35;
/// Quiet kept at the end of an utterance.
const TAIL_FRAMES: usize = 15;
/// Utterances with fewer loud frames are clicks, not speech.
const MIN_LOUD_FRAMES: usize = 8;
const MAX_LEN: usize = 6 * RATE;

pub struct Utterance {
    pub audio: Vec<f32>,
    /// How many input samples had been pushed when the utterance was complete.
    pub end: u64,
}

pub struct Segmenter {
    /// Input samples per output sample.
    step: f64,
    frac: f64,
    history: [f32; 4],
    lowpass: [Biquad; 2],
    consumed: u64,
    frame: Vec<f32>,
    pre_roll: VecDeque<f32>,
    speech: Vec<f32>,
    speaking: bool,
    loud: usize,
    quiet: usize,
}

impl Segmenter {
    pub fn new(sr: f32) -> Self {
        // Fourth-order Butterworth below the new Nyquist.
        let mut lowpass = [Biquad::default(), Biquad::default()];
        lowpass[0].set_lowpass(sr, 7200.0, 0.541);
        lowpass[1].set_lowpass(sr, 7200.0, 1.307);
        Self {
            step: sr as f64 / RATE as f64,
            frac: 0.0,
            history: [0.0; 4],
            lowpass,
            consumed: 0,
            frame: Vec::with_capacity(FRAME),
            pre_roll: VecDeque::with_capacity(PRE_ROLL + FRAME),
            speech: Vec::new(),
            speaking: false,
            loud: 0,
            quiet: 0,
        }
    }

    /// Feed audio at the rate given to `new`. `emit` is called for every utterance that ends.
    pub fn push(&mut self, input: &[f32], gate_db: f32, mut emit: impl FnMut(Utterance)) {
        let gate = db_to_gain(gate_db);
        for &x in input {
            self.consumed += 1;
            let x = self.lowpass[0].process(x);
            let x = self.lowpass[1].process(x);
            self.history.rotate_left(1);
            self.history[3] = x;
            while self.frac < 1.0 {
                let h = &self.history;
                self.frame.push(cubic(h[0], h[1], h[2], h[3], self.frac as f32));
                self.frac += self.step;
                if self.frame.len() == FRAME {
                    self.end_frame(gate, &mut emit);
                }
            }
            self.frac -= 1.0;
        }
    }

    /// The input is over: emit the utterance in progress, if any.
    pub fn finish(&mut self, mut emit: impl FnMut(Utterance)) {
        if self.speaking {
            self.end_utterance(&mut emit);
        }
    }

    fn end_frame(&mut self, gate: f32, emit: &mut impl FnMut(Utterance)) {
        let rms = (self.frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
        let loud = rms > gate;
        if self.speaking {
            self.speech.extend_from_slice(&self.frame);
            if loud {
                self.loud += 1;
                self.quiet = 0;
            } else {
                self.quiet += 1;
            }
            if self.quiet >= HANG_FRAMES || self.speech.len() >= MAX_LEN {
                self.end_utterance(emit);
            }
        } else if loud {
            self.speaking = true;
            self.loud = 1;
            self.quiet = 0;
            self.speech.extend(self.pre_roll.drain(..));
            self.speech.extend_from_slice(&self.frame);
        } else {
            self.pre_roll.extend(&self.frame);
            let extra = self.pre_roll.len().saturating_sub(PRE_ROLL);
            self.pre_roll.drain(..extra);
        }
        self.frame.clear();
    }

    fn end_utterance(&mut self, emit: &mut impl FnMut(Utterance)) {
        self.speaking = false;
        let mut audio = std::mem::take(&mut self.speech);
        if self.loud >= MIN_LOUD_FRAMES {
            audio.truncate(audio.len() - self.quiet.saturating_sub(TAIL_FRAMES) * FRAME);
            emit(Utterance { audio, end: self.consumed });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seconds of each utterance found in `signal` at 44.1 kHz, fed in mic-sized blocks.
    fn lengths(signal: &[f32]) -> Vec<f32> {
        let mut seg = Segmenter::new(44100.0);
        let mut found = Vec::new();
        for block in signal.chunks(441) {
            seg.push(block, -40.0, |u| found.push(u.audio.len() as f32 / RATE as f32));
        }
        seg.finish(|u| found.push(u.audio.len() as f32 / RATE as f32));
        found
    }

    fn tone(seconds: f32, amp: f32) -> Vec<f32> {
        (0..(seconds * 44100.0) as usize).map(|i| amp * (i as f32 * 0.03).sin()).collect()
    }

    #[test]
    fn one_utterance_per_burst() {
        let mut signal = tone(0.5, 0.0);
        for _ in 0..3 {
            signal.extend(tone(0.4, 0.3));
            signal.extend(tone(0.6, 0.0));
        }
        let found = lengths(&signal);
        println!("{found:.2?}");
        assert_eq!(found.len(), 3);
        // The burst plus pre-roll and tail.
        assert!(found.iter().all(|&s| (0.6..0.8).contains(&s)), "{found:?}");
    }

    #[test]
    fn quiet_noise_and_clicks_are_not_speech() {
        let mut signal = tone(2.0, 0.001);
        signal.extend(tone(0.03, 0.5));
        signal.extend(tone(1.0, 0.001));
        assert!(lengths(&signal).is_empty());
    }

    #[test]
    fn long_speech_is_cut_at_the_limit() {
        let found = lengths(&tone(13.0, 0.3));
        assert_eq!(found.len(), 3);
        assert!((found[0] - 6.0).abs() < 0.2, "{found:?}");
    }
}
