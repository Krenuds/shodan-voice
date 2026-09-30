//! A loaded CLAP plugin: the main-thread instance plus the audio-thread processor.

use eframe::egui;
use super::host::{HostMainThread, HostShared, ShodanHost};
use super::window::PluginWindow;
use crate::audio::module::{Module, ModuleCtx};
use clack_extensions::audio_ports::{AudioPortInfoBuffer, AudioPortFlags, PluginAudioPorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags};
use clack_host::events::event_types::ParamValueEvent;
use clack_host::prelude::*;
use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub const MAX_FRAMES: usize = crate::audio::module::MAX_BLOCK;

#[derive(Clone, Debug)]
pub struct ParamMeta {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub stepped: bool,
}

#[derive(Clone, Copy, Debug)]
struct PortLayout {
    /// Channel count of each port.
    channels: [usize; 8],
    count: usize,
    main: Option<usize>,
}

impl PortLayout {
    fn read(instance: &mut PluginInstance<ShodanHost>, input: bool) -> Self {
        let mut layout = PortLayout { channels: [0; 8], count: 0, main: None };
        let handle = instance.plugin_handle();
        let Some(ports) = handle.get_extension::<PluginAudioPorts>() else {
            // No audio-ports extension: assume one stereo port each way.
            layout.channels[0] = 2;
            layout.count = 1;
            layout.main = Some(0);
            return layout;
        };
        let mut buf = AudioPortInfoBuffer::new();
        for i in 0..ports.count(&handle, input).min(8) {
            let Some(info) = ports.get(&handle, i, input, &mut buf) else { continue };
            let idx = layout.count;
            layout.channels[idx] = info.channel_count as usize;
            if info.flags.contains(AudioPortFlags::IS_MAIN) || layout.main.is_none() {
                layout.main = Some(idx);
            }
            layout.count += 1;
        }
        layout
    }
}

pub struct LoadedPlugin {
    pub instance: PluginInstance<ShodanHost>,
    pub name: String,
    pub bundle: std::path::PathBuf,
    pub plugin_id: String,
    pub params: Vec<ParamMeta>,
    param_tx: Option<rtrb::Producer<(u32, f64)>>,
    /// Values set before the plugin was activated; sent with the first process call.
    queued: Vec<(u32, f64)>,
    /// Values just set from the GUI, which the plugin only reports back once the audio thread has
    /// passed them on (never, while it isn't running). Shown instead, so a drag doesn't snap back.
    recent: Vec<(u32, f64, Instant)>,
    inputs: PortLayout,
    outputs: PortLayout,
    pub window: Option<PluginWindow>,
}

