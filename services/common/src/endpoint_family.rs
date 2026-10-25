// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for a FAMILY of emulated services that share one SigV4
//! signing name while each serves its own AWS endpoint.
//!
//! [`crate::claims_request`] narrows a shared-name claim by the first URI path segment,
//! which separates peers whose modelled path spaces are DISJOINT. A family whose spaces
//! OVERLAP needs more: the six Amazon Chime SDK services all declare `aws.auth#sigv4`
//! name `chime`, and `/app-instances` is bound by two of them, `/meetings` by two,
//! `/tags` by five. On the first segment alone every one of those requests has two or
//! more claimants, and the router — which refuses rather than guesses — answers
//! `500 AmbiguousService`, an error code no AWS model carries, to all of it.
//!
//! [`claims_request_in_endpoint_family`] is that rule, stated once for a whole family
//! and naming no service: the endpoint the request addresses decides when its host names
//! one (AWS's own discriminator), and otherwise the request must address an operation
//! the service's OWN model declares, or a first path segment no sibling publishes.
//!
//! It is the claim rule at full strength, so reach for it whenever the colliding services
//! serve distinct AWS endpoints: it adds the tier [`crate::bedrock`] and [`crate::lex`]
//! lack — the endpoint prefix the request's Host names decides outright — and falls back
//! to the modelled-route and unshared-first-segment tests those two use. Both of them are
//! to be MIGRATED onto it; they are left as they are because this change does not own
//! those families' issues, and each is covered by its own suite under
//! `core/router/tests/all/`.
//!
//! # Why no owner for an identically declared route
//!
//! [`crate::lex`] and [`crate::bedrock`] hand a segment their sharers declare IDENTICALLY
//! to one named OWNER, so the refusal never reaches the client. This rule deliberately
//! does NOT, and the difference is in what the sharers are. Those two families are API
//! VERSIONS or tiers of one product: the owner holds the same resources as the member it
//! shadows (and there the shadowed member is `@deprecated`), so answering from the owner
//! answers about the right resource. An endpoint family is six DIFFERENT services with
//! DISJOINT resources: `GET /tags` is declared identically by five Chime models, and the
//! only thing that says which service's resource a tagging request is about is the ARN it
//! carries — in the query string for the read and in the BODY for `TagResource` /
//! `UntagResource`, which a claim predicate reading [`http::request::Parts`] cannot see.
//! Naming an owner would therefore answer four services' tagging traffic out of a store
//! that does not hold the resource, which is worse than refusing: it would report "no such
//! resource" for resources that exist. ([`crate::shared_path`] settles exactly this case
//! where the ARN is an `@httpLabel` IN THE PATH, as the Lambda and IVS families bind it;
//! these five bind no such route.) So the family rule narrows the claim to one member
//! wherever anything on the wire can, leaves the identically declared route to the
//! router's refusal, and recovers even that case as soon as the request names an
//! endpoint host — which is what
//! `core/router/tests/all/chime_signing_name_routing.rs` pins. All five services'
//! tagging operations are scaffolded 501s today, so nothing implemented is reached either
//! way.

use emulator_server::{service_from_authority, service_from_host};
use smithy_server_runtime::ErasedServiceDefinition;

use crate::{addresses_modeled_route, nth_path_segment, RPCV2_CBOR_PATH_SEGMENT};

/// One FAMILY of emulated services that share a SigV4 signing name while each serves
/// its own AWS endpoint — the two lists the family's claim rule
/// ([`claims_request_in_endpoint_family`]) reads.
///
/// Declared ONCE per family, so every member reads the same lists instead of a copy of
/// its own that can drift into zero or two claimants for a space they share.
#[derive(Debug, Clone, Copy)]
pub struct EndpointFamily<'a> {
    /// Each member's `aws.api#service` `endpointPrefix` — the per-service key AWS itself
    /// tells the family apart by, since it is the host a client is pointed at.
    pub endpoint_prefixes: &'a [&'a str],
    /// The first URI path segments MORE THAN ONE member's model declares. A
    /// segment-level claim on one of those has several claimants, so inside this set the
    /// claim is the whole modelled route instead.
    pub shared_path_segments: &'a [&'a str],
}

