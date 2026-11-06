// SPDX-License-Identifier: CC0-1.0

//! Whether an invocation's payload is the JSON an invocation carries, and WHY a payload
//! that is not is refused.
//!
//! Every operation that takes an event — `Invoke`, `InvokeAsync`,
//! `InvokeWithResponseStream` — documents its payload as "The JSON that you want to provide
//! to your Lambda function as input", so a payload that is not JSON never reaches a
//! function and is refused with the modelled `InvalidRequestContentException`.
//!
//! The refusal carries the parse DIAGNOSTIC, not just the fact of the failure: it is the
//! only part of the answer that tells a caller which byte of its event to fix. The READINGS
//! it is built from — the unquoted word a parse stopped on where a value belongs, the
//! character or the end of input it stopped on otherwise together with what was expected
//! there, the position it stopped at, and the mark of the container it was still inside —
//! are the tree's shared ones ([`::service_common::json_parse_fault`]), taken from the parse
//! that actually ran, so this service and every other one handed the same payload cannot
//! disagree about where it went wrong. A fault class worded wrongly is corrected THERE, for
//! every service that quotes it.
//!
//! What is this service's own is the frame: the payload is a document INSIDE the request
//! body, so the diagnostic is prefixed twice — once for the body
//! ([`REQUEST_BODY_PREFIX`]) and once for the payload in it ([`PAYLOAD_PREFIX`]) — and the
//! clause for the position the parse ended at follows the sentence, with the payload's own
//! source WITHHELD: an event is the caller's and none of it is echoed back.

use ::service_common::json_parse_fault as fault;

/// The outer prefix: the request body is what could not be parsed.
const REQUEST_BODY_PREFIX: &str = "Could not parse request body into json";

/// The inner prefix: the payload INSIDE that body is what could not be parsed.
const PAYLOAD_PREFIX: &str = "Could not parse payload into json";

/// The message an invocation payload that is not JSON is refused with, or `None` when the
/// payload IS JSON — and for an EMPTY payload, which is the documented no-event invocation
/// rather than a body with nothing in it.
#[must_use]
pub fn unparseable_payload_message(payload: &[u8]) -> Option<String> {
    if payload.is_empty() {
        return None;
    }
    let error = ::serde_json::from_slice::<::serde_json::Value>(payload).err()?;
    // The parse reads the payload as text; a byte sequence that is not text at all is no
    // JSON either, and the position it stopped at is stated over the text the reader saw.
    let payload = ::std::string::String::from_utf8_lossy(payload);
    Some(format!(
        "{REQUEST_BODY_PREFIX}: {PAYLOAD_PREFIX}: {}",
        diagnostic(&payload, &error)
    ))
}

