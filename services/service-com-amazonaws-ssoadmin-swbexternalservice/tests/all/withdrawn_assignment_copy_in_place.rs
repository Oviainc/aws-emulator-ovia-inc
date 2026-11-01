// SPDX-License-Identifier: CC0-1.0

//! WHICH copy of a permission set an account still holds once one of the assignments
//! granted there is WITHDRAWN again, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: withdrawing an account assignment delivers no copy of the
//! permission set, so it cannot move the definition the account is reported to hold — the
//! account goes on reporting the copy captured LAST of those delivered to it for as long as
//! anything still holds the permission set there (another assignment granted on it, or a
//! provisioning that targeted it), and it is reported as holding nothing only once no record
//! of that permission set is left in the account at all. Withdrawing an assignment therefore
//! never rewinds an account to a definition the permission set has moved past, and never
//! brings one up to date either.
//!
//! Both readers of that relation are driven on every row —
//! `ListAccountsForProvisionedPermissionSet` and `ListPermissionSetsProvisionedToAccount` —
//! so neither can report a copy the other does not.
//!
//! Asserted over a TABLE whose rows differ along what the rule generalises over: how many
//! assignments the account holds (one, two, three) and how many are withdrawn, WHICH of them
//! is withdrawn (the one that captured last, an earlier one, all of them), whether a
//! `ProvisionPermissionSet` call ever targeted the account, whether the permission set's
//! definition changed between the grants and after the withdrawal, the number of accounts in
//! play, and account ids at both ends of `AccountId`'s own class (`length 12..12`,
//! `^\d{12}$`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns and the calls run
//! against an ORGANIZATION instance, because the permission-set surface is withdrawn on an
//! account instance. Only the stored COMPLETION INSTANT of a provisioning is moved directly —
//! the clock's business, not the wire's — so nothing sleeps; the copy itself is still applied
//! by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::permission_set_provisioning::{
    completed, create_permission_set, hold_provisionings, organization_fixture,
};
use crate::ssoadmin_permission_sets::call;

/// `ProvisioningStatus` as the model declares its two values
/// (`LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`).
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// `ProvisionTargetType::AWS_ACCOUNT`, the target type that names ONE account.
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";

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
const PRINCIPAL_C: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// One (account id, provisioning status) pair the relation holds.
type HeldCopy = (String, &'static str);

/// One thing a row does to the fixture, in the order the row lists them.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Grant an account assignment on an account, which PROVISIONS the permission set into
    /// it with the definition current at this instant.
    Assign(&'static str, &'static str),
    /// Withdraw one such assignment again, which delivers no copy.
    Unassign(&'static str, &'static str),
    /// Provision the permission set into ONE account and let the workflow complete, so the
    /// account holds the copy that call delivered whether or not an assignment reaches it.
    Provision(&'static str),
    /// Change the permission set's definition, which leaves every copy captured before it
    /// behind.
    Change,
}

/// One row of the table: the steps to run, and the copies the relation then holds.
struct Row {
    label: &'static str,
    steps: &'static [Step],
    /// (account id, status) — every copy BOTH readers must report once the steps have run;
    /// an account not named here holds no copy at all.
    expected: &'static [(&'static str, &'static str)],
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
    let (operation, request) = match step {
        Step::Assign(account_id, principal_id) => (
            "CreateAccountAssignment",
            json!({
                "InstanceArn": instance_arn,
                "TargetId": account_id,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "PermissionSetArn": permission_set_arn,
                "PrincipalType": "GROUP",
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
                "PrincipalType": "GROUP",
                "PrincipalId": principal_id,
            }),
        ),
        Step::Provision(account_id) => (
            "ProvisionPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": permission_set_arn,
                "TargetType": TARGET_TYPE_ONE_ACCOUNT,
                "TargetId": account_id,
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
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} for {step:?} expected 200, got {status} {body}"
    );
    if matches!(step, Step::Provision(_)) {
        // The workflow this call accepted is now due, so the copy it carries reaches its
        // target account before the next step — exactly as it does for a caller that waited
        // the provisioning span out.
        hold_provisionings(engine, completed());
        accounts_for(router, instance_arn, permission_set_arn, None, case).await;
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

/// The table: every way a withdrawal can meet the records of an account's copy.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the grant that captured last is withdrawn, the account holds no other \
                    copy record",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_MID, PRINCIPAL_B),
            ],
            expected: &[(ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "an EARLIER grant is withdrawn, so the copy captured last stands",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_MID, PRINCIPAL_A),
            ],
            expected: &[(ACCOUNT_MID, LATEST_PROVISIONED)],
        },
        Row {
            label: "three grants at three definitions, the two newest withdrawn in turn",
            steps: &[
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_B),
                Step::Change,
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_C),
                Step::Unassign(ACCOUNT_LOW, PRINCIPAL_C),
                Step::Unassign(ACCOUNT_LOW, PRINCIPAL_B),
            ],
            expected: &[(ACCOUNT_LOW, LATEST_PROVISIONED)],
        },
        Row {
            label: "a provisioning targeted the account before the grants, and the newer \
                    grant is withdrawn",
            steps: &[
                Step::Provision(ACCOUNT_HIGH),
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_HIGH, PRINCIPAL_B),
            ],
            expected: &[(ACCOUNT_HIGH, LATEST_PROVISIONED)],
        },
        Row {
            label: "every grant is withdrawn on an account no provisioning targeted, which \
                    deprovisions it",
            steps: &[
                Step::Assign(ACCOUNT_OTHER, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_OTHER, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_OTHER, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_OTHER, PRINCIPAL_A),
            ],
            expected: &[],
        },
        Row {
            label: "every grant is withdrawn on an account a provisioning targeted, which \
                    keeps the copy delivered last",
            steps: &[
                Step::Provision(ACCOUNT_OTHER),
                Step::Change,
                Step::Assign(ACCOUNT_OTHER, PRINCIPAL_A),
                Step::Unassign(ACCOUNT_OTHER, PRINCIPAL_A),
            ],
            expected: &[(ACCOUNT_OTHER, LATEST_PROVISIONED)],
        },
        Row {
            label: "a withdrawal on one account leaves another account's copy alone",
            steps: &[
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_A),
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_LOW, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_LOW, PRINCIPAL_B),
            ],
            expected: &[
                (ACCOUNT_LOW, LATEST_PROVISIONED),
                (ACCOUNT_HIGH, LATEST_NOT_PROVISIONED),
            ],
        },
        Row {
            label: "a withdrawal brings no account up to date: both grants captured before \
                    the change",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
                Step::Change,
                Step::Unassign(ACCOUNT_MID, PRINCIPAL_A),
            ],
            expected: &[(ACCOUNT_MID, LATEST_NOT_PROVISIONED)],
        },
        Row {
            label: "a change AFTER the withdrawal leaves the copy the account kept behind",
            steps: &[
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_HIGH, PRINCIPAL_B),
                Step::Unassign(ACCOUNT_HIGH, PRINCIPAL_B),
                Step::Change,
            ],
            expected: &[(ACCOUNT_HIGH, LATEST_NOT_PROVISIONED)],
        },
        Row {
            label: "nothing is withdrawn, so the grant that captured last is the copy in \
                    place",
            steps: &[
                Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
                Step::Change,
                Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
            ],
            expected: &[(ACCOUNT_MID, LATEST_PROVISIONED)],
        },
    ]
}