impl LoadedPlugin {
    pub fn load(bundle: &Path, plugin_id: &str, name: &str, repaint: egui::Context) -> Result<Self, String> {
        // SAFETY: loading a CLAP bundle executes its entry code; that is inherent to hosting.
        let entry = unsafe { PluginEntry::load(bundle) }.map_err(|e| format!("failed to load {}: {e}", bundle.display()))?;
        let info = HostInfo::new("SHODAN Voice", "shodan-voice", "https://github.com/", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
        let id = CString::new(plugin_id).map_err(|e| e.to_string())?;
        let mut instance = PluginInstance::<ShodanHost>::new(|_| HostShared::new(repaint), |shared| HostMainThread::new(shared), &entry, &id, &info)
            .map_err(|e| format!("failed to create {name}: {e}"))?;

        let params = read_params(&mut instance);
        let inputs = PortLayout::read(&mut instance, true);
        let outputs = PortLayout::read(&mut instance, false);
        if outputs.main.is_none() {
            return Err(format!("{name} has no audio output"));
        }
        Ok(Self {
            instance,
            name: name.to_string(),
            bundle: bundle.to_path_buf(),
            plugin_id: plugin_id.to_string(),
            params,
            param_tx: None,
            queued: Vec::new(),
            recent: Vec::new(),
            inputs,
            outputs,
            window: None,
        })
    }

    /// Activate at the engine's sample rate and return the processor for the audio thread.
    pub fn activate(&mut self, sample_rate: f64) -> Result<Box<dyn Module>, String> {
        if self.instance.is_active() {
            self.instance.try_deactivate().map_err(|e| e.to_string())?;
        }
        let config = PluginAudioConfiguration { sample_rate, min_frames_count: 1, max_frames_count: MAX_FRAMES as u32 };
        let stopped = self.instance.activate(|_, _| (), config).map_err(|e| format!("{} failed to activate: {e}", self.name))?;
        let (mut tx, rx) = rtrb::RingBuffer::new(4096);
        for v in self.queued.drain(..) {
            let _ = tx.push(v);
        }
        self.param_tx = Some(tx);
        Ok(Box::new(ClapProcessor::new(PluginAudioProcessor::Stopped(stopped), self.inputs, self.outputs, rx)))
    }

    /// Deactivate once the audio thread has given the processor back (or the stream is gone).
    pub fn deactivate(&mut self) -> bool {
        if !self.instance.is_active() {
            return true;
        }
        self.instance.try_deactivate().is_ok()
    }

    pub fn get_value(&mut self, id: u32) -> Option<f64> {
        if let Some(&(_, v, _)) = self.recent.iter().find(|r| r.0 == id) {
            return Some(v);
        }
        let params = self.instance.access_handler(|h| h.params.get())?;
        params.get_value(&self.instance.plugin_handle(), ClapId::new(id))
    }

    pub fn value_text(&mut self, id: u32, value: f64) -> Option<String> {
        let params = self.instance.access_handler(|h| h.params.get())?;
        let mut buf = [0u8; 128];
        let text = params.value_to_text(&self.instance.plugin_handle(), ClapId::new(id), value, &mut buf).ok()?;
        let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
        Some(String::from_utf8_lossy(&text[..end]).into_owned())
    }

    pub fn set_value(&mut self, id: u32, value: f64) {
        self.recent.retain(|r| r.0 != id);
        self.recent.push((id, value, Instant::now()));
        match self.param_tx.as_mut() {
            Some(tx) => {
                let _ = tx.push((id, value));
            }
            None => {
                self.queued.retain(|q| q.0 != id);
                self.queued.push((id, value));
            }
        }
    }

    /// Service plugin requests; call once per GUI frame.
    pub fn idle(&mut self) {
        // While running, the plugin has the value by now; stopped, only `queued` has it.
        if self.param_tx.is_some() {
            self.recent.retain(|r| r.2.elapsed() < Duration::from_millis(250));
        }
        let (callback, closed, resize) = self.instance.access_shared_handler(|s| {
            (
                s.callback_requested.swap(false, Ordering::AcqRel),
                s.gui_closed.swap(false, Ordering::AcqRel),
                s.resize_request.lock().ok().and_then(|mut r| r.take()),
            )
        });
        if callback {
            self.instance.call_on_main_thread_callback();
        }
        if let Some(w) = self.window.as_mut() {
            if let Some(size) = resize {
                w.resize(size.width, size.height);
            }
            if closed || w.close_requested() {
                self.close_editor();
            }
        }
    }

    pub fn has_editor(&mut self) -> bool {
        self.instance.access_handler(|h| h.gui.get()).is_some()
    }

    pub fn open_editor(&mut self) -> Result<(), String> {
        if self.window.is_some() {
            return Ok(());
        }
        let gui = self.instance.access_handler(|h| h.gui.get()).ok_or("plugin has no editor")?;
        let handle = self.instance.plugin_handle();
        self.window = Some(PluginWindow::open(gui, &handle, &self.name)?);
        Ok(())
    }

    pub fn close_editor(&mut self) {
        if let Some(w) = self.window.take() {
            if let Some(gui) = self.instance.access_handler(|h| h.gui.get()) {
                gui.destroy(&self.instance.plugin_handle());
            }
            drop(w);
        }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        self.close_editor();
        self.deactivate();
    }
}

fn read_params(instance: &mut PluginInstance<ShodanHost>) -> Vec<ParamMeta> {
    let Some(params) = instance.access_handler(|h| h.params.get()) else { return Vec::new() };
    let handle = instance.plugin_handle();
    let mut buf = ParamInfoBuffer::new();
    let text = |b: &[u8]| {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    };
    (0..params.count(&handle))
        .filter_map(|i| {
            let info = params.get_info(&handle, i, &mut buf)?;
            if info.flags.intersects(ParamInfoFlags::IS_HIDDEN | ParamInfoFlags::IS_READONLY) {
                return None;
            }
            Some(ParamMeta {
                id: info.id.get(),
                name: text(info.name),
                module: text(info.module),
                min: info.min_value,
                max: info.max_value,
                default: info.default_value,
                stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
            })
        })
        .collect()
}

/// `AudioPorts` only holds scratch pointer arrays that are rebuilt on every process call.
struct SendPorts(AudioPorts);
// SAFETY: the raw pointers inside are never dereferenced outside the process call that set them.
unsafe impl Send for SendPorts {}

struct ClapProcessor {
    proc: PluginAudioProcessor<ShodanHost>,
    failed: bool,
    inputs: PortLayout,
    outputs: PortLayout,
    in_ports: SendPorts,
    out_ports: SendPorts,
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    events: EventBuffer,
    param_rx: rtrb::Consumer<(u32, f64)>,
    steady: u64,
}

impl ClapProcessor {
    fn new(proc: PluginAudioProcessor<ShodanHost>, inputs: PortLayout, outputs: PortLayout, param_rx: rtrb::Consumer<(u32, f64)>) -> Self {
        let bufs = |l: &PortLayout| (0..l.count).map(|p| vec![0.0; l.channels[p].max(1) * MAX_FRAMES]).collect::<Vec<_>>();
        let total = |l: &PortLayout| l.channels[..l.count].iter().sum::<usize>();
        Self {
            proc,
            failed: false,
            in_ports: SendPorts(AudioPorts::with_capacity(total(&inputs), inputs.count)),
            out_ports: SendPorts(AudioPorts::with_capacity(total(&outputs), outputs.count)),
            in_bufs: bufs(&inputs),
            out_bufs: bufs(&outputs),
            inputs,
            outputs,
            events: EventBuffer::with_capacity(256),
            param_rx,
            steady: 0,
        }
    }
}

impl Module for ClapProcessor {
    fn process(&mut self, _ctx: &ModuleCtx, left: &mut [f32], right: &mut [f32]) {
        if self.failed {
            return;
        }
        let n = left.len().min(MAX_FRAMES);

        // Feed our stereo signal into the plugin's main input.
        for (p, buf) in self.in_bufs.iter_mut().enumerate() {
            let ch = self.inputs.channels[p];
            buf.fill(0.0);
            if Some(p) != self.inputs.main {
                continue;
            }
            match ch {
                1 => {
                    for i in 0..n {
                        buf[i] = (left[i] + right[i]) * 0.5;
                    }
                }
                c if c >= 2 => {
                    buf[..n].copy_from_slice(&left[..n]);
                    buf[MAX_FRAMES..MAX_FRAMES + n].copy_from_slice(&right[..n]);
                }
                _ => {}
            }
        }

        self.events.clear();
        while let Ok((id, value)) = self.param_rx.pop() {
            self.events.push(&ParamValueEvent::new(0, ClapId::new(id), Pckn::match_all(), value));
        }

        let Ok(started) = self.proc.ensure_processing_started() else {
            self.failed = true;
            return;
        };
        let inputs = self.inputs;
        let outputs = self.outputs;
        let ins = self.in_ports.0.with_input_buffers(self.in_bufs.iter_mut().enumerate().map(|(p, buf)| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                buf.chunks_exact_mut(MAX_FRAMES).take(inputs.channels[p]).map(|b| InputChannel { buffer: &mut b[..n], is_constant: false }),
            ),
        }));
        let mut outs = self.out_ports.0.with_output_buffers(self.out_bufs.iter_mut().enumerate().map(|(p, buf)| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(buf.chunks_exact_mut(MAX_FRAMES).take(outputs.channels[p]).map(|b| &mut b[..n])),
        }));
        let ok = started.process(&ins, &mut outs, &self.events.as_input(), &mut OutputEvents::void(), Some(self.steady), None).is_ok();
        self.steady += n as u64;
        if !ok {
            return;
        }

        let Some(main) = outputs.main else { return };
        let buf = &self.out_bufs[main];
        match outputs.channels[main] {
            1 => {
                left[..n].copy_from_slice(&buf[..n]);
                right[..n].copy_from_slice(&buf[..n]);
            }
            c if c >= 2 => {
                left[..n].copy_from_slice(&buf[..n]);
                right[..n].copy_from_slice(&buf[MAX_FRAMES..MAX_FRAMES + n]);
            }
            _ => return,
        }
        // Plugins are outside our control: never let NaN/inf through to the limiter.
        for x in left[..n].iter_mut().chain(&mut right[..n]) {
            *x = if x.is_finite() { x.clamp(-4.0, 4.0) } else { 0.0 };
        }
    }

    fn stop(&mut self) {
        self.proc.ensure_processing_stopped();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a real plugin: `CLAP_PATH=<dir with .clap> cargo test -- --ignored clap`.
    #[test]
    #[ignore]
    fn clap_load_activate_process_unload() {
        let found = crate::plugins::scan::scan();
        let info = found.iter().find(|p| !p.is_instrument).expect("no CLAP effect found on CLAP_PATH");
        println!("testing {} ({}) from {}", info.name, info.id, info.bundle.display());

        let mut plugin = LoadedPlugin::load(&info.bundle, &info.id, &info.name, egui::Context::default()).expect("load");
        println!("{} params, e.g. {:?}", plugin.params.len(), plugin.params.first());
        assert!(!plugin.params.is_empty());

        let mut proc = plugin.activate(48000.0).expect("activate");
        let first = plugin.params[0].clone();
        plugin.set_value(first.id, first.max);

        let n = 512;
        for block in 0..200 {
            let mut l: Vec<f32> = (0..n).map(|i| ((block * n + i) as f32 * 0.03).sin() * 0.5).collect();
            let mut r = l.clone();
            proc.process(&ModuleCtx { dry: &[] }, &mut l, &mut r);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        }
        plugin.idle();
        let v = plugin.get_value(first.id);
        println!("param '{}' now {:?} ({:?})", first.name, v, v.and_then(|v| plugin.value_text(first.id, v)));
        assert_eq!(v, Some(first.max));

        proc.stop();
        drop(proc);
        assert!(plugin.deactivate());
        // Re-activation (as after an audio device change) must work too.
        let mut proc = plugin.activate(44100.0).expect("re-activate");
        let (mut l, mut r) = (vec![0.1; 256], vec![0.1; 256]);
        proc.process(&ModuleCtx { dry: &[] }, &mut l, &mut r);
        proc.stop();
        drop(proc);
        drop(plugin);
    }

    /// Opens the plugin's own editor in our Win32 window, pumps messages briefly, closes it.
    #[test]
    #[ignore]
    fn clap_editor_opens_and_closes() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
        let found = crate::plugins::scan::scan();
        let info = found.iter().find(|p| !p.is_instrument).expect("no CLAP effect found on CLAP_PATH");
        let mut plugin = LoadedPlugin::load(&info.bundle, &info.id, &info.name, egui::Context::default()).expect("load");
        let _proc = plugin.activate(48000.0).expect("activate");
        assert!(plugin.has_editor(), "plugin reports no editor");
        plugin.open_editor().expect("open editor");
        assert!(plugin.window.is_some());
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(1500) {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            plugin.idle();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        plugin.close_editor();
        assert!(plugin.window.is_none());
    }
}
