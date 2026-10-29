// SPDX-License-Identifier: CC0-1.0

//! The TWO readers of "which copy of a permission set an account holds", end to end over
//! the `awsJson1_1` wire: `ListAccountsForProvisionedPermissionSet` and
//! `ListPermissionSetsProvisionedToAccount`.
//!
//! THE RULE under test: the two operations report the SAME relation, each by the axis its
//! caller asked for — an account holds a permission set once that permission set has been
//! provisioned there (which both granting an account assignment on the account and
//! provisioning the permission set into it do), the copy in place is the one the last
//! provisioning put there, and its status is that copy's revision compared against the
//! permission set's revision now. So for every (permission set, account) pair, the account
//! is reported for the permission set under exactly the status under which the permission
//! set is reported for the account, and a pair neither holds is reported by neither.
//!
//! This is what makes the two reads one rule rather than two: both are driven here over the
//! same fixture, so a change to how a copy is decided cannot move one reader without the
//! other.
//!
//! Asserted over a TABLE whose rows differ along what the rule generalises over: how the
//! copy reached the account (an account assignment, a direct `AWS_ACCOUNT` provisioning, an
//! `ALL_PROVISIONED_ACCOUNTS` provisioning, both ways at once), the number of permission
//! sets and accounts in play (one, several), the account ids at both ends of `AccountId`'s
//! own class (`length 12..12`, `^\d{12}$`), how many principals a permission set was
//! assigned to on one account, whether a change to the permission set left the copy behind,
//! whether a later provisioning brought it back up to date, and whether the assignment was
//! withdrawn or the permission set deleted afterwards.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns, the calls run
//! against an ORGANIZATION instance because the permission-set surface is withdrawn on an
//! account instance, and a direct provisioning's completion instant is moved through the
//! clock helper `permission_set_provisioning.rs` owns, so nothing sleeps.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::permission_set_provisioning::hold_provisionings;
use crate::ssoadmin_permission_sets::{assert_refused, call, organization_engine, router_over};

/// `ProvisioningStatus` as the model declares its two values
/// (`LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`).
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// `ProvisionTargetType`, as the model declares its two values
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// Accounts spanning `AccountId`'s own class (`length 12..12`, `^\d{12}$`): both ends of
/// the 12-digit range and two from inside it. Every row reads EVERY one of them, so a copy
/// reported for an account it does not belong to fails the row.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_OTHER: &str = "555555555555";
const ACCOUNT_HIGH: &str = "999999999999";
const EVERY_ACCOUNT: [&str; 4] = [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_OTHER, ACCOUNT_HIGH];

/// Principals the assignments are granted to; `PrincipalId` takes an identity-store user or
/// group id, which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL_A: &str = "11111111-2222-3333-4444-555555555555";
const PRINCIPAL_B: &str = "66666666-7777-8888-9999-000000000000";

/// One (permission set slot, account id, provisioning status) the relation holds.
type HeldCopy = (usize, String, &'static str);

/// One thing a row does to the fixture, in the order the row lists them.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Grant an account assignment of a permission set on an account, which provisions
    /// that permission set into it.
    Assign(usize, &'static str, &'static str),
    /// Withdraw one such assignment again.
    Unassign(usize, &'static str, &'static str),
    /// Provision a permission set into ONE account, whatever assignments that account
    /// holds.
    ProvisionAccount(usize, &'static str),
    /// Provision a permission set into every account that already holds it.
    ProvisionAllProvisioned(usize),
    /// Change the permission set's definition, which leaves every copy of it behind.
    Change(usize),
    /// Delete the permission set, which takes the copies it was provisioned as with it.
    Delete(usize),
}

/// One row of the table: the fixture to build, and the copies the relation then holds.
struct Row {
    label: &'static str,
    /// How many permission sets the row creates, so a row can leave some provisioned
    /// nowhere.
    permission_sets: usize,
    steps: &'static [Step],
    /// The permission sets the row deleted: the accounts-side read of one of those is the
    /// operation's own not-found refusal rather than a page.
    deleted: &'static [usize],
    /// (permission set slot, account id, status) — every copy BOTH readers must report.
    expected: &'static [(usize, &'static str, &'static str)],
}

/// An instant far enough behind that every provisioning the store holds has completed.
fn completed() -> std::time::SystemTime {
    std::time::SystemTime::now() - std::time::Duration::from_secs(60)
}

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// Create a permission set over the wire and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {body}"))
        .to_string()
}

