// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNTS a `ProvisionPermissionSet` call provisions a permission set into, end to
//! end over the `awsJson1_1` wire.
//!
//! THE RULE under test: a provisioning puts the permission set into every account its
//! target names ("the process by which a specified permission set is provisioned to the
//! specified target"), whether or not that account holds an assignment of the permission
//! set, so from the instant the workflow completes each target account holds the definition
//! the call carried and is one of the accounts every read of where the permission set is
//! provisioned reports ("lists all the Amazon Web Services accounts where the specified
//! permission set is provisioned"), under the status that definition implies.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the account ids at both ends of `TargetId`'s own class (`length 12..12`,
//! `^\d{12}$`) and one in between, the number of accounts already holding the permission
//! set (none, one, several), whether the target account holds an assignment of this
//! permission set, of another permission set, or none at all, and every KIND of change to
//! a permission set's definition the implemented API can make (an AWS managed policy, a
//! customer managed policy reference, an inline policy, a permissions boundary, an update).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance. Where a test has to observe BOTH sides of the
//! completion instant it moves that instant through the clock helper
//! `permission_set_provisioning.rs` owns — the clock's business, not the wire's.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::permission_set_provisioning::hold_provisionings;
use crate::ssoadmin_permission_sets::{call, organization_engine, router_over};

/// `ProvisionTargetType`, as the model declares its two values
/// (`ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// `ProvisioningStatus` as the model's enum spells it
/// (`LATEST_PERMISSION_SET_NOT_PROVISIONED | LATEST_PERMISSION_SET_PROVISIONED`).
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// Accounts spanning `TargetId`'s class (`length 12..12`, `^\d{12}$`): both ends of the
/// 12-digit range and two from inside it.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_OTHER: &str = "555555555555";
const ACCOUNT_HIGH: &str = "999999999999";

/// The principal an assignment is granted to; `PrincipalId` takes an identity-store id,
/// which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL: &str = "11111111-2222-3333-4444-555555555555";

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// An instant far enough ahead that every held provisioning is still in flight.
fn in_flight() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3600)
}

/// An instant far enough behind that every held provisioning has completed.
fn completed() -> SystemTime {
    SystemTime::now() - Duration::from_secs(60)
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

/// Grant one account assignment over the wire, which also provisions the permission set
/// into `account_id`.
async fn assign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "CreateAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Withdraw that assignment again over the wire.
async fn unassign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "DeleteAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": TARGET_TYPE_ONE_ACCOUNT,
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Provision a permission set over the wire and return the status object the call answered
/// with, asserting the call was accepted.
async fn provision_ok(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    target_type: &str,
    target_id: Option<&str>,
    case: &str,
) -> Value {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "TargetType": target_type,
    });
    if let Some(target_id) = target_id {
        request["TargetId"] = json!(target_id);
    }
    let (status, body) = call(router, "ProvisionPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ProvisionPermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSetProvisioningStatus"].clone()
}

