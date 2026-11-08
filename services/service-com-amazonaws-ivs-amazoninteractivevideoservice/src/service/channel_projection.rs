// SPDX-License-Identifier: CC0-1.0

//! The WIRE PROJECTION of a channel: the failover-slate configuration ARN every channel
//! object this service serves carries.
//!
//! # The gap this closes
//!
//! A channel's optional configuration ARNs are reported as a PRESENT key holding the
//! empty string while nothing is configured — the rule the model writes out for each one
//! it declares, and the one `crate::state::CHANNEL_FAILOVER_SLATE_CONFIGURATION_ARN`
//! states for the failover slate. The failover slate is the one of those ARNs this model
//! revision declares on neither `Channel` nor `ChannelSummary`
//! (`smithy_query(query="shape",shape="Channel,ChannelSummary")`), so it cannot be a
//! field of either generated shape and the generated serializer — which writes exactly
//! the members a shape declares — cannot emit it. A client that decides "is a failover
//! slate configured?" by comparing the value with the empty string then finds no key at
//! all, which is a different answer from "nothing is configured".
//!
//! So it is written by the SERIALIZER, at the one place every modelled response of this
//! service is rendered ([`smithy_server_runtime::Service::override_protocol`], wired in
//! `super`), from the channel the handler already returned rather than from the request:
//! nothing is keyed on which operation was called, and a channel object gains the member
//! wherever it is serialized — the full `Channel` a single or batch read answers with and
//! the `ChannelSummary` a list answers with alike.
//!
//! [`ChannelProjection`] is that wrapper: every write is forwarded to the serializer the
//! framework's protocol built, unchanged, except a structure that IS a channel, whose own
//! members are followed by the failover-slate ARN. The wrapper re-enters at each level,
//! because the codec walks a nested structure and a list element with its own serializer.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// This service's namespace, which the synthetic member below sits in: it is a member of
/// one of ITS shapes that its model does not declare. Nothing collides — no member of
/// that name exists in the model — and the id never leaves the process, since the JSON
/// codec writes a member's NAME as the object key.
const NAMESPACE: &str = "com.amazonaws.ivs";

/// The index a synthetic member schema carries.
///
/// A member's index orders the slots the DESERIALIZE path fills; this schema is only ever
/// written, and the JSON writer reads a member's name alone.
const SYNTHETIC_INDEX: usize = 0;

/// The failover-slate configuration ARN of a channel, beside the configuration ARNs the
/// shape declares.
static M_FAILOVER_SLATE_CONFIGURATION_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ivs#Channel$failoverSlateConfigurationArn",
        NAMESPACE,
        "failoverSlateConfigurationArn",
    ),
    ShapeType::String,
    "failoverSlateConfigurationArn",
    SYNTHETIC_INDEX,
)
.with_json_name("failoverSlateConfigurationArn");

/// Every schema this service's codec hands the serializer for a structure that projects a
/// channel.
///
/// A channel reaches the wire two ways, and the codec names the structure differently in
/// each: as a LIST element it is written against the element shape's own schema
/// (`Channel`, `ChannelSummary`), and as a single member against that MEMBER's schema. The
/// members are the three this model declares targeting `Channel` —
/// `smithy_query(query="operation",operation="CreateChannel,GetChannel,UpdateChannel")`,
/// each with the output member `channel` — and no member targets `ChannelSummary`
/// directly. Matching on these ids reaches every channel object this service serves and
/// no other structure of it.
static CHANNEL_PROJECTIONS: [ShapeId; 5] = [
    ShapeId::from_static("com.amazonaws.ivs#Channel", NAMESPACE, "Channel"),
    ShapeId::from_static("com.amazonaws.ivs#ChannelSummary", NAMESPACE, "ChannelSummary"),
    ShapeId::from_static(
        "com.amazonaws.ivs#CreateChannelResponse$channel",
        NAMESPACE,
        "channel",
    ),
    ShapeId::from_static(
        "com.amazonaws.ivs#GetChannelResponse$channel",
        NAMESPACE,
        "channel",
    ),
    ShapeId::from_static(
        "com.amazonaws.ivs#UpdateChannelResponse$channel",
        NAMESPACE,
        "channel",
    ),
];

/// Whether the structure being written against `schema` is a channel object.
fn projects_a_channel(schema: &Schema) -> bool {
    CHANNEL_PROJECTIONS
        .iter()
        .any(|projection| schema.shape_id() == projection)
}

/// One structure, serialized with the channel projection applied to every channel object
/// under it.
///
/// This is what this service's protocol hands the framework's `serialize_output` in place
/// of the output it was given, so the status, the headers and every modelled member stay
/// exactly what the framework rendered.
pub struct ChannelProjection<'a> {
    inner: &'a dyn SerializableStruct,
}

impl<'a> ChannelProjection<'a> {
    /// Wrap the structure a modelled response is about to be rendered from.
    pub fn new(inner: &'a dyn SerializableStruct) -> Self {
        Self { inner }
    }
}

impl SerializableStruct for ChannelProjection<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.inner.serialize_members(&mut Projecting { inner: s })
    }
}

/// One channel object, serialized with the failover-slate ARN beside the members its
/// shape declares.
struct ProjectedChannel<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for ProjectedChannel<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // The shape's own members first, still through the wrapper so a structure nested
        // in a channel is reached by the same rule rather than escaping it.
        self.0.serialize_members(&mut Projecting { inner: s })?;
        s.write_string(
            &M_FAILOVER_SLATE_CONFIGURATION_ARN,
            crate::state::CHANNEL_FAILOVER_SLATE_CONFIGURATION_ARN,
        )
    }
}

/// The serializer a wrapped structure's members are written through: the codec's own, with
/// a channel object written as the projection of one.
struct Projecting<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for Projecting<'_> {
    /// The one rewrite: a structure that is a channel carries the projection's member
    /// after its own; every other structure is re-wrapped so the rule reaches each depth.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if projects_a_channel(schema) {
            return self.inner.write_struct(schema, &ProjectedChannel(value));
        }
        self.inner
            .write_struct(schema, &ChannelProjection::new(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut Projecting { inner })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut Projecting { inner })
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
