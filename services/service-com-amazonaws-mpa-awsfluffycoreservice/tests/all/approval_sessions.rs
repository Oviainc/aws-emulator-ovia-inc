// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the approval SESSION: `StartApprovalTeamBaseline`, which is the
//! one operation of this model that mints a session, and `GetSession`, which reports it.
//!
//! The rules under test:
//!
//! * a session is a STORED record. `StartApprovalTeamBaseline` records one against the
//!   team its ARN names and `GetSession` reports every member off that record, so a
//!   session ARN the store holds answers with the team, the strategy, the approvers and
//!   the timestamps that were recorded, and a session ARN it does not hold answers 404
//!   with the one not-found sentence this service states — whatever the ARN's shape.
//! * the ARN-addressed boundary every read of this service shares: an ARN outside the
//!   caller's account is refused 403 BEFORE the lookup, and a value that is not a
//!   well-formed MPA ARN is refused 400 with `state::INVALID_ARN_MESSAGE`.
//! * only the CLOCK moves a session: `CREATING` until its activation comes due,
//!   `PENDING` after it, and `FAILED` / `EXPIRED` once its expiration has passed — and a
//!   read never moves it, so two reads of the same session agree.
//!
//! The tables span the class each rule generalises over (approver count, which approvers
//! the request selects, team-name form, `MinApprovalsRequired`, the ARN field that is
//! wrong, the status the team is in), not one recorded shape. Where a table names a
//! published bound (20 approvers, a 64-character team name) a comment names the modelled
//! member it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{
    encode_label, error_code, expected_arn, instant, text, Harness, REGION,
};

/// Slack over a clock-owed transition's delay before it is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// `Approvers` is `length(min: 1, max: 20)` on `CreateApprovalTeam`, and
/// `StartApprovalTeamBaselineApproverIds` is `length(max: 20)`.
const MAX_APPROVERS: usize = 20;

/// `ApprovalTeamName` is `length(max: 64)`, pattern `^[a-zA-Z0-9._-]+$`.
const MAX_TEAM_NAME: usize = 64;

/// The message of an error envelope, whichever case the renderer used: a handler-raised
/// modelled error carries the modelled `Message` member, while a refusal the framework
/// boundary rendered (this service's malformed-ARN rewording, say) carries `message`.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An approver: `(PrimaryIdentityId, PrimaryIdentitySourceArn)`.
fn approver(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{n:012}",
            engine::emulator_account_id()
        ),
    )
}

/// A policy ARN that satisfies `PolicyReference$PolicyArn`'s modelled pattern.
fn policy_arn() -> String {
    "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string()
}

/// `POST /approval-teams` — `CreateApprovalTeam`.
async fn create_team(
    harness: &Harness,
    name: &str,
    approvers: usize,
    min_approvals: i64,
) -> (u16, Value) {
    let body = json!({
        "Name": name,
        "Description": "session table",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": min_approvals } },
        "Approvers": (1..=approvers)
            .map(approver)
            .map(|(id, source)| json!({
                "PrimaryIdentityId": id,
                "PrimaryIdentitySourceArn": source,
            }))
            .collect::<Vec<_>>(),
        "Policies": [{ "PolicyArn": policy_arn() }],
    });
    harness.create(&body).await
}

/// `POST /approval-teams/{Arn}/baseline` — `StartApprovalTeamBaseline`.
async fn start_baseline(
    harness: &Harness,
    team_arn: &str,
    approver_ids: Option<&[String]>,
) -> (u16, Value) {
    let body = approver_ids.map(|ids| json!({ "ApproverIds": ids }));
    harness
        .call(
            "POST",
            &format!("/approval-teams/{}/baseline", encode_label(team_arn)),
            body,
        )
        .await
}

/// `GET /sessions/{SessionArn}` — `GetSession`.
async fn get_session(harness: &Harness, session_arn: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/sessions/{}", encode_label(session_arn)), None)
        .await
}

