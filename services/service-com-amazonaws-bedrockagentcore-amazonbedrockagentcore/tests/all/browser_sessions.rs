// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore browser session lifecycle:
//! `StartBrowserSession` and `GetBrowserSession`.
//!
//! The rule under test: a start MINTS a `sessionId` matching its modelled pattern,
//! persists the configuration the request asked for (name, time-to-live, viewport,
//! extensions, enterprise policies, profile, proxy, certificates, file system
//! configurations) under the named browser together with the instant it was created, and
//! reports that stored record back; a replayed `clientToken` answers with the session
//! that token already started instead of starting a second one; a read reports the stored
//! record with the status the clock implies — `READY` until the stored time-to-live has
//! run out, `TERMINATED` from then on; and a browser is the CONTROL plane's resource, so
//! a session can only be started on, or read from, a browser that component holds.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator
//! serves with, so the wire shape (routes, status codes, `restJson1` bodies, modelled
//! error codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::{DateTime, Format};
use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;
use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcorecontrol;

use super::harness::label;

/// The browser tool the service itself provides, held by every account without anything
/// creating it: the control plane's `BrowserId` pattern reserves this id as its first
/// alternative (source: smithy_query query="shape" shape="BrowserId" on the control
/// plane's model).
const SYSTEM_BROWSER: &str = "aws.browser.v1";

/// The `name` a case creates its own browser under, through the control plane that owns
/// browsers. `CreateBrowser$name` is `SandboxName`, pattern ^[a-zA-Z][a-zA-Z0-9_]{0,47}$
/// (source: smithy_query query="operation" operation="CreateBrowser" on the control
/// plane's model).
const CUSTOM_BROWSER_NAME: &str = "demo_browser";
/// A second browser name, for a case that needs two browsers that both exist.
const OTHER_BROWSER_NAME: &str = "demo_other_browser";

/// A `browserId`-shaped identifier nothing ever created, which therefore names no
/// browser.
const MISSING_BROWSER: &str = "demo_missing_browser-0a1b2c3d4e";

/// The emulator, with request authentication off so a test signs nothing, plus the
/// shared engine so a case can age a stored session's clock without sleeping.
///
/// The AgentCore CONTROL plane is mounted beside this service because it owns the
/// browsers a session runs on: a case that needs a custom browser creates a real one
/// through `CreateBrowser` rather than naming a browser nothing holds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(agentcorecontrol::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Create one custom browser through the control plane and return the `browserId` it
/// minted. `CreateBrowser` is `PUT /browsers`, answering 202 (source: smithy_query
/// query="operation" operation="CreateBrowser" on the control plane's model).
async fn create_browser(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "PUT",
        "/browsers",
        Some(json!({"name": name, "networkConfiguration": {"networkMode": "PUBLIC"}})),
    )
    .await;
    assert_eq!(
        status, 202,
        "the fixture browser must be created, got {status} with {created:?}"
    );
    created["browserId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `StartBrowserSession`: `PUT /browsers/{browserIdentifier}/sessions/start`.
async fn start_session(router: &MultiServiceRouter, browser: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/browsers/{}/sessions/start", label(browser)),
        Some(body),
    )
    .await
}

/// `GetBrowserSession`: `GET /browsers/{browserIdentifier}/sessions/get?sessionId=…`.
async fn get_session(
    router: &MultiServiceRouter,
    browser: &str,
    session_id: &str,
) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!(
            "/browsers/{}/sessions/get?sessionId={}",
            label(browser),
            label(session_id)
        ),
        None,
    )
    .await
}

/// Move a stored session's creation instant `by` further into the past, so a case can
/// reach the far side of a time-to-live without sleeping for it. This is the test
/// reaching into the store the handlers share, not an emulator behaviour.
fn age_session(engine: &engine::InMemoryEngine, browser: &str, session_id: &str, by: Duration) {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let mut store = state.lock();
    let session = store
        .browser_session_mut(browser, session_id)
        .expect("the session that was just started");
    session.created_at -= by;
}

/// How many sessions this emulator holds for one browser — the check that a replayed
/// `clientToken` started nothing new, and that a refused start stored nothing.
fn session_count(engine: &engine::InMemoryEngine, browser: &str) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store
        .browsers
        .get(browser)
        .map_or(0, |held| held.sessions.len())
}

