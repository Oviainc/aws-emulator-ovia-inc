// SPDX-License-Identifier: CC0-1.0

//! The FRONT DOOR AWS serves AmplifyUIBuilder through: the two response headers every
//! answer carries that this service's model states nowhere.
//!
//! AmplifyUIBuilder is fronted by a layer that finishes every response it serves:
//!
//! * it declares the JSON media type WITH its charset parameter —
//!   `application/json; charset=utf-8` — rather than the bare media type the restJson1
//!   codec writes, which is what a client matching the header by exact string sees; and
//! * it puts an `ETag` validator on the representation it returns, so a client can
//!   cache it or revalidate it conditionally.
//!
//! Neither is modelled: no operation of this service binds an `@httpHeader` output
//! member, and the media type comes from the protocol trait, so neither can come out of
//! the generated codec and neither is something an operation handler could restate — the
//! headers are a property of the SERVICE's response path, uniform across its operations.
//! They are recorded behaviour of the real service (the same front door
//! `service-com-amazonaws-amplifybackend-amplifybackend` records on its error channel).
//!
//! [`served`] is that finishing step, applied once to whatever the framework rendered —
//! a success, a modelled refusal, a framework refusal or the scaffold's 501 — from the
//! single response path this service owns (`super::untyped_errors::UntypedErrorOperation`
//! and [`super::EmulatedService::override_routing_error`]). Nothing else about the
//! response is touched: the status, the body bytes and every other header are the
//! framework's.

use aws_smithy_runtime_api::http::Response as RtResponse;
use sha2::{Digest, Sha256};

/// The charset this service's JSON responses declare. restJson1 bodies are UTF-8
/// (`aws.protocols#restJson1`), so this names the encoding the bytes already use.
const UTF8_CHARSET: &str = "charset=utf-8";

/// The media type an AmplifyUIBuilder JSON response declares, as a client reading the
/// header verbatim sees it. Recorded behaviour, not modelled.
pub const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// The response header the validator is carried in.
const ETAG_HEADER: &str = "etag";

/// The header the media type is declared in.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// Whether `value` declares a JSON media type: `application/json`, or one of the
/// structured `+json` suffix types (RFC 6839). The comparison is on the essence alone —
/// the part ahead of the first `;` — because a media type's parameters are not part of
/// its identity, and case-insensitively, because a media type is case-insensitive.
fn is_json_media_type(value: &str) -> bool {
    let essence = value.split(';').next().unwrap_or_default().trim();
    essence.eq_ignore_ascii_case("application/json")
        || essence
            .rsplit_once('+')
            .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json"))
}

/// Whether `value` already states a `charset` parameter — in which case the front door
/// leaves it alone rather than declaring the encoding twice.
fn states_a_charset(value: &str) -> bool {
    value.split(';').skip(1).any(|parameter| {
        parameter
            .split('=')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("charset")
    })
}

/// `value` with the utf-8 charset parameter appended, or `None` when there is nothing to
/// add: the media type is not JSON, or it already states a charset.
fn charset_qualified(value: &str) -> Option<String> {
    let value = value.trim();
    (is_json_media_type(value) && !states_a_charset(value)).then(|| {
        let essence_and_parameters = value.trim_end_matches(';').trim_end();
        format!("{essence_and_parameters}; {UTF8_CHARSET}")
    })
}

/// The `ETag` validator for a response body.
///
/// Recorded behaviour fixes that the header is PRESENT; its recorded value is
/// environment-specific, so what a client can rely on is the contract of an entity tag —
/// the same representation always answers the same tag, and a different representation a
/// different one. So it is a function of the BODY BYTES alone: the length in hex, then a
/// truncated SHA-256 (128 bits, so two distinct bodies cannot share a tag). It is a WEAK
/// validator (`W/`), because it identifies the payload rather than promising byte-exact
/// equality of two serializations. The radix and the digest construction are this
/// emulator's choice, which nothing recorded constrains; that the header is there and is
/// derived from the body is not.
pub fn entity_tag(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    format!("W/\"{:x}-{}\"", body.len(), hex::encode(&digest[..16]))
}

