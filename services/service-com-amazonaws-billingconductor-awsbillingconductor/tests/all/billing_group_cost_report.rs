// SPDX-License-Identifier: CC0-1.0

//! `GetBillingGroupCostReport` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` (-> `BatchAssociateResourcesToCustomLineItem`) ->
//! `GetBillingGroupCostReport`.
//!
//! RULE: the margin summary of a billing group REPORTS, for each group of the breakdown
//! `GroupBy` names over the billing periods the half-open `BillingPeriodRange` covers —
//! the billing period the call is made in when the request names no range — the actual
//! Amazon Web Services charges of the group (its accounts' metered usage, which this
//! emulator records none of), the pro forma charges computed from the stored custom line
//! items in effect in those periods (a flat item contributes its charge value and a
//! percentage item its percentage of the combined value of the resources associated to
//! it, each signed by its `FEE`/`CREDIT` type and counted under the product its
//! presentation details name), the margin between the two and that margin as a share of
//! the pro forma amount; it writes nothing, so a later read sees exactly what the first
//! one saw.
//!
//! The rows span what the rule generalises over rather than one call: the charge kind of
//! an item (flat / percentage / a percentage of a percentage / a percentage of the
//! billing group), its type (fee / credit), its value (zero, fractional, whole), how many
//! items a group holds, how the association was made (at create / by a later batch call),
//! how the associations between items are arranged (a chain, two items sharing one base,
//! two items feeding each other), where an item's own billing period range falls relative
//! to the requested one, which attributes the breakdown names and in which order, how
//! many periods the range covers (one, several, the published maximum and one past it),
//! how a billing period is spelled (`2026-1` / `2026-01`), which form the billing group
//! ARN takes (full ARN / bare id) and the page size a caller asks for.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// The product a custom line item with no `PresentationDetails` presents its charges
/// under: the name this service's own model gives it
/// (`smithy_query(query="service_info")`: `AWSBillingConductor`).
const OWN_PRODUCT_NAME: &str = "AWSBillingConductor";

/// The currency of the `aws` partition every ARN this emulator mints lives in
/// (`CurrencyCode` has exactly `CNY` and `USD`;
/// `smithy_query(query="shape",shape="CurrencyCode")`).
const PARTITION_CURRENCY: &str = "USD";