/// `BrowserSessionId`'s modelled pattern, `^[0-9a-zA-Z]{1,40}$`, checked without a regex
/// dependency.
fn is_modelled_session_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// A `DateTimestamp` member carries `@timestampFormat("date-time")`, so on this
/// service's restJson1 wire it is an RFC 3339 string rather than an epoch number
/// (source: smithy_query query="shape" shape="DateTimestamp").
fn assert_date_time(case: &str, member_name: &str, value: &Value) {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member_name} as a date-time string, got {value:?}")
    });
    assert!(
        DateTime::from_str(text, Format::DateTime).is_ok(),
        "case {case:?}: expected {member_name} to parse as RFC 3339, got {text:?}"
    );
}

fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    member(body, name).as_str().unwrap_or("")
}

/// One certificate, as `Certificate$location` models it: the `secretsManager` member of
/// the `CertificateLocation` union (source: smithy_query query="shape"
/// shape="CertificateLocation").
fn certificate(secret: &str) -> Value {
    json!({"location": {"secretsManager": {"secretArn": secret}}})
}

/// An Amazon S3 resource location, as `ResourceLocation`'s only member models it
/// (source: smithy_query query="shape" shape="ResourceLocation,S3Location").
fn s3_location(bucket: &str, prefix: &str) -> Value {
    json!({"s3": {"bucket": bucket, "prefix": prefix}})
}

