// SPDX-License-Identifier: CC0-1.0

//! The public address a Lightsail container service announces, and what answers
//! there.
//!
//! The rule under test: `ContainerService$url` is "The publicly accessible URL of
//! the container service", so it must be an address the caller that read it can
//! actually reach — which, for an emulator that owns no DNS zone, is the endpoint
//! the request arrived at and nothing else. The region, the service name and the
//! per-service token AWS puts in the hostname's labels are carried in the path
//! instead, and the emulator serves that path: the modelled `404` for a service
//! whose current deployment declares no public endpoint ("If no public endpoint is
//! specified in the currentDeployment, this URL returns a 404 response"), and `501`
//! where one is declared, because this emulator runs no container runtime and the
//! deployed image's own response is not something it can produce.
//!
//! Every case goes through the multi-service router, so the control-plane calls are
//! claimed, routed and serialized exactly as a client's are, and the public-URL
//! fetches are unsigned plain HTTP requests exactly as a client following that URL
//! makes them. The harness here is its own (rather than
//! `container_service_lifecycle`'s): these cases vary the endpoint host per request
//! and send raw non-AWS requests, neither of which that suite's fixed-host AWS
//! caller does.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The path space a container service's public URL is published under
/// (`crate::state::CONTAINER_SERVICE_PUBLIC_PATH_PREFIX`).
const PUBLIC_PATH_PREFIX: &str = "/lightsail-container-service";

/// The endpoint most cases below reach the emulator on: the loopback address and
/// port the emulator's own listener binds (`emulator_server`'s default data port).
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The hostname AWS publishes container-service URLs under, which this emulator
/// must NEVER announce: it is a public DNS name this account does not own, so a
/// client that follows it leaves the emulator entirely.
const AWS_PUBLIC_SUFFIX: &str = "cs.amazonlightsail.com";

