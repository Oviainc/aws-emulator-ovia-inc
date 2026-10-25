// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for the Amazon Chime service FAMILY: six emulated services
//! that share ONE SigV4 signing name while serving six different endpoints.
//!
//! `aws.auth#sigv4$name` is `chime` for every one of them, while each carries its own
//! `aws.api#service$endpointPrefix` (source: `smithy_query(query="service_info")` on each
//! of the six models — `chime`, `identity-chime`, `media-pipelines-chime`,
//! `meetings-chime`, `messaging-chime`, `voice-chime`). The signing name is therefore
//! NOT a service identity here, and a service that claims on it alone claims all five
//! siblings' traffic: every request has six claimants and the router — which refuses
//! rather than guesses — answers `500 AmbiguousService` for all of it.
//!
//! So the claim is keyed on what DOES identify one service, in the order the request
//! carries it:
//!
//! 1. the ENDPOINT the request addresses — the `Host` header's (or the HTTP/2
//!    `:authority`'s) first label, when that label is one of the family's endpoint
//!    prefixes. That is the one signal that names a single family member, and it is read
//!    HERE rather than through `emulator_server::resolve_route_key`, which prefers the
//!    SigV4 credential scope and so answers `chime` for all six;
//! 2. otherwise the request's own URI, matched against the first path segments THIS
//!    service's model binds ([`claims_chime_family_request`]'s `own_path_segments`,
//!    the generated `crate::ROUTE_PATH_SEGMENTS`), restricted to the segments no sibling
//!    publishes.
//!
//! The signing name is used only as the weak hint it is: a request must route to the
//! family (by the shared signing name or by a family endpoint prefix) before any member
//! considers it, so narrowing the claim here cannot make a chime service claim another
//! service's traffic.
//!
//! # The path spaces more than one member publishes
//!
//! Step 2 settles every request except one on a path space two or more of the six models
//! bind IDENTICALLY. Those segments are [`CHIME_FAMILY_SHARED_SEGMENTS`], and each is
//! settled by the deeper reader named below, passed by the claiming service as
//! `claims_shared_space`:
//!
//! * `/app-instances` — the Chime SDK Identity model binds the app-instance resource
//!   itself (`/app-instances`, `/app-instances/{AppInstanceArn}`, `…/admins`,
//!   `…/retention-settings`), while the Chime SDK Messaging model binds exactly one
//!   sub-space of it, `/app-instances/{AppInstanceArn}/streaming-configurations`. So the
//!   THIRD path segment decides: [`addresses_app_instance_streaming_configurations`].
//! * `/meetings` — the Chime SDK Meetings model owns the meeting resource
//!   (`/meetings`, `/meetings/{MeetingId}`, `…/attendees`, `…/transcription`), while the
//!   legacy `chime` model binds exactly one sub-space of it,
//!   `/meetings/{MeetingId}/dial-outs`: [`addresses_meeting_dial_outs`].
//! * the telephony space `/phone-numbers`, `/phone-number-orders`,
//!   `/phone-number-countries`, `/settings` and `/search`
//!   ([`CHIME_TELEPHONY_SHARED_SEGMENTS`]) — the legacy `chime` model and the Chime SDK
//!   Voice model bind the SAME URIs for the same operation names (the Voice endpoint was
//!   carved out of the legacy service keeping them), so nothing in the request's path
//!   tells the two apart. The route key does: `chime` IS the legacy endpoint's
//!   `endpointPrefix`, so a request that names no endpoint is the legacy endpoint's, and
//!   the Voice endpoint claims this space only when the request addresses
//!   `voice-chime`.
//! * `/tags` — bound identically (`GET /tags`, `POST /tags?operation=tag-resource`,
//!   `POST /tags?operation=untag-resource`) by five of the six models. What names the
//!   service there is the resource ARN, and it is NOT always readable: the list
//!   operation carries it in the `arn` query parameter, but tag and untag carry it in
//!   the request BODY, which the `Service::can_handle` seam is not handed. So the family
//!   resolves this space by endpoint prefix ONLY, uniformly for all three operations:
//!   no member claims a `/tags` request that names no family endpoint, and the router
//!   answers its terminal `404 UnknownService` rather than serving one of five claimants
//!   arbitrarily or resolving the list call while leaving tag and untag unroutable.
//!
//! A shared segment this list MISSES keeps the behaviour it has today — several
//! claimants, so the router's `AmbiguousService` — rather than being mis-routed: the
//! list only ever REMOVES claims.
//!
//! Two follow-ups this rule leaves open, named here so neither is implicit:
//!
//! * the `/tags` space is reachable only at a family endpoint host, so a client pointed
//!   at a bare emulator endpoint cannot reach `ListTagsForResource` / `TagResource` /
//!   `UntagResource` for any family member. Giving the space a default owner, or
//!   resolving the LIST call through [`crate::shared_path`] from its `arn` query
//!   parameter, would answer it from one service instead of the router's
//!   `404 UnknownService`;
//! * the `/app-instances` and `/meetings` splits are stated here as literal segment
//!   readers; [`crate::addresses_modeled_route`] derives the same answer from each
//!   model's own routing table and should replace them.

