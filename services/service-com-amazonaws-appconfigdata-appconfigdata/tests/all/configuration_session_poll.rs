// SPDX-License-Identifier: CC0-1.0

//! The AppConfigData configuration-polling loop, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, deserialization, the handler, and the
//! restJson1 header/payload bindings are all exercised. The only thing written
//! directly into state is the DEPLOYED CONFIGURATION: AppConfigData has no operation
//! that deploys one (that is AppConfig's control plane, whose operations are still
//! scaffolds), so a deployment is seeded through the service's own state the way a
//! future deployment hook would write it.
//!
//! The rules under test, none of which is specific to one payload or one identifier:
//!
//! * a session is persisted under the opaque token it returns, and that token
//!   resolves back to the session on a later request;
//! * a poll returns the configuration deployed for the session's triple, with its
//!   content type and version label, only when that configuration's version differs
//!   from the version the session was last served; otherwise the payload is empty;
//!   every poll returns a fresh single-use token and a non-zero poll interval;
//! * a token that resolves to no session, or that has been used or has expired, is
//!   refused with `BadRequestException` (400) naming the modelled problem;
//! * a session whose triple has no deployed configuration is refused with
//!   `ResourceNotFoundException` (404);
//! * input outside a modelled constraint is refused with `BadRequestException` (400).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_appconfigdata_appconfigdata::state::{
    ComAmazonawsAppconfigdataAppconfigdataState, DEFAULT_POLL_INTERVAL_IN_SECONDS, MAX_SESSIONS,
};
use service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `appconfig`
/// signing name — the one AppConfigData signs with, per the model's `aws.auth#sigv4`
/// trait. Request authentication is switched off below, so only the scope matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/appconfig/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The AppConfigData endpoint host an SDK client addresses.
const ENDPOINT_HOST: &str = "appconfigdata.us-east-1.amazonaws.com";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store, as the handlers reach it.
    fn state(&self) -> engine::ServiceState<ComAmazonawsAppconfigdataAppconfigdataState> {
        self.engine
            .state::<ComAmazonawsAppconfigdataAppconfigdataState>()
    }

    async fn start_configuration_session(&self, body: &str) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}/configurationsessions"))
            .header("host", ENDPOINT_HOST)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(Bytes::copy_from_slice(body.as_bytes()))
            .expect("the StartConfigurationSession request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn get_latest_configuration(&self, token: &str) -> Reply {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!(
                "https://{ENDPOINT_HOST}/configuration?configuration_token={token}"
            ))
            .header("host", ENDPOINT_HOST)
            .header("authorization", AUTHORIZATION)
            .body(Bytes::new())
            .expect("the GetLatestConfiguration request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    /// Start a session and return its initial token.
    async fn session_token(&self, application: &str, environment: &str, profile: &str) -> String {
        let reply = self
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"{application}",
                      "EnvironmentIdentifier":"{environment}",
                      "ConfigurationProfileIdentifier":"{profile}"}}"#
            ))
            .await;
        assert_eq!(
            reply.status, 201,
            "StartConfigurationSession({application}/{environment}/{profile}): expected 201, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.initial_token()
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        Self {
            status,
            headers,
            body: response.into_body(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn initial_token(&self) -> String {
        self.json()["InitialConfigurationToken"]
            .as_str()
            .unwrap_or_else(|| panic!("no InitialConfigurationToken in {}", self.text()))
            .to_string()
    }

    fn next_token(&self) -> String {
        self.header("next-poll-configuration-token")
            .unwrap_or_else(|| {
                panic!(
                    "no Next-Poll-Configuration-Token header; headers {:?}",
                    self.headers
                )
            })
            .to_string()
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    /// `Details.InvalidParameters.configuration_token.Problem`, as the modelled
    /// `BadRequestException` carries it.
    fn token_problem(&self) -> String {
        let body = self.json();
        body["Details"]["InvalidParameters"]["configuration_token"]["Problem"]
            .as_str()
            .unwrap_or_else(|| panic!("no token problem in {}", self.text()))
            .to_string()
    }
}

/// The token pattern the model declares for `Token`: `^\S{1,8192}$`.
fn is_modelled_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 8192
        && !token.chars().any(char::is_whitespace)
        && token.chars().all(|c| !c.is_control())
}

/// One deployed-configuration class: a poll must return whatever bytes and content
/// type are deployed, so the rows differ in payload form, size and content type.
struct ContentCase {
    name: &'static str,
    content_type: &'static str,
    first: &'static [u8],
    first_label: Option<&'static str>,
    second: &'static [u8],
    second_label: Option<&'static str>,
}

const CONTENT_CASES: &[ContentCase] = &[
    ContentCase {
        name: "json document",
        content_type: "application/json",
        first: br#"{"feature":"on"}"#,
        first_label: Some("v1"),
        second: br#"{"feature":"off"}"#,
        second_label: Some("v2"),
    },
    ContentCase {
        name: "yaml document",
        content_type: "application/x-yaml",
        first: b"feature: on\n",
        first_label: Some("release-1"),
        second: b"feature: off\nextra: 2\n",
        second_label: None,
    },
    ContentCase {
        name: "plain text",
        content_type: "text/plain",
        first: b"on",
        first_label: None,
        second: b"off",
        second_label: Some("flag-2"),
    },
    ContentCase {
        name: "binary blob with non-utf8 bytes",
        content_type: "application/octet-stream",
        first: &[0x00, 0x9f, 0x92, 0x96, 0xff],
        first_label: None,
        second: &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        second_label: Some("binary-2"),
    },
    ContentCase {
        name: "empty deployed content",
        content_type: "application/json",
        first: b"",
        first_label: Some("empty-1"),
        second: br#"{"now":"not empty"}"#,
        second_label: Some("empty-2"),
    },
];

/// The whole polling loop, for every content class: first poll serves the deployed
/// configuration, the next poll serves an empty payload because nothing changed, and
/// a poll after a change serves the new content and label. Every poll rotates the
/// token.
#[tokio::test]
async fn a_poll_returns_content_only_when_the_deployed_version_changed() {
    for case in CONTENT_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let token = emulator
            .session_token("MyApp", "Prod", "MyProfile")
            .await;
        assert!(
            is_modelled_token(&token),
            "case {name}: the initial token must satisfy the modelled token pattern, got {token:?}"
        );

        emulator.state().lock().deploy_configuration(
            "MyApp",
            "Prod",
            "MyProfile",
            case.first,
            case.content_type,
            case.first_label,
        );

        // FIRST POLL — the session has been served nothing yet, so the deployed
        // content comes back with its content type and version label.
        let first = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            first.status,
            200,
            "case {name}: first poll expected 200, got {} body {}",
            first.status,
            first.text()
        );
        assert_eq!(
            first.body.as_ref(),
            case.first,
            "case {name}: first poll expected the deployed content, got {:?}",
            first.body.as_ref()
        );
        assert_eq!(
            first.header("content-type"),
            Some(case.content_type),
            "case {name}: first poll expected the deployed content type, got {:?}",
            first.header("content-type")
        );
        assert_eq!(
            first.header("version-label"),
            case.first_label,
            "case {name}: first poll expected the deployed version label, got {:?}",
            first.header("version-label")
        );
        assert_eq!(
            first.header("next-poll-interval-in-seconds"),
            Some(DEFAULT_POLL_INTERVAL_IN_SECONDS.to_string().as_str()),
            "case {name}: a session with no minimum must be told the default interval, got {:?}",
            first.header("next-poll-interval-in-seconds")
        );
        let second_token = first.next_token();
        assert!(
            is_modelled_token(&second_token) && second_token != token,
            "case {name}: the rotated token must be fresh and well formed, got {second_token:?}"
        );

        // SECOND POLL — nothing was deployed in between, so the client already has
        // the latest version: an empty payload, no version label, a fresh token.
        let second = emulator.get_latest_configuration(&second_token).await;
        assert_eq!(
            second.status,
            200,
            "case {name}: second poll expected 200, got {} body {}",
            second.status,
            second.text()
        );
        assert!(
            second.body.is_empty(),
            "case {name}: an unchanged configuration must return an empty payload, got {:?}",
            second.body.as_ref()
        );
        assert_eq!(
            second.header("version-label"),
            None,
            "case {name}: an unchanged configuration reports no version label, got {:?}",
            second.header("version-label")
        );
        assert_eq!(
            second.header("content-type"),
            Some(case.content_type),
            "case {name}: the content type describes the session's configuration and is \
             reported even when the payload is empty, got {:?}",
            second.header("content-type")
        );
        let third_token = second.next_token();
        assert!(
            is_modelled_token(&third_token) && third_token != second_token,
            "case {name}: the second poll must rotate the token too, got {third_token:?}"
        );

        // THIRD POLL — after a change, the new content and its label come back.
        emulator.state().lock().deploy_configuration(
            "MyApp",
            "Prod",
            "MyProfile",
            case.second,
            case.content_type,
            case.second_label,
        );
        let third = emulator.get_latest_configuration(&third_token).await;
        assert_eq!(
            third.status,
            200,
            "case {name}: third poll expected 200, got {} body {}",
            third.status,
            third.text()
        );
        assert_eq!(
            third.body.as_ref(),
            case.second,
            "case {name}: a changed configuration must return the new content, got {:?}",
            third.body.as_ref()
        );
        assert_eq!(
            third.header("version-label"),
            case.second_label,
            "case {name}: a changed configuration must return its version label, got {:?}",
            third.header("version-label")
        );

        // FOURTH POLL — and the change is served exactly once.
        let fourth = emulator
            .get_latest_configuration(&third.next_token())
            .await;
        assert_eq!(
            fourth.status,
            200,
            "case {name}: fourth poll expected 200, got {} body {}",
            fourth.status,
            fourth.text()
        );
        assert!(
            fourth.body.is_empty(),
            "case {name}: re-polling after the change must return an empty payload, got {:?}",
            fourth.body.as_ref()
        );
    }
}

