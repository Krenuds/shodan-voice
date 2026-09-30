//! Device handling: mic → engine (in the capture callback) → ring → adaptive resampler → output,
//! and the same processed signal through a second ring to the optional headphone monitor.
//!
//! Running the engine where the audio arrives means every output is only one buffer hop away
//! from the mic. Each output runs on its own clock (and possibly sample rate), so it pulls
//! through a resampler whose ratio is nudged to keep the ring's *minimum* fill just above a
//! small safety margin. That absorbs rate mismatch and clock drift at the lowest delay.

use super::engine::{Engine, RackLink};
use super::module::RackModule;
use crate::dsp::filters::cubic;
use crate::shared::Shared;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, SampleFormat, SizedSample, StreamConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

fn describe(devices: impl Iterator<Item = Device>) -> Vec<DeviceInfo> {
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| id.clone());
            Some(DeviceInfo { id, name })
        })
        .collect()
}

pub fn list_inputs() -> Vec<DeviceInfo> {
    cpal::default_host().input_devices().map(|d| describe(d)).unwrap_or_default()
}

pub fn list_outputs() -> Vec<DeviceInfo> {
    cpal::default_host().output_devices().map(|d| describe(d)).unwrap_or_default()
}

fn find(id: Option<&str>, input: bool) -> Result<Device, String> {
    let host = cpal::default_host();
    let dev = match id {
        Some(id) => host.device_by_id(&id.parse().map_err(|e| format!("bad device id {id}: {e:?}"))?),
        None if input => host.default_input_device(),
        None => host.default_output_device(),
    };
    dev.ok_or_else(|| match (input, id.is_some()) {
        (true, false) => "no microphone found. Plug one in, then press Refresh devices".to_string(),
        (true, true) => "the selected microphone is not connected".to_string(),
        (false, false) => "no output device found".to_string(),
        (false, true) => "the selected output device is not connected".to_string(),
    })
}

/// Stereo fractional-rate reader over a ring of interleaved frames, acting as a jitter buffer.
struct Resampler {
    history: [[f32; 2]; 4],
    frac: f64,
    nominal: f64,
    source_rate: f64,
    correction: f64,
    primed: bool,
    /// Frames per mic callback. The two clocks slowly slip past each other, and when they do
    /// the output can ask a moment before the next block lands, so one block is the floor.
    in_block: Arc<AtomicUsize>,
    /// Extra safety on top of one block (frames); grows after a dropout.
    margin: f64,
    min_headroom: f64,
    window_frames: f64,
    calm_frames: f64,
}

impl Resampler {
    fn new(source_rate: f64, dest_rate: f64, in_block: Arc<AtomicUsize>) -> Self {
        Self {
            history: [[0.0; 2]; 4],
            frac: 0.0,
            nominal: source_rate / dest_rate,
            source_rate,
            correction: 0.0,
            primed: false,
            in_block,
            margin: 0.0015 * source_rate,
            min_headroom: f64::MAX,
            window_frames: 0.0,
            calm_frames: 0.0,
        }
    }

