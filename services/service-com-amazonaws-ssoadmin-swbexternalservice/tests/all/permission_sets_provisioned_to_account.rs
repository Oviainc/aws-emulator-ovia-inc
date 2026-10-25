// SPDX-License-Identifier: CC0-1.0

//! `ListPermissionSetsProvisionedToAccount`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation reports the permission sets that are provisioned
//! into the Amazon Web Services account the request names ("Lists all the permission sets
//! that are provisioned to a specified Amazon Web Services account") — each permission set
//! exactly once, as its `PermissionSetArn`, in ascending ARN order, one page per call under
//! the `@paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults,
//! items: PermissionSets}` contract, with a continuation token only while permission sets
//! are left over. An account holds a permission set once it has been provisioned there,
//! which is what granting an account assignment on that account does, so the number of
//! principals assigned through it changes nothing about how often it is reported.
//!
//! THE FILTER rule: `ProvisioningStatus` selects among those permission sets by the
//! definition the account holds of each — `LATEST_PERMISSION_SET_PROVISIONED` for a copy
//! that is the permission set's current definition, `LATEST_PERMISSION_SET_NOT_PROVISIONED`
//! for one the permission set has changed since — and a request that names no
//! `ProvisioningStatus` is not filtered.
//!
//! An account nothing is provisioned into is an EMPTY page, while an `InstanceArn` nothing
//! is held under is a `ResourceNotFoundException`, a `NextToken` this enumeration did not
//! issue is a `ValidationException`, and the whole permission-set surface stays withdrawn
//! on an account instance. Being a report, it changes nothing a later caller sees.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the NUMBER of provisioned permission sets (none, one, several), the account ids at
//! both ends of `AccountId`'s own `^\d{12}$` class, the number of principals assigned on
//! one account, the order the assignments were granted in, every `MaxResults` the model's
//! `range(min: 1, max: 100)` admits around the collection size, and every KIND of change to
//! a permission set the implemented API can make.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_engine, router_over, with_store,
    UNKNOWN_INSTANCE_ARN,
};

/// `ProvisioningStatus::LATEST_PERMISSION_SET_PROVISIONED`, as the model's
/// `ProvisioningStatus` enum (`LATEST_PERMISSION_SET_NOT_PROVISIONED |
/// LATEST_PERMISSION_SET_PROVISIONED`) spells it.
const LATEST_PROVISIONED: &str = "LATEST_PERMISSION_SET_PROVISIONED";

/// `ProvisioningStatus::LATEST_PERMISSION_SET_NOT_PROVISIONED`, from the same enum.
const LATEST_NOT_PROVISIONED: &str = "LATEST_PERMISSION_SET_NOT_PROVISIONED";

/// Account ids spanning `AccountId`'s own class (`length 12..12`, `^\d{12}$`): both ends
/// of the 12-digit range and one in between.
const ACCOUNT_LOW: &str = "000000000000";
const ACCOUNT_MID: &str = "123456789012";
const ACCOUNT_HIGH: &str = "999999999999";

/// Principal ids the assignments are granted to. `PrincipalId` takes the identity-store
/// user or group id, which the recorded fixtures and the model's pattern give as a
/// hyphenated UUID.
const PRINCIPAL_A: &str = "11111111-2222-3333-4444-555555555555";
const PRINCIPAL_B: &str = "66666666-7777-8888-9999-000000000000";
const PRINCIPAL_C: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

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
    let (status, created) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {created}"
    );
    created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {created}"))
        .to_string()
}

/// Grant one account assignment over the wire, which is what PROVISIONS the permission
/// set into `account_id`.
async fn assign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    principal_id: &str,
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
            "PrincipalId": principal_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Withdraw one account assignment over the wire.