/// Apply one step of a row over the wire, asserting the call was accepted.
///
/// `change` counts the definition changes made so far, so each one carries a description no
/// earlier definition of that permission set had and therefore really is a change.
async fn apply(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    arns: &[String],
    step: Step,
    change: &mut usize,
    case: &str,
) {
    let (operation, request) = match step {
        Step::Assign(slot, account_id, principal_id) => (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": account_id,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "PermissionSetArn": arns[slot],
                "PrincipalType": "USER",
                "PrincipalId": principal_id,
            }),
        ),
        Step::Unassign(slot, account_id, principal_id) => (
            "DeleteAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": account_id,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "PermissionSetArn": arns[slot],
                "PrincipalType": "USER",
                "PrincipalId": principal_id,
            }),
        ),
        Step::ProvisionAccount(slot, account_id) => (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": arns[slot],
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "TargetId": account_id,
            }),
        ),
        Step::ProvisionAllProvisioned(slot) => (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": arns[slot],
                "TargetType": TARGET_TYPE_ALL_PROVISIONED,
            }),
        ),
        Step::Change(slot) => {
            *change += 1;
            (
                "UpdatePermissionSet",
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": arns[slot],
                    "Description": format!("Changed {change}"),
                }),
            )
        }
        Step::Delete(slot) => (
            "DeletePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": arns[slot]}),
        ),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} for {step:?} expected 200, got {status} {body}"
    );
    // A provisioning is asynchronous: its copy reaches the target accounts one span after
    // the call. Move that instant behind us so the step is complete before the next one,
    // exactly as a caller that waited the span out would see it.
    if matches!(
        step,
        Step::ProvisionAccount(..) | Step::ProvisionAllProvisioned(..)
    ) {
        hold_provisionings(engine, completed());
    }
}

/// The accounts the permission set is provisioned into under `provisioning_status`, sorted.
async fn accounts_for(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    provisioning_status: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    if let Some(provisioning_status) = provisioning_status {
        request["ProvisioningStatus"] = json!(provisioning_status);
    }
    let (status, body) = call(router, "ListAccountsForProvisionedPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccountsForProvisionedPermissionSet \
         ({provisioning_status:?}) expected 200, got {status} {body}"
    );
    let mut accounts: Vec<String> = body["AccountIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: AccountIds must be a list, got {body}"))
        .iter()
        .map(|account| {
            account
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"))
                .to_string()
        })
        .collect();
    accounts.sort();
    accounts
}

/// The permission sets provisioned into one account under `provisioning_status`, sorted.
async fn permission_sets_for(
    router: &MultiServiceRouter,
    instance_arn: &str,
    account_id: &str,
    provisioning_status: Option<&str>,
    case: &str,
) -> Vec<String> {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "AccountId": account_id,
    });
    if let Some(provisioning_status) = provisioning_status {
        request["ProvisioningStatus"] = json!(provisioning_status);
    }
    let (status, body) = call(router, "ListPermissionSetsProvisionedToAccount", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPermissionSetsProvisionedToAccount \
         ({provisioning_status:?}) expected 200, got {status} {body}"
    );
    let mut sets: Vec<String> = body["PermissionSets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: PermissionSets must be a list, got {body}"))
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"))
                .to_string()
        })
        .collect();
    sets.sort();
    sets
}

/// Every copy the ACCOUNTS-side reader reports, read one live permission set at a time
/// under each status the model declares, and checked to partition that reader's own
/// unfiltered page.
async fn copies_from_accounts_side(
    router: &MultiServiceRouter,
    instance_arn: &str,
    live: &[(usize, String)],
    case: &str,
) -> Vec<HeldCopy> {
    let mut held: Vec<HeldCopy> = Vec::new();
    for (slot, permission_set_arn) in live {
        let mut filtered: Vec<String> = Vec::new();
        for status in [LATEST_PROVISIONED, LATEST_NOT_PROVISIONED] {
            for account_id in
                accounts_for(router, instance_arn, permission_set_arn, Some(status), case).await
            {
                filtered.push(account_id.clone());
                held.push((*slot, account_id, status));
            }
        }
        filtered.sort();
        let unfiltered = accounts_for(router, instance_arn, permission_set_arn, None, case).await;
        assert_eq!(
            filtered, unfiltered,
            "case {case:?}: the two statuses must partition the accounts reported for \
             permission set {slot}: expected {unfiltered:?}, got {filtered:?}"
        );
    }
    held.sort();
    held
}

