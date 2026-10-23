// SPDX-License-Identifier: CC0-1.0

//! The MediaPackageV2 data-plane hostnames, on the PRODUCTION router.
//!
//! The control plane hands encoders and players live URLs — `IngestEndpoints[].Url` on a
//! channel, the manifest `Url` on an origin endpoint — whose FIRST label is an opaque
//! digest rather than a service name. The router's route key is that first label
//! (`emulator_server::resolve_route_key`), so nothing in the key can attribute such a
//! request to a service: unless the publishing service claims the request by its `Host`,
//! the assembled router finds zero claimants and answers its own `404 UnknownService`,
//! which is a protocol-level "no such service" where AWS gives an authorization verdict.
//!
//! The rule this suite pins: a request addressed to a hostname mediapackagev2 published
//! reaches mediapackagev2 on the ASSEMBLED router — exactly one service claims it, so the
//! answer is neither `404 UnknownService` (none claimed) nor `500 AmbiguousService`
//! (several did) — and is answered with an authorization decision: an unauthenticated
//! ingest push is `403` with an access-denied body, and an anonymous egress fetch is `403`
//! until the origin endpoint's resource policy grants the action, `404` after.
//!
//! Every other service in the emulator is mounted beside it here, which is what the
//! service's own suites cannot show: `services/service-com-amazonaws-mediapackagev2-\
//! mediapackagev2/tests/all/published_data_plane_urls.rs` and its siblings run a router
//! holding only mediapackagev2, where no other service could claim or decline anything.
//! The rows differ along the published URL (each ingest endpoint of the pair, the manifest)
//! and the grant state, not along the resource names.
//!
//! Assembling the router costs stack, so the cases run on an explicitly sized thread,
//! exactly as `boot_serve` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Stack for assembling the router, matching `boot_serve`'s.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region the fixtures are created in; the service reads it from the credential scope,
/// and the hostnames it publishes carry it.
const REGION: &str = "us-west-2";

/// The names the fixtures are created under. Nothing in the rule depends on them.
const GROUP: &str = "rtr-group";
const CHANNEL: &str = "rtr-channel";
const ENDPOINT: &str = "rtr-endpoint";

/// A SigV4 `Authorization` header scoped to mediapackagev2 — what an SDK sends for a
/// CONTROL-plane call, and the signal the router resolves its route key from. Signature
/// verification is off in this test process, so only the scope matters.
const CONTROL_PLANE_SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-west-2/mediapackagev2/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The router's own refusals. A data-plane request may answer NEITHER: one says no service
/// claimed the hostname, the other that several did, and both are routing failures rather
/// than the authorization verdict AWS gives.
const ROUTER_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// One HTTP answer, reduced to what a caller branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: String,
}

/// Send one request through the assembled router and reduce the answer.
///
/// `authorization` is `None` for a data-plane row: an anonymous player and an encoder that
/// was never given credentials send no credential at all, which is also what leaves the
/// `Host` header as the only signal that can name a service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    host: &str,
    authorization: Option<&str>,
    payload: &[u8],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host);
    if let Some(authorization) = authorization {
        builder = builder
            .header("authorization", authorization)
            .header("content-type", "application/json");
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    Answer { status, body }
}