/// Redeploying identical bytes is not a change, so a session that already holds them
/// is not made to download them again — the version is compared, not the call count.
#[tokio::test]
async fn redeploying_identical_content_is_not_a_change() {
    let emulator = Emulator::new();
    let token = emulator.session_token("MyApp", "Prod", "MyProfile").await;
    for _ in 0..3 {
        emulator.state().lock().deploy_configuration(
            "MyApp",
            "Prod",
            "MyProfile",
            b"{\"feature\":\"on\"}".to_vec(),
            "application/json",
            Some("v1"),
        );
    }
    let first = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        first.body.as_ref(),
        b"{\"feature\":\"on\"}",
        "the first poll must serve the deployed content, got {:?}",
        first.text()
    );

    emulator.state().lock().deploy_configuration(
        "MyApp",
        "Prod",
        "MyProfile",
        b"{\"feature\":\"on\"}".to_vec(),
        "application/json",
        Some("v1"),
    );
    let second = emulator
        .get_latest_configuration(&first.next_token())
        .await;
    assert!(
        second.body.is_empty(),
        "redeploying identical content must not re-serve it, got {:?}",
        second.text()
    );
}

/// How a token is made unusable before the poll that uses it.
enum TokenState {
    /// A syntactically valid token this emulator never minted.
    NeverMinted(&'static str),
    /// A token that already served a poll.
    AlreadyUsed,
    /// A token whose 24-hour lifetime has passed.
    Expired,
}

/// A token resolves to exactly one session and is good for exactly one call: anything
/// else is `BadRequestException` (400) naming the modelled problem.
#[tokio::test]
async fn an_unusable_token_is_refused_with_the_modelled_problem() {
    // `Corrupted` / `Expired` are two of the three modelled
    // `InvalidParameterProblem` values (the third, `PollIntervalNotSatisfied`, is
    // covered by the poll-interval test below).
    let cases = [
        (
            "token this emulator never minted",
            TokenState::NeverMinted("not-a-real-token"),
            "Corrupted",
        ),
        (
            "hex-shaped token this emulator never minted",
            TokenState::NeverMinted(
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            "Corrupted",
        ),
        (
            "single-character token",
            TokenState::NeverMinted("x"),
            "Corrupted",
        ),
        ("token already used for a poll", TokenState::AlreadyUsed, "Expired"),
        ("token past its 24-hour lifetime", TokenState::Expired, "Expired"),
    ];

    for (name, state, expected_problem) in cases {
        let emulator = Emulator::new();
        emulator.state().lock().deploy_configuration(
            "MyApp",
            "Prod",
            "MyProfile",
            b"{}".to_vec(),
            "application/json",
            None,
        );

        let token = match state {
            TokenState::NeverMinted(token) => token.to_string(),
            TokenState::AlreadyUsed => {
                let token = emulator.session_token("MyApp", "Prod", "MyProfile").await;
                let used = emulator.get_latest_configuration(&token).await;
                assert_eq!(
                    used.status, 200,
                    "case {name}: the first poll must succeed, got {} body {}",
                    used.status,
                    used.text()
                );
                token
            }
            TokenState::Expired => {
                let token = emulator.session_token("MyApp", "Prod", "MyProfile").await;
                let state = emulator.state();
                let mut guard = state.lock();
                let session = guard
                    .sessions
                    .get_mut(&token)
                    .expect("the session was persisted under its token");
                session.expires_at = SystemTime::now() - Duration::from_secs(1);
                drop(guard);
                token
            }
        };

        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.token_problem(),
            expected_problem,
            "case {name}: expected problem {expected_problem}, got {}",
            reply.text()
        );
    }
}

/// A session whose triple has no deployed configuration is a 404 — the lookup is by
/// the session's own triple, not "is anything deployed".
#[tokio::test]
async fn a_session_with_no_deployed_configuration_is_not_found() {
    let deployed = ("MyApp", "Prod", "MyProfile");
    let cases = [
        ("unknown application", ("OtherApp", "Prod", "MyProfile")),
        ("unknown environment", ("MyApp", "Beta", "MyProfile")),
        ("unknown configuration profile", ("MyApp", "Prod", "OtherProfile")),
        ("nothing in common", ("a", "b", "c")),
        (
            "maximum-length identifiers",
            (
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ),
        ),
    ];

    for (name, (application, environment, profile)) in cases {
        let emulator = Emulator::new();
        emulator.state().lock().deploy_configuration(
            deployed.0,
            deployed.1,
            deployed.2,
            b"{\"feature\":\"on\"}".to_vec(),
            "application/json",
            Some("v1"),
        );
        let token = emulator
            .session_token(application, environment, profile)
            .await;
        let reply = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            reply.status, 404,
            "case {name}: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {name}: expected ResourceNotFoundException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.json()["ResourceType"].as_str(),
            Some("Configuration"),
            "case {name}: expected the Configuration resource type, got {}",
            reply.text()
        );
    }
}

