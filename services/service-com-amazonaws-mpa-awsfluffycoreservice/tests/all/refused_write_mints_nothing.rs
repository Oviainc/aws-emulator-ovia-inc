// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the write boundary the write paths of this service hold: a
//! request the service REFUSES leaves the store exactly as it found it.
//!
//! The rule under test: every service-minted id a write path hands out — an approver's
//! `ApproverId`, and the initiation-order id a session's ARN carries — is consumed by a
//! change that is really recorded and by nothing else, so a refused request records
//! nothing and the next accepted request mints the id that FOLLOWS the last one the store
//! holds. A caller that reads a team, a requested version or a session therefore never
//! sees an id skipped for a reason no response explains.
//!
//! The table spans the class the rule generalises over: which refusal the request earns
//! (M above N with the approvers replaced, with `Approvers` absent, with approvers added,
//! and at the maximum `Approvers` length; one identity named twice; the session quota),
//! how many approvers the refused request names (0 named, 1, 3 and the `Approvers`
//! `length(1..20)` maximum) and how many of those are new to the team (none, one, two,
//! all).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, start_baseline, MAX_APPROVERS};
use crate::approval_team_lifecycle::{error_code, text, Harness, REGION};

/// An approver the team `approval_sessions::active_team` records already carries, so a
/// change that names it KEEPS it.
fn kept_approver(n: usize) -> Value {
    json!({
        "PrimaryIdentityId": format!("identity-{n:04}"),
        "PrimaryIdentitySourceArn": format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{n:012}",
            engine::emulator_account_id()
        ),
    })
}

/// An approver no team in these tables carries, so a change that names it ADDS it — and
/// an added approver is the one an `ApproverId` is minted for.
fn added_approver(n: usize) -> Value {
    json!({
        "PrimaryIdentityId": format!("identity-added-{n:04}"),
        "PrimaryIdentitySourceArn": format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-added-{n:04}",
            engine::emulator_account_id()
        ),
    })
}

/// A policy reference the seeded catalog resolves (`state::seeded_policy_catalog` holds
/// the AWS-managed approval-team policy), since an attachment it cannot resolve is
/// refused.
fn policy_arn() -> String {
    "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string()
}

/// What the store HOLDS, read straight out of it: every team with the version it runs on,
/// its live approvers' ids and the ids of the version it is waiting on, plus every
/// session's ARN. Two reads that agree say a request changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreShape {
    teams: Vec<TeamShape>,
    sessions: Vec<String>,
}

/// One stored team, as the shape above records it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TeamShape {
    arn: String,
    version_id: String,
    /// The ids of the approvers the team runs with.
    live: Vec<String>,
    /// The ids of the approvers of the version it is waiting on, if any.
    requested: Option<Vec<String>>,
}

impl StoreShape {
    fn read(harness: &Harness) -> Self {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let guard = store.lock();
        let teams = guard
            .approval_teams
            .values()
            .map(|team| TeamShape {
                arn: team.arn.clone(),
                version_id: team.version_id.clone(),
                live: team
                    .approvers
                    .iter()
                    .map(|approver| approver.approver_id.clone())
                    .collect(),
                requested: team.pending_update.as_ref().map(|update| {
                    update
                        .approvers
                        .iter()
                        .map(|approver| approver.approver_id.clone())
                        .collect()
                }),
            })
            .collect();
        let sessions = guard.sessions.keys().cloned().collect();
        Self { teams, sessions }
    }

    /// Every `ApproverId` the store really holds, live or requested.
    fn approver_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for team in &self.teams {
            ids.extend(team.live.iter().cloned());
            ids.extend(team.requested.iter().flatten().cloned());
        }
        ids
    }

    /// The `ApproverId` the next approver this store invites must carry: the successor of
    /// the highest sequence number any id it holds carries.
    fn next_approver_id(&self) -> String {
        let highest = self
            .approver_ids()
            .iter()
            .filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        format!("approver-{}", highest + 1)
    }

    /// The tail every session ARN carries (`session-<seq>`), for the session the next
    /// accepted start or update must record: the successor of the highest sequence number
    /// the store's own session ARNs carry.
    fn next_session_arn_suffix(&self) -> String {
        let highest = self
            .sessions
            .iter()
            .filter_map(|arn| arn.rsplit_once("/session-").and_then(|(_, n)| n.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);
        format!("session-{:012}", highest + 1)
    }
}

