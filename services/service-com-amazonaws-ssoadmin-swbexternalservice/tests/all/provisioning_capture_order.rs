// SPDX-License-Identifier: CC0-1.0

//! WHICH copy of a permission set an account ends up holding when several provisionings of
//! it overlap, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: a provisioning copies the definition it captured when it was
//! ACCEPTED, so the copy an account holds is the one captured LAST — whichever kind of
//! provisioning captured it (granting an account assignment provisions the permission set
//! into its account at the instant the grant is accepted, a `ProvisionPermissionSet` call
//! provisions when its workflow completes) and whatever order the copies land in. A
//! provisioning that captured an earlier definition therefore never rewinds a copy captured
//! after it, however much later it lands, while a provisioning that captured the latest
//! definition does bring the accounts it targeted up to date.
//!
//! Both readers of that relation are driven on every row —
//! `ListAccountsForProvisionedPermissionSet` and `ListPermissionSetsProvisionedToAccount` —
//! so neither can report a copy the other does not.
//!
//! Asserted over a TABLE whose rows differ along what the rule generalises over: which kind
//! of provisioning captured last (a grant on an account that already held an assignment, a
//! grant on one that held none, another `ProvisionPermissionSet` call), the in-flight
//! provisioning's target type (`AWS_ACCOUNT`, `ALL_PROVISIONED_ACCOUNTS`), the ORDER the
//! overlapping provisionings land in (acceptance order and the reverse), the number of
//! accounts and principals in play, the account ids at both ends of `AccountId`'s own class
//! (`length 12..12`, `^\d{12}$`), whether the later grant is withdrawn afterwards, and the
//! rows where nothing followed the in-flight provisioning, where its copy is the one that
//! lands.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns and the calls
//! run against an ORGANIZATION instance, because the permission-set surface is withdrawn on
//! an account instance. Only the stored COMPLETION INSTANT of a provisioning is moved
//! directly — the clock's business, not the wire's — so nothing sleeps and a row can decide
//! which of its overlapping workflows completes first; the copy itself is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::permission_set_provisioning::{
    completed, create_permission_set, hold_unapplied_provisionings, in_flight,
    organization_fixture,
};
use crate::ssoadmin_permission_sets::{call, with_store};

/// `ProvisioningStatus` as the model declares its two values
/// (`LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`).
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// `ProvisionTargetType`, as the model declares its two values
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// Accounts spanning `AccountId`'s own class (`length 12..12`, `^\d{12}$`): both ends of the
/// 12-digit range and two from inside it. Every row reads EVERY one of them, so a copy
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

/// One (account id, provisioning status) pair the relation holds.
type HeldCopy = (String, &'static str);

/// One thing a row does to the fixture, in the order the row lists them.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Grant an account assignment on an account, which PROVISIONS the permission set into
    /// it with the definition current at this instant.
    Assign(&'static str, &'static str),
    /// Withdraw one such assignment again.
    Unassign(&'static str, &'static str),
    /// Accept a provisioning of ONE account. It stays IN FLIGHT until a [`Step::Land`] names
    /// it, so a definition change and a grant can follow it while it is still running.
    ProvisionAccount(&'static str),
    /// Accept a provisioning of every account that already holds the permission set, which
    /// stays in flight the same way.
    ProvisionAllProvisioned,
    /// Change the permission set's definition, which leaves every copy captured before it
    /// behind.
    Change,
    /// Complete the provisioning this row accepted at this position (counting its
    /// provisionings from zero), leaving every other one still in flight.
    Land(usize),
}

/// One row of the table: the steps to run, and the copies the relation then holds.
struct Row {
    label: &'static str,
    steps: &'static [Step],
    /// (account id, status) — every copy BOTH readers must report once the steps have run;
    /// an account not named here holds no copy at all.
    expected: &'static [(&'static str, &'static str)],
}

/// Make the `index`-th provisioning the row accepted DUE, leaving every other one in flight.
///
/// Only the stored instant moves; the copy is applied by the service itself on the next
/// request, which the read below performs, so the landing happens exactly as it does for a
/// caller that waited the span out.
async fn land(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    permission_set_arn: &str,
    index: usize,
    case: &str,
) {
    with_store(engine, |store| {
        let accepted = store.permission_set_provisionings.len();
        let record = store
            .permission_set_provisionings
            .get_mut(index)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the row accepted {accepted} provisionings, not {index}")
            });
        record.succeeded_due = completed();
    });
    // One request, so the service applies what the clock now owes before the next step.
    accounts_for(router, instance_arn, permission_set_arn, None, case).await;
}

