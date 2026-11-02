// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Pre-dispatch protocol-error rendering.
//!
//! A framework-level failure that happens *before* a modeled error struct
//! exists — constraint validation, an unrouted operation, a malformed body —
//! still has to be rendered on the wire in the service's own protocol. This
//! module renders it, compiled once for the whole fleet; a service's only
//! per-service inputs are the protocol kind and (for restXml) its
//! `@xmlNamespace`. The macro emits a three-line call into it.
//!
//! # JSON error messages are escaped
//!
//! The JSON families run every error message through `escape_json`, so a
//! message containing a character JSON requires escaping is emitted as valid
//! JSON (see `escape_json`'s own docs below). Status codes and error codes are
//! independent of this — each protocol family's variant-to-status and
//! variant-to-error-code mapping is fixed below.
//!
//! # The protocol asymmetries are deliberate
//!
//! The four protocol families deliberately disagree, and the disagreements are
//! load-bearing — they are what real AWS SDKs expect:
//!
//! | | unrouted operation | `Malformed` arm | content-type |
//! |---|---|---|---|
//! | restXml | **501** `NotImplemented` | **yes** — `InvalidRequest` (400) | `application/xml` |
//! | rpcv2Cbor | **404** `UnknownOperationError` | no (falls to `BadRequestException`) | `application/cbor` |
//! | awsQuery / ec2Query | **400** `InvalidAction` | no (falls to `InvalidRequest`) | `text/xml` |
//! | awsJson / restJson1 | **501** `NotImplemented` | no (falls to `InternalServerError`) | caller-supplied |
//!
//! A request that names none of its service's protocols has no protocol to render
//! in; [`PreDispatchProtocol::Unrecognised`] answers it with one fixed `404`
//! whatever the error.
//!
//! restXml is the only family with a dedicated `Malformed` arm. That is
//! intentional: a bad `aws-chunked` framing or an unparseable body envelope is
//! the *client's* fault, and reporting it as `InternalError` would make AWS SDKs
//! treat it as retryable — so a client with broken framing would retry a request
//! that can never succeed. `InvalidRequest` is terminal and is what real
//! services return. The 404 on rpcv2Cbor is mandated by that protocol's
//! operation-routing rules; the 501 elsewhere is the emulator's deliberate "modeled
//! but NOT REGISTERED" signal.
//!
//! # Do not count unimplemented operations from this module
//!
//! `UnknownOperation` means the routing key matched no entry in the service's
//! `#[smithy_service(operations(...))]` allow-list — the operation is not wired at
//! all. A REGISTERED but scaffolded operation never reaches here: it answers 501
//! from its own handler, with the message "operation {op} is scaffolded but not
//! implemented in this emulator". So a consumer tallying unimplemented operations
//! must key on that message, not on this arm — and must not assume 501 either,
//! since this arm keeps rpcv2Cbor's 404 and the query protocols' 400.

use aws_smithy_runtime_api::http::{Response, StatusCode};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;

/// Response header a `noErrorWrapping` restXml error echoes, naming the request;
/// the body's `<RequestId>` carries the same value.
const REQUEST_ID_HEADER: &str = "x-amz-request-id";
/// Second request-correlation header on that error, mirroring the body's
/// `<HostId>`.
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";

/// A fresh request-id for one unwrapped error response, carried identically in
/// the body's `<RequestId>` element and the `x-amz-request-id` header. Each
/// response carries its own id so two responses are distinguishable; the id
/// names a request for correlation and is not a security token, so a random
/// value is correct. Generate it once per response and use the one value in
/// both places.
fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A fresh extended request-id (`<HostId>` / `x-amz-id-2`) for one unwrapped
/// error response. Unlike the request-id it is not a UUID: the S3 `x-amz-id-2`
/// shape is a 64-lowercase-hex-character string, so hex-encode 32 random bytes
/// (two v4 UUIDs' worth). Like the request-id, generate it once per response
/// and use the one value in both the body's `<HostId>` and the header.
fn new_host_id() -> String {
    let mut hex = String::with_capacity(64);
    for byte in uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(uuid::Uuid::new_v4().as_bytes().iter())
    {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Which protocol's error envelope to render, plus the only per-service inputs.
///
/// Borrowed rather than owned so the generated call site is a `const`-shaped
/// expression with no allocation on the error path.
#[derive(Debug, Clone, Copy)]
pub enum PreDispatchProtocol<'a> {
    /// restXml. Carries the service's `@xmlNamespace`, or `None` when the model
    /// declares none — in which case the envelope carries no `xmlns` at all,
    /// matching what `serialize_error` stamps on modeled errors.
    RestXml {
        xml_namespace: Option<&'a str>,
        /// Whether the model's restXml trait sets `noErrorWrapping`: drop the
        /// `<ErrorResponse><Error>…` wrapper for a bare `<Error>` root plus the
        /// `x-amz-request-id` / `x-amz-id-2` headers. Defaults to the wrapped
        /// envelope, which every other restXml service expects.
        no_error_wrapping: bool,
    },
    /// Smithy rpcv2Cbor.
    RpcV2Cbor,
    /// awsQuery (`ec2 = false`) or ec2Query (`ec2 = true`). The two differ only
    /// in envelope shape, which `query_error_body` already handles.
    Query { ec2: bool },
    /// The awsJson family and restJson1. `content_type` is the caller's, because it
    /// is per-protocol: `application/json` for restJson1, and `-1.0` / `-1.1` for
    /// awsJson1_0 / awsJson1_1 respectively. It is NOT a constant here — naming one
    /// is what made every awsJson1_1 service answer errors with awsJson1_0's
    /// content-type. The single source is the macro's `content_type_of` table.
    Json {
        content_type: &'a str,
        /// Whether this protocol resolves an error shape from `x-amzn-errortype`.
        /// True for restJson1 only: the awsJson families convey the type in the
        /// body's `__type` and real services send no such header on them.
        errortype_header: bool,
    },
    /// The request names none of the protocols its service declares, so there is
    /// no protocol to render in. Answered with one fixed, protocol-neutral
    /// refusal whatever the error: `404`, no `content-type` header, and
    /// [`UNRECOGNISED_PROTOCOL_BODY`] as the body. This is the answer AWS
    /// endpoints give such a request.
    Unrecognised,
}

/// The body of the refusal for a request that names none of its service's
/// protocols (see [`PreDispatchProtocol::Unrecognised`]): an empty
/// `UnknownOperationException` element and one newline.
pub const UNRECOGNISED_PROTOCOL_BODY: &str = "<UnknownOperationException/>\n";

/// The status of the refusal for a request that names none of its service's
/// protocols.
const UNRECOGNISED_PROTOCOL_STATUS: u16 = 404;

/// Render a pre-dispatch [`ProtocolError`] as a wire response in `protocol`'s
/// error format.
///
/// The mapping from error variant to status code and error code is fixed per
/// protocol family (see the module docs).
pub fn protocol_error_response(
    protocol: PreDispatchProtocol<'_>,
    error: ProtocolError,
) -> Response<SdkBody> {
    protocol_error_response_with(protocol, error, None)
}

/// Why the framework refused a request before any handler ran, described
/// structurally rather than as a sentence.
///
/// Services do not agree on how to word these refusals, or even which error
/// code to name — two services rejecting the same absent member may answer with
/// different codes and unrelated prose. None of that is recorded in a model, so
/// this crate describes WHAT it refused and leaves the choice of words to
/// whoever is emulating a particular service. [`Refused`] is that choice.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// One or more values failed the rules the model declares — a member the
    /// shape requires was absent, or a value fell outside a declared bound.
    ///
    /// Both arrive here: absence is found while reading the request, and a bad
    /// value while walking the one that was read. They are the same refusal to a
    /// caller, so they are the same refusal to a renderer.
    Invalid(Vec<Violation>),
    /// Request authentication refused the caller (HTTP 403).
    Unauthenticated,
}

