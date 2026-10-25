// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomLineItem` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` -> `DeleteCustomLineItem` -> `ListCustomLineItems`.
//!
//! RULE: `DeleteCustomLineItem` removes the stored custom line item its `Arn` names (in
//! either form the `CustomLineItemArn` pattern admits) from the billing periods the
//! request names, so a later read reports it only for the periods the delete did not
//! cover: a request carrying no `BillingPeriodRange` deletes the item itself (no read of
//! any billing period reports it again, and the associations other custom line items
//! held to it go with it), and a request carrying one deletes it only from the half-open
//! span that range covers, so the item keeps every OTHER period it applied in: the
//! periods before the range's inclusive start and the periods from its exclusive end on,
//! both of them when the range falls strictly inside the periods the item applied in. It
//! leaves the item untouched when the range shares no period with it, and removes the
//! record only once nothing of it is left. It answers the deleted item's full ARN,
//! refuses an `Arn` that
//! names no stored item with 400 `ValidationException`
//! (`MISSING_CUSTOMLINEITEM`), refuses a range that covers no billing period with
//! 400 `ValidationException` (`ILLEGAL_BILLING_PERIOD_RANGE`), and — because the
//! operation acts "in the current, or previous billing period" — refuses a range
//! STARTING in any other period with 400 `ValidationException`
//! (`INVALID_BILLING_PERIOD_FOR_OPERATION`), all three while leaving the stored item
//! exactly as it was.
//!
//! Every billing period here is therefore expressed as an offset from the CURRENT
//! billing period ([`period`]), which is the only window the request may start in.
//!
//! The rows span what the rule generalises over rather than one recorded call: the form
//! the `Arn` takes (full ARN / bare resource id), the charge kind of the item (flat /
//! percentage, with and without associated resources), the span the stored item was
//! created with (none / open-ended / bounded, before / around / after the window), where
//! the request's span falls relative to it (disjoint before, disjoint after, abutting,
//! overlapping the head, overlapping the tail, strictly inside, equal, covering), how the
//! request's span
//! is bounded (open end / bounded), how many bounded deletes the item has taken (one /
//! several), how far outside the window a refused range starts
//! (one month, a year, a decade, the epoch month, a far-future month) and how a billing
//! period is spelled (`2026-7` / `2026-07`).

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
    /// one — the world in which the delete this suite is about is reachable at all. The
    /// denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`.
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
        member_str(&body, "Arn", &format!("billing group {name:?}"))
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
        for (member, value) in object(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/create-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {response}"
        );
        member_str(&response, "Arn", &format!("custom line item {name:?}"))
    }

    /// `DeleteCustomLineItem` on `arn`, with the `BillingPeriodRange` the request names
    /// (none when `range` is `Value::Null`).
    async fn delete(&self, arn: &str, range: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if !range.is_null() {
            body["BillingPeriodRange"] = range.clone();
        }
        self.send("POST", "/delete-custom-line-item", &body).await
    }

    /// `DeleteCustomLineItem` that is expected to succeed, answering the item's ARN.
    async fn delete_ok(&self, case: &str, arn: &str, range: &Value) -> Value {
        let (status, body) = self.delete(arn, range).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body
    }

    /// The item as `ListCustomLineItems` reports it for `billing_period` (the current
    /// one when `None`), or `None` when the read reports no such item.
    async fn read_back(&self, case: &str, arn: &str, billing_period: Option<&str>) -> Option<Value> {
        let mut body = json!({ "Filters": { "Arns": [arn] } });
        if let Some(period) = billing_period {
            body["BillingPeriod"] = json!(period);
        }
        let (status, response) = self.send("POST", "/list-custom-line-items", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer HTTP 200, got {status} with {response}"
        );
        let elements = response
            .get("CustomLineItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            elements.len() <= 1,
            "case {case:?}: one ARN names at most one item, got {elements:?}"
        );
        elements.into_iter().next()
    }

    /// Does `ListCustomLineItems` report the item for `billing_period` (the current one
    /// when `None`)?
    async fn reports(&self, case: &str, arn: &str, billing_period: Option<&str>) -> bool {
        self.read_back(case, arn, billing_period).await.is_some()
    }

    /// The item as `ListCustomLineItems` reports it for the current billing period,
    /// which this case requires to be readable.
    async fn stored(&self, case: &str, arn: &str) -> Value {
        self.read_back(case, arn, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn} to be readable now"))
    }

    /// The item as `ListCustomLineItems` reports it for `billing_period`, which this
    /// case requires to be readable there.
    async fn stored_at(&self, case: &str, arn: &str, billing_period: &str) -> Value {
        self.read_back(case, arn, Some(billing_period))
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {arn} to be readable for {billing_period}")
            })
    }
}