/// THE RULE: withdrawing an assignment delivers no copy, so the account goes on reporting
/// the copy captured LAST of those still recorded there, and holds nothing only once no
/// record of the permission set is left in it.
#[tokio::test]
async fn a_withdrawal_never_moves_the_copy_an_account_is_reported_to_hold() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.label;
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Withdrawal{index}")).await;

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

/// The same rule read on BOTH sides of the withdrawal: the account reported as holding the
/// latest definition before one of its assignments is withdrawn is reported exactly the same
/// way afterwards, under each `ProvisioningStatus` filter and with no filter at all.
#[tokio::test]
async fn the_status_an_account_is_reported_under_is_unchanged_by_the_withdrawal() {
    let case = "two grants at two definitions, the newer one withdrawn";
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
    let mut change = 0usize;

    for step in [
        Step::Assign(ACCOUNT_MID, PRINCIPAL_A),
        Step::Change,
        Step::Assign(ACCOUNT_MID, PRINCIPAL_B),
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

    // What every filter answers while both assignments are granted.
    let expectations: [(Option<&str>, Vec<&str>); 3] = [
        (None, vec![ACCOUNT_MID]),
        (Some(LATEST_PROVISIONED), vec![ACCOUNT_MID]),
        (Some(LATEST_NOT_PROVISIONED), vec![]),
    ];
    for (filter, expected) in &expectations {
        let listed =
            accounts_for(&router, &instance_arn, &permission_set_arn, *filter, case).await;
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: before the withdrawal under {filter:?}, expected {expected:?}, \
             got {listed:?}"
        );
    }

    apply(
        &router,
        &engine,
        &instance_arn,
        &permission_set_arn,
        Step::Unassign(ACCOUNT_MID, PRINCIPAL_B),
        &mut change,
        case,
    )
    .await;

    // The withdrawal delivered no copy, so every one of those answers is the same.
    for (filter, expected) in &expectations {
        let listed =
            accounts_for(&router, &instance_arn, &permission_set_arn, *filter, case).await;
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: after the withdrawal under {filter:?}, expected {expected:?}, \
             got {listed:?}"
        );
    }
}
