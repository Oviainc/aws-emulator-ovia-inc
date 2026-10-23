// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACME HTTP surface an ACM ACME endpoint publishes: the URL
//! `AcmeEndpoint$EndpointUrl` advertises, and what answers at it.
//!
//! RULE 1: a server-managed URL this service hands a client is minted on the host the
//! service PUBLISHES for that Region — for an ACME endpoint the ACME enrollment host,
//! `acm-acme-enroll.<region>.<dns suffix>` — with the resource's own id in the PATH,
//! never as a DNS label of a host form the service does not publish.
//!
//! RULE 2: a URL this service advertises is a URL it SERVES. A plain `GET` of the
//! `EndpointUrl` of an `ACTIVE` endpoint answers the RFC 8555 §7.1.1 directory
//! resource, whose members are absolute URLs under that same endpoint's base; the
//! `newNonce` resource answers a fresh anti-replay nonce (§7.2); an interaction this
//! emulator cannot perform answers an ACME problem document rather than a success; and
//! a URL no `ACTIVE` endpoint backs answers nothing at all.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every URL fetched is one a DIFFERENT
//! operation (`CreateAcmeEndpoint`, then `DescribeAcmeEndpoint`) advertised, so an
//! answer that is not computed from the persisted endpoint record cannot pass.
//!
//! The tables below span what the rules generalise over: the Region (five, including
//! one of each AWS partition-style name shape), the endpoint (several per Region, so
//! one host serves many), the resource the directory names, and the HTTP method.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{call_in_region, router_with_engine, ACCOUNT};

/// The leading label of the host ACM serves a Region's managed ACME endpoints on.
///
/// Written out here rather than read from the service, so this test pins the published
/// host FORM: the recorded behaviour grades `AcmeEndpoint$EndpointUrl` as having to be
/// on `acm-acme-enroll.<region>.<domain>`, and an ACME client is configured with that
/// URL as its ACME server.
const ENROLLMENT_LABEL: &str = "acm-acme-enroll";

/// The DNS suffix of the AWS endpoint hosts this emulator publishes.
const DNS_SUFFIX: &str = "amazonaws.com";

/// The Regions the table spans: the recorded one, one in each of three other
/// geographies, and one whose name carries three hyphen-separated parts — so the host
/// assertion holds for every Region-name shape an ACM ARN may carry
/// (`AcmeEndpointArn`'s `@pattern` spells the Region `[a-z0-9-]+`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "eu-west-1",
    "ap-southeast-2",
    "me-central-1",
    "us-gov-west-1",
];

/// Every member RFC 8555 §7.1.1 requires of the directory object, beside the resource
/// path this service mints its URL from.
const DIRECTORY_MEMBERS: &[(&str, &str)] = &[
    ("newNonce", "new-nonce"),
    ("newAccount", "new-account"),
    ("newOrder", "new-order"),
    ("revokeCert", "revoke-cert"),
    ("keyChange", "key-change"),
];

/// A `CreateAcmeEndpoint` request body that carries caller-chosen text in every member
/// that takes any, so a URL derived from the request could not escape notice.
fn create_body(marker: &str) -> Value {
    json!({
        "AuthorizationBehavior": "PRE_APPROVED",
        "Contact": "REQUIRED",
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
        "Tags": [{ "Key": "marker", "Value": marker }],
        "CertificateTags": [{ "Key": "cert-marker", "Value": marker }],
    })
}

/// Create one ACME endpoint in `region` and return its ARN.
async fn create_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    marker: &str,
) -> String {
    let (status, response) =
        call_in_region(router, region, "CreateAcmeEndpoint", &create_body(marker)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAcmeEndpoint returned no AcmeEndpointArn"))
        .to_string()
}

/// `DescribeAcmeEndpoint`'s `AcmeEndpoint` member for `arn`.
async fn describe_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) -> Value {
    let (status, response) = call_in_region(
        router,
        region,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeEndpoint({arn}) must succeed, got {status} {response}"
    );
    response["AcmeEndpoint"].clone()
}