/// Every configuration member a start accepts is reported back by a read, for rows that
/// differ along which members were supplied, their sizes and the published bounds of the
/// values they carry.
#[tokio::test]
async fn a_started_session_round_trips_through_get_browser_session() {
    let (router, _engine) = emulator();
    let custom = create_browser(&router, CUSTOM_BROWSER_NAME).await;

    // Rows differ along the browser, whether `name` was supplied and how long it is, the
    // time-to-live (omitted so the modelled default applies, both published bounds of
    // `BrowserSessionTimeout` 1..28800, and the documented recommended minimum), the
    // viewport (both published bounds of `ViewPortWidth` 320..3840 and `ViewPortHeight`
    // 240..2160), and which of the remaining optional members were supplied.
    let cases: Vec<(&str, String, Value, i64)> = vec![
        (
            "no-optional-members-takes-the-modelled-default",
            SYSTEM_BROWSER.to_string(),
            json!({}),
            3_600,
        ),
        (
            "named-at-the-lowest-timeout-and-the-smallest-viewport",
            SYSTEM_BROWSER.to_string(),
            json!({
                "name": "a",
                "sessionTimeoutSeconds": 1,
                "viewPort": {"width": 320, "height": 240},
            }),
            1,
        ),
        (
            "longest-name-at-the-highest-timeout-and-the-largest-viewport",
            custom.clone(),
            json!({
                "name": "n".repeat(100),
                "sessionTimeoutSeconds": 28_800,
                "viewPort": {"width": 3_840, "height": 2_160},
            }),
            28_800,
        ),
        (
            "recommended-minimum-timeout-with-two-certificates",
            custom.clone(),
            json!({
                "name": "research session",
                "sessionTimeoutSeconds": 60,
                "viewPort": {"width": 1_280, "height": 720},
                "certificates": [
                    certificate("arn:aws:secretsmanager:us-east-1:123456789012:secret:ca-one"),
                    certificate("arn:aws:secretsmanager:us-east-1:123456789012:secret:ca-two"),
                ],
            }),
            60,
        ),
        (
            "extensions-enterprise-policies-and-a-profile",
            SYSTEM_BROWSER.to_string(),
            json!({
                "sessionTimeoutSeconds": 1_800,
                "extensions": [{"location": s3_location("agent-extensions", "ad-blocker.crx")}],
                // `BrowserEnterprisePolicyType` values: MANAGED | RECOMMENDED (source:
                // smithy_query query="shape" shape="BrowserEnterprisePolicyType").
                "enterprisePolicies": [
                    {"location": s3_location("agent-policies", "managed.json"), "type": "MANAGED"},
                    {"location": s3_location("agent-policies", "recommended.json"), "type": "RECOMMENDED"},
                ],
                // `BrowserProfileId`'s pattern is
                // ^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$ (source: smithy_query
                // query="shape" shape="BrowserProfileConfiguration").
                "profileConfiguration": {"profileIdentifier": "shopping_profile-0a1b2c3d4e"},
            }),
            1_800,
        ),
        (
            "a-proxy-with-credentials-and-a-bypass",
            custom.clone(),
            json!({
                "sessionTimeoutSeconds": 7_200,
                // `ExternalProxy`: server is a DNS hostname, port 1..65535 (source:
                // smithy_query query="shape" shape="ExternalProxy").
                "proxyConfiguration": {
                    "proxies": [{"externalProxy": {
                        "server": "proxy.example.com",
                        "port": 8_080,
                        "domainPatterns": [".example.com", "example.net"],
                        "credentials": {"basicAuth": {
                            "secretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:proxy",
                        }},
                    }}],
                    "bypass": {"domainPatterns": ["internal.example.org"]},
                },
                "enterprisePolicies": [],
            }),
            7_200,
        ),
        (
            "both-filesystem-union-members-mounted",
            SYSTEM_BROWSER.to_string(),
            json!({
                "sessionTimeoutSeconds": 900,
                // The ARN forms `S3FilesConfiguration` and `EfsConfiguration` publish as
                // member patterns (source: smithy_query query="shape"
                // shape="S3FilesConfiguration,EfsConfiguration").
                "filesystemConfigurations": [
                    {"s3FilesConfiguration": {
                        "accessPointArn": "arn:aws:s3files:us-east-1:123456789012:file-system/fs-0123456789abcdef0/access-point/fsap-0123456789abcdef0",
                        "mountPath": "/mnt/s3",
                        "fileSystemArn": "arn:aws:s3files:us-east-1:123456789012:file-system/fs-0123456789abcdef0",
                    }},
                    {"efsConfiguration": {
                        "accessPointArn": "arn:aws:elasticfilesystem:us-east-1:123456789012:access-point/fsap-0123456789abcdef0",
                        "mountPath": "/mnt/efs",
                        "fileSystemArn": "arn:aws:elasticfilesystem:us-east-1:123456789012:file-system/fs-0123456789abcdef0",
                    }},
                ],
            }),
            900,
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, browser, request, expected_timeout) in cases {
        let (status, started) = start_session(&router, &browser, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartBrowserSession, got {status} with {started:?}"
        );
        assert_eq!(
            text(&started, "browserIdentifier"),
            browser,
            "case {case:?}: expected the browser the request named, got {started:?}"
        );
        let session_id = text(&started, "sessionId").to_string();
        assert!(
            is_modelled_session_id(&session_id),
            "case {case:?}: expected a sessionId matching ^[0-9a-zA-Z]{{1,40}}$, got {session_id:?}"
        );
        assert!(
            minted.insert(session_id.clone()),
            "case {case:?}: expected a sessionId no earlier start minted, got {session_id:?}"
        );
        let created_at = member(&started, "createdAt").clone();
        assert_date_time(case, "createdAt", &created_at);

        let (status, read) = get_session(&router, &browser, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBrowserSession, got {status} with {read:?}"
        );
        assert_eq!(
            text(&read, "sessionId"),
            session_id,
            "case {case:?}: expected the started session back, got {read:?}"
        );
        assert_eq!(
            text(&read, "browserIdentifier"),
            browser,
            "case {case:?}: expected the browser the session was started on, got {read:?}"
        );
        // The creation instant survives the round trip character for character, so no
        // precision of the stored instant is lost on the way out.
        assert_eq!(
            member(&read, "createdAt"),
            &created_at,
            "case {case:?}: expected the creation instant the start reported, got {read:?}"
        );
        assert_date_time(case, "createdAt (read back)", member(&read, "createdAt"));
        assert_eq!(
            member(&read, "lastUpdatedAt"),
            &created_at,
            "case {case:?}: expected a session nothing has changed to report its creation instant as lastUpdatedAt, got {read:?}"
        );
        assert_eq!(
            member(&read, "name"),
            request.get("name").unwrap_or(&Value::Null),
            "case {case:?}: expected the name the request asked for, got {read:?}"
        );
        assert_eq!(
            member(&read, "sessionTimeoutSeconds").as_i64(),
            Some(expected_timeout),
            "case {case:?}: expected the stored time-to-live {expected_timeout}, got {read:?}"
        );
        // Every remaining configuration member is reported exactly as it was asked for,
        // and a member the request omitted is reported by no value at all.
        for configuration in [
            "viewPort",
            "extensions",
            "enterprisePolicies",
            "profileConfiguration",
            "proxyConfiguration",
            "certificates",
            "filesystemConfigurations",
        ] {
            assert_eq!(
                member(&read, configuration),
                request.get(configuration).unwrap_or(&Value::Null),
                "case {case:?}: expected {configuration} as the request asked for it, got {read:?}"
            );
        }
        assert_eq!(
            text(&read, "status"),
            "READY",
            "case {case:?}: expected a session inside its time-to-live to be READY, got {read:?}"
        );

        // A read changes nothing a later caller sees.
        let (status, again) = get_session(&router, &browser, &session_id).await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {case:?}: expected a second read to report the same session, got {status} with {again:?}"
        );
    }
}

