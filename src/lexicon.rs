//! The robot language: every word it knows and the notes that word is.
//!
//! A word is always the same motif, so listeners can learn it. Text that is not in [`WORDS`] is
//! not spoken at all. To teach the robot a word, add a line to [`WORDS`].

use crate::shared::WordBus;

/// One tone of a motif. Pitches are semitones above the module's key.
pub struct Note {
    pub from: f32,
    /// Pitch at the end of the note; differs from `from` for a slide.
    pub to: f32,
    pub beats: f32,
}

pub struct Word {
    pub text: &'static str,
    pub notes: &'static [Note],
}

/// A steady note.
const fn n(semi: f32, beats: f32) -> Note {
    Note { from: semi, to: semi, beats }
}

/// A note that slides from one pitch to another.
const fn g(from: f32, to: f32, beats: f32) -> Note {
    Note { from, to, beats }
}

pub static WORDS: &[Word] = &[
    // Up a fifth, short-long: a bright "uh-HUH".
    Word { text: "yes", notes: &[n(0.0, 1.0), n(7.0, 2.0)] },
    // The mirror of yes, ending in a droop.
    Word { text: "no", notes: &[n(7.0, 1.0), g(0.0, -3.0, 2.0)] },
    // A rising major arpeggio.
    Word { text: "good", notes: &[n(0.0, 1.0), n(4.0, 1.0), n(7.0, 1.0), n(12.0, 2.0)] },
    // Falling through a tritone into a long slump.
    Word { text: "bad", notes: &[n(6.0, 1.0), n(3.0, 1.0), g(0.0, -5.0, 3.0)] },
];

/// Index into [`WORDS`] of a spoken word, ignoring case and punctuation.
pub fn lookup(text: &str) -> Option<u32> {
    let text = text.trim_matches(|c: char| !c.is_alphanumeric());
    WORDS.iter().position(|w| w.text.eq_ignore_ascii_case(text)).map(|i| i as u32)
}

/// Send the words of a transcript to the robot. Returns the ones it has no motif for, which
/// stay silent. Allocates, so not for the audio thread.
pub fn say(text: &str, bus: &WordBus) -> Vec<String> {
    let mut unknown = Vec::new();
    for token in text.split_whitespace() {
        match lookup(token) {
            Some(word) => bus.push(word),
            None => unknown.push(token.to_string()),
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_ignores_case_and_punctuation() {
        assert_eq!(lookup("Yes,"), Some(0));
        assert_eq!(lookup("\"BAD!\""), Some(3));
        assert_eq!(lookup("maybe"), None);
    }

    #[test]
    fn words_are_unique_and_playable() {
        for (i, w) in WORDS.iter().enumerate() {
            assert_eq!(lookup(w.text), Some(i as u32), "'{}' is listed twice", w.text);
            assert!(!w.notes.is_empty() && w.notes.iter().all(|n| n.beats > 0.0), "'{}' has an empty note", w.text);
        }
    }

    #[test]
    fn say_sends_only_known_words() {
        let bus = WordBus::default();
        let mut seen = bus.head();
        assert_eq!(say("Yes, that is good.", &bus), ["that", "is"]);
        assert_eq!(bus.next(&mut seen), Some(0));
        assert_eq!(bus.next(&mut seen), Some(2));
        assert_eq!(bus.next(&mut seen), None);
    }
}
