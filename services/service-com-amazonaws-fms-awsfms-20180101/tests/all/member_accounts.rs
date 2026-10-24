// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMemberAccounts`: the administrator gate that decides
//! whether an organization-scoped call is answered at all, and the page of
//! organization accounts it reports when it is.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the WIRE
//! response.
//!
//! The rules asserted here:
//!
//! 1. Firewall Manager answers an organization-scoped call only for a Firewall
//!    Manager administrator or the organization's management account; with no
//!    administrator association the caller is neither, and the call is refused with
//!    HTTP 400 `AccessDeniedException` — whatever else the request carries.
//! 2. When it is answered, the reported member accounts are exactly the accounts of
//!    the administrator's organization the store records (the associated
//!    administrator, plus every `AWSAccountId` a stored policy names under the
//!    `ACCOUNT` key of its `IncludeMap` / `ExcludeMap`), each once, in ascending
//!    account-id order.
//! 3. The page is bounded by `MaxResults` and by the model's maximum for it, and the
//!    `NextToken` round-trips to the next page.
//!
//! Each rule is asserted over a TABLE: the rows differ in account-id length and
//! digits, in which scope map and which scope key names the accounts, in how many
//! accounts there are, and in the page size.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::policy_lifecycle::{call, call_in, emulator, error_code};

/// The largest page the operation serves, which is the maximum the model's
/// `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults` (range
/// 1..100, from `smithy_query(query="operation", operation="ListMemberAccounts")`).
const MAX_PAGE_SIZE: usize = 100;

/// The `CustomerPolicyScopeIdType` keys of a policy's `IncludeMap` / `ExcludeMap`,
/// from the model's own catalog (`ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES`
/// for `com.amazonaws.fms#CustomerPolicyScopeIdType`): the accounts live under
/// `ACCOUNT`, organizational units under `ORG_UNIT`.
const SCOPE_KEYS: &[&str] =
    service_com_amazonaws_fms_awsfms_20180101::ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES;

/// The `message` member of an awsJson1_1 error envelope.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A policy document carrying the given scope map(s). Only the members `PutPolicy`
/// requires, plus the maps under test, so a row differs from the others ONLY in its
/// scope maps.
///
/// A row names at most ONE of the two maps: "You can specify inclusions or
/// exclusions, but not both" (`Policy$IncludeMap` / `Policy$ExcludeMap`), so a
/// document carrying both is refused — the rule the `policy_document_validation`
/// suite asserts. The accounts a policy contributes are therefore spread across
/// SEVERAL stored policies here, which is also what makes the aggregation across
/// records visible.
fn policy_with(name: &str, include: Option<Value>, exclude: Option<Value>) -> Value {
    let mut policy = json!({
        "PolicyName": name,
        "SecurityServicePolicyData": {
            "Type": "WAFV2",
            "ManagedServiceData": "{\"type\":\"WAFV2\"}"
        },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    });
    if let Some(include) = include {
        policy["IncludeMap"] = include;
    }
    if let Some(exclude) = exclude {
        policy["ExcludeMap"] = exclude;
    }
    policy
}

/// Store one policy, asserting the create itself succeeded.
async fn put_policy(router: &smithy_server_runtime::MultiServiceRouter, policy: &Value) {
    let (status, body) = call(router, "PutPolicy", &json!({ "Policy": policy })).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected PutPolicy to answer 200, got {status} with {body}",
        policy["PolicyName"]
    );
}

/// Read every page of `ListMemberAccounts`, following `NextToken`, and return the
/// accounts in the order they were served together with each page's length.
///
/// Asserts the page bound and the token invariant on every page: a page is never
/// larger than the effective bound, and a `NextToken` appears only while accounts
/// remain.
async fn page_through(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let bound = max_results.unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE);
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // Enough iterations for the largest table below at the smallest page size, so a
    // token that never clears fails here instead of looping forever.
    for page in 0..1_000 {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListMemberAccounts", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} must answer 200, got {status} with {body}"
        );
        let accounts = body["MemberAccounts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: page {page} carries no MemberAccounts: {body}"))
            .iter()
            .map(|account| {
                account
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: page {page} holds a non-string account: {body}")
                    })
                    .to_string()
            })
            .collect::<Vec<String>>();
        assert!(
            accounts.len() <= bound,
            "case {case}: page {page} must hold at most {bound} accounts, got {}",
            accounts.len()
        );
        served.extend(accounts);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return served;
        }
        assert_eq!(
            served.len() % bound,
            0,
            "case {case}: page {page} carried a NextToken after a short page of {} \
             accounts (served {} so far)",
            served.len() % bound,
            served.len()
        );
    }
    panic!("case {case}: NextToken never cleared after 1000 pages");
}