async fn unassign(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    account_id: &str,
    principal_id: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "DeleteAccountAssignment",
        &json!({
            "InstanceArn": instance_arn,
            "TargetId": account_id,
            "TargetType": "AWS_ACCOUNT",
            "PermissionSetArn": permission_set_arn,
            "PrincipalType": "USER",
            "PrincipalId": principal_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAccountAssignment on {account_id} expected 200, got {status} {body}"
    );
}

/// Walk the whole enumeration, following `NextToken`, and return every `PermissionSetArn`
/// returned together with the number of pages it took.
async fn list_permission_sets(
    router: &MultiServiceRouter,
    instance_arn: &str,
    account_id: &str,
    provisioning_status: Option<&str>,
    max_results: Option<i64>,
    case: &str,
) -> (Vec<String>, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({
            "InstanceArn": instance_arn,
            "AccountId": account_id,
        });
        if let Some(status) = provisioning_status {
            request["ProvisioningStatus"] = json!(status);
        }
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListPermissionSetsProvisionedToAccount", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPermissionSetsProvisionedToAccount expected 200, got {status} \
             {body}"
        );
        let page: Vec<String> = body["PermissionSets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: PermissionSets must be a list, got {body}"))
            .iter()
            .map(|arn| {
                arn.as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: an entry is not a string: {body}"))
                    .to_string()
            })
            .collect();
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page must hold at most MaxResults {limit} permission sets, got {}",
                page.len()
            );
        }
        pages += 1;
        collected.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            pages <= 64,
            "case {case:?}: the enumeration must terminate, got {pages} pages"
        );
    }
    (collected, pages)
}

/// Assert `listed` is exactly `expected`, each permission set once, in ascending ARN order.
fn assert_reports_exactly(listed: &[String], expected: &[&str], case: &str) {
    let mut want: Vec<String> = expected.iter().map(|arn| arn.to_string()).collect();
    want.sort();
    want.dedup();
    assert_eq!(
        listed,
        &want[..],
        "case {case:?}: expected exactly {want:?} in ARN order, got {listed:?}"
    );
    assert!(
        listed.windows(2).all(|pair| pair[0] < pair[1]),
        "case {case:?}: the enumeration must be strictly ascending by PermissionSetArn, got \
         {listed:?}"
    );
    for arn in listed {
        assert!(
            arn.starts_with("arn:aws:sso:::permissionSet/"),
            "case {case:?}: an entry must be a PermissionSetArn, got {arn:?}"
        );
    }
}

/// One row of the reporting table: the assignments to grant, as (which created permission
/// set, which account, which principal), and the permission sets the account named by the
/// read then holds.
struct ProvisionCase {
    label: &'static str,
    /// How many permission sets the row creates, so a row can leave some provisioned
    /// nowhere.
    permission_sets: usize,
    /// (permission-set index, account id, principal id), granted in this order.
    assignments: Vec<(usize, &'static str, &'static str)>,
    /// The account the read is made for.
    account_id: &'static str,
    /// The indexes of the permission sets expected back for that account.
    expected: Vec<usize>,
}

