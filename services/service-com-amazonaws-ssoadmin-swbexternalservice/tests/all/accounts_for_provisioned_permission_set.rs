// SPDX-License-Identifier: CC0-1.0

//! `ListAccountsForProvisionedPermissionSet`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation reports the Amazon Web Services accounts the
//! permission set the request names is provisioned into ("Lists all the Amazon Web
//! Services accounts where the specified permission set is provisioned") — each account
//! exactly once, as its `AccountId`, in ascending account order, one page per call under
//! the `@paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults,
//! items: AccountIds}` contract, with a continuation token only while accounts are left
//! over. An account holds the permission set once it has been provisioned there, which is
//! what granting an account assignment on that account does, so the number of principals
//! assigned through it changes nothing about how often the account is reported.
//!
//! THE FILTER rule: `ProvisioningStatus` selects among those accounts by the definition
//! each of them holds — `LATEST_PERMISSION_SET_PROVISIONED` for an account holding the
//! permission set's current definition, `LATEST_PERMISSION_SET_NOT_PROVISIONED` for one
//! whose copy the permission set has changed since — and a request that names no
//! `ProvisioningStatus` is not filtered.
//!
//! A permission set provisioned nowhere is an EMPTY page, while an `InstanceArn` or
//! `PermissionSetArn` nothing is held under is a `ResourceNotFoundException`, a
//! `NextToken` this enumeration did not issue is a `ValidationException`, and the whole
//! permission-set surface stays withdrawn on an account instance. Being a report, it
//! changes nothing a later caller sees.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the NUMBER of provisioned accounts (none, one, several), the account ids at both
//! ends of `AccountId`'s own `^\d{12}$` class, the number of principals assigned on one
//! account, the order the assignments were granted in, every `MaxResults` the model's
//! `range(min: 1, max: 100)` admits around the collection size, and every KIND of change
//! to a permission set the implemented API can make.
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
    call, error_code, organization_engine, router, router_over, seeded_instance_arn, with_store,
    UNKNOWN_INSTANCE_ARN, UNKNOWN_PERMISSION_SET_ARN,
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

