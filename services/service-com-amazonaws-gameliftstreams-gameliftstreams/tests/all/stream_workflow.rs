// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the GameLift Streams slice: register an application, host
//! it in a stream group, start a stream session, poll it, terminate it and tear the
//! group down — all over the wire, through the same router the emulator serves.
//!
//! Every assertion reads a RESPONSE BODY, so a handler that answered 2xx without
//! persisting anything fails here.
//!
//! The request harness below (`emulator`, `call`, `as_label`, `text`,
//! `application_body` and the resource helpers) is `pub(crate)` so the other suites
//! in this test target reuse it instead of growing a second copy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_gameliftstreams_gameliftstreams::state::{
    APPLICATION_QUOTA, SESSIONS_PER_STREAM_GROUP, STREAM_GROUP_QUOTA,
};
use service_com_amazonaws_gameliftstreams_gameliftstreams::EmulatedService;

/// Longer than `crate::state::TRANSITION` (500 ms), so a wait of this length is
/// past every transition the clock owes after one call.
pub(crate) const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The region every request below signs for, and therefore the region segment of
/// every ARN the emulator mints for them.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical emulator account id (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A `ClientToken` is modelled `length 32..128, pattern ^[\x21-\x7E]+$`.
const TOKEN_A: &str = "client-token-aaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN_B: &str = "client-token-bbbbbbbbbbbbbbbbbbbbbbbb";

/// One emulator with one shared engine, reached exactly as the production router
/// reaches it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide authentication switch: these
    // tests exercise handlers, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one restJson1 request and return its status and parsed body.
///
/// The `Authorization` credential scope is what routes the request to this
/// service and what names the region its ARNs carry.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_scope_region(router, method, path_and_query, body, REGION).await
}

/// The same call, with the credential scope naming `scope_region` instead of
/// [`REGION`] — which is what decides the region segment of every ARN the
/// emulator mints for the request, so a caller can send a region the emulator
/// never chose itself.
pub(crate) async fn call_in_scope_region(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    scope_region: &str,
) -> (u16, Value) {
    let (status, _headers, parsed) =
        dispatch(router, method, path_and_query, body, scope_region).await;
    (status, parsed)
}

/// The same call, also returning the response HEADERS — which is where restJson1
/// carries the error SHAPE (`x-amzn-errortype`) a client resolves an error from.
pub(crate) async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> (u16, HashMap<String, String>, Value) {
    dispatch(router, method, path_and_query, body, REGION).await
}

/// The one place a request is built and dispatched, so no suite grows a second
/// copy of the wire harness.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    scope_region: &str,
) -> (u16, HashMap<String, String>, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON request body serializes"))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!(
            "https://gameliftstreams.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("host", format!("gameliftstreams.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATORTEST/20260101/{scope_region}/\
                 gameliftstreams/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers: HashMap<String, String> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&bytes).into_owned())
        })
    };
    (status, headers, parsed)
}

/// Percent-encode the `/` separators of an ARN so it survives as one URI path
/// segment.
pub(crate) fn as_label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

pub(crate) fn text(value: &Value, pointer: &str, case: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} is not a string in {value}"))
        .to_string()
}

/// A minimal `CreateApplication` body with `Description`.
pub(crate) fn application_body(description: &str) -> Value {
    json!({
        "Description": description,
        "RuntimeEnvironment": {"Type": "PROTON", "Version": "20250516"},
        "ExecutablePath": "game.exe",
        "ApplicationSourceUri": "s3://my-bucket/build/",
    })
}

