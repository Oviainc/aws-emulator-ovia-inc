// SPDX-License-Identifier: CC0-1.0

//! `ListCustomLineItemVersions` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` -> `UpdateCustomLineItem` -> `ListCustomLineItemVersions`.
//!
//! RULE: `ListCustomLineItemVersions` reports the PERSISTED generations of the one
//! custom line item its `Arn` names (in either form the `CustomLineItemArn` pattern
//! admits), oldest first — one element per generation, carrying the values that
//! generation held, the billing periods it is in effect in (`StartBillingPeriod`
//! inclusive; `EndBillingPeriod` exclusive: the end its own write named, or the period
//! the generation that replaced it took effect in) and the instant it took effect
//! (`StartTime`). A write that takes effect in a new billing period opens a generation
//! and closes the one before it; a write inside the current generation's billing period
//! replaces that generation, because one billing period has one version of the item.
//! The newest element is the item as a read of the item itself reports it, a frozen
//! element keeps what it held, the request's billing-period filter selects the
//! generations in effect anywhere in that span, `MaxResults`/`NextToken` page the
//! sequence, and an `Arn` that names no stored item — or a filter range whose end
//! precedes its start — is refused 400 `ValidationException`.
//!
//! The rows span what the rule generalises over rather than one recorded call: how many
//! writes the item has taken (none, one, several), which billing period each takes
//! effect in (the generation's own, a later one), whether a write names a range at all
//! and whether that range has an end, how a month is spelled (`2019-1` / `2019-01`),
//! which attribute a write changes, where a filter's span falls relative to a
//! generation's (before, at the start, inside, across, after, open at either end), the
//! page size (each published bound of `MaxCustomLineItemResults` and one past each), and
//! the form the `Arn` takes (full ARN / bare resource id).

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

