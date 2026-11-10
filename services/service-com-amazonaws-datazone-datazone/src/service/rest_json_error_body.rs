// SPDX-License-Identifier: CC0-1.0

//! What this service's ERROR body carries on the wire.
//!
//! RULE: this service speaks `restJson1` (`smithy_query(query="service_info")` reports
//! protocols `[restJson1]`), and a `restJson1` failure names its error SHAPE in the
//! `x-amzn-errortype` response header — the header this model's own CORS trait exposes to
//! a browser client — so the BODY of a failure carries the error shape's own modelled
//! members and nothing else:
//!
//! * no shape DISCRIMINATOR member. The `__type` member belongs to the AWS JSON protocol
//!   family, which conveys the shape in the body because it has no such header; a
//!   `restJson1` client resolves the shape from the header, and reading an unmodelled
//!   member out of the body is not something a client generated from this model can do.
//! * no member written as the EMPTY STRING. Every error shape of this model declares one
//!   member, `message` (`smithy_query(query="shape",shape="AccessDeniedException")`), and
//!   a non-optional Rust `String` has only one way to say "this failure carries no text":
//!   the empty string. On the wire that is an absent member, not a present empty one — the
//!   recorded answer to a refusal this service states nothing about carries no member at
//!   all — so an empty value is omitted rather than written.
//!
//! WHERE IT IS APPLIED. The framework's shared `restJson1` renderer writes the `__type`
//! member for every service it serves (`vendor/smithy-schema-server/src/rest_json.rs`),
//! and that renderer is not this crate's to change, so the body is corrected at the seams
//! a service owns over the answers it writes: the protocol
//! [`smithy_server_runtime::Service::override_protocol`] hands the dispatch walk
//! ([`DataZoneRestJson`]), which serializes every failure a HANDLER raised, and
//! [`smithy_server_runtime::Service::override_routing_error`], which answers a request no
//! modeled operation serves. Everything else about an answer stays with the framework
//! renderer that owns it — the status, every header including `x-amzn-errortype`, request
//! deserialization and the whole SUCCESS path are untouched.
//!
//! One class of failure is left as the framework writes it: a refusal rendered BEFORE a
//! handler runs (an absent required member, a value outside a modelled bound, an
//! unauthenticated caller). The dispatch walk renders it from the protocol's pre-dispatch
//! ENVELOPE — a framework enum describing which protocol family states the failure, not a
//! response this crate is handed — so no service-owned seam sees those bytes:
//! `override_refusal` chooses only the code and the message inside that envelope.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};

/// The body member the AWS JSON protocols name the error shape in, which `restJson1`
/// names in a header instead.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// The lowest status a JSON error envelope is rendered under: below it a response is a
/// success and carries no error body at all.
const FIRST_ERROR_STATUS: u16 = 400;

