// SPDX-License-Identifier: CC0-1.0

//! `CreateContainerServiceRegistryLogin`, and the registry address it hands out,
//! driven end to end.
//!
//! The rules under test:
//!
//! * every call CREATES one temporary set of log in credentials for the container
//!   image registry of the account — a fresh username and password per call,
//!   never repeated — expiring 12 hours after the call, as the modelled
//!   `ContainerServiceRegistryLogin$expiresAt` documents ("The log in credentials
//!   expire 12 hours after they are created");
//! * `registry` is "The address to use to push container images to the container
//!   image registry of a Lightsail account", so it is an address the caller that
//!   read it can reach: the endpoint the request arrived at — whatever that
//!   endpoint is — carrying the region the request was signed for, and never a
//!   hostname this emulator does not answer. A request naming no host names no
//!   endpoint to announce, and then no address is reported at all;
//! * that address is served BY THE EMULATOR, and it authenticates: a client
//!   presenting a set of credentials this account was handed is logged in (and
//!   answered `501`, because no container image registry protocol is served
//!   here), and one presenting anything else — no credentials, another password,
//!   an unknown username, another region's set — is refused with `401` and an
//!   HTTP Basic challenge.
//!
//! Every control-plane case goes through the same front door a client uses
//! (`POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>` and an
//! `awsJson1_1` body) dispatched by the multi-service router, and every registry
//! case is an unsigned plain HTTP request exactly as a registry client makes it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The operation under test.
const OPERATION: &str = "CreateContainerServiceRegistryLogin";

/// The path space the account's container image registry is published under
/// (`crate::state::CONTAINER_IMAGE_REGISTRY_PATH_PREFIX`).
const REGISTRY_PATH_PREFIX: &str = "/lightsail-container-image-registry";

/// The endpoint most cases below reach the emulator on: the loopback address and
/// port the emulator's own listener binds (`emulator_server`'s default data port).
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The documented validity of one set of log in credentials, in seconds, from the
/// modelled `ContainerServiceRegistryLogin$expiresAt`: "The log in credentials
/// expire 12 hours after they are created".
const LIFETIME_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// How far `expiresAt` may sit from `now + 12h` and still be the same instant: the
/// slack covers only the wall-clock time the call itself takes.
const CLOCK_SLACK_SECONDS: f64 = 120.0;

/// The content type the registry address answers with, which is its own (it is not
/// an AWS API) — so an answer carrying it came from this service and not from the
/// router's `UnknownService` refusal.
const REGISTRY_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// The hostname forms AWS publishes a container image registry under, which this
/// emulator must NEVER announce of its own accord: they are public DNS names this
/// account does not own, so a client that follows one leaves the emulator
/// entirely. (An endpoint host the REQUEST arrived at is a different matter — that
/// address is one the caller demonstrably reaches the emulator at, so a row below
/// uses one.)
const AWS_REGISTRY_HOSTNAME_FRAGMENTS: &[&str] = &["dkr.ecr.", "amazonlightsail.com"];

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// One answer from the registry address: the status, the challenge a `401` carries,
/// the content type, the `x-amzn-errortype` an AWS error envelope would carry, and
/// the body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    challenge: Option<String>,
    content_type: Option<String>,
    error_type: Option<String>,
    body: String,
}

