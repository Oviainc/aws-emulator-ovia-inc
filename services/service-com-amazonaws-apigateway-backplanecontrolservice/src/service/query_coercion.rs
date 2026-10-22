// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer for values bound to the QUERY STRING:
//! how a query value that cannot be converted to the modelled type of the member it is
//! bound to is refused.
//!
//! Two refusals happen before a handler runs, and they are not the same error class:
//!
//! * a value bound to the request (`@httpQuery`) that cannot be CONVERTED to the
//!   modelled type of its member — `includeValues=maybe` where the member targets a
//!   boolean, `limit=lots` where it targets an integer — never becomes a value at all.
//!   It fails in the protocol/deserialization layer, ahead of the operation, and this
//!   API Gateway surface answers it with the protocol-level
//!   [`SERIALIZATION_EXCEPTION`] (HTTP 400) whose body states the two facts the caller
//!   has to reconcile — the value it sent and the type that value could not become —
//!   under the body member [`MESSAGE_MEMBER`];
//! * a value that WAS converted and then failed a rule the model declares (an absent
//!   `@required` member, a `@length` / `@range` / `@pattern` / enum violation) is a
//!   constraint refusal, which the framework answers as `ValidationException`.
//!
//! The framework renders BOTH as `400 ValidationException` with a lowercase `message`
//! (`vendor/smithy-server-runtime/src/pre_dispatch.rs`, where every
//! `ProtocolError::Serde` takes that code and the JSON envelope's member name is fixed),
//! so a client branching on the error code to decide whether to fix its serializer or
//! fix a value takes the wrong branch, and a client reading `Message` on this surface's
//! protocol errors sees nothing. The vendored tree is read-only from a service crate, so
//! the separation is made here, at the one seam a service has before dispatch
//! ([`crate::service::EmulatedService::override_operation`]) — exactly as
//! `services/service-com-amazonaws-xray-awsxray/src/service/request_layer.rs` does for
//! the same error class in a request BODY.
//!
//! # Where it runs
//!
//! [`refusal_for`] is called at the head of `crate::service::StatedPerOperation::run`,
//! the wrapper this service already serves every modelled operation through, and
//! [`serialization_exception`] answers the request when it reports a sentence. That
//! placement is the ORDER the two layers run in: a value that cannot be converted never
//! becomes a value, so there are no constraint violations for that wrapper to state
//! after it. It is also why nothing here needs a list of the service's operations — the
//! wrapper is generic over the operation it serves, so `O::INPUT_SCHEMA` is already in
//! hand — and why the refusal is computed exactly once per request.
//!
//! # Which members this layer answers for, and how it decides
//!
//! Nothing about the surface is written down here: the members are read off the
//! operation's own input schema ([`coercible_query_members`]) — every member with an
//! `@httpQuery` binding whose target is one of the scalar types a query value is
//! CONVERTED into. A member whose conversion has no such stated form (a timestamp, a
//! blob, an enum, a list) is left to the framework, which refuses it in its own
//! envelope.
//!
//! The conversion test mirrors the framework's own query-value reader
//! (`vendor/smithy-schema-server/src/http_binding.rs`'s `StringValueDeserializer`:
//! `"true"` / `"false"` for a boolean, `str::parse` for each numeric width) and calls
//! the same `str::parse` for the numeric widths, while the query string is read with the
//! framework's own `parse_form_urlencoded`, so the two cannot disagree about which
//! values convert. The unit test
//! `this_layer_and_the_frameworks_own_reader_agree_on_every_query_value` pins that
//! agreement against the vendored reader itself, and
//! `tests/all/query_value_coercion.rs` pins it end to end: a value this layer accepts is
//! never answered as a coercion failure, and a value it refuses never reaches the
//! framework's `ValidationException` path.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::body::SdkBody;

/// The error code a query value that cannot be converted to its modelled type is
/// refused with.
///
/// It is the PROTOCOL's error, not one of this service's modelled shapes: the request
/// never became an input, so no operation ran to answer with its own error. It is also
/// the code the framework itself names for a request it cannot process at all
/// (`pre_dispatch.rs`'s catch-all arm), which is the same refusal one unconvertible
/// value earns. What it must NOT be is `ValidationException`, the code the framework
/// would otherwise answer: that is the code for a value that was read and then broke a
/// modelled constraint.
pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The body member this surface's protocol-level error carries its sentence in.
///
/// Capital `Message`, which is what the recordings of this service's protocol errors
/// carry, and what a client of it reads. (A MODELLED error of this service keeps the
/// lowercase `message` its model declares — `smithy_query(query="shape",
/// shape="BadRequestException")`: one member, `message` — and is rendered by the
/// framework, untouched by this layer.)
pub const MESSAGE_MEMBER: &str = "Message";

