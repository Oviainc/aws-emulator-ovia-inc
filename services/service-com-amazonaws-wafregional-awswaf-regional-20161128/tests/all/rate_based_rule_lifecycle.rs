// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `RateBasedRule` lifecycle, end to end over the wire:
//! `CreateRateBasedRule`, `GetRateBasedRule` and `ListRateBasedRules`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_PAGE_SIZE, MAX_RATE_BASED_RULES, RESERVED_METRIC_NAMES,
};

use crate::wire::{
    call, change_token, error_code, expect_error, is_resource_id, ok_call, router, token_status,
};

/// `RateLimit`'s modelled `@range`, from
/// `smithy_query(query="operation", operation="CreateRateBasedRule")`:
/// `RateLimit (RateLimit, range 100..2000000000)`.
const RATE_LIMIT_MIN: i64 = 100;
/// The upper bound of that same `@range`.
const RATE_LIMIT_MAX: i64 = 2_000_000_000;

/// Create a rate-based rule and return `(RuleId, the whole response body)`.
async fn create_rule(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
    rate_limit: i64,
) -> (String, Value) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRateBasedRule",
        &json!({
            "Name": name,
            "MetricName": metric_name,
            "RateKey": "IP",
            "RateLimit": rate_limit,
            "ChangeToken": token,
        }),
    )
    .await;
    let rule_id = body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRateBasedRule returned no RuleId: {body}"))
        .to_string();
    (rule_id, body)
}

/// Walk every page of `ListRateBasedRules` at `limit` and return the `(RuleId, Name)`
/// pairs it reports, in page order.
///
/// The walk itself is asserted here, once for every caller: a page holds at most
/// `Limit` summaries, and `NextMarker` is present exactly while rules remain — the
/// latter only when the caller knows how many are stored (`expected_total`).
async fn walk_pages(
    router: &MultiServiceRouter,
    limit: i64,
    expected_total: Option<usize>,
) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({"Limit": limit});
        if let Some(marker) = &marker {
            request["NextMarker"] = json!(marker);
        }
        let page = ok_call(router, "ListRateBasedRules", &request).await;
        let rules = page["Rules"]
            .as_array()
            .unwrap_or_else(|| panic!("case Limit {limit}: no Rules array: {page}"))
            .clone();
        if limit > 0 {
            assert!(
                rules.len() as i64 <= limit,
                "case Limit {limit}: a page must hold at most {limit} summaries, got {}",
                rules.len()
            );
        }
        for summary in &rules {
            seen.push((
                summary["RuleId"].as_str().unwrap_or_default().to_string(),
                summary["Name"].as_str().unwrap_or_default().to_string(),
            ));
        }
        marker = page["NextMarker"].as_str().map(str::to_string);
        pages += 1;
        if let Some(total) = expected_total {
            // The marker is present exactly when a rule remains past this page.
            assert_eq!(
                marker.is_some(),
                seen.len() < total,
                "case Limit {limit}: NextMarker must be present only while rules remain: {page}"
            );
        }
        if marker.is_none() {
            return seen;
        }
        assert!(
            pages <= expected_total.unwrap_or(MAX_RATE_BASED_RULES) + 1,
            "case Limit {limit}: paging did not terminate after {pages} pages"
        );
    }
}

/// The `(RuleId, Name)` pairs `ListRateBasedRules` reports, walking every page.
async fn list_rules(router: &MultiServiceRouter, limit: i64) -> Vec<(String, String)> {
    walk_pages(router, limit, None).await
}

