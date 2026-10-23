// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendedActionSummaries`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts —
//! so a response that is not computed from persisted state fails here.
//!
//! The rule under test: the summaries hold the account-wide aggregate of the
//! recommended actions that pass the request's filters — keyed
//! `ALL_Recommended_Action_Summary`, counting every one of them and summing
//! their estimated monthly savings — and it is reported whether or not any
//! action matched.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction, RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_TYPE_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::DEFAULT_MAX_RESULTS;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, unenrolled_emulator_with_engine,
};
use crate::recommended_actions::{record, stored, stored_count, MEMBER_ACCOUNT_ID};

/// The operation under test.
const OPERATION: &str = "ListRecommendedActionSummaries";

/// The `key` of the account-wide aggregate, the one key this service reports a
/// recommended-action summary under.
const ALL_KEY: &str = "ALL_Recommended_Action_Summary";

/// The account every emulated caller presents.
const CALLER_ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// A third account, so a case can span more than two owners.
const THIRD_ACCOUNT_ID: &str = "310987654321";

/// The two modelled `RecommendedActionType` values, named so a case reads.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// One `EstimatedMonthlySavings`, as Compute Optimizer's analysis records it.
fn savings(
    currency: &str,
    before_discount: f64,
    after_discount: f64,
    mode: &str,
) -> EstimatedMonthlySavings {
    EstimatedMonthlySavings {
        currency: currency.to_string(),
        before_discount_savings: before_discount,
        after_discount_savings: after_discount,
        savings_estimation_mode: mode.to_string(),
    }
}

/// One recorded action, owned by `account_id`, carrying `estimate`.
///
/// Built on the one recommended-action fixture builder this crate has, with the
/// three members these cases vary overridden: the owning account and the action
/// type (which the aggregate spans), and the estimate it sums.
fn action(
    index: usize,
    account_id: Option<&str>,
    recommended_action_type: &str,
    estimate: Option<EstimatedMonthlySavings>,
) -> RecommendedAction {
    RecommendedAction {
        account_id: account_id.map(ToString::to_string),
        recommended_action_type: Some(recommended_action_type.to_string()),
        estimated_monthly_savings: estimate,
        ..stored(
            index,
            &format!("vol-{index:04}"),
            account_id.unwrap_or(CALLER_ACCOUNT_ID),
            "us-east-1",
            14,
            recommended_action_type,
            Some("gp2"),
            false,
            &[("env", "prod")],
        )
    }
}

/// One expected total: `(count, currency, before, after, mode)`.
type ExpectedTotal = (i64, &'static str, f64, f64, &'static str);

/// The summaries one response reports.
fn reported(body: &Value) -> Vec<Value> {
    member(body, "recommendedActionSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("recommendedActionSummaries is a list, got {body}"))
        .clone()
}

/// A f64 member of a summary's totals.
fn savings_figure(summary: &Value, name: &str) -> f64 {
    summary
        .get("total")
        .and_then(|total| total.get("estimatedMonthlySavings"))
        .and_then(|estimate| estimate.get(name))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("a summary carries total.estimatedMonthlySavings.{name}: {summary}")
        })
}

