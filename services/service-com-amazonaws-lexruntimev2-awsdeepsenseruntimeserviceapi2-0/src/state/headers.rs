// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's structured `x-amz-lex-*` HTTP headers, read
//! and written in ONE place so the request side and the response side of the
//! runtime cannot disagree about it.
//!
//! Several members of this service are modelled as a plain string carried on a
//! header while the value they hold is a structure, a list or a map. The model
//! states the encoding that bridges the two, and states it for every one of
//! them: `RecognizeUtterance` says "the following request fields must be
//! compressed with gzip and then base64 encoded before you send them to Amazon
//! Lex V2 — `requestAttributes`, `sessionState`" and "the following response
//! fields are compressed using gzip and then base64 encoded by Amazon Lex V2 —
//! `inputTranscript`, `interpretations`, `messages`, `requestAttributes`,
//! `sessionState`", and each `PutSession` header member repeats it on its own
//! ("the `sessionState` field is compressed with gzip and then base64
//! encoded"). Source: `smithy_query(query="doc", shape="RecognizeUtteranceRequest",
//! member="sessionState")` and the same query for each member named above.
//!
//! So one rule covers all of them: the payload of such a header is the member's
//! own JSON — the structure's, written with the service's own `restJson1` codec
//! and the model's schemas, or the raw UTF-8 text where the member is text —
//! compressed with gzip and then base64 encoded. The functions below are that
//! rule; nothing else in this crate encodes or decodes a header payload.

use aws_smithy_schema::codec::FinishSerializer;
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::json::{JsonShapeDeserializer, JsonShapeSerializer};
use aws_smithy_schema_server::DeserializableStruct;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashMap;
use std::io::{Read, Write};

/// The most bytes one header payload may inflate to.
///
/// A resource bound, not a modelled quota: the compressed payload arrives from
/// the caller, and a few kilobytes of gzip can inflate to gigabytes, so the
/// inflate is stopped at a cap instead of being allowed to allocate whatever the
/// stream asks for. The cap is far above anything a session can legitimately
/// carry — [`super::MAX_ATTRIBUTE_BYTES`] bounds the attribute maps inside it at
/// 12 KB — so no real request is refused by it.
pub const MAX_HEADER_PAYLOAD_BYTES: usize = 256 * 1024;

/// A header payload the caller has to fix: it is not the encoding the model
/// documents, or what it decodes to is not the member's modelled shape.
///
/// Carries the member and the rule it broke rather than a rendered sentence, so
/// the operation that refuses it words its own `ValidationException`.
#[derive(Debug, Clone)]
pub struct HeaderRejected {
    /// The member the caller has to fix, by its Smithy member name.
    pub member: &'static str,
    /// What is wrong with it, in terms of the step that failed.
    pub reason: String,
}

impl HeaderRejected {
    /// A refusal naming the member and the step of the documented encoding that
    /// failed, so the request can be fixed from the message alone.
    pub fn message(&self) -> String {
        format!(
            "{} must be the gzip-compressed, base64-encoded JSON of its modelled shape: {}",
            self.member, self.reason
        )
    }

    /// Build a refusal of `member` with `reason`.
    fn new(member: &'static str, reason: impl Into<String>) -> Self {
        Self {
            member,
            reason: reason.into(),
        }
    }
}