/// The members of a JSON object, or none when it is not one.
fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// A string member a response is required to carry.
fn member_str(body: &Value, member: &str, case: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a {member} member, got {body}"))
        .to_string()
}

/// The bare resource id of a full ARN — the OTHER form the `CustomLineItemArn` pattern
/// admits.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
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
/// The periods a delete may START in are the current one and the one before it, which
/// the wall clock decides, so every period in this suite is named relative to the
/// service's own current billing period rather than by a fixed month.
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
        .map(|fields| {
            fields
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                .any(|name| name == expected_field)
        })
        .unwrap_or(false);
    assert!(
        named,
        "case {case:?}: expected Fields to name {expected_field:?}, got {body}"
    );
}

/// The billing periods every row reads the item back at: two years of quarters around
/// the spans the rows use, plus the current billing period (the one a read with no
/// `BillingPeriod` uses).
const PROBE_PERIODS: &[&str] = &[
    "2025-01", "2026-01", "2026-02", "2026-03", "2026-05", "2026-06", "2026-07", "2026-08",
    "2026-09", "2026-12", "2027-06",
];

/// THE RULE for a request that names no billing periods: the item itself is deleted, so
/// no read of any billing period reports it again, whatever span it was created with and
/// whichever form of its ARN the request used.
#[tokio::test]
async fn a_delete_naming_no_billing_period_range_removes_the_item_from_every_period() {
    // (case, the create members beyond the ones every create needs, whether the request
    // addresses the item by its bare id, a billing period the item is readable at)
    let rows: Vec<(&str, Value, bool, Option<&str>)> = vec![
        ("a flat item with no span, by full ARN", json!({}), false, None),
        ("a flat item with no span, by bare id", json!({}), true, None),
        (
            "a percentage item with no span",
            json!({ "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 12.5 } } }),
            false,
            None,
        ),
        (
            "an item with a bounded span, by full ARN",
            json!({
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-03",
                    "ExclusiveEndBillingPeriod": "2026-09",
                },
            }),
            false,
            Some("2026-05"),
        ),
        (
            "an item with an open-ended span, by bare id",
            json!({ "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-03" } }),
            true,
            Some("2026-07"),
        ),
        (
            "an item whose span is spelled with a one-digit month",
            json!({ "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-3" } }),
            false,
            Some("2026-07"),
        ),
    ];

    for (case, create, by_bare_id, readable_at) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let item = emulator.custom_line_item(&group, "deleted", create).await;
        assert!(
            emulator.reports(case, &item, readable_at).await,
            "case {case:?}: expected the created item to be readable before the delete"
        );

        let addressed = if by_bare_id {
            bare_id(&item)
        } else {
            item.clone()
        };
        let answered = emulator.delete_ok(case, &addressed, &Value::Null).await;
        assert_eq!(
            member_str(&answered, "Arn", case),
            item,
            "case {case:?}: expected the deleted item's full ARN, got {answered}"
        );

        for period in PROBE_PERIODS.iter().copied().map(Some).chain([None]) {
            assert!(
                !emulator.reports(case, &item, period).await,
                "case {case:?}: expected no read to report the deleted item, but the read \
                 for {period:?} did"
            );
        }

        // The item is gone, so a second delete has no item to address.
        let (status, body) = emulator.delete(&addressed, &Value::Null).await;
        assert_refused(
            &format!("{case}, deleted twice"),
            status,
            &body,
            "MISSING_CUSTOMLINEITEM",
            "Arn",
        );
    }
}