/// `GET /approval-teams/{Arn}` — `GetApprovalTeam`.
async fn get_team(harness: &Harness, team_arn: &str) -> (u16, Value) {
    harness.get(team_arn).await
}

/// Wait out a clock-owed transition of `delay`.
fn wait_for_the_clock(delay: Duration) {
    std::thread::sleep(delay + CLOCK_SLACK);
}

/// The approvers the store recorded for a team, in team order:
/// `(ApproverId, PrimaryIdentityId, PrimaryIdentitySourceArn)`.
fn stored_approvers(harness: &Harness, team_arn: &str) -> Vec<(String, String, String)> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .team(team_arn)
        .expect("a stored team")
        .approvers
        .iter()
        .map(|approver| {
            (
                approver.approver_id.clone(),
                approver.primary_identity_id.clone(),
                approver.primary_identity_source_arn.clone(),
            )
        })
        .collect()
}

/// The approver ids the store recorded for a team, in team order.
fn stored_approver_ids(harness: &Harness, team_arn: &str) -> Vec<String> {
    stored_approvers(harness, team_arn)
        .into_iter()
        .map(|(approver_id, _, _)| approver_id)
        .collect()
}

/// Force a stored session's expiration to have already passed, so the clock-owed
/// `FAILED` / `EXPIRED` outcome is due on the next call. Only the DUE INSTANT is moved;
/// the outcome itself is the service's own.
fn expire_session(harness: &Harness, session_arn: &str, expiration: SystemTime) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let session = guard
        .session_mut(session_arn)
        .expect("a stored session to expire");
    session.expiration_time = expiration;
}

/// Force a stored team's pending deletion window to have already elapsed.
fn expire_deletion_window(harness: &Harness, team_arn: &str) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let team = guard.team_mut(team_arn).expect("a stored team to expire");
    team.deletion_completion_time = Some(SystemTime::now() - Duration::from_secs(1));
}

/// A team made ACTIVE: created, then the clock-owed invitation responses waited out.
async fn active_team(harness: &Harness, name: &str, approvers: usize, min_approvals: i64) -> String {
    let (status, created) = create_team(harness, name, approvers, min_approvals).await;
    assert_eq!(status, 200, "case {name}: create returned {created:?}");
    wait_for_the_clock(state::INVITATION_RESPONSE_DELAY);
    let arn = text(&created, "Arn");
    let (status, team) = get_team(harness, &arn).await;
    assert_eq!(status, 200, "case {name}: get returned {team:?}");
    assert_eq!(
        text(&team, "Status"),
        "ACTIVE",
        "case {name}: expected an ACTIVE team before a baseline, got {team:?}"
    );
    arn
}

/// One row of the baseline/get round-trip table.
struct SessionCase {
    case: &'static str,
    team_name: String,
    approvers: usize,
    min_approvals: i64,
    /// How many of the team's approvers the request selects; `None` names none at all
    /// (the member is absent, so the whole team is in the session).
    selected: Option<usize>,
}

/// The round-trip table: rows differ along approver count, which approvers the request
/// selects (absent, one, a subset, all), `MinApprovalsRequired` and team-name form.
fn session_cases() -> Vec<SessionCase> {
    vec![
        SessionCase {
            case: "one approver, ApproverIds absent",
            team_name: "solo-team".to_string(),
            approvers: 1,
            min_approvals: 1,
            selected: None,
        },
        SessionCase {
            case: "three approvers, one selected",
            team_name: "Team.Beta_2".to_string(),
            approvers: 3,
            min_approvals: 2,
            selected: Some(1),
        },
        SessionCase {
            case: "five approvers, a subset selected",
            team_name: "a".to_string(),
            approvers: 5,
            min_approvals: 5,
            selected: Some(3),
        },
        SessionCase {
            case: "maximum approvers, every one selected, longest name",
            // `ApprovalTeamName` is `length(max: 64)`; `Approvers` is `length(1..20)`.
            team_name: "n".repeat(MAX_TEAM_NAME),
            approvers: MAX_APPROVERS,
            min_approvals: 20,
            selected: Some(MAX_APPROVERS),
        },
    ]
}