/// Walk the whole enumeration, following `NextToken`, and return every `AccountId`
/// returned together with the number of pages it took.
async fn list_accounts(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
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
            "PermissionSetArn": permission_set_arn,
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
        let (status, body) = call(router, "ListAccountsForProvisionedPermissionSet", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAccountsForProvisionedPermissionSet expected 200, got {status} \
             {body}"
        );
        let page: Vec<String> = body["AccountIds"]
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
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page must hold at most MaxResults {limit} accounts, got {}",
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

/// Assert `listed` is exactly `expected`, each account once, in ascending account order.
fn assert_reports_exactly(listed: &[String], expected: &[&str], case: &str) {
    let mut want: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
    want.sort();
    want.dedup();
    assert_eq!(
        listed, &want[..],
        "case {case:?}: expected exactly {want:?} in account order, got {listed:?}"
    );
    assert!(
        listed.windows(2).all(|pair| pair[0] < pair[1]),
        "case {case:?}: the enumeration must be strictly ascending by AccountId, got {listed:?}"
    );
    for account in listed {
        assert!(
            account.len() == 12 && account.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: AccountId must be 12 digits, got {account:?}"
        );
    }
}

/// One row of the reporting table: the assignments to grant, in order, and the accounts
/// the permission set is then provisioned into.
struct ProvisionCase {
    label: &'static str,
    assignments: Vec<(&'static str, &'static str)>,
    expected: Vec<&'static str>,
}

/// THE RULE: every account the permission set is provisioned into is reported once, in
/// account order — over a table spanning the number of accounts, the ends of the
/// `AccountId` class, the number of principals assigned on one account, and the order the
/// assignments were granted in.
#[tokio::test]
async fn every_provisioned_account_is_reported_once_in_account_order() {
    let rows: Vec<ProvisionCase> = vec![
        ProvisionCase {
            label: "a permission set provisioned nowhere",
            assignments: vec![],
            expected: vec![],
        },
        ProvisionCase {
            label: "one account, one principal",
            assignments: vec![(ACCOUNT_MID, PRINCIPAL_A)],
            expected: vec![ACCOUNT_MID],
        },
        ProvisionCase {
            label: "one account, three principals",
            assignments: vec![
                (ACCOUNT_MID, PRINCIPAL_A),
                (ACCOUNT_MID, PRINCIPAL_B),
                (ACCOUNT_MID, PRINCIPAL_C),
            ],
            expected: vec![ACCOUNT_MID],
        },
        ProvisionCase {
            label: "three accounts, granted in account order",
            assignments: vec![
                (ACCOUNT_LOW, PRINCIPAL_A),
                (ACCOUNT_MID, PRINCIPAL_A),
                (ACCOUNT_HIGH, PRINCIPAL_A),
            ],
            expected: vec![ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH],
        },
        ProvisionCase {
            label: "three accounts, granted in the reverse order",
            assignments: vec![
                (ACCOUNT_HIGH, PRINCIPAL_A),
                (ACCOUNT_MID, PRINCIPAL_A),
                (ACCOUNT_LOW, PRINCIPAL_A),
            ],
            expected: vec![ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH],
        },
        ProvisionCase {
            label: "both ends of the AccountId range, several principals interleaved",
            assignments: vec![
                (ACCOUNT_HIGH, PRINCIPAL_B),
                (ACCOUNT_LOW, PRINCIPAL_A),
                (ACCOUNT_HIGH, PRINCIPAL_A),
                (ACCOUNT_LOW, PRINCIPAL_C),
            ],
            expected: vec![ACCOUNT_LOW, ACCOUNT_HIGH],
        },
        ProvisionCase {
            // The same assignment granted twice grants it once, so the account it
            // provisioned is still one account.
            label: "the same assignment granted twice",
            assignments: vec![(ACCOUNT_MID, PRINCIPAL_A), (ACCOUNT_MID, PRINCIPAL_A)],
            expected: vec![ACCOUNT_MID],
        },
    ];

    for (index, case) in rows.iter().enumerate() {
        let label = case.label;
        let (router, instance_arn, _engine) = organization_fixture();
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Row{index}")).await;
        // A second permission set, provisioned somewhere else entirely, so a row that
        // escaped the permission-set filter would show.
        let other = create_permission_set(&router, &instance_arn, &format!("Other{index}")).await;
        assign(&router, &instance_arn, &other, "555555555555", PRINCIPAL_A, label).await;

        for (account_id, principal_id) in &case.assignments {
            assign(
                &router,
                &instance_arn,
                &permission_set_arn,
                account_id,
                principal_id,
                label,
            )
            .await;
        }

        let (listed, pages) =
            list_accounts(&router, &instance_arn, &permission_set_arn, None, None, label).await;
        assert_reports_exactly(&listed, &case.expected, label);
        assert_eq!(
            pages, 1,
            "case {label:?}: {} accounts fit one default page, got {pages} pages",
            case.expected.len()
        );
    }
}

/// A permission set provisioned nowhere answers an EMPTY page with no continuation token —
/// never a refusal, and never a page belonging to another permission set.
#[tokio::test]
async fn a_permission_set_provisioned_nowhere_answers_an_empty_page() {
    let (router, instance_arn, _engine) = organization_fixture();
    let empty = create_permission_set(&router, &instance_arn, "Empty").await;
    let filled = create_permission_set(&router, &instance_arn, "Filled").await;
    for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
        assign(&router, &instance_arn, &filled, account_id, PRINCIPAL_A, "filled").await;
    }

    for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
        for max_results in [None, Some(1), Some(100)] {
            let case = format!("ProvisioningStatus {provisioning_status:?}, MaxResults {max_results:?}");
            let mut request = json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": empty,
            });
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            let (status, body) =
                call(&router, "ListAccountsForProvisionedPermissionSet", &request).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
            assert_eq!(
                body["AccountIds"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: a permission set provisioned nowhere lists no account, got {body}"
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
/// exactly the provisioned accounts, once each, in the same account order, and the last
/// page carries no token.
#[tokio::test]
async fn pagination_walks_every_account_exactly_once_for_any_page_size() {
    // Six accounts, so several page sizes cut the collection in different places.
    let accounts: Vec<String> = (0..6).map(|index| format!("{:012}", index * 111_111)).collect();
    let expected: Vec<&str> = accounts.iter().map(String::as_str).collect();
    // `MaxResults` is `range(min: 1, max: 100)`: both published bounds, every size around
    // the collection's own, and one past it.
    let page_sizes = [
        None,
        Some(1),
        Some(2),
        Some(4),
        Some(accounts.len() as i64 - 1),
        Some(accounts.len() as i64),
        Some(accounts.len() as i64 + 1),
        Some(100),
    ];

    for max_results in page_sizes {
        let case = format!("MaxResults {max_results:?}");
        let (router, instance_arn, _engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Paged").await;
        for account_id in &accounts {
            assign(
                &router,
                &instance_arn,
                &permission_set_arn,
                account_id,
                PRINCIPAL_A,
                &case,
            )
            .await;
        }

        let (listed, pages) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            None,
            max_results,
            &case,
        )
        .await;
        assert_reports_exactly(&listed, &expected, &case);

        let expected_pages = match max_results {
            Some(limit) => {
                let limit = limit.clamp(1, 100) as usize;
                accounts.len().div_ceil(limit)
            }
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} accounts at that page size take {expected_pages} pages, got {pages}",
            accounts.len()
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

/// THE FILTER RULE: `ProvisioningStatus` selects a provisioned account by the definition
/// it holds — the permission set's current one, or a copy a later change left behind — and
/// a provisioning that follows the change brings that account back up to date. Asserted
/// over every kind of change to a permission set the API can make.
#[tokio::test]
async fn the_provisioning_status_filter_selects_by_the_definition_an_account_holds() {
    for case in change_cases() {
        let label = case.label;
        let (router, instance_arn, _engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Filtered").await;
        let provisioned = [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH];
        for account_id in provisioned {
            assign(
                &router,
                &instance_arn,
                &permission_set_arn,
                account_id,
                PRINCIPAL_A,
                label,
            )
            .await;
        }

        // Before any change, every provisioned account holds the permission set's current
        // definition.
        let (latest, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&latest, &provisioned, &format!("{label}: before the change"));
        let (stale, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&stale, &[], &format!("{label}: before the change, stale"));

        // The change itself, over the wire.
        let mut request = json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
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

        // Every account now holds a copy the change left behind: it is still provisioned —
        // the unfiltered enumeration reports it — but no longer with the latest definition.
        let (latest, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&latest, &[], &format!("{label}: after the change, latest"));
        let (stale, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(&stale, &provisioned, &format!("{label}: after the change, stale"));
        let (all, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            None,
            None,
            label,
        )
        .await;
        assert_reports_exactly(&all, &provisioned, &format!("{label}: after the change, unfiltered"));

        // A provisioning that follows the change puts the current definition into that one
        // account, and leaves the others where they were.
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            ACCOUNT_MID,
            PRINCIPAL_B,
            label,
        )
        .await;
        let (latest, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(
            &latest,
            &[ACCOUNT_MID],
            &format!("{label}: after re-provisioning, latest"),
        );
        let (stale, _) = list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            Some(LATEST_NOT_PROVISIONED),
            None,
            label,
        )
        .await;
        assert_reports_exactly(
            &stale,
            &[ACCOUNT_LOW, ACCOUNT_HIGH],
            &format!("{label}: after re-provisioning, stale"),
        );
    }
}

/// The other edge of the rule: an account stops being reported when the permission set is
/// no longer provisioned there — which is when the LAST assignment of it on that account is
/// withdrawn, not when the first one is. Asserted over an account carrying several
/// principals and over accounts withdrawn in both orders.
#[tokio::test]
async fn an_account_is_reported_until_its_last_assignment_is_withdrawn() {
    let (router, instance_arn, _engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Withdrawn").await;
    // ACCOUNT_MID carries three principals, ACCOUNT_LOW one, ACCOUNT_HIGH two.
    let granted: Vec<(&str, &str)> = vec![
        (ACCOUNT_MID, PRINCIPAL_A),
        (ACCOUNT_MID, PRINCIPAL_B),
        (ACCOUNT_MID, PRINCIPAL_C),
        (ACCOUNT_LOW, PRINCIPAL_A),
        (ACCOUNT_HIGH, PRINCIPAL_A),
        (ACCOUNT_HIGH, PRINCIPAL_B),
    ];
    for (account_id, principal_id) in &granted {
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            account_id,
            principal_id,
            "granted",
        )
        .await;
    }

    // (the assignment to withdraw, the accounts still provisioned afterwards)
    let steps: Vec<((&str, &str), Vec<&str>)> = vec![
        // One of three principals: the account still holds the permission set.
        (
            (ACCOUNT_MID, PRINCIPAL_B),
            vec![ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH],
        ),
        // The ONLY principal of an account: that account is deprovisioned.
        ((ACCOUNT_LOW, PRINCIPAL_A), vec![ACCOUNT_MID, ACCOUNT_HIGH]),
        (
            (ACCOUNT_HIGH, PRINCIPAL_A),
            vec![ACCOUNT_MID, ACCOUNT_HIGH],
        ),
        // The last principal of that account, in the reverse order of granting.
        ((ACCOUNT_HIGH, PRINCIPAL_B), vec![ACCOUNT_MID]),
        ((ACCOUNT_MID, PRINCIPAL_C), vec![ACCOUNT_MID]),
        // Nothing is provisioned anywhere once the last assignment is gone.
        ((ACCOUNT_MID, PRINCIPAL_A), vec![]),
    ];
    for ((account_id, principal_id), expected) in steps {
        let case = format!("withdrawn {principal_id} on {account_id}");
        unassign(
            &router,
            &instance_arn,
            &permission_set_arn,
            account_id,
            principal_id,
            &case,
        )
        .await;
        let (listed, _) =
            list_accounts(&router, &instance_arn, &permission_set_arn, None, None, &case).await;
        assert_reports_exactly(&listed, &expected, &case);
    }
}

/// A resource nothing is held under is a `ResourceNotFoundException` — never an empty 200,
/// which would tell a client the permission set exists and is provisioned nowhere. The
/// rows span the unknown instance, the unknown permission set, a permission set of another
/// instance and one a `DeletePermissionSet` has removed.
///
/// The STATUS is the 400 this service answers every refusal of a request with, whatever
/// 4xx the error shape's `@httpError` declares (`crate::scaffolded::modeled_error_status`),
/// so the error CODE is what tells a missing resource from the other refusals.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
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

    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    assign(&router, &instance_arn, &deleted, ACCOUNT_MID, PRINCIPAL_A, "deleted").await;
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": deleted}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );

    let rows: Vec<(&str, String, String)> = vec![
        (
            "an InstanceArn no instance is held under",
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
        (
            "a PermissionSetArn that was never minted",
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "both unknown",
            UNKNOWN_INSTANCE_ARN.to_string(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "a permission set a delete has removed",
            instance_arn.clone(),
            deleted,
        ),
    ];
    for (case, instance, permission_set) in rows {
        for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
            let case = format!("{case}, ProvisioningStatus {provisioning_status:?}");
            let mut request =
                json!({"InstanceArn": instance, "PermissionSetArn": permission_set});
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            let (status, body) =
                call(&router, "ListAccountsForProvisionedPermissionSet", &request).await;
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
/// operation on another permission set, and a token of the same permission set under the
/// other `ProvisioningStatus` filter.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, instance_arn, _engine) = organization_fixture();
    let first = create_permission_set(&router, &instance_arn, "First").await;
    let second = create_permission_set(&router, &instance_arn, "Second").await;
    for permission_set_arn in [&first, &second] {
        for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
            assign(
                &router,
                &instance_arn,
                permission_set_arn,
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

    // A real token of THIS operation, but issued for the other permission set.
    let other_scope_token = cursor_of(
        &router,
        "ListAccountsForProvisionedPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": second,
            "MaxResults": 1,
        }),
    )
    .await;
    // A real token of this operation on the SAME permission set, but of the enumeration
    // filtered by a `ProvisioningStatus`: its keys belong to a different selection of rows.
    let other_filter_token = cursor_of(
        &router,
        "ListAccountsForProvisionedPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": first,
            "ProvisioningStatus": LATEST_PROVISIONED,
            "MaxResults": 1,
        }),
    )
    .await;
    // A real token of ANOTHER operation of this service: `ListPermissionSets` pages by
    // permission-set ARN, so its keys mean something else.
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
        ("a token of another permission set", other_scope_token),
        ("a token of the same enumeration under a filter", other_filter_token),
    ];
    for (case, token) in rows {
        let (status, body) = call(
            &router,
            "ListAccountsForProvisionedPermissionSet",
            &json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": first,
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
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
        for max_results in [None, Some(1), Some(100)] {
            let case =
                format!("ProvisioningStatus {provisioning_status:?}, MaxResults {max_results:?}");
            let mut request = json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
            });
            if let Some(status) = provisioning_status {
                request["ProvisioningStatus"] = json!(status);
            }
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            let (status, body) =
                call(&router, "ListAccountsForProvisionedPermissionSet", &request).await;
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

/// The operation is a REPORT: repeating it answers the same thing, and it leaves the
/// stored assignments — and the provisioning each of them recorded — exactly as they were,
/// so nothing a later caller sees depends on who listed.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Stable").await;
    for account_id in [ACCOUNT_LOW, ACCOUNT_MID, ACCOUNT_HIGH] {
        assign(
            &router,
            &instance_arn,
            &permission_set_arn,
            account_id,
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
                    row.principal_id.clone(),
                    row.provisioned_revision.clone(),
                )
            })
            .collect::<Vec<_>>()
    });

    let request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "MaxResults": 2,
    });
    let (status, first) = call(&router, "ListAccountsForProvisionedPermissionSet", &request).await;
    assert_eq!(status, 200, "the first read expected 200, got {status} {first}");

    // Walk the whole enumeration, under every filter, so any state a page consumed would
    // show.
    for provisioning_status in [None, Some(LATEST_PROVISIONED), Some(LATEST_NOT_PROVISIONED)] {
        let case = format!("walked under {provisioning_status:?}");
        list_accounts(
            &router,
            &instance_arn,
            &permission_set_arn,
            provisioning_status,
            Some(1),
            &case,
        )
        .await;
    }

    let (status, again) = call(&router, "ListAccountsForProvisionedPermissionSet", &request).await;
    assert_eq!(status, 200, "the repeated read expected 200, got {status} {again}");
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