/// Where `payload` stopped parsing, in the words this service's refusals quote: the fault,
/// the start marker of a container the payload left open, and the position the parse ended
/// at — each clause reporting its source as redacted.
///
/// `error` is the parse that refused `payload`: call this where that parse failed, and
/// nowhere else.
fn diagnostic(payload: &str, error: &::serde_json::Error) -> String {
    // A payload that stops on an unquoted word where a VALUE belongs is reported by naming
    // that word and the position the WORD starts at, which is what lets a caller whose
    // event was built by templating or concatenation locate the fault; every other fault
    // names the character the parse stopped on, or the end of the input, and the position
    // the parse itself ended at.
    let (reading, line, column) = match fault::unrecognized_token(payload, error) {
        Some((word, line, column)) => (
            format!(
                "Unrecognized token '{word}': was expecting ({})",
                fault::VALUE_CLASSES
            ),
            line,
            column,
        ),
        None => {
            let (line, column) = fault::stopped_at(payload, error);
            (fault::unexpected_input(payload, error), line, column)
        }
    };
    let framed = match fault::unclosed_container_start(payload, error) {
        Some((marker_line, marker_column)) => format!(
            "{reading} (start marker at {})",
            fault::location_clause(fault::REDACTED_SOURCE, marker_line, marker_column)
        ),
        None => reading,
    };
    format!("{framed}{}", fault::source_location(line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a payload that is not JSON is refused with the two prefixes, the fault the
    /// parse stopped on and the position it stopped at — and a payload that IS JSON, or is
    /// empty, is not refused at all.
    ///
    /// The rows span what the rule generalises over: the class of fault (a bare word, a
    /// character no value starts with, a container left open, a string left open, a payload
    /// with no content), where in the payload the fault is (its first value, and inside an
    /// object), and which LINE it is on, which the location clause carries beside the
    /// column.
    #[test]
    fn a_payload_that_is_not_json_is_refused_with_the_fault_the_parse_stopped_on() {
        let redacted = fault::REDACTED_SOURCE;
        let value_classes = fault::VALUE_CLASSES;
        let cases: &[(&str, &[u8], Option<String>)] = &[
            ("an object", br#"{"k":1}"#, None),
            ("an array", b"[1,2]", None),
            ("a bare number", b"12", None),
            ("a bare keyword", b"null", None),
            ("an empty payload", b"", None),
            (
                "a bare word",
                b"not json",
                Some(format!(
                    "Unrecognized token 'not': was expecting ({value_classes})\n at [Source: \
                     {redacted}; line: 1, column: 1]"
                )),
            ),
            (
                "a character no value starts with",
                b"*",
                Some(format!(
                    "Unexpected character ('*' (code 42)): was expecting a valid value \
                     ({value_classes})\n at [Source: {redacted}; line: 1, column: 1]"
                )),
            ),
            (
                "a bare word on the third line",
                b"\n\n  not json",
                Some(format!(
                    "Unrecognized token 'not': was expecting ({value_classes})\n at [Source: \
                     {redacted}; line: 3, column: 3]"
                )),
            ),
            (
                "a bare word where a member's value belongs",
                br#"{"k": nope}"#,
                Some(format!(
                    "Unrecognized token 'nope': was expecting ({value_classes})\n at [Source: \
                     {redacted}; line: 1, column: 7]"
                )),
            ),
            (
                "an object left open",
                br#"{"k": 1"#,
                Some(format!(
                    "Unexpected end-of-input: expected close marker for Object (start marker \
                     at [Source: {redacted}; line: 1, column: 1])\n at [Source: {redacted}; \
                     line: 1, column: 8]"
                )),
            ),
            (
                "an array left open inside an object",
                br#"{"k": [1, 2"#,
                Some(format!(
                    "Unexpected end-of-input: expected close marker for Array (start marker \
                     at [Source: {redacted}; line: 1, column: 7])\n at [Source: {redacted}; \
                     line: 1, column: 12]"
                )),
            ),
            (
                "a member whose value never came",
                br#"{"k":"#,
                Some(format!(
                    "Unexpected end-of-input: expected a value\n at [Source: {redacted}; \
                     line: 1, column: 6]"
                )),
            ),
            (
                "a string left open",
                br#""event"#,
                Some(format!(
                    "Unexpected end-of-input: expected close marker for String\n at [Source: \
                     {redacted}; line: 1, column: 7]"
                )),
            ),
            (
                "whitespace and nothing else",
                b"  \n ",
                Some(format!(
                    "Unexpected end-of-input: expected a value\n at [Source: {redacted}; \
                     line: 2, column: 2]"
                )),
            ),
        ];

        let frame = format!("{REQUEST_BODY_PREFIX}: {PAYLOAD_PREFIX}: ");
        for (case, payload, expected) in cases {
            let refused = unparseable_payload_message(payload);
            let stated = refused.as_ref().map(|message| {
                message
                    .strip_prefix(&frame)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected the two prefixes, got {message:?}")
                    })
                    .to_string()
            });
            assert_eq!(
                stated.as_deref(),
                expected.as_deref(),
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
