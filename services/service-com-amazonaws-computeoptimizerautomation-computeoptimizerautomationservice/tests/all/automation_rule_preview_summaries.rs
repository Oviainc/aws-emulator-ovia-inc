// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationRulePreviewSummaries`.
//!
//! The rule under test: the summaries report the SAME selection
//! `ListAutomationRulePreview` reports one by one — the recommended actions this
//! service holds for the accounts the rule type and `organizationScope` cover,
//! restricted to the requested `recommendedActionTypes` and to the `criteria`
//! conditions — reduced to one summary per distinct `recommendedActionType`
//! among them, each carrying how many of those actions it groups and the sums of
//! their estimated monthly savings, paged by `maxResults`.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /`
//! through the same router the emulator binary mounts, over an inventory seeded
//! where Compute Optimizer's own analysis would have written it (no operation of
//! this API mints a recommended action). So every expectation below is about what
//! the handler computes from persisted state.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token,
    unenrolled_emulator_with_engine,
};

/// The two values of the model's `RecommendedActionType`
/// (`smithy_query(query="shape",shape="RecommendedActionType")`), which is the
/// dimension a preview summary groups on.
const UPGRADE: &str = "UpgradeEbsVolumeType";
const SNAPSHOT_AND_DELETE: &str = "SnapshotAndDeleteUnattachedEbsVolume";

/// The one account every emulated caller presents.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A second, member account id for the organization-scope rows: analysis output
/// about another account of the organization is seeded, never minted, so a
/// literal 12-digit id (the model's `AccountId` pattern) stands in for it.
const MEMBER_ACCOUNT: &str = "210987654321";

/// One row of seeded analysis output, with only the members a summary reads off
/// it: which account and type it belongs to, the region a `criteria` condition
/// selects on, and its estimate — or `None` for an action Compute Optimizer has
/// estimated no savings for.
///
/// The preview suite's own seed builder fixes the currency at USD and carries a
/// volume configuration no summary reads, so these rows are built here: the
/// aggregation cases below vary exactly the two members
/// (`currency`, `savingsEstimationMode`) that one holds constant.
struct Action<'a> {
    id: &'a str,
    account: String,
    action_type: &'a str,
    region: &'a str,
    /// `(beforeDiscountSavings, afterDiscountSavings, currency, savingsEstimationMode)`.
    savings: Estimate<'a>,
}

/// What one seeded action's `estimatedMonthlySavings` carries, or `None` for an
/// action Compute Optimizer has estimated no savings for.
type Estimate<'a> = Option<(f64, f64, &'a str, &'a str)>;

/// One row of the aggregation table: the case, the estimates of the group's
/// actions, and the total the rule says the group reports.
type AggregationCase<'a> = (&'a str, Vec<Estimate<'a>>, Summary);

impl Action<'_> {
    fn stored(&self) -> RecommendedAction {
        let resource_id = format!("vol-{}", self.id.to_ascii_lowercase());
        RecommendedAction {
            recommended_action_id: Some(self.id.to_string()),
            resource_arn: Some(format!(
                "arn:aws:ec2:{}:{}:volume/{resource_id}",
                self.region, self.account
            )),
            resource_id: Some(resource_id),
            account_id: Some(self.account.clone()),
            region: Some(self.region.to_string()),
            resource_type: Some("EbsVolume".to_string()),
            look_back_period_in_days: Some(14),
            recommended_action_type: Some(self.action_type.to_string()),
            current_resource_summary: None,
            current_resource_details: None,
            recommended_resource_summary: None,
            recommended_resource_details: None,
            restart_needed: Some(false),
            estimated_monthly_savings: self.savings.map(|(before, after, currency, mode)| {
                EstimatedMonthlySavings {
                    currency: currency.to_string(),
                    before_discount_savings: before,
                    after_discount_savings: after,
                    savings_estimation_mode: mode.to_string(),
                }
            }),
            resource_tags: Some(Vec::new()),
        }
    }
}