/// Assert a response holds exactly the account-wide aggregate, with `want` as
/// its total. Savings are compared as numbers, so a figure the wire renders as
/// `0` and one it renders as `0.0` are the same value.
fn assert_aggregate(case: &str, body: &Value, want: &ExpectedTotal) {
    let (count, currency, before, after, mode) = *want;
    let summaries = reported(body);
    assert_eq!(
        summaries.len(),
        1,
        "case {case:?}: expected exactly the account-wide aggregate, got {body}"
    );
    let got = &summaries[0];
    assert_eq!(
        got.get("key").and_then(Value::as_str),
        Some(ALL_KEY),
        "case {case:?}: expected key {ALL_KEY:?}, got {got}"
    );
    assert_eq!(
        got.get("total")
            .and_then(|total| total.get("recommendedActionCount"))
            .and_then(Value::as_i64),
        Some(count),
        "case {case:?}: expected recommendedActionCount {count}, got {got}"
    );
    assert_eq!(
        got.get("total")
            .and_then(|total| total.get("estimatedMonthlySavings"))
            .and_then(|estimate| estimate.get("currency"))
            .and_then(Value::as_str),
        Some(currency),
        "case {case:?}: expected currency {currency:?}, got {got}"
    );
    assert_eq!(
        got.get("total")
            .and_then(|total| total.get("estimatedMonthlySavings"))
            .and_then(|estimate| estimate.get("savingsEstimationMode"))
            .and_then(Value::as_str),
        Some(mode),
        "case {case:?}: expected savingsEstimationMode {mode:?}, got {got}"
    );
    let got_before = savings_figure(got, "beforeDiscountSavings");
    let got_after = savings_figure(got, "afterDiscountSavings");
    assert!(
        (got_before - before).abs() < 1e-9,
        "case {case:?}: expected beforeDiscountSavings {before}, got {got_before} in {got}"
    );
    assert!(
        (got_after - after).abs() < 1e-9,
        "case {case:?}: expected afterDiscountSavings {after}, got {got_after} in {got}"
    );
}

// ---------------------------------------------------------------------------
// The aggregate
// ---------------------------------------------------------------------------

