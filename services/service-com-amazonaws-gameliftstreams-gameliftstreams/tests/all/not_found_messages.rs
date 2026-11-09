// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` message every not-found path of this service
//! builds from the REQUEST.
//!
//! The rule under test, over the whole class of not-found paths rather than one
//! recorded request: a `ResourceNotFoundException` names the full ARN that the
//! identifier the request carried resolves to — `Resource '<arn>' does not exist`
//! for the resource the operation addresses, `Stream session '<arn>' does not
//! exist` with a `streamsession/<group>/<session>` ARN composed from BOTH path
//! labels for the session-scoped operations — and appends a per-request
//! correlation id as ` (Internal ID: <uuid>)`.
//!
//! The model (`smithy_query(query="shape", shape="ResourceNotFoundException")`)
//! types the error with a single required `Message` and `@httpError 404` and says
//! nothing about the text, so what is asserted here is the STRUCTURE the service
//! builds from the request — the resolved ARN and the suffix — never a recorded
//! id value: the correlation id is only required to be a UUID and to differ
//! between two requests.
//!
//! The table's rows differ along the operation, its HTTP method, the resource kind,
//! and the
//! form of each identifier the request carries: `Identifier` is modelled as "an
//! Amazon Resource Name (ARN) or ID" (`length 1..128, pattern
//! ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$`),
//! so a bare id resolves against the request's region and this account while an
//! identifier already in ARN form — including one naming another region and
//! account — is its own resolved ARN.

use serde_json::{json, Value};

use super::stream_workflow::{
    active_stream_group, as_label, call, emulator, ready_application, text, ACCOUNT, REGION,
};

/// An ARN that names a resource in another Region and account: still a legal
/// `Identifier`, and already resolved.
const ELSEWHERE: &str = "arn:aws:gameliftstreams:eu-west-1:111122223333:application/a-elsewhere";

/// The error code a restJson1 error body carries.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .or_else(|| body["Code"].as_str())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `<subject> '<arn>' does not exist (Internal ID: <uuid>)`, split into its three
/// parts, or `None` when the message does not follow the form at all.
fn parse_not_found(message: &str) -> Option<(String, String, String)> {
    let (head, internal_id) = message
        .strip_suffix(')')?
        .rsplit_once(" (Internal ID: ")?;
    let (subject, arn) = head.strip_suffix("' does not exist")?.split_once(" '")?;
    Some((
        subject.to_string(),
        arn.to_string(),
        internal_id.to_string(),
    ))
}

/// A canonical 8-4-4-4-12 lowercase-hex UUID, which is all the per-request
/// correlation id has to be.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<usize> = value.split('-').map(str::len).collect();
    groups == [8, 4, 4, 4, 12]
        && value
            .chars()
            .all(|ch| ch == '-' || (ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()))
}

/// Assert one answer is a 404 `ResourceNotFoundException` whose message names
/// `subject` and `arn`, and return the correlation id it carried.
fn assert_not_found(status: u16, body: &Value, subject: &str, arn: &str, case: &str) -> String {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {body}"
    );
    let message = text(body, "/Message", case);
    let Some((got_subject, got_arn, internal_id)) = parse_not_found(&message) else {
        panic!(
            "case {case:?}: expected \"<subject> '<arn>' does not exist \
             (Internal ID: <uuid>)\", got {message:?}"
        );
    };
    assert_eq!(
        got_subject, subject,
        "case {case:?}: expected the message to open with {subject:?}, got {message:?}"
    );
    assert_eq!(
        got_arn, arn,
        "case {case:?}: expected the resolved ARN {arn:?}, got {got_arn:?} in {message:?}"
    );
    assert!(
        is_uuid(&internal_id),
        "case {case:?}: expected a UUID internal id, got {internal_id:?} in {message:?}"
    );
    internal_id
}

/// A `StartStreamSession` body naming `application`.
fn start_body(application: &str) -> Value {
    json!({
        "Protocol": "WebRTC",
        "SignalRequest": "{\"type\":\"offer\",\"sdp\":\"v=0\"}",
        "ApplicationIdentifier": application,
    })
}

/// A `LocationConfigurations` body both `AddStreamGroupLocations` and
/// `CreateStreamGroup` accept.
fn one_location() -> Value {
    json!([{"LocationName": REGION, "AlwaysOnCapacity": 1}])
}

/// One not-found path: the case name, the HTTP method and path to request, the
/// request body, and the subject and resolved ARN its message must name.
type Row = (
    String,
    &'static str,
    String,
    Option<Value>,
    &'static str,
    String,
);

