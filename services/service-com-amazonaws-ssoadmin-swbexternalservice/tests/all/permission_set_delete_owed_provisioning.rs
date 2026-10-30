// SPDX-License-Identifier: CC0-1.0

//! `DeletePermissionSet` against the WORK still owed on the permission set it removes,
//! end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: deleting a resource removes every row the store holds for it,
//! including the work still OWED on it, so no later sweep of the clock can write state for
//! a resource no reader can see. A `ProvisionPermissionSet` that has not landed yet is one
//! of those rows: its copy is delivered when the clock makes it due, whether or not
//! anybody calls, so a delete that left the record behind would re-deliver a copy of a
//! permission set the delete had just forgotten — while the instance-level delete already
//! drops the same owed work for everything held under the instance it removes.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over:
//! both `ProvisionTargetType` values the model declares, the account ids at both ends of
//! `TargetId`'s own `^\d{12}$` class, the number of accounts holding the permission set,
//! the number of provisionings in flight when the delete lands, whether the target account
//! holds an assignment at all, and whether the provisioning had ALREADY landed when the
//! delete arrived. Every row also carries a SECOND permission set under the same instance
//! whose own provisioning must land untouched, so the removal is scoped to the permission
//! set the delete names.
//!
//! The responses go over `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. No read of this API reports a copy of a permission set that is gone —
//! both projections filter on the permission set still existing — so the leaked row itself
//! is read from the store the way this crate's other provisioning suites do, while
//! everything about the surviving permission set is asserted over the wire. The harness is
//! the one `ssoadmin_permission_sets.rs` owns, and the calls run against an ORGANIZATION
//! instance because the permission-set surface is withdrawn on an account instance.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::permission_set_provisioning::{
    completed, create_permission_set, hold_unapplied_provisionings, in_flight,
    organization_fixture,
};
use crate::ssoadmin_permission_sets::{call, error_code, with_store};

/// The two values `ProvisionTargetType` accepts on the wire, as the model declares them
/// (`smithy_query(query="operation",operation="ProvisionPermissionSet")`: `TargetType`
/// targets `ProvisionTargetType`, one of `ALL_PROVISIONED_ACCOUNTS | AWS_ACCOUNT`).
const TARGET_TYPE_ONE_ACCOUNT: &str = "AWS_ACCOUNT";
const TARGET_TYPE_ALL_PROVISIONED: &str = "ALL_PROVISIONED_ACCOUNTS";

/// Account ids spanning `TargetId`'s own class (`length 12..12`, `^\d{12}$`): both ends of
/// the 12-digit range and one in between.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_HIGH: &str = "999999999999";

/// An account no row of this suite ever assigns anything on, so a provisioning is the only
/// thing that can put a permission set there.
const ACCOUNT_UNASSIGNED: &str = "555555555555";

/// Principal ids the assignments are granted to; `PrincipalId` takes the identity-store
/// user or group id, which the model's pattern gives as a hyphenated UUID.
const PRINCIPAL_A: &str = "11111111-2222-3333-4444-555555555555";

/// `StatusValues` as the model spells the terminal status a landed provisioning reports.
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// One row of the table: the surface the delete races, and when it races it.
struct Case {
    /// What the row is, named in every assertion it makes.
    label: &'static str,
    /// The accounts that hold an assignment of each permission set before the
    /// provisionings are accepted — which is what makes them "provisioned accounts" for an
    /// `ALL_PROVISIONED_ACCOUNTS` target.
    assigned: &'static [&'static str],
    /// The `TargetType` every provisioning of the row carries.
    target_type: &'static str,
    /// Its `TargetId`, for the target type that names one account.
    target_id: Option<&'static str>,
    /// How many provisionings of the doomed permission set are accepted before the delete.
    provisionings: usize,
    /// Whether the clock had already delivered those copies when the delete arrived.
    landed_before_delete: bool,
}

/// The rows: both target types, both ends of the account-id class, one and several
/// accounts, one and several provisionings in flight, a target outside the assigned
/// accounts, and a provisioning that had already landed.
const CASES: &[Case] = &[
    Case {
        label: "one in flight, a target of the lowest account id",
        assigned: &[],
        target_type: TARGET_TYPE_ONE_ACCOUNT,
        target_id: Some(ACCOUNT_LOW),
        provisionings: 1,
        landed_before_delete: false,
    },
    Case {
        label: "three in flight, a target of the highest account id",
        assigned: &[],
        target_type: TARGET_TYPE_ONE_ACCOUNT,
        target_id: Some(ACCOUNT_HIGH),
        provisionings: 3,
        landed_before_delete: false,
    },
    Case {
        label: "one in flight over every provisioned account, one account holding it",
        assigned: &[ACCOUNT_MID],
        target_type: TARGET_TYPE_ALL_PROVISIONED,
        target_id: None,
        provisionings: 1,
        landed_before_delete: false,
    },
    Case {
        label: "two in flight over every provisioned account, three accounts holding it",
        assigned: &[ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH],
        target_type: TARGET_TYPE_ALL_PROVISIONED,
        target_id: None,
        provisionings: 2,
        landed_before_delete: false,
    },
    Case {
        label: "one in flight, a target the assignments never reach",
        assigned: &[ACCOUNT_MID],
        target_type: TARGET_TYPE_ONE_ACCOUNT,
        target_id: Some(ACCOUNT_UNASSIGNED),
        provisionings: 1,
        landed_before_delete: false,
    },
    Case {
        label: "a provisioning that had already landed when the delete arrived",
        assigned: &[ACCOUNT_MID],
        target_type: TARGET_TYPE_ONE_ACCOUNT,
        target_id: Some(ACCOUNT_HIGH),
        provisionings: 1,
        landed_before_delete: true,
    },
];

