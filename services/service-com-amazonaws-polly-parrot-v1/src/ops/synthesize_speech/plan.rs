// SPDX-License-Identifier: CC0-1.0

//! What the request asks to be SPOKEN: the text, the lexicons that change it and the
//! SSML that shapes it, turned into a timed sequence of phonemes and the speech marks
//! that describe them.
//!
//! This is the whole front end, and it is where every input of the request reaches the
//! audio:
//!
//! * the text is tokenised, numbers are read as words and each word is read into
//!   phonemes by the spelling rules in [`super::phonemes`];
//! * a word a named lexicon defines is spoken as that lexeme's replacement — an
//!   `<alias>`'s text, or a `<phoneme>`'s pronunciation directly — so a lexicon changes
//!   what is spoken and therefore what comes out;
//! * `ssml` input is read as a document and its markup is APPLIED: `<prosody>` changes
//!   the rate, the pitch and the loudness of everything inside it, `<break>` and `<p>`
//!   and `<s>` pause, `<emphasis>` leans on its content, `<say-as>` reads its content as
//!   characters, digits or a number, `<sub>` speaks its alias, `<phoneme>` speaks its
//!   pronunciation, `<amazon:effect>` whispers or changes the vocal-tract length, and
//!   `<mark>` leaves an `ssml` speech mark at the point it sits at;
//! * every mark type the model declares comes off the same timeline: `sentence` and
//!   `word` with the byte offsets of the text they cover, `viseme` with the mouth shape
//!   of each phoneme as it is spoken, and `ssml` for each `<mark>`.
//!
//! The segment durations here are the usual lengths of English speech sounds
//! ([`super::phonemes`]) and the pauses between words, clauses and sentences are CHOSEN
//! — Amazon Polly publishes no pause lengths — but every one of them is a property of
//! the utterance, not of any recorded response: nothing in this module is keyed to a
//! particular text, and nothing is echoed back.

use super::lexicon::{Lexicons, Replacement};
use super::phonemes::{self, Phoneme, SILENCE_VISEME};
use super::ssml;
use super::SpeechMark;

/// How a stretch of speech is shaped: everything `<prosody>` and its neighbours set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prosody {
    /// How fast it is spoken, 1.0 being the normal rate. A rate below 1 lengthens every
    /// segment inside it, which is what makes slowed speech a longer stream.
    pub rate: f32,
    /// A multiplier on the fundamental frequency, 1.0 being the voice's own pitch.
    pub pitch: f32,
    /// A multiplier on amplitude, 1.0 being the normal loudness and 0.0 silent.
    pub volume: f32,
    /// A multiplier on every formant frequency, as a vocal-tract-length change makes.
    pub formant_scale: f32,
    /// Whether the content is whispered, which replaces voicing with breath noise.
    pub whispered: bool,
}

impl Default for Prosody {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch: 1.0,
            volume: 1.0,
            formant_scale: 1.0,
            whispered: false,
        }
    }
}

/// One stretch of the utterance the renderer produces in one piece.
#[derive(Debug, Clone)]
pub struct Segment {
    /// The phoneme spoken, or `None` for a pause.
    pub phoneme: Option<&'static Phoneme>,
    /// How long this segment lasts, in milliseconds, after the rate is applied.
    pub duration_ms: f32,
    /// How it is shaped.
    pub prosody: Prosody,
}

/// A planned utterance: what to produce, in order, and what to say about it.
#[derive(Debug, Clone, Default)]
pub struct Utterance {
    /// The phonemes and pauses, in the order they are spoken.
    pub segments: Vec<Segment>,
    /// Every speech mark of the utterance — `sentence`, `word`, `viseme` and `ssml` —
    /// in stream order. A request takes the types it asked for.
    pub marks: Vec<SpeechMark>,
}

impl Utterance {
    /// How long the whole utterance lasts, in milliseconds.
    pub fn duration_ms(&self) -> f32 {
        self.segments.iter().map(|segment| segment.duration_ms).sum()
    }

