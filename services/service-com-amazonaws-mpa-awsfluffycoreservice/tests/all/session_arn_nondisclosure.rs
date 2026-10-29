// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how every operation ADDRESSED BY A SESSION ARN answers a session
//! ARN that resolves to no session the caller can see.
//!
//! The rule under test: this service does not disclose whether such a session exists, so
//! an operation whose resource IS a session (`GetSession` on `GET /sessions/{SessionArn}`,
//! `CancelSession` on `PUT /sessions/{SessionArn}` — the only two the model binds to a
//! `SessionArn`) refuses a well-formed `SessionArn` that resolves to no session with
//! `AccessDeniedException` (403) and the one bare access-denied sentence
//! (`state::ACCESS_DENIED_MESSAGE`), whose text carries no part of the request. It is
//! never the `ResourceNotFoundException` (404) the ARN-addressed lookups of the resources
//! this service DOES report an existence for answer.
//!
//! The table spans the fields `SessionArn`'s pattern
//! `^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$`
//! lets vary — partition, region (each bound of its `{1,20}`), the team segment, the
//! session segment, and the whole value's length up to `@length(max: 2048)` — crossed
//! with both session-addressed operations, rather than the one shape a recording carries.
//! The contrast rows show what the rule does NOT cover: a session the store does hold is
//! still served by both operations, a value past `SessionArn`'s length bound is still the
//! framework's 400, and a TEAM ARN no stored team carries still answers 404, because a
//! team's existence is reported.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, get_session, message, start_baseline};
use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};
use crate::session_cancellation::cancel_session;

/// `SessionArn` is `@length(min: 20, max: 2048)`.
const MAX_SESSION_ARN: usize = 2048;

/// The team the table's own ARNs name, so a row that varies only the session segment
/// names a team the store really holds.
const TEAM_NAME: &str = "nondisclosure-team";

/// The two operations the model addresses by a `SessionArn`
/// (`smithy_query(query="digest")`: `GetSession` GET `/sessions/{SessionArn}` and
/// `CancelSession` PUT `/sessions/{SessionArn}`), each called through its own route.
#[derive(Debug, Clone, Copy)]
enum SessionAddressed {
    GetSession,
    CancelSession,
}

impl SessionAddressed {
    fn as_str(self) -> &'static str {
        match self {
            Self::GetSession => "GetSession",
            Self::CancelSession => "CancelSession",
        }
    }

    async fn call(self, harness: &Harness, session_arn: &str) -> (u16, Value) {
        match self {
            Self::GetSession => get_session(harness, session_arn).await,
            Self::CancelSession => cancel_session(harness, session_arn).await,
        }
    }
}

/// Every session-addressed operation of this model.
const SESSION_ADDRESSED: [SessionAddressed; 2] =
    [SessionAddressed::GetSession, SessionAddressed::CancelSession];

