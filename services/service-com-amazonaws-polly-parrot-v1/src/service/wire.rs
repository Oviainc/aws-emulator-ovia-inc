// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` ERROR response NAMES the error shape and carries a
//! modelled exception's `message` member.
//!
//! RULE: every error document Amazon Polly answers with CARRIES the error shape's
//! `message` member — when the service supplies no message the member is written with
//! JSON `null` rather than left out — so a client can tell "no message was supplied"
//! from "an empty message was supplied".
//!
//! RULE: the `x-amzn-errortype` header of an error answer names the error shape
//! QUALIFIED by the service's own error namespace — the shape name, a colon, and the
//! namespace URI the service's model declares — because the shape name is the only
//! variable part of that header value and the namespace is a property of the service
//! that answered. An SDK or a proxy that matches the whole header value reads the
//! qualified form AWS sends, and matching the shape name alone still works because the
//! qualified value begins with it. It is the namespace of THIS SERVICE'S MODEL, so only
//! a shape that model declares carries it: the framework's synthetic shapes render as
//! the request boundary renders them (see [`declared_by_this_service`]).
//!
//! Every error shape in this service's model carries exactly one optional member,
//! `message` (target `ErrorMessage`, a string with no HTTP binding:
//! `smithy_query(query="shape",shape="SynthesisTaskNotFoundException",
//! model_path="services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json")`),
//! and one code — `SynthesisTaskNotFoundException` — is answered with no message at
//! all (see [`crate::service::errors`]). The framework's generic serializer writes an
//! absent optional member by writing NOTHING, which is right for a success body but
//! drops the key out of the error document: every SDK then reports the error's message
//! as the empty string it substitutes for an absent key, where Amazon Polly's own
//! answer reads as "none".
//!
//! [`NullMessageRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`], so EVERY modelled
//! failure of EVERY operation goes out with the member present and with the qualified
//! errortype header, and no handler, shape declaration or error constructor mentions
//! either rendering at all. Everything else —
//! routing, request deserialization, success bodies, and an error whose message IS
//! supplied — is the framework's own restJson1 protocol, delegated to unchanged.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The member names an error document's message can occupy.
///
/// Source: the `message` member every error shape of this service's model declares
/// (`smithy_query(query="shape",shape="<AnyException>")` reports
/// `members: {message: {target: ErrorMessage}}`). The capitalised spelling is accepted
/// too because that is the other name an AWS JSON error document uses for the same
/// member, and an SDK reads whichever of the two is present.
const MESSAGE_MEMBER_NAMES: &[&str] = &["message", "Message"];

/// The header an error answer names its shape in.
const ERRORTYPE_HEADER: &str = "x-amzn-errortype";

/// The namespace every shape of this service's own model lives in, as a shape id
/// spells it (`com.amazonaws.polly#SynthesisTaskNotFoundException`).
const MODEL_NAMESPACE: &str = "com.amazonaws.polly";

/// The error namespace this service qualifies that header's shape name with.
///
/// Source: the service's own `@xmlNamespace` URI — `smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json")`
/// reports `xmlNamespace: {uri: "http://polly.amazonaws.com/doc/v1"}`. It is a property
/// of the SERVICE, so it is the same text behind every error code this service answers
/// and only the shape name before it varies. The generated service identity does not
/// carry it for a JSON protocol, which is why it is named here.
const ERROR_NAMESPACE: &str = "http://polly.amazonaws.com/doc/v1";

/// What separates the shape name from the namespace in that header value.
const ERROR_NAMESPACE_SEPARATOR: char = ':';

/// Whether `error_schema` is an error THIS SERVICE'S MODEL declares, as opposed to one
/// of the framework's synthetic shapes (the scaffold channel's 501, the request-shape
/// refusal), which live in `smithy.framework`.
///
/// Only the former is named in the service's error namespace. A synthetic shape is not
/// an error of this service's model and is rendered on the request-boundary path too,
/// where this protocol never runs, so qualifying it here would give one logical failure
/// two different header values depending on which layer refused the request.
fn declared_by_this_service(error_schema: &Schema) -> bool {
    error_schema
        .shape_id()
        .to_string()
        .split_once('#')
        .is_some_and(|(namespace, _)| namespace == MODEL_NAMESPACE)
}