/// Grant one account assignment over the wire, which is what first provisions the
/// permission set into `account_id`.
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
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": PRINCIPAL_A,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Accept one provisioning over the wire and return the `RequestId` it minted.
async fn provision(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &Case,
) -> String {
    let mut request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "TargetType": case.target_type,
    });
    if let Some(target_id) = case.target_id {
        request["TargetId"] = json!(target_id);
    }
    let (status, body) = call(router, "ProvisionPermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "case {:?}: ProvisionPermissionSet expected 200, got {status} {body}",
        case.label
    );
    body["PermissionSetProvisioningStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the provisioning minted no RequestId: {body}", case.label))
        .to_string()
}

/// `DescribePermissionSetProvisioningStatus` for one handle.
async fn describe_status(
    router: &MultiServiceRouter,
    instance_arn: &str,
    request_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribePermissionSetProvisioningStatus",
        &json!({
            "InstanceArn": instance_arn,
            "ProvisionPermissionSetRequestId": request_id,
        }),
    )
    .await
}

/// Every account the store records a provisioned COPY of `permission_set_arn` for, in
/// ascending account order.
///
/// Read from the store because no read of this API reports a copy whose permission set is
/// gone, which is exactly the row this rule forbids; the other provisioning suites of this
/// crate reach the store the same way.
fn copied_accounts(engine: &Arc<engine::InMemoryEngine>, permission_set_arn: &str) -> Vec<String> {
    with_store(engine, |store| {
        let mut accounts: Vec<String> = store
            .provisioned_copies
            .iter()
            .filter(|copy| copy.permission_set_arn == permission_set_arn)
            .map(|copy| copy.account_id.clone())
            .collect();
        accounts.sort();
        accounts
    })
}

/// How many provisioning records the store holds for `permission_set_arn`, and how many of
/// those are still unapplied — the write the clock owes.
fn provisioning_rows(
    engine: &Arc<engine::InMemoryEngine>,
    permission_set_arn: &str,
) -> (usize, usize) {
    with_store(engine, |store| {
        let rows: Vec<bool> = store
            .permission_set_provisionings
            .iter()
            .filter(|record| record.permission_set_arn == permission_set_arn)
            .map(|record| record.applied)
            .collect();
        (rows.len(), rows.iter().filter(|applied| !**applied).count())
    })
}

/// Any call at all, which is what runs the store's clock sweep (`on_request`) past an
/// instant a transition has been made due at.
async fn sweep_the_clock(router: &MultiServiceRouter, case: &str) {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInstances expected 200, got {status} {body}"
    );
}

/// The accounts the surviving permission set must be reported as provisioned into: the
/// accounts its assignments granted it on, plus the one account a target of one account
/// names.
fn expected_kept_accounts(case: &Case) -> Vec<String> {
    let mut accounts: Vec<String> = case.assigned.iter().map(|id| id.to_string()).collect();
    if let Some(target_id) = case.target_id {
        if !accounts.iter().any(|held| held == target_id) {
            accounts.push(target_id.to_string());
        }
    }
    accounts.sort();
    accounts
}