use http::request::Parts;

/// The SigV4 signing name all six Amazon Chime services share, which is why none of them
/// may claim on it alone. Source: `aws.auth#sigv4$name` on each of the six service
/// shapes (`smithy_query(query="service_info")`, `sigv4Name`).
///
/// It is also the legacy Amazon Chime endpoint's `endpointPrefix`, so it is one of
/// [`CHIME_FAMILY_ENDPOINT_PREFIXES`] as well — which is why a request that names no
/// endpoint resolves to that endpoint's key.
pub const CHIME_SIGNING_NAME: &str = "chime";

/// The `endpointPrefix` of every service in the family, sorted.
///
/// Source: `smithy_query(query="service_info")` on the six models'
/// `awsService.endpointPrefix` — `chime` (Chime, `UCBuzzConsoleService`),
/// `identity-chime` (Chime SDK Identity), `media-pipelines-chime` (Chime SDK Media
/// Pipelines), `meetings-chime` (Chime SDK Meetings), `messaging-chime` (Chime SDK
/// Messaging) and `voice-chime` (Chime SDK Voice).
pub const CHIME_FAMILY_ENDPOINT_PREFIXES: &[&str] = &[
    "chime",
    "identity-chime",
    "media-pipelines-chime",
    "meetings-chime",
    "messaging-chime",
    "voice-chime",
];

/// The first URI path segments MORE THAN ONE of the six models binds, so that the path
/// space alone cannot say which member a request belongs to.
///
/// Derived from the `smithy.api#http` URIs of all six models
/// (`smithy_query(query="digest")` on each): `app-instances` (Identity + Messaging),
/// `meetings` (legacy Chime + Meetings), the five telephony segments in
/// [`CHIME_TELEPHONY_SHARED_SEGMENTS`] (legacy Chime + Voice) and `tags` (Identity,
/// Messaging, Meetings, Media Pipelines and Voice).
pub const CHIME_FAMILY_SHARED_SEGMENTS: &[&str] = &[
    "app-instances",
    "meetings",
    "phone-number-countries",
    "phone-number-orders",
    "phone-numbers",
    "search",
    "settings",
    "tags",
];

/// The telephony path space the legacy `chime` endpoint and the `voice-chime` endpoint
/// bind identically, operation for operation.
///
/// Source: both models' `smithy.api#http` URIs — `GET /phone-numbers`,
/// `GET /phone-number-orders`, `GET /phone-number-countries`, `GET /settings` and
/// `GET /search?type=phone-numbers`, with the same operation names on both sides
/// (`ListPhoneNumbers`, `ListPhoneNumberOrders`, `ListSupportedPhoneNumberCountries`,
/// `GetGlobalSettings`, `SearchAvailablePhoneNumbers`, …).
pub const CHIME_TELEPHONY_SHARED_SEGMENTS: &[&str] = &[
    "phone-number-countries",
    "phone-number-orders",
    "phone-numbers",
    "search",
    "settings",
];

/// Whether `path` is in the `/app-instances` space at all.
#[must_use]
pub fn addresses_app_instance_space(path: &str) -> bool {
    crate::nth_path_segment(path, 0) == Some("app-instances")
}

/// Whether `path` addresses `/app-instances/{AppInstanceArn}/streaming-configurations` —
/// the ONLY `/app-instances` sub-space the Chime SDK Messaging model binds
/// (`GET`, `PUT` and `DELETE MessagingStreamingConfigurations`), and one the Chime SDK
/// Identity model does not bind at all.
///
/// Both sides of that collision read this ONE predicate — Messaging claims a path it
/// answers `true` for, Identity claims the complement — so the two claims cannot drift
/// into zero or two claimants for the space they share.
#[must_use]
pub fn addresses_app_instance_streaming_configurations(path: &str) -> bool {
    addresses_app_instance_space(path)
        && crate::nth_path_segment(path, 2) == Some("streaming-configurations")
}