/// A session reports `READY` until the time-to-live the request asked for has run out,
/// and `TERMINATED` from that instant on — whatever that time-to-live is.
#[tokio::test]
async fn session_status_follows_the_time_to_live_the_request_asked_for() {
    let (router, engine) = emulator();

    // `BrowserSessionTimeout` is range 1..28800 with default 3600 (source: smithy_query
    // query="operation" operation="StartBrowserSession"): both published bounds, the
    // default and the documented recommended minimum.
    for timeout in [1_i64, 60, 3_600, 28_800] {
        let case = format!("timeout-{timeout}");
        let (status, started) =
            start_session(&router, SYSTEM_BROWSER, json!({"sessionTimeoutSeconds": timeout})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartBrowserSession, got {status} with {started:?}"
        );
        let session_id = text(&started, "sessionId").to_string();

        // One second short of the time-to-live the session is still usable.
        let timeout_secs = u64::try_from(timeout).expect("a modelled timeout is positive");
        age_session(
            &engine,
            SYSTEM_BROWSER,
            &session_id,
            Duration::from_secs(timeout_secs - 1),
        );
        let (status, read) = get_session(&router, SYSTEM_BROWSER, &session_id).await;
        assert_eq!(
            (status, text(&read, "status")),
            (200, "READY"),
            "case {case:?}: expected READY one second before the time-to-live runs out, got {status} with {read:?}"
        );

        // At the instant it runs out the session is terminated, with no call needed to
        // make it so.
        age_session(&engine, SYSTEM_BROWSER, &session_id, Duration::from_secs(1));
        let (status, read) = get_session(&router, SYSTEM_BROWSER, &session_id).await;
        assert_eq!(
            (status, text(&read, "status")),
            (200, "TERMINATED"),
            "case {case:?}: expected TERMINATED once the time-to-live has run out, got {status} with {read:?}"
        );
        assert_eq!(
            member(&read, "sessionTimeoutSeconds").as_i64(),
            Some(timeout),
            "case {case:?}: expected a terminated session to still report its time-to-live, got {read:?}"
        );
    }
}

