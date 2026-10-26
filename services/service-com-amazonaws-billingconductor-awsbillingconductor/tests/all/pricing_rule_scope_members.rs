// SPDX-License-Identifier: CC0-1.0

//! `CreatePricingRule` judged against the members its `Scope` keys on, driven over
//! the service's real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the
//! same path the production router serves.
//!
//! RULE under test: a pricing rule describes exactly ONE scope, and the members that
//! scope keys on are the whole of what the rule carries — so a request naming a
//! member its `Scope` does not key on is refused with the modelled 400
//! `ValidationException` under the `ValidationExceptionReason` the model enumerates
//! for that member, and stores nothing; a request naming exactly the keyed members is
//! served and reads back carrying those members and no others.
//!
//! The tables below span the whole class: every `PricingRuleScope` value crossed with
//! every scope-conditioned member, so each (scope, member) pairing is asserted on the
//! side its scope puts it — refused where the scope does not key it, stored where it
//! does — and the `Type` is varied across the rows so no row's result depends on the
//! rate the rule prices by. `tests/all/pricing_rule_core_loop.rs` asserts the other
//! direction of the same condition (a keyed member left OUT).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Which members each `PricingRuleScope` value keys on.
///
/// From the model's own member documentation
/// (`smithy_query(query="operation",operation="CreatePricingRule")` for the scope value
/// set, and `smithy_query(query="doc",shape="CreatePricingRuleInput",member="Service|UsageType|Operation|BillingEntity")`
/// for each condition): `Service` — "If the Scope attribute is set to SERVICE or SKU,
/// the attribute indicates which service the PricingRule is applicable for";
/// `UsageType` / `Operation` — "If the Scope attribute is set to SKU, this attribute
/// indicates which usage type / operation the PricingRule is modifying";
/// `BillingEntity` — the seller a `BILLING_ENTITY`-scoped rule prices. A `GLOBAL` rule
/// keys on none of them.
const SCOPE_KEYED_MEMBERS: &[(&str, &[&str])] = &[
    ("GLOBAL", &[]),
    ("SERVICE", &["Service"]),
    ("BILLING_ENTITY", &["BillingEntity"]),
    ("SKU", &["Service", "UsageType", "Operation"]),
];

/// Each scope-conditioned member: its name, the `ValidationExceptionReason` the model
/// enumerates for it (`smithy_query(query="shape",shape="ValidationExceptionReason")`),
/// and a value of the member's own modelled form.
const CONDITIONED_MEMBERS: &[(&str, &str, &str)] = &[
    ("Service", "ILLEGAL_SERVICE", "AmazonEC2"),
    ("UsageType", "ILLEGAL_USAGE_TYPE", "USW2-BoxUsage:m2.2xlarge"),
    ("Operation", "ILLEGAL_OPERATION", "RunInstances:0202"),
    ("BillingEntity", "ILLEGAL_BILLING_ENTITY", "AWS Marketplace"),
];