/// One row of the billing-period-range table: the case, the span the item is created
/// with (none when it applies to every period), the span the delete names, the periods a
/// later read still reports the item for, and the periods it does not.
type SpanRow = (&'static str, Option<Value>, Value, Vec<String>, Vec<String>);

/// THE RULE for a request that names a billing period range: the item is deleted only
/// from the half-open span that range covers, so a later read reports it exactly for the
/// periods the delete did not cover.
///
/// Every span the DELETE names starts in the window the operation may act in (the
/// current billing period or the one before it — [`period`] counts from the current
/// one); the span the item was CREATED with is free, so the rows still put the
/// request's span before, after, over and across the item's.
#[tokio::test]
async fn a_delete_naming_a_billing_period_range_removes_the_item_only_from_those_periods() {
    // (case, the span the item is created with, the span the delete names, the periods
    // the item is still reported for, the periods it is not)
    let rows: Vec<SpanRow> = vec![
        (
            "an open-ended delete of an item with no span",
            None,
            range(&period(0), None),
            vec![period(-3), period(-1)],
            vec![period(0), period(1), period(12)],
        ),
        (
            "a bounded delete of an item with no span takes only the periods it named",
            None,
            range(&period(-1), Some(&period(1))),
            vec![period(-3), period(-2), period(1), period(2), period(12)],
            vec![period(-1), period(0)],
        ),
        (
            "a bounded delete strictly inside a bounded span keeps both sides of it",
            Some(range(&period(-1), Some(&period(6)))),
            range(&period(0), Some(&period(2))),
            vec![period(-1), period(2), period(5)],
            vec![period(-2), period(0), period(1), period(6)],
        ),
        (
            "a bounded delete strictly inside an open-ended span keeps both sides of it",
            Some(range(&period(-1), None)),
            range(&period(0), Some(&period(1))),
            vec![period(-1), period(1), period(12)],
            vec![period(-2), period(0)],
        ),
        (
            "a bounded delete of the head of an open-ended item",
            Some(range(&period(-1), None)),
            range(&period(-1), Some(&period(1))),
            vec![period(1), period(2), period(12)],
            vec![period(-2), period(-1), period(0)],
        ),
        (
            "a delete overlapping the head of a bounded span",
            Some(range(&period(-1), Some(&period(3)))),
            range(&period(-1), Some(&period(1))),
            vec![period(1), period(2)],
            vec![period(-1), period(0), period(3)],
        ),
        (
            "a delete overlapping the tail of a bounded span",
            Some(range(&period(-3), Some(&period(3)))),
            range(&period(0), None),
            vec![period(-3), period(-1)],
            vec![period(0), period(2), period(3)],
        ),
        (
            "a delete covering a bounded span exactly",
            Some(range(&period(-1), Some(&period(0)))),
            range(&period(-1), Some(&period(0))),
            vec![],
            vec![period(-2), period(-1), period(0)],
        ),
        (
            "a delete covering more than the whole span",
            Some(range(&period(0), Some(&period(2)))),
            range(&period(-1), None),
            vec![],
            vec![period(-1), period(0), period(1), period(2)],
        ),
        (
            "a delete of the periods after an open-ended span starts",
            Some(range(&period(0), None)),
            range(&period(0), None),
            vec![],
            vec![period(0), period(1), period(12)],
        ),
        (
            "a delete whose span abuts the start of the item's",
            Some(range(&period(0), Some(&period(3)))),
            range(&period(-1), Some(&period(0))),
            vec![period(0), period(2)],
            vec![period(-1), period(3)],
        ),
        (
            "a delete whose span lies entirely before the item's",
            Some(range(&period(3), Some(&period(6)))),
            range(&period(-1), Some(&period(0))),
            vec![period(3), period(5)],
            vec![period(-1), period(0), period(6)],
        ),
        (
            "a delete whose span lies entirely after the item's",
            Some(range(&period(-6), Some(&period(-3)))),
            range(&period(0), None),
            vec![period(-6), period(-4)],
            vec![period(-3), period(0), period(1)],
        ),
        (
            "a one-digit month names the same period as its zero-padded spelling",
            Some(range(
                &one_digit_month(&period(-1)),
                Some(&one_digit_month(&period(3))),
            )),
            range(&one_digit_month(&period(0)), None),
            vec![period(-1)],
            vec![period(0), period(2)],
        ),
        (
            "a one-digit month bounds a delete strictly inside the item's span",
            Some(range(&period(-1), Some(&period(3)))),
            range(
                &one_digit_month(&period(0)),
                Some(&one_digit_month(&period(1))),
            ),
            vec![period(-1), period(1), period(2)],
            vec![period(0), period(3)],
        ),
    ];

    for (case, created_with, deleted_with, reported, unreported) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let create = match &created_with {
            Some(span) => json!({ "BillingPeriodRange": span }),
            None => json!({}),
        };
        let item = emulator.custom_line_item(&group, "deleted", create).await;

        let answered = emulator.delete_ok(case, &item, &deleted_with).await;
        assert_eq!(
            member_str(&answered, "Arn", case),
            item,
            "case {case:?}: expected the item's full ARN, got {answered}"
        );

        for probe in &reported {
            assert!(
                emulator.reports(case, &item, Some(probe)).await,
                "case {case:?}: expected the read for {probe:?} to still report the item"
            );
        }
        for probe in &unreported {
            assert!(
                !emulator.reports(case, &item, Some(probe)).await,
                "case {case:?}: expected the read for {probe:?} not to report the item"
            );
        }
    }
}