#[tokio::test]
async fn every_not_found_path_names_the_resolved_arn_and_a_per_request_internal_id() {
    let router = emulator();
    // The fixtures the rows that must get PAST an existing resource need.
    let (app_id, app_arn) = ready_application(&router, "not-found-app").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "not-found-group", &app_arn, &[REGION]).await;

    let missing_app_arn = format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/a-gone");
    let missing_group_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/sg-gone");
    // The composite ARN of a session in a stream group that is not there, and of one
    // in the group that is.
    let session_in_missing_group =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/sg-gone/ss-gone");
    let session_in_group = format!(
        "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group_id}/ss-gone"
    );

    // (case, method, path, body, expected subject, expected resolved ARN)
    let rows: Vec<Row> = vec![
        // ---- resource-scoped, the identifier in the path label ----
        (
            "GetApplication by bare id".to_string(),
            "GET",
            "/applications/a-gone".to_string(),
            None,
            "Resource",
            missing_app_arn.clone(),
        ),
        (
            "GetApplication by an ARN in another Region and account".to_string(),
            "GET",
            format!("/applications/{}", as_label(ELSEWHERE)),
            None,
            "Resource",
            ELSEWHERE.to_string(),
        ),
        (
            "DeleteApplication by bare id".to_string(),
            "DELETE",
            "/applications/a-gone".to_string(),
            None,
            "Resource",
            missing_app_arn.clone(),
        ),
        (
            "DeleteApplication by an ARN in another Region and account".to_string(),
            "DELETE",
            format!("/applications/{}", as_label(ELSEWHERE)),
            None,
            "Resource",
            ELSEWHERE.to_string(),
        ),
        (
            "GetStreamGroup by bare id".to_string(),
            "GET",
            "/streamgroups/sg-gone".to_string(),
            None,
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "GetStreamGroup by ARN".to_string(),
            "GET",
            format!("/streamgroups/{}", as_label(&missing_group_arn)),
            None,
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "UpdateStreamGroup by bare id".to_string(),
            "PATCH",
            "/streamgroups/sg-gone".to_string(),
            Some(json!({"Description": "unreachable"})),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "UpdateStreamGroup by ARN".to_string(),
            "PATCH",
            format!("/streamgroups/{}", as_label(&missing_group_arn)),
            Some(json!({"Description": "unreachable"})),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "AddStreamGroupLocations by bare id".to_string(),
            "POST",
            "/streamgroups/sg-gone/locations".to_string(),
            Some(json!({"LocationConfigurations": one_location()})),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "AddStreamGroupLocations by ARN".to_string(),
            "POST",
            format!(
                "/streamgroups/{}/locations",
                as_label(&missing_group_arn)
            ),
            Some(json!({"LocationConfigurations": one_location()})),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "DeleteStreamGroup by bare id".to_string(),
            "DELETE",
            "/streamgroups/sg-gone".to_string(),
            None,
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "DeleteStreamGroup by ARN".to_string(),
            "DELETE",
            format!("/streamgroups/{}", as_label(&missing_group_arn)),
            None,
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "StartStreamSession in a stream group that is not there".to_string(),
            "POST",
            "/streamgroups/sg-gone/streamsessions".to_string(),
            Some(start_body(&app_id)),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "StartStreamSession in a stream group ARN that is not there".to_string(),
            "POST",
            format!(
                "/streamgroups/{}/streamsessions",
                as_label(&missing_group_arn)
            ),
            Some(start_body(&app_id)),
            "Resource",
            missing_group_arn.clone(),
        ),
        // ---- resource-scoped, the identifier in the BODY ----
        (
            "CreateStreamGroup with a DefaultApplicationIdentifier that is not there"
                .to_string(),
            "POST",
            "/streamgroups".to_string(),
            Some(json!({
                "Description": "group-with-a-missing-application",
                "StreamClass": "gen5n_high",
                "DefaultApplicationIdentifier": "a-gone",
                "LocationConfigurations": one_location(),
            })),
            "Resource",
            missing_app_arn.clone(),
        ),
        (
            "DisassociateApplications with an ApplicationIdentifiers entry that is not there"
                .to_string(),
            "POST",
            format!("/streamgroups/{group_id}/disassociations"),
            Some(json!({"ApplicationIdentifiers": ["a-gone"]})),
            "Resource",
            missing_app_arn.clone(),
        ),
        (
            "DisassociateApplications in a stream group that is not there".to_string(),
            "POST",
            "/streamgroups/sg-gone/disassociations".to_string(),
            Some(json!({"ApplicationIdentifiers": [app_id.clone()]})),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "StartStreamSession with an ApplicationIdentifier that is not there".to_string(),
            "POST",
            format!("/streamgroups/{group_id}/streamsessions"),
            Some(start_body("a-gone")),
            "Resource",
            missing_app_arn.clone(),
        ),
        // ---- session-scoped: the stream session is the resource addressed, even
        // when it is the enclosing stream group that is absent ----
        (
            "GetStreamSession in a stream group that is not there".to_string(),
            "GET",
            "/streamgroups/sg-gone/streamsessions/ss-gone".to_string(),
            None,
            "Stream session",
            session_in_missing_group.clone(),
        ),
        (
            "GetStreamSession of a session the stream group does not hold".to_string(),
            "GET",
            format!("/streamgroups/{group_id}/streamsessions/ss-gone"),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "GetStreamSession with the stream group named by ARN".to_string(),
            "GET",
            format!(
                "/streamgroups/{}/streamsessions/ss-gone",
                as_label(&group_arn)
            ),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "GetStreamSession with the session named by ARN".to_string(),
            "GET",
            format!(
                "/streamgroups/{group_id}/streamsessions/{}",
                as_label(&session_in_group)
            ),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "TerminateStreamSession in a stream group that is not there".to_string(),
            "DELETE",
            "/streamgroups/sg-gone/streamsessions/ss-gone".to_string(),
            None,
            "Stream session",
            session_in_missing_group.clone(),
        ),
        (
            "TerminateStreamSession of a session the stream group does not hold".to_string(),
            "DELETE",
            format!("/streamgroups/{group_id}/streamsessions/ss-gone"),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "TerminateStreamSession with the stream group named by ARN".to_string(),
            "DELETE",
            format!(
                "/streamgroups/{}/streamsessions/ss-gone",
                as_label(&group_arn)
            ),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "TerminateStreamSession with the session named by ARN".to_string(),
            "DELETE",
            format!(
                "/streamgroups/{group_id}/streamsessions/{}",
                as_label(&session_in_group)
            ),
            None,
            "Stream session",
            session_in_group.clone(),
        ),
        // An `Identifier` naming another KIND of resource is not a not-found path at
        // all: an ARN that names no stream group is a bad value for the label, so it
        // is refused 400 `ValidationException` before the store is read. That class
        // is tabled in `refusal_envelope.rs`, which is why no row for it is here.
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, method, path, body, subject, arn) in &rows {
        let (status, got) = call(&router, method, path, body.clone()).await;
        let internal_id = assert_not_found(status, &got, subject, arn, case);
        assert!(
            !seen.contains(&internal_id),
            "case {case:?}: the internal id identifies ONE request, but {internal_id:?} \
             was already used"
        );
        seen.push(internal_id);
    }

    // None of those refusals touched the store: both fixtures still read back.
    let (status, got) = call(&router, "GET", &format!("/applications/{app_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetApplication after the refusals: expected 200, got {status} {got}"
    );
    assert_eq!(
        text(&got, "/Arn", "the fixture application"),
        app_arn,
        "the refusals must leave the application exactly as it was, got {got}"
    );
    let (status, got) = call(&router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup after the refusals: expected 200, got {status} {got}"
    );
    assert_eq!(
        got["Description"], "not-found-group",
        "the refusals must leave the stream group exactly as it was, got {got}"
    );
    assert_eq!(
        got["DefaultApplication"]["Arn"],
        Value::String(app_arn.clone()),
        "a refused call must not change the default application, got {got}"
    );
}

