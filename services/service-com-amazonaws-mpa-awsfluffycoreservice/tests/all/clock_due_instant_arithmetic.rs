// SPDX-License-Identifier: CC0-1.0

//! Tests for the arithmetic every clock-owed DUE INSTANT this service records is computed
//! with.
//!
//! The rule under test: a due instant is the CHECKED sum of the handler's own `now` and
//! the span the transition takes, computed before anything is written — the stored
//! instant is exactly that sum, and a `now` the clock cannot represent that sum for
//! records nothing at all (the construction answers `None` / `false`, mints no id and
//! leaves the record it was asked to move exactly as it was), so the calling operation can
//! answer the modelled server error it declares instead of panicking or half-writing a
//! record.
//!
//! The first table spans the class the rule generalises over: every clock-owed instant
//! this store records (an invited approver's response, an identity source's activation and
//! its removal, the approval a requested change waits on, the approval a requested
//! deletion waits on), each against a set of `now` values (the epoch, the real clock, a
//! century after the epoch, and the last instant this platform's `SystemTime` represents,
//! which no span at all can be added to).
//!
//! The second table drives the same rule end to end over the wire, where `now` is the
//! handler's own clock: every instant a write records sits exactly one span after the
//! instant that write stamped on the record, and the transition it owes is materialised
//! once it falls due. Its rows differ along the number of approvers a team is created
//! with, whether a change adds an approver to it, the number of tags the identity source
//! carries and the length of the pending window the team's deletion asks for.
//!
//! Every request in the second table goes over the wire through the `MultiServiceRouter`
//! the harness in `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the
//! JSON envelope and the HTTP status are all exercised.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{text, Harness, REGION};

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// An IAM Identity Center instance ARN that satisfies `IdcInstanceArn`'s modelled pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
fn instance_arn(n: usize) -> String {
    format!("arn:aws:sso:::instance/ssoins-{n:016}")
}

/// A policy ARN the seeded catalog resolves (`state::seeded_policy_catalog` holds the
/// AWS-managed approval-team policy), since an attachment the catalog cannot resolve is
/// refused.
fn policy_arn() -> String {
    "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string()
}

/// An approver: `(PrimaryIdentityId, PrimaryIdentitySourceArn)`.
///
/// The identity source it authenticates through is numbered well past the sources this
/// suite's own `CreateIdentitySource` calls mint, so the source a row deletes is never one
/// a stored team's approvers use — a deletion of such a source is refused as in use.
fn approver(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{:012}",
            engine::emulator_account_id(),
            n + APPROVER_SOURCE_OFFSET
        ),
    )
}

/// How far past the sources this suite creates the approvers' own sources are numbered.
const APPROVER_SOURCE_OFFSET: usize = 900;

/// `Approvers` is `length(min: 1, max: 20)` — the most a team can carry.
const MAX_APPROVERS: usize = 20;

/// The last instant this platform's `SystemTime` represents, found without assuming the
/// clock's width: the offset from the epoch is doubled until it is no longer
/// representable, bisected to the boundary, and its sub-second part filled to the last
/// nanosecond, so adding any span at all has to carry into the seconds.
fn last_representable_instant() -> SystemTime {
    let mut representable = Duration::ZERO;
    let mut beyond = Duration::from_secs(1);
    while SystemTime::UNIX_EPOCH.checked_add(beyond).is_some() {
        representable = beyond;
        beyond = beyond
            .checked_mul(2)
            .expect("a SystemTime ceiling inside Duration's range");
    }
    while beyond - representable > Duration::from_secs(1) {
        let middle = representable + (beyond - representable) / 2;
        if SystemTime::UNIX_EPOCH.checked_add(middle).is_some() {
            representable = middle;
        } else {
            beyond = middle;
        }
    }
    let filled = representable + Duration::new(0, 999_999_999);
    SystemTime::UNIX_EPOCH
        .checked_add(filled)
        .unwrap_or_else(|| SystemTime::UNIX_EPOCH + representable)
}