/// THE RULE on the instant a narrowed item reports: narrowing CHANGES the stored record,
/// so `LastModifiedTime` advances with it, while a delete that changes nothing leaves the
/// record — and that instant — exactly as it was. `CreationTime` is the instant the item
/// was created either way.
#[tokio::test]
async fn narrowing_an_item_advances_last_modified_time_and_changing_nothing_leaves_it() {
    // (case, the span the item is created with, the span the delete names, whether it
    // narrows the item, a billing period the item is still reported for afterwards)
    let rows: Vec<(&str, Value, Value, bool, String)> = vec![
        (
            "an open-ended delete of the item's tail",
            range(&period(-1), Some(&period(3))),
            range(&period(0), None),
            true,
            period(-1),
        ),
        (
            "a bounded delete of the item's head",
            range(&period(-1), Some(&period(3))),
            range(&period(-1), Some(&period(0))),
            true,
            period(1),
        ),
        (
            "a delete whose span lies entirely after the item's",
            range(&period(-6), Some(&period(-3))),
            range(&period(0), None),
            false,
            period(-5),
        ),
        (
            "a delete whose span lies entirely before the item's",
            range(&period(3), Some(&period(6))),
            range(&period(-1), Some(&period(0))),
            false,
            period(4),
        ),
    ];

    for (case, created_with, deleted_with, narrows, readable_at) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let item = emulator
            .custom_line_item(
                &group,
                "narrowed",
                json!({ "BillingPeriodRange": created_with }),
            )
            .await;
        let before = emulator.stored_at(case, &item, &readable_at).await;
        // The instants this service records are wall-clock milliseconds, so the delete is
        // made in a LATER millisecond than the create — otherwise "advances" could not be
        // told from "unchanged" whatever the handler did.
        std::thread::sleep(std::time::Duration::from_millis(5));

        emulator.delete_ok(case, &item, &deleted_with).await;

        let after = emulator.stored_at(case, &item, &readable_at).await;
        let modified_before = before.get("LastModifiedTime").and_then(Value::as_i64);
        let modified_after = after.get("LastModifiedTime").and_then(Value::as_i64);
        if narrows {
            assert!(
                modified_after > modified_before,
                "case {case:?}: expected LastModifiedTime to advance past \
                 {modified_before:?}, got {modified_after:?}"
            );
        } else {
            assert_eq!(
                modified_after, modified_before,
                "case {case:?}: expected LastModifiedTime to stay at {modified_before:?}, \
                 got {modified_after:?}"
            );
        }
        assert_eq!(
            after.get("CreationTime"),
            before.get("CreationTime"),
            "case {case:?}: expected CreationTime to be the instant the item was created, \
             got {after}"
        );
    }
}

