// SPDX-License-Identifier: CC0-1.0

//! `UpdateCustomLineItem` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` -> `UpdateCustomLineItem` -> `ListCustomLineItems`.
//!
//! RULE: `UpdateCustomLineItem` REPLACES, on the stored custom line item its `Arn`
//! names (in either form the `CustomLineItemArn` pattern admits), exactly the
//! attributes the request carries — `Name`, `Description`, the charge value of the
//! item's own charge kind, the charge details' `LineItemFilters`, and the
//! billing-period range the updated item applies in — keeps every attribute the
//! request omits, advances `LastModifiedTime`, and reports the item as stored; it
//! refuses an `Arn` that names no stored item with 404 `ResourceNotFoundException`,
//! and charge details naming both kinds or the kind the item is not, or a
//! billing-period range that covers no period, with 400 `ValidationException` while
//! leaving the stored item untouched.
//!
//! The rows span what the rule generalises over rather than one recorded call: which
//! members the request sets (each alone, in combination, and none at all), the charge
//! kind of the item (flat / percentage), where a charge value falls relative to its
//! modelled range (both published bounds, inside, and one step past each), the form the
//! `Arn` takes (full ARN / bare resource id), how a `LineItemFilters` list is spelled
//! and how long it is (absent / one filter of either `Attribute` / cleared), how the
//! billing-period range is ordered
//! (open end, one month apart, far apart, equal bounds, reversed) and how a billing
//! period is spelled (`2026-1` / `2026-01`).

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

/// `CustomLineItemChargeValue`'s published bounds — the `@range` of the flat charge
/// value this operation updates
/// (`smithy_query(query="shape",shape="CustomLineItemChargeValue")`).
const FLAT_CHARGE_BOUNDS: (f64, f64) = (0.0, 1_000_000.0);

/// `CustomLineItemPercentageChargeValue`'s published bounds — the `@range` of the
/// percentage value this operation updates
/// (`smithy_query(query="shape",shape="CustomLineItemPercentageChargeValue")`).
const PERCENTAGE_BOUNDS: (f64, f64) = (0.0, 10_000.0);

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
    /// one — the world in which the update this suite is about is reachable at all. The
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

    /// `UpdateCustomLineItem` on `arn` with whatever other members `request` carries.
    async fn update(&self, arn: &str, request: Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        for (member, value) in object(&request) {
            body[member] = value;
        }
        self.send("POST", "/update-custom-line-item", &body).await
    }

    /// `UpdateCustomLineItem` that is expected to succeed.
    async fn update_ok(&self, case: &str, arn: &str, request: Value) -> Value {
        let (status, body) = self.update(arn, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body
    }

    /// The stored item as `ListCustomLineItems` reports it for `billing_period` (the
    /// current one when `None`), or `None` when the read reports no such item.
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

    /// The stored item, which this case requires to be readable.
    async fn stored(&self, case: &str, arn: &str) -> Value {
        self.read_back(case, arn, None)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn} to be readable"))
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

/// A fresh emulator holding one billing group and one custom line item created from
/// `create`, returning the emulator, the group ARN and the item ARN.
async fn fixture(name: &str, create: Value) -> (Emulator, String, String) {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, name, create).await;
    (emulator, group, item)
}

/// One `LineItemFilter`, as the model shapes it
/// (`smithy_query(query="shape",shape="LineItemFilter")`: `@required Attribute`,
/// `@required MatchOption`, and the value lists).
fn line_item_filter(attribute: &str, match_option: &str, attribute_value: &str) -> Value {
    json!({
        "Attribute": attribute,
        "MatchOption": match_option,
        "AttributeValues": [attribute_value],
    })
}

/// Assert a refusal: the status, the modelled error shape, and (for a
/// `ValidationException`) the `Reason` the model enumerates and the member at fault.
fn assert_refused(
    case: &str,
    status: u16,
    body: &Value,
    expected_status: u16,
    expected_shape: &str,
    expected_reason: Option<&str>,
    expected_field: Option<&str>,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
    );
    assert_eq!(
        body.get("__type").and_then(Value::as_str),
        Some(expected_shape),
        "case {case:?}: expected the modelled {expected_shape}, got {body}"
    );
    let message = body.get("Message").and_then(Value::as_str).unwrap_or("");
    assert!(
        !message.is_empty(),
        "case {case:?}: the modelled @required Message must state the refusal, got {body}"
    );
    if let Some(reason) = expected_reason {
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some(reason),
            "case {case:?}: expected the Reason {reason:?}, got {body}"
        );
    }
    if let Some(field) = expected_field {
        let named = body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                    .any(|name| name == field)
            })
            .unwrap_or(false);
        assert!(
            named,
            "case {case:?}: expected Fields to name {field:?}, got {body}"
        );
    }
}