/// Every copy the PERMISSION-SETS-side reader reports, read one account at a time under
/// each status the model declares, and checked to partition that reader's own unfiltered
/// page.
async fn copies_from_permission_sets_side(
    router: &MultiServiceRouter,
    instance_arn: &str,
    arns: &[String],
    case: &str,
) -> Vec<HeldCopy> {
    let mut held: Vec<HeldCopy> = Vec::new();
    for account_id in EVERY_ACCOUNT {
        let mut filtered: Vec<String> = Vec::new();
        for status in [LATEST_PROVISIONED, LATEST_NOT_PROVISIONED] {
            for permission_set_arn in
                permission_sets_for(router, instance_arn, account_id, Some(status), case).await
            {
                let slot = arns
                    .iter()
                    .position(|arn| *arn == permission_set_arn)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: account {account_id} was reported an unknown \
                             permission set {permission_set_arn:?}"
                        )
                    });
                filtered.push(permission_set_arn);
                held.push((slot, account_id.to_string(), status));
            }
        }
        filtered.sort();
        let unfiltered = permission_sets_for(router, instance_arn, account_id, None, case).await;
        assert_eq!(
            filtered, unfiltered,
            "case {case:?}: the two statuses must partition the permission sets reported \
             for account {account_id}: expected {unfiltered:?}, got {filtered:?}"
        );
    }
    held.sort();
    held
}

