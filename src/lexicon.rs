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
    /// Silence of this length instead of a tone.
    pub rest: bool,
}

pub struct Word {
    pub text: &'static str,
    /// Other things people say that mean this word. Like `text`, one of these may be a phrase
    /// of several words. A phrase must not begin with a different word of the lexicon: while it
    /// is being said its beginning would already have been spoken as that word (tested).
    pub also: &'static [&'static str],
    pub notes: &'static [Note],
}

/// A steady note.
const fn n(semi: f32, beats: f32) -> Note {
    Note { from: semi, to: semi, beats, rest: false }
}

/// A note that slides from one pitch to another.
const fn g(from: f32, to: f32, beats: f32) -> Note {
    Note { from, to, beats, rest: false }
}

/// A silence inside a motif.
const fn r(beats: f32) -> Note {
    Note { from: 0.0, to: 0.0, beats, rest: true }
}

/// Words of one class share a way of sounding, so a listener who knows a few can guess at the
/// class of the rest.
pub static WORDS: &[Word] = &[
    // Answers: plain steps between steady notes.
    // Up a fifth, short-long: a bright "uh-HUH".
    Word { text: "yes", also: &["yeah", "yep", "yup"], notes: &[n(0.0, 1.0), n(7.0, 2.0)] },
    // The mirror of yes, ending in a droop.
    Word { text: "no", also: &["nope", "nah"], notes: &[n(7.0, 1.0), g(0.0, -3.0, 2.0)] },
    // Two pips on one note.
    Word { text: "ok", also: &["okay", "alright", "sure"], notes: &[n(7.0, 0.5), n(7.0, 1.0)] },
    // Up and back down again, undecided.
    Word { text: "maybe", also: &["perhaps"], notes: &[g(2.0, 5.0, 1.5), g(5.0, 2.0, 1.5)] },
    // Wanders, then asks.
    Word { text: "don't know", also: &["dunno"], notes: &[n(5.0, 1.0), n(1.0, 1.0), g(3.0, 6.0, 2.0)] },
    // Social: long sweeps.
    Word { text: "hello", also: &["hi", "hey"], notes: &[g(-5.0, 7.0, 2.0)] },
    // The mirror of hello, slower.
    Word { text: "bye", also: &["goodbye"], notes: &[g(7.0, -5.0, 3.0)] },
    // A high twinkle.
    Word { text: "thanks", also: &["thank", "thank you"], notes: &[n(12.0, 0.5), n(7.0, 0.5), n(12.0, 2.0)] },
    // Low, and sinking.
    Word { text: "sorry", also: &[], notes: &[n(-2.0, 1.0), g(-2.0, -7.0, 2.0)] },
    // People: one held tone, low for me and high for you.
    Word { text: "me", also: &["i", "i'm", "my"], notes: &[n(-5.0, 2.0)] },
    Word { text: "you", also: &["your", "you're"], notes: &[n(9.0, 2.0)] },
    // Actions: short pips.
    Word { text: "go", also: &["going", "move", "forward", "forwards", "ahead", "go forward", "go ahead"], notes: &[n(0.0, 0.5), n(0.0, 0.5), n(7.0, 1.0)] },
    // The mirror of go.
    Word { text: "come", also: &["coming", "come on", "follow", "following"], notes: &[n(7.0, 0.5), n(7.0, 0.5), n(0.0, 1.0)] },
    // Two low thuds.
    Word { text: "stop", also: &[], notes: &[n(-7.0, 0.5), r(0.25), n(-7.0, 1.0)] },
    // The same note again after a long pause.
    Word { text: "wait", also: &["hold on"], notes: &[n(2.0, 1.0), r(1.0), n(2.0, 1.0)] },
    // Three alarm chirps.
    Word { text: "help", also: &[], notes: &[g(4.0, 11.0, 0.75), g(4.0, 11.0, 0.75), g(4.0, 11.0, 0.75)] },
    // A high trill.
    Word { text: "look", also: &["see"], notes: &[n(12.0, 0.5), n(9.0, 0.5), n(12.0, 0.5), n(9.0, 0.5)] },
    // Reaching up and holding on.
    Word { text: "want", also: &["need"], notes: &[g(-2.0, 5.0, 1.5), n(5.0, 1.0)] },
    // Directions: three steps, down for left and up for right.
    Word { text: "left", also: &[], notes: &[n(4.0, 0.5), n(2.0, 0.5), n(0.0, 1.0)] },
    Word { text: "right", also: &[], notes: &[n(4.0, 0.5), n(6.0, 0.5), n(8.0, 1.0)] },
    // Warnings: a klaxon, built on the tritone.
    // Two-tone alarm, twice.
    Word { text: "warning", also: &["warn", "alert", "caution", "careful", "beware", "watch out"], notes: &[n(6.0, 0.5), n(0.0, 0.5), n(6.0, 0.5), n(0.0, 0.5)] },
    // Two falling siren wails.
    Word { text: "danger", also: &["dangerous", "hazard", "threat"], notes: &[g(12.0, 6.0, 1.0), g(12.0, 6.0, 1.0)] },
    // The alarm, then the two thuds of stop.
    Word {
        text: "stay back",
        also: &["stand back", "get back", "back off", "stay away", "keep away", "keep back"],
        notes: &[n(6.0, 0.5), n(0.0, 0.5), n(-7.0, 0.5), r(0.25), n(-7.0, 1.0)],
    },
    // The alarm sped up, fleeing upwards.
    Word { text: "run", also: &["running", "run away", "flee", "escape", "get out"], notes: &[n(0.0, 0.25), n(6.0, 0.25), n(0.0, 0.25), n(6.0, 0.25), g(6.0, 12.0, 1.0)] },
    // Up the tritone, then a long fall.
    Word { text: "enemy", also: &["enemies", "hostile", "hostiles", "intruder"], notes: &[n(0.0, 0.5), n(6.0, 0.5), g(6.0, -6.0, 2.0)] },
    // Places: a small step home for here, a leap away for there.
    Word { text: "here", also: &[], notes: &[n(2.0, 0.5), n(0.0, 1.5)] },
    Word { text: "there", also: &[], notes: &[n(0.0, 0.5), g(9.0, 14.0, 1.5)] },
    // Qualities.
    // A rising major arpeggio.
    Word { text: "good", also: &["great", "nice"], notes: &[n(0.0, 1.0), n(4.0, 1.0), n(7.0, 1.0), n(12.0, 2.0)] },
    // Falling through a tritone into a long slump.
    Word { text: "bad", also: &[], notes: &[n(6.0, 1.0), n(3.0, 1.0), g(0.0, -5.0, 3.0)] },
    // Questions: all end on the same rising slide.
    Word { text: "what", also: &[], notes: &[n(5.0, 1.0), g(5.0, 10.0, 1.5)] },
    Word { text: "where", also: &[], notes: &[n(10.0, 0.5), n(0.0, 0.5), g(5.0, 10.0, 1.5)] },
    // Time: an octave drop, the shortest word.
    Word { text: "now", also: &[], notes: &[n(12.0, 0.5), n(0.0, 0.5)] },
];

