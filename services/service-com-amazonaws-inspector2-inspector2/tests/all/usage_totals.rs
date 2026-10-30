// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListUsageTotals`.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.
//!
//! The rule under test: `ListUsageTotals` reports one row per account in the request's
//! scope — the accounts `accountIds` names, or, when it names none, the calling account
//! together with every account this service holds an Amazon Inspector record for — and
//! each row carries one usage entry per usage type the scans that account has ENABLED
//! meter, with the amount this emulator has metered for it; the rows are ordered by
//! account id and paged by `maxResults` behind an opaque `nextToken`.

use serde_json::{json, Value};

use crate::filters_and_enablement::{post_in, router};

/// `ListUsageTotals`'s route.
/// Source: `smithy_query(query="operation", operation="ListUsageTotals")` —
/// `POST /usage/list`.
const URI: &str = "/usage/list";

/// `Enable`'s route, which is how an account's scan types are turned on.
/// Source: `smithy_query(query="operation", operation="Enable")` — `POST /enable`.
const ENABLE_URI: &str = "/enable";

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const CALLER: &str = "000000000000";

/// Every `ResourceScanType` token, in model order.
/// Source: `smithy_query(query="shape", shape="ResourceScanType")`.
const RESOURCE_SCAN_TYPES: [&str; 5] = ["EC2", "ECR", "LAMBDA", "LAMBDA_CODE", "CODE_REPOSITORY"];

/// The usage types one scan type's scans meter, as the service under test reports them.
///
/// Source: `smithy_query(query="shape", shape="UsageType,ResourceScanType")` — the
/// `AZURE_*` usage types are metered on Azure connector resources rather than on a
/// `ResourceScanType`, so no scan type lists them.
fn expected_usage_types(resource_scan_type: &str) -> &'static [&'static str] {
    match resource_scan_type {
        "EC2" => &["EC2_INSTANCE_HOURS", "EC2_AGENTLESS_INSTANCE_HOURS"],
        "ECR" => &["ECR_INITIAL_SCAN", "ECR_RESCAN"],
        "LAMBDA" => &["LAMBDA_FUNCTION_HOURS"],
        "LAMBDA_CODE" => &["LAMBDA_FUNCTION_CODE_HOURS"],
        "CODE_REPOSITORY" => &[
            "CODE_REPOSITORY_SAST",
            "CODE_REPOSITORY_IAC",
            "CODE_REPOSITORY_SCA",
        ],
        other => panic!("unmodelled ResourceScanType {other:?}"),
    }
}

/// The usage types an account with `enabled` scan types meters, in the order the answer
/// has to list them: scan types in model order, each one's usage types in model order.
fn expected_usage_types_of(enabled: &[&str]) -> Vec<String> {
    RESOURCE_SCAN_TYPES
        .iter()
        .filter(|scan_type| enabled.contains(*scan_type))
        .flat_map(|scan_type| expected_usage_types(scan_type))
        .map(|usage_type| (*usage_type).to_string())
        .collect()
}

/// A 12-digit account id, so a case can name several distinct accounts.
/// Source: `smithy_query(query="shape", shape="UsageAccountId")` — `[0-9]{12}`.
fn account_id(index: usize) -> String {
    format!("{:012}", 100_000_000_000_u64 + index as u64)
}

