// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for the TWO emulated services whose SigV4 signing name is
//! `greengrass`, stated once here and read by both of them.
//!
//! `aws.api#service` names the endpoint prefix `greengrass`, the ARN namespace
//! `greengrass` and the sigv4 name `greengrass` on BOTH Greengrass models — the
//! 2017-06-07 API and the 2020-11-30 one — and both are `restJson1`
//! (`smithy_query(query="service_info")` on each model). Serving two API versions of one
//! product from a single endpoint is normal AWS and is not an error condition: AWS
//! separates the two by the HTTP request itself, since every operation of either version
//! carries its own `restJson1` method + URI binding.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! both services see the key `greengrass`: a service that claims on the signing name
//! ALONE claims its peer's traffic too, the router finds two claimants and — refusing to
//! serve a request by an arbitrary one — answers `500 AmbiguousService`, a code no AWS
//! model carries, for every Greengrass request of either version.
//!
//! Nor can a path segment separate them: the two models publish the SAME two first
//! segments, `greengrass` and `tags` (`smithy_query(query="digest")` on each model), so
//! [`crate::claims_request`]'s path conjunct has two claimants for every request.
//!
//! The claim is therefore the whole modelled ROUTE — method plus URI pattern, which is
//! what distinguishes `restJson1` APIs on the wire — narrowed below for the few routes
//! the two models bind IDENTICALLY, where the route itself discriminates nothing. Source
//! for every route named here: `smithy_query(query="digest")` on both models.

use smithy_server_runtime::ErasedServiceDefinition;

/// The Smithy shape id of each emulated service that signs as `greengrass`, as
/// `definition().identity().shape_id()` reports it. A member names itself by its shape id
/// rather than by a flag the call site passes, so the table below cannot disagree with
/// the service reading it.
///
/// The 2017-06-07 API: Greengrass groups and the definition resources (cores, devices,
/// functions, loggers, resources, subscriptions, connectors) they are versioned from,
/// bound under `/greengrass/groups/…`, `/greengrass/definition/…`, `/greengrass/bulk/…`
/// and `/greengrass/updates`.
pub const V1: &str = "com.amazonaws.greengrass#Greengrass";
/// The 2020-11-30 API: components, deployments and core devices, bound under
/// `/greengrass/v2/…`.
pub const V2: &str = "com.amazonaws.greengrassv2#GreengrassV2";

/// Which member claims a route BOTH models bind, where the method and URI pattern — the
/// discriminator every other request is separated by — says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// The named member answers the route outright, because both models state the same
    /// fact about the same resource there: the route is one operation of one AWS
    /// endpoint that both API versions publish, so answering from one store answers
    /// about the right resource whichever version the caller used.
    OwnedBy(&'static str),
    /// The route carries a resource ARN as its `@httpLabel` at this pattern position, and
    /// a resource belongs to exactly ONE of the two: the ARN names the resource and so
    /// its owner ([`arn_claimant`]).
    ByResourceArn(usize),
}