/// Whether `path` is in the `/meetings` space at all.
#[must_use]
pub fn addresses_meeting_space(path: &str) -> bool {
    crate::nth_path_segment(path, 0) == Some("meetings")
}

/// Whether `path` addresses `/meetings/{MeetingId}/dial-outs` — the ONLY `/meetings`
/// sub-space the legacy `chime` model binds (`CreateMeetingDialOut`), and one the Chime
/// SDK Meetings model does not bind at all.
///
/// Read by both sides of that collision, for the reason
/// [`addresses_app_instance_streaming_configurations`] gives.
#[must_use]
pub fn addresses_meeting_dial_outs(path: &str) -> bool {
    addresses_meeting_space(path) && crate::nth_path_segment(path, 2) == Some("dial-outs")
}

/// Whether `path`'s first segment is one of [`CHIME_TELEPHONY_SHARED_SEGMENTS`].
#[must_use]
pub fn addresses_shared_telephony_space(path: &str) -> bool {
    crate::nth_path_segment(path, 0)
        .is_some_and(|segment| CHIME_TELEPHONY_SHARED_SEGMENTS.contains(&segment))
}

/// The family endpoint prefix the request's `Host` (or HTTP/2 `:authority`) names, or
/// `None` when it names none — a bare emulator endpoint (`localhost:9297`, an IP
/// literal) or another service's host.
///
/// Read through the framework's own host readers, so the host-parsing rule (strip
/// `user@` and `:port`, take the first label, reject loopback and IP literals, normalize)
/// stays in one place.
#[must_use]
pub fn chime_family_endpoint_addressed(parts: &Parts) -> Option<&'static str> {
    let label = emulator_server::service_from_host(&parts.headers)
        .or_else(|| emulator_server::service_from_authority(&parts.uri))?;
    CHIME_FAMILY_ENDPOINT_PREFIXES
        .iter()
        .copied()
        .find(|prefix| prefix.eq_ignore_ascii_case(&label))
}

/// Whether the request is addressed to the chime family at all: its resolved routing key
/// is the shared signing name, or one of the family's endpoint prefixes.
///
/// Both are read through `crate::routes_to_any`, the owner of that rule: the framework
/// resolves the routing key from the credential scope first (the SHARED signing name) and
/// falls back to the `Host`/`:authority` label (an `endpointPrefix`) for a request that
/// carries no scope, so an unsigned request addressed by endpoint host reaches the family
/// too. [`CHIME_SIGNING_NAME`] is itself the legacy endpoint's prefix, and so already one
/// of [`CHIME_FAMILY_ENDPOINT_PREFIXES`].
fn routes_to_the_family(parts: &Parts) -> bool {
    crate::routes_to_any(parts, CHIME_FAMILY_ENDPOINT_PREFIXES)
}

/// Whether the member of the Chime family whose `endpointPrefix` is `own_endpoint_prefix`
/// claims `parts`.
///
/// * `own_endpoint_prefix` — this service's `aws.api#service$endpointPrefix`, one of
///   [`CHIME_FAMILY_ENDPOINT_PREFIXES`].
/// * `own_path_segments` — the first URI path segments this service's model declares, as
///   the generated `crate::ROUTE_PATH_SEGMENTS`.
/// * `claims_shared_space` — this service's claim on the path spaces the family shares
///   ([`CHIME_FAMILY_SHARED_SEGMENTS`]); `|_| false` for a member that publishes none of
///   them, or that claims its share of one only at its own endpoint.
///
/// See this module's header for why the three conjuncts are in this order and what each
/// one is grounded in.
pub fn claims_chime_family_request(
    parts: &Parts,
    own_endpoint_prefix: &str,
    own_path_segments: &[&str],
    claims_shared_space: impl FnOnce(&Parts) -> bool,
) -> bool {
    if !routes_to_the_family(parts) {
        return false;
    }
    match chime_family_endpoint_addressed(parts) {
        // The request names ONE family endpoint: that endpoint serves it, whatever its
        // path, so an operation this emulator has not modelled answers from the service
        // the caller addressed rather than from a sibling.
        Some(prefix) => prefix.eq_ignore_ascii_case(own_endpoint_prefix),
        None => claims_own_exclusive_path(parts, own_path_segments) || claims_shared_space(parts),
    }
}