/// Rule 1: no administrator association, no organization-scoped answer — and the
/// refusal does not depend on what else the request carries.
#[tokio::test]
async fn list_member_accounts_is_refused_until_an_administrator_is_associated() {
    // Request shapes spanning everything the operation accepts: no arguments, each
    // published `MaxResults` bound, and a `NextToken` this service never minted (so
    // the gate is shown to run BEFORE the token is judged).
    let shapes = [
        ("no-arguments", json!({})),
        ("max-results-at-the-minimum", json!({ "MaxResults": 1 })),
        ("max-results-at-the-maximum", json!({ "MaxResults": 100 })),
        (
            "foreign-next-token",
            json!({ "NextToken": "not.a.token.this.service.minted" }),
        ),
    ];
    // Account ids differing in length and digits: the model constrains
    // `AWSAccountId` only to `^[0-9]+$`, length 1..1024.
    for admin in ["000000000000", "123456789012", "7"] {
        let router = emulator();

        for (shape, request) in &shapes {
            let (status, body) = call(&router, "ListMemberAccounts", request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {admin}/{shape}: with no administrator association the call must be \
                 refused, got {status} with {body}"
            );
        }

        // The refusal's message is built from the request: it names the calling
        // account and the Region the request was signed for, so it moves with the
        // Region rather than being a constant.
        for region in ["us-east-1", "eu-central-1", "ap-southeast-4"] {
            let (status, body) =
                call_in(&router, "ListMemberAccounts", &json!({}), region).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {admin}/{region}: expected the authorization refusal, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "No default admin could be found for account {} in Region {region}",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "case {admin}/{region}: the refusal must name the calling account and the \
                 Region of the request, got {body}"
            );
        }

        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": admin }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {admin}: expected AssociateAdminAccount to answer 200, got {status} with {body}"
        );

        for (shape, request) in &shapes {
            let (status, body) = call(&router, "ListMemberAccounts", request).await;
            if *shape == "foreign-next-token" {
                // The gate is open, so the request's own shape is judged now: a
                // token this service did not mint names no page.
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {admin}/{shape}: a foreign NextToken must be refused as invalid, \
                     got {status} with {body}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {admin}/{shape}: an associated administrator must be answered, \
                 got {status} with {body}"
            );
            // The administrator account is itself an account of the organization
            // (`AssociateAdminAccount`: "This account must be a member account of
            // the organization"), and the store records nothing else yet.
            assert_eq!(
                body["MemberAccounts"],
                json!([admin]),
                "case {admin}/{shape}: expected exactly the associated administrator, \
                 got {}",
                body["MemberAccounts"]
            );
            assert!(
                body.get("NextToken").is_none(),
                "case {admin}/{shape}: a single-account last page must carry no NextToken, \
                 got {body}"
            );
        }
    }
}

/// One row of the scope-map table: the policy to store, and the accounts storing it
/// adds to the organization.
struct Row {
    name: &'static str,
    include: Option<Value>,
    exclude: Option<Value>,
    adds: &'static [&'static str],
}

/// Rule 2: the reported accounts are exactly the ones the store records, deduped and
/// in ascending order — asserted after every row, so each row's contribution is
/// visible on its own.
#[tokio::test]
async fn list_member_accounts_reports_the_accounts_the_store_records() {
    // `SCOPE_KEYS` is the model's catalog; the rows below use both of its entries,
    // and only `ACCOUNT` may contribute accounts.
    assert_eq!(
        SCOPE_KEYS.to_vec(),
        vec!["ACCOUNT", "ORG_UNIT"],
        "the model's CustomerPolicyScopeIdType catalog changed: {SCOPE_KEYS:?}"
    );
    let admin = "123456789012";
    let rows = [
        Row {
            name: "no-scope-maps",
            include: None,
            exclude: None,
            adds: &[],
        },
        Row {
            name: "included-accounts",
            include: Some(json!({ "ACCOUNT": ["111111111111", "222222222222"] })),
            exclude: None,
            adds: &["111111111111", "222222222222"],
        },
        Row {
            name: "excluded-accounts",
            include: None,
            exclude: Some(json!({ "ACCOUNT": ["333333333333"] })),
            adds: &["333333333333"],
        },
        Row {
            name: "organizational-units-only",
            include: Some(json!({ "ORG_UNIT": ["ou-abc1-12345678"] })),
            exclude: None,
            adds: &[],
        },
        Row {
            name: "both-keys-in-the-include-map",
            include: Some(json!({ "ACCOUNT": ["444444444444"], "ORG_UNIT": ["ou-abc1-87654321"] })),
            exclude: None,
            adds: &["444444444444"],
        },
        Row {
            name: "both-keys-in-the-exclude-map",
            include: None,
            exclude: Some(json!({ "ACCOUNT": ["7"], "ORG_UNIT": ["ou-abc1-11112222"] })),
            // "7" is an `AWSAccountId` at its minimum length (1..1024, `^[0-9]+$`).
            adds: &["7"],
        },
        Row {
            name: "values-that-are-not-account-ids",
            // A `CustomerPolicyScopeId` is a free-form string, so a non-account id
            // can appear under ACCOUNT; `MemberAccounts` may only carry
            // `AWSAccountId`s, so those values are not accounts.
            include: Some(json!({ "ACCOUNT": ["not-an-account-id", "555555555555", "12_34"] })),
            exclude: None,
            adds: &["555555555555"],
        },
        Row {
            name: "included-accounts-another-row-already-named",
            include: Some(json!({ "ACCOUNT": ["111111111111", "7"] })),
            exclude: None,
            adds: &[],
        },
        Row {
            name: "excluded-accounts-another-row-already-named",
            include: None,
            exclude: Some(json!({ "ACCOUNT": ["333333333333"] })),
            adds: &[],
        },
    ];

    let router = emulator();
    call(
        &router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": admin }),
    )
    .await;
    // The administrator is an account of its own organization, so it is the
    // expectation every row grows from.
    let mut expected: BTreeSet<String> = BTreeSet::from([admin.to_string()]);

    for row in &rows {
        put_policy(
            &router,
            &policy_with(row.name, row.include.clone(), row.exclude.clone()),
        )
        .await;
        expected.extend(row.adds.iter().map(|account| account.to_string()));

        let (status, body) = call(&router, "ListMemberAccounts", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {}: expected ListMemberAccounts to answer 200, got {status} with {body}",
            row.name
        );
        let expected_order: Vec<&String> = expected.iter().collect();
        assert_eq!(
            body["MemberAccounts"],
            json!(expected_order),
            "case {}: expected {expected_order:?} in ascending account-id order, got {}",
            row.name,
            body["MemberAccounts"]
        );

        // A read changes nothing a later caller sees.
        let (repeat_status, repeat) = call(&router, "ListMemberAccounts", &json!({})).await;
        assert_eq!(
            (repeat_status, &repeat),
            (status, &body),
            "case {}: repeating the read must answer identically, got {repeat_status} with \
             {repeat}",
            row.name
        );
    }
}