/// `RequiredMinimumPollIntervalInSeconds` is modelled `range 15..86400`: each
/// published bound is accepted and one step past it is refused with the modelled
/// `BadRequestException`.
#[tokio::test]
async fn the_requested_minimum_poll_interval_is_held_to_its_modelled_range() {
    let cases: &[(&str, Option<i32>, u16)] = &[
        ("one below the minimum", Some(14), 400),
        ("the minimum bound", Some(15), 201),
        ("inside the range", Some(600), 201),
        ("the maximum bound", Some(86_400), 201),
        ("one above the maximum", Some(86_401), 400),
        ("zero", Some(0), 400),
        ("negative", Some(-15), 400),
        ("absent (optional member)", None, 201),
    ];

    for (name, requested, expected_status) in cases {
        let emulator = Emulator::new();
        let interval = requested
            .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
            .unwrap_or_default();
        let reply = emulator
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                      "ConfigurationProfileIdentifier":"MyProfile"{interval}}}"#
            ))
            .await;
        assert_eq!(
            reply.status, *expected_status,
            "case {name}: expected {expected_status}, got {} body {}",
            reply.status,
            reply.text()
        );
        if *expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {name}: expected BadRequestException, got {}",
                reply.text()
            );
        } else {
            assert!(
                is_modelled_token(&reply.initial_token()),
                "case {name}: expected a well-formed token, got {}",
                reply.text()
            );
        }
    }
}

