// SPDX-License-Identifier: CC0-1.0

//! What a stored PLS lexicon SAYS, for the synthesiser that applies it.
//!
//! `PutLexicon` stores a lexicon's document and checks that it is a lexicon
//! ([`crate::ops::put_lexicon::parse_pls`]); this reads the lexemes back out of that
//! document, because a pronunciation lexicon exists to change how a grapheme is spoken.
//! Each `<lexeme>` pairs its `<grapheme>`s with the replacement PLS gives it — an
//! `<alias>`, which is other TEXT to speak, or a `<phoneme>`, which is the pronunciation
//! to speak directly — and a word of the text that matches a grapheme is spoken as that
//! replacement instead.
//!
//! Only elements bound to the PLS namespace ([`crate::ops::put_lexicon::PLS_NAMESPACE`])
//! are read, exactly as `PutLexicon` counts only those: an element borrowed from another
//! vocabulary is not a lexeme.

use crate::ops::put_lexicon::PLS_NAMESPACE;

use super::phonemes::{self, Phoneme};

/// What a lexeme replaces its graphemes with.
#[derive(Debug, Clone)]
pub enum Replacement {
    /// `<alias>`: other text, which is spoken in place of the grapheme.
    Alias(String),
    /// `<phoneme>`: the pronunciation itself, which is spoken as given.
    Pronunciation(Vec<&'static Phoneme>),
}

/// One `<lexeme>`: the written forms it matches and what they are spoken as.
#[derive(Debug, Clone)]
pub struct Lexeme {
    /// The `<grapheme>`s this lexeme matches, as written.
    pub graphemes: Vec<String>,
    /// What they are spoken as.
    pub replacement: Replacement,
}

/// The lexemes of the lexicons a request named, in the order it named them.
///
/// A word is looked up in that order, so the FIRST lexicon that defines a grapheme is
/// the one that applies to it — which is what makes naming several lexicons compose:
/// each one contributes the graphemes the earlier ones do not define.
#[derive(Debug, Clone, Default)]
pub struct Lexicons {
    /// Every lexeme of every named lexicon, in request order.
    lexemes: Vec<Lexeme>,
}

impl Lexicons {
    /// Add the lexemes of one lexicon's PLS document, after the lexicons already added.
    pub fn add(&mut self, content: &str) {
        self.lexemes.extend(lexemes(content));
    }

    /// What `word` is spoken as, or `None` when no lexeme matches it.
    ///
    /// A grapheme matches the word as WRITTEN: PLS matches the orthography it was given,
    /// so a lexeme for one spelling does not silently apply to another.
    pub fn replacement_for(&self, word: &str) -> Option<&Replacement> {
        self.lexemes
            .iter()
            .find(|lexeme| lexeme.graphemes.iter().any(|grapheme| grapheme == word))
            .map(|lexeme| &lexeme.replacement)
    }

    /// Whether any named lexicon defines a lexeme at all.
    pub fn is_empty(&self) -> bool {
        self.lexemes.is_empty()
    }
}

/// Which element's character data is being collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collecting {
    Grapheme,
    Alias,
    Pronunciation,
}