/// The caller-visible answer chosen for a [`Refusal`]: the code a client
/// branches on, and the message it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// Error code / `__type` shape name.
    pub code: String,
    /// Human-readable message, unescaped — each protocol escapes it itself.
    pub message: String,
}

/// Chooses how a [`Refusal`] is stated. Returning `None` keeps this crate's
/// default wording, so an implementation only has to cover what it wants to
/// change.
pub type RefusalRenderer = dyn Fn(&Refusal) -> Option<Refused> + Send + Sync;

/// Describe `error` as a [`Refusal`], when it is one this crate can describe
/// structurally. Anything else has no structured form and renders from its own
/// message.
pub fn refusal_of(error: &ProtocolError) -> Option<Refusal> {
    match error {
        // Absence travels as a codec error because that is where it is found.
        ProtocolError::Serde(::aws_smithy_schema::serde::SerdeError::MissingMember {
            member_name,
        }) => Some(Refusal::Invalid(vec![Violation {
            path: member_name.clone(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }])),
        ProtocolError::Invalid(violations) => Some(Refusal::Invalid(violations.clone())),
        ProtocolError::Unauthenticated { .. } => Some(Refusal::Unauthenticated),
        _ => None,
    }
}

/// Ask `renderer` how to state `error`, if it is a refusal with a structured
/// form and a renderer was supplied. `None` means "use the default wording".
pub fn render_refusal(
    error: &ProtocolError,
    renderer: Option<&RefusalRenderer>,
) -> Option<Refused> {
    let renderer = renderer?;
    renderer(&refusal_of(error)?)
}

/// As [`protocol_error_response`], with `chosen` replacing the code and message
/// this crate would otherwise pick. The status and the envelope stay with the
/// protocol, which owns them.
pub fn protocol_error_response_with(
    protocol: PreDispatchProtocol<'_>,
    error: ProtocolError,
    chosen: Option<Refused>,
) -> Response<SdkBody> {
    match protocol {
        PreDispatchProtocol::RestXml {
            xml_namespace,
            no_error_wrapping,
        } => rest_xml(xml_namespace, no_error_wrapping, error, chosen),
        PreDispatchProtocol::RpcV2Cbor => rpc_v2_cbor(error, chosen),
        PreDispatchProtocol::Query { ec2 } => query(ec2, error, chosen),
        PreDispatchProtocol::Json {
            content_type,
            errortype_header,
        } => json(content_type, errortype_header, error, chosen),
        PreDispatchProtocol::Unrecognised => Response::new(
            StatusCode::try_from(UNRECOGNISED_PROTOCOL_STATUS).expect("status is a valid constant"),
            SdkBody::from(UNRECOGNISED_PROTOCOL_BODY),
        ),
    }
}

/// Build a response with one `content-type` header. `status` is a modeled
/// constant at every call site below, so `try_from` cannot fail.
fn respond(status: u16, content_type: &'static str, body: impl Into<SdkBody>) -> Response<SdkBody> {
    let mut resp = Response::new(
        StatusCode::try_from(status).expect("status is a valid constant"),
        body.into(),
    );
    resp.headers_mut().insert("content-type", content_type);
    resp
}

/// The client-facing text for a deserialization failure.
///
/// A required member absent from the request is worded like a constraint
/// violation, because to a caller they are the same class of refusal and a
/// service states them alike. Every other codec failure keeps its own wording.
fn serde_message(error: &::aws_smithy_schema::serde::SerdeError) -> String {
    match error {
        ::aws_smithy_schema::serde::SerdeError::MissingMember { member_name } => {
            aws_smithy_schema_server::missing_member_message(member_name)
        }
        other => other.to_string(),
    }
}

/// The default wording for a refusal this crate can describe. One renderer for
/// every validation class, so a new class cannot arrive with its own sentence.
fn invalid_message(violations: &[Violation]) -> String {
    aws_smithy_schema_server::envelope(violations).unwrap_or_else(|| "invalid request".to_string())
}

/// Replace the code and message this crate picked with the ones chosen for a
/// [`Refusal`], when a choice was made. Keeps both together so a renderer can
/// never apply one without the other.
fn apply(chosen: Option<Refused>, code: &str, message: String) -> (String, String) {
    match chosen {
        Some(Refused {
            code: chosen,
            message,
        }) => {
            let kept = shape_name_only(&chosen);
            // Nothing recognisable survived; keep the code this protocol chose
            // rather than answering with an empty one.
            let code = if kept.is_empty() {
                code.to_string()
            } else {
                kept
            };
            (code, message)
        }
        None => (code.to_string(), message),
    }
}

/// Reduce a chosen error code to the characters a shape id can hold.
///
/// Every protocol puts the code somewhere that cannot carry arbitrary bytes: an
/// XML element, a JSON string, an HTTP header value. Each renderer escapes the
/// MESSAGE, because a message is prose; a code names an error SHAPE, so it is
/// constrained here once instead of escaped four ways — and a header value has
/// no escaping to apply at all, only bytes it will or will not accept.
///
/// A namespaced id (`smithy.example#SomeException`) passes through unchanged.
fn shape_name_only(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '#' | '_' | '-'))
        .collect()
}