#[tokio::test]
async fn two_identical_not_found_requests_differ_only_in_the_internal_id() {
    let router = emulator();
    let case = "the same GetApplication twice";
    let expected = format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:application/a-gone");

    let (first_status, first) = call(&router, "GET", "/applications/a-gone", None).await;
    let first_id = assert_not_found(first_status, &first, "Resource", &expected, case);
    let (second_status, second) = call(&router, "GET", "/applications/a-gone", None).await;
    let second_id = assert_not_found(second_status, &second, "Resource", &expected, case);

    assert_ne!(
        first_id, second_id,
        "case {case:?}: the internal id identifies one request, so two requests cannot \
         share {first_id:?}"
    );
    // Everything else about the two messages is identical: the ARN comes from the
    // request, not from the clock or a counter.
    let strip = |message: &str, internal_id: &str| message.replace(internal_id, "<id>");
    assert_eq!(
        strip(&text(&first, "/Message", case), &first_id),
        strip(&text(&second, "/Message", case), &second_id),
        "case {case:?}: only the internal id may differ between two identical requests"
    );
}

#[tokio::test]
async fn a_session_that_exists_is_not_reported_missing() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "live-session-app").await;
    let (group_id, _) =
        active_stream_group(&router, "live-session-group", &app_arn, &[REGION]).await;

    let (status, started) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(start_body(&app_id)),
    )
    .await;
    assert_eq!(
        status, 201,
        "StartStreamSession: expected 201, got {status} {started}"
    );
    let session_arn = text(&started, "/Arn", "a started stream session");

    // The session-scoped lookup still finds the session the start persisted, by bare
    // id and by ARN, so the shared not-found path did not swallow the success one.
    let session_id = session_arn
        .rsplit('/')
        .next()
        .expect("a stream session ARN ends in its id")
        .to_string();
    for (case, identifier) in [
        ("bare id", session_id.clone()),
        ("ARN", as_label(&session_arn)),
    ] {
        let (status, got) = call(
            &router,
            "GET",
            &format!("/streamgroups/{group_id}/streamsessions/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetStreamSession 200, got {status} {got}"
        );
        assert_eq!(
            text(&got, "/Arn", case),
            session_arn,
            "case {case:?}: both identifier forms name the same session, got {got}"
        );
    }
}