/// THE RULE: every permission set provisioned into the account is reported once, in ARN
/// order — over a table spanning the number of permission sets, the ends of the `AccountId`
/// class, the number of principals assigned on one account, and the order the assignments
/// were granted in.
#[tokio::test]
async fn every_provisioned_permission_set_is_reported_once_in_arn_order() {
    let rows: Vec<ProvisionCase> = vec![
        ProvisionCase {
            label: "an account nothing is provisioned into",
            permission_sets: 2,
            assignments: vec![(0, ACCOUNT_HIGH, PRINCIPAL_A)],
            account_id: ACCOUNT_MID,
            expected: vec![],
        },
        ProvisionCase {
            label: "one permission set, one principal",
            permission_sets: 2,
            assignments: vec![(0, ACCOUNT_MID, PRINCIPAL_A)],
            account_id: ACCOUNT_MID,
            expected: vec![0],
        },
        ProvisionCase {
            label: "one permission set, three principals",
            permission_sets: 2,
            assignments: vec![
                (0, ACCOUNT_MID, PRINCIPAL_A),
                (0, ACCOUNT_MID, PRINCIPAL_B),
                (0, ACCOUNT_MID, PRINCIPAL_C),
            ],
            account_id: ACCOUNT_MID,
            expected: vec![0],
        },
        ProvisionCase {
            label: "three permission sets, granted in creation order",
            permission_sets: 3,
            assignments: vec![
                (0, ACCOUNT_MID, PRINCIPAL_A),
                (1, ACCOUNT_MID, PRINCIPAL_A),
                (2, ACCOUNT_MID, PRINCIPAL_A),
            ],
            account_id: ACCOUNT_MID,
            expected: vec![0, 1, 2],
        },
        ProvisionCase {
            label: "three permission sets, granted in the reverse order",
            permission_sets: 3,
            assignments: vec![
                (2, ACCOUNT_MID, PRINCIPAL_A),
                (1, ACCOUNT_MID, PRINCIPAL_A),
                (0, ACCOUNT_MID, PRINCIPAL_A),
            ],
            account_id: ACCOUNT_MID,
            expected: vec![0, 1, 2],
        },
        ProvisionCase {
            label: "the low end of the AccountId class, two of three permission sets",
            permission_sets: 3,
            assignments: vec![
                (2, ACCOUNT_LOW, PRINCIPAL_B),
                (0, ACCOUNT_LOW, PRINCIPAL_A),
                (1, ACCOUNT_HIGH, PRINCIPAL_A),
            ],
            account_id: ACCOUNT_LOW,
            expected: vec![0, 2],
        },
        ProvisionCase {
            label: "the high end of the AccountId class, principals interleaved",
            permission_sets: 3,
            assignments: vec![
                (1, ACCOUNT_HIGH, PRINCIPAL_C),
                (0, ACCOUNT_LOW, PRINCIPAL_A),
                (1, ACCOUNT_HIGH, PRINCIPAL_A),
                (2, ACCOUNT_HIGH, PRINCIPAL_B),
            ],
            account_id: ACCOUNT_HIGH,
            expected: vec![1, 2],
        },
        ProvisionCase {
            // The same assignment granted twice grants it once, so the permission set it
            // provisioned is still one permission set.
            label: "the same assignment granted twice",
            permission_sets: 1,
            assignments: vec![(0, ACCOUNT_MID, PRINCIPAL_A), (0, ACCOUNT_MID, PRINCIPAL_A)],
            account_id: ACCOUNT_MID,
            expected: vec![0],
        },
    ];

    for (index, case) in rows.iter().enumerate() {
        let label = case.label;
        let (router, instance_arn, _engine) = organization_fixture();
        let mut arns: Vec<String> = Vec::new();
        for slot in 0..case.permission_sets {
            arns.push(
                create_permission_set(&router, &instance_arn, &format!("Row{index}Set{slot}"))
                    .await,
            );
        }

        for (slot, account_id, principal_id) in &case.assignments {
            assign(
                &router,
                &instance_arn,
                &arns[*slot],
                account_id,
                principal_id,
                label,
            )
            .await;
        }

        let expected: Vec<&str> = case
            .expected
            .iter()
            .map(|slot| arns[*slot].as_str())
            .collect();
        let (listed, pages) = list_permission_sets(
            &router,
            &instance_arn,
            case.account_id,
            None,
            None,
            label,
        )
        .await;
        assert_reports_exactly(&listed, &expected, label);
        assert_eq!(
            pages, 1,
            "case {label:?}: {} permission sets fit one default page, got {pages} pages",
            expected.len()
        );
    }
}

