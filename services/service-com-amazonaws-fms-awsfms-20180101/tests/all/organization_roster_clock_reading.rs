// SPDX-License-Identifier: CC0-1.0

//! Tests for the instant the organisation's ROSTER is derived at.
//!
//! The rule asserted here, in one sentence: every clock-derived derivation of
//! the organisation this store records — who its accounts are, which of them a
//! stored policy applies to, and whether an item's ARN names one of them —
//! answers AS OF the instant its caller hands it, so one request decides its
//! administrator gate and everything it classifies, pages and resolves from ONE
//! reading of a clock that is not monotonic.
//!
//! The classes the rule ranges over:
//!
//! * the INSTANT, relative to the completion instant of a requested tear-down of
//!   the default-administrator association: long before it, a nanosecond either
//!   side of it, exactly ON it, and long after — so the half-open boundary is
//!   pinned rather than only its interior;
//! * the ROLE the association carries, every value of the model's own
//!   `AccountRoleStatus` catalog (`com.amazonaws.fms#AccountRoleStatus`),
//!   crossed with where the completion instant sits;
//! * the DERIVATION: the roster itself, the roster joined with the discoverable
//!   inventory, a policy's scope read through the roster, the resolution of ONE
//!   named account against that scope, and the per-item classification both
//!   batch membership operations make against it;
//! * the ACCOUNT the derivation is read for: the administrator's own (which the
//!   clock decides) beside an account a stored policy's scope map names and one
//!   no record of the store names (which it does not), so a derivation that
//!   simply ignored its instant fails here too.
//!
//! At every row every derivation is read at the SAME instant as the gate, and
//! all of them must agree: a call the gate admits is one whose roster still
//! holds the administrator's own account, so that account's own resources are
//! not classified as belonging to no account of the organisation.
//!
//! [`a_roster_derived_answer_agrees_with_the_gate_that_admitted_the_call`] then
//! drives the operations the rule covers through the awsJson1_1 harness in
//! [`crate::policy_lifecycle`], so the assertions are about the WIRE response.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{Policy, ACCOUNT_ROLE_STATUS_VALUES};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gate_clock_reading::association;
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::compliance_status::put_policy;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The error code an organisation-scoped call answers when no administrator
/// association is active at the instant it decides from.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// An account the organisation holds because a stored policy's scope map names
/// it. Nothing about it is clock-derived, so it is in the roster at every
/// instant. The model constrains `AWSAccountId` to `^[0-9]+$`, length 1..1024.
const SCOPED_ACCOUNT: &str = "111111111111";

/// An account no record of this store names, so it is in the roster at no
/// instant.
const FOREIGN_ACCOUNT: &str = "999999999999";

/// The resource type the resource set under test declares, read off this
/// service's own catalog of the resource types a set may name.
const ITEM_TYPE: &str = state::RESOURCE_TYPE_EC2_INSTANCE;

/// The `FailedItemReason` an item whose ARN names no account of the
/// organisation is reported with (the model's `com.amazonaws.fms#FailedItemReason`).
const NOT_VALID_ACCOUNT_ID: &str = "NOT_VALID_ACCOUNT_ID";

/// A resource of `account`, as the service that owns it writes the ARN: an ARN
/// carrying every field, so the account is the only field under test.
fn item_of(account: &str) -> String {
    format!("arn:aws:ec2:{REGION}:{account}:instance/i-0123456789abcdef0")
}

/// What ONE reading of the store at ONE instant says about an account: every
/// roster-derived answer the operations of this service are built from, read AS
/// OF that instant.
#[derive(Debug)]
struct RosterReadings {
    /// Whether the gate admits an organisation-scoped call at this instant.
    gate_admits: bool,
    /// Whether the roster holds the account
    /// (`ComAmazonawsFmsAwsfms20180101State::member_accounts`).
    in_member_accounts: bool,
    /// Whether the roster joined with the discoverable inventory holds it
    /// (`state::organization_accounts`).
    in_organization_accounts: bool,
    /// Whether a policy naming no `IncludeMap` — "Firewall Manager applies the
    /// policy to all accounts except for those specified by the `ExcludeMap`" —
    /// applies to it.
    in_policy_scope: bool,
    /// The same resolution made for THAT ONE account
    /// (`policy_applies_to_account`), which the compliance surfaces answer
    /// their not-found from.
    policy_applies: bool,
    /// Whether an item whose ARN names that account is associable to a resource
    /// set declaring [`ITEM_TYPE`]: the classification both batch membership
    /// operations make.
    item_associable: bool,
}

