// SPDX-License-Identifier: CC0-1.0

//! `CreateStreamSessionAdminShell` end to end, over the same router the emulator
//! serves.
//!
//! The rule under test: the operation addresses the stream session its two path
//! labels name, and it answers from the STORED session — an identifier that can
//! name no resource of its label's type is refused 400 `ValidationException`
//! before the store is read, an identifier that names nothing stored is 404
//! `ResourceNotFoundException`, and a session whose status is not one of the
//! statuses the operation documents as eligible ("The stream session must be in
//! one of the following statuses: ACTIVE, CONNECTED, PENDING_CLIENT_RECONNECTION,
//! or RECONNECTING") is refused 400 `ValidationException` naming that session and
//! the eligible statuses. Nothing it answers moves the session.
//!
//! The first table spans the INELIGIBLE status class: every status a stream
//! session can hold in this emulator that the operation refuses (started,
//! placement failed, terminating, terminated), each asserted against the status the
//! session reads back as. A session whose placement SUCCEEDED gets past this gate,
//! which `stream_session_placement.rs` covers. The second table spans the
//! identifier class: unknown group, unknown session, either label carrying another
//! resource type's ARN, and both labels in ARN form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    active_stream_group, as_label, call, call_with_headers, emulator, ready_application, text,
    ACCOUNT, PAST_TRANSITION, REGION,
};

/// How the service refuses a session that is not in one of the statuses
/// `CreateStreamSessionAdminShell` documents as eligible, over the stream session's
/// own ARN. The statuses are values of the model's `StreamSessionStatus`
/// (`smithy_query(query="shape", shape="StreamSessionStatus")`: ACTIVATING, ACTIVE,
/// CONNECTED, PENDING_CLIENT_RECONNECTION, RECONNECTING, TERMINATING, TERMINATED,
/// ERROR), in the order the operation's documentation lists the eligible ones.
fn not_eligible(session_arn: &str) -> String {
    format!(
        "Stream session '{session_arn}' is not in ACTIVE, CONNECTED, \
         PENDING_CLIENT_RECONNECTION or RECONNECTING status for session access"
    )
}

/// Start one stream session in `group` and return its id and ARN. On return the
/// session is `ACTIVATING`: the clock owes it its next status.
async fn started_session(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    application: &str,
) -> (String, String) {
    let (status, session) = call(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n",
            "ApplicationIdentifier": application,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected StartStreamSession 201, got {status} {session}"
    );
    let arn = text(&session, "/Arn", case);
    let id = arn
        .rsplit('/')
        .next()
        .expect("a stream session ARN ends in the session id")
        .to_string();
    (id, arn)
}