/// Create an application and wait until the clock has made it `READY`.
pub(crate) async fn ready_application(
    router: &MultiServiceRouter,
    description: &str,
) -> (String, String) {
    let (status, created) = call(
        router,
        "POST",
        "/applications",
        Some(application_body(description)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateApplication 201, got {status} {created}"
    );
    std::thread::sleep(PAST_TRANSITION);
    (
        text(&created, "/Id", description),
        text(&created, "/Arn", description),
    )
}

/// Create a stream group with one `AlwaysOnCapacity: 1` location per entry of
/// `locations`, and return its id and ARN. The group is still `ACTIVATING` on
/// return: the clock owes it `ACTIVE`.
pub(crate) async fn stream_group(
    router: &MultiServiceRouter,
    description: &str,
    application: &str,
    locations: &[&str],
) -> (String, String) {
    let configurations: Vec<Value> = locations
        .iter()
        .map(|location| json!({"LocationName": location, "AlwaysOnCapacity": 1}))
        .collect();
    let (status, created) = call(
        router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": description,
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": application,
            "LocationConfigurations": configurations,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    (
        text(&created, "/Id", description),
        text(&created, "/Arn", description),
    )
}

/// Create an `ACTIVE` stream group whose default application is `application`.
pub(crate) async fn active_stream_group(
    router: &MultiServiceRouter,
    description: &str,
    application: &str,
    locations: &[&str],
) -> (String, String) {
    let group = stream_group(router, description, application, locations).await;
    std::thread::sleep(PAST_TRANSITION);
    group
}

#[tokio::test]
async fn the_core_streaming_workflow_round_trips_through_state() {
    let router = emulator();

    // ---- 1. CreateApplication: a minted id and ARN, persisted configuration ----
    let (status, app) = call(
        &router,
        "POST",
        "/applications",
        Some(json!({
            "Description": "my-game",
            "RuntimeEnvironment": {"Type": "PROTON", "Version": "20250516"},
            "ExecutablePath": "game.exe",
            "ApplicationSourceUri": "s3://my-bucket/build/",
            "ApplicationLogPaths": ["logs/game.log"],
            "ApplicationLogOutputUri": "s3://my-bucket/logs",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateApplication: expected 201, got {status} {app}"
    );
    let app_id = text(&app, "/Id", "CreateApplication");
    let app_arn = text(&app, "/Arn", "CreateApplication");
    assert_eq!(
        app_arn,
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/{app_id}"),
        "CreateApplication: ARN must carry the minted id, got {app_arn}"
    );
    // "If the request is successful, Amazon GameLift Streams begins to create an
    // application and sets the status to INITIALIZED."
    assert_eq!(
        app["Status"], "INITIALIZED",
        "CreateApplication: expected INITIALIZED, got {app}"
    );
    assert_eq!(
        app["AssociatedStreamGroups"],
        json!([]),
        "CreateApplication: a new application belongs to no stream group, got {app}"
    );
    let created_at = app["CreatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("CreateApplication: CreatedAt is not a timestamp in {app}"));

    // ---- 2. GetApplication reads the same record back, and the clock has moved
    // it on to READY ----
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetApplication: expected 200, got {status} {got}"
    );
    for (member, expected) in [
        ("Arn", app_arn.as_str()),
        ("Id", app_id.as_str()),
        ("Description", "my-game"),
        ("ExecutablePath", "game.exe"),
        ("ApplicationSourceUri", "s3://my-bucket/build/"),
        ("ApplicationLogOutputUri", "s3://my-bucket/logs"),
    ] {
        assert_eq!(
            got[member], expected,
            "GetApplication: expected {member} {expected:?}, got {got}"
        );
    }
    assert_eq!(
        got["ApplicationLogPaths"],
        json!(["logs/game.log"]),
        "GetApplication: expected the stored log paths, got {got}"
    );
    assert_eq!(
        got["RuntimeEnvironment"],
        json!({"Type": "PROTON", "Version": "20250516"}),
        "GetApplication: expected the stored runtime environment, got {got}"
    );
    assert_eq!(
        got["CreatedAt"].as_f64(),
        Some(created_at),
        "GetApplication: CreatedAt must not move between reads, got {got}"
    );
    assert_eq!(
        got["Status"], "READY",
        "GetApplication: the clock owed READY by now, got {got}"
    );

    // ---- 3. CreateStreamGroup, naming the application by its ARN ----
    let (status, group) = call(
        &router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": "my-group",
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": app_arn,
            "LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": 1}],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateStreamGroup: expected 201, got {status} {group}"
    );
    let group_id = text(&group, "/Id", "CreateStreamGroup");
    let group_arn = text(&group, "/Arn", "CreateStreamGroup");
    assert_eq!(
        group_arn,
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/{group_id}"),
        "CreateStreamGroup: ARN must carry the minted id, got {group_arn}"
    );
    // "Amazon GameLift Streams assigns a unique ID to the stream group resource and
    // sets the status to ACTIVATING."
    assert_eq!(
        group["Status"], "ACTIVATING",
        "CreateStreamGroup: expected ACTIVATING, got {group}"
    );
    assert_eq!(
        group["DefaultApplication"],
        json!({"Id": app_id, "Arn": app_arn}),
        "CreateStreamGroup: the default application comes from the stored record, got {group}"
    );
    assert_eq!(
        group["AssociatedApplications"],
        json!([app_arn]),
        "CreateStreamGroup: expected the default application's ARN, got {group}"
    );
    assert_eq!(
        group["StreamClass"], "gen5n_high",
        "CreateStreamGroup: expected the requested stream class, got {group}"
    );
    assert_eq!(
        group["LocationStates"],
        json!([{
            "LocationName": "us-west-2",
            "Status": "ACTIVATING",
            "AlwaysOnCapacity": 1,
            // The settings this configuration left out hold their default of 0.
            "OnDemandCapacity": 0,
            "TargetIdleCapacity": 0,
            "MaximumCapacity": 0,
            "RequestedCapacity": 1,
            "AllocatedCapacity": 0,
            "IdleCapacity": 0,
            // The service VPC this location's capacity runs in.
            "InternalVpcIpv4CidrBlock": "10.0.0.0/16",
        }]),
        "CreateStreamGroup: location states must derive from the request and the \
         group's status, got {group}"
    );
    // 365 days: "Stream groups expire when they are 365 days old ... indicated by
    // the date value in the ExpiresAt field."
    let group_created = group["CreatedAt"].as_f64().expect("CreatedAt");
    let group_expires = group["ExpiresAt"].as_f64().expect("ExpiresAt");
    assert!(
        (group_expires - group_created - 365.0 * 24.0 * 3600.0).abs() < 1.0,
        "CreateStreamGroup: expected ExpiresAt 365 days after CreatedAt, got \
         {group_created} and {group_expires}"
    );

    // The association is visible from the application's side too.
    let (_, got) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        got["AssociatedStreamGroups"],
        json!([group_arn]),
        "GetApplication: expected the new stream group's ARN, got {got}"
    );
    assert_eq!(
        got["ReplicationStatuses"][0]["Location"], "us-west-2",
        "GetApplication: the application now replicates to the group's location, got {got}"
    );

    // ---- 4. The clock makes the group ACTIVE and allocates its always-on capacity
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup: expected 200, got {status} {got}"
    );
    assert_eq!(
        got["Status"], "ACTIVE",
        "GetStreamGroup: the clock owed ACTIVE by now, got {got}"
    );
    assert_eq!(
        got["LocationStates"][0]["AllocatedCapacity"],
        json!(1),
        "GetStreamGroup: an ACTIVE location holds its always-on capacity, got {got}"
    );
    assert_eq!(
        got["LocationStates"][0]["IdleCapacity"],
        json!(1),
        "GetStreamGroup: no session is running yet, got {got}"
    );

    // ---- 5. StartStreamSession ----
    let signal_request = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n";
    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": signal_request,
            "ApplicationIdentifier": app_id,
            "UserId": "player-1",
            "ConnectionTimeoutSeconds": 120,
            "SessionLengthSeconds": 3600,
            "AdditionalLaunchArgs": ["--windowed"],
            "AdditionalEnvironmentVariables": {"LANG": "en_US"},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "StartStreamSession: expected 201, got {status} {session}"
    );
    let session_arn = text(&session, "/Arn", "StartStreamSession");
    let session_id = session_arn
        .rsplit('/')
        .next()
        .expect("a session ARN ends in the session id")
        .to_string();
    assert_eq!(
        session_arn,
        format!(
            "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group_id}/{session_id}"
        ),
        "StartStreamSession: the session ARN carries its stream group, got {session_arn}"
    );
    // "Session transitions to ACTIVATING status."
    assert_eq!(
        session["Status"], "ACTIVATING",
        "StartStreamSession: expected ACTIVATING, got {session}"
    );
    assert_eq!(
        session["StreamGroupId"], group_id,
        "StartStreamSession: expected the hosting group's id, got {session}"
    );
    assert_eq!(
        session["ApplicationArn"], app_arn,
        "StartStreamSession: expected the stored application's ARN, got {session}"
    );
    assert_eq!(
        session["Location"], "us-west-2",
        "StartStreamSession: expected the group's only location, got {session}"
    );

    // ---- 6. GetStreamSession returns exactly what StartStreamSession stored ----
    let session_path = format!("/streamgroups/{group_id}/streamsessions/{session_id}");
    let (status, got) = call(&router, "GET", &session_path, None).await;
    assert_eq!(
        status, 200,
        "GetStreamSession: expected 200, got {status} {got}"
    );
    for (member, expected) in [
        ("Arn", session_arn.as_str()),
        ("StreamGroupId", group_id.as_str()),
        ("ApplicationArn", app_arn.as_str()),
        ("UserId", "player-1"),
        ("SignalRequest", signal_request),
        ("Protocol", "WebRTC"),
    ] {
        assert_eq!(
            got[member], expected,
            "GetStreamSession: expected {member} {expected:?}, got {got}"
        );
    }
    assert_eq!(
        got["ConnectionTimeoutSeconds"],
        json!(120),
        "GetStreamSession: expected the stored connection timeout, got {got}"
    );
    assert_eq!(
        got["SessionLengthSeconds"],
        json!(3600),
        "GetStreamSession: expected the stored session length, got {got}"
    );
    assert_eq!(
        got["AdditionalLaunchArgs"],
        json!(["--windowed"]),
        "GetStreamSession: expected the stored launch args, got {got}"
    );
    assert_eq!(
        got["AdditionalEnvironmentVariables"],
        json!({"LANG": "en_US"}),
        "GetStreamSession: expected the stored environment, got {got}"
    );
    assert_eq!(
        got["CreatedAt"].as_f64(),
        session["CreatedAt"].as_f64(),
        "GetStreamSession: CreatedAt must not move between reads, got {got}"
    );

    // A running session occupies the location's capacity.
    let (_, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        got["LocationStates"][0]["IdleCapacity"],
        json!(0),
        "GetStreamGroup: the running session consumes the idle capacity, got {got}"
    );

    // ---- 7. Placement resolves. This emulator runs no stream server, so the
    // session reaches the modelled failure state instead of reporting a stream that
    // never started. ----
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "GET", &session_path, None).await;
    assert_eq!(
        status, 200,
        "GetStreamSession after placement: expected 200, got {status} {got}"
    );
    assert_eq!(
        got["Status"], "ERROR",
        "GetStreamSession after placement: expected the modelled failure state, got {got}"
    );
    assert_eq!(
        got["StatusReason"], "internalError",
        "GetStreamSession after placement: expected a reason that says so, got {got}"
    );
    assert!(
        got.get("SignalResponse").is_none(),
        "GetStreamSession: no stream server answered the offer, so SignalResponse \
         must be absent, got {got}"
    );
    let settled = got["LastUpdatedAt"].as_f64();
    let (_, got_again) = call(&router, "GET", &session_path, None).await;
    assert_eq!(
        got_again["LastUpdatedAt"].as_f64(),
        settled,
        "GetStreamSession: a read must not move LastUpdatedAt, got {got_again}"
    );

    // ---- 8. TerminateStreamSession on a fresh session ----
    let (status, second) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": signal_request,
            "ApplicationIdentifier": app_id,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "StartStreamSession (second): expected 201, got {status} {second}"
    );
    let second_id = text(&second, "/Arn", "second session")
        .rsplit('/')
        .next()
        .expect("a session ARN ends in the session id")
        .to_string();
    let second_path = format!("/streamgroups/{group_id}/streamsessions/{second_id}");
    let (status, body) = call(&router, "DELETE", &second_path, None).await;
    assert_eq!(
        status, 204,
        "TerminateStreamSession: expected 204, got {status} {body}"
    );
    let (_, got) = call(&router, "GET", &second_path, None).await;
    assert_eq!(
        got["Status"], "TERMINATING",
        "GetStreamSession after terminate: expected TERMINATING, got {got}"
    );
    assert_eq!(
        got["StatusReason"], "apiTerminated",
        "GetStreamSession after terminate: expected apiTerminated, got {got}"
    );
    // @idempotent: terminating again changes nothing.
    let (status, _) = call(&router, "DELETE", &second_path, None).await;
    assert_eq!(
        status, 204,
        "TerminateStreamSession (replay): expected 204, got {status}"
    );
    std::thread::sleep(PAST_TRANSITION);
    let (_, got) = call(&router, "GET", &second_path, None).await;
    assert_eq!(
        got["Status"], "TERMINATED",
        "GetStreamSession: the clock owed TERMINATED by now, got {got}"
    );
    assert_eq!(
        got["StatusReason"], "apiTerminated",
        "GetStreamSession: the terminate reason survives the transition, got {got}"
    );

    // ---- 9. DeleteStreamGroup ----
    let (status, body) = call(&router, "DELETE", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 204,
        "DeleteStreamGroup: expected 204, got {status} {body}"
    );
    // "During the deletion process, the stream group's status is DELETING."
    let (_, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        got["Status"], "DELETING",
        "GetStreamGroup after delete: expected DELETING, got {got}"
    );
    // "Any application associations referring to that stream group are
    // automatically removed."
    let (_, got) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        got["AssociatedStreamGroups"],
        json!([]),
        "GetApplication after DeleteStreamGroup: the association is gone, got {got}"
    );
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 404,
        "GetStreamGroup once deleted: expected 404, got {status} {got}"
    );
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": signal_request,
            "ApplicationIdentifier": app_id,
        })),
    )
    .await;
    assert_eq!(
        status, 404,
        "StartStreamSession on a deleted group: expected 404, got {status} {got}"
    );
    let (_, listed) = call(&router, "GET", "/streamgroups", None).await;
    assert_eq!(
        listed["Items"],
        json!([]),
        "ListStreamGroups once deleted: expected no items, got {listed}"
    );

    // ---- 10. DeleteApplication ----
    let (status, body) = call(&router, "DELETE", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 204,
        "DeleteApplication: expected 204, got {status} {body}"
    );
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 404,
        "GetApplication once deleted: expected 404, got {status} {got}"
    );
    let (_, listed) = call(&router, "GET", "/applications", None).await;
    assert_eq!(
        listed["Items"],
        json!([]),
        "ListApplications once deleted: expected no items, got {listed}"
    );
}

