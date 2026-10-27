// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sesv2-simpleemailservice-v2` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! `override_protocol` is the sixth seam, and this service takes it for one thing the
//! generic renderer cannot express: an error document whose `message` member is present
//! and `null` when the code carries no message. See [`wire`].

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};

pub mod wire;

/// This service's endpoint prefix, as its model gives it
/// (`smithy_query(query="service_info")`: `awsService.endpointPrefix` `email`) — the name
/// a host-addressed request resolves to, beside the SigV4 signing name (`ses`) the
/// generated identity carries.
pub const ENDPOINT_PREFIX: &str = "email";

/// The routing key the generated table holds `ListEmailIdentities` under.
const LIST_EMAIL_IDENTITIES: &str = "ListEmailIdentities";

/// The method and URI every AWS SDK, and the CLI, sends `ListEmailIdentities` on.
///
/// The checked-in model binds the operation to `POST /v2/email/list-identities`; the
/// wire form the service answers is a GET over the identity collection, with the
/// paging members in the query string. Both are served: the modelled route through
/// the generated routing table, this one through [`SdkListIdentitiesRoute`].
const SDK_LIST_IDENTITIES_METHOD: &str = "GET";

/// The path segments of that URI, as [`request_path`] splits a request's own.
const SDK_LIST_IDENTITIES_SEGMENTS: [&str; 3] = ["v2", "email", "identities"];

/// The modelled route the adapted request is dispatched on.
const MODELLED_LIST_IDENTITIES_URI: &str = "/v2/email/list-identities";

/// `ListEmailIdentities`' paging members, which the GET form carries in the query
/// string under these names and the modelled route carries in the request body.
///
/// `Filter` is not among them: the GET form has no query binding for a map, so a
/// filtered listing is the modelled route's own.
const LIST_IDENTITIES_QUERY_MEMBERS: [&str; 2] = ["NextToken", "PageSize"];

/// Serves `GET /v2/email/identities` — the wire form of `ListEmailIdentities` real
/// clients send — by running the modelled operation against the same request
/// restated on the route the model binds.
///
/// The paging members are the only difference between the two forms: the GET carries
/// `NextToken` and `PageSize` in the query string, the modelled route carries them in
/// the body, so the adaptation is to move them and to leave everything else alone.
/// Deserialization, validation and the handler itself are untouched — the handler is
/// the one the modelled route resolves to.
///
/// Only the DISPATCHED request is restated; the [`OperationContext`] still carries the
/// parts the client sent, so a handler reading the URI off `ctx` sees the original
/// target.
#[derive(Debug, Default)]
struct SdkListIdentitiesRoute;

impl ErasedOperation for SdkListIdentitiesRoute {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let routed = svc
                .definition()
                .routing_table()
                .get(LIST_EMAIL_IDENTITIES)
                .and_then(|operation| Some((operation, on_modelled_route(req)?)));
            match routed {
                Some((operation, adapted)) => operation.run(svc, &adapted, ctx).await,
                // The request could not be restated, so nothing modelled answers it
                // after all; report it exactly as the framework would have.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The same `ListEmailIdentities` request on the route the model binds: a POST to
/// [`MODELLED_LIST_IDENTITIES_URI`] whose body carries the paging members the GET
/// form sent in its query string.
///
/// `None` when the request cannot be restated — its target did not parse, or the
/// runtime API rejected the route — which the caller answers as the framework's own
/// "named no modelled operation" rather than serving the operation a request it never
/// received.
fn on_modelled_route(req: &RtRequest) -> Option<RtRequest> {
    let query = req
        .uri()
        .parse::<http::Uri>()
        .ok()?
        .query()
        .unwrap_or_default()
        .to_string();
    let mut members = serde_json::Map::new();
    for member in LIST_IDENTITIES_QUERY_MEMBERS {
        if let Some(value) = query_value(&query, member) {
            // `PageSize` is modelled as an integer and `NextToken` as a string: a
            // query string carries both as text, so a value that reads as a whole
            // number is restated as one and anything else is restated verbatim, for
            // the deserializer to accept or refuse exactly as it would on the
            // modelled route.
            let restated = match value.parse::<i64>() {
                Ok(number) => serde_json::Value::from(number),
                Err(_) => serde_json::Value::String(value),
            };
            members.insert(member.to_string(), restated);
        }
    }
    let body = serde_json::Value::Object(members).to_string();

    let mut out = RtRequest::new(aws_smithy_types::body::SdkBody::from(body));
    out.set_uri(MODELLED_LIST_IDENTITIES_URI).ok()?;
    out.set_method("POST").ok()?;
    for (name, value) in req.headers().iter() {
        // The body is this function's own, so the headers describing the one the
        // client sent are not carried over; everything else is kept, as the router's
        // own request rebuild keeps it.
        if name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        out.headers_mut().append(name.to_string(), value.to_string());
    }
    out.headers_mut().append("content-type", "application/json");
    Some(out)
}

/// The value of the query parameter `name`, percent-decoded, or `None` when the query
/// string does not carry it.
///
/// Decoded with the framework's own query decoder, so a value an SDK percent-encoded
/// reads here exactly as it was sent.
fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (smithy_server_runtime::percent_decode(key) == name)
                .then(|| smithy_server_runtime::percent_decode(value))
        })
}