    /// Fill `out` from `rx`. Returns false if the input ran dry (the rest of `out` is silence).
    fn read(&mut self, rx: &mut rtrb::Consumer<f32>, out: &mut [[f32; 2]]) -> bool {
        let sr = self.source_rate;
        let ratio = self.nominal * (1.0 + self.correction);
        let needed = out.len() as f64 * ratio + 2.0;
        let fill = (rx.slots() / 2) as f64;
        let target = self.in_block.load(Ordering::Relaxed) as f64 + self.margin;
        if !self.primed {
            if fill < needed + target {
                out.fill([0.0; 2]);
                return true;
            }
            self.primed = true;
            self.min_headroom = f64::MAX;
            self.window_frames = 0.0;
        }

        // Track the lowest headroom seen; every quarter second steer it towards the margin.
        self.min_headroom = self.min_headroom.min(fill - needed);
        self.window_frames += out.len() as f64 * ratio;
        self.calm_frames += out.len() as f64 * ratio;
        if self.window_frames > 0.25 * sr {
            let excess = self.min_headroom - target;
            if excess > 0.012 * sr {
                // Far too much buffered (startup, stall): drop it now instead of draining slowly.
                let frames = (excess - 0.002 * sr) as usize;
                if let Ok(chunk) = rx.read_chunk((frames * 2).min(rx.slots() / 2 * 2)) {
                    chunk.commit_all();
                }
                self.correction = 0.0;
            } else {
                self.correction = (excess / sr * 2.0).clamp(-0.01, 0.01);
            }
            self.min_headroom = f64::MAX;
            self.window_frames = 0.0;
        }
        if self.calm_frames > 60.0 * sr {
            // A minute without a dropout: try a slightly tighter margin.
            self.margin = (self.margin - 0.00025 * sr).max(0.0015 * sr);
            self.calm_frames = 0.0;
        }

        for (n, frame) in out.iter_mut().enumerate() {
            while self.frac >= 1.0 {
                if rx.slots() < 2 {
                    self.primed = false;
                    self.margin = (self.margin + 0.003 * sr).min(0.03 * sr);
                    self.calm_frames = 0.0;
                    out[n..].fill([0.0; 2]);
                    return false;
                }
                let l = rx.pop().unwrap_or(0.0);
                let r = rx.pop().unwrap_or(0.0);
                self.history.rotate_left(1);
                self.history[3] = [l, r];
                self.frac -= 1.0;
            }
            let t = self.frac as f32;
            let h = &self.history;
            for c in 0..2 {
                frame[c] = cubic(h[0][c], h[1][c], h[2][c], h[3][c], t);
            }
            self.frac += ratio;
        }
        true
    }
}

pub struct AudioSettings {
    pub input: Option<String>,
    pub output: Option<String>,
    /// Headphone monitor: `None` = off, `Some(None)` = default output device.
    pub monitor: Option<Option<String>>,
}

pub struct Running {
    _streams: Vec<cpal::Stream>,
    /// Rate the engine (and plugins) run at: the microphone's rate.
    pub sample_rate: f64,
    pub description: String,
    pub errors: Arc<Mutex<Vec<String>>>,
}

fn build_config(dev: &Device, input: bool) -> Result<(StreamConfig, SampleFormat), String> {
    let sup = if input { dev.default_input_config() } else { dev.default_output_config() }.map_err(|e| e.to_string())?;
    Ok((sup.config(), sup.sample_format()))
}

/// Ask for a small buffer first; fall back to the device default.
fn with_buffer_fallback<S>(cfg: &StreamConfig, mut build: impl FnMut(StreamConfig) -> Result<S, cpal::Error>) -> Result<S, String> {
    let small = StreamConfig { buffer_size: BufferSize::Fixed(128), ..*cfg };
    build(small).or_else(|_| build(*cfg)).map_err(|e| e.to_string())
}