/// How far two amounts may differ and still be the same amount: the reported strings are
/// decimals rounded well inside this, so a comparison at this tolerance pins the amount
/// without pinning the rendering.
const TOLERANCE: f64 = 1e-6;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): the store records no payer account by default, so this harness designates
    /// one — the world in which the read under test is reachable at all. The denial every
    /// other caller gets is asserted in `tests/all/payer_account_gate.rs`.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send a JSON body to one of this service's routes with the method the model binds
    /// that route to, and return the status plus the parsed response body.
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a billing group and return its ARN.
    async fn billing_group(&self, name: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PRICING_PLAN },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case billing group {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// Create a custom line item from the members `request` carries, filling in the ones
    /// every create needs, and return its ARN.
    async fn custom_line_item(&self, group_arn: &str, name: &str, request: Value) -> String {
        let mut body = json!({
            "Name": name,
            "Description": format!("the {name} custom line item"),
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
        });
        for (member, value) in members(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/create-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {response}"
        );
        arn_of(&response, &format!("custom line item {name:?}"))
    }

    /// Associate `resources` to the percentage custom line item `target`.
    async fn associate(&self, target: &str, resources: &[String]) {
        let (status, body) = self
            .send(
                "PUT",
                "/batch-associate-resources-to-custom-line-item",
                &json!({ "TargetArn": target, "ResourceArns": resources }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case associate {resources:?} to {target}: expected HTTP 200, got {status} with {body}"
        );
    }

    /// `GetBillingGroupCostReport` with whatever members the caller wants on the request.
    async fn report(&self, request: Value) -> (u16, Value) {
        self.send("POST", "/get-billing-group-cost-report", &request)
            .await
    }

    /// The rows of a SUCCESSFUL report, in response order.
    async fn rows(&self, case: &str, request: Value) -> Vec<Value> {
        let (status, body) = self.report(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: one page holds every row here, so no NextToken is due; got {body}"
        );
        results(&body)
    }
}

/// The members of a JSON object, or none when it is not one.
fn members(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// The elements of a report's `BillingGroupCostReportResults`, in response order.
fn results(body: &Value) -> Vec<Value> {
    body.get("BillingGroupCostReportResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One reported amount, read as the decimal its string carries.
fn amount(row: &Value, member: &str, case: &str) -> f64 {
    let text = row
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {row}"));
    text.parse::<f64>().unwrap_or_else(|_| {
        panic!("case {case:?}: expected {member} to be a decimal amount, got {text:?}")
    })
}

/// Assert one reported amount, naming the case and the actual value.
fn assert_amount(row: &Value, member: &str, expected: f64, case: &str) {
    let actual = amount(row, member, case);
    assert!(
        (actual - expected).abs() < TOLERANCE,
        "case {case:?}: expected {member} {expected}, got {actual} in {row}"
    );
}

/// Assert the whole money side of one row: the actual cost is zero, the pro forma cost is
/// `proforma`, the margin is the difference and the margin percentage is that margin as a
/// share of the pro forma amount (zero where there is no pro forma amount).
fn assert_summary(row: &Value, group_arn: &str, proforma: f64, case: &str) {
    assert_eq!(
        row.get("Arn").and_then(Value::as_str),
        Some(group_arn),
        "case {case:?}: expected the billing group's own Arn, got {row}"
    );
    assert_eq!(
        row.get("Currency").and_then(Value::as_str),
        Some(PARTITION_CURRENCY),
        "case {case:?}: expected the partition's currency, got {row}"
    );
    assert_amount(row, "AWSCost", 0.0, case);
    assert_amount(row, "ProformaCost", proforma, case);
    assert_amount(row, "Margin", proforma, case);
    let expected_percentage = if proforma == 0.0 { 0.0 } else { 100.0 };
    assert_amount(row, "MarginPercentage", expected_percentage, case);
}

/// The `Attributes` of one row as `(Key, Value)` pairs, in response order.
fn attributes(row: &Value) -> Vec<(String, String)> {
    row.get("Attributes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|attribute| {
                    (
                        attribute
                            .get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        attribute
                            .get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The bare resource id of a full ARN — the OTHER form the `BillingGroupArn` pattern
/// admits, so a request may name the group either way.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// RULE: the pro forma cost of a billing group is the SIGNED sum of the charges of the
/// custom line items in effect for it, the actual cost is the metered usage this emulator
/// records (none), and the margin and its percentage follow from the two.
#[tokio::test]
async fn the_proforma_cost_is_the_signed_sum_of_the_custom_line_items_in_effect() {
    let em = Emulator::new();

    // Each row is its OWN billing group, so the groups differ along the charge kinds they
    // hold while the request stays the same. (case, group ARN, expected pro forma cost)
    let mut cases: Vec<(&str, String, f64)> = Vec::new();

    cases.push(("a group with no custom line item", em.billing_group("empty").await, 0.0));

    let one_fee = em.billing_group("onefee").await;
    em.custom_line_item(
        &one_fee,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    cases.push(("one flat fee", one_fee, 100.0));

    let two_fees = em.billing_group("twofees").await;
    em.custom_line_item(
        &two_fees,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    em.custom_line_item(
        &two_fees,
        "feehalf",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 0.5 } } }),
    )
    .await;
    cases.push(("two flat fees, one of them fractional", two_fees, 100.5));

    let only_credit = em.billing_group("onlycredit").await;
    em.custom_line_item(
        &only_credit,
        "credit25",
        json!({ "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 25.0 } } }),
    )
    .await;
    cases.push(("one flat credit, which comes OFF the bill", only_credit, -25.0));

    let fee_and_credit = em.billing_group("feeandcredit").await;
    em.custom_line_item(
        &fee_and_credit,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    em.custom_line_item(
        &fee_and_credit,
        "credit25",
        json!({ "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 25.0 } } }),
    )
    .await;
    cases.push(("a flat fee and a flat credit", fee_and_credit, 75.0));

    let zero_valued = em.billing_group("zerovalued").await;
    em.custom_line_item(
        &zero_valued,
        "fee0",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 0.0 } } }),
    )
    .await;
    cases.push(("a fee worth the modelled minimum charge value", zero_valued, 0.0));

    // A percentage item is "multiplied against the combined value of its associated
    // resources": 10% of the 200 the flat item charges, beside that 200 itself.
    let percentage = em.billing_group("percentage").await;
    let percentage_base = em
        .custom_line_item(
            &percentage,
            "fee200",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 200.0 } } }),
        )
        .await;
    em.custom_line_item(
        &percentage,
        "tenpercent",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 10.0, "AssociatedValues": [percentage_base] },
            }
        }),
    )
    .await;
    cases.push(("a percentage fee associated at create", percentage, 220.0));

    // The same arithmetic with the association made by a LATER batch call, and the
    // percentage item a CREDIT: half of the 10 charged, taken off the bill.
    let percentage_credit = em.billing_group("percentagecredit").await;
    let credit_base = em
        .custom_line_item(
            &percentage_credit,
            "fee10",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } } }),
        )
        .await;
    let half_credit = em
        .custom_line_item(
            &percentage_credit,
            "halfcredit",
            json!({
                "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 50.0 } }
            }),
        )
        .await;
    em.associate(&half_credit, &[credit_base]).await;
    cases.push((
        "a percentage credit associated by a later batch call",
        percentage_credit,
        5.0,
    ));

    // A percentage item may feed another one: 100 flat, 10% of it, then 50% of that.
    let chained = em.billing_group("chained").await;
    let chain_base = em
        .custom_line_item(
            &chained,
            "fee100",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
        )
        .await;
    let chain_middle = em
        .custom_line_item(
            &chained,
            "tenpercent",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": { "PercentageValue": 10.0, "AssociatedValues": [chain_base] },
                }
            }),
        )
        .await;
    em.custom_line_item(
        &chained,
        "halfofthat",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 50.0, "AssociatedValues": [chain_middle] },
            }
        }),
    )
    .await;
    cases.push(("a percentage of a percentage", chained, 115.0));

    // A percentage of the BILLING GROUP is a percentage of its metered usage, of which
    // this emulator records none — so it adds nothing beside the flat item.
    let group_percentage = em.billing_group("grouppercentage").await;
    em.custom_line_item(
        &group_percentage,
        "fee40",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 40.0 } } }),
    )
    .await;
    em.custom_line_item(
        &group_percentage,
        "groupshare",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": {
                    "PercentageValue": 10.0,
                    "AssociatedValues": [group_percentage.clone()],
                },
            }
        }),
    )
    .await;
    cases.push((
        "a percentage of the billing group's own metered usage",
        group_percentage,
        40.0,
    ));

    let unassociated = em.billing_group("unassociated").await;    em.custom_line_item(
        &unassociated,
        "quarter",
        json!({
            "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 25.0 } }
        }),
    )
    .await;
    cases.push(("a percentage with nothing associated to it", unassociated, 0.0));

    // Two percentage items associated to the SAME flat item: each takes its own share of
    // the one value, which is counted once beside them.
    let shared = em.billing_group("shared").await;
    let shared_base = em
        .custom_line_item(
            &shared,
            "fee100",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
        )
        .await;
    em.custom_line_item(
        &shared,
        "tenth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": {
                    "PercentageValue": 10.0,
                    "AssociatedValues": [shared_base.clone()],
                },
            }
        }),
    )
    .await;
    em.custom_line_item(
        &shared,
        "fifth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 20.0, "AssociatedValues": [shared_base] },
            }
        }),
    )
    .await;
    cases.push((
        "two percentage items sharing one associated item",
        shared,
        130.0,
    ));

    // Nothing stops two percentage items from being associated to each other. Neither has
    // a settled value to lend the other, so the pair is worth nothing — and the report
    // still answers rather than walking the loop.
    let cyclic = em.billing_group("cyclic").await;
    let first_half = em
        .custom_line_item(
            &cyclic,
            "firsthalf",
            json!({
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 50.0 } }
            }),
        )
        .await;
    let second_half = em
        .custom_line_item(
            &cyclic,
            "secondhalf",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": {
                        "PercentageValue": 50.0,
                        "AssociatedValues": [first_half.clone()],
                    },
                }
            }),
        )
        .await;
    em.associate(&first_half, &[second_half]).await;
    cases.push((
        "two percentage items associated to each other",
        cyclic,
        0.0,
    ));

    // An item whose own billing period range ended long ago is in effect in no period the
    // report covers, so none of its charge is reported.
    let expired = em.billing_group("expired").await;
    em.custom_line_item(
        &expired,
        "fee500",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 500.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": "2020-01",
                "ExclusiveEndBillingPeriod": "2020-02",
            },
        }),
    )
    .await;
    cases.push(("an item in effect only in a past billing period", expired, 0.0));

    for (case, group_arn, proforma) in cases {
        let rows = em.rows(case, json!({ "Arn": group_arn.clone() })).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: a report with no GroupBy is one row for the group, got {rows:?}"
        );
        assert_summary(&rows[0], &group_arn, proforma, case);
        assert_eq!(
            rows[0].get("Attributes"),
            None,
            "case {case:?}: no breakdown was asked for, so no Attributes are due; got {:?}",
            rows[0]
        );
    }
}