/// `MaxCustomLineItemResults`' published bounds — the `@range` of this operation's
/// `MaxResults` (`smithy_query(query="shape",shape="MaxCustomLineItemResults")`).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 100);

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this service's control plane
    /// is reachable at all (see `tests/all/payer_account_gate.rs` for the denial every
    /// other caller gets).
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

    /// `UpdateCustomLineItem` on `arn`, which this case requires to succeed.
    async fn update_ok(&self, case: &str, arn: &str, request: Value) {
        let mut body = json!({ "Arn": arn });
        for (member, value) in object(&request) {
            body[member] = value;
        }
        let (status, response) = self.send("POST", "/update-custom-line-item", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer HTTP 200, got {status} with {response}"
        );
    }

    /// `BatchAssociateResourcesToCustomLineItem`, which this case requires to succeed.
    async fn associate_ok(&self, case: &str, target: &str, resources: &[String]) {
        let (status, response) = self
            .send(
                "PUT",
                "/batch-associate-resources-to-custom-line-item",
                &json!({ "TargetArn": target, "ResourceArns": resources }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the association to answer HTTP 200, got {status} with \
             {response}"
        );
    }

    /// `ListCustomLineItemVersions` on `arn` with whatever other members `request`
    /// carries.
    async fn versions(&self, arn: &str, request: Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        for (member, value) in object(&request) {
            body[member] = value;
        }
        self.send("POST", "/list-custom-line-item-versions", &body)
            .await
    }

    /// The versions a read reports, in the order it reports them, for a read this case
    /// requires to succeed.
    async fn versions_ok(&self, case: &str, arn: &str, request: Value) -> Vec<Value> {
        let (status, body) = self.versions(arn, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        body.get("CustomLineItemVersions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The item as `ListCustomLineItems` reports it for `billing_period`, or `None` when
    /// that read reports no such item.
    async fn item(&self, case: &str, arn: &str, billing_period: &str) -> Option<Value> {
        let (status, body) = self
            .send(
                "POST",
                "/list-custom-line-items",
                &json!({ "BillingPeriod": billing_period, "Filters": { "Arns": [arn] } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the item read to answer HTTP 200, got {status} with {body}"
        );
        body.get("CustomLineItems")
            .and_then(Value::as_array)
            .and_then(|elements| elements.first().cloned())
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

/// A `BillingPeriodRange` as a write carries it (`@required
/// InclusiveStartBillingPeriod`, optional `ExclusiveEndBillingPeriod`).
fn period_range(start: &str, end: Option<&str>) -> Value {
    let mut range = json!({ "InclusiveStartBillingPeriod": start });
    if let Some(end) = end {
        range["ExclusiveEndBillingPeriod"] = json!(end);
    }
    range
}

/// A `BillingPeriod` as a comparable month count, so `2019-7` and `2019-07` are the
/// same period — the normalisation the modelled pattern (`^\d{4}-(0?[1-9]|1[012])$`)
/// makes necessary.
fn period_key(value: &str) -> Option<i64> {
    let (year, month) = value.split_once('-')?;
    Some(year.parse::<i64>().ok()? * 12 + month.parse::<i64>().ok()?)
}

/// Do two values name the same billing period, whatever their spelling? `None` on
/// either side names no period, so only two absent bounds agree.
fn same_period(actual: Option<&str>, expected: Option<&str>) -> bool {
    match (actual, expected) {
        (Some(actual), Some(expected)) => period_key(actual) == period_key(expected),
        (None, None) => true,
        _ => false,
    }
}

/// The `(StartBillingPeriod, EndBillingPeriod)` of every reported version, in order.
fn spans(versions: &[Value]) -> Vec<(Option<String>, Option<String>)> {
    versions
        .iter()
        .map(|version| {
            (
                version
                    .get("StartBillingPeriod")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                version
                    .get("EndBillingPeriod")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// A dot-separated member path of a reported version, or `Value::Null` when the path
/// names nothing.
fn pluck(version: &Value, path: &str) -> Value {
    let mut cursor = version;
    for member in path.split('.') {
        match cursor.get(member) {
            Some(next) => cursor = next,
            None => return Value::Null,
        }
    }
    cursor.clone()
}

/// One `LineItemFilter`, as the model shapes it
/// (`smithy_query(query="shape",shape="LineItemFilter")`).
fn line_item_filter(attribute: &str, match_option: &str, attribute_value: &str) -> Value {
    json!({
        "Attribute": attribute,
        "MatchOption": match_option,
        "AttributeValues": [attribute_value],
    })
}

/// Assert a refusal: the status, the modelled error shape and, for a
/// `ValidationException`, the `Reason` the model enumerates and the member at fault.
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
            .is_some_and(|fields| {
                fields
                    .iter()
                    .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                    .any(|name| name == field)
            });
        assert!(
            named,
            "case {case:?}: expected Fields to name {field:?}, got {body}"
        );
    }
}

/// One version's billing-period span as a row expects it:
/// `(StartBillingPeriod, EndBillingPeriod)`, the end absent when the generation is
/// open-ended.
type Span = (&'static str, Option<&'static str>);

/// One row of the history table: the case, the range the create names, the update
/// bodies to apply in order, and the span of every version expected afterwards, oldest
/// first.
type HistoryRow = (&'static str, Value, Vec<Value>, Vec<Span>);

/// One row of the refusal table: the case, the `Arn` the request names, the rest of the
/// request, and the expected `Reason` and member at fault (absent where the refusal is
/// the framework's own modelled-constraint answer).
type RefusalRow = (
    &'static str,
    String,
    Value,
    Option<&'static str>,
    Option<&'static str>,
);

/// A fresh emulator holding one billing group and one custom line item created from
/// `create`, returning the emulator, the group ARN and the item ARN.
async fn fixture(create: Value) -> (Emulator, String, String) {
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    let item = emulator.custom_line_item(&group, "original", create).await;
    (emulator, group, item)
}

/// THE RULE on the history: one version per billing period the item has been in effect
/// in with a configuration of its own — a write taking effect in a new billing period
/// opens one and closes the one before it at that period, a write taking effect in the
/// current generation's period replaces it, and the newest generation is open-ended
/// unless its own write named an end.
#[tokio::test]
async fn the_history_is_one_version_per_billing_period_the_item_was_in_effect_in() {
    // (case, the range the create names, the update bodies in order, the
    // (StartBillingPeriod, EndBillingPeriod) of every expected version, oldest first)
    let rows: Vec<HistoryRow> = vec![
        (
            "a create and nothing else",
            period_range("2019-01", None),
            vec![],
            vec![("2019-01", None)],
        ),
        (
            "one update in a later billing period",
            period_range("2019-01", None),
            vec![json!({ "BillingPeriodRange": period_range("2019-03", None) })],
            vec![("2019-01", Some("2019-03")), ("2019-03", None)],
        ),
        (
            "two updates in later billing periods",
            period_range("2019-01", None),
            vec![
                json!({ "BillingPeriodRange": period_range("2019-02", None) }),
                json!({ "BillingPeriodRange": period_range("2019-05", None) }),
            ],
            vec![
                ("2019-01", Some("2019-02")),
                ("2019-02", Some("2019-05")),
                ("2019-05", None),
            ],
        ),
        (
            "four writes, one per consecutive billing period",
            period_range("2019-01", None),
            vec![
                json!({ "BillingPeriodRange": period_range("2019-02", None) }),
                json!({ "BillingPeriodRange": period_range("2019-03", None) }),
                json!({ "BillingPeriodRange": period_range("2019-04", None) }),
            ],
            vec![
                ("2019-01", Some("2019-02")),
                ("2019-02", Some("2019-03")),
                ("2019-03", Some("2019-04")),
                ("2019-04", None),
            ],
        ),
        (
            "an update in the generation's own billing period replaces it",
            period_range("2019-01", None),
            vec![json!({
                "Name": "renamed",
                "BillingPeriodRange": period_range("2019-01", None),
            })],
            vec![("2019-01", None)],
        ),
        (
            "an update naming no range at all stays in the generation",
            period_range("2019-01", None),
            vec![json!({ "Name": "renamed" })],
            vec![("2019-01", None)],
        ),
        (
            "a successor closes a generation before the end its own write named",
            period_range("2019-01", Some("2019-06")),
            vec![json!({ "BillingPeriodRange": period_range("2019-03", None) })],
            vec![("2019-01", Some("2019-03")), ("2019-03", None)],
        ),
        (
            "the newest generation ends where its own write said",
            period_range("2019-01", None),
            vec![json!({
                "BillingPeriodRange": period_range("2019-01", Some("2019-04")),
            })],
            vec![("2019-01", Some("2019-04"))],
        ),
        (
            "a month spelled without its leading zero names the same period",
            period_range("2019-1", None),
            vec![json!({ "BillingPeriodRange": period_range("2019-03", None) })],
            vec![("2019-01", Some("2019-3")), ("2019-3", None)],
        ),
    ];

    for (case, create, updates, expected) in rows {
        let (emulator, _, item) = fixture(json!({ "BillingPeriodRange": create })).await;
        for update in &updates {
            emulator.update_ok(case, &item, update.clone()).await;
        }

        let reported = emulator.versions_ok(case, &item, json!({})).await;
        let actual = spans(&reported);
        assert_eq!(
            actual.len(),
            expected.len(),
            "case {case:?}: expected {} versions, got {actual:?}",
            expected.len()
        );
        for (index, ((start, end), (expected_start, expected_end))) in
            actual.iter().zip(expected.iter()).enumerate()
        {
            assert!(
                same_period(start.as_deref(), Some(expected_start)),
                "case {case:?}: expected version {index} to start in {expected_start:?}, got \
                 {start:?}"
            );
            assert!(
                same_period(end.as_deref(), *expected_end),
                "case {case:?}: expected version {index} to end at {expected_end:?}, got {end:?}"
            );
        }

        // Every version names the item it is a version of, and the instants are ordered:
        // the shared creation time, then each generation's own start time.
        let mut previous_start_time = 0;
        for (index, version) in reported.iter().enumerate() {
            assert_eq!(
                version.get("Arn").and_then(Value::as_str),
                Some(item.as_str()),
                "case {case:?}: expected version {index} to name {item}, got {version}"
            );
            let creation_time = pluck(version, "CreationTime").as_i64().unwrap_or_default();
            let start_time = pluck(version, "StartTime").as_i64().unwrap_or_default();
            let modified_time = pluck(version, "LastModifiedTime")
                .as_i64()
                .unwrap_or_default();
            assert!(
                creation_time > 0 && start_time >= creation_time,
                "case {case:?}: expected version {index} to start at or after the item's \
                 creation, got start {start_time} and creation {creation_time}"
            );
            assert!(
                modified_time >= start_time,
                "case {case:?}: expected version {index} to be modified at or after it took \
                 effect, got {modified_time} and {start_time}"
            );
            assert!(
                start_time >= previous_start_time,
                "case {case:?}: expected the versions ordered oldest first, got version {index} \
                 starting at {start_time} after {previous_start_time}"
            );
            previous_start_time = start_time;
        }
    }
}

/// THE RULE on the values: each version carries the values that generation held, so a
/// frozen generation keeps what it had while the newest one is the item as a read of the
/// item itself reports it.
#[tokio::test]
async fn each_version_reports_the_values_that_generation_held() {
    let filter = line_item_filter("SERVICE", "EQUAL", "AWSCostExplorer");
    // (case, what the update changes, the member path, its value in the frozen version,
    // its value in the newest one)
    let rows: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "the name",
            json!({ "Name": "renamed" }),
            "Name",
            json!("original"),
            json!("renamed"),
        ),
        (
            "the description",
            json!({ "Description": "a new description" }),
            "Description",
            json!("the original custom line item"),
            json!("a new description"),
        ),
        (
            "the flat charge value",
            json!({ "ChargeDetails": { "Flat": { "ChargeValue": 42.5 } } }),
            "ChargeDetails.Flat.ChargeValue",
            json!(1.0),
            json!(42.5),
        ),
        (
            "the charge value at the top of its modelled range",
            json!({ "ChargeDetails": { "Flat": { "ChargeValue": 1000000.0 } } }),
            "ChargeDetails.Flat.ChargeValue",
            json!(1.0),
            json!(1000000.0),
        ),
        (
            "the line item filters",
            json!({ "ChargeDetails": { "LineItemFilters": [filter.clone()] } }),
            "ChargeDetails.LineItemFilters",
            Value::Null,
            json!([filter.clone()]),
        ),
    ];

    for (case, change, path, frozen_value, newest_value) in rows {
        let (emulator, group, item) =
            fixture(json!({ "BillingPeriodRange": period_range("2019-01", None) })).await;
        let mut update = object(&change);
        update.insert(
            "BillingPeriodRange".to_string(),
            period_range("2019-03", None),
        );
        emulator
            .update_ok(case, &item, Value::Object(update))
            .await;

        let reported = emulator.versions_ok(case, &item, json!({})).await;
        assert_eq!(
            reported.len(),
            2,
            "case {case:?}: expected the frozen generation and the newest one, got {reported:?}"
        );
        assert_eq!(
            pluck(&reported[0], path),
            frozen_value,
            "case {case:?}: expected the frozen version's {path} to be {frozen_value}, got {}",
            reported[0]
        );
        assert_eq!(
            pluck(&reported[1], path),
            newest_value,
            "case {case:?}: expected the newest version's {path} to be {newest_value}, got {}",
            reported[1]
        );

        // Every generation of one item shares its identity: the billing group it applies
        // to, the currency of its partition and its creation instant; and nothing in
        // this surface sets a product code, so none is reported.
        for (index, version) in reported.iter().enumerate() {
            assert_eq!(
                pluck(version, "BillingGroupArn"),
                json!(group),
                "case {case:?}: expected version {index} to name the item's billing group, got \
                 {version}"
            );
            assert_eq!(
                pluck(version, "CurrencyCode"),
                json!("USD"),
                "case {case:?}: expected the currency of this partition on version {index}, got \
                 {version}"
            );
            assert!(
                pluck(version, "ProductCode").is_null(),
                "case {case:?}: expected no ProductCode on version {index}, got {version}"
            );
        }
        assert_eq!(
            pluck(&reported[0], "CreationTime"),
            pluck(&reported[1], "CreationTime"),
            "case {case:?}: expected every version to share the item's creation time, got \
             {reported:?}"
        );

        // ROUND TRIP: the newest version is the item a read of the item itself reports
        // for the billing period that generation applies in.
        let stored = emulator
            .item(case, &item, "2019-03")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected {item} to be readable"));
        for member in [
            "Arn",
            "Name",
            "Description",
            "BillingGroupArn",
            "ChargeDetails",
            "CurrencyCode",
            "AssociationSize",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                pluck(&reported[1], member),
                pluck(&stored, member),
                "case {case:?}: expected the newest version's {member} to be the stored item's, \
                 got {} and {stored}",
                reported[1]
            );
        }
    }
}

/// A frozen generation keeps the `AssociationSize` it had: the history reports what the
/// item WAS, so associating a resource afterwards changes the newest version only.
#[tokio::test]
async fn a_frozen_version_keeps_the_association_size_it_had() {
    let case = "associations before and after the generation was frozen";
    let emulator = Emulator::new();
    let group = emulator.billing_group("payer").await;
    // Only a PERCENTAGE custom line item has associated resources.
    let item = emulator
        .custom_line_item(
            &group,
            "original",
            json!({
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } },
                "BillingPeriodRange": period_range("2019-01", None),
            }),
        )
        .await;
    let first = emulator
        .custom_line_item(&group, "first_child", json!({}))
        .await;
    let second = emulator
        .custom_line_item(&group, "second_child", json!({}))
        .await;

    emulator
        .associate_ok(case, &item, std::slice::from_ref(&first))
        .await;
    emulator
        .update_ok(
            case,
            &item,
            json!({ "BillingPeriodRange": period_range("2019-03", None) }),
        )
        .await;
    emulator
        .associate_ok(case, &item, std::slice::from_ref(&second))
        .await;

    let reported = emulator.versions_ok(case, &item, json!({})).await;
    assert_eq!(
        reported.len(),
        2,
        "case {case:?}: expected the frozen generation and the newest one, got {reported:?}"
    );
    assert_eq!(
        pluck(&reported[0], "AssociationSize"),
        json!(1),
        "case {case:?}: expected the frozen version to keep its one association, got {}",
        reported[0]
    );
    assert_eq!(
        pluck(&reported[1], "AssociationSize"),
        json!(2),
        "case {case:?}: expected the newest version to report both associations, got {}",
        reported[1]
    );
}

/// THE RULE on the filter: a billing-period range selects the generations in effect
/// anywhere inside it — both of its bounds inclusive, an absent bound open.
#[tokio::test]
async fn the_billing_period_filter_selects_the_versions_in_effect_in_that_span() {
    // The fixture's generations: [2019-01, 2019-03), [2019-03, 2019-06), [2019-06, ..).
    // (case, the filter, the StartBillingPeriod of every expected version)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter at all", json!({}), vec!["2019-01", "2019-03", "2019-06"]),
        (
            "an empty filter",
            json!({ "Filters": {} }),
            vec!["2019-01", "2019-03", "2019-06"],
        ),
        (
            "one period, the first generation's own start",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-01", "EndBillingPeriod": "2019-01" } } }),
            vec!["2019-01"],
        ),
        (
            "one period inside the middle generation",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-04", "EndBillingPeriod": "2019-04" } } }),
            vec!["2019-03"],
        ),
        (
            "a span across two generations",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-02", "EndBillingPeriod": "2019-03" } } }),
            vec!["2019-01", "2019-03"],
        ),
        (
            "a span open at its end",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-03" } } }),
            vec!["2019-03", "2019-06"],
        ),
        (
            "a span open at its start",
            json!({ "Filters": { "BillingPeriodRange": {
                "EndBillingPeriod": "2019-02" } } }),
            vec!["2019-01"],
        ),
        (
            "a span entirely after the last generation began",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-09", "EndBillingPeriod": "2019-11" } } }),
            vec!["2019-06"],
        ),
        (
            "a span entirely before the item existed",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2018-01", "EndBillingPeriod": "2018-12" } } }),
            vec![],
        ),
        (
            "months spelled without their leading zeroes",
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-6", "EndBillingPeriod": "2019-6" } } }),
            vec!["2019-06"],
        ),
    ];

    for (case, request, expected) in rows {
        let (emulator, _, item) =
            fixture(json!({ "BillingPeriodRange": period_range("2019-01", None) })).await;
        for start in ["2019-03", "2019-06"] {
            emulator
                .update_ok(
                    case,
                    &item,
                    json!({ "BillingPeriodRange": period_range(start, None) }),
                )
                .await;
        }

        let reported = emulator.versions_ok(case, &item, request).await;
        let starts = spans(&reported);
        assert_eq!(
            starts.len(),
            expected.len(),
            "case {case:?}: expected {} versions, got {starts:?}",
            expected.len()
        );
        for (index, ((start, _), expected_start)) in starts.iter().zip(expected.iter()).enumerate() {
            assert!(
                same_period(start.as_deref(), Some(expected_start)),
                "case {case:?}: expected the selected version {index} to start in \
                 {expected_start:?}, got {start:?}"
            );
        }
    }
}

/// THE RULE on paging: whatever the page size, walking the pages reports every version
/// once, in the order an unpaged read reports them, and the last page carries no
/// `NextToken`.
#[tokio::test]
async fn paging_walks_every_version_once_in_order() {
    let (emulator, _, item) =
        fixture(json!({ "BillingPeriodRange": period_range("2019-01", None) })).await;
    for start in ["2019-02", "2019-03", "2019-04"] {
        emulator
            .update_ok(
                "the paged fixture",
                &item,
                json!({ "BillingPeriodRange": period_range(start, None) }),
            )
            .await;
    }
    let unpaged = emulator
        .versions_ok("unpaged", &item, json!({}))
        .await;
    assert_eq!(
        unpaged.len(),
        4,
        "case \"unpaged\": expected the four generations, got {unpaged:?}"
    );

    // Each published bound of `MaxResults`, and the sizes between one and the number of
    // versions there are.
    for max_results in [MAX_RESULTS_BOUNDS.0, 2, 3, 4, MAX_RESULTS_BOUNDS.1] {
        let case = format!("MaxResults {max_results}");
        let mut walked: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": max_results });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.versions(&item, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let page = body
                .get("CustomLineItemVersions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case:?}: expected at most {max_results} versions in a page, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= 8,
                "case {case:?}: expected the walk to end, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: expected the paged walk to report every version once in order, got \
             {walked:?}"
        );
    }
}

/// THE RULE on identity and on reads: either spelling of the `Arn` names the same item,
/// and a version read changes nothing a later caller sees.
#[tokio::test]
async fn either_spelling_of_the_arn_reports_the_same_history_and_a_read_changes_nothing() {
    let case = "both spellings of the Arn";
    let (emulator, _, item) =
        fixture(json!({ "BillingPeriodRange": period_range("2019-01", None) })).await;
    emulator
        .update_ok(
            case,
            &item,
            json!({ "BillingPeriodRange": period_range("2019-03", None) }),
        )
        .await;

    let by_arn = emulator.versions_ok(case, &item, json!({})).await;
    let by_id = emulator
        .versions_ok(case, &bare_id(&item), json!({}))
        .await;
    assert_eq!(
        by_arn, by_id,
        "case {case:?}: expected the bare resource id to report the same history, got {by_id:?}"
    );

    let again = emulator.versions_ok(case, &item, json!({})).await;
    assert_eq!(
        again, by_arn,
        "case {case:?}: expected a repeated read to report the same history, got {again:?}"
    );
    let stored = emulator.item(case, &item, "2019-03").await;
    assert!(
        stored.is_some(),
        "case {case:?}: expected the item itself to be unchanged by the reads"
    );
}

/// THE RULE on refusals: an `Arn` that names no stored item, a filter range whose end
/// precedes its start, and a `MaxResults` outside its modelled range are each refused
/// 400 `ValidationException`; a value the model allows is served.
#[tokio::test]
async fn a_request_the_store_or_the_model_cannot_serve_is_refused() {
    let (emulator, _, item) =
        fixture(json!({ "BillingPeriodRange": period_range("2019-01", None) })).await;
    let absent_arn = format!(
        "arn:aws:billingconductor::{}:customlineitem/BBBBBBBBBB",
        ::engine::emulator_account_id()
    );

    // (case, the Arn, the rest of the request, the expected Reason and member at fault;
    // `None` where the refusal is the framework's own modelled-constraint answer)
    let refused: Vec<RefusalRow> = vec![
        (
            "a bare resource id that names no item",
            "AAAAAAAAAA".to_string(),
            json!({}),
            Some("MISSING_CUSTOMLINEITEM"),
            Some("Arn"),
        ),
        (
            "a full ARN that names no item",
            absent_arn,
            json!({}),
            Some("MISSING_CUSTOMLINEITEM"),
            Some("Arn"),
        ),
        (
            "a filter range whose end precedes its start",
            item.clone(),
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": "2019-05", "EndBillingPeriod": "2019-03" } } }),
            Some("ILLEGAL_BILLING_PERIOD_RANGE"),
            Some("Filters.BillingPeriodRange.EndBillingPeriod"),
        ),
        (
            "a MaxResults one below its modelled range",
            item.clone(),
            json!({ "MaxResults": MAX_RESULTS_BOUNDS.0 - 1 }),
            None,
            None,
        ),
        (
            "a MaxResults one above its modelled range",
            item.clone(),
            json!({ "MaxResults": MAX_RESULTS_BOUNDS.1 + 1 }),
            None,
            None,
        ),
    ];

    for (case, arn, request, reason, field) in refused {
        let (status, body) = emulator.versions(&arn, request).await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "ValidationException",
            reason,
            field,
        );
    }

    // Each published bound of `MaxResults` is served.
    for max_results in [MAX_RESULTS_BOUNDS.0, MAX_RESULTS_BOUNDS.1] {
        let case = format!("MaxResults {max_results}, a published bound");
        let reported = emulator
            .versions_ok(&case, &item, json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the item's one generation, got {reported:?}"
        );
    }

    // A deleted item has no history to report, so the read is refused exactly as it is
    // for an item that never existed.
    let (status, body) = emulator
        .send("POST", "/delete-custom-line-item", &json!({ "Arn": item }))
        .await;
    assert_eq!(
        status, 200,
        "case \"the delete\": expected HTTP 200, got {status} with {body}"
    );
    let (status, body) = emulator.versions(&item, json!({})).await;
    assert_refused(
        "the history of a deleted item",
        status,
        &body,
        400,
        "ValidationException",
        Some("MISSING_CUSTOMLINEITEM"),
        Some("Arn"),
    );
}

/// An item created without a `BillingPeriodRange` is in effect from the billing period
/// its create landed in, so its one version reports that period and a filter naming it
/// selects that version.
#[tokio::test]
async fn a_create_without_a_range_starts_the_version_in_the_period_it_landed_in() {
    let case = "a create naming no billing period range";
    let (emulator, _, item) = fixture(json!({})).await;

    let reported = emulator.versions_ok(case, &item, json!({})).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the item's one generation, got {reported:?}"
    );
    let start = reported[0]
        .get("StartBillingPeriod")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        period_key(&start).is_some(),
        "case {case:?}: expected a billing period as StartBillingPeriod, got {start:?}"
    );
    assert!(
        reported[0]
            .get("EndBillingPeriod")
            .is_none_or(Value::is_null),
        "case {case:?}: expected the open-ended generation to report no end, got {}",
        reported[0]
    );

    let selected = emulator
        .versions_ok(
            case,
            &item,
            json!({ "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": start, "EndBillingPeriod": start } } }),
        )
        .await;
    assert_eq!(
        selected, reported,
        "case {case:?}: expected the version to be selected by the period it starts in, got \
         {selected:?}"
    );
}