/// One set of log in credentials, as a caller was handed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Login {
    username: String,
    password: String,
    registry: Option<String>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: a control-plane call routes by `X-Amz-Target`
        // and names its region in the credential SCOPE alone, and a request to a
        // registry address is unsigned by construction — a registry client
        // authenticates with HTTP Basic credentials.
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

    /// Create one set of log in credentials in `region` through `host`, asserting
    /// everything the rule says about a single set bar uniqueness (which only a
    /// collection of them shows), and answering it.
    ///
    /// The password is a secret, so nothing here ever prints it — only its length.
    async fn login(&self, case: &str, host: Option<&str>, region: &str) -> Login {
        let before = unix_now();
        let (status, body) = self.call(host, region, OPERATION, json!({})).await;
        let after = unix_now();
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );

        let registry_login = &body["registryLogin"];
        let username = string_member(case, registry_login, "username");
        let password = string_member(case, registry_login, "password");
        assert_ne!(
            username, password,
            "case {case}: expected the username and the password to be different values"
        );
        assert!(
            !password.chars().any(char::is_whitespace),
            "case {case}: expected an opaque whitespace-free password, got one of length {}",
            password.len()
        );

        let expires_at = registry_login["expiresAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected a numeric expiresAt, got {registry_login:?}")
        });
        let earliest = before + LIFETIME_SECONDS - CLOCK_SLACK_SECONDS;
        let latest = after + LIFETIME_SECONDS + CLOCK_SLACK_SECONDS;
        assert!(
            (earliest..=latest).contains(&expires_at),
            "case {case}: expected expiresAt in {earliest}..={latest} (the call's instant \
             plus the documented 12 hours), got {expires_at}"
        );

        let registry = registry_login["registry"].as_str().map(str::to_string);
        match (host, registry.as_deref()) {
            (Some(host), announced) => assert_eq!(
                announced,
                Some(format!("https://{host}{REGISTRY_PATH_PREFIX}/{region}").as_str()),
                "case {case}: expected the registry address on the caller's own endpoint"
            ),
            // A request that names no host names no endpoint to announce.
            (None, announced) => assert_eq!(
                announced, None,
                "case {case}: expected no registry address at all, got {announced:?}"
            ),
        }
        for fragment in AWS_REGISTRY_HOSTNAME_FRAGMENTS {
            assert!(
                registry
                    .as_deref()
                    .is_none_or(|address| !address.contains(fragment)),
                "case {case}: expected no real-AWS registry hostname in the announced \
                 address, got {registry:?}"
            );
        }

        Login {
            username,
            password,
            registry,
        }
    }

    /// Reach `path` on `host`, unsigned, presenting `credentials` as HTTP Basic
    /// credentials when there are any — exactly as a registry client does.
    async fn reach(
        &self,
        method: &str,
        host: &str,
        path: &str,
        credentials: Option<(&str, &str)>,
    ) -> Answer {
        let header = credentials.map(|(user, password)| {
            format!(
                "Basic {}",
                ::aws_smithy_types::base64::encode(format!("{user}:{password}"))
            )
        });
        self.reach_with_authorization(method, host, path, header.as_deref())
            .await
    }

    /// Reach `path` on `host`, unsigned, with `authorization` sent VERBATIM as the
    /// `Authorization` header — for the cases about how that header is spelled.
    async fn reach_with_authorization(
        &self,
        method: &str,
        host: &str,
        path: &str,
        authorization: Option<&str>,
    ) -> Answer {
        let mut builder = ::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host);
        if let Some(authorization) = authorization {
            builder = builder.header("authorization", authorization);
        }
        let request = builder
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
            challenge: header("www-authenticate"),
            content_type: header("content-type"),
            error_type: header("x-amzn-errortype"),
            body: String::from_utf8_lossy(response.body()).to_string(),
        }
    }

    /// Reach the announced address of `login` with `credentials`.
    async fn reach_registry(
        &self,
        case: &str,
        login: &Login,
        credentials: Option<(&str, &str)>,
    ) -> Answer {
        let registry = login
            .registry
            .as_deref()
            .unwrap_or_else(|| panic!("case {case}: expected an announced registry address"));
        let (host, path) = split_announced(case, registry);
        self.reach("GET", &host, &path, credentials).await
    }
}

/// Seconds since the Unix epoch, the unit `expiresAt` is carried in.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// The string member `name` of `value`, required to be present and non-empty.
fn string_member(case: &str, value: &Value, name: &str) -> String {
    let member = value[name].as_str().unwrap_or_else(|| {
        panic!("case {case}: expected a string {name} in the registryLogin, got {value:?}")
    });
    assert!(
        !member.is_empty(),
        "case {case}: expected a non-empty {name}"
    );
    member.to_string()
}

