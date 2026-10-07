//! TD-PSOLA: grains cut two periods wide around each glottal pulse and overlap-added at the new
//! period. Formants stay put by construction; resampling each grain moves them on purpose.
//! Offline only: it needs the whole utterance.

use super::world::{self, FRAME_MS, Reshape};

/// Grain spacing in unvoiced stretches, where there is no period to follow.
const UNVOICED_MS: f32 = 5.0;

struct Mark {
    at: usize,
    /// Half the grain width: one local period when voiced.
    half: usize,
    voiced: bool,
}

/// Analysis marks: one per glottal pulse (the waveform peak near each expected period) where
/// voiced, a fixed grid where not.
fn marks(x: &[f32], f0: &[f64], hop: f64, sr: f32) -> Vec<Mark> {
    let uv = (UNVOICED_MS * 0.001 * sr) as usize;
    let f0_at = |t: usize| f0[((t as f64 / hop).round() as usize).min(f0.len() - 1)];
    let mut out: Vec<Mark> = Vec::new();
    let mut t = 0;
    while t < x.len() {
        let f = f0_at(t);
        if f <= 0.0 {
            out.push(Mark { at: t, half: uv, voiced: false });
            t += uv;
            continue;
        }
        let period = (sr as f64 / f) as usize;
        let (lo, hi) = match out.last() {
            Some(m) if m.voiced => (t.saturating_sub(period / 4), t + period / 4),
            _ => (t, t + period),
        };
        let hi = hi.min(x.len());
        if lo >= hi {
            break;
        }
        let peak = (lo..hi).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(t);
        out.push(Mark { at: peak, half: period, voiced: true });
        t = peak + period;
    }
    out
}

pub fn psola(x: &[f32], sr: f32, r: &Reshape) -> Vec<f32> {
    let xd: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    let (_, f0) = world::f0_track(&xd, sr as i32);
    let new_f0 = world::target_f0(&f0, r);
    let hop = FRAME_MS * 0.001 * sr as f64;
    let marks = marks(x, &f0, hop, sr);
    if marks.is_empty() {
        return x.to_vec();
    }
    let new_at = |t: usize| new_f0[((t as f64 / hop).round() as usize).min(new_f0.len() - 1)];
    let warp = r.formant.clamp(0.5, 2.0);

    let mut out = vec![0.0f32; x.len()];
    let mut m = 0;
    let mut ts = marks[0].at;
    while ts < x.len() {
        while m + 1 < marks.len() && marks[m + 1].at.abs_diff(ts) <= marks[m].at.abs_diff(ts) {
            m += 1;
        }
        let mark = &marks[m];
        let (step, scale) = if mark.voiced { ((sr as f64 / new_at(mark.at).max(30.0)) as usize, warp) } else { (mark.half, 1.0) };
        // The grain, read `scale` times faster than it was recorded, centred on `ts`.
        let half_out = (mark.half as f32 / scale) as isize;
        for k in -half_out..half_out {
            let o = ts as isize + k;
            let src = mark.at as f32 + k as f32 * scale;
            if o < 0 || o as usize >= out.len() || src < 0.0 || src >= (x.len() - 1) as f32 {
                continue;
            }
            let i = src as usize;
            let s = x[i] + (x[i + 1] - x[i]) * (src - i as f32);
            let w = 0.5 + 0.5 * (std::f32::consts::PI * k as f32 / half_out as f32).cos();
            out[o as usize] += w * s;
        }
        ts += step.max(1);
    }
    out
}