/// The table: every way a copy can reach an account, and every way it can be left behind,
/// deprovisioned or removed again.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "nothing provisioned anywhere",
            permission_sets: 2,
            steps: &[],
            deleted: &[],
            expected: &[],
        },
        Row {
            label: "one assignment, one principal",
            permission_sets: 2,
            steps: &[Step::Assign(0, ACCOUNT_MID, PRINCIPAL_A)],
            deleted: &[],
            expected: &[(0, ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "one permission set assigned to two principals on one account",
            permission_sets: 1,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_B),
            ],
            deleted: &[],
            expected: &[(0, ACCOUNT_LOW, LATEST_PROVISIONED)],
        },
        Row {
            label: "a direct AWS_ACCOUNT provisioning into an account holding no assignment",
            permission_sets: 2,
            steps: &[Step::ProvisionAccount(1, ACCOUNT_HIGH)],
            deleted: &[],
            expected: &[(1, ACCOUNT_HIGH, LATEST_PROVISIONED)],
        },
        Row {
            label: "an assignment and a direct provisioning on the same pair",
            permission_sets: 1,
            steps: &[
                Step::Assign(0, ACCOUNT_OTHER, PRINCIPAL_A),
                Step::ProvisionAccount(0, ACCOUNT_OTHER),
            ],
            deleted: &[],
            expected: &[(0, ACCOUNT_OTHER, LATEST_PROVISIONED)],
        },
        Row {
            label: "several permission sets across the ends of the AccountId class",
            permission_sets: 3,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(1, ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(1, ACCOUNT_HIGH, PRINCIPAL_B),
                Step::ProvisionAccount(2, ACCOUNT_MID),
            ],
            deleted: &[],
            expected: &[
                (0, ACCOUNT_LOW, LATEST_PROVISIONED),
                (1, ACCOUNT_LOW, LATEST_PROVISIONED),
                (1, ACCOUNT_HIGH, LATEST_PROVISIONED),
                (2, ACCOUNT_MID, LATEST_PROVISIONED),
            ],
        },
        Row {
            label: "a change leaves every copy of the changed permission set behind",
            permission_sets: 2,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::ProvisionAccount(0, ACCOUNT_HIGH),
                Step::Assign(1, ACCOUNT_LOW, PRINCIPAL_A),
                Step::Change(0),
            ],
            deleted: &[],
            expected: &[
                (0, ACCOUNT_LOW, LATEST_NOT_PROVISIONED),
                (0, ACCOUNT_HIGH, LATEST_NOT_PROVISIONED),
                (1, ACCOUNT_LOW, LATEST_PROVISIONED),
            ],
        },
        Row {
            label: "a provisioning after the change brings one of the two copies back up to date",
            permission_sets: 1,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(0, ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Change(0),
                Step::ProvisionAccount(0, ACCOUNT_HIGH),
            ],
            deleted: &[],
            expected: &[
                (0, ACCOUNT_LOW, LATEST_NOT_PROVISIONED),
                (0, ACCOUNT_HIGH, LATEST_PROVISIONED),
            ],
        },
        Row {
            label: "an ALL_PROVISIONED_ACCOUNTS provisioning refreshes every holder",
            permission_sets: 2,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::ProvisionAccount(0, ACCOUNT_MID),
                Step::Assign(1, ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Change(0),
                Step::Change(1),
                Step::ProvisionAllProvisioned(0),
            ],
            deleted: &[],
            expected: &[
                (0, ACCOUNT_LOW, LATEST_PROVISIONED),
                (0, ACCOUNT_MID, LATEST_PROVISIONED),
                (1, ACCOUNT_HIGH, LATEST_NOT_PROVISIONED),
            ],
        },
        Row {
            label: "the last assignment withdrawn deprovisions the account",
            permission_sets: 2,
            steps: &[
                Step::Assign(0, ACCOUNT_MID, PRINCIPAL_A),
                Step::Assign(0, ACCOUNT_MID, PRINCIPAL_B),
                Step::Assign(1, ACCOUNT_MID, PRINCIPAL_A),
                Step::Unassign(0, ACCOUNT_MID, PRINCIPAL_A),
                Step::Unassign(1, ACCOUNT_MID, PRINCIPAL_A),
            ],
            deleted: &[],
            expected: &[(0, ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "a deleted permission set is provisioned nowhere",
            permission_sets: 2,
            steps: &[
                Step::Assign(0, ACCOUNT_LOW, PRINCIPAL_A),
                Step::ProvisionAccount(1, ACCOUNT_LOW),
                Step::Assign(1, ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Delete(1),
            ],
            deleted: &[1],
            expected: &[(0, ACCOUNT_LOW, LATEST_PROVISIONED)],
        },
    ]
}

/// THE RULE: both readers project ONE relation, so every (permission set, account) pair is
/// reported by both under the same status, or by neither.
#[tokio::test]
async fn both_readers_report_the_same_held_copies() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.label;
        let (router, instance_arn, engine) = organization_fixture();
        let mut arns: Vec<String> = Vec::new();
        for slot in 0..row.permission_sets {
            arns.push(
                create_permission_set(&router, &instance_arn, &format!("Row{index}Set{slot}"))
                    .await,
            );
        }

        let mut change = 0usize;
        for step in row.steps {
            apply(
                &router,
                &engine,
                &instance_arn,
                &arns,
                *step,
                &mut change,
                case,
            )
            .await;
        }

        let live: Vec<(usize, String)> = arns
            .iter()
            .enumerate()
            .filter(|(slot, _)| !row.deleted.contains(slot))
            .map(|(slot, arn)| (slot, arn.clone()))
            .collect();
        let mut expected: Vec<HeldCopy> = row
            .expected
            .iter()
            .map(|(slot, account_id, status)| (*slot, (*account_id).to_string(), *status))
            .collect();
        expected.sort();

        let from_accounts = copies_from_accounts_side(&router, &instance_arn, &live, case).await;
        let from_permission_sets =
            copies_from_permission_sets_side(&router, &instance_arn, &arns, case).await;
        assert_eq!(
            from_accounts, from_permission_sets,
            "case {case:?}: the two readers must report the same copies, got \
             {from_accounts:?} from the accounts side and {from_permission_sets:?} from the \
             permission-sets side"
        );
        assert_eq!(
            from_accounts, expected,
            "case {case:?}: expected the copies {expected:?}, got {from_accounts:?}"
        );

        // A permission set the row deleted is a permission set the instance no longer
        // holds, so the accounts-side read of it is that operation's own not-found refusal
        // while the permission-sets side simply leaves it out — asserted above by the pair
        // sets agreeing without it.
        for slot in row.deleted {
            let (status, body) = call(
                &router,
                "ListAccountsForProvisionedPermissionSet",
                &json!({"InstanceArn": instance_arn, "PermissionSetArn": arns[*slot]}),
            )
            .await;
            assert_refused(
                case,
                &format!("the accounts-side read of the deleted permission set {slot}"),
                "ResourceNotFoundException",
                status,
                &body,
            );
        }
    }
}