/// An account nothing is provisioned into answers an EMPTY page with no continuation token —
/// never a refusal, and never a page belonging to another account.
#[tokio::test]
async fn an_account_nothing_is_provisioned_into_answers_an_empty_page() {
    let (router, instance_arn, _engine) = organization_fixture();
    let filled = create_permission_set(&router, &instance_arn, "Filled").await;
    for account_id in [ACCOUNT_LOW, ACCOUNT_HIGH] {
        assign(&router, &instance_arn, &filled, account_id, PRINCIPAL_A, "filled").await;
    }

    for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
        for max_results in [None, Some(1), Some(100)] {
            let case =
                format!("ProvisioningStatus {provisioning_status:?}, MaxResults {max_results:?}");
            let mut request = json!({
                "InstanceArn": instance_arn,
                "AccountId": ACCOUNT_MID,
            });
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            let (status, body) =
                call(&router, "ListPermissionSetsProvisionedToAccount", &request).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
            assert_eq!(
                body["PermissionSets"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: an account nothing is provisioned into lists none, got {body}"
            );
            assert_eq!(
                body["NextToken"],
                Value::Null,
                "case {case:?}: with nothing left over there is no token, got {body}"
            );
        }
    }
}

/// THE PAGINATION RULE: for ANY page size the model admits, following `NextToken` walks
/// exactly the provisioned permission sets, once each, in the same ARN order, and the last
/// page carries no token.
#[tokio::test]
async fn pagination_walks_every_permission_set_exactly_once_for_any_page_size() {
    // Six permission sets, so several page sizes cut the collection in different places.
    let count = 6usize;
    // `MaxResults` is `range(min: 1, max: 100)`: both published bounds, every size around
    // the collection's own, and one past it.
    let page_sizes = [
        None,
        Some(1),
        Some(2),
        Some(4),
        Some(count as i64 - 1),
        Some(count as i64),
        Some(count as i64 + 1),
        Some(100),
    ];

    for max_results in page_sizes {
        let case = format!("MaxResults {max_results:?}");
        let (router, instance_arn, _engine) = organization_fixture();
        let mut arns: Vec<String> = Vec::new();
        for slot in 0..count {
            arns.push(create_permission_set(&router, &instance_arn, &format!("Paged{slot}")).await);
            assign(
                &router,
                &instance_arn,
                arns.last().expect("the permission set was just created"),
                ACCOUNT_MID,
                PRINCIPAL_A,
                &case,
            )
            .await;
        }
        let expected: Vec<&str> = arns.iter().map(String::as_str).collect();

        let (listed, pages) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            None,
            max_results,
            &case,
        )
        .await;
        assert_reports_exactly(&listed, &expected, &case);

        let expected_pages = match max_results {
            Some(limit) => {
                let limit = limit.clamp(1, 100) as usize;
                count.div_ceil(limit)
            }
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {count} permission sets at that page size take {expected_pages} \
             pages, got {pages}"
        );
    }
}

/// One way of changing a permission set, as the request that makes it: every change to a
/// permission set's definition leaves the copy a provisioned account holds behind.
struct ChangeCase {
    label: &'static str,
    operation: &'static str,
    /// The members the change carries beside `InstanceArn` and `PermissionSetArn`.
    body: Value,
}

/// Every kind of change to a permission set's definition the implemented API can make:
/// attaching an AWS managed policy, attaching a customer managed policy reference, putting
/// an inline policy and putting a permissions boundary.
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
            body: json!({
                "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
            }),
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
    ]
}

