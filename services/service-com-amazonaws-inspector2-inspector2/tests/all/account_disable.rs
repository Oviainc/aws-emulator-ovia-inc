// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Disable`, read back through `Enable`,
//! `BatchGetAccountStatus` and `ListUsageTotals`.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.
//!
//! The rule under test: `Disable` turns off exactly the scan types the request names —
//! every modelled scan type when it names none — for each account in its scope (the
//! accounts `accountIds` names, or the calling account when it names none), leaving the
//! account-level status ENABLED while any scan type is still on and DISABLED once none
//! is, and every later read reports what it stored.

use serde_json::{json, Value};

use crate::filters_and_enablement::{post_in, router};

/// `Disable`'s route.
/// Source: `smithy_query(query="operation", operation="Disable")` — `POST /disable`.
const DISABLE_URI: &str = "/disable";

/// `Enable`'s route, which is how an account's scan types are turned on.
/// Source: `smithy_query(query="operation", operation="Enable")` — `POST /enable`.
const ENABLE_URI: &str = "/enable";

/// `BatchGetAccountStatus`'s route.
/// Source: `smithy_query(query="operation", operation="BatchGetAccountStatus")` —
/// `POST /status/batch/get`.
const STATUS_URI: &str = "/status/batch/get";

/// `ListUsageTotals`'s route, which reports one row per account this service holds an
/// Amazon Inspector record for.
/// Source: `smithy_query(query="operation", operation="ListUsageTotals")` —
/// `POST /usage/list`.
const USAGE_URI: &str = "/usage/list";

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const CALLER: &str = "000000000000";

/// Every `ResourceScanType` token, in model order.
/// Source: `smithy_query(query="shape", shape="ResourceScanType")`.
const RESOURCE_SCAN_TYPES: [&str; 5] = ["EC2", "ECR", "LAMBDA", "LAMBDA_CODE", "CODE_REPOSITORY"];

/// The `ResourceStatus`/`ResourceState` member each `ResourceScanType` is reported under.
/// Source: `smithy_query(query="shape", shape="ResourceStatus,ResourceState")`.
fn resource_member(resource_type: &str) -> &'static str {
    match resource_type {
        "EC2" => "ec2",
        "ECR" => "ecr",
        "LAMBDA" => "lambda",
        "LAMBDA_CODE" => "lambdaCode",
        "CODE_REPOSITORY" => "codeRepository",
        other => panic!("unmodelled ResourceScanType {other:?}"),
    }
}

/// POST a restJson1 request from `us-east-1`.
async fn post(
    router: &smithy_server_runtime::MultiServiceRouter,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _, parsed) = post_in(router, "us-east-1", uri, body).await;
    (status, parsed)
}

/// One row of the table: the scan types the account is enabled for, and the
/// `resourceTypes` the `Disable` call names — `None` for a request that leaves the
/// optional member out entirely.
#[derive(Debug, Clone, Copy)]
struct Case {
    enabled: &'static [&'static str],
    requested: Option<&'static [&'static str]>,
}

/// The scan types a `Disable` carrying `requested` turns off: the ones it names, and every
/// modelled type when it names none, since `DisableResourceTypeList` is a 0..5 list on an
/// optional member and turning off every scan type turns Amazon Inspector itself off.
fn disabled_types(requested: Option<&[&str]>) -> Vec<&'static str> {
    match requested {
        Some(types) if !types.is_empty() => RESOURCE_SCAN_TYPES
            .into_iter()
            .filter(|resource_type| types.contains(resource_type))
            .collect(),
        _ => RESOURCE_SCAN_TYPES.to_vec(),
    }
}

/// The `Status` each scan type is left in, and the account-level `Status` that follows
/// from them.
fn expected(case: Case) -> (Vec<(&'static str, &'static str)>, &'static str) {
    let turned_off = disabled_types(case.requested);
    let per_type: Vec<(&'static str, &'static str)> = RESOURCE_SCAN_TYPES
        .into_iter()
        .map(|resource_type| {
            let on = case.enabled.contains(&resource_type) && !turned_off.contains(&resource_type);
            (resource_type, if on { "ENABLED" } else { "DISABLED" })
        })
        .collect();
    let account_status = if per_type.iter().any(|(_, status)| *status == "ENABLED") {
        "ENABLED"
    } else {
        "DISABLED"
    };
    (per_type, account_status)
}