/// `clientToken` is an `@idempotencyToken` on an `@idempotent` operation: replaying one
/// answers with the session it already started, and starts nothing new. Two starts that
/// carry no token are two sessions.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_session_it_already_started() {
    let (router, engine) = emulator();
    let browser = create_browser(&router, CUSTOM_BROWSER_NAME).await;

    // `ClientToken` is len 33..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$ (source:
    // smithy_query query="operation" operation="StartBrowserSession"). Rows differ along
    // the token's length and the body sent with the replay, which the service ignores for
    // a token it has already seen.
    let cases: Vec<(&str, String, Value, Value)> = vec![
        (
            "shortest-token-identical-replay",
            "a".repeat(33),
            json!({"name": "first", "sessionTimeoutSeconds": 120}),
            json!({"name": "first", "sessionTimeoutSeconds": 120}),
        ),
        (
            "hyphenated-token-replayed-with-a-different-body",
            "replay-token-0123456789-abcdefghij-0".to_string(),
            json!({"name": "second", "viewPort": {"width": 1_024, "height": 768}}),
            json!({"name": "ignored on a replay", "viewPort": {"width": 800, "height": 600}}),
        ),
        (
            "longest-token-replay",
            "z".repeat(256),
            json!({"sessionTimeoutSeconds": 3_600}),
            json!({"sessionTimeoutSeconds": 3_600}),
        ),
    ];

    let mut expected_sessions = 0;
    for (case, token, first_body, replay_body) in cases {
        let mut first_request = first_body.clone();
        first_request["clientToken"] = json!(token);
        let (status, started) = start_session(&router, &browser, first_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the first start, got {status} with {started:?}"
        );
        expected_sessions += 1;

        let mut replay_request = replay_body;
        replay_request["clientToken"] = json!(token);
        let (status, replayed) = start_session(&router, &browser, replay_request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the replay, got {status} with {replayed:?}"
        );
        assert_eq!(
            replayed, started,
            "case {case:?}: expected the replay to answer with the session the token already started, got {replayed:?}"
        );
        assert_eq!(
            session_count(&engine, &browser),
            expected_sessions,
            "case {case:?}: expected the replay to start no second session"
        );

        // The configuration the replay carried is ignored: the stored session is still
        // the one the first call asked for.
        let session_id = text(&started, "sessionId").to_string();
        let (status, read) = get_session(&router, &browser, &session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the started session to be readable, got {status} with {read:?}"
        );
        assert_eq!(
            member(&read, "createdAt"),
            member(&started, "createdAt"),
            "case {case:?}: expected the creation instant of the first start, got {read:?}"
        );
        assert_eq!(
            member(&read, "viewPort"),
            first_body.get("viewPort").unwrap_or(&Value::Null),
            "case {case:?}: expected the viewport the FIRST start asked for, got {read:?}"
        );
    }

    // Two starts with no token at all are two sessions.
    let (_, first) = start_session(&router, &browser, json!({})).await;
    let (_, second) = start_session(&router, &browser, json!({})).await;
    assert_ne!(
        text(&first, "sessionId"),
        text(&second, "sessionId"),
        "two starts carrying no clientToken must be two sessions, got {first:?} and {second:?}"
    );
    assert_eq!(
        session_count(&engine, &browser),
        expected_sessions + 2,
        "expected every token-less start to be stored as its own session"
    );
}

/// A lookup that misses is a 404 `ResourceNotFoundException` whose message names the
/// resource that did not resolve, resolved outside-in: the BROWSER when the identifier
/// names none, and the SESSION when the browser is held but that session is not.
#[tokio::test]
async fn a_lookup_that_misses_is_refused_with_404_naming_the_resource_that_did_not_resolve() {
    let (router, _engine) = emulator();
    let held = create_browser(&router, CUSTOM_BROWSER_NAME).await;
    let other = create_browser(&router, OTHER_BROWSER_NAME).await;

    let (_, started) = start_session(&router, &held, json!({})).await;
    let known = text(&started, "sessionId").to_string();

    // (case, the browser read, the session read, the label and identifier the refusal
    // must name)
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "unknown-session-on-the-service-provided-browser",
            SYSTEM_BROWSER.to_string(),
            "01abcdef0123456789".to_string(),
            "Session",
            "01abcdef0123456789".to_string(),
        ),
        (
            "unknown-session-on-a-browser-that-is-held",
            held.clone(),
            "0a1b2c3d4e5f6a7b".to_string(),
            "Session",
            "0a1b2c3d4e5f6a7b".to_string(),
        ),
        (
            "known-session-on-another-browser-that-is-held",
            other.clone(),
            known.clone(),
            "Session",
            known.clone(),
        ),
        (
            "session-on-a-browser-nothing-created",
            MISSING_BROWSER.to_string(),
            known.clone(),
            "Browser",
            MISSING_BROWSER.to_string(),
        ),
    ];

    for (case, browser, session_id, resource, identifier) in cases {
        let (status, body) = get_session(&router, &browser, &session_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        let expected = format!("{resource} not found: {identifier}");
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message(&body)
        );
    }

    // The session that was started is still readable after every refusal.
    let (status, read) = get_session(&router, &held, &known).await;
    assert_eq!(
        (status, text(&read, "sessionId")),
        (200, known.as_str()),
        "after every refusal the started session must still be readable, got {status} with {read:?}"
    );
}

