// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` ERROR document carries a modelled error's members.
//!
//! RULE: every error document Amazon EFS answers with CARRIES the error shape's
//! `Message` member — when no rule produces a sentence for that error class the member
//! is written with JSON `null` rather than left out — so a client can tell "this class
//! carries no sentence" from "the sentence is the empty string".
//!
//! RULE: the operations that WRITE a replication configuration answer a not-found
//! refusal with a document holding the SENTENCE ALONE, naming the error shape in the
//! `x-amzn-errortype` header instead of in an `ErrorCode` member. [`MessageOnly`] is
//! that body, and its own documentation states the rule; the handlers reach it through
//! the [`FileSystemNotFound`] / [`ReplicationNotFound`] aliases.
//!
//! Every error shape in this service's model is a structure with a required `ErrorCode`
//! and an OPTIONAL `Message` (target `ErrorMessage`, a plain string with no HTTP
//! binding), some of them with one more required member naming the resource:
//! `smithy_query(query="shape",shape="AccessPointAlreadyExists",
//! model_path="services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json")`.
//! Most of this service's refusals DO carry a request-derived sentence (see
//! [`crate::state::file_system_not_found_message`] and its neighbours), and one class —
//! `AccessPointAlreadyExists` — carries none at all, because neither the model nor the
//! documentation states a rule that would produce one and the actionable datum travels
//! in its required `AccessPointId`.
//!
//! The framework's generic serializer writes an absent optional member by writing
//! NOTHING, which is right for a success body but drops the key out of the error
//! document — and an SDK cannot see the difference the rule is about: botocore
//! substitutes the EMPTY STRING for an absent `Message` key in a JSON error document
//! (`{"Error": {"Message": "", "Code": ""}}` is the dict it starts from and fills with
//! `body.get("message", body.get("Message", ""))`), and reports `None` only for a key
//! whose value is JSON null. An omitted member therefore reaches every client as
//! `Message == ""`, i.e. as "the sentence is empty", which is the opposite of what this
//! error class states.
//!
//! [`NullMessageRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`], so EVERY modelled
//! failure of EVERY operation goes out with the member present and no handler, shape
//! declaration or error constructor mentions the rendering at all. Everything else —
//! routing, request deserialization, success bodies (where an absent optional member
//! stays absent, as in every AWS JSON response), and an error whose `Message` IS
//! supplied — is the framework's own restJson1 protocol, delegated to unchanged.

use std::marker::PhantomData;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The member names an error document's message can occupy.
///
/// Source: the `Message` member every error shape of this service's model declares
/// (`smithy_query(query="shape",shape="<AnyError>")` reports
/// `members: {ErrorCode: {...}, Message: {target: ErrorMessage}}`). The lowercase
/// spelling is accepted too because that is the other name an AWS JSON error document
/// uses for the same member — it is the Smithy default for the detail member, and the
/// scaffold channel's own synthetic shapes spell it that way — and an SDK reads
/// whichever of the two is present.
const MESSAGE_MEMBER_NAMES: &[&str] = &["Message", "message"];

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

/// A modelled error shape whose refusal body this service answers with as the SENTENCE
/// ALONE — see [`MessageOnly`], which renders it.
pub trait MessageOnlyShape {
    /// The shape's own schema, as this crate's generated types declare it. The member
    /// written is that schema's message member, so the key on the wire stays the
    /// model's.
    const SCHEMA: &'static Schema;
    /// The shape name the error document's `__type` and the `x-amzn-errortype` header
    /// carry: with no `ErrorCode` member in the body, this is where a client of these
    /// operations reads the error class.
    const SHAPE_NAME: &'static str;
}

/// `FileSystemNotFound`, as the replication-write path renders it.
#[derive(Debug)]
pub struct FileSystemNotFoundShape;

impl MessageOnlyShape for FileSystemNotFoundShape {
    const SCHEMA: &'static Schema = &crate::ops::types::SCHEMA_FILE_SYSTEM_NOT_FOUND;
    const SHAPE_NAME: &'static str = "FileSystemNotFound";
}

/// `ReplicationNotFound`, as the replication-write path renders it.
#[derive(Debug)]
pub struct ReplicationNotFoundShape;

impl MessageOnlyShape for ReplicationNotFoundShape {
    const SCHEMA: &'static Schema = &crate::ops::types::SCHEMA_REPLICATION_NOT_FOUND;
    const SHAPE_NAME: &'static str = "ReplicationNotFound";
}

/// The body of a refusal that carries the SENTENCE ALONE: the message member of `S` and
/// nothing else of the shape, in particular no `ErrorCode`.
///
/// RULE: on the operations that WRITE a replication configuration
/// (`CreateReplicationConfiguration`, `DeleteReplicationConfiguration`) Amazon EFS
/// answers a not-found refusal with a document holding only the message, and names the
/// error shape in the `x-amzn-errortype` header (and the `__type` discriminator) rather
/// than in an `ErrorCode` member — unlike the replication READ, whose document carries
/// `ErrorCode` as well. The status, the shape name and the sentence are unchanged by
/// this type; only the member is left out.
///
/// It is used as the PAYLOAD of those operations' `FileSystemNotFound` /
/// `ReplicationNotFound` error variants (as [`FileSystemNotFound`] and
/// [`ReplicationNotFound`]): `#[operation_error]` names a variant's payload type after
/// the variant, so an operation chooses this rendering by importing the alias, which
/// shadows the generated shape of the same name.
pub struct MessageOnly<S: MessageOnlyShape> {
    message: String,
    shape: PhantomData<S>,
}

/// Written by hand rather than derived: a derive would bound the marker `S` on `Debug`,
/// and the marker carries no data to print. The shape name comes from `S` instead.
impl<S: MessageOnlyShape> std::fmt::Debug for MessageOnly<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(S::SHAPE_NAME)
            .field("message", &self.message)
            .finish()
    }
}

impl<S: MessageOnlyShape> MessageOnly<S> {
    /// The refusal body carrying `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            shape: PhantomData,
        }
    }

    /// The wire error code, which the generated `OperationError::shape_name` reads off
    /// the variant's payload type.
    pub fn shape_name() -> &'static str {
        S::SHAPE_NAME
    }
}

impl<S: MessageOnlyShape> std::fmt::Display for MessageOnly<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", S::SHAPE_NAME, self.message)
    }
}

impl<S: MessageOnlyShape> std::error::Error for MessageOnly<S> {}

impl<S: MessageOnlyShape> SerializableStruct for MessageOnly<S> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // The shape's own message member, so `@jsonName` and the model's casing are
        // honoured; a shape that declares none has no sentence to write.
        match message_member(S::SCHEMA) {
            Some(message) => serializer.write_string(message, self.message.as_str()),
            None => Ok(()),
        }
    }
}

/// `FileSystemNotFound` with a body carrying the sentence alone; see [`MessageOnly`].
pub type FileSystemNotFound = MessageOnly<FileSystemNotFoundShape>;

/// `ReplicationNotFound` with a body carrying the sentence alone; see [`MessageOnly`].
pub type ReplicationNotFound = MessageOnly<ReplicationNotFoundShape>;

/// An error structure serialized with its own members, plus its message member as JSON
/// `null` when it supplied none.
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
/// error document that always carries the error shape's `Message` member.
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
        match message_member(error_schema) {
            Some(message) => self.inner.serialize_error(
                &MessageAlwaysPresent {
                    inner: error,
                    message,
                },
                error_schema,
                error_code,
                http_status,
            ),
            // A shape with no message member has nothing to keep present.
            None => self
                .inner
                .serialize_error(error, error_schema, error_code, http_status),
        }
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
