// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
//! Code more than one emulated service needs, as a path dependency.
//!
//! Today that is three things: [`claims_request`], the request-claim rule a
//! service whose SigV4 signing name is SHARED with other services uses for its
//! `Service::can_handle`; [`stable_hex_id`], the one deterministic mixing
//! function a service derives a wire-visible id with; and [`network`], the
//! contract the account's EC2 network — which subnets a Region has — is modelled
//! against, so no two services answer "does this subnet exist" differently.
//!
//! # Why a shared rule
//!
//! The router resolves a request's routing key from the SigV4 credential scope
//! first, which carries the SIGNING name — and several AWS services sign under one
//! name while serving distinct endpoints. Every AWS Marketplace-family service signs
//! as `aws-marketplace` (Marketplace Catalog, Reporting, Deployment, Discovery,
//! Agreement, Entitlement, Metering), `iam` is shared, and so is `sagemaker`. A
//! service that claims on the signing name alone therefore claims its peers' traffic
//! too, every claimant is a claimant for every request, and the router — which
//! refuses to pick one — answers `500 AmbiguousService` to all of them.
//!
//! The rule below is the fix, stated once: a service claims a request when the
//! signing name matches AND the request addresses that service's OWN modelled wire
//! surface. What "addresses" means follows the protocol the model declares, and
//! nothing here names a service:
//!
//! * A REST protocol (`restJson1`, `restXml`) binds each operation to a URI, so the
//!   request's first URI path segment must be one the service's model declares —
//!   the framework's own discriminator, [`claims_modeled_path`].
//! * An RPC protocol carries the target service out of band and posts every
//!   operation to one path: `awsJson1_0` / `awsJson1_1` name it in
//!   `X-Amz-Target: <Service>.<Operation>`, and `rpcv2Cbor` in the URI
//!   `/service/<Service>/operation/<Operation>`. So the service NAMED there must be
//!   this one.
//!
//! Path segments alone cannot separate RPC peers: every one of them posts to `/`,
//! so the generated `ROUTE_PATH_SEGMENTS` is empty (or just the two protocol
//! sentinels) for all of them and they all keep claiming everything. The RPC
//! conjunct is what separates them.
//!
//! # Services still carrying their own copy
//!
//! The path half of the rule predates this crate and is inlined in the `can_handle` of
//! the service crates that needed it before there was anywhere shared to put it —
//! `grep -rln 'fn claims_modeled_path' services/*/src/service.rs` lists them (today:
//! `connect`, `connectcontactlens`, `iam`, `iamtoolbox`, `chimesdkmeetings`,
//! `personalizeevents`, `personalizeruntime`, `signerdata`, and the marketplace family
//! before this crate existed; `s3` carries only `first_path_segment`). Those copies are to be
//! MIGRATED onto [`claims_request`] (they are REST-only cases, so the call is
//! `claims_request(parts, signing_name, crate::ROUTE_PATH_SEGMENTS, None)`); they are
//! left in place here only because this change does not own those services' issues and
//! each is covered by its own routing suite under `core/router/tests/all/`. Nothing new
//! should inline the rule again — call this crate.

pub mod network;

use emulator_server::{resolve_route_key, service_from_target, ROOT_PATH_SEGMENT};

/// A stable 64-bit mixing of `parts`, rendered as 16 hexadecimal digits.
///
/// For an id a service DERIVES from what it identifies and then puts ON THE
/// WIRE — X-Ray's inferred segment id, say. Two properties matter and neither is
/// free:
///
/// * the same input mixes to the same id on every READ, so a caller that reads a
///   derived resource twice sees one id; and
/// * the same input mixes to the same id in every BUILD, which `std`'s
///   `DefaultHasher` does not promise (`std` reserves the right to change its
///   algorithm), so an id taken from it is reproducible only within one
///   toolchain.
///
/// The mixing is SHA-256 from the maintained `sha2` crate — nothing is
/// hand-rolled here — over the parts, each followed by a NUL so that two
/// different part lists whose concatenation coincides still mix differently; the
/// id is its first 8 bytes. Truncated like that it is an OPAQUE id, not an
/// unguessable one: nothing may authenticate on it.
///
/// Several service crates still carry their own hand-written mixing function
/// (`grep -rln 'FNV' services/*/src/state.rs`); they are to be migrated onto
/// this one, and nothing new should write another.
#[must_use]
pub fn stable_hex_id(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update(b"\0");
    }
    digest.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The fixed first path segment of every `rpcv2Cbor` request
