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
//!
//! ## The media type this service announces
//!
//! The model declares the `restJson1` protocol trait, whose generic default response
//! media type is `application/json`, and that is what every recorded response of this
//! service to a request CARRYING A BODY announces, while a response to a request that
//! declared no media type of its own — a `GET` or a `DELETE` with no body — is recorded
//! announcing `application/x-amz-json-1.1`, on a success and on a refusal alike. The rule
//! this service's front door follows is therefore: a response announces the media type
//! the REQUEST declared for its own body, and a request that declared none is answered
//! under this front door's own default. The media type a client reads is what it keys its
//! parsing off, so the default is announced rather than left at the protocol-implied
//! value.
//!
//! The pair, and the one function that re-announces across it, are
//! [`service_common::response_media_type`]'s; [`RESPONSE_MEDIA_TYPE`] is this service's
//! own pair and [`announced`] applies it under the condition above. The seams it is
//! applied at are `override_operation` (every routed request, including the responses the
//! framework renders inside the request walk), the collection-listing wire routes this
//! file serves itself, and `override_routing_error`.
//!
//! Two responses keep the protocol-implied value because no seam that reaches them is
//! handed the request whose media type decides it: the transport-level failure
//! (`override_framework_error`) and the authentication refusal, which the router renders
//! before this service is asked anything.

use service_common::response_media_type::ResponseMediaType;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};

pub mod wire;

/// Unit cases for the media type this service announces (see the module header); the
/// module is test-only, so nothing it holds is part of the served surface.
#[cfg(test)]
mod response_media_type;

/// This service's endpoint prefix, as its model gives it
/// (`smithy_query(query="service_info")`: `awsService.endpointPrefix` `email`) — the name
/// a host-addressed request resolves to, beside the SigV4 signing name (`ses`) the
/// generated identity carries.
pub const ENDPOINT_PREFIX: &str = "email";

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which the framework's renderers stamp on every response — the one value
/// [`RESPONSE_MEDIA_TYPE`] replaces.
const REST_JSON_MEDIA_TYPE: &str = "application/json";

/// The pair this service's responses are re-announced across: the media type its model's
/// protocol trait implies, which the framework's renderers stamp, and this front door's
/// own default (see the module header). Named once, so every seam that re-announces reads
/// the same two values.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType = ResponseMediaType::new(
    REST_JSON_MEDIA_TYPE,
    service_common::response_media_type::AWS_JSON_1_1,
);

/// The request header a caller declares its own body's media type in.
const CONTENT_TYPE: &str = "content-type";

/// `resp` as this service announces it for `req`: left exactly as the framework rendered
/// it when the request declared a media type of its own, and re-announced under this
/// front door's default when it declared none (see the module header).
///
/// Only the one header is ever touched, and only where it still carries the
/// protocol-implied value — [`ResponseMediaType::announce`] leaves a body whose own
/// producer chose another media type alone.
fn announced(req: &RtRequest, resp: RtResponse) -> RtResponse {
    if declares_a_media_type(req) {
        resp
    } else {
        RESPONSE_MEDIA_TYPE.announce(resp)
    }
}

/// Whether `req` declared a media type for its own body.
///
/// A request carrying a body declares one; a `GET` or a `DELETE` that carries none sends
/// the header empty or not at all, and nothing it said decides the media type of the
/// answer.
fn declares_a_media_type(req: &RtRequest) -> bool {
    req.headers()
        .get(CONTENT_TYPE)
        .is_some_and(|declared| !declared.trim().is_empty())
}

/// Serves the modelled operation this service's routing table names for the request,
/// exactly as that table would, then announces the media type this service announces on
/// what it answered.
///
/// Wrapping the whole walk is what reaches the responses the framework renders INSIDE it —
/// the refusal of a value outside a modelled bound — which `Service::override_protocol`
/// deliberately does not. It holds no operation of its own: the one it delegates to is the
/// one the routing table names for the request, resolved from the `svc` the framework
/// passes in, so every status, code, message and body byte stays the framework's and this
/// wrapper owns one header.
#[derive(Debug, Default)]
struct AnnouncedMediaTypeRoute;

impl ErasedOperation for AnnouncedMediaTypeRoute {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let modelled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            let answered = match modelled {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable through an `override_operation` that claims a request only
                // when the routing table holds its key; answered as the framework answers
                // an unroutable request rather than guessed at.
                None => definition.routing_error_response(req),
            };
            announced(req, answered)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

/// The method every AWS SDK, and the CLI, sends a paged listing of a collection on.
///
/// The checked-in model binds such a listing to a POST on a route of its own (`POST
/// /v2/email/list-identities`, `POST /v2/email/list-configuration-sets`); the wire form
/// the service answers is a GET over the collection itself, with the paging members in
/// the query string. Both are served: the modelled route through the generated routing
/// table, this one through [`SdkCollectionListingRoute`].
const SDK_LISTING_METHOD: &str = "GET";

/// The paging members a listing's GET form carries in the query string under these
/// names and its modelled route carries in the request body.
///
/// `Filter` is not among them: the GET form has no query binding for a map, so a
/// filtered listing is the modelled route's own.
const LISTING_QUERY_MEMBERS: [&str; 2] = ["NextToken", "PageSize"];

/// Serves the GET-over-the-collection wire form of a paged listing — the form real
/// clients send — by running the modelled operation against the same request restated
/// on the route the model binds.
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
#[derive(Debug)]
struct SdkCollectionListingRoute {
    /// The key the generated routing table holds the modelled operation under.
    operation: &'static str,
    /// The path segments of the collection the GET form addresses, as
    /// [`request_segments`] splits a request's own.
    segments: &'static [&'static str],
    /// The URI of the modelled route the adapted request is dispatched on.
    modelled_uri: &'static str,
}