/// The inventory the selection cases report over: three `UpgradeEbsVolumeType`
/// actions in the calling account (one of them with no estimate at all), two
/// `SnapshotAndDeleteUnattachedEbsVolume` ones, and one upgrade in a member
/// account — spanning three regions, so a `criteria` condition cuts across the
/// groups. Every figure is a multiple of 0.25, so each expected sum below is
/// exact in binary floating point.
///
/// `recommendedActionId` is the model's `^[0-9A-Za-z]{16}$`.
fn inventory() -> Vec<RecommendedAction> {
    [
        Action {
            id: "SA00000000000001",
            account: caller_account(),
            action_type: UPGRADE,
            region: "us-east-1",
            savings: Some((10.0, 6.0, "USD", "BeforeDiscount")),
        },
        Action {
            id: "SA00000000000002",
            account: caller_account(),
            action_type: SNAPSHOT_AND_DELETE,
            region: "us-east-1",
            savings: Some((100.0, 90.0, "USD", "AfterDiscount")),
        },
        Action {
            id: "SA00000000000003",
            account: caller_account(),
            action_type: UPGRADE,
            region: "eu-west-1",
            savings: Some((2.5, 1.25, "USD", "BeforeDiscount")),
        },
        Action {
            id: "SA00000000000004",
            account: caller_account(),
            action_type: UPGRADE,
            region: "ap-southeast-2",
            savings: None,
        },
        Action {
            id: "SA00000000000005",
            account: caller_account(),
            action_type: SNAPSHOT_AND_DELETE,
            region: "us-east-1",
            savings: Some((5.0, 4.0, "USD", "AfterDiscount")),
        },
        Action {
            id: "SA00000000000006",
            account: MEMBER_ACCOUNT.to_string(),
            action_type: UPGRADE,
            region: "us-east-1",
            savings: Some((7.0, 3.0, "USD", "BeforeDiscount")),
        },
    ]
    .iter()
    .map(Action::stored)
    .collect()
}

/// The crate's awsJson1_0 harness with the caller enrolled and the analysis
/// inventory holding `actions`. The engine comes back too, so a case can check
/// the store after a call.
fn emulator_with(actions: &[RecommendedAction]) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    seed_inventory(&engine, actions);
    (router, engine)
}

/// Record `actions` where Compute Optimizer's analysis writes them.
fn seed_inventory(engine: &Arc<engine::InMemoryEngine>, actions: &[RecommendedAction]) {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let mut guard = state.lock();
    for action in actions {
        let id = action
            .recommended_action_id
            .clone()
            .expect("a seeded action carries its id");
        guard.recommended_actions.insert(id, action.clone());
    }
}