/// The `Status` and `LastUpdatedAt` a session reads back as, through the
/// implemented reader.
async fn read_session(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    session: &str,
) -> (String, Value) {
    let (status, got) = call(
        router,
        "GET",
        &format!("/streamgroups/{group}/streamsessions/{session}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetStreamSession 200, got {status} {got}"
    );
    (
        text(&got, "/Status", case),
        got.get("LastUpdatedAt").cloned().unwrap_or(Value::Null),
    )
}

/// Call `CreateStreamSessionAdminShell` and return the status, the wire error code
/// and the modelled `Message`.
async fn admin_shell(
    router: &MultiServiceRouter,
    group: &str,
    session: &str,
) -> (u16, String, String) {
    let (status, headers, body) = call_with_headers(
        router,
        "POST",
        &format!("/streamgroups/{group}/streamsessions/{session}/access"),
        None,
    )
    .await;
    let code = headers
        .get("x-amzn-errortype")
        .cloned()
        .unwrap_or_else(|| format!("<no x-amzn-errortype header; body {body}>"));
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// Every status a stream session can hold in this emulator is ineligible for a
/// terminal session, and each one is refused 400 `ValidationException` naming the
/// status the stored session holds and the statuses the operation documents — never
/// 501, and never a success.
#[tokio::test]
async fn an_ineligible_stream_session_status_is_refused_as_a_validation_error() {
    let router = emulator();
    let (app_id, _) = ready_application(&router, "admin-shell-statuses").await;
    let (group_id, _) =
        active_stream_group(&router, "admin-shell-status-group", &app_id, &["us-west-2"]).await;

    // (case, terminate the session first, wait out the transition the clock owes,
    // the status the session is then in, whether that status is TERMINAL). The rows
    // differ along how far through its lifecycle the session is: just started,
    // settled by the clock, asked to stop, and stopped.
    //
    // The group holds ONE stream of capacity, which the first row's session is
    // placed on as soon as the clock resolves it, so the placement of the second
    // row's session finds none and ends in ERROR — the `placementTimeout` outcome
    // `stream_session_placement.rs` pins.
    let cases: [(&str, bool, bool, &str, bool); 4] = [
        ("a freshly started session", false, false, "ACTIVATING", false),
        ("a session whose placement failed", false, true, "ERROR", true),
        ("a session asked to terminate", true, false, "TERMINATING", false),
        ("a terminated session", true, true, "TERMINATED", true),
    ];
    for (case, terminate, settle, expected_status, terminal) in cases {
        let (session_id, session_arn) = started_session(&router, case, &group_id, &app_id).await;
        if terminate {
            let (status, body) = call(
                &router,
                "DELETE",
                &format!("/streamgroups/{group_id}/streamsessions/{session_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected TerminateStreamSession 204, got {status} {body}"
            );
        }
        if settle {
            std::thread::sleep(PAST_TRANSITION);
        }
        let (status_before, updated_before) =
            read_session(&router, case, &group_id, &session_id).await;
        assert_eq!(
            status_before, expected_status,
            "case {case:?}: expected the session to be {expected_status}, got {status_before}"
        );

        let (status, code, message) = admin_shell(&router, &group_id, &session_id).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException for a {expected_status} \
             session, got {status} {code:?} {message:?}"
        );
        let expected_message = not_eligible(&session_arn);
        assert!(
            message.starts_with(&expected_message),
            "case {case:?}: expected the refusal {expected_message:?}, got {message:?}"
        );
        assert!(
            message.contains("(Internal ID: "),
            "case {case:?}: this service's handler messages carry a per-request internal \
             id, got {message:?}"
        );

        // A refused call persists nothing. Asserted on the rows whose status is
        // TERMINAL, which the clock owes no further transition from, so the
        // comparison answers "did the refusal write?" rather than "how long did
        // these three calls take?" — a transient status would move on its own.
        if terminal {
            let (status_after, updated_after) =
                read_session(&router, case, &group_id, &session_id).await;
            assert_eq!(
                (status_after.as_str(), &updated_after),
                (status_before.as_str(), &updated_before),
                "case {case:?}: the refusal must not move the session, got \
                 {status_after} {updated_after}"
            );
        }
    }
}

/// The two path labels are resolved the way every session-scoped operation of this
/// service resolves them: a label that can name no resource of its own type is a
/// 400 `ValidationException` decided from the request, an identifier naming nothing
/// stored is a 404 `ResourceNotFoundException` reported against the composite stream
/// session ARN, and either label in ARN form resolves the same stored session a bare
/// id does.
#[tokio::test]
async fn the_path_labels_resolve_the_stream_session_they_name() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "admin-shell-identifiers").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "admin-shell-id-group", &app_id, &["us-west-2"]).await;
    let (session_id, session_arn) =
        started_session(&router, "identifier rows", &group_id, &app_id).await;

    // (case, group label, session label, expected status, expected error code,
    // a fragment of the expected message).
    let cases: [(&str, String, String, u16, &str, String); 6] = [
        (
            "an unknown stream group",
            "sg-missing".to_string(),
            session_id.clone(),
            404,
            "ResourceNotFoundException",
            format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/sg-missing/{session_id}"),
        ),
        (
            "an unknown stream session",
            group_id.clone(),
            "ss-missing".to_string(),
            404,
            "ResourceNotFoundException",
            format!(
                "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group_id}/ss-missing"
            ),
        ),
        (
            "a stream group named by an application ARN",
            as_label(&app_arn),
            session_id.clone(),
            400,
            "ValidationException",
            "'Identifier' is not a valid resource ARN of type 'streamgroup'".to_string(),
        ),
        (
            "a stream session named by an application ARN",
            group_id.clone(),
            as_label(&app_arn),
            400,
            "ValidationException",
            "'StreamSessionIdentifier' is not a valid resource ARN of type 'streamsession'"
                .to_string(),
        ),
        (
            "the stream group by ARN",
            as_label(&group_arn),
            session_id.clone(),
            400,
            "ValidationException",
            not_eligible(&session_arn),
        ),
        (
            "the stream session by ARN",
            group_id.clone(),
            as_label(&session_arn),
            400,
            "ValidationException",
            not_eligible(&session_arn),
        ),
    ];
    for (case, group_label, session_label, expected_status, expected_code, fragment) in cases {
        let (status, code, message) = admin_shell(&router, &group_label, &session_label).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             {code:?} {message:?}"
        );
        assert!(
            message.contains(&fragment),
            "case {case:?}: expected a message carrying {fragment:?}, got {message:?}"
        );
    }
}