/// THE RULE on the members: every member the request carries replaces the stored one,
/// every member it omits keeps what state holds, and the answer is what a later read of
/// the same item reports.
#[tokio::test]
async fn every_member_the_request_carries_replaces_the_stored_one_and_the_rest_is_kept() {
    // (case, the update body, the Name / Description / flat charge value / filter count
    // the item is expected to carry afterwards)
    let filter = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    let rows: Vec<(&str, Value, &str, &str, f64, usize)> = vec![
        ("nothing but the Arn", json!({}), "original", "the original custom line item", 1.0, 0),
        ("Name alone", json!({ "Name": "renamed" }), "renamed", "the original custom line item", 1.0, 0),
        (
            "Description alone",
            json!({ "Description": "a new description" }),
            "original",
            "a new description",
            1.0,
            0,
        ),
        (
            "Name and Description",
            json!({ "Name": "renamed_again", "Description": "both at once" }),
            "renamed_again",
            "both at once",
            1.0,
            0,
        ),
        (
            "the flat charge value alone",
            json!({ "ChargeDetails": { "Flat": { "ChargeValue": 42.5 } } }),
            "original",
            "the original custom line item",
            42.5,
            0,
        ),
        (
            "the line item filters alone",
            json!({ "ChargeDetails": { "LineItemFilters": [filter.clone()] } }),
            "original",
            "the original custom line item",
            1.0,
            1,
        ),
        (
            "the charge value and the filters together",
            json!({
                "ChargeDetails": {
                    "Flat": { "ChargeValue": 7.0 },
                    "LineItemFilters": [filter.clone()],
                },
            }),
            "original",
            "the original custom line item",
            7.0,
            1,
        ),
        (
            "every member at once",
            json!({
                "Name": "everything",
                "Description": "every member at once",
                "ChargeDetails": {
                    "Flat": { "ChargeValue": 1234.5 },
                    "LineItemFilters": [filter.clone()],
                },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-1" },
            }),
            "everything",
            "every member at once",
            1234.5,
            1,
        ),
    ];

    for (case, request, name, description, charge_value, filters) in rows {
        let (emulator, group, item) = fixture("original", json!({})).await;
        let before = emulator.stored(case, &item).await;
        let created_at = before
            .get("CreationTime")
            .and_then(Value::as_i64)
            .unwrap_or_default();

        let answer = emulator.update_ok(case, &item, request).await;
        assert_eq!(
            answer.get("Arn").and_then(Value::as_str),
            Some(item.as_str()),
            "case {case:?}: expected the updated item's ARN, got {answer}"
        );
        assert_eq!(
            answer.get("BillingGroupArn").and_then(Value::as_str),
            Some(group.as_str()),
            "case {case:?}: expected the billing group the item applies to, got {answer}"
        );
        assert_eq!(
            answer.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected the Name {name:?}, got {answer}"
        );
        assert_eq!(
            answer.get("Description").and_then(Value::as_str),
            Some(description),
            "case {case:?}: expected the Description {description:?}, got {answer}"
        );
        assert_eq!(
            answer
                .pointer("/ChargeDetails/Flat/ChargeValue")
                .and_then(Value::as_f64),
            Some(charge_value),
            "case {case:?}: expected the flat charge value {charge_value}, got {answer}"
        );
        assert_eq!(
            answer.pointer("/ChargeDetails/Percentage"),
            None,
            "case {case:?}: a flat item reports no percentage value, got {answer}"
        );
        assert_eq!(
            answer.pointer("/ChargeDetails/Type").and_then(Value::as_str),
            Some("FEE"),
            "case {case:?}: the charge Type is the one the create set, got {answer}"
        );
        assert_eq!(
            answer
                .pointer("/ChargeDetails/LineItemFilters")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0),
            filters,
            "case {case:?}: expected {filters} line item filter(s), got {answer}"
        );
        assert_eq!(
            answer.get("AssociationSize").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: nothing is associated to a flat item, got {answer}"
        );
        let modified = answer
            .get("LastModifiedTime")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            modified >= created_at && modified > 0,
            "case {case:?}: expected LastModifiedTime at or after the creation time \
             {created_at}, got {modified}"
        );

        // The ROUND TRIP: the same values are what a later read reports, and the
        // creation time is not rewritten by an update.
        let read = emulator
            .read_back(case, &item, Some("2026-1"))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {item} to be readable"));
        for member in [
            "Name",
            "Description",
            "ChargeDetails",
            "BillingGroupArn",
            "LastModifiedTime",
            "AssociationSize",
        ] {
            assert_eq!(
                read.get(member),
                answer.get(member),
                "case {case:?}: the read must report the {member} the update answered, got \
                 {read} after {answer}"
            );
        }
        assert_eq!(
            read.get("CreationTime").and_then(Value::as_i64),
            Some(created_at),
            "case {case:?}: an update does not rewrite the creation time, got {read}"
        );
    }
}

