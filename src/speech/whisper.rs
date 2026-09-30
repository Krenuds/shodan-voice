//! Whisper (whisper.cpp) as the recogniser: one 16 kHz utterance in, its text out.

use super::segment::RATE;
use std::path::PathBuf;
use std::sync::OnceLock;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// `SHODAN_WHISPER_MODEL`, or `models/ggml-base.en.bin` in the config folder.
pub fn model_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SHODAN_WHISPER_MODEL") {
        return Some(path.into());
    }
    crate::presets::config_dir().map(|d| d.join("models").join("ggml-base.en.bin"))
}

/// The model, loaded once per process however many recognisers there are.
fn context() -> Result<&'static WhisperContext, String> {
    static CONTEXT: OnceLock<Result<WhisperContext, String>> = OnceLock::new();
    CONTEXT
        .get_or_init(|| {
            // Without a log backend this silences whisper.cpp's chatter on stderr.
            whisper_rs::install_logging_hooks();
            let path = model_path().ok_or("no config folder to look for the Whisper model in")?;
            if !path.exists() {
                return Err(format!("Whisper model not found: {}", path.display()));
            }
            WhisperContext::new_with_params(&path, WhisperContextParameters::default()).map_err(|e| format!("{}: {e}", path.display()))
        })
        .as_ref()
        .map_err(String::clone)
}

pub struct Recognizer {
    state: WhisperState,
    padded: Vec<f32>,
}

impl Recognizer {
    /// Loads the model if this is the first recogniser, which takes a moment.
    pub fn new() -> Result<Self, String> {
        let mut recognizer = Self { state: context()?.create_state().map_err(|e| e.to_string())?, padded: Vec::new() };
        // The first pass is several times slower than the rest; spend it on silence.
        recognizer.transcribe(&[])?;
        Ok(recognizer)
    }

    pub fn transcribe(&mut self, audio: &[f32]) -> Result<String, String> {
        // Whisper refuses very short input; a second of it is safe.
        self.padded.clear();
        self.padded.extend_from_slice(audio);
        self.padded.resize(audio.len().max(RATE + RATE / 10), 0.0);

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Utterances are at most a few seconds; a runaway decoder must not talk for longer.
        params.set_max_tokens(48);

        self.state.full(params, &self.padded).map_err(|e| e.to_string())?;
        let text: String = self.state.as_iter().map(|segment| segment.to_string()).collect();
        Ok(strip_annotations(&text))
    }
}

/// Whisper writes sounds that are not words as "[BLANK_AUDIO]" or "(music)"; drop those.
fn strip_annotations(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_are_removed() {
        assert_eq!(strip_annotations(" [BLANK_AUDIO]"), "");
        assert_eq!(strip_annotations(" Yes. (laughs) No."), "Yes.  No.");
    }
}
