// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelSession`: the operation that ends an approval session this
//! store holds.
//!
//! The rules under test:
//!
//! * cancelling an OUTSTANDING session (`CREATING` or `PENDING`) writes the terminal
//!   `CANCELLED` status and a completion instant onto the STORED record, so `GetSession`
//!   reports it `CANCELLED` with that completion time afterwards and `GetApprovalTeam`
//!   stops reporting it as any approver's pending baseline session. Nothing claims an
//!   approver answered, and no `SessionStatusCode` is reported: none of the four tokens
//!   the model declares stands for a cancellation.
//! * the operation is `@idempotent`: cancelling a session that is already `CANCELLED`
//!   succeeds and moves nothing, so the completion instant the first call recorded is the
//!   one every later read reports.
//! * a session that reached ANOTHER terminal status is no longer cancellable:
//!   `ConflictException` (409), and the stored record is left as it stood.
//! * the ARN-addressed boundary every operation of this service shares, applied to a
//!   WRITE: an ARN outside the caller's account is refused 403 before any lookup, an ARN
//!   this store holds nothing under is 404 with the one not-found sentence this service
//!   states, and a value that is not a well-formed MPA ARN is 400 with
//!   `state::INVALID_ARN_MESSAGE`.
//!
//! The tables span the classes each rule generalises over (the status the session was
//! outstanding in, approver count and selection, team-name form, which terminal status it
//! had already reached, which ARN field is wrong), not one recorded shape.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    active_team, expire_session, get_session, get_team, message, start_baseline,
    stored_approver_ids, wait_for_the_clock,
};
use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness, REGION};

/// `PUT /sessions/{SessionArn}` — `CancelSession`.
async fn cancel_session(harness: &Harness, session_arn: &str) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/sessions/{}", encode_label(session_arn)),
            None,
        )
        .await
}

/// The status a stored session carries, read straight out of the store.
fn stored_status(harness: &Harness, session_arn: &str) -> String {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .session(session_arn)
        .expect("a stored session")
        .status
        .as_wire()
        .to_string()
}

/// Put a stored session into a terminal status no operation of this model reaches.
///
/// `APPROVED` is reached only by an approver's vote and this model records none, so the
/// status is set on the stored record directly — exactly as the session tables set a due
/// instant directly. The refusal under test is still the service's own.
fn force_status(harness: &Harness, session_arn: &str, status: state::SessionStatus) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let session = guard
        .session_mut(session_arn)
        .expect("a stored session to put into a terminal status");
    session.status = status;
    session.completion_time = Some(SystemTime::now());
}

/// A team made ACTIVE with one started baseline session, which the test then cancels.
async fn team_with_session(
    harness: &Harness,
    name: &str,
    approvers: usize,
    min_approvals: i64,
) -> (String, String) {
    let team_arn = active_team(harness, name, approvers, min_approvals).await;
    let (status, started) = start_baseline(harness, &team_arn, None).await;
    assert_eq!(status, 200, "case {name}: baseline returned {started:?}");
    (team_arn, text(&started, "BaselineSessionArn"))
}

/// One row of the cancellation table.
struct CancelCase {
    case: &'static str,
    team_name: String,
    approvers: usize,
    min_approvals: i64,
    /// How many of the team's approvers the baseline request selects; `None` names none
    /// at all, so the whole team is in the session.
    selected: Option<usize>,
    /// Whether the session's activation is waited out, so the cancel lands on a `PENDING`
    /// session rather than on one still `CREATING`.
    wait_for_activation: bool,
}

/// Rows differ along the status the session was outstanding in when it was cancelled,
/// the approver count, which approvers the session asks and the team-name form.
fn cancel_cases() -> Vec<CancelCase> {
    vec![
        CancelCase {
            case: "one approver, cancelled while still CREATING",
            team_name: "cancel-creating".to_string(),
            approvers: 1,
            min_approvals: 1,
            selected: None,
            wait_for_activation: false,
        },
        CancelCase {
            case: "three approvers, cancelled once PENDING",
            team_name: "Cancel.Team_2".to_string(),
            approvers: 3,
            min_approvals: 2,
            selected: None,
            wait_for_activation: true,
        },
        CancelCase {
            case: "five approvers, a subset asked, cancelled once PENDING",
            team_name: "c".to_string(),
            approvers: 5,
            min_approvals: 5,
            selected: Some(2),
            wait_for_activation: true,
        },
        CancelCase {
            case: "maximum approvers, longest team name, cancelled while CREATING",
            // `ApprovalTeamName` is `length(max: 64)`; `Approvers` is `length(1..20)`.
            team_name: "z".repeat(64),
            approvers: 20,
            min_approvals: 20,
            selected: Some(20),
            wait_for_activation: false,
        },
    ]
}

