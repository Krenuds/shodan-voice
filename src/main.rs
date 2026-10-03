// Release builds are a GUI app with no console window; `--render` still prints when run from a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod gui;
#[cfg(feature = "clap-host")]
mod plugins;

// The engine lives in `shodan-core`; its modules are re-exported so the GUI can name them as before.
use shodan_core::{dsp, lexicon, modules, params, presets, render, shared, speech, trace};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--render") {
        attach_console();
        if let Err(e) = render::run(&args[1..]) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = gui::run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// Reattach to the parent terminal so `--render` output is visible in release builds.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}