/// Every session token this service writes is one the model declares. The catalogs come
/// from the generated `shared_types` value lists, rendered from the model's own
/// `SessionStatus` / `SessionStatusCode` / `SessionExecutionStatus` / `SessionResponse` /
/// `ActionCompletionStrategy` / `AdditionalSecurityRequirement` enums.
#[test]
fn the_session_tokens_this_service_writes_are_ones_the_model_declares() {
    for status in [
        state::SessionStatus::Creating,
        state::SessionStatus::Pending,
        state::SessionStatus::Approved,
        state::SessionStatus::Cancelled,
        state::SessionStatus::Failed,
    ] {
        assert!(
            model::SESSION_STATUS_VALUES.contains(&status.as_wire()),
            "case {status:?}: expected a token SessionStatus declares, got {:?}",
            status.as_wire()
        );
    }

    for code in [
        state::SessionStatusCode::Rejected,
        state::SessionStatusCode::Expired,
        state::SessionStatusCode::ConfigurationChanged,
        state::SessionStatusCode::AllApproversInSession,
    ] {
        assert!(
            model::SESSION_STATUS_CODE_VALUES.contains(&code.as_wire()),
            "case {code:?}: expected a token SessionStatusCode declares, got {:?}",
            code.as_wire()
        );
    }

    for execution in [
        state::SessionExecutionStatus::Pending,
        state::SessionExecutionStatus::Executed,
        state::SessionExecutionStatus::Failed,
    ] {
        assert!(
            model::SESSION_EXECUTION_STATUS_VALUES.contains(&execution.as_wire()),
            "case {execution:?}: expected a token SessionExecutionStatus declares, got {:?}",
            execution.as_wire()
        );
    }

    for response in [
        state::SessionResponse::Approved,
        state::SessionResponse::Rejected,
        state::SessionResponse::NoResponse,
    ] {
        assert!(
            model::SESSION_RESPONSE_VALUES.contains(&response.as_wire()),
            "case {response:?}: expected a token SessionResponse declares, got {:?}",
            response.as_wire()
        );
    }

    let strategy = state::ActionCompletionStrategy::AutoCompletionUponApproval;
    assert!(
        model::ACTION_COMPLETION_STRATEGY_VALUES.contains(&strategy.as_wire()),
        "case {strategy:?}: expected a token ActionCompletionStrategy declares, got {:?}",
        strategy.as_wire()
    );

    let requirement = state::AdditionalSecurityRequirement::ApproverVerificationRequired;
    assert!(
        model::ADDITIONAL_SECURITY_REQUIREMENT_VALUES.contains(&requirement.as_wire()),
        "case {requirement:?}: expected a token AdditionalSecurityRequirement declares, got {:?}",
        requirement.as_wire()
    );
}

