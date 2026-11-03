// SPDX-License-Identifier: CC0-1.0

//! WHERE this service's lenient reading of a request body is applied: the protocol
//! seam, so it runs before the framework judges that body.
//!
//! The RULE itself — what each of JSON's scalar types states for a member of each
//! modelled scalar type, and the declared spelling of the one member this service
//! reads without regard to case — lives in [`super::lenient_scalars`], which is
//! where it is stated and tested. This module is the one place it is applied.
//!
//! WHY IT HAS TO BE APPLIED HERE. The JSON codec resolves a scalar from the one
//! token it finds and refuses every other kind outright
//! (`vendor/smithy-schema-server/src/json.rs`, whose
//! `read_boolean`/`read_long`/`read_string` accept only their own token), and an
//! enum's value set is judged by the framework's validating walk — both before any
//! handler runs. So a request the recorded service serves with HTTP 200 was
//! refused 400 with nothing created, and no handler could have known.
//!
//! The structural fix belongs in that codec, which a service crate cannot edit
//! (`vendor/` is read-only). So this module closes it once, at the protocol seam
//! `service.rs` already owns: [`LenientRestJson`] is the model's own `restJson1`
//! with the input deserializer wrapped, the same way
//! `services/service-com-amazonaws-location-locationservice/src/service/date_time_timestamps.rs`
//! supplies the timestamp format that codegen drops. Keep the two in step.
//!
//! Nothing else about either walk changes: a member's wire name, its order, its
//! schema and every constraint the model puts on it stay exactly as the model gave
//! them, and a response is written exactly as before — a response carries each
//! member in its modelled type, which is what a client reads.
//!
//! The wrapper adds one stack frame per nesting level and nothing else: no buffer,
//! no copy of the document, no caller-sized allocation and no second pass. A scalar
//! leaf is read through the codec's own `read_document`, which reads the ONE token
//! that is there under the codec's own depth guard, so a value the member cannot
//! hold costs the bytes it already occupies in the body the transport capped and is
//! then refused. Nesting depth stays bounded by the codec itself, whose
//! `read_struct`/`read_list`/`read_map` refuse a body nested past their own maximum.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};

use super::lenient_scalars::{
    boolean_of, declared_spelling, double_of, integral_of, not_stated, reads_from_the_request,
    text_of,
};

/// The inner deserializer one [`LenientReader`] forwards to, however it is held.
///
/// The protocol seam hands back an owned `Box<dyn ShapeDeserializer>` while the
/// nested walks hand each level a `&mut dyn ShapeDeserializer`, and both have to
/// wear the same wrapper — so the wrapper is generic over this, and the forwarding
/// body below is written once.
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
/// scalar leaf, which is read from whichever scalar the body carries there.
///
/// A nested structure, list or map re-enters the wrapper, because the codec walks
/// each level with its own deserializer and a scalar is rarely the input's own
/// member.
struct LenientReader<D>(D);

impl<D: InnerDeserializer> LenientReader<D> {
    /// The one token that is there, read through the codec itself — or `None` for a
    /// member whose value does not come from the JSON body at all.
    ///
    /// A member bound to the URI or a header is read by the binding deserializer
    /// from TEXT, so there is no other scalar type it could have been sent in and
    /// nothing for the rules below to restate: such a member is left entirely to the
    /// codec ([`reads_from_the_request`]).
    ///
    /// The member's own schema goes with the read, so a `document` member — which
    /// holds any JSON the caller likes — is untouched by every rule below: its value
    /// is read by [`ShapeDeserializer::read_document`] and never reaches one.
    fn token(&mut self, schema: &Schema) -> Result<Option<Document>, SerdeError> {
        if reads_from_the_request(schema) {
            return Ok(None);
        }
        self.0.get().read_document(schema).map(Some)
    }
}

impl<D: InnerDeserializer> ShapeDeserializer for LenientReader<D> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_struct(schema, &mut |member, d| consumer(member, &mut LenientReader(d)))
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_list(schema, &mut |d| consumer(&mut LenientReader(d)))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0
            .get()
            .read_map(schema, &mut |key, d| consumer(key, &mut LenientReader(d)))
    }

    fn read_boolean(&mut self, schema: &Schema) -> Result<bool, SerdeError> {
        let Some(document) = self.token(schema)? else {
            return self.0.get().read_boolean(schema);
        };
        boolean_of(&document).ok_or_else(|| not_stated(&document, "a boolean"))
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        let held = self.read_long(schema)?;
        i8::try_from(held)
            .map_err(|_| SerdeError::custom(format!("number {held} does not fit in i8")))
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        let held = self.read_long(schema)?;
        i16::try_from(held)
            .map_err(|_| SerdeError::custom(format!("number {held} does not fit in i16")))
    }

    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        let held = self.read_long(schema)?;
        i32::try_from(held)
            .map_err(|_| SerdeError::custom(format!("number {held} does not fit in i32")))
    }

    fn read_long(&mut self, schema: &Schema) -> Result<i64, SerdeError> {
        let Some(document) = self.token(schema)? else {
            return self.0.get().read_long(schema);
        };
        integral_of(&document).ok_or_else(|| not_stated(&document, "a whole number"))
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        Ok(self.read_double(schema)? as f32)
    }

    fn read_double(&mut self, schema: &Schema) -> Result<f64, SerdeError> {
        let Some(document) = self.token(schema)? else {
            return self.0.get().read_double(schema);
        };
        double_of(&document).ok_or_else(|| not_stated(&document, "a number"))
    }

    fn read_big_integer(&mut self, schema: &Schema) -> Result<BigInteger, SerdeError> {
        // The codec already reads these from a number OR a string, which is the
        // same leniency, so they are left to it.
        self.0.get().read_big_integer(schema)
    }

    fn read_big_decimal(&mut self, schema: &Schema) -> Result<BigDecimal, SerdeError> {
        self.0.get().read_big_decimal(schema)
    }

    /// The text of whichever scalar is there, and for the member this service reads
    /// without regard to case the spelling the model declares — so the value the
    /// walk judges, the value the handler reads and the value the resource reports
    /// back are one.
    fn read_string(&mut self, schema: &Schema) -> Result<String, SerdeError> {
        let text = match self.token(schema)? {
            Some(document) => {
                text_of(&document).ok_or_else(|| not_stated(&document, "a string"))?
            }
            None => self.0.get().read_string(schema)?,
        };
        match declared_spelling(schema, &text) {
            Some(spelling) => Ok(spelling.to_string()),
            None => Ok(text),
        }
    }

    fn read_blob(&mut self, schema: &Schema) -> Result<Blob, SerdeError> {
        // A blob travels as base64 TEXT, so a number in its place states no blob:
        // the codec's own reading is the whole rule.
        self.0.get().read_blob(schema)
    }

    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        // The codec already reads an instant from a number OR a string, per the
        // member's own `@timestampFormat`.
        self.0.get().read_timestamp(schema)
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

/// This service's wire protocol: `restJson1` exactly as the model declares it,
/// reading a request body the way this service reads one (see the module header).
#[derive(Debug)]
pub struct LenientRestJson {
    inner: RestJsonServerProtocol,
}

impl LenientRestJson {
    /// Build the protocol once, at service construction:
    /// [`smithy_server_runtime::Service::override_protocol`] hands back a borrow of
    /// it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose
    /// table is the model's); an author-overridden protocol drives only the modelled
    /// sequence — `deserialize_input`, `serialize_output`, `serialize_error`.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for LenientRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for LenientRestJson {
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
        Ok(Box::new(LenientReader(inner)))
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
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}
