// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of WHERE a JSON document a caller submitted stopped parsing.
//!
//! Several AWS services take a JSON document as an opaque string member — an IAM policy
//! document, a message filter policy — and refuse one they cannot parse with their JSON
//! parser's own diagnostic rather than with a restatement of the member's type. Those
//! diagnostics all say the same three things: what the parse stopped ON, what a value could
//! have been there instead, and the line and column it stopped at. The SENTENCE each service
//! frames them in is its own (the services differ in what they prefix and in which fault
//! classes they report), but the three readings are the same question about the same
//! document, so they are stated once here: two services handed the same document cannot
//! disagree about where it went wrong.
//!
//! Everything here derives from [`serde_json::Error`], which is the parse that actually ran:
//! `line()` and `column()` are where it stopped, and its rendered cause is why. Nothing
//! re-scans the document looking for a likely fault.
//!
//! The reported position is ONE-based, and a column counts BYTES from the start of its line,
//! which is the convention the parser reports in.

/// The token classes a JSON value may begin with, as a parser's diagnostic lists them.
///
/// A JSON value is a string, a number, an array, an object or one of the three literal
/// tokens, and a diagnostic about a value position lists exactly those. The list is stated
/// ONCE here because the services that quote it frame it in sentences of their own — one
/// says `was expecting a valid value (<classes>)` where the parse stopped on a character and
/// another `Unrecognized token '<word>': was expecting (<classes>)` where it stopped on a
/// word — so the sentence is the caller's and the classes are not.
pub const VALUE_CLASSES: &str = "JSON String, Number, Array, Object or token 'null', 'true' or 'false'";

/// The clause a diagnostic carries where the parse stopped at a VALUE position: the classes
/// [`VALUE_CLASSES`] lists, in the sentence this module frames them in.
///
/// Spelled out because the lookup below answers with one `&'static str`; the test at the foot
/// of this module checks the clause carries [`VALUE_CLASSES`], so the two cannot drift apart.
const VALUE_POSITION: &str =
    "was expecting a valid value (JSON String, Number, Array, Object or token 'null', 'true' \
     or 'false')";

/// The cause of `error` without the position it renders after it.
///
/// `serde_json` renders a failure as `<cause> at line <l> column <c>`; the position is
/// reported on its own, so only the cause is wanted from the text.
#[must_use]
pub fn cause(error: &serde_json::Error) -> String {
    let rendered = error.to_string();
    rendered
        .split(" at line ")
        .next()
        .unwrap_or(&rendered)
        .trim()
        .to_string()
}

/// The byte offset in `document` of the position `line`:`column`, or `None` when the
/// document holds no such position — which includes the position one past its end, where a
/// document that stopped mid-value fails.
#[must_use]
pub fn byte_offset(document: &str, line: usize, column: usize) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut start = 0;
    for _ in 1..line {
        start += document[start..].find('\n')? + 1;
    }
    let offset = start + column - 1;
    (offset < document.len()).then_some(offset)
}

/// The character the parse stopped on: the `column`th byte of the `line`th line of
/// `document`, both counted from one, and `None` when the position is past its end.
///
/// A column counts bytes, so a position inside a multi-byte character names that character.
#[must_use]
pub fn character_at(document: &str, line: usize, column: usize) -> Option<char> {
    let line_start = byte_offset(document, line, 1)?;
    let mut offset = byte_offset(document, line, column)?;
    while offset > line_start && !document.is_char_boundary(offset) {
        offset -= 1;
    }
    document[offset..].chars().next()
}