/// One clock-owed instant this store records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owed {
    /// The response an invited approver is owed, through the one path that invites them.
    ApproverResponse,
    /// A new identity source's activation.
    SourceActivation,
    /// The removal a started identity-source deletion owes.
    SourceRemoval,
    /// The approval a requested change to a team waits on.
    UpdateApproval,
    /// The approval a requested deletion of a team waits on.
    DeletionApproval,
}

/// Every row of the first table.
fn owed_instants() -> Vec<Owed> {
    vec![
        Owed::ApproverResponse,
        Owed::SourceActivation,
        Owed::SourceRemoval,
        Owed::UpdateApproval,
        Owed::DeletionApproval,
    ]
}

impl Owed {
    /// The span this transition takes, as `crate::state` fixes it.
    fn span(self) -> Duration {
        match self {
            Self::ApproverResponse => state::INVITATION_RESPONSE_DELAY,
            Self::SourceActivation => state::IDENTITY_SOURCE_ACTIVATION_DELAY,
            Self::SourceRemoval => state::IDENTITY_SOURCE_DELETION_DELAY,
            Self::UpdateApproval => state::UPDATE_APPROVAL_DELAY,
            Self::DeletionApproval => state::DELETION_APPROVAL_DELAY,
        }
    }

    /// Record this transition from `now`, through the store path the handlers use, and
    /// answer the due instant it recorded — `None` when it recorded nothing at all.
    fn record(
        self,
        store: &mut state::ComAmazonawsMpaAwsfluffycoreserviceState,
        now: SystemTime,
    ) -> Option<SystemTime> {
        match self {
            Self::ApproverResponse => {
                let (identity_id, source_arn) = approver(1);
                let invited = store.invite_requested_approvers(
                    [(identity_id.as_str(), source_arn.as_str())],
                    &[],
                    now,
                )?;
                invited.first().and_then(|a| a.invitation_response_due)
            }
            Self::SourceActivation => fresh_source(now, 1)?.activation_due,
            Self::SourceRemoval => {
                // Only the REMOVAL is asked for at `now`: the source itself is recorded at
                // an instant the clock can always extend.
                let mut source = fresh_source(SystemTime::UNIX_EPOCH, 1)
                    .expect("a source recorded at the epoch");
                if !source.begin_deletion(now) {
                    return None;
                }
                source.deletion_due
            }
            Self::UpdateApproval => state::PendingTeamUpdate::requested(
                2,
                "a requested change".to_string(),
                state::StoredApprovalStrategy::MofN {
                    min_approvals_required: 1,
                },
                Vec::new(),
                state::session_arn(REGION, "a-team", 1),
                now,
            )
            .approval_due,
            Self::DeletionApproval => {
                state::deletion_schedule(now, state::DEFAULT_PENDING_WINDOW_DAYS)
                    .map(|schedule| schedule.approval_due)
            }
        }
    }
}

/// A freshly created identity source, as `CreateIdentitySource` records one.
fn fresh_source(now: SystemTime, seq: u64) -> Option<state::IdentitySource> {
    state::IdentitySource::creating(
        state::identity_source_arn(REGION, seq),
        state::IdentitySourceType::IamIdentityCenter,
        instance_arn(1),
        REGION.to_string(),
        BTreeMap::new(),
        now,
        seq,
    )
}

/// The `now` values every row of the first table runs against.
fn clock_instants() -> Vec<(&'static str, SystemTime)> {
    vec![
        ("the epoch", SystemTime::UNIX_EPOCH),
        ("the real clock", SystemTime::now()),
        (
            "a century after the epoch",
            SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 365 * 24 * 60 * 60),
        ),
        (
            "the last instant this clock represents",
            last_representable_instant(),
        ),
    ]
}

