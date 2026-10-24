// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `StartApprovalTeamBaseline` MINTS, over and above the
//! single-session round trip `approval_sessions.rs` pins.
//!
//! The rules under test:
//!
//! * every accepted start RECORDS A NEW session: each call mints its own `SessionArn`,
//!   distinct from every ARN the store already holds, readable on its own through
//!   `GetSession`, carrying the approver set that call selected, and leaving the sessions
//!   started before it untouched. A session id is never reused, so the store's
//!   initiation order and the order the ARNs were minted in agree.
//! * the store is BOUNDED: once it holds `state::MAX_SESSIONS` sessions another start is
//!   refused with the modelled `ValidationException` (400) — the only refusal this
//!   operation declares for a request it cannot satisfy — and records nothing.
//!
//! The table spans the class the first rule generalises over (how many starts a team
//! takes, how many of its approvers each start selects, whether `ApproverIds` is present
//! at all, approver count, team-name form), not one shape. Where it names a published
//! bound a comment names the modelled member it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts.

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    active_team, get_session, start_baseline, stored_approver_ids, MAX_APPROVERS, MAX_TEAM_NAME,
};
use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};

/// One row of the minting table.
struct MintingCase {
    case: &'static str,
    team_name: String,
    approvers: usize,
    min_approvals: i64,
    /// One entry per start the row makes, naming how many of the team's approvers that
    /// start selects; `None` leaves `ApproverIds` absent, which asks the whole team.
    starts: Vec<Option<usize>>,
}

/// The minting table: rows differ along approver count, the number of starts, how many
/// approvers each start selects, whether `ApproverIds` is present and team-name form.
fn minting_cases() -> Vec<MintingCase> {
    vec![
        MintingCase {
            case: "one approver, a single start with ApproverIds absent",
            team_name: "mint-solo".to_string(),
            approvers: 1,
            min_approvals: 1,
            starts: vec![None],
        },
        MintingCase {
            case: "two approvers, two starts, absent then explicit",
            team_name: "Mint.Team_2".to_string(),
            approvers: 2,
            min_approvals: 1,
            starts: vec![None, Some(1)],
        },
        MintingCase {
            case: "four approvers, three starts selecting growing subsets",
            team_name: "m".to_string(),
            approvers: 4,
            min_approvals: 2,
            starts: vec![Some(1), Some(2), Some(4)],
        },
        MintingCase {
            case: "maximum approvers, longest name, five starts",
            // `ApprovalTeamName` is `length(max: 64)`; `Approvers` is `length(1..20)`.
            team_name: "n".repeat(MAX_TEAM_NAME),
            approvers: MAX_APPROVERS,
            min_approvals: 20,
            starts: vec![Some(MAX_APPROVERS), None, Some(1), Some(7), None],
        },
    ]
}

