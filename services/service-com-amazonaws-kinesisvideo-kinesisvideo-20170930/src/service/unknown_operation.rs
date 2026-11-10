// SPDX-License-Identifier: CC0-1.0

//! What the `kinesisvideo` FRONT DOOR answers a request it can name no operation for.
//!
//! An endpoint of this family rejects a request whose method and URI it binds no operation
//! for AHEAD of any operation handler, in the front door's own envelope rather than in a
//! service's modelled error vocabulary: HTTP 400 with the body
//!
//! ```text
//! {"Output":{"__type":"<the front door's unknown-operation shape>",
//!            "Message":"Unable to determine operation name"},"Version":"1.0"}
//! ```
//!
//! Nothing in it comes from the request or from this store: no operation was identified,
//! so there is no name, path, member or value to report, and this error class interpolates
//! none — the whole body is the template. The shape it names belongs to the front door and
//! not to any Kinesis Video model, which is why no model declares it and why no
//! `x-amzn-errortype` names a shape of this service's.
//!
//! # Why ONE wording serves every address
//!
//! This emulator answers the whole family at the endpoint a client is talking to, and the
//! addresses `GetDataEndpoint` announces for a stream's planes are the SAME front door on
//! ports of their own ([`super::super::state::data_plane_serving`]). So each of these is
//! one endpoint answering a path it binds no operation for, and each is answered alike:
//!
//! - a request this service claimed that names no modelled operation of it
//!   ([`super::EmulatedService::override_routing_error`]);
//! - a control call that reached an address announced for a data plane
//!   ([`super::data_plane_address`]);
//! - a media call that reached an address that does not serve ITS plane — the
//!   control-plane front door the discovery call was made to, or the other plane's
//!   address.
//!
//! The reply is served both as a plain response (the routing-error seam, which answers
//! one) and as the [`ErasedOperation`] below (the `override_operation` seam, which names
//! one), because a path bound to no operation here can be recognised in either seam
//! depending on whether the generated routing table could name a MODELLED operation for
//! it. Both go through [`unknown_operation_response`], so the two cannot drift.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The status the front door rejects an unroutable request with: the request named no
/// operation, which is a fault in the request rather than a missing resource.
pub const UNKNOWN_OPERATION_STATUS: u16 = 400;

/// The shape the front door names for it, in the namespace the front door itself declares
/// — no Kinesis Video Smithy model binds this shape, so it is stated here rather than
/// serialized through an error schema.
pub const UNKNOWN_OPERATION_TYPE: &str =
    "com.amazon.kinesis.video.v20170930#UnknownOperationException";

/// The one sentence that class carries. It states no operation, path or value, because the
/// front door identified none.
pub const UNKNOWN_OPERATION_MESSAGE: &str = "Unable to determine operation name";

/// The version of the front door's own envelope, which wraps the fault in `Output`.
const ENVELOPE_VERSION: &str = "1.0";

/// The media type the envelope is written in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The envelope's bytes, built through the JSON writer rather than written out as a
/// literal, so the one wording above is the only place its text lives.
#[must_use]
pub fn unknown_operation_body() -> String {
    ::serde_json::json!({
        "Output": {
            "__type": UNKNOWN_OPERATION_TYPE,
            "Message": UNKNOWN_OPERATION_MESSAGE,
        },
        "Version": ENVELOPE_VERSION,
    })
    .to_string()
}

/// The front door's answer to a request it can name no operation for.
///
/// No `x-amzn-errortype` header: no operation was identified, so no shape of a service's
/// own is named, and the fault's type travels in the envelope's `__type` where the front
/// door states it.
#[must_use]
pub fn unknown_operation_response() -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(UNKNOWN_OPERATION_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(unknown_operation_body()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// The front door's answer, as the one [`ErasedOperation`] a service hands back from
/// `override_operation` for a request whose path this endpoint binds no operation for.
///
/// Stateless, and reads nothing from the request: nothing it answers depends on anything
/// the request carries, and nothing it is handed is touched — no store, no stream, no
/// media.
#[derive(Debug, Clone, Copy)]
pub struct UnknownOperation;

impl ErasedOperation for UnknownOperation {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { unknown_operation_response() })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The MODELLED operation the routing key names decides which schemes sign a
        // request, and the router resolves that from the routing table rather than from
        // the operation a service handed back, so nothing here weakens them.
        None
    }
}
