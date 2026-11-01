// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` requests and responses carry a TIMESTAMP.
//!
//! RULE: a timestamp member whose model declares `@timestampFormat("date-time")` is
//! read from, and written to, the JSON wire as an ISO 8601 / RFC 3339 date-time
//! STRING (`"2024-01-01T00:00:00Z"`), never as the epoch-seconds NUMBER the
//! `restJson1` protocol defaults to.
//!
//! The rule is the protocol's here, not any one shape's: this model declares a single
//! timestamp shape, `com.amazonaws.location#Timestamp`, and it carries
//! `@timestampFormat("date-time")` —
//! `smithy_query(query="shape",shape="Timestamp,DevicePositionUpdate,GetDevicePositionResponse",
//! model_path="services/service-com-amazonaws-location-locationservice/smithy-model/com-amazonaws-location-locationservice.json")`
//! reports `"timestampFormat": "date-time"` on the shape and on every member that
//! targets it. So EVERY timestamp of EVERY shape of this service — a device position's
//! `SampleTime` and `ReceivedTime`, a resource's `CreateTime`/`UpdateTime`, a key's
//! `ExpireTime`, a history query's `StartTimeInclusive`/`EndTimeExclusive` — is a
//! date-time one, in both directions.
//!
//! WHY IT HAS TO BE APPLIED HERE. The JSON codec resolves a timestamp's wire form from
//! the MEMBER schema alone (`vendor/smithy-schema-server/src/json.rs`'s `read_timestamp`
//! / `write_timestamp` read `Schema::timestamp_format()`) and falls back to the
//! protocol default of epoch seconds when the member schema reports no format. The
//! schemas built from the model carry no member-level format at all (grep
//! `vendor/smithy-macros`: it emits no `with_timestamp_format`), so the date-time STRING
//! an SDK sends failed an integer parse on the way in, and an instant went out as a bare
//! number a client parsing a date-time member cannot read.
//!
//! The structural fix belongs in that codegen, which a service crate cannot edit
//! (`vendor/` is read-only), and the alternative on the service side is to hand-write the
//! schema and codec of every structure that holds a timestamp (forty-odd of them here).
//! So this module closes it once, at the protocol seam `service.rs` already owns: a
//! timestamp whose member schema declares no format is read as, and written as, a
//! date-time value. Nothing else about either walk changes — a member's wire name, its
//! order and its schema stay the ones the model gave it, and no other member kind is
//! touched.
//!
//! The response half mirrors `services/service-com-amazonaws-mq-mq/src/service/
//! iso8601_timestamps.rs`, which closes the same codegen gap for a model whose single
//! timestamp shape also declares `date-time`; keep the two in step. The request half is
//! this service's own: that model binds no timestamp on an input.
//!
//! A member that DOES declare a format keeps it, so this never overrides the model: it
//! only supplies the format the codegen dropped.
//!
//! The wrappers add one stack frame per nesting level and nothing else: no buffer, no
//! copy of the document and no caller-sized allocation. Nesting DEPTH stays bounded by
//! the codec itself, whose `read_struct`/`read_list`/`read_map` take a depth guard and
//! refuse a body nesting past their own maximum, which the protocol boundary renders as a
//! client refusal.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};

/// This model's one timestamp shape, carrying the `@timestampFormat` the model declares
/// on it and the generated member schemas drop.
///
/// The codecs read nothing but [`Schema::timestamp_format`] out of the schema handed to
/// `read_timestamp`, so standing this in for a member that reports no format is what
/// makes the member's declared format reach the parse.
static DATE_TIME_TIMESTAMP: Schema = Schema::new(
    ShapeId::from_static(
        "com.amazonaws.location#Timestamp",
        "com.amazonaws.location",
        "Timestamp",
    ),
    ShapeType::Timestamp,
)
.with_timestamp_format(TimestampFormat::DateTime);

/// The date-time text an instant is reported as: RFC 3339 in UTC, which is what Smithy's
/// `date-time` format is and what [`Format::DateTime`] renders.
///
/// An instant with no date-time rendering at all (one outside the range the formatter can
/// express) keeps the protocol's own number rather than being given a fake string — see
/// [`DateTimeWriter::write_timestamp`].
fn date_time_text(value: &DateTime) -> Option<String> {
    value.fmt(Format::DateTime).ok()
}

// =================================================================================
// Request side
// =================================================================================

/// The inner deserializer one [`DateTimeReader`] forwards to, however it is held.
///
/// The protocol seam hands back an owned `Box<dyn ShapeDeserializer>` while the nested
/// walks hand each level a `&mut dyn ShapeDeserializer`, and both have to wear the same
/// wrapper — so the wrapper is generic over this, and the forwarding body below is
/// written once.
trait InnerDeserializer {
    fn get(&mut self) -> &mut dyn ShapeDeserializer;
    fn peek(&self) -> &dyn ShapeDeserializer;
}

impl InnerDeserializer for Box<dyn ShapeDeserializer + '_> {
    fn get(&mut self) -> &mut dyn ShapeDeserializer {
        &mut **self
    }

    fn peek(&self) -> &dyn ShapeDeserializer {
        &**self
    }
}

impl InnerDeserializer for &mut (dyn ShapeDeserializer + '_) {
    fn get(&mut self) -> &mut dyn ShapeDeserializer {
        &mut **self
    }

    fn peek(&self) -> &dyn ShapeDeserializer {
        &**self
    }
}