/// Every stored recommended action, rendered: a read that changed any field of
/// any action in place would change this string.
fn inventory_snapshot(engine: &engine::InMemoryEngine) -> String {
    let state = service_state(engine).expect("the engine carries this service's state");
    let snapshot = format!(
        "{:?}",
        state
            .lock()
            .recommended_actions
            .iter_in_order()
            .collect::<Vec<_>>()
    );
    snapshot
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

/// The summaries a response lists, in response order.
fn summaries(body: &Value) -> Vec<Summary> {
    body.get("previewResultSummaries")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| {
            let total = summary.get("total").unwrap_or(&Value::Null);
            let savings = total.get("estimatedMonthlySavings").unwrap_or(&Value::Null);
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
                before: savings
                    .get("beforeDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                after: savings
                    .get("afterDiscountSavings")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                currency: savings
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mode: savings
                    .get("savingsEstimationMode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        })
        .collect()
}

/// The group keys a response lists, in response order.
fn keys(body: &Value) -> Vec<String> {
    summaries(body)
        .into_iter()
        .map(|summary| summary.key)
        .collect()
}

/// An expected summary, written as a tuple so a table row stays readable.
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

/// Compare what came back against what the rule says, naming the case and the
/// actual values. The figures are summed floating point, so they are compared
/// within a tolerance far below the smallest amount any row carries.
fn assert_summaries(actual: &[Summary], expected: &[Summary], case: &str) {
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

/// The selection a request describes, grouped by `recommendedActionType`: one
/// summary per type the selection holds, in the order the store first holds an
/// action of that type, each totalling exactly the actions of that type.
#[tokio::test]
async fn summaries_group_the_selected_actions_by_action_type() {
    let (router, _engine) = emulator_with(&inventory());

    // (case, request, the summaries the rule says come back)
    let cases: Vec<(&str, Value, Vec<Summary>)> = vec![
        (
            "both action types, no criteria: every action of the calling account",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            vec![
                // The upgrade with no estimate is one of the three actions and
                // adds nothing to either sum.
                expect(UPGRADE, 3, 12.5, 7.25, "USD", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 2, 105.0, 94.0, "USD", "AfterDiscount"),
            ],
        ),
        (
            "one action type: the other type is not selected, so it is no group",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [UPGRADE] }),
            vec![expect(UPGRADE, 3, 12.5, 7.25, "USD", "BeforeDiscount")],
        ),
        (
            "the other action type alone",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [SNAPSHOT_AND_DELETE] }),
            vec![expect(
                SNAPSHOT_AND_DELETE,
                2,
                105.0,
                94.0,
                "USD",
                "AfterDiscount",
            )],
        ),
        (
            "a criteria condition cuts across both groups",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }]
                },
            }),
            vec![
                expect(UPGRADE, 1, 10.0, 6.0, "USD", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 2, 105.0, 94.0, "USD", "AfterDiscount"),
            ],
        ),
        (
            "an organization preview with no scope spans every account the store holds \
             actions for",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            vec![
                expect(UPGRADE, 4, 19.5, 10.25, "USD", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 2, 105.0, 94.0, "USD", "AfterDiscount"),
            ],
        ),
        (
            "an organization scope naming only the member account: the type it holds no \
             action of is no group",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
            vec![expect(UPGRADE, 1, 7.0, 3.0, "USD", "BeforeDiscount")],
        ),
        (
            "criteria nothing satisfies: no action is selected, so there is no group",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["sa-east-1"] }]
                },
            }),
            vec![],
        ),
        (
            "a criteria condition on the estimate itself",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "estimatedMonthlySavings": [
                        { "comparison": "NumericGreaterThanEquals", "values": [10.0] }
                    ]
                },
            }),
            vec![
                expect(UPGRADE, 1, 10.0, 6.0, "USD", "BeforeDiscount"),
                expect(SNAPSHOT_AND_DELETE, 1, 100.0, 90.0, "USD", "AfterDiscount"),
            ],
        ),
    ];

    for (case, request, expected) in cases {
        let body = call_ok(
            &router,
            "ListAutomationRulePreviewSummaries",
            &request,
            case,
        )
        .await;
        assert_summaries(&summaries(&body), &expected, case);
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: an unpaged selection of at most two groups has no next page, got \
             {body}"
        );
    }
}

