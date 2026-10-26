// SPDX-License-Identifier: CC0-1.0

//! A custom line item is VERSIONED by billing period, end to end over the wire:
//! `CreateBillingGroup` -> `CreateCustomLineItem` -> `UpdateCustomLineItem` ->
//! `ListCustomLineItems` / `ListCustomLineItemVersions` / `GetBillingGroupCostReport`.
//!
//! RULE: a write applied from a billing period on changes the item only for that period
//! and the later ones — it opens a new version there — while every earlier billing period
//! keeps the version that was in force in it, so a read scoped to a billing period
//! reports the version in force in THAT period, whichever operation does the reading.
//!
//! The rows span what the rule generalises over rather than one recorded call: which
//! member the update changes (`Name`, `Description`, the flat charge value, the
//! percentage value, the `LineItemFilters`), the charge kind of the item (flat /
//! percentage), the form of the billing-period range the write names (open-ended,
//! bounded, one month, a month spelled without its leading zero), how many writes have
//! landed (one, three), how far the period read is from the write (the period before it,
//! several periods before it, the period itself, years after it) and which operation does
//! the read (the item listing, the version history, the cost report).

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

/// The billing period every item here is created to apply from, so each probe period is
/// either inside the item's life or knowingly before it.
const CREATED_FROM: &str = "2026-01";