fn rest_xml(
    xml_namespace: Option<&str>,
    no_error_wrapping: bool,
    error: ProtocolError,
    chosen: Option<Refused>,
) -> Response<SdkBody> {
    // `message` is XML-escaped below and a chosen `code` has already been reduced
    // to characters an envelope can carry, so error text cannot break it.
    let (status, code, message) = match error {
        // NOTE: `e` here is the inner `SerdeError`, so this renders the codec's
        // own message — NOT `ProtocolError`'s "serde error: {e}" prefix.
        ProtocolError::Serde(e) => (400, "ValidationException", serde_message(&e)),
        ProtocolError::Invalid(v) => (400, "ValidationException", invalid_message(&v)),
        ProtocolError::UnknownOperation(op) => (
            501,
            "NotImplemented",
            format!("operation {op} is not implemented in this emulator"),
        ),
        ProtocolError::Malformed(detail) => (
            400,
            "InvalidRequest",
            format!("malformed request: {detail}"),
        ),
        ProtocolError::Unauthenticated { code, message } => (403, code, message.to_string()),
        // TERMINAL, not a server-fault name. A 400 here is the client's fault, but
        // SDKs classify by the error CODE as well as the status and treat
        // `Internal*` as retryable — so `InternalError` on a 400 makes a client
        // retry a request that can never succeed. This module's own header text
        // already argues exactly that for the `Malformed` arm above; the catch-all
        // contradicted it. `InvalidRequest` is what that arm uses.
        other => (400, "InvalidRequest", other.to_string()),
    };
    // BOTH interpolated values are escaped. `xmlns` is model-derived
    // (`@xmlNamespace`), not server-chosen, so a namespace carrying `"`, `&` or
    // `<` would break the envelope exactly as an unescaped message did — and it
    // sits inside an ATTRIBUTE, where an unescaped `"` terminates the attribute
    // early and silently changes the document's shape rather than merely
    // malforming it. `code` needs no escaping here because `shape_name_only` has
    // already reduced a chosen one to characters an envelope can carry; the arms
    // above supply a server-chosen `&'static str`.
    let xmlns = match xml_namespace {
        Some(ns) => format!(" xmlns=\"{}\"", crate::xml_escape(ns)),
        None => String::new(),
    };
    let (code, message) = apply(chosen, code, message);
    let message = crate::xml_escape(&message);
    // Two envelope shapes, selected by the model's `noErrorWrapping` (see the
    // `RestXml` variant). The unwrapped shape is a bare `<Error>` root with no
    // `<Type>`, trailing `<RequestId>`/`<HostId>` elements, and the matching
    // `x-amz-request-id` / `x-amz-id-2` response headers — what an SDK of a
    // `noErrorWrapping` service reads the error `<Code>` out of. That root carries
    // no `xmlns`, as S3 sends it and `rest_xml.rs` renders handler errors: botocore
    // finds no `<Code>` under a namespaced root. One request-id and one host-id are
    // generated per response and used in both the body element and the header.
    if no_error_wrapping {
        let request_id = new_request_id();
        let host_id = new_host_id();
        let body = format!(
            "<?xml version=\"1.0\"?><Error><Code>{code}</Code>\
             <Message>{message}</Message><RequestId>{request_id}</RequestId>\
             <HostId>{host_id}</HostId></Error>"
        );
        let mut resp = respond(status, "application/xml", body);
        resp.headers_mut()
            .insert(REQUEST_ID_HEADER, request_id);
        resp.headers_mut()
            .insert(EXTENDED_REQUEST_ID_HEADER, host_id);
        return resp;
    }
    let body = format!(
        "<?xml version=\"1.0\"?><ErrorResponse{xmlns}><Error><Type>Sender</Type>\
         <Code>{code}</Code><Message>{message}</Message></Error></ErrorResponse>"
    );
    respond(status, "application/xml", body)
}

fn rpc_v2_cbor(error: ProtocolError, chosen: Option<Refused>) -> Response<SdkBody> {
    let (status, code, message) = match error {
        ProtocolError::Serde(e) => (400, "ValidationException", serde_message(&e)),
        ProtocolError::Invalid(v) => (400, "ValidationException", invalid_message(&v)),
        ProtocolError::UnknownOperation(op) => (
            404,
            "UnknownOperationError",
            format!("operation not found: {op}"),
        ),
        ProtocolError::Unauthenticated { code, message } => (403, code, message.to_string()),
        other => (400, "BadRequestException", other.to_string()),
    };
    let (code, message) = apply(chosen, code, message);
    let body = aws_smithy_schema_server::rpc::cbor_synthetic_error_body(&code, &message);
    let mut resp = respond(status, "application/cbor", body);
    resp.headers_mut().insert("smithy-protocol", "rpc-v2-cbor");
    resp
}

fn query(ec2: bool, error: ProtocolError, chosen: Option<Refused>) -> Response<SdkBody> {
    // The query protocols route on the `Action` body field and have no 404
    // semantics, so every framework failure here is a 400; an authentication
    // refusal is the one 403.
    // The two codes below are what the query services were measured to name, and
    // they distinguish the two layers: a body that could not be read at all
    // (`MalformedInput`) from one that was read and then failed a declared bound
    // (`ValidationError`). A service that names them differently says so through
    // its refusal hook.
    let (status, code, message) = match error {
        ProtocolError::Serde(e) => (400, "MalformedInput", serde_message(&e)),
        ProtocolError::Invalid(v) => (400, "ValidationError", invalid_message(&v)),
        ProtocolError::UnknownOperation(op) => {
            (400, "InvalidAction", format!("unknown action: {op}"))
        }
        ProtocolError::Unauthenticated { code, message } => (403, code, message.to_string()),
        other => (400, "InvalidRequest", other.to_string()),
    };
    let (code, message) = apply(chosen, code, message);
    let body = aws_smithy_schema_server::query_error_body(ec2, &code, &message);
    respond(status, "text/xml", body)
}

