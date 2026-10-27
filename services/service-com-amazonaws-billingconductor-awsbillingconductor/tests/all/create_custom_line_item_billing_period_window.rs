// SPDX-License-Identifier: CC0-1.0

//! The billing periods a `CreateCustomLineItem` may put an item into, end to end over
//! the wire: `CreateBillingGroup` -> `CreateCustomLineItem` -> `ListCustomLineItems`.
//!
//! RULE: an operation the model documents as acting in the current or previous billing
//! period may take effect only there, so the period a request's `BillingPeriodRange`
//! INCLUSIVELY STARTS in has to be one of those two: a start in any other period — a
//! month whose bill is closed, or one that has not begun — is refused with the modelled
//! 400 `ValidationException` carrying the `ValidationExceptionReason` the model
//! enumerates for a period the operation may not act in
//! (`INVALID_BILLING_PERIOD_FOR_OPERATION`) and the member at fault, and nothing is
//! written. The EXCLUSIVE end only says how far forward the item reaches, so it is free,
//! exactly as an absent end (the open future) is.
//!
//! The rows span what the rule generalises over rather than one recorded call: which of
//! the two admitted periods the range starts in, how far outside the window a refused
//! start lies and in which direction (one month, a year, a decade, the epoch month, each
//! end of the modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$`), how the range
//! is bounded (open end, bounded inside the window, bounded far past it), how the month
//! is spelled (one-digit / zero-padded), and the charge kind of the item (flat /
//! percentage).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this service's control plane is
    /// reachable at all (see `tests/all/payer_account_gate.rs` for the denial every other
    /// caller gets).
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

    /// Send a JSON body to one of this service's routes and return the status and the
    /// parsed body.
    async fn send(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
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
        member_str(&body, "Arn", &format!("billing group {name:?}"))
    }

    /// `CreateCustomLineItem` with the `ChargeDetails` and the `BillingPeriodRange` the
    /// case names (none when `range` is `Value::Null`).
    async fn create(&self, group: &str, name: &str, charge: &Value, range: &Value) -> (u16, Value) {
        let mut body = json!({
            "Name": name,
            "Description": "the item the window is tested with",
            "BillingGroupArn": group,
            "ChargeDetails": charge,
        });
        if !range.is_null() {
            body["BillingPeriodRange"] = range.clone();
        }
        self.send("/create-custom-line-item", &body).await
    }

    /// Does `ListCustomLineItems` report the item `arn` for `billing_period` (the current
    /// one when `None`)?
    async fn reports(&self, case: &str, arn: &str, billing_period: Option<&str>) -> bool {
        let mut body = json!({ "Filters": { "Arns": [arn] } });
        if let Some(period) = billing_period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, response) = self.send("/list-custom-line-items", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer HTTP 200, got {status} with {response}"
        );
        !items(&response).is_empty()
    }

    /// How many custom line items the whole store holds for `billing_period`, whatever
    /// their name — what a refused create may not have added to.
    async fn item_count(&self, case: &str, billing_period: &str) -> usize {
        let (status, response) = self
            .send(
                "/list-custom-line-items",
                &json!({ "BillingPeriod": billing_period }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer HTTP 200, got {status} with {response}"
        );
        items(&response).len()
    }
}