/// The path component of a request URI, split into its non-empty segments — the same
/// view of the request the REST route table matches on. An unparsable URI has no
/// segments, which claims nothing.
fn request_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The `com-amazonaws-sesv2-simpleemailservice-v2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol: the model's own `restJson1`, with the error
    /// document of [`wire`]. Built once here because [`Service::override_protocol`]
    /// hands back a borrow of it.
    protocol: wire::NullMessageRestJson,
    /// Serves `ListEmailIdentities` on the method and URI real clients send it on;
    /// [`Service::override_operation`] hands such a request to it.
    sdk_list_identities_route: SdkListIdentitiesRoute,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: wire::NullMessageRestJson::new(),
            sdk_list_identities_route: SdkListIdentitiesRoute,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `ses` — a signing name it SHARES with three other
        // emulated services: SES v1 (`awsQuery`, endpoint `email`), Pinpoint Email
        // (`restJson1`, endpoint `email`) and MailManager (`awsJson1_0` + `rpcv2Cbor`,
        // endpoint `mail-manager`). The router resolves a request's routing key from the
        // SigV4 credential scope first, so all four see the SAME key, and claiming on the
        // signing name alone makes all four claim every request — which the router
        // answers `500 AmbiguousService` for, a code no AWS model carries and an SDK
        // retries to its ceiling instead of surfacing.
        //
        // So the claim is narrowed the way the framework narrows every other
        // signing-name collision: the routing key must name this endpoint (its signing
        // name, or its own endpoint prefix for a host-addressed request) AND the request
        // must address this service's own modelled URI path space, which for SESv2 is the
        // `/v2/…` space its 116 operations bind (the generated
        // `crate::ROUTE_PATH_SEGMENTS`). Pinpoint Email's `/v1/…`, SES v1's awsQuery
        // `POST /` and MailManager's RPC form are then each claimed by their own service.
        service_common::routes_to_any(
            parts,
            &[self.entry.identity().signing_name(), ENDPOINT_PREFIX],
        ) && service_common::claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The model's own routing table resolves every route the model binds.
        if self.entry.protocol().routing_key(req).is_ok() {
            return None;
        }
        // What it cannot: `ListEmailIdentities`. The checked-in model binds it to
        // `POST /v2/email/list-identities`, while the wire form the service answers —
        // the one every AWS SDK and the CLI send, and the only one a paginator can
        // follow — is a GET over the identity collection with the paging members in
        // the query string. The route table resolves that request to no operation at
        // all, so it is claimed here and served by the modelled operation itself.
        if req.method() == SDK_LIST_IDENTITIES_METHOD
            && request_segments(req.uri()) == SDK_LIST_IDENTITIES_SEGMENTS
        {
            return Some(&self.sdk_list_identities_route);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // SES API v2 states a request it cannot read as `BadRequestException`
        // ("The input you provided is invalid") — the model binds that error to
        // every operation as a service-common error, and it is the code an SES
        // client branches on for a malformed or incomplete request. The framework's
        // own structured wording is kept: it names the member and the rule it
        // broke, which is what the caller has to fix. The one difference SES makes
        // to it — a clause about a value's FORM (its enum value set or its pattern)
        // states the member and the requirement without the value that violated
        // them — is single-homed in `crate::state`, so a refusal made at the
        // boundary and one a handler makes itself read alike.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: crate::state::boundary_refusal_message(violations)
                    .unwrap_or_else(|| "The input you provided is invalid.".to_string()),
            }),
            // An unauthenticated caller is the emulator's own 403, not a modelled
            // SES error: leave the framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the ONE difference the generic
        // renderer cannot express: a modelled exception's `message` member is present
        // in the error document even when the code carries no message, as JSON `null`
        // rather than dropped (see `wire`). Borrowed from the field built in `new`,
        // never constructed per request.
        //
        // This seam drives the MODELLED sequence only. A framework-level refusal (a
        // `BadRequestException` for a value outside the model's bounds) still renders
        // in the generated entry's envelope, which is correct: that message is built
        // from the request and is never absent.
        Some(&self.protocol)
    }
}