/// Over every clock-owed instant and every `now`: the instant recorded is the CHECKED sum
/// of `now` and the transition's span, and a `now` that sum cannot be computed from
/// records nothing.
#[test]
fn every_clock_owed_instant_is_the_checked_sum_of_now_and_its_span() {
    let ceiling = last_representable_instant();
    assert!(
        ceiling.checked_add(Duration::from_nanos(1)).is_none(),
        "the last row must name an instant no span can be added to, but {ceiling:?} takes \
         one more nanosecond"
    );

    for owed in owed_instants() {
        for (label, now) in clock_instants() {
            let case = format!("{owed:?} at {label}");
            let mut store = state::ComAmazonawsMpaAwsfluffycoreserviceState::default();
            let recorded = owed.record(&mut store, now);
            let expected = now.checked_add(owed.span());
            assert_eq!(
                recorded, expected,
                "case {case:?}: expected the due instant {expected:?}, got {recorded:?}"
            );
            if now == ceiling {
                assert!(
                    recorded.is_none(),
                    "case {case:?}: an instant the clock cannot extend must record no due \
                     instant at all, got {recorded:?}"
                );
            }
        }
    }
}

/// The other half of the rule: when the sum cannot be computed, nothing is written — no
/// `ApproverId` is minted and the record a deletion was asked for is left exactly as it
/// was, so the calling operation's refusal leaves the store as it found it.
#[test]
fn a_due_instant_the_clock_cannot_compute_writes_nothing() {
    let ceiling = last_representable_instant();

    // The one path that invites approvers mints no id for a response it cannot record.
    let mut store = state::ComAmazonawsMpaAwsfluffycoreserviceState::default();
    let (identity_id, source_arn) = approver(1);
    let refused = store.invite_requested_approvers(
        [(identity_id.as_str(), source_arn.as_str())],
        &[],
        ceiling,
    );
    assert!(
        refused.is_none(),
        "expected no approver set for an instant the clock cannot extend, got {refused:?}"
    );
    let next = store.mint_approver_id();
    let first_of_a_fresh_store =
        state::ComAmazonawsMpaAwsfluffycoreserviceState::default().mint_approver_id();
    assert_eq!(
        next, first_of_a_fresh_store,
        "case {:?}: a refused invitation must consume no ApproverId, so the next id is \
         {first_of_a_fresh_store:?}, got {next:?}",
        "the invitation the clock cannot record"
    );

    // A source whose removal cannot be recorded keeps its status, its activation and its
    // (absent) removal.
    let mut source = fresh_source(SystemTime::UNIX_EPOCH, 1).expect("a source at the epoch");
    let before = source.clone();
    let recorded = source.begin_deletion(ceiling);
    assert!(
        !recorded,
        "case {:?}: expected no removal to be recorded, got {:?}",
        "the removal the clock cannot record", source.deletion_due
    );
    assert_eq!(
        source.status, before.status,
        "case {:?}: the source's status must not move, got {:?}",
        "the removal the clock cannot record", source.status
    );
    assert_eq!(
        source.activation_due, before.activation_due,
        "case {:?}: the source's activation must not move, got {:?}",
        "the removal the clock cannot record", source.activation_due
    );
    assert_eq!(
        source.deletion_due, None,
        "case {:?}: no removal may be owed, got {:?}",
        "the removal the clock cannot record", source.deletion_due
    );

    // And a source whose activation cannot be recorded is not built at all.
    let refused = fresh_source(ceiling, 1);
    assert!(
        refused.is_none(),
        "case {:?}: expected no identity source to be built, got {refused:?}",
        "the activation the clock cannot record"
    );

    // The removal a deletion the clock CAN record owes is recorded once and only once: a
    // repeat keeps the instant the first one scheduled.
    let asked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut source = fresh_source(SystemTime::UNIX_EPOCH, 2).expect("a source at the epoch");
    assert!(
        source.begin_deletion(asked_at),
        "case {:?}: expected the removal to be recorded",
        "the first deletion"
    );
    let first = source.deletion_due;
    assert_eq!(
        first,
        Some(asked_at + state::IDENTITY_SOURCE_DELETION_DELAY),
        "case {:?}: expected the removal one span after the request, got {first:?}",
        "the first deletion"
    );
    assert!(
        source.begin_deletion(asked_at + Duration::from_secs(60)),
        "case {:?}: a source already DELETING is already scheduled",
        "the repeat deletion"
    );
    assert_eq!(
        source.deletion_due, first,
        "case {:?}: a repeat must not postpone the removal, got {:?}",
        "the repeat deletion", source.deletion_due
    );
}