/// One response, finished as this service's front door finishes it: a JSON media type
/// declared with its charset, and an `ETag` over the body.
///
/// The tag is derived from the bytes the client will receive, so it is set only on a body
/// that is in memory and non-empty — a response with no representation (an operation
/// whose output is `Unit`) has no entity to tag, and a streamed body's bytes are not
/// readable without consuming them. An `ETag` the framework already wrote (a modelled
/// `@httpHeader` output member, should the model ever bind one) is left as the model
/// rendered it.
///
/// A header the HTTP layer cannot represent is OMITTED rather than failing the response,
/// for the reason `aws_smithy_schema_server::protocol::insert_error_response_headers`
/// gives: replacing the answer a caller has to read with a transport failure is strictly
/// worse than answering without one of these two headers.
pub fn served(mut response: RtResponse) -> RtResponse {
    let qualified = response
        .headers()
        .get(CONTENT_TYPE_HEADER)
        .and_then(charset_qualified);
    if let Some(qualified) = qualified {
        let _ = response
            .headers_mut()
            .try_insert(CONTENT_TYPE_HEADER, qualified);
    }

    if response.headers().get(ETAG_HEADER).is_none() {
        let tag = response
            .body()
            .bytes()
            .filter(|bytes| !bytes.is_empty())
            .map(entity_tag);
        if let Some(tag) = tag {
            let _ = response.headers_mut().try_insert(ETAG_HEADER, tag);
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// A response carrying `body`, and `content_type` when it is `Some`.
    fn response(
        status: u16,
        content_type: Option<&'static str>,
        body: &'static str,
    ) -> RtResponse {
        let mut response = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        if let Some(content_type) = content_type {
            response.headers_mut().insert(CONTENT_TYPE_HEADER, content_type);
        }
        response
    }

    /// The charset parameter is added to a JSON media type in whatever form it arrives,
    /// and to nothing else.
    #[test]
    fn a_json_media_type_is_declared_with_its_charset() {
        // (case, header as rendered, what the front door declares)
        let rows: [(&str, &str, Option<&str>); 9] = [
            ("the restJson1 media type", "application/json", Some(JSON_CONTENT_TYPE)),
            ("mixed case", "Application/JSON", Some("Application/JSON; charset=utf-8")),
            (
                "a media type with another parameter",
                "application/json; q=0.9",
                Some("application/json; q=0.9; charset=utf-8"),
            ),
            (
                "a +json suffix type",
                "application/merge-patch+json",
                Some("application/merge-patch+json; charset=utf-8"),
            ),
            ("a charset already stated", JSON_CONTENT_TYPE, None),
            (
                "a charset stated without spacing",
                "application/json;charset=UTF-8",
                None,
            ),
            ("a non-JSON media type", "application/octet-stream", None),
            ("text", "text/plain", None),
            ("an empty header", "", None),
        ];

        for (case, rendered, expected) in rows {
            let actual = charset_qualified(rendered);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?} ({rendered:?}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The validator is a function of the body alone: equal bodies answer one tag,
    /// different bodies different tags, and it is a weak, quoted validator.
    #[test]
    fn the_entity_tag_is_a_function_of_the_body() {
        let bodies: [(&str, &[u8]); 5] = [
            ("an empty body", b""),
            ("a small body", b"{}"),
            ("a component payload", br#"{"id":"a","name":"Hero"}"#),
            ("the same payload with one byte changed", br#"{"id":"b","name":"Hero"}"#),
            ("a long body", b"0123456789012345678901234567890123456789"),
        ];

        let mut tags: Vec<(&str, String)> = Vec::new();
        for (case, body) in bodies {
            let tag = entity_tag(body);
            assert!(
                tag.starts_with("W/\"") && tag.ends_with('"'),
                "case {case:?}: expected a weak quoted validator, got {tag:?}"
            );
            assert_eq!(
                tag,
                entity_tag(body),
                "case {case:?}: the same body must answer the same tag, got {tag:?}"
            );
            for (other, other_tag) in &tags {
                assert_ne!(
                    &tag, other_tag,
                    "cases {case:?} and {other:?} are different bodies, so their tags must \
                     differ, got {tag:?} twice"
                );
            }
            tags.push((case, tag));
        }
    }

    /// One response the framework rendered, and what the front door must make of it.
    struct Row {
        about: &'static str,
        status: u16,
        /// The media type the codec declared, if any.
        rendered: Option<&'static str>,
        body: &'static str,
        /// The media type the front door must leave declared.
        declared: Option<&'static str>,
        /// Whether the answer must carry a validator.
        tagged: bool,
    }

    /// What a response gains, per kind of response the framework renders.
    #[test]
    fn every_response_leaves_with_the_front_doors_headers() {
        let rows = [
            Row {
                about: "a payload success",
                status: 200,
                rendered: Some("application/json"),
                body: r#"{"id":"a"}"#,
                declared: Some(JSON_CONTENT_TYPE),
                tagged: true,
            },
            Row {
                about: "a document success",
                status: 200,
                rendered: Some("application/json"),
                body: r#"{"entities":[]}"#,
                declared: Some(JSON_CONTENT_TYPE),
                tagged: true,
            },
            Row {
                about: "a Unit success",
                status: 200,
                rendered: None,
                body: "",
                declared: None,
                tagged: false,
            },
            Row {
                about: "a modelled refusal",
                status: 404,
                rendered: Some("application/json"),
                body: r#"{"message":"The item specified was not found"}"#,
                declared: Some(JSON_CONTENT_TYPE),
                tagged: true,
            },
            Row {
                about: "the scaffold's 501",
                status: 501,
                rendered: Some("application/json"),
                body: r#"{"__type":"NotImplemented"}"#,
                declared: Some(JSON_CONTENT_TYPE),
                tagged: true,
            },
        ];

        for row in rows {
            let case = row.about;
            let answered = served(response(row.status, row.rendered, row.body));
            assert_eq!(
                answered.status().as_u16(),
                row.status,
                "case {case:?}: the status must not move, got {}",
                answered.status().as_u16()
            );
            assert_eq!(
                answered.headers().get(CONTENT_TYPE_HEADER),
                row.declared,
                "case {case:?}: expected content-type {:?}, got {:?}",
                row.declared,
                answered.headers().get(CONTENT_TYPE_HEADER)
            );
            assert_eq!(
                answered.headers().get(ETAG_HEADER).is_some(),
                row.tagged,
                "case {case:?}: expected an ETag present={}, got {:?}",
                row.tagged,
                answered.headers().get(ETAG_HEADER)
            );
            assert_eq!(
                answered.body().bytes(),
                Some(row.body.as_bytes()),
                "case {case:?}: the body must not change"
            );
        }
    }

    /// A validator the framework already wrote is not replaced.
    #[test]
    fn an_already_tagged_response_keeps_its_tag() {
        let mut rendered = response(200, Some("application/json"), r#"{"id":"a"}"#);
        rendered.headers_mut().insert(ETAG_HEADER, "W/\"modelled\"");
        let answered = served(rendered);
        assert_eq!(
            answered.headers().get(ETAG_HEADER),
            Some("W/\"modelled\""),
            "a tag the model rendered must survive, got {:?}",
            answered.headers().get(ETAG_HEADER)
        );
    }
}