/// One control-plane call: the control-plane host, signed with the scope above.
async fn control(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> Value {
    let payload = body.to_string();
    let answer = send(
        router,
        method,
        path,
        &format!("mediapackagev2.{REGION}.amazonaws.com"),
        Some(CONTROL_PLANE_SIGV4),
        payload.as_bytes(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the control-plane call {method} {path} must succeed, got {} with body {}",
        answer.status, answer.body
    );
    serde_json::from_str(&answer.body).unwrap_or_else(|error| {
        panic!(
            "the control-plane call {method} {path} answered 200 with a body that is not JSON: \
             {error}: {}",
            answer.body
        )
    })
}

/// Split a published `https://host/path` URL into its host and its path.
fn split_url(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("a published URL is https, got {url:?}"));
    match rest.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// The first dot-separated label of a hostname — the router's whole route key for a
/// host-routed request.
fn first_label(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

/// Assert this answer came from a service rather than from the router refusing to route it.
fn assert_not_a_router_refusal(case: &str, answer: &Answer) {
    for refusal in ROUTER_REFUSALS {
        assert!(
            !answer.body.contains(refusal),
            "case {case:?}: a published data-plane URL must be claimed by exactly one mounted \
             service, so the answer must not be the router's {refusal}, got {} with body {}",
            answer.status,
            answer.body
        );
    }
}

/// On the assembled router, every URL the MediaPackageV2 control plane publishes is claimed
/// by MediaPackageV2 and answered with an authorization decision.
#[test]
fn published_data_plane_urls_reach_mediapackagev2_on_the_assembled_router() {
    std::thread::Builder::new()
        .name("mediapackagev2-data-plane".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let router = build_router(engine);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a runtime");
            runtime.block_on(assert_the_data_plane_is_claimed_and_authorizes(&router));
        })
        .expect("spawn the routing thread")
        .join()
        .expect("the routing thread must not panic");
}

/// The body of the case above, run on the sized thread's runtime.
async fn assert_the_data_plane_is_claimed_and_authorizes(router: &MultiServiceRouter) {
    let group = control(
        router,
        "POST",
        "/channelGroup",
        json!({"ChannelGroupName": GROUP}),
    )
    .await;
    let egress_domain = group["EgressDomain"]
        .as_str()
        .expect("the group publishes an egress domain")
        .to_string();
    let channel = control(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel"),
        json!({"ChannelName": CHANNEL, "InputType": "HLS"}),
    )
    .await;
    let endpoint = control(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
        json!({
            "OriginEndpointName": ENDPOINT,
            "ContainerType": "TS",
            "HlsManifests": [{"ManifestName": "index"}],
        }),
    )
    .await;

    // --- the ingest half: each published endpoint of the redundant pair ---------------
    let ingest_endpoints = channel["IngestEndpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !ingest_endpoints.is_empty(),
        "a channel must publish ingest endpoints, got {channel}"
    );
    for (index, ingest_endpoint) in ingest_endpoints.iter().enumerate() {
        let url = ingest_endpoint["Url"]
            .as_str()
            .unwrap_or_else(|| panic!("ingest endpoint {index} publishes no Url: {channel}"));
        let (host, path) = split_url(url);
        let case = format!("an unauthenticated PUT to ingest endpoint {index}");
        // The premise of the rule: the route key cannot name a service here, so only the
        // service's own claim on the hostname can route this request.
        assert_ne!(
            first_label(&host),
            "mediapackagev2",
            "case {case:?}: a published ingest hostname's first label is opaque, so this suite \
             would not be testing a host-based claim if it named the service, got {host}"
        );
        let answer = send(
            router,
            "PUT",
            &format!("{path}/0.mp4"),
            &host,
            None,
            b"segment bytes",
        )
        .await;
        assert_not_a_router_refusal(&case, &answer);
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.body.to_lowercase().contains("access denied"),
            "case {case:?}: the refusal must say access denied, got {}",
            answer.body
        );
    }

    // --- the egress half: the manifest URL, before and after the grant ----------------
    let manifest_url = endpoint["HlsManifests"][0]["Url"]
        .as_str()
        .expect("the origin endpoint publishes a manifest URL")
        .to_string();
    let (egress_host, egress_path) = split_url(&manifest_url);
    assert_eq!(
        egress_host, egress_domain,
        "the manifest URL must be served from the group's published egress domain, got \
         {manifest_url}"
    );
    assert_ne!(
        first_label(&egress_host),
        "mediapackagev2",
        "a published egress hostname's first label is opaque, so this suite would not be \
         testing a host-based claim if it named the service, got {egress_host}"
    );

    let case = "an anonymous manifest GET with no policy attached";
    let answer = send(router, "GET", &egress_path, &egress_host, None, b"").await;
    assert_not_a_router_refusal(case, &answer);
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.body.to_lowercase().contains("access denied"),
        "case {case:?}: the refusal must say access denied, got {}",
        answer.body
    );

    control(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy"),
        json!({"Policy": json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "Action": "mediapackagev2:GetObject",
                "Resource": endpoint["Arn"].as_str().unwrap_or_default(),
            }],
        })
        .to_string()}),
    )
    .await;

    let case = "an anonymous manifest GET once GetObject is granted to everyone";
    let answer = send(router, "GET", &egress_path, &egress_host, None, b"").await;
    assert_not_a_router_refusal(case, &answer);
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404 (authorized, nothing packaged), got {} with body {}",
        answer.status, answer.body
    );
}