/// The `ApproverId`s reported for a session, in the order `GetSession` reports them.
fn reported_approver_ids(session: &Value) -> Vec<String> {
    session
        .get("ApproverResponses")
        .and_then(Value::as_array)
        .map(|responses| {
            responses
                .iter()
                .map(|response| text(response, "ApproverId"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every accepted start mints a NEW session: a distinct ARN, its own approver set, and
/// no disturbance to the sessions started before it.
#[tokio::test]
async fn every_accepted_start_mints_a_new_distinct_session() {
    let harness = Harness::new();
    // Every session ARN this test has seen, across every team: an id is never reused.
    let mut every_arn: BTreeSet<String> = BTreeSet::new();

    for case in minting_cases() {
        let label = case.case;
        let team_arn =
            active_team(&harness, &case.team_name, case.approvers, case.min_approvals).await;
        let team_approver_ids = stored_approver_ids(&harness, &team_arn);
        assert_eq!(
            team_approver_ids.len(),
            case.approvers,
            "case {label}: expected {} stored approvers, got {team_approver_ids:?}",
            case.approvers
        );

        // Each start, in order, with the approvers that row selects.
        let mut minted: Vec<String> = Vec::new();
        let mut expected_ids: Vec<Vec<String>> = Vec::new();
        for (nth, selected) in case.starts.iter().enumerate() {
            let selection: Option<Vec<String>> =
                selected.map(|count| team_approver_ids[..count].to_vec());
            let (status, started) = start_baseline(&harness, &team_arn, selection.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {label}, start {nth}: expected 200, got {status} with {started:?}"
            );
            let session_arn = text(&started, "BaselineSessionArn");
            assert!(
                every_arn.insert(session_arn.clone()),
                "case {label}, start {nth}: expected an unused SessionArn, got the \
                 already-minted {session_arn:?}"
            );
            minted.push(session_arn);
            expected_ids.push(selection.unwrap_or_else(|| team_approver_ids.clone()));
        }

        // The store holds exactly the sessions that were minted, in minting order, and
        // its initiation-order sequence never goes backwards.
        let (stored_arns, stored_seqs): (Vec<String>, Vec<u64>) = {
            let store = state::service_state(&*harness.engine).expect("the service store");
            let guard = store.lock();
            guard
                .team_sessions_in_initiation_order(&team_arn)
                .into_iter()
                .map(|session| (session.arn.clone(), session.seq))
                .unzip()
        };
        assert_eq!(
            stored_arns, minted,
            "case {label}: expected the store to hold the minted sessions in minting \
             order, got {stored_arns:?}"
        );
        assert!(
            stored_seqs.windows(2).all(|pair| pair[0] < pair[1]),
            "case {label}: expected strictly increasing initiation sequence numbers, got \
             {stored_seqs:?}"
        );

        // Every minted session reads back on its own, with the approver set its own start
        // selected: a later start leaves an earlier session alone.
        for (nth, session_arn) in minted.iter().enumerate() {
            let (status, session) = get_session(&harness, session_arn).await;
            assert_eq!(
                status, 200,
                "case {label}, session {nth}: expected 200, got {status} with {session:?}"
            );
            assert_eq!(
                text(&session, "SessionArn"),
                *session_arn,
                "case {label}, session {nth}: wrong SessionArn in {session:?}"
            );
            assert_eq!(
                text(&session, "ApprovalTeamArn"),
                team_arn,
                "case {label}, session {nth}: wrong ApprovalTeamArn in {session:?}"
            );
            assert_eq!(
                session["NumberOfApprovers"].as_u64(),
                Some(expected_ids[nth].len() as u64),
                "case {label}, session {nth}: expected {} approvers, got {session:?}",
                expected_ids[nth].len()
            );
            assert_eq!(
                reported_approver_ids(&session),
                expected_ids[nth],
                "case {label}, session {nth}: wrong approver set in {session:?}"
            );
        }
    }
}

/// The store is bounded: once it holds `MAX_SESSIONS` sessions another start earns the
/// modelled `ValidationException` rather than growing this process, and records nothing.
#[tokio::test]
async fn a_start_past_the_session_quota_is_refused() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "quota-session-team", 2, 1).await;
    let (status, started) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(status, 200, "the first start returned {started:?}");
    let first = text(&started, "BaselineSessionArn");

    // Fill the store to its ceiling through the state the handlers share, so the test
    // does not have to make MAX_SESSIONS requests to reach the bound the handler
    // enforces. Only the records are added; the bound itself is the handler's.
    {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let mut guard = store.lock();
        let template = guard.session(&first).expect("the started session").clone();
        while guard.sessions.len() < state::MAX_SESSIONS {
            let seq = guard.next_session_seq();
            let mut filler = template.clone();
            filler.arn = state::session_arn(REGION, &template.approval_team_name, seq);
            filler.seq = seq;
            guard.insert_session(filler);
        }
    }

    let (status, body) = start_baseline(&harness, &team_arn, None).await;
    assert_eq!(
        status, 400,
        "case past the session quota: expected 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case past the session quota: wrong error code in {body:?}"
    );
    // The refused start recorded nothing, and the sessions the store already held still
    // read back.
    let store = state::service_state(&*harness.engine).expect("the service store");
    let held = store.lock().sessions.len();
    assert_eq!(
        held,
        state::MAX_SESSIONS,
        "case past the session quota: a refused start must record nothing, got {held}"
    );
    let (status, session) = get_session(&harness, &first).await;
    assert_eq!(
        status, 200,
        "case past the session quota: the session already held must still read back, got \
         {status} with {session:?}"
    );
}