/// Well-formed session ARNs in the CALLER'S OWN account — so the account boundary is
/// satisfied and the lookup is what refuses them — that name no stored session.
///
/// `known` is the ARN of the session the store does hold, so the last row differs from a
/// stored session by a single character.
fn unknown_session_arns(known: &str) -> Vec<(String, String)> {
    let own = engine::emulator_account_id();
    let padded_prefix = format!("arn:aws:mpa:{REGION}:{own}:session/{TEAM_NAME}/");
    vec![
        (
            "a session segment no session of a stored team carries".to_string(),
            format!("arn:aws:mpa:{REGION}:{own}:session/{TEAM_NAME}/session-999999999999"),
        ),
        (
            "a team segment no stored team carries".to_string(),
            format!("arn:aws:mpa:{REGION}:{own}:session/absent-team/session-000000000001"),
        ),
        (
            // The pattern accepts a single character in either segment.
            "the shortest team and session segments the pattern allows".to_string(),
            format!("arn:aws:mpa:{REGION}:{own}:session/a/b"),
        ),
        (
            // The team segment is `[a-zA-Z0-9._-]+`: dots, underscores and dashes too.
            "a team segment of dots, underscores and dashes".to_string(),
            format!("arn:aws:mpa:{REGION}:{own}:session/Team.Alpha_1-x/session-1"),
        ),
        (
            // The session segment is `[a-zA-Z0-9_-]+`: underscores and dashes, no dot.
            "a session segment of underscores and dashes".to_string(),
            format!("arn:aws:mpa:{REGION}:{own}:session/{TEAM_NAME}/a_b-c"),
        ),
        (
            "another region".to_string(),
            format!("arn:aws:mpa:eu-west-1:{own}:session/{TEAM_NAME}/session-000000000001"),
        ),
        (
            // The region segment is `[a-z0-9-]{1,20}` — its lower bound.
            "a one-character region".to_string(),
            format!("arn:aws:mpa:a:{own}:session/{TEAM_NAME}/session-000000000001"),
        ),
        (
            // …and its upper bound.
            "a region at the pattern's twenty-character bound".to_string(),
            format!(
                "arn:aws:mpa:{}:{own}:session/{TEAM_NAME}/session-000000000001",
                "r".repeat(20)
            ),
        ),
        (
            // The partition segment is `aws(-[^:]+)?`.
            "another partition, in the caller's account".to_string(),
            format!("arn:aws-us-gov:mpa:{REGION}:{own}:session/{TEAM_NAME}/session-1"),
        ),
        (
            "an ARN at SessionArn's length maximum".to_string(),
            format!(
                "{padded_prefix}{}",
                "x".repeat(MAX_SESSION_ARN - padded_prefix.len())
            ),
        ),
        (
            "the stored session's ARN with one more character".to_string(),
            format!("{known}0"),
        ),
    ]
}

/// A team made ACTIVE with one baseline session the store really holds, and that
/// session's ARN.
async fn team_with_session(harness: &Harness) -> (String, String) {
    let team_arn = active_team(harness, TEAM_NAME, 2, 1).await;
    let (status, started) = start_baseline(harness, &team_arn, None).await;
    assert_eq!(status, 200, "baseline returned {status} with {started:?}");
    (team_arn, text(&started, "BaselineSessionArn"))
}

/// The status the store holds for a session, and how many sessions it holds at all — what
/// a refused call must leave exactly as it stood.
fn stored_sessions(harness: &Harness, team_arn: &str) -> Vec<String> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .team_sessions_in_initiation_order(team_arn)
        .iter()
        .map(|session| format!("{}:{}", session.arn, session.status.as_wire()))
        .collect()
}

/// Every session-addressed operation refuses a session ARN that resolves to no session
/// with 403 `AccessDeniedException` and the one access-denied sentence, whatever the ARN
/// names — and the refusal quotes nothing the request carried, which is the disclosure it
/// withholds.
#[tokio::test]
async fn an_unresolvable_session_arn_is_denied_by_every_session_addressed_operation() {
    let harness = Harness::new();
    let (team_arn, known) = team_with_session(&harness).await;
    let before = stored_sessions(&harness, &team_arn);

    for (case, session_arn) in unknown_session_arns(&known) {
        for operation in SESSION_ADDRESSED {
            let (status, body) = operation.call(&harness, &session_arn).await;
            let op = operation.as_str();
            assert_eq!(
                status, 403,
                "case {case:?} / {op}: expected 403 for {session_arn:?}, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?} / {op}: expected error code AccessDeniedException, got {body:?}"
            );
            // One sentence, fixed for this class: it is the whole message, so a caller
            // reads nothing about the session out of it.
            assert_eq!(
                text(&body, "Message"),
                state::ACCESS_DENIED_MESSAGE,
                "case {case:?} / {op}: expected the access-denied sentence, got {body:?}"
            );
            assert!(
                !text(&body, "Message").contains(session_arn.as_str()),
                "case {case:?} / {op}: the refusal must not quote the ARN asked for, got \
                 {body:?}"
            );
            assert!(
                !text(&body, "Message").contains(op),
                "case {case:?} / {op}: the refusal must not name the operation, got {body:?}"
            );
        }
    }

    // Every one of those calls was refused BEFORE it could touch the store: the session
    // this store does hold is untouched, and no session was recorded or removed.
    assert_eq!(
        stored_sessions(&harness, &team_arn),
        before,
        "a refused session-addressed call must leave the stored sessions exactly as they \
         stood"
    );
}