/// The Amazon Chime SDK family: six emulated services whose models all declare
/// `aws.auth#sigv4` name `chime` while each carries its own `endpointPrefix`.
///
/// Source: `smithy_query(query="service_info")` on each of the six models in this
/// workspace — Chime -> `chime`, Chime SDK Identity -> `identity-chime`, Chime SDK
/// Messaging -> `messaging-chime`, Chime SDK Meetings -> `meetings-chime`, Chime SDK
/// Voice -> `voice-chime`, Chime SDK Media Pipelines -> `media-pipelines-chime` (all six
/// also report `sigv4Name: "chime"`).
///
/// `shared_path_segments` is every first path segment more than one of those models
/// binds, from `smithy_query(query="digest")` on the same six: `tags` (five of them bind
/// `/tags`), `app-instances` (Identity's `/app-instances…` and Messaging's
/// `/app-instances/{AppInstanceArn}/streaming-configurations`), `meetings` (Chime's
/// `/meetings/{MeetingId}/dial-outs` beside every Chime SDK Meetings operation) and the
/// telephony space Chime and Voice both publish (`phone-numbers`,
/// `phone-number-orders`, `phone-number-countries`, `settings`, `search`).
///
/// `core/router/tests/all/chime_signing_name_routing.rs` pins both lists against the six
/// crates' generated `ROUTE_PATH_SEGMENTS`, so neither can drift away from the models.
pub const CHIME_FAMILY: EndpointFamily<'static> = EndpointFamily {
    endpoint_prefixes: &[
        "chime",
        "identity-chime",
        "media-pipelines-chime",
        "meetings-chime",
        "messaging-chime",
        "voice-chime",
    ],
    shared_path_segments: &[
        "app-instances",
        "meetings",
        "phone-number-countries",
        "phone-number-orders",
        "phone-numbers",
        "search",
        "settings",
        "tags",
    ],
};

/// Whether a member of `family` — the service with this `endpoint_prefix`, this model
/// `definition` and these generated `path_segments` — claims `parts`.
///
/// The rule is the one AWS itself applies, in three tiers, and names no service:
///
/// 1. The request must address this family: its resolved routing key is the signing name
///    every member shares, or one member's endpoint prefix.
/// 2. If the request's HOST names a member's endpoint prefix, that member serves it and
///    the others do not. This is AWS's own discriminator — the endpoint a client is
///    pointed at — and it is read AHEAD of the shared signing name, which identifies
///    nothing on its own.
/// 3. Otherwise the host carries no endpoint (a client pointed at the emulator's own
///    endpoint, which is every recorded request), so the request is placed by what it
///    addresses: an operation THIS model declares — method, URI pattern and the query
///    literals the pattern pins, via [`addresses_modeled_route`] — or a first path
///    segment this model declares and no sibling does.
///
/// A route two members' models declare IDENTICALLY (`GET /tags` across the Chime family)
/// is claimed by both: nothing on the wire distinguishes them once the host does not, so
/// the router's refusal is the honest answer and this rule does not invent an owner — see
/// this module's header for why an endpoint family differs from [`crate::lex`] and
/// [`crate::bedrock`] here.
pub fn claims_request_in_endpoint_family(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
    endpoint_prefix: &str,
    path_segments: &[&str],
    family: &EndpointFamily<'_>,
) -> bool {
    if !addresses_endpoint_family(parts, definition.identity().signing_name(), family) {
        return false;
    }
    if let Some(addressed) = family_endpoint_addressed(parts, family) {
        return addressed.eq_ignore_ascii_case(endpoint_prefix.trim());
    }
    addresses_modeled_route(parts, definition)
        || claims_unshared_path(parts, path_segments, family.shared_path_segments)
}