/// The usage types one row lists, in the order it lists them.
fn usage_types_in(row: &Value) -> Vec<String> {
    row["usage"]
        .as_array()
        .map(|usage| {
            usage
                .iter()
                .map(|entry| entry["type"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The account ids one answer's rows name, in the order it lists them.
fn account_ids_in(answer: &Value) -> Vec<String> {
    answer["totals"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["accountId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every usage entry of one row reports an amount this emulator has metered, in the one
/// currency the model declares, for the cloud provider whose resources the scan covers.
fn assert_entries_are_metered_amounts(case: &str, row: &Value) {
    for entry in row["usage"].as_array().cloned().unwrap_or_default() {
        let usage_type = entry["type"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            entry["total"].as_f64(),
            Some(0.0),
            "case {case}: no resource has been scanned, so the {usage_type} meter must stand at \
             zero, got {entry}"
        );
        assert_eq!(
            entry["estimatedMonthlyCost"].as_f64(),
            Some(0.0),
            "case {case}: a usage of zero costs nothing whatever the rate, got {entry}"
        );
        assert_eq!(
            entry["currency"], "USD",
            "case {case}: the amount must be reported in the modelled currency, got {entry}"
        );
        assert_eq!(
            entry["cloudProvider"], "AWS",
            "case {case}: an Amazon Web Services scan meters Amazon Web Services usage, got \
             {entry}"
        );
    }
}

/// Turn `resource_types` on for `account_ids` (the calling account when the list is
/// empty), and fail the test if `Enable` itself did not answer.
async fn enable(
    router: &smithy_server_runtime::MultiServiceRouter,
    account_ids: &[String],
    resource_types: &[&str],
) {
    let mut request = json!({ "resourceTypes": resource_types });
    if !account_ids.is_empty() {
        request["accountIds"] = json!(account_ids);
    }
    let (status, error_type, answer) = post_in(router, "us-east-1", ENABLE_URI, &request).await;
    assert_eq!(
        status, 200,
        "Enable {account_ids:?} / {resource_types:?}: expected 200, got {status} / {answer} \
         ({error_type:?})"
    );
}

/// A `ListUsageTotals` call carrying `request`, answered with 200.
async fn list(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> Value {
    let (status, error_type, answer) = post_in(router, "us-east-1", URI, request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} / {answer} ({error_type:?})"
    );
    answer
}

/// The usage entries an account reports are the meters of the scan types it has enabled:
/// the table varies WHICH and HOW MANY scan types are on, and an account with nothing on
/// meters nothing.
#[tokio::test]
async fn usage_entries_are_the_meters_of_the_enabled_scan_types() {
    // Rows: each scan type on its own — including ones this issue never cited — two
    // combinations, and all five at once (the modelled `resourceTypes` bounds are 1..5).
    let cases: [&[&str]; 8] = [
        &["EC2"],
        &["ECR"],
        &["LAMBDA"],
        &["LAMBDA_CODE"],
        &["CODE_REPOSITORY"],
        &["EC2", "ECR"],
        &["LAMBDA", "LAMBDA_CODE", "CODE_REPOSITORY"],
        &RESOURCE_SCAN_TYPES,
    ];

    for enabled in cases {
        let case = format!("{enabled:?}");
        let router = router();

        // Before `Enable`, the calling account has no scan on, so it meters nothing.
        let answer = list(&router, &case, &json!({})).await;
        assert_eq!(
            account_ids_in(&answer),
            vec![CALLER.to_string()],
            "case {case}: a request naming no account must report the calling account, got \
             {answer}"
        );
        assert_eq!(
            answer["totals"][0]["usage"],
            json!([]),
            "case {case}: an account with no scan enabled meters nothing, got {answer}"
        );
        assert!(
            answer.get("nextToken").is_none(),
            "case {case}: an answer carrying every row in scope must hand out no cursor, got \
             {answer}"
        );

        enable(&router, &[], enabled).await;

        let answer = list(&router, &case, &json!({})).await;
        assert_eq!(
            account_ids_in(&answer),
            vec![CALLER.to_string()],
            "case {case}: the answer must report the calling account, got {answer}"
        );
        assert_eq!(
            usage_types_in(&answer["totals"][0]),
            expected_usage_types_of(enabled),
            "case {case}: the usage entries must be the meters of exactly the enabled scan \
             types, got {answer}"
        );
        assert_entries_are_metered_amounts(&case, &answer["totals"][0]);
    }
}

/// `accountIds` is the scope: one row per DISTINCT account it names, in account order,
/// each carrying that account's own meters — an account the service holds no record for
/// included, which meters nothing.
#[tokio::test]
async fn account_ids_scope_the_answer_to_one_row_per_distinct_account() {
    let router = router();
    let known: Vec<String> = (0..3).map(account_id).collect();
    let unknown = account_id(90);

    // Two of the three accounts are enabled, for different scan types, so a row's meters
    // have to come from ITS own account rather than from the request.
    enable(&router, &known[0..1], &["EC2"]).await;
    enable(&router, &known[1..2], &["LAMBDA", "CODE_REPOSITORY"]).await;

    // Rows: one id; several ids out of order; an id repeated; an unknown id; a mix.
    let cases: [(&str, Vec<String>, Vec<String>); 5] = [
        ("one known id", vec![known[0].clone()], vec![known[0].clone()]),
        (
            "three ids out of order",
            vec![known[2].clone(), known[0].clone(), known[1].clone()],
            vec![known[0].clone(), known[1].clone(), known[2].clone()],
        ),
        (
            "one id repeated",
            vec![known[1].clone(), known[1].clone()],
            vec![known[1].clone()],
        ),
        ("an unknown id", vec![unknown.clone()], vec![unknown.clone()]),
        (
            "a known and an unknown id",
            vec![unknown.clone(), known[0].clone()],
            vec![known[0].clone(), unknown.clone()],
        ),
    ];

    for (case, requested, expected_rows) in cases {
        let answer = list(&router, case, &json!({ "accountIds": requested })).await;
        assert_eq!(
            account_ids_in(&answer),
            expected_rows,
            "case {case}: the answer must report one row per distinct requested account, in \
             account order, got {answer}"
        );

        for (index, reported) in expected_rows.iter().enumerate() {
            let enabled: &[&str] = if *reported == known[0] {
                &["EC2"]
            } else if *reported == known[1] {
                &["LAMBDA", "CODE_REPOSITORY"]
            } else {
                &[]
            };
            assert_eq!(
                usage_types_in(&answer["totals"][index]),
                expected_usage_types_of(enabled),
                "case {case}: the row for {reported} must carry that account's own meters, got \
                 {answer}"
            );
            assert_entries_are_metered_amounts(case, &answer["totals"][index]);
        }
    }
}

/// A request naming no account reports the calling account together with every account
/// this service holds a record for, so enabling an account adds its row.
#[tokio::test]
async fn a_request_naming_no_account_reports_every_account_with_a_record() {
    let router = router();

    // Rows: the empty store, then one, then three recorded accounts.
    let mut expected = vec![CALLER.to_string()];
    let answer = list(&router, "empty store", &json!({})).await;
    assert_eq!(
        account_ids_in(&answer),
        expected,
        "case empty store: only the calling account is in scope, got {answer}"
    );

    for index in 0..3 {
        let id = account_id(index);
        enable(&router, std::slice::from_ref(&id), &["ECR"]).await;
        expected.push(id.clone());
        expected.sort();

        let case = format!("{} recorded accounts", index + 1);
        let answer = list(&router, &case, &json!({})).await;
        assert_eq!(
            account_ids_in(&answer),
            expected,
            "case {case}: every account with a record, and the caller, must be in scope, got \
             {answer}"
        );
        let caller_row = answer["totals"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["accountId"].as_str() == Some(CALLER))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            caller_row["usage"],
            json!([]),
            "case {case}: the calling account has no scan enabled, so it meters nothing, got \
             {answer}"
        );
    }
}

/// `maxResults` caps the rows ONE answer carries, and the `nextToken` that follows a
/// capped answer resumes after its last row: the pages of a walk are the whole scope,
/// each account exactly once, whatever the page size.
#[tokio::test]
async fn pages_walk_every_account_in_scope_exactly_once() {
    let router = router();
    let accounts: Vec<String> = (0..7).map(account_id).collect();
    enable(&router, &accounts, &["EC2", "ECR"]).await;

    let mut expected = accounts.clone();
    expected.push(CALLER.to_string());
    expected.sort();

    // Rows: page sizes from the modelled minimum to the modelled maximum, around and
    // past the number of accounts in scope.
    // Source: `smithy_query(query="operation", operation="ListUsageTotals")` —
    // `maxResults` range 1..500.
    for max_results in [1_usize, 2, 3, 7, 8, 500] {
        let case = format!("maxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            let mut request = json!({ "maxResults": max_results });
            if let Some(cursor) = &token {
                request["nextToken"] = json!(cursor);
            }
            let answer = list(&router, &case, &request).await;
            let rows = account_ids_in(&answer);
            assert!(
                rows.len() <= max_results,
                "case {case}: a page must carry at most maxResults rows, got {}",
                rows.len()
            );
            seen.extend(rows);
            token = answer["nextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: the walk must terminate, but it is still paging after {pages} \
                 pages having seen {seen:?}"
            );
            if token.is_none() {
                assert!(
                    seen.len() == expected.len(),
                    "case {case}: the last page must end the walk, got {seen:?}"
                );
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case}: the pages together must be the whole scope, in account order, with no \
             account twice"
        );
    }
}

/// A `nextToken` no answer handed out names no position, so it is refused rather than
/// read as the start of the list — while the cursor an answer DID hand out resumes the
/// walk after that answer's last row.
#[tokio::test]
async fn a_next_token_no_answer_handed_out_is_refused() {
    let router = router();
    let accounts: Vec<String> = (0..2).map(account_id).collect();
    enable(&router, &accounts, &["EC2"]).await;

    // Rows: a non-hex token, hex of odd length, well-formed hex that is not an account
    // id, hex of an ARN of another service, and hex of a value one digit short of an
    // account id.
    let tokens = [
        "not-a-token",
        "303030303030303030303",
        "4141",
        "61726e3a6177733a73333a3a3a6d792d6275636b6574",
        "3030303030303030303030",
    ];
    for token in tokens {
        let request = json!({ "nextToken": token });
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;

        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} / {answer}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {token:?}: expected ValidationException, got {error_type:?} / {answer}"
        );
        assert_eq!(
            answer["reason"], "CANNOT_PARSE",
            "case {token:?}: expected the CANNOT_PARSE reason, got {answer}"
        );
        assert_eq!(
            answer["fields"][0]["name"], "nextToken",
            "case {token:?}: the refusal must name the member that failed, got {answer}"
        );
        assert!(
            answer.get("totals").is_none(),
            "case {token:?}: a refused request must carry no usage totals, got {answer}"
        );
    }

    // The cursor an answer handed out IS honoured, so the refusals above are about the
    // token's value rather than about paging at all.
    let first = list(&router, "first page", &json!({ "maxResults": 1 })).await;
    let cursor = first["nextToken"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| panic!("case first page: three accounts are in scope, so a one-row page must hand out a cursor, got {first}"));
    let second = list(
        &router,
        "second page",
        &json!({ "maxResults": 1, "nextToken": cursor }),
    )
    .await;
    assert_eq!(
        account_ids_in(&second),
        vec![accounts[0].clone()],
        "case second page: the cursor must resume after the first page's last row, got {second}"
    );
}

/// The modelled bounds are enforced before the handler runs, and a request inside them is
/// answered: a scope at the `accountIds` maximum is paged by the largest page the model
/// allows.
#[tokio::test]
async fn the_modelled_bounds_are_enforced_at_the_boundary() {
    let router = router();

    // Rows inside the bounds: the `maxResults` range bounds, and the `accountIds` length
    // bounds (1 and 7000 — the maximum, which the default page size then splits).
    let accepted: [(&str, Value, usize); 4] = [
        ("maxResults=1", json!({ "maxResults": 1 }), 1),
        ("maxResults=500", json!({ "maxResults": 500 }), 1),
        (
            "accountIds x1",
            json!({ "accountIds": [account_id(0)] }),
            1,
        ),
        (
            "accountIds x7000",
            json!({ "accountIds": (0..7000).map(account_id).collect::<Vec<_>>() }),
            500,
        ),
    ];
    for (case, request, rows) in accepted {
        let answer = list(&router, case, &request).await;
        assert_eq!(
            account_ids_in(&answer).len(),
            rows,
            "case {case}: expected {rows} rows, got {}",
            account_ids_in(&answer).len()
        );
        // Only the 7000-account scope has rows left over after one page, so only it is
        // followed by a cursor.
        let expects_cursor = case == "accountIds x7000";
        assert_eq!(
            answer.get("nextToken").is_some(),
            expects_cursor,
            "case {case}: a cursor must follow an answer exactly when rows remain, got \
             nextToken {:?}",
            answer.get("nextToken")
        );
    }

    // One past each published bound, and a value outside `UsageAccountId`'s pattern, is
    // refused at the boundary, so the handler never sees it.
    let refused: [(&str, Value); 5] = [
        ("maxResults=0", json!({ "maxResults": 0 })),
        ("maxResults=501", json!({ "maxResults": 501 })),
        ("accountIds=[]", json!({ "accountIds": [] })),
        (
            "accountIds x7001",
            json!({ "accountIds": (0..7001).map(account_id).collect::<Vec<_>>() }),
        ),
        ("an 11-digit accountId", json!({ "accountIds": ["00000000000"] })),
    ];
    for (case, request) in refused {
        let (status, error_type, answer) = post_in(&router, "us-east-1", URI, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} / {answer}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {error_type:?} / {answer}"
        );
    }
}