/// Every `Identifier` path label accepts the SAME resource named by its bare id or
/// by its full ARN (the model's `Identifier` pattern allows either form).
#[tokio::test]
async fn an_identifier_resolves_a_resource_by_bare_id_and_by_arn() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "resolve-me").await;
    let (group_id, group_arn) = active_stream_group(&router, "resolve-group", &app_id, &["us-west-2"]).await;
    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_arn,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "StartStreamSession by application ARN: expected 201, got {status} {session}"
    );
    let session_arn = text(&session, "/Arn", "session");
    let session_id = session_arn
        .rsplit('/')
        .next()
        .expect("a session ARN ends in the session id")
        .to_string();

    // Each row names the same resource a different way; every row must answer with
    // the same ARN.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "application by id",
            format!("/applications/{app_id}"),
            app_arn.clone(),
        ),
        (
            "application by arn",
            format!("/applications/{}", as_label(&app_arn)),
            app_arn.clone(),
        ),
        (
            "stream group by id",
            format!("/streamgroups/{group_id}"),
            group_arn.clone(),
        ),
        (
            "stream group by arn",
            format!("/streamgroups/{}", as_label(&group_arn)),
            group_arn.clone(),
        ),
        (
            "session by group id and session id",
            format!("/streamgroups/{group_id}/streamsessions/{session_id}"),
            session_arn.clone(),
        ),
        (
            "session by group arn and session id",
            format!(
                "/streamgroups/{}/streamsessions/{session_id}",
                as_label(&group_arn)
            ),
            session_arn.clone(),
        ),
        (
            "session by group id and session arn",
            format!(
                "/streamgroups/{group_id}/streamsessions/{}",
                as_label(&session_arn)
            ),
            session_arn.clone(),
        ),
    ];
    for (case, path, expected_arn) in cases {
        let (status, got) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {path}, got {status} {got}"
        );
        assert_eq!(
            got["Arn"], expected_arn,
            "case {case:?}: expected Arn {expected_arn:?}, got {got}"
        );
    }
}