/// The `ApproverId`s a `GetApprovalTeam` response lists, for the live configuration
/// (`member` `None`) or for the version the team is waiting on.
fn reported_approver_ids(body: &Value, member: Option<&str>) -> Vec<String> {
    let approvers = match member {
        None => &body["Approvers"],
        Some(member) => &body[member]["Approvers"],
    };
    approvers
        .as_array()
        .map(|list| list.iter().map(|approver| text(approver, "ApproverId")).collect())
        .unwrap_or_default()
}

/// One row of the refused-write table.
struct RefusedCase {
    case: &'static str,
    /// The `UpdateApprovalTeam` body the service refuses.
    patch: Value,
    status: u16,
    code: &'static str,
    /// Fill the store to its session ceiling (`state::MAX_SESSIONS`) first, so the
    /// request earns the quota refusal rather than recording a session.
    fill_sessions: bool,
}

/// The table: each row is a refusal of a different class, and the rows differ in how many
/// approvers the refused request names and how many of them are new to the team.
fn refused_cases() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            case: "M above N: the approvers replaced by one the team already has",
            patch: json!({
                "Approvers": [kept_approver(1)],
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 2 } },
            }),
            status: 400,
            code: "ValidationException",
            fill_sessions: false,
        },
        RefusedCase {
            case: "M above N: Approvers absent, M above the team's own count",
            patch: json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 3 } } }),
            status: 400,
            code: "ValidationException",
            fill_sessions: false,
        },
        RefusedCase {
            case: "M above N: three approvers, two of them added",
            patch: json!({
                "Approvers": [kept_approver(1), added_approver(1), added_approver(2)],
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 20 } },
            }),
            status: 400,
            code: "ValidationException",
            fill_sessions: false,
        },
        RefusedCase {
            case: "M above N: the maximum approvers, every one of them added",
            patch: json!({
                // `Approvers` is `length(min: 1, max: 20)` — the maximum — and
                // `MofNApprovalStrategy$MinApprovalsRequired` is `range(min: 1)` with no
                // maximum, so one past the count is a value the model accepts.
                "Approvers": (1..=MAX_APPROVERS).map(added_approver).collect::<Vec<_>>(),
                "ApprovalStrategy": {
                    "MofN": { "MinApprovalsRequired": MAX_APPROVERS + 1 },
                },
            }),
            status: 400,
            code: "ValidationException",
            fill_sessions: false,
        },
        RefusedCase {
            case: "one added identity named twice",
            patch: json!({ "Approvers": [added_approver(3), added_approver(3)] }),
            status: 400,
            code: "ValidationException",
            fill_sessions: false,
        },
        RefusedCase {
            case: "a change that is otherwise valid, past the session quota",
            patch: json!({
                "Approvers": [kept_approver(1), kept_approver(2), added_approver(4)],
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            }),
            status: 402,
            code: "ServiceQuotaExceededException",
            fill_sessions: true,
        },
    ]
}

