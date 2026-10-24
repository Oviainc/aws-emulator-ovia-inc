// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendedActionSummaries`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0
//! `POST /` through the same [`MultiServiceRouter`] the emulator binary mounts —
//! so a response that is not computed from persisted state fails here.
//!
//! The rule under test: a summaries list reports ONE row per distinct value of
//! the dimension it groups by — `recommendedActionType` for a recommended action
//! — over the actions that pass the request's filters, in the order the store
//! first holds an action of that value, each row keyed by that value and
//! totalling only its own actions. A value nothing matched is no row, so a
//! request nothing matched reports no rows.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction, RECOMMENDED_ACTION_FILTER_NAME_VALUES,
    RECOMMENDED_ACTION_TYPE_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::DEFAULT_MAX_RESULTS;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token, member,
    unenrolled_emulator_with_engine,
};
use crate::recommended_actions::{record, stored, stored_count, MEMBER_ACCOUNT_ID};

/// The operation under test.
const OPERATION: &str = "ListRecommendedActionSummaries";

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

/// One recorded action, owned by `account_id`, of `recommended_action_type`,
/// carrying `estimate`.
///
/// Built on the one recommended-action fixture builder this crate has, with the
/// three members these cases vary overridden: the owning account, the action
/// type (which is the grouping dimension), and the estimate its group sums. An
/// action the store records NO type for is written by passing `None`.
fn action(
    index: usize,
    account_id: Option<&str>,
    recommended_action_type: Option<&str>,
    estimate: Option<EstimatedMonthlySavings>,
) -> RecommendedAction {
    RecommendedAction {
        account_id: account_id.map(ToString::to_string),
        recommended_action_type: recommended_action_type.map(ToString::to_string),
        estimated_monthly_savings: estimate,
        ..stored(
            index,
            &format!("vol-{index:04}"),
            account_id.unwrap_or(CALLER_ACCOUNT_ID),
            "us-east-1",
            14,
            recommended_action_type.unwrap_or(UPGRADE),
            Some("gp2"),
            false,
            &[("env", "prod")],
        )
    }
}

/// One summary as a response reports it: the grouping key, the group's count and
/// its aggregated estimate.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    key: String,
    count: i64,
    before: f64,
    after: f64,
    currency: String,
    mode: String,
}

/// An expected summary, written as a call so a table row stays readable.
fn expect(key: &str, count: i64, before: f64, after: f64, currency: &str, mode: &str) -> Summary {
    Summary {
        key: key.to_string(),
        count,
        before,
        after,
        currency: currency.to_string(),
        mode: mode.to_string(),
    }
}