/// Apply one step of a row over the wire, asserting the call was accepted.
///
/// `change` counts the definition changes made so far, so each one carries a description no
/// earlier definition of the permission set had and therefore really is a change.
async fn apply(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    permission_set_arn: &str,
    step: Step,
    change: &mut usize,
    case: &str,
) {
    if let Step::Land(index) = step {
        land(
            router,
            engine,
            instance_arn,
            permission_set_arn,
            index,
            case,
        )
        .await;
        return;
    }
    let (operation, request) = match step {
        Step::Assign(account_id, principal_id) => (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": account_id,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "PermissionSetArn": permission_set_arn,
                "PrincipalType": "USER",
                "PrincipalId": principal_id,
            }),
        ),
        Step::Unassign(account_id, principal_id) => (
            "DeleteAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": account_id,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "PermissionSetArn": permission_set_arn,
                "PrincipalType": "USER",
                "PrincipalId": principal_id,
            }),
        ),
        Step::ProvisionAccount(account_id) => (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "TargetId": account_id,
            }),
        ),
        Step::ProvisionAllProvisioned => (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "TargetType": TARGET_TYPE_ALL_PROVISIONED,
            }),
        ),
        Step::Change => {
            *change += 1;
            (
                "UpdatePermissionSet",
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "Description": format!("Changed {change}"),
                }),
            )
        }
        Step::Land(_) => unreachable!("a Land step is applied above"),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} for {step:?} expected 200, got {status} {body}"
    );
    // Every workflow this row has accepted and not landed yet stays in flight, so the row
    // decides which of them completes and when.
    hold_unapplied_provisionings(engine, in_flight());
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

/// Every copy the ACCOUNTS-side reader reports for the permission set, under each status the
/// model declares.
async fn copies_from_accounts_side(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<HeldCopy> {
    let mut held: Vec<HeldCopy> = Vec::new();
    for status in [LATEST_PROVISIONED, LATEST_NOT_PROVISIONED] {
        for account_id in
            accounts_for(router, instance_arn, permission_set_arn, Some(status), case).await
        {
            held.push((account_id, status));
        }
    }
    held.sort();
    held
}

/// Every copy the PERMISSION-SETS-side reader reports for the same permission set, read one
/// account at a time under each status the model declares.
async fn copies_from_permission_sets_side(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<HeldCopy> {
    let mut held: Vec<HeldCopy> = Vec::new();
    for account_id in EVERY_ACCOUNT {
        for status in [LATEST_PROVISIONED, LATEST_NOT_PROVISIONED] {
            let request = json!({
                "InstanceArn": instance_arn,
                "AccountId": account_id,
                "ProvisioningStatus": status,
            });
            let (code, body) =
                call(router, "ListPermissionSetsProvisionedToAccount", &request).await;
            assert_eq!(
                code, 200,
                "case {case:?}: ListPermissionSetsProvisionedToAccount ({status}) for \
                 {account_id} expected 200, got {code} {body}"
            );
            let reported = body["PermissionSets"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: PermissionSets must be a list: {body}"));
            for arn in reported {
                let arn = arn
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"));
                assert_eq!(
                    arn, permission_set_arn,
                    "case {case:?}: account {account_id} was reported an unknown permission \
                     set {arn:?}"
                );
                held.push((account_id.to_string(), status));
            }
        }
    }
    held.sort();
    held
}

/// The table: every way a provisioning accepted later can overlap one still in flight, and
/// the rows where nothing followed the in-flight one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a grant while an ALL_PROVISIONED_ACCOUNTS provisioning is in flight \
                    keeps its copy",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::ProvisionAllProvisioned,
                Step::Change,
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "a grant while an AWS_ACCOUNT provisioning of the same account is in \
                    flight keeps its copy",
            steps: &[
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_A),
                Step::ProvisionAccount(ACCOUNT_LOW),
                Step::Change,
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_B),
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_LOW, LATEST_PROVISIONED)],
        },
        Row {
            label: "a grant on an account that held no assignment keeps its copy against an \
                    in-flight provisioning of that account",
            steps: &[
                Step::ProvisionAccount(ACCOUNT_OTHER),
                Step::Change,
                Step::Assign(ACCOUNT_OTHER, PRINCIPAL_A),
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_OTHER, LATEST_PROVISIONED)],
        },
        Row {
            label: "two overlapping provisionings landing in acceptance order",
            steps: &[
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::ProvisionAccount(ACCOUNT_HIGH),
                Step::Change,
                Step::ProvisionAccount(ACCOUNT_HIGH),
                Step::Land(0),
                Step::Land(1),
            ],
            expected: &[(ACCOUNT_HIGH, LATEST_PROVISIONED)],
        },
        Row {
            label: "two overlapping provisionings landing in the REVERSE order",
            steps: &[
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::ProvisionAccount(ACCOUNT_HIGH),
                Step::Change,
                Step::ProvisionAccount(ACCOUNT_HIGH),
                Step::Land(1),
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_HIGH, LATEST_PROVISIONED)],
        },
        Row {
            label: "two overlapping provisionings of an account holding no assignment, \
                    landing in the REVERSE order",
            steps: &[
                Step::ProvisionAccount(ACCOUNT_LOW),
                Step::Change,
                Step::ProvisionAccount(ACCOUNT_LOW),
                Step::Land(1),
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_LOW, LATEST_PROVISIONED)],
        },
        Row {
            label: "only the account a later grant reached keeps the latest definition",
            steps: &[
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::ProvisionAllProvisioned,
                Step::Change,
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_B),
                Step::Land(0),
            ],
            expected: &[
                (ACCOUNT_LOW, LATEST_PROVISIONED),
                (ACCOUNT_HIGH, LATEST_NOT_PROVISIONED),
            ],
        },
        Row {
            label: "the account keeps the later copy after the grant that captured it is \
                    withdrawn",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::ProvisionAccount(ACCOUNT_MID),
                Step::Change,
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
                Step::Land(0),
                Step::Unassign(ACCOUNT_MID, PRINCIPAL_B),
            ],
            expected: &[(ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "nothing followed the in-flight provisioning, so the definition it \
                    captured is the one that lands",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::ProvisionAccount(ACCOUNT_MID),
                Step::Change,
                Step::Land(0),
            ],
            expected: &[(ACCOUNT_MID, LATEST_NOT_PROVISIONED)],
        },
        Row {
            label: "a provisioning accepted after the change brings the account it targeted \
                    up to date",
            steps: &[
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(ACCOUNT_OTHER, PRINCIPAL_A),
                Step::Change,
                Step::ProvisionAccount(ACCOUNT_LOW),
                Step::Land(0),
            ],
            expected: &[
                (ACCOUNT_LOW, LATEST_PROVISIONED),
                (ACCOUNT_OTHER, LATEST_NOT_PROVISIONED),
            ],
        },
    ]
}

