// SPDX-License-Identifier: CC0-1.0

//! `ListBillingGroupCostReports` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` (-> `BatchAssociateResourcesToCustomLineItem`) ->
//! `ListBillingGroupCostReports`.
//!
//! RULE: the listing REPORTS one summary row per billing group that existed in the one
//! `BillingPeriod` the request names — the billing period the call is made in when it
//! names none — carrying the group's own ARN, the actual Amazon Web Services charges of
//! the group (its accounts' metered usage, which this emulator records none of), the pro
//! forma charges of every custom line item in effect for it in that period (signed by
//! each item's `FEE`/`CREDIT` type and summed over the products they are presented
//! under, because this listing reports no breakdown), the margin between the two, that
//! margin as a share of the pro forma amount and the currency of the group's partition;
//! `Filters.BillingGroupArns` narrows the rows to the groups it names, the rows run in
//! creation order and are cut into `MaxResults` pages resumed by `NextToken`, and the
//! call writes nothing, so a later read sees exactly what the first one saw.
//!
//! The rows span what the rule generalises over rather than one call: how many custom
//! line items a group holds (none, one, several), the charge kind of an item (flat,
//! percentage, a percentage of a percentage), its type (fee / credit), its value (zero,
//! fractional, whole), the product it is presented under (this service's own, another
//! service's, several at once), where an item's own billing period range falls relative
//! to the one asked about, which groups the filter names (none, one, several, one that
//! exists nowhere) and in which spelling (full ARN / bare id), which billing period is
//! asked about (before the group existed, the one it was created in, a later one) and in
//! which spelling (`2026-1` / `2026-01`), and the page size a caller asks for (each
//! published bound of the modelled range and one past it).

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

/// The currency of the `aws` partition every ARN this emulator mints lives in
/// (`CurrencyCode` has exactly `CNY` and `USD`;
/// `smithy_query(query="shape",shape="CurrencyCode")`).
const PARTITION_CURRENCY: &str = "USD";

/// The smallest page this operation may be asked for: the minimum of the modelled
/// `MaxBillingGroupResults` range
/// (`smithy_query(query="operation",operation="ListBillingGroupCostReports")`: 1..100).
const MIN_MAX_RESULTS: i64 = 1;