/// An identifier that names nothing answers `ResourceNotFoundException` (404) —
/// never an empty success — on every path that takes one.
#[tokio::test]
async fn an_unknown_identifier_answers_not_found() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "present").await;
    let (group_id, _) = active_stream_group(&router, "present-group", &app_id, &["us-west-2"]).await;

    let cases: Vec<(&str, &str, String)> = vec![
        ("GetApplication", "GET", "/applications/a-missing".to_string()),
        (
            "DeleteApplication",
            "DELETE",
            "/applications/a-missing".to_string(),
        ),
        (
            "GetStreamGroup",
            "GET",
            "/streamgroups/sg-missing".to_string(),
        ),
        (
            "DeleteStreamGroup",
            "DELETE",
            "/streamgroups/sg-missing".to_string(),
        ),
        (
            "GetStreamSession with an unknown group",
            "GET",
            "/streamgroups/sg-missing/streamsessions/ss-missing".to_string(),
        ),
        (
            "GetStreamSession with an unknown session",
            "GET",
            format!("/streamgroups/{group_id}/streamsessions/ss-missing"),
        ),
        (
            "TerminateStreamSession with an unknown session",
            "DELETE",
            format!("/streamgroups/{group_id}/streamsessions/ss-missing"),
        ),
    ];
    for (case, method, path) in cases {
        let (status, got) = call(&router, method, &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {method} {path}, got {status} {got}"
        );
    }

    // And a stream session ARN whose resource type is not `streamsession` names no
    // stream session at all, rather than resolving by its last path segment: that is
    // a bad value for the label, refused 400 `ValidationException` before the store
    // is read (the whole class is tabled in `refusal_envelope.rs`).
    let wrong_type = format!(
        "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application%2F{app_id}"
    );
    let (status, got) = call(
        &router,
        "GET",
        &format!("/streamgroups/{group_id}/streamsessions/{wrong_type}"),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"session named by an application ARN\": expected 400, got {status} {got}"
    );
}