/// A required identifier that is absent, or outside its modelled 1..128 length, is
/// refused with `BadRequestException` (400) — never a 200 and never a 501.
#[tokio::test]
async fn a_malformed_start_request_is_refused_with_bad_request() {
    let over_long = "i".repeat(129);
    let cases: &[(&str, String)] = &[
        (
            "no ApplicationIdentifier",
            r#"{"EnvironmentIdentifier":"Prod","ConfigurationProfileIdentifier":"MyProfile"}"#
                .to_string(),
        ),
        (
            "no EnvironmentIdentifier",
            r#"{"ApplicationIdentifier":"MyApp","ConfigurationProfileIdentifier":"MyProfile"}"#
                .to_string(),
        ),
        (
            "no ConfigurationProfileIdentifier",
            r#"{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod"}"#.to_string(),
        ),
        (
            "empty ApplicationIdentifier",
            r#"{"ApplicationIdentifier":"","EnvironmentIdentifier":"Prod","ConfigurationProfileIdentifier":"MyProfile"}"#
                .to_string(),
        ),
        (
            "over-long EnvironmentIdentifier",
            format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"{over_long}","ConfigurationProfileIdentifier":"MyProfile"}}"#
            ),
        ),
    ];

    for (name, body) in cases {
        let emulator = Emulator::new();
        let reply = emulator.start_configuration_session(body).await;
        assert_eq!(
            reply.status, 400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {name}: expected BadRequestException, got {}",
            reply.text()
        );
    }
}

