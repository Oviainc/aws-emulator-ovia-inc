// SPDX-License-Identifier: CC0-1.0

//! What MEDIA TYPE this service declares on a successful answer, including the one whose
//! body is a package version's raw asset bytes.
//!
//! RULE: every successful answer this service puts on the wire declares the service's own
//! media type — `application/json` — for the WHOLE response, whatever the body's own bytes
//! are. CodeArtifact's control plane is a `restJson1` service and the asset download is one
//! of its operations, not a storage endpoint: the recorded `GetPackageVersionAsset`
//! response carries `Content-Type: application/json` beside the asset bytes, exactly as
//! every JSON-bodied answer of the same service does.
//!
//! The generic `restJson1` serializer cannot express that: it types the response off the
//! `@httpPayload` member's own target, so a blob payload goes out as
//! `application/octet-stream` (the Smithy default for an un-annotated streaming blob). That
//! default is right for a service that hands out opaque objects and wrong here, and which
//! media type a service announces is a property of THIS SERVICE rather than of one
//! operation — so it is stated once, for every modelled answer, instead of being patched
//! into the one handler whose output happens to be a payload today. Every other answer of
//! this service is a JSON document the renderer already stamps that way, so the one value
//! re-announced is the payload-implied one.
//!
//! [`JsonMediaTypeRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`]. The re-announcement
//! itself is the shared rule of [`::service_common::response_media_type`] — this service
//! states only its own pair, [`ASSET_PAYLOAD_MEDIA_TYPE`] -> [`ANNOUNCED_MEDIA_TYPE`], so a
//! response whose media type its producer chose deliberately is left exactly as written.
//! Everything else — routing, request deserialization, the JSON body itself, the bound
//! response headers and every modelled error document — is the framework's own `restJson1`
//! protocol, delegated to unchanged.
//!
//! A repository ENDPOINT's own traffic ([`super::npm_registry`]) does not pass through
//! here: it is served by an operation that builds its own response, so a package manager
//! still receives a tarball under the media type its client expects.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;

use ::service_common::response_media_type::ResponseMediaType;

/// The media type the `restJson1` renderer stamps on a response whose body is a streaming
/// blob `@httpPayload` — the Smithy default for an un-annotated blob, and the only value
/// this service re-announces.
const ASSET_PAYLOAD_MEDIA_TYPE: &str = "application/octet-stream";

/// The media type this service announces on its answers.
const ANNOUNCED_MEDIA_TYPE: &str = "application/json";

/// The pair this service's answers are re-announced across.
const MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(ASSET_PAYLOAD_MEDIA_TYPE, ANNOUNCED_MEDIA_TYPE);

/// This service's wire protocol: `restJson1` exactly as the model declares it, with every
/// successful answer declaring [`ANNOUNCED_MEDIA_TYPE`].
#[derive(Debug)]
pub struct JsonMediaTypeRestJson {
    inner: RestJsonServerProtocol,
}

impl JsonMediaTypeRestJson {
    /// Build the protocol once, at service construction: `override_protocol` hands back a
    /// borrow of it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated `ServiceDefinition`'s own protocol (whose table is the
    /// model's); an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error`.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for JsonMediaTypeRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for JsonMediaTypeRestJson {
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
        let response =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        // The body, the status and every bound header are the framework's; only the media
        // type the answer declares is this service's own, and only where the renderer
        // stamped the payload-implied one.
        Ok(MEDIA_TYPE.announce(response))
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