/// A rule created with `CreateRateBasedRule` is readable through `GetRateBasedRule`
/// and reported by `ListRateBasedRules`, with every member as it was STORED.
///
/// The table spans the class the rule generalises over: the rate limit (each published
/// `@range` bound and a value between them), the name and the metric name.
#[tokio::test]
async fn created_rate_based_rules_are_listed_and_readable_back() {
    let router = router();

    // `(Name, MetricName, RateLimit)`: the `@range` minimum, a value inside it, and
    // the `@range` maximum.
    let cases: [(&str, &str, i64); 3] = [
        ("floor-rule", "floorRule", RATE_LIMIT_MIN),
        ("middle-rule", "middleRule9", 5_000),
        ("ceiling rule", "ceilingRule", RATE_LIMIT_MAX),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (name, metric_name, rate_limit) in cases {
        let case = format!("{name}/{metric_name}/{rate_limit}");
        let (rule_id, body) = create_rule(&router, name, metric_name, rate_limit).await;
        assert!(
            is_resource_id(&rule_id),
            "case {case:?}: {rule_id:?} is not a ResourceId"
        );
        assert!(
            created.iter().all(|(id, _)| id != &rule_id),
            "case {case:?}: every rule must get its own RuleId, got {rule_id:?} twice"
        );
        assert_eq!(
            body["Rule"]["RateLimit"].as_i64(),
            Some(rate_limit),
            "case {case:?}: the created rule must carry the requested RateLimit: {body}"
        );
        assert_eq!(
            body["Rule"]["MatchPredicates"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a new rule holds no predicates: {body}"
        );

        // Round trip: the reader answers the record that was stored.
        let read = ok_call(&router, "GetRateBasedRule", &json!({"RuleId": rule_id})).await;
        for (member, expected) in [
            ("RuleId", json!(rule_id)),
            ("Name", json!(name)),
            ("MetricName", json!(metric_name)),
            ("RateKey", json!("IP")),
            ("RateLimit", json!(rate_limit)),
        ] {
            assert_eq!(
                read["Rule"][member], expected,
                "case {case:?}: GetRateBasedRule must report the stored {member}: {read}"
            );
        }
        created.push((rule_id, name.to_string()));
    }

    created.sort();
    let listed = list_rules(&router, 0).await;
    assert_eq!(
        listed, created,
        "ListRateBasedRules must report every stored rule exactly once, in RuleId order"
    );
}

/// `ListRateBasedRules` pages the stored rules: `Limit` bounds the page, `NextMarker`
/// resumes it, and walking the pages yields every stored rule exactly once.
#[tokio::test]
async fn list_rate_based_rules_pages_by_limit_and_marker() {
    let router = router();
    let mut created: Vec<(String, String)> = Vec::new();
    for n in 0..5 {
        let name = format!("page-probe-{n}");
        let (rule_id, _) = create_rule(&router, &name, &format!("pageProbe{n}"), 1_000).await;
        created.push((rule_id, name));
    }
    created.sort();

    // `Limit` 0 is the model's declared default and asks for the service's own page
    // size; 100 is its `@range` maximum. Each row walks every page.
    for limit in [0_i64, 1, 2, 4, 5, MAX_PAGE_SIZE as i64] {
        let seen = walk_pages(&router, limit, Some(created.len())).await;
        assert_eq!(
            seen, created,
            "case Limit {limit}: paging must yield every stored rule exactly once, in id order"
        );
    }

    // `PaginationLimit`'s modelled `@range` is 0..100, so one past either bound is
    // refused before the handler runs.
    for limit in [-1_i64, MAX_PAGE_SIZE as i64 + 1] {
        let (status, body) = call(&router, "ListRateBasedRules", &json!({"Limit": limit})).await;
        assert_eq!(
            status, 400,
            "case Limit {limit}: a value outside 0..100 must be refused, got {status}: {body}"
        );
    }
}

/// `ListRateBasedRules` is a read: it reports what is stored and changes nothing a
/// later caller sees, and on an account with no rules it reports an empty list rather
/// than failing.
#[tokio::test]
async fn listing_reports_state_without_changing_it() {
    let router = router();

    let empty = ok_call(&router, "ListRateBasedRules", &json!({"Limit": 0})).await;
    assert_eq!(
        empty["Rules"].as_array().map(Vec::len),
        Some(0),
        "with nothing stored, Rules must be an empty list: {empty}"
    );
    assert!(
        empty["NextMarker"].is_null(),
        "with nothing stored, no NextMarker is owed: {empty}"
    );

    let (rule_id, _) = create_rule(&router, "read-probe", "readProbe", 2_000).await;
    let token = change_token(&router).await;
    let before = list_rules(&router, 0).await;
    for _ in 0..3 {
        assert_eq!(
            list_rules(&router, 0).await,
            before,
            "repeated listing must report the same rules"
        );
        ok_call(&router, "GetRateBasedRule", &json!({"RuleId": &rule_id})).await;
    }
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "a read must not consume the outstanding change token"
    );
    assert_eq!(
        before,
        vec![(rule_id, "read-probe".to_string())],
        "the listed rule is the one that was created"
    );
}

/// A refused `CreateRateBasedRule` stores nothing, so `ListRateBasedRules` keeps
/// reporting exactly the rules that were accepted.
///
/// The rows are the ways one create is refused: a `RateLimit` outside the modelled
/// `@range` (one past each published bound), a `RateKey` other than the only value the
/// model admits, a `MetricName` outside the characters AWS WAF Classic allows or one it
/// reserves, and a change token no `GetChangeToken` handed out.
#[tokio::test]
async fn refused_creates_store_nothing() {
    let router = router();
    let (kept, _) = create_rule(&router, "keeper", "keeper", 1_000).await;
    let expected = vec![(kept, "keeper".to_string())];

    // Rows refused by the model's own constraints: the boundary answers these before
    // the handler runs, so only the status and the state are asserted.
    for (label, rate_limit) in [
        ("one below the RateLimit minimum", RATE_LIMIT_MIN - 1),
        ("one above the RateLimit maximum", RATE_LIMIT_MAX + 1),
    ] {
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "CreateRateBasedRule",
            &json!({
                "Name": "out-of-range",
                "MetricName": "outOfRange",
                "RateKey": "IP",
                "RateLimit": rate_limit,
                "ChangeToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: a RateLimit of {rate_limit} must be refused, got {status}: {body}"
        );
        assert_eq!(
            list_rules(&router, 0).await,
            expected,
            "case {label}: a refused create must store nothing"
        );
    }

    // Rows the service itself refuses, each with the error its model declares.
    let reserved_metric = RESERVED_METRIC_NAMES
        .first()
        .expect("AWS WAF reserves at least one metric name");
    let rows: [(&str, Value, &str); 3] = [
        (
            "a MetricName carrying a character AWS WAF does not allow",
            json!({
                "Name": "bad-metric",
                "MetricName": "bad-metric-name",
                "RateKey": "IP",
                "RateLimit": 1_000,
            }),
            "WAFDisallowedNameException",
        ),
        (
            "a MetricName AWS WAF reserves",
            json!({
                "Name": "reserved-metric",
                "MetricName": reserved_metric,
                "RateKey": "IP",
                "RateLimit": 1_000,
            }),
            "WAFDisallowedNameException",
        ),
        (
            "a change token no GetChangeToken minted",
            json!({
                "Name": "stale-token",
                "MetricName": "staleToken",
                "RateKey": "IP",
                "RateLimit": 1_000,
                "ChangeToken": "11111111-2222-4333-8444-555555555555",
            }),
            "WAFStaleDataException",
        ),
    ];
    for (label, mut request, expected_code) in rows {
        if request.get("ChangeToken").is_none() {
            request["ChangeToken"] = json!(change_token(&router).await);
        }
        expect_error(&router, "CreateRateBasedRule", &request, expected_code, label).await;
        assert_eq!(
            list_rules(&router, 0).await,
            expected,
            "case {label}: a refused create must store nothing"
        );
    }

    // Reading a rule that was never stored is the modelled nonexistent-item error, not
    // an empty success.
    let body = expect_error(
        &router,
        "GetRateBasedRule",
        &json!({"RuleId": "11111111-2222-4333-8444-555555555555"}),
        "WAFNonexistentItemException",
        "reading a rule that does not exist",
    )
    .await;
    assert!(
        !error_code(&body).contains("NotImplemented"),
        "case reading a rule that does not exist: the operation must be implemented, got {}",
        error_code(&body)
    );
}

/// "The only valid value for RateKey is IP": `RateKey` is modelled as an enum with
/// that single value, so a request naming any other value is refused at the boundary —
/// HTTP 400 `ValidationException` naming the member and the enum's value set — and
/// stores nothing. The handler is never reached, so there is no modelled
/// `WAFInvalidParameterException` to assert here.
#[tokio::test]
async fn create_refuses_a_rate_key_other_than_ip() {
    let router = router();
    // `RateKey`'s only modelled value, from
    // `smithy_query(query="shape", shape="RateKey")` → `RATE_KEY_VALUES = ["IP"]`; each
    // row is a value outside it, including the right value in the wrong case.
    for rate_key in ["HTTP_METHOD", "ip", "FORWARDED_IP"] {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "CreateRateBasedRule",
            &json!({
                "Name": "bad-rate-key",
                "MetricName": "badRateKey",
                "RateKey": rate_key,
                "RateLimit": 1_000,
                "ChangeToken": token,
            }),
            "ValidationException",
            rate_key,
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("rateKey") && message.contains("IP"),
            "case {rate_key}: the refusal must name the member and the admitted value \
             set, got {message:?}"
        );
        assert!(
            list_rules(&router, 0).await.is_empty(),
            "case {rate_key}: a refused create must store nothing"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {rate_key}: a refused call must not consume the change token"
        );
    }
}

/// The rule state a caller can grow is bounded: at the account's rate-based-rule quota
/// `CreateRateBasedRule` answers its modelled `WAFLimitsExceededException`, and
/// `ListRateBasedRules` reports exactly the rules that were stored.
#[tokio::test]
async fn create_refuses_past_the_rate_based_rule_quota() {
    let router = router();
    for n in 0..MAX_RATE_BASED_RULES {
        create_rule(&router, &format!("quota-{n}"), &format!("quota{n}"), 1_000).await;
    }

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateRateBasedRule",
        &json!({
            "Name": "one-too-many",
            "MetricName": "oneTooMany",
            "RateKey": "IP",
            "RateLimit": 1_000,
            "ChangeToken": token,
        }),
        "WAFLimitsExceededException",
        "at the quota",
    )
    .await;
    assert_eq!(
        token_status(&router, &token).await,
        "PROVISIONED",
        "case at the quota: a refused call must not consume the change token"
    );
    assert_eq!(
        list_rules(&router, 0).await.len(),
        MAX_RATE_BASED_RULES,
        "case at the quota: exactly the accepted rules are stored"
    );
}