/// (`POST /service/{Service}/operation/{Operation}`), as
/// `emulator-core-build-support` publishes it in `ROUTE_PATH_SEGMENTS` for a service
/// that speaks that protocol.
pub const RPCV2_CBOR_PATH_SEGMENT: &str = "service";

/// Whether `service` claims `parts`: its signing name matches the request's resolved
/// routing key AND the request addresses this service's own modelled wire surface.
///
/// * `signing_name` — the service's SigV4 signing name
///   (`definition().identity().signing_name()`).
/// * `path_segments` — the first URI path segments the model declares, as the
///   generated `crate::ROUTE_PATH_SEGMENTS`.
/// * `rpc_service_shape_id` — `Some(shape id)` when the model declares an RPC
///   protocol (`awsJson1_0`, `awsJson1_1`, `rpcv2Cbor`), whose wire form names the
///   service by the shape's LOCAL name; `None` for a purely REST service, which is
///   then discriminated by its path space exactly as the framework does.
///
/// An RPC service claims ONLY requests that name it AND arrive on a path its protocol
/// uses: its path space is the protocol's own (`/`, or `/service/...`), which every RPC
/// peer sharing the signing name also posts to, so a path test alone cannot tell them
/// apart — and a request that names one service while addressing another's REST path
/// (`POST /CancelChangeSet` with an `X-Amz-Target`) contradicts itself, so it must reach
/// NOBODY rather than two claimants. A model that declares an RPC protocol AND binds
/// some operations to URIs (Glue and Transfer each do) keeps both claims: the RPC form,
/// or one of its own literal path segments.
pub fn claims_request(
    parts: &http::request::Parts,
    signing_name: &str,
    path_segments: &[&str],
    rpc_service_shape_id: Option<&str>,
) -> bool {
    if !routes_to(parts, signing_name) {
        return false;
    }
    match rpc_service_shape_id {
        None => claims_modeled_path(parts, path_segments),
        Some(shape_id) if is_rpc_addressed(parts) => {
            rpc_names_this_service(parts, shape_local_name(shape_id))
                && (claims_rpc_path(parts) || claims_literal_path(parts, path_segments))
        }
        // Not addressed in an RPC form at all: only a URI this model binds itself.
        Some(_) => claims_literal_path(parts, path_segments),
    }
}

/// Whether `parts` is addressed in an RPC form: `awsJson1_0` / `awsJson1_1` name their
/// target service in `X-Amz-Target: <Service>.<Operation>`, and `rpcv2Cbor` in the URI
/// `/service/<Service>/operation/<Operation>`.
fn is_rpc_addressed(parts: &http::request::Parts) -> bool {
    parts.headers.contains_key("x-amz-target")
        || rpcv2_cbor_service_named(parts.uri.path()).is_some()
}

/// Whether the service an RPC-addressed request names is the one whose shape-local name
/// is `declared`.
///
/// This is why an RPC peer does not also claim a REST peer's path: a request carrying an
/// `X-Amz-Target` that names an RPC service is claimed by that service only on a path
/// its protocol uses, so the path's own owner serves it alone.
///
/// The `X-Amz-Target` side is read through the framework's OWN reader,
/// `emulator_server::service_from_target`, which splits on the `.`, strips a trailing
/// `_<digits>` API-version suffix and lower-cases the result, so the target-parsing rule
/// stays in one place and a client sending either the versioned or the unversioned form
/// reaches the service either way. The `rpcv2Cbor` side has no framework reader: its
/// `{Service}` label is compared case-insensitively.
fn rpc_names_this_service(parts: &http::request::Parts, declared: &str) -> bool {
    if let Some(named) = service_from_target(&parts.headers) {
        return named == strip_version_suffix(declared).to_ascii_lowercase();
    }
    rpcv2_cbor_service_named(parts.uri.path())
        .is_some_and(|named| named.eq_ignore_ascii_case(declared))
}

