// SPDX-License-Identifier: CC0-1.0

//! How this service's `awsJson1_0` responses serialize a NESTED structure.
//!
//! Step Functions is served by AWS's SWF code base, and the structures it shares
//! with that base model are serialized with the awsJson type discriminator
//! beside their own members: a `__type` key carrying the structure's shape id in
//! AWS's SWF base-model namespace, [`SWF_BASE_MODEL_NAMESPACE`]. The recordings
//! of this API show it on `LoggingConfiguration`, `TracingConfiguration`,
//! `EncryptionConfiguration`, `CloudWatchEventsExecutionDataDetails` and every
//! `HistoryEvent` of an execution's history — the shapes of
//! [`DISCRIMINATED_SHAPES`] — and NOT on the service's own list items (a
//! `ListTagsForResource` tag, a `ListStateMachines` item), so the discriminator is
//! emitted for exactly those shapes, wherever in whatever operation's response
//! body they occur, and for nothing else.
//!
//! The framework's own serializer writes a structure member from the MEMBER
//! schema (the target's schema never reaches it), so the shape a member resolves
//! to is read from [`DISCRIMINATED_MEMBERS`], a catalog of this service's Smithy
//! model checked against the model itself by
//! `tests/all/nested_type_discriminator.rs`.
//!
//! This is the one place the rule lives: [`SwfJsonProtocol`] is the protocol
//! `service.rs` hands the dispatch sequence through `override_protocol`, so
//! EVERY operation's response passes through it and no handler and no shape
//! declaration mentions the discriminator at all.
//!
//! The same seam owns the STATUS a refused request is answered with, for the
//! same reason — it is a property of this service's wire protocol rather than
//! of any one operation. See [`error_status`].

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::json::JsonCodec;
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::rpc::HttpRpcServerProtocol;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The namespace the discriminator names a shared structure in.
///
/// Step Functions' public model is `com.amazonaws.sfn`, but the wire
/// discriminator carries AWS's internal SWF base-model namespace — the same
/// lineage as the model's `xmlNamespace`
/// (`http://swf.amazonaws.com/doc/2015-07-20/`), and the counterpart of the
/// `com.amazonaws.swf.service.v2.model#…` an error's own `__type` carries.
pub const SWF_BASE_MODEL_NAMESPACE: &str = "com.amazonaws.swf.base.model";

/// The structures a response body serializes with a discriminator, by shape name.
///
/// Source: the recordings quoted by
/// `issues/.../states-describe-responses-omit-the-type-discriminator-on.md` and
/// the two resolved issues before it, which show `__type` on the four
/// configuration and data-detail structures and on no list item; and the
/// recordings quoted by
/// `issues/.../states-step-functions-execution-history-is-serialized-without-the.md`,
/// which show it on every element of `GetExecutionHistoryOutput.events`
/// (`$.events[0].__type: expected 'com.amazonaws.swf.base.model#HistoryEvent'`) —
/// a history event is a structure of that same SWF base model, which is why it is
/// discriminated where this service's OWN list items are not.
pub const DISCRIMINATED_SHAPES: &[&str] = &[
    "CloudWatchEventsExecutionDataDetails",
    "EncryptionConfiguration",
    "HistoryEvent",
    "LoggingConfiguration",
    "TracingConfiguration",
];

/// Every member of this service's model that targets one of
/// [`DISCRIMINATED_SHAPES`], as `(member name, target shape name)`.
///
/// Source: the Smithy model
/// (`smithy_query(query="shape",shape="DescribeStateMachineOutput,DescribeExecutionOutput,
/// DescribeStateMachineForExecutionOutput,StartSyncExecutionOutput,CreateStateMachineInput,
/// UpdateStateMachineInput,LogDestination")`). The framework hands the serializer a
/// member's schema rather than its target's, so this is how a member is resolved
/// to the shape the discriminator names; `nested_type_discriminator.rs` re-derives
/// it from the model and fails if a row is wrong or a member is missing.
pub const DISCRIMINATED_MEMBERS: &[(&str, &str)] = &[
    ("encryptionConfiguration", "EncryptionConfiguration"),
    ("inputDetails", "CloudWatchEventsExecutionDataDetails"),
    ("loggingConfiguration", "LoggingConfiguration"),
    ("outputDetails", "CloudWatchEventsExecutionDataDetails"),
    ("tracingConfiguration", "TracingConfiguration"),
];

/// The `__type` value for one shape name: the shape id in the SWF base-model
/// namespace.
pub fn discriminator(shape_name: &str) -> String {
    format!("{SWF_BASE_MODEL_NAMESPACE}#{shape_name}")
}