/// A table spanning what the rule generalises over: how many actions the store
/// holds, which accounts own them, which types they are, whether they carry an
/// estimate at all, and whether the estimates agree on a savings mode or a
/// currency.
#[tokio::test]
async fn the_account_wide_aggregate_totals_every_stored_action() {
    let scenarios: Vec<(&str, Vec<RecommendedAction>, ExpectedTotal)> = vec![
        // Nothing has been analysed yet, and the aggregate is still reported:
        // zero actions, zero savings, in no currency, with the conservative
        // before-discount mode.
        ("an-empty-store", vec![], (0, "", 0.0, 0.0, "BeforeDiscount")),
        (
            "one-action",
            vec![action(
                1,
                Some(CALLER_ACCOUNT_ID),
                UPGRADE,
                Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
            )],
            (1, "USD", 12.5, 9.75, "AfterDiscount"),
        ),
        (
            "several-actions-are-summed",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 20.0, 18.0, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 0.25, 0.125, "AfterDiscount")),
                ),
            ],
            (3, "USD", 32.75, 27.875, "AfterDiscount"),
        ),
        (
            "the-aggregate-spans-both-modelled-action-types",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    SNAPSHOT_AND_DELETE,
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
            ],
            (2, "USD", 19.5, 16.25, "AfterDiscount"),
        ),
        (
            "the-aggregate-spans-every-owning-account",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(MEMBER_ACCOUNT_ID),
                    SNAPSHOT_AND_DELETE,
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(THIRD_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
            ],
            (3, "USD", 22.0, 18.5, "AfterDiscount"),
        ),
        (
            "an-action-the-store-holds-no-account-for-is-still-counted",
            vec![
                action(
                    1,
                    None,
                    UPGRADE,
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            (2, "USD", 13.5, 10.25, "AfterDiscount"),
        ),
        (
            "an-action-without-an-estimate-is-counted-with-no-savings",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(2, Some(CALLER_ACCOUNT_ID), UPGRADE, None),
            ],
            (2, "USD", 12.5, 9.75, "AfterDiscount"),
        ),
        (
            "no-stored-action-carries-an-estimate",
            vec![
                action(1, Some(CALLER_ACCOUNT_ID), UPGRADE, None),
                action(2, Some(CALLER_ACCOUNT_ID), SNAPSHOT_AND_DELETE, None),
            ],
            // No recorded estimate names a currency, so the aggregate names
            // none and both sums are zero.
            (2, "", 0.0, 0.0, "BeforeDiscount"),
        ),
        (
            "estimates-disagreeing-on-the-mode",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
            ],
            (2, "USD", 17.5, 13.75, "BeforeDiscount"),
        ),
        (
            "estimates-agreeing-on-before-discount",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 12.5, 9.75, "BeforeDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
            ],
            (2, "USD", 17.5, 13.75, "BeforeDiscount"),
        ),
        (
            "estimates-disagreeing-on-the-currency",
            // The analysis denominates an account's estimates in one currency
            // and the emulator converts nothing, so the sum keeps the currency
            // of the first recorded estimate.
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("EUR", 10.0, 8.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    UPGRADE,
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
            ],
            (2, "EUR", 11.0, 8.5, "AfterDiscount"),
        ),
    ];

    // The table spans the whole modelled `RecommendedActionType` value set, so a
    // type added to the model cannot be left untested — the aggregate spans
    // them, so each must be counted into it.
    let covered: BTreeSet<String> = scenarios
        .iter()
        .flat_map(|(_, actions, _)| {
            actions
                .iter()
                .filter_map(|a| a.recommended_action_type.clone())
        })
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_TYPE_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionType must appear in the table"
    );

    for (case, actions, expected) in &scenarios {
        let (router, engine) = emulator_with_engine();
        record(&engine, actions.clone());
        let recorded = stored_count(&engine);

        let listed = call_ok(&router, OPERATION, &json!({}), case).await;
        assert_aggregate(case, &listed, expected);
        assert!(
            listed.get("nextToken").is_none(),
            "case {case:?}: the aggregate is one summary, so there is no further page: {listed}"
        );

        // Summarising is a read: it records nothing, and a second call answers
        // exactly the same.
        assert_eq!(
            stored_count(&engine),
            recorded,
            "case {case:?}: summarising must not change the store"
        );
        let again = call_ok(&router, OPERATION, &json!({}), case).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a repeated read must answer the same, got {again} then {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// The actions the filter cases are taken over, in record order: three
/// accounts, both action types, one action with no estimate.
fn fixtures() -> Vec<RecommendedAction> {
    vec![
        RecommendedAction {
            account_id: Some(CALLER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
            ..stored(
                1,
                "vol-aaa1",
                CALLER_ACCOUNT_ID,
                "us-east-1",
                14,
                UPGRADE,
                Some("gp2"),
                false,
                &[("env", "prod")],
            )
        },
        RecommendedAction {
            account_id: Some(MEMBER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 20.0, 18.0, "AfterDiscount")),
            ..stored(
                2,
                "vol-bbb2",
                MEMBER_ACCOUNT_ID,
                "us-west-2",
                32,
                SNAPSHOT_AND_DELETE,
                Some("gp2"),
                true,
                &[("env", "test"), ("team", "infra")],
            )
        },
        RecommendedAction {
            account_id: Some(CALLER_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: Some(savings("USD", 5.0, 4.0, "AfterDiscount")),
            ..stored(
                3,
                "vol-ccc3",
                CALLER_ACCOUNT_ID,
                "eu-west-1",
                93,
                UPGRADE,
                Some("io1"),
                true,
                &[],
            )
        },
        RecommendedAction {
            account_id: Some(THIRD_ACCOUNT_ID.to_string()),
            estimated_monthly_savings: None,
            ..stored(
                4,
                "vol-ddd4",
                THIRD_ACCOUNT_ID,
                "us-east-1",
                14,
                UPGRADE,
                None,
                false,
                &[("env", "prod")],
            )
        },
    ]
}

/// The filter lists the cross-check is taken over: every modelled
/// `RecommendedActionFilterName`, each selecting a different subset.
fn filter_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("no-filters", json!([])),
        (
            "ResourceType",
            json!([{"name": "ResourceType", "values": ["EbsVolume"]}]),
        ),
        (
            "RecommendedActionType-one",
            json!([{"name": "RecommendedActionType", "values": [UPGRADE]}]),
        ),
        (
            "RecommendedActionType-the-other",
            json!([{"name": "RecommendedActionType", "values": [SNAPSHOT_AND_DELETE]}]),
        ),
        (
            "ResourceId",
            json!([{"name": "ResourceId", "values": ["vol-aaa1", "vol-bbb2"]}]),
        ),
        (
            "AccountId-the-callers-own",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID]}]),
        ),
        (
            "AccountId-a-member-account-only",
            json!([{"name": "AccountId", "values": [MEMBER_ACCOUNT_ID]}]),
        ),
        (
            "AccountId-two-accounts",
            json!([{"name": "AccountId", "values": [CALLER_ACCOUNT_ID, THIRD_ACCOUNT_ID]}]),
        ),
        (
            "LookBackPeriodInDays",
            json!([{"name": "LookBackPeriodInDays", "values": ["14"]}]),
        ),
        (
            "CurrentResourceDetailsEbsVolumeType",
            json!([{"name": "CurrentResourceDetailsEbsVolumeType", "values": ["gp2"]}]),
        ),
        (
            "ResourceTagsKey",
            json!([{"name": "ResourceTagsKey", "values": ["team"]}]),
        ),
        (
            "ResourceTagsValue",
            json!([{"name": "ResourceTagsValue", "values": ["prod"]}]),
        ),
        (
            "RestartNeeded",
            json!([{"name": "RestartNeeded", "values": ["true"]}]),
        ),
        (
            "two-filters-are-anded",
            json!([
                {"name": "RecommendedActionType", "values": [UPGRADE]},
                {"name": "RestartNeeded", "values": ["false"]}
            ]),
        ),
        (
            "a-filter-nothing-matches",
            json!([{"name": "ResourceId", "values": ["vol-nothing"]}]),
        ),
    ]
}