/// RULE: a report grouped by billing period reports one row per period of the requested
/// range that the group has a charge in, each carrying that period's own charges and the
/// period as its attribute in the canonical spelling.
#[tokio::test]
async fn a_report_grouped_by_billing_period_reports_each_period_of_the_range() {
    let em = Emulator::new();
    let group = em.billing_group("periods").await;

    // Three items differing in where their own half-open range falls: every period, one
    // period only, and from a period on with no end at all.
    em.custom_line_item(
        &group,
        "always",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } } }),
    )
    .await;
    em.custom_line_item(
        &group,
        "januaryonly",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": "2026-01",
                "ExclusiveEndBillingPeriod": "2026-02",
            },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "februaryonwards",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 5.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-02" },
        }),
    )
    .await;

    // The range is spelled with a single-digit month, which the `BillingPeriod` pattern
    // admits; the reported attribute is the canonical zero-padded period either way.
    let rows = em
        .rows(
            "three periods grouped by billing period",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-1",
                    "ExclusiveEndBillingPeriod": "2026-04",
                },
                "GroupBy": ["BILLING_PERIOD"],
            }),
        )
        .await;

    // (the period the row is for, its pro forma cost)
    let expected = [("2026-01", 11.0), ("2026-02", 6.0), ("2026-03", 6.0)];
    assert_eq!(
        rows.len(),
        expected.len(),
        "case three periods grouped by billing period: expected one row per period, got {rows:?}"
    );
    for (row, (period, proforma)) in rows.iter().zip(expected) {
        let case = format!("the {period} row");
        assert_eq!(
            attributes(row),
            vec![("BILLING_PERIOD".to_string(), period.to_string())],
            "case {case:?}: expected the period as the row's one attribute, got {row}"
        );
        assert_summary(row, &group, proforma, &case);
    }

    // The same range with NO breakdown is the one row that aggregates those periods.
    let whole = em
        .rows(
            "the same range with no breakdown",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-01",
                    "ExclusiveEndBillingPeriod": "2026-04",
                },
            }),
        )
        .await;
    assert_eq!(
        whole.len(),
        1,
        "case the same range with no breakdown: expected one row, got {whole:?}"
    );
    assert_summary(&whole[0], &group, 23.0, "the same range with no breakdown");
}