/// The `PricingRuleType` values, with the rate member each one prices by, so a row can
/// carry a different type without carrying the wrong rate
/// (`smithy_query(query="shape",shape="PricingRuleType")`).
const RULE_TYPES: &[&str] = &["MARKUP", "DISCOUNT", "TIERING"];

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this control plane is
    /// reachable at all (see `tests/all/payer_account_gate.rs` for the denial every
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

    /// POST a JSON body to one of this service's routes and return the status plus the
    /// parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
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

    /// Every stored pricing rule, read back through `ListPricingRules`.
    async fn stored_rules(&self) -> Vec<Value> {
        let mut collected = Vec::new();
        let mut request = json!({});
        loop {
            let (status, body) = self.post("/list-pricing-rules", &request).await;
            assert_eq!(
                status, 200,
                "ListPricingRules {request}: expected 200, got {status} with {body}"
            );
            collected.extend(body["PricingRules"].as_array().cloned().unwrap_or_default());
            match body["NextToken"].as_str() {
                Some(token) => request["NextToken"] = json!(token),
                None => return collected,
            }
        }
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

/// A value of `member`'s own modelled form, from [`CONDITIONED_MEMBERS`].
fn member_value(member: &str) -> Value {
    let (_, _, value) = CONDITIONED_MEMBERS
        .iter()
        .find(|(name, _, _)| *name == member)
        .unwrap_or_else(|| panic!("{member} is not a scope-conditioned member"));
    json!(value)
}

/// A create body for `scope` and `rule_type` carrying the rate that type prices by,
/// the members `scope` keys on, and nothing else.
fn well_formed_body(name: &str, scope: &str, rule_type: &str) -> Value {
    let mut body = json!({"Name": name, "Scope": scope, "Type": rule_type});
    if rule_type == "TIERING" {
        body["Tiering"] = json!({"FreeTier": {"Activated": true}});
    } else {
        body["ModifierPercentage"] = json!(5.0);
    }
    let keyed = SCOPE_KEYED_MEMBERS
        .iter()
        .find(|(value, _)| *value == scope)
        .map(|(_, members)| *members)
        .unwrap_or_default();
    for member in keyed {
        body[*member] = member_value(member);
    }
    body
}

/// RULE: a member the request's `Scope` does not key on makes the request describe a
/// rule the service's pricing cannot apply, so it is refused with the modelled 400
/// `ValidationException` under the reason enumerated for that member, naming it in
/// `Fields`, and nothing — not the rule, not even its name — is stored.
#[tokio::test]
async fn a_member_the_scope_does_not_key_on_is_refused_and_stores_nothing() {
    let em = Emulator::new();

    // Every (scope, member) pairing the scope does NOT key on, with the `Type` cycled
    // so the refusal is shown to hold whatever rate the rule would have priced by.
    let mut rows: Vec<(String, Value, &str, &str)> = Vec::new();
    let mut index = 0usize;
    for (scope, keyed) in SCOPE_KEYED_MEMBERS {
        for (member, reason, _) in CONDITIONED_MEMBERS {
            if keyed.contains(member) {
                continue;
            }
            let rule_type = RULE_TYPES[index % RULE_TYPES.len()];
            let name = format!("Illegal-{index}");
            let mut body = well_formed_body(&name, scope, rule_type);
            body[*member] = member_value(member);
            rows.push((
                format!("{scope} {rule_type} naming {member}"),
                body,
                reason,
                member,
            ));
            index += 1;
        }
    }
    // GLOBAL keys on none of the four, SERVICE and BILLING_ENTITY on one each, SKU on
    // three: 4 + 3 + 3 + 1 pairings a scope does not key on.
    assert_eq!(
        rows.len(),
        11,
        "expected every scope/member pairing the scope does not key on, got {rows:?}"
    );

    for (case, body, reason, member) in &rows {
        let (status, answered) = em.post("/create-pricing-rule", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {answered}"
        );
        assert_eq!(
            answered["Reason"].as_str(),
            Some(*reason),
            "case {case:?}: expected the enumerated reason {reason:?}, got {answered}"
        );
        assert_eq!(
            answered["Fields"][0]["Name"].as_str(),
            Some(*member),
            "case {case:?}: expected the refusal to name the member {member:?}, got {answered}"
        );
        assert!(
            answered["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the model marks Message @required, got {answered}"
        );
        assert_eq!(
            answered["Arn"].as_str(),
            None,
            "case {case:?}: a refused create mints no ARN, got {answered}"
        );
    }

    let stored = em.stored_rules().await;
    assert!(
        stored.is_empty(),
        "expected a refused create to store nothing, got {stored:?}"
    );

    // Nothing was stored, so the name each refused request asked for is still free: a
    // well-formed create under that same name is served.
    for (case, body, _, _) in &rows {
        let scope = body["Scope"].as_str().unwrap_or_default();
        let rule_type = body["Type"].as_str().unwrap_or_default();
        let name = body["Name"].as_str().unwrap_or_default();
        let (status, answered) = em
            .post(
                "/create-pricing-rule",
                &well_formed_body(name, scope, rule_type),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the name a refused create asked for to still be free, \
             got {status} with {answered}"
        );
    }
}

/// RULE: a request naming more than one member outside its scope's key is refused the
/// same way — one of the members it may not carry is named in `Fields`, under that
/// member's own enumerated reason, and nothing is stored.
#[tokio::test]
async fn several_members_outside_the_scope_key_are_refused_naming_one_of_them() {
    let em = Emulator::new();

    // (case, body, the members the scope does not key on)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "GLOBAL MARKUP naming a whole SKU key",
            json!({"Name": "GlobalWithSkuKey", "Scope": "GLOBAL", "Type": "MARKUP",
                   "ModifierPercentage": 5.0, "Service": "AmazonEC2",
                   "UsageType": "USW2-BoxUsage:m2.2xlarge", "Operation": "RunInstances:0202"}),
            vec!["Service", "UsageType", "Operation"],
        ),
        (
            "SERVICE DISCOUNT naming the usage type and the operation only a SKU rule acts on",
            json!({"Name": "ServiceWithSku", "Scope": "SERVICE", "Type": "DISCOUNT",
                   "ModifierPercentage": 5.0, "Service": "AmazonEC2",
                   "UsageType": "USW2-BoxUsage:m2.2xlarge", "Operation": "RunInstances:0202"}),
            vec!["UsageType", "Operation"],
        ),
        (
            "SKU TIERING naming a billing entity beside its own key",
            json!({"Name": "SkuWithEntity", "Scope": "SKU", "Type": "TIERING",
                   "Tiering": {"FreeTier": {"Activated": false}}, "Service": "AmazonEC2",
                   "UsageType": "USW2-BoxUsage:m2.2xlarge", "Operation": "RunInstances:0202",
                   "BillingEntity": "AWS Marketplace"}),
            vec!["BillingEntity"],
        ),
        (
            "BILLING_ENTITY MARKUP naming a service and a usage type",
            json!({"Name": "EntityWithService", "Scope": "BILLING_ENTITY", "Type": "MARKUP",
                   "ModifierPercentage": 5.0, "BillingEntity": "AWS",
                   "Service": "AmazonEC2", "UsageType": "USW2-BoxUsage:m2.2xlarge"}),
            vec!["Service", "UsageType"],
        ),
    ];

    for (case, body, illegal) in &rows {
        let (status, answered) = em.post("/create-pricing-rule", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {answered}"
        );
        let named = answered["Fields"][0]["Name"].as_str().unwrap_or_default();
        assert!(
            illegal.contains(&named),
            "case {case:?}: expected the refusal to name one of {illegal:?}, got {answered}"
        );
        let expected_reason = CONDITIONED_MEMBERS
            .iter()
            .find(|(member, _, _)| *member == named)
            .map(|(_, reason, _)| *reason);
        assert_eq!(
            answered["Reason"].as_str(),
            expected_reason,
            "case {case:?}: expected the reason enumerated for {named:?} \
             ({expected_reason:?}), got {answered}"
        );
    }

    let stored = em.stored_rules().await;
    assert!(
        stored.is_empty(),
        "expected a refused create to store nothing, got {stored:?}"
    );
}

