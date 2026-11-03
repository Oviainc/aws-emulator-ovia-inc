// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connectcampaigns-amazonconnectcampaignservice` service: how it claims requests and how it words the
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
//!   before any handler ran. It delegates to the shared AWS renderer
//!   ([`service_common::validation_message::aws_validation_refusal_withholding`]), so a
//!   constraint refusal of any operation of this service reads as AWS words it: the
//!   framework's envelope, member paths and requirements, with a `@pattern` quoted without
//!   its whole-value anchors and a clause that states a value only where that value is the
//!   COLLECTION the caller submitted. See the inline note on that method.
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
//! ## The media type this service announces
//!
//! The model declares the `restJson1` protocol trait, whose generic default response
//! media type is `application/json`, while every recorded response of this service —
//! success and refusal alike — announces `application/x-amz-json-1.1`: its front end
//! labels its JSON bodies with that media type whatever the modelled protocol is, and
//! the media type a client reads is what it keys its parsing and content negotiation
//! off. The rule, the one function that applies it and the operation-seam wrapper that
//! reaches the refusal tail with it all live in
//! [`service_common::response_media_type`]; [`RESPONSE_MEDIA_TYPE`] is this service's
//! own pair, and the seams it is applied at are `override_operation` (every routed
//! request, including the refusal the framework renders inside the request walk),
//! `override_routing_error` and `override_framework_error`.
//!
//! One response is announced by the framework alone, because no author seam reaches it:
//! the authentication refusal, which the router renders through
//! `definition().refusal_response(..)` before this service is asked anything.

use service_common::response_media_type::{ResponseMediaType, ResponseMediaTypeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::collections::HashSet;
use std::sync::Arc;

/// How this service renders the value of a LIST member whose `@length` a request broke:
/// the elements it submitted, bracketed, which is the value a list has. The EMPTY list is
/// the one such value a structural refusal determines on its own, because a refusal
/// carries a collection's element COUNT and no elements, and zero elements render as this
/// and nothing else.
const EMPTY_LIST_VALUE: &str = "[]";

/// Whether `violation` is the framework's clause about a LIST member the request EMPTIED —
/// a `@length` broken by a collection the walk rendered as zero items.
///
/// The element count is read back out of the framework's own rendering through
/// [`service_common::validation_message::list_item_count`], so this service and any other
/// that words a list's length refusal read one rendering the same way. A LIST only: a
/// map's own rendering counts ENTRIES, not items, so `list_item_count` does not read it and
/// an empty map keeps the valueless clause. That is deliberate — `[]` is the rendering of a
/// list, and nothing here invents the rendering of another collection kind.
fn empties_a_list(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) && violation.value.as_deref().is_some_and(|stored| {
        service_common::validation_message::list_item_count(stored) == Some(0)
    })
}

/// `refusal` stated the way this service states a constraint refusal, or `None` for a
/// refusal that is not one.
///
/// The envelope, the member paths and every requirement are the shared AWS renderer's
/// ([`service_common::validation_message::aws_validation_refusal_withholding`]), so a
/// constraint refusal of any operation of this service reads as AWS words it, with a
/// `@pattern` quoted without the anchors a whole-value pattern wraps it in.
///
/// What this service decides is the VALUE half of each clause, and it states that half
/// only where the value is a COLLECTION the caller submitted: a scalar member's value is
/// never echoed back — neither its own bytes nor the framework's elision of them — so the
/// clause names the member and the requirement and nothing else. A member that was ABSENT
/// keeps the framework's `Value null`, which is the one thing that half reports about a
/// value nobody sent, and a collection the refusal carries only a COUNT of has that count
/// dropped too, since a count is not a value a caller sent — except for the one such value
/// a structural refusal determines, the empty list, which is restated as the list it is
/// (see [`empties_a_list`]).
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let mut stated: Vec<Violation> = Vec::with_capacity(violations.len());
    // The member paths whose clause states no value at all: every one the framework wrote
    // a value half for that is not the collection the caller submitted.
    let mut withheld: HashSet<String> = HashSet::new();
    for violation in violations {
        if empties_a_list(violation) {
            stated.push(Violation {
                value: Some(EMPTY_LIST_VALUE.to_string()),
                ..violation.clone()
            });
            continue;
        }
        if violation.value.is_some() {
            withheld.insert(violation.path.clone());
        }
        stated.push(violation.clone());
    }
    service_common::validation_message::aws_validation_refusal_withholding(
        &Refusal::Invalid(stated),
        |path| withheld.contains(path),
    )
}

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which the framework's renderers stamp on every response — the one value
/// [`RESPONSE_MEDIA_TYPE`] replaces.
const REST_JSON_MEDIA_TYPE: &str = "application/json";