/// The unquoted word the parse stopped on where a VALUE belongs, with the line and column
/// that word begins at — or `None` when the parse stopped on something else.
///
/// A JSON value is a string, a number, an array, an object or one of the three literals, so
/// a bare word where a value belongs is a word the parser does not recognise, and a parser
/// that names it names the WHOLE word and the position it STARTS at. The word is read at the
/// position the parse stopped: a parse that stopped while matching a literal is already a
/// character or more into the word, so the word is extended backwards to where it starts as
/// well as forwards to where it ends.
///
/// `None` covers every other fault — a document that ends early, a stray punctuation mark, a
/// bare word where a member NAME belongs (which is a fault about the missing quote, not about
/// the word) — each of which a caller reports in its own words.
#[must_use]
pub fn unrecognized_token<'a>(
    document: &'a str,
    error: &serde_json::Error,
) -> Option<(&'a str, usize, usize)> {
    if !expected_a_value(&cause(error)) {
        return None;
    }
    let stopped_at = byte_offset(document, error.line(), error.column())?;
    if !document.is_char_boundary(stopped_at) {
        return None;
    }
    let start = document[..stopped_at]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word_character(*character))
        .map(|(offset, _)| offset)
        .last()
        .unwrap_or(stopped_at);
    let word = &document[start..];
    if !word.chars().next().is_some_and(is_word_start) {
        return None;
    }
    let end = word
        .find(|character: char| !is_word_character(character))
        .unwrap_or(word.len());
    let (line, column) = position_of(document, start);
    Some((&word[..end], line, column))
}

/// Whether `cause` is a parse failure where a VALUE was expected: nothing that can start one
/// was there, or what was there began a literal and then did not spell it.
fn expected_a_value(cause: &str) -> bool {
    cause == "expected value" || cause == "expected ident"
}

/// Whether `character` may begin an unquoted word: a letter, or one of the two marks an
/// identifier starts with. A digit begins a NUMBER, whose own fault is not a word.
fn is_word_start(character: char) -> bool {
    character.is_alphabetic() || character == '_' || character == '$'
}

/// Whether `character` may be part of an unquoted word, once one has begun.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// The one-based line, and the one-based column in bytes, of byte `offset` of `document`.
fn position_of(document: &str, offset: usize) -> (usize, usize) {
    let before = &document[..offset];
    let line = 1 + before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |at| at + '\n'.len_utf8());
    (line, 1 + offset - line_start)
}

/// What the parse stopped on, as a diagnostic names it: the offending character and its
/// character code, or the end of the input, and what was expected in its place.
///
/// The caller frames it in its own sentence and appends [`source_location`].
#[must_use]
pub fn unexpected_input(document: &str, error: &serde_json::Error) -> String {
    let cause = cause(error);
    // A document that ends mid-value has no offending character to name: the position is
    // the end of what was submitted.
    let offender = if cause.starts_with("EOF while parsing") {
        None
    } else {
        character_at(document, error.line(), error.column())
    };
    let expected = expected_where_parsing_stopped(&cause, offender);
    match offender {
        Some(offender) => format!(
            "Unexpected character ('{}' (code {})): {expected}",
            render_character(offender),
            u32::from(offender)
        ),
        None => format!("Unexpected end-of-input: {expected}"),
    }
}

/// What the parser expected where it stopped, from its own diagnosis of the failure.
///
/// A cause with no phrasing of its own is reported as the parser states it, so a failure this
/// list does not name still describes itself.
#[must_use]
pub fn expected_where_parsing_stopped(cause: &str, offender: Option<char>) -> &str {
    // A comma is trailing because what follows it closes the collection, so what was
    // expected depends on which collection that is: an object wants its next field name, an
    // array its next value.
    let cause = match (cause, offender) {
        ("trailing comma", Some(']')) => "expected value",
        ("trailing comma", _) => "key must be a string",
        (cause, _) => cause,
    };
    match cause {
        "key must be a string" => "was expecting double-quote to start field name",
        "expected value" => VALUE_POSITION,
        "expected ident" => "was expecting a valid token ('null', 'true' or 'false')",
        "expected `:`" => "was expecting a colon to separate field name and value",
        "expected `,` or `}`" => "was expecting comma to separate Object entries",
        "expected `,` or `]`" => "was expecting comma to separate Array entries",
        "trailing characters" => "was expecting end of the document",
        "EOF while parsing an object" => "expected close marker for Object",
        "EOF while parsing a list" => "expected close marker for Array",
        "EOF while parsing a string" => "expected close marker for String",
        "EOF while parsing a value" => "expected a value",
        other => other,
    }
}

/// A character as a diagnostic prints it: itself when it is printable, an escape when it is a
/// control character that would otherwise be invisible in the message.
#[must_use]
pub fn render_character(character: char) -> String {
    if character.is_control() {
        character.escape_default().to_string()
    } else {
        character.to_string()
    }
}