/// Read every roster-derived answer about `account` at `now`, through the same
/// store functions the handlers reach for.
fn readings(
    store: &state::ComAmazonawsFmsAwsfms20180101State,
    policy: &Policy,
    account: &str,
    now: SystemTime,
) -> RosterReadings {
    // No discoverable inventory in these stores, so the join contributes
    // nothing of its own and what it reports is the roster.
    let accounts = state::organization_accounts(store, BTreeSet::new(), now);
    let partition = state::emulator_partition();
    let types = [ITEM_TYPE.to_string()];
    RosterReadings {
        gate_admits: state::require_administrator_at::<()>(store, REGION, now).is_ok(),
        in_member_accounts: store.member_accounts(now).contains(account),
        in_organization_accounts: accounts.contains(account),
        in_policy_scope: store.policy_scope_accounts(policy, now).contains(account),
        policy_applies: store.policy_applies_to_account(policy, account, now),
        item_associable: state::resource_set_item_failure_reason(
            &item_of(account),
            REGION,
            &partition,
            &accounts,
            &types,
        )
        .is_none(),
    }
}

/// Assert every derivation read at ONE instant agrees about `account`.
fn assert_agrees(case: &str, account: &str, expected: bool, got: &RosterReadings) {
    for (derivation, answer) in [
        ("member_accounts", got.in_member_accounts),
        ("organization_accounts", got.in_organization_accounts),
        ("policy_scope_accounts", got.in_policy_scope),
        ("policy_applies_to_account", got.policy_applies),
        ("item classification", got.item_associable),
    ] {
        assert_eq!(
            answer, expected,
            "case {case}: {derivation} must report account {account} in the organisation \
             {expected} at the instant it was given, got {answer} ({got:?})"
        );
    }
}

/// Store the two policies the readings are made against and report the
/// `PolicyId` of the one whose scope is the WHOLE organisation.
///
/// The first names [`SCOPED_ACCOUNT`] under the `ACCOUNT` key of its
/// `IncludeMap`, which is how an account other than the administrator comes to
/// be recorded as one of the organisation's; the second names no scope map at
/// all, so its scope is the roster itself.
async fn put_policies(router: &MultiServiceRouter) -> String {
    put_policy(
        router,
        "roster-of-the-organisation",
        Some(json!({ "ACCOUNT": [SCOPED_ACCOUNT] })),
        None,
    )
    .await;
    put_policy(router, "scoped-to-the-whole-organisation", None, None).await
}

/// The stored policy document `policy_id` names.
fn stored_policy(engine: &Arc<engine::InMemoryEngine>, policy_id: &str) -> Policy {
    with_store(engine, |store| {
        store
            .policies
            .get(policy_id)
            .map(|record| record.policy.clone())
            .unwrap_or_else(|| panic!("the policy {policy_id} that was just stored"))
    })
}

/// Seed the default-administrator association under test: the role it carries,
/// and the instant a requested tear-down of it completes at.
///
/// Written through the store, so that instant is an INPUT rather than something
/// a test would have to wait for.
fn seed_association(
    engine: &Arc<engine::InMemoryEngine>,
    role_status: &str,
    removal_due: Option<SystemTime>,
) {
    with_store(engine, |store| {
        store.admin_account = Some(association(role_status, removal_due));
    });
}