/// The pair this service's responses are re-announced across: the media type its
/// model's protocol trait implies, and the one its recorded wire announces (see the
/// module header). Named once, so every seam that re-announces reads the same two
/// values.
pub const RESPONSE_MEDIA_TYPE: ResponseMediaType = ResponseMediaType::new(
    REST_JSON_MEDIA_TYPE,
    service_common::response_media_type::AWS_JSON_1_1,
);

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality. That path conjunct is what separates two services sharing one
/// SigV4 signing name — ConnectCampaigns v1 and ConnectCampaignsV2 both sign as
/// `connect-campaigns`, so the key alone matches both and the router rejects every
/// such request as `500 AmbiguousService` instead of serving either one — and their
/// modelled path spaces are disjoint: v1 publishes `campaigns`, `campaigns-state`,
/// `campaigns-summary`, `connect-instance` and `tags` here, while every V2 route
/// lives under `v2` there.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-connectcampaigns-amazonconnectcampaignservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper every routable request is served through, so a refusal raised
    /// inside the request walk announces this service's media type too. Held in a
    /// field, built once, because [`Service::override_operation`] hands back a borrow
    /// of it.
    media_type: ResponseMediaTypeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            media_type: ResponseMediaTypeOperation::new(RESPONSE_MEDIA_TYPE),
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]:
        // this service's signing name (`connect-campaigns`) is shared with the
        // ConnectCampaignsV2 API, so the claim is the route key AND the
        // model-derived first URI path segment (v1's unprefixed route space).
        // Matching on the signing name alone would have both services claim every
        // `connect-campaigns`-signed request, which the router answers with 500
        // `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the generated routing table can serve is served through the
        // shared media-type wrapper, which delegates to that same table entry and then
        // re-announces the media type of what it answered — reaching the refusal the
        // framework renders inside the request walk (see the module header). The
        // routing stays the model's: a request whose routing key names no modelled
        // operation is declined here, so the framework answers it as the unroutable
        // request it is — through `override_routing_error` below, which re-announces
        // that answer too.
        service_common::response_media_type::routes(&self.entry, req)
            .then_some(&self.media_type as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can
        // state it the way the service it emulates states it, which no model records.
        //
        // The whole wording is delegated to [`stated_refusal`], which delegates in turn to
        // the shared AWS renderer: the envelope, the member paths and every requirement
        // stay the framework's, and the renderings AWS does differently are applied to
        // every constrained member of every operation of this service alike — a
        // `@pattern` requirement quoted without the anchors a whole-value pattern wraps it
        // in, and a clause that states a value only where that value is the COLLECTION the
        // caller submitted. A refusal that is not a constraint violation — an
        // unauthenticated caller — keeps the framework's code and wording.
        stated_refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated routing
        // table resolved it. The framework's protocol-correct default for this
        // service's envelope is kept word for word, status, code and body alike; only
        // the media type is re-announced, so an unroutable request is answered under
        // the same media type as a routed one (see the module header).
        Some(RESPONSE_MEDIA_TYPE.announce(self.entry.routing_error_response(req)))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. The entry's protocol-correct rendering is kept word for
        // word; only the media type is re-announced (see the module header).
        //
        // The error is borrowed and `ProtocolError` is neither `Clone` nor
        // reconstructable in general, so the default renderer is reached by rebuilding
        // the one variant the router ever raises here — `Malformed`, from the
        // `aws-chunked` decode it runs before naming an operation. `ProtocolError` is
        // an open enum this crate does not own, so the remaining arm declines to
        // `None`, keeping the framework's own rendering of an error this service cannot
        // rebuild rather than losing what that error said.
        match error {
            aws_smithy_schema_server::ProtocolError::Malformed(message) => {
                let rebuilt = aws_smithy_schema_server::ProtocolError::Malformed(message.clone());
                // Rendered through the framework's own renderer for this service's
                // protocol envelope rather than through the entry: this seam is handed
                // the error alone, and the entry's renderer resolves the envelope from
                // the REQUEST it is given. The model declares restJson1, and the media
                // type this service announces is re-stamped on the rendering anyway.
                Some(RESPONSE_MEDIA_TYPE.announce(
                    smithy_server_runtime::protocol_error_response(
                        smithy_server_runtime::PreDispatchProtocol::Json {
                            content_type: REST_JSON_MEDIA_TYPE,
                            errortype_header: true,
                        },
                        rebuilt,
                    ),
                ))
            }
            _ => None,
        }
    }
}