/// THE FILTER RULE: `ProvisioningStatus` selects a provisioned permission set by the
/// definition the account holds of it — the permission set's current one, or a copy a later
/// change left behind — and a provisioning that follows the change brings that copy back up
/// to date. Each permission set is judged against ITS OWN definition, so changing one
/// leaves the others where they were. Asserted over every kind of change the API can make.
#[tokio::test]
async fn the_provisioning_status_filter_selects_by_the_definition_the_account_holds() {
    for case in change_cases() {
        let label = case.label;
        let (router, instance_arn, _engine) = organization_fixture();
        let mut arns: Vec<String> = Vec::new();
        for slot in 0..3 {
            arns.push(
                create_permission_set(&router, &instance_arn, &format!("Filtered{slot}")).await,
            );
            assign(
                &router,
                &instance_arn,
                arns.last().expect("the permission set was just created"),
                ACCOUNT_MID,
                PRINCIPAL_A,
                label,
            )
            .await;
        }
        let all: Vec<&str> = arns.iter().map(String::as_str).collect();

        // Before any change, the account holds the current definition of each.
        let (latest, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&latest, &all, &format!("{label}: before the change"));
        let (stale, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&stale, &[], &format!("{label}: before the change, stale"));

        // The change itself, to ONE of the three permission sets, over the wire.
        let changed = arns[1].clone();
        let mut request = json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": changed,
        });
        for (member, value) in case.body.as_object().expect("the change carries members") {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, case.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {} expected 200, got {status} {body}",
            case.operation
        );

        // The account's copy of the changed permission set is now behind: it is still
        // provisioned — the unfiltered enumeration reports it — but no longer with the
        // latest definition. The other two are untouched.
        let untouched: Vec<&str> = vec![arns[0].as_str(), arns[2].as_str()];
        let (latest, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&latest, &untouched, &format!("{label}: after the change"));
        let (stale, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(
            &stale,
            &[changed.as_str()],
            &format!("{label}: after the change, stale"),
        );
        let (unfiltered, _) =
            list_permission_sets(&router, &instance_arn, ACCOUNT_MID, None, None, label).await;
        assert_reports_exactly(&unfiltered, &all, &format!("{label}: after the change, all"));

        // A provisioning that follows the change puts the current definition into the
        // account again.
        assign(
            &router,
            &instance_arn,
            &changed,
            ACCOUNT_MID,
            PRINCIPAL_B,
            label,
        )
        .await;
        let (latest, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&latest, &all, &format!("{label}: after re-provisioning"));
        let (stale, _) = list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(
            &stale,
            &[],
            &format!("{label}: after re-provisioning, stale"),
        );
    }
}

/// The other edge of the rule: a permission set stops being reported for an account when it
/// is no longer provisioned there — which is when the LAST assignment of it on that account
/// is withdrawn, not when the first one is, or when the permission set itself is deleted.
#[tokio::test]
async fn a_permission_set_is_reported_until_its_last_assignment_is_withdrawn() {
    let (router, instance_arn, _engine) = organization_fixture();
    let mut arns: Vec<String> = Vec::new();
    for slot in 0..3 {
        arns.push(create_permission_set(&router, &instance_arn, &format!("Held{slot}")).await);
    }
    // arns[0] carries three principals on the account, arns[1] one, arns[2] two.
    let granted: Vec<(usize, &str)> = vec![
        (0, PRINCIPAL_A),
        (0, PRINCIPAL_B),
        (0, PRINCIPAL_C),
        (1, PRINCIPAL_A),
        (2, PRINCIPAL_A),
        (2, PRINCIPAL_B),
    ];
    for (slot, principal_id) in &granted {
        assign(
            &router,
            &instance_arn,
            &arns[*slot],
            ACCOUNT_MID,
            principal_id,
            "granted",
        )
        .await;
    }

    // (the assignment to withdraw, the permission sets still provisioned afterwards)
    let steps: Vec<((usize, &str), Vec<usize>)> = vec![
        // One of three principals: the account still holds the permission set.
        ((0, PRINCIPAL_B), vec![0, 1, 2]),
        // The ONLY principal of a permission set: it is deprovisioned from the account.
        ((1, PRINCIPAL_A), vec![0, 2]),
        ((2, PRINCIPAL_A), vec![0, 2]),
        // The last principal of that permission set, in the reverse order of granting.
        ((2, PRINCIPAL_B), vec![0]),
        ((0, PRINCIPAL_C), vec![0]),
        // Nothing is provisioned into the account once the last assignment is gone.
        ((0, PRINCIPAL_A), vec![]),
    ];
    for ((slot, principal_id), expected) in steps {
        let case = format!("withdrawn {principal_id} on permission set {slot}");
        unassign(
            &router,
            &instance_arn,
            &arns[slot],
            ACCOUNT_MID,
            principal_id,
            &case,
        )
        .await;
        let expected: Vec<&str> = expected.iter().map(|slot| arns[*slot].as_str()).collect();
        let (listed, _) =
            list_permission_sets(&router, &instance_arn, ACCOUNT_MID, None, None, &case).await;
        assert_reports_exactly(&listed, &expected, &case);
    }

    // A permission set a delete has removed is provisioned nowhere either.
    let kept = create_permission_set(&router, &instance_arn, "Kept").await;
    let removed = create_permission_set(&router, &instance_arn, "Removed").await;
    for permission_set_arn in [&kept, &removed] {
        assign(
            &router,
            &instance_arn,
            permission_set_arn,
            ACCOUNT_MID,
            PRINCIPAL_A,
            "deleted",
        )
        .await;
    }
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": removed}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );
    let (listed, _) = list_permission_sets(
        &router,
        &instance_arn,
        ACCOUNT_MID,
        None,
        None,
        "after DeletePermissionSet",
    )
    .await;
    assert_reports_exactly(&listed, &[kept.as_str()], "after DeletePermissionSet");
}