/// A browser is the CONTROL plane's resource: a session is started only on a browser that
/// component holds, and an identifier naming none is refused with a 404 that stores
/// nothing — whatever the identifier looks like.
#[tokio::test]
async fn a_session_is_started_only_on_a_browser_the_control_plane_holds() {
    let (router, engine) = emulator();

    // Rows span the forms an identifier can take: one shaped like a `browserId` the
    // control plane mints, one shaped like the service-provided tool's reserved id but
    // not it, and one of no particular shape.
    for (case, browser) in [
        ("browser-id-shaped", MISSING_BROWSER),
        ("system-tool-shaped", "aws.browser.v2"),
        ("arbitrary-identifier", "not a browser at all"),
    ] {
        let (status, body) = start_session(&router, browser, json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from a start on a browser nothing holds, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        let expected = format!("Browser not found: {browser}");
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message(&body)
        );
        assert_eq!(
            session_count(&engine, browser),
            0,
            "case {case:?}: a refused start must store nothing"
        );
    }

    // The same start on a browser the control plane really created succeeds, so the
    // refusals above are about the browser and not about the operation.
    let created = create_browser(&router, CUSTOM_BROWSER_NAME).await;
    let (status, started) = start_session(&router, &created, json!({})).await;
    assert_eq!(
        status, 200,
        "expected a start on a created browser to succeed, got {status} with {started:?}"
    );
    assert_eq!(
        session_count(&engine, &created),
        1,
        "expected the created browser to hold the one session that was started on it"
    );
}

/// Input outside a modelled constraint is refused with a 400 `ValidationException`, and
/// a violated numeric range is stated the way this service states one — the member and
/// the bound, with no echo of the value that missed it: one case on each side of every
/// range and length `StartBrowserSessionRequest` publishes for the members under test.
///
/// The clause is pinned only for the numeric-range rows: that is the one class this
/// service has its own wording for (`src/service/validation.rs`), and a refusal of
/// another class keeps the framework's sentence, which `validation_range_wording.rs`
/// owns.
#[tokio::test]
async fn input_outside_a_modelled_constraint_is_refused_with_400() {
    let (router, engine) = emulator();

    // (case, the request, the clause a numeric-range refusal must carry)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "timeout-below-the-range",
            json!({"sessionTimeoutSeconds": 0}),
            Some(
                "Value at 'sessionTimeoutSeconds' failed to satisfy constraint: Member must have \
                 value greater than or equal to 1",
            ),
        ),
        (
            "timeout-above-the-range",
            json!({"sessionTimeoutSeconds": 28_801}),
            Some(
                "Value at 'sessionTimeoutSeconds' failed to satisfy constraint: Member must have \
                 value less than or equal to 28800",
            ),
        ),
        (
            "viewport-width-below-the-range",
            json!({"viewPort": {"width": 319, "height": 720}}),
            Some(
                "Value at 'viewPort.width' failed to satisfy constraint: Member must have value \
                 greater than or equal to 320",
            ),
        ),
        (
            "viewport-width-above-the-range",
            json!({"viewPort": {"width": 3_841, "height": 720}}),
            Some(
                "Value at 'viewPort.width' failed to satisfy constraint: Member must have value \
                 less than or equal to 3840",
            ),
        ),
        (
            "viewport-height-below-the-range",
            json!({"viewPort": {"width": 1_280, "height": 239}}),
            Some(
                "Value at 'viewPort.height' failed to satisfy constraint: Member must have value \
                 greater than or equal to 240",
            ),
        ),
        (
            "viewport-height-above-the-range",
            json!({"viewPort": {"width": 1_280, "height": 2_161}}),
            Some(
                "Value at 'viewPort.height' failed to satisfy constraint: Member must have value \
                 less than or equal to 2160",
            ),
        ),
        ("certificates-below-its-length", json!({"certificates": []}), None),
        ("extensions-below-its-length", json!({"extensions": []}), None),
        ("empty-name", json!({"name": ""}), None),
        ("name-past-its-length", json!({"name": "n".repeat(101)}), None),
        (
            "client-token-below-its-length",
            json!({"clientToken": "a".repeat(32)}),
            None,
        ),
    ];

    for (case, request, clause) in cases {
        let (status, body) = start_session(&router, SYSTEM_BROWSER, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        if let Some(clause) = clause {
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected message {expected:?}, got {:?}",
                message(&body)
            );
        }
        assert_eq!(
            session_count(&engine, SYSTEM_BROWSER),
            0,
            "case {case:?}: a refused start must store nothing"
        );
    }
}