/// `Disable` turns off exactly the scan types the request names — all of them when it
/// names none — and the account-level status follows the scan types that are left on. What
/// it reports is what `BatchGetAccountStatus` reads back.
#[tokio::test]
async fn disable_turns_off_exactly_the_resource_types_requested() {
    // Rows differ along WHICH and HOW MANY scan types are enabled and named, along the
    // form of `resourceTypes` (a list, an empty list, the member absent), and along
    // whether any scan type survives the call.
    let cases = [
        Case {
            enabled: &RESOURCE_SCAN_TYPES,
            requested: Some(&["EC2"]),
        },
        Case {
            enabled: &["EC2", "ECR"],
            requested: Some(&["ECR"]),
        },
        Case {
            enabled: &["EC2"],
            requested: Some(&["EC2"]),
        },
        Case {
            enabled: &RESOURCE_SCAN_TYPES,
            requested: Some(&RESOURCE_SCAN_TYPES),
        },
        // A scan type that was never on: the call turns nothing off for it, and the type
        // that IS on stays on.
        Case {
            enabled: &["CODE_REPOSITORY"],
            requested: Some(&["LAMBDA"]),
        },
        Case {
            enabled: &["LAMBDA", "LAMBDA_CODE"],
            requested: Some(&["LAMBDA", "LAMBDA_CODE"]),
        },
        // `resourceTypes` absent, and an explicitly empty list: naming no scan type means
        // the whole service.
        Case {
            enabled: &["EC2", "ECR", "LAMBDA"],
            requested: None,
        },
        Case {
            enabled: &["EC2", "ECR"],
            requested: Some(&[]),
        },
    ];

    for case in cases {
        let router = router();
        let (status, enabled) =
            post(&router, ENABLE_URI, &json!({ "resourceTypes": case.enabled })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: Enable: expected 200, got {status} / {enabled}"
        );

        let request = match case.requested {
            Some(types) => json!({ "resourceTypes": types }),
            None => json!({}),
        };
        let (status, disabled) = post(&router, DISABLE_URI, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: Disable: expected 200, got {status} / {disabled}"
        );
        assert_eq!(
            disabled["accounts"][0]["accountId"], CALLER,
            "case {case:?}: Disable must default to the calling account, got {disabled}"
        );
        assert!(
            disabled["failedAccounts"].is_null(),
            "case {case:?}: no account failed, so failedAccounts must be absent, got {disabled}"
        );

        let (per_type, account_status) = expected(case);
        assert_eq!(
            disabled["accounts"][0]["status"], account_status,
            "case {case:?}: Disable's account status: expected {account_status}, got {disabled}"
        );

        let (status, after) = post(&router, STATUS_URI, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetAccountStatus: expected 200, got {status} / {after}"
        );
        assert_eq!(
            after["accounts"][0]["state"]["status"], account_status,
            "case {case:?}: read-back account status: expected {account_status}, got {after}"
        );

        for (resource_type, expected_status) in per_type {
            let member = resource_member(resource_type);
            assert_eq!(
                disabled["accounts"][0]["resourceStatus"][member], expected_status,
                "case {case:?}: Disable's resourceStatus.{member}: expected \
                 {expected_status}, got {disabled}"
            );
            assert_eq!(
                after["accounts"][0]["resourceState"][member]["status"], expected_status,
                "case {case:?}: read-back resourceState.{member}: expected {expected_status}, \
                 got {after}"
            );
        }
    }
}