/// The aggregated estimate of a group: the sums of the before- and
/// after-discount figures its estimates carry, with the currency the group
/// agrees on and the mode it agrees on — and, where it agrees on neither, the
/// free-form `currency` naming none while the enum-typed
/// `savingsEstimationMode` falls back to the conservative before-discount
/// reading. A stored mode the model does not declare reads the same way: a total
/// can only name a value of `SavingsEstimationMode`.
#[tokio::test]
async fn a_group_total_aggregates_the_estimates_of_its_actions() {
    // (case, the group's estimates, the total the rule says it reports)
    let cases: Vec<AggregationCase> = vec![
        (
            "one estimate: the total is that estimate",
            vec![Some((10.0, 6.0, "USD", "BeforeDiscount"))],
            expect(UPGRADE, 1, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
        (
            "estimates agreeing on currency and mode",
            vec![
                Some((10.0, 6.0, "USD", "AfterDiscount")),
                Some((0.25, 0.125, "USD", "AfterDiscount")),
                Some((4.0, 2.0, "USD", "AfterDiscount")),
            ],
            expect(UPGRADE, 3, 14.25, 8.125, "USD", "AfterDiscount"),
        ),
        (
            "an action with no estimate: counted, adding nothing",
            vec![Some((10.0, 6.0, "USD", "BeforeDiscount")), None],
            expect(UPGRADE, 2, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
        (
            "no action of the group carries an estimate: the count stands, the sums are \
             zero and no currency is named",
            vec![None, None],
            expect(UPGRADE, 2, 0.0, 0.0, "", "BeforeDiscount"),
        ),
        (
            "estimates disagreeing on mode: the before-discount reading",
            vec![
                Some((10.0, 6.0, "USD", "BeforeDiscount")),
                Some((2.0, 1.0, "USD", "AfterDiscount")),
            ],
            expect(UPGRADE, 2, 12.0, 7.0, "USD", "BeforeDiscount"),
        ),
        (
            "estimates disagreeing on currency: the sums are not one currency's total, so \
             the total names no currency",
            vec![
                Some((10.0, 6.0, "USD", "BeforeDiscount")),
                Some((2.0, 1.0, "EUR", "BeforeDiscount")),
            ],
            expect(UPGRADE, 2, 12.0, 7.0, "", "BeforeDiscount"),
        ),
        (
            "a currency that is not the dollar, agreed on",
            vec![
                Some((10.0, 6.0, "JPY", "AfterDiscount")),
                Some((2.0, 1.0, "JPY", "AfterDiscount")),
            ],
            expect(UPGRADE, 2, 12.0, 7.0, "JPY", "AfterDiscount"),
        ),
        (
            "a stored mode the model's SavingsEstimationMode does not declare: the total \
             must name one of the two it does, so the conservative reading stands",
            vec![Some((10.0, 6.0, "USD", "NotAModeTheModelDeclares"))],
            expect(UPGRADE, 1, 10.0, 6.0, "USD", "BeforeDiscount"),
        ),
    ];

    for (case, estimates, expected) in cases {
        // One fresh store per row: the row IS the whole inventory.
        let actions: Vec<RecommendedAction> = estimates
            .iter()
            .enumerate()
            .map(|(index, savings)| {
                Action {
                    id: &format!("SB0000000000000{index}"),
                    account: caller_account(),
                    action_type: UPGRADE,
                    region: "us-east-1",
                    savings: *savings,
                }
                .stored()
            })
            .collect();
        let (router, _engine) = emulator_with(&actions);

        let body = call_ok(
            &router,
            "ListAutomationRulePreviewSummaries",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            case,
        )
        .await;
        assert_summaries(&summaries(&body), std::slice::from_ref(&expected), case);
    }
}

/// The summaries total exactly the actions the unsummarised preview lists for
/// the same request: same group keys in the same first-appearance order, same
/// counts, same savings sums. The two operations read one selection.
#[tokio::test]
async fn summaries_total_the_actions_the_unsummarised_preview_lists() {
    let (router, _engine) = emulator_with(&inventory());

    let requests: Vec<(&str, Value)> = vec![
        (
            "both action types over the calling account",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
            ),
        (
            "one action type",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [SNAPSHOT_AND_DELETE] }),
        ),
        (
            "the whole organization",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
        ),
        (
            "a scoped organization preview",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT, &caller_account()] },
            }),
        ),
        (
            "a criteria-restricted preview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringLike", "values": ["us-*"] }]
                },
            }),
        ),
    ];

    for (case, request) in requests {
        let previewed = call_ok(&router, "ListAutomationRulePreview", &request, case).await;
        let results = previewed
            .get("previewResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // The same grouping, computed here from the listed actions.
        let mut expected: Vec<Summary> = Vec::new();
        for result in &results {
            let action_type = result
                .get("recommendedActionType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let savings = result.get("estimatedMonthlySavings");
            let before = savings
                .and_then(|s| s.get("beforeDiscountSavings"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let after = savings
                .and_then(|s| s.get("afterDiscountSavings"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            match expected.iter_mut().find(|group| group.key == action_type) {
                Some(group) => {
                    group.count += 1;
                    group.before += before;
                    group.after += after;
                }
                None => expected.push(Summary {
                    key: action_type,
                    count: 1,
                    before,
                    after,
                    // The currency and mode of a group are asserted by the
                    // aggregation table above; here the counts and sums are
                    // what must agree, so these are carried over below.
                    currency: String::new(),
                    mode: String::new(),
                }),
            }
        }

        let summarised = call_ok(
            &router,
            "ListAutomationRulePreviewSummaries",
            &request,
            case,
        )
        .await;
        let actual = summaries(&summarised);
        assert_eq!(
            actual.len(),
            expected.len(),
            "case {case:?}: the {} previewed actions fall into {} groups, got {actual:?}",
            results.len(),
            expected.len()
        );
        let expected: Vec<Summary> = expected
            .into_iter()
            .zip(actual.iter())
            .map(|(group, actual)| Summary {
                currency: actual.currency.clone(),
                mode: actual.mode.clone(),
                ..group
            })
            .collect();
        assert_summaries(&actual, &expected, case);
        let total: i64 = actual.iter().map(|group| group.count).sum();
        assert_eq!(
            total,
            results.len() as i64,
            "case {case:?}: the group counts must total the {} previewed actions, got {actual:?}",
            results.len()
        );
    }
}

/// The summaries are a REPORT: the call leaves the inventory exactly as it was,
/// creates no rule, and answers the same thing when asked twice.
#[tokio::test]
async fn summarising_writes_nothing() {
    let (router, engine) = emulator_with(&inventory());
    let case = "a paged summary over a seeded inventory";
    let before = inventory_snapshot(&engine);

    let request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "maxResults": 1,
    });
    let first = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &request,
        case,
    )
    .await;
    let again = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &request,
        case,
    )
    .await;

    assert_eq!(
        summaries(&first),
        summaries(&again),
        "case {case:?}: two identical reads must report the same groups, got {first} then {again}"
    );
    assert_eq!(
        inventory_snapshot(&engine),
        before,
        "case {case:?}: a read must leave every stored action untouched"
    );
    let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
    assert_eq!(
        listed
            .get("automationRules")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case {case:?}: a preview summary creates no rule, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Walking with any page size visits every group exactly once, in the same
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_walks_every_group_once_for_every_page_size() {
    let (router, _engine) = emulator_with(&inventory());
    let request = json!({
        "ruleType": "OrganizationRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
    });
    let unpaged = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &request,
        "one unpaged page",
    )
    .await;
    let all = summaries(&unpaged);
    assert_eq!(
        keys(&unpaged),
        vec![UPGRADE.to_string(), SNAPSHOT_AND_DELETE.to_string()],
        "the whole organization holds an action of both types, got {unpaged}"
    );

    // Every page size from below the group count to far above it, and the
    // modelled maximum.
    for page_size in [1_u64, 2, 3, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<Summary> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut body = request.clone();
            body["maxResults"] = json!(page_size);
            if let Some(token) = next.as_deref() {
                body["nextToken"] = json!(token);
            }
            let response =
                call_ok(&router, "ListAutomationRulePreviewSummaries", &body, &case).await;
            let page = summaries(&response);
            assert!(
                page.len() as u64 <= page_size,
                "case {case:?}: a page holds at most maxResults summaries, got {page:?}"
            );
            walked.extend(page);
            match response.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        is_next_token(token),
                        "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                    );
                    next = Some(token.to_string());
                }
                None => break,
            }
            assert!(
                walked.len() <= all.len(),
                "case {case:?}: the walk must terminate, got {walked:?}"
            );
        }
        assert_summaries(&walked, &all, &case);
    }
}