fn json(
    content_type: &str,
    errortype_header: bool,
    error: ProtocolError,
    chosen: Option<Refused>,
) -> Response<SdkBody> {
    // Every arm yields the message UNESCAPED so a chosen one can replace it on
    // equal terms; the single escape below then covers both.
    let (status, type_name, message) = match error {
        ProtocolError::Serde(e) => (400, "ValidationException", serde_message(&e)),
        ProtocolError::Invalid(v) => (400, "ValidationException", invalid_message(&v)),
        ProtocolError::UnknownOperation(op) => (
            501,
            "NotImplemented",
            format!("operation {op} is not implemented in this emulator"),
        ),
        ProtocolError::Unauthenticated { code, message } => (403, code, message.to_string()),
        // TERMINAL, for the same reason as the restXml arm — and it matters more
        // here: `x-amzn-errortype` below now carries this name, which is how
        // restJson1 resolves the error shape, so a server-fault name is no longer
        // just cosmetic. `SerializationException` is the code AWS itself returns for
        // an awsJson body it cannot process, and it is terminal.
        other => (400, "SerializationException", other.to_string()),
    };
    let (type_name, message) = apply(chosen, type_name, message);
    let message = escape_json(&message);
    let body = format!(r#"{{"__type":"{type_name}","message":"{message}"}}"#);
    // `content_type` is per-service, so it cannot be a `&'static str` here.
    let mut resp = Response::new(
        StatusCode::try_from(status).expect("status is a valid constant"),
        SdkBody::from(body),
    );
    resp.headers_mut()
        .insert("content-type", content_type.to_string());
    // restJson1 resolves an error SHAPE from this header. The awsJson families do
    // not — they read the body's `__type`, which is written above — so emitting it
    // there attaches a header real services never send.
    if errortype_header {
        resp.headers_mut().insert("x-amzn-errortype", type_name);
    }
    resp
}

/// Escape `input` for use inside a JSON string literal, per RFC 8259 §7.
///
/// Escaping only `"` is not enough, and the shortfall is reached by ordinary
/// traffic rather than by a hostile request. A constraint violation renders the
/// member's Smithy `@pattern` VERBATIM into the message, and patterns are full of
/// backslashes: `^[a-zA-Z0-9|!\-_*'\(\)]+$` produces `\-` and `\(`, neither of
/// which is a legal JSON escape. The whole 400 body then fails to parse, so an
/// SDK reports an opaque deserialization failure instead of the
/// `ValidationException` the emulator actually returned — the error is not merely
/// mis-worded, it is invisible. `\t` and `\n` are worse: they parse, and silently
/// corrupt the message.
///
/// The two-character forms are used where RFC 8259 defines one; every other
/// control character takes `\u00XX`. `/` is left alone (escaping it is optional).
fn escape_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // RFC 8259 requires escaping every code point below 0x20; the ones
            // with no short form go out as `\u00XX`.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_schema::serde::SerdeError;

    /// A request that names none of its service's protocols is answered with the
    /// `404` AWS endpoints send: no `content-type`, and an empty
    /// `UnknownOperationException` element plus one newline as the body.
    #[test]
    fn an_unrecognised_request_is_answered_with_a_404_and_no_content_type() {
        let response = protocol_error_response(
            PreDispatchProtocol::Unrecognised,
            ProtocolError::Malformed("x".into()),
        );
        assert_eq!(response.status().as_u16(), 404);
        assert_eq!(response.headers().get("content-type"), None);
        assert_eq!(
            response.body().bytes(),
            Some(&b"<UnknownOperationException/>\n"[..])
        );
        assert_eq!(response.body().bytes().map(<[u8]>::len), Some(29));
    }

    /// The answer does not depend on the error or on a chosen wording: there is no
    /// protocol to word it in.
    #[test]
    fn an_unrecognised_request_ignores_the_error_and_the_chosen_wording() {
        let chosen = Refused {
            code: "Reworded".to_string(),
            message: "reworded".to_string(),
        };
        let reworded = protocol_error_response_with(
            PreDispatchProtocol::Unrecognised,
            ProtocolError::UnknownOperation("Op".into()),
            Some(chosen),
        );
        let plain = protocol_error_response(
            PreDispatchProtocol::Unrecognised,
            ProtocolError::Malformed("other".into()),
        );
        assert_eq!(reworded.status(), plain.status());
        assert_eq!(reworded.headers().get("content-type"), None);
        assert_eq!(reworded.body().bytes(), plain.body().bytes());
    }

    /// Both validation classes reach the renderer as the same refusal: absence,
    /// found while reading the request, and a bad value, found while walking the
    /// one that was read. Anything else has no structured form.
    #[test]
    fn both_validation_classes_share_one_structured_refusal() {
        let absent = ProtocolError::Serde(SerdeError::MissingMember {
            member_name: "tableName".to_string(),
        });
        assert_eq!(
            refusal_of(&absent),
            Some(Refusal::Invalid(vec![Violation {
                path: "tableName".to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }]))
        );
        let bad_value = ProtocolError::Invalid(vec![Violation {
            path: "limit".to_string(),
            kind: ViolationKind::MinValue("1".to_string()),
            value: Some("0".to_string()),
            sensitive: false,
        }]);
        assert!(matches!(refusal_of(&bad_value), Some(Refusal::Invalid(v)) if v.len() == 1));
        assert_eq!(refusal_of(&ProtocolError::Malformed("x".into())), None);
        assert_eq!(
            refusal_of(&ProtocolError::Serde(SerdeError::custom("other"))),
            None
        );
    }

    /// The renderer is consulted only when one exists, and may decline — both of
    /// which leave the default wording in place.
    #[test]
    fn a_declining_or_absent_renderer_keeps_the_default() {
        let error = ProtocolError::Serde(SerdeError::MissingMember {
            member_name: "TableName".to_string(),
        });
        assert_eq!(render_refusal(&error, None), None);
        let declines: &RefusalRenderer = &|_: &Refusal| None;
        assert_eq!(render_refusal(&error, Some(declines)), None);
        let chooses: &RefusalRenderer = &|_: &Refusal| {
            Some(Refused {
                code: "MissingParameter".to_string(),
                message: "no".to_string(),
            })
        };
        assert_eq!(
            render_refusal(&error, Some(chooses)),
            Some(Refused {
                code: "MissingParameter".to_string(),
                message: "no".to_string()
            })
        );
    }

    /// A renderer that declines on an error with no structured form must not be
    /// reached at all, so a non-refusal keeps its codec wording.
    #[test]
    fn a_renderer_is_not_consulted_for_an_unstructured_error() {
        let called = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
        let error = ProtocolError::Malformed("bad".into());
        let flag = ::std::sync::Arc::clone(&called);
        let spy: ::std::sync::Arc<RefusalRenderer> = ::std::sync::Arc::new(move |_: &Refusal| {
            flag.store(true, ::std::sync::atomic::Ordering::SeqCst);
            None
        });
        assert_eq!(render_refusal(&error, Some(spy.as_ref())), None);
        assert!(!called.load(::std::sync::atomic::Ordering::SeqCst));
    }

    /// A chosen code reaches an XML element, a JSON string and a header value, so
    /// a hostile one must break none of them — and must not panic the header
    /// insert, which rejects bytes outright rather than escaping them.
    ///
    /// Asserts the envelope still PARSES and that the code arriving in it is
    /// exactly what the constraint allows. Probing one offset of the body would
    /// pass while the document was malformed three characters later.
    #[test]
    fn a_hostile_chosen_code_breaks_no_envelope() {
        let hostile = [
            "Bad\"Code",
            "Bad<Code>&amp",
            "Bad\\Code",
            "Bad\nCode",
            "Bad Code\u{7f}",
            "!!!",
            "",
            "</Code><Code>Injected",
            "a\",\"message\":\"pwned",
        ];
        for code in hostile {
            let expected = shape_name_only(code);

            // JSON: the body must parse, and __type must be exactly what survived.
            let json_resp = protocol_error_response_with(
                PreDispatchProtocol::Json {
                    content_type: "application/x-amz-json-1.0",
                    errortype_header: true,
                },
                ProtocolError::Malformed("m".into()),
                Some(Refused {
                    code: code.to_string(),
                    message: "m".to_string(),
                }),
            );
            let body = body_of(&json_resp);
            let parsed: serde_json::Value = serde_json::from_str(&body)
                .unwrap_or_else(|e| panic!("{code:?} made unparseable JSON: {body} ({e})"));
            let got = parsed["__type"].as_str().expect("__type is a string");
            if expected.is_empty() {
                // Nothing survived; the protocol keeps the code it chose.
                assert_eq!(got, "SerializationException", "{code:?} -> {body}");
            } else {
                assert_eq!(got, expected, "{code:?} -> {body}");
            }

            // XML: the code must carry no character that can end an element.
            let xml_resp = protocol_error_response_with(
                PreDispatchProtocol::RestXml {
                    xml_namespace: None,
                    no_error_wrapping: false,
                },
                ProtocolError::Malformed("m".into()),
                Some(Refused {
                    code: code.to_string(),
                    message: "m".to_string(),
                }),
            );
            let xml = body_of(&xml_resp);
            let inner = xml
                .split("<Code>")
                .nth(1)
                .and_then(|rest| rest.split("</Code>").next())
                .unwrap_or_else(|| panic!("{code:?} produced no <Code>: {xml}"));
            assert!(
                !inner.contains(['<', '>', '&', '"']),
                "{code:?} leaked markup into <Code>{inner}</Code>: {xml}"
            );
            assert_eq!(xml.matches("<Code>").count(), 1, "{code:?} -> {xml}");
        }
    }

    /// A namespaced shape id is a legitimate code and must survive intact.
    #[test]
    fn a_namespaced_chosen_code_passes_through() {
        assert_eq!(
            shape_name_only("smithy.example#SomeException"),
            "smithy.example#SomeException"
        );
    }

    /// Every protocol must honour a chosen code AND message — a protocol that
    /// applied one without the other would answer with a mismatched pair.
    #[test]
    fn every_protocol_honours_a_chosen_code_and_message() {
        // The message must share NO text with the default, or a protocol that
        // kept the default message would still pass.
        let chosen = Refused {
            code: "MissingParameter".to_string(),
            message: "chosen-wording-sentinel".to_string(),
        };
        let protocols = [
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            PreDispatchProtocol::RpcV2Cbor,
            PreDispatchProtocol::Query { ec2: false },
            PreDispatchProtocol::Json {
                content_type: "application/x-amz-json-1.0",
                errortype_header: true,
            },
        ];
        for proto in protocols {
            let resp = protocol_error_response_with(
                proto,
                ProtocolError::Serde(SerdeError::MissingMember {
                    member_name: "TableName".to_string(),
                }),
                Some(chosen.clone()),
            );
            let body = body_of(&resp);
            assert!(body.contains("MissingParameter"), "code missing: {body}");
            assert!(
                !body.contains("ValidationException") && !body.contains("MalformedInput"),
                "default code leaked: {body}"
            );
            // The CHOSEN message survives, and the default one is gone.
            assert!(
                body.contains("chosen-wording-sentinel"),
                "chosen message missing: {body}"
            );
            assert!(
                !body.contains("must not be null"),
                "default message leaked: {body}"
            );
        }
    }

    /// Without a choice, the default wording states the member path.
    #[test]
    fn the_default_states_the_missing_member_path() {
        let resp = protocol_error_response_with(
            PreDispatchProtocol::Json {
                content_type: "application/x-amz-json-1.0",
                errortype_header: false,
            },
            ProtocolError::Serde(aws_smithy_schema_server::within_member(
                aws_smithy_schema_server::missing_member("WriteCapacityUnits"),
                "ProvisionedThroughput",
            )),
            None,
        );
        let body = body_of(&resp);
        assert!(
            body.contains("'provisionedThroughput.writeCapacityUnits'"),
            "{body}"
        );
        assert!(body.contains("Member must not be null"), "{body}");
    }

    /// The query protocols name the two layers differently: a body that could not
    /// be read against one that was read and then failed a declared bound. A
    /// client branches on the code, so one code for both would lose that.
    #[test]
    fn the_query_default_distinguishes_unreadable_from_out_of_bounds() {
        for ec2 in [false, true] {
            let unreadable = body_of(&protocol_error_response_with(
                PreDispatchProtocol::Query { ec2 },
                ProtocolError::Serde(SerdeError::custom("nope")),
                None,
            ));
            let out_of_bounds = body_of(&protocol_error_response_with(
                PreDispatchProtocol::Query { ec2 },
                ProtocolError::Invalid(vec![aws_smithy_schema_server::constraints::Violation {
                    path: "tags".to_string(),
                    kind: aws_smithy_schema_server::ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                }]),
                None,
            ));
            assert!(
                unreadable.contains("MalformedInput"),
                "ec2={ec2}: {unreadable}"
            );
            assert!(
                out_of_bounds.contains("ValidationError"),
                "ec2={ec2}: {out_of_bounds}"
            );
            assert!(
                !out_of_bounds.contains("MalformedInput"),
                "the two layers must not share one code: {out_of_bounds}"
            );
        }
    }

    fn body_of(resp: &Response<SdkBody>) -> String {
        String::from_utf8_lossy(resp.body().bytes().expect("in-memory body")).into_owned()
    }

    fn content_type(resp: &Response<SdkBody>) -> String {
        resp.headers()
            .get("content-type")
            .unwrap_or_default()
            .to_string()
    }

    fn serde() -> ProtocolError {
        ProtocolError::Serde(SerdeError::custom("bad field"))
    }

    /// The four protocol families deliberately disagree on how an UNROUTED
    /// operation is reported. These are wire contracts that real AWS SDKs
    /// depend on, so pin all four in one place: a future refactor that
    /// "harmonises" them is a regression, not a cleanup.
    #[test]
    fn unknown_operation_status_differs_per_protocol_and_that_is_deliberate() {
        let cases = [
            (
                PreDispatchProtocol::RestXml {
                    xml_namespace: None,
                    no_error_wrapping: false,
                },
                501,
                "NotImplemented",
            ),
            // 404, not 501: mandated by the rpcv2Cbor operation-routing rules.
            (PreDispatchProtocol::RpcV2Cbor, 404, "UnknownOperationError"),
            // 400, not 501: the query protocols route on the `Action` body
            // field and have no 404/501 semantics.
            (
                PreDispatchProtocol::Query { ec2: false },
                400,
                "InvalidAction",
            ),
            (
                PreDispatchProtocol::Json {
                    content_type: "application/x-amz-json-1.0",
                    errortype_header: true,
                },
                501,
                "NotImplemented",
            ),
        ];
        for (proto, want_status, want_code) in cases {
            let resp = protocol_error_response(
                proto,
                ProtocolError::UnknownOperation("DoesNotExist".into()),
            );
            assert_eq!(
                resp.status().as_u16(),
                want_status,
                "{proto:?} must report an unrouted operation as {want_status}"
            );
            // CBOR is binary; check the code appears in the encoded bytes.
            assert!(
                body_of(&resp).contains(want_code),
                "{proto:?} body must carry {want_code}: {}",
                body_of(&resp)
            );
        }
    }

    /// restXml is the ONLY family with a dedicated `Malformed` arm, and it must
    /// report `InvalidRequest` (terminal) rather than `InternalError` — the
    /// latter is a server-fault code that AWS SDKs retry, so a client with
    /// broken framing would retry forever.
    #[test]
    fn rest_xml_malformed_is_invalid_request_not_internal_error() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            ProtocolError::Malformed("bad chunk framing".into()),
        );
        assert_eq!(resp.status().as_u16(), 400);
        let body = body_of(&resp);
        assert!(body.contains("<Code>InvalidRequest</Code>"), "{body}");
        assert!(!body.contains("InternalError"), "{body}");
    }

    /// The other three families have no `Malformed` arm, so it must fall through
    /// to their generic bucket.
    #[test]
    fn malformed_falls_through_to_the_generic_arm_elsewhere() {
        let m = || ProtocolError::Malformed("bad".into());
        assert!(body_of(&protocol_error_response(
            PreDispatchProtocol::RpcV2Cbor,
            m()
        ))
        .contains("BadRequestException"));
        assert!(body_of(&protocol_error_response(
            PreDispatchProtocol::Query { ec2: false },
            m()
        ))
        .contains("InvalidRequest"));
        assert!(body_of(&protocol_error_response(
            PreDispatchProtocol::Json {
                content_type: "application/json",
                errortype_header: true,
            },
            m()
        ))
        .contains("SerializationException"));
    }

    #[test]
    fn rest_xml_stamps_the_service_xml_namespace_only_when_the_model_declares_one() {
        let with = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: Some("http://s3.amazonaws.com/doc/2006-03-01/"),
                no_error_wrapping: false,
            },
            serde(),
        );
        assert!(body_of(&with)
            .contains("<ErrorResponse xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">"));
        let without = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            serde(),
        );
        assert!(body_of(&without).contains("<ErrorResponse><Error>"));
    }

    #[test]
    fn rest_xml_escapes_the_message_so_error_text_cannot_break_the_envelope() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            ProtocolError::Serde(SerdeError::custom("a <b> & \"c\"")),
        );
        let body = body_of(&resp);
        assert!(body.contains("&lt;b&gt;"), "{body}");
        assert!(!body.contains("<b>"), "{body}");
    }

    /// The `xmlns` is model-derived, so it must be escaped too — and it is the
    /// more dangerous of the two, because it sits inside an ATTRIBUTE. An
    /// unescaped `"` closes the attribute early, so the injected text becomes
    /// MARKUP rather than a malformed value: the envelope still parses, and
    /// reports something the service never said.
    ///
    /// This was interpolated raw one line above the message that was already
    /// escaped.
    #[test]
    fn rest_xml_escapes_the_service_namespace_so_it_cannot_inject_markup() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: Some(r#"http://x/"><Injected>boom</Injected><Error x=""#),
                no_error_wrapping: false,
            },
            serde(),
        );
        let body = body_of(&resp);
        assert!(
            !body.contains("<Injected>"),
            "a namespace must not be able to introduce an element: {body}"
        );
        assert!(
            body.contains("&quot;") && body.contains("&gt;"),
            "the namespace's metacharacters must arrive escaped: {body}"
        );
        // Exactly one Error element, i.e. the envelope's shape is unchanged.
        assert_eq!(
            body.matches("<Error>").count(),
            1,
            "the envelope must still carry exactly one <Error>: {body}"
        );
    }

    /// XML 1.0 has NO escaped form for most C0 control characters, so the five
    /// predefined entities are not enough: a message carrying `U+0001` produced an
    /// envelope no SDK could parse. This is the restXml twin of the JSON escaping
    /// defect, and it is why `xml_escape` REPLACES those code points rather than
    /// trying to escape them. Tab / newline / carriage return are legal and must
    /// survive.
    #[test]
    fn rest_xml_replaces_control_characters_that_xml_cannot_represent() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            ProtocolError::Serde(SerdeError::custom(
                "bell\u{07}nul\u{00}vt\u{0b}us\u{1f}keep\ttab\nand\rnewline",
            )),
        );
        let body = body_of(&resp);
        for illegal in ['\u{07}', '\u{00}', '\u{0b}', '\u{1f}'] {
            assert!(
                !body.contains(illegal),
                "U+{:04X} is illegal in XML 1.0 and must not reach the body: {body:?}",
                illegal as u32
            );
        }
        // Numeric character references are not a way out either.
        assert!(
            !body.contains("&#x7;") && !body.contains("&#7;"),
            "{body:?}"
        );
        // REPLACED, not deleted: one U+FFFD per illegal code point. Deleting would
        // leave `bellnul...` reading as a legal value and make the error misleading.
        assert_eq!(
            body.matches('\u{FFFD}').count(),
            4,
            "each illegal code point must become one U+FFFD: {body:?}"
        );
        // The LEGAL whitespace survives — but as numeric references, not raw. Raw
        // #x9/#xA/#xD are normalized to a space inside an attribute value and CR is
        // folded to LF in element text, so emitting them raw would silently change
        // the value a client reads. This is what keeps `xml_escape` lossless in the
        // same way `escape_json` already is.
        for (raw, reference) in [('\t', "&#x9;"), ('\n', "&#xA;"), ('\r', "&#xD;")] {
            assert!(
                body.contains(reference),
                "{raw:?} must survive as {reference}: {body:?}"
            );
            assert!(
                !body.contains(raw),
                "{raw:?} must NOT be emitted raw (it would be normalized away): {body:?}"
            );
        }
        assert!(body.contains("keep"), "{body:?}");

        // The OTHER three bounds of the `Char` production, which the noncharacter
        // test does not reach. Without these, narrowing `..='\u{D7FF}'` to `D7FE` or
        // shifting `'\u{E000}'..=` to `E001` silently deletes legal characters and
        // no test notices.
        let edges = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            ProtocolError::Serde(SerdeError::custom("p\u{20}q\u{D7FF}r\u{E000}s")),
        );
        let edges = body_of(&edges);
        for legal in ['\u{20}', '\u{D7FF}', '\u{E000}'] {
            assert!(
                edges.contains(legal),
                "U+{:04X} is legal XML and must survive: {edges:?}",
                legal as u32
            );
        }
        assert!(
            !edges.contains('\u{FFFD}'),
            "no legal character may be replaced: {edges:?}"
        );
    }

    /// `U+FFFE` / `U+FFFF` are the exclusions a `< 0x20` test misses: both are
    /// valid Rust `char`s, both are outside the XML 1.0 `Char` production, and
    /// both make a document unparseable. Surrogates cannot occur in a `char`, so
    /// these two are the whole remaining gap.
    ///
    /// The input also carries a LEGAL `U+FFFD`, which must survive untouched — that
    /// is what pins the upper bound of the `[#xE000-#xFFFD]` range against an
    /// off-by-one in either direction.
    #[test]
    fn rest_xml_replaces_the_noncharacters_a_control_check_would_miss() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            ProtocolError::Serde(SerdeError::custom("a\u{FFFE}b\u{FFFF}c\u{FFFD}d")),
        );
        let body = body_of(&resp);
        assert!(
            !body.contains('\u{FFFE}'),
            "U+FFFE must not reach the body: {body:?}"
        );
        assert!(
            !body.contains('\u{FFFF}'),
            "U+FFFF must not reach the body: {body:?}"
        );
        // Three U+FFFD: the two replacements, plus the one the input already
        // carried. U+FFFD is the last LEGAL code point of that range, so it must
        // survive untouched — which also pins the bound against an off-by-one.
        assert_eq!(
            body.matches('\u{FFFD}').count(),
            3,
            "U+FFFE and U+FFFF replaced, and the input's own U+FFFD preserved: {body:?}"
        );
        // The surrounding legal text is intact, so nothing wholesale was lost.
        assert!(
            body.contains('a') && body.contains('b') && body.contains('d'),
            "{body:?}"
        );
    }

    /// restJson1 resolves an error SHAPE from `x-amzn-errortype`; the awsJson
    /// families read the body's `__type` and real services send no such header on
    /// them. So the header follows the protocol, and the body's `__type` — which
    /// every JSON protocol reads — must carry the shape name either way.
    #[test]
    fn the_errortype_header_follows_the_protocol_and_the_body_always_names_the_shape() {
        // `ProtocolError` is not `Clone`, so each case builds a fresh one.
        type Make = fn() -> ProtocolError;
        let cases: [(Make, u16, &str); 3] = [
            (|| serde(), 400, "ValidationException"),
            (
                || ProtocolError::UnknownOperation("Nope".into()),
                501,
                "NotImplemented",
            ),
            (
                || ProtocolError::InvalidHeader {
                    name: "x-amz-target",
                    message: "bad".into(),
                },
                400,
                "SerializationException",
            ),
        ];
        for (make, want_status, want_type) in cases {
            for (ct, errortype_header) in [
                ("application/json", true),
                ("application/x-amz-json-1.0", false),
                ("application/x-amz-json-1.1", false),
            ] {
                let resp = protocol_error_response(
                    PreDispatchProtocol::Json {
                        content_type: ct,
                        errortype_header,
                    },
                    make(),
                );
                assert_eq!(resp.status().as_u16(), want_status);
                let header = resp.headers().get("x-amzn-errortype");
                if errortype_header {
                    assert_eq!(header.unwrap_or_default(), want_type, "restJson1 {ct}");
                } else {
                    assert!(header.is_none(), "awsJson sends no errortype header: {ct}");
                }
                // And it must agree with the body, not merely be present.
                assert!(
                    body_of(&resp).contains(&format!(r#""__type":"{want_type}""#)),
                    "header and body must resolve the SAME shape: {}",
                    body_of(&resp)
                );
            }
        }
    }

    /// No pre-dispatch arm may name a SERVER fault on a 4xx.
    ///
    /// SDKs classify by the error CODE as well as the status and treat `Internal*` /
    /// `*ServerError` as retryable, so such a name on a 400 makes a client retry a
    /// request that can never succeed. A name like `InternalError` or
    /// `InternalServerError` on a 400 is therefore wrong regardless of how accurate it
    /// is about the server's internals. Asserted
    /// as an invariant over every family and every error variant rather than pinning
    /// four names, so a new arm or a new variant cannot reintroduce it.
    #[test]
    fn no_client_error_is_named_as_a_server_fault() {
        type Make = fn() -> ProtocolError;
        let errors: [Make; 4] = [
            || serde(),
            || ProtocolError::Malformed("bad".into()),
            || ProtocolError::InvalidHeader {
                name: "x-amz-target",
                message: "bad".into(),
            },
            || ProtocolError::UnknownOperation("Nope".into()),
        ];
        for proto in [
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            PreDispatchProtocol::RpcV2Cbor,
            PreDispatchProtocol::Query { ec2: false },
            PreDispatchProtocol::Query { ec2: true },
            PreDispatchProtocol::Json {
                content_type: "application/json",
                errortype_header: true,
            },
        ] {
            for make in errors {
                let resp = protocol_error_response(proto, make());
                let status = resp.status().as_u16();
                let body = body_of(&resp);
                if !(400..500).contains(&status) {
                    continue;
                }
                for server_fault in ["InternalError", "InternalServerError", "InternalFailure"] {
                    assert!(
                        !body.contains(server_fault),
                        "{proto:?} answered {status} but named the server-fault code \
                         {server_fault}, which SDKs retry: {body}"
                    );
                }
            }
        }
    }

    #[test]
    fn json_content_type_is_per_service() {
        for ct in ["application/json", "application/x-amz-json-1.0"] {
            let resp = protocol_error_response(
                PreDispatchProtocol::Json {
                    content_type: ct,
                    errortype_header: true,
                },
                serde(),
            );
            assert_eq!(content_type(&resp), ct);
        }
    }

    #[test]
    fn cbor_sets_the_smithy_protocol_header() {
        let resp = protocol_error_response(PreDispatchProtocol::RpcV2Cbor, serde());
        assert_eq!(content_type(&resp), "application/cbor");
        assert_eq!(
            resp.headers().get("smithy-protocol").unwrap_or_default(),
            "rpc-v2-cbor"
        );
    }

    #[test]
    fn ec2_query_and_aws_query_use_different_envelopes() {
        let aws = body_of(&protocol_error_response(
            PreDispatchProtocol::Query { ec2: false },
            serde(),
        ));
        let ec2 = body_of(&protocol_error_response(
            PreDispatchProtocol::Query { ec2: true },
            serde(),
        ));
        assert_ne!(aws, ec2, "ec2Query and awsQuery envelopes must differ");
    }

    /// Constraint-validation failures are always a 400 client error, never a 5xx.
    #[test]
    fn validation_is_always_a_client_error() {
        for proto in [
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            PreDispatchProtocol::RpcV2Cbor,
            PreDispatchProtocol::Query { ec2: false },
            PreDispatchProtocol::Json {
                content_type: "application/json",
                errortype_header: true,
            },
        ] {
            assert_eq!(
                protocol_error_response(proto, serde()).status().as_u16(),
                400,
                "{proto:?} must report a validation failure as 400"
            );
        }
    }

    /// `body` parses as JSON **and** its decoded `message` is exactly `expected`.
    ///
    /// This is the SDK's own step, performed by the SDK's own kind of parser, so
    /// it is the strongest available statement that the envelope survived. It
    /// subsumes every property a hand-rolled escape scan could check — an
    /// unescaped `"`, an illegal `\x` escape, a raw control character — and adds
    /// two a scan structurally cannot see:
    ///
    ///   * An unescaped `"` silently TERMINATES the string literal rather than
    ///     producing an illegal byte, so a scanner that does not track string
    ///     boundaries reads straight past it. That is the original symptom
    ///     `escape_json` exists to fix.
    ///   * Double-escaping PARSES. `\\-` is a legal `\\` escape followed by `-`,
    ///     so the body is well-formed but the message decodes to the wrong text.
    ///     Only comparing the decoded value against the original catches it.
    fn json_message_round_trips(body: &str, expected: &str) -> Result<(), String> {
        let parsed: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("body is not valid JSON: {e}"))?;
        let got = parsed
            .get("message")
            .and_then(|m| m.as_str())
            .ok_or_else(|| "body has no string `message` field".to_string())?;
        if got != expected {
            return Err(format!(
                "decoded message {got:?} does not round-trip to the original {expected:?}"
            ));
        }
        Ok(())
    }

    /// The JSON error envelope must survive ANY message, including the one class
    /// that reaches it on ordinary traffic: a constraint violation renders the
    /// member's Smithy `@pattern` verbatim, and patterns carry backslashes.
    ///
    /// This is the assertion whose absence let the defect ship — the module had
    /// tests for content-type, status and XML escaping, but none that the JSON
    /// body it builds is parseable. A body that fails to parse is worse than a
    /// mis-worded one: the SDK reports a deserialization failure and the
    /// `ValidationException` never reaches the caller at all.
    ///
    /// Parsing alone is not enough either, so this also pins that the message
    /// DECODES back to the original text: a double-escaped body parses fine and
    /// still shows the caller the wrong pattern.
    #[test]
    fn json_envelope_survives_any_message_including_regex_patterns() {
        // Real patterns taken from shipped models, plus the control characters
        // that parse-but-corrupt rather than fail outright.
        let messages = [
            r"Member must satisfy pattern: ^[a-zA-Z0-9|!\-_*'\(\)]+$",
            r"Member must satisfy pattern: ^\p{Print}+$",
            r"Member must satisfy pattern: ^[^\$]+$",
            "saw \"quoted\"",
            "tab\there",
            "newline\nhere",
            "backslash\\here",
            "bell\u{07}here",
        ];
        for msg in messages {
            // Both arms that can carry caller- or model-derived text. The
            // expected message is read off each error rather than hardcoded,
            // because the two arms render differently: the `Serde` arm emits the
            // INNER codec error's text, while `InvalidHeader` falls to the
            // generic arm and so carries `ProtocolError`'s own `Display`.
            let inner = SerdeError::custom(msg);
            let serde_expected = inner.to_string();
            let header = ProtocolError::InvalidHeader {
                name: "x-amz-target",
                message: msg.into(),
            };
            let header_expected = header.to_string();
            for (error, expected) in [
                (ProtocolError::Serde(inner), serde_expected),
                (header, header_expected),
            ] {
                let body = body_of(&protocol_error_response(
                    PreDispatchProtocol::Json {
                        content_type: "application/json",
                        errortype_header: true,
                    },
                    error,
                ));
                if let Err(why) = json_message_round_trips(&body, &expected) {
                    panic!("message {msg:?} produced a broken body ({why}): {body}");
                }
            }
        }
    }

    /// Non-vacuity guard for the check above: it must actually REJECT a
    /// quote-only-escaped body, otherwise it would pass for the wrong reason.
    #[test]
    fn the_well_formedness_check_rejects_a_quote_only_escaped_body() {
        // Escaping `"` alone leaves a pattern's own backslashes verbatim as illegal
        // escapes, so the body does not parse at all.
        let pattern = r"pattern: ^[a-z|!\-_]+$";
        let old = format!(r#"{{"__type":"ValidationException","message":"{pattern}"}}"#);
        assert!(
            json_message_round_trips(&old, pattern).is_err(),
            "must reject the illegal \\- escape, or the check proves nothing"
        );

        // An unescaped `"` — the original symptom `escape_json` was written to
        // fix. It must be rejected: the quote ends the string literal early, so
        // the rest of the envelope is garbage.
        let quoted = r#"saw "quoted""#;
        let unescaped = format!(r#"{{"__type":"ValidationException","message":"{quoted}"}}"#);
        assert!(
            json_message_round_trips(&unescaped, quoted).is_err(),
            "must reject an unescaped quote"
        );

        // A raw control character is the other half of the unparseable class.
        let tabbed = "tab\there";
        assert!(
            json_message_round_trips(&format!("{{\"message\":\"{tabbed}\"}}"), tabbed).is_err(),
            "must reject a raw control character"
        );

        // Double-escaping PARSES, and is therefore the case an escape-sequence
        // scan cannot reach: `\\-` is a legal `\\` followed by `-`. Only the
        // decoded-value comparison rejects it.
        let doubled = format!(r#"{{"message":"{}"}}"#, escape_json(&escape_json(pattern)));
        assert!(
            json_message_round_trips(&doubled, pattern).is_err(),
            "must reject a double-escaped message: {doubled}"
        );

        // And it must ACCEPT the body this module actually builds, so it is not
        // simply rejecting everything.
        let good = body_of(&protocol_error_response(
            PreDispatchProtocol::Json {
                content_type: "application/json",
                errortype_header: true,
            },
            ProtocolError::Malformed(pattern.to_string()),
        ));
        assert!(
            json_message_round_trips(&good, &format!("malformed request: {pattern}")).is_ok(),
            "must accept the body the module builds: {good}"
        );
    }

    // =====================================================================
    // `noErrorWrapping` pre-dispatch error envelope
    // =====================================================================

    /// Assert `s` is a canonical v4 UUID: `8-4-4-4-12` lowercase hex, version
    /// nibble `4`, variant nibble `8`/`9`/`a`/`b`.
    fn assert_is_v4_uuid(s: &str) {
        let groups: Vec<&str> = s.split('-').collect();
        assert_eq!(groups.len(), 5, "a UUID has five groups: {s}");
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "group lengths must be 8-4-4-4-12: {s}"
        );
        assert!(
            s.chars()
                .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a UUID is lowercase hex and hyphens: {s}"
        );
        assert_eq!(&groups[2][0..1], "4", "version nibble must be 4: {s}");
        assert!(
            matches!(&groups[3][0..1], "8" | "9" | "a" | "b"),
            "variant nibble must be 8/9/a/b: {s}"
        );
    }

    /// Assert `s` is the S3 `x-amz-id-2` shape: exactly 64 lowercase hex chars.
    fn assert_is_host_id(s: &str) {
        assert_eq!(s.len(), 64, "a host-id is 64 hex characters: {s}");
        assert!(
            s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a host-id is lowercase hex: {s}"
        );
    }

    /// Pull the text of the first `<Element>text</Element>` out of `body`.
    fn element_text<'a>(body: &'a str, element: &str) -> &'a str {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        let start = body.find(&open).unwrap_or_else(|| panic!("<{element}> in {body}")) + open.len();
        let end = body[start..]
            .find(&close)
            .unwrap_or_else(|| panic!("</{element}> in {body}"))
            + start;
        &body[start..end]
    }

    /// A `noErrorWrapping` restXml service renders a pre-dispatch error as a bare
    /// `<Error>` root — no `<ErrorResponse>` wrapper and no `<Type>` — with the
    /// request-correlation elements and the matching `x-amz-request-id` /
    /// `x-amz-id-2` headers. Each id is generated per response: the header
    /// carries the same value as the body element, the request-id is a v4 UUID,
    /// and the host-id is 64 lowercase hex characters.
    #[test]
    fn rest_xml_no_error_wrapping_emits_a_bare_error_root_with_matching_request_id() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: true,
            },
            ProtocolError::UnknownOperation("GetThing".into()),
        );
        assert_eq!(resp.status().as_u16(), 501);
        let body = body_of(&resp);
        assert!(
            body.starts_with("<?xml version=\"1.0\"?><Error><Code>NotImplemented</Code>"),
            "unwrapped pre-dispatch error must be a bare <Error> root: {body}"
        );
        assert!(!body.contains("<ErrorResponse"), "{body}");
        assert!(!body.contains("<Type>"), "{body}");

        let body_request_id = element_text(&body, "RequestId");
        let body_host_id = element_text(&body, "HostId");
        assert_is_v4_uuid(body_request_id);
        assert_is_host_id(body_host_id);
        assert_eq!(
            resp.headers().get("x-amz-request-id").unwrap_or_default(),
            body_request_id,
            "x-amz-request-id header must equal the body's <RequestId>: {body}"
        );
        assert_eq!(
            resp.headers().get("x-amz-id-2").unwrap_or_default(),
            body_host_id,
            "x-amz-id-2 header must equal the body's <HostId>: {body}"
        );
    }

    #[test]
    fn rest_xml_no_error_wrapping_leaves_the_namespace_off_the_error_root() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: Some("http://s3.amazonaws.com/doc/2006-03-01/"),
                no_error_wrapping: true,
            },
            serde(),
        );
        let body = body_of(&resp);
        assert!(
            body.starts_with("<?xml version=\"1.0\"?><Error><Code>"),
            "the unwrapped <Error> root must carry no namespace: {body}"
        );
        assert!(!body.contains("<ErrorResponse"), "{body}");
    }

    /// The default (flag absent) keeps the wrapped envelope and adds no
    /// request-id headers — a no-regression pin for every other restXml service.
    #[test]
    fn rest_xml_default_stays_wrapped_with_no_request_id_headers() {
        let resp = protocol_error_response(
            PreDispatchProtocol::RestXml {
                xml_namespace: None,
                no_error_wrapping: false,
            },
            serde(),
        );
        let body = body_of(&resp);
        assert!(body.contains("<ErrorResponse><Error><Type>Sender</Type>"), "{body}");
        assert!(resp.headers().get("x-amz-request-id").is_none());
        assert!(resp.headers().get("x-amz-id-2").is_none());
    }
}