/// How far two reported amounts may differ and still be the same amount.
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
    /// one — the world in which these calls are reachable at all. The denial every other
    /// caller gets is asserted in `tests/all/payer_account_gate.rs`.
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
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// Create a custom line item applying from [`CREATED_FROM`] on, overriding the
    /// default members with whatever `request` carries, and return its ARN.
    async fn custom_line_item(&self, group_arn: &str, name: &str, request: Value) -> String {
        let mut body = json!({
            "Name": name,
            "Description": "the original custom line item",
            "BillingGroupArn": group_arn,
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": CREATED_FROM },
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

    /// `UpdateCustomLineItem` that is expected to succeed.
    async fn update_ok(&self, case: &str, arn: &str, request: Value) {
        let mut body = json!({ "Arn": arn });
        for (member, value) in members(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/update-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
    }

    /// The item as `ListCustomLineItems` reports it for `billing_period`, or `None` when
    /// that read reports no such item.
    async fn listed(&self, case: &str, arn: &str, billing_period: &str) -> Option<Value> {
        let (status, response) = self
            .send(
                "POST",
                "/list-custom-line-items",
                &json!({ "BillingPeriod": billing_period, "Filters": { "Arns": [arn] } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read for {billing_period} to answer HTTP 200, got \
             {status} with {response}"
        );
        let elements = response
            .get("CustomLineItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            elements.len() <= 1,
            "case {case:?}: one ARN names at most one item in {billing_period}, got {elements:?}"
        );
        elements.into_iter().next()
    }

    /// The item as the read of `billing_period` is required to report it.
    async fn listed_ok(&self, case: &str, arn: &str, billing_period: &str) -> Value {
        self.listed(case, arn, billing_period)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the item to be reported for {billing_period}")
            })
    }

    /// Every generation `ListCustomLineItemVersions` reports for the item, oldest first.
    async fn versions(&self, case: &str, arn: &str) -> Vec<Value> {
        let (status, body) = self
            .send(
                "POST",
                "/list-custom-line-item-versions",
                &json!({ "Arn": arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("CustomLineItemVersions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The pro forma cost of each billing period a cost report over `[start, end)`
    /// covers, keyed by the period it is reported for.
    async fn proforma_by_period(
        &self,
        case: &str,
        group: &str,
        start: &str,
        end: &str,
    ) -> Vec<(String, f64)> {
        let (status, body) = self
            .send(
                "POST",
                "/get-billing-group-cost-report",
                &json!({
                    "Arn": group,
                    "BillingPeriodRange": {
                        "InclusiveStartBillingPeriod": start,
                        "ExclusiveEndBillingPeriod": end,
                    },
                    "GroupBy": ["BILLING_PERIOD"],
                }),
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
            .iter()
            .map(|row| {
                let period = row
                    .get("Attributes")
                    .and_then(Value::as_array)
                    .and_then(|attributes| attributes.first().cloned())
                    .and_then(|attribute| {
                        attribute
                            .get("Value")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a BILLING_PERIOD attribute, got {row}")
                    });
                let proforma = row
                    .get("ProformaCost")
                    .and_then(Value::as_str)
                    .and_then(|amount| amount.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a decimal ProformaCost, got {row}")
                    });
                (period, proforma)
            })
            .collect()
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

/// The value a reported item carries at `path`, a slash-separated member path, as JSON —
/// `Value::Null` when the item carries nothing there.
fn at(item: &Value, path: &str) -> Value {
    item.pointer(&format!("/{path}"))
        .cloned()
        .unwrap_or(Value::Null)
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

/// A fresh emulator holding one billing group and one custom line item created from
/// `create`, returning the emulator, the group ARN and the item ARN.
async fn fixture(name: &str, create: Value) -> (Emulator, String, String) {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, name, create).await;
    (emulator, group, item)
}

/// One row of the single-write table: what the item is created as, what the update
/// changes and from which billing periods each value is then expected.
struct Row {
    case: &'static str,
    /// The `ChargeDetails` the item is CREATED with, which fix its charge kind.
    create: Value,
    /// The update body, including the `BillingPeriodRange` it is applied in.
    update: Value,
    /// The member path the row compares, slash-separated.
    path: &'static str,
    /// The value the billing periods before the write keep.
    kept: Value,
    /// The value the billing periods the write claims report.
    updated: Value,
    /// The periods that must report `kept`.
    keeping: Vec<&'static str>,
    /// The periods that must report `updated`.
    updating: Vec<&'static str>,
    /// The periods that must report no item at all.
    absent: Vec<&'static str>,
}

/// THE RULE on a single write: the update's values answer for the billing period it is
/// applied from and the later ones its range covers, and every earlier period still
/// reports the value the item had there.
#[tokio::test]
async fn an_update_changes_only_the_billing_periods_it_is_applied_in() {
    let filter = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    let rows: Vec<Row> = vec![
        Row {
            case: "the name, applied from an open-ended range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Name": "renamed",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-06" },
            }),
            path: "Name",
            kept: json!("versioned"),
            updated: json!("renamed"),
            keeping: vec!["2026-01", "2026-02", "2026-05"],
            updating: vec!["2026-06", "2026-07", "2031-12"],
            absent: vec![],
        },
        Row {
            case: "the description, applied from an open-ended range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Description": "a new description",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-06" },
            }),
            path: "Description",
            kept: json!("the original custom line item"),
            updated: json!("a new description"),
            keeping: vec!["2026-01", "2026-05"],
            updating: vec!["2026-06", "2027-01"],
            absent: vec![],
        },
        Row {
            case: "the flat charge value, applied in a bounded range",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "ChargeDetails": { "Flat": { "ChargeValue": 42.5 } },
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-06",
                    "ExclusiveEndBillingPeriod": "2026-09",
                },
            }),
            path: "ChargeDetails/Flat/ChargeValue",
            kept: json!(10.0),
            updated: json!(42.5),
            keeping: vec!["2026-01", "2026-05"],
            updating: vec!["2026-06", "2026-07", "2026-08"],
            // The write named the periods the item applies in from 2026-06 on, so the
            // item applies in none of the periods from its exclusive end.
            absent: vec!["2026-09", "2027-01"],
        },
        Row {
            case: "the percentage value of a percentage item",
            create: json!({ "Type": "CREDIT", "Percentage": { "PercentageValue": 2.0 } }),
            update: json!({
                "ChargeDetails": { "Percentage": { "PercentageValue": 7.5 } },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-06" },
            }),
            path: "ChargeDetails/Percentage/PercentageValue",
            kept: json!(2.0),
            updated: json!(7.5),
            keeping: vec!["2026-01", "2026-05"],
            updating: vec!["2026-06", "2029-03"],
            absent: vec![],
        },
        Row {
            case: "the line item filters",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "ChargeDetails": { "LineItemFilters": [filter.clone()] },
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": "2026-06" },
            }),
            path: "ChargeDetails/LineItemFilters",
            kept: Value::Null,
            updated: json!([filter.clone()]),
            keeping: vec!["2026-01", "2026-05"],
            updating: vec!["2026-06", "2026-12"],
            absent: vec![],
        },
        Row {
            case: "a one-month range, with the month spelled without its leading zero",
            create: json!({ "Type": "FEE", "Flat": { "ChargeValue": 10.0 } }),
            update: json!({
                "Name": "one_month",
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": "2026-6",
                    "ExclusiveEndBillingPeriod": "2026-7",
                },
            }),
            path: "Name",
            kept: json!("versioned"),
            updated: json!("one_month"),
            keeping: vec!["2026-01", "2026-05", "2026-5"],
            updating: vec!["2026-06", "2026-6"],
            absent: vec!["2026-07", "2026-08"],
        },
    ];

    for row in rows {
        let case = row.case;
        let (emulator, _group, item) =
            fixture("versioned", json!({ "ChargeDetails": row.create })).await;
        emulator.update_ok(case, &item, row.update).await;

        for period in row.keeping {
            let read = emulator.listed_ok(case, &item, period).await;
            assert_eq!(
                at(&read, row.path),
                row.kept,
                "case {case:?}: expected {period} to keep {} at {}, got {read}",
                row.kept,
                row.path
            );
        }
        for period in row.updating {
            let read = emulator.listed_ok(case, &item, period).await;
            assert_eq!(
                at(&read, row.path),
                row.updated,
                "case {case:?}: expected {period} to report {} at {}, got {read}",
                row.updated,
                row.path
            );
        }
        for period in row.absent {
            let read = emulator.listed(case, &item, period).await;
            assert!(
                read.is_none(),
                "case {case:?}: expected no item at all for {period}, got {read:?}"
            );
        }
    }
}