/// The `EndpointUrl` `DescribeAcmeEndpoint` reports for `arn`.
async fn endpoint_url(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) -> String {
    let endpoint = describe_endpoint(router, case, region, arn).await;
    endpoint["EndpointUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the endpoint carried no EndpointUrl: {endpoint}"))
        .to_string()
}

/// The endpoint id of an ACME endpoint ARN: the part after `acme-endpoint/`.
fn endpoint_id(arn: &str) -> &str {
    arn.rsplit_once("acme-endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an ACME endpoint ARN"))
        .1
}

/// The `(host, path)` an `https://` URL addresses.
fn split_url(case: &str, url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: expected an https URL, got {url:?}"));
    match rest.find('/') {
        Some(path_start) => (
            rest[..path_start].to_string(),
            rest[path_start..].to_string(),
        ),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// Fetch `url` from the emulator with `method`, exactly as a client holding that URL
/// would: the URL's own host in the `Host` header, its path as the request target, and
/// NO credential of any kind — an ACME client has none.
async fn fetch(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    url: &str,
) -> ::http::Response<Bytes> {
    let (host, path) = split_url(case, url);
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("a valid request");
    router.dispatch(request).await
}

/// The response body as JSON, or `Value::Null` when it is empty.
fn body_json(case: &str, response: &::http::Response<Bytes>) -> Value {
    let bytes = response.body();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected a JSON body, got {error} for {:?}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// One response header, as text.
fn header(response: &::http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Make the activation every stored ACME endpoint is still owed fall due, so the next
/// request materialises `ACTIVE` without this test waiting out
/// [`state::ACME_ENDPOINT_ACTIVATION_DELAY`] on the wall clock.
///
/// Writes only the due instant the emulator itself stores: the transition is still
/// applied by `materialize_due` on the next request, exactly as in production.
fn activate_endpoints_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for endpoint in store.acme_endpoints.values_mut() {
        if let Some(pending) = endpoint.pending_status.as_mut() {
            pending.due_at = past;
        }
    }
}

/// Hold every stored ACME endpoint in `CREATING` for the rest of the test by moving the
/// activation it is owed an hour out.
fn hold_endpoints_creating(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let held = SystemTime::now() + Duration::from_secs(3600);
    for endpoint in store.acme_endpoints.values_mut() {
        if let Some(pending) = endpoint.pending_status.as_mut() {
            pending.due_at = held;
        }
    }
}

/// RULE 1: the `EndpointUrl` of every endpoint, in every Region, is minted on that
/// Region's ACME enrollment host with the endpoint's own id in the path — unique per
/// endpoint, stable across reads, and carrying nothing the caller sent.
#[tokio::test]
async fn endpoint_url_is_minted_on_the_region_acme_enrollment_host() {
    let (router, _engine) = router_with_engine();
    let mut seen = BTreeSet::new();

    for region in REGIONS {
        // Two endpoints per Region, created from identical requests: one host serves
        // many endpoints, so what distinguishes their URLs must be the path.
        for attempt in 0..2 {
            let case = format!("{region} endpoint #{attempt}");
            let marker = "client-supplied-marker";
            let arn = create_endpoint(&router, &case, region, marker).await;
            let url = endpoint_url(&router, &case, region, &arn).await;
            let (host, path) = split_url(&case, &url);

            // The host is the Region's enrollment host, label by label: the first
            // label is the enrollment label, the second is the Region, and at least
            // one non-empty label follows.
            let labels: Vec<&str> = host.split('.').collect();
            assert!(
                labels.len() > 2 && labels.iter().all(|label| !label.is_empty()),
                "case {case:?}: expected a host of at least three non-empty labels, got {host:?} from {url:?}"
            );
            assert_eq!(
                &labels[..2],
                &[ENROLLMENT_LABEL, *region],
                "case {case:?}: expected {url:?} to be on {ENROLLMENT_LABEL}.{region}.<domain>, got host {host:?}"
            );
            assert_eq!(
                host,
                format!("{ENROLLMENT_LABEL}.{region}.{DNS_SUFFIX}"),
                "case {case:?}: expected the Region's enrollment host, got {host:?}"
            );

            // The endpoint's own id identifies it in the PATH, not as a host label.
            let id = endpoint_id(&arn);
            assert!(
                path.contains(id),
                "case {case:?}: expected the endpoint id {id} in the path of {url:?}"
            );
            assert!(
                !host.contains(id),
                "case {case:?}: the endpoint id must not be a host label, got {host:?}"
            );
            assert!(
                path.ends_with("/directory"),
                "case {case:?}: expected the directory resource at the end of {url:?}"
            );

            // Server-managed: nothing the caller sent appears anywhere in it.
            for sent in [marker, "marker", "cert-marker", ACCOUNT] {
                assert!(
                    !url.contains(sent),
                    "case {case:?}: a server-managed URL must carry nothing the caller sent, but {url:?} contains {sent:?}"
                );
            }

            // Unique per endpoint, and identical on every later read.
            assert!(
                seen.insert(url.clone()),
                "case {case:?}: every endpoint must get its own URL, but {url} was served twice"
            );
            let reread = endpoint_url(&router, &case, region, &arn).await;
            assert_eq!(
                reread, url,
                "case {case:?}: the URL must be identical on every read, got {reread:?}"
            );
        }
    }
}

/// RULE 2: a plain `GET` of the URL the service just advertised answers the RFC 8555
/// directory, whose five required members are absolute URLs under that endpoint's own
/// base — and fetching it changes nothing a later caller sees.
#[tokio::test]
async fn the_advertised_endpoint_url_serves_the_acme_directory() {
    let (router, engine) = router_with_engine();

    for region in REGIONS {
        let case = format!("{region} directory");
        let arn = create_endpoint(&router, &case, region, "marker-value").await;
        activate_endpoints_now(&engine);
        let url = endpoint_url(&router, &case, region, &arn).await;
        let before = describe_endpoint(&router, &case, region, &arn).await;

        let response = fetch(&router, &case, "GET", &url).await;
        let status = response.status().as_u16();
        let document = body_json(&case, &response);
        assert_eq!(
            status, 200,
            "case {case:?}: the ACME directory at {url} must be served, got {status} {document}"
        );
        assert_eq!(
            header(&response, "content-type"),
            "application/json",
            "case {case:?}: expected a JSON directory object, got {document}"
        );

        // The base every resource of this endpoint hangs off: the advertised URL
        // without its `directory` segment.
        let base = url
            .strip_suffix("/directory")
            .unwrap_or_else(|| panic!("case {case:?}: {url} is not a directory URL"));
        for (member, resource) in DIRECTORY_MEMBERS {
            assert_eq!(
                document[*member].as_str().unwrap_or_default(),
                format!("{base}/{resource}"),
                "case {case:?}: expected {member} under {base}, got {document}"
            );
        }

        // Reading the directory must not change what a later caller sees.
        let after = describe_endpoint(&router, &case, region, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: fetching the directory must not change the endpoint record, got {after}"
        );
        let again = fetch(&router, &case, "GET", &url).await;
        assert_eq!(
            body_json(&case, &again),
            document,
            "case {case:?}: a second fetch must answer the same directory"
        );
    }
}

/// RULE 2: the `newNonce` resource the directory names answers a FRESH anti-replay
/// nonce — `200` to a `HEAD` and `204` to a `GET` (RFC 8555 §7.2), never the same nonce
/// twice, each spelled in the base64url alphabet §6.5.1 requires.
#[tokio::test]
async fn new_nonce_answers_a_fresh_nonce_per_request() {
    let (router, engine) = router_with_engine();
    let case = "new-nonce";
    let region = REGIONS[0];
    let arn = create_endpoint(&router, case, region, "marker-value").await;
    activate_endpoints_now(&engine);
    let url = endpoint_url(&router, case, region, &arn).await;
    let directory = body_json(case, &fetch(&router, case, "GET", &url).await);
    let nonce_url = directory["newNonce"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no newNonce in {directory}"))
        .to_string();

    // The table: both methods RFC 8555 §7.2 fixes an answer for, several times each, so
    // freshness is asserted across methods and across repeats.
    let mut nonces = BTreeSet::new();
    for attempt in 0..4 {
        for (method, expected) in [("HEAD", 200), ("GET", 204)] {
            let case = format!("{method} {nonce_url} #{attempt}");
            let response = fetch(&router, &case, method, &nonce_url).await;
            let status = response.status().as_u16();
            assert_eq!(
                status, expected,
                "case {case:?}: expected {expected}, got {status} {:?}",
                body_json(&case, &response)
            );
            assert_eq!(
                header(&response, "cache-control"),
                "no-store",
                "case {case:?}: a newNonce answer must not be cached"
            );
            let nonce = header(&response, "replay-nonce");
            assert!(
                !nonce.is_empty(),
                "case {case:?}: expected a Replay-Nonce header"
            );
            assert!(
                nonce
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "case {case:?}: expected a base64url nonce, got {nonce:?}"
            );
            assert!(
                nonces.insert(nonce.clone()),
                "case {case:?}: a nonce must never be answered twice, but {nonce} was"
            );
        }
    }
}

/// RULE 2: an ACME interaction this emulator cannot perform is refused with an ACME
/// problem document — `501`, never a success — and a method a resource is not addressed
/// with is refused `405` naming the method it is.
#[tokio::test]
async fn unimplemented_acme_interactions_answer_problem_documents() {
    let (router, engine) = router_with_engine();
    let region = REGIONS[0];
    let arn = create_endpoint(&router, "jws resources", region, "marker-value").await;
    activate_endpoints_now(&engine);
    let url = endpoint_url(&router, "jws resources", region, &arn).await;
    let base = url
        .strip_suffix("/directory")
        .expect("the advertised URL ends at the directory resource")
        .to_string();
    let directory = body_json("jws resources", &fetch(&router, "jws", "GET", &url).await);

    // The table: every resource the directory names that is reached with a JWS POST,
    // plus an account's own URL, which is reached the same way.
    let mut targets: Vec<String> = DIRECTORY_MEMBERS
        .iter()
        .filter(|(member, _)| *member != "newNonce")
        .map(|(member, _)| {
            directory[*member]
                .as_str()
                .unwrap_or_else(|| panic!("no {member} in {directory}"))
                .to_string()
        })
        .collect();
    targets.push(format!("{base}/acct/1"));

    for target in &targets {
        let case = format!("POST {target}");
        let response = fetch(&router, &case, "POST", target).await;
        let status = response.status().as_u16();
        let document = body_json(&case, &response);
        assert_eq!(
            status, 501,
            "case {case:?}: an interaction nothing performs must not succeed, got {status} {document}"
        );
        assert_eq!(
            header(&response, "content-type"),
            "application/problem+json",
            "case {case:?}: expected an ACME problem document, got {document}"
        );
        assert!(
            document["type"]
                .as_str()
                .unwrap_or_default()
                .starts_with("urn:ietf:params:acme:error:"),
            "case {case:?}: expected an ACME error type, got {document}"
        );

        // The same resource reached with the wrong method names the right one.
        let case = format!("GET {target}");
        let response = fetch(&router, &case, "GET", target).await;
        let status = response.status().as_u16();
        assert_eq!(
            status, 405,
            "case {case:?}: a JWS resource is addressed with POST, got {status} {}",
            body_json(&case, &response)
        );
        assert_eq!(
            header(&response, "allow"),
            "POST",
            "case {case:?}: a 405 must name the methods the resource takes"
        );
    }

    // And the directory itself, which is the one resource fetched with a plain GET.
    for method in ["POST", "PUT", "DELETE"] {
        let case = format!("{method} {url}");
        let response = fetch(&router, &case, method, &url).await;
        let status = response.status().as_u16();
        assert_eq!(
            status, 405,
            "case {case:?}: the directory is fetched with GET, got {status} {}",
            body_json(&case, &response)
        );
    }
}

/// RULE 2: nothing is served at a URL no `ACTIVE` endpoint of that Region backs — an
/// unknown endpoint id, another Region's endpoint, an endpoint still `CREATING`, a
/// resource no endpoint publishes, and a path that is not an ACME path at all.
#[tokio::test]
async fn no_acme_resource_is_served_without_an_active_endpoint_behind_it() {
    let (router, engine) = router_with_engine();
    let region = REGIONS[0];
    let other_region = REGIONS[1];

    let active = create_endpoint(&router, "active", region, "marker-value").await;
    let elsewhere = create_endpoint(&router, "elsewhere", other_region, "marker-value").await;
    activate_endpoints_now(&engine);
    let active_url = endpoint_url(&router, "active", region, &active).await;
    let base = active_url
        .strip_suffix("/directory")
        .expect("the advertised URL ends at the directory resource")
        .to_string();
    let host = format!("{ENROLLMENT_LABEL}.{region}.{DNS_SUFFIX}");

    // The table: what distinguishes each row is WHY no active endpoint of this Region's
    // host answers for it.
    let cases: Vec<(&str, String)> = vec![
        (
            "an endpoint id no record answers to",
            format!("https://{host}/acme/00000000-0000-4000-8000-000000000000/directory"),
        ),
        (
            "another Region's endpoint on this Region's host",
            format!("https://{host}/acme/{}/directory", endpoint_id(&elsewhere)),
        ),
        (
            "a resource no endpoint publishes",
            format!("{base}/not-a-resource"),
        ),
        ("a path that is not an ACME path", format!("https://{host}/")),
        (
            "an endpoint id with no resource under it",
            format!("https://{host}/acme/{}", endpoint_id(&active)),
        ),
    ];
    for (case, url) in &cases {
        let response = fetch(&router, case, "GET", url).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 404,
            "case {case:?}: {url} must not be served, got {status} {document}"
        );
        assert_eq!(
            header(&response, "content-type"),
            "application/problem+json",
            "case {case:?}: expected an ACME problem document, got {document}"
        );
    }

    // And the same URL while its endpoint is still `CREATING`: "after creation, ACME
    // clients can use the endpoint URL", so before that there is nothing there.
    let (router, engine) = router_with_engine();
    let case = "an endpoint that is still CREATING";
    let creating = create_endpoint(&router, case, region, "marker-value").await;
    hold_endpoints_creating(&engine);
    let url = endpoint_url(&router, case, region, &creating).await;
    let response = fetch(&router, case, "GET", &url).await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 404,
        "case {case:?}: {url} must serve nothing yet, got {status} {}",
        body_json(case, &response)
    );
    // ...and once the activation the clock owed it falls due, the very same URL serves.
    activate_endpoints_now(&engine);
    let response = fetch(&router, case, "GET", &url).await;
    let status = response.status().as_u16();
    assert_eq!(
        status, 200,
        "case {case:?}: once ACTIVE, {url} must serve the directory, got {status} {}",
        body_json(case, &response)
    );
}

/// RULE 1: the ACME surface is published on the enrollment host and nowhere else — the
/// control-plane host does not serve it, so the one address a client is given is the one
/// that answers.
#[tokio::test]
async fn the_acme_surface_is_not_served_on_the_control_plane_host() {
    let (router, engine) = router_with_engine();
    let region = REGIONS[0];
    let arn = create_endpoint(&router, "control plane host", region, "marker-value").await;
    activate_endpoints_now(&engine);
    let url = endpoint_url(&router, "control plane host", region, &arn).await;
    let (_host, path) = split_url("control plane host", &url);

    let case = "the same path on the control-plane host";
    let control_plane = format!("https://{}.{region}.{DNS_SUFFIX}{path}", state::SIGNING_NAME);
    let response = fetch(&router, case, "GET", &control_plane).await;
    let status = response.status().as_u16();
    assert_ne!(
        status, 200,
        "case {case:?}: the ACME surface is published on the enrollment host only, but {control_plane} answered {status} {}",
        body_json(case, &response)
    );
}