/// A replayed `@idempotencyToken` answers with the resource the first call created
/// instead of creating a second one — on every create in this slice.
#[tokio::test]
async fn a_replayed_client_token_does_not_create_a_second_resource() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "idempotent-app").await;

    // CreateApplication.
    let mut body = application_body("token-app");
    body["ClientToken"] = json!(TOKEN_A);
    let (first_status, first) = call(&router, "POST", "/applications", Some(body.clone())).await;
    let (second_status, second) = call(&router, "POST", "/applications", Some(body)).await;
    assert_eq!(
        (first_status, second_status),
        (201, 201),
        "case \"CreateApplication replay\": expected 201 twice, got {first_status} and \
         {second_status}"
    );
    assert_eq!(
        first["Arn"], second["Arn"],
        "case \"CreateApplication replay\": expected one application, got {first} and {second}"
    );

    // CreateStreamGroup.
    let group_body = json!({
        "Description": "token-group",
        "StreamClass": "gen5n_high",
        "DefaultApplicationIdentifier": app_id,
        "LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": 1}],
        "ClientToken": TOKEN_A,
    });
    let (first_status, first) = call(&router, "POST", "/streamgroups", Some(group_body.clone())).await;
    let (second_status, second) = call(&router, "POST", "/streamgroups", Some(group_body)).await;
    assert_eq!(
        (first_status, second_status),
        (201, 201),
        "case \"CreateStreamGroup replay\": expected 201 twice, got {first_status} and \
         {second_status}"
    );
    assert_eq!(
        first["Arn"], second["Arn"],
        "case \"CreateStreamGroup replay\": expected one stream group, got {first} and {second}"
    );
    let group_id = text(&first, "/Id", "CreateStreamGroup replay");
    std::thread::sleep(PAST_TRANSITION);

    // StartStreamSession.
    let session_body = json!({
        "Protocol": "WebRTC",
        "SignalRequest": "offer",
        "ApplicationIdentifier": app_arn,
        "ClientToken": TOKEN_B,
    });
    let path = format!("/streamgroups/{group_id}/streamsessions");
    let (first_status, first) = call(&router, "POST", &path, Some(session_body.clone())).await;
    let (second_status, second) = call(&router, "POST", &path, Some(session_body)).await;
    assert_eq!(
        (first_status, second_status),
        (201, 201),
        "case \"StartStreamSession replay\": expected 201 twice, got {first_status} and \
         {second_status}"
    );
    assert_eq!(
        first["Arn"], second["Arn"],
        "case \"StartStreamSession replay\": expected one session, got {first} and {second}"
    );

    // Two applications exist: the helper's and the one the replayed
    // CreateApplication token created once.
    let (_, listed) = call(&router, "GET", "/applications", None).await;
    let ids: Vec<&str> = listed["Items"]
        .as_array()
        .expect("Items is a list")
        .iter()
        .filter_map(|item| item["Id"].as_str())
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "case \"CreateApplication replay\": expected 2 stored applications, got {ids:?}"
    );
    assert!(
        ids.contains(&app_id.as_str()),
        "case \"CreateApplication replay\": expected {app_id} in {ids:?}"
    );
}

/// The entry a `ClientToken` created is released with its resource: once the
/// resource is gone the token names nothing, so a later create with the same token
/// mints a fresh resource rather than answering with the deleted one.
#[tokio::test]
async fn a_client_token_is_released_with_the_resource_it_created() {
    let router = emulator();
    let mut body = application_body("released-token");
    body["ClientToken"] = json!(TOKEN_A);
    let (status, first) = call(&router, "POST", "/applications", Some(body.clone())).await;
    assert_eq!(status, 201, "the first create: got {status} {first}");
    let first_arn = text(&first, "/Arn", "the first create");
    let first_id = text(&first, "/Id", "the first create");

    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(
        &router,
        "DELETE",
        &format!("/applications/{first_id}"),
        None,
    )
    .await;
    assert_eq!(status, 204, "DeleteApplication: got {status} {got}");
    std::thread::sleep(PAST_TRANSITION);

    let (status, second) = call(&router, "POST", "/applications", Some(body)).await;
    assert_eq!(
        status, 201,
        "the replay after deletion: expected 201, got {status} {second}"
    );
    assert_ne!(
        second["Arn"],
        json!(first_arn),
        "the replay after deletion must mint a fresh application, got {second}"
    );
}

