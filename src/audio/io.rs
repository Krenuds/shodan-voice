//! Device handling: mic → ring buffer → adaptive resampler → engine → output (+ optional monitor).
//!
//! The input and output devices run on separate clocks (and possibly different sample rates),
//! so the output side pulls input through a resampler whose ratio is nudged to keep the ring
//! buffer near a small target fill. That absorbs both rate mismatch and clock drift.

use super::engine::{Engine, SlotLink};
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

/// Fractional-rate reader over a ring buffer of interleaved `C`-channel frames.
struct Resampler<const C: usize> {
    history: [[f32; C]; 4],
    frac: f64,
    nominal: f64,
    source_rate: f64,
    avg_fill: f64,
    primed: bool,
}

impl<const C: usize> Resampler<C> {
    fn new(source_rate: f64, dest_rate: f64) -> Self {
        Self { history: [[0.0; C]; 4], frac: 0.0, nominal: source_rate / dest_rate, source_rate, avg_fill: 0.0, primed: false }
    }

    /// Fill `out` with frames resampled from `rx`. `target` is the desired fill in frames.
    /// Returns false if the input ran dry (the rest of `out` is silence).
    fn read(&mut self, rx: &mut rtrb::Consumer<f32>, out: &mut [[f32; C]], target: f64) -> bool {
        let fill = (rx.slots() / C) as f64;
        if !self.primed {
            if fill < target {
                out.fill([0.0; C]);
                return true;
            }
            self.primed = true;
            self.avg_fill = fill;
        }
        if fill > target * 4.0 + 2400.0 {
            // Way behind (e.g. after a stall): drop down to the target instead of slowly draining.
            let drop = ((fill - target) as usize) * C;
            if let Ok(chunk) = rx.read_chunk(drop) {
                chunk.commit_all();
            }
            self.avg_fill = target;
        }
        self.avg_fill += (fill - self.avg_fill) * 0.02;
        let correction = ((self.avg_fill - target) / self.source_rate * 0.4).clamp(-0.005, 0.005);
        let ratio = self.nominal * (1.0 + correction);

        for (n, frame) in out.iter_mut().enumerate() {
            while self.frac >= 1.0 {
                if rx.slots() < C {
                    self.primed = false;
                    out[n..].fill([0.0; C]);
                    return false;
                }
                let mut f = [0.0; C];
                for s in &mut f {
                    *s = rx.pop().unwrap_or(0.0);
                }
                self.history.rotate_left(1);
                self.history[3] = f;
                self.frac -= 1.0;
            }
            let t = self.frac as f32;
            let h = &self.history;
            for c in 0..C {
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
    pub sample_rate: f64,
    pub description: String,
    pub errors: Arc<Mutex<Vec<String>>>,
}

fn build_config(dev: &Device, input: bool) -> Result<(StreamConfig, SampleFormat), String> {
    let sup = if input { dev.default_input_config() } else { dev.default_output_config() }.map_err(|e| e.to_string())?;
    Ok((sup.config(), sup.sample_format()))
}

/// Try a small fixed buffer first for low latency; fall back to the device default.
fn with_buffer_fallback<S>(cfg: &StreamConfig, mut build: impl FnMut(StreamConfig) -> Result<S, cpal::Error>) -> Result<S, String> {
    let small = StreamConfig { buffer_size: BufferSize::Fixed(256), ..*cfg };
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

pub fn start(settings: &AudioSettings, shared: Arc<Shared>, link: Option<SlotLink>, seed: u64) -> Result<Running, String> {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let in_dev = find(settings.input.as_deref(), true)?;
    let out_dev = find(settings.output.as_deref(), false)?;
    let (in_cfg, in_fmt) = build_config(&in_dev, true)?;
    let (out_cfg, out_fmt) = build_config(&out_dev, false)?;
    let in_rate = in_cfg.sample_rate as f64;
    let out_rate = out_cfg.sample_rate as f64;

    let (in_tx, in_rx) = rtrb::RingBuffer::<f32>::new(in_rate as usize);
    let chunk = Arc::new(AtomicUsize::new(480));
    let in_stream = dispatch_format!(in_fmt, build_input(&in_dev, &in_cfg, in_tx, chunk.clone(), &errors))?;

    let monitor = match &settings.monitor {
        Some(id) => {
            let dev = find(id.as_deref(), false)?;
            let (cfg, fmt) = build_config(&dev, false)?;
            let (tx, rx) = rtrb::RingBuffer::<f32>::new(out_rate as usize * 2);
            let stream = dispatch_format!(fmt, build_monitor(&dev, &cfg, rx, out_rate, &errors))?;
            Some((stream, tx))
        }
        None => None,
    };
    let (monitor_stream, monitor_tx) = match monitor {
        Some((s, tx)) => (Some(s), Some(tx)),
        None => (None, None),
    };

    let engine = Engine::new(out_rate as f32, shared.clone(), seed, link);
    let out = OutputState {
        engine,
        shared,
        rx: in_rx,
        resampler: Resampler::new(in_rate, out_rate),
        chunk,
        in_rate,
        monitor: monitor_tx,
        mono: vec![[0.0]; 8192],
        l: vec![0.0; 8192],
        r: vec![0.0; 8192],
    };
    let out_stream = dispatch_format!(out_fmt, build_output(&out_dev, &out_cfg, out, &errors))?;

    in_stream.play().map_err(|e| e.to_string())?;
    out_stream.play().map_err(|e| e.to_string())?;
    let mut streams = vec![in_stream, out_stream];
    if let Some(m) = monitor_stream {
        m.play().map_err(|e| e.to_string())?;
        streams.push(m);
    }
    let name = |d: &Device| d.description().map(|x| x.name().to_string()).unwrap_or_default();
    let description = format!(
        "{} ({} Hz, {} ch) → {} ({} Hz, {} ch)",
        name(&in_dev),
        in_cfg.sample_rate,
        in_cfg.channels,
        name(&out_dev),
        out_cfg.sample_rate,
        out_cfg.channels
    );
    Ok(Running { _streams: streams, sample_rate: out_rate, description, errors })
}

fn build_input<T>(dev: &Device, cfg: &StreamConfig, tx: rtrb::Producer<f32>, chunk: Arc<AtomicUsize>, errors: &Arc<Mutex<Vec<String>>>) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = cfg.channels as usize;
    let tx = Arc::new(Mutex::new(Some(tx)));
    with_buffer_fallback(cfg, |c| {
        // The closure may be built twice (buffer fallback); hand the producer to whichever runs.
        let tx_slot = tx.clone();
        let chunk = chunk.clone();
        let mut tx: Option<rtrb::Producer<f32>> = None;
        dev.build_input_stream::<T, _, _>(
            c,
            move |data: &[T], _| {
                if tx.is_none() {
                    tx = tx_slot.try_lock().ok().and_then(|mut s| s.take());
                }
                let Some(tx) = tx.as_mut() else { return };
                let frames = data.len() / channels;
                chunk.fetch_max(frames, Ordering::Relaxed);
                let Ok(mut w) = tx.write_chunk_uninit(frames.min(tx.slots())) else { return };
                let (a, b) = w.as_mut_slices();
                for (slot, frame) in a.iter_mut().chain(b.iter_mut()).zip(data.chunks(channels)) {
                    let sum: f32 = frame.iter().map(|&s| <f32 as FromSample<T>>::from_sample_(s)).sum();
                    slot.write(sum / channels as f32);
                }
                unsafe { w.commit_all() };
            },
            error_sink(errors, "input"),
            None,
        )
    })
}

struct OutputState {
    engine: Engine,
    shared: Arc<Shared>,
    rx: rtrb::Consumer<f32>,
    resampler: Resampler<1>,
    chunk: Arc<AtomicUsize>,
    in_rate: f64,
    monitor: Option<rtrb::Producer<f32>>,
    mono: Vec<[f32; 1]>,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl OutputState {
    fn render(&mut self, frames: usize) {
        let target = (self.chunk.load(Ordering::Relaxed) as f64 * 1.5 + self.in_rate * 0.003).max(self.in_rate * 0.006);
        if !self.resampler.read(&mut self.rx, &mut self.mono[..frames], target) {
            self.shared.meters.underruns.fetch_add(1, Ordering::Relaxed);
        }
        let mono: &[f32] = self.mono[..frames].as_flattened();
        self.engine.process(mono, &mut self.l[..frames], &mut self.r[..frames]);
        if let Some(tx) = self.monitor.as_mut()
            && tx.slots() >= frames * 2 {
                for i in 0..frames {
                    let _ = tx.push(self.l[i]);
                    let _ = tx.push(self.r[i]);
                }
            }
    }
}

fn build_output<T>(dev: &Device, cfg: &StreamConfig, state: OutputState, errors: &Arc<Mutex<Vec<String>>>) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = cfg.channels as usize;
    let state = Arc::new(Mutex::new(Some(state)));
    with_buffer_fallback(cfg, |c| {
        let slot = state.clone();
        let mut st: Option<OutputState> = None;
        dev.build_output_stream::<T, _, _>(
            c,
            move |data: &mut [T], _| {
                if st.is_none() {
                    st = slot.try_lock().ok().and_then(|mut s| s.take());
                }
                let Some(st) = st.as_mut() else { return };
                for block in data.chunks_mut(channels * 4096) {
                    let frames = block.len() / channels;
                    st.render(frames);
                    for (i, frame) in block.chunks_mut(channels).enumerate() {
                        let (l, r) = (st.l[i], st.r[i]);
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
            error_sink(errors, "output"),
            None,
        )
    })
}

fn build_monitor<T>(dev: &Device, cfg: &StreamConfig, rx: rtrb::Consumer<f32>, source_rate: f64, errors: &Arc<Mutex<Vec<String>>>) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = cfg.channels as usize;
    let rx = Arc::new(Mutex::new(Some(rx)));
    let target = source_rate * 0.02;
    with_buffer_fallback(cfg, |c| {
        let slot = rx.clone();
        let mut rx: Option<rtrb::Consumer<f32>> = None;
        let mut rs = Resampler::<2>::new(source_rate, c.sample_rate as f64);
        let mut buf = vec![[0.0f32; 2]; 8192];
        dev.build_output_stream::<T, _, _>(
            c,
            move |data: &mut [T], _| {
                if rx.is_none() {
                    rx = slot.try_lock().ok().and_then(|mut s| s.take());
                }
                let Some(rx) = rx.as_mut() else { return };
                for block in data.chunks_mut(channels * 8192) {
                    let frames = block.len() / channels;
                    rs.read(rx, &mut buf[..frames], target);
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
            error_sink(errors, "monitor"),
            None,
        )
    })
}