/// Whether `parts` arrives on a path an RPC protocol uses: the ROOT path every
/// `awsJson` operation posts to, or the `/service/<Service>/operation/<Operation>` an
/// `rpcv2Cbor` one does.
fn claims_rpc_path(parts: &http::request::Parts) -> bool {
    let path = parts.uri.path();
    first_path_segment(path).is_none() || rpcv2_cbor_service_named(path).is_some()
}

/// Whether the routing key the framework resolves for `parts` is `signing_name`.
///
/// This is `emulator_server::can_handle_based_on_route_key`'s rule applied over
/// `Parts`: that helper takes a whole buffered `http::Request`, which the
/// `Service::can_handle` seam does not receive. Comparison is case-insensitive on a
/// trimmed name, as there.
pub fn routes_to(parts: &http::request::Parts, signing_name: &str) -> bool {
    match resolve_route_key(parts) {
        Some(key) => key.eq_ignore_ascii_case(signing_name.trim()),
        None => false,
    }
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments a service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, keeping both of its
/// edge cases: an EMPTY list means the model published no usable discriminator and is
/// "no constraint", and [`ROOT_PATH_SEGMENT`] is how a service that must answer
/// `POST /` claims the root path without claiming every path. Matching is EXACT
/// segment equality, deliberately never a prefix test.
pub fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&ROOT_PATH_SEGMENT),
    }
}

/// [`claims_modeled_path`] over the LITERAL segments only: the two protocol
/// sentinels (`/` for an RPC service's root post, `service` for an `rpcv2Cbor` one)
/// are dropped, and an empty list claims nothing.
///
/// Used for a service that declares an RPC protocol, where those sentinels are the
/// path space every peer sharing the signing name posts to. Its RPC traffic is
/// claimed by name instead, so here only a URI its own model binds counts.
fn claims_literal_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.iter().any(|declared| {
            *declared == segment
                && *declared != ROOT_PATH_SEGMENT
                && *declared != RPCV2_CBOR_PATH_SEGMENT
        }),
        None => false,
    }
}

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// The `<Service>` of an `rpcv2Cbor` URI `/service/<Service>/operation/<Operation>`,
/// or `None` for any other path.
fn rpcv2_cbor_service_named(path: &str) -> Option<&str> {
    let rest = path.trim_start_matches('/').strip_prefix("service/")?;
    let (service, operation) = rest.split_once("/operation/")?;
    (!service.is_empty() && !service.contains('/') && !operation.is_empty()).then_some(service)
}

/// The local name of a Smithy shape id: `com.amazonaws.marketplacemetering#AWSMPMeteringService`
/// -> `AWSMPMeteringService`. An id with no namespace is already a local name.
pub fn shape_local_name(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// Strip a trailing `_<digits>` API-version suffix, e.g. `DynamoDB_20120810` ->
/// `DynamoDB`, which is what `emulator_server::service_from_target` does to the target
/// it reads. Leaves any other name intact (`AWSMPCommerceService_v20200301` keeps its
/// suffix: `v20200301` is not digits).
fn strip_version_suffix(name: &str) -> &str {
    match name.rfind('_') {
        Some(index) => {
            let suffix = &name[index + 1..];
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                &name[..index]
            } else {
                name
            }
        }
        None => name,
    }
}

#[cfg(test)]
mod tests {
    //! The claim rule over the shapes a shared signing name produces: two REST peers
    //! with disjoint path spaces, and RPC peers that all post to one path and are
    //! separated only by the service they name.

    use super::*;

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    fn parts(signing_name: &str, uri: &str, target: Option<&str>) -> http::request::Parts {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", authorization(signing_name));
        if let Some(target) = target {
            builder = builder.header("x-amz-target", target);
        }
        builder.body(()).expect("a well-formed test request").into_parts().0
    }