/// The host and the request target of an announced `https://<host><path>` address,
/// which is what a client reaching that address sends.
fn split_announced(case: &str, address: &str) -> (String, String) {
    let without_scheme = address
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case}: expected an https address, got {address:?}"));
    let (host, path) = without_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case}: expected a path in {address:?}"));
    (host.to_string(), format!("/{path}"))
}

/// Assert that `answer` is the registry's HTTP Basic challenge: `401`, a
/// `WWW-Authenticate: Basic …` header, this service's own content type, and no AWS
/// error envelope.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 401,
        "case {case}: expected HTTP 401, got {answer:?}"
    );
    assert!(
        answer
            .challenge
            .as_deref()
            .is_some_and(|challenge| challenge.starts_with("Basic ")),
        "case {case}: expected an HTTP Basic challenge, got {answer:?}"
    );
    assert_eq!(
        answer.content_type.as_deref(),
        Some(REGISTRY_CONTENT_TYPE),
        "case {case}: expected the registry's own content type, got {answer:?}"
    );
    assert!(
        answer.error_type.is_none(),
        "case {case}: expected this service to serve it, not an AWS error envelope, got \
         {answer:?}"
    );
}

/// Assert that `answer` is what a logged-in client is answered: `501`, because the
/// container image registry protocol itself is not served here.
fn assert_logged_in(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 501,
        "case {case}: expected HTTP 501 for a logged-in client, got {answer:?}"
    );
    assert!(
        answer.challenge.is_none(),
        "case {case}: expected no challenge for a logged-in client, got {answer:?}"
    );
    assert!(
        answer.error_type.is_none(),
        "case {case}: expected this service to serve it, not an AWS error envelope, got \
         {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One call for credentials. The rows differ along everything the rule
/// generalises over: the FORM of the endpoint a client can reach an emulator at
/// (a loopback IP literal with the default port and with another, an IPv6
/// literal, `localhost`, an AWS-style endpoint host, a bare internal name) and
/// the region the call is signed for.
const ENDPOINT_ROWS: &[(&str, &str, &str)] = &[
    (
        "the loopback IP and the default port",
        ENDPOINT_HOST,
        "us-east-1",
    ),
    (
        "the loopback IP on another port",
        "127.0.0.1:34567",
        "us-west-2",
    ),
    ("an IPv6 loopback literal", "[::1]:9297", "eu-west-1"),
    ("localhost with a port", "localhost:8443", "ap-northeast-1"),
    (
        "an AWS-style endpoint host",
        "lightsail.us-west-2.amazonaws.com",
        "us-west-2",
    ),
    ("a bare internal hostname", "emulator.internal", "sa-east-1"),
];

/// The rows whose announced address is then REACHED: the loopback IP on two
/// ports, `localhost`, and the endpoint host that names this service.
///
/// The router resolves a request's route key from its authority, and a host whose
/// first label is neither an IP literal, `localhost`, nor an emulated service's
/// name resolves to a key that names no service — such a request is answered
/// `UnknownService` before any service is reached. That is a router-level property
/// of every address this emulator publishes, not of this one, so the two rows
/// above with such a host ([`ENDPOINT_ROWS`]' IPv6 literal and bare internal
/// hostname) are out of this suite's scope.
const REACHED_ENDPOINT_ROWS: &[(&str, &str, &str)] = &[
    ENDPOINT_ROWS[0],
    ENDPOINT_ROWS[1],
    ENDPOINT_ROWS[3],
    ENDPOINT_ROWS[4],
];

/// RULE: every call creates one temporary set of credentials for the account's
/// registry — expiring 12 hours after the call — reported with the address those
/// credentials log in at, which is the endpoint the request arrived at carrying the
/// region it was signed for. No two calls hand out the same credentials.
#[tokio::test]
async fn every_call_creates_one_set_of_credentials_for_the_requested_endpoint() {
    let emulator = Emulator::new();
    let mut usernames: BTreeSet<String> = BTreeSet::new();
    let mut passwords: BTreeSet<String> = BTreeSet::new();
    let mut created = 0usize;

    for (case, host, region) in ENDPOINT_ROWS {
        // Twice per row: the store's state before the call is part of the class,
        // and nothing an earlier call wrote may change what a later one answers.
        for round in 0..2 {
            let case = format!("{case} (call {round})");
            let login = emulator.login(&case, Some(host), region).await;
            assert!(
                usernames.insert(login.username.clone()),
                "case {case}: expected a fresh username, got one already handed out"
            );
            assert!(
                passwords.insert(login.password.clone()),
                "case {case}: expected a fresh password, got one already handed out"
            );
            created += 1;
        }
    }

    assert_eq!(
        (usernames.len(), passwords.len()),
        (created, created),
        "expected every one of the {created} sets created to be distinct, got {} usernames \
         and {} passwords",
        usernames.len(),
        passwords.len()
    );
}

/// RULE: a request that names no host names no endpoint to announce, so no registry
/// address is reported at all — an address nothing answers on is worse than none —
/// while the credentials the call created are handed out as always.
#[tokio::test]
async fn a_request_naming_no_host_announces_no_registry_address() {
    let emulator = Emulator::new();
    for region in ["us-east-1", "eu-central-1"] {
        let login = emulator
            .login(&format!("a hostless call in {region}"), None, region)
            .await;
        assert!(
            login.registry.is_none(),
            "case a hostless call in {region}: expected no registry address, got {:?}",
            login.registry
        );
    }
}

/// RULE: the announced address is served by the emulator and authenticates against
/// the credentials this account was handed — a client presenting a live set is
/// logged in, and the answer behind the log in is the honest `501`, since no
/// container image registry protocol is served here.
#[tokio::test]
async fn the_announced_address_logs_a_client_in_with_the_credentials_it_created() {
    let emulator = Emulator::new();
    for (case, host, region) in REACHED_ENDPOINT_ROWS {
        let login = emulator.login(case, Some(host), region).await;
        let credentials = Some((login.username.as_str(), login.password.as_str()));
        let answer = emulator.reach_registry(case, &login, credentials).await;
        assert_logged_in(case, &answer);
        assert_eq!(
            answer.content_type.as_deref(),
            Some(REGISTRY_CONTENT_TYPE),
            "case {case}: expected the registry's own content type, got {answer:?}"
        );

        // Logging in is a pure read: the same credentials work again afterwards,
        // and so does a second set created later.
        let again = emulator.reach_registry(case, &login, credentials).await;
        assert_eq!(
            again, answer,
            "case {case}: expected logging in twice to answer the same"
        );
        let second = emulator.login(&format!("{case}, a second set"), Some(host), region).await;
        for (which, set) in [("the first", &login), ("the second", &second)] {
            let case = format!("{case}, {which} set after a second was created");
            let answer = emulator
                .reach_registry(
                    &case,
                    set,
                    Some((set.username.as_str(), set.password.as_str())),
                )
                .await;
            assert_logged_in(&case, &answer);
        }
    }
}

/// RULE: every path beneath the announced address belongs to the registry the
/// address names — a registry client appends the protocol's own path to it — so a
/// logged-in client reaches the same registry there, whatever method it uses.
#[tokio::test]
async fn the_registry_protocol_paths_under_the_address_reach_the_same_registry() {
    let emulator = Emulator::new();
    let login = emulator
        .login("the set under test", Some(ENDPOINT_HOST), "us-east-1")
        .await;
    let credentials = Some((login.username.as_str(), login.password.as_str()));
    let registry = login.registry.clone().expect("an announced address");
    let (host, base) = split_announced("the set under test", &registry);

    // The paths a Docker registry client walks: the version check, a blob upload,
    // and a manifest put.
    for suffix in [
        "",
        "/v2/",
        "/v2/myservice.web/blobs/uploads/",
        "/v2/myservice.web/manifests/latest",
    ] {
        for method in ["GET", "HEAD", "POST", "PUT"] {
            let case = format!("{method} {base}{suffix}");
            let path = format!("{base}{suffix}");
            let answer = emulator
                .reach(method, &host, &path, credentials)
                .await;
            assert_logged_in(&case, &answer);
            assert_eq!(
                answer.body.is_empty(),
                method == "HEAD",
                "case {case}: expected a body on everything but HEAD, got {answer:?}"
            );
            // And the same path with no credentials is refused there too.
            let case = format!("{case} unauthenticated");
            let refused = emulator.reach(method, &host, &path, None).await;
            assert_refused(&case, &refused);
        }
    }
}

/// RULE: anything other than a live set of credentials for that registry is
/// refused with the HTTP Basic challenge — no credentials at all, the right
/// username with another password, an unknown username, a password that is another
/// set's, or a set created for another region's registry address.
#[tokio::test]
async fn a_client_presenting_no_live_set_of_credentials_is_refused() {
    let emulator = Emulator::new();
    let first = emulator
        .login("the first set", Some(ENDPOINT_HOST), "us-east-1")
        .await;
    let second = emulator
        .login("the second set", Some(ENDPOINT_HOST), "us-east-1")
        .await;
    // A set created in another region, whose address is another registry's.
    let elsewhere = emulator
        .login("a set in another region", Some(ENDPOINT_HOST), "eu-west-1")
        .await;

    let rows: Vec<(String, Option<(String, String)>)> = vec![
        ("no credentials at all".to_string(), None),
        (
            "the right username with another set's password".to_string(),
            Some((first.username.clone(), second.password.clone())),
        ),
        (
            "a username no call handed out".to_string(),
            Some(("AWS".to_string(), first.password.clone())),
        ),
        (
            "an empty username and password".to_string(),
            Some((String::new(), String::new())),
        ),
        (
            "the right username with an empty password".to_string(),
            Some((first.username.clone(), String::new())),
        ),
        (
            "another region's set at this registry's address".to_string(),
            Some((elsewhere.username.clone(), elsewhere.password.clone())),
        ),
    ];

    for (case, credentials) in &rows {
        let presented = credentials
            .as_ref()
            .map(|(user, password)| (user.as_str(), password.as_str()));
        let answer = emulator.reach_registry(case, &first, presented).await;
        assert_refused(case, &answer);
    }

    // The refusals above are about the credentials, not about the address: the set
    // that belongs to this registry still logs in, and the set created elsewhere
    // logs in at ITS own address.
    let answer = emulator
        .reach_registry(
            "the set that belongs to this registry",
            &first,
            Some((first.username.as_str(), first.password.as_str())),
        )
        .await;
    assert_logged_in("the set that belongs to this registry", &answer);
    let answer = emulator
        .reach_registry(
            "the other region's set at its own address",
            &elsewhere,
            Some((elsewhere.username.as_str(), elsewhere.password.as_str())),
        )
        .await;
    assert_logged_in("the other region's set at its own address", &answer);
}

/// RULE: the credentials are read from the `Authorization` header the way RFC 9110
/// defines `credentials = scheme 1*SP token68` — the scheme name is
/// case-insensitive and any run of whitespace separates it from the credentials —
/// so a client that spells the header differently still logs in, while a header
/// that carries no Basic credentials at all does not.
#[tokio::test]
async fn the_authorization_header_is_read_as_the_rfc_defines_it() {
    let emulator = Emulator::new();
    let login = emulator
        .login("the set under test", Some(ENDPOINT_HOST), "us-east-1")
        .await;
    let registry = login.registry.clone().expect("an announced address");
    let (host, path) = split_announced("the set under test", &registry);
    let encode = |text: String| ::aws_smithy_types::base64::encode(text);
    let encoded = encode(format!("{}:{}", login.username, login.password));

    for (case, header) in [
        ("the canonical spelling", format!("Basic {encoded}")),
        ("an all-lowercase scheme", format!("basic {encoded}")),
        ("an all-uppercase scheme", format!("BASIC {encoded}")),
        ("two spaces after the scheme", format!("Basic  {encoded}")),
        ("a tab after the scheme", format!("Basic\t{encoded}")),
        ("surrounding whitespace", format!("  Basic {encoded}  ")),
    ] {
        let answer = emulator
            .reach_with_authorization("GET", &host, &path, Some(&header))
            .await;
        assert_logged_in(case, &answer);
    }

    for (case, header) in [
        ("another scheme carrying the same token", format!("Bearer {encoded}")),
        ("the token with no scheme at all", encoded.clone()),
        ("the scheme with no credentials", "Basic".to_string()),
        ("credentials that are not base64", "Basic not base64!!".to_string()),
        (
            "credentials carrying no colon",
            format!("Basic {}", encode(login.username.clone())),
        ),
    ] {
        let answer = emulator
            .reach_with_authorization("GET", &host, &path, Some(&header))
            .await;
        assert_refused(case, &answer);
    }
}

/// RULE: credentials belong to the account that created them in ONE emulator — two
/// emulators share nothing, so a set created in one logs in nowhere else.
#[tokio::test]
async fn credentials_created_in_one_emulator_log_in_nowhere_else() {
    let first = Emulator::new();
    let second = Emulator::new();
    let login = first
        .login("the set under test", Some(ENDPOINT_HOST), "us-east-1")
        .await;
    let credentials = Some((login.username.as_str(), login.password.as_str()));

    assert_logged_in(
        "its own emulator",
        &first.reach_registry("its own emulator", &login, credentials).await,
    );
    assert_refused(
        "another emulator",
        &second
            .reach_registry("another emulator", &login, credentials)
            .await,
    );
}

/// RULE: the claim is exactly the published path space — a path that is not a
/// registry address this service announced is left to the router, which answers its
/// `UnknownService` envelope.
#[tokio::test]
async fn a_path_outside_the_published_space_is_not_claimed() {
    let emulator = Emulator::new();
    // The announced address has a region under the prefix; the prefix alone names
    // no registry.
    for (case, path) in [
        ("the prefix with nothing under it", REGISTRY_PATH_PREFIX.to_string()),
        (
            "the prefix with an empty region",
            format!("{REGISTRY_PATH_PREFIX}//v2/"),
        ),
        (
            "a neighbouring path that only starts the same",
            "/lightsail-container-image-registries/us-east-1".to_string(),
        ),
        ("an unrelated path", "/v2/".to_string()),
    ] {
        let answer = emulator.reach("GET", ENDPOINT_HOST, &path, None).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("UnknownService"),
            "case {case}: expected the router's UnknownService refusal, got {answer:?}"
        );
    }
}

/// RULE (the table's own source): the regions the rows are signed for are regions
/// this service serves, so the address each row announces is a region's and not a
/// fallback's. `GetRegions` is the catalog they come from.
#[tokio::test]
async fn the_table_regions_are_regions_this_service_serves() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call(Some(ENDPOINT_HOST), "us-east-1", "GetRegions", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case reading the region catalog: expected HTTP 200, got {status} {body:?}"
    );
    let served: BTreeSet<String> = body["regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the region catalog: expected a list, got {body:?}"))
        .iter()
        .filter_map(|region| region["name"].as_str().map(str::to_string))
        .collect();

    for (case, _, region) in ENDPOINT_ROWS {
        assert!(
            served.contains(*region),
            "case {case}: expected {region} to be a region this service serves, got {served:?}"
        );
    }
}