/// THE RULE swept over a contiguous run of billing periods: after one open-ended write,
/// every period of the item's life reports exactly one version — the earlier periods the
/// one the item had, the rest the one the write left — and no period the item applied in
/// before the write stops reporting it.
#[tokio::test]
async fn every_billing_period_of_the_item_reports_one_version_after_an_open_ended_write() {
    let (emulator, _group, item) = fixture("swept", json!({})).await;
    let case = "a sweep across the write";
    // The write lands in the middle of the swept run, so periods on both sides of it are
    // checked, plus the period before the item existed at all.
    let applied_from = "2026-07";
    emulator
        .update_ok(
            case,
            &item,
            json!({
                "Name": "swept_update",
                "BillingPeriodRange": { "InclusiveStartBillingPeriod": applied_from },
            }),
        )
        .await;

    assert!(
        emulator.listed(case, &item, "2025-12").await.is_none(),
        "case {case:?}: the item applies from {CREATED_FROM}, so the period before it \
         reports nothing"
    );
    for month in 1..=24 {
        let period = format!("2026-{month:02}");
        let period = if month <= 12 {
            period
        } else {
            format!("2027-{:02}", month - 12)
        };
        let read = emulator.listed_ok(case, &item, &period).await;
        let expected = if period.as_str() < applied_from {
            "swept"
        } else {
            "swept_update"
        };
        assert_eq!(
            at(&read, "Name"),
            json!(expected),
            "case {case:?}: expected {period} to report {expected:?}, got {read}"
        );
    }
}

