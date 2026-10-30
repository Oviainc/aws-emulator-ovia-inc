// SPDX-License-Identifier: CC0-1.0

//! This service's SENTENCE for a document it could not read as JSON.
//!
//! Every refusal about a document the caller sent — an execution `input`, a state
//! machine `definition`, the input a state is tested on — quotes a parser DIAGNOSTIC
//! inside the wrapper [`crate::state::resource_error_message`] builds. The two
//! READINGS that diagnostic is built from — the unquoted word a parse stopped on
//! where a value belongs, and the character or end-of-input it stopped on otherwise,
//! with what was expected there — are the tree's shared ones
//! ([`::service_common::json_parse_fault`]), taken from the parse that actually ran,
//! so this service and every other one handed the same document cannot disagree
//! about where it went wrong. A fault class worded wrongly or missing is corrected
//! THERE, for every service that quotes it.
//!
//! What is this service's own is the sentence those readings are framed in: it names
//! the word and the classes a value could have begun with — the shared
//! [`::service_common::json_parse_fault::VALUE_CLASSES`], so no two services list
//! them differently — and it carries NO location clause, which is what this service
//! is recorded answering for an execution `input` and for a state machine
//! `definition`.
//!
//! ONE refusal of this service does carry one: `TestState`'s, for the input a state
//! is tested on, which is recorded carrying the parser's own source-location clause
//! after the diagnostic ([`diagnostic_with_source`]).

/// Where `document` stopped parsing, in the words this service's refusals quote.
///
/// ONE construction for every document this service reads, so no two of them report
/// a parse failure differently. A document that stops on an unquoted word where a
/// VALUE belongs is reported by naming that word and the classes a value could have
/// begun with — RECORDED text: the service answers `Unrecognized token 'not': was
/// expecting (JSON String, Number, Array, Object or token 'null', 'true' or
/// 'false')` for a document that opens on a bare word — which is what lets a caller
/// whose payload was built by templating or concatenation locate the fault; and
/// every other fault names the character the parse stopped on, or the end of the
/// input, and what was expected there. The position a parser's cursor stopped at
/// names neither the word nor what was expected, so no bare parser string reaches a
/// message.
///
/// The quoted word is clipped by [`super::asl::echoed`], like every other caller
/// value this crate quotes, so a refusal cannot be made much larger than the
/// request that caused it.
///
/// `error` is the parse that refused `document`: call this where that parse failed,
/// and nowhere else.
pub fn diagnostic(document: &str, error: &serde_json::Error) -> String {
    use ::service_common::json_parse_fault as fault;

    match fault::unrecognized_token(document, error) {
        Some((word, _, _)) => format!(
            "Unrecognized token '{}': was expecting ({})",
            super::asl::echoed(word),
            fault::VALUE_CLASSES
        ),
        None => fault::unexpected_input(document, error),
    }
}

/// The most characters of the submitted document a source-location clause quotes.
///
/// A document is caller-sized (the modelled `SensitiveData` is 256 KiB), so the
/// clause quotes a SNIPPET of it and counts the characters it dropped, the way the
/// parser whose clause this is does: a refusal cannot be made much larger than the
/// request that caused it.
const MAX_SOURCE_CHARS: usize = 500;

/// [`diagnostic`] with the parser's SOURCE LOCATION clause after it: the document
/// the parse was reading, and where in it the parse stopped.
///
/// `TestState` is recorded carrying this clause in its `input` refusal, where the
/// operations that start an execution with an input are recorded carrying the
/// diagnostic alone — which is why the clause is a construction of its own rather
/// than part of [`diagnostic`].
///
/// The position is the parse's own, as the shared readings report it: the start of
/// the word for a document that stopped on an unquoted word (the word is named in
/// full, so the position is where it begins), and the line and column the parse
/// reached otherwise, counted from one and in BYTES. The quoted source is the
/// document as submitted, clipped by [`quoted_source`].
///
/// `error` is the parse that refused `document`: call this where that parse failed,
/// and nowhere else.
pub fn diagnostic_with_source(document: &str, error: &serde_json::Error) -> String {
    use ::service_common::json_parse_fault as fault;

    let (line, column) = match fault::unrecognized_token(document, error) {
        Some((_, line, column)) => (line, column),
        None => (error.line(), error.column()),
    };
    format!(
        "{}\n at [Source: (String){}; line: {line}, column: {column}]",
        diagnostic(document, error),
        quoted_source(document)
    )
}

/// `document` as the source-location clause quotes it: in quotes when it is short,
/// and its first [`MAX_SOURCE_CHARS`] characters in quotes followed by the count of
/// the characters dropped when it is not.
fn quoted_source(document: &str) -> String {
    let characters = document.chars().count();
    if characters <= MAX_SOURCE_CHARS {
        return format!("\"{document}\"");
    }
    let kept: String = document.chars().take(MAX_SOURCE_CHARS).collect();
    format!("\"{kept}\"[truncated {} chars]", characters - MAX_SOURCE_CHARS)
}