/// The largest page this operation may be asked for: the maximum of that same modelled
/// range.
const MAX_MAX_RESULTS: i64 = 100;

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

    /// `ListBillingGroupCostReports` with whatever members the caller wants on the
    /// request.
    async fn list(&self, request: Value) -> (u16, Value) {
        self.send("POST", "/list-billing-group-cost-reports", &request)
            .await
    }

    /// The whole body of a SUCCESSFUL listing.
    async fn listed(&self, case: &str, request: Value) -> Value {
        let (status, body) = self.list(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body
    }

    /// The rows of a SUCCESSFUL listing that fits in one page, in response order.
    async fn rows(&self, case: &str, request: Value) -> Vec<Value> {
        let body = self.listed(case, request).await;
        assert_eq!(
            body.get("NextToken"),
            None,
            "case {case:?}: one page holds every row here, so no NextToken is due; got {body}"
        );
        reports(&body)
    }

    /// `GetBillingGroupCostReport` for ONE group with no breakdown — the single-group
    /// report of the same billing period the listing answers for by default.
    async fn group_report(&self, case: &str, group_arn: &str) -> Vec<Value> {
        let (status, body) = self
            .send(
                "POST",
                "/get-billing-group-cost-report",
                &json!({ "Arn": group_arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("BillingGroupCostReportResults")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
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

/// The elements of a listing's `BillingGroupCostReports`, in response order.
fn reports(body: &Value) -> Vec<Value> {
    body.get("BillingGroupCostReports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Arn` of each reported row, in response order.
fn reported_arns(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The one row reporting `group_arn`, or `None` when the listing reports that group
/// nowhere.
fn row_of<'rows>(rows: &'rows [Value], group_arn: &str) -> Option<&'rows Value> {
    rows.iter()
        .find(|row| row.get("Arn").and_then(Value::as_str) == Some(group_arn))
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
/// admits, so a filter may name the group either way.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for
/// earlier), in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// A billing group and a custom line item are created into the billing period the wall
/// clock says it is, so every period named here is named relative to the service's own
/// current billing period rather than by a fixed month.
fn period(months: i64) -> String {
    let current =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    let (year, month) = current
        .split_once('-')
        .expect("the current billing period is spelled YYYY-MM");
    let total = year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month")
        - 1
        + months;
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// The same billing period with its month spelled without a leading zero — the other
/// spelling the modelled `BillingPeriod` pattern allows. A month from October on has
/// only the one spelling, so for those this is the value it was given.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// RULE: every billing group is reported with ONE row whose pro forma cost is the signed
/// sum of the custom line items in effect for it in the billing period asked about,
/// whatever product each is presented under, whose actual cost is the metered usage this
/// emulator records (none), and whose margin and margin percentage follow from the two.
#[tokio::test]
async fn every_billing_group_is_reported_with_the_summary_of_its_custom_line_items() {
    let em = Emulator::new();

    // (case, the group's ARN, the pro forma cost due for it in the current period)
    let mut cases: Vec<(&str, String, f64)> = Vec::new();

    cases.push((
        "a group with no custom line item at all",
        em.billing_group("empty").await,
        0.0,
    ));

    let one_fee = em.billing_group("onefee").await;
    em.custom_line_item(
        &one_fee,
        "fee100",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 100.0 } } }),
    )
    .await;
    cases.push(("one flat fee", one_fee, 100.0));

    let fee_and_credit = em.billing_group("feeandcredit").await;
    em.custom_line_item(
        &fee_and_credit,
        "feefractional",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 12.5 } } }),
    )
    .await;
    em.custom_line_item(
        &fee_and_credit,
        "credit20",
        json!({ "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 20.0 } } }),
    )
    .await;
    cases.push((
        "a fractional flat fee and a flat credit, which comes OFF the bill",
        fee_and_credit,
        -7.5,
    ));

    let zero_valued = em.billing_group("zerovalued").await;
    em.custom_line_item(
        &zero_valued,
        "fee0",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 0.0 } } }),
    )
    .await;
    cases.push((
        "a fee worth the modelled minimum charge value",
        zero_valued,
        0.0,
    ));

    // A percentage item is "multiplied against the combined value of its associated
    // resources", and may itself feed another one: 200 flat, 10% of it, then half of that.
    let chained = em.billing_group("chained").await;
    let chain_base = em
        .custom_line_item(
            &chained,
            "fee200",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 200.0 } } }),
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
    let chain_top = em
        .custom_line_item(
            &chained,
            "halfofthat",
            json!({
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 50.0 } }
            }),
        )
        .await;
    em.associate(&chain_top, &[chain_middle]).await;
    cases.push((
        "a percentage of a flat fee and a percentage of that percentage",
        chained,
        230.0,
    ));

    // Items presented under DIFFERENT products: this listing reports no breakdown, so the
    // one row carries their sum.
    let products = em.billing_group("products").await;
    em.custom_line_item(
        &products,
        "storage",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "PresentationDetails": { "Service": "S3" },
        }),
    )
    .await;
    em.custom_line_item(
        &products,
        "computecredit",
        json!({
            "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 3.0 } },
            "PresentationDetails": { "Service": "EC2" },
        }),
    )
    .await;
    em.custom_line_item(
        &products,
        "unpresented",
        json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 7.0 } } }),
    )
    .await;
    cases.push((
        "three items presented under three different products",
        products,
        14.0,
    ));

    // An item whose own half-open range ended before the period asked about is in effect
    // in none of it, so none of its charge is reported.
    let expired = em.billing_group("expired").await;
    em.custom_line_item(
        &expired,
        "fee500",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 500.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(-1),
                "ExclusiveEndBillingPeriod": period(0),
            },
        }),
    )
    .await;
    cases.push((
        "an item whose billing period range ended before the period asked about",
        expired,
        0.0,
    ));

    let rows = em.rows("every group, no filter", json!({})).await;
    assert_eq!(
        rows.len(),
        cases.len(),
        "case \"every group, no filter\": expected one row per billing group ({}), got {} in {:?}",
        cases.len(),
        rows.len(),
        reported_arns(&rows)
    );
    for (case, group_arn, proforma) in &cases {
        let row = row_of(&rows, group_arn).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a row for {group_arn}, got {:?}",
                reported_arns(&rows)
            )
        });
        assert_summary(row, group_arn, *proforma, case);
    }
}