/// The summaries one response reports, in response order. A response that
/// carries no summaries at all reads as the empty list, whether it renders the
/// member as `[]` or leaves it out.
fn reported(body: &Value) -> Vec<Summary> {
    let listed = member(body, "recommendedActionSummaries");
    if listed.is_null() {
        return Vec::new();
    }
    listed
        .as_array()
        .unwrap_or_else(|| panic!("recommendedActionSummaries is a list, got {body}"))
        .iter()
        .map(|summary| {
            let total = summary.get("total").unwrap_or(&Value::Null);
            let estimate = total.get("estimatedMonthlySavings").unwrap_or(&Value::Null);
            Summary {
                key: summary
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                count: total
                    .get("recommendedActionCount")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
                before: estimate
                    .get("beforeDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                after: estimate
                    .get("afterDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                currency: estimate
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mode: estimate
                    .get("savingsEstimationMode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// Compare what came back against what the rule says, naming the case and the
/// actual values. The figures are summed floating point, so they are compared
/// within a tolerance far below the smallest amount any row carries.
fn assert_summaries(case: &str, actual: &[Summary], expected: &[Summary]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "case {case:?}: expected {} summaries {expected:?}, got {actual:?}",
        expected.len()
    );
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert_eq!(
            actual.key, expected.key,
            "case {case:?}: expected group key {:?}, got {actual:?}",
            expected.key
        );
        assert_eq!(
            actual.count, expected.count,
            "case {case:?}: expected {} actions in group {:?}, got {actual:?}",
            expected.count, expected.key
        );
        assert!(
            (actual.before - expected.before).abs() < 1e-9,
            "case {case:?}: expected beforeDiscountSavings {} in group {:?}, got {actual:?}",
            expected.before,
            expected.key
        );
        assert!(
            (actual.after - expected.after).abs() < 1e-9,
            "case {case:?}: expected afterDiscountSavings {} in group {:?}, got {actual:?}",
            expected.after,
            expected.key
        );
        assert_eq!(
            actual.currency, expected.currency,
            "case {case:?}: expected currency {:?} in group {:?}, got {actual:?}",
            expected.currency, expected.key
        );
        assert_eq!(
            actual.mode, expected.mode,
            "case {case:?}: expected savingsEstimationMode {:?} in group {:?}, got {actual:?}",
            expected.mode, expected.key
        );
    }
}

// ---------------------------------------------------------------------------
// Grouping and totalling
// ---------------------------------------------------------------------------

/// A table spanning what the rule generalises over: how many actions the store
/// holds, how many distinct types they carry and in which order the store first
/// holds each, which accounts own them, whether they carry an estimate at all,
/// and whether a group's estimates agree on a currency and a savings mode.
#[tokio::test]
async fn summaries_group_the_stored_actions_by_action_type() {
    let scenarios: Vec<(&str, Vec<RecommendedAction>, Vec<Summary>)> = vec![
        // Nothing has been analysed yet, so there is no group to report: a row
        // for a grouping value no action carries would be a key the service
        // never emits.
        ("an-empty-store", vec![], vec![]),
        (
            "one-action-is-one-group",
            vec![action(
                1,
                Some(CALLER_ACCOUNT_ID),
                Some(UPGRADE),
                Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
            )],
            vec![expect(UPGRADE, 1, 12.5, 9.75, "USD", "AfterDiscount")],
        ),
        (
            "actions-of-one-type-are-summed-into-one-group",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 20.0, 18.0, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 0.25, 0.125, "AfterDiscount")),
                ),
            ],
            vec![expect(UPGRADE, 3, 32.75, 27.875, "USD", "AfterDiscount")],
        ),
        (
            "both-modelled-types-are-two-groups",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
            ],
            vec![
                expect(UPGRADE, 2, 15.0, 12.0, "USD", "AfterDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 7.0, 6.5, "USD", "AfterDiscount"),
            ],
        ),
        (
            // Same two groups, recorded the other way round: the rows follow the
            // order the store first holds each type, so the key set is the same
            // and the order is not fixed to one spelling.
            "the-groups-follow-the-order-the-store-first-holds-each-type",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            vec![
                expect(SNAPSHOT_AND_DELETE, 1, 7.0, 6.5, "USD", "AfterDiscount"),
                expect(UPGRADE, 1, 12.5, 9.75, "USD", "AfterDiscount"),
            ],
        ),
        (
            // A group spans every owning account: the dimension grouped by is
            // the action type, not the account.
            "a-group-spans-every-owning-account",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(MEMBER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 7.0, 6.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(THIRD_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 2.5, 2.25, "AfterDiscount")),
                ),
            ],
            vec![
                expect(UPGRADE, 2, 19.5, 16.25, "USD", "AfterDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 2.5, 2.25, "USD", "AfterDiscount"),
            ],
        ),
        (
            "an-action-the-store-holds-no-account-for-is-still-in-its-group",
            vec![
                action(
                    1,
                    None,
                    Some(UPGRADE),
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            vec![expect(UPGRADE, 2, 13.5, 10.25, "USD", "AfterDiscount")],
        ),
        (
            // The grouping value is what the row is keyed by, so an action
            // carrying none belongs to no group — and the groups that do exist
            // are unaffected by it.
            "an-action-the-store-holds-no-type-for-is-in-no-group",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    None,
                    Some(savings("USD", 99.0, 98.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
            ],
            vec![expect(UPGRADE, 1, 12.5, 9.75, "USD", "AfterDiscount")],
        ),
        (
            "an-action-without-an-estimate-is-counted-with-no-savings",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(2, Some(CALLER_ACCOUNT_ID), Some(UPGRADE), None),
            ],
            vec![expect(UPGRADE, 2, 12.5, 9.75, "USD", "AfterDiscount")],
        ),
        (
            "no-stored-action-carries-an-estimate",
            vec![
                action(1, Some(CALLER_ACCOUNT_ID), Some(UPGRADE), None),
                action(2, Some(CALLER_ACCOUNT_ID), Some(SNAPSHOT_AND_DELETE), None),
            ],
            // No recorded estimate names a currency, so neither group names one
            // and every sum is zero; the mode is the conservative reading.
            vec![
                expect(UPGRADE, 1, 0.0, 0.0, "", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 0.0, 0.0, "", "BeforeDiscount"),
            ],
        ),
        (
            // Agreement is judged per GROUP: the upgrade group's estimates
            // disagree on the mode, so it reports the conservative one, while
            // the other group keeps the mode its own estimate names.
            "a-groups-estimates-disagreeing-on-the-mode",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("USD", 1.0, 0.75, "AfterDiscount")),
                ),
            ],
            vec![
                expect(UPGRADE, 2, 17.5, 13.75, "USD", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 1.0, 0.75, "USD", "AfterDiscount"),
            ],
        ),
        (
            "a-groups-estimates-agreeing-on-before-discount",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 12.5, 9.75, "BeforeDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 5.0, 4.0, "BeforeDiscount")),
                ),
            ],
            vec![expect(UPGRADE, 2, 17.5, 13.75, "USD", "BeforeDiscount")],
        ),
        (
            // The emulator converts nothing, so a group whose estimates are not
            // in one currency names no currency while still summing both
            // figures; a group that does agree names its own.
            "a-groups-estimates-disagreeing-on-the-currency",
            vec![
                action(
                    1,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("EUR", 10.0, 8.0, "AfterDiscount")),
                ),
                action(
                    2,
                    Some(CALLER_ACCOUNT_ID),
                    Some(UPGRADE),
                    Some(savings("USD", 1.0, 0.5, "AfterDiscount")),
                ),
                action(
                    3,
                    Some(CALLER_ACCOUNT_ID),
                    Some(SNAPSHOT_AND_DELETE),
                    Some(savings("EUR", 4.0, 3.0, "AfterDiscount")),
                ),
            ],
            vec![
                expect(UPGRADE, 2, 11.0, 8.5, "", "AfterDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 4.0, 3.0, "EUR", "AfterDiscount"),
            ],
        ),
    ];

    // The table spans the whole modelled `RecommendedActionType` value set, so a
    // type added to the model cannot be left untested — the rows are keyed by
    // that value, so each must be reported as its own group.
    let covered: BTreeSet<String> = scenarios
        .iter()
        .flat_map(|(_, _, expected)| expected.iter().map(|summary| summary.key.clone()))
        .collect();
    let modelled: BTreeSet<String> = RECOMMENDED_ACTION_TYPE_VALUES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        covered, modelled,
        "every modelled RecommendedActionType must be expected as its own group somewhere \
         in the table"
    );

    for (case, actions, expected) in &scenarios {
        let (router, engine) = emulator_with_engine();
        record(&engine, actions.clone());
        let recorded = stored_count(&engine);

        let listed = call_ok(&router, OPERATION, &json!({}), case).await;
        assert_summaries(case, &reported(&listed), expected);
        assert!(
            listed.get("nextToken").is_none(),
            "case {case:?}: every group fits the default page, so no token is issued: {listed}"
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

/// For every filter list, the groups report exactly the actions
/// `ListRecommendedActions` reports for the SAME filters, grouped by their
/// `recommendedActionType` — so a filter nothing matches reports no group at
/// all.
#[tokio::test]
async fn filters_narrow_the_actions_each_group_totals() {
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

        // The oracle: group the actions that operation reported the same way,
        // in the order it first reported each type, keeping each group's key,
        // count and the two sums of its estimates. How a group names a currency
        // and a savings mode is the table above's business, so this cross-check
        // asserts the grouping itself.
        let mut expected: Vec<(String, i64, f64, f64)> = Vec::new();
        for listed in &actions {
            let key = listed
                .get("recommendedActionType")
                .and_then(Value::as_str)
                .expect("every fixture action carries its type")
                .to_string();
            let figure = |name: &str| -> f64 {
                listed
                    .get("estimatedMonthlySavings")
                    .and_then(|estimate| estimate.get(name))
                    .and_then(Value::as_f64)
                    .unwrap_or_default()
            };
            let before = figure("beforeDiscountSavings");
            let after = figure("afterDiscountSavings");
            match expected.iter_mut().find(|(group, ..)| group == &key) {
                Some((_, count, summed_before, summed_after)) => {
                    *count += 1;
                    *summed_before += before;
                    *summed_after += after;
                }
                None => expected.push((key, 1, before, after)),
            }
        }

        let summarised = call_ok(&router, OPERATION, &request, case).await;
        let got = reported(&summarised);
        assert_eq!(
            got.len(),
            expected.len(),
            "case {case:?}: expected {} groups {expected:?}, got {got:?}",
            expected.len()
        );
        for (got, (key, count, before, after)) in got.iter().zip(expected.iter()) {
            assert_eq!(
                &got.key, key,
                "case {case:?}: expected group key {key:?}, got {got:?}"
            );
            assert_eq!(
                got.count, *count,
                "case {case:?}: expected {count} actions in group {key:?}, got {got:?}"
            );
            assert!(
                (got.before - before).abs() < 1e-9,
                "case {case:?}: expected beforeDiscountSavings {before} in group {key:?}, \
                 got {got:?}"
            );
            assert!(
                (got.after - after).abs() < 1e-9,
                "case {case:?}: expected afterDiscountSavings {after} in group {key:?}, \
                 got {got:?}"
            );
        }

        // The invariants that hold whatever the filters are: the keys are
        // distinct, each is a value the matched actions actually carry, and the
        // counts partition the matched actions.
        let keys: BTreeSet<String> = got.iter().map(|summary| summary.key.clone()).collect();
        assert_eq!(
            keys.len(),
            got.len(),
            "case {case:?}: a grouping key is reported once, got {got:?}"
        );
        let present: BTreeSet<String> = actions
            .iter()
            .filter_map(|listed| {
                listed
                    .get("recommendedActionType")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect();
        assert_eq!(
            keys, present,
            "case {case:?}: the keys are the types the matched actions carry, got {got:?}"
        );
        let counted: i64 = got.iter().map(|summary| summary.count).sum();
        assert_eq!(
            counted,
            i64::try_from(actions.len()).expect("a small fixture set"),
            "case {case:?}: the group counts must add up to the {} matched actions, got {got:?}",
            actions.len()
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

/// Whatever the page size, a walk reports every group exactly once, in the same
/// order, and ends without a token.
#[tokio::test]
async fn paging_walks_every_group_once_for_every_page_size() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

    // The whole result, read in one page, is what a paged walk must reassemble.
    let whole = reported(&call_ok(&router, OPERATION, &json!({}), "one-page").await);
    assert_eq!(
        whole.len(),
        2,
        "the fixtures hold both action types, so two groups are expected, got {whole:?}"
    );

    for page_size in [1_i32, 2, 3, DEFAULT_MAX_RESULTS] {
        let case = format!("maxResults-{page_size}");
        let mut walked: Vec<Summary> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let body = call_ok(&router, OPERATION, &request, &case).await;
            let page = reported(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case:?}: a page holds at most {page_size} groups, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages and {walked:?}"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case {case:?}: a token matches the modelled NextToken pattern, got \
                         {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_summaries(&case, &walked, &whole);
    }

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
        }
    }

    // A page of one of two groups leaves a further page, and that page holds the
    // group the first one stopped before — the walk resumes, it does not restart.
    let first = call_ok(&router, OPERATION, &json!({"maxResults": 1}), "resume").await;
    assert_summaries("resume-first-page", &reported(&first), &whole[..1]);
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one of two groups leaves a further page")
        .to_string();
    let second = call_ok(
        &router,
        OPERATION,
        &json!({"maxResults": 1, "nextToken": token}),
        "resume",
    )
    .await;
    assert_summaries("resume-second-page", &reported(&second), &whole[1..]);
    assert!(
        second.get("nextToken").is_none(),
        "case \"resume-second-page\": the last page ends the walk: {second}"
    );
}

/// A token this operation did not issue for this very result set is refused
/// rather than answered with a page of something else.
#[tokio::test]
async fn a_token_resumes_only_the_filters_it_was_taken_with() {
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());

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

    // A token taken with one filter list does not resume another: the page it
    // would resume belongs to a different result set.
    let taken_with_filters = call_ok(
        &router,
        OPERATION,
        &json!({
            "maxResults": 1,
            "filters": [{"name": "AccountId", "values": [CALLER_ACCOUNT_ID, MEMBER_ACCOUNT_ID]}]
        }),
        "token-of-other-filters",
    )
    .await;
    let token = taken_with_filters
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page of one of two groups leaves a further page")
        .to_string();
    assert_refused(
        call(
            &router,
            OPERATION,
            &json!({"maxResults": 1, "nextToken": token.clone()}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "token-of-other-filters",
    );
    // The same token resumes the walk it WAS taken with.
    let resumed = call_ok(
        &router,
        OPERATION,
        &json!({
            "maxResults": 1,
            "nextToken": token,
            "filters": [{"name": "AccountId", "values": [CALLER_ACCOUNT_ID, MEMBER_ACCOUNT_ID]}]
        }),
        "token-of-the-same-filters",
    )
    .await;
    assert_summaries(
        "token-of-the-same-filters",
        &reported(&resumed),
        &[expect(
            SNAPSHOT_AND_DELETE,
            1,
            20.0,
            18.0,
            "USD",
            "AfterDiscount",
        )],
    );

    // The token `ListRecommendedActions` issues for its own walk fingerprints
    // that operation's name into the cursor, so it is refused here.
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

    // The gate is not the absence of data: once enrolled, the same store answers
    // with one group per action type it holds.
    let (router, engine) = emulator_with_engine();
    record(&engine, fixtures());
    let listed = call_ok(&router, OPERATION, &json!({}), "enrolled").await;
    assert_summaries(
        "enrolled",
        &reported(&listed),
        &[
            expect(UPGRADE, 3, 17.5, 13.75, "USD", "AfterDiscount"),
            expect(SNAPSHOT_AND_DELETE, 1, 20.0, 18.0, "USD", "AfterDiscount"),
        ],
    );

    // The groups partition the actions `ListRecommendedActions` reports for the
    // same (absent) filters, so no action is lost or double counted.
    let actions = call_ok(
        &router,
        "ListRecommendedActions",
        &json!({}),
        "enrolled-oracle",
    )
    .await;
    let mut per_type: BTreeMap<String, i64> = BTreeMap::new();
    for listed in member(&actions, "recommendedActions")
        .as_array()
        .expect("recommendedActions is a list")
    {
        let key = listed
            .get("recommendedActionType")
            .and_then(Value::as_str)
            .expect("every fixture action carries its type")
            .to_string();
        *per_type.entry(key).or_default() += 1;
    }
    let summarised: BTreeMap<String, i64> = reported(&listed)
        .into_iter()
        .map(|summary| (summary.key, summary.count))
        .collect();
    assert_eq!(
        summarised, per_type,
        "the group counts must match the listed actions per type, got {summarised:?}"
    );
}