    /// Two REST services sharing `aws-marketplace`: each one's own path space reaches
    /// it and the other's does not, and a request signed for another service reaches
    /// neither.
    #[test]
    fn rest_peers_claim_only_their_own_path_space() {
        const CATALOG: &[&str] = &["CancelChangeSet", "ListAssessments"];
        const REPORTING: &[&str] = &["getBuyerDashboard"];

        // (case, signing name, uri, claimed by catalog, claimed by reporting)
        let rows: [(&str, &str, &str, bool, bool); 5] = [
            ("catalog path", "aws-marketplace", "/CancelChangeSet", true, false),
            ("catalog list path", "aws-marketplace", "/ListAssessments", true, false),
            ("reporting path", "aws-marketplace", "/getBuyerDashboard", false, true),
            ("neither path", "aws-marketplace", "/Nope", false, false),
            ("another service", "s3", "/CancelChangeSet", false, false),
        ];

        for (case, signing_name, uri, by_catalog, by_reporting) in rows {
            let parts = parts(signing_name, uri, None);
            let catalog = claims_request(&parts, "aws-marketplace", CATALOG, None);
            let reporting = claims_request(&parts, "aws-marketplace", REPORTING, None);
            assert_eq!(
                (catalog, reporting),
                (by_catalog, by_reporting),
                "case {case:?}: expected (catalog, reporting) == ({by_catalog}, {by_reporting}), \
                 got ({catalog}, {reporting})"
            );
        }
    }

    /// RPC peers sharing one signing name all post to `/` (or `/service/...`), so only
    /// the service the request NAMES decides.
    #[test]
    fn rpc_peers_claim_only_the_requests_that_name_them() {
        const METERING: &str = "com.amazonaws.marketplacemetering#AWSMPMeteringService";
        const ENTITLEMENT: &str =
            "com.amazonaws.marketplaceentitlementservice#AWSMPEntitlementService";
        // What `emulator-core-build-support` publishes for them: metering (awsJson1_1,
        // no http bindings) gets an EMPTY list, entitlement (awsJson1_1 + rpcv2Cbor)
        // the two protocol sentinels.
        const METERING_SEGMENTS: &[&str] = &[];
        const ENTITLEMENT_SEGMENTS: &[&str] = &["/", "service"];

        // (case, uri, x-amz-target, claimed by metering, claimed by entitlement)
        let rows: [(&str, &str, Option<&str>, bool, bool); 8] = [
            ("metering target", "/", Some("AWSMPMeteringService.MeterUsage"), true, false),
            (
                "entitlement target",
                "/",
                Some("AWSMPEntitlementService.GetEntitlements"),
                false,
                true,
            ),
            (
                "entitlement cbor uri",
                "/service/AWSMPEntitlementService/operation/GetEntitlements",
                None,
                false,
                true,
            ),
            (
                "a third service's cbor uri",
                "/service/AWSMPCommerceService_v20200301/operation/DescribeAgreement",
                None,
                false,
                false,
            ),
            ("a REST peer's path", "/CancelChangeSet", None, false, false),
            (
                // A request that names one service while addressing another's REST
                // path contradicts itself: it reaches nobody, never two claimants.
                "a target on a REST peer's path",
                "/CancelChangeSet",
                Some("AWSMPMeteringService.MeterUsage"),
                false,
                false,
            ),
            (
                "a cbor uri naming it, with its own operation",
                "/service/AWSMPMeteringService/operation/MeterUsage",
                None,
                true,
                false,
            ),
            ("root post naming nobody", "/", None, false, false),
        ];

        for (case, uri, target, by_metering, by_entitlement) in rows {
            let parts = parts("aws-marketplace", uri, target);
            let metering =
                claims_request(&parts, "aws-marketplace", METERING_SEGMENTS, Some(METERING));
            let entitlement = claims_request(
                &parts,
                "aws-marketplace",
                ENTITLEMENT_SEGMENTS,
                Some(ENTITLEMENT),
            );
            assert_eq!(
                (metering, entitlement),
                (by_metering, by_entitlement),
                "case {case:?}: expected (metering, entitlement) == ({by_metering}, \
                 {by_entitlement}), got ({metering}, {entitlement})"
            );
        }
    }