/// THE RULE: the copy an account holds is the one captured LAST, whatever order the
/// provisionings carrying the copies land in.
#[tokio::test]
async fn the_copy_in_place_is_the_one_captured_last() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.label;
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Overlap{index}")).await;

        let mut change = 0usize;
        for step in row.steps {
            apply(
                &router,
                &engine,
                &instance_arn,
                &permission_set_arn,
                *step,
                &mut change,
                case,
            )
            .await;
        }

        let mut expected: Vec<HeldCopy> = row
            .expected
            .iter()
            .map(|(account_id, status)| ((*account_id).to_string(), *status))
            .collect();
        expected.sort();

        let from_accounts =
            copies_from_accounts_side(&router, &instance_arn, &permission_set_arn, case).await;
        let from_permission_sets =
            copies_from_permission_sets_side(&router, &instance_arn, &permission_set_arn, case)
                .await;
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
    }
}

/// The same rule read at EVERY step of one overlap: the grant made while the provisioning
/// was in flight is reported as holding the latest definition straight away, and goes on
/// being reported that way once the older workflow completes — no read between the two
/// instants disagrees.
#[tokio::test]
async fn the_landing_of_an_older_workflow_changes_no_read() {
    let case = "a grant made while an older provisioning was in flight";
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
    let mut change = 0usize;

    // The account holds the permission set at its first definition.
    for step in [
        Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
        // Accepted here, so the definition this workflow carries is the one above.
        Step::ProvisionAllProvisioned,
        Step::Change,
    ] {
        apply(
            &router,
            &engine,
            &instance_arn,
            &permission_set_arn,
            step,
            &mut change,
            case,
        )
        .await;
    }
    let behind = accounts_for(
        &router,
        &instance_arn,
        &permission_set_arn,
        Some(LATEST_NOT_PROVISIONED),
        case,
    )
    .await;
    assert_eq!(
        behind,
        vec![ACCOUNT_MID.to_string()],
        "case {case:?}: the change leaves the account behind, got {behind:?}"
    );

    // The grant made while that workflow is still in flight provisions the CURRENT
    // definition, so the account holds the latest one from this instant on.
    apply(
        &router,
        &engine,
        &instance_arn,
        &permission_set_arn,
        Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
        &mut change,
        case,
    )
    .await;
    for (label, status, expected) in [
        ("after the grant", LATEST_PROVISIONED, vec![ACCOUNT_MID]),
        ("after the grant", LATEST_NOT_PROVISIONED, vec![]),
    ] {
        let listed = accounts_for(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(status),
            case,
        )
        .await;
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: {label} under {status}, expected {expected:?}, got {listed:?}"
        );
    }

    // The older workflow completing copies the definition IT captured, which is not the one
    // the account holds, so it changes nothing a reader sees.
    apply(
        &router,
        &engine,
        &instance_arn,
        &permission_set_arn,
        Step::Land(0),
        &mut change,
        case,
    )
    .await;
    for (label, status, expected) in [
        ("after the landing", LATEST_PROVISIONED, vec![ACCOUNT_MID]),
        ("after the landing", LATEST_NOT_PROVISIONED, vec![]),
    ] {
        let listed = accounts_for(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(status),
            case,
        )
        .await;
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: {label} under {status}, expected {expected:?}, got {listed:?}"
        );
    }
}