/// The `CustomLineItems` a read reports.
fn items(body: &Value) -> Vec<Value> {
    body.get("CustomLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A string member a response is required to carry.
fn member_str(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a {member} member, got {body}"))
        .to_string()
}

/// A `CustomLineItemBillingPeriodRange`, as the model shapes it
/// (`smithy_query(query="shape",shape="CustomLineItemBillingPeriodRange")`: `@required
/// InclusiveStartBillingPeriod`, optional `ExclusiveEndBillingPeriod`).
fn range(inclusive_start: &str, exclusive_end: Option<&str>) -> Value {
    let mut value = json!({ "InclusiveStartBillingPeriod": inclusive_start });
    if let Some(end) = exclusive_end {
        value["ExclusiveEndBillingPeriod"] = json!(end);
    }
    value
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for
/// earlier), in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
///
/// The periods a create may put an item into are the current one and the one before it,
/// which the wall clock decides, so every period here is named relative to the service's
/// own current billing period rather than by a fixed month.
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

/// A FLAT charge, the kind most of the rows use.
fn flat_charge() -> Value {
    json!({ "Type": "FEE", "Flat": { "ChargeValue": 1.0 } })
}

/// Assert a refusal: the status, the modelled error shape, the `Reason` the model
/// enumerates and the member at fault.
fn assert_refused(
    case: &str,
    status: u16,
    body: &Value,
    expected_reason: &str,
    expected_field: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        body.get("__type").and_then(Value::as_str),
        Some("ValidationException"),
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    assert!(
        !body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "case {case:?}: the modelled @required Message must state the refusal, got {body}"
    );
    assert_eq!(
        body.get("Reason").and_then(Value::as_str),
        Some(expected_reason),
        "case {case:?}: expected the Reason {expected_reason:?}, got {body}"
    );
    let named = body
        .get("Fields")
        .and_then(Value::as_array)
        .is_some_and(|fields| {
            fields
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                .any(|name| name == expected_field)
        });
    assert!(
        named,
        "case {case:?}: expected Fields to name {expected_field:?}, got {body}"
    );
}

/// THE RULE on the billing period a create may put an item into: the period the
/// `BillingPeriodRange` starts in has to be the current billing period or the one before
/// it. A range starting there is served and the item is readable back for the periods its
/// span covers; a range starting in any other period is refused with the modelled 400
/// `ValidationException` and the reason the model enumerates for a period this operation
/// may not act in, and no item is minted.
#[tokio::test]
async fn a_create_whose_range_starts_outside_the_current_or_previous_period_is_refused() {
    // (case, the range the create names, the charge details, whether the operation may
    // act there)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "no range at all, so every billing period",
            Value::Null,
            flat_charge(),
            true,
        ),
        (
            "a range starting in the current billing period, open ended",
            range(&period(0), None),
            flat_charge(),
            true,
        ),
        (
            "a range starting in the current billing period, bounded inside the window",
            range(&period(0), Some(&period(1))),
            flat_charge(),
            true,
        ),
        (
            // The exclusive end only says how far forward the item reaches, so a far
            // future end is as admissible as no end at all.
            "a range starting in the current billing period, ending years later",
            range(&period(0), Some(&period(70))),
            flat_charge(),
            true,
        ),
        (
            "a range from the previous billing period to the current one",
            range(&period(-1), Some(&period(0))),
            flat_charge(),
            true,
        ),
        (
            "a range starting in the previous billing period, one-digit month",
            range(&one_digit_month(&period(-1)), None),
            flat_charge(),
            true,
        ),
        (
            "a percentage item starting in the previous billing period",
            range(&period(-1), None),
            json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 12.5 } }),
            true,
        ),
        (
            "one month before the previous billing period",
            range(&period(-2), Some(&period(-1))),
            flat_charge(),
            false,
        ),
        (
            "a year before the current billing period",
            range(&period(-12), None),
            flat_charge(),
            false,
        ),
        (
            "a decade before the current billing period",
            range(&period(-120), Some(&period(-119))),
            flat_charge(),
            false,
        ),
        (
            // The month the `Instant` members this service records count from.
            "the epoch month",
            range("1970-01", Some("1971-01")),
            flat_charge(),
            false,
        ),
        (
            // The earliest month the modelled `BillingPeriod` pattern admits.
            "the first month of year zero",
            range("0000-01", Some("0001-01")),
            flat_charge(),
            false,
        ),
        (
            "the month after the current billing period",
            range(&period(1), None),
            flat_charge(),
            false,
        ),
        (
            "a year after the current billing period",
            range(&period(12), Some(&period(13))),
            flat_charge(),
            false,
        ),
        (
            "a future month spelled with a one-digit month",
            range(&one_digit_month(&period(2)), None),
            flat_charge(),
            false,
        ),
        (
            // The latest month the modelled `BillingPeriod` pattern admits.
            "the last month of year 9999",
            range("9999-12", None),
            flat_charge(),
            false,
        ),
        (
            "a percentage item starting a year before the window",
            range(&period(-12), None),
            json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.0 } }),
            false,
        ),
    ];

    for (case, created_with, charge, inside_the_window) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;

        let (status, body) = emulator
            .create(&group, "guarded", &charge, &created_with)
            .await;
        if inside_the_window {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            // ROUND TRIP: the minted item is readable back for the period its own span
            // starts in (the current one when it named no span at all).
            let arn = member_str(&body, "Arn", case);
            let effective_from = created_with
                .get("InclusiveStartBillingPeriod")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| period(0));
            assert!(
                emulator
                    .reports(case, &arn, Some(&effective_from))
                    .await,
                "case {case:?}: expected the read for {effective_from:?} to report the \
                 minted item"
            );
            continue;
        }

        assert_refused(
            case,
            status,
            &body,
            "INVALID_BILLING_PERIOD_FOR_OPERATION",
            "BillingPeriodRange.InclusiveStartBillingPeriod",
        );
        assert!(
            body.get("Arn").is_none(),
            "case {case:?}: a refused create mints no ARN, got {body}"
        );
        // NOTHING was written: no billing period reports any custom line item at all,
        // inside the refused range or outside it.
        for probe in [
            period(-120),
            period(-2),
            period(-1),
            period(0),
            period(1),
            period(120),
            "1970-01".to_string(),
            "9999-12".to_string(),
        ] {
            assert_eq!(
                emulator.item_count(case, &probe).await,
                0,
                "case {case:?}: expected the refused create to leave no item for {probe:?}"
            );
        }
    }
}

