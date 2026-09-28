//! Finds CLAP plugins in the standard Windows locations (plus `CLAP_PATH`).

use clack_host::prelude::PluginEntry;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub bundle: PathBuf,
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// True for instruments/synths, which make no sense in a voice chain.
    pub is_instrument: bool,
}

pub fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = std::env::var("CLAP_PATH") {
        paths.extend(std::env::split_paths(&p));
    }
    if let Ok(common) = std::env::var("COMMONPROGRAMFILES") {
        paths.push(Path::new(&common).join("CLAP"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        paths.push(Path::new(&local).join("Programs").join("Common").join("CLAP"));
    }
    paths
}

fn find_bundles(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("clap")) && path.is_file() {
            out.push(path);
        } else if path.is_dir() && depth < 4 {
            find_bundles(&path, out, depth + 1);
        }
    }
}

/// Load every bundle and list its plugins. Loading runs plugin code, so a broken bundle
/// can misbehave; bundles that fail to load are skipped.
pub fn scan() -> Vec<PluginInfo> {
    let mut bundles = Vec::new();
    for dir in search_paths() {
        find_bundles(&dir, &mut bundles, 0);
    }
    let mut found = Vec::new();
    for bundle in bundles {
        // SAFETY: loading a CLAP bundle executes its entry code; that is inherent to hosting.
        let Ok(entry) = (unsafe { PluginEntry::load(&bundle) }) else { continue };
        let Some(factory) = entry.get_plugin_factory() else { continue };
        for d in factory.plugin_descriptors() {
            let Some(id) = d.id().and_then(|s| s.to_str().ok()) else { continue };
            let text = |s: Option<&std::ffi::CStr>| s.map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
            let is_instrument = d.features().any(|f| f.to_bytes() == b"instrument");
            found.push(PluginInfo {
                bundle: bundle.clone(),
                id: id.to_string(),
                name: text(d.name()).trim().to_string(),
                vendor: text(d.vendor()),
                is_instrument,
            });
        }
    }
    found.sort_by_key(|a| a.name.to_lowercase());
    found
}
