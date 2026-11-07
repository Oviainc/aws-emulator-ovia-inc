// SPDX-License-Identifier: CC0-1.0

//! WHERE this service's own routes are served: the endpoint a control call arrives on,
//! and not an address this service announced for a DATA plane.
//!
//! `GetDataEndpoint` exists because a stream's media APIs are not served at the
//! control-plane endpoint — "Gets an endpoint for a specified stream for either reading or
//! writing" — and the separation holds in both directions: the address announced for a
//! plane serves that plane's media routes, and the control operations are served at the
//! endpoint the discovery call itself was made to. Kinesis Video Media states the same
//! rule from the data side ("Caller used wrong endpoint to write data to a stream"), and a
//! public data host carries no control route at all.
//!
//! So a control-plane call that reached an address this store announced for a plane
//! ([`super::super::state::published_plane`]) reached an endpoint that binds no operation
//! for its path, and is answered as such: the framework's own refusal of an unrouted path
//! — HTTP 404 with the bare `UnknownOperationException` element as the whole body — with
//! no operation run, no member reported and nothing in the store touched. Every other
//! address — the endpoint the client is already talking to, this service's published
//! regional host — keeps serving the whole control surface.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// Whether this request reached an address this store announced for a data plane.
///
/// Only the addresses this emulator PUBLISHED are read: it knows the plane of those and of
/// no others, so a request that reached a host naming a service — a caller resolving AWS's
/// own hostnames — is served the control surface as before.
pub fn addressed_a_data_plane<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let host = req.headers().get(::http::header::HOST.as_str());
    let Some(address) =
        ::service_common::data_plane_addressing::reached_emulator_endpoint(req.uri(), host)
    else {
        return false;
    };
    crate::state::published_plane(engine, &address).is_some()
}

/// Answers a control-plane call that reached a data plane's address: the refusal an
/// endpoint gives a path it binds no operation for.
///
/// Served as an operation rather than decided in `can_handle`, because the answer is this
/// endpoint's own and not the router's "no service claimed it": the request did reach this
/// emulator, at an address that serves another plane's routes.
#[derive(Debug, Clone, Copy)]
pub struct UnroutedOnDataPlaneAddress;

impl ErasedOperation for UnroutedOnDataPlaneAddress {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { unrouted_address_response() })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The MODELLED operation the routing key names decides which schemes sign a
        // request, and the router resolves that from the routing table rather than from
        // the operation this service handed back, so nothing here weakens them.
        None
    }
}

/// HTTP 404 with the bare `UnknownOperationException` element as the whole body and no
/// error-type header: the path is bound to no operation here, so no operation was
/// identified to name one for, and there is no member and no value to report.
///
/// The body is the framework's own ([`::smithy_server_runtime::UNRECOGNISED_PROTOCOL_BODY`]),
/// which is the one wording this emulator states an unrouted path in — the same answer
/// the ingest plane gives a `/putMedia` that reached an address that does not route it.
fn unrouted_address_response() -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(404).expect("the status is a constant of this module"),
        SdkBody::from(::smithy_server_runtime::UNRECOGNISED_PROTOCOL_BODY),
    )
}
