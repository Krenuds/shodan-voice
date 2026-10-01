//! Timing trace for chasing stalls: which stage of mic → recogniser → robot → output stops, and
//! when. The GUI writes `trace.log` in the config folder, new each run: a summary line every
//! `SUMMARY` and events (window in front, recogniser passes, words) as they happen.
//!
//! The audio threads only touch counters here; the file is written by the other threads.

use crate::shared::AtomicF32;
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const SUMMARY: Duration = Duration::from_millis(250);

/// One audio callback: how often it ran and the longest wait between two runs.
#[derive(Default)]
pub struct Stage {
    calls: AtomicU32,
    frames: AtomicU32,
    last_us: AtomicU64,
    max_gap_us: AtomicU64,
}

impl Stage {
    /// Safe on the audio thread.
    pub fn tick(&self, epoch: Instant, frames: usize) {
        let now = epoch.elapsed().as_micros() as u64;
        let last = self.last_us.swap(now, Ordering::Relaxed);
        if last != 0 {
            self.max_gap_us.fetch_max(now.saturating_sub(last), Ordering::Relaxed);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.frames.fetch_add(frames as u32, Ordering::Relaxed);
    }

    /// Calls, frames and longest gap (ms) since the last take.
    fn take(&self) -> (u32, u32, f32) {
        let calls = self.calls.swap(0, Ordering::Relaxed);
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let gap = self.max_gap_us.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        (calls, frames, gap)
    }
}

pub struct Trace {
    pub epoch: Instant,
    pub input: Stage,
    /// Main output, monitor.
    pub output: [Stage; 2],
    /// Mic samples TitoBot could not hand to the recogniser (its ring was full).
    pub tap_dropped: AtomicU32,
    /// Words TitoBot took off the bus, and words it started to play (with the last one).
    pub words_taken: AtomicU32,
    pub words_played: AtomicU32,
    pub last_played: AtomicU32,
    /// Mic audio waiting for the recogniser, ms, as the audio thread sees it after each push.
    pub speech_backlog_ms: AtomicF32,
    pub gui_frames: AtomicU32,
    file: Mutex<Option<BufWriter<File>>>,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            input: Stage::default(),
            output: Default::default(),
            tap_dropped: AtomicU32::new(0),
            words_taken: AtomicU32::new(0),
            words_played: AtomicU32::new(0),
            last_played: AtomicU32::new(u32::MAX),
            speech_backlog_ms: AtomicF32::default(),
            gui_frames: AtomicU32::new(0),
            file: Mutex::new(None),
        }
    }
}

impl Trace {
    /// Start writing to `path`, replacing what the last run wrote.
    pub fn open(&self, path: &Path) {
        if let Ok(file) = File::create(path)
            && let Ok(mut f) = self.file.lock()
        {
            *f = Some(BufWriter::new(file));
        }
    }

    /// Log a line. Not for the audio thread: it locks and allocates.
    pub fn event(&self, args: fmt::Arguments) {
        let t = self.epoch.elapsed().as_secs_f64();
        if let Ok(mut f) = self.file.lock()
            && let Some(f) = f.as_mut()
        {
            let _ = writeln!(f, "{t:9.3}  {args}");
            let _ = f.flush();
        }
    }

    /// The summary line, every `SUMMARY`: callbacks and their longest gap per stage, what
    /// TitoBot did, how far behind the recogniser is, GUI frames.
    pub fn summary(&self, underruns: u32) {
        let (ic, _, ig) = self.input.take();
        let (oc, _, og) = self.output[0].take();
        let (mc, _, mg) = self.output[1].take();
        let played = self.words_played.swap(0, Ordering::Relaxed);
        let last = self.last_played.load(Ordering::Relaxed);
        let last = crate::lexicon::WORDS.get(last as usize).map_or("-", |w| w.text);
        self.event(format_args!(
            "in {ic:3} gap {ig:6.1}  out {oc:3} gap {og:6.1}  mon {mc:3} gap {mg:6.1}  under {underruns}  \
             backlog {:6.0} ms  dropped {}  taken {}  played {played} (last {last})  gui {}",
            self.speech_backlog_ms.get(),
            self.tap_dropped.swap(0, Ordering::Relaxed),
            self.words_taken.swap(0, Ordering::Relaxed),
            self.gui_frames.swap(0, Ordering::Relaxed),
        ));
    }
}

/// Title of the window in front, to see when the user alt-tabs.
pub fn foreground_title() -> String {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(GetForegroundWindow(), buf.as_mut_ptr(), buf.len() as i32);
        return String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    }
    #[allow(unreachable_code)]
    String::new()
}