/// A token of a transcript without the punctuation around it.
pub fn bare(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric())
}

/// Two tokens are the same word, ignoring case, the punctuation around them and the kind of
/// apostrophe.
pub fn same(a: &str, b: &str) -> bool {
    let fold = |c: char| if c == '’' { '\'' } else { c.to_ascii_lowercase() };
    bare(a).chars().map(fold).eq(bare(b).chars().map(fold))
}

/// The word that `tokens` begin with and how many of them it takes; the longest one, so a
/// phrase wins over its first word.
fn match_at(tokens: &[&str]) -> Option<(u32, usize)> {
    let mut best = None;
    for (i, word) in WORDS.iter().enumerate() {
        for name in std::iter::once(&word.text).chain(word.also) {
            let len = name.split(' ').count();
            if best.is_none_or(|(_, b)| len > b) && len <= tokens.len() && name.split(' ').zip(tokens).all(|(n, t)| same(n, t)) {
                best = Some((i as u32, len));
            }
        }
    }
    best
}

/// A stretch of a transcript: a word the robot knows, or one token it does not.
pub struct Span<'a> {
    /// Index into [`WORDS`].
    pub word: Option<u32>,
    /// The first token of the stretch.
    pub token: &'a str,
    /// Position in the transcript of the last token of the stretch.
    pub last: usize,
}

