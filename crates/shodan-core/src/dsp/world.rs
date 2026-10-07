//! Source-filter re-pitching through WORLD (Morise): F0, spectral envelope and aperiodicity are
//! analysed as separate streams, edited on their own and resynthesised. Offline only: it needs
//! the whole utterance.

/// WORLD analysis hop.
pub const FRAME_MS: f64 = 5.0;

/// How to reshape a voice. Shared by the WORLD and PSOLA paths.
#[derive(Clone, Copy, Debug)]
pub struct Reshape {
    /// Where the median pitch of the utterance lands, in Hz.
    pub f0: f32,
    /// Pull of every frame toward the target pitch: 0 keeps the contour, 1 is a flat monotone.
    pub flatten: f32,
    /// Formant scale: 1 keeps the vocal tract, below 1 makes it bigger.
    pub formant: f32,
    /// Extra aperiodicity above 1 kHz (WORLD only): breath and grit in the excitation.
    pub grit: f32,
}

/// Harvest F0 track, one value per `FRAME_MS`, 0 where unvoiced. Returns (frame times, f0).
pub fn f0_track(x: &[f64], fs: i32) -> (Vec<f64>, Vec<f64>) {
    let mut opt = rsworld_sys::HarvestOption::new();
    opt.f0_floor = 60.0;
    opt.f0_ceil = 500.0;
    opt.frame_period = FRAME_MS;
    rsworld::harvest(&x.to_vec(), fs, &opt)
}

/// The new pitch of each frame: the median maps to `r.f0`, the contour around it shrinks by `flatten`.
pub fn target_f0(f0: &[f64], r: &Reshape) -> Vec<f64> {
    let mut voiced: Vec<f64> = f0.iter().copied().filter(|&f| f > 0.0).map(f64::ln).collect();
    if voiced.is_empty() {
        return f0.to_vec();
    }
    voiced.sort_by(f64::total_cmp);
    let median = voiced[voiced.len() / 2];
    let keep = 1.0 - r.flatten.clamp(0.0, 1.0) as f64;
    let target = (r.f0 as f64).ln();
    f0.iter().map(|&f| if f > 0.0 { (target + keep * (f.ln() - median)).exp() } else { 0.0 }).collect()
}

/// Read `env` at fractional bin `pos`, interpolating in the log domain.
fn log_interp(env: &[f64], pos: f64) -> f64 {
    let last = env.len() - 1;
    let pos = pos.min(last as f64);
    let i = pos.floor() as usize;
    let j = (i + 1).min(last);
    let t = pos - i as f64;
    (env[i].max(1e-30).ln() * (1.0 - t) + env[j].max(1e-30).ln() * t).exp()
}

pub fn reshape(x: &[f32], sr: f32, r: &Reshape) -> Vec<f32> {
    let fs = sr as i32;
    let x: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    let (times, f0) = f0_track(&x, fs);
    let mut ct = rsworld_sys::CheapTrickOption::new(fs);
    let sp = rsworld::cheaptrick(&x, fs, &times, &f0, &mut ct);
    let ap = rsworld::d4c(&x, fs, &times, &f0, &rsworld_sys::D4COption::new());

    let new_f0 = target_f0(&f0, r);
    let bins = sp[0].len();
    let hz_per_bin = sr as f64 / (2.0 * (bins - 1) as f64);
    let warp = r.formant.clamp(0.5, 2.0) as f64;
    // Only voiced frames are warped: fricatives keep their place, so /s/ stays an /s/.
    let sp: Vec<Vec<f64>> = sp
        .iter()
        .zip(&f0)
        .map(|(env, &f)| if f > 0.0 && warp != 1.0 { (0..bins).map(|k| log_interp(env, k as f64 / warp)).collect() } else { env.clone() })
        .collect();
    let grit = r.grit.clamp(0.0, 1.0) as f64;
    let ap: Vec<Vec<f64>> = ap
        .iter()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(k, &v)| {
                    let w = ((k as f64 * hz_per_bin - 1000.0) / 3000.0).clamp(0.0, 1.0);
                    v.clamp(1e-3, 1.0).powf(1.0 - grit * w)
                })
                .collect()
        })
        .collect();

    let y = rsworld::synthesis(&new_f0, &sp, &ap, FRAME_MS, fs);
    let mut out: Vec<f32> = y.iter().map(|&v| v as f32).collect();
    out.resize(x.len(), 0.0);
    out
}