/// `MaxResults` bounds a page and `NextToken` continues it, so walking the pages
/// visits every stored resource exactly once — for any page size.
#[tokio::test]
async fn listing_pages_every_resource_exactly_once() {
    let router = emulator();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..5 {
        let (status, created) = call(
            &router,
            "POST",
            "/applications",
            Some(application_body(&format!("page-app-{index}"))),
        )
        .await;
        assert_eq!(status, 201, "seeding application {index}: got {created}");
        expected.push(text(&created, "/Id", "seed"));
    }
    expected.sort();

    // `MaxResults` is modelled `range 1..100`; the rows span its lower bound, sizes
    // that do not divide the total, the exact total and one past it.
    for page_size in [1_u64, 2, 3, 5, 6] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?MaxResults={page_size}");
        let mut pages = 0;
        loop {
            let (status, listed) = call(&router, "GET", &format!("/applications{query}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {listed}"
            );
            let items = listed["Items"].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: Items is not a list in {listed}")
            });
            assert!(
                items.len() as u64 <= page_size,
                "case {case:?}: a page may hold at most {page_size} items, got {}",
                items.len()
            );
            for item in items {
                seen.push(
                    item["Id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: no Id in {item}"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match listed["NextToken"].as_str() {
                Some(token) => query = format!("?MaxResults={page_size}&NextToken={token}"),
                None => break,
            }
        }
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every application exactly once"
        );
    }
}

/// `ApplicationLogOutputUri` is "Required if you specify one or more
/// ApplicationLogPaths" — a dependency the model cannot express, so the handler
/// refuses it with `ValidationException` (400).
#[tokio::test]
async fn log_paths_require_a_log_output_uri() {
    let router = emulator();
    // (case, ApplicationLogPaths, ApplicationLogOutputUri, expected status)
    let cases: Vec<(&str, Vec<&str>, Option<&str>, u16)> = vec![
        ("no paths, no uri", vec![], None, 201),
        ("no paths, a uri", vec![], Some("s3://my-bucket/logs"), 201),
        ("one path, no uri", vec!["logs/game.log"], None, 400),
        ("one path, empty uri", vec!["logs/game.log"], Some(""), 400),
        (
            "one path, a uri",
            vec!["logs/game.log"],
            Some("s3://my-bucket/logs"),
            201,
        ),
        (
            "many paths, no uri",
            vec!["logs/a.log", "logs/b.log", "logs/c.utrace"],
            None,
            400,
        ),
        (
            "many paths, a uri",
            vec!["logs/a.log", "logs/b.log", "logs/c.utrace"],
            Some("s3://my-bucket/logs/nested"),
            201,
        ),
    ];
    for (case, paths, output_uri, expected) in cases {
        let mut body = application_body("log-rule");
        body["ApplicationLogPaths"] = json!(paths);
        if let Some(uri) = output_uri {
            body["ApplicationLogOutputUri"] = json!(uri);
        }
        let (status, got) = call(&router, "POST", "/applications", Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got}"
        );
    }
}

/// A stream session can only start in a stream group that is `ACTIVE`, and only
/// for an application the group is associated with.
#[tokio::test]
async fn starting_a_session_checks_the_group_and_the_application() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "session-app").await;
    let (other_id, _) = ready_application(&router, "unassociated-app").await;

    // Straight after CreateStreamGroup the group is still ACTIVATING.
    let (status, group) = call(
        &router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": "activating-group",
            "StreamClass": "gen5n_high",
            "DefaultApplicationIdentifier": app_id,
            "LocationConfigurations": [{"LocationName": "us-west-2", "AlwaysOnCapacity": 1}],
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateStreamGroup: got {status} {group}");
    let group_id = text(&group, "/Id", "activating-group");
    let path = format!("/streamgroups/{group_id}/streamsessions");
    let session_body = json!({
        "Protocol": "WebRTC",
        "SignalRequest": "offer",
        "ApplicationIdentifier": app_id,
    });
    let (status, got) = call(&router, "POST", &path, Some(session_body.clone())).await;
    assert_eq!(
        status, 409,
        "case \"group still ACTIVATING\": expected 409 ConflictException, got {status} {got}"
    );

    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "POST", &path, Some(session_body)).await;
    assert_eq!(
        status, 201,
        "case \"group now ACTIVE\": expected 201, got {status} {got}"
    );

    // (case, ApplicationIdentifier, requested Locations, expected status)
    let cases: Vec<(&str, &str, Option<Vec<&str>>, u16)> = vec![
        ("unknown application", "a-nope", None, 404),
        ("unassociated application", other_id.as_str(), None, 400),
        (
            "location the group does not hold",
            app_id.as_str(),
            Some(vec!["eu-west-1"]),
            400,
        ),
        (
            "location the group holds",
            app_id.as_str(),
            Some(vec!["us-west-2"]),
            201,
        ),
        (
            "first held location of a priority list",
            app_id.as_str(),
            Some(vec!["us-west-2", "eu-west-1"]),
            201,
        ),
    ];
    for (case, application, locations, expected) in cases {
        let mut body = json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": application,
        });
        if let Some(locations) = locations {
            body["Locations"] = json!(locations);
        }
        let (status, got) = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got}"
        );
    }
}

/// One row of [`each_resource_store_is_bounded_by_its_quota`]: the resource it
/// creates, where, how to build the nth request body, and the emulator's bound.
struct QuotaCase {
    case: &'static str,
    path: &'static str,
    body: fn(usize) -> Value,
    quota: usize,
}