/// Every route the two models bind IDENTICALLY, and which member claims it.
///
/// Eight routes qualify, in three groups; every other route of either model is bound by
/// that model alone, so the route already names one claimant.
///
/// * `GET`, `PUT` and `DELETE /greengrass/servicerole` —
///   `GetServiceRoleForAccount`, `AssociateServiceRoleToAccount` and
///   `DisassociateServiceRoleFromAccount` in both models. The Greengrass service role is
///   ONE association per account and Region: both API versions read and write the same
///   account-level fact, so one owner answers about the same resource either way. The
///   2020-11-30 service owns it, as the version AWS publishes the API under today and the
///   one a current client reaches this endpoint with; both versions' three handlers are
///   scaffolded 501s as this is written, so nothing implemented is lost either way.
/// * `GET` and `PUT /greengrass/things/{…}/connectivityInfo` — `GetConnectivityInfo` and
///   `UpdateConnectivityInfo` in both models, keyed by the core device's thing name. Again
///   one fact per thing, published by both versions, so it has one owner, for the same
///   reason and the same way round.
/// * `GET`, `POST` and `DELETE /tags/{…}` — `ListTagsForResource`, `TagResource` and
///   `UntagResource` in both models. Here the two services hold DIFFERENT resources, and
///   the ARN in the path is what names which — so this group is split by that ARN rather
///   than owned, the case [`crate::shared_path`] exists for.
const SHARED_ROUTES: &[SharedRoute] = &[
    SharedRoute {
        method: "GET",
        pattern: &["greengrass", "servicerole"],
        claim: Claim::OwnedBy(V2),
    },
    SharedRoute {
        method: "PUT",
        pattern: &["greengrass", "servicerole"],
        claim: Claim::OwnedBy(V2),
    },
    SharedRoute {
        method: "DELETE",
        pattern: &["greengrass", "servicerole"],
        claim: Claim::OwnedBy(V2),
    },
    SharedRoute {
        method: "GET",
        pattern: &["greengrass", "things", LABEL, "connectivityInfo"],
        claim: Claim::OwnedBy(V2),
    },
    SharedRoute {
        method: "PUT",
        pattern: &["greengrass", "things", LABEL, "connectivityInfo"],
        claim: Claim::OwnedBy(V2),
    },
    SharedRoute {
        method: "GET",
        pattern: &["tags", LABEL],
        claim: Claim::ByResourceArn(1),
    },
    SharedRoute {
        method: "POST",
        pattern: &["tags", LABEL],
        claim: Claim::ByResourceArn(1),
    },
    SharedRoute {
        method: "DELETE",
        pattern: &["tags", LABEL],
        claim: Claim::ByResourceArn(1),
    },
];

/// One route shape both models bind, matched on its method and its whole segment count.
#[derive(Debug, Clone, Copy)]
struct SharedRoute {
    /// The HTTP method, compared exactly (as the framework's route matcher compares it).
    method: &'static str,
    /// The path pattern's segments, with [`LABEL`] standing for an `@httpLabel`.
    pattern: &'static [&'static str],
    /// Which member claims a request on this route.
    claim: Claim,
}

/// Stands for an `@httpLabel` segment in [`SHARED_ROUTES`]: it matches any one non-empty
/// segment. Not a wildcard over several segments — a route shape's segment count is
/// matched exactly, as the framework's own route matcher does for a pattern with no
/// greedy label.
const LABEL: &str = "{}";

/// Whether the Greengrass-family service whose generated `ServiceDefinition` is
/// `definition` claims `parts`.
///
/// * `path_segments` — that service's generated `crate::ROUTE_PATH_SEGMENTS`, the first
///   URI path segments its model declares.
///
/// The rule names no service except in [`SHARED_ROUTES`], and is three tiers:
///
/// 1. the request's resolved routing key is the signing name both models declare (or,
///    for a host-addressed request carrying no credential scope, the endpoint prefix,
///    which is the same token), and its first path segment is one this model publishes —
///    [`crate::claims_request`], whose fourth argument is `None` because both models are
///    `restJson1` and so have no RPC wire form to be claimed by name;
/// 2. for a route both models bind, the member [`SHARED_ROUTES`] gives it; and
/// 3. otherwise the member whose own model binds a route for the request, which is how
///    AWS itself dispatches a REST request once the endpoint has chosen the service.
///
/// A path inside the shared space that NEITHER model binds is claimed by nobody and the
/// router answers it `404 UnknownService`, rather than one version answering for an
/// operation it does not publish.
pub fn claims_request(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
    path_segments: &[&str],
) -> bool {
    let identity = definition.identity();
    if !crate::claims_request(parts, identity.signing_name(), path_segments, None) {
        return false;
    }
    match shared_route_claimant(parts.method.as_str(), parts.uri.path()) {
        Some(claimant) => identity.shape_id() == claimant,
        None => crate::addresses_modeled_route(parts, definition),
    }
}