/// For every filter list, the aggregate counts and sums exactly the actions
/// `ListRecommendedActions` reports for the SAME filters — and it is still
/// reported, with zero totals, for a filter nothing matches.
#[tokio::test]
async fn filters_narrow_the_actions_the_aggregate_counts() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    // The table spans the whole modelled value set, so a name added to the model
    // cannot be left untested.
    let covered: BTreeSet<String> = filter_cases()
        .iter()
        .flat_map(|(_, filters)| {
            filters
                .as_array()
                .expect("a filter list")
                .iter()
                .map(|filter| filter["name"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_FILTER_NAME_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionFilterName must appear in the table"
    );

    for (case, filters) in filter_cases() {
        let request = json!({"filters": filters, "maxResults": DEFAULT_MAX_RESULTS});
        let actions = call_ok(&router, "ListRecommendedActions", &request, case).await;
        let actions = member(&actions, "recommendedActions")
            .as_array()
            .expect("recommendedActions is a list")
            .clone();

        // The oracle: count and sum the actions that operation reported.
        let count = i64::try_from(actions.len()).expect("a small fixture set");
        let figure = |name: &str| -> f64 {
            actions
                .iter()
                .map(|listed| {
                    listed
                        .get("estimatedMonthlySavings")
                        .and_then(|estimate| estimate.get(name))
                        .and_then(Value::as_f64)
                        .unwrap_or_default()
                })
                .sum()
        };
        let before: f64 = figure("beforeDiscountSavings");
        let after: f64 = figure("afterDiscountSavings");

        let summarised = call_ok(&router, OPERATION, &request, case).await;
        let summaries = reported(&summarised);
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected exactly the account-wide aggregate, got {summarised}"
        );
        let summary = &summaries[0];
        assert_eq!(
            summary.get("key").and_then(Value::as_str),
            Some(ALL_KEY),
            "case {case:?}: expected key {ALL_KEY:?}, got {summary}"
        );
        assert_eq!(
            summary
                .get("total")
                .and_then(|total| total.get("recommendedActionCount"))
                .and_then(Value::as_i64),
            Some(count),
            "case {case:?}: these filters match {count} actions, got {summary}"
        );
        let got_before = savings_figure(summary, "beforeDiscountSavings");
        let got_after = savings_figure(summary, "afterDiscountSavings");
        assert!(
            (got_before - before).abs() < 1e-9,
            "case {case:?}: expected beforeDiscountSavings {before}, got {got_before}"
        );
        assert!(
            (got_after - after).abs() < 1e-9,
            "case {case:?}: expected afterDiscountSavings {after}, got {got_after}"
        );
    }

    // A filter name outside the modelled value set is refused, not ignored, and
    // the refusal reports the member and the accepted set without echoing the
    // rejected value back to the caller.
    let (status, body) = call(
        &router,
        OPERATION,
        &json!({"filters": [{"name": "ResourceArn", "values": ["vol-aaa1"]}]}),
    )
    .await;
    assert_refused(
        (status, body.clone()),
        400,
        "InvalidParameterValueException",
        "unknown-filter-name",
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        message.starts_with("1 validation error detected: Value at 'filters")
            && message.contains("member.name")
            && !message.contains("ResourceArn")
            && RECOMMENDED_ACTION_FILTER_NAME_VALUES
                .iter()
                .all(|name| message.contains(name)),
        "case \"unknown-filter-name\": the refusal must name the member and the accepted set \
         without echoing the rejected value, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The aggregate is one summary, so every page size serves it in a first page
/// that ends the walk — and a token this operation did not issue is refused
/// rather than answered with a page of something else.
#[tokio::test]
async fn one_page_holds_the_aggregate_and_a_foreign_token_is_refused() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    // `maxResults` carries a member `@range` of min 1 / max 100, which the
    // generated request walk enforces before the handler runs: each bound of
    // that range is served, and a value past either end is refused — including
    // 1000, the ceiling the member's PROSE names, which the range trait
    // overrides.
    for (case, max_results, expected_status) in [
        ("maxResults-at-the-range-minimum", 1, 200),
        ("maxResults-at-the-range-maximum", DEFAULT_MAX_RESULTS, 200),
        ("maxResults-below-the-range-minimum", 0, 400),
        (
            "maxResults-above-the-range-maximum",
            DEFAULT_MAX_RESULTS + 1,
            400,
        ),
        ("maxResults-at-the-documented-ceiling", 1000, 400),
        ("maxResults-negative", -1, 400),
    ] {
        let answered = call(&router, OPERATION, &json!({"maxResults": max_results})).await;
        if expected_status == 400 {
            assert_refused(answered, 400, "InvalidParameterValueException", case);
        } else {
            let (status, body) = answered;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
            );
            // Four actions, one aggregate: even a page of one ends the walk.
            assert_aggregate(case, &body, &(4, "USD", 37.5, 31.75, "AfterDiscount"));
            assert!(
                body.get("nextToken").is_none(),
                "case {case:?}: the aggregate fits any page, so no token is issued: {body}"
            );
        }
    }

    // A token this operation did not issue is refused: one that does not decode
    // at all, one whose cursor was issued for another result set, and the one
    // `ListRecommendedActions` issues for its own walk, which fingerprints its
    // own operation name into the cursor.
    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": "notatokenweissued"})).await,
        400,
        "InvalidParameterValueException",
        "foreign-token",
    );
    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": hex::encode("0:")})).await,
        400,
        "InvalidParameterValueException",
        "token-with-a-foreign-fingerprint",
    );
    let first = call_ok(
        &router,
        "ListRecommendedActions",
        &json!({"maxResults": 1}),
        "another-operations-token",
    )
    .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one of four actions leaves a further page")
        .to_string();
    assert_refused(
        call(&router, OPERATION, &json!({"nextToken": token})).await,
        400,
        "InvalidParameterValueException",
        "token-of-another-list-operation",
    );
}

// ---------------------------------------------------------------------------
// The enrollment gate
// ---------------------------------------------------------------------------

/// An account that is not opted in is refused the whole call, whatever the store
/// holds, rather than told there is nothing to summarise.
#[tokio::test]
async fn an_account_that_is_not_opted_in_is_refused() {
    let (router, engine) = unenrolled_emulator_with_engine();
    record(&engine, fixtures());

    for (case, request) in [
        ("unenrolled-no-filters", json!({})),
        (
            "unenrolled-with-filters",
            json!({"filters": [{"name": "RecommendedActionType", "values": [UPGRADE]}]}),
        ),
        ("unenrolled-with-max-results", json!({"maxResults": 1})),
    ] {
        assert_refused(
            call(&router, OPERATION, &request).await,
            403,
            "OptInRequiredException",
            case,
        );
    }
    assert_eq!(
        stored_count(&engine),
        fixtures().len(),
        "a refusal must not change the store"
    );

    // The gate is not the absence of data: once enrolled, the same store answers.
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());
    let listed = call_ok(&router, OPERATION, &json!({}), "enrolled").await;
    assert_aggregate("enrolled", &listed, &(4, "USD", 37.5, 31.75, "AfterDiscount"));
}
