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
//! is recorded answering.

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