impl SdkCollectionListingRoute {
    /// Whether `req` is the GET form this route serves.
    fn claims(&self, req: &RtRequest) -> bool {
        req.method() == SDK_LISTING_METHOD && request_segments(req.uri()) == self.segments
    }

    /// The same request on the route the model binds: a POST to
    /// [`Self::modelled_uri`] whose body carries the paging members the GET form sent
    /// in its query string.
    ///
    /// `None` when the request cannot be restated — its target did not parse, or the
    /// runtime API rejected the route — which the caller answers as the framework's own
    /// "named no modelled operation" rather than serving the operation a request it
    /// never received.
    fn on_modelled_route(&self, req: &RtRequest) -> Option<RtRequest> {
        let query = req
            .uri()
            .parse::<http::Uri>()
            .ok()?
            .query()
            .unwrap_or_default()
            .to_string();
        let mut members = serde_json::Map::new();
        for member in LISTING_QUERY_MEMBERS {
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
        out.set_uri(self.modelled_uri).ok()?;
        out.set_method("POST").ok()?;
        for (name, value) in req.headers().iter() {
            // The body is this method's own, so the headers describing the one the
            // client sent are not carried over; everything else is kept, as the
            // router's own request rebuild keeps it.
            if name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("content-type")
            {
                continue;
            }
            out.headers_mut().append(name.to_string(), value.to_string());
        }
        out.headers_mut().append("content-type", "application/json");
        Some(out)
    }
}

impl ErasedOperation for SdkCollectionListingRoute {
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
                .get(self.operation)
                .and_then(|operation| Some((operation, self.on_modelled_route(req)?)));
            let answered = match routed {
                Some((operation, adapted)) => operation.run(svc, &adapted, ctx).await,
                // The request could not be restated, so nothing modelled answers it
                // after all; report it exactly as the framework would have.
                None => svc.definition().routing_error_response(req),
            };
            // Announced from the request the CLIENT sent, not from the restated one this
            // route dispatches: the media type of the answer is decided by what the caller
            // declared (see the module header).
            announced(req, answered)
        })
    }
}

/// Every listing this service serves on the GET-over-the-collection form as well as on
/// the route its model binds.
///
/// One row per operation whose modelled route the model puts beside the collection
/// rather than on it: `ListEmailIdentities` (`POST /v2/email/list-identities`, the GET
/// form over `/v2/email/identities`) and `ListConfigurationSets` (`POST
/// /v2/email/list-configuration-sets`, the GET form over `/v2/email/configuration-sets`).
const SDK_COLLECTION_LISTINGS: [SdkCollectionListingRoute; 2] = [
    SdkCollectionListingRoute {
        operation: "ListEmailIdentities",
        segments: &["v2", "email", "identities"],
        modelled_uri: "/v2/email/list-identities",
    },
    SdkCollectionListingRoute {
        operation: "ListConfigurationSets",
        segments: &["v2", "email", "configuration-sets"],
        modelled_uri: "/v2/email/list-configuration-sets",
    },
];

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
    /// Serves each listing of [`SDK_COLLECTION_LISTINGS`] on the method and URI real
    /// clients send it on; [`Service::override_operation`] hands such a request to the
    /// row that claims it.
    sdk_collection_listings: [SdkCollectionListingRoute; 2],
    /// The wrapper every routable request is served through, so the media type this
    /// service announces is on the responses the framework renders inside the request
    /// walk too. Held in a field because [`Service::override_operation`] hands back a
    /// borrow of it.
    media_type: AnnouncedMediaTypeRoute,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: wire::NullMessageRestJson::new(),
            sdk_collection_listings: SDK_COLLECTION_LISTINGS,
            media_type: AnnouncedMediaTypeRoute,
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
        // Every request the model's own routing table resolves is served through the
        // shared media-type wrapper, which delegates to that same table entry and then
        // announces the media type of what it answered — reaching the responses the
        // framework renders inside the request walk (see the module header). The routing
        // stays the model's.
        if service_common::response_media_type::routes(&self.entry, req) {
            return Some(&self.media_type);
        }
        // What it cannot: the listings of `SDK_COLLECTION_LISTINGS`. The checked-in
        // model binds each of them to a POST on a route beside the collection, while
        // the wire form the service answers — the one every AWS SDK and the CLI send,
        // and the only one a paginator can follow — is a GET over the collection with
        // the paging members in the query string. The route table resolves such a
        // request to no operation at all, so it is claimed here and served by the
        // modelled operation itself.
        self.sdk_collection_listings
            .iter()
            .find(|listing| listing.claims(req))
            .map(|listing| listing as &dyn ErasedOperation)
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
        // routing table resolved it. The framework's protocol-correct default for this
        // service's envelope is kept word for word, status, code and body alike; only the
        // media type is announced the way this service announces one (see the module
        // header).
        Some(announced(req, self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The seam is handed the failure alone, and the media type of
        // an answer is decided by what the REQUEST declared, so there is nothing to
        // announce from here: the entry's protocol-correct rendering is kept (see the
        // module header). The error is borrowed, so the default renderer can still consume
        // it on `None`.
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