fn error_sink(errors: &Arc<Mutex<Vec<String>>>, what: &'static str) -> impl FnMut(cpal::Error) + Send + 'static {
    let errors = errors.clone();
    move |e| {
        if let Ok(mut v) = errors.lock() {
            v.push(format!("{what}: {e}"));
        }
    }
}

macro_rules! dispatch_format {
    ($fmt:expr, $f:ident($($arg:expr),*)) => {
        match $fmt {
            SampleFormat::F32 => $f::<f32>($($arg),*),
            SampleFormat::I16 => $f::<i16>($($arg),*),
            SampleFormat::I32 => $f::<i32>($($arg),*),
            SampleFormat::U16 => $f::<u16>($($arg),*),
            other => Err(format!("unsupported sample format {other}")),
        }
    };
}

/// Latency meter slot each output reports into.
const MAIN: usize = 0;
const MONITOR: usize = 1;

/// `build` creates the rack's modules once the engine's sample rate (the microphone's) is known.
pub fn start(settings: &AudioSettings, shared: Arc<Shared>, link: Option<RackLink>, build: impl FnOnce(f64) -> Vec<RackModule>) -> Result<Running, String> {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let in_dev = find(settings.input.as_deref(), true)?;
    let out_dev = find(settings.output.as_deref(), false)?;
    let (in_cfg, in_fmt) = build_config(&in_dev, true)?;
    let (out_cfg, out_fmt) = build_config(&out_dev, false)?;
    let in_rate = in_cfg.sample_rate as f64;
    let ring = in_rate as usize * 2;

    // Until the first mic callback reports its real size, assume a 10 ms shared-mode block.
    let in_block = Arc::new(AtomicUsize::new(in_rate as usize / 100));
    let (main_tx, main_rx) = rtrb::RingBuffer::<f32>::new(ring);
    let out_stream = dispatch_format!(out_fmt, build_output(&out_dev, &out_cfg, main_rx, in_rate, in_block.clone(), shared.clone(), MAIN, &errors))?;

    let (monitor_tx, monitor_stream) = match &settings.monitor {
        Some(id) => {
            let dev = find(id.as_deref(), false)?;
            let (cfg, fmt) = build_config(&dev, false)?;
            let (tx, rx) = rtrb::RingBuffer::<f32>::new(ring);
            let stream = dispatch_format!(fmt, build_output(&dev, &cfg, rx, in_rate, in_block.clone(), shared.clone(), MONITOR, &errors))?;
            (Some(tx), Some(stream))
        }
        None => (None, None),
    };

    let input = InputState {
        engine: Engine::new(in_rate as f32, shared.clone(), build(in_rate), link),
        shared: shared.clone(),
        in_block,
        main: main_tx,
        monitor: monitor_tx,
        mono: vec![0.0; 4096],
        l: vec![0.0; 4096],
        r: vec![0.0; 4096],
    };
    let in_stream = dispatch_format!(in_fmt, build_input(&in_dev, &in_cfg, input, &errors))?;

    out_stream.play().map_err(|e| e.to_string())?;
    let mut streams = vec![out_stream];
    if let Some(m) = monitor_stream {
        m.play().map_err(|e| e.to_string())?;
        streams.push(m);
    }
    in_stream.play().map_err(|e| e.to_string())?;
    streams.push(in_stream);

    let name = |d: &Device| d.description().map(|x| x.name().to_string()).unwrap_or_default();
    let description = format!(
        "{} ({} Hz, {} ch) ➡ {} ({} Hz, {} ch)",
        name(&in_dev),
        in_cfg.sample_rate,
        in_cfg.channels,
        name(&out_dev),
        out_cfg.sample_rate,
        out_cfg.channels
    );
    Ok(Running { _streams: streams, sample_rate: in_rate, description, errors })
}

struct InputState {
    engine: Engine,
    shared: Arc<Shared>,
    in_block: Arc<AtomicUsize>,
    main: rtrb::Producer<f32>,
    monitor: Option<rtrb::Producer<f32>>,
    mono: Vec<f32>,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl InputState {
    fn push(tx: &mut rtrb::Producer<f32>, l: &[f32], r: &[f32]) {
        let frames = l.len().min(tx.slots() / 2);
        let Ok(mut w) = tx.write_chunk_uninit(frames * 2) else { return };
        let (a, b) = w.as_mut_slices();
        let interleaved = l[..frames].iter().zip(&r[..frames]).flat_map(|(&x, &y)| [x, y]);
        for (slot, s) in a.iter_mut().chain(b.iter_mut()).zip(interleaved) {
            slot.write(s);
        }
        // SAFETY: exactly frames * 2 slots were written above.
        unsafe { w.commit_all() };
    }

    fn process<T>(&mut self, data: &[T], channels: usize)
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        for block in data.chunks(channels * 4096) {
            let frames = block.len() / channels;
            for (m, frame) in self.mono.iter_mut().zip(block.chunks(channels)) {
                let sum: f32 = frame.iter().map(|&s| <f32 as FromSample<T>>::from_sample_(s)).sum();
                *m = sum / channels as f32;
            }
            self.engine.process(&self.mono[..frames], &mut self.l[..frames], &mut self.r[..frames]);
            Self::push(&mut self.main, &self.l[..frames], &self.r[..frames]);
            if let Some(tx) = self.monitor.as_mut() {
                Self::push(tx, &self.l[..frames], &self.r[..frames]);
            }
        }
    }
}

fn build_input<T>(dev: &Device, cfg: &StreamConfig, state: InputState, errors: &Arc<Mutex<Vec<String>>>) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = cfg.channels as usize;
    let state = Arc::new(Mutex::new(Some(state)));
    with_buffer_fallback(cfg, |c| {
        // The closure may be built twice (buffer fallback); hand the state to whichever runs.
        let slot = state.clone();
        let mut st: Option<InputState> = None;
        dev.build_input_stream::<T, _, _>(
            c,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                if st.is_none() {
                    st = slot.try_lock().ok().and_then(|mut s| s.take());
                }
                let Some(st) = st.as_mut() else { return };
                let ts = info.timestamp();
                st.shared.meters.input_ms.set(ts.callback.duration_since(ts.capture).as_secs_f32() * 1000.0);
                st.in_block.store(data.len() / channels, Ordering::Relaxed);
                st.process(data, channels);
            },
            error_sink(errors, "input"),
            None,
        )
    })
}