/// The content type a container service's public endpoint answers with, which is
/// its own (it is not an AWS API) — so an answer carrying it came from this
/// service and not from the router's `UnknownService` refusal.
const ENDPOINT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the `content-type`, the declared
/// `content-length`, the `x-amzn-errortype` an AWS error envelope carries, and the
/// body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    content_type: Option<String>,
    content_length: Option<String>,
    error_type: Option<String>,
    body: String,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: a control-plane call routes by `X-Amz-Target`
        // and names its region in the credential SCOPE alone, and a fetch of a
        // published public URL is unsigned by construction.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region`, reaching the emulator at `host` (or naming no
    /// host at all when it is `None`), answering the HTTP status and parsed body.
    async fn call(
        &self,
        host: Option<&str>,
        region: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create the container service `body` describes (HTTP 201, the modelled
    /// success status), answering the `containerService` it reports.
    async fn create(&self, host: Option<&str>, region: &str, body: Value) -> Value {
        let (status, parsed) = self
            .call(host, region, "CreateContainerService", body)
            .await;
        assert_eq!(
            status, 201,
            "case creating in {region} at {host:?}: expected HTTP 201, got {status} {parsed:?}"
        );
        parsed["containerService"].clone()
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it to a caller reaching the emulator at `host`.
    async fn read(&self, host: Option<&str>, region: &str, name: &str) -> Value {
        let (status, parsed) = self
            .call(
                host,
                region,
                "GetContainerServices",
                json!({"serviceName": name}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case reading {name} in {region} at {host:?}: expected HTTP 200, got {status} \
             {parsed:?}"
        );
        parsed["containerServices"][0].clone()
    }

    /// Follow a published public URL: `method` on `path`, reaching the emulator at
    /// `host`, unsigned and with no AWS header of any kind.
    async fn fetch(&self, method: &str, host: &str, path: &str) -> Answer {
        let request = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .body(bytes::Bytes::new())
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        Answer {
            status: response.status().as_u16(),
            content_type: header("content-type"),
            content_length: header("content-length"),
            error_type: header("x-amzn-errortype"),
            body: String::from_utf8_lossy(response.body()).to_string(),
        }
    }
}

/// Poll `GetContainerServices` until the service `name` reports `want`, answering
/// the whole record. The transition is owed by the clock, not by the poll.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let service = emulator.read(Some(ENDPOINT_HOST), region, name).await;
        if service["state"].as_str() == Some(want) {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            service["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The URL `service` announces, required to be present.
fn announced_url(case: &str, service: &Value) -> String {
    service["url"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an announced url, got {service:?}"))
        .to_string()
}

/// The path of a `https://<host><path>` URL announced for `host`, which is what a
/// client following that URL sends as its request target.
fn path_of(case: &str, url: &str, host: &str) -> String {
    url.strip_prefix(&format!("https://{host}"))
        .unwrap_or_else(|| panic!("case {case}: expected a url on '{host}', got {url:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Where the URL points
// ---------------------------------------------------------------------------

/// RULE: the announced URL is on the endpoint the request reached the emulator on
/// — whatever that endpoint is — and never on a hostname the emulator does not
/// answer; the rest of the URL carries the service's region, name and own token,
/// so two services never share one.
///
/// The rows differ along the FORM of the endpoint a client can reach an emulator
/// at: a loopback IP literal with the default port and with another, an IPv6
/// literal, `localhost`, an AWS-style endpoint host, and a bare internal name.
#[tokio::test]
async fn the_announced_url_is_on_the_endpoint_the_request_arrived_at() {
    // (case, endpoint host, region)
    let rows: [(&str, &str, &str); 6] = [
        ("the loopback IP and the default port", "127.0.0.1:9297", "us-east-1"),
        ("the loopback IP on another port", "127.0.0.1:34567", "us-west-2"),
        ("an IPv6 loopback literal", "[::1]:9297", "eu-west-1"),
        ("localhost with a port", "localhost:8443", "ap-northeast-1"),
        (
            "an AWS-style endpoint host",
            "lightsail.us-west-2.amazonaws.com",
            "us-west-2",
        ),
        ("a bare internal hostname", "emulator.internal", "sa-east-1"),
    ];

    let emulator = Emulator::new();
    let mut announced: Vec<String> = Vec::new();
    for (index, (case, host, region)) in rows.into_iter().enumerate() {
        let name = format!("endpoint-{index}");
        let created = emulator
            .create(
                Some(host),
                region,
                json!({"serviceName": name, "power": "nano", "scale": 1}),
            )
            .await;
        let url = announced_url(case, &created);
        let prefix = format!("https://{host}{PUBLIC_PATH_PREFIX}/{region}/{name}/");
        let token = url.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected a url '{prefix}<token>', got {url:?}")
        });
        assert!(
            !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected a non-empty alphanumeric service token, got {token:?}"
        );
        assert!(
            !url.contains(AWS_PUBLIC_SUFFIX),
            "case {case:?}: expected no real-AWS hostname in the announced url, got {url:?}"
        );

        // The read announces the same address for the same endpoint: the URL is a
        // function of the record and the endpoint, not of the call that made it.
        let read = emulator.read(Some(host), region, &name).await;
        assert_eq!(
            announced_url(case, &read),
            url,
            "case {case:?}: expected the read to announce the created url"
        );
        announced.push(url);
    }

    let mut unique = announced.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        announced.len(),
        "case every row: expected one url per service, got {announced:?}"
    );
}

/// RULE: the URL follows the endpoint of the request that READS it — one record
/// read through two endpoints announces each caller's own address, because each
/// caller can only reach the emulator at the one it used. Everything else about
/// the URL (the region, the name, the service's own token) is the record's and so
/// is identical across those reads.
#[tokio::test]
async fn one_record_announces_whichever_endpoint_reads_it() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let name = "two-endpoints";
    let created = emulator
        .create(
            Some(ENDPOINT_HOST),
            region,
            json!({"serviceName": name, "power": "nano", "scale": 1}),
        )
        .await;
    // The one token this service was minted with, read off the URL the create
    // announced: the part of the address that belongs to the record.
    let token = announced_url("the create", &created)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    for (case, host) in [
        ("the endpoint it was created through", ENDPOINT_HOST),
        ("another loopback port", "127.0.0.1:19297"),
        ("an AWS-style endpoint host", "lightsail.us-east-1.amazonaws.com"),
    ] {
        let read = emulator.read(Some(host), region, name).await;
        assert_eq!(
            announced_url(case, &read),
            format!("https://{host}{PUBLIC_PATH_PREFIX}/{region}/{name}/{token}"),
            "case {case:?}: expected the url on the reader's own endpoint"
        );
    }
}

/// RULE: a request that names no host names no endpoint to announce, so no URL is
/// reported at all — a literal address nothing serves is worse than none.
#[tokio::test]
async fn a_request_naming_no_host_announces_no_url() {
    let emulator = Emulator::new();
    let created = emulator
        .create(
            None,
            "us-east-1",
            json!({"serviceName": "hostless", "power": "nano", "scale": 1}),
        )
        .await;
    assert!(
        created["url"].is_null(),
        "case a create naming no host: expected no url, got {:?}",
        created["url"]
    );
    let read = emulator.read(None, "us-east-1", "hostless").await;
    assert!(
        read["url"].is_null(),
        "case a read naming no host: expected no url, got {:?}",
        read["url"]
    );
}

// ---------------------------------------------------------------------------
// What answers there
// ---------------------------------------------------------------------------

/// One service whose announced URL is then followed, and what the rule says is
/// published there.
#[derive(Debug, Clone, Copy)]
struct ServedRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// The deployment the create carries: `Some(true)` with a public endpoint,
    /// `Some(false)` with containers but none, `None` for no deployment at all.
    deployment: Option<bool>,
    /// The state the clock settles the service into.
    settled_state: &'static str,
    /// The status the published URL answers once it has settled.
    want_status: u16,
}

/// The table. The rows differ along what the rule generalises over: whether the
/// current deployment declares a public endpoint, declares containers without
/// one, or does not exist at all.
const SERVED_ROWS: &[ServedRow] = &[
    ServedRow {
        case: "a current deployment fronted by a public endpoint",
        region: "us-west-2",
        name: "fronted-service",
        deployment: Some(true),
        settled_state: "RUNNING",
        // The image's own response is what belongs here, and this emulator runs no
        // container runtime to produce it.
        want_status: 501,
    },
    ServedRow {
        case: "a current deployment that declares no public endpoint",
        region: "eu-west-1",
        name: "headless-service",
        deployment: Some(false),
        settled_state: "RUNNING",
        want_status: 404,
    },
    ServedRow {
        case: "a service with no deployment at all",
        region: "us-east-1",
        name: "bare-service",
        deployment: None,
        settled_state: "READY",
        want_status: 404,
    },
];

/// The create body `row` describes.
fn create_body(row: &ServedRow) -> Value {
    let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
    match row.deployment {
        None => {}
        Some(public) => {
            let mut deployment = json!({
                "containers": {"web": {"image": "nginx:latest", "ports": {"80": "HTTP"}}},
            });
            if public {
                deployment["publicEndpoint"] =
                    json!({"containerName": "web", "containerPort": 80});
            }
            body["deployment"] = deployment;
        }
    }
    body
}

/// RULE: the announced URL is served BY THE EMULATOR — the request reaches this
/// service rather than the router's unknown-service refusal — and what it answers
/// is decided by the current deployment: the modelled 404 where it declares no
/// public endpoint, and the honest 501 where it declares one, since no container
/// runs here.
#[tokio::test]
async fn the_announced_url_is_served_by_the_emulator() {
    let emulator = Emulator::new();
    for row in SERVED_ROWS {
        let case = row.case;
        let created = emulator
            .create(Some(ENDPOINT_HOST), row.region, create_body(row))
            .await;
        let url = announced_url(case, &created);
        let path = path_of(case, &url, ENDPOINT_HOST);

        // Nothing is the CURRENT deployment while the service is still being
        // created, so the URL publishes nothing yet, whatever the create asked for.
        if created["currentDeployment"].is_null() {
            let early = emulator.fetch("GET", ENDPOINT_HOST, &path).await;
            assert_eq!(
                early.status, 404,
                "case {case:?} before the clock activated the deployment: expected 404, got \
                 {early:?}"
            );
        }

        wait_for_state(&emulator, row.region, row.name, row.settled_state).await;

        // Every method answers the same status: the published URL is the
        // container's own address, not an AWS API, so nothing about what is
        // published there is method-shaped. A `HEAD` carries that answer's
        // metadata without its body, which is what the method means.
        let mut answers = Vec::new();
        for method in ["GET", "HEAD", "POST"] {
            let answer = emulator.fetch(method, ENDPOINT_HOST, &path).await;
            assert_eq!(
                answer.status, row.want_status,
                "case {case:?} ({method}): expected HTTP {}, got {answer:?}",
                row.want_status
            );
            assert_eq!(
                answer.content_type.as_deref(),
                Some(ENDPOINT_CONTENT_TYPE),
                "case {case:?} ({method}): expected the public endpoint's own content type, \
                 got {answer:?}"
            );
            assert!(
                answer.error_type.is_none(),
                "case {case:?} ({method}): expected this service to serve it, not an AWS error \
                 envelope, got {answer:?}"
            );
            assert_eq!(
                answer.body.is_empty(),
                method == "HEAD",
                "case {case:?} ({method}): expected a body on everything but HEAD, got {answer:?}"
            );
            answers.push(answer);
        }
        let lengths: Vec<Option<String>> = answers
            .iter()
            .map(|answer| answer.content_length.clone())
            .collect();
        assert!(
            lengths.iter().all(|length| *length == lengths[0] && length.is_some()),
            "case {case:?}: expected one declared content length across GET/HEAD/POST, got \
             {lengths:?}"
        );

        // A path BENEATH the announced URL addresses the same service: a client
        // appends the container's own path to it.
        for suffix in ["index.html", "deep/path/under/it"] {
            let answer = emulator
                .fetch("GET", ENDPOINT_HOST, &format!("{path}/{suffix}"))
                .await;
            assert_eq!(
                answer.status, row.want_status,
                "case {case:?} (/{suffix}): expected the same HTTP {}, got {answer:?}",
                row.want_status
            );
        }

        // And the same path reached through an endpoint whose host DOES name this
        // service is served the same way: which service answers is settled by the
        // host or the published path, and both lead here.
        let through_named_host = emulator
            .fetch("GET", "lightsail.us-east-1.amazonaws.com", &path)
            .await;
        assert_eq!(
            through_named_host.status, row.want_status,
            "case {case:?} (an endpoint host naming this service): expected the same HTTP {}, \
             got {through_named_host:?}",
            row.want_status
        );

        // Following the URL is a pure read: the record it reports is unchanged.
        let before = emulator.read(Some(ENDPOINT_HOST), row.region, row.name).await;
        emulator.fetch("GET", ENDPOINT_HOST, &path).await;
        emulator.fetch("GET", ENDPOINT_HOST, &path).await;
        let after = emulator.read(Some(ENDPOINT_HOST), row.region, row.name).await;
        assert_eq!(
            before, after,
            "case {case:?}: following the published url must change nothing a later caller sees"
        );
    }
}

/// RULE: a public URL is addressed to ONE service — the region, the name and the
/// per-service token must all agree with a stored record — and a URL that agrees
/// with none publishes nothing, told apart from a service that fronts no container
/// by neither the status nor the body.
#[tokio::test]
async fn a_public_url_naming_no_stored_service_publishes_nothing() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let name = "fronted-service";
    let created = emulator
        .create(
            Some(ENDPOINT_HOST),
            region,
            create_body(&ServedRow {
                case: "the service the URLs below are varied from",
                region,
                name,
                deployment: Some(true),
                settled_state: "RUNNING",
                want_status: 501,
            }),
        )
        .await;
    let url = announced_url("the service under test", &created);
    let token = url.rsplit('/').next().unwrap_or_default().to_string();
    wait_for_state(&emulator, region, name, "RUNNING").await;

    // (case, path)
    let rows: [(&str, String); 4] = [
        (
            "another region's path for the same name and token",
            format!("{PUBLIC_PATH_PREFIX}/us-west-2/{name}/{token}"),
        ),
        (
            "a name no service of that region carries",
            format!("{PUBLIC_PATH_PREFIX}/{region}/absent-service/{token}"),
        ),
        (
            "the right service with another service's token",
            format!("{PUBLIC_PATH_PREFIX}/{region}/{name}/aaaaaaaaaaaaa"),
        ),
        (
            "a region that is not a region at all",
            format!("{PUBLIC_PATH_PREFIX}/not-a-region/{name}/{token}"),
        ),
    ];
    for (case, path) in rows {
        let answer = emulator.fetch("GET", ENDPOINT_HOST, &path).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert!(
            answer.error_type.is_none(),
            "case {case:?}: expected this service to serve it, got {answer:?}"
        );
    }

    // And the URL the service really published still answers for its deployment,
    // so the 404s above are about the address, not about the service.
    let answer = emulator
        .fetch("GET", ENDPOINT_HOST, &path_of("the published url", &url, ENDPOINT_HOST))
        .await;
    assert_eq!(
        answer.status, 501,
        "case the published url: expected the deployment's own 501, got {answer:?}"
    );
}

/// RULE: the claim is exactly the published path space — a path that is not one a
/// container service's URL names is left to the router, which answers its
/// `UnknownService` envelope, so this service claims nothing beyond what it
/// published.
#[tokio::test]
async fn a_path_outside_the_published_space_is_not_claimed() {
    let emulator = Emulator::new();
    for (case, path) in [
        ("the prefix with nothing under it", PUBLIC_PATH_PREFIX.to_string()),
        (
            "the prefix with only a region",
            format!("{PUBLIC_PATH_PREFIX}/us-east-1"),
        ),
        (
            "the prefix with a region and a name but no token",
            format!("{PUBLIC_PATH_PREFIX}/us-east-1/some-service"),
        ),
        ("an unrelated path", "/health".to_string()),
        ("the root", "/".to_string()),
    ] {
        let answer = emulator.fetch("GET", ENDPOINT_HOST, &path).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("UnknownService"),
            "case {case:?}: expected the router's UnknownService refusal, got {answer:?}"
        );
    }
}