/// Cancelling an outstanding session ends it: the stored record reaches the terminal
/// `CANCELLED` status with a completion instant, `GetSession` reads that back, and the
/// team read stops reporting the session as pending for its approvers.
#[tokio::test]
async fn cancelling_an_outstanding_session_ends_it_as_cancelled() {
    let harness = Harness::new();

    for case in cancel_cases() {
        let label = case.case;
        let team_arn =
            active_team(&harness, &case.team_name, case.approvers, case.min_approvals).await;
        let team_approver_ids = stored_approver_ids(&harness, &team_arn);
        let selected: Vec<String> = match case.selected {
            None => team_approver_ids.clone(),
            Some(count) => team_approver_ids.iter().take(count).cloned().collect(),
        };
        let requested = case.selected.map(|_| selected.clone());
        let (status, started) = start_baseline(&harness, &team_arn, requested.as_deref()).await;
        assert_eq!(status, 200, "case {label}: baseline returned {started:?}");
        let session_arn = text(&started, "BaselineSessionArn");

        if case.wait_for_activation {
            wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
        }
        let (status, before) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: GetSession returned {before:?}");
        let expected_before = if case.wait_for_activation {
            "PENDING"
        } else {
            "CREATING"
        };
        assert_eq!(
            text(&before, "Status"),
            expected_before,
            "case {label}: expected an outstanding {expected_before} session before the \
             cancel, got {before:?}"
        );

        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: CancelSession returned {status} with {body:?}"
        );

        let (status, after) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: GetSession returned {after:?}");
        assert_eq!(
            text(&after, "Status"),
            "CANCELLED",
            "case {label}: a cancelled session is CANCELLED, got {after:?}"
        );
        assert!(
            after.get("StatusCode").is_none(),
            "case {label}: no SessionStatusCode token stands for a cancellation, got {after:?}"
        );
        let completion = instant(&after, "CompletionTime");
        let initiation = instant(&after, "InitiationTime");
        assert!(
            completion >= initiation,
            "case {label}: expected a CompletionTime at or after the initiation \
             {initiation}, got {completion}"
        );
        let expiration = instant(&after, "ExpirationTime");
        assert!(
            completion <= expiration,
            "case {label}: a cancelled session completed before it would have expired \
             ({expiration}), got {completion}"
        );
        assert_eq!(
            text(&after, "ExecutionStatus"),
            "PENDING",
            "case {label}: nothing was executed, so ExecutionStatus stays PENDING, got \
             {after:?}"
        );
        assert_eq!(
            after["NumberOfApprovers"].as_u64(),
            Some(selected.len() as u64),
            "case {label}: the cancel must not change who the session asked, got {after:?}"
        );
        for entry in after["ApproverResponses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: ApproverResponses is missing in {after:?}"))
        {
            assert_eq!(
                text(entry, "Response"),
                "NO_RESPONSE",
                "case {label}: a cancelled session answered for nobody, got {entry:?}"
            );
            assert!(
                entry.get("ResponseTime").is_none(),
                "case {label}: an unanswered approver has no ResponseTime, got {entry:?}"
            );
        }

        // A read does not move the cancellation the write recorded.
        let (status, again) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: the second GetSession returned {again:?}");
        assert_eq!(
            again["CompletionTime"], after["CompletionTime"],
            "case {label}: a read moved CompletionTime"
        );
        assert_eq!(
            again["Status"], after["Status"],
            "case {label}: a read moved Status"
        );

        // The team read agrees: a cancelled session is pending for nobody.
        let (status, team) = get_team(&harness, &team_arn).await;
        assert_eq!(status, 200, "case {label}: GetApprovalTeam returned {team:?}");
        for entry in team["Approvers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: Approvers is missing in {team:?}"))
        {
            assert!(
                entry.get("PendingBaselineSessionArn").is_none(),
                "case {label}: a cancelled session is not pending for approver {:?}, got \
                 {entry:?}",
                text(entry, "ApproverId")
            );
        }
    }
}