/// THE RULE on a start that is no billing period at all: it names none of the periods
/// the operation may act in, so the create is refused with the modelled 400
/// `ValidationException` and nothing is minted.
#[tokio::test]
async fn a_create_whose_range_starts_at_a_value_that_is_no_billing_period_is_refused() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;

    // (case, the value the range starts at) — the modelled `BillingPeriod` pattern
    // `^\d{4}-(0?[1-9]|1[012])$` admits none of them.
    let rows: Vec<(&str, &str)> = vec![
        ("a month of zero", "2026-00"),
        ("a thirteenth month", "2026-13"),
        ("a two-digit year", "26-01"),
        ("a year on its own", "2026"),
        ("empty", ""),
    ];

    for (case, start) in rows {
        let (status, body) = emulator
            .create(&group, "guarded", &flat_charge(), &range(start, None))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("ValidationException"),
            "case {case:?}: expected the modelled ValidationException, got {body}"
        );
        assert_eq!(
            emulator.item_count(case, &period(0)).await,
            0,
            "case {case:?}: expected the refused create to leave no item behind"
        );
    }
}

/// THE RULE on the exclusive end, which the window does NOT judge: an end at or before
/// the start covers no billing period, so it is refused for naming an empty span
/// (`ILLEGAL_BILLING_PERIOD_RANGE`) — the refusal the ordering of the range earns, not
/// the one its start earns — while an end anywhere after a start inside the window is
/// served.
#[tokio::test]
async fn an_end_at_or_before_a_start_inside_the_window_is_refused_as_an_empty_span() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;

    // (case, the start, the end)
    let rows: Vec<(&str, String, String)> = vec![
        ("an end equal to the start", period(0), period(0)),
        ("an end one month before the start", period(0), period(-1)),
        ("an end a year before the start", period(0), period(-12)),
        (
            "equal bounds spelled both ways",
            one_digit_month(&period(-1)),
            period(-1),
        ),
    ];

    for (case, start, end) in rows {
        let (status, body) = emulator
            .create(
                &group,
                "guarded",
                &flat_charge(),
                &range(&start, Some(&end)),
            )
            .await;
        assert_refused(
            case,
            status,
            &body,
            "ILLEGAL_BILLING_PERIOD_RANGE",
            "BillingPeriodRange.ExclusiveEndBillingPeriod",
        );
        assert_eq!(
            emulator.item_count(case, &period(0)).await,
            0,
            "case {case:?}: expected the refused create to leave no item behind"
        );
    }
}