/// gzip `payload` and base64 the result: the encoding every structured header
/// value of this service carries.
///
/// The gzip header is written with no modification time and no file name
/// (`flate2`'s default), so the same payload always encodes to the same bytes —
/// a header value is part of the response a caller compares, not a per-run
/// value.
fn encode_bytes(payload: &[u8]) -> Result<String, String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(payload)
        .map_err(|e| format!("compressing a response header: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("compressing a response header: {e}"))?;
    Ok(aws_smithy_types::base64::encode(compressed))
}

/// Encode one modelled value for a header: serialize it to JSON with the
/// service's own `restJson1` codec, then [`encode_bytes`].
///
/// `write` is handed the serializer so the caller names the model's own schema
/// for the value (a struct, a list or a map), which is what keeps the JSON keys
/// the wire names — `@jsonName` included — that a client decoding the header
/// expects.
pub fn encode_json(
    write: impl FnOnce(&mut JsonShapeSerializer) -> Result<(), SerdeError>,
) -> Result<String, String> {
    // `true`: restJson1 honours `@jsonName`.
    let mut serializer = JsonShapeSerializer::new_with_config(true);
    write(&mut serializer).map_err(|e| format!("serializing a response header: {e}"))?;
    encode_bytes(&serializer.finish())
}

/// Encode a TEXT header payload: the member's own characters, with no JSON
/// quoting around them.
///
/// `RecognizeUtteranceResponse$inputTranscript` is modelled as a string whose
/// value is "the text used to process the request", and the gzip-then-base64
/// rule applies to it like any other header payload — but there is no structure
/// to serialize, so what is compressed is the text itself.
pub fn encode_text(text: &str) -> Result<String, String> {
    encode_bytes(text.as_bytes())
}

/// Decode one header value to the bytes it carries: base64, then gunzip, with
/// the inflate stopped at [`MAX_HEADER_PAYLOAD_BYTES`].
fn decode_bytes(member: &'static str, value: &str) -> Result<Vec<u8>, HeaderRejected> {
    let compressed = aws_smithy_types::base64::decode(value)
        .map_err(|e| HeaderRejected::new(member, format!("it is not base64 ({e})")))?;
    let mut payload = Vec::new();
    // The read is capped at one byte PAST the cap, so a payload that reaches
    // the cap is told apart from one that merely fills it exactly.
    let capped = u64::try_from(MAX_HEADER_PAYLOAD_BYTES).unwrap_or(u64::MAX);
    GzDecoder::new(compressed.as_slice())
        .take(capped.saturating_add(1))
        .read_to_end(&mut payload)
        .map_err(|e| HeaderRejected::new(member, format!("it is not a gzip stream ({e})")))?;
    if payload.len() > MAX_HEADER_PAYLOAD_BYTES {
        return Err(HeaderRejected::new(
            member,
            format!("it inflates to more than {MAX_HEADER_PAYLOAD_BYTES} bytes"),
        ));
    }
    Ok(payload)
}

/// Decode one header value into the modelled structure it carries.
///
/// The structure is read with the service's own `restJson1` codec and its own
/// schema, so a payload that is JSON but not this shape — a required member
/// missing, a member of the wrong type — is refused here rather than silently
/// dropped.
pub fn decode_struct<T: DeserializableStruct>(
    member: &'static str,
    value: &str,
) -> Result<T, HeaderRejected> {
    let payload = decode_bytes(member, value)?;
    // `true`: restJson1 honours `@jsonName`, so the keys read are the wire
    // names, exactly as they are written.
    let mut deserializer = JsonShapeDeserializer::new_with_config(&payload, true);
    T::deserialize(&mut deserializer)
        .map_err(|e| HeaderRejected::new(member, format!("it does not decode to the shape ({e})")))
}

/// Decode one header value into the modelled `map<string, string>` it carries.
///
/// `StringMap` is `map<NonEmptyString, String>`
/// (`smithy_query(query="shape", shape="StringMap")`): a JSON object whose keys
/// are non-empty and whose values are strings, and nothing else the model
/// constrains. So the payload is read straight into the map, a value that is not
/// a string is refused, and so is the one constraint the shape does carry — an
/// empty key.
pub fn decode_string_map(
    member: &'static str,
    value: &str,
) -> Result<HashMap<String, String>, HeaderRejected> {
    let payload = decode_bytes(member, value)?;
    let decoded: HashMap<String, String> = serde_json::from_slice(&payload).map_err(|e| {
        HeaderRejected::new(
            member,
            format!("it does not decode to a map of strings ({e})"),
        )
    })?;
    if decoded.keys().any(String::is_empty) {
        return Err(HeaderRejected::new(
            member,
            "its keys must each be at least 1 character long",
        ));
    }
    Ok(decoded)
}

/// The member of `schema` named `member`, as the model declares it.
///
/// A header payload is the JSON of a member of the operation's own input or
/// output, so its schema is read off that schema rather than re-declared.
pub fn member_schema<'a>(schema: &'a Schema, member: &str) -> Option<&'a Schema> {
    schema
        .members()
        .iter()
        .copied()
        .find(|candidate| candidate.member_name() == Some(member))
}

#[cfg(test)]
mod tests {
    //! The encoding itself: what is written is what is read back, and a value
    //! that is not the documented encoding is refused rather than guessed at.

    use super::*;
    use crate::ops::types::{DialogAction, SessionState};
    use aws_smithy_schema::serde::ShapeSerializer;