/// The ONE member that claims `method` + `path` when both models bind that route shape,
/// or `None` when no entry of [`SHARED_ROUTES`] matches it — the common case, where the
/// route itself names one claimant.
///
/// The segment count must match exactly and each pattern segment must equal the
/// request's, except [`LABEL`], which matches any one segment.
fn shared_route_claimant(method: &str, path: &str) -> Option<&'static str> {
    let segments = path_segments(path);
    let route = SHARED_ROUTES.iter().find(|route| {
        route.method == method
            && route.pattern.len() == segments.len()
            && route
                .pattern
                .iter()
                .zip(&segments)
                .all(|(expected, actual)| *expected == LABEL || expected == actual)
    })?;
    Some(match route.claim {
        Claim::OwnedBy(owner) => owner,
        Claim::ByResourceArn(index) => {
            arn_claimant(segments.get(index).copied().unwrap_or_default())
        }
    })
}

/// The member that owns the resource the ARN `label` names.
///
/// Both API versions mint ARNs in the `greengrass` namespace, and the resource field says
/// which version's resource it is: the 2017-06-07 API's ARNs carry that API's own URI
/// PATH as the resource (`arn:…:greengrass:<region>:<account>:/greengrass/groups/<id>`),
/// while the 2020-11-30 API's carry a `<type>:<id>` resource
/// (`…:components:<name>:versions:<version>`). So a path-style resource names a
/// 2017-06-07 resource, and every other label — a `<type>:<id>` resource, another
/// service's ARN, or a string that is no ARN at all — names a 2020-11-30 one, where it is
/// answered by that service's own modelled refusal rather than by nobody.
///
/// The label is percent-decoded first: a non-greedy `@httpLabel` carrying an ARN has its
/// `:` and `/` encoded by most clients and sent raw by some. It is read through
/// [`crate::arn`], the one parse this crate states for a caller-supplied ARN;
/// [`crate::shared_path::shared_path_arn`], which settles this same case elsewhere, keys
/// on the resource TYPE, which a path-style resource does not carry at all.
fn arn_claimant(label: &str) -> &'static str {
    let decoded = smithy_server_runtime::percent_decode(label);
    let path_style = crate::arn::resource(&decoded)
        .is_some_and(|resource| resource.starts_with('/'))
        && crate::arn::vendor(&decoded).is_some_and(|vendor| vendor.eq_ignore_ascii_case(NAMESPACE));
    if path_style {
        V1
    } else {
        V2
    }
}

/// The ARN namespace both models declare (`aws.api#service$arnNamespace`), and so the
/// only one an ARN either of them mints can carry.
const NAMESPACE: &str = "greengrass";

/// The `/`-separated, non-empty segments of a URI path, as the framework's route matcher
/// splits them: leading and trailing `/` are ignored, so `/tags/X/` and `/tags/X` are the
/// same two segments.
fn path_segments(path: &str) -> Vec<&str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    }
}

#[cfg(test)]
mod tests {
    //! The part of the rule that needs no built service: which routes both models bind,
    //! and which member each one goes to.

    use super::*;

    /// Every service that signs as `greengrass`, for the table above to be checked
    /// against.
    const MEMBERS: &[&str] = &[V1, V2];

    /// The label as a client that percent-encodes a non-greedy `@httpLabel` sends it.
    fn encoded(arn: &str) -> String {
        arn.replace(':', "%3A").replace('/', "%2F")
    }