/// An `InstanceArn` nothing is held under is a `ResourceNotFoundException` — never an empty
/// 200, which would tell a client the instance exists and has nothing provisioned.
///
/// The STATUS is the 400 this service answers every refusal of a request with, whatever
/// 4xx the error shape's `@httpError` declares (`crate::scaffolded::modeled_error_status`),
/// so the error CODE is what tells a missing resource from the other refusals.
#[tokio::test]
async fn an_unknown_instance_is_not_found() {
    let (router, instance_arn, _engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Present").await;
    assign(
        &router,
        &instance_arn,
        &permission_set_arn,
        ACCOUNT_MID,
        PRINCIPAL_A,
        "present",
    )
    .await;

    for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
        for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
            let case = format!(
                "AccountId {account_id}, ProvisioningStatus {provisioning_status:?} on an unknown \
                 instance"
            );
            let mut request =
                json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "AccountId": account_id});
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            let (status, body) =
                call(&router, "ListPermissionSetsProvisionedToAccount", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// A `NextToken` this ENUMERATION did not issue is refused `ValidationException` rather
/// than answered with an empty page, which would end a caller's walk early. The rows span
/// a value never issued, the empty token, a token of another operation, a token of the SAME
/// operation on another account, and a token of the same account under the other
/// `ProvisioningStatus` filter.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, instance_arn, _engine) = organization_fixture();
    for slot in 0..3 {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Token{slot}")).await;
        for account_id in [ACCOUNT_MID, ACCOUNT_HIGH] {
            assign(
                &router,
                &instance_arn,
                &permission_set_arn,
                account_id,
                PRINCIPAL_A,
                "seed",
            )
            .await;
        }
    }

    /// The token a truncated first page of `request` carries.
    async fn cursor_of(router: &MultiServiceRouter, operation: &str, request: &Value) -> String {
        let (status, body) = call(router, operation, request).await;
        assert_eq!(
            status, 200,
            "the seeding page of {operation} expected 200, got {status} {body}"
        );
        body["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("a truncated page of {operation} must carry a token: {body}"))
            .to_string()
    }

    // A real token of THIS operation, but issued for the other account.
    let other_scope_token = cursor_of(
        &router,
        "ListPermissionSetsProvisionedToAccount",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": ACCOUNT_HIGH,
            "MaxResults": 1,
        }),
    )
    .await;
    // A real token of this operation on the SAME account, but of the enumeration filtered
    // by a `ProvisioningStatus`: its keys belong to a different selection of rows.
    let other_filter_token = cursor_of(
        &router,
        "ListPermissionSetsProvisionedToAccount",
        &json!({
            "InstanceArn": instance_arn,
            "AccountId": ACCOUNT_MID,
            "ProvisioningStatus": LATEST_PROVISIONED,
            "MaxResults": 1,
        }),
    )
    .await;
    // A real token of ANOTHER operation of this service: `ListPermissionSets` walks every
    // permission set of the instance, so its keys belong to a different selection.
    let other_operation_token = cursor_of(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn, "MaxResults": 1}),
    )
    .await;

    let rows: Vec<(&str, String)> = vec![
        ("a value this service never issued", "not-a-token".to_string()),
        ("the empty token Token's 0..2048 length admits", String::new()),
        ("a token of another operation", other_operation_token),
        ("a token of another account", other_scope_token),
        ("a token of the same enumeration under a filter", other_filter_token),
    ];
    for (case, token) in rows {
        let (status, body) = call(
            &router,
            "ListPermissionSetsProvisionedToAccount",
            &json!({
                "InstanceArn": instance_arn,
                "AccountId": ACCOUNT_MID,
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// The permission-set surface stays WITHDRAWN on an account instance: the operation is
/// refused `ValidationException` (HTTP 400) there, which is the restriction and no longer
/// the scaffold's 501.
#[tokio::test]
async fn the_operation_is_withdrawn_on_an_account_instance() {
    let (router, instance_arn) = account_instance_router().await;

    for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
        for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
            let case =
                format!("AccountId {account_id}, ProvisioningStatus {provisioning_status:?}");
            let mut request = json!({
                "InstanceArn": instance_arn,
                "AccountId": account_id,
            });
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            let (status, body) =
                call(&router, "ListPermissionSetsProvisionedToAccount", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 on an account instance, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// The operation is a REPORT: repeating it answers the same thing, and it leaves the stored
/// assignments — and the provisioning each of them recorded — exactly as they were, so
/// nothing a later caller sees depends on who listed.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn, engine) = organization_fixture();
    for slot in 0..3 {
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Stable{slot}")).await;
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            ACCOUNT_MID,
            PRINCIPAL_A,
            "stable",
        )
        .await;
    }

    let stored_before = with_store(&engine, |store| {
        store
            .account_assignments
            .iter()
            .map(|row| {
                (
                    row.account_id.clone(),
                    row.permission_set_arn.clone(),
                    row.principal_id.clone(),
                    row.provisioned_revision.clone(),
                )
            })
            .collect::<Vec<_>>()
    });

    let request = json!({
        "InstanceArn": instance_arn,
        "AccountId": ACCOUNT_MID,
        "MaxResults": 2,
    });
    let (status, first) = call(&router, "ListPermissionSetsProvisionedToAccount", &request).await;
    assert_eq!(
        status, 200,
        "the first read expected 200, got {status} {first}"
    );

    // Walk the whole enumeration, under every filter, so any state a page consumed would
    // show.
    for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
        let case = format!("walked under {provisioning_status:?}");
        list_permission_sets(
            &router,
            &instance_arn,
            ACCOUNT_MID,
            provisioning_status,
            Some(1),
            &case,
        )
        .await;
    }

    let (status, again) = call(&router, "ListPermissionSetsProvisionedToAccount", &request).await;
    assert_eq!(
        status, 200,
        "the repeated read expected 200, got {status} {again}"
    );
    assert_eq!(
        first, again,
        "a report must answer the same twice, got {first} then {again}"
    );

    let stored_after = with_store(&engine, |store| {
        store
            .account_assignments
            .iter()
            .map(|row| {
                (
                    row.account_id.clone(),
                    row.permission_set_arn.clone(),
                    row.principal_id.clone(),
                    row.provisioned_revision.clone(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        stored_after, stored_before,
        "a report must leave the stored assignments untouched, got {stored_after:?}"
    );
}