/// The HTTP status every CLIENT error of this service is answered with.
///
/// `awsJson1_0` names the error in the body's `__type` rather than in the status
/// line, and this service answers one status for the whole client-error class:
/// the recordings of its refusals show HTTP 400 for error shapes whose
/// `@httpError` is 404 (the `ResourceNotFound` family) and 409 (the
/// `ConflictException` family) alike. See [`error_status`].
pub const CLIENT_ERROR_STATUS: u16 = 400;

/// The status a modelled error is answered with, from the status its shape
/// declares.
///
/// A client error (`4xx`) is answered with [`CLIENT_ERROR_STATUS`] whatever
/// `@httpError` the shape carries, so a caller reads WHICH refusal it was from
/// the error code in the body — the only place this service reports it. A
/// server error keeps its own status: nothing recorded contradicts it, and a
/// `5xx` is what tells a client the call may be retried as sent.
pub fn error_status(modelled_status: u16) -> u16 {
    match modelled_status {
        400..=499 => CLIENT_ERROR_STATUS,
        other => other,
    }
}

/// The discriminator the structure about to be written at `schema` carries, or
/// `None` for a structure no recording shows one on.
///
/// `schema` is a MEMBER schema when the structure occupies a structure member
/// (the framework's serializer passes the member, not its target) and the
/// target's own schema when it is a collection element, so both are resolved:
/// the first through [`DISCRIMINATED_MEMBERS`], the second by its shape name.
fn discriminator_for(schema: &Schema) -> Option<String> {
    let shape_name = match schema.member_name() {
        Some(member) => DISCRIMINATED_MEMBERS
            .iter()
            .find(|(name, _)| *name == member)
            .map(|(_, target)| *target)?,
        None => DISCRIMINATED_SHAPES
            .iter()
            .copied()
            .find(|shape| *shape == schema.shape_id().shape_name())?,
    };
    Some(discriminator(shape_name))
}

/// Schema for the `__type` member itself.
///
/// The awsJson codec reads a member's wire key off `member_name()`, so the name
/// given here IS the key written. It is not a member of any modeled shape —
/// which is exactly why the discriminator cannot come from a shape declaration
/// and is written here instead.
static TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    aws_smithy_schema::ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// A structure serialized with its own members only, but with every structure
/// NESTED under it passed through the discriminator rule.
///
/// This is what wraps an operation's output: a response's top-level body carries
/// no `__type` (the operation names the shape), only the structures inside it do.
struct Discriminating<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for Discriminating<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut NestedSerializer { inner: serializer })
    }
}

/// A structure serialized as its discriminator followed by its own members.
struct WithDiscriminator<'a> {
    discriminator: String,
    inner: &'a dyn SerializableStruct,
}

impl SerializableStruct for WithDiscriminator<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(&TYPE_MEMBER_SCHEMA, &self.discriminator)?;
        self.inner
            .serialize_members(&mut NestedSerializer { inner: serializer })
    }
}

/// The serializer the members of an already-opened structure are written
/// through: every write is the wrapped serializer's, except that a nested
/// structure is written with its discriminator when it has one and descended
/// into either way, so a structure at any depth is reached.
struct NestedSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for NestedSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        match discriminator_for(schema) {
            Some(discriminator) => self.inner.write_struct(
                schema,
                &WithDiscriminator {
                    discriminator,
                    inner: value,
                },
            ),
            None => self.inner.write_struct(schema, &Discriminating(value)),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|elements| {
            write_elements(&mut NestedSerializer { inner: elements })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|entries| {
            write_entries(&mut NestedSerializer { inner: entries })
        })
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

/// This service's wire protocol: `awsJson1_0` exactly as the model declares it,
/// with a response body's nested structures carrying the SWF type discriminator.
///
/// Everything but the success body is the framework's own awsJson1_0 protocol,
/// delegated to unchanged — routing, request deserialization, and the error
/// envelope (whose top-level `__type` the framework already writes).
#[derive(Debug)]
pub struct SwfJsonProtocol {
    inner: HttpRpcServerProtocol<JsonCodec>,
}

impl SwfJsonProtocol {
    /// Build the protocol once, at service construction: `override_protocol`
    /// borrows it from `&self` and must never build one per request.
    pub fn new() -> Self {
        Self {
            inner: aws_smithy_schema_server::AwsJson10(),
        }
    }
}

impl Default for SwfJsonProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for SwfJsonProtocol {
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
        self.inner.serialize_output(
            &Discriminating(output),
            output_schema,
            operation_name,
            success_status,
        )
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // The envelope is the framework's own `awsJson1_0`; what this service
        // decides is the STATUS, which is one value for the whole client-error
        // class rather than the one each error shape's `@httpError` declares.
        // See [`error_status`].
        self.inner.serialize_error(
            error,
            error_schema,
            error_code,
            error_status(http_status),
        )
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