/// The reported poll interval is the session's own minimum when it set one, and a
/// session that set one may not poll faster than it: the early poll is refused with
/// `PollIntervalNotSatisfied` and leaves its token usable.
#[tokio::test]
async fn a_sessions_minimum_poll_interval_is_reported_and_enforced() {
    let cases: &[(&str, Option<i32>, i32, bool)] = &[
        ("no minimum requested", None, DEFAULT_POLL_INTERVAL_IN_SECONDS, true),
        ("the minimum bound", Some(15), 15, false),
        ("a ten-minute minimum", Some(600), 600, false),
        ("the maximum bound", Some(86_400), 86_400, false),
    ];

    for (name, requested, expected_interval, immediate_repoll_allowed) in cases {
        let emulator = Emulator::new();
        emulator.state().lock().deploy_configuration(
            "MyApp",
            "Prod",
            "MyProfile",
            b"{\"feature\":\"on\"}".to_vec(),
            "application/json",
            Some("v1"),
        );
        let interval = requested
            .map(|seconds| format!(r#","RequiredMinimumPollIntervalInSeconds":{seconds}"#))
            .unwrap_or_default();
        let started = emulator
            .start_configuration_session(&format!(
                r#"{{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod",
                      "ConfigurationProfileIdentifier":"MyProfile"{interval}}}"#
            ))
            .await;
        assert_eq!(
            started.status, 201,
            "case {name}: expected 201, got {} body {}",
            started.status,
            started.text()
        );

        // The first poll is always allowed: the session has not polled yet.
        let first = emulator
            .get_latest_configuration(&started.initial_token())
            .await;
        assert_eq!(
            first.status, 200,
            "case {name}: first poll expected 200, got {} body {}",
            first.status,
            first.text()
        );
        assert_eq!(
            first.header("next-poll-interval-in-seconds"),
            Some(expected_interval.to_string().as_str()),
            "case {name}: expected a {expected_interval}s poll interval, got {:?}",
            first.header("next-poll-interval-in-seconds")
        );

        let next = first.next_token();
        let immediate = emulator.get_latest_configuration(&next).await;
        if *immediate_repoll_allowed {
            assert_eq!(
                immediate.status, 200,
                "case {name}: a session with no minimum may poll again at once, got {} body {}",
                immediate.status,
                immediate.text()
            );
        } else {
            assert_eq!(
                immediate.status, 400,
                "case {name}: polling inside the minimum interval expected 400, got {} body {}",
                immediate.status,
                immediate.text()
            );
            assert_eq!(
                immediate.token_problem(),
                "PollIntervalNotSatisfied",
                "case {name}: expected PollIntervalNotSatisfied, got {}",
                immediate.text()
            );
            let state = emulator.state();
            let session = state.lock().session(&next).cloned();
            assert_eq!(
                session.map(|session| session.is_consumed()),
                Some(false),
                "case {name}: a refused poll must leave the token usable"
            );
        }
    }
}

