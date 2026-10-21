// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The [`ServerProtocol`] trait — the inverse of
//! [`aws_smithy_schema::protocol::ClientProtocolInner`] for use on the server.
//!
//! A server protocol does three things, in order:
//!
//! 1. Inspect a request and decide which operation it targets
//!    ([`ServerProtocol::routing_key`]).
//! 2. Deserialize the request body into an operation input via a
//!    [`ShapeDeserializer`] ([`ServerProtocol::deserialize_input`]).
//! 3. Serialize the operation's output (or error) back to a response
//!    ([`ServerProtocol::serialize_output`] /
//!    [`ServerProtocol::serialize_error`]).
//!
//! Steps (2) and (3) are body-only; transport-bound members (HTTP headers,
//! status code) are handled directly by generated code, the same way
//! `ClientProtocolInner` handles them on the client side.
//!
//! Implementors target HTTP today, but the trait is transport-agnostic: pick
//! `Request = aws_smithy_runtime_api::http::Request`,
//! `Response = aws_smithy_runtime_api::http::Response` for HTTP.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};

/// Statically-dispatched server protocol trait — the one implementors write.
///
/// `Request` and `Response` are associated types so a protocol can target any
/// transport (HTTP, MQTT, in-memory, …). For the common HTTP case, set both
/// to `aws_smithy_runtime_api::http::Request` / `Response`.
///
/// Server protocols are stateless; a single instance is shared across all
/// inbound requests for a service.
pub trait ServerProtocol: Send + Sync + std::fmt::Debug {
    /// The protocol's request message type (e.g., `http::Request`).
    type Request;

    /// The protocol's response message type (e.g., `http::Response`).
    type Response;

    /// Returns the Smithy shape ID of this protocol.
    fn protocol_id(&self) -> &ShapeId;

    /// Extracts the operation name from a request.
    ///
    /// This is the _routing key_: the value the server uses to dispatch the
    /// request to a specific handler. For `awsJson1_0` and `awsJson1_1`, it
    /// is the suffix of the `x-amz-target` header (e.g.,
    /// `DynamoDB_20120810.PutItem` -> `PutItem`). For REST protocols it is
    /// derived from the URI and HTTP method.
    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError>;

    /// Reads the request body and returns a [`ShapeDeserializer`] over it.
    ///
    /// The deserializer reads only body members. Callers that need to read
    /// transport-bound members (HTTP headers, URI labels) read those
    /// directly from the request before consuming the deserializer.
    ///
    /// `input_schema` is the schema of the operation's input shape; protocols
    /// may use it to validate trait-bound transport bindings (e.g.,
    /// `@httpHeader`) before producing the body deserializer.
    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError>;

    /// Serializes an operation output into a response.
    ///
    /// `output_schema` is the schema of the output shape; the protocol uses
    /// it to drive the body codec and to honour transport bindings.
    ///
    /// `success_status` is the operation's success HTTP status, supplied by the
    /// caller rather than read off `output_schema`. A protocol applies it as the
    /// response status unless a `@httpResponseCode` output member binds one, which
    /// wins.
    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError>;

    /// Serializes an operation error into a response.
    ///
    /// `error_schema` is the schema of the concrete error variant. Protocols
    /// use the schema's [`@error` trait][@error] (and any `@httpError` trait)
    /// to pick an appropriate transport status (HTTP 4xx vs. 5xx for HTTP).
    /// The `error_code` is the Smithy shape name of the error and is used by
    /// awsJson protocols to populate the `__type` field in the body.
    ///
    /// [@error]: https://smithy.io/2.0/spec/type-refinement-traits.html#smithy-api-error-trait
    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError>;

    /// Serialize a `@streaming` union's active variant as one
    /// `vnd.amazon.eventstream` frame then EOS (`event_type` = member name,
    /// `is_exception` = in-band error). Errors for non-streaming protocols.
    fn serialize_event_stream_message(
        &self,
        _event_type: &str,
        _is_exception: bool,
        _event: &dyn SerializableStruct,
        _event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        Err(ProtocolError::Malformed(
            "this protocol does not support event-stream output".into(),
        ))
    }
}

/// Errors produced by a [`ServerProtocol`].
///
/// Wrapping these in a single enum (rather than reusing
/// [`SerdeError`][aws_smithy_schema::serde::SerdeError] directly) keeps
/// transport-level failures (a missing header, a malformed routing key)
/// distinct from body-level failures, which makes it easier for the dispatch
/// layer to choose the right HTTP status code.
#[derive(Debug)]
pub enum ProtocolError {
    /// A transport-level header is missing.
    MissingHeader(&'static str),
    /// A transport-level header is malformed.
    InvalidHeader { name: &'static str, message: String },
    /// The routing key did not match any known operation.
    UnknownOperation(String),
    /// Reading the request body failed.
    BodyRead(String),
    /// A serde-level error from the codec.
    Serde(SerdeError),
    /// The request body was malformed.
    Malformed(String),
    /// The request parsed, but one or more values failed the constraints the
    /// model declares. Carried structurally rather than as a sentence so the
    /// words stating it can be chosen outside this crate.
    Invalid(Vec<crate::constraints::Violation>),
    /// Request authentication refused the caller: a 403 with this code and message.
    Unauthenticated {
        code: &'static str,
        message: &'static str,
    },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader(name) => write!(f, "missing required header `{name}`"),
            Self::InvalidHeader { name, message } => {
                write!(f, "invalid header `{name}`: {message}")
            }
            Self::UnknownOperation(op) => write!(f, "unknown operation `{op}`"),
            Self::BodyRead(message) => write!(f, "failed to read request body: {message}"),
            Self::Serde(e) => write!(f, "serde error: {e}"),
            Self::Malformed(message) => write!(f, "malformed request: {message}"),
            Self::Unauthenticated { message, .. } => write!(f, "{message}"),
            // The default wording; a service may state it differently.
            Self::Invalid(violations) => match crate::constraints::envelope(violations) {
                Some(message) => write!(f, "{message}"),
                None => write!(f, "invalid request"),
            },
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serde(e) => Some(e),
            _ => None,
        }
    }
}

