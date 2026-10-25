// SPDX-License-Identifier: CC0-1.0

//! `ListCustomLineItems` end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` (-> `BatchAssociateResourcesToCustomLineItem`) ->
//! `ListCustomLineItems`.
//!
//! RULE: `ListCustomLineItems` REPORTS, for the requested billing period — the current
//! one when the request names none — every stored custom line item whose own half-open
//! `BillingPeriodRange` covers that period and whose attributes satisfy every filter
//! member the request set (each member lists the values of ONE attribute, so it is an
//! independent membership test and an absent member restricts nothing), one page at a
//! time over a stable order, each element derived from the stored item; it writes
//! nothing, so a later read sees exactly what the first one saw.
//!
//! The rows span what the rule generalises over rather than one recorded call: which
//! filter member is set and how many values it lists, the form an ARN-valued filter
//! takes (full ARN / bare resource id), the charge kind of the item (flat / percentage),
//! how many resources are associated to it, which optional members the create set, where
//! the asked-for billing period falls relative to an item's range (before, at the
//! inclusive start, inside, at the exclusive end, after, and either bound left open),
//! how a billing period is spelled (`2026-1` / `2026-01`) and the page size a caller
//! asks for.

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

/// The default page size this service applies when a request names no `MaxResults`: the
/// upper bound of the modelled `MaxCustomLineItemResults` range (1..100)
/// (`smithy_query(query="operation",operation="ListCustomLineItems")`).
const DEFAULT_PAGE_SIZE: usize = 100;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the router was built over, so a test can place a resource this
    /// emulator's ARN builder cannot mint (one in another partition) in the store.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): the store records no payer account by default, so this harness designates
    /// one — the world in which the reads these tests are about are reachable at all.
    /// The denial every other caller gets, `ListCustomLineItems` included, is asserted in
    /// `tests/all/payer_account_gate.rs` over the default store.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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
    /// Create a billing group billing the accounts `linked`, and return its ARN. A
    /// custom line item's `AccountId` has to be one of them, so a fixture that applies
    /// an item to an account links that account here.
    async fn billing_group(&self, name: &str, linked: &[&str]) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": linked },
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

    /// `ListCustomLineItems` with whatever members the caller wants on the request.
    async fn list(&self, request: Value) -> (u16, Value) {
        self.send("POST", "/list-custom-line-items", &request).await
    }

    /// The `Name`s one page of a successful read reports, in the order it reports them.
    async fn listed_names(&self, case: &str, request: Value) -> Vec<String> {
        let (status, body) = self.list(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        names(&body)
    }

    /// Move a stored custom line item into another PARTITION, returning its new ARN.
    ///
    /// Nothing in this emulator mints an ARN outside the `aws` partition
    /// (`engine::emulator_arn` builds `arn:aws:…`), so the only way to read an item of
    /// the China partition — the other partition `CustomLineItemArn` admits — is to put
    /// one in the store, the way this harness designates the payer account.
    fn rehome(&self, arn: &str, partition: &str) -> String {
        let state = self.engine.state::<BillingConductorState>();
        let mut state = state.lock();
        let mut record = state
            .custom_line_items
            .remove(arn)
            .unwrap_or_else(|| panic!("case partition {partition:?}: expected {arn} to be stored"));
        let moved = arn.replacen("arn:aws:", &format!("arn:{partition}:"), 1);
        record.arn = moved.clone();
        state.custom_line_items.insert(moved.clone(), record);
        moved
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

/// The elements of a read's `CustomLineItems`, in response order.
fn elements(body: &Value) -> Vec<Value> {
    body.get("CustomLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Name`s of a read's `CustomLineItems`, in response order.
fn names(body: &Value) -> Vec<String> {
    elements(body)
        .iter()
        .map(|element| {
            element
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The bare resource id of a full ARN — the OTHER form the `BillingGroupArn` and
/// `CustomLineItemArn` patterns admit, so a filter may name a resource either way.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// Four custom line items across two billing groups, differing in the attributes the
/// filter members select on: their names, their billing group, their ARN and the account
/// each applies to.
struct Fixture {
    emulator: Emulator,
    /// The billing group `alpha` and `beta` apply to.
    first_group: String,
    /// The billing group `gamma` and `delta` apply to.
    second_group: String,
    /// The ARN of the `gamma` custom line item.
    gamma: String,
    /// The ARN of the `delta` custom line item.
    delta: String,
}

/// One linked account of the FIRST billing group, which one of its items applies to
/// (`AccountId`, `^[0-9]{12}$`).
const FIRST_ACCOUNT: &str = "111122223333";

/// A linked account of the SECOND billing group, so an `AccountIds` filter can name
/// more than one account. An account belongs to at most one billing group, so the two
/// groups' items apply to different accounts.
const SECOND_ACCOUNT: &str = "222233334444";

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let first_group = emulator.billing_group("first", &[FIRST_ACCOUNT]).await;
        let second_group = emulator.billing_group("second", &[SECOND_ACCOUNT]).await;
        emulator
            .custom_line_item(
                &first_group,
                "alpha",
                json!({ "AccountId": FIRST_ACCOUNT }),
            )
            .await;
        emulator
            .custom_line_item(
                &first_group,
                "beta",
                json!({
                    "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 12.5 } }
                }),
            )
            .await;
        let gamma = emulator
            .custom_line_item(
                &second_group,
                "gamma",
                json!({ "AccountId": SECOND_ACCOUNT }),
            )
            .await;
        let delta = emulator
            .custom_line_item(
                &second_group,
                "delta",
                json!({
                    "AccountId": SECOND_ACCOUNT,
                    "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 1.0 } },
                }),
            )
            .await;
        Self {
            emulator,
            first_group,
            second_group,
            gamma,
            delta,
        }
    }
}

/// Every stored item is reported when nothing filters it out, and each filter member
/// reports exactly the items whose attribute it names — however many values it lists,
/// whichever form an ARN takes, and intersected when several members are set.
#[tokio::test]
async fn every_filter_reports_the_custom_line_items_whose_attribute_it_names() {
    let fixture = Fixture::new().await;
    // Creation order, which is the order a read reports (the ARN carries the monotonic
    // id this service mints).
    let all = ["alpha", "beta", "gamma", "delta"];

    // (case, the `Filters` member of the request, the names expected in response order)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filters at all", json!({}), all.to_vec()),
        (
            "Names with one value",
            json!({ "Names": ["alpha"] }),
            vec!["alpha"],
        ),
        (
            "Names with two values",
            json!({ "Names": ["beta", "gamma"] }),
            vec!["beta", "gamma"],
        ),
        (
            "Names naming no stored item",
            json!({ "Names": ["nosuchitem"] }),
            vec![],
        ),
        (
            "BillingGroups by full ARN",
            json!({ "BillingGroups": [fixture.first_group] }),
            vec!["alpha", "beta"],
        ),
        (
            "BillingGroups by bare id",
            json!({ "BillingGroups": [bare_id(&fixture.second_group)] }),
            vec!["gamma", "delta"],
        ),
        (
            "BillingGroups listing both groups",
            json!({ "BillingGroups": [fixture.first_group, fixture.second_group] }),
            all.to_vec(),
        ),
        (
            "Arns by full ARN",
            json!({ "Arns": [fixture.gamma] }),
            vec!["gamma"],
        ),
        (
            "Arns by bare id",
            json!({ "Arns": [bare_id(&fixture.delta)] }),
            vec!["delta"],
        ),
        (
            "Arns listing two items",
            json!({ "Arns": [fixture.gamma, bare_id(&fixture.delta)] }),
            vec!["gamma", "delta"],
        ),
        (
            "AccountIds with one value",
            json!({ "AccountIds": [FIRST_ACCOUNT] }),
            vec!["alpha"],
        ),
        (
            "AccountIds with both accounts",
            json!({ "AccountIds": [FIRST_ACCOUNT, SECOND_ACCOUNT] }),
            vec!["alpha", "gamma", "delta"],
        ),
        (
            // `AccountIdList` is the one filter list the model lets a caller send empty
            // (`@length(min: 0)`): it names no value to restrict to, so it restricts
            // nothing.
            "AccountIds listing no value",
            json!({ "AccountIds": [] }),
            all.to_vec(),
        ),
        (
            "Names and AccountIds intersected",
            json!({ "Names": ["alpha", "gamma"], "AccountIds": [FIRST_ACCOUNT] }),
            vec!["alpha"],
        ),
        (
            "BillingGroups and Names with no item in common",
            json!({ "BillingGroups": [fixture.second_group], "Names": ["alpha"] }),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = fixture
            .emulator
            .listed_names(case, json!({ "Filters": filters }))
            .await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the custom line items {expected:?}, got {listed:?}"
        );
    }

    // A read writes nothing, so reading again reports the same items.
    let first = fixture.emulator.listed_names("repeated read", json!({})).await;
    let second = fixture.emulator.listed_names("repeated read", json!({})).await;
    assert_eq!(
        second, first,
        "case \"repeated read\": expected a read to leave what a later read sees unchanged, \
         got {first:?} then {second:?}"
    );
    assert_eq!(
        first,
        all.to_vec(),
        "case \"repeated read\": expected every stored custom line item, got {first:?}"
    );
}

/// Each reported element is the stored item: the members the create set, the charge
/// details of the kind it was created with, and the derived `AssociationSize` and
/// `CurrencyCode`.
#[tokio::test]
async fn a_listed_element_reports_what_the_create_persisted() {
    // (case, the create's members beyond Name/Description/BillingGroupArn, how many
    // resources to associate afterwards)
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "a flat FEE with no optional member",
            json!({ "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 42.5 } } }),
            0,
        ),
        (
            "a flat CREDIT, itemized, presented under a service",
            json!({
                "ChargeDetails": { "Type": "CREDIT", "Flat": { "ChargeValue": 0.0 } },
                "AccountId": FIRST_ACCOUNT,
                "ComputationRule": "ITEMIZED",
                "PresentationDetails": { "Service": "AmazonEC2" },
            }),
            0,
        ),
        (
            "a percentage FEE with one associated resource",
            json!({
                "ChargeDetails": {
                    "Type": "FEE",
                    "Percentage": { "PercentageValue": 10.0 },
                    "LineItemFilters": [{
                        "Attribute": "LINE_ITEM_TYPE",
                        "MatchOption": "NOT_EQUAL",
                        "Values": ["SAVINGS_PLAN_NEGATION"],
                    }],
                },
                "ComputationRule": "CONSOLIDATED",
            }),
            1,
        ),
        (
            "a percentage CREDIT with three associated resources",
            json!({
                "ChargeDetails": { "Type": "CREDIT", "Percentage": { "PercentageValue": 100.0 } },
                "AccountId": SECOND_ACCOUNT,
            }),
            3,
        ),
    ];

    for (case, request, associations) in cases {
        let emulator = Emulator::new();
        // The group bills both accounts, because a custom line item's `AccountId` has
        // to be an account of the billing group the item applies to.
        let group = emulator
            .billing_group("home", &[FIRST_ACCOUNT, SECOND_ACCOUNT])
            .await;
        let arn = emulator
            .custom_line_item(&group, "subject", request.clone())
            .await;
        // The resources a percentage item's charge is computed over, all of them in
        // scope of the billing group the item applies to: that group, and custom line
        // items of that same group — so the count varies without the item itself
        // changing.
        let mut resources: Vec<String> = Vec::new();
        for index in 0..associations {
            let resource = if index == 0 {
                group.clone()
            } else {
                emulator
                    .custom_line_item(&group, &format!("resource{index}"), json!({}))
                    .await
            };
            resources.push(resource);
        }
        if !resources.is_empty() {
            emulator.associate(&arn, &resources).await;
        }

        let (status, body) = emulator.list(json!({ "Filters": { "Arns": [arn] } })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let reported = elements(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the created custom line item, got {body}"
        );
        let element = &reported[0];
        let created = members(&request);

        assert_eq!(
            element["Arn"],
            json!(arn),
            "case {case:?}: expected the created ARN, got {element}"
        );
        assert_eq!(
            element["Name"],
            json!("subject"),
            "case {case:?}: expected the created name, got {element}"
        );
        assert_eq!(
            element["Description"],
            json!("the subject custom line item"),
            "case {case:?}: expected the created description, got {element}"
        );
        assert_eq!(
            element["BillingGroupArn"],
            json!(group),
            "case {case:?}: expected the billing group the item applies to, got {element}"
        );
        assert_eq!(
            element["ChargeDetails"],
            created["ChargeDetails"],
            "case {case:?}: expected the charge details the create set, got {element}"
        );
        assert_eq!(
            element.get("AccountId").cloned().unwrap_or(Value::Null),
            created.get("AccountId").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the account the item applies to, got {element}"
        );
        assert_eq!(
            element.get("ComputationRule").cloned().unwrap_or(Value::Null),
            created.get("ComputationRule").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the computation rule the create set, got {element}"
        );
        assert_eq!(
            element
                .get("PresentationDetails")
                .cloned()
                .unwrap_or(Value::Null),
            created
                .get("PresentationDetails")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: expected the presentation details the create set, got {element}"
        );
        assert_eq!(
            element["AssociationSize"],
            json!(associations),
            "case {case:?}: expected {associations} associated resources, got {element}"
        );
        // `CurrencyCode` has exactly two values (CNY, USD), one per partition the
        // `CustomLineItemArn` pattern admits; this emulator's resources live in the
        // `aws` partition, which bills in USD.
        assert_eq!(
            element["CurrencyCode"],
            json!("USD"),
            "case {case:?}: expected the currency of this partition, got {element}"
        );
        // Nothing in this service's surface sets a product code, so none is reported.
        assert!(
            element.get("ProductCode").is_none_or(Value::is_null),
            "case {case:?}: expected no ProductCode, got {element}"
        );
        let creation_time = element["CreationTime"].as_i64().unwrap_or_default();
        let modified_time = element["LastModifiedTime"].as_i64().unwrap_or_default();
        assert!(
            creation_time > 0,
            "case {case:?}: expected a creation time in epoch millis, got {creation_time}"
        );
        if associations == 0 {
            // Nothing changed the item after its create, so the two times are the same
            // instant.
            assert_eq!(
                modified_time, creation_time,
                "case {case:?}: expected an unmodified item's times to agree, got {modified_time} \
                 and {creation_time}"
            );
        } else {
            // Associating resources CHANGED the item, so its last-modified time is at
            // or after its creation.
            assert!(
                modified_time >= creation_time,
                "case {case:?}: expected the last-modified time to be at or after the creation \
                 time, got {modified_time} and {creation_time}"
            );
        }
    }
}

/// The `CurrencyCode` a read reports is the one the item's PARTITION bills in:
/// `CurrencyCode` has exactly two values and `CustomLineItemArn` admits exactly two
/// partitions — China (`arn:aws-cn`), which bills in CNY, and the one every other
/// resource of this emulator lives in, which bills in the USD the modelled charge
/// values are documented in.
#[tokio::test]
async fn the_reported_currency_is_the_one_the_items_partition_bills_in() {
    // (case, the partition segment of the item's ARN, the expected CurrencyCode)
    let cases: [(&str, &str, &str); 2] = [
        ("the aws partition", "aws", "USD"),
        ("the China partition", "aws-cn", "CNY"),
    ];

    for (case, partition, expected) in cases {
        let emulator = Emulator::new();
        let group = emulator.billing_group("home", &[]).await;
        let minted = emulator.custom_line_item(&group, "subject", json!({})).await;
        let arn = if partition == "aws" {
            minted
        } else {
            emulator.rehome(&minted, partition)
        };

        let (status, body) = emulator.list(json!({ "Filters": { "Arns": [arn] } })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let reported = elements(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the stored custom line item, got {body}"
        );
        assert_eq!(
            reported[0]["CurrencyCode"],
            json!(expected),
            "case {case:?}: expected the currency of that partition, got {}",
            reported[0]
        );
    }
}

/// An item is reported for a billing period its own half-open range covers: the
/// inclusive start is in it, the exclusive end is not, an absent bound is open, and an
/// item created with no range at all is reported for every period — including the
/// current one, which is what a request naming no `BillingPeriod` asks for.
#[tokio::test]
async fn the_requested_billing_period_selects_the_items_effective_in_it() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("home", &[]).await;
    // (name, the BillingPeriodRange the create sets)
    let items: [(&str, Value); 5] = [
        ("always", Value::Null),
        (
            "firstquarter",
            json!({ "InclusiveStartBillingPeriod": "2026-1", "ExclusiveEndBillingPeriod": "2026-4" }),
        ),
        (
            "fromjuly",
            json!({ "InclusiveStartBillingPeriod": "2026-7" }),
        ),
        (
            "longover",
            json!({ "InclusiveStartBillingPeriod": "1970-1", "ExclusiveEndBillingPeriod": "1971-1" }),
        ),
        (
            "faraway",
            json!({ "InclusiveStartBillingPeriod": "2999-1" }),
        ),
    ];
    for (name, range) in &items {
        let request = if range.is_null() {
            json!({})
        } else {
            json!({ "BillingPeriodRange": range })
        };
        emulator.custom_line_item(&group, name, request).await;
    }

    // (the BillingPeriod the read asks for, the names expected in response order)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("2025-12", vec!["always"]),
        ("2026-01", vec!["always", "firstquarter"]),
        // The same period, spelled with a one-digit month: both forms the
        // `BillingPeriod` pattern admits name one month.
        ("2026-1", vec!["always", "firstquarter"]),
        ("2026-03", vec!["always", "firstquarter"]),
        // The end is EXCLUSIVE, so the item is gone the month it names.
        ("2026-04", vec!["always"]),
        ("2026-06", vec!["always"]),
        ("2026-07", vec!["always", "fromjuly"]),
        ("2030-11", vec!["always", "fromjuly"]),
        ("1970-01", vec!["always", "longover"]),
        ("1970-12", vec!["always", "longover"]),
        ("1971-01", vec!["always"]),
        ("2999-12", vec!["always", "fromjuly", "faraway"]),
    ];
    for (billing_period, expected) in cases {
        let listed = emulator
            .listed_names(billing_period, json!({ "BillingPeriod": billing_period }))
            .await;
        assert_eq!(
            listed, expected,
            "case billing period {billing_period:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // No `BillingPeriod`: the CURRENT billing period, which is neither inside the range
    // that ended in 1971 nor at or after the one that opens in 2999, and is covered by
    // the item stored with no range.
    let listed = emulator.listed_names("no billing period", json!({})).await;
    assert!(
        listed.contains(&"always".to_string()),
        "case \"no billing period\": expected the item effective in every period, got {listed:?}"
    );
    for over in ["longover", "faraway"] {
        assert!(
            !listed.contains(&over.to_string()),
            "case \"no billing period\": expected {over:?} to be outside the current billing \
             period, got {listed:?}"
        );
    }
}

/// A page holds at most the `MaxResults` the caller asked for (the service's own default
/// when it asks for none), and the `NextToken` resumes exactly where the page stopped:
/// walking the pages yields every matching item once, in one stable order, and the last
/// page carries no token.
#[tokio::test]
async fn a_page_is_bounded_by_max_results_and_resumed_by_the_next_token() {
    let emulator = Emulator::new();
    let group = emulator.billing_group("home", &[]).await;
    let stored: Vec<String> = (0..7).map(|index| format!("item{index}")).collect();
    for name in &stored {
        emulator.custom_line_item(&group, name, json!({})).await;
    }

    // (case, the MaxResults the request names, the names expected over all the pages)
    let cases: Vec<(&str, Option<usize>, Vec<String>)> = vec![
        ("one at a time", Some(1), stored.clone()),
        ("two at a time", Some(2), stored.clone()),
        ("three at a time", Some(3), stored.clone()),
        ("exactly the stored count", Some(7), stored.clone()),
        ("more than there are", Some(100), stored.clone()),
        ("no MaxResults at all", None, stored.clone()),
        // A filter is applied to the whole sequence, not to one page: these three
        // items span the stored order and still come back in it.
        (
            "two at a time under a filter",
            Some(2),
            vec!["item1".to_string(), "item4".to_string(), "item6".to_string()],
        ),
    ];

    for (case, max_results, expected) in cases {
        let filtered = case.contains("filter");
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let page_limit = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
        // One request per expected element plus one is more than enough: a page that
        // reported nothing and still handed back a token would hang this loop, so the
        // bound is the assertion against it.
        for _ in 0..expected.len() + 1 {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["MaxResults"] = json!(max);
            }
            if filtered {
                request["Filters"] = json!({ "Names": ["item4", "item1", "item6"] });
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.list(request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body}"
            );
            let page = names(&body);
            assert!(
                page.len() <= page_limit,
                "case {case:?}: expected at most {page_limit} items in a page, got {page:?}"
            );
            collected.extend(page);
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert!(
            next_token.is_none(),
            "case {case:?}: expected the last page to carry no NextToken, got {next_token:?}"
        );
        assert_eq!(
            collected, expected,
            "case {case:?}: expected the pages to yield {expected:?}, got {collected:?}"
        );
    }
}