/// At the completion instant of a requested tear-down the administrator's own
/// account leaves the organisation's roster, and the gate, the roster, a
/// policy's scope and the per-item classification all answer that from the SAME
/// instant — on either side of the boundary and exactly on it. The accounts the
/// clock does not decide are unmoved by it.
#[tokio::test]
async fn every_roster_derivation_turns_over_at_the_instant_the_gate_turns_over() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let policy_id = put_policies(&router).await;
    let policy = stored_policy(&engine, &policy_id);

    let started = SystemTime::now();
    let due = started + state::DISASSOCIATION_SPAN;
    seed_association(&engine, state::ROLE_STATUS_PENDING_DELETION, Some(due));

    // Each row: the instant a request would decide from, and whether the
    // administrator's own account is still one of the organisation's then.
    let probes: &[(&str, SystemTime, bool)] = &[
        ("long before the completion instant", started, true),
        (
            "a nanosecond before the completion instant",
            due - Duration::from_nanos(1),
            true,
        ),
        ("exactly at the completion instant", due, false),
        (
            "a nanosecond after the completion instant",
            due + Duration::from_nanos(1),
            false,
        ),
        (
            "long after the completion instant",
            due + Duration::from_secs(3_600),
            false,
        ),
    ];

    with_store(&engine, |store| {
        for (case, probe, still_a_member) in probes {
            let got = readings(store, &policy, ACCOUNT, *probe);
            assert_eq!(
                got.gate_admits, *still_a_member,
                "case {case:?}: expected the gate to admit the call {still_a_member}, got {}",
                got.gate_admits
            );
            assert_agrees(case, ACCOUNT, *still_a_member, &got);

            // An account a stored policy's scope map names is in the roster
            // whatever the instant: tearing the administrator association down
            // moves only the administrator's own account.
            let scoped = readings(store, &policy, SCOPED_ACCOUNT, *probe);
            assert_agrees(case, SCOPED_ACCOUNT, true, &scoped);

            // And an account no record names is in it at no instant.
            let foreign = readings(store, &policy, FOREIGN_ACCOUNT, *probe);
            assert!(
                !foreign.in_member_accounts,
                "case {case:?}: account {FOREIGN_ACCOUNT} is named by no record, so no instant \
                 may put it in the roster, got {foreign:?}"
            );
            assert!(
                !foreign.item_associable,
                "case {case:?}: an item of account {FOREIGN_ACCOUNT} must not be associable, got \
                 {foreign:?}"
            );
        }
    });
}

/// Whatever role the association carries and wherever the completion instant
/// sits, every roster-derived answer read at one instant agrees with the gate's
/// decision at that same instant.
#[tokio::test]
async fn every_roster_derivation_agrees_with_the_gate_for_every_modelled_role() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let policy_id = put_policies(&router).await;
    let policy = stored_policy(&engine, &policy_id);

    let now = SystemTime::now();
    // Where the completion instant of a requested tear-down sits relative to the
    // instant the request decides from: none asked for, still ahead, already
    // passed.
    let completions: &[(&str, Option<SystemTime>)] = &[
        ("no tear-down requested", None),
        (
            "a tear-down completing later",
            Some(now + Duration::from_secs(3_600)),
        ),
        (
            "a tear-down already completed",
            Some(now - Duration::from_millis(1)),
        ),
    ];

    // Every role the model's `AccountRoleStatus` carries, so a role the issue
    // did not cite is covered too.
    for role_status in ACCOUNT_ROLE_STATUS_VALUES {
        for (completion, removal_due) in completions {
            let case = format!("{role_status} / {completion}");
            seed_association(&engine, role_status, *removal_due);
            with_store(&engine, |store| {
                let got = readings(store, &policy, ACCOUNT, now);
                assert_agrees(&case, ACCOUNT, got.gate_admits, &got);
                let scoped = readings(store, &policy, SCOPED_ACCOUNT, now);
                assert!(
                    scoped.in_member_accounts,
                    "case {case}: account {SCOPED_ACCOUNT} is named by a stored policy, so the \
                     administrator's role cannot remove it from the roster, got {scoped:?}"
                );
            });
        }
    }
}

/// One tear-down state of the association, as a request over the wire sees it.
struct AssociationCase {
    name: &'static str,
    role_status: &'static str,
    /// Where the completion instant sits relative to the instant the request
    /// decides from, in milliseconds; `None` when no tear-down was requested.
    completes_in_millis: Option<i64>,
    /// Whether the association is still an administrator — and so still an
    /// account of the organisation — at that instant.
    still_an_administrator: bool,
}

/// The class the wire rule is asserted over: the rows differ in the role the
/// association carries and in which side of the request's instant the
/// completion instant of a requested tear-down falls on.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            name: "no tear-down requested",
            role_status: state::ROLE_STATUS_READY,
            completes_in_millis: None,
            still_an_administrator: true,
        },
        AssociationCase {
            name: "a tear-down completing in an hour",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(3_600_000),
            still_an_administrator: true,
        },
        AssociationCase {
            name: "a role being deleted, completing in half a minute",
            role_status: state::ROLE_STATUS_DELETING,
            completes_in_millis: Some(30_000),
            still_an_administrator: true,
        },
        AssociationCase {
            name: "a tear-down that completed a millisecond ago",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(-1),
            still_an_administrator: false,
        },
        AssociationCase {
            name: "a tear-down that completed an hour ago",
            role_status: state::ROLE_STATUS_DELETING,
            completes_in_millis: Some(-3_600_000),
            still_an_administrator: false,
        },
        AssociationCase {
            name: "a role already deleted",
            role_status: state::ROLE_STATUS_DELETED,
            completes_in_millis: None,
            still_an_administrator: false,
        },
    ]
}