/// `CancelSession` is `@idempotent`: a repeat cancel succeeds and changes nothing, so the
/// completion instant the first call recorded is the one every later read reports.
#[tokio::test]
async fn cancelling_a_cancelled_session_changes_nothing() {
    let harness = Harness::new();

    // Rows: how long the caller waits before repeating the cancel, so the repeat is not
    // served out of the same instant as the first call.
    let cases: [(&str, Duration); 2] = [
        ("repeated straight away", Duration::ZERO),
        ("repeated after a pause", Duration::from_millis(50)),
    ];

    for (label, pause) in cases {
        let (_, session_arn) =
            team_with_session(&harness, &format!("idem-{}", pause.as_millis()), 2, 1).await;

        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: the first CancelSession returned {status} with {body:?}"
        );
        let (_, first) = get_session(&harness, &session_arn).await;
        let completion = first["CompletionTime"].clone();

        std::thread::sleep(pause);
        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: the repeat CancelSession returned {status} with {body:?}"
        );

        let (_, second) = get_session(&harness, &session_arn).await;
        assert_eq!(
            text(&second, "Status"),
            "CANCELLED",
            "case {label}: the session stays CANCELLED, got {second:?}"
        );
        assert_eq!(
            second["CompletionTime"], completion,
            "case {label}: the repeat moved CompletionTime, got {second:?}"
        );
    }
}

/// A session that already reached a terminal status other than `CANCELLED` can no longer
/// be cancelled: 409 `ConflictException`, naming the status that made the request
/// impossible, and the stored record is left as it stood.
#[tokio::test]
async fn a_session_that_is_no_longer_outstanding_cannot_be_cancelled() {
    let harness = Harness::new();

    // Rows: the terminal status the session had already reached, and how it got there.
    let cases: [(&str, &str, Option<state::SessionStatus>); 2] = [
        ("expired unanswered", "FAILED", None),
        (
            "approved by its approvers",
            "APPROVED",
            Some(state::SessionStatus::Approved),
        ),
    ];

    for (label, expected_status, forced) in cases {
        let (_, session_arn) = team_with_session(
            &harness,
            &format!("terminal-{}", expected_status.to_lowercase()),
            2,
            1,
        )
        .await;
        match forced {
            // The clock is what takes a session to `FAILED` / `EXPIRED`.
            None => {
                expire_session(&harness, &session_arn, SystemTime::now() - Duration::from_secs(1));
                let (_, session) = get_session(&harness, &session_arn).await;
                assert_eq!(
                    text(&session, "Status"),
                    expected_status,
                    "case {label}: expected a {expected_status} session before the cancel, got \
                     {session:?}"
                );
            }
            Some(status) => force_status(&harness, &session_arn, status),
        }

        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 409,
            "case {label}: expected 409 for a {expected_status} session, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label}: wrong error code in {body:?}"
        );
        assert!(
            text(&body, "Message").contains(expected_status),
            "case {label}: the refusal must name the status that conflicts, got {body:?}"
        );

        // The refused call wrote nothing: the session is still in the status it had.
        assert_eq!(
            stored_status(&harness, &session_arn),
            expected_status,
            "case {label}: a refused cancel must leave the stored session alone"
        );
    }
}

/// Cancelling one session leaves every other stored session alone, whichever team it
/// belongs to.
#[tokio::test]
async fn cancelling_one_session_leaves_the_others_outstanding() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "many-sessions", 3, 2).await;
    let approvers = stored_approver_ids(&harness, &team_arn);
    let other_team_arn = active_team(&harness, "other-team", 1, 1).await;

    let mut arns = Vec::new();
    for selected in [&approvers[0..1], &approvers[1..2], &approvers[2..3]] {
        let (status, started) = start_baseline(&harness, &team_arn, Some(selected)).await;
        assert_eq!(status, 200, "baseline returned {started:?}");
        arns.push(text(&started, "BaselineSessionArn"));
    }
    let (status, started) = start_baseline(&harness, &other_team_arn, None).await;
    assert_eq!(status, 200, "baseline on the other team returned {started:?}");
    arns.push(text(&started, "BaselineSessionArn"));

    // Cancel the middle session of the first team.
    let cancelled = arns[1].clone();
    let (status, body) = cancel_session(&harness, &cancelled).await;
    assert_eq!(status, 200, "CancelSession returned {status} with {body:?}");

    for arn in &arns {
        let (status, session) = get_session(&harness, arn).await;
        assert_eq!(status, 200, "case {arn}: GetSession returned {session:?}");
        let expected = if arn == &cancelled {
            "CANCELLED"
        } else {
            "CREATING"
        };
        assert_eq!(
            text(&session, "Status"),
            expected,
            "case {arn}: expected {expected}, got {session:?}"
        );
        assert_eq!(
            session.get("CompletionTime").is_some(),
            arn == &cancelled,
            "case {arn}: only the cancelled session has completed, got {session:?}"
        );
    }
}

