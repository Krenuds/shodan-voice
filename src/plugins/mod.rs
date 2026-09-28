//! CLAP plugin hosting for the two effect slots.

mod host;
pub mod scan;
pub mod slot;
mod window;

pub use scan::PluginInfo;
pub use slot::LoadedPlugin;
