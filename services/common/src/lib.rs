// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
//! Code more than one emulated service needs, as a path dependency.
//!
//! Today that is eight things:
//!
//! * [`claims_request`], the request-claim rule a service whose SigV4 signing name is
//!   SHARED with other services uses for its `Service::can_handle`;
//! * [`arn`], the one parse of a caller-supplied resource ARN, so no two services
//!   disagree about which vendor an ARN names;
//! * [`nth_path_segment`] and [`addresses_modeled_route`], for the rarer case of two such
//!   services publishing the SAME path space — the segment their modelled URIs differ at,
//!   or (where only the whole route tells them apart) whether a service's own model binds
//!   a route for the request at all. [`bedrock`] states that split once for the six
//!   services that share the signing name `bedrock`, and [`lex`] for the four that share
//!   `lex`;
//! * [`shared_path`], the reader for the one case the path half of the claim rule
//!   cannot settle on its own: two colliding models binding the same URI path space,
//!   where the ARN in the path names the resource and so the service that owns it;
//! * [`front_door`], the body-rejection layer an AWS REST service puts in front of its
//!   application code, as a wrapper a service serves its operations behind;
//! * [`validation`], which restores the value a constraint violation echoes to the one
//!   the caller submitted, so a service wording its own `ValidationException` from the
//!   framework's violations does not report a value nobody sent;
//! * [`iam_trust_policy`], the ONE reading of an IAM trust policy, for every service that
//!   resolves a caller-supplied execution role before it writes — so two services asked
//!   whether a document lets them assume a role cannot answer differently;
//! * [`stable_hex_id`], the one deterministic mixing function a service derives a
//!   wire-visible id with;
//! * [`network`], the contract the account's EC2 network — which subnets a Region has —
//!   is modelled against, so no two services answer "does this subnet exist"
//!   differently.
//!
//! Beside them, two SHARED STORES live here, each held once in the shared engine so
//! the services on either side of a hand-off cannot hold two copies of one fact:
//!
//! * [`iot_job_executions`], the IoT job-execution store the IoT control plane writes
//!   (a job targets a thing) and the IoT Jobs data plane reads and updates (the device
//!   polls, starts and reports);
//! * [`db_data_sources`], the directory of which database service minted an immutable
//!   DB resource id — written by the database control planes when they mint one, read
//!   by the services that monitor those resources, which cannot tell an RDS id from a
//!   DocumentDB id by its form.
//!
//! # Why a shared rule
//!
//! The router resolves a request's routing key from the SigV4 credential scope
//! first, which carries the SIGNING name — and several AWS services sign under one
//! name while serving distinct endpoints. Every AWS Marketplace-family service signs
//! as `aws-marketplace` (Marketplace Catalog, Reporting, Deployment, Discovery,
//! Agreement, Entitlement, Metering), `iam` is shared, and so is `sagemaker` (nine
//! services: the control plane plus the metrics, a2i-runtime, edge,
//! featurestore-runtime, job-runtime, training-session-runtime and two runtime
//! endpoints). A service that claims on the signing name alone therefore claims its
//! peers' traffic too, every claimant is a claimant for every request, and the
//! router — which refuses to pick one — answers `500 AmbiguousService` to all of them.
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
//! When two REST peers' path spaces overlap at EVERY segment a prefix test can read, no
//! segment index separates them either. What does is the whole modelled route: AWS
//! dispatches a REST request on the endpoint host and then on the operation's method +
//! URI, so a peer claims exactly the requests its OWN model binds a route for — the key
//! half and that route half together are [`claims_modeled_route`].
//!
//! Two REST peers can also publish the same first path segment — the two SageMaker
//! Runtime endpoints both bind `/endpoints/{EndpointName}/…` and even share an
//! `endpointPrefix` — and then the first segment separates nothing either. The segment
//! their modelled URIs actually differ at does, which is what [`nth_path_segment`] reads
//! and what [`addresses_runtime_bidirectional_stream`] states once for that pair: the
//! service that owns the narrower space claims it, its peer claims the complement, so a
//! path inside the shared space always has exactly one claimant.
//!
//! # Services still carrying their own copy
//!
//! The path half of the rule predates this crate and is inlined in the `can_handle` of
//! the service crates that needed it before there was anywhere shared to put it —
//! `grep -rln 'fn claims_modeled_path' services/*/src/service.rs` lists them (today:
//! `connect`, `connectcontactlens`, `iam`, `iamtoolbox`, `chimesdkmeetings`,
//! `personalizeevents`, `personalizeruntime`, `signerdata`, and the marketplace family
//! before this crate existed; `s3` carries only `first_path_segment`, and `appconfig`,
//! `appconfigdata`, `ivs` and `ivsrealtime` carry a `can_handle_parts` shim that calls the
//! framework helper on a rebuilt request). Those copies are to be MIGRATED onto
//! [`claims_request`] (they are REST-only cases, so the call is
//! `claims_request(parts, signing_name, crate::ROUTE_PATH_SEGMENTS, None)`); they are
//! left in place here only because this change does not own those services' issues and
//! each is covered by its own routing suite under `core/router/tests/all/`. Nothing new
//! should inline the rule again — call this crate.
//!
//! # Beside the claim rule
//!
//! [`rds_cluster_parameters`] is the second thing more than one service needs: the engine
//! parameter catalog and the parameter rules of the ONE AWS `rds` front door, which Amazon
//! RDS, Amazon DocumentDB and Amazon Neptune are three Smithy models of. The data is pure
//! — no modelled shape appears in it — so each of those crates keeps its own generated
//! shapes and state while the catalog and the rules over it live here once.