/// `Disable` acts only on the accounts in its scope: an account it does not name keeps the
/// enablement it had, and reading an account back reports each one from its own record.
#[tokio::test]
async fn disable_is_scoped_to_the_accounts_it_names() {
    let other = "012345678901";
    let third = "109876543210";
    let router = router();

    let (status, enabled) = post(
        &router,
        ENABLE_URI,
        &json!({ "resourceTypes": ["EC2", "ECR"], "accountIds": [CALLER, other, third] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Enable: expected 200, got {status} / {enabled}"
    );

    // Two of the three accounts, and only the EC2 scan type.
    let (status, disabled) = post(
        &router,
        DISABLE_URI,
        &json!({ "resourceTypes": ["EC2"], "accountIds": [CALLER, third] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Disable: expected 200, got {status} / {disabled}"
    );
    let reported: Vec<&str> = disabled["accounts"]
        .as_array()
        .expect("accounts is a list")
        .iter()
        .map(|account| account["accountId"].as_str().expect("an accountId"))
        .collect();
    assert_eq!(
        reported,
        vec![CALLER, third],
        "Disable must report every account it names, got {disabled}"
    );

    let (status, after) = post(
        &router,
        STATUS_URI,
        &json!({ "accountIds": [CALLER, other, third] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetAccountStatus: expected 200, got {status} / {after}"
    );
    // The two named accounts lost EC2 and kept ECR; the account left out kept both.
    for (index, (account, ec2)) in [(CALLER, "DISABLED"), (other, "ENABLED"), (third, "DISABLED")]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            after["accounts"][index]["accountId"], account,
            "case {account}: reported out of request order: {after}"
        );
        assert_eq!(
            after["accounts"][index]["resourceState"]["ec2"]["status"], ec2,
            "case {account}: EC2 status: expected {ec2}, got {after}"
        );
        assert_eq!(
            after["accounts"][index]["resourceState"]["ecr"]["status"], "ENABLED",
            "case {account}: ECR was not named by the Disable call, got {after}"
        );
        assert_eq!(
            after["accounts"][index]["state"]["status"], "ENABLED",
            "case {account}: a scan type is still on, so the account stays ENABLED: {after}"
        );
    }
}

/// Disabling an account Amazon Inspector was never enabled for reports everything off and
/// records nothing for it: `ListUsageTotals` without `accountIds` reports one row per
/// account this service holds a record for, so an account `Disable` only read is absent
/// from it.
#[tokio::test]
async fn disabling_a_never_enabled_account_stores_nothing() {
    let never_enabled = "012345678901";
    let router = router();

    let (status, disabled) = post(
        &router,
        DISABLE_URI,
        &json!({ "accountIds": [never_enabled] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Disable: expected 200, got {status} / {disabled}"
    );
    assert_eq!(
        disabled["accounts"][0]["status"], "DISABLED",
        "a never-enabled account reports DISABLED, got {disabled}"
    );
    for resource_type in RESOURCE_SCAN_TYPES {
        let member = resource_member(resource_type);
        assert_eq!(
            disabled["accounts"][0]["resourceStatus"][member], "DISABLED",
            "case {resource_type}: expected DISABLED, got {disabled}"
        );
    }

    let (status, usage) = post(&router, USAGE_URI, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListUsageTotals: expected 200, got {status} / {usage}"
    );
    let listed: Vec<&str> = usage["totals"]
        .as_array()
        .expect("totals is a list")
        .iter()
        .map(|total| total["accountId"].as_str().expect("an accountId"))
        .collect();
    assert_eq!(
        listed,
        vec![CALLER],
        "Disable must not record anything for {never_enabled}, got {usage}"
    );
}

/// A `resourceTypes` entry outside `ResourceScanType` is refused at the boundary, so no
/// unmodelled scan type ever reaches the store.
/// Source: `smithy_query(query="shape", shape="ResourceScanType")` — the five tokens
/// above are the whole value space.
#[tokio::test]
async fn an_unmodelled_resource_type_is_refused() {
    let router = router();
    for unmodelled in ["", "ec2", "EC3", "ALL"] {
        let (status, error_type, body) = post_in(
            &router,
            "us-east-1",
            DISABLE_URI,
            &json!({ "resourceTypes": [unmodelled] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unmodelled:?}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {unmodelled:?}: expected ValidationException, got {error_type:?} / {body}"
        );
    }
}

/// Disabling every scan type turns Amazon Inspector off for the account, so a later
/// `Enable` is a fresh enablement rather than a replay of the disabled one — even when it
/// carries the `@idempotencyToken` the disabled enablement was made under.
#[tokio::test]
async fn enable_after_a_full_disable_is_not_a_replay() {
    let router = router();
    let token = "token-1";

    let (status, enabled) = post(
        &router,
        ENABLE_URI,
        &json!({ "resourceTypes": ["EC2", "ECR"], "clientToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "first Enable: expected 200, got {status} / {enabled}"
    );

    let (status, disabled) = post(&router, DISABLE_URI, &json!({})).await;
    assert_eq!(
        status, 200,
        "Disable: expected 200, got {status} / {disabled}"
    );
    assert_eq!(
        disabled["accounts"][0]["status"], "DISABLED",
        "disabling every scan type must report the account DISABLED, got {disabled}"
    );

    let (status, again) = post(
        &router,
        ENABLE_URI,
        &json!({ "resourceTypes": ["EC2"], "clientToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Enable after Disable: expected 200, got {status} / {again}"
    );
    assert_eq!(
        again["accounts"][0]["status"], "ENABLED",
        "an Enable after a full Disable must enable the account, got {again}"
    );
    assert_eq!(
        again["accounts"][0]["resourceStatus"]["ec2"], "ENABLED",
        "the scan type the second Enable names must be on, got {again}"
    );
    assert_eq!(
        again["accounts"][0]["resourceStatus"]["ecr"], "DISABLED",
        "the scan type only the disabled enablement had must stay off, got {again}"
    );
}