/// Walk `ListAccountsForProvisionedPermissionSet` and return every account it reports.
async fn list_provisioned_accounts(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListAccountsForProvisionedPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": permission_set_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAccountsForProvisionedPermissionSet expected 200, got {status} {body}"
    );
    body["AccountIds"]
        .as_array()
        .map(|accounts| {
            accounts
                .iter()
                .map(|account| {
                    account
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: an AccountId is not a string: {body}")
                        })
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// THE RULE: once the permission set is deleted, the store holds neither its provisioning
/// records nor any copy of it, and no later sweep of the clock writes one — whatever the
/// target, however many provisionings were accepted, and whether or not they had already
/// landed.
#[tokio::test]
async fn deleting_a_permission_set_drops_the_provisioning_work_still_owed_on_it() {
    for case in CASES {
        let label = case.label;
        let (router, instance_arn, engine) = organization_fixture();
        let doomed = create_permission_set(&router, &instance_arn, "Doomed").await;
        let kept = create_permission_set(&router, &instance_arn, "Kept").await;
        for account_id in case.assigned {
            assign(&router, &instance_arn, &doomed, account_id, label).await;
            assign(&router, &instance_arn, &kept, account_id, label).await;
        }

        // The provisionings of both permission sets, held in flight as they are accepted
        // so the delete lands inside the window the clock owes them.
        let mut doomed_handles: Vec<String> = Vec::new();
        for _ in 0..case.provisionings {
            doomed_handles.push(provision(&router, &instance_arn, &doomed, case).await);
            hold_unapplied_provisionings(&engine, in_flight());
        }
        let kept_handle = provision(&router, &instance_arn, &kept, case).await;
        hold_unapplied_provisionings(&engine, in_flight());

        if case.landed_before_delete {
            // The clock has already delivered them: the delete then races nothing, and the
            // copies it forgets must stay forgotten all the same.
            hold_unapplied_provisionings(&engine, completed());
            sweep_the_clock(&router, label).await;
            assert!(
                !copied_accounts(&engine, &doomed).is_empty(),
                "case {label:?}: the provisioning must have landed before the delete, \
                 got no copy of the deleted set"
            );
        } else {
            let (rows, unapplied) = provisioning_rows(&engine, &doomed);
            assert_eq!(
                (rows, unapplied),
                (case.provisionings, case.provisionings),
                "case {label:?}: expected {} provisionings of the doomed set, all unapplied, \
                 got {rows} rows of which {unapplied} unapplied",
                case.provisionings
            );
        }

        let (status, body) = call(
            &router,
            "DeletePermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": doomed}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: DeletePermissionSet expected 200, got {status} {body}"
        );

        // The rows go with the resource: no provisioning record and no copy survives the
        // delete, so there is nothing left for a sweep to apply.
        let (rows, unapplied) = provisioning_rows(&engine, &doomed);
        assert_eq!(
            (rows, unapplied),
            (0, 0),
            "case {label:?}: the deleted set must hold no provisioning record, got {rows} rows \
             of which {unapplied} unapplied"
        );
        assert_eq!(
            copied_accounts(&engine, &doomed),
            Vec::<String>::new(),
            "case {label:?}: the deleted set must hold no copy right after the delete, got {:?}",
            copied_accounts(&engine, &doomed)
        );

        // Then the clock passes the instant every remaining provisioning was due at, and
        // any call runs the sweep: still nothing is written for the permission set that is
        // gone.
        hold_unapplied_provisionings(&engine, completed());
        sweep_the_clock(&router, label).await;
        assert_eq!(
            copied_accounts(&engine, &doomed),
            Vec::<String>::new(),
            "case {label:?}: the sweep must write no copy of the deleted set, got {:?}",
            copied_accounts(&engine, &doomed)
        );
        let (rows, _) = provisioning_rows(&engine, &doomed);
        assert_eq!(
            rows, 0,
            "case {label:?}: the sweep must keep no provisioning of the deleted set, got {rows} \
             rows"
        );
        for handle in &doomed_handles {
            let (status, described) = describe_status(&router, &instance_arn, handle).await;
            assert_eq!(
                status, 400,
                "case {label:?}: the describe of {handle} expected a refusal, got {status} \
                 {described}"
            );
            assert_eq!(
                error_code(&described),
                "ResourceNotFoundException",
                "case {label:?}: the describe of {handle} expected ResourceNotFoundException, \
                 got {described}"
            );
        }

        // And the removal is SCOPED: the other permission set's own provisioning lands,
        // reports its terminal status and is reported where it was provisioned.
        let (status, described) = describe_status(&router, &instance_arn, &kept_handle).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the surviving set's provisioning expected 200, got {status} \
             {described}"
        );
        assert_eq!(
            described["PermissionSetProvisioningStatus"]["Status"]
                .as_str()
                .unwrap_or_default(),
            STATUS_SUCCEEDED,
            "case {label:?}: the surviving set's provisioning expected {STATUS_SUCCEEDED}, got \
             {described}"
        );
        let listed = list_provisioned_accounts(&router, &instance_arn, &kept, label).await;
        assert_eq!(
            listed,
            expected_kept_accounts(case),
            "case {label:?}: the surviving set's provisioned accounts expected {:?}, got {listed:?}",
            expected_kept_accounts(case)
        );
        assert_eq!(
            copied_accounts(&engine, &kept),
            case.target_id
                .map(|target_id| vec![target_id.to_string()])
                .unwrap_or_else(|| {
                    let mut accounts: Vec<String> =
                        case.assigned.iter().map(|id| id.to_string()).collect();
                    accounts.sort();
                    accounts
                }),
            "case {label:?}: the surviving set's copies expected the accounts its provisioning \
             targeted, got {:?}",
            copied_accounts(&engine, &kept)
        );

        // The deleted set itself is gone to every reader of it, which is the delete's own
        // rule and the reason no read can expose the row this one forbids.
        let (status, body) = call(
            &router,
            "ListAccountsForProvisionedPermissionSet",
            &json!({"InstanceArn": instance_arn, "PermissionSetArn": doomed}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a read of the deleted set expected a refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: a read of the deleted set expected ResourceNotFoundException, got \
             {body}"
        );
    }
}