/// THE RULE under REPEATED writes: each one takes over from the billing period it is
/// applied in, so the item holds one version per stretch of periods, every stretch keeps
/// the values its own write left, and the history reports each generation with the
/// stretch it applies in.
#[tokio::test]
async fn repeated_writes_leave_one_version_per_stretch_of_billing_periods() {
    let (emulator, group, item) = fixture("history", json!({})).await;
    let case = "three writes, each applied from a later billing period";
    // (the period the write is applied from, the name and the flat charge value it
    // leaves)
    let writes = [
        ("2026-03", "second", 20.0),
        ("2026-06", "third", 30.0),
        ("2026-09", "fourth", 40.0),
    ];
    for (from, name, charge_value) in writes {
        emulator
            .update_ok(
                case,
                &item,
                json!({
                    "Name": name,
                    "ChargeDetails": { "Flat": { "ChargeValue": charge_value } },
                    "BillingPeriodRange": { "InclusiveStartBillingPeriod": from },
                }),
            )
            .await;
    }

    // (the period read, the name and the charge value the version in force there holds)
    let expected = [
        ("2026-01", "history", 10.0),
        ("2026-02", "history", 10.0),
        ("2026-03", "second", 20.0),
        ("2026-05", "second", 20.0),
        ("2026-06", "third", 30.0),
        ("2026-08", "third", 30.0),
        ("2026-09", "fourth", 40.0),
        ("2030-04", "fourth", 40.0),
    ];
    for (period, name, charge_value) in expected {
        let read = emulator.listed_ok(case, &item, period).await;
        assert_eq!(
            at(&read, "Name"),
            json!(name),
            "case {case:?}: expected {period} to report {name:?}, got {read}"
        );
        assert_eq!(
            at(&read, "ChargeDetails/Flat/ChargeValue"),
            json!(charge_value),
            "case {case:?}: expected {period} to charge {charge_value}, got {read}"
        );
    }

    // The history is those same four generations, each with the stretch it applies in.
    let reported = emulator.versions(case, &item).await;
    let generations = [
        ("history", CREATED_FROM, Some("2026-03")),
        ("second", "2026-03", Some("2026-06")),
        ("third", "2026-06", Some("2026-09")),
        ("fourth", "2026-09", None),
    ];
    assert_eq!(
        reported.len(),
        generations.len(),
        "case {case:?}: expected {} generations, got {reported:?}",
        generations.len()
    );
    for (version, (name, start, end)) in reported.iter().zip(generations) {
        assert_eq!(
            at(version, "Name"),
            json!(name),
            "case {case:?}: expected the generation named {name:?}, got {version}"
        );
        assert_eq!(
            at(version, "StartBillingPeriod"),
            json!(start),
            "case {case:?}: expected the {name:?} generation to start in {start}, got {version}"
        );
        assert_eq!(
            at(version, "EndBillingPeriod"),
            end.map_or(Value::Null, |end| json!(end)),
            "case {case:?}: expected the {name:?} generation to end at {end:?}, got {version}"
        );
    }

    // The bill of a billing period is computed from the version in force in it, so the
    // charge each period carries is that generation's own charge value.
    let reported = emulator
        .proforma_by_period(case, &group, "2026-01", "2026-11")
        .await;
    let expected = [
        ("2026-01", 10.0),
        ("2026-02", 10.0),
        ("2026-03", 20.0),
        ("2026-04", 20.0),
        ("2026-05", 20.0),
        ("2026-06", 30.0),
        ("2026-07", 30.0),
        ("2026-08", 30.0),
        ("2026-09", 40.0),
        ("2026-10", 40.0),
    ];
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected one row per billing period, got {reported:?}"
    );
    for ((period, proforma), (expected_period, expected_proforma)) in reported.iter().zip(expected)
    {
        assert_eq!(
            period, expected_period,
            "case {case:?}: expected the {expected_period} row, got {period}"
        );
        assert!(
            (proforma - expected_proforma).abs() < TOLERANCE,
            "case {case:?}: expected the {expected_period} bill to be {expected_proforma}, got \
             {proforma}"
        );
    }
}

/// A write that names NO billing period range takes over wherever the item applies, so it
/// leaves no earlier version behind: every period of the item's life reports the values it
/// left, and the history is still the one generation.
#[tokio::test]
async fn a_write_naming_no_range_replaces_the_version_it_applies_in() {
    let (emulator, _group, item) = fixture("whole", json!({})).await;
    let case = "an update naming no billing period range";
    emulator
        .update_ok(case, &item, json!({ "Name": "replaced" }))
        .await;

    for period in ["2026-01", "2026-06", "2027-11"] {
        let read = emulator.listed_ok(case, &item, period).await;
        assert_eq!(
            at(&read, "Name"),
            json!("replaced"),
            "case {case:?}: expected {period} to report the replaced version, got {read}"
        );
    }
    let reported = emulator.versions(case, &item).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the item's one generation, got {reported:?}"
    );
}