/// THE RULE on an `Arn` that names no stored item: the delete is refused with the
/// modelled 400 `ValidationException` and the reason the model enumerates for a custom
/// line item that is not there, whatever became of the item.
#[tokio::test]
async fn a_delete_of_an_arn_that_names_no_stored_item_is_refused() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let deleted = emulator
        .custom_line_item(&group, "already_deleted", json!({}))
        .await;
    emulator
        .delete_ok("already deleted", &deleted, &Value::Null)
        .await;

    // An item whose billing group was deleted goes with it: a custom line item applies
    // to a billing group, so it cannot outlive one.
    let orphaned_group = emulator.billing_group("second").await;
    let orphaned = emulator
        .custom_line_item(&orphaned_group, "orphaned", json!({}))
        .await;
    let (status, body) = emulator
        .send(
            "POST",
            "/delete-billing-group",
            &json!({ "Arn": orphaned_group }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the billing group delete: expected HTTP 200, got {status} with {body}"
    );

    // (case, the Arn the request carries)
    let rows: Vec<(&str, String)> = vec![
        (
            "a bare id nothing ever minted",
            "ZZZZZZZZZZ".to_string(),
        ),
        (
            "a full ARN nothing ever minted",
            format!(
                "arn:aws:billingconductor::{}:customlineitem/YYYYYYYYYY",
                ::engine::emulator_account_id()
            ),
        ),
        ("an item already deleted, by full ARN", deleted.clone()),
        ("an item already deleted, by bare id", bare_id(&deleted)),
        ("an item its billing group took with it", orphaned.clone()),
    ];

    for (case, arn) in rows {
        for span in [Value::Null, range("2026-06", None)] {
            let (status, body) = emulator.delete(&arn, &span).await;
            assert_refused(
                &format!("{case} with BillingPeriodRange={span}"),
                status,
                &body,
                "MISSING_CUSTOMLINEITEM",
                "Arn",
            );
        }
    }
}

/// THE RULE on a billing period range that covers no period: the start is inclusive and
/// the end exclusive, so an end at or before the start names nothing to delete and the
/// request is refused with the stored item left exactly as it was.
#[tokio::test]
async fn a_billing_period_range_that_covers_no_period_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, "kept", json!({})).await;
    let before = emulator.stored("the untouched item", &item).await;

    // (case, the start, the end) — equal bounds, one month reversed, far reversed, and
    // equal bounds spelled differently. A span that covers no period names nothing to
    // delete whatever period it names, so the rows span both a start inside the window
    // the operation may act in and starts outside it.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an end equal to the start, in the window",
            period(0),
            period(0),
        ),
        (
            "an end one month before the start, in the window",
            period(0),
            period(-1),
        ),
        (
            "an end equal to the start",
            "2026-05".to_string(),
            "2026-05".to_string(),
        ),
        (
            "an end one month before the start",
            "2026-05".to_string(),
            "2026-04".to_string(),
        ),
        (
            "an end years before the start",
            "2026-05".to_string(),
            "2019-12".to_string(),
        ),
        (
            "equal bounds spelled both ways",
            "2026-5".to_string(),
            "2026-05".to_string(),
        ),
        (
            "an end in the same month, one-digit",
            "2026-5".to_string(),
            "2026-5".to_string(),
        ),
    ];

    for (case, start, end) in rows {
        let (status, body) = emulator.delete(&item, &range(&start, Some(&end))).await;
        assert_refused(
            case,
            status,
            &body,
            "ILLEGAL_BILLING_PERIOD_RANGE",
            "BillingPeriodRange.ExclusiveEndBillingPeriod",
        );
        assert_eq!(
            emulator.stored(case, &item).await,
            before,
            "case {case:?}: a refused delete must leave the stored item untouched"
        );
    }
}