/// A started baseline session is STORED, and `GetSession` reads every member back out of
/// the store — across the whole class of team and selection shapes, not one of them.
#[tokio::test]
async fn a_started_baseline_session_round_trips_through_get_session() {
    let harness = Harness::new();

    for case in session_cases() {
        let label = case.case;
        let team_arn = active_team(&harness, &case.team_name, case.approvers, case.min_approvals)
            .await;
        let team_approvers = stored_approvers(&harness, &team_arn);
        let team_approver_ids: Vec<String> = team_approvers
            .iter()
            .map(|(approver_id, _, _)| approver_id.clone())
            .collect();

        let selected: Vec<String> = match case.selected {
            None => team_approver_ids.clone(),
            Some(count) => team_approver_ids.iter().take(count).cloned().collect(),
        };
        let requested = case.selected.map(|_| selected.clone());
        let (status, started) = start_baseline(&harness, &team_arn, requested.as_deref()).await;
        assert_eq!(status, 200, "case {label}: baseline returned {started:?}");

        let session_arn = text(&started, "BaselineSessionArn");
        let expected_prefix = format!(
            "arn:aws:mpa:{REGION}:{}:session/{}/",
            engine::emulator_account_id(),
            case.team_name
        );
        assert!(
            session_arn.starts_with(&expected_prefix),
            "case {label}: expected a SessionArn under {expected_prefix:?}, got {session_arn:?}"
        );

        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: GetSession returned {session:?}");
        assert_eq!(
            text(&session, "SessionArn"),
            session_arn,
            "case {label}: GetSession reported another session"
        );
        assert_eq!(
            text(&session, "ApprovalTeamArn"),
            expected_arn(&case.team_name),
            "case {label}: wrong ApprovalTeamArn in {session:?}"
        );
        assert_eq!(
            text(&session, "ApprovalTeamName"),
            case.team_name,
            "case {label}: wrong ApprovalTeamName in {session:?}"
        );
        assert_eq!(
            text(&session, "ProtectedResourceArn"),
            team_arn,
            "case {label}: wrong ProtectedResourceArn in {session:?}"
        );
        assert_eq!(
            session["ApprovalStrategy"]["MofN"]["MinApprovalsRequired"].as_i64(),
            Some(case.min_approvals),
            "case {label}: wrong ApprovalStrategy in {session:?}"
        );
        assert_eq!(
            session["NumberOfApprovers"].as_u64(),
            Some(selected.len() as u64),
            "case {label}: NumberOfApprovers must count the approvers in the session, got \
             {session:?}"
        );
        assert_eq!(
            text(&session, "ActionName"),
            "StartApprovalTeamBaseline",
            "case {label}: wrong ActionName in {session:?}"
        );
        assert_eq!(
            text(&session, "ExecutionStatus"),
            "PENDING",
            "case {label}: nothing was executed, so ExecutionStatus must be PENDING in \
             {session:?}"
        );
        assert_eq!(
            text(&session, "ActionCompletionStrategy"),
            "AUTO_COMPLETION_UPON_APPROVAL",
            "case {label}: wrong ActionCompletionStrategy in {session:?}"
        );
        assert_eq!(
            text(&session, "RequesterAccountId"),
            engine::emulator_account_id(),
            "case {label}: wrong RequesterAccountId in {session:?}"
        );
        assert_eq!(
            text(&session, "RequesterRegion"),
            REGION,
            "case {label}: wrong RequesterRegion in {session:?}"
        );
        assert_eq!(
            text(&session, "RequesterServicePrincipal"),
            state::SERVICE_PRINCIPAL,
            "case {label}: wrong RequesterServicePrincipal in {session:?}"
        );
        assert!(
            session.get("CompletionTime").is_none(),
            "case {label}: an outstanding session has no CompletionTime, got {session:?}"
        );

        // The expiration is the recorded span after the initiation, both read off the
        // same record.
        let initiation = instant(&session, "InitiationTime");
        let expiration = instant(&session, "ExpirationTime");
        let span = expiration - initiation;
        let expected_span = state::SESSION_EXPIRATION.as_secs_f64();
        assert!(
            (span - expected_span).abs() < 1.0,
            "case {label}: expected ExpirationTime {expected_span}s after InitiationTime, got \
             {span}s"
        );

        // Exactly the selected approvers are in the session, each unanswered, and each
        // read off the team's own approver record.
        let responses = session["ApproverResponses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: ApproverResponses is missing in {session:?}"));
        let reported: BTreeSet<String> = responses
            .iter()
            .map(|entry| text(entry, "ApproverId"))
            .collect();
        let expected: BTreeSet<String> = selected.iter().cloned().collect();
        assert_eq!(
            reported, expected,
            "case {label}: the session must ask exactly the selected approvers, got {session:?}"
        );
        for entry in responses {
            let approver_id = text(entry, "ApproverId");
            let (_, identity_id, identity_source_arn) = team_approvers
                .iter()
                .find(|(id, _, _)| id == &approver_id)
                .unwrap_or_else(|| {
                    panic!("case {label}: {approver_id:?} is not an approver of the team")
                });
            assert_eq!(
                &text(entry, "IdentityId"),
                identity_id,
                "case {label}: wrong IdentityId for {approver_id} in {entry:?}"
            );
            assert_eq!(
                &text(entry, "IdentitySourceArn"),
                identity_source_arn,
                "case {label}: wrong IdentitySourceArn for {approver_id} in {entry:?}"
            );
            assert_eq!(
                text(entry, "Response"),
                "NO_RESPONSE",
                "case {label}: no vote was cast, so {approver_id} must be unanswered in {entry:?}"
            );
            assert!(
                entry.get("ResponseTime").is_none(),
                "case {label}: an unanswered approver has no ResponseTime, got {entry:?}"
            );
        }

        assert_eq!(
            session["AdditionalSecurityRequirements"].as_array().map(Vec::len),
            Some(0),
            "case {label}: no additional requirement was applied, got {session:?}"
        );

        // A READ changes nothing: the second answer is the first one.
        let (status, again) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: the second GetSession returned {again:?}");
        assert_eq!(
            again["SessionArn"], session["SessionArn"],
            "case {label}: a read moved the session"
        );
        assert_eq!(
            again["InitiationTime"], session["InitiationTime"],
            "case {label}: a read moved InitiationTime"
        );
        assert_eq!(
            again["ExpirationTime"], session["ExpirationTime"],
            "case {label}: a read moved ExpirationTime"
        );
        assert_eq!(
            again["NumberOfApprovers"], session["NumberOfApprovers"],
            "case {label}: a read moved NumberOfApprovers"
        );
    }
}