/// One row of the end-to-end table.
struct WireCase {
    case: &'static str,
    /// How many approvers the team is created with. `Approvers` is `length(min: 1, max:
    /// 20)`.
    approvers: usize,
    /// Whether the change asked for after the team is active ADDS an approver.
    adds_an_approver: bool,
    /// Tags on the identity source the row creates.
    source_tags: usize,
    /// `PendingWindowDays` the row's deletion asks for, or `None` for the default
    /// (`state::DEFAULT_PENDING_WINDOW_DAYS`).
    pending_window_days: Option<i32>,
}

/// Rows differ along the approver count, whether a change adds an approver, the number of
/// tags the identity source carries and the pending window the deletion asks for.
fn wire_cases() -> Vec<WireCase> {
    vec![
        WireCase {
            case: "one approver, no change, an untagged source, the default window",
            approvers: 1,
            adds_an_approver: false,
            source_tags: 0,
            pending_window_days: None,
        },
        WireCase {
            case: "three approvers, a change that adds one, three tags, the shortest window",
            approvers: 3,
            adds_an_approver: true,
            source_tags: 3,
            // `PendingWindowDays` is `range(min: 1, max: 36500)` — the minimum.
            pending_window_days: Some(1),
        },
        WireCase {
            case: "the maximum approvers, a change that adds one, the maximum tags, the \
                   longest window",
            // `Approvers` is `length(min: 1, max: 20)` — the maximum.
            approvers: MAX_APPROVERS,
            adds_an_approver: true,
            // `state::MAX_TAGS_PER_RESOURCE`: the ceiling this service enforces,
            // corroborated by `UntagResource$TagKeys`' `length(min: 0, max: 50)`.
            source_tags: state::MAX_TAGS_PER_RESOURCE,
            // `state::MAX_PENDING_WINDOW_DAYS` — the longest window this service records.
            pending_window_days: Some(state::MAX_PENDING_WINDOW_DAYS),
        },
    ]
}

/// The `CreateApprovalTeam` body for a row.
fn create_team_body(name: &str, approvers: usize, min_approvals: usize) -> Value {
    json!({
        "Name": name,
        "Description": "clock arithmetic",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": min_approvals } },
        "Approvers": (1..=approvers)
            .map(approver)
            .map(|(id, source)| json!({
                "PrimaryIdentityId": id,
                "PrimaryIdentitySourceArn": source,
            }))
            .collect::<Vec<_>>(),
        "Policies": [{ "PolicyArn": policy_arn() }],
    })
}

/// The `CreateIdentitySource` body for a row.
fn create_source_body(n: usize, tags: usize) -> Value {
    let mut body = json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": instance_arn(n),
                "Region": REGION,
            }
        }
    });
    if tags > 0 {
        body["Tags"] = Value::Object(
            (1..=tags)
                .map(|t| (format!("key{t:02}"), json!(format!("value{t:02}"))))
                .collect(),
        );
    }
    body
}

/// The team the store holds, cloned so the assertions do not hold the store's lock.
fn stored_team(harness: &Harness, arn: &str) -> state::ApprovalTeam {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.team(arn).expect("a stored team").clone()
}

/// The identity source the store holds, if it still holds one.
fn stored_source(harness: &Harness, arn: &str) -> Option<state::IdentitySource> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.identity_source(arn).cloned()
}

/// `POST /identity-sources/?List`: the status reported for `arn`, or `None` when the
/// listing no longer carries it.
async fn listed_source_status(harness: &Harness, arn: &str) -> Option<String> {
    let (status, body) = harness.call("POST", "/identity-sources/?List", None).await;
    assert_eq!(status, 200, "ListIdentitySources returned {body:?}");
    body.get("IdentitySources")
        .and_then(Value::as_array)
        .and_then(|sources| {
            sources
                .iter()
                .find(|source| text(source, "IdentitySourceArn") == arn)
                .map(|source| text(source, "Status"))
        })
}

/// Wait out a clock-owed transition of `span`.
fn wait_for_the_clock(span: Duration) {
    std::thread::sleep(span + CLOCK_SLACK);
}