/// The accounts the permission set is provisioned into under `provisioning_status`, sorted.
async fn provisioned_accounts(
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
        "case {case:?}: ListAccountsForProvisionedPermissionSet expected 200, got {status} {body}"
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

/// The permission sets provisioned into one account under `provisioning_status`, sorted —
/// the MIRROR read of the same copies.
async fn provisioned_permission_sets(
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
        "case {case:?}: ListPermissionSetsProvisionedToAccount expected 200, got {status} {body}"
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

/// Assert an enumeration reported exactly `expected`, whatever order either is in.
fn assert_same(listed: &[String], expected: &[&str], case: &str) {
    let mut want: Vec<String> = expected.iter().map(|value| value.to_string()).collect();
    want.sort();
    assert_eq!(
        listed,
        &want[..],
        "case {case:?}: expected {want:?}, got {listed:?}"
    );
}

/// One way of changing a permission set's definition, as the request that makes it: the
/// operations the model gives for a permission set's policies, boundary and description.
struct ChangeCase {
    label: &'static str,
    operation: &'static str,
    body: Value,
}

/// Every kind of change to a permission set's definition the implemented API can make. Each
/// of them leaves the copy a provisioned account holds behind, so each of them is a
/// definition a later provisioning has to carry into the accounts its target names.
fn change_cases() -> Vec<ChangeCase> {
    vec![
        ChangeCase {
            label: "an AWS managed policy attached",
            operation: "AttachManagedPolicyToPermissionSet",
            body: json!({"ManagedPolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess"}),
        },
        ChangeCase {
            label: "a customer managed policy reference attached",
            operation: "AttachCustomerManagedPolicyReferenceToPermissionSet",
            body: json!({
                "CustomerManagedPolicyReference": {"Name": "ReadOnlyPolicy", "Path": "/"}
            }),
        },
        ChangeCase {
            label: "an inline policy put",
            operation: "PutInlinePolicyToPermissionSet",
            body: json!({"InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}),
        },
        ChangeCase {
            label: "a permissions boundary put",
            operation: "PutPermissionsBoundaryToPermissionSet",
            body: json!({
                "PermissionsBoundary": {
                    "ManagedPolicyArn": "arn:aws:iam::aws:policy/PowerUserAccess"
                }
            }),
        },
        ChangeCase {
            label: "the permission set updated",
            operation: "UpdatePermissionSet",
            body: json!({"Description": "Reprovisioned", "SessionDuration": "PT4H"}),
        },
    ]
}

/// One change from the table above, for a test that needs some change rather than all of
/// them: putting an inline policy on the permission set.
fn inline_policy_change() -> ChangeCase {
    ChangeCase {
        label: "an inline policy put",
        operation: "PutInlinePolicyToPermissionSet",
        body: json!({"InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}),
    }
}

/// Apply one change to a permission set over the wire.
async fn apply_change(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    change: &ChangeCase,
    case: &str,
) {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
    });
    for (member, value) in change.body.as_object().expect("the change carries members") {
        request[member] = value.clone();
    }
    let (status, body) = call(router, change.operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} expected 200, got {status} {body}",
        change.operation
    );
}

/// How the target account stands with assignments before the provisioning: what the rule
/// must NOT depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetState {
    /// The account holds no assignment at all.
    Unassigned,
    /// The account holds an assignment of ANOTHER permission set, so it is provisioned
    /// with that one and not with the one being provisioned here.
    AssignedAnotherPermissionSet,
    /// The account already holds an assignment of the permission set being provisioned.
    AssignedThisPermissionSet,
}

/// One row of the table: the target account, how it stands with assignments, and the other
/// accounts that already hold the permission set through an assignment.
#[derive(Debug)]
struct Row {
    target: &'static str,
    state: TargetState,
    already_provisioned: &'static [&'static str],
}

/// The table: the ends of `TargetId`'s own 12-digit class and values from inside it, each
/// kind of assignment state the target account can be in, and none, one or several other
/// accounts already holding the permission set.
fn rows() -> Vec<Row> {
    vec![
        Row {
            target: ACCOUNT_LOW,
            state: TargetState::Unassigned,
            already_provisioned: &[],
        },
        Row {
            target: ACCOUNT_HIGH,
            state: TargetState::Unassigned,
            already_provisioned: &[],
        },
        Row {
            target: ACCOUNT_OTHER,
            state: TargetState::Unassigned,
            already_provisioned: &[ACCOUNT_MID],
        },
        Row {
            target: ACCOUNT_MID,
            state: TargetState::Unassigned,
            already_provisioned: &[ACCOUNT_LOW, ACCOUNT_HIGH],
        },
        Row {
            target: ACCOUNT_OTHER,
            state: TargetState::AssignedAnotherPermissionSet,
            already_provisioned: &[],
        },
        Row {
            target: ACCOUNT_HIGH,
            state: TargetState::AssignedAnotherPermissionSet,
            already_provisioned: &[ACCOUNT_LOW, ACCOUNT_MID],
        },
        Row {
            target: ACCOUNT_MID,
            state: TargetState::AssignedThisPermissionSet,
            already_provisioned: &[],
        },
        Row {
            target: ACCOUNT_LOW,
            state: TargetState::AssignedThisPermissionSet,
            already_provisioned: &[ACCOUNT_MID, ACCOUNT_HIGH],
        },
    ]
}

/// The fixture one row runs against: the permission set under test, a second permission set
/// for the rows whose target is assigned another one, and the accounts that already hold the
/// permission set when the provisioning is made.
async fn fixture_for(
    row: &Row,
    case: &str,
) -> (
    MultiServiceRouter,
    String,
    Arc<engine::InMemoryEngine>,
    String,
    Vec<String>,
) {
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
    let mut holding: Vec<String> = Vec::new();
    for account_id in row.already_provisioned {
        assign(&router, &instance_arn, &permission_set_arn, account_id, case).await;
        holding.push((*account_id).to_string());
    }
    match row.state {
        TargetState::Unassigned => {}
        TargetState::AssignedAnotherPermissionSet => {
            let other = create_permission_set(&router, &instance_arn, "Other").await;
            assign(&router, &instance_arn, &other, row.target, case).await;
        }
        TargetState::AssignedThisPermissionSet => {
            assign(
                &router,
                &instance_arn,
                &permission_set_arn,
                row.target,
                case,
            )
            .await;
            holding.push(row.target.to_string());
        }
    }
    holding.sort();
    holding.dedup();
    (router, instance_arn, engine, permission_set_arn, holding)
}

/// THE RULE: the account an `AWS_ACCOUNT` target names is provisioned by the call — it
/// holds the definition the call carried once the workflow completes, and both reads of
/// where the permission set is provisioned report it from then on — whatever assignments
/// that account holds, and with the accounts outside the target left as they were.
#[tokio::test]
async fn a_target_account_is_provisioned_whatever_assignments_it_holds() {
    for row in rows() {
        let case = format!("{row:?}");
        let (router, instance_arn, engine, permission_set_arn, holding) =
            fixture_for(&row, &case).await;

        // Before the call, the permission set is provisioned into exactly the accounts
        // that hold an assignment of it.
        let before =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, &case).await;
        assert_same(&before, &to_refs(&holding), &format!("{case}: before"));

        let status = provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            TARGET_TYPE_ONE_ACCOUNT,
            Some(row.target),
            &case,
        )
        .await;
        assert_eq!(
            status["AccountId"].as_str(),
            Some(row.target),
            "case {case:?}: the status names the account the target named, got {status}"
        );

        // While the workflow is in flight nothing has landed: the accounts holding the
        // permission set are the ones that held it before the call.
        hold_provisionings(&engine, in_flight());
        let in_progress =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, &case).await;
        assert_same(&in_progress, &to_refs(&holding), &format!("{case}: in flight"));

        // Once the clock makes the workflow due, the target account is provisioned — and
        // it holds the permission set's current definition, which is what the call carried.
        hold_provisionings(&engine, completed());
        let mut expected: Vec<String> = holding.clone();
        expected.push(row.target.to_string());
        expected.sort();
        expected.dedup();
        let after =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, &case).await;
        assert_same(&after, &to_refs(&expected), &format!("{case}: completed"));
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            &case,
        )
        .await;
        assert!(
            latest.contains(&row.target.to_string()),
            "case {case:?}: expected {} among the accounts holding the latest definition, got \
             {latest:?}",
            row.target
        );
        let stale = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            &case,
        )
        .await;
        assert_same(&stale, &[], &format!("{case}: completed, stale"));

        // The MIRROR read answers the same copy: the account is provisioned with this
        // permission set.
        let sets = provisioned_permission_sets(
            &router,
            &instance_arn,
            row.target,
            Some(LATEST_PROVISIONED),
            &case,
        )
        .await;
        assert!(
            sets.contains(&permission_set_arn),
            "case {case:?}: expected the permission set provisioned to {}, got {sets:?}",
            row.target
        );
    }
}