/// A token is an opaque handle to stored state, not an encoding of the request: two
/// sessions over identical input get different tokens, and no token carries the
/// identifiers it was started with.
#[tokio::test]
async fn tokens_are_opaque_and_unique() {
    let cases = [
        ("short identifiers", ("a", "b", "c")),
        ("ordinary identifiers", ("MyApp", "Prod", "MyProfile")),
        (
            "identifiers that look like ids",
            ("abcd123", "wxyz789", "pqrs456"),
        ),
        (
            "maximum-length identifiers",
            (
                &"a".repeat(128)[..],
                &"b".repeat(128)[..],
                &"c".repeat(128)[..],
            ),
        ),
    ];

    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();
    for (name, (application, environment, profile)) in cases {
        let first = emulator
            .session_token(application, environment, profile)
            .await;
        let second = emulator
            .session_token(application, environment, profile)
            .await;
        assert_ne!(
            first, second,
            "case {name}: two sessions over identical input must get different tokens"
        );
        for token in [&first, &second] {
            assert!(
                is_modelled_token(token),
                "case {name}: {token:?} does not satisfy the modelled token pattern"
            );
            for identifier in [application, environment, profile] {
                // An identifier shorter than 8 characters can appear in 64 hex
                // characters by chance, so the echo check is applied to the rows
                // whose identifiers cannot: it is the token's independence from the
                // request that is under test, not hex arithmetic.
                if identifier.len() >= 8 {
                    assert!(
                        !token.contains(identifier),
                        "case {name}: the token must not echo the request, {token:?} contains {identifier:?}"
                    );
                }
            }
            assert!(
                !seen.contains(token),
                "case {name}: {token:?} was minted twice"
            );
            seen.push(token.clone());
        }
        // And both resolve to their OWN session: each is usable exactly once.
        emulator.state().lock().deploy_configuration(
            application,
            environment,
            profile,
            b"content".to_vec(),
            "text/plain",
            None,
        );
        for token in [&first, &second] {
            let reply = emulator.get_latest_configuration(token).await;
            assert_eq!(
                reply.status, 200,
                "case {name}: {token:?} must resolve to its session, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.body.as_ref(),
                b"content",
                "case {name}: {token:?} must serve its own session's configuration, got {:?}",
                reply.text()
            );
        }
    }
}

/// The session store is bounded: entries past their expiry are reclaimed when the
/// next session is stored, and the store never grows past `MAX_SESSIONS` however many
/// sessions are minted — while the token most recently handed out still resolves.
///
/// Driven against the state API rather than over HTTP: the bound is a property of the
/// store, and reaching it takes more sessions than an HTTP test should mint.
#[test]
fn the_session_store_is_bounded() {
    let mut state = ComAmazonawsAppconfigdataAppconfigdataState::default();
    let now = SystemTime::now();

    // An entry whose lifetime has passed is reclaimed, not kept for ever.
    let stale = state.start_session("MyApp", "Prod", "MyProfile", None, now);
    state
        .sessions
        .get_mut(&stale)
        .expect("the session was persisted under its token")
        .expires_at = now - Duration::from_secs(1);
    let fresh = state.start_session("MyApp", "Prod", "MyProfile", None, now);
    assert!(
        state.session(&stale).is_none(),
        "case expired entry: an entry past its expiry must be reclaimed"
    );
    assert!(
        state.session(&fresh).is_some(),
        "case expired entry: the live entry must survive the sweep"
    );

    // And minting far past the cap holds the store at the cap.
    let mut latest = fresh;
    for round in 0..(MAX_SESSIONS + 200) {
        latest = state.start_session("MyApp", "Prod", "MyProfile", None, now);
        assert!(
            state.sessions.len() <= MAX_SESSIONS,
            "case round {round}: the store must stay within {MAX_SESSIONS} entries, holds {}",
            state.sessions.len()
        );
    }
    assert!(
        state.session(&latest).is_some(),
        "case cap reached: the most recently minted token must still resolve"
    );
}