/// RULE: a request naming exactly the members its scope keys on is served, and the
/// stored rule carries those members with the values the request sent and reports NO
/// value for the members the scope does not key on.
///
/// This is the other half of the same condition: the refusal above is about what a
/// scope may not carry, and this is about what it does — over every scope, crossed with
/// every `Type`.
#[tokio::test]
async fn a_rule_carrying_exactly_its_scope_key_is_stored_and_reports_no_other_member() {
    let em = Emulator::new();

    for (scope, keyed) in SCOPE_KEYED_MEMBERS {
        for rule_type in RULE_TYPES {
            let case = format!("{scope} {rule_type}");
            let name = format!("{scope}-{rule_type}");
            let body = well_formed_body(&name, scope, rule_type);
            let (status, answered) = em.post("/create-pricing-rule", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a rule carrying exactly its scope's key to be \
                 served, got {status} with {answered}"
            );
            let arn = answered["Arn"].as_str().unwrap_or_default().to_string();
            assert!(
                !arn.is_empty(),
                "case {case:?}: expected a minted PricingRuleArn, got {answered}"
            );

            let stored = em.stored_rules().await;
            let element = stored
                .iter()
                .find(|element| element["Arn"].as_str() == Some(arn.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: the created rule {arn} is not stored"));
            for (member, _, _) in CONDITIONED_MEMBERS {
                let reported = element.get(*member).cloned();
                let expected = if keyed.contains(member) {
                    Some(member_value(member))
                } else {
                    None
                };
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the stored {member} to be {expected:?}, got \
                     {reported:?} in {element}"
                );
            }
        }
    }
}