/// Whether a served response reports the administrator's own account as one of
/// the organisation's: the roster-derived part of each answer.
fn reports_the_administrators_account(operation: &str, body: &Value) -> bool {
    match operation {
        // The roster itself.
        "ListMemberAccounts" => body["MemberAccounts"]
            .as_array()
            .is_some_and(|accounts| accounts.iter().any(|account| account == ACCOUNT)),
        // An item of the administrator's own account is associable, so the batch
        // reports no failed item at all.
        "BatchAssociateResource" | "BatchDisassociateResource" => {
            body["FailedItems"].as_array().is_some_and(Vec::is_empty)
        }
        // One entry per account the policy applies to, which is the roster.
        "ListComplianceStatus" => body["PolicyComplianceStatusList"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["MemberAccount"] == ACCOUNT)),
        // The detail of ONE (policy, member account) pair: answered at all means
        // the pair resolved against the policy's scope.
        "GetComplianceDetail" => body["PolicyComplianceDetail"]["MemberAccount"] == ACCOUNT,
        // Answered at all means the account was accepted as a member of the
        // organisation ("The account must be a member of your organization").
        "ListDiscoveredResources" => body["Items"].is_array(),
        other => panic!("no roster reading is defined for {other}"),
    }
}

/// Over the wire: every operation whose answer is derived from the
/// organisation's roster either is refused with `AccessDeniedException` —
/// because the association is no longer an administrator at the instant the
/// request decides from — or is served reporting the administrator's own account
/// as one of the organisation's. Never served while claiming that account is
/// outside the organisation, which is what a second, later reading of the clock
/// inside one request produces.
#[tokio::test]
async fn a_roster_derived_answer_agrees_with_the_gate_that_admitted_the_call() {
    for case in association_cases() {
        let name = case.name;
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let whole_organisation = put_policies(&router).await;
        let (status, created) = call(
            &router,
            "PutResourceSet",
            &json!({
                "ResourceSet": {
                    "Name": "roster-classified-items",
                    "ResourceTypeList": [ITEM_TYPE],
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: PutResourceSet must answer 200, got {status} with {created}"
        );
        let resource_set = created["ResourceSet"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: PutResourceSet minted no Id: {created}"))
            .to_string();

        let now = SystemTime::now();
        let removal_due = case.completes_in_millis.map(|millis| {
            if millis >= 0 {
                now + Duration::from_millis(u64::try_from(millis).unwrap_or_default())
            } else {
                now - Duration::from_millis(u64::try_from(-millis).unwrap_or_default())
            }
        });
        seed_association(&engine, case.role_status, removal_due);

        // One row per operation whose answer the roster decides, the payloads
        // differing in form: no members, an identifier with ARNs, a policy id, a
        // policy id with a member account, an account id with a resource type.
        let calls: Vec<(&str, Value)> = vec![
            ("ListMemberAccounts", json!({})),
            (
                "BatchAssociateResource",
                json!({
                    "ResourceSetIdentifier": resource_set,
                    "Items": [item_of(ACCOUNT)],
                }),
            ),
            (
                "BatchDisassociateResource",
                json!({
                    "ResourceSetIdentifier": resource_set,
                    "Items": [item_of(ACCOUNT)],
                }),
            ),
            (
                "ListComplianceStatus",
                json!({ "PolicyId": whole_organisation }),
            ),
            (
                "GetComplianceDetail",
                json!({ "PolicyId": whole_organisation, "MemberAccount": ACCOUNT }),
            ),
            (
                "ListDiscoveredResources",
                json!({ "MemberAccountIds": [ACCOUNT], "ResourceType": ITEM_TYPE }),
            ),
        ];

        for (operation, request) in &calls {
            let (status, body) = call(&router, operation, request).await;
            if case.still_an_administrator {
                assert_eq!(
                    status, 200,
                    "case {name:?} / {operation}: an administrator still associated at the \
                     request's instant must be served, got {status} with {body}"
                );
                assert!(
                    reports_the_administrators_account(operation, &body),
                    "case {name:?} / {operation}: a served call must report account {ACCOUNT} as \
                     one of the organisation's, got {body}"
                );
                assert!(
                    !body.to_string().contains(NOT_VALID_ACCOUNT_ID),
                    "case {name:?} / {operation}: the administrator's own resource must not be \
                     refused as naming no account of the organisation, got {body}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, ACCESS_DENIED),
                    "case {name:?} / {operation}: an administrator already removed at the \
                     request's instant must be refused, got {status} with {body}"
                );
            }
        }
    }
}
