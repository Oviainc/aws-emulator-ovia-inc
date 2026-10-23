// SPDX-License-Identifier: CC0-1.0

//! The object headers a STORED map asset is delivered with.
//!
//! Two of this service's operations hand back a body they did not compute:
//! `GetGlyphs` serves a glyph range and `GetSprites` a sprite file, each an
//! object of the versioned, encrypted asset bucket the Maps endpoint keeps its
//! static content in. Such a response carries the object-store delivery headers
//! beside the modeled ones — `x-amz-version-id` for the object version it came
//! from and `x-amz-server-side-encryption` for how that object is stored — which
//! is what the recorded responses for those assets show, and what a cache or
//! client that keys on an asset's version reads.
//!
//! What identifies such a response is its media type: a stored object is
//! delivered with the asset bucket's own type (the `content_type` of the
//! asset-delivery record in [`crate::state`]), while everything this service
//! COMPUTES from the request — a style descriptor, a tile, a static map —
//! answers with the media type of the document it built and carries no object
//! headers.
//!
//! The model cannot say this: `GetGlyphsResponse` and `GetSpritesResponse` bind
//! only `Content-Type`, `Cache-Control` and `ETag`
//! (`smithy_query(query="operation",operation="GetGlyphs,GetSprites",
//! model_path="services/service-com-amazonaws-geomaps-mapsservice/smithy-model/com-amazonaws-geomaps-mapsservice.json")`),
//! so no output member can carry either header and no handler can emit one.
//! [`AssetDeliveryRestJson`] is the protocol `service.rs` hands the dispatch
//! sequence through `Service::override_protocol`: it is the model's own
//! `restJson1` codec, delegated to unchanged for every operation, with the two
//! object headers added to a response that delivers a stored object.
//!
//! Neither header is invented per response: the encryption algorithm is the
//! asset-delivery record in [`crate::state`], and the version id is derived from
//! the object's own bytes, so it is stable across repeat reads of one asset and
//! changes exactly when the asset's content does.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::{insert_response_headers, RestJsonServerProtocol};
use sha2::{Digest as _, Sha256};

/// The operations that serve a stored object of the asset bucket.
///
/// `GetGlyphs` serves a glyph range and `GetSprites` a sprite file. Whether a
/// given response really is a stored object is decided by its media type (see
/// [`AssetDeliveryRestJson::with_object_headers`]), so an operation that builds
/// its own document cannot pick up the object headers.
pub const OBJECT_DELIVERED_OPERATIONS: &[&str] = &["GetGlyphs", "GetSprites"];

/// The response header naming the object version an asset was read from.
pub const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// The response header naming how the asset bucket encrypts its objects.
pub const SERVER_SIDE_ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";

/// The domain separator mixed into an object's version id, so the id is not the
/// same digest as the `ETag` computed over the same bytes.
const VERSION_ID_DOMAIN: &[u8] = b"geo-maps/asset-version-id\0";

/// How many bytes of the digest a version id carries. 16 bytes is the same
/// 128-bit identifier width the `ETag` uses.
const VERSION_ID_BYTES: usize = 16;

/// This service's wire protocol: `restJson1` exactly as the model declares it,
/// with the object-store delivery headers on the operations that serve a stored
/// asset.
pub struct AssetDeliveryRestJson<Eng> {
    inner: RestJsonServerProtocol,
    /// The shared engine, for the asset-delivery record in this service's
    /// state: the encryption algorithm reported is catalog data, not a constant
    /// of this module.
    engine: Arc<Eng>,
}

impl<Eng> AssetDeliveryRestJson<Eng> {
    /// Build the protocol once, at service construction: `override_protocol`
    /// borrows it from `&self` and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a
    /// request's operation through the generated `ServiceDefinition`'s own
    /// protocol (whose table is the model's); an author-overridden protocol
    /// drives only the modeled sequence — `deserialize_input`,
    /// `serialize_output`, `serialize_error` — none of which consults routes.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
            engine,
        }
    }
}

impl<Eng: Send + Sync + 'static> AssetDeliveryRestJson<Eng> {
    /// `response` with the delivery headers of the object it carries, when it
    /// carries one.
    ///
    /// A stored object is the response whose media type is the asset bucket's:
    /// the encryption algorithm and that media type both come from the catalog's
    /// asset-delivery record, and the version id is derived from the bytes being
    /// served. A response the service computed itself is left exactly as the
    /// model describes it, and so is every response when the engine carries no
    /// maps catalog (unreachable in this emulator: the router builds one engine
    /// and hands it to every operation) rather than inventing delivery data.
    fn with_object_headers(&self, mut response: Response) -> Result<Response, ProtocolError> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Ok(response);
        };
        let delivery = state.lock().asset_delivery.clone();
        if response.headers().get("content-type") != Some(delivery.content_type.as_str()) {
            return Ok(response);
        }
        let Some(version_id) = object_version_id(&response) else {
            return Ok(response);
        };
        insert_response_headers(
            &mut response,
            [
                (VERSION_ID_HEADER.to_string(), version_id),
                (
                    SERVER_SIDE_ENCRYPTION_HEADER.to_string(),
                    delivery.server_side_encryption,
                ),
            ],
        )?;
        Ok(response)
    }
}

/// The version id of the object a response carries: a digest of the bytes
/// served, so one asset reads back under the same id every time and any change
/// to its content is a new version.
///
/// Derived from the response body, and from the content-addressed `ETag` when
/// the body is not in memory (no asset this service serves is streamed, so the
/// fallback is a safeguard rather than a path). A response with neither gets no
/// version id, because there is nothing about the object to identify it by.
fn object_version_id(response: &Response) -> Option<String> {
    let etag = response.headers().get("etag");
    let content: &[u8] = match response.body().bytes() {
        Some(bytes) => bytes,
        None => etag?.as_bytes(),
    };
    let mut digest = Sha256::new();
    digest.update(VERSION_ID_DOMAIN);
    digest.update(content);
    Some(hex::encode(&digest.finalize()[..VERSION_ID_BYTES]))
}

impl<Eng> std::fmt::Debug for AssetDeliveryRestJson<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The engine is not `Debug` and is not this protocol's configuration;
        // what identifies the protocol is the codec it delegates to.
        f.debug_struct("AssetDeliveryRestJson")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<Eng: Send + Sync + 'static> ServerProtocol for AssetDeliveryRestJson<Eng> {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        // Routing is the generated entry's protocol's job (see `new`); this
        // answers through the same code path, which with no routes reports the
        // request as naming no operation rather than guessing one.
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
        if OBJECT_DELIVERED_OPERATIONS.contains(&operation_name) {
            return self.with_object_headers(response);
        }
        Ok(response)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // A refused request served no object, so it carries no object headers.
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