/// One charge kind a custom line item can have, and the values its own charge member
/// takes.
struct ChargeKind {
    case: &'static str,
    /// The charge details the item is CREATED with, which fix its kind.
    create: Value,
    /// The `UpdateCustomLineItemChargeDetails` member that carries this kind.
    member: &'static str,
    /// That member's own value member.
    value_member: &'static str,
    /// Values inside the modelled `@range`, including both published bounds.
    accepted: Vec<f64>,
    /// One step past each published bound.
    refused: Vec<f64>,
}

impl ChargeKind {
    /// The update body that sets THIS kind's charge value. Built member by member
    /// because the member names are rows of the table rather than literals.
    fn charge_value_update(&self, value: f64) -> Value {
        let mut kind = Map::new();
        kind.insert(self.value_member.to_string(), json!(value));
        let mut charge_details = Map::new();
        charge_details.insert(self.member.to_string(), Value::Object(kind));
        json!({ "ChargeDetails": Value::Object(charge_details) })
    }
}

/// The charge value the update replaces is the one of the item's OWN kind, across the
/// whole modelled range: both published bounds and a value inside are stored, and one
/// step past either bound is refused by the modelled constraint.
#[tokio::test]
async fn the_charge_value_is_replaced_across_its_modelled_range_and_refused_outside_it() {
    let kinds = [
        ChargeKind {
            case: "a flat item",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 1.0 } }),
            member: "Flat",
            value_member: "ChargeValue",
            accepted: vec![FLAT_CHARGE_BOUNDS.0, 12.25, FLAT_CHARGE_BOUNDS.1],
            refused: vec![FLAT_CHARGE_BOUNDS.0 - 0.01, FLAT_CHARGE_BOUNDS.1 + 0.01],
        },
        ChargeKind {
            case: "a percentage item",
            create: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.0 } }),
            member: "Percentage",
            value_member: "PercentageValue",
            accepted: vec![PERCENTAGE_BOUNDS.0, 12.5, PERCENTAGE_BOUNDS.1],
            refused: vec![PERCENTAGE_BOUNDS.0 - 0.01, PERCENTAGE_BOUNDS.1 + 0.01],
        },
    ];

    for kind in &kinds {
        let (emulator, _group, item) =
            fixture("charges", json!({ "ChargeDetails": kind.create.clone() })).await;
        let path = format!("/ChargeDetails/{}/{}", kind.member, kind.value_member);

        for &value in &kind.accepted {
            let case = format!("{} updated to {value}", kind.case);
            let answer = emulator
                .update_ok(&case, &item, kind.charge_value_update(value))
                .await;
            assert_eq!(
                answer.pointer(&path).and_then(Value::as_f64),
                Some(value),
                "case {case:?}: expected the answer to carry {value}, got {answer}"
            );
            let read = emulator.stored(&case, &item).await;
            assert_eq!(
                read.pointer(&path).and_then(Value::as_f64),
                Some(value),
                "case {case:?}: expected the stored item to carry {value}, got {read}"
            );
        }

        for &value in &kind.refused {
            let case = format!(
                "{} updated to {value}, outside the modelled range",
                kind.case
            );
            let (status, body) = emulator
                .update(&item, kind.charge_value_update(value))
                .await;
            assert_refused(&case, status, &body, 400, "ValidationException", None, None);
        }
    }
}