fn build_output<T>(
    dev: &Device,
    cfg: &StreamConfig,
    rx: rtrb::Consumer<f32>,
    source_rate: f64,
    in_block: Arc<AtomicUsize>,
    shared: Arc<Shared>,
    meter: usize,
    errors: &Arc<Mutex<Vec<String>>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = cfg.channels as usize;
    let rx = Arc::new(Mutex::new(Some(rx)));
    with_buffer_fallback(cfg, |c| {
        let slot = rx.clone();
        let shared = shared.clone();
        let mut rx: Option<rtrb::Consumer<f32>> = None;
        let mut rs = Resampler::new(source_rate, c.sample_rate as f64, in_block.clone());
        let mut buf = vec![[0.0f32; 2]; 4096];
        dev.build_output_stream::<T, _, _>(
            c,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                if rx.is_none() {
                    rx = slot.try_lock().ok().and_then(|mut s| s.take());
                }
                let Some(rx) = rx.as_mut() else { return };
                let ts = info.timestamp();
                let device_ms = ts.playback.duration_since(ts.callback).as_secs_f32() * 1000.0;
                let buffered_ms = (rx.slots() / 2) as f32 / source_rate as f32 * 1000.0;
                let m = &shared.meters;
                m.output_ms[meter].set(device_ms + buffered_ms);
                for block in data.chunks_mut(channels * 4096) {
                    let frames = block.len() / channels;
                    if !rs.read(rx, &mut buf[..frames]) {
                        m.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    for (frame, &[l, r]) in block.chunks_mut(channels).zip(&buf[..frames]) {
                        match frame {
                            [only] => *only = T::from_sample((l + r) * 0.5),
                            [a, b, rest @ ..] => {
                                *a = T::from_sample(l);
                                *b = T::from_sample(r);
                                for x in rest {
                                    *x = T::from_sample(0.0);
                                }
                            }
                            [] => {}
                        }
                    }
                }
            },
            error_sink(errors, if meter == MAIN { "output" } else { "monitor" }),
            None,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mic delivers 10 ms blocks at 44.1 kHz; output pulls 10 ms blocks at 48 kHz on a clock
    /// that runs 0.05 % fast (a clock-phase wrap every 20 s, far worse than real hardware).
    /// Starting the margin at one mic block means no dropouts at all.
    #[test]
    fn jitter_buffer_settles_low_without_dropouts() {
        let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(88200);
        let mut rs = Resampler::new(44100.0, 48000.0, Arc::new(AtomicUsize::new(441)));
        let mut out = vec![[0.0f32; 2]; 480];
        let (in_period, out_period) = (441.0 / 44100.0, 480.0 / (48000.0 * 1.0005));
        let (mut t_in, mut t_out) = (0.0f64, 0.0037f64);
        let (mut dropouts_late, mut fills) = (0, Vec::new());
        let mut phase = 0.0f32;
        let mut last_dropout = 0.0;
        while t_out < 300.0 {
            if t_in <= t_out {
                for _ in 0..441 {
                    phase += 0.02;
                    let _ = tx.push(phase.sin());
                    let _ = tx.push(phase.sin());
                }
                t_in += in_period;
            } else {
                let ok = rs.read(&mut rx, &mut out);
                if t_out > 1.0 {
                    if !ok {
                        dropouts_late += 1;
                        last_dropout = t_out;
                    }
                    fills.push((rx.slots() / 2) as f64 / 44100.0 * 1000.0);
                }
                t_out += out_period;
            }
        }
        let avg = fills.iter().sum::<f64>() / fills.len() as f64;
        println!("avg buffered after callback {avg:.2} ms, {dropouts_late} dropouts, last at {last_dropout:.0} s");
        assert_eq!(dropouts_late, 0, "last at {last_dropout:.0} s");
        assert!(avg < 12.0, "buffer settled too high: {avg:.2} ms");
    }
}