/// THE RULE on the billing period a delete may act in: `DeleteCustomLineItem` deletes
/// the item "in the current, or previous billing period", so the period its
/// `BillingPeriodRange` STARTS in — the period from which the deletion takes effect —
/// has to be one of those two. A range starting in any other period (a month whose bill
/// is closed, or one that has not begun) is refused with the modelled 400
/// `ValidationException` and the reason the model enumerates for a period this
/// operation may not act in, and the stored item keeps every period it applied in.
///
/// The rows differ along how far outside the window the start lies and in which
/// direction (one month, a year, a decade, the epoch month, each end of the modelled
/// `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$`), how the range is bounded (open
/// end / bounded), and how the month is spelled — plus the two periods the window does
/// admit and the month on each side of them.
#[tokio::test]
async fn a_delete_whose_range_starts_outside_the_current_or_previous_period_is_refused() {
    // (case, the range the delete names, whether the operation may act there)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a range starting in the current billing period, open ended",
            range(&period(0), None),
            true,
        ),
        (
            "a range starting in the current billing period, bounded",
            range(&period(0), Some(&period(1))),
            true,
        ),
        (
            "a range from the previous billing period to the current one",
            range(&period(-1), Some(&period(0))),
            true,
        ),
        (
            "a range starting in the previous billing period, one-digit month",
            range(&one_digit_month(&period(-1)), None),
            true,
        ),
        (
            "one month before the previous billing period",
            range(&period(-2), Some(&period(-1))),
            false,
        ),
        (
            "a year before the current billing period",
            range(&period(-12), None),
            false,
        ),
        (
            "a decade before the current billing period",
            range(&period(-120), Some(&period(-119))),
            false,
        ),
        (
            // The month the `Instant` members this service records count from.
            "the epoch month",
            range("1970-01", None),
            false,
        ),
        (
            // The earliest month the modelled `BillingPeriod` pattern admits.
            "the first month of year zero",
            range("0000-01", Some("0001-01")),
            false,
        ),
        (
            "the month after the current billing period",
            range(&period(1), None),
            false,
        ),
        (
            "a year after the current billing period",
            range(&period(12), Some(&period(13))),
            false,
        ),
        (
            "a future month spelled with a one-digit month",
            range(&one_digit_month(&period(2)), None),
            false,
        ),
        (
            // The latest month the modelled `BillingPeriod` pattern admits.
            "the last month of year 9999",
            range("9999-12", None),
            false,
        ),
    ];

    for (case, deleted_with, inside_the_window) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        // An item with no span of its own applies to EVERY billing period, so a range
        // the operation may act in always has something of it to delete and one it may
        // not act in is refused for the period it names, not for naming nothing.
        let item = emulator.custom_line_item(&group, "guarded", json!({})).await;
        let before = emulator.stored(case, &item).await;

        let (status, body) = emulator.delete(&item, &deleted_with).await;
        if inside_the_window {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
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
        // Nothing of the item moved: it is still reported, unchanged, for the periods
        // it applied in — including the ones the refused range named.
        assert_eq!(
            emulator.stored(case, &item).await,
            before,
            "case {case:?}: a refused delete must leave the stored item untouched"
        );
        for probe in [period(-120), period(-1), period(0), period(120)] {
            assert!(
                emulator.reports(case, &item, Some(&probe)).await,
                "case {case:?}: expected the read for {probe:?} to still report the item"
            );
        }
    }
}

/// THE RULE on a start that is no billing period at all: it names none of the periods
/// the operation may act in, so the request is refused with the modelled 400
/// `ValidationException` rather than deleting anything.
#[tokio::test]
async fn a_delete_whose_range_starts_at_a_value_that_is_no_billing_period_is_refused() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, "guarded", json!({})).await;
    let before = emulator.stored("the untouched item", &item).await;

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
        let (status, body) = emulator.delete(&item, &range(start, None)).await;
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
            emulator.stored(case, &item).await,
            before,
            "case {case:?}: a refused delete must leave the stored item untouched"
        );
    }
}