/// The copy a provisioning delivers carries the definition as it stood when the call was
/// accepted, so a change made to the permission set afterwards leaves the target account
/// behind — and provisioning it again brings that account back up to date. Asserted for
/// every kind of change the API can make, on an account that holds no assignment at all.
#[tokio::test]
async fn a_provisioned_account_holds_the_definition_the_call_carried() {
    for change in change_cases() {
        let case = format!("{} after provisioning", change.label);
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;

        provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_OTHER),
            &case,
        )
        .await;
        hold_provisionings(&engine, completed());
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            &case,
        )
        .await;
        assert_same(&latest, &[ACCOUNT_OTHER], &format!("{case}: provisioned"));

        // The change leaves the account holding a copy of an earlier definition.
        apply_change(&router, &instance_arn, &permission_set_arn, &change, &case).await;
        let stale = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            &case,
        )
        .await;
        assert_same(&stale, &[ACCOUNT_OTHER], &format!("{case}: changed"));
        let stale_sets = provisioned_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_OTHER,
            Some(LATEST_NOT_PROVISIONED),
            &case,
        )
        .await;
        assert_same(
            &stale_sets,
            &[permission_set_arn.as_str()],
            &format!("{case}: changed, mirror"),
        );

        // Provisioning it again carries the new definition into the same account.
        provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_OTHER),
            &case,
        )
        .await;
        hold_provisionings(&engine, completed());
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            &case,
        )
        .await;
        assert_same(
            &latest,
            &[ACCOUNT_OTHER],
            &format!("{case}: reprovisioned"),
        );
    }
}