/// `body` as this service sends it — the shape discriminator dropped and every member
/// whose value is the empty string omitted — or `None` when it already carries neither,
/// or is not a JSON object at all.
pub fn served_error_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object: &mut Map<String, Value> = document.as_object_mut()?;
    let members = object.len();
    object.remove(SHAPE_DISCRIMINATOR);
    object.retain(|_, value| !value.as_str().is_some_and(str::is_empty));
    if object.len() == members {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// `resp` as this service answers it: a failure whose body carries the error shape's own
/// members and nothing else ([`served_error_body`]).
///
/// A success, and a failure whose body already carries only those members, is returned
/// untouched — status, headers and every byte of the body exactly as the renderer wrote
/// them.
#[must_use]
pub fn with_served_error_body(mut resp: Response) -> Response {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match served_error_body(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the rewrite, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with the
/// error body this service is recorded answering (see the module docs).
#[derive(Debug)]
pub struct DataZoneRestJson {
    inner: RestJsonServerProtocol,
}

impl DataZoneRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it and
    /// must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table is
    /// the model's); an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for DataZoneRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for DataZoneRestJson {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // Nothing about a REQUEST changes: it is read by the model's own codec.
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // A SUCCESS is the framework's own, byte for byte.
        self.inner
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let resp = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        Ok(with_served_error_body(resp))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::{served_error_body, with_served_error_body, SHAPE_DISCRIMINATOR};
    use aws_smithy_types::body::SdkBody;
    use serde_json::{Map, Value};

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Map<String, Value> {
        serde_json::from_slice::<Value>(bytes)
            .expect("the rewritten body is JSON")
            .as_object()
            .expect("the rewritten body is a JSON object")
            .clone()
    }

    /// A response carrying `status` and `body`.
    fn response(status: u16, body: &str) -> aws_smithy_runtime_api::http::Response {
        aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// One row of the table below: the case, the body the framework renderer wrote, and
    /// the members expected on the wire with the text each carries.
    type Row = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );

    /// RULE over the bodies the framework renderer writes for this service's error shapes:
    /// the discriminator is gone, a member carrying text survives with its text, and a
    /// member written as the empty string is absent. The rows differ along what the rule
    /// generalises over — the error class, whether the text is present, and how many
    /// members the body carries.
    #[test]
    fn an_error_body_carries_the_shapes_own_members_and_nothing_else() {
        let rows: &[Row] = &[
            (
                "a not-found carrying its text",
                r#"{"__type":"ResourceNotFoundException","message":"Domain dzd_x not found"}"#,
                &[("message", "Domain dzd_x not found")],
            ),
            (
                "a denial this service states nothing about",
                r#"{"__type":"AccessDeniedException","message":""}"#,
                &[],
            ),
            (
                "a refusal of the input",
                r#"{"__type":"ValidationException","message":"1 validation error detected"}"#,
                &[("message", "1 validation error detected")],
            ),
            (
                "a discriminator carrying its namespace",
                r#"{"__type":"com.amazonaws.datazone#ConflictException","message":"busy"}"#,
                &[("message", "busy")],
            ),
            (
                "a body that is only the discriminator",
                r#"{"__type":"ThrottlingException"}"#,
                &[],
            ),
        ];

        for (case, rendered, expected) in rows {
            let rewritten = served_error_body(rendered.as_bytes())
                .unwrap_or_else(|| panic!("case {case:?}: expected a rewritten body"));
            let object = decoded(&rewritten);
            assert!(
                object.get(SHAPE_DISCRIMINATOR).is_none(),
                "case {case:?}: the discriminator must be gone, got {object:?}"
            );
            assert_eq!(
                object.len(),
                expected.len(),
                "case {case:?}: expected {} member(s), got {object:?}",
                expected.len()
            );
            for (member, text) in expected.iter() {
                let actual = object.get(*member).and_then(Value::as_str);
                assert_eq!(
                    actual,
                    Some(*text),
                    "case {case:?}: expected {member:?} to carry {text:?}, got {actual:?}"
                );
            }
        }
    }

    /// Nothing to rewrite: a body that already carries only its own non-empty members, one
    /// that is not a JSON object, and one that is not JSON at all.
    #[test]
    fn a_body_this_rule_does_not_own_is_left_alone() {
        for (case, body) in [
            ("already served", r#"{"message":"Domain not found"}"#),
            ("no members at all", "{}"),
            ("not an object", r#"["AccessDeniedException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                served_error_body(body.as_bytes()).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A SUCCESS body is returned byte-identical, whatever members it carries — the rule
    /// is about a failure's envelope only.
    #[test]
    fn a_success_is_returned_untouched() {
        for (case, status, body) in [
            ("a created resource", 201u16, r#"{"id":"x","description":""}"#),
            ("a read", 200, r#"{"__type":"not an error at all"}"#),
        ] {
            let answered = with_served_error_body(response(status, body));
            assert_eq!(
                answered.body().bytes().expect("bytes"),
                body.as_bytes(),
                "case {case:?}: a success body must be byte-identical"
            );
        }
    }

    /// A failure's body is rewritten, and a `content-length` the renderer advertised is
    /// corrected to the new length.
    #[test]
    fn a_rewritten_failure_body_is_described_by_its_advertised_length() {
        let body = r#"{"__type":"AccessDeniedException","message":""}"#;
        let mut resp = response(403, body);
        resp.headers_mut()
            .insert("content-length", body.len().to_string());
        let answered = with_served_error_body(resp);
        let bytes = answered.body().bytes().expect("bytes").to_vec();
        assert_eq!(
            bytes,
            b"{}".to_vec(),
            "expected a body carrying no members, got {:?}",
            String::from_utf8_lossy(&bytes)
        );
        let advertised = answered.headers().get("content-length");
        assert_eq!(
            advertised,
            Some(bytes.len().to_string().as_str()),
            "the advertised length must describe the rewritten body, got {advertised:?}"
        );
    }
}