/// Fill the store to its session ceiling through the state the handlers share, so the
/// test does not have to make `MAX_SESSIONS` requests to reach the bound the handler
/// enforces. Only the records are added, each under the next initiation-order id the
/// store hands out; the bound itself is the handler's.
async fn fill_the_session_store(harness: &Harness, team_arn: &str) {
    let (status, started) = start_baseline(harness, team_arn, None).await;
    assert_eq!(status, 200, "the filler baseline returned {started:?}");
    let first = text(&started, "BaselineSessionArn");
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

/// Over the whole table: the refused change records nothing, and the next approver the
/// store invites carries the id that follows the last one it really holds.
#[tokio::test]
async fn a_refused_change_records_nothing_and_consumes_no_approver_id() {
    for (index, case) in refused_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(&harness, &format!("refused-write-{index}"), 2, 1).await;
        if case.fill_sessions {
            fill_the_session_store(&harness, &arn).await;
        }

        let before = StoreShape::read(&harness);
        let expected_id = before.next_approver_id();

        let (status, body) = harness.update(&arn, &case.patch).await;
        assert_eq!(
            status, case.status,
            "case {label}: expected {}, got {status} with {body:?}",
            case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {label}: wrong error code in {body:?}"
        );

        // Nothing the store holds moved: no approver was invited, no version requested
        // and no session recorded.
        let after = StoreShape::read(&harness);
        assert_eq!(
            after, before,
            "case {label}: a refused change must leave the store as it was, got {after:?}"
        );

        // The team reads back on the configuration it was running on, with no version
        // waiting for its approvers.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert!(
            team.get("PendingUpdate").is_none_or(Value::is_null),
            "case {label}: a refused change must record no PendingUpdate, got {team:?}"
        );

        // The next accepted write that invites an approver mints the id that FOLLOWS the
        // last one the store holds — the refused request consumed none.
        let (status, created) = harness
            .create(&json!({
                "Name": format!("successor-team-{index}"),
                "Description": "the next accepted write",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": [added_approver(100)],
                "Policies": [{ "PolicyArn": policy_arn() }],
            }))
            .await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let successor_arn = text(&created, "Arn");
        let (status, successor) = harness.get(&successor_arn).await;
        assert_eq!(status, 200, "case {label}: get returned {successor:?}");
        assert_eq!(
            reported_approver_ids(&successor, None),
            vec![expected_id.clone()],
            "case {label}: expected the next approver to carry {expected_id:?}, got \
             {successor:?}"
        );
    }
}

/// The ids a caller reads are consecutive however many times a change is refused in
/// between: N refusals skip no id, and the accepted change that follows them carries the
/// successor of the last id the store holds — for the approver it invites and for the
/// session it records.
#[tokio::test]
async fn repeated_refusals_skip_no_approver_or_session_id() {
    let harness = Harness::new();
    let arn = active_team(&harness, "repeated-refusals", 1, 1).await;
    let before = StoreShape::read(&harness);
    let expected_approver = before.next_approver_id();
    let expected_session = before.next_session_arn_suffix();
    assert!(
        before.sessions.is_empty(),
        "a freshly created team has started no session, got {before:?}"
    );

    // The same unsatisfiable change, asked for again and again.
    let refused = json!({
        "Approvers": [added_approver(1)],
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 2 } },
    });
    for attempt in 1..=3 {
        let (status, body) = harness.update(&arn, &refused).await;
        assert_eq!(
            status, 400,
            "attempt {attempt}: expected 400, got {status} with {body:?}"
        );
        let after = StoreShape::read(&harness);
        assert_eq!(
            after, before,
            "attempt {attempt}: a refused change must leave the store as it was, got \
             {after:?}"
        );
    }

    // The accepted change that follows them: the approver it adds carries the next id,
    // and the session it is put to the team in carries the next initiation-order id.
    let (status, updated) = harness
        .update(
            &arn,
            &json!({
                "Approvers": [kept_approver(1), added_approver(1)],
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            }),
        )
        .await;
    assert_eq!(status, 200, "the accepted change returned {updated:?}");

    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {team:?}");
    let requested = reported_approver_ids(&team, Some("PendingUpdate"));
    assert_eq!(
        requested,
        vec![
            before
                .approver_ids()
                .first()
                .cloned()
                .expect("the team's one approver"),
            expected_approver.clone(),
        ],
        "expected the kept approver's own id and {expected_approver:?} for the added one, \
         got {team:?}"
    );
    let session_arn = text(&team, "UpdateSessionArn");
    assert!(
        session_arn.ends_with(&expected_session),
        "expected the update session's ARN to end with {expected_session:?}, got \
         {session_arn:?}"
    );
}
