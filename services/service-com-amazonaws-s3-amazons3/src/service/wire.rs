// SPDX-License-Identifier: CC0-1.0

//! How this service's `restXml` responses serialize an UNWRAPPED output document.
//!
//! One S3 operation answers a document with no envelope at all: `GetBucketLocation`
//! sends `<LocationConstraint>us-west-2</LocationConstraint>` as the whole body, the
//! single output member promoted to the response root. The model says so twice — the
//! output shape carries `@xmlName("LocationConstraint")` and the operation carries
//! `aws.customizations#s3UnwrappedXmlOutput`
//! (`smithy_query(query="operation",operation="GetBucketLocation",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
//! reports `s3UnwrappedXmlOutput: true`) — and every SDK reads the Region off that
//! root's own text: botocore's `parse_get_bucket_location` takes `root.text` and reports
//! `None` for anything nested.
//!
//! The framework's generic restXml serializer opens the output shape's element and then
//! writes each member inside it, which for this one operation produces the member nested
//! under an identically-named root — a document whose root carries no text, so the Region
//! is lost. [`UnwrappedXmlRestXml`] is the protocol `service.rs` hands the dispatch
//! sequence through `Service::override_protocol`: every operation but the unwrapped ones
//! is the framework's own restXml protocol, delegated to unchanged, and an operation named
//! by [`UNWRAPPED_XML_OUTPUT_OPERATIONS`] has its single member written AS the root.
//!
//! This is the one place that rule lives, so no handler and no shape declaration mentions
//! the envelope at all.

use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::{xml_escape, RestXmlServerProtocol};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The `xmlns` every S3 response document carries, from the model's own
/// `@xmlNamespace` (`smithy_query(query="service_info")` reports
/// `xmlNamespace.uri`). The framework stamps it on an enveloped response's root; an
/// unwrapped one gets it here, on the root this module writes.
pub const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Whether this service's model declares `restXml`'s `noErrorWrapping`
/// (`smithy_query(query="service_info")` reports `protocol_config.restXml.noErrorWrapping:
/// true`), which is what makes an S3 error document a bare `<Error>` root. The generated
/// protocol is built with the same flag; this protocol replaces it, so it must carry it
/// too or every error would go out in an envelope no S3 client reads.
const NO_ERROR_WRAPPING: bool = true;

/// The operations whose output document carries NO envelope: the single output member's
/// element is the response root.
///
/// Source: the operations this service's model marks
/// `aws.customizations#s3UnwrappedXmlOutput`, re-derived from the model by
/// `tests/all/bucket_location.rs` so a model update that marks another operation cannot
/// quietly keep the envelope.
pub const UNWRAPPED_XML_OUTPUT_OPERATIONS: &[&str] = &["GetBucketLocation"];

/// This service's wire protocol: `restXml` exactly as the model declares it, with an
/// unwrapped output document for the operations the model marks as carrying one.
///
/// Everything but those outputs is the framework's own restXml protocol, delegated to
/// unchanged — request deserialization and the bare `<Error>` envelope included.
#[derive(Debug)]
pub struct UnwrappedXmlRestXml {
    inner: RestXmlServerProtocol,
}

impl UnwrappedXmlRestXml {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table is
    /// the model's); an author-overridden protocol drives only the modeled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    pub fn new() -> Self {
        Self {
            inner: RestXmlServerProtocol::new(&[])
                .with_namespace(S3_XML_NAMESPACE)
                .with_no_error_wrapping(NO_ERROR_WRAPPING),
        }
    }
}

impl Default for UnwrappedXmlRestXml {
    fn default() -> Self {
        Self::new()
    }
}

/// The XML element an output shape occupies: its `@xmlName` when the model renames it
/// (`GetBucketLocationOutput` is `LocationConstraint`), else the shape's own name — the
/// same resolution the framework's XML codec performs.
fn root_element_name(schema: &Schema) -> &str {
    match schema.xml_name() {
        Some(name) => name.value(),
        None => schema.shape_id().shape_name(),
    }
}

/// The ONE string an unwrapped output's single member carries, or `None` for an output
/// that wrote no string at all (an absent member — `us-east-1`'s null location
/// constraint) or wrote anything else.
///
/// An unwrapped output document is a root element holding text, so the only member shape
/// it can express is one string. Every other write is recorded as "not a shape this
/// document can carry" and makes the caller fall back to the enveloped form rather than
/// emit a document that drops a value.
#[derive(Default)]
struct SingleString {
    value: Option<String>,
    unsupported: bool,
}

impl SingleString {
    /// Note a write this document cannot carry.
    fn refuse(&mut self) -> Result<(), SerdeError> {
        self.unsupported = true;
        Ok(())
    }
}

impl ShapeSerializer for SingleString {
    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if self.value.is_some() {
            // A second member: more than a root's text, so not an unwrapped document.
            return self.refuse();
        }
        self.value = Some(value.to_string());
        Ok(())
    }

    fn write_struct(
        &mut self,
        _schema: &Schema,
        _value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_list(
        &mut self,
        _schema: &Schema,
        _write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        _write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_boolean(&mut self, _schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_byte(&mut self, _schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_short(&mut self, _schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_integer(&mut self, _schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_long(&mut self, _schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_float(&mut self, _schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_double(&mut self, _schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        _value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        _value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_blob(&mut self, _schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_timestamp(&mut self, _schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // An absent member writes nothing on the wire either way.
        Ok(())
    }
}

/// The unwrapped document for `output`, or `None` when its members are not one string —
/// in which case the caller serializes it in the framework's enveloped form rather than
/// inventing a document for it.
///
/// An absent member (`us-east-1`'s null location constraint) is the empty root, which is
/// exactly what AWS sends and what every SDK reads back as "no constraint".
fn unwrapped_document(
    output: &dyn SerializableStruct,
    output_schema: &Schema,
) -> Result<Option<String>, SerdeError> {
    let mut members = SingleString::default();
    output.serialize_members(&mut members)?;
    if members.unsupported {
        return Ok(None);
    }
    let root = root_element_name(output_schema);
    Ok(Some(match members.value {
        Some(value) => format!(
            "<{root} xmlns=\"{S3_XML_NAMESPACE}\">{}</{root}>",
            xml_escape(&value)
        ),
        None => format!("<{root} xmlns=\"{S3_XML_NAMESPACE}\"/>"),
    }))
}

impl ServerProtocol for UnwrappedXmlRestXml {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        // Routing is the generated entry's protocol's job (see `new`); this answers
        // through the same code path, which with no routes reports the request as naming
        // no operation rather than guessing one.
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
        if UNWRAPPED_XML_OUTPUT_OPERATIONS.contains(&operation_name) {
            if let Some(body) =
                unwrapped_document(output, output_schema).map_err(ProtocolError::Serde)?
            {
                let status = StatusCode::try_from(success_status).map_err(|_| {
                    ProtocolError::Malformed(format!(
                        "{operation_name} declares the unusable success status {success_status}"
                    ))
                })?;
                let mut response = Response::new(status, SdkBody::from(body));
                response
                    .headers_mut()
                    .insert("content-type", "application/xml");
                return Ok(response);
            }
        }
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