/// Qualify the errortype header of an error response the framework has already
/// rendered: the shape name it wrote, this service's namespace behind it.
///
/// The header is REWRITTEN rather than appended to, so passing one response through
/// here twice cannot qualify it twice. Only a response that ALREADY names its shape
/// there is touched, so a protocol that sends no errortype header is not given one.
///
/// Set FALLIBLY, for the reason the framework's own insert is: a header value rejects
/// bytes a shape name could carry, and a panic on the ERROR path would become a 500 an
/// SDK retries. On a rejected value the header keeps the name the framework wrote.
fn qualify_errortype_header(response: &mut Response) {
    let Some(shape) = response
        .headers()
        .get(ERRORTYPE_HEADER)
        .map(str::to_string)
    else {
        return;
    };
    if shape.contains(ERROR_NAMESPACE_SEPARATOR) {
        return;
    }
    let qualified = format!("{shape}{ERROR_NAMESPACE_SEPARATOR}{ERROR_NAMESPACE}");
    if ::http::HeaderValue::from_str(&qualified).is_ok() {
        response.headers_mut().insert(ERRORTYPE_HEADER, qualified);
    }
}

/// The error shape's message member, or `None` for a shape that declares none.
///
/// Only a plain string member in the BODY qualifies: a member bound to a header or to
/// the payload is not part of the JSON object this rule is about, and writing `null`
/// for it would mean something else entirely.
fn message_member(error_schema: &Schema) -> Option<&Schema> {
    error_schema.members().iter().copied().find(|member| {
        member
            .member_name()
            .is_some_and(|name| MESSAGE_MEMBER_NAMES.contains(&name))
            && matches!(member.shape_type(), ShapeType::String)
            && member.http_header().is_none()
            && member.http_payload().is_none()
            && member.http_query().is_none()
    })
}

/// An error structure serialized with its own members, plus its `message` member as
/// JSON `null` when it supplied none.
struct MessageAlwaysPresent<'a> {
    inner: &'a dyn SerializableStruct,
    message: &'a Schema,
}

impl SerializableStruct for MessageAlwaysPresent<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // The borrow of `serializer` ends with the tracker, so the null below is
        // written through the same serializer the members went to — after them, which
        // is where the absent key belongs in the object.
        let wrote_message = {
            let mut tracker = TrackMessage {
                inner: &mut *serializer,
                message_name: self.message.member_name(),
                wrote_message: false,
            };
            self.inner.serialize_members(&mut tracker)?;
            tracker.wrote_message
        };
        if !wrote_message {
            serializer.write_null(self.message)?;
        }
        Ok(())
    }
}

/// The serializer an error's members are written through: every write is the wrapped
/// serializer's, and a write of the message member is noted so the wrapper above knows
/// whether the key still has to be written as `null`.
struct TrackMessage<'a> {
    inner: &'a mut dyn ShapeSerializer,
    message_name: Option<&'a str>,
    wrote_message: bool,
}

impl TrackMessage<'_> {
    /// Note a write of the message member, whatever shape it was written as.
    fn note(&mut self, schema: &Schema) {
        if self.message_name.is_some() && schema.member_name() == self.message_name {
            self.wrote_message = true;
        }
    }
}

impl ShapeSerializer for TrackMessage<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_struct(schema, value)
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_list(schema, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_map(schema, write_entries)
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.note(schema);
        self.inner.write_null(schema)
    }
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with an
/// error document that always carries the error shape's `message` member.
#[derive(Debug)]
pub struct NullMessageRestJson {
    inner: RestJsonServerProtocol,
}

impl NullMessageRestJson {
    /// Build the protocol once, at service construction: `override_protocol` hands back
    /// a borrow of it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table
    /// is the model's); an author-overridden protocol drives only the modelled sequence
    /// — `deserialize_input`, `serialize_output`, `serialize_error` — none of which
    /// consults routes.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for NullMessageRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for NullMessageRestJson {
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
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
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
        let mut response = match message_member(error_schema) {
            Some(message) => self.inner.serialize_error(
                &MessageAlwaysPresent {
                    inner: error,
                    message,
                },
                error_schema,
                error_code,
                http_status,
            )?,
            // A shape with no message member has nothing to keep present.
            None => self
                .inner
                .serialize_error(error, error_schema, error_code, http_status)?,
        };
        // Every error this service's MODEL declares names its shape qualified, whether
        // or not the shape had a message to render.
        if declared_by_this_service(error_schema) {
            qualify_errortype_header(&mut response);
        }
        Ok(response)
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