/// RULE: a report grouped by product name reports one row per product the group's charges
/// are presented under, and naming several attributes breaks the summary down by all of
/// them, in the order the request listed them.
#[tokio::test]
async fn a_report_grouped_by_product_name_reports_each_presented_product() {
    let em = Emulator::new();
    let group = em.billing_group("products").await;

    // Items differing in the product they present under: two under one service, one under
    // another, one under none at all, and one of them bounded to the second period.
    em.custom_line_item(
        &group,
        "storage",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "PresentationDetails": { "Service": "S3" },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "storageextra",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 2.0 } },
            "PresentationDetails": { "Service": "S3" },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": "2026-02",
                "ExclusiveEndBillingPeriod": "2026-03",
            },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "computecredit",
        json!({
            "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 3.0 } },
            "PresentationDetails": { "Service": "EC2" },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "unpresented",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 7.0 } } }),
    )
    .await;

    let two_months = json!({
        "InclusiveStartBillingPeriod": "2026-01",
        "ExclusiveEndBillingPeriod": "2026-03",
    });

    // By product over both periods: each product's charges summed across the range.
    for (case, group_by) in [
        ("grouped by product name", json!(["PRODUCT_NAME"])),
        (
            // Naming one attribute twice breaks the summary down no further.
            "grouped by product name twice",
            json!(["PRODUCT_NAME", "PRODUCT_NAME"]),
        ),
    ] {
        let rows = em
            .rows(
                case,
                json!({
                    "Arn": group.clone(),
                    "BillingPeriodRange": two_months.clone(),
                    "GroupBy": group_by,
                }),
            )
            .await;
        // Ordered by product name: this service's own, then EC2, then S3.
        let expected = [(OWN_PRODUCT_NAME, 14.0), ("EC2", -6.0), ("S3", 22.0)];
        assert_eq!(
            rows.len(),
            expected.len(),
            "case {case:?}: expected one row per presented product, got {rows:?}"
        );
        for (row, (product, proforma)) in rows.iter().zip(expected) {
            let row_case = format!("{case}: the {product} row");
            assert_eq!(
                attributes(row),
                vec![("PRODUCT_NAME".to_string(), product.to_string())],
                "case {row_case:?}: expected the product as the row's one attribute, got {row}"
            );
            assert_summary(row, &group, proforma, &row_case);
        }
    }

    // By product AND period: one row per pair, with the attributes in the order asked for.
    let case = "grouped by product name and billing period";
    let rows = em
        .rows(
            case,
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": two_months,
                "GroupBy": ["PRODUCT_NAME", "BILLING_PERIOD"],
            }),
        )
        .await;
    // (period, product, pro forma cost), in the order the rows run.
    let expected = [
        ("2026-01", OWN_PRODUCT_NAME, 7.0),
        ("2026-01", "EC2", -3.0),
        ("2026-01", "S3", 10.0),
        ("2026-02", OWN_PRODUCT_NAME, 7.0),
        ("2026-02", "EC2", -3.0),
        ("2026-02", "S3", 12.0),
    ];
    assert_eq!(
        rows.len(),
        expected.len(),
        "case {case:?}: expected one row per product and period, got {rows:?}"
    );
    for (row, (period, product, proforma)) in rows.iter().zip(expected) {
        let row_case = format!("{case}: the {product} row of {period}");
        assert_eq!(
            attributes(row),
            vec![
                ("PRODUCT_NAME".to_string(), product.to_string()),
                ("BILLING_PERIOD".to_string(), period.to_string()),
            ],
            "case {row_case:?}: expected both attributes in the requested order, got {row}"
        );
        assert_summary(row, &group, proforma, &row_case);
    }
}

