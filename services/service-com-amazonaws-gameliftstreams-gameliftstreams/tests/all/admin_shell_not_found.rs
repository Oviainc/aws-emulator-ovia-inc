// SPDX-License-Identifier: CC0-1.0

//! Which resource `CreateStreamSessionAdminShell` reports as missing.
//!
//! The rule under test: the operation resolves its two path labels OUTSIDE-IN —
//! the stream group named by `Identifier` ("The stream group that runs this stream
//! session") before the stream session named by `StreamSessionIdentifier` — and a
//! 404 `ResourceNotFoundException` names the resource that actually failed to
//! resolve. A request whose stream group is absent is therefore reported against
//! the GROUP, through this service's generic `Resource '<arn>' does not exist`
//! text over the stream-group ARN that label resolves to, whatever the second
//! label carries; only a request whose group resolved reports the stream session,
//! over the composite `streamsession/<group>/<session>` ARN.
//!
//! The rows differ along what the rule generalises over: which of the two
//! resources is missing, and the form of each identifier the request carries.
//! `Identifier` is modelled as "an Amazon Resource Name (ARN) or ID" (`length
//! 1..128, pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:\n]*):([0-9]{12}):([^:\n]*)$)$`),
//! so a bare id resolves against the request's region and this account while an
//! identifier already in ARN form — including one naming another Region and
//! account — is its own resolved ARN.
//!
//! Every message is also checked for the per-request correlation id this service
//! appends to every handler message; its value is per run and nothing is asserted
//! about it beyond being a distinct UUID.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    active_stream_group, as_label, call, call_with_headers, emulator, ready_application, text,
    ACCOUNT, PAST_TRANSITION, REGION,
};

/// A stream group ARN in another Region and account: still a legal `Identifier`,
/// and already resolved, so it is reported exactly as the caller wrote it.
const ELSEWHERE_GROUP: &str =
    "arn:aws:gameliftstreams:eu-west-1:111122223333:streamgroup/sg-elsewhere";

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

/// Call `CreateStreamSessionAdminShell` over the two path labels and return the
/// status, the wire error code and the body.
async fn admin_shell(
    router: &MultiServiceRouter,
    group: &str,
    session: &str,
) -> (u16, String, Value) {
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
    (status, code, body)
}

/// Start one stream session in `group` and return its id and ARN.
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

#[tokio::test]
async fn the_resource_that_failed_to_resolve_is_the_one_reported_missing() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "admin-shell-miss").await;
    let (group_id, group_arn) =
        active_stream_group(&router, "admin-shell-miss-group", &app_arn, &[REGION]).await;

    let missing_group = "sg-gone";
    let missing_group_arn =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamgroup/{missing_group}");
    let session_in_missing_group = format!(
        "arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{missing_group}/ss-gone"
    );
    let session_in_group =
        format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:streamsession/{group_id}/ss-gone");

    // (case, Identifier label, StreamSessionIdentifier label, expected subject,
    // expected resolved ARN). The first block is the group-missing class — the
    // group is what failed to resolve, so the generic resource text names the
    // GROUP whatever the session label says; the second is the session-missing
    // class, reached only once the group resolved.
    let rows: Vec<(String, String, String, &'static str, String)> = vec![
        (
            "a stream group that is not there, by bare id".to_string(),
            missing_group.to_string(),
            "ss-gone".to_string(),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "a stream group that is not there, by ARN".to_string(),
            as_label(&missing_group_arn),
            "ss-gone".to_string(),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "a stream group that is not there, by an ARN in another Region and account"
                .to_string(),
            as_label(ELSEWHERE_GROUP),
            "ss-gone".to_string(),
            "Resource",
            ELSEWHERE_GROUP.to_string(),
        ),
        (
            "a stream group that is not there, with the session named by ARN".to_string(),
            missing_group.to_string(),
            as_label(&session_in_missing_group),
            "Resource",
            missing_group_arn.clone(),
        ),
        (
            "a session the stream group does not hold, by bare id".to_string(),
            group_id.clone(),
            "ss-gone".to_string(),
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "a session the stream group does not hold, the group named by ARN".to_string(),
            as_label(&group_arn),
            "ss-gone".to_string(),
            "Stream session",
            session_in_group.clone(),
        ),
        (
            "a session the stream group does not hold, named by ARN".to_string(),
            group_id.clone(),
            as_label(&session_in_group),
            "Stream session",
            session_in_group.clone(),
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, group_label, session_label, subject, arn) in &rows {
        let (status, code, body) = admin_shell(&router, group_label, session_label).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} \
             {code:?} {body}"
        );
        let message = text(&body, "/Message", case);
        let Some((got_subject, got_arn, internal_id)) = parse_not_found(&message) else {
            panic!(
                "case {case:?}: expected \"<subject> '<arn>' does not exist \
                 (Internal ID: <uuid>)\", got {message:?}"
            );
        };
        assert_eq!(
            got_subject, *subject,
            "case {case:?}: expected the message to open with {subject:?}, got {message:?}"
        );
        assert_eq!(
            got_arn, *arn,
            "case {case:?}: expected the ARN of the resource that failed to resolve, \
             {arn:?}, got {got_arn:?} in {message:?}"
        );
        assert!(
            is_uuid(&internal_id),
            "case {case:?}: expected a UUID internal id, got {internal_id:?} in {message:?}"
        );
        assert!(
            !seen.contains(&internal_id),
            "case {case:?}: the internal id identifies ONE request, but {internal_id:?} \
             was already used"
        );
        seen.push(internal_id);
    }

    // The gate the rows above fail at did not swallow the success path: the stream
    // session this group really holds is granted a terminal session once the clock
    // has placed it.
    let case = "the stream session the group holds";
    let (session_id, _) = started_session(&router, case, &group_id, &app_id).await;
    std::thread::sleep(PAST_TRANSITION);
    let (status, _code, granted) = admin_shell(&router, &group_id, &session_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStreamSessionAdminShell 200 on an eligible \
         session, got {status} {granted}"
    );
    assert!(
        text(&granted, "/SessionId", case).starts_with(&format!("{session_id}-")),
        "case {case:?}: the grant names the stream session the terminal is into, \
         got {granted}"
    );
}