/// Where the parse stopped, as a diagnostic's location clause reports it.
///
/// The clause names the line and the column but NOT the document: a service does not echo a
/// submitted document back in its refusal, so the source is reported as redacted.
#[must_use]
pub fn source_location(line: usize, column: usize) -> String {
    format!(
        "\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` \
         disabled); line: {line}, column: {column}]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parse failure of `document`, or a panic when it parses.
    fn failure(document: &str) -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>(document)
            .expect_err("the document must not parse")
    }

    /// One row: what a failing assertion calls it, the document, and the word the parse
    /// stopped on with the position it starts at (`None` when the fault names no word).
    type WordRow = (
        &'static str,
        &'static str,
        Option<(&'static str, usize, usize)>,
    );

    /// The rows differ along what the reading generalises over: whether the fault is a word
    /// at all, how far into the word the parse got, where the word sits (the first value, a
    /// member's value, a later line), and which other fault class the document carries.
    #[test]
    fn the_word_a_parse_stopped_on_is_read_at_the_position_the_parser_reports() {
        let rows: [WordRow; 11] = [
            // A word that begins a literal and then does not spell it: the parse is already
            // inside the word when it stops.
            ("a bare word of its own", "this-is-not-json", Some(("this", 1, 1))),
            ("a bare word spelling no literal", "nope", Some(("nope", 1, 1))),
            (
                "a bare word where a member's value belongs",
                r#"{"event": created}"#,
                Some(("created", 1, 11)),
            ),
            (
                "a bare word on the second line",
                "{\n  \"event\": oops\n}",
                Some(("oops", 2, 12)),
            ),
            (
                "a word a template left unsubstituted",
                r#"{"event": $event}"#,
                Some(("$event", 1, 11)),
            ),
            (
                "a literal spelled short",
                r#"{"event": tru}"#,
                Some(("tru", 1, 11)),
            ),
            // Faults that are not a word where a value belongs.
            ("a stray comma", r#"{"a": [1,, 2], "b": 3}"#, None),
            ("a missing comma between members", r#"{"a": 1 "b": 2}"#, None),
            ("an unquoted member name", r#"{event: 1}"#, None),
            ("a document that ends early", "{", None),
            ("an empty document", "", None),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = unrecognized_token(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} (the parser stopped at \
                 line {}, column {}: {})",
                error.line(),
                error.column(),
                cause(&error)
            );
        }
    }

    /// A fault that names no word still describes itself: the character it stopped on, its
    /// code, and what was expected there.
    #[test]
    fn a_fault_that_names_no_word_names_the_character_it_stopped_on() {
        // (case, the document, what the diagnostic must say)
        let rows: [(&str, &str, &str); 3] = [
            (
                "a missing comma between members",
                r#"{"a": 1 "b": 2}"#,
                "Unexpected character ('\"' (code 34)): was expecting comma to separate Object \
                 entries",
            ),
            (
                "an unquoted member name",
                r#"{event: 1}"#,
                "Unexpected character ('e' (code 101)): was expecting double-quote to start \
                 field name",
            ),
            (
                "a document that ends early",
                r#"{"a": 1"#,
                "Unexpected end-of-input: expected close marker for Object",
            ),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = unexpected_input(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The location clause names the position and never the document.
    #[test]
    fn the_location_clause_names_the_position_and_not_the_document() {
        let clause = source_location(2, 7);
        assert!(
            clause.starts_with("\n at [Source: REDACTED "),
            "the source must be reported as redacted, got {clause:?}"
        );
        assert!(
            clause.ends_with("line: 2, column: 7]"),
            "the clause must name the position it was given, got {clause:?}"
        );
    }

    /// The clause for a value position lists the classes this module states once, so the
    /// sentence and the list cannot drift apart.
    #[test]
    fn the_value_position_clause_lists_the_classes_a_value_may_begin_with() {
        assert!(
            VALUE_POSITION.contains(VALUE_CLASSES),
            "the clause {VALUE_POSITION:?} must list {VALUE_CLASSES:?}"
        );
    }
}