/// RULE: `Filters.BillingGroupArns` narrows the listing to the billing groups it names,
/// in either spelling the `BillingGroupArn` pattern admits; an ARN that names no stored
/// group selects none.
#[tokio::test]
async fn the_filter_narrows_the_listing_to_the_billing_groups_it_names() {
    let em = Emulator::new();
    let first = em.billing_group("first").await;
    let second = em.billing_group("second").await;
    let third = em.billing_group("third").await;
    // A well-formed `BillingGroupArn` (the pattern's bare-id form, 10 characters) that
    // names no stored group.
    let absent = "ZZZZZZZZZZ".to_string();

    // (case, the Filters member, the ARNs the rows are due to carry)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "one group by its full ARN",
            json!({ "BillingGroupArns": [second.clone()] }),
            vec![second.clone()],
        ),
        (
            "one group by its bare id",
            json!({ "BillingGroupArns": [bare_id(&third)] }),
            vec![third.clone()],
        ),
        (
            "two groups, one in each spelling",
            json!({ "BillingGroupArns": [first.clone(), bare_id(&third)] }),
            vec![first.clone(), third.clone()],
        ),
        (
            "every group, named explicitly",
            json!({ "BillingGroupArns": [third.clone(), second.clone(), first.clone()] }),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "one group named twice",
            json!({ "BillingGroupArns": [second.clone(), second.clone()] }),
            vec![second.clone()],
        ),
        (
            "only an ARN that names no stored group",
            json!({ "BillingGroupArns": [absent.clone()] }),
            vec![],
        ),
        (
            "a stored group beside one that exists nowhere",
            json!({ "BillingGroupArns": [absent, first.clone()] }),
            vec![first.clone()],
        ),
    ];

    for (case, filters, expected) in cases {
        let mut request = json!({});
        if !members(&filters).is_empty() {
            request["Filters"] = filters;
        }
        let rows = em.rows(case, request).await;
        let actual = reported_arns(&rows);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the rows {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a billing group is reported for the billing period the request names — the one
/// the call is made in when it names none — and only for the periods the group existed
/// in; the two spellings of a period name the same period.
#[tokio::test]
async fn a_group_is_reported_for_the_billing_period_asked_about() {
    let em = Emulator::new();
    let group = em.billing_group("periods").await;
    // Two items differing in where their own half-open range falls: every period from the
    // current one on, and the current period only.
    em.custom_line_item(
        &group,
        "fromcurrent",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 4.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
        }),
    )
    .await;
    em.custom_line_item(
        &group,
        "currentonly",
        json!({
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 11.0 } },
            "BillingPeriodRange": {
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(1),
            },
        }),
    )
    .await;

    // (case, the BillingPeriod member, the pro forma cost due, or None for no row at all)
    let cases: Vec<(&str, Option<String>, Option<f64>)> = vec![
        (
            "the period before the group was created",
            Some(period(-1)),
            None,
        ),
        (
            "the period the group was created in",
            Some(period(0)),
            Some(15.0),
        ),
        (
            "the same period with a single-digit month",
            Some(one_digit_month(&period(0))),
            Some(15.0),
        ),
        (
            "the period after, where only the open-ended item is in effect",
            Some(period(1)),
            Some(4.0),
        ),
        (
            "a period a year on, where only the open-ended item is in effect",
            Some(period(12)),
            Some(4.0),
        ),
        ("no billing period at all", None, Some(15.0)),
    ];

    for (case, billing_period, expected) in cases {
        let mut request = json!({});
        if let Some(billing_period) = billing_period {
            request["BillingPeriod"] = json!(billing_period);
        }
        let rows = em.rows(case, request).await;
        match expected {
            None => assert!(
                rows.is_empty(),
                "case {case:?}: the group did not exist then, so no row is due; got {:?}",
                reported_arns(&rows)
            ),
            Some(proforma) => {
                assert_eq!(
                    rows.len(),
                    1,
                    "case {case:?}: expected exactly one row, got {:?}",
                    reported_arns(&rows)
                );
                assert_summary(&rows[0], &group, proforma, case);
            }
        }
    }
}

