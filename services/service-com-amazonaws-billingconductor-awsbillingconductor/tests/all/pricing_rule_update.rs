// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdatePricingRule` (and the `CreatePricingRule` that gives
//! it something to update), driven over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE under test: `UpdatePricingRule` resolves the rule its `Arn` names (the
//! modelled `PricingRuleArn` pattern spells that either as the full ARN or as the
//! bare 10-character id, and both name one record), replaces exactly the members
//! the request carries, keeps every member it omits at the value state already
//! holds, and answers with the rule's whole state afterwards — the members no
//! update can change (`Scope`, `Service`, `BillingEntity`, `UsageType`,
//! `Operation`) included, plus `AssociatedPricingPlanCount` derived from the
//! pricing plans that name the rule. `ModifierPercentage` is stored "rounded to the
//! nearest 2 decimal places" (the member's own documentation). A rule prices either
//! by a percentage (`MARKUP`/`DISCOUNT`) or by tiers (`TIERING`), never both, so a
//! request that names the member its resulting `Type` does not price by, or leaves
//! the one it needs absent, is refused 400 `ValidationException`; an `Arn` no rule
//! answers to is 404 `ResourceNotFoundException` and a `Name` another rule holds is
//! 409 `ConflictException`.
//!
//! Every test asserts that rule over a SET of inputs: the rows differ along which
//! optional members the request names (none, one, several), which member class they
//! fall in (text, number, enum, nested structure, list), how the `Arn` is spelled,
//! how much precision the modifier carries, and how many pricing plans name the
//! rule. Each row's result is read back out of state through a second call, so an
//! assertion passes only if the change was persisted rather than echoed.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

    /// `CreatePricingRule` (`POST /create-pricing-rule`): the created rule's full ARN.
    async fn create_rule(&self, body: &Value) -> String {
        let (status, parsed) = self.call("POST", "/create-pricing-rule", body).await;
        assert_eq!(
            status, 200,
            "CreatePricingRule {body}: expected 200, got {status} with {parsed:?}"
        );
        let arn = parsed
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_pricing_rule_arn(&arn),
            "CreatePricingRule {body}: expected a full pricingrule ARN, got {arn:?}"
        );
        arn
    }

    /// `UpdatePricingRule` (`PUT /update-pricing-rule`) against `arn`, carrying the
    /// members of `patch` beside it.
    async fn update_rule(&self, arn: &str, patch: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.call("PUT", "/update-pricing-rule", &body).await
    }

    /// The rule's persisted state, read through `ListPricingRules` — an
    /// INDEPENDENT read that changes nothing, so a verification cannot be
    /// satisfied by the handler under test echoing its own request.
    async fn read_stored(&self, arn: &str) -> Value {
        let body = json!({ "Filters": { "Arns": [arn] } });
        let (status, parsed) = self.call("POST", "/list-pricing-rules", &body).await;
        assert_eq!(
            status, 200,
            "ListPricingRules for {arn}: expected 200, got {status} with {parsed:?}"
        );
        let listed = parsed
            .get("PricingRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        listed
            .into_iter()
            .find(|element| element.get("Arn").and_then(Value::as_str) == Some(arn))
            .unwrap_or_else(|| panic!("ListPricingRules reported no rule {arn}"))
    }

    /// `CreatePricingPlan` naming `pricing_rule_arns`: the plan's ARN.
    async fn create_plan(&self, name: &str, pricing_rule_arns: &[&str]) -> String {
        let body = json!({ "Name": name, "PricingRuleArns": pricing_rule_arns });
        let (status, parsed) = self.call("POST", "/create-pricing-plan", &body).await;
        assert_eq!(
            status, 200,
            "CreatePricingPlan {name}: expected 200, got {status} with {parsed:?}"
        );
        parsed
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

/// Does `value` have the full form of the modelled `PricingRuleArn` pattern,
/// `arn:aws:billingconductor::<12 digits>:pricingrule/[a-zA-Z0-9]{10}`
/// (`smithy_query(query="shape",shape="PricingRuleArn")`)?
fn is_pricing_rule_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:aws:billingconductor::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix("pricingrule/") else {
        return false;
    };
    account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && id.len() == 10
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The bare-id form of a full pricing-rule ARN — the other spelling the modelled
/// pattern admits for the same resource.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A member of a response body, as a string (absent reads as `None`).
fn text(body: &Value, member: &str) -> Option<String> {
    body.get(member).and_then(Value::as_str).map(str::to_string)
}

/// A member of a response body, as a number (absent reads as `None`).
fn number(body: &Value, member: &str) -> Option<f64> {
    body.get(member).and_then(Value::as_f64)
}

/// Two modifier percentages are the same value: a `double` travels as JSON, so the
/// comparison is to within a tolerance far below the two decimal places the member
/// is rounded to.
fn same_percentage(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

/// The request body that creates the `MARKUP` rule the tables start from: every
/// member `UpdatePricingRule` can change carries a known starting value, and every
/// member it cannot change is set too, so a row can assert both halves.
fn markup_rule(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the rule as created",
        "Scope": "SERVICE",
        "Service": "AmazonEC2",
        "Type": "MARKUP",
        "ModifierPercentage": 10.0,
        "BillingEntity": "AWS",
        "UsageType": "USW2-BoxUsage:m2.2xlarge",
        "Operation": "RunInstances:0202",
    })
}

/// One row of the member-replacement table.
struct UpdateCase {
    label: &'static str,
    /// The members the `UpdatePricingRule` request names beside `Arn`.
    patch: Value,
    /// `Name`, `Description`, `Type`, `ModifierPercentage` and `Tiering` as the rule
    /// must report them after the call.
    name: &'static str,
    description: Option<&'static str>,
    rule_type: &'static str,
    modifier: Option<f64>,
    tiering: Option<Value>,
}

#[tokio::test]
async fn update_replaces_the_members_it_names_and_keeps_the_rest() {
    let cases = vec![
        UpdateCase {
            label: "no member named: the rule is untouched",
            patch: json!({}),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "MARKUP",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "description only",
            patch: json!({ "Description": "a second description" }),
            name: "base-rule",
            description: Some("a second description"),
            rule_type: "MARKUP",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "an empty description is a value, not an omission",
            patch: json!({ "Description": "" }),
            name: "base-rule",
            description: Some(""),
            rule_type: "MARKUP",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "name only",
            patch: json!({ "Name": "renamed-rule" }),
            name: "renamed-rule",
            description: Some("the rule as created"),
            rule_type: "MARKUP",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "name set to the name it already holds",
            patch: json!({ "Name": "base-rule" }),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "MARKUP",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "modifier percentage only",
            patch: json!({ "ModifierPercentage": 12.5 }),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "MARKUP",
            modifier: Some(12.5),
            tiering: None,
        },
        UpdateCase {
            label: "type only, within the percentage-priced types",
            patch: json!({ "Type": "DISCOUNT" }),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "DISCOUNT",
            modifier: Some(10.0),
            tiering: None,
        },
        UpdateCase {
            label: "three members at once",
            patch: json!({
                "Name": "three-at-once",
                "Description": "changed with the others",
                "ModifierPercentage": 4.0,
            }),
            name: "three-at-once",
            description: Some("changed with the others"),
            rule_type: "MARKUP",
            modifier: Some(4.0),
            tiering: None,
        },
        UpdateCase {
            label: "becomes a TIERING rule with a free tier",
            patch: json!({ "Type": "TIERING", "Tiering": { "FreeTier": { "Activated": true } } }),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "TIERING",
            modifier: None,
            tiering: Some(json!({ "FreeTier": { "Activated": true } })),
        },
        UpdateCase {
            label: "becomes a TIERING rule with custom tiers",
            patch: json!({
                "Type": "TIERING",
                "Tiering": { "CustomTiers": [
                    { "BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.5 },
                    { "BeginRangeInclusive": 100.0, "RateValue": 0.5 },
                ] },
            }),
            name: "base-rule",
            description: Some("the rule as created"),
            rule_type: "TIERING",
            modifier: None,
            tiering: Some(json!({ "CustomTiers": [
                { "BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 1.5 },
                { "BeginRangeInclusive": 100.0, "RateValue": 0.5 },
            ] })),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let em = Emulator::new();
        let arn = em.create_rule(&markup_rule("base-rule")).await;
        let (status, body) = em.update_rule(&arn, &case.patch).await;
        assert_eq!(
            status, 200,
            "case {:?} (row {index}): expected 200, got {status} with {body:?}",
            case.label
        );

        for (label, reported) in [("the response", &body), ("state", &em.read_stored(&arn).await)] {
            assert_eq!(
                text(reported, "Arn").as_deref(),
                Some(arn.as_str()),
                "case {:?} in {label}: expected Arn {arn:?}, got {:?}",
                case.label,
                text(reported, "Arn")
            );
            assert_eq!(
                text(reported, "Name").as_deref(),
                Some(case.name),
                "case {:?} in {label}: expected Name {:?}, got {:?}",
                case.label,
                case.name,
                text(reported, "Name")
            );
            assert_eq!(
                text(reported, "Description").as_deref(),
                case.description,
                "case {:?} in {label}: expected Description {:?}, got {:?}",
                case.label,
                case.description,
                text(reported, "Description")
            );
            assert_eq!(
                text(reported, "Type").as_deref(),
                Some(case.rule_type),
                "case {:?} in {label}: expected Type {:?}, got {:?}",
                case.label,
                case.rule_type,
                text(reported, "Type")
            );
            match (case.modifier, number(reported, "ModifierPercentage")) {
                (Some(expected), Some(actual)) => assert!(
                    same_percentage(expected, actual),
                    "case {:?} in {label}: expected ModifierPercentage {expected}, got {actual}",
                    case.label
                ),
                (expected, actual) => assert!(
                    expected.is_none() && actual.is_none(),
                    "case {:?} in {label}: expected ModifierPercentage {expected:?}, got {actual:?}",
                    case.label
                ),
            }
            assert_eq!(
                reported.get("Tiering"),
                case.tiering.as_ref(),
                "case {:?} in {label}: expected Tiering {:?}, got {:?}",
                case.label,
                case.tiering,
                reported.get("Tiering")
            );

            // The members no update operation can change come back as created.
            for (member, expected) in [
                ("Scope", "SERVICE"),
                ("Service", "AmazonEC2"),
                ("BillingEntity", "AWS"),
                ("UsageType", "USW2-BoxUsage:m2.2xlarge"),
                ("Operation", "RunInstances:0202"),
            ] {
                assert_eq!(
                    text(reported, member).as_deref(),
                    Some(expected),
                    "case {:?} in {label}: expected {member} {expected:?}, got {:?}",
                    case.label,
                    text(reported, member)
                );
            }
            assert_eq!(
                reported.get("AssociatedPricingPlanCount").and_then(Value::as_i64),
                Some(0),
                "case {:?} in {label}: expected AssociatedPricingPlanCount 0, got {:?}",
                case.label,
                reported.get("AssociatedPricingPlanCount")
            );
            let last_modified = reported
                .get("LastModifiedTime")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                last_modified > 0,
                "case {:?} in {label}: expected a LastModifiedTime in epoch millis, got {last_modified}",
                case.label
            );
        }
    }
}

/// One row of the modifier-rounding table.
struct RoundingCase {
    label: &'static str,
    requested: f64,
    stored: f64,
}

#[tokio::test]
async fn modifier_percentage_is_stored_rounded_to_two_decimal_places() {
    // "Your entry will be rounded to the nearest 2 decimal places."
    // (`smithy_query(query="doc",shape="UpdatePricingRuleInput",member="ModifierPercentage")`).
    let cases = [
        RoundingCase {
            label: "already two decimals",
            requested: 12.34,
            stored: 12.34,
        },
        RoundingCase {
            label: "a whole percentage",
            requested: 7.0,
            stored: 7.0,
        },
        RoundingCase {
            label: "zero, the published lower bound of the range",
            requested: 0.0,
            stored: 0.0,
        },
        RoundingCase {
            label: "rounds down",
            requested: 4.7142,
            stored: 4.71,
        },
        RoundingCase {
            label: "rounds up",
            requested: 12.3456,
            stored: 12.35,
        },
        RoundingCase {
            label: "rounds up into the next whole percentage",
            requested: 99.999,
            stored: 100.0,
        },
        RoundingCase {
            label: "rounds a value below one hundredth up to one hundredth",
            requested: 0.006,
            stored: 0.01,
        },
        RoundingCase {
            label: "rounds a value below half a hundredth away",
            requested: 0.004,
            stored: 0.0,
        },
        RoundingCase {
            label: "a percentage above one hundred, which the range does not cap",
            requested: 250.567,
            stored: 250.57,
        },
    ];

    let em = Emulator::new();
    for (index, case) in cases.iter().enumerate() {
        let arn = em.create_rule(&markup_rule(&format!("rounding-{index}"))).await;
        let (status, body) = em
            .update_rule(&arn, &json!({ "ModifierPercentage": case.requested }))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body:?}",
            case.label
        );
        for (label, reported) in [("the response", &body), ("state", &em.read_stored(&arn).await)] {
            let actual = number(reported, "ModifierPercentage").unwrap_or(f64::NAN);
            assert!(
                same_percentage(case.stored, actual),
                "case {:?} in {label}: expected ModifierPercentage {} for a requested {}, got {actual}",
                case.label,
                case.stored,
                case.requested
            );
        }
    }
}

#[tokio::test]
async fn either_spelling_of_the_arn_names_the_same_rule() {
    let em = Emulator::new();
    let arn = em.create_rule(&markup_rule("two-spellings")).await;
    let id = bare_id(&arn);

    // The rows differ in how the SAME rule is addressed; each writes a description
    // the next row has to see.
    let cases = [
        ("the bare 10-character id", id.clone(), "written by the id"),
        ("the full ARN", arn.clone(), "written by the ARN"),
    ];
    for (label, addressed_as, description) in cases {
        let (status, body) = em
            .update_rule(&addressed_as, &json!({ "Description": description }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {body:?}"
        );
        assert_eq!(
            text(&body, "Arn").as_deref(),
            Some(arn.as_str()),
            "case {label:?}: a response always reports the full ARN, got {:?}",
            text(&body, "Arn")
        );
        let stored = em.read_stored(&arn).await;
        assert_eq!(
            text(&stored, "Description").as_deref(),
            Some(description),
            "case {label:?}: expected Description {description:?} in state, got {:?}",
            text(&stored, "Description")
        );
    }
}

#[tokio::test]
async fn tiering_members_are_replaced_one_by_one() {
    let em = Emulator::new();
    let arn = em
        .create_rule(&json!({
            "Name": "tiered-rule",
            "Scope": "GLOBAL",
            "Type": "TIERING",
            "Tiering": {
                "FreeTier": { "Activated": true },
                "CustomTiers": [{ "BeginRangeInclusive": 0.0, "RateValue": 2.0 }],
            },
        }))
        .await;

    // Each row names ONE member of `Tiering`; the other must keep what the previous
    // row left, which is what makes the merge a replacement rather than a reset.
    let cases = [
        (
            "the free tier alone",
            json!({ "Tiering": { "FreeTier": { "Activated": false } } }),
            json!({
                "FreeTier": { "Activated": false },
                "CustomTiers": [{ "BeginRangeInclusive": 0.0, "RateValue": 2.0 }],
            }),
        ),
        (
            "the custom tiers alone, replaced rather than appended",
            json!({ "Tiering": { "CustomTiers": [
                { "BeginRangeInclusive": 0.0, "EndRangeExclusive": 50.0, "RateValue": 1.0 },
                { "BeginRangeInclusive": 50.0, "RateValue": 0.25 },
            ] } }),
            json!({
                "FreeTier": { "Activated": false },
                "CustomTiers": [
                    { "BeginRangeInclusive": 0.0, "EndRangeExclusive": 50.0, "RateValue": 1.0 },
                    { "BeginRangeInclusive": 50.0, "RateValue": 0.25 },
                ],
            }),
        ),
        (
            "both members at once",
            json!({ "Tiering": {
                "FreeTier": { "Activated": true },
                "CustomTiers": [{ "BeginRangeInclusive": 10.0, "RateValue": 3.0 }],
            } }),
            json!({
                "FreeTier": { "Activated": true },
                "CustomTiers": [{ "BeginRangeInclusive": 10.0, "RateValue": 3.0 }],
            }),
        ),
        (
            "no tiering member at all",
            json!({ "Description": "left the tiers alone" }),
            json!({
                "FreeTier": { "Activated": true },
                "CustomTiers": [{ "BeginRangeInclusive": 10.0, "RateValue": 3.0 }],
            }),
        ),
    ];

    for (label, patch, expected) in cases {
        let (status, body) = em.update_rule(&arn, &patch).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {body:?}"
        );
        for (where_, reported) in [("the response", &body), ("state", &em.read_stored(&arn).await)] {
            assert_eq!(
                reported.get("Tiering"),
                Some(&expected),
                "case {label:?} in {where_}: expected Tiering {expected:?}, got {:?}",
                reported.get("Tiering")
            );
        }
    }
}

#[tokio::test]
async fn associated_pricing_plan_count_counts_the_plans_that_name_the_rule() {
    let em = Emulator::new();
    let arn = em.create_rule(&markup_rule("counted-rule")).await;
    let other = em.create_rule(&markup_rule("uncounted-rule")).await;
    let id = bare_id(&arn);

    // The rows differ in how many plans name the rule and how they spell its ARN; a
    // plan that names a DIFFERENT rule must not be counted.
    struct PlanCase<'a> {
        label: &'static str,
        /// The pricing plan this row creates first, if any: its name and the rule
        /// ARNs it names.
        plan: Option<&'static str>,
        names_rules: Vec<&'a str>,
        expected: i64,
    }
    let cases = vec![
        PlanCase {
            label: "no plan names the rule yet",
            plan: None,
            names_rules: vec![],
            expected: 0,
        },
        PlanCase {
            label: "one plan names it by its full ARN",
            plan: Some("plan-by-arn"),
            names_rules: vec![arn.as_str()],
            expected: 1,
        },
        PlanCase {
            label: "a second plan names it by its bare id",
            plan: Some("plan-by-id"),
            names_rules: vec![id.as_str()],
            expected: 2,
        },
        PlanCase {
            label: "a plan that names only another rule is not counted",
            plan: Some("plan-for-other-rule"),
            names_rules: vec![other.as_str()],
            expected: 2,
        },
        PlanCase {
            label: "a plan that names it alongside another rule is counted once",
            plan: Some("plan-with-both"),
            names_rules: vec![arn.as_str(), other.as_str()],
            expected: 3,
        },
    ];

    for case in &cases {
        if let Some(name) = case.plan {
            em.create_plan(name, &case.names_rules).await;
        }
        let (status, body) = em.update_rule(&arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body:?}",
            case.label
        );
        let actual = body
            .get("AssociatedPricingPlanCount")
            .and_then(Value::as_i64);
        assert_eq!(
            actual,
            Some(case.expected),
            "case {:?}: expected AssociatedPricingPlanCount {}, got {actual:?}",
            case.label,
            case.expected
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    label: &'static str,
    /// How the request spells `Arn`: `None` means the rule under test's own ARN.
    arn: Option<&'static str>,
    patch: Value,
    status: u16,
    code: &'static str,
    /// The `Reason` a `ValidationException`/`ConflictException` carries, when the
    /// shape has one.
    reason: Option<&'static str>,
}

#[tokio::test]
async fn a_request_the_rule_cannot_satisfy_is_refused_and_changes_nothing() {
    let cases = [
        RefusalCase {
            label: "a bare id no rule answers to",
            arn: Some("ZZZZZZZZZZ"),
            patch: json!({ "Description": "never stored" }),
            status: 404,
            code: "ResourceNotFoundException",
            reason: None,
        },
        RefusalCase {
            label: "a full ARN no rule answers to",
            arn: Some("arn:aws:billingconductor::000000000000:pricingrule/ZZZZZZZZZY"),
            patch: json!({ "Description": "never stored" }),
            status: 404,
            code: "ResourceNotFoundException",
            reason: None,
        },
        RefusalCase {
            label: "a name another rule already holds",
            arn: None,
            patch: json!({ "Name": "other-rule" }),
            status: 409,
            code: "ConflictException",
            reason: Some("RESOURCE_NAME_CONFLICT"),
        },
        RefusalCase {
            label: "a TIERING type with no tiering configuration to price by",
            arn: None,
            patch: json!({ "Type": "TIERING" }),
            status: 400,
            code: "ValidationException",
            reason: Some("ILLEGAL_TIERING_INPUT"),
        },
        RefusalCase {
            label: "a tiering configuration on a percentage-priced rule",
            arn: None,
            patch: json!({ "Tiering": { "FreeTier": { "Activated": true } } }),
            status: 400,
            code: "ValidationException",
            reason: Some("ILLEGAL_TIERING_INPUT"),
        },
        RefusalCase {
            label: "a modifier percentage alongside a TIERING type",
            arn: None,
            patch: json!({
                "Type": "TIERING",
                "Tiering": { "FreeTier": { "Activated": true } },
                "ModifierPercentage": 3.0,
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("ILLEGAL_MODIFIER_PERCENTAGE"),
        },
    ];

    for case in &cases {
        let em = Emulator::new();
        let arn = em.create_rule(&markup_rule("rule-under-test")).await;
        em.create_rule(&markup_rule("other-rule")).await;
        let before = em.read_stored(&arn).await;

        let addressed_as = case.arn.unwrap_or(arn.as_str());
        let (status, body) = em.update_rule(addressed_as, &case.patch).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body:?}",
            case.label, case.status
        );
        let code = error_code(&body);
        assert_eq!(
            code, case.code,
            "case {:?}: expected error code {:?}, got {code:?} ({body:?})",
            case.label, case.code
        );
        assert!(
            body.get("Message").and_then(Value::as_str).is_some(),
            "case {:?}: the modelled error carries a required Message, got {body:?}",
            case.label
        );
        if let Some(reason) = case.reason {
            assert_eq!(
                text(&body, "Reason").as_deref(),
                Some(reason),
                "case {:?}: expected Reason {reason:?}, got {:?}",
                case.label,
                text(&body, "Reason")
            );
        }

        // A refused request leaves the rule exactly as it was.
        let after = em.read_stored(&arn).await;
        for member in ["Name", "Description", "Type", "ModifierPercentage", "Tiering"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {:?}: a refusal must not change {member}: was {:?}, now {:?}",
                case.label,
                before.get(member),
                after.get(member)
            );
        }
    }
}

#[tokio::test]
async fn a_renamed_rule_frees_its_old_name_for_another_rule() {
    let em = Emulator::new();
    let arn = em.create_rule(&markup_rule("first-name")).await;

    let (status, body) = em.update_rule(&arn, &json!({ "Name": "second-name" })).await;
    assert_eq!(
        status, 200,
        "renaming: expected 200, got {status} with {body:?}"
    );

    // The freed name is usable again, and the rule that takes it is a different one.
    let reused = em.create_rule(&markup_rule("first-name")).await;
    assert_ne!(
        reused, arn,
        "the rule created under the freed name must be a new rule, got {reused}"
    );

    // And the new name is now taken: renaming the second rule onto it conflicts.
    let (status, body) = em.update_rule(&reused, &json!({ "Name": "second-name" })).await;
    assert_eq!(
        status, 409,
        "taking a name in use: expected 409, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "taking a name in use: expected ConflictException, got {body:?}"
    );
}