/// The deserializer an input is read with: every call is the codec's own, except a
/// timestamp leaf whose member schema declares no format, which is read as a date-time.
///
/// A nested structure, list or map re-enters the wrapper, because the codec walks each
/// level with its own deserializer and a timestamp is rarely the input's own member.
struct DateTimeReader<D>(D);

impl<D: InnerDeserializer> ShapeDeserializer for DateTimeReader<D> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_struct(schema, &mut |member, d| consumer(member, &mut DateTimeReader(d)))
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_list(schema, &mut |d| consumer(&mut DateTimeReader(d)))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_map(schema, &mut |key, d| consumer(key, &mut DateTimeReader(d)))
    }

    fn read_boolean(&mut self, schema: &Schema) -> Result<bool, SerdeError> {
        self.0.get().read_boolean(schema)
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        self.0.get().read_byte(schema)
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        self.0.get().read_short(schema)
    }

    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        self.0.get().read_integer(schema)
    }

    fn read_long(&mut self, schema: &Schema) -> Result<i64, SerdeError> {
        self.0.get().read_long(schema)
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        self.0.get().read_float(schema)
    }

    fn read_double(&mut self, schema: &Schema) -> Result<f64, SerdeError> {
        self.0.get().read_double(schema)
    }

    fn read_big_integer(&mut self, schema: &Schema) -> Result<BigInteger, SerdeError> {
        self.0.get().read_big_integer(schema)
    }

    fn read_big_decimal(&mut self, schema: &Schema) -> Result<BigDecimal, SerdeError> {
        self.0.get().read_big_decimal(schema)
    }

    fn read_string(&mut self, schema: &Schema) -> Result<String, SerdeError> {
        self.0.get().read_string(schema)
    }

    fn read_blob(&mut self, schema: &Schema) -> Result<Blob, SerdeError> {
        self.0.get().read_blob(schema)
    }

    /// The one rewrite on the way in: a member whose schema reports no timestamp format
    /// is read with this model's own `date-time` format instead of the protocol default.
    /// A member that declares a format keeps it.
    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        match schema.timestamp_format() {
            Some(_) => self.0.get().read_timestamp(schema),
            None => self.0.get().read_timestamp(&DATE_TIME_TIMESTAMP),
        }
    }

    fn read_document(&mut self, schema: &Schema) -> Result<Document, SerdeError> {
        self.0.get().read_document(schema)
    }

    fn is_null(&self) -> bool {
        self.0.peek().is_null()
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        self.0.get().read_null()
    }

    fn container_size(&self) -> Option<usize> {
        self.0.peek().container_size()
    }
}

// =================================================================================
// Response side
// =================================================================================

/// One output shape, serialized with every format-less timestamp rendered as date-time
/// text.
struct DateTimes<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for DateTimes<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DateTimeWriter { inner: s })
    }
}

/// The serializer an output is walked with: every call is the codec's own, except a
/// timestamp leaf, which is written as its date-time text. A nested structure, list or
/// map re-enters the wrapper, because the codec walks each level with its own serializer
/// and a timestamp is rarely the output's own member.
struct DateTimeWriter<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for DateTimeWriter<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &DateTimes(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_list(schema, &|inner| write_elements(&mut DateTimeWriter { inner }))
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_map(schema, &|inner| write_entries(&mut DateTimeWriter { inner }))
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

    /// The one rewrite on the way out: the member's own schema, carrying the date-time
    /// text of its instant. A member that declares a format, or an instant the formatter
    /// cannot express, falls through to the codec's own rendering, so a member is never
    /// dropped.
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        if schema.timestamp_format().is_some() {
            return self.inner.write_timestamp(schema, value);
        }
        match date_time_text(value) {
            Some(text) => self.inner.write_string(schema, &text),
            None => self.inner.write_timestamp(schema, value),
        }
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}

// =================================================================================
// The protocol
// =================================================================================

/// This service's wire protocol: `restJson1` exactly as the model declares it, with every
/// timestamp in the `date-time` form the model's one timestamp shape declares.
#[derive(Debug)]
pub struct DateTimeRestJson {
    inner: RestJsonServerProtocol,
}

impl DateTimeRestJson {
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

impl Default for DateTimeRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for DateTimeRestJson {
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
        let inner = self.inner.deserialize_input(request, input_schema)?;
        Ok(Box::new(DateTimeReader(inner)))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_output(
            &DateTimes(output),
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
        // Wrapped for the same reason the success body is: the seam is uniform, so a
        // timestamp member an error shape of this model gains later cannot silently go
        // out in the protocol's default form. The wrapper is a no-op for a shape that
        // holds no timestamp, which is every error shape this model declares today.
        self.inner
            .serialize_error(&DateTimes(error), error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner.serialize_event_stream_message(
            event_type,
            is_exception,
            &DateTimes(event),
            event_schema,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::date_time_text;
    use aws_smithy_types::DateTime;

    /// RULE: an instant is reported as an RFC 3339 UTC date-time string. The rows span
    /// what the rendering generalises over: the epoch itself, a whole second, an instant
    /// carrying sub-second digits, and one before the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time() {
        let rows: [(f64, &str); 4] = [
            (0.0, "1970-01-01T00:00:00Z"),
            (1_704_067_200.0, "2024-01-01T00:00:00Z"),
            (1_704_067_200.25, "2024-01-01T00:00:00.25Z"),
            (-1.0, "1969-12-31T23:59:59Z"),
        ];
        for (seconds, expected) in rows {
            let rendered = date_time_text(&DateTime::from_secs_f64(seconds));
            assert_eq!(
                rendered.as_deref(),
                Some(expected),
                "case {seconds}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