/// Charge details that name both kinds, or the kind the item is not, are refused —
/// which kind an item has is fixed when it is created — and the stored item is left
/// exactly as it was.
#[tokio::test]
async fn charge_details_naming_both_kinds_or_the_wrong_one_are_refused_and_change_nothing() {
    let flat = json!({ "Type": "FEE", "Flat": { "ChargeValue": 3.0 } });
    let percentage = json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 4.0 } });
    // (case, the create's charge details, the update's charge details, the member the
    // refusal names)
    let rows: Vec<(&str, Value, Value, &str)> = vec![
        (
            "a percentage value on a flat item",
            flat.clone(),
            json!({ "Percentage": { "PercentageValue": 5.0 } }),
            "ChargeDetails.Percentage",
        ),
        (
            "a flat charge value on a percentage item",
            percentage.clone(),
            json!({ "Flat": { "ChargeValue": 5.0 } }),
            "ChargeDetails.Flat",
        ),
        (
            "both kinds on a flat item",
            flat.clone(),
            json!({ "Flat": { "ChargeValue": 5.0 }, "Percentage": { "PercentageValue": 5.0 } }),
            "ChargeDetails",
        ),
        (
            "both kinds on a percentage item",
            percentage.clone(),
            json!({ "Flat": { "ChargeValue": 5.0 }, "Percentage": { "PercentageValue": 5.0 } }),
            "ChargeDetails",
        ),
    ];

    for (case, create, update, field) in rows {
        let (emulator, _group, item) = fixture("kinds", json!({ "ChargeDetails": create })).await;
        let before = emulator.stored(case, &item).await;

        let (status, body) = emulator
            .update(&item, json!({ "Name": "refused", "ChargeDetails": update }))
            .await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "ValidationException",
            Some("ILLEGAL_UPDATE_CHARGE_DETAILS"),
            Some(field),
        );

        let after = emulator.stored(case, &item).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update changes nothing, got {after} instead of {before}"
        );
    }
}

/// The billing-period range the request names is the range the updated item applies
/// in: a read of a period inside it reports the item with its new values, and a read
/// outside it reports no such item. A range that covers no billing period at all is
/// refused and changes nothing.
#[tokio::test]
async fn the_billing_period_range_decides_which_periods_report_the_updated_item() {
    // (case, the range, the periods that must report the item, the periods that must not)
    let rows: Vec<(&str, Value, Vec<&str>, Vec<&str>)> = vec![
        (
            "a half-open range of three months",
            json!({
                "InclusiveStartBillingPeriod": "2026-05",
                "ExclusiveEndBillingPeriod": "2026-08",
            }),
            vec!["2026-05", "2026-5", "2026-06", "2026-07"],
            vec!["2026-04", "2026-08", "2027-01"],
        ),
        (
            "an open-ended range",
            json!({ "InclusiveStartBillingPeriod": "2026-10" }),
            vec!["2026-10", "2026-11", "2030-12"],
            vec!["2026-09", "2025-10"],
        ),
        (
            "a one-month range",
            json!({
                "InclusiveStartBillingPeriod": "2026-1",
                "ExclusiveEndBillingPeriod": "2026-2",
            }),
            vec!["2026-1", "2026-01"],
            vec!["2025-12", "2026-02"],
        ),
    ];

    for (case, range, inside, outside) in rows {
        let (emulator, _group, item) = fixture("ranged", json!({})).await;
        emulator
            .update_ok(
                case,
                &item,
                json!({ "Name": "ranged_update", "BillingPeriodRange": range }),
            )
            .await;

        for period in inside {
            let read = emulator
                .read_back(case, &item, Some(period))
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the item to be reported for {period}")
                });
            assert_eq!(
                read.get("Name").and_then(Value::as_str),
                Some("ranged_update"),
                "case {case:?}: expected the updated Name for {period}, got {read}"
            );
        }
        for period in outside {
            let read = emulator.read_back(case, &item, Some(period)).await;
            assert!(
                read.is_none(),
                "case {case:?}: expected no item for {period}, got {read:?}"
            );
        }
    }

    // A range whose exclusive end is at or before its inclusive start covers no billing
    // period, in either spelling of a month.
    let unordered = [
        ("equal bounds", "2026-05", "2026-05"),
        ("equal bounds spelled differently", "2026-5", "2026-05"),
        ("a reversed range", "2026-08", "2026-05"),
        ("a reversed range across a year", "2027-01", "2026-12"),
    ];
    for (case, start, end) in unordered {
        let (emulator, _group, item) = fixture("unordered", json!({})).await;
        let before = emulator.stored(case, &item).await;
        let (status, body) = emulator
            .update(
                &item,
                json!({
                    "Name": "refused",
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": start,
                        "ExclusiveEndBillingPeriod": end,
                    },
                }),
            )
            .await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "ValidationException",
            Some("ILLEGAL_BILLING_PERIOD_RANGE"),
            Some("BillingPeriodRange.ExclusiveEndBillingPeriod"),
        );
        let after = emulator.stored(case, &item).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update changes nothing, got {after} instead of {before}"
        );
    }
}

