// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the store's session ceiling on EVERY operation that starts a
//! session: `StartApprovalTeamBaseline`, `StartActiveApprovalTeamDeletion` and
//! `UpdateApprovalTeam`.
//!
//! The rule all three hold: the store records at most `state::MAX_SESSIONS` sessions, so a
//! request that would record one beyond that is refused — with the error each operation
//! declares for a request it cannot satisfy, and with the SAME sentence whichever
//! operation refuses it — and records nothing, while a request made while the store can
//! still hold a session is accepted and its session recorded.
//!
//! The table's rows differ along how many sessions the store already holds: one below the
//! ceiling, exactly the ceiling, and well past it. The ceiling comes from
//! `state::MAX_SESSIONS`, whose own doc comment owns the reason for the bound.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    message, record_baseline_sessions, start_baseline, wait_for_the_clock,
};
use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};

/// A `CreateApprovalTeam` body every modelled constraint accepts: one approver,
/// `MinApprovalsRequired` 1 and a policy version a fresh store seeds.
fn create_body(name: &str) -> Value {
    json!({
        // `Name` matches `^[a-zA-Z0-9._-]+$`.
        "Name": name,
        "Description": "session ceiling table",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ids-000000000001",
                engine::emulator_account_id()
            ),
        }],
        "Policies": [{
            "PolicyArn": format!(
                "{}/{}",
                state::policy_arn("IdentityCenter", "MPAApprovalTeamPolicy"),
                state::DEFAULT_POLICY_VERSION
            )
        }],
    })
}

/// Two ACTIVE teams: the one the fixture sessions are recorded against, and the one the
/// request under test is made on. The ceiling under test is the STORE's, so the request it
/// refuses is made on a team nothing else about it could refuse.
///
/// Both are created before the clock-owed invitation responses are waited out, so a row
/// costs one wait rather than two.
async fn two_active_teams(harness: &Harness, label: &str) -> (String, String) {
    let mut arns: Vec<String> = Vec::new();
    for role in ["filler", "target"] {
        let (status, created) = harness.create(&create_body(&format!("{label}-{role}"))).await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating the {role} team returned {status} with {created:?}"
        );
        arns.push(text(&created, "Arn"));
    }
    wait_for_the_clock(state::INVITATION_RESPONSE_DELAY);
    for arn in &arns {
        let (status, team) = harness.get(arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading {arn} back returned {status} with {team:?}"
        );
        assert_eq!(
            text(&team, "Status"),
            "ACTIVE",
            "case {label:?}: expected an ACTIVE team before a session is started, got \
             {team:?}"
        );
    }
    (arns[0].clone(), arns[1].clone())
}

/// How many sessions the store really holds.
fn sessions_held(harness: &Harness) -> usize {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.sessions.len()
}

/// One session-starting operation under test.
#[derive(Debug, Clone, Copy)]
enum SessionStart {
    Baseline,
    ActiveDeletion,
    Update,
}

impl SessionStart {
    /// Every operation of this model that records a session.
    fn all() -> [Self; 3] {
        [Self::Baseline, Self::ActiveDeletion, Self::Update]
    }

    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "StartApprovalTeamBaseline",
            Self::ActiveDeletion => "StartActiveApprovalTeamDeletion",
            Self::Update => "UpdateApprovalTeam",
        }
    }

    /// The `(status, error code)` this operation reports the ceiling with: the error ITS
    /// model declares for a request it cannot satisfy — `ServiceQuotaExceededException`
    /// (402) where the operation declares one, and the `ValidationException` (400) where
    /// it declares no quota error.
    fn refusal(self) -> (u16, &'static str) {
        match self {
            Self::Baseline | Self::ActiveDeletion => (400, "ValidationException"),
            Self::Update => (402, "ServiceQuotaExceededException"),
        }
    }

    /// Make the request that would record a session against `team_arn`.
    async fn call(self, harness: &Harness, team_arn: &str) -> (u16, Value) {
        match self {
            Self::Baseline => start_baseline(harness, team_arn, None).await,
            Self::ActiveDeletion => harness.start_deletion(team_arn, None).await,
            // A change to an ACTIVE team's configuration is what its approvers are asked
            // to approve, so the request opens an update session.
            Self::Update => {
                harness
                    .update(team_arn, &json!({ "Description": "a change to be approved" }))
                    .await
            }
        }
    }
}

/// Every operation that starts a session holds the one store ceiling: below it the request
/// is accepted and its session recorded, at it and past it the request earns the refusal
/// its own model declares, with the same sentence on every path, and records nothing.
#[tokio::test]
async fn every_session_start_holds_the_one_store_ceiling() {
    let max = state::MAX_SESSIONS;

    // (row, how many sessions the store already holds, whether the start is accepted)
    let rows: [(&str, usize, bool); 3] = [
        ("one below the ceiling", max - 1, true),
        ("exactly the ceiling", max, false),
        ("well past the ceiling", max + 5, false),
    ];

    // The sentence each operation refused with, per filled store, so the operations are
    // compared with each other rather than with a sentence spelled out here.
    let mut refusals: BTreeMap<usize, Vec<(&'static str, String)>> = BTreeMap::new();

    for (op_index, start) in SessionStart::all().into_iter().enumerate() {
        for (row_index, (row, held, accepted)) in rows.into_iter().enumerate() {
            // A store of its own per row: nothing a previous row recorded can carry in.
            let harness = Harness::new();
            let case = format!("{} / {row}", start.name());
            let (filler_arn, target_arn) =
                two_active_teams(&harness, &format!("ceiling-{op_index}-{row_index}")).await;

            // Fill the store through the state the handlers share, so the test does not
            // have to make `MAX_SESSIONS` requests to reach the bound the handlers
            // enforce. Only the records are added, each through the service's own
            // `start_session`; the bound itself is the handler's.
            record_baseline_sessions(&harness, &filler_arn, held);
            assert_eq!(
                sessions_held(&harness),
                held,
                "case {case:?}: the fixture must leave the store holding {held} sessions"
            );

            let (status, body) = start.call(&harness, &target_arn).await;

            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a start with {held} of {max} sessions held returned \
                     {status} with {body:?}"
                );
                assert_eq!(
                    sessions_held(&harness),
                    held + 1,
                    "case {case:?}: an accepted start must record its session"
                );
                continue;
            }

            let (expected_status, expected_code) = start.refusal();
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status} with {held} of {max} \
                 sessions held, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {body:?}"
            );
            let refusal = message(&body);
            assert!(
                refusal.contains(&max.to_string()),
                "case {case:?}: the refusal must name the ceiling of {max}, it reads \
                 {refusal:?}"
            );
            assert_eq!(
                sessions_held(&harness),
                held,
                "case {case:?}: a refused start must record nothing"
            );
            refusals.entry(held).or_default().push((start.name(), refusal));
        }
    }

    // One rule, one sentence: for every filled store the operations all refused, each
    // operation's message is the others'.
    for (held, raised) in &refusals {
        let (first_name, first) = &raised[0];
        for (name, refusal) in &raised[1..] {
            assert_eq!(
                refusal, first,
                "case {held} sessions held: {name} refuses with {refusal:?}, {first_name} \
                 with {first:?}"
            );
        }
        assert_eq!(
            raised.len(),
            SessionStart::all().len(),
            "case {held} sessions held: only {} of the session-starting operations \
             refused it",
            raised.len()
        );
    }
}
