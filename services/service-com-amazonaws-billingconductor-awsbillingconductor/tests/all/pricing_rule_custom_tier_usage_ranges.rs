// SPDX-License-Identifier: CC0-1.0

//! The usage a pricing rule's custom tiers price, asserted on both paths that set
//! them (`CreatePricingRule`, `UpdatePricingRule`) over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`].
//!
//! RULE under test: a `CustomTier` prices the HALF-OPEN usage range
//! `[BeginRangeInclusive, EndRangeExclusive)` — an absent end leaving the tier open
//! (`smithy_query(query="shape",shape="CustomTier")`) — so a tier whose exclusive
//! end is at or below its inclusive begin prices no usage at all, and two tiers of
//! one configuration may not both cover a unit of usage, which would leave its rate
//! undecided. Either way the request is refused 400 `ValidationException` under the
//! reason `ValidationExceptionReason` enumerates for an illegal tiering input
//! (`ILLEGAL_TIERING_INPUT`), stores nothing, and is refused identically whether the
//! rule is being created or updated.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over: how the range fails (end equal to the begin, end below it,
//! fractional bounds, an open tier), how two tiers meet (identical, partially
//! overlapping, one nested in the other, an open tier reached by a later one), WHERE
//! in the list the offending tier sits, how many tiers the configuration holds, and
//! the `Scope` the rule carries. The rows on the other side of each bound — tiers
//! that touch at a shared bound, a minimal non-empty tier, a list in descending
//! order — are served and read back out of state through `ListPricingRules`, so
//! nothing passes on an echoed request.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The status the modelled `ValidationException` is served on
/// (`smithy_query(query="shape",shape="ValidationException")`: `httpError` 400).
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` value an illegal tiering input earns
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const ILLEGAL_TIERING_INPUT: &str = "ILLEGAL_TIERING_INPUT";

/// The declared member path a refusal about the tiers names: the `CustomTiers`
/// member of the request's `Tiering`
/// (`smithy_query(query="shape",shape="CreateTieringInput")`).
const CUSTOM_TIERS_MEMBER: &str = "Tiering.CustomTiers";

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
    /// tiering configuration `tiering` beside it.
    async fn update_tiering(&self, arn: &str, tiering: &Value) -> (u16, Value) {
        self.call(
            "PUT",
            "/update-pricing-rule",
            &json!({ "Arn": arn, "Type": "TIERING", "Tiering": tiering }),
        )
        .await
    }

    /// Every stored rule a filter selects, read through `ListPricingRules` — an
    /// independent read.
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

/// One row: a tiering configuration and the `Scope` members a create needs beside it.
struct Tiers {
    case: &'static str,
    /// The whole `Tiering` member, as a request carries it.
    tiering: Value,
    /// The `Scope` and the members that scope keys on.
    scope: Value,
}

/// The table of configurations that price usage no rule could apply: a tier covering
/// nothing, or two tiers covering common usage. The rows differ along how the range
/// fails, how the pair meets, where the offending tier sits in the list, how many
/// tiers there are and the `Scope` the rule carries.
fn unpriceable_tiers() -> Vec<Tiers> {
    vec![
        Tiers {
            case: "one tier whose exclusive end equals its inclusive begin",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 100.0, "EndRangeExclusive": 100.0, "RateValue": 1.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "one tier whose exclusive end is below its inclusive begin",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 100.0, "EndRangeExclusive": 50.0, "RateValue": 1.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "one tier whose fractional end is just below its begin",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.5, "EndRangeExclusive": 0.25, "RateValue": 0.1}]}),
            scope: json!({"Scope": "SERVICE", "Service": "AmazonEC2"}),
        },
        Tiers {
            case: "a zero-width tier at the lower modelled bound of the range",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 0.0, "RateValue": 2.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "a well-ordered tier followed by an inverted one, beside a free tier",
            tiering: json!({"FreeTier": {"Activated": true},
                            "CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 10.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 20.0, "EndRangeExclusive": 15.0, "RateValue": 0.5}]}),
            scope: json!({"Scope": "BILLING_ENTITY", "BillingEntity": "AWS Marketplace"}),
        },
        Tiers {
            case: "two tiers over the identical usage range",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 9.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "two tiers that partially overlap",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 50.0, "EndRangeExclusive": 150.0, "RateValue": 0.5}]}),
            scope: json!({"Scope": "SERVICE", "Service": "AmazonRDS"}),
        },
        Tiers {
            case: "a tier nested inside another",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 10.0, "EndRangeExclusive": 20.0, "RateValue": 3.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "a later tier reaching into an open-ended one",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 100.0, "EndRangeExclusive": 200.0, "RateValue": 0.5}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "two open-ended tiers",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 50.0, "RateValue": 0.5}]}),
            scope: json!({"Scope": "SKU", "Service": "AmazonEC2",
                          "UsageType": "USW2-BoxUsage:m2.2xlarge",
                          "Operation": "RunInstances:0202"}),
        },
        Tiers {
            case: "the third of three tiers overlapping the first",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 10.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 10.0, "EndRangeExclusive": 20.0, "RateValue": 0.9},
                {"BeginRangeInclusive": 5.0, "EndRangeExclusive": 8.0, "RateValue": 0.8}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
    ]
}

/// The table of configurations on the OTHER side of each bound: every unit of usage
/// is priced by at most one tier and every tier prices some usage. The rows differ
/// along how narrow a tier is, whether tiers touch at a shared bound, whether one is
/// open-ended, the order the list carries them in and how many there are.
fn priceable_tiers() -> Vec<Tiers> {
    vec![
        Tiers {
            case: "two tiers touching at a shared bound",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 100.0, "EndRangeExclusive": 200.0, "RateValue": 0.5}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "a bounded tier followed by an open-ended one beginning at its end",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.0},
                {"BeginRangeInclusive": 100.0, "RateValue": 0.25}]}),
            scope: json!({"Scope": "SERVICE", "Service": "AmazonEC2"}),
        },
        Tiers {
            case: "a single open-ended tier at the lower modelled bound",
            tiering: json!({"CustomTiers": [{"BeginRangeInclusive": 0.0, "RateValue": 1.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "a minimal non-empty tier",
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 0.000001,
                 "RateValue": 0.0}]}),
            scope: json!({"Scope": "GLOBAL"}),
        },
        Tiers {
            case: "disjoint tiers listed in descending order, beside a free tier",
            tiering: json!({"FreeTier": {"Activated": false},
                            "CustomTiers": [
                {"BeginRangeInclusive": 200.0, "EndRangeExclusive": 300.0, "RateValue": 0.1},
                {"BeginRangeInclusive": 100.0, "EndRangeExclusive": 150.0, "RateValue": 0.2},
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 50.0, "RateValue": 0.3}]}),
            scope: json!({"Scope": "BILLING_ENTITY", "BillingEntity": "AWS Marketplace"}),
        },
        Tiers {
            case: "ten disjoint tiers, the modelled maximum of the list",
            tiering: json!({"CustomTiers": (0..10)
                .map(|index| json!({
                    "BeginRangeInclusive": f64::from(index) * 10.0,
                    "EndRangeExclusive": f64::from(index) * 10.0 + 10.0,
                    "RateValue": f64::from(index),
                }))
                .collect::<Vec<Value>>()}),
            scope: json!({"Scope": "GLOBAL"}),
        },
    ]
}

/// The whole create body of one row, named so each row mints a distinct name.
fn create_body(row: &Tiers, name: &str) -> Value {
    let mut body = json!({ "Name": name, "Type": "TIERING", "Tiering": row.tiering.clone() });
    for (member, value) in row.scope.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

/// Assert one refusal: the modelled 400 `ValidationException`, the enumerated reason
/// for an illegal tiering input, the tiers named as the member at fault, and a
/// non-empty `@required` message.
fn assert_refusal(case: &str, path: &str, status: u16, body: &Value) {
    assert_eq!(
        status, VALIDATION_STATUS,
        "case {case:?} on {path}: expected HTTP {VALIDATION_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} on {path}: expected the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(ILLEGAL_TIERING_INPUT),
        "case {case:?} on {path}: expected the enumerated reason {ILLEGAL_TIERING_INPUT:?}, \
         got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(CUSTOM_TIERS_MEMBER),
        "case {case:?} on {path}: expected the refusal to name {CUSTOM_TIERS_MEMBER:?}, got {body}"
    );
    assert!(
        body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?} on {path}: the model marks Message @required, got {body}"
    );
}

/// RULE: `CreatePricingRule` refuses a tiering configuration holding a tier that
/// prices no usage, or two tiers pricing common usage, with the modelled 400
/// `ValidationException` under the enumerated reason, and mints nothing.
#[tokio::test]
async fn a_create_whose_custom_tiers_price_no_usage_or_the_same_usage_is_refused() {
    let em = Emulator::new();
    for (index, row) in unpriceable_tiers().iter().enumerate() {
        let body = create_body(row, &format!("BadTiers{index}"));
        let (status, answered) = em.create(&body).await;
        assert_refusal(row.case, "CreatePricingRule", status, &answered);
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

/// RULE, the other side of each bound: a configuration whose tiers each price some
/// usage and no two the same usage is served, and the tiers read back out of state
/// exactly as the request listed them.
#[tokio::test]
async fn a_create_whose_custom_tiers_price_distinct_usage_is_served_and_round_trips() {
    let em = Emulator::new();
    for (index, row) in priceable_tiers().iter().enumerate() {
        let body = create_body(row, &format!("GoodTiers{index}"));
        let arn = em.create_served(row.case, &body).await;
        let stored = em.stored(&arn).await;
        assert_eq!(
            stored["Tiering"]["CustomTiers"], row.tiering["CustomTiers"],
            "case {:?}: expected the stored tiers to be the ones the request sent, got {stored}",
            row.case
        );
    }
}

/// RULE, where the model itself bounds the input: `CustomTiersList` is declared
/// `@length(min: 1, max: 10)` (`smithy_query(query="shape",shape="CreateTieringInput")`),
/// so a list past that maximum — and an empty one — is refused with the modelled 400
/// `ValidationException` and stores nothing, which is also what keeps the ordering
/// rule above deciding a list whose size the model has already bounded.
#[tokio::test]
async fn the_modelled_length_of_the_custom_tier_list_bounds_what_a_request_carries() {
    let em = Emulator::new();

    // One tier past the modelled maximum, each pricing a disjoint usage range — so
    // the ONLY rule broken is the list's own length.
    let past_the_maximum: Vec<Value> = (0..11)
        .map(|index| {
            json!({
                "BeginRangeInclusive": f64::from(index) * 10.0,
                "EndRangeExclusive": f64::from(index) * 10.0 + 10.0,
                "RateValue": f64::from(index),
            })
        })
        .collect();

    // (case, the tier list the request carries)
    let rows: Vec<(&str, Value)> = vec![
        (
            "one tier past the modelled maximum of ten",
            Value::Array(past_the_maximum),
        ),
        ("an empty tier list", json!([])),
    ];

    for (index, (case, tiers)) in rows.iter().enumerate() {
        let body = json!({
            "Name": format!("TierListLength{index}"),
            "Scope": "GLOBAL",
            "Type": "TIERING",
            "Tiering": { "CustomTiers": tiers },
        });
        let (status, answered) = em.create(&body).await;
        assert_eq!(
            status, VALIDATION_STATUS,
            "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {answered}"
        );
        assert!(
            answered["Arn"].as_str().is_none(),
            "case {case:?}: a refused create mints no ARN, got {answered}"
        );
    }

    let listed = em.list(&json!({})).await;
    assert!(
        listed.is_empty(),
        "expected a refused create to store nothing, got {listed:?}"
    );
}

/// RULE: the create path and the update path judge one rule's tiers the same way, so
/// a configuration one refuses the other refuses with the same status, reason and
/// member at fault — and the rule the update addressed keeps the tiers it was stored
/// with.
#[tokio::test]
async fn create_and_update_refuse_the_same_unpriceable_tiers_alike() {
    for (index, row) in unpriceable_tiers().iter().enumerate() {
        let em = Emulator::new();

        // The rule the update addresses: a well-ordered tiering rule of this row's
        // scope, so only the tiers differ between the stored rule and the request.
        let legal = Tiers {
            case: row.case,
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 1.0, "RateValue": 1.0}]}),
            scope: row.scope.clone(),
        };
        let arn = em
            .create_served(row.case, &create_body(&legal, &format!("StoredTiers{index}")))
            .await;
        let before = em.stored(&arn).await;

        let (create_status, created) = em
            .create(&create_body(row, &format!("RefusedTiers{index}")))
            .await;
        assert_refusal(row.case, "CreatePricingRule", create_status, &created);

        let (update_status, updated) = em.update_tiering(&arn, &row.tiering).await;
        assert_refusal(row.case, "UpdatePricingRule", update_status, &updated);

        assert_eq!(
            (
                created["Reason"].as_str(),
                created["Fields"][0]["Name"].as_str()
            ),
            (
                updated["Reason"].as_str(),
                updated["Fields"][0]["Name"].as_str()
            ),
            "case {:?}: expected one reason and one member at fault for one configuration, got \
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

/// RULE, where it bites on the update path: an update that REPLACES the stored tiers
/// with well-ordered, disjoint ones is served and the rule reads back with exactly
/// the new tiers — so the refusal above is about the tiers the request carries, not a
/// blanket refusal of the member.
#[tokio::test]
async fn an_update_replacing_the_tiers_with_priceable_ones_is_served() {
    let em = Emulator::new();
    for (index, row) in priceable_tiers().iter().enumerate() {
        let seed = Tiers {
            case: row.case,
            tiering: json!({"CustomTiers": [
                {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 2.0, "RateValue": 4.0}]}),
            scope: row.scope.clone(),
        };
        let arn = em
            .create_served(row.case, &create_body(&seed, &format!("ReplacedTiers{index}")))
            .await;

        let (status, answered) = em.update_tiering(&arn, &row.tiering).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected UpdatePricingRule to be served, got {status} with {answered}",
            row.case
        );
        let stored = em.stored(&arn).await;
        assert_eq!(
            stored["Tiering"]["CustomTiers"], row.tiering["CustomTiers"],
            "case {:?}: expected the update to replace the tiers, got {stored}",
            row.case
        );
    }
}