/// A token is bound to the operation and the selection it was taken with:
/// resuming with anything else would answer a page of another result set, so it
/// is refused.
#[tokio::test]
async fn a_token_resumes_only_the_selection_it_was_taken_with() {
    let (router, _engine) = emulator_with(&inventory());
    let selection = json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        "maxResults": 1,
    });
    let first = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &selection,
        "first page",
    )
    .await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a two-group selection paged at one group has a next page")
        .to_string();
    // The token of the UNSUMMARISED preview of the same request, whose pages are
    // single actions rather than groups.
    let other_operation_token = call_ok(
        &router,
        "ListAutomationRulePreview",
        &selection,
        "the unsummarised preview's first page",
    )
    .await
    .get("nextToken")
    .and_then(Value::as_str)
    .expect("a five-action selection paged at one action has a next page")
    .to_string();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a token resumed with different action types",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token resumed with different criteria",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "criteria": {
                    "region": [{ "comparison": "StringEquals", "values": ["us-east-1"] }]
                },
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token resumed with a different rule type",
            json!({
                "ruleType": "OrganizationRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": token.clone(),
            }),
        ),
        (
            "the unsummarised preview's token",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": other_operation_token.clone(),
            }),
        ),
        (
            "a token this service never issued",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": "notAtokenThisServiceIssued",
            }),
        ),
    ];
    for (case, request) in cases {
        assert_refused(
            call(&router, "ListAutomationRulePreviewSummaries", &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // And the other way round: a summaries token is not a preview token either.
    assert_refused(
        call(
            &router,
            "ListAutomationRulePreview",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
                "nextToken": token.clone(),
            }),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "the summaries token on the unsummarised preview",
    );

    // The same token with the same selection still resumes, where the first page
    // stopped.
    let mut resumed_request = selection.clone();
    resumed_request["nextToken"] = json!(token);
    let resumed = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &resumed_request,
        "the token resumed with its own selection",
    )
    .await;
    assert_eq!(
        keys(&resumed),
        vec![SNAPSHOT_AND_DELETE.to_string()],
        "the token must resume exactly where the first page stopped, got {resumed}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A request the model admits but this service cannot evaluate is refused with
/// the operation's only client-input error — the same reading its unsummarised
/// sibling takes, so one request cannot be valid for a preview and invalid for
/// that preview's totals.
#[tokio::test]
async fn a_request_the_service_cannot_evaluate_is_refused() {
    let (router, engine) = emulator_with(&inventory());

    let cases: Vec<(&str, Value)> = vec![
        (
            "no action type to include",
            json!({ "ruleType": "AccountRule", "recommendedActionTypes": [] }),
        ),
        (
            "an organization scope on an account-scoped preview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "organizationScope": { "accountIds": [MEMBER_ACCOUNT] },
            }),
        ),
        (
            "a numeric comparison on a string criterion",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "region": [{ "comparison": "NumericGreaterThan", "values": ["us-east-1"] }]
                },
            }),
        ),
        (
            "a string comparison on a double criterion",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "estimatedMonthlySavings": [{ "comparison": "StringLike", "values": [1.0] }]
                },
            }),
        ),
        (
            "a condition with no comparison operator",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": { "region": [{ "values": ["us-east-1"] }] },
            }),
        ),
        (
            "a condition with no values",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": { "region": [{ "comparison": "StringEquals", "values": [] }] },
            }),
        ),
        (
            "a resourceTag condition that names no tag key",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE],
                "criteria": {
                    "resourceTag": [{ "comparison": "StringEquals", "values": ["Production"] }]
                },
            }),
        ),
    ];
    for (case, request) in &cases {
        assert_refused(
            call(&router, "ListAutomationRulePreviewSummaries", request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
        // The refused request is refused by the unsummarised preview too.
        assert_refused(
            call(&router, "ListAutomationRulePreview", request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // Nothing was written while those requests were refused.
    let snapshot = inventory_snapshot(&engine);
    assert!(
        snapshot.contains("SA00000000000001"),
        "a refused request must leave the inventory in place, got {snapshot}"
    );
}

/// `maxResults` is modelled `range(min: 1, max: 100)`, so each published bound
/// is accepted and a value one past either is refused at the protocol boundary.
#[tokio::test]
async fn max_results_honours_its_modelled_bounds() {
    let (router, _engine) = emulator_with(&inventory());
    let request = |max_results: i64| {
        json!({
            "ruleType": "OrganizationRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            "maxResults": max_results,
        })
    };

    for max_results in [1, 100] {
        let case = format!("maxResults={max_results} is a published bound");
        let body = call_ok(
            &router,
            "ListAutomationRulePreviewSummaries",
            &request(max_results),
            &case,
        )
        .await;
        let page = summaries(&body);
        assert!(
            !page.is_empty() && page.len() as i64 <= max_results,
            "case {case:?}: a page holds between one and maxResults summaries, got {page:?}"
        );
    }

    for max_results in [0, 101] {
        let case = format!("maxResults={max_results} is one past a published bound");
        let (status, body) = call(
            &router,
            "ListAutomationRulePreviewSummaries",
            &request(max_results),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a maxResults outside range(min: 1, max: 100) is refused, got \
             {status} and {body}"
        );
    }
}

/// The service-wide enrollment gate stands in front of this operation too: an
/// account that is not opted in is refused the call rather than handed an empty
/// summary.
#[tokio::test]
async fn an_unenrolled_account_is_refused() {
    let (router, engine) = unenrolled_emulator_with_engine();
    seed_inventory(&engine, &inventory());

    assert_refused(
        call(
            &router,
            "ListAutomationRulePreviewSummaries",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
            }),
        )
        .await,
        403,
        "OptInRequiredException",
        "an account that never opted in",
    );

    // Opting in over the wire opens the gate, and the summary then reports the
    // inventory that was there all along.
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        "opt in",
    )
    .await;
    let body = call_ok(
        &router,
        "ListAutomationRulePreviewSummaries",
        &json!({
            "ruleType": "AccountRule",
            "recommendedActionTypes": [UPGRADE, SNAPSHOT_AND_DELETE],
        }),
        "opted in",
    )
    .await;
    assert_summaries(
        &summaries(&body),
        &[
            expect(UPGRADE, 3, 12.5, 7.25, "USD", "BeforeDiscount"),
            expect(SNAPSHOT_AND_DELETE, 2, 105.0, 94.0, "USD", "AfterDiscount"),
        ],
        "opted in",
    );
}