/// A well-formed session ARN this store holds nothing under cannot be cancelled: 404 with
/// the one not-found sentence this service states, whatever the ARN names.
#[tokio::test]
async fn an_unknown_session_arn_cannot_be_cancelled() {
    let harness = Harness::new();
    let (_, known) = team_with_session(&harness, "known-cancel-team", 1, 1).await;
    let account = engine::emulator_account_id();

    let cases: [(&str, String); 4] = [
        (
            "no session of a team that exists",
            format!("arn:aws:mpa:{REGION}:{account}:session/known-cancel-team/session-999999999999"),
        ),
        (
            "a team this account never created",
            format!("arn:aws:mpa:{REGION}:{account}:session/absent-team/session-000000000001"),
        ),
        (
            "another region",
            format!("arn:aws:mpa:eu-west-1:{account}:session/known-cancel-team/session-000000000001"),
        ),
        (
            "a session id one character off the stored one",
            format!("{known}0"),
        ),
    ];

    for (label, session_arn) in cases {
        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 404,
            "case {label}: expected 404 for {session_arn:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label}: wrong error code in {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {label}: wrong not-found message in {body:?}"
        );
    }

    // None of those probes touched the session this store does hold.
    assert_eq!(
        stored_status(&harness, &known),
        "CREATING",
        "a refused cancel must leave the stored session alone"
    );
}

/// A session ARN outside the caller's own account is refused 403 BEFORE any lookup, so a
/// cancellation the caller may not make never depends on what is stored under the ARN.
#[tokio::test]
async fn a_session_arn_in_another_account_cannot_be_cancelled() {
    let harness = Harness::new();

    let cases: [(&str, String); 3] = [
        (
            "another account",
            format!("arn:aws:mpa:{REGION}:999999999999:session/probe/session-000000000001"),
        ),
        (
            "another account, another region",
            "arn:aws:mpa:eu-central-1:210987654321:session/probe/session-000000000002".to_string(),
        ),
        (
            "another partition and account",
            format!("arn:aws-us-gov:mpa:{REGION}:123123123123:session/probe/session-000000000003"),
        ),
    ];

    for (label, session_arn) in cases {
        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 403,
            "case {label}: expected 403 for {session_arn:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {label}: wrong error code in {body:?}"
        );
        assert!(
            text(&body, "Message").contains("mpa:CancelSession"),
            "case {label}: the refusal must name the attempted action, got {body:?}"
        );
    }
}

/// A `SessionArn` that is not a well-formed MPA session ARN is refused 400 with the one
/// sentence this service states for the malformed-ARN class, whichever field is wrong.
#[tokio::test]
async fn a_malformed_session_arn_cannot_be_cancelled() {
    let harness = Harness::new();
    let account = engine::emulator_account_id();

    // Rows: the ARN field the value gets wrong. Each is at least the 20 characters
    // `SessionArn`'s `length(min: 20)` requires, so the pattern is what refuses it.
    let cases: [(&str, String); 5] = [
        (
            "an approval-team ARN where a session ARN belongs",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/not-a-session"),
        ),
        (
            "no session id segment",
            format!("arn:aws:mpa:{REGION}:{account}:session/only-the-team"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:iam:{REGION}:{account}:session/team/session-1"),
        ),
        (
            "an account id that is not twelve digits",
            format!("arn:aws:mpa:{REGION}:12345:session/team/session-1"),
        ),
        (
            "not an ARN at all",
            "a-plain-string-of-some-length".to_string(),
        ),
    ];

    for (label, session_arn) in cases {
        let (status, body) = cancel_session(&harness, &session_arn).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400 for {session_arn:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: wrong error code in {body:?}"
        );
        assert_eq!(
            message(&body),
            state::INVALID_ARN_MESSAGE,
            "case {label}: wrong malformed-ARN message in {body:?}"
        );
    }
}