pub mod rds_cluster_parameters;

pub mod arn;
pub mod bedrock;
pub mod db_data_sources;
pub mod front_door;
pub mod iam_trust_policy;
pub mod iot_job_executions;
pub mod lex;
pub mod network;
pub mod shared_path;
pub mod validation;

use aws_smithy_runtime_api::http::Request as RuntimeApiRequest;
use aws_smithy_types::body::SdkBody;
use emulator_server::{resolve_route_key, service_from_target, ROOT_PATH_SEGMENT};
use smithy_server_runtime::ErasedServiceDefinition;

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
///
/// The one-key case of [`routes_to_any`], which it delegates to so the two cannot
/// disagree about what "the key names this service" means.
pub fn routes_to(parts: &http::request::Parts, signing_name: &str) -> bool {
    routes_to_any(parts, &[signing_name])
}

/// Whether the routing key the framework resolves for `parts` is one of `route_keys`.
///
/// [`routes_to`] over the several keys ONE endpoint answers on. A service's SigV4
/// signing name is not always the name its endpoint is addressed by: the framework
/// resolves the routing key from the credential scope first (the signing name), and
/// falls back to the `Host`/`:authority` label (the `endpointPrefix`) for a request
/// that carries no scope. Both name the same endpoint, so both are its keys.
pub fn routes_to_any(parts: &http::request::Parts, route_keys: &[&str]) -> bool {
    match resolve_route_key(parts) {
        Some(key) => route_keys
            .iter()
            .any(|route_key| key.eq_ignore_ascii_case(route_key.trim())),
        None => false,
    }
}