/// The HTTP status a protocol-level refusal of the request carries: the caller sent
/// something that cannot be read, so it is a client error.
const SERIALIZATION_EXCEPTION_STATUS: u16 = 400;

/// The media type every response of this restJson1 service carries.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The wire answer to a query value this service cannot convert: `400
/// SerializationException`, with the sentence under the capital-[`MESSAGE_MEMBER`] body
/// member this surface words a protocol error with, plus the `__type` discriminator and
/// the `x-amzn-errortype` header restJson1 resolves an error shape from. The two name
/// the SAME shape, as every other error response of this service does.
pub fn serialization_exception(message: &str) -> RtResponse {
    let body = serde_json::json!({
        "__type": SERIALIZATION_EXCEPTION,
        MESSAGE_MEMBER: message,
    });
    let mut response = RtResponse::new(
        StatusCode::try_from(SERIALIZATION_EXCEPTION_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
        .headers_mut()
        .insert("x-amzn-errortype", SERIALIZATION_EXCEPTION);
    response
}

/// The sentence this layer refuses `req` with, for the operation whose input
/// `input_schema` describes, or `None` when every query value the request carries
/// converts (the overwhelmingly common case) and when it carries no query string at all.
///
/// The schema comes from the CALLER — `StatedPerOperation::run` is generic over the
/// operation it serves, so it hands this its `O::INPUT_SCHEMA` — which is why this
/// module needs no list of the service's operations and cannot drift from one.
///
/// The query string is parsed ONCE, with the framework's own reader, and the members are
/// then walked in the input schema's own order — the order the framework's HTTP-binding
/// reader walks them in — so the member reported is the one that reader would have failed
/// on first, and only the FIRST value bound to a name is read, as it reads it.
pub fn refusal_for(input_schema: &'static Schema, req: &RtRequest) -> Option<String> {
    let (_, query) = req.uri().split_once('?')?;
    let bound = ::aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes());
    for (name, target) in coercible_query_members(input_schema) {
        let Some(value) = first_value(&bound, name) else {
            continue;
        };
        if !converts(value, target) {
            return Some(unconvertible_message(value, target));
        }
    }
    None
}

/// The `@httpQuery` name and modelled target type of each member of `schema` whose query
/// value is CONVERTED into a scalar — the members this layer answers for. Read off the
/// schema, so the model decides which they are.
fn coercible_query_members(
    schema: &'static Schema,
) -> impl Iterator<Item = (&'static str, ShapeType)> {
    schema
        .members()
        .iter()
        .filter(|member| converted_type_name(member.shape_type()).is_some())
        .filter_map(|member| {
            member
                .http_query()
                .map(|query| (query.value(), member.shape_type()))
        })
}

/// The FIRST value bound to `name` among the request's decoded query pairs; `None` when
/// the request binds `name` no value at all, which leaves the member absent rather than
/// unconvertible.
///
/// `bound` comes from the FRAMEWORK's own reader
/// (`aws_smithy_schema_server::parse_form_urlencoded`, whose `form_decode` is the same
/// algorithm as `http_binding.rs`'s `percent_decode_query` down to its malformed-escape
/// and invalid-UTF-8 fallbacks), so this layer cannot decode a value differently from
/// the reader whose verdict it has to agree with.
///
/// First, not last: the framework's reader keeps the first value bound to a name for a
/// non-list member, so a later duplicate is never converted and must not be refused.
fn first_value<'a>(bound: &'a [(String, String)], name: &str) -> Option<&'a str> {
    bound
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Whether `value` converts to `target`, by the same rule the framework's query-value
/// reader applies: exactly `true` or `false` for a boolean, and `str::parse` at the
/// modelled width for a number. Anything this layer does not answer for converts
/// vacuously — it is not this layer's refusal to make.
fn converts(value: &str, target: ShapeType) -> bool {
    match target {
        ShapeType::Boolean => matches!(value, "true" | "false"),
        ShapeType::Byte => value.parse::<i8>().is_ok(),
        ShapeType::Short => value.parse::<i16>().is_ok(),
        ShapeType::Integer => value.parse::<i32>().is_ok(),
        ShapeType::Long => value.parse::<i64>().is_ok(),
        ShapeType::Float => value.parse::<f32>().is_ok(),
        ShapeType::Double => value.parse::<f64>().is_ok(),
        _ => true,
    }
}