/// The clock, and only the clock, moves a session: it is `CREATING` while its activation
/// is still due and `PENDING` once that instant has passed.
#[tokio::test]
async fn a_session_is_creating_until_its_activation_comes_due() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "clock-team", 2, 1).await;

    let (status, started) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(status, 200, "baseline returned {started:?}");
    let session_arn = text(&started, "BaselineSessionArn");

    let (status, session) = get_session(&harness, &session_arn).await;
    assert_eq!(status, 200, "GetSession returned {session:?}");
    assert_eq!(
        text(&session, "Status"),
        "CREATING",
        "case straight after the start: expected CREATING, got {session:?}"
    );
    assert!(
        session.get("StatusCode").is_none(),
        "case straight after the start: a CREATING session carries no StatusCode, got {session:?}"
    );

    wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
    let (status, session) = get_session(&harness, &session_arn).await;
    assert_eq!(status, 200, "GetSession returned {session:?}");
    assert_eq!(
        text(&session, "Status"),
        "PENDING",
        "case once the activation is due: expected PENDING, got {session:?}"
    );
    assert!(
        session.get("CompletionTime").is_none(),
        "case once the activation is due: a PENDING session has not completed, got {session:?}"
    );
}

/// A session nobody answered before its expiration is FAILED with the `EXPIRED` status
/// code, completed at the instant it expired — whichever status it was waiting in.
#[tokio::test]
async fn a_session_that_expires_unanswered_fails_as_expired() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "expiry-team", 2, 1).await;

    // Rows: the status the session was waiting in when its expiration passed.
    let cases: [(&str, bool); 2] = [
        ("expired while still CREATING", false),
        ("expired after it became PENDING", true),
    ];

    for (label, wait_for_activation) in cases {
        let (status, started) = start_baseline(&harness, &team_arn, None).await;
        assert_eq!(status, 200, "case {label}: baseline returned {started:?}");
        let session_arn = text(&started, "BaselineSessionArn");
        if wait_for_activation {
            wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
            let (_, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                text(&session, "Status"),
                "PENDING",
                "case {label}: expected a PENDING session before expiry, got {session:?}"
            );
        }

        let expiration = SystemTime::now() - Duration::from_secs(1);
        expire_session(&harness, &session_arn, expiration);

        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: GetSession returned {session:?}");
        assert_eq!(
            text(&session, "Status"),
            "FAILED",
            "case {label}: an expired session is FAILED, got {session:?}"
        );
        assert_eq!(
            text(&session, "StatusCode"),
            "EXPIRED",
            "case {label}: an expired session carries the EXPIRED code, got {session:?}"
        );
        let completion = instant(&session, "CompletionTime");
        let expected = instant(&session, "ExpirationTime");
        assert!(
            (completion - expected).abs() < 1.0,
            "case {label}: expected CompletionTime at the expiration instant {expected}, got \
             {completion}"
        );
        for entry in session["ApproverResponses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: ApproverResponses is missing"))
        {
            assert_eq!(
                text(entry, "Response"),
                "NO_RESPONSE",
                "case {label}: an expired session answered for nobody, got {entry:?}"
            );
        }

        // The materialised outcome does not move between reads.
        let (_, again) = get_session(&harness, &session_arn).await;
        assert_eq!(
            again["CompletionTime"], session["CompletionTime"],
            "case {label}: a read moved CompletionTime"
        );
    }
}