/// RULE: a request outside the rules the model and the documentation state is REFUSED —
/// a range that covers no period or more than the published maximum, and a billing group
/// that does not exist — while every request inside them is served, in either form the
/// `BillingGroupArn` pattern admits and at either end of the modelled page size.
#[tokio::test]
async fn a_report_outside_the_stated_rules_is_refused() {
    let em = Emulator::new();
    let group = em.billing_group("refusals").await;
    em.custom_line_item(
        &group,
        "fee8",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 8.0 } } }),
    )
    .await;

    // (case, the BillingPeriodRange member, the expected status)
    // The published maximum is 12 months ("You can specify up to 12 months",
    // `smithy_query(query="doc",shape="GetBillingGroupCostReportInput",member="BillingPeriodRange")`):
    // each bound of the range is tested, and one month past it.
    let ranges: Vec<(&str, Value, u16)> = vec![
        (
            "a one-month range",
            json!({
                "InclusiveStartBillingPeriod": "2026-05",
                "ExclusiveEndBillingPeriod": "2026-06",
            }),
            200,
        ),
        (
            "a range of the published maximum twelve months",
            json!({
                "InclusiveStartBillingPeriod": "2026-01",
                "ExclusiveEndBillingPeriod": "2027-01",
            }),
            200,
        ),
        (
            "a range one month past the published maximum",
            json!({
                "InclusiveStartBillingPeriod": "2026-01",
                "ExclusiveEndBillingPeriod": "2027-02",
            }),
            400,
        ),
        (
            "a range whose exclusive end is its inclusive start",
            json!({
                "InclusiveStartBillingPeriod": "2026-05",
                "ExclusiveEndBillingPeriod": "2026-05",
            }),
            400,
        ),
        (
            "a range whose end precedes its start",
            json!({
                "InclusiveStartBillingPeriod": "2026-06",
                "ExclusiveEndBillingPeriod": "2026-05",
            }),
            400,
        ),
        (
            "a reversed range spelled with single-digit months",
            json!({
                "InclusiveStartBillingPeriod": "2026-6",
                "ExclusiveEndBillingPeriod": "2026-5",
            }),
            400,
        ),
    ];

    for (case, range, expected) in ranges {
        let (status, body) = em
            .report(json!({ "Arn": group.clone(), "BillingPeriodRange": range }))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {body}"
            );
            assert_eq!(
                body.get("Reason").and_then(Value::as_str),
                Some("ILLEGAL_BILLING_PERIOD_RANGE"),
                "case {case:?}: expected the modelled range reason, got {body}"
            );
        }
    }

    // A billing group nothing created is a reference to a resource that doesn't exist.
    let (status, body) = em.report(json!({ "Arn": "NOSUCHGROUP0" })).await;
    assert_eq!(
        status, 404,
        "case an unknown billing group: expected HTTP 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case an unknown billing group: expected the modelled not-found, got {body}"
    );
    assert_eq!(
        body.get("ResourceType").and_then(Value::as_str),
        Some("billinggroup"),
        "case an unknown billing group: expected the resource type, got {body}"
    );

    // The bare resource id names the same group the full ARN does, so it reports the same
    // summary; and both ends of the modelled MaxResults range serve the whole page.
    for (case, request) in [
        ("the group by bare resource id", json!({ "Arn": bare_id(&group) })),
        (
            "the minimum modelled page size",
            json!({ "Arn": group.clone(), "MaxResults": 200 }),
        ),
        (
            "the maximum modelled page size",
            json!({ "Arn": group.clone(), "MaxResults": 300 }),
        ),
    ] {
        let rows = em.rows(case, request).await;
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected the group's one row, got {rows:?}"
        );
        assert_summary(&rows[0], &group, 8.0, case);
    }

    // A page size outside the modelled range, and an attribute outside the enumerated
    // breakdown, are refused at the boundary.
    for (case, request) in [
        (
            "a page size below the modelled minimum",
            json!({ "Arn": group.clone(), "MaxResults": 100 }),
        ),
        (
            "a page size above the modelled maximum",
            json!({ "Arn": group.clone(), "MaxResults": 301 }),
        ),
        (
            "an attribute outside the enumerated breakdown",
            json!({ "Arn": group.clone(), "GroupBy": ["NOT_AN_ATTRIBUTE"] }),
        ),
    ] {
        let (status, body) = em.report(request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {body}"
        );
    }
}

