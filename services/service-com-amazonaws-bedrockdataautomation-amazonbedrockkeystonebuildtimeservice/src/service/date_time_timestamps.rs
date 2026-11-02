// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` responses carry a TIMESTAMP.
//!
//! RULE: a timestamp member goes onto the wire in the format its target timestamp
//! shape declares. This model declares exactly ONE timestamp shape,
//! `com.amazonaws.bedrockdataautomation#DateTimestamp`, and it carries
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="DateTimestamp",
//! model_path="services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json")`),
//! so EVERY timestamp this service answers with — a project's `creationTime` and
//! `lastModifiedTime`, a blueprint's `creationTime`, `lastModifiedTime` and
//! `optimizationTime`, a library's and an ingestion job's `creationTime` and
//! `completionTime`, and the `creationTime` of every summary a listing serves — is an
//! ISO-8601 / RFC-3339 date-time STRING such as `"2026-10-30T15:43:01.513Z"`, never
//! the epoch-seconds number the protocol defaults to. The rule is the protocol's, not
//! any one shape's.
//!
//! WHY IT HAS TO BE APPLIED HERE. The JSON codec resolves a timestamp's wire form from
//! the MEMBER schema alone (`vendor/smithy-schema-server/src/json.rs`'s
//! `write_timestamp` reads `Schema::timestamp_format()`) and falls back to the
//! protocol default of epoch seconds when the member schema reports no format. The
//! schema built from the model carries no member-level format at all (grep
//! `vendor/smithy-macros`: it emits no `with_timestamp_format`), so every timestamp of
//! this service went onto the wire as a bare number — not even the modelled JSON type,
//! so an SDK bound to this model fails to parse the member it is told is a date-time.
//!
//! The structural fix belongs in that codegen, which a service crate cannot edit
//! (`vendor/` is read-only), and the alternative on the service side is to hand-write
//! the schema and codec of every structure that holds a timestamp (seven of them here,
//! several of them deeply nested). So this module closes it once, at the protocol seam
//! `service.rs` already owns: the output is serialized with every timestamp written as
//! the date-time TEXT of its instant, and nothing else about the walk changes — the
//! member's wire name, its order and its schema stay the ones the model gave it, and
//! no other member kind is touched.
//!
//! Only the modelled SUCCESS body goes through it. No error shape of this model
//! carries a timestamp member, so an error document has nothing to re-render, and
//! request deserialization is untouched: no input member of this model is a timestamp
//! either (every list request filters on ARNs, stages and page tokens).

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The date-time text an instant is reported as: RFC-3339 in UTC, which is what
/// Smithy's `date-time` format is and what [`Format::DateTime`] renders.
///
/// An instant with no date-time rendering at all (one outside the range the formatter
/// can express) keeps the protocol's own number rather than being given a fake
/// string — see [`DateTimeSerializer::write_timestamp`].
fn date_time_text(value: &DateTime) -> Option<String> {
    value.fmt(Format::DateTime).ok()
}

/// One output shape, serialized with every timestamp rendered as date-time text.
struct DateTimeTimestamps<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for DateTimeTimestamps<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DateTimeSerializer { inner: s })
    }
}

/// The serializer the output is walked with: every call is the codec's own, except a
/// timestamp leaf, which is written as its date-time text. A nested structure, list or
/// map re-enters the wrapper, because the codec walks each level with its own
/// serializer and a timestamp is rarely the output's own member — a project's and a
/// blueprint's timestamps both sit one structure down, and a listing's sit inside a
/// list.
struct DateTimeSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for DateTimeSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &DateTimeTimestamps(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_list(schema, &|inner| write_elements(&mut DateTimeSerializer { inner }))
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_map(schema, &|inner| write_entries(&mut DateTimeSerializer { inner }))
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
    /// text of its instant. A member that already declares a format renders in THAT
    /// format — this seam supplies only the one the codegen dropped and can never
    /// contradict the model — and an instant the formatter cannot express falls through
    /// to the codec's own rendering, so a member is never dropped.
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

/// This service's wire protocol: `restJson1` exactly as the model declares it, with
/// every timestamp of a modelled response in the `date-time` form this model's one
/// timestamp shape declares.
#[derive(Debug)]
pub struct DateTimeRestJson {
    inner: RestJsonServerProtocol,
}

impl DateTimeRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of it
    /// and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated service definition's own protocol (whose table
    /// is the model's); an author-overridden protocol drives only the modelled
    /// sequence — `deserialize_input`, `serialize_output`, `serialize_error`.
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
        // No input member of this model is a timestamp, so the request side is the
        // framework's own.
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
            &DateTimeTimestamps(output),
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
        // No error shape of this model carries a timestamp member.
        self.inner
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        // No operation of this model is an event-streaming one — no member of any
        // output is marked `@streaming` over a union — so this path serves no event
        // shape and has no timestamp to re-render. The wrapper goes on anyway, so the
        // rule holds for the whole protocol rather than only for the two paths this
        // model happens to use today.
        self.inner.serialize_event_stream_message(
            event_type,
            is_exception,
            &DateTimeTimestamps(event),
            event_schema,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::date_time_text;
    use aws_smithy_types::DateTime;

    /// RULE: an instant is reported as an RFC-3339 UTC date-time string. The rows span
    /// what the rendering generalises over: the epoch itself, a whole second, an
    /// instant carrying sub-second digits, and one before the epoch.
    #[test]
    fn an_instant_renders_as_an_rfc3339_date_time() {
        let rows: [(f64, &str); 4] = [
            (0.0, "1970-01-01T00:00:00Z"),
            (1_793_374_981.0, "2026-10-30T15:43:01Z"),
            (1_793_374_981.5, "2026-10-30T15:43:01.5Z"),
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