/// `GetApprovalTeam` reports the SAME outstanding baseline session for an approver that
/// `GetSession` reports, and reports none for an approver the session does not ask or
/// once that session is over.
#[tokio::test]
async fn the_team_read_reports_the_approver_s_outstanding_baseline_session() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "pending-baseline-team", 3, 2).await;
    let team_approvers = stored_approver_ids(&harness, &team_arn);
    let selected = vec![team_approvers[0].clone(), team_approvers[2].clone()];

    let (status, started) = start_baseline(&harness, &team_arn, Some(&selected)).await;
    assert_eq!(status, 200, "baseline returned {started:?}");
    let session_arn = text(&started, "BaselineSessionArn");

    let (status, team) = get_team(&harness, &team_arn).await;
    assert_eq!(status, 200, "GetApprovalTeam returned {team:?}");
    for entry in team["Approvers"]
        .as_array()
        .expect("Approvers is an array")
    {
        let approver_id = text(entry, "ApproverId");
        let reported = entry.get("PendingBaselineSessionArn").and_then(Value::as_str);
        let expected = if selected.contains(&approver_id) {
            Some(session_arn.as_str())
        } else {
            None
        };
        assert_eq!(
            reported, expected,
            "case approver {approver_id}: expected PendingBaselineSessionArn {expected:?}, got \
             {reported:?}"
        );
    }

    // A session that is over is no longer pending for anyone.
    expire_session(
        &harness,
        &session_arn,
        SystemTime::now() - Duration::from_secs(1),
    );
    let (status, team) = get_team(&harness, &team_arn).await;
    assert_eq!(status, 200, "GetApprovalTeam returned {team:?}");
    for entry in team["Approvers"]
        .as_array()
        .expect("Approvers is an array")
    {
        let approver_id = text(entry, "ApproverId");
        assert!(
            entry.get("PendingBaselineSessionArn").is_none(),
            "case approver {approver_id}: an expired session is not pending, got {entry:?}"
        );
    }
}

/// A well-formed session ARN this store holds nothing under answers 404 with the one
/// not-found sentence this service states — whatever the ARN names.
#[tokio::test]
async fn an_unknown_session_arn_is_not_found() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "known-team", 1, 1).await;
    let (status, started) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(status, 200, "baseline returned {started:?}");
    let known = text(&started, "BaselineSessionArn");

    let account = engine::emulator_account_id();
    let cases: [(&str, String); 4] = [
        (
            "no session of a team that exists",
            format!("arn:aws:mpa:{REGION}:{account}:session/known-team/session-999999999999"),
        ),
        (
            "a team this account never created",
            format!("arn:aws:mpa:{REGION}:{account}:session/absent-team/session-000000000001"),
        ),
        (
            "another region",
            format!("arn:aws:mpa:eu-west-1:{account}:session/known-team/session-000000000001"),
        ),
        (
            "a session id one character off the stored one",
            format!("{known}0"),
        ),
    ];

    for (label, session_arn) in cases {
        let (status, body) = get_session(&harness, &session_arn).await;
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
}