/// The refusal is about the LOOKUP, not about the operations: the session the store does
/// hold is still served by both session-addressed operations, so the 403 above is not a
/// blanket denial.
#[tokio::test]
async fn a_stored_session_is_still_served_by_every_session_addressed_operation() {
    let harness = Harness::new();
    let (_, known) = team_with_session(&harness).await;

    let (status, read) = get_session(&harness, &known).await;
    assert_eq!(
        status, 200,
        "case a stored session / GetSession: expected 200, got {status} with {read:?}"
    );
    assert_eq!(
        text(&read, "SessionArn"),
        known,
        "case a stored session / GetSession: expected the session asked for, got {read:?}"
    );

    let (status, cancelled) = cancel_session(&harness, &known).await;
    assert_eq!(
        status, 200,
        "case a stored session / CancelSession: expected 200, got {status} with {cancelled:?}"
    );
    let (status, after) = get_session(&harness, &known).await;
    assert_eq!(
        status, 200,
        "case a stored session / GetSession after the cancel: expected 200, got {status} \
         with {after:?}"
    );
    assert_eq!(
        text(&after, "Status"),
        "CANCELLED",
        "case a stored session / GetSession after the cancel: expected CANCELLED, got \
         {after:?}"
    );

    // A session that reached a terminal status is still a session the caller can see, so
    // it is not refused as unresolvable: `CancelSession` is `@idempotent` and succeeds.
    let (status, again) = cancel_session(&harness, &known).await;
    assert_eq!(
        status, 200,
        "case a cancelled session / CancelSession: expected the idempotent 200, got \
         {status} with {again:?}"
    );
}

/// What the rule does NOT cover, so it is not applied wider than it holds: a value past
/// `SessionArn`'s own length bound is still the framework's 400, and an ARN-addressed
/// lookup of a resource whose existence this service DOES report — an approval team —
/// still answers 404 for an ARN no stored team carries.
#[tokio::test]
async fn the_nondisclosure_rule_is_confined_to_the_session_lookup() {
    let harness = Harness::new();
    let own = engine::emulator_account_id();
    let (_, known) = team_with_session(&harness).await;
    assert!(
        known.len() <= MAX_SESSION_ARN,
        "a minted session ARN must fit SessionArn's length bound, got {}",
        known.len()
    );

    // One character past `@length(max: 2048)`: the boundary validator refuses the value
    // before any session is looked up, so the answer is the modelled input error.
    let prefix = format!("arn:aws:mpa:{REGION}:{own}:session/{TEAM_NAME}/");
    let too_long = format!(
        "{prefix}{}",
        "x".repeat(MAX_SESSION_ARN + 1 - prefix.len())
    );
    for operation in SESSION_ADDRESSED {
        let (status, body) = operation.call(&harness, &too_long).await;
        let op = operation.as_str();
        assert_eq!(
            status, 400,
            "case one past SessionArn's length bound / {op}: expected 400, got {status} \
             with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case one past SessionArn's length bound / {op}: expected ValidationException, \
             got {body:?}"
        );
        assert!(
            !message(&body).is_empty(),
            "case one past SessionArn's length bound / {op}: the refusal must carry a \
             message, got {body:?}"
        );
    }

    // A team's existence IS reported: the team lookup keeps its 404.
    let absent_team = format!("arn:aws:mpa:{REGION}:{own}:approval-team/never-created");
    let (status, body) = harness.get(&absent_team).await;
    assert_eq!(
        status, 404,
        "case an approval team never created: expected 404, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case an approval team never created: expected ResourceNotFoundException, got \
         {body:?}"
    );
    assert_eq!(
        text(&body, "Message"),
        state::RESOURCE_NOT_FOUND_MESSAGE,
        "case an approval team never created: expected the not-found sentence, got {body:?}"
    );
}