/// THE RULE on what a delete takes with the item: an association names a resource that
/// exists, so the associations other percentage custom line items held TO the deleted
/// item go with it — and nothing else about those items changes.
#[tokio::test]
async fn deleting_an_item_releases_the_associations_other_items_held_to_it() {
    let case = "a percentage item associated to a deleted child item";
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let child = emulator.custom_line_item(&group, "child", json!({})).await;
    let sibling = emulator
        .custom_line_item(&group, "sibling", json!({}))
        .await;
    let parent = emulator
        .custom_line_item(
            &group,
            "parent",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": {
                        "PercentageValue": 10.0,
                        "AssociatedValues": [child, sibling],
                    },
                },
            }),
        )
        .await;

    let before = emulator.stored(case, &parent).await;
    assert_eq!(
        before.get("AssociationSize").and_then(Value::as_i64),
        Some(2),
        "case {case:?}: expected the parent to report its two associations, got {before}"
    );

    emulator.delete_ok(case, &child, &Value::Null).await;

    let after = emulator.stored(case, &parent).await;
    assert_eq!(
        after.get("AssociationSize").and_then(Value::as_i64),
        Some(1),
        "case {case:?}: expected the deleted item's association to be released, got {after}"
    );
    assert!(
        emulator.reports(case, &sibling, None).await,
        "case {case:?}: expected the sibling the delete did not name to stay readable"
    );
    assert_eq!(
        after.get("ChargeDetails"),
        before.get("ChargeDetails"),
        "case {case:?}: expected the parent's charge details to be unchanged, got {after}"
    );

    // The resources the surface reports as associated to the parent no longer include
    // the deleted item.
    let (status, associated) = emulator
        .send(
            "POST",
            "/list-resources-associated-to-custom-line-item",
            &json!({ "Arn": parent }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the association read to answer HTTP 200, got {status} \
         with {associated}"
    );
    let arns: Vec<String> = associated
        .get("AssociatedResources")
        .and_then(Value::as_array)
        .map(|elements| {
            elements
                .iter()
                .filter_map(|element| element.get("Arn").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![sibling],
        "case {case:?}: expected only the resource the delete did not name, got {associated}"
    );
}

/// THE RULE under REPEATED bounded deletes: each one takes only the periods ITS range
/// covers, so the stretches of billing periods the item still applies in accumulate —
/// every period no delete has named is still reported, however many deletes landed, and
/// the record is only gone once nothing of it is left.
#[tokio::test]
async fn repeated_bounded_deletes_each_take_only_the_periods_they_name() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    // No span of its own, so the item starts out applying to every billing period and
    // each delete carves out exactly the periods it names.
    let item = emulator.custom_line_item(&group, "twice", json!({})).await;

    // (the span the delete names, the periods a read still reports the item for
    // afterwards, the periods it does not) — the second delete names periods abutting the
    // first hole, so two bounded deletes leave the periods on either side of both.
    let steps: Vec<(Value, Vec<String>, Vec<String>)> = vec![
        (
            range(&period(0), Some(&period(1))),
            vec![period(-2), period(-1), period(1), period(12)],
            vec![period(0)],
        ),
        (
            range(&period(-1), Some(&period(0))),
            vec![period(-2), period(1), period(12)],
            vec![period(-1), period(0)],
        ),
    ];

    for (index, (deleted_with, reported, unreported)) in steps.into_iter().enumerate() {
        let case = format!("delete {} of {deleted_with}", index + 1);
        emulator.delete_ok(&case, &item, &deleted_with).await;
        for probe in &reported {
            assert!(
                emulator.reports(&case, &item, Some(probe)).await,
                "case {case:?}: expected the read for {probe:?} to still report the item"
            );
        }
        for probe in &unreported {
            assert!(
                !emulator.reports(&case, &item, Some(probe)).await,
                "case {case:?}: expected the read for {probe:?} not to report the item"
            );
        }
    }

    // What is left of the item is still ONE stored item with its own history: a
    // `CustomLineItemVersionListElement` carries one start and one end billing period,
    // so the generation is reported by the OUTER bounds of the stretches it still
    // applies in. Read on an item bisected once, where both bounds are the ones its
    // create named.
    let case = "the history of a bisected item";
    let bounded = emulator
        .custom_line_item(
            &group,
            "bisected",
            json!({ "BillingPeriodRange": range(&period(-1), Some(&period(6))) }),
        )
        .await;
    emulator
        .delete_ok(case, &bounded, &range(&period(0), Some(&period(2))))
        .await;
    assert!(
        emulator.reports(case, &bounded, Some(&period(5))).await
            && !emulator.reports(case, &bounded, Some(&period(1))).await,
        "case {case:?}: expected the bisected item to be reported for {:?} and not for {:?}",
        period(5),
        period(1)
    );
    let (status, versions) = emulator
        .send(
            "POST",
            "/list-custom-line-item-versions",
            &json!({ "Arn": bounded }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {versions}"
    );
    let elements = versions
        .get("CustomLineItemVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        elements.len(),
        1,
        "case {case:?}: expected the item's one generation, got {versions}"
    );
    let generation = elements.first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        generation.get("StartBillingPeriod").and_then(Value::as_str),
        Some(period(-1).as_str()),
        "case {case:?}: expected the generation to start at {:?}, got {generation}",
        period(-1)
    );
    assert_eq!(
        generation.get("EndBillingPeriod").and_then(Value::as_str),
        Some(period(6).as_str()),
        "case {case:?}: expected the generation to end at {:?}, got {generation}",
        period(6)
    );
}