/// `LineItemFilters` is a LIST member, so a request carrying it states the whole new
/// membership — including an EMPTY one, which clears the filters — and a request that
/// omits it leaves the stored filters alone.
#[tokio::test]
async fn the_line_item_filters_the_request_carries_replace_the_stored_list_and_an_empty_one_clears_it()
{
    // The two `Attribute`s the model enumerates, each with the value list that attribute
    // takes (`smithy_query(query="shape",shape="LineItemFilterAttributeName,
    // LineItemFilter,LineItemFilterValue")`).
    let by_service = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    let by_line_item_type = json!({
        "Attribute": "LINE_ITEM_TYPE",
        "MatchOption": "NOT_EQUAL",
        "Values": ["SAVINGS_PLAN_NEGATION"],
    });

    let (emulator, _group, item) = fixture(
        "filtered",
        json!({
            "ChargeDetails": {
                "Type": "FEE",
                "Flat": { "ChargeValue": 1.0 },
                "LineItemFilters": [by_service.clone()],
            },
        }),
    )
    .await;

    // A request that carries no `ChargeDetails` at all leaves the created filter alone.
    let case = "a Name-only update";
    let answer = emulator
        .update_ok(case, &item, json!({ "Name": "still_filtered" }))
        .await;
    assert_filters(case, &answer, &[("SERVICE", "EQUAL")]);
    let read = emulator.stored(case, &item).await;
    assert_filters(case, &read, &[("SERVICE", "EQUAL")]);

    // Charge details that carry other members but not the list leave it alone too.
    let case = "a charge-value update that omits the filters";
    let answer = emulator
        .update_ok(
            case,
            &item,
            json!({ "ChargeDetails": { "Flat": { "ChargeValue": 2.0 } } }),
        )
        .await;
    assert_filters(case, &answer, &[("SERVICE", "EQUAL")]);

    // A list the request DOES carry replaces the stored membership entirely.
    let case = "a filter on the other attribute";
    let answer = emulator
        .update_ok(
            case,
            &item,
            json!({ "ChargeDetails": { "LineItemFilters": [by_line_item_type] } }),
        )
        .await;
    assert_filters(case, &answer, &[("LINE_ITEM_TYPE", "NOT_EQUAL")]);
    let read = emulator.stored(case, &item).await;
    assert_filters(case, &read, &[("LINE_ITEM_TYPE", "NOT_EQUAL")]);

    // An EMPTY list is a membership of none: the filters are cleared, and an empty list
    // and an absent one are the same membership, so the member is reported absent.
    let case = "an empty filter list";
    let answer = emulator
        .update_ok(
            case,
            &item,
            json!({ "ChargeDetails": { "LineItemFilters": [] } }),
        )
        .await;
    assert_filters(case, &answer, &[]);
    let read = emulator.stored(case, &item).await;
    assert_filters(case, &read, &[]);
    assert_eq!(
        read.pointer("/ChargeDetails/Flat/ChargeValue")
            .and_then(Value::as_f64),
        Some(2.0),
        "case {case:?}: clearing the filters leaves the charge value alone, got {read}"
    );
}

/// Assert the `(Attribute, MatchOption)` of every `LineItemFilter` a reported item
/// carries, in order; an EMPTY expectation requires the member to be absent, because an
/// empty list and an absent one are the same membership.
fn assert_filters(case: &str, body: &Value, expected: &[(&str, &str)]) {
    let reported = body
        .pointer("/ChargeDetails/LineItemFilters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if expected.is_empty() {
        assert!(
            body.pointer("/ChargeDetails/LineItemFilters").is_none(),
            "case {case:?}: expected no line item filters at all, got {body}"
        );
        return;
    }
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected {} line item filter(s), got {body}",
        expected.len()
    );
    for (index, (attribute, match_option)) in expected.iter().enumerate() {
        let filter = &reported[index];
        assert_eq!(
            filter.get("Attribute").and_then(Value::as_str),
            Some(*attribute),
            "case {case:?}: expected filter {index} on {attribute}, got {filter}"
        );
        assert_eq!(
            filter.get("MatchOption").and_then(Value::as_str),
            Some(*match_option),
            "case {case:?}: expected filter {index} to match {match_option}, got {filter}"
        );
    }
}