/// Rule 3: the page is bounded and the token round-trips, for page sizes spanning
/// the model's range and for a caller that sends no `MaxResults` at all.
#[tokio::test]
async fn the_member_account_page_is_bounded_and_the_token_round_trips() {
    let admin = "900000000001";
    // More accounts than one page holds, so a default page genuinely truncates and
    // the assertions below cannot pass vacuously. Carried by two policies, so the
    // set spans more than one stored record.
    let first: Vec<String> = (1..=90).map(|n| format!("{n:012}")).collect();
    let second: Vec<String> = (91..=150).map(|n| format!("{n:012}")).collect();

    let router = emulator();
    call(
        &router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": admin }),
    )
    .await;
    // The halves are carried by different scope MAPS as well as by different
    // policies: one `IncludeMap` (with an `ORG_UNIT` key beside its accounts) and
    // one `ExcludeMap`, since no single policy may carry both.
    put_policy(
        &router,
        &policy_with(
            "first-half",
            Some(json!({ "ACCOUNT": first, "ORG_UNIT": ["ou-abc1-12345678"] })),
            None,
        ),
    )
    .await;
    put_policy(
        &router,
        &policy_with("second-half", None, Some(json!({ "ACCOUNT": second }))),
    )
    .await;

    let mut expected: BTreeSet<String> = BTreeSet::from([admin.to_string()]);
    expected.extend((1..=150).map(|n| format!("{n:012}")));
    let expected_order: Vec<String> = expected.iter().cloned().collect();
    assert_eq!(
        expected_order.len(),
        151,
        "the table must span more than one page, got {} accounts",
        expected_order.len()
    );

    // Page sizes: the model's minimum, an odd size that does not divide the total,
    // the model's maximum, and no `MaxResults` at all.
    for max_results in [Some(1), Some(7), Some(100), None] {
        let case = format!("max-results-{max_results:?}");
        let served = page_through(&router, &case, max_results).await;
        assert_eq!(
            served, expected_order,
            "case {case}: the pages together must serve every account once, in order"
        );
    }

    // The default page is bounded by the model's maximum even though the caller
    // asked for nothing.
    let (status, body) = call(&router, "ListMemberAccounts", &json!({})).await;
    assert_eq!(
        status, 200,
        "case default-page: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["MemberAccounts"].as_array().map(Vec::len),
        Some(MAX_PAGE_SIZE),
        "case default-page: expected {MAX_PAGE_SIZE} accounts, got {}",
        body["MemberAccounts"]
    );
    let token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case default-page: expected a NextToken, got {body}"))
        .to_string();
    let (status, resumed) = call(
        &router,
        "ListMemberAccounts",
        &json!({ "NextToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case default-page: resuming from the token must answer 200, got {status} with {resumed}"
    );
    assert_eq!(
        resumed["MemberAccounts"],
        json!(expected_order[MAX_PAGE_SIZE..].to_vec()),
        "case default-page: the token must resume exactly after the first page, got {}",
        resumed["MemberAccounts"]
    );

    // One past each published `MaxResults` bound is outside the modelled range, so
    // the request is refused rather than served an unbounded or empty page.
    for out_of_range in [0, 101] {
        let (status, body) = call(
            &router,
            "ListMemberAccounts",
            &json!({ "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case max-results-{out_of_range}: a MaxResults outside the modelled 1..100 range \
             must be refused, got {status} with {body}"
        );
    }
}