    /// RULE: a header payload round-trips — gzip-then-base64 out, base64-then-
    /// gunzip back in — for text of every size and character class a session
    /// can carry.
    #[test]
    fn a_text_payload_round_trips_through_the_documented_encoding() {
        // (case, text)
        let rows: [(&str, String); 6] = [
            ("the empty transcript", String::new()),
            ("one word", "flowers".to_string()),
            ("a sentence with spaces", "I would like to order flowers".to_string()),
            ("non-ASCII text", "Je voudrais des fleurs — s'il vous plaît".to_string()),
            ("text that compresses well", "ha".repeat(2_000)),
            // 1024 is the longest `Text` the sibling `RecognizeText` accepts,
            // so a transcript of that length is the longest a turn can report.
            ("the longest utterance a turn carries", "x".repeat(1_024)),
        ];

        for (case, text) in rows {
            let encoded = encode_text(&text).unwrap_or_else(|e| panic!("case {case:?}: {e}"));
            let decoded = decode_bytes("inputTranscript", &encoded)
                .unwrap_or_else(|e| panic!("case {case:?}: {}", e.message()));
            let actual = String::from_utf8(decoded)
                .unwrap_or_else(|e| panic!("case {case:?}: not UTF-8: {e}"));
            assert_eq!(
                actual, text,
                "case {case:?}: expected the payload back, got {actual:?}"
            );
            assert!(
                text.is_empty() || !encoded.contains(&text),
                "case {case:?}: the header must be encoded, not plain: {encoded}"
            );
        }
    }

    /// RULE: a structured payload round-trips through the same encoding, read
    /// back with the model's own schema.
    #[test]
    fn a_structured_payload_round_trips_through_the_documented_encoding() {
        let state = SessionState {
            dialog_action: Some(DialogAction {
                r#type: "ElicitSlot".to_string(),
                slot_to_elicit: Some("Size".to_string()),
                slot_elicitation_style: None,
                sub_slot_to_elicit: None,
            }),
            ..SessionState::default()
        };
        let schema = &SessionState::SCHEMA;
        let encoded = encode_json(|serializer| serializer.write_struct(schema, &state))
            .expect("the session state encodes");

        let decoded: SessionState =
            decode_struct("sessionState", &encoded).expect("the session state decodes");
        let actual = decoded
            .dialog_action
            .as_ref()
            .map(|action| (action.r#type.clone(), action.slot_to_elicit.clone()));
        assert_eq!(
            actual,
            Some(("ElicitSlot".to_string(), Some("Size".to_string()))),
            "the decoded dialog action must be the one encoded, got {decoded:?}"
        );
    }

    /// RULE: a header value that is not the documented encoding is refused, and
    /// the refusal names the member and the step that failed.
    #[test]
    fn a_value_that_is_not_the_documented_encoding_is_refused() {
        // (case, header value, the word the refusal must carry)
        let rows: [(&str, String, &str); 4] = [
            ("plain JSON, not encoded at all", "{\"x\":1}".to_string(), "base64"),
            ("base64 of plain JSON, not gzipped", aws_smithy_types::base64::encode("{}"), "gzip"),
            (
                "base64 of a truncated gzip stream",
                aws_smithy_types::base64::encode(
                    encode_text("truncated")
                        .and_then(|encoded| {
                            aws_smithy_types::base64::decode(&encoded)
                                .map_err(|e| e.to_string())
                                .map(|bytes| bytes[..bytes.len() / 2].to_vec())
                        })
                        .expect("a gzip stream to truncate"),
                ),
                "gzip",
            ),
            (
                "a payload that inflates past the cap",
                encode_text(&"a".repeat(MAX_HEADER_PAYLOAD_BYTES + 1))
                    .expect("an over-large payload encodes"),
                "inflates",
            ),
        ];

        for (case, value, expected) in rows {
            let actual = decode_bytes("sessionState", &value);
            let message = match actual {
                Ok(payload) => panic!(
                    "case {case:?}: expected a refusal, got {} bytes of payload",
                    payload.len()
                ),
                Err(rejected) => rejected.message(),
            };
            assert!(
                message.contains(expected) && message.contains("sessionState"),
                "case {case:?}: expected a refusal naming sessionState and {expected:?}, \
                 got {message:?}"
            );
        }
    }
}