    /// RULE: a route both models bind goes to the member the table names — the account-
    /// and thing-level facts both versions publish to the version that serves them, and a
    /// tagging request to the version that owns the resource its ARN names — while every
    /// other route matches no entry, so the route itself decides.
    #[test]
    fn a_route_both_models_bind_has_exactly_one_claimant() {
        // (case, method, path, expected claimant)
        //
        // The rows span the three groups, both ARN conventions and both spellings of an
        // ARN label's separators, plus the near misses a segment-count or method
        // mismatch must not draw in.
        let rows: Vec<(&str, &str, String, Option<&str>)> = vec![
            (
                "the account's service role, read",
                "GET",
                "/greengrass/servicerole".to_string(),
                Some(V2),
            ),
            (
                "the account's service role, associated",
                "PUT",
                "/greengrass/servicerole".to_string(),
                Some(V2),
            ),
            (
                "the account's service role, disassociated",
                "DELETE",
                "/greengrass/servicerole".to_string(),
                Some(V2),
            ),
            (
                "a thing's connectivity info, read",
                "GET",
                "/greengrass/things/laser-1/connectivityInfo".to_string(),
                Some(V2),
            ),
            (
                "a thing's connectivity info, written",
                "PUT",
                "/greengrass/things/laser-1/connectivityInfo".to_string(),
                Some(V2),
            ),
            (
                "the same route with a trailing slash",
                "GET",
                "/greengrass/servicerole/".to_string(),
                Some(V2),
            ),
            (
                "a 2020-11-30 component version's tags",
                "GET",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:greengrass:us-east-1:000000000000:components:probe:versions:1.0.0"
                    )
                ),
                Some(V2),
            ),
            (
                "a 2020-11-30 deployment tagged",
                "POST",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:greengrass:eu-west-1:123456789012:deployments:d-1")
                ),
                Some(V2),
            ),
            (
                "a 2017-06-07 group's tags, read",
                "GET",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:greengrass:us-east-1:000000000000:/greengrass/groups/g-1")
                ),
                Some(V1),
            ),
            (
                "a 2017-06-07 definition version untagged",
                "DELETE",
                format!(
                    "/tags/{}",
                    encoded(
                        "arn:aws:greengrass:us-west-2:000000000000:/greengrass/definition/cores/c-1/versions/v-1"
                    )
                ),
                Some(V1),
            ),
            (
                // Sent raw, a path-style ARN's own separators make the request deeper
                // than the one-segment label the pattern binds, so it matches no route
                // of either model and reaches nobody — which is what the framework's own
                // route matcher does with it too.
                "a 2017-06-07 group ARN sent raw",
                "POST",
                "/tags/arn:aws:greengrass:us-east-1:000000000000:/greengrass/groups/g-1"
                    .to_string(),
                None,
            ),
            (
                "another service's ARN",
                "GET",
                format!(
                    "/tags/{}",
                    encoded("arn:aws:iot:us-east-1:000000000000:thing/laser-1")
                ),
                Some(V2),
            ),
            (
                "a label that is no ARN",
                "POST",
                "/tags/not-an-arn".to_string(),
                Some(V2),
            ),
            // Routes exactly one model binds: no entry matches, so the modelled route
            // already names one claimant.
            (
                "a 2020-11-30 component version created",
                "POST",
                "/greengrass/v2/createComponentVersion".to_string(),
                None,
            ),
            (
                "a 2017-06-07 group created",
                "POST",
                "/greengrass/groups".to_string(),
                None,
            ),
            (
                "a thing's runtime configuration, which only the 2017-06-07 API binds",
                "GET",
                "/greengrass/things/laser-1/runtimeconfig".to_string(),
                None,
            ),
            (
                "a method neither model binds on a shared path",
                "POST",
                "/greengrass/servicerole".to_string(),
                None,
            ),
            (
                "the tagging segment with no label",
                "GET",
                "/tags".to_string(),
                None,
            ),
            (
                "a deeper path under the tagging segment",
                "GET",
                "/tags/arn/extra".to_string(),
                None,
            ),
            ("the root path", "GET", "/".to_string(), None),
        ];

        for (case, method, path, expected) in rows {
            let actual = shared_route_claimant(method, &path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({method} {path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every claimant the table can name is a member of this family, no route shape is
    /// declared twice, and an ARN-split route's label position really is a label.
    #[test]
    fn every_claimant_is_a_member_and_nothing_repeats() {
        let mut routes: Vec<(&str, &[&str])> = Vec::new();
        for route in SHARED_ROUTES {
            let key = (route.method, route.pattern);
            assert!(
                !routes.contains(&key),
                "route {} {:?} is declared more than once",
                route.method,
                route.pattern
            );
            routes.push(key);
            match route.claim {
                Claim::OwnedBy(owner) => assert!(
                    MEMBERS.contains(&owner),
                    "route {} {:?} is owned by {owner:?}, which is not a member of the \
                     greengrass family",
                    route.method,
                    route.pattern
                ),
                Claim::ByResourceArn(index) => assert_eq!(
                    route.pattern.get(index),
                    Some(&LABEL),
                    "route {} {:?} splits on segment {index}, which is not a label",
                    route.method,
                    route.pattern
                ),
            }
        }
    }
}