impl From<SerdeError> for ProtocolError {
    fn from(e: SerdeError) -> Self {
        Self::Serde(e)
    }
}

/// Insert response headers that carry request-influenced bytes, rejecting any
/// the HTTP layer cannot represent instead of panicking.
///
/// `@httpHeader` values, a joined list of them, and an `@httpPrefixHeaders`
/// name built from a map key all reach the response as `name: value`. The
/// `Headers::insert` the response exposes is `try_insert(...).expect(...)`, so
/// a byte it cannot represent - a control character including CR or LF, or a
/// space, `:` or other non-token byte in a name - aborts the whole request
/// with a panic. Routing these through `try_insert` turns that into a clean
/// [`ProtocolError::Malformed`], covering every invalid byte rather than a
/// hand-picked subset, and never silently rewrites the bytes a client sent.
/// Server-chosen constants (`content-type`, `smithy-protocol`) stay on the
/// infallible `insert`: they are fixed valid strings, so a failure there would
/// be a server bug, not request input.
pub fn insert_response_headers(
    resp: &mut aws_smithy_runtime_api::http::Response,
    headers: impl IntoIterator<Item = (String, String)>,
) -> Result<(), ProtocolError> {
    for (name, value) in headers {
        resp.headers_mut().try_insert(name, value).map_err(|e| {
            ProtocolError::Malformed(format!("response header is not valid HTTP: {e}"))
        })?;
    }
    Ok(())
}

/// Insert response headers on an error response, omitting any the HTTP layer
/// cannot represent rather than failing the response.
///
/// This is the error-path counterpart to [`insert_response_headers`]. On a
/// success response a header the HTTP layer cannot represent is a clean failure
/// (the caller propagates it), but on an error response that would be strictly
/// worse than the modeled failure it is replacing: propagating turns a modeled
/// 4xx into a transport error that typically maps to a 500, which an SDK
/// retries. So an unrepresentable header is dropped here and the already-written
/// error body still carries the shape. This matches the `x-amzn-errortype`
/// guard on the same paths, which omits that header on the same condition for
/// the same reason. `try_insert` is still what classifies a header - the bytes
/// are never rewritten, only kept or dropped whole.
pub fn insert_error_response_headers(
    resp: &mut aws_smithy_runtime_api::http::Response,
    headers: impl IntoIterator<Item = (String, String)>,
) {
    for (name, value) in headers {
        let _ = resp.headers_mut().try_insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::{Response, StatusCode};
    use aws_smithy_types::body::SdkBody;

    fn empty_response() -> Response {
        Response::new(StatusCode::try_from(200u16).expect("200"), SdkBody::empty())
    }

    /// A header the HTTP layer can represent is recorded as given, with no
    /// rewriting of the bytes.
    #[test]
    fn insert_response_headers_records_a_valid_header_verbatim() {
        let mut resp = empty_response();
        insert_response_headers(&mut resp, [("x-test".to_string(), "value".to_string())])
            .expect("a valid header must be accepted");
        assert_eq!(resp.headers().get("x-test"), Some("value"));
    }

    /// A value the HTTP layer cannot represent is rejected with a
    /// `ProtocolError::Malformed` instead of panicking the request. A CR or LF
    /// in a value - the response-splitting bytes - is one such case, and
    /// `try_insert` refuses it where the panicking `insert` would abort the
    /// whole request.
    #[test]
    fn insert_response_headers_rejects_an_unrepresentable_value() {
        let mut resp = empty_response();
        let err = insert_response_headers(
            &mut resp,
            [("x-test".to_string(), "value\r\nInjected: evil".to_string())],
        )
        .expect_err("a value the HTTP layer cannot represent must be refused");
        assert!(
            matches!(err, ProtocolError::Malformed(_)),
            "expected ProtocolError::Malformed, got: {err:?}"
        );
    }

    /// An invalid name - here a space, which the removed stripping did not even
    /// address - is rejected the same way, so the name is covered as fully as
    /// the value.
    #[test]
    fn insert_response_headers_rejects_an_invalid_name() {
        let mut resp = empty_response();
        let err =
            insert_response_headers(&mut resp, [("x prefix evil".to_string(), "v".to_string())])
                .expect_err("a name the HTTP layer cannot represent must be refused");
        assert!(
            matches!(err, ProtocolError::Malformed(_)),
            "expected ProtocolError::Malformed, got: {err:?}"
        );
    }

    /// On the error path an unrepresentable header is dropped rather than
    /// failing the response, so a modeled 4xx error is never downgraded to a
    /// propagated transport error (a 500 an SDK retries). A representable header
    /// recorded alongside it still survives.
    #[test]
    fn insert_error_response_headers_drops_the_bad_header_and_keeps_the_good_one() {
        let mut resp = empty_response();
        insert_error_response_headers(
            &mut resp,
            [
                ("x-good".to_string(), "ok".to_string()),
                ("x-bad".to_string(), "value\r\nInjected: evil".to_string()),
            ],
        );
        assert_eq!(resp.headers().get("x-good"), Some("ok"));
        assert_eq!(
            resp.headers().get("x-bad"),
            None,
            "the invalid header is omitted"
        );
    }
}