/// RULE: the report is a READ — it writes nothing, so reading it again, and reading it
/// after another read, reports exactly the same summary.
#[tokio::test]
async fn a_report_writes_nothing_so_a_later_read_sees_the_same_summary() {
    let em = Emulator::new();
    let group = em.billing_group("stable").await;
    let base = em
        .custom_line_item(
            &group,
            "fee60",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 60.0 } } }),
        )
        .await;
    em.custom_line_item(
        &group,
        "fifth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 20.0, "AssociatedValues": [base] },
            }
        }),
    )
    .await;

    // Rows differ along the request that is repeated: no breakdown, each attribute, and
    // both of them over a multi-period range.
    let requests: Vec<(&str, Value)> = vec![
        ("no breakdown", json!({ "Arn": group.clone() })),
        (
            "grouped by billing period",
            json!({ "Arn": group.clone(), "GroupBy": ["BILLING_PERIOD"] }),
        ),
        (
            "grouped by product name",
            json!({ "Arn": group.clone(), "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "grouped by both over three periods",
            json!({
                "Arn": group.clone(),
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-01",
                    "ExclusiveEndBillingPeriod": "2026-04",
                },
                "GroupBy": ["BILLING_PERIOD", "PRODUCT_NAME"],
            }),
        ),
    ];

    for (case, request) in requests {
        let (first_status, first) = em.report(request.clone()).await;
        let (second_status, second) = em.report(request).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case:?}: a read changes nothing, so the second read must answer the same; \
             got {first_status} with {first} then {second_status} with {second}"
        );
    }

    // And the pro forma cost is still the one the stored items describe: 60 plus a fifth
    // of it.
    let rows = em
        .rows("after four reads", json!({ "Arn": group.clone() }))
        .await;
    assert_eq!(
        rows.len(),
        1,
        "case after four reads: expected the group's one row, got {rows:?}"
    );
    assert_summary(&rows[0], &group, 72.0, "after four reads");
}
