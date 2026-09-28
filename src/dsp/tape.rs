//! The "tape": a history buffer with a moving read head. Stutters, reversals and slow-downs
//! all move the head behind the live signal; afterwards it either skips back or rushes back
//! at a raised tape speed, which gives SHODAN's sped-up bursts.

use super::filters::DelayLine;
use std::f32::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glitch {
    /// Replay the fragment starting at the onset `repeats` extra times.
    Stutter { slice: f64, repeats: u32 },
    /// Play the fragment normally, then backwards, then carry on.
    Reverse { slice: f64 },
    /// Drag the tape at `rate` (< 1) for `len` samples.
    Warp { rate: f64, len: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Normal,
    Stutter { start: f64, end: f64, remaining: u32 },
    Reverse { start: f64, end: f64, backward: bool },
    Warp { rate: f64, remaining: u32 },
}

#[derive(Clone, Copy)]
struct Head {
    pos: f64,
    rate: f64,
}

pub struct Tape {
    line: DelayLine,
    head: Head,
    fading: Head,
    fade_len: u32,
    fade_left: u32,
    state: State,
    /// Minimum distance behind the write position (room for cubic interpolation).
    base: f64,
    pre_roll: f64,
    max_lag: f64,
}

impl Tape {
    pub fn new(sr: f32) -> Self {
        Self {
            line: DelayLine::new((sr * 4.0) as usize),
            head: Head { pos: 0.0, rate: 1.0 },
            fading: Head { pos: 0.0, rate: 1.0 },
            fade_len: (0.004 * sr) as u32,
            fade_left: 0,
            state: State::Normal,
            base: 4.0,
            pre_roll: (0.006 * sr) as f64,
            max_lag: (1.6 * sr) as f64,
        }
    }

    fn live(&self) -> f64 {
        self.line.write as f64 - self.base
    }

    /// How far (in samples) the read head is behind live.
    pub fn lag(&self) -> f64 {
        (self.live() - self.head.pos).max(0.0)
    }

    pub fn busy(&self) -> bool {
        self.state != State::Normal
    }

    /// Start a glitch at the current write position. Ignored while another glitch runs.
    pub fn trigger(&mut self, g: Glitch) {
        if self.busy() {
            return;
        }
        let start = (self.line.write as f64 - 1.0 - self.pre_roll).max(self.head.pos);
        self.state = match g {
            Glitch::Stutter { slice, repeats } => State::Stutter { start, end: start + slice, remaining: repeats },
            Glitch::Reverse { slice } => State::Reverse { start, end: start + slice, backward: false },
            Glitch::Warp { rate, len } => State::Warp { rate, remaining: len },
        };
    }

    fn jump(&mut self, pos: f64, rate: f64) {
        self.fading = self.head;
        self.head = Head { pos, rate };
        self.fade_left = self.fade_len;
    }

    /// `catch_up` 0 = skip back to live, 1 = fastest tape rush.
    #[inline]
    pub fn process(&mut self, x: f32, catch_up: f32) -> f32 {
        self.line.push(x);
        let live = self.live();
        if self.line.write < 8 {
            self.head.pos = live.max(0.0);
            return 0.0;
        }

        // Read.
        let mut y = self.line.read_abs(self.head.pos);
        if self.fade_left > 0 {
            let t = self.fade_left as f32 / self.fade_len as f32;
            let old = self.line.read_abs(self.fading.pos.min(live));
            y = y * ((1.0 - t) * FRAC_PI_2).sin() + old * ((1.0 - t) * FRAC_PI_2).cos();
            self.fading.pos += self.fading.rate;
            self.fade_left -= 1;
        }

        // Advance.
        self.head.rate = 1.0;
        match self.state {
            State::Normal => {
                let lag = live - self.head.pos;
                if lag > self.max_lag || (lag > 1.0 && catch_up < 0.02) {
                    self.jump(live, 1.0);
                } else if lag > 0.0 {
                    self.head.rate = 1.1 + 0.7 * catch_up as f64;
                }
            }
            State::Stutter { start, end, remaining } => {
                if self.head.pos + 1.0 >= end {
                    if remaining > 0 {
                        let back = self.head.pos + 1.0 - (end - start);
                        self.state = State::Stutter { start, end, remaining: remaining - 1 };
                        self.jump(back, 1.0);
                        self.head.pos -= 1.0; // advanced below
                    } else {
                        self.state = State::Normal;
                    }
                }
            }
            State::Reverse { start, end, backward } => {
                if !backward && self.head.pos + 1.0 >= end {
                    self.state = State::Reverse { start, end, backward: true };
                    self.jump(self.head.pos, -1.0);
                } else if backward {
                    self.head.rate = -1.0;
                    if self.head.pos - 1.0 <= start {
                        self.state = State::Normal;
                        self.jump(end, 1.0);
                        self.head.pos -= 1.0;
                    }
                }
            }
            State::Warp { rate, remaining } => {
                self.head.rate = rate;
                self.state = if remaining > 1 { State::Warp { rate, remaining: remaining - 1 } } else { State::Normal };
            }
        }
        if self.live() - self.head.pos > self.max_lag * 1.2 {
            // Never outrun the buffer, whatever the glitch wanted.
            self.state = State::Normal;
            self.jump(live, 1.0);
        }
        self.head.pos = (self.head.pos + self.head.rate).min(live + 1.0);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    /// A ramp makes the read position visible in the output.
    fn ramp(n: usize) -> f32 {
        n as f32 * 1e-4
    }

    #[test]
    fn tracks_live_input_when_idle() {
        let mut tape = Tape::new(SR);
        let mut last = 0.0;
        for n in 0..10_000 {
            last = tape.process(ramp(n), 0.5);
        }
        assert!((last - ramp(10_000 - 5)).abs() < 1e-3, "{last}");
        assert_eq!(tape.lag(), 0.0);
    }

    #[test]
    fn stutter_repeats_slice_then_catches_up() {
        let mut tape = Tape::new(SR);
        let slice = (0.08 * SR) as f64;
        for n in 0..10_000 {
            tape.process(ramp(n), 0.5);
        }
        tape.trigger(Glitch::Stutter { slice, repeats: 2 });
        // Count backward jumps of the read head: one per repeat.
        let mut jumps = 0;
        let mut prev = tape.lag();
        let mut max_lag: f64 = 0.0;
        for n in 10_000..10_000 + SR as usize * 3 {
            let y = tape.process(ramp(n), 0.5);
            assert!(y.is_finite());
            if tape.lag() > prev + slice / 2.0 {
                jumps += 1;
            }
            prev = tape.lag();
            max_lag = max_lag.max(prev);
        }
        assert_eq!(jumps, 2);
        assert!((max_lag - 2.0 * slice).abs() < 50.0, "max lag {max_lag}");
        assert!(tape.lag() < 1.0, "did not catch up: {}", tape.lag());
    }

    #[test]
    fn skip_mode_returns_to_live_immediately() {
        let mut tape = Tape::new(SR);
        for n in 0..10_000 {
            tape.process(ramp(n), 0.0);
        }
        tape.trigger(Glitch::Reverse { slice: 2000.0 });
        let mut lag_after = f64::MAX;
        for n in 10_000..20_000 {
            tape.process(ramp(n), 0.0);
            if !tape.busy() {
                lag_after = tape.lag();
            }
        }
        assert!(lag_after < 1.0);
    }

    #[test]
    fn lag_never_exceeds_cap() {
        let mut tape = Tape::new(SR);
        let cap = 1.6 * SR as f64 * 1.2;
        for n in 0..(SR as usize * 20) {
            if n % 5000 == 0 {
                tape.trigger(Glitch::Stutter { slice: 0.25 * SR as f64, repeats: 5 });
            }
            let y = tape.process(ramp(n % 1000), 1.0);
            assert!(y.is_finite());
            assert!(tape.lag() <= cap + 2.0, "lag {}", tape.lag());
        }
    }
}