/// Whether `parts` is addressed to `family` at all: the routing key the framework
/// resolves is the signing name its members share, or one of their endpoint prefixes
/// (which is what the key is for a host-addressed, unsigned request).
///
/// Both halves are the crate's own key reader ([`crate::routes_to`] /
/// [`crate::routes_to_any`]), so "the key names this endpoint" keeps one meaning —
/// trimmed and case-insensitive — across every claim rule here.
fn addresses_endpoint_family(
    parts: &http::request::Parts,
    signing_name: &str,
    family: &EndpointFamily<'_>,
) -> bool {
    crate::routes_to(parts, signing_name) || crate::routes_to_any(parts, family.endpoint_prefixes)
}

/// The `family` endpoint prefix the request's HOST names, or `None` when the host names
/// no member of the family — a loopback or IP endpoint, or any other host, which is what
/// a client pointed at the emulator sends.
///
/// Read through the framework's own host readers (`emulator_server::service_from_host`,
/// and `service_from_authority` for the HTTP/2 `:authority`, which `http` parses onto the
/// URI), so the "first label of the host, minus userinfo and port, never a bare loopback
/// or IP literal" rule stays in one place.
///
/// The label is compared to a prefix EXACTLY (case-insensitively), so an AWS variant host
/// — a FIPS or dualstack endpoint, whose first label carries a suffix
/// (`<prefix>-fips.<region>.amazonaws.com`) — names no member here and the claim falls
/// through to tier 3, which still places every route a member's model binds. So this tier
/// is "the host names a member outright", not "the host is an AWS endpoint of one".
fn family_endpoint_addressed<'f>(
    parts: &http::request::Parts,
    family: &EndpointFamily<'f>,
) -> Option<&'f str> {
    let host = service_from_host(&parts.headers).or_else(|| service_from_authority(&parts.uri))?;
    family
        .endpoint_prefixes
        .iter()
        .copied()
        .find(|prefix| host.eq_ignore_ascii_case(prefix))
}

/// Whether `parts`' first path segment is one `path_segments` declares and `shared` does
/// NOT — a segment of this model's own path space that no sibling under the shared
/// signing name publishes.
///
/// This is what keeps a request on a service's OWN space reaching that service even when
/// it names no modelled route (an unmodelled method, a missing `@required` header), so it
/// is answered in that service's own protocol envelope rather than as an unknown service.
///
/// The segment is read through [`nth_path_segment`], so the leading-slash rule stays in
/// one place; a ROOT path (`POST /`) reads as the empty segment, which no modelled
/// literal equals, and the `rpcv2Cbor` sentinel is excluded because it is the path space
/// every peer of such a service posts to rather than one model's own.
///
/// Two cases the framework's own [`crate::claims_modeled_path`] keeps are deliberately
/// NOT kept here, because this tier is the WEAKEST of the three and widening it hands a
/// sibling's traffic back to a member:
///
/// * an EMPTY `path_segments` list, which that helper reads as "no path constraint": here
///   it claims nothing, so a member whose model binds no URI claims only what the tiers
///   above place; and
/// * `emulator_server::ROOT_PATH_SEGMENT`, how a service that must answer `POST /` claims
///   the root path. Every member of an endpoint family is REST today (all six Chime
///   models are `restJson1`) and tier 3's [`addresses_modeled_route`] already places a
///   root post for any model that binds one, so nothing is lost; a family that later
///   mounts an RPC member — every one of which posts to `/` — needs that member placed by
///   its protocol target instead, as [`crate::claims_request`]'s fourth argument does.
fn claims_unshared_path(
    parts: &http::request::Parts,
    path_segments: &[&str],
    shared: &[&str],
) -> bool {
    match nth_path_segment(parts.uri.path(), 0) {
        Some(segment) => {
            !segment.is_empty()
                && segment != RPCV2_CBOR_PATH_SEGMENT
                && path_segments.contains(&segment)
                && !shared.contains(&segment)
        }
        None => false,
    }
}
