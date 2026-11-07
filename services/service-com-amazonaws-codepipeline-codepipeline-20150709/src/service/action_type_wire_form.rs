// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of an `ActionType`: the shape discriminator its `id` carries.
//!
//! # The gap this closes
//!
//! This service speaks `awsJson1_1` (`smithy_query query="service_info"`), and on that
//! protocol an `ActionTypeId` arrives at a client carrying `__type` — the member the
//! awsJson family names a serialized shape with — beside the four members the shape
//! declares:
//!
//! ```text
//! "id": {"__type": "ActionTypeId", "category": "Source", "owner": "AWS", …}
//! ```
//!
//! `__type` is a PROTOCOL member rather than a modelled one: the model declares it
//! nowhere, so the generated serializer (which writes exactly the members `ActionTypeId`
//! declares) cannot emit it, and a client that keys an action type's shape off that
//! discriminator — the same discriminator it reads an error's class from — finds nothing.
//! The framework writes one for an ERROR body
//! (`vendor/smithy-schema-server/src/rpc.rs`'s `__TYPE_MEMBER_SCHEMA`), but nothing on the
//! success path does, and `vendor/` is read-only.
//!
//! So it is closed here, on the SERVICE side, at the typed [`Operation`] seam: the wrapped
//! operation's output is serialized with the discriminator written into every
//! `ActionTypeId` it carries, and nothing else about the walk changes — every other
//! member, its name, its order and its schema stay the ones the model gave it.
//!
//! # Which operations serve one
//!
//! The two whose output carries an `ActionType`: `ListActionTypes` (`actionTypes`) and
//! `CreateCustomActionType` (`actionType`) — the only two members in this model that
//! target the `ActionType` shape. Both are served through this wrapper from
//! [`crate::service::management_events::audited_table`], where every operation of this
//! service is constructed.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, PreDispatchProtocol, Service,
};

/// The member the awsJson protocols name a serialized shape in, as the framework's own
/// error renderer declares it.
static TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// The member whose value carries the discriminator: `ActionType$id`, the one member of
/// this model that a recorded response shows it on.
///
/// Matched on the MEMBER's shape id, which is what the codec hands the serializer for a
/// nested structure, so the rule holds wherever an `ActionType` is serialized — inside
/// `ListActionTypes`' list and inside `CreateCustomActionType`'s single answer alike —
/// and reaches no other structure of this service.
static ACTION_TYPE_ID_MEMBER: ShapeId = ShapeId::from_static(
    "com.amazonaws.codepipeline#ActionType$id",
    "com.amazonaws.codepipeline",
    "id",
);

/// The shape name the discriminator carries, which is the target of that member.
const ACTION_TYPE_ID_SHAPE: &str = "ActionTypeId";

/// One output shape, serialized with the discriminator on every `ActionTypeId` under it.
///
/// Built by [`ActionTypeWireFormOperation`]; there is no reason to name this type
/// elsewhere.
pub struct ActionTypeWireForm<T>(pub T);

impl<T: SerializableStruct> SerializableStruct for ActionTypeWireForm<T> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DiscriminatingSerializer { inner: s })
    }
}

/// One structure NESTED under a wrapped output, re-wrapped so the rule reaches every
/// depth: the `id` sits inside an `ActionType`, which sits inside the output or inside a
/// list of them, and the framework's codec walks a nested structure with its OWN
/// serializer, so the wrapper has to re-enter at each level.
struct Nested<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for Nested<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0
            .serialize_members(&mut DiscriminatingSerializer { inner: s })
    }
}

/// The `ActionTypeId` structure, serialized with the shape discriminator beside its own
/// members.
struct Discriminated<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for Discriminated<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&TYPE_MEMBER_SCHEMA, ACTION_TYPE_ID_SHAPE)?;
        // Still through the wrapper, so a structure nested under this one would be
        // reached by the same rule rather than escaping it.
        self.0
            .serialize_members(&mut DiscriminatingSerializer { inner: s })
    }
}

/// The serializer the wrapped output is walked with: every call is forwarded to the
/// codec's own serializer unchanged, except the `ActionType$id` member, which is written
/// carrying the shape discriminator.
struct DiscriminatingSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for DiscriminatingSerializer<'_> {
    /// The one rewrite: the member's own schema, with the discriminator written into the
    /// structure it carries.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if schema.shape_id() == &ACTION_TYPE_ID_MEMBER {
            return self.inner.write_struct(schema, &Discriminated(value));
        }
        self.inner.write_struct(schema, &Nested(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut DiscriminatingSerializer { inner })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut DiscriminatingSerializer { inner })
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

/// One modelled operation served with every `ActionTypeId` of its response carrying the
/// awsJson shape discriminator.
pub struct ActionTypeWireFormOperation<O> {
    /// The operation that serves the call.
    operation: O,
    /// The protocol envelope the framework renders a pre-dispatch failure of this
    /// operation in — the same value the generated shim passes.
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<O> ActionTypeWireFormOperation<O> {
    /// Wrap one operation, already constructed from the shared engine, whose pre-dispatch
    /// failures render in `pre_dispatch`.
    pub fn new(operation: O, pre_dispatch: PreDispatchProtocol<'static>) -> Self {
        Self {
            operation,
            pre_dispatch,
        }
    }
}

impl<O: Operation> Operation for ActionTypeWireFormOperation<O> {
    type Input = O::Input;
    type Output = ActionTypeWireForm<O::Output>;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The handler's own answer, untouched: only how it is RENDERED changes, which is
        // the wrapper around it rather than anything about the output it built.
        self.operation
            .handle(input, ctx)
            .await
            .map(ActionTypeWireForm)
    }
}

impl<O> ErasedOperation for ActionTypeWireFormOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation: the handler it calls is the wrapped one, so the only difference
            // is the serializer the output is written with.
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, self.pre_dispatch).await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: wrapping must not change which
        // credentials sign a call.
        self.operation.auth_schemes()
    }
}