    /// An RPC model that ALSO binds operations to URIs keeps both claims.
    #[test]
    fn a_mixed_model_claims_its_literal_paths_and_its_rpc_form() {
        const SHAPE: &str = "com.amazonaws.glue#AWSGlue";
        const SEGMENTS: &[&str] = &["/", "dev-endpoint"];

        // (case, uri, target, claimed)
        let rows: [(&str, &str, Option<&str>, bool); 5] = [
            ("its own rpc form", "/", Some("AWSGlue.GetTables"), true),
            ("its own bound path", "/dev-endpoint/x", None, true),
            ("another path", "/CancelChangeSet", None, false),
            ("another service's rpc form", "/", Some("AWSMPMeteringService.MeterUsage"), false),
            (
                // Its own path, but naming another service out of band: the request
                // contradicts itself, so it reaches nobody rather than two claimants.
                "its own bound path, naming another service",
                "/dev-endpoint/x",
                Some("AWSMPMeteringService.MeterUsage"),
                false,
            ),
        ];

        for (case, uri, target, expected) in rows {
            let parts = parts("glue", uri, target);
            let claimed = claims_request(&parts, "glue", SEGMENTS, Some(SHAPE));
            assert_eq!(
                claimed, expected,
                "case {case:?}: expected claimed == {expected}, got {claimed}"
            );
        }
    }

    #[test]
    fn a_version_suffixed_target_names_the_same_service() {
        // (case, declared shape id, target, expected)
        let rows: [(&str, &str, &str, bool); 3] = [
            (
                "suffix on both",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "DynamoDB_20120810.GetItem",
                true,
            ),
            (
                "suffix on the model only",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "DynamoDB.GetItem",
                true,
            ),
            (
                "a different service",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "AWSKMS.Encrypt",
                false,
            ),
        ];

        for (case, shape_id, target, expected) in rows {
            let parts = parts("dynamodb", "/", Some(target));
            let claimed = claims_request(&parts, "dynamodb", &[], Some(shape_id));
            assert_eq!(
                claimed, expected,
                "case {case:?}: expected claimed == {expected}, got {claimed}"
            );
        }
    }

    /// The two properties a derived, wire-visible id needs: the same parts mix
    /// to the same 16 hexadecimal digits every time, and parts that differ —
    /// including parts that only differ in where one ends — mix to different
    /// ids.
    #[test]
    fn a_stable_id_is_reproducible_and_separates_its_parts() {
        // (case, parts)
        let rows: [(&str, &[&str]); 6] = [
            ("nothing", &[]),
            ("one empty part", &[""]),
            ("one part", &["inferred"]),
            ("two parts", &["inferred", "1-58406520-a006649127e371903a2de979"]),
            (
                "three parts",
                &[
                    "inferred",
                    "1-58406520-a006649127e371903a2de979",
                    "0000000000000001",
                ],
            ),
            // The same characters, split differently: a mixing function that
            // concatenated its parts would answer the same id for this row as
            // for the one above.
            (
                "the same characters split differently",
                &[
                    "inferred",
                    "1-58406520-a006649127e371903a2de9790000000000000001",
                ],
            ),
        ];

        let mut seen: Vec<(&str, String)> = Vec::new();
        for (case, parts) in rows {
            let id = stable_hex_id(parts);
            assert_eq!(
                id.len(),
                16,
                "case {case:?}: an id is 16 hexadecimal digits, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case:?}: an id is hexadecimal, got {id:?}"
            );
            assert_eq!(
                stable_hex_id(parts),
                id,
                "case {case:?}: the same parts must mix to the same id"
            );
            if let Some((other, _)) = seen.iter().find(|(_, other)| *other == id) {
                panic!("case {case:?}: mixes to the same id as case {other:?}: {id:?}");
            }
            seen.push((case, id));
        }
    }
}