/// A transcript as the robot understands it. Allocates, so not for the audio thread.
pub fn spans(text: &str) -> Vec<Span<'_>> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (word, len) = match match_at(&tokens[i..]) {
            Some((word, len)) => (Some(word), len),
            None => (None, 1),
        };
        spans.push(Span { word, token: tokens[i], last: i + len - 1 });
        i += len;
    }
    spans
}

/// Index into [`WORDS`] of a spoken word or phrase, ignoring case and punctuation.
#[cfg(test)]
fn lookup(text: &str) -> Option<u32> {
    match spans(text).as_slice() {
        [only] => only.word,
        _ => None,
    }
}

/// What the recogniser writes for a breath or a click rather than for speech: a lone "you".
pub fn is_noise(text: &str) -> bool {
    let mut tokens = text.split_whitespace();
    tokens.next().is_some_and(|t| same(t, "you")) && tokens.next().is_none()
}

/// Send the words of a transcript to the robot. Returns the ones it has no motif for, which
/// stay silent. Allocates, so not for the audio thread.
pub fn say(text: &str, bus: &WordBus) -> Vec<String> {
    let mut unknown = Vec::new();
    for span in spans(text) {
        match span.word {
            Some(word) => bus.push(word),
            None => unknown.push(span.token.to_string()),
        }
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> Option<u32> {
        WORDS.iter().position(|w| w.text == text).map(|i| i as u32)
    }

    #[test]
    fn lookup_ignores_case_and_punctuation() {
        assert_eq!(lookup("Yes,"), id("yes"));
        assert_eq!(lookup("\"BAD!\""), id("bad"));
        assert_eq!(lookup("cable"), None);
    }

    #[test]
    fn other_forms_are_the_same_word() {
        assert_eq!(lookup("Yeah."), id("yes"));
        assert_eq!(lookup("I’m"), id("me"));
        assert_eq!(lookup("Thank you!"), id("thanks"));
        assert_eq!(lookup("thank"), id("thanks"));
    }

    #[test]
    fn a_phrase_wins_over_its_first_word() {
        let words: Vec<_> = spans("I don't know, come on.").iter().map(|s| (s.word, s.last)).collect();
        assert_eq!(words, [(id("me"), 0), (id("don't know"), 2), (id("come"), 4)]);
    }

    #[test]
    fn words_are_unique_and_playable() {
        for (i, w) in WORDS.iter().enumerate() {
            for name in std::iter::once(&w.text).chain(w.also) {
                assert_eq!(lookup(name), Some(i as u32), "'{name}' is listed twice");
            }
            assert!(!w.notes.is_empty() && w.notes.iter().all(|n| n.beats > 0.0), "'{}' has an empty note", w.text);
            assert!(!w.notes[0].rest && !w.notes[w.notes.len() - 1].rest, "'{}' begins or ends with a rest", w.text);
        }
    }

    #[test]
    fn no_two_words_sound_the_same() {
        let motif = |w: &Word| w.notes.iter().map(|n| (n.from, n.to, n.beats, n.rest)).collect::<Vec<_>>();
        for (i, a) in WORDS.iter().enumerate() {
            for b in &WORDS[i + 1..] {
                assert_ne!(motif(a), motif(b), "'{}' and '{}'", a.text, b.text);
            }
        }
    }

    /// The listener counts the words it has passed on; a phrase that began as another word
    /// would be spoken as that word and then never as itself.
    #[test]
    fn a_phrase_does_not_begin_with_another_word() {
        for (i, w) in WORDS.iter().enumerate() {
            for name in std::iter::once(&w.text).chain(w.also) {
                let tokens: Vec<&str> = name.split(' ').collect();
                for end in 1..tokens.len() {
                    let begun = match_at(&tokens[..end]).map(|(word, _)| word);
                    assert!(begun.is_none_or(|word| word == i as u32), "'{name}' begins with another word");
                }
            }
        }
    }

    #[test]
    fn only_a_lone_you_is_noise() {
        assert!(is_noise(" You. "));
        assert!(!is_noise("you go"));
        assert!(!is_noise("yes"));
        assert!(!is_noise(""));
    }

    #[test]
    fn say_sends_only_known_words() {
        let bus = WordBus::default();
        let mut seen = bus.head();
        assert_eq!(say("Yes, that is good.", &bus), ["that", "is"]);
        assert_eq!(bus.next(&mut seen), id("yes"));
        assert_eq!(bus.next(&mut seen), id("good"));
        assert_eq!(bus.next(&mut seen), None);
    }
}