/// Both forms the `CustomLineItemArn` pattern admits name the SAME stored item, so an
/// update addressed either way lands on it — and the answer always reports the full ARN.
#[tokio::test]
async fn either_form_of_the_arn_names_the_same_stored_item() {
    let (emulator, _group, item) = fixture("addressed", json!({})).await;
    let forms = [("the full ARN", item.clone()), ("the bare id", bare_id(&item))];

    for (case, addressed) in forms {
        let name = format!("named_by_{}", addressed.len());
        let answer = emulator
            .update_ok(case, &addressed, json!({ "Name": name.clone() }))
            .await;
        assert_eq!(
            answer.get("Arn").and_then(Value::as_str),
            Some(item.as_str()),
            "case {case:?}: expected the full ARN in the answer, got {answer}"
        );
        let read = emulator.stored(case, &item).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the update to land on the stored item, got {read}"
        );
    }
}

/// An `Arn` that names no stored item is refused 404 `ResourceNotFoundException`,
/// whichever form it takes, and nothing is created for it.
#[tokio::test]
async fn an_arn_that_names_no_stored_item_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id().to_string();
    let absent = [
        ("a bare id", "ABCDEFGHIJ".to_string()),
        (
            "a full ARN in this account",
            format!("arn:aws:billingconductor::{account}:customlineitem/ZYXWVUTSRQ"),
        ),
        (
            "a full ARN in another account",
            "arn:aws:billingconductor::111122223333:customlineitem/0123456789".to_string(),
        ),
        (
            "a full ARN in the China partition",
            "arn:aws-cn:billingconductor::111122223333:customlineitem/CN12345678".to_string(),
        ),
    ];

    for (case, arn) in absent {
        let (status, body) = emulator
            .update(&arn, json!({ "Description": "updated" }))
            .await;
        assert_refused(
            case,
            status,
            &body,
            404,
            "ResourceNotFoundException",
            None,
            None,
        );
        assert_eq!(
            body.get("ResourceType").and_then(Value::as_str),
            Some("customlineitem"),
            "case {case:?}: expected the refusal to name the resource type, got {body}"
        );
        let resource_id = body
            .get("ResourceId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            resource_id.ends_with(&bare_id(&arn)),
            "case {case:?}: expected the refusal to name the item asked for, got {body}"
        );
        assert!(
            emulator.read_back(case, &arn, None).await.is_none(),
            "case {case:?}: a refused update creates nothing"
        );
    }
}

/// The operation is `@idempotent`: applying the SAME update twice leaves the item in the
/// same state, and a read between the two calls sees the same values the first answered.
#[tokio::test]
async fn applying_the_same_update_twice_leaves_the_same_item() {
    let (emulator, _group, item) = fixture("repeated", json!({})).await;
    let request = json!({
        "Name": "repeated_update",
        "Description": "applied twice",
        "ChargeDetails": { "Flat": { "ChargeValue": 99.5 } },
    });

    let case = "the same update twice";
    let first = emulator.update_ok(case, &item, request.clone()).await;
    let between = emulator.stored(case, &item).await;
    let second = emulator.update_ok(case, &item, request).await;
    let after = emulator.stored(case, &item).await;

    for member in ["Arn", "Name", "Description", "ChargeDetails", "AssociationSize"] {
        assert_eq!(
            first.get(member),
            second.get(member),
            "case {case:?}: expected the {member} to be unchanged by the second call, got \
             {second} after {first}"
        );
        assert_eq!(
            between.get(member),
            after.get(member),
            "case {case:?}: expected the stored {member} to be unchanged, got {after} after \
             {between}"
        );
    }
    let first_modified = first
        .get("LastModifiedTime")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let second_modified = second
        .get("LastModifiedTime")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    assert!(
        second_modified >= first_modified,
        "case {case:?}: LastModifiedTime never goes backwards, got {second_modified} after \
         {first_modified}"
    );
}

/// A read is a read: `ListCustomLineItems` before and after an update reports exactly
/// what the update left, and reading twice reports the same thing both times.
#[tokio::test]
async fn a_read_after_an_update_is_stable() {
    let (emulator, _group, item) = fixture("stable", json!({})).await;
    let case = "a stable read";
    emulator
        .update_ok(case, &item, json!({ "Name": "stable_update" }))
        .await;
    let first = emulator.stored(case, &item).await;
    let second = emulator.stored(case, &item).await;
    assert_eq!(
        first, second,
        "case {case:?}: a read must not change what a later caller sees, got {second} after \
         {first}"
    );
}