/// RULE: the rows run in creation order and are cut into pages of the request's
/// `MaxResults`, each page resumed by the `NextToken` the previous one answered with;
/// a `MaxResults` outside the modelled range is refused 400 `ValidationException`.
#[tokio::test]
async fn pages_are_cut_at_max_results_and_resumed_by_the_next_token() {
    let em = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(em.billing_group(&format!("group{index}")).await);
    }

    // Page sizes differing in how they divide the five groups: one at a time, two
    // (leaving a short last page), one short of the whole listing, exactly the whole
    // listing, more than it, the modelled maximum and no MaxResults at all.
    let page_sizes: Vec<Option<i64>> = vec![
        Some(MIN_MAX_RESULTS),
        Some(2),
        Some(4),
        Some(5),
        Some(6),
        Some(MAX_MAX_RESULTS),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("MaxResults {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["MaxResults"] = json!(page_size);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let body = em.listed(&case, request).await;
            let rows = reports(&body);
            let expected_rows = page_size
                .unwrap_or(MAX_MAX_RESULTS)
                .min(created.len() as i64 - walked.len() as i64);
            assert_eq!(
                rows.len() as i64,
                expected_rows,
                "case {case:?}: expected a page of {expected_rows} row(s) after {} walked, \
                 got {:?}",
                walked.len(),
                reported_arns(&rows)
            );
            walked.extend(reported_arns(&rows));
            pages += 1;
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= created.len(),
                "case {case:?}: the pages never ran out after {pages} of them, walking {walked:?}"
            );
        }
        assert_eq!(
            walked, created,
            "case {case:?}: the pages together are the whole listing in creation order; \
             got {walked:?}"
        );
    }

    // Each published bound of the modelled `MaxBillingGroupResults` range is served, and
    // a value past either bound is refused.
    let bounds: Vec<(i64, u16)> = vec![
        (MIN_MAX_RESULTS - 1, 400),
        (MIN_MAX_RESULTS, 200),
        (MAX_MAX_RESULTS, 200),
        (MAX_MAX_RESULTS + 1, 400),
    ];
    for (max_results, expected) in bounds {
        let (status, body) = em.list(json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, expected,
            "case \"MaxResults {max_results}\": expected HTTP {expected}, got {status} with {body}"
        );
        if expected == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationException",
                "case \"MaxResults {max_results}\": expected a ValidationException, \
                 got {code:?} in {body}"
            );
        }
    }
}

/// RULE: the listing REPORTS; it writes nothing, so repeating it answers the same thing
/// and the single-group report of the same period agrees with it.
#[tokio::test]
async fn listing_the_cost_reports_writes_nothing_and_agrees_with_the_single_group_report() {
    let em = Emulator::new();
    let plain = em.billing_group("plain").await;
    let charged = em.billing_group("charged").await;
    let base = em
        .custom_line_item(
            &charged,
            "fee60",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 60.0 } } }),
        )
        .await;
    em.custom_line_item(
        &charged,
        "fifth",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Percentage": { "PercentageValue": 20.0, "AssociatedValues": [base] },
            }
        }),
    )
    .await;

    // Requests differing in what they ask for, each repeated: the whole listing, one
    // group, and one page of it.
    let requests: Vec<(&str, Value)> = vec![
        ("the whole listing, twice", json!({})),
        (
            "one group by its filter, twice",
            json!({ "Filters": { "BillingGroupArns": [charged.clone()] } }),
        ),
        (
            "one page of the listing, twice",
            json!({ "MaxResults": 1 }),
        ),
    ];
    for (case, request) in requests {
        let first = em.listed(case, request.clone()).await;
        let second = em.listed(case, request).await;
        assert_eq!(
            first, second,
            "case {case:?}: a read changes nothing a later caller sees, so the second \
             answer is the first; got {second} after {first}"
        );
    }

    // The two cost reports of this service answer for the same state: the listing's row
    // for a group carries the amounts its own single-group report does for the same
    // billing period with no breakdown.
    for (case, group_arn, proforma) in [
        ("a group with no custom line item", &plain, 0.0),
        ("a group with a flat and a percentage item", &charged, 72.0),
    ] {
        let rows = em.rows(case, json!({})).await;
        let row = row_of(&rows, group_arn).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a row for {group_arn}, got {:?}",
                reported_arns(&rows)
            )
        });
        assert_summary(row, group_arn, proforma, case);
        let single = em.group_report(case, group_arn).await;
        assert_eq!(
            single.len(),
            1,
            "case {case:?}: a report with no breakdown is one row, got {single:?}"
        );
        for member in ["AWSCost", "ProformaCost", "Margin", "MarginPercentage"] {
            let listed = amount(row, member, case);
            let reported = amount(&single[0], member, case);
            assert!(
                (listed - reported).abs() < TOLERANCE,
                "case {case:?}: the listing's {member} is the single-group report's; \
                 got {listed} and {reported}"
            );
        }
    }
}
