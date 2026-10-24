// SPDX-License-Identifier: CC0-1.0

//! The ONE rate member a pricing rule holds, asserted on both paths that set it
//! (`CreatePricingRule`, `UpdatePricingRule`) over the service's real `restJson1`
//! wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a pricing rule prices EITHER by a percentage
//! (`Type` = `MARKUP`/`DISCOUNT`, through `ModifierPercentage`) or by tiers
//! (`Type` = `TIERING`, through `Tiering`), never both — so a request that names
//! the rate member its resulting `Type` does not price by is refused 400
//! `ValidationException` under the reason `ValidationExceptionReason` enumerates
//! for that member (`ILLEGAL_MODIFIER_PERCENTAGE`, `ILLEGAL_TIERING_INPUT`),
//! stores nothing, and is refused identically whether the rule is being created
//! or updated.
//!
//! Each test asserts the rule over a SET of inputs: the rows differ along which
//! member is the illegal one, the `Type` and the `Scope` the rule carries, the
//! magnitude of the modifier (including the lower modelled bound), and the shape
//! of the tiering configuration (free tier only, custom tiers only, both, one tier
//! or two). Every acceptance is verified by reading the rule back out of state
//! through `ListPricingRules`, so nothing passes on an echoed request.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A live emulator holding only this service, with the emulated account designated
/// the organization's payer account — the world in which this control plane is
/// reachable at all (the denial every other caller gets is asserted in
/// `tests/all/payer_account_gate.rs`).
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
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

    /// Send one request over the wire and return the status plus the parsed body.
    async fn call(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let builder = http::Request::builder()
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
            );
        let payload = Bytes::from(serde_json::to_vec(body).expect("serialize the body"));
        let response = self
            .router
            .dispatch(builder.body(payload).expect("build the request"))
            .await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `CreatePricingRule` (`POST /create-pricing-rule`).
    async fn create(&self, body: &Value) -> (u16, Value) {
        self.call("POST", "/create-pricing-rule", body).await
    }

    /// `CreatePricingRule` for a request that must be served: the minted ARN.
    async fn create_served(&self, case: &str, body: &Value) -> String {
        let (status, parsed) = self.create(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreatePricingRule to be served, got {status} with {parsed}"
        );
        parsed["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreatePricingRule minted no Arn: {parsed}"))
            .to_string()
    }

    /// `UpdatePricingRule` (`PUT /update-pricing-rule`) against `arn`, carrying the
    /// members of `patch` beside it.
    async fn update(&self, arn: &str, patch: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.call("PUT", "/update-pricing-rule", &body).await
    }

    /// Every stored rule, read through `ListPricingRules` — an independent read.
    async fn list(&self, body: &Value) -> Vec<Value> {
        let (status, parsed) = self.call("POST", "/list-pricing-rules", body).await;
        assert_eq!(
            status, 200,
            "ListPricingRules {body}: expected 200, got {status} with {parsed}"
        );
        parsed["PricingRules"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The persisted state of one rule.
    async fn stored(&self, arn: &str) -> Value {
        let listed = self.list(&json!({ "Filters": { "Arns": [arn] } })).await;
        assert_eq!(
            listed.len(),
            1,
            "expected exactly the rule {arn:?} to be reported, got {listed:?}"
        );
        listed.into_iter().next().unwrap_or(Value::Null)
    }
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

/// A request whose `Type` and rate members contradict each other, with the refusal
/// the model enumerates for the member at fault.
struct IllegalRate {
    case: &'static str,
    /// The members that decide the rate, WITHOUT a `Name` — so the same shape can
    /// be posted as a create and sent as an update patch.
    rate: Value,
    /// The members a create needs beside the rate for its `Scope`.
    scope: Value,
    reason: &'static str,
    field: &'static str,
}

/// The table: a rule naming the rate member its `Type` does not price by. The rows
/// differ along which member is illegal, the `Type` and `Scope` carried, the
/// magnitude of the modifier and the shape of the tiering configuration.
fn illegal_rates() -> Vec<IllegalRate> {
    vec![
        IllegalRate {
            case: "a TIERING rule naming a modifier percentage beside a free tier",
            rate: json!({"Type": "TIERING", "Tiering": {"FreeTier": {"Activated": true}},
                         "ModifierPercentage": 10.0}),
            scope: json!({"Scope": "GLOBAL"}),
            reason: "ILLEGAL_MODIFIER_PERCENTAGE",
            field: "ModifierPercentage",
        },
        IllegalRate {
            case: "a TIERING rule naming the lower-bound modifier beside custom tiers",
            rate: json!({"Type": "TIERING",
                         "Tiering": {"CustomTiers": [
                             {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 50.0,
                              "RateValue": 0.75},
                             {"BeginRangeInclusive": 50.0, "RateValue": 0.5}]},
                         "ModifierPercentage": 0.0}),
            scope: json!({"Scope": "SERVICE", "Service": "AmazonEC2"}),
            reason: "ILLEGAL_MODIFIER_PERCENTAGE",
            field: "ModifierPercentage",
        },
        IllegalRate {
            case: "a TIERING rule naming a large modifier beside a full tiering block",
            rate: json!({"Type": "TIERING",
                         "Tiering": {"FreeTier": {"Activated": false},
                                     "CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                      "RateValue": 1.0}]},
                         "ModifierPercentage": 99.99}),
            scope: json!({"Scope": "BILLING_ENTITY", "BillingEntity": "AWS Marketplace"}),
            reason: "ILLEGAL_MODIFIER_PERCENTAGE",
            field: "ModifierPercentage",
        },
        IllegalRate {
            case: "a MARKUP rule naming a free-tier tiering configuration",
            rate: json!({"Type": "MARKUP", "ModifierPercentage": 10.0,
                         "Tiering": {"FreeTier": {"Activated": true}}}),
            scope: json!({"Scope": "GLOBAL"}),
            reason: "ILLEGAL_TIERING_INPUT",
            field: "Tiering",
        },
        IllegalRate {
            case: "a DISCOUNT rule naming custom tiers",
            rate: json!({"Type": "DISCOUNT", "ModifierPercentage": 2.5,
                         "Tiering": {"CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                      "RateValue": 0.1}]}}),
            scope: json!({"Scope": "GLOBAL"}),
            reason: "ILLEGAL_TIERING_INPUT",
            field: "Tiering",
        },
        IllegalRate {
            case: "a SKU-scoped DISCOUNT naming a whole tiering configuration",
            rate: json!({"Type": "DISCOUNT", "ModifierPercentage": 1.25,
                         "Tiering": {"FreeTier": {"Activated": true},
                                     "CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                      "EndRangeExclusive": 10.0,
                                                      "RateValue": 0.2}]}}),
            scope: json!({"Scope": "SKU", "Service": "AmazonEC2",
                          "UsageType": "USW2-BoxUsage:m2.2xlarge",
                          "Operation": "RunInstances:0202"}),
            reason: "ILLEGAL_TIERING_INPUT",
            field: "Tiering",
        },
        IllegalRate {
            case: "a SERVICE-scoped MARKUP naming a deactivated free tier",
            rate: json!({"Type": "MARKUP", "ModifierPercentage": 5.0,
                         "Tiering": {"FreeTier": {"Activated": false}}}),
            scope: json!({"Scope": "SERVICE", "Service": "AmazonRDS"}),
            reason: "ILLEGAL_TIERING_INPUT",
            field: "Tiering",
        },
    ]
}

/// The whole create body of one row, named so each row mints a distinct name.
fn create_body(row: &IllegalRate, name: &str) -> Value {
    let mut body = json!({ "Name": name });
    for part in [&row.scope, &row.rate] {
        for (member, value) in part.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
    }
    body
}

/// Assert one refusal: the modelled 400 `ValidationException`, the enumerated
/// reason, the member at fault in `Fields`, and a non-empty `@required` message.
fn assert_refusal(case: &str, path: &str, status: u16, body: &Value, reason: &str, field: &str) {
    assert_eq!(
        status, 400,
        "case {case:?} on {path}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} on {path}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(reason),
        "case {case:?} on {path}: expected the enumerated reason {reason:?}, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(field),
        "case {case:?} on {path}: expected the refusal to name the member {field:?}, got {body}"
    );
    assert!(
        body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?} on {path}: the model marks Message @required, got {body}"
    );
}

/// RULE: `CreatePricingRule` refuses a rule that names the rate member its `Type`
/// does not price by — `ModifierPercentage` on a `TIERING` rule, `Tiering` on a
/// `MARKUP`/`DISCOUNT` rule — with the modelled 400 `ValidationException` under the
/// enumerated reason for that member, and mints nothing.
#[tokio::test]
async fn a_create_naming_the_rate_member_its_type_does_not_price_by_is_refused() {
    let em = Emulator::new();
    for (index, row) in illegal_rates().iter().enumerate() {
        let body = create_body(row, &format!("BothRates{index}"));
        let (status, answered) = em.create(&body).await;
        assert_refusal(
            row.case,
            "CreatePricingRule",
            status,
            &answered,
            row.reason,
            row.field,
        );
        assert!(
            answered["Arn"].as_str().is_none(),
            "case {:?}: a refused create mints no ARN, got {answered}",
            row.case
        );
    }

    // A refused create stores nothing: the whole table left the store empty.
    let listed = em.list(&json!({})).await;
    assert!(
        listed.is_empty(),
        "expected a refused create to store nothing, got {listed:?}"
    );
}

/// RULE: the create path and the update path decide one rule's rate the same way,
/// so the combination one refuses the other refuses with the same status, reason
/// and member at fault — and the rule the update addressed is left exactly as it
/// was stored.
#[tokio::test]
async fn create_and_update_refuse_the_same_illegal_rate_combination_alike() {
    for (index, row) in illegal_rates().iter().enumerate() {
        let em = Emulator::new();

        // The rule the update addresses: the legal form of this row's rate, i.e.
        // the same members minus the one its `Type` does not price by.
        let mut legal = create_body(row, &format!("LegalRate{index}"));
        if let Some(members) = legal.as_object_mut() {
            members.remove(row.field);
        }
        let arn = em.create_served(row.case, &legal).await;
        let before = em.stored(&arn).await;

        let (create_status, created) = em.create(&create_body(row, &format!("Both{index}"))).await;
        assert_refusal(
            row.case,
            "CreatePricingRule",
            create_status,
            &created,
            row.reason,
            row.field,
        );

        let (update_status, updated) = em.update(&arn, &row.rate).await;
        assert_refusal(
            row.case,
            "UpdatePricingRule",
            update_status,
            &updated,
            row.reason,
            row.field,
        );

        assert_eq!(
            create_status, update_status,
            "case {:?}: expected one status for one combination, got {create_status} on create \
             and {update_status} on update",
            row.case
        );
        assert_eq!(
            (created["Reason"].as_str(), created["Fields"][0]["Name"].as_str()),
            (updated["Reason"].as_str(), updated["Fields"][0]["Name"].as_str()),
            "case {:?}: expected one reason and one member at fault for one combination, got \
             {created} on create and {updated} on update",
            row.case
        );

        let after = em.stored(&arn).await;
        assert_eq!(
            after, before,
            "case {:?}: expected a refused update to change nothing, got {after} where {before} \
             was stored",
            row.case
        );
    }
}

/// RULE: the single-rate form of each `Type` is served and the stored rule holds
/// EXACTLY ONE rate member — the one its `Type` prices by — as `ListPricingRules`
/// reports it.
///
/// The rows span every `PricingRuleType` the model enumerates
/// (`smithy_query(query="shape",shape="PricingRuleType")`: DISCOUNT | MARKUP |
/// TIERING) and differ along the `Scope`, the modifier's magnitude and the shape of
/// the tiering configuration.
#[tokio::test]
async fn an_accepted_rule_holds_exactly_the_rate_member_its_type_prices_by() {
    // (case, body, the rate member the stored rule must hold)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "GLOBAL MARKUP at the lower modelled ModifierPercentage bound",
            json!({"Name": "OnlyMarkupZero", "Scope": "GLOBAL", "Type": "MARKUP",
                   "ModifierPercentage": 0.0}),
            "ModifierPercentage",
        ),
        (
            "SERVICE DISCOUNT with a fractional modifier",
            json!({"Name": "OnlyDiscount", "Scope": "SERVICE", "Type": "DISCOUNT",
                   "Service": "AmazonEC2", "ModifierPercentage": 12.75}),
            "ModifierPercentage",
        ),
        (
            "GLOBAL TIERING with a free tier only",
            json!({"Name": "OnlyFreeTier", "Scope": "GLOBAL", "Type": "TIERING",
                   "Tiering": {"FreeTier": {"Activated": true}}}),
            "Tiering",
        ),
        (
            "BILLING_ENTITY TIERING with two custom tiers",
            json!({"Name": "OnlyCustomTiers", "Scope": "BILLING_ENTITY",
                   "BillingEntity": "AWS Marketplace", "Type": "TIERING",
                   "Tiering": {"CustomTiers": [
                       {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0,
                        "RateValue": 0.5},
                       {"BeginRangeInclusive": 100.0, "RateValue": 0.25}]}}),
            "Tiering",
        ),
    ];

    let em = Emulator::new();
    for (case, body, held) in &rows {
        let arn = em.create_served(case, body).await;
        let stored = em.stored(&arn).await;
        let absent = if *held == "Tiering" {
            "ModifierPercentage"
        } else {
            "Tiering"
        };
        assert!(
            !stored[held].is_null(),
            "case {case:?}: expected the stored rule to hold its {held}, got {stored}"
        );
        assert_eq!(
            stored[held], body[held],
            "case {case:?}: expected the stored {held} to be the one the request sent, got {stored}"
        );
        assert!(
            stored[absent].is_null(),
            "case {case:?}: expected the stored rule to hold no {absent}, got {stored}"
        );
    }
}