/// Each store this slice introduces is bounded: a create past the emulator's own
/// quota is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// rather than growing the store further.
#[tokio::test]
async fn each_resource_store_is_bounded_by_its_quota() {
    let cases = [
        QuotaCase {
            case: "applications",
            path: "/applications",
            body: |index| application_body(&format!("quota-app-{index}")),
            quota: APPLICATION_QUOTA,
        },
        QuotaCase {
            case: "stream groups",
            path: "/streamgroups",
            body: |index| {
                json!({
                    "Description": format!("quota-group-{index}"),
                    "StreamClass": "gen5n_high",
                })
            },
            quota: STREAM_GROUP_QUOTA,
        },
    ];
    for QuotaCase {
        case,
        path,
        body,
        quota,
    } in cases
    {
        let router = emulator();
        for index in 0..quota {
            let (status, got) = call(&router, "POST", path, Some(body(index))).await;
            assert_eq!(
                status, 201,
                "case {case:?}: create {index} within the quota expected 201, got {status} {got}"
            );
        }
        let (status, got) = call(&router, "POST", path, Some(body(quota))).await;
        assert_eq!(
            status, 402,
            "case {case:?}: the create past the quota of {quota} expected 402 \
             ServiceQuotaExceededException, got {status} {got}"
        );
    }
}

/// One stream group's session store is bounded too: sessions that reached a
/// terminal status are released to make room, and a group whose live sessions fill
/// the bound refuses the start, so no number of `StartStreamSession` calls grows
/// the store past the bound.
#[tokio::test]
async fn a_stream_group_holds_a_bounded_number_of_sessions() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "bounded-sessions").await;
    let (group_id, _) =
        active_stream_group(&router, "bounded-group", &app_id, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}/streamsessions");
    let body = json!({
        "Protocol": "WebRTC",
        "SignalRequest": "offer",
        "ApplicationIdentifier": app_id,
    });

    // Ask for well past the bound. Each answer is either the session (201) or the
    // modelled refusal (409) — never anything else, and never an empty success.
    let attempts = SESSIONS_PER_STREAM_GROUP + 30;
    let mut started: Vec<String> = Vec::new();
    for index in 0..attempts {
        let (status, got) = call(&router, "POST", &path, Some(body.clone())).await;
        assert!(
            status == 201 || status == 409,
            "attempt {index}: expected 201 or 409 ConflictException, got {status} {got}"
        );
        if status == 201 {
            started.push(text(&got, "/Arn", "a started session"));
        }
    }

    // Once the clock has resolved the placements, the group takes a new session
    // again: the terminal ones were released rather than filling the store.
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(&router, "POST", &path, Some(body)).await;
    assert_eq!(
        status, 201,
        "after the placements resolved: expected 201, got {status} {got}"
    );

    // And the store never holds more than the bound: only that many of every
    // session ever started are still retrievable.
    let mut retrievable = 0;
    for arn in &started {
        let session_id = arn
            .rsplit('/')
            .next()
            .expect("a session ARN ends in the session id");
        let (status, _) = call(
            &router,
            "GET",
            &format!("/streamgroups/{group_id}/streamsessions/{session_id}"),
            None,
        )
        .await;
        if status == 200 {
            retrievable += 1;
        }
    }
    assert!(
        retrievable <= SESSIONS_PER_STREAM_GROUP,
        "the session store must hold at most {SESSIONS_PER_STREAM_GROUP} sessions, but \
         {retrievable} of the {} started are still stored",
        started.len()
    );
}

/// Every optional member `StartStreamSession` accepts is STORED and echoed from
/// state, so a discarded write can never look like a stored one.
#[tokio::test]
async fn the_optional_session_configuration_round_trips_through_state() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "config-app").await;
    let (group_id, _) =
        active_stream_group(&router, "config-group", &app_id, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}/streamsessions");

    // Each row sets a different subset of the optional configuration; the response
    // and every later read must carry back exactly what was sent. The resolutions
    // stay inside `Resolution`'s published bounds (Width/Height 320..4096, at most
    // 2,073,600 pixels in total, both even).
    let cases: Vec<(&str, Value)> = vec![
        ("nothing extra", json!({})),
        (
            "the smallest monitor",
            json!({"DisplayConfiguration": {"Resolution": {"Width": 320, "Height": 320}}}),
        ),
        (
            "a 720p monitor",
            json!({"DisplayConfiguration": {"Resolution": {"Width": 1280, "Height": 720}}}),
        ),
        (
            "a monitor at the pixel budget",
            json!({"DisplayConfiguration": {"Resolution": {"Width": 1920, "Height": 1080}}}),
        ),
        (
            "performance stats shared",
            json!({"PerformanceStatsConfiguration": {"SharedWithClient": true}}),
        ),
        (
            "performance stats withheld",
            json!({"PerformanceStatsConfiguration": {"SharedWithClient": false}}),
        ),
        (
            "a role to assume",
            json!({"RoleArn": "arn:aws:iam::000000000000:role/gameliftstreams-session"}),
        ),
        (
            "all three at once",
            json!({
                "DisplayConfiguration": {"Resolution": {"Width": 1024, "Height": 768}},
                "PerformanceStatsConfiguration": {"SharedWithClient": true},
                "RoleArn": "arn:aws:iam::000000000000:role/gameliftstreams-session",
            }),
        ),
    ];
    for (case, extra) in cases {
        let mut body = json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_id,
        });
        for (member, value) in extra.as_object().expect("each row is a JSON object") {
            body[member] = value.clone();
        }
        let (status, started) = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} {started}"
        );
        let session_id = text(&started, "/Arn", case)
            .rsplit('/')
            .next()
            .expect("a session ARN ends in the session id")
            .to_string();
        for member in [
            "DisplayConfiguration",
            "PerformanceStatsConfiguration",
            "RoleArn",
        ] {
            assert_eq!(
                started.get(member),
                extra.get(member),
                "case {case:?}: StartStreamSession must answer with the stored {member}, \
                 got {started}"
            );
        }
        let (status, got) = call(&router, "GET", &format!("{path}/{session_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamSession 200, got {status} {got}"
        );
        for member in [
            "DisplayConfiguration",
            "PerformanceStatsConfiguration",
            "RoleArn",
        ] {
            assert_eq!(
                got.get(member),
                extra.get(member),
                "case {case:?}: GetStreamSession must answer with the stored {member}, got {got}"
            );
        }
    }
}