/// Whether `parts` addresses a first path segment this service's model declares and NO
/// sibling does.
///
/// Matching is EXACT segment equality, never a prefix test, exactly as the framework's
/// own `can_handle_based_on_route_key_and_path` matches: a path space shared with a
/// sibling is excluded here and settled by `claims_shared_space` instead.
fn claims_own_exclusive_path(parts: &Parts, own_path_segments: &[&str]) -> bool {
    crate::nth_path_segment(parts.uri.path(), 0).is_some_and(|segment| {
        own_path_segments.contains(&segment) && !CHIME_FAMILY_SHARED_SEGMENTS.contains(&segment)
    })
}

#[cfg(test)]
mod tests {
    //! The readers that settle the three path spaces the family shares, over the shapes
    //! a path arrives in: the sub-space each side owns, the sibling's sub-space, a
    //! shorter path, and a path outside the space.

    use super::*;

    #[test]
    fn the_shared_app_instance_and_meeting_spaces_are_split_at_their_third_segment() {
        // (case, path, in the app-instance space, is streaming-configurations)
        let rows: [(&str, &str, bool, bool); 7] = [
            (
                "the messaging streaming-configuration sub-space",
                "/app-instances/arn%3Aaws%3Achime%3Aus-east-1%3A000000000000%3Aapp-instance%2Fai1/streaming-configurations",
                true,
                true,
            ),
            (
                "the identity app-instance resource",
                "/app-instances/arn%3Aaws%3Achime%3Aus-east-1%3A000000000000%3Aapp-instance%2Fai1",
                true,
                false,
            ),
            ("the identity collection", "/app-instances", true, false),
            (
                "an identity sub-space of its own",
                "/app-instances/arn%3Aaws%3Achime%3Aus-east-1%3A000000000000%3Aapp-instance%2Fai1/admins",
                true,
                false,
            ),
            ("another first segment", "/channels", false, false),
            ("the root path", "/", false, false),
            (
                "a segment the app-instance one is a prefix of",
                "/app-instance-users",
                false,
                false,
            ),
        ];

        for (case, path, in_space, streaming) in rows {
            let actual = (
                addresses_app_instance_space(path),
                addresses_app_instance_streaming_configurations(path),
            );
            assert_eq!(
                actual,
                (in_space, streaming),
                "case {case:?} ({path}): expected (in the app-instance space, is \
                 streaming-configurations) = {:?}, got {actual:?}",
                (in_space, streaming)
            );
        }

        // (case, path, in the meeting space, is dial-outs)
        let meetings: [(&str, &str, bool, bool); 5] = [
            ("the legacy dial-out sub-space", "/meetings/m1/dial-outs", true, true),
            ("the meeting resource", "/meetings/m1", true, false),
            ("the meeting collection", "/meetings", true, false),
            ("a meetings sub-space of its own", "/meetings/m1/attendees", true, false),
            ("another first segment", "/tags", false, false),
        ];

        for (case, path, in_space, dial_outs) in meetings {
            let actual = (
                addresses_meeting_space(path),
                addresses_meeting_dial_outs(path),
            );
            assert_eq!(
                actual,
                (in_space, dial_outs),
                "case {case:?} ({path}): expected (in the meeting space, is dial-outs) = \
                 {:?}, got {actual:?}",
                (in_space, dial_outs)
            );
        }
    }

    #[test]
    fn the_shared_telephony_space_is_every_segment_the_two_telephony_models_both_bind() {
        for segment in CHIME_TELEPHONY_SHARED_SEGMENTS {
            let path = format!("/{segment}");
            assert!(
                addresses_shared_telephony_space(&path),
                "case {segment:?} ({path}): expected the shared telephony space, got false"
            );
            assert!(
                CHIME_FAMILY_SHARED_SEGMENTS.contains(segment),
                "case {segment:?}: every telephony segment is a shared segment, but \
                 CHIME_FAMILY_SHARED_SEGMENTS does not list it"
            );
        }
        for path in ["/accounts", "/voice-connectors", "/channels", "/"] {
            assert!(
                !addresses_shared_telephony_space(path),
                "case {path:?}: expected it outside the shared telephony space, got true"
            );
        }
    }
}