/// An `ALL_PROVISIONED_ACCOUNTS` target names every account that holds the permission set
/// at the instant the call is accepted, so the accounts a direct provisioning put it into
/// are among them — whether they hold an assignment of it or not. Asserted over the number
/// of such accounts (one, several) and over both ways an account comes to hold it.
#[tokio::test]
async fn provisioning_every_provisioned_account_reaches_the_directly_provisioned_ones() {
    // (case, the accounts provisioned directly, the accounts provisioned by an assignment)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one directly provisioned account", vec![ACCOUNT_OTHER], vec![]),
        (
            "several directly provisioned accounts",
            vec![ACCOUNT_LOW, ACCOUNT_OTHER, ACCOUNT_HIGH],
            vec![],
        ),
        (
            "directly provisioned and assigned accounts",
            vec![ACCOUNT_LOW, ACCOUNT_OTHER],
            vec![ACCOUNT_MID, ACCOUNT_HIGH],
        ),
    ];

    for (case, directly, assigned) in cases {
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
        for account_id in &assigned {
            assign(&router, &instance_arn, &permission_set_arn, account_id, case).await;
        }
        for account_id in &directly {
            provision_ok(
                &router,
                &instance_arn,
                &permission_set_arn,
                TARGET_TYPE_ONE_ACCOUNT,
                Some(account_id),
                case,
            )
            .await;
        }
        hold_provisionings(&engine, completed());
        let mut expected: Vec<&str> = directly.clone();
        expected.extend(assigned.iter().copied());
        let holding =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, case).await;
        assert_same(&holding, &expected, &format!("{case}: before the change"));

        // A change leaves every one of them behind, which is what the reprovisioning of
        // every provisioned account is for.
        apply_change(
            &router,
            &instance_arn,
            &permission_set_arn,
            &inline_policy_change(),
            case,
        )
        .await;
        let stale = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            case,
        )
        .await;
        assert_same(&stale, &expected, &format!("{case}: changed"));

        let status = provision_ok(
            &router,
            &instance_arn,
            &permission_set_arn,
            TARGET_TYPE_ALL_PROVISIONED,
            None,
            case,
        )
        .await;
        assert!(
            status["AccountId"].is_null(),
            "case {case:?}: a target of every provisioned account names no single account, got \
             {status}"
        );
        hold_provisionings(&engine, completed());
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            case,
        )
        .await;
        assert_same(&latest, &expected, &format!("{case}: reprovisioned"));
    }
}