/// A virtual monitor must have even dimensions and must not exceed the published
/// pixel budget — rules `Resolution`'s documentation states and the model cannot
/// express, so the handler refuses them with `ValidationException` (400).
#[tokio::test]
async fn a_virtual_monitor_obeys_its_published_resolution_rules() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "monitor-app").await;
    let (group_id, _) =
        active_stream_group(&router, "monitor-group", &app_id, &["us-west-2"]).await;
    let path = format!("/streamgroups/{group_id}/streamsessions");

    // (case, Width, Height, expected status). `Resolution` publishes Width/Height
    // 320..4096 and "The total number of pixels (width x height) must not exceed
    // 2,073,600"; the rows span the lower bound, the pixel budget exactly, one past
    // it, and both parities.
    let cases: [(&str, i64, i64, u16); 7] = [
        ("the published lower bound", 320, 320, 201),
        ("720p", 1280, 720, 201),
        ("exactly the pixel budget", 1920, 1080, 201),
        ("one even step past the budget", 1922, 1080, 400),
        ("4K, far past the budget", 3840, 2160, 400),
        ("an odd width", 1281, 720, 400),
        ("an odd height", 1280, 721, 400),
    ];
    for (case, width, height, expected) in cases {
        let body = json!({
            "Protocol": "WebRTC",
            "SignalRequest": "offer",
            "ApplicationIdentifier": app_id,
            "DisplayConfiguration": {"Resolution": {"Width": width, "Height": height}},
        });
        let (status, got) = call(&router, "POST", &path, Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({width}x{height}): expected {expected}, got {status} {got}"
        );
    }
}

/// `LocationConfiguration`'s capacity members are modelled `@range(min: 0)`, and
/// this service's capacity arithmetic reads them back out of state, so a negative
/// one is refused rather than stored.
#[tokio::test]
async fn a_stream_group_refuses_a_negative_location_capacity() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "capacity-app").await;

    // (case, the member, its value, expected status). The rows span each capacity
    // member, the published lower bound, and the extremes either side of it.
    let cases: [(&str, &str, i64, u16); 10] = [
        ("always-on at the lower bound", "AlwaysOnCapacity", 0, 201),
        ("always-on above it", "AlwaysOnCapacity", 2, 201),
        ("always-on one below it", "AlwaysOnCapacity", -1, 400),
        ("always-on at i32::MIN", "AlwaysOnCapacity", -2_147_483_648, 400),
        ("on-demand at the lower bound", "OnDemandCapacity", 0, 201),
        ("on-demand one below it", "OnDemandCapacity", -1, 400),
        ("target-idle at the lower bound", "TargetIdleCapacity", 0, 201),
        ("target-idle one below it", "TargetIdleCapacity", -7, 400),
        ("maximum at the lower bound", "MaximumCapacity", 0, 201),
        ("maximum one below it", "MaximumCapacity", -1, 400),
    ];
    for (case, member, value, expected) in cases {
        let mut configuration = json!({"LocationName": "us-west-2"});
        configuration[member] = json!(value);
        let (status, got) = call(
            &router,
            "POST",
            "/streamgroups",
            Some(json!({
                "Description": "capacity-group",
                "StreamClass": "gen5n_high",
                "DefaultApplicationIdentifier": app_id,
                "LocationConfigurations": [configuration],
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} ({member}={value}): expected {expected}, got {status} {got}"
        );
        if status == 201 {
            // Whatever the capacity, the reported idle capacity is never negative
            // and the arithmetic behind it never overflows.
            let idle = got["LocationStates"][0]["IdleCapacity"].as_i64();
            assert_eq!(
                idle,
                Some(0),
                "case {case:?}: a group that is still ACTIVATING holds nothing idle, got {got}"
            );
        }
    }
}

/// An application that a stream group still refers to cannot be deleted: "If any
/// active stream groups exist for this application, this request returns a
/// ValidationException." And one that is still `INITIALIZED` cannot be deleted
/// either: "You cannot delete an application that's in PROCESSING or INITIALIZED
/// status."
#[tokio::test]
async fn deleting_an_application_respects_its_documented_conditions() {
    let router = emulator();

    // Still INITIALIZED.
    let (status, created) = call(
        &router,
        "POST",
        "/applications",
        Some(application_body("fresh-app")),
    )
    .await;
    assert_eq!(status, 201, "CreateApplication: got {status} {created}");
    let app_id = text(&created, "/Id", "fresh-app");
    let (status, got) = call(&router, "DELETE", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 409,
        "case \"application still INITIALIZED\": expected 409, got {status} {got}"
    );

    // READY, but a stream group refers to it.
    std::thread::sleep(PAST_TRANSITION);
    let (_, group_arn) = active_stream_group(&router, "holder", &app_id, &["us-west-2"]).await;
    let (status, got) = call(&router, "DELETE", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 400,
        "case \"application held by a stream group\": expected 400, got {status} {got}"
    );

    // Once the group is gone, the delete goes through.
    let (status, got) = call(&router, "DELETE", &format!("/streamgroups/{}", as_label(&group_arn)), None).await;
    assert_eq!(
        status, 204,
        "DeleteStreamGroup by ARN: expected 204, got {status} {got}"
    );
    let (status, got) = call(&router, "DELETE", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 204,
        "case \"application released by its stream group\": expected 204, got {status} {got}"
    );
}