/// A session ARN outside the caller's own account is refused 403 BEFORE any lookup, so
/// the answer does not depend on whether a session is stored under it.
#[tokio::test]
async fn a_session_arn_in_another_account_is_denied() {
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
        let (status, body) = get_session(&harness, &session_arn).await;
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
            text(&body, "Message").contains("mpa:GetSession"),
            "case {label}: the refusal must name the attempted action, got {body:?}"
        );
    }
}

/// A `SessionArn` that is not a well-formed MPA session ARN is refused 400 with the one
/// sentence this service states for the malformed-ARN class, whichever field is wrong.
#[tokio::test]
async fn a_malformed_session_arn_is_refused() {
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
        let (status, body) = get_session(&harness, &session_arn).await;
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

/// A baseline session can only be started on an ACTIVE team, and only for approvers that
/// team has: every other request is refused with the modelled `ValidationException`, and
/// nothing is recorded for it.
#[tokio::test]
async fn a_baseline_request_the_team_cannot_answer_is_refused() {
    let harness = Harness::new();

    // A team is PENDING until its approvers' invitation responses come due.
    let (status, created) = create_team(&harness, "pending-team", 2, 1).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let pending_arn = text(&created, "Arn");
    let (status, body) = start_baseline(&harness, &pending_arn, None).await;
    assert_eq!(
        status, 400,
        "case a PENDING team: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a PENDING team: wrong error code in {body:?}"
    );
    assert!(
        text(&body, "Message").contains("PENDING"),
        "case a PENDING team: the refusal must name the status, got {body:?}"
    );

    let team_arn = active_team(&harness, "refusal-team", 2, 1).await;
    let approvers = stored_approver_ids(&harness, &team_arn);

    // Rows: an approver id the team does not have, in several forms, and a repeated one.
    let cases: [(&str, Vec<String>, &str); 4] = [
        (
            "an approver id of no team",
            vec!["approver-999999".to_string()],
            "approver-999999",
        ),
        (
            "a mix of a real and an unknown approver",
            vec![approvers[0].clone(), "approver-abc".to_string()],
            "approver-abc",
        ),
        (
            "an approver id that is a plain name",
            vec!["not-an-approver".to_string()],
            "not-an-approver",
        ),
        (
            "the same approver twice",
            vec![approvers[0].clone(), approvers[0].clone()],
            &approvers[0],
        ),
    ];

    for (label, ids, named) in cases {
        let (status, body) = start_baseline(&harness, &team_arn, Some(&ids)).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400 for {ids:?}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: wrong error code in {body:?}"
        );
        assert!(
            text(&body, "Message").contains(named),
            "case {label}: the refusal must name the offending value {named:?}, got {body:?}"
        );
    }

    // A refused request recorded nothing: the team still has no session.
    let store = state::service_state(&*harness.engine).expect("the service store");
    let sessions = store.lock().team_sessions_in_initiation_order(&team_arn).len();
    assert_eq!(
        sessions, 0,
        "case refused requests: no session may be recorded, got {sessions}"
    );
}

/// A session cannot outlive the team it asks: once the team's pending deletion window has
/// elapsed and the team is gone, its sessions answer the same 404 the team does.
#[tokio::test]
async fn a_session_of_a_removed_team_is_not_found() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "doomed-team", 1, 1).await;
    let (status, started) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(status, 200, "baseline returned {started:?}");
    let session_arn = text(&started, "BaselineSessionArn");

    let (status, _) = harness
        .start_deletion(&team_arn, Some(json!({ "PendingWindowDays": 7 })))
        .await;
    assert_eq!(status, 200, "start_deletion must be accepted for an ACTIVE team");
    expire_deletion_window(&harness, &team_arn);

    let (status, body) = get_session(&harness, &session_arn).await;
    assert_eq!(
        status, 404,
        "case the team is gone: expected 404, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case the team is gone: wrong error code in {body:?}"
    );
    let (status, body) = get_team(&harness, &team_arn).await;
    assert_eq!(
        status, 404,
        "case the team is gone: the team itself must answer 404, got {status} with {body:?}"
    );
}