    /// The marks of the types `wanted` names, in stream order.
    pub fn marks_of_types(&self, wanted: &[String]) -> Vec<SpeechMark> {
        self.marks
            .iter()
            .filter(|mark| wanted.iter().any(|want| want == mark.mark_type))
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The pauses between things. CHOSEN lengths: Amazon Polly publishes none.
// ---------------------------------------------------------------------------

/// The silence between two words of a sentence.
const WORD_GAP_MS: f32 = 45.0;
/// The pause a comma, a semicolon or a colon makes.
const CLAUSE_PAUSE_MS: f32 = 220.0;
/// The pause after a sentence ends, and what `<s>` leaves behind it.
const SENTENCE_PAUSE_MS: f32 = 380.0;
/// The pause `<p>` leaves behind a paragraph.
const PARAGRAPH_PAUSE_MS: f32 = 600.0;
/// The silence every utterance ends with, which the closing `sil` viseme sits on.
const TRAILING_SILENCE_MS: f32 = 80.0;

/// The longest utterance this emulator plans, in milliseconds.
///
/// CHOSEN, and it is the FIRST of the two bounds on the work one request can ask for
/// (the second is [`super::render::max_samples`], which sizes the buffer the renderer
/// allocates). `Text` is capped at [`super::MAX_TOTAL_CHARACTERS`] characters, but how
/// long those characters take to speak is NOT: SSML markup is not billed, a
/// `<prosody rate>` below 1 stretches every segment and each `<break>` may add ten
/// seconds, so a document inside the character limit could otherwise ask for hours of
/// audio and the memory to hold it. Past this bound the plan adds no further segment, so
/// a request's answer is at most three minutes of the chosen encoding — at the highest
/// rate any format documents, 180 s × 48000 Hz × 2 bytes ≈ 17 MB — whatever the document
/// asks for.
pub const MAX_UTTERANCE_MS: f32 = 180_000.0;

/// How long `<break strength>` pauses for.
///
/// The keywords are the published ones, in the published order (none is no pause at
/// all); the milliseconds each stands for are CHOSEN, because the documentation gives
/// the strengths a relative order and no durations.
fn break_strength_ms(strength: &str) -> f32 {
    match strength {
        "none" => 0.0,
        "x-weak" => 100.0,
        "weak" => 200.0,
        "strong" => 700.0,
        "x-strong" => 1000.0,
        // "medium" is the documented default, and anything the vocabulary admitted that
        // is not one of the others is it.
        _ => 400.0,
    }
}

/// The rate, pitch and volume each `<prosody>` keyword sets, as a multiplier.
///
/// The keyword sets are the published ones ([`ssml::SUPPORTED_ELEMENTS`]); the
/// multipliers are CHOSEN, monotonic in the published order, with `medium`/`default`
/// being the voice's own setting.
fn rate_keyword(value: &str) -> Option<f32> {
    Some(match value {
        "x-slow" => 0.5,
        "slow" => 0.75,
        "medium" => 1.0,
        "fast" => 1.25,
        "x-fast" => 1.5,
        _ => return None,
    })
}

/// The pitch each `<prosody pitch>` keyword sets, as a multiplier.
fn pitch_keyword(value: &str) -> Option<f32> {
    Some(match value {
        "x-low" => 0.7,
        "low" => 0.85,
        "default" | "medium" => 1.0,
        "high" => 1.15,
        "x-high" => 1.3,
        _ => return None,
    })
}

/// The loudness each `<prosody volume>` keyword sets, as a multiplier.
fn volume_keyword(value: &str) -> Option<f32> {
    Some(match value {
        "silent" => 0.0,
        "x-soft" => 0.25,
        "soft" => 0.5,
        "default" | "medium" => 1.0,
        "loud" => 1.4,
        "x-loud" => 1.8,
        _ => return None,
    })
}

/// A setting that is either a keyword or a percentage: an unsigned percentage is the
/// absolute setting, a signed one is a change to the current value.
fn keyword_or_percent(value: &str, current: f32, keyword: impl Fn(&str) -> Option<f32>) -> f32 {
    if let Some(from_keyword) = keyword(value) {
        return from_keyword;
    }
    match ssml::percentage(value) {
        Some((true, magnitude)) => {
            let change = if value.starts_with('-') {
                1.0 - magnitude as f32 / 100.0
            } else {
                1.0 + magnitude as f32 / 100.0
            };
            (current * change).max(0.0)
        }
        Some((false, magnitude)) => (magnitude as f32 / 100.0).max(0.0),
        None => current,
    }
}

/// How `<say-as interpret-as>` reads its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SayAs {
    /// Each character on its own, by name.
    Characters,
    /// Each digit on its own, by name.
    Digits,
    /// The whole run as one number.
    Cardinal,
    /// The whole run as a position in a sequence.
    Ordinal,
    /// Two numbers, one over the other.
    Fraction,
    /// Read, but not out loud: the content is replaced by silence of its own length.
    Expletive,
}

impl SayAs {
    /// The reading `interpret-as` names, or `None` for one that is read as it is
    /// written.
    fn from_attribute(value: &str) -> Option<Self> {
        Some(match value {
            "characters" | "spell-out" => Self::Characters,
            "digits" | "telephone" => Self::Digits,
            "cardinal" | "number" => Self::Cardinal,
            "ordinal" => Self::Ordinal,
            "fraction" => Self::Fraction,
            "expletive" => Self::Expletive,
            _ => return None,
        })
    }
}

/// Plan what `text` says.
///
/// `Err` only when `ssml` input cannot be read as XML at all, which the caller answers
/// with `InvalidSsmlException`; the document's vocabulary has already been checked by
/// [`super::billed_characters`], which every path into synthesis runs first.
pub fn plan(text: &str, text_type: &str, lexicons: &Lexicons) -> Result<Utterance, String> {
    let mut builder = Builder::new(text, lexicons);
    if text_type == "ssml" {
        builder.walk_ssml()?;
    } else {
        let characters: Vec<(char, usize)> =
            text.char_indices().map(|(at, ch)| (ch, at)).collect();
        builder.speak_run(&characters, Prosody::default(), None);
    }
    Ok(builder.finish())
}

/// Builds one utterance: the timeline, the segments and the marks.
struct Builder<'a> {
    /// The request's `Text`, which every `start`/`end` offset indexes into.
    source: &'a str,
    /// The lexicons the request named, in that order.
    lexicons: &'a Lexicons,
    /// Where the next segment starts, in milliseconds.
    time_ms: f32,
    segments: Vec<Segment>,
    marks: Vec<SpeechMark>,
    /// The sentence being spoken: the index of its mark and where it started.
    sentence: Option<(usize, usize)>,
    /// Where the last word spoken ended, which closes a sentence that runs to the end
    /// of the text.
    last_word_end: usize,
    /// Whether a word gap is owed before the next word.
    gap_owed: bool,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str, lexicons: &'a Lexicons) -> Self {
        Self {
            source,
            lexicons,
            time_ms: 0.0,
            segments: Vec::new(),
            marks: Vec::new(),
            sentence: None,
            last_word_end: 0,
            gap_owed: false,
        }
    }

    /// The marks in stream order, after the utterance's closing silence.
    fn finish(mut self) -> Utterance {
        self.close_sentence(self.last_word_end);
        // Every utterance ends in silence, and the closing `sil` viseme sits on it.
        self.pause(TRAILING_SILENCE_MS);
        // A sentence mark precedes the words it contains, and a viseme falls where its
        // phoneme does, so the stream is ordered by time and then by how wide the mark
        // is.
        self.marks.sort_by_key(|mark| {
            let width = match mark.mark_type {
                "sentence" => 0,
                "word" => 1,
                "ssml" => 2,
                _ => 3,
            };
            (mark.time, width)
        });
        Utterance {
            segments: self.segments,
            marks: self.marks,
        }
    }

    /// Speak one run of character data: its words, its numbers and its punctuation.
    ///
    /// `characters` carries each character with the byte offset it sits at in the
    /// request's `Text`, so every mark this produces reports offsets into the text the
    /// caller sent, markup and all.
    fn speak_run(
        &mut self,
        characters: &[(char, usize)],
        prosody: Prosody,
        say_as: Option<SayAs>,
    ) {
        let mut index = 0usize;
        while index < characters.len() {
            let (ch, at) = characters[index];
            if is_token_character(ch) {
                let start = at;
                let mut end = at + ch.len_utf8();
                let mut token = String::new();
                while index < characters.len() {
                    let (ch, at) = characters[index];
                    // A period between two digits is a decimal point rather than the
                    // end of a sentence.
                    let decimal_point = ch == '.'
                        && token.chars().next_back().is_some_and(|c| c.is_ascii_digit())
                        && characters
                            .get(index + 1)
                            .is_some_and(|(next, _)| next.is_ascii_digit());
                    if !is_token_character(ch) && !decimal_point {
                        break;
                    }
                    token.push(ch);
                    end = at + ch.len_utf8();
                    index += 1;
                }
                self.speak_token(&token, Some((start, end)), prosody, say_as);
                continue;
            }
            match ch {
                '.' | '!' | '?' => self.close_sentence(at + ch.len_utf8()),
                ',' | ';' | ':' => self.pause(CLAUSE_PAUSE_MS / prosody.rate.max(0.1)),
                _ => {}
            }
            index += 1;
        }
    }

    /// Speak one token — a word, a number or a mixture — as one word mark.
    fn speak_token(
        &mut self,
        token: &str,
        span: Option<(usize, usize)>,
        prosody: Prosody,
        say_as: Option<SayAs>,
    ) {
        let words = self.words_of(token, say_as);
        if words.is_empty() {
            return;
        }
        if say_as == Some(SayAs::Expletive) {
            // Read, and then not said: the content is replaced by silence as long as
            // speaking it would have taken.
            let hidden: f32 = words
                .iter()
                .flatten()
                .map(|phoneme| f32::from(phoneme.duration_ms))
                .sum();
            self.pause(hidden / prosody.rate.max(0.1));
            return;
        }
        if self.gap_owed {
            self.pause(WORD_GAP_MS / prosody.rate.max(0.1));
        }
        if let Some((start, end)) = span {
            if self.sentence.is_none() {
                // The sentence starts where its first word does, and its mark is
                // completed when the sentence ends.
                self.sentence = Some((self.marks.len(), start));
                self.marks.push(SpeechMark {
                    time: self.time_ms as u64,
                    mark_type: "sentence",
                    span: Some((start, start)),
                    value: String::new(),
                });
            }
            self.marks.push(SpeechMark {
                time: self.time_ms as u64,
                mark_type: "word",
                span: Some((start, end)),
                value: self.source.get(start..end).unwrap_or_default().to_string(),
            });
            self.last_word_end = end;
        }
        for (position, word) in words.iter().enumerate() {
            if position > 0 {
                self.pause(WORD_GAP_MS / prosody.rate.max(0.1));
            }
            for phoneme in word {
                self.push_phoneme(phoneme, prosody);
            }
        }
        self.gap_owed = true;
    }

    /// The words one token is spoken as: the lexicons first, then the reading
    /// `<say-as>` asks for, then the spelling rules.
    fn words_of(&self, token: &str, say_as: Option<SayAs>) -> Vec<Vec<&'static Phoneme>> {
        if let Some(replacement) = self.lexicons.replacement_for(token) {
            return match replacement {
                // The pronunciation is spoken as given.
                Replacement::Pronunciation(spoken) => vec![spoken.clone()],
                // An alias is other TEXT, so it is read the way text is read — which is
                // what makes an alias of several words take longer to speak than the
                // grapheme it replaces.
                Replacement::Alias(alias) => read_words(alias),
            };
        }
        match say_as {
            Some(SayAs::Characters) => token
                .chars()
                .filter_map(spoken_character)
                .flat_map(|name| read_words(&name))
                .collect(),
            Some(SayAs::Digits) => token
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .filter_map(spoken_character)
                .flat_map(|name| read_words(&name))
                .collect(),
            Some(SayAs::Cardinal) | Some(SayAs::Expletive) | None => read_number_or_words(token),
            Some(SayAs::Ordinal) => ordinal_words(token),
            Some(SayAs::Fraction) => {
                let mut parts = token.split(['/', ':']);
                let numerator = parts.next().unwrap_or_default();
                let denominator = parts.next().unwrap_or_default();
                let mut words = read_number_or_words(numerator);
                if !denominator.is_empty() {
                    words.extend(read_words("over"));
                    words.extend(read_number_or_words(denominator));
                }
                words
            }
        }
    }

    /// Add one phoneme to the timeline, with the viseme it shows.
    fn push_phoneme(&mut self, phoneme: &'static Phoneme, prosody: Prosody) {
        if self.time_ms >= MAX_UTTERANCE_MS {
            return;
        }
        self.marks.push(SpeechMark {
            time: self.time_ms as u64,
            mark_type: "viseme",
            span: None,
            value: phoneme.viseme.to_string(),
        });
        let duration = f32::from(phoneme.duration_ms) / prosody.rate.clamp(0.1, 10.0);
        self.segments.push(Segment {
            phoneme: Some(phoneme),
            duration_ms: duration,
            prosody,
        });
        self.time_ms += duration;
    }

    /// Add a pause, with the `sil` viseme the closed mouth shows.
    fn pause(&mut self, duration_ms: f32) {
        if self.time_ms >= MAX_UTTERANCE_MS {
            return;
        }
        let duration = duration_ms.max(0.0);
        if duration <= 0.0 {
            return;
        }
        self.marks.push(SpeechMark {
            time: self.time_ms as u64,
            mark_type: "viseme",
            span: None,
            value: SILENCE_VISEME.to_string(),
        });
        self.segments.push(Segment {
            phoneme: None,
            duration_ms: duration,
            prosody: Prosody::default(),
        });
        self.time_ms += duration;
        self.gap_owed = false;
    }

    /// Close the sentence being spoken at `end`, completing its mark, and pause.
    fn close_sentence(&mut self, end: usize) {
        let Some((mark_index, start)) = self.sentence.take() else {
            return;
        };
        let end = end.max(start);
        if let Some(mark) = self.marks.get_mut(mark_index) {
            mark.span = Some((start, end));
            mark.value = self
                .source
                .get(start..end)
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        self.last_word_end = end;
        self.pause(SENTENCE_PAUSE_MS);
    }

    /// Read `ssml` input as a document and apply its markup.
    fn walk_ssml(&mut self) -> Result<(), String> {
        use quick_xml::events::Event;

        let mut reader = quick_xml::Reader::from_str(self.source);
        // The prosody in force, innermost last. The document element's frame is the
        // voice's own settings.
        let mut frames: Vec<Frame> = vec![Frame::root()];
        loop {
            let start = reader.buffer_position() as usize;
            let event = reader.read_event().map_err(|e| e.to_string())?;
            let end = reader.buffer_position() as usize;
            match event {
                Event::Eof => break,
                Event::Start(ref element) | Event::Empty(ref element) => {
                    let name = ssml::element_name(element);
                    let parent = frames.last().cloned().unwrap_or_else(Frame::root);
                    let frame = parent.nested(&name, element, start);
                    // A pause and a mark are POINTS in the stream, so they take effect
                    // where the element sits rather than when it closes.
                    match name.as_str() {
                        "break" => {
                            let time = attribute(element, "time")
                                .and_then(|value| ssml::duration_ms(&value))
                                .map(|ms| ms as f32);
                            let strength = attribute(element, "strength");
                            self.pause(time.unwrap_or_else(|| {
                                break_strength_ms(strength.as_deref().unwrap_or("medium"))
                            }));
                        }
                        "mark" => {
                            if let Some(label) = attribute(element, "name") {
                                self.marks.push(SpeechMark {
                                    time: self.time_ms as u64,
                                    mark_type: "ssml",
                                    span: None,
                                    value: label,
                                });
                            }
                        }
                        _ => {}
                    }
                    if matches!(event, Event::Start(_)) {
                        frames.push(frame);
                    } else {
                        // An empty element encloses nothing, so a capture on it speaks
                        // straight away.
                        self.close_frame(&frame, end);
                    }
                }
                Event::End(_) => {
                    if frames.len() > 1 {
                        let frame = frames.pop().unwrap_or_else(Frame::root);
                        self.close_frame(&frame, end);
                    }
                }
                Event::Text(ref text) => {
                    let raw = String::from_utf8_lossy(text.as_ref()).to_string();
                    let characters: Vec<(char, usize)> = raw
                        .char_indices()
                        .map(|(offset, ch)| (ch, start + offset))
                        .collect();
                    self.take_characters(&characters, &mut frames);
                }
                Event::CData(ref data) => {
                    // The content of a CDATA section is character data read verbatim;
                    // it starts after the section's `<![CDATA[` opening.
                    const OPENING: usize = "<![CDATA[".len();
                    let raw = String::from_utf8_lossy(data.as_ref()).to_string();
                    let characters: Vec<(char, usize)> = raw
                        .char_indices()
                        .map(|(offset, ch)| (ch, start + OPENING + offset))
                        .collect();
                    self.take_characters(&characters, &mut frames);
                }
                Event::GeneralRef(ref reference) => {
                    // A reference is character data too: it stands for the one character
                    // it names, at the offset the reference itself sits at.
                    let name = String::from_utf8_lossy(reference.as_ref()).to_string();
                    if let Some(ch) = referenced_character(&name) {
                        self.take_characters(&[(ch, start)], &mut frames);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Hand one run of character data to the innermost frame: a frame that captures its
    /// content collects it, and any other speaks it.
    fn take_characters(&mut self, characters: &[(char, usize)], frames: &mut [Frame]) {
        if let Some(frame) = frames
            .iter_mut()
            .rev()
            .find(|frame| frame.capture.is_some())
        {
            for (ch, _) in characters {
                frame.collected.push(*ch);
            }
            return;
        }
        let frame = frames.last().cloned().unwrap_or_else(Frame::root);
        self.speak_run(characters, frame.prosody, frame.say_as);
    }

    /// Finish one element: speak what it captured, and pause where it ends one.
    fn close_frame(&mut self, frame: &Frame, end: usize) {
        let span = Some((frame.start, end.max(frame.start)));
        match &frame.capture {
            Some(Capture::Pronunciation { spelling, alphabet }) => {
                match phonemes::from_phonetic(spelling, alphabet) {
                    Some(spoken) => self.speak_phonemes(spoken, span, frame.prosody),
                    // A pronunciation this service cannot read is not a reason to drop
                    // the content: the text inside is spoken instead.
                    None => self.speak_collected(frame, span),
                }
            }
            Some(Capture::Alias(alias)) => {
                let words = read_words(alias);
                self.speak_words(words, span, frame.prosody);
            }
            Some(Capture::Content) => self.speak_collected(frame, span),
            None => {}
        }
        match frame.name.as_str() {
            "s" => {
                self.close_sentence(end);
            }
            "p" => {
                self.close_sentence(end);
                self.pause(PARAGRAPH_PAUSE_MS);
            }
            _ => {}
        }
    }

    /// Speak the text a capturing frame collected, with the reading it asks for.
    fn speak_collected(&mut self, frame: &Frame, span: Option<(usize, usize)>) {
        let collected = frame.collected.trim().to_string();
        if collected.is_empty() {
            return;
        }
        let words: Vec<Vec<&'static Phoneme>> = collected
            .split_whitespace()
            .flat_map(|token| self.words_of(token, frame.say_as))
            .collect();
        if frame.say_as == Some(SayAs::Expletive) {
            let hidden: f32 = words
                .iter()
                .flatten()
                .map(|phoneme| f32::from(phoneme.duration_ms))
                .sum();
            self.pause(hidden / frame.prosody.rate.max(0.1));
            return;
        }
        self.speak_words(words, span, frame.prosody);
    }

    /// Speak one pronunciation as one word.
    fn speak_phonemes(
        &mut self,
        spoken: Vec<&'static Phoneme>,
        span: Option<(usize, usize)>,
        prosody: Prosody,
    ) {
        self.speak_words(vec![spoken], span, prosody);
    }

    /// Speak a sequence of words that were not tokenised out of the text itself — the
    /// content of a `<sub>`, a `<phoneme>` or a `<say-as>` — under one word mark.
    fn speak_words(
        &mut self,
        words: Vec<Vec<&'static Phoneme>>,
        span: Option<(usize, usize)>,
        prosody: Prosody,
    ) {
        if words.iter().all(|word| word.is_empty()) {
            return;
        }
        if self.gap_owed {
            self.pause(WORD_GAP_MS / prosody.rate.max(0.1));
        }
        if let Some((start, end)) = span {
            if self.sentence.is_none() {
                self.sentence = Some((self.marks.len(), start));
                self.marks.push(SpeechMark {
                    time: self.time_ms as u64,
                    mark_type: "sentence",
                    span: Some((start, start)),
                    value: String::new(),
                });
            }
            self.marks.push(SpeechMark {
                time: self.time_ms as u64,
                mark_type: "word",
                span: Some((start, end)),
                value: self.source.get(start..end).unwrap_or_default().to_string(),
            });
            self.last_word_end = end;
        }
        for (position, word) in words.iter().enumerate() {
            if position > 0 {
                self.pause(WORD_GAP_MS / prosody.rate.max(0.1));
            }
            for phoneme in word {
                self.push_phoneme(phoneme, prosody);
            }
        }
        self.gap_owed = true;
    }
}

/// What one open element sets for everything inside it.
#[derive(Debug, Clone)]
struct Frame {
    /// The element's name, as the vocabulary spells it.
    name: String,
    /// The prosody in force inside it.
    prosody: Prosody,
    /// The reading `<say-as>` asks for inside it.
    say_as: Option<SayAs>,
    /// What this element speaks INSTEAD of its content, if anything.
    capture: Option<Capture>,
    /// The character data inside a capturing element.
    collected: String,
    /// Where the element starts in the text, for the marks its content produces.
    start: usize,
}

/// What an element speaks instead of the text inside it.
#[derive(Debug, Clone)]
enum Capture {
    /// `<phoneme ph>`: the pronunciation to speak.
    Pronunciation {
        /// The phonetic spelling.
        spelling: String,
        /// The alphabet it is written in.
        alphabet: String,
    },
    /// `<sub alias>`: the text to speak in its place.
    Alias(String),
    /// The content is spoken, but as one unit and under a reading of its own.
    Content,
}

impl Frame {
    /// The document's own frame: the voice's settings, nothing captured.
    fn root() -> Self {
        Self {
            name: "speak".to_string(),
            prosody: Prosody::default(),
            say_as: None,
            capture: None,
            collected: String::new(),
            start: 0,
        }
    }

    /// The frame `element` opens inside this one.
    fn nested(&self, name: &str, element: &quick_xml::events::BytesStart<'_>, start: usize) -> Self {
        let mut prosody = self.prosody;
        let mut say_as = self.say_as;
        let mut capture = None;
        match name {
            "prosody" => {
                if let Some(rate) = attribute(element, "rate") {
                    prosody.rate = keyword_or_percent(&rate, prosody.rate, rate_keyword);
                }
                if let Some(pitch) = attribute(element, "pitch") {
                    prosody.pitch = keyword_or_percent(&pitch, prosody.pitch, pitch_keyword);
                }
                if let Some(volume) = attribute(element, "volume") {
                    prosody.volume = match volume_keyword(&volume) {
                        Some(absolute) => absolute,
                        // A decibel value is a CHANGE to the current loudness, which is
                        // what the unit means.
                        None => match ssml::decibels(&volume) {
                            Some(decibels) => {
                                let signed = if volume.starts_with('-') {
                                    -(decibels as f32)
                                } else {
                                    decibels as f32
                                };
                                prosody.volume * 10f32.powf(signed / 20.0)
                            }
                            None => prosody.volume,
                        },
                    };
                }
            }
            "emphasis" => match attribute(element, "level").as_deref() {
                // Leaning on a word is saying it slower and louder; reducing it is the
                // opposite.
                Some("strong") => {
                    prosody.rate *= 0.85;
                    prosody.volume *= 1.3;
                }
                Some("reduced") => {
                    prosody.rate *= 1.1;
                    prosody.volume *= 0.8;
                }
                _ => {
                    prosody.rate *= 0.95;
                    prosody.volume *= 1.15;
                }
            },
            "amazon:effect" => {
                if attribute(element, "name").as_deref() == Some("whispered") {
                    prosody.whispered = true;
                }
                if attribute(element, "phonation").as_deref() == Some("soft") {
                    prosody.volume *= 0.7;
                }
                if let Some(length) = attribute(element, "vocal-tract-length") {
                    // A longer vocal tract resonates LOWER, so the formants scale by the
                    // inverse of the length.
                    let scaled = keyword_or_percent(&length, 1.0, |_| None);
                    if scaled > 0.0 {
                        prosody.formant_scale = 1.0 / scaled;
                    }
                }
            }
            "phoneme" => {
                if let Some(spelling) = attribute(element, "ph") {
                    capture = Some(Capture::Pronunciation {
                        spelling,
                        alphabet: attribute(element, "alphabet").unwrap_or_default(),
                    });
                }
            }
            "sub" => {
                if let Some(alias) = attribute(element, "alias") {
                    capture = Some(Capture::Alias(alias));
                }
            }
            "say-as" => {
                say_as = attribute(element, "interpret-as")
                    .as_deref()
                    .and_then(SayAs::from_attribute);
                // A reading of its own needs the whole run, so the content is collected
                // rather than spoken word by word as it arrives.
                if say_as.is_some() {
                    capture = Some(Capture::Content);
                }
            }
            _ => {}
        }
        Self {
            name: name.to_string(),
            prosody,
            say_as,
            capture,
            collected: String::new(),
            start,
        }
    }
}

/// One attribute of an element, unescaped.
fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
        // `xml:lang` and the vendor-prefixed attributes are matched on the name the
        // vocabulary publishes, so the prefix is part of the comparison.
        (key == name).then(|| {
            let raw = String::from_utf8_lossy(attribute.value.as_ref()).to_string();
            quick_xml::escape::unescape(&raw)
                .map(|value| value.to_string())
                .unwrap_or(raw)
        })
    })
}

/// The character a general or numeric reference stands for.
fn referenced_character(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        numeric => {
            let digits = numeric.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Whether `ch` belongs to a token — a word or a number — rather than separating two.
fn is_token_character(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '\'' || ch == '\u{2019}' || ch == '-'
}

/// How one character is NAMED when it is spelled out.
fn spoken_character(ch: char) -> Option<String> {
    if let Some(digit) = ch.to_digit(10) {
        return DIGITS.get(digit as usize).map(|name| (*name).to_string());
    }
    phonemes::letter_name(ch).map(|name| name.to_string())
}

/// Read a stretch of text as words: each whitespace-separated token by the spelling
/// rules, with a number read as a number.
fn read_words(text: &str) -> Vec<Vec<&'static Phoneme>> {
    text.split_whitespace()
        .flat_map(read_number_or_words)
        .collect()
}

/// Read one token: a number as a number, anything else by the spelling rules.
fn read_number_or_words(token: &str) -> Vec<Vec<&'static Phoneme>> {
    let trimmed: String = token
        .chars()
        .filter(|ch| is_token_character(*ch) || *ch == '.')
        .collect();
    if trimmed.chars().any(|ch| ch.is_ascii_digit()) {
        return number_words(&trimmed)
            .iter()
            .map(|word| phonemes::pronounce(word))
            .filter(|spoken| !spoken.is_empty())
            .collect();
    }
    let spoken = phonemes::pronounce(&trimmed);
    if spoken.is_empty() {
        return Vec::new();
    }
    vec![spoken]
}

/// The words a token carrying digits is read as: the digit runs as numbers, the letter
/// runs by the spelling rules, in the order they are written.
fn number_words(token: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let characters: Vec<char> = token.chars().collect();
    let mut index = 0usize;
    while index < characters.len() {
        if characters[index].is_ascii_digit() {
            let start = index;
            while index < characters.len()
                && (characters[index].is_ascii_digit()
                    || (characters[index] == '.'
                        && characters
                            .get(index + 1)
                            .is_some_and(|next| next.is_ascii_digit())))
            {
                index += 1;
            }
            let number: String = characters[start..index].iter().collect();
            words.extend(spoken_number(&number));
            continue;
        }
        let start = index;
        while index < characters.len() && !characters[index].is_ascii_digit() {
            index += 1;
        }
        let run: String = characters[start..index].iter().collect();
        if !run.is_empty() {
            words.push(run);
        }
    }
    words
}

/// The words a written number is read as: `3.5` is three point five, and a run of
/// digits too long to be a number is read digit by digit.
fn spoken_number(number: &str) -> Vec<String> {
    let mut parts = number.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    let mut words = match whole.parse::<u64>() {
        Ok(value) => cardinal_words(value),
        // More digits than a number carries is a sequence of digits, which is how a
        // long run of them is read.
        Err(_) => whole
            .chars()
            .filter_map(|ch| ch.to_digit(10))
            .filter_map(|digit| DIGITS.get(digit as usize).map(|name| (*name).to_string()))
            .collect(),
    };
    if let Some(fraction) = fraction {
        words.push("point".to_string());
        words.extend(
            fraction
                .chars()
                .filter_map(|ch| ch.to_digit(10))
                .filter_map(|digit| DIGITS.get(digit as usize).map(|name| (*name).to_string())),
        );
    }
    words
}

/// The names of the digits.
const DIGITS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// The numbers with names of their own.
const SMALL: &[&str] = &[
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

/// The tens.
const TENS: &[&str] = &[
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// The scales, in the order a number is read in.
const SCALES: &[(u64, &str)] = &[
    (1_000_000_000_000, "trillion"),
    (1_000_000_000, "billion"),
    (1_000_000, "million"),
    (1_000, "thousand"),
];

/// The words `value` is read as.
pub fn cardinal_words(value: u64) -> Vec<String> {
    if let Some(name) = SMALL.get(value as usize) {
        return vec![(*name).to_string()];
    }
    for (scale, name) in SCALES {
        if value >= *scale {
            let mut words = cardinal_words(value / scale);
            words.push((*name).to_string());
            if !value.is_multiple_of(*scale) {
                words.extend(cardinal_words(value % scale));
            }
            return words;
        }
    }
    if value >= 100 {
        let mut words = cardinal_words(value / 100);
        words.push("hundred".to_string());
        if !value.is_multiple_of(100) {
            words.extend(cardinal_words(value % 100));
        }
        return words;
    }
    let tens = TENS
        .get((value / 10) as usize)
        .copied()
        .unwrap_or_default()
        .to_string();
    let ones = value % 10;
    if ones == 0 {
        return vec![tens];
    }
    vec![tens, SMALL[ones as usize].to_string()]
}

/// The words a position in a sequence is read as.
fn ordinal_words(token: &str) -> Vec<Vec<&'static Phoneme>> {
    let digits: String = token.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let Ok(value) = digits.parse::<u64>() else {
        return read_number_or_words(token);
    };
    let mut words = cardinal_words(value);
    if let Some(last) = words.pop() {
        words.push(ordinal_form(&last));
    }
    words
        .iter()
        .map(|word| phonemes::pronounce(word))
        .filter(|spoken| !spoken.is_empty())
        .collect()
}

/// The ordinal name of the last word of a number.
fn ordinal_form(word: &str) -> String {
    match word {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        other => match other.strip_suffix('y') {
            Some(stem) => format!("{stem}ieth"),
            None => format!("{other}th"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ids of the phonemes an utterance speaks, in order.
    fn spoken_ids(utterance: &Utterance) -> Vec<&'static str> {
        utterance
            .segments
            .iter()
            .filter_map(|segment| segment.phoneme.map(|phoneme| phoneme.id))
            .collect()
    }

    /// RULE: a word a lexicon defines is spoken as the lexeme's replacement, so the
    /// utterance is longer when the replacement is longer.
    #[test]
    fn a_lexicon_changes_what_is_spoken() {
        let mut lexicons = Lexicons::default();
        lexicons.add(
            "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" version=\"1.0\" \
             alphabet=\"ipa\" xml:lang=\"en-US\"><lexeme><grapheme>AWS</grapheme>\
             <alias>Amazon Web Services</alias></lexeme></lexicon>",
        );
        let plain = plan("Please deploy AWS today.", "text", &Lexicons::default())
            .expect("plain text plans");
        let applied =
            plan("Please deploy AWS today.", "text", &lexicons).expect("plain text plans");
        assert!(
            applied.duration_ms() > plain.duration_ms() * 1.2,
            "the lexicon must lengthen the utterance: plain {} ms, applied {} ms",
            plain.duration_ms(),
            applied.duration_ms()
        );
    }

    /// RULE: `<prosody rate>` scales every segment inside it, so slowed speech lasts
    /// longer and faster speech less.
    #[test]
    fn prosody_rate_scales_the_timeline() {
        let plain = plan("This is spoken at a rate.", "text", &Lexicons::default())
            .expect("plain text plans");
        for (rate, bound) in [("x-slow", 1.3f32), ("50%", 1.3)] {
            let slowed = plan(
                &format!("<speak><prosody rate=\"{rate}\">This is spoken at a rate.</prosody></speak>"),
                "ssml",
                &Lexicons::default(),
            )
            .expect("ssml plans");
            assert!(
                slowed.duration_ms() > plain.duration_ms() * bound,
                "rate {rate:?}: expected more than {bound}x of {} ms, got {} ms",
                plain.duration_ms(),
                slowed.duration_ms()
            );
        }
        let fast = plan(
            "<speak><prosody rate=\"x-fast\">This is spoken at a rate.</prosody></speak>",
            "ssml",
            &Lexicons::default(),
        )
        .expect("ssml plans");
        assert!(
            fast.duration_ms() < plain.duration_ms(),
            "x-fast must shorten the utterance: plain {} ms, fast {} ms",
            plain.duration_ms(),
            fast.duration_ms()
        );
    }

    /// RULE: every mark type comes off one timeline, and a `<mark>` sits where it is
    /// written.
    #[test]
    fn marks_cover_every_type() {
        let utterance = plan(
            "<speak>Hi there. <mark name=\"here\"/>Go!</speak>",
            "ssml",
            &Lexicons::default(),
        )
        .expect("ssml plans");
        for wanted in ["sentence", "word", "viseme", "ssml"] {
            assert!(
                utterance.marks.iter().any(|mark| mark.mark_type == wanted),
                "type {wanted:?}: no mark of it, got {:?}",
                utterance.marks
            );
        }
        let visemes: Vec<&str> = utterance
            .marks
            .iter()
            .filter(|mark| mark.mark_type == "viseme")
            .map(|mark| mark.value.as_str())
            .collect();
        assert_eq!(
            visemes.last().copied(),
            Some(SILENCE_VISEME),
            "the viseme stream ends on silence, got {visemes:?}"
        );
        let times: Vec<u64> = utterance.marks.iter().map(|mark| mark.time).collect();
        assert!(
            times.windows(2).all(|pair| pair[0] <= pair[1]),
            "marks are in stream order, got {times:?}"
        );
    }

    /// RULE: numbers are read as words, so a digit run is spoken rather than skipped.
    #[test]
    fn numbers_are_read_as_words() {
        for (value, expected) in [
            (0u64, vec!["zero"]),
            (21, vec!["twenty", "one"]),
            (105, vec!["one", "hundred", "five"]),
            (1_000, vec!["one", "thousand"]),
            (2_300_000, vec!["two", "million", "three", "hundred", "thousand"]),
        ] {
            assert_eq!(
                cardinal_words(value),
                expected,
                "number {value}: read as {:?}",
                cardinal_words(value)
            );
        }
        let utterance = plan("Call 42 now.", "text", &Lexicons::default()).expect("text plans");
        assert!(
            spoken_ids(&utterance).len() > phonemes::pronounce("call").len(),
            "the number must be spoken, got {:?}",
            spoken_ids(&utterance)
        );
    }

    /// RULE: `<break>` pauses for the time it asks for.
    #[test]
    fn a_break_pauses_for_its_time() {
        let without = plan("<speak>one two</speak>", "ssml", &Lexicons::default())
            .expect("ssml plans")
            .duration_ms();
        for (markup, expected) in [
            ("<break time=\"1s\"/>", 1000.0f32),
            ("<break time=\"250ms\"/>", 250.0),
            ("<break strength=\"x-strong\"/>", 1000.0),
            ("<break strength=\"none\"/>", 0.0),
        ] {
            let with = plan(
                &format!("<speak>one {markup} two</speak>"),
                "ssml",
                &Lexicons::default(),
            )
            .expect("ssml plans")
            .duration_ms();
            let added = with - without;
            assert!(
                (added - expected).abs() < 60.0,
                "markup {markup:?}: expected about {expected} ms more, got {added} ms"
            );
        }
    }
}