/// The name a conversion failure calls `target` by, and so also WHICH targets this layer
/// answers for: the scalar types a query value is converted into. `None` for every other
/// modelled type — a timestamp, a blob, an enum, a list — whose refusal this layer
/// leaves to the framework rather than inventing a sentence for.
fn converted_type_name(target: ShapeType) -> Option<&'static str> {
    match target {
        ShapeType::Boolean => Some("Boolean"),
        ShapeType::Byte => Some("Byte"),
        ShapeType::Short => Some("Short"),
        ShapeType::Integer => Some("Integer"),
        ShapeType::Long => Some("Long"),
        ShapeType::Float => Some("Float"),
        ShapeType::Double => Some("Double"),
        _ => None,
    }
}

/// The sentence a failed conversion states: the value the request sent and the type it
/// could not be converted to — the two facts a caller has to reconcile, and the two the
/// refusal is built from. Built from the REQUEST, which is why the recorded example
/// (`'maybe' can not be converted to Boolean`) is one instance of it rather than the
/// requirement.
fn unconvertible_message(value: &str, target: ShapeType) -> String {
    let named = converted_type_name(target).unwrap_or("its modelled type");
    format!("'{value}' can not be converted to {named}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::aws_smithy_schema::codec::Codec;
    use ::aws_smithy_schema_server::json::JsonCodec;
    use ::aws_smithy_schema_server::HttpBindingDeserializer;
    use ::smithy_server_runtime::DeserializableStruct;

    /// Whether the FRAMEWORK's own reader binds `query` to `GetApiKeys`'s input.
    ///
    /// Drives the vendored `HttpBindingDeserializer` over that operation's real input
    /// schema and generated input shape — the same reader, the same schema and the same
    /// `DeserializableStruct` the dispatch sequence uses — so its verdict here IS the
    /// verdict this layer has to agree with. The body is an empty JSON object because
    /// `GetApiKeysRequest` binds every one of its members to the query string.
    fn reads_through_the_framework(query: &str) -> bool {
        let codec = JsonCodec::rest_json();
        let body = codec.create_deserializer(b"{}");
        let mut deserializer =
            HttpBindingDeserializer::new(None, "/apikeys", query, Vec::new(), body);
        crate::ops::types::GetApiKeysInput::deserialize(&mut deserializer).is_ok()
    }

    /// The agreement this layer stands on: for every query value, [`converts`] answers
    /// exactly what the vendored reader answers.
    ///
    /// This is the case that makes a change to that reader
    /// (`vendor/smithy-schema-server/src/http_binding.rs`'s `StringValueDeserializer`)
    /// fail loudly here instead of quietly sending a conversion failure back to the
    /// framework's `ValidationException` envelope — or, worse, refusing a value the
    /// framework would have accepted. The rows span both modelled types `GetApiKeys`
    /// binds to the query string, the boolean vocabulary, the integer width's bound and
    /// one past it, and a member whose type this layer does not answer for.
    #[test]
    fn this_layer_and_the_frameworks_own_reader_agree_on_every_query_value() {
        // (case, query string, the value bound, its modelled target)
        let rows: &[(&str, &str, &str, ShapeType)] = &[
            ("the boolean true", "includeValues=true", "true", ShapeType::Boolean),
            ("the boolean false", "includeValues=false", "false", ShapeType::Boolean),
            ("a capitalised boolean", "includeValues=True", "True", ShapeType::Boolean),
            ("an upper-case boolean", "includeValues=TRUE", "TRUE", ShapeType::Boolean),
            ("a numeric boolean", "includeValues=1", "1", ShapeType::Boolean),
            ("a word that is not one", "includeValues=maybe", "maybe", ShapeType::Boolean),
            ("an empty boolean", "includeValues=", "", ShapeType::Boolean),
            // A bare key with no `=` at all: both readers see the empty value, so the
            // bare form is pinned beside the explicit one.
            ("a valueless boolean key", "includeValues", "", ShapeType::Boolean),
            ("a valueless integer key", "limit", "", ShapeType::Integer),
            ("the default page size", "limit=25", "25", ShapeType::Integer),
            ("a negative integer", "limit=-1", "-1", ShapeType::Integer),
            ("the largest i32", "limit=2147483647", "2147483647", ShapeType::Integer),
            (
                "one past the largest i32",
                "limit=2147483648",
                "2147483648",
                ShapeType::Integer,
            ),
            ("a fractional integer", "limit=2.5", "2.5", ShapeType::Integer),
            ("a word as an integer", "limit=lots", "lots", ShapeType::Integer),
            // `+` is a SPACE in a query component and `%2B` is a literal plus, so these
            // two rows pin the decoding rule through the framework's own reader.
            ("a plus-encoded sign", "limit=%2B5", "+5", ShapeType::Integer),
            ("a plus read as a space", "limit=+5", " 5", ShapeType::Integer),
            (
                "a plus before a boolean word",
                "includeValues=+true",
                " true",
                ShapeType::Boolean,
            ),
            // A string member converts from any text, so neither layer refuses it.
            ("a string member", "name=maybe", "maybe", ShapeType::String),
        ];

        for (case, query, value, target) in rows.iter().copied() {
            let framework = reads_through_the_framework(query);
            let layer = converts(value, target);
            assert_eq!(
                layer, framework,
                "case {case:?} ({query:?}): this layer says converts == {layer}, the \
                 framework's own reader says {framework}"
            );
        }

        // Non-vacuity: the rows must cover BOTH answers, or the agreement proves nothing.
        assert!(
            rows.iter().any(|row| converts(row.2, row.3))
                && rows.iter().any(|row| !converts(row.2, row.3)),
            "the rows must span both converting and non-converting values"
        );
    }

    /// The conversion rule, over the vocabulary of a query value: the boolean words, the
    /// numeric widths and one value past each width's bound.
    #[test]
    fn a_query_value_converts_exactly_where_its_modelled_type_accepts_it() {
        // (case, value, target, converts)
        let rows: &[(&str, &str, ShapeType, bool)] = &[
            ("the boolean true", "true", ShapeType::Boolean, true),
            ("the boolean false", "false", ShapeType::Boolean, true),
            ("a capitalised boolean", "True", ShapeType::Boolean, false),
            ("a numeric boolean", "1", ShapeType::Boolean, false),
            ("a word that is not one", "maybe", ShapeType::Boolean, false),
            ("an empty boolean", "", ShapeType::Boolean, false),
            ("an integer", "25", ShapeType::Integer, true),
            ("a negative integer", "-1", ShapeType::Integer, true),
            ("the largest i32", "2147483647", ShapeType::Integer, true),
            ("one past the largest i32", "2147483648", ShapeType::Integer, false),
            ("a fractional integer", "2.5", ShapeType::Integer, false),
            ("a word as an integer", "lots", ShapeType::Integer, false),
            ("the largest i8", "127", ShapeType::Byte, true),
            ("one past the largest i8", "128", ShapeType::Byte, false),
            ("the largest i16", "32767", ShapeType::Short, true),
            ("one past the largest i16", "32768", ShapeType::Short, false),
            ("a long", "9223372036854775807", ShapeType::Long, true),
            ("one past the largest i64", "9223372036854775808", ShapeType::Long, false),
            ("a double", "2.5", ShapeType::Double, true),
            ("a word as a double", "lots", ShapeType::Double, false),
            // A type this layer does not answer for is never refused here.
            ("a string", "anything", ShapeType::String, true),
            ("a timestamp", "half past four", ShapeType::Timestamp, true),
        ];

        for (case, value, target, expected) in rows.iter().copied() {
            let actual = converts(value, target);
            assert_eq!(
                actual, expected,
                "case {case:?} ({value:?} as {target:?}): expected converts == {expected}, \
                 got {actual}"
            );
        }
    }

    /// Only the FIRST value bound to a name is read, and it is decoded as a query
    /// component — the two properties the framework's own reader has.
    #[test]
    fn the_first_bound_value_is_read_and_decoded() {
        // (case, query, name, value)
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            ("one value", "includeValues=true", "includeValues", Some("true")),
            (
                "the first of two",
                "includeValues=true&includeValues=maybe",
                "includeValues",
                Some("true"),
            ),
            (
                "among other parameters",
                "limit=3&includeValues=false&name=k",
                "includeValues",
                Some("false"),
            ),
            ("a percent escape", "name=a%2Fb", "name", Some("a/b")),
            ("a plus as a space", "name=a+b", "name", Some("a b")),
            ("no value at all", "includeValues", "includeValues", Some("")),
            ("an unbound name", "limit=3", "includeValues", None),
        ];

        for (case, query, name, expected) in rows.iter().copied() {
            let bound = ::aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes());
            let actual = first_value(&bound, name);
            assert_eq!(
                actual, expected,
                "case {case:?} ({query:?}): expected {expected:?}, got {actual:?}"
            );
        }
    }
}