/// The lexemes one PLS document declares.
///
/// A document `PutLexicon` would have refused cannot be stored, so anything unreadable
/// here is read as "no lexemes" rather than as a failure: a lexicon that defines nothing
/// changes nothing about how the text is spoken.
fn lexemes(content: &str) -> Vec<Lexeme> {
    use quick_xml::events::Event;
    use quick_xml::name::ResolveResult;

    let mut reader = quick_xml::NsReader::from_str(content);
    let mut found: Vec<Lexeme> = Vec::new();
    // The document element's `alphabet`, which is the alphabet a `<phoneme>` that
    // declares none is written in.
    let mut document_alphabet = String::new();
    let mut in_lexeme = false;
    let mut graphemes: Vec<String> = Vec::new();
    let mut alias: Option<String> = None;
    let mut pronunciation: Option<String> = None;
    let mut pronunciation_alphabet = String::new();
    let mut collecting: Option<Collecting> = None;

    while let Ok((resolved, event)) = reader.read_resolved_event() {
        let in_pls = matches!(
            resolved,
            ResolveResult::Bound(namespace) if namespace.0 == PLS_NAMESPACE.as_bytes()
        );
        match event {
            Event::Eof => break,
            Event::Start(ref element) | Event::Empty(ref element) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if !in_pls {
                    continue;
                }
                match name.as_str() {
                    "lexicon" => {
                        if let Some(declared) = attribute(element, "alphabet") {
                            document_alphabet = declared;
                        }
                    }
                    "lexeme" => {
                        in_lexeme = true;
                        graphemes.clear();
                        alias = None;
                        pronunciation = None;
                        pronunciation_alphabet = document_alphabet.clone();
                    }
                    "grapheme" if in_lexeme => {
                        graphemes.push(String::new());
                        collecting = Some(Collecting::Grapheme);
                    }
                    "alias" if in_lexeme => {
                        alias = Some(String::new());
                        collecting = Some(Collecting::Alias);
                    }
                    "phoneme" if in_lexeme => {
                        if let Some(declared) = attribute(element, "alphabet") {
                            pronunciation_alphabet = declared;
                        }
                        pronunciation = Some(String::new());
                        collecting = Some(Collecting::Pronunciation);
                    }
                    _ => {}
                }
                // An empty element carries no character data, so nothing is collected
                // into it.
                if matches!(event, Event::Empty(_)) {
                    collecting = None;
                }
            }
            Event::Text(ref text) => {
                let raw = String::from_utf8_lossy(text.as_ref()).to_string();
                let data = quick_xml::escape::unescape(&raw)
                    .map(|unescaped| unescaped.to_string())
                    .unwrap_or(raw);
                match collecting {
                    Some(Collecting::Grapheme) => {
                        if let Some(last) = graphemes.last_mut() {
                            last.push_str(&data);
                        }
                    }
                    Some(Collecting::Alias) => {
                        if let Some(value) = alias.as_mut() {
                            value.push_str(&data);
                        }
                    }
                    Some(Collecting::Pronunciation) => {
                        if let Some(value) = pronunciation.as_mut() {
                            value.push_str(&data);
                        }
                    }
                    None => {}
                }
            }
            Event::End(ref element) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if !in_pls {
                    continue;
                }
                match name.as_str() {
                    "grapheme" | "alias" | "phoneme" => collecting = None,
                    "lexeme" => {
                        in_lexeme = false;
                        let written: Vec<String> = graphemes
                            .iter()
                            .map(|grapheme| grapheme.trim().to_string())
                            .filter(|grapheme| !grapheme.is_empty())
                            .collect();
                        // A `<phoneme>` is the pronunciation itself and takes
                        // precedence over an `<alias>`, which is text that still has to
                        // be read; a lexeme declaring neither replaces nothing.
                        let replacement = pronunciation
                            .as_deref()
                            .map(|spelling| spelling.trim())
                            .filter(|spelling| !spelling.is_empty())
                            .and_then(|spelling| {
                                phonemes::from_phonetic(spelling, &pronunciation_alphabet)
                                    .map(Replacement::Pronunciation)
                            })
                            .or_else(|| {
                                alias
                                    .as_deref()
                                    .map(|text| text.trim())
                                    .filter(|text| !text.is_empty())
                                    .map(|text| Replacement::Alias(text.to_string()))
                            });
                        if let (false, Some(replacement)) = (written.is_empty(), replacement) {
                            found.push(Lexeme {
                                graphemes: written,
                                replacement,
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    found
}

/// One attribute of an element, by name.
fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        (attribute.key.as_ref() == name.as_bytes())
            .then(|| String::from_utf8_lossy(attribute.value.as_ref()).to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PLS document with `body` as its lexemes.
    fn pls(body: &str) -> String {
        format!(
            "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"1.0\" alphabet=\"ipa\" \
             xml:lang=\"en-US\">{body}</lexicon>"
        )
    }

    /// RULE: a lexeme's graphemes are matched as written and answer the replacement the
    /// document gives them.
    #[test]
    fn lexemes_are_read_out_of_the_document() {
        let mut lexicons = Lexicons::default();
        lexicons.add(&pls(
            "<lexeme><grapheme>AWS</grapheme><grapheme>aws</grapheme>\
             <alias>Amazon Web Services</alias></lexeme>\
             <lexeme><grapheme>tomato</grapheme><phoneme>təˈmeɪtoʊ</phoneme></lexeme>\
             <lexeme><grapheme>empty</grapheme></lexeme>",
        ));
        for grapheme in ["AWS", "aws"] {
            match lexicons.replacement_for(grapheme) {
                Some(Replacement::Alias(alias)) => assert_eq!(
                    alias, "Amazon Web Services",
                    "grapheme {grapheme:?}: wrong alias"
                ),
                other => panic!("grapheme {grapheme:?}: expected an alias, got {other:?}"),
            }
        }
        match lexicons.replacement_for("tomato") {
            Some(Replacement::Pronunciation(spoken)) => {
                let ids: Vec<&str> = spoken.iter().map(|ph| ph.id).collect();
                assert_eq!(
                    ids,
                    vec!["t", "ax", "m", "ey", "t", "ow"],
                    "a phoneme replacement is read as its pronunciation, got {ids:?}"
                );
            }
            other => panic!("expected a pronunciation, got {other:?}"),
        }
        assert!(
            lexicons.replacement_for("empty").is_none(),
            "a lexeme with no replacement replaces nothing"
        );
        assert!(
            lexicons.replacement_for("Aws").is_none(),
            "a grapheme matches the spelling it was written with"
        );
    }

    /// RULE: the lexicons are consulted in the order the request named them, and an
    /// element of another vocabulary is not a lexeme.
    #[test]
    fn earlier_lexicons_win_and_foreign_elements_are_ignored() {
        let mut lexicons = Lexicons::default();
        lexicons.add(&pls(
            "<lexeme><grapheme>G</grapheme><alias>first</alias></lexeme>",
        ));
        lexicons.add(&pls(
            "<lexeme><grapheme>G</grapheme><alias>second</alias></lexeme>\
             <lexeme><grapheme>H</grapheme><alias>only here</alias></lexeme>",
        ));
        match lexicons.replacement_for("G") {
            Some(Replacement::Alias(alias)) => {
                assert_eq!(alias, "first", "the first lexicon named wins")
            }
            other => panic!("expected an alias, got {other:?}"),
        }
        assert!(
            lexicons.replacement_for("H").is_some(),
            "a later lexicon still contributes the graphemes the first does not define"
        );

        let mut foreign = Lexicons::default();
        foreign.add(
            "<lexicon xmlns=\"http://example.invalid/other\"><lexeme><grapheme>G</grapheme>\
             <alias>no</alias></lexeme></lexicon>",
        );
        assert!(
            foreign.is_empty(),
            "an element outside the PLS namespace is not a lexeme"
        );
    }
}