/// A provisioning is what put the permission set into the account, so the account keeps it
/// when an assignment granted there afterwards is withdrawn again — holding the LAST copy
/// delivered to it, which is the grant's when the permission set changed in between. An
/// account that was never provisioned directly is deprovisioned with its last assignment,
/// which is the row this is read against.
#[tokio::test]
async fn a_directly_provisioned_account_keeps_the_permission_set_when_an_assignment_goes() {
    // (case, provisioned directly first, the permission set changed before the grant, the
    //  accounts left holding it after the withdrawal)
    let cases: Vec<(&str, bool, bool, Vec<&str>)> = vec![
        ("provisioned directly first", true, false, vec![ACCOUNT_OTHER]),
        (
            "provisioned directly, then changed, then assigned",
            true,
            true,
            vec![ACCOUNT_OTHER],
        ),
        ("never provisioned directly", false, false, vec![]),
    ];

    for (case, provisioned_directly, changed_before_grant, expected) in cases {
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
        if provisioned_directly {
            provision_ok(
                &router,
                &instance_arn,
                &permission_set_arn,
                TARGET_TYPE_ONE_ACCOUNT,
                Some(ACCOUNT_OTHER),
                case,
            )
            .await;
            hold_provisionings(&engine, completed());
        }
        if changed_before_grant {
            // The copy the provisioning delivered is now behind; the grant below delivers
            // the current definition to the same account.
            apply_change(
                &router,
                &instance_arn,
                &permission_set_arn,
                &inline_policy_change(),
                case,
            )
            .await;
        }
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            ACCOUNT_OTHER,
            case,
        )
        .await;
        let holding =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, case).await;
        assert_same(&holding, &[ACCOUNT_OTHER], &format!("{case}: assigned"));

        unassign(
            &router,
            &instance_arn,
            &permission_set_arn,
            ACCOUNT_OTHER,
            case,
        )
        .await;
        let holding =
            provisioned_accounts(&router, &instance_arn, &permission_set_arn, None, case).await;
        assert_same(&holding, &expected, &format!("{case}: withdrawn"));
        // The copy the account keeps is the last one delivered there, so it is the current
        // definition whether or not the permission set changed before the grant.
        let latest = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            case,
        )
        .await;
        assert_same(&latest, &expected, &format!("{case}: withdrawn, latest"));
        let stale = provisioned_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            case,
        )
        .await;
        assert_same(&stale, &[], &format!("{case}: withdrawn, stale"));
    }
}

/// A permission set that no longer exists is provisioned nowhere: deleting it takes the
/// copies its provisionings delivered with it, so a later read of the account reports
/// neither the permission set nor a provisioning of it.
#[tokio::test]
async fn deleting_a_permission_set_takes_the_copies_it_was_provisioned_as() {
    let case = "a deleted permission set";
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Rollout").await;
    let kept = create_permission_set(&router, &instance_arn, "Kept").await;
    for arn in [&permission_set_arn, &kept] {
        let status = provision_ok(
            &router,
            &instance_arn,
            arn,
            TARGET_TYPE_ONE_ACCOUNT,
            Some(ACCOUNT_OTHER),
            case,
        )
        .await;
        assert_eq!(
            status["PermissionSetArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the status names the permission set provisioned, got {status}"
        );
    }
    hold_provisionings(&engine, completed());
    let sets = provisioned_permission_sets(&router, &instance_arn, ACCOUNT_OTHER, None, case).await;
    assert_same(
        &sets,
        &[permission_set_arn.as_str(), kept.as_str()],
        &format!("{case}: provisioned"),
    );

    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeletePermissionSet expected 200, got {status} {body}"
    );
    let sets = provisioned_permission_sets(&router, &instance_arn, ACCOUNT_OTHER, None, case).await;
    assert_same(&sets, &[kept.as_str()], &format!("{case}: deleted"));
}

/// Borrow a list of owned ids as the `&str` rows the assertions take.
fn to_refs(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}