/// End to end: every instant these write paths record is one span after the instant the
/// same write stamped on the record, and the transition it owes is materialised once it
/// falls due.
#[tokio::test]
async fn every_write_records_its_due_instant_one_span_after_its_own_stamp() {
    for (index, case) in wire_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();

        // 1. A created team: every approver it invites is owed a response one
        //    INVITATION_RESPONSE_DELAY after the instant the team was created at.
        let (status, created) = harness
            .create(&create_team_body(
                &format!("clock-arithmetic-{index}"),
                case.approvers,
                1,
            ))
            .await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let team_arn = text(&created, "Arn");
        let team = stored_team(&harness, &team_arn);
        assert_eq!(
            team.approvers.len(),
            case.approvers,
            "case {label}: expected {} invited approvers, got {:?}",
            case.approvers,
            team.approvers.len()
        );
        for approver in &team.approvers {
            assert_eq!(
                approver.invitation_response_due,
                Some(team.creation_time + state::INVITATION_RESPONSE_DELAY),
                "case {label}: expected approver {:?} to be owed a response one span after \
                 {:?}, got {:?}",
                approver.approver_id,
                team.creation_time,
                approver.invitation_response_due
            );
        }

        // The transition that instant owes is materialised once it falls due, and the
        // team reads back ACTIVE through GetApprovalTeam.
        wait_for_the_clock(state::INVITATION_RESPONSE_DELAY);
        let (status, read_back) = harness.get(&team_arn).await;
        assert_eq!(status, 200, "case {label}: get returned {read_back:?}");
        assert_eq!(
            text(&read_back, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case {label}: the invited approvers' responses were due, so the team is \
             active, got {read_back:?}"
        );

        // 2. A change an ACTIVE team's approvers are asked to approve: the approval is
        //    owed one UPDATE_APPROVAL_DELAY after the instant the change was requested
        //    at, and an approver the change ADDS is owed a response one
        //    INVITATION_RESPONSE_DELAY after that same instant.
        if case.adds_an_approver {
            let (added_id, added_source) = approver(case.approvers + 1);
            // A team already carrying the maximum `Approvers` keeps one fewer than it has,
            // to make room for the approver the change adds.
            let kept = case.approvers.min(MAX_APPROVERS - 1);
            let requested: Vec<Value> = (1..=kept)
                .map(approver)
                .chain(std::iter::once((added_id.clone(), added_source)))
                .map(|(id, source)| {
                    json!({
                        "PrimaryIdentityId": id,
                        "PrimaryIdentitySourceArn": source,
                    })
                })
                .collect();
            let (status, updated) = harness
                .update(&team_arn, &json!({ "Approvers": requested }))
                .await;
            assert_eq!(status, 200, "case {label}: update returned {updated:?}");

            let team = stored_team(&harness, &team_arn);
            let pending = team
                .pending_update
                .as_ref()
                .unwrap_or_else(|| panic!("case {label}: the change must be recorded"));
            assert_eq!(
                pending.approval_due,
                Some(pending.update_initiation_time + state::UPDATE_APPROVAL_DELAY),
                "case {label}: expected the approval one span after {:?}, got {:?}",
                pending.update_initiation_time,
                pending.approval_due
            );
            for approver in &pending.approvers {
                let expected = if approver.primary_identity_id == added_id {
                    // The approver the change adds is newly invited.
                    Some(pending.update_initiation_time + state::INVITATION_RESPONSE_DELAY)
                } else {
                    // An approver the change keeps keeps the response it already gave:
                    // nothing is owed for it any more.
                    None
                };
                assert_eq!(
                    approver.invitation_response_due, expected,
                    "case {label}: expected approver {:?} to be owed {expected:?}, got {:?}",
                    approver.primary_identity_id, approver.invitation_response_due
                );
            }
        }

        // 3. A created identity source: its activation is owed one
        //    IDENTITY_SOURCE_ACTIVATION_DELAY after the instant it was created at.
        let (status, created) = harness
            .call(
                "POST",
                "/identity-sources",
                Some(create_source_body(index + 1, case.source_tags)),
            )
            .await;
        assert_eq!(status, 200, "case {label}: create source returned {created:?}");
        let source_arn = text(&created, "IdentitySourceArn");
        let source = stored_source(&harness, &source_arn)
            .unwrap_or_else(|| panic!("case {label}: the source must be stored"));
        assert_eq!(
            source.activation_due,
            Some(source.creation_time + state::IDENTITY_SOURCE_ACTIVATION_DELAY),
            "case {label}: expected the activation one span after {:?}, got {:?}",
            source.creation_time,
            source.activation_due
        );

        wait_for_the_clock(state::IDENTITY_SOURCE_ACTIVATION_DELAY);
        let listed = listed_source_status(&harness, &source_arn).await;
        assert_eq!(
            listed.as_deref(),
            Some(state::IdentitySourceStatus::Active.as_wire()),
            "case {label}: the activation was due, so the source is active, got {listed:?}"
        );

        // 4. A started identity-source deletion: the removal is owed one
        //    IDENTITY_SOURCE_DELETION_DELAY after the call, which is the only instant here
        //    no response carries — so it is bracketed by the call itself.
        let asked_from = SystemTime::now();
        let (status, body) = harness
            .call(
                "DELETE",
                &format!(
                    "/identity-sources/{}",
                    crate::approval_team_lifecycle::encode_label(&source_arn)
                ),
                None,
            )
            .await;
        let asked_until = SystemTime::now();
        assert_eq!(status, 200, "case {label}: delete source returned {body:?}");
        let source = stored_source(&harness, &source_arn)
            .unwrap_or_else(|| panic!("case {label}: the source must still be stored"));
        assert_eq!(
            source.status,
            state::IdentitySourceStatus::Deleting,
            "case {label}: expected the source to be DELETING, got {:?}",
            source.status
        );
        let due = source
            .deletion_due
            .unwrap_or_else(|| panic!("case {label}: the removal must be owed at an instant"));
        assert!(
            due >= asked_from + state::IDENTITY_SOURCE_DELETION_DELAY
                && due <= asked_until + state::IDENTITY_SOURCE_DELETION_DELAY,
            "case {label}: expected the removal one span after an instant inside the call \
             ({asked_from:?}..{asked_until:?}), got {due:?}"
        );
        assert_eq!(
            source.activation_due, None,
            "case {label}: a source being removed waits on no activation, got {:?}",
            source.activation_due
        );

        wait_for_the_clock(state::IDENTITY_SOURCE_DELETION_DELAY);
        let listed = listed_source_status(&harness, &source_arn).await;
        assert_eq!(
            listed, None,
            "case {label}: the removal was due, so the source is gone, got {listed:?}"
        );

        // 5. A requested deletion of the team: the approval is owed one
        //    DELETION_APPROVAL_DELAY after the instant the request was recorded at, and
        //    the window the request asked for runs from that approval.
        let deletion_team = format!("clock-arithmetic-deletion-{index}");
        let (status, created) = harness
            .create(&create_team_body(&deletion_team, case.approvers, 1))
            .await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let deleted_arn = text(&created, "Arn");
        wait_for_the_clock(state::INVITATION_RESPONSE_DELAY);
        let body = case
            .pending_window_days
            .map(|days| json!({ "PendingWindowDays": days }));
        let (status, started) = harness.start_deletion(&deleted_arn, body).await;
        assert_eq!(
            status, 200,
            "case {label}: start deletion returned {status} with {started:?}"
        );
        let team = stored_team(&harness, &deleted_arn);
        let deletion = team
            .pending_deletion
            .as_ref()
            .unwrap_or_else(|| panic!("case {label}: the deletion must be recorded"));
        assert_eq!(
            deletion.approval_due,
            Some(deletion.start_time + state::DELETION_APPROVAL_DELAY),
            "case {label}: expected the approval one span after {:?}, got {:?}",
            deletion.start_time,
            deletion.approval_due
        );
        let window = state::pending_window(
            case.pending_window_days
                .unwrap_or(state::DEFAULT_PENDING_WINDOW_DAYS),
        )
        .unwrap_or_else(|| panic!("case {label}: the window must be a duration"));
        assert_eq!(
            Some(deletion.completion_time),
            deletion.approval_due.map(|due| due + window),
            "case {label}: expected the removal one window after the approval, got {:?}",
            deletion.completion_time
        );
    }
}