/// The claim rule for a service whose signing name is SHARED and whose peers' modelled
/// path spaces OVERLAP: the request's resolved routing key names this endpoint, and this
/// service's own model binds a route for the request.
///
/// * `route_keys` — the keys this endpoint answers on: its signing name
///   (`definition().identity().signing_name()`) and its `endpointPrefix`, which the model
///   states and a host-addressed request resolves to.
/// * `definition` — this service's model-derived half (`Service::definition()`), whose
///   protocol and routing table are the model's own statement of which requests this
///   service serves. The route half is [`addresses_modeled_route`], so route matching
///   stays in the one place that owns it.
///
/// This is [`claims_request`]'s rule with its path conjunct sharpened from "a first path
/// segment my model declares" to "a whole route my model binds", which is how AWS itself
/// dispatches a REST request once the endpoint host has chosen the service. Peers whose
/// first path segments collide are therefore still separated, as long as their models
/// bind no identical method + URI pattern; where two peers DO bind the same route, the
/// request is ambiguous on this rule alone and needs a further discriminator read off
/// the request (see [`addresses_runtime_bidirectional_stream`] and [`shared_path`] for
/// that shape).
///
/// A request this service's model does not bind is NOT claimed, even when no peer binds
/// it either: a path no emulated model publishes is the router's `404 UnknownService`,
/// which is honest, rather than one peer answering for an endpoint it does not serve.
pub fn claims_modeled_route(
    parts: &http::request::Parts,
    route_keys: &[&str],
    definition: &dyn ErasedServiceDefinition,
) -> bool {
    routes_to_any(parts, route_keys) && addresses_modeled_route(parts, definition)
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

/// Whether the service whose generated `ServiceDefinition` is `definition` binds a
/// modelled route for the request `parts` came from.
///
/// The LAST discriminator available to two peers under one signing name whose modelled
/// path spaces overlap and whose URIs do not differ at any single segment
/// ([`nth_path_segment`] is the cheaper reader for that case). It names nothing about
/// either service: the question it asks is exactly the one the router asks AFTER a
/// service claims a request — the service's own wire protocol resolves the request's
/// routing key (for a REST protocol, from the method and the URI patterns its model
/// binds; for `awsJson`, from `X-Amz-Target`; for `rpcv2Cbor`, from the URI), and the
/// model-derived routing table answers whether that key names one of this service's
/// operations. So a peer claims a path inside the space it shares only for the routes its
/// OWN model binds, and the framework's route matcher stays the single owner of route
/// matching.
///
/// `Service::can_handle` is handed the request's parts, while a protocol reads a whole
/// runtime-API request, so the three things a routing key is read from — the method, the
/// URI and the headers — are re-presented to it on an EMPTY body. No protocol that keys
/// on the body is affected: `awsQuery` and `ec2Query` route on the `Action` form field,
/// publish no `smithy.api#http` bindings at all, and are therefore separated by the
/// service their request NAMES rather than by a route (see [`claims_request`]).
///
/// `false` when the request cannot be rebuilt (a URI or method the runtime API rejects),
/// when the protocol resolves no routing key, and when the key it resolves names no
/// operation this service wires.
///
/// The rebuild mirrors the router's own `runtime_api_request_from_parts`
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`), which is private to that
/// agent-immutable crate: the URI, the method and every header value, `append`ed so a
/// repeated header keeps all of its values. The two must stay in step — the ONE
/// difference is deliberate, the empty body, since no routing key this is used for reads
/// one.
pub fn addresses_modeled_route(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
) -> bool {
    let mut request = RuntimeApiRequest::new(SdkBody::empty());
    if request.set_uri(parts.uri.to_string().as_str()).is_err() {
        return false;
    }
    if request.set_method(parts.method.as_str()).is_err() {
        return false;
    }
    for (name, value) in parts.headers.iter() {
        if let Ok(value) = value.to_str() {
            // `append`, not `insert`: `HeaderMap::iter` yields one entry per value, so a
            // repeated header keeps every one of them, as the router's own rebuild does.
            request
                .headers_mut()
                .append(name.as_str().to_string(), value.to_string());
        }
    }
    match definition.protocol().routing_key(&request) {
        Ok(key) => definition.routing_table().get(key).is_some(),
        Err(_) => false,
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

/// The `index`-th (0-based) `/`-separated segment of a URI path, or `None` when the path
/// has no such segment.
///
/// For two peers under one signing name whose modelled URIs share their first path
/// segment, so that [`claims_modeled_path`] cannot separate them: the segment their URIs
/// differ at decides instead. Leading `/` is ignored and empty segments are NOT skipped,
/// so the index is the position in the URI as written.
///
/// That last part is deliberately UNLIKE [`first_path_segment`], which maps an empty
/// first segment to `None` so that a ROOT path can be told from a named one. Here an
/// empty segment is reported as `Some("")`: a caller is comparing a deeper segment
/// against a literal its model binds, which is never empty, so an empty segment must
/// compare unequal rather than disappear.
pub fn nth_path_segment(path: &str, index: usize) -> Option<&str> {
    path.trim_start_matches('/').split('/').nth(index)
}

/// The index, in `/endpoints/{EndpointName}/<operation>`, of the segment the two
/// SageMaker Runtime endpoints' modelled URIs differ at.
const RUNTIME_INVOCATION_SEGMENT_INDEX: usize = 2;

/// That segment's value for the ONE operation the SageMaker Runtime HTTP2 endpoint binds.
///
/// Source: `smithy_query(query="digest")` on both models — HTTP2 binds only
/// `POST /endpoints/{EndpointName}/invocations-bidirectional-stream`, while SageMaker
/// Runtime binds `…/invocations`, `…/async-invocations` and
/// `…/invocations-response-stream` under the same prefix.
const RUNTIME_BIDIRECTIONAL_STREAM_SEGMENT: &str = "invocations-bidirectional-stream";

/// Whether `path` addresses the bidirectional-stream invocation — the one operation that
/// separates the two emulated SageMaker Runtime endpoints.
///
/// Those two share their SigV4 signing name, their `runtime.sagemaker` endpointPrefix AND
/// their first URI path segment (`endpoints`), so [`claims_request`] alone leaves both
/// claiming each other's traffic. The HTTP2 endpoint claims a path this answers `true`
/// for; its sibling claims the complement. Both sides read this ONE predicate rather
/// than a constant of their own, so the two claims cannot drift into zero or two
/// claimants for the space they share.
pub fn addresses_runtime_bidirectional_stream(path: &str) -> bool {
    nth_path_segment(path, RUNTIME_INVOCATION_SEGMENT_INDEX)
        == Some(RUNTIME_BIDIRECTIONAL_STREAM_SEGMENT)
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
    //! with disjoint path spaces, RPC peers that all post to one path and are
    //! separated only by the service they name, and two REST peers that share a path
    //! space and are separated by a deeper segment.

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

    /// The segment index is the position in the path as written, whatever the leading or
    /// trailing slashes — the reader two peers sharing a path space are separated by.
    #[test]
    fn nth_path_segment_reads_the_position_in_the_path() {
        // (case, path, index, expected)
        let rows: [(&str, &str, usize, Option<&str>); 9] = [
            ("first of three", "/endpoints/ep-1/invocations", 0, Some("endpoints")),
            ("second of three", "/endpoints/ep-1/invocations", 1, Some("ep-1")),
            ("third of three", "/endpoints/ep-1/invocations", 2, Some("invocations")),
            (
                "third of three, the sibling's operation",
                "/endpoints/ep-1/invocations-bidirectional-stream",
                2,
                Some("invocations-bidirectional-stream"),
            ),
            ("past the end", "/endpoints/ep-1", 2, None),
            ("no leading slash", "endpoints/ep-1/invocations", 2, Some("invocations")),
            ("trailing slash leaves an empty segment", "/endpoints/ep-1/", 2, Some("")),
            ("the root path", "/", 0, Some("")),
            ("the empty path", "", 0, Some("")),
        ];

        for (case, path, index, expected) in rows {
            let actual = nth_path_segment(path, index);
            assert_eq!(
                actual, expected,
                "case {case:?}: segment {index} of {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The one predicate both SageMaker Runtime endpoints read: true for exactly the
    /// invocation the HTTP2 one binds, false for every other path in the space they
    /// share, so the two complementary claims cannot leave a path with zero or two
    /// claimants.
    #[test]
    fn the_bidirectional_stream_invocation_is_told_from_every_other_invocation() {
        // (case, path, expected) — the four invocations the two models bind
        // (`smithy_query(query="digest")` on each), plus paths around them.
        let rows: [(&str, &str, bool); 8] = [
            (
                "the HTTP2 endpoint's own invocation",
                "/endpoints/ep-1/invocations-bidirectional-stream",
                true,
            ),
            ("the sibling's invocations", "/endpoints/ep-1/invocations", false),
            (
                "the sibling's async invocations",
                "/endpoints/ep-1/async-invocations",
                false,
            ),
            (
                "the sibling's response stream",
                "/endpoints/ep-1/invocations-response-stream",
                false,
            ),
            (
                "an invocation neither model binds",
                "/endpoints/ep-1/no-such-invocation",
                false,
            ),
            ("no invocation segment at all", "/endpoints/ep-1", false),
            (
                "the segment at the wrong depth",
                "/endpoints/invocations-bidirectional-stream",
                false,
            ),
            (
                "an endpoint name that looks like the segment",
                "/endpoints/invocations-bidirectional-stream/invocations",
                false,
            ),
        ];

        for (case, path, expected) in rows {
            let actual = addresses_runtime_bidirectional_stream(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path:?} expected {expected}, got {actual}"
            );
        }
    }

    /// The route half of [`claims_modeled_route`] is [`addresses_modeled_route`], which
    /// reads a service's OWN generated protocol and routing table; it is exercised
    /// end-to-end against the real models in
    /// `core/router/tests/all/bedrock_agentcore_signing_name_routing.rs` and the owning
    /// crates' own suites, where those tables exist. What is unit-testable here is the
    /// KEY half, below.
    #[allow(dead_code)]
    fn route_half_is_covered_by_the_router_suites() {}

    /// An endpoint answers on every key that names it: its signing name, and the
    /// `endpointPrefix` an unsigned, host-addressed request resolves to.
    #[test]
    fn an_endpoint_answers_on_its_signing_name_and_on_its_endpoint_prefix() {
        /// The keys ONE endpoint answers on: the signing name it shares with its peer,
        /// and its own (longer) endpoint prefix.
        const KEYS: &[&str] = &["shared", "shared-control"];

        // (case, authorization scope, host, expected)
        let rows: [(&str, Option<&str>, Option<&str>, bool); 7] = [
            ("the shared signing name, signed", Some("shared"), None, true),
            (
                "its own endpoint prefix, signed for it",
                Some("shared-control"),
                None,
                true,
            ),
            (
                "its own endpoint prefix, host-addressed and unsigned",
                None,
                Some("shared-control.us-east-1.amazonaws.com"),
                true,
            ),
            (
                "the shared signing name as a host, unsigned",
                None,
                Some("shared.us-east-1.amazonaws.com"),
                true,
            ),
            (
                "another service's host, unsigned",
                None,
                Some("lambda.us-east-1.amazonaws.com"),
                false,
            ),
            (
                // The credential scope outranks the host, so a request signed for
                // another service does not reach this endpoint however it is addressed.
                "another service's scope on this endpoint's host",
                Some("lambda"),
                Some("shared-control.us-east-1.amazonaws.com"),
                false,
            ),
            ("no routing signal at all", None, None, false),
        ];

        for (case, scope, host, expected) in rows {
            let mut builder = http::Request::builder().method("POST").uri("/widgets/");
            if let Some(scope) = scope {
                builder = builder.header("authorization", authorization(scope));
            }
            if let Some(host) = host {
                builder = builder.header("host", host);
            }
            let parts = builder
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0;
            let actual = routes_to_any(&parts, KEYS);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected routes_to_any == {expected}, got {actual}"
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
