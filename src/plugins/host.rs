//! Host-side callback handlers for CLAP plugins.

use eframe::egui;
use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPortsImpl};
use clack_extensions::gui::{GuiSize, HostGui, HostGuiImpl, PluginGui};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::params::{HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags, PluginParams};
use clack_host::prelude::*;
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ShodanHost;

impl HostHandlers for ShodanHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder.register::<HostLog>().register::<HostGui>().register::<HostParams>();
    }
}

/// Plugin → host requests that must be handled on the GUI (main) thread.
pub struct HostShared {
    repaint: egui::Context,
    pub callback_requested: AtomicBool,
    pub gui_closed: AtomicBool,
    pub resize_request: Mutex<Option<GuiSize>>,
}

impl HostShared {
    pub fn new(repaint: egui::Context) -> Self {
        Self { repaint, callback_requested: AtomicBool::new(false), gui_closed: AtomicBool::new(false), resize_request: Mutex::new(None) }
    }
}

impl<'a> SharedHandler<'a> for HostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Release);
        self.repaint.request_repaint();
    }
}

pub struct HostMainThread<'a> {
    _shared: &'a HostShared,
    pub params: Cell<Option<PluginParams>>,
    pub gui: Cell<Option<PluginGui>>,
}

impl<'a> HostMainThread<'a> {
    pub fn new(shared: &'a HostShared) -> Self {
        Self { _shared: shared, params: Cell::new(None), gui: Cell::new(None) }
    }
}

impl<'a> MainThreadHandler<'a> for HostMainThread<'a> {
    fn initialized(&self, instance: InitializedPluginHandle<'a>) {
        self.params.set(instance.get_extension());
        self.gui.set(instance.get_extension());
    }
}

impl HostLogImpl for HostShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        if severity > LogSeverity::Info {
            eprintln!("[plugin {severity}] {message}");
        }
    }
}

impl HostAudioPortsImpl for HostMainThread<'_> {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }
    fn rescan(&self, _flags: AudioPortRescanFlags) {}
}

impl HostParamsImplMainThread for HostMainThread<'_> {
    fn rescan(&self, _flags: ParamRescanFlags) {}
    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostParamsImplShared for HostShared {
    fn request_flush(&self) {
        // We process continuously, so parameter changes are flushed by the next process call.
    }
}

impl HostGuiImpl for HostShared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        if let Ok(mut r) = self.resize_request.lock() {
            *r = Some(new_size);
        }
        self.repaint.request_repaint();
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.gui_closed.store(true, Ordering::Release);
        self.repaint.request_repaint();
    }
}
