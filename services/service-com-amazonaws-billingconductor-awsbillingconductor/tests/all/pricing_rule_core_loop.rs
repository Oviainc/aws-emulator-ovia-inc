// SPDX-License-Identifier: CC0-1.0

//! `CreatePricingRule` and the read it is observable through, `ListPricingRules`,
//! driven over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! This suite is the cover `pricing_rule_lifecycle.rs` (the create/read/delete
//! lifecycle) does not carry: the refusals a `Scope`/`Type` combination owes when a
//! member it keys on is absent, the `ConflictException` a taken name owes, a
//! `ClientToken` replay whose body DIFFERS from the first call's, the ARN filter
//! (including an ARN naming no stored rule), the `BillingPeriod` the read reports,
//! and the member-for-member round trip of every member a create accepts.
//!
//! Every test asserts a RULE over a SET of inputs rather than one recorded call: the
//! rows of each table differ along what the rule generalises over (the `Scope`, the
//! `Type`, which optional members the request carries, how many tiers, which filter,
//! which page size), and every value an assertion checks comes back OUT of state
//! through the read operation.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The members `CreatePricingRule` takes and `PricingRuleListElement` reports back
/// under the same name and the same JSON shape, so a stored rule can be compared to
/// the request that created it member by member
/// (`smithy_query(query="operation",operation="CreatePricingRule,ListPricingRules")`
/// and `smithy_query(query="shape",shape="PricingRuleListElement")`).
///
/// `Tags` is NOT among them: `PricingRuleListElement` models no `Tags` member, and
/// the operation that would report a rule's tags, `ListTagsForResource`, is still a
/// scaffold (its own stub issue), so a create's tags are stored but not yet readable
/// — one accepted row below sends them so the create path is exercised with them.
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "Scope",
    "Type",
    "ModifierPercentage",
    "Service",
    "BillingEntity",
    "UsageType",
    "Operation",
    "Tiering",
];

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

    /// POST a JSON body to one of this service's routes and return the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.post_with_token(path, body, None).await
    }

    /// As [`Self::post`], with the `ClientToken` in the header the model binds it to
    /// (`X-Amzn-Client-Token`).
    async fn post_with_token(
        &self,
        path: &str,
        body: &Value,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
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
            );
        if let Some(token) = client_token {
            builder = builder.header("x-amzn-client-token", token);
        }
        let request = builder
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

/// Does `value` match the full form of the modelled `PricingRuleArn` pattern,
/// `arn:aws:billingconductor::[0-9]{12}:pricingrule/[a-zA-Z0-9]{10}`?
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

/// The bare resource id of a full ARN — the other spelling the modelled
/// `PricingRuleArn` pattern admits for the same rule.
fn bare_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// Create one rule and return its ARN, failing the test with the case label when the
/// create is not served.
async fn create_rule(em: &Emulator, case: &str, body: &Value) -> String {
    let (status, answered) = em.post("/create-pricing-rule", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreatePricingRule to be served, got {status} with {answered}"
    );
    let arn = answered["Arn"].as_str().unwrap_or_default().to_string();
    assert!(
        is_pricing_rule_arn(&arn),
        "case {case:?}: expected a PricingRuleArn of the modelled form, got {arn:?}"
    );
    arn
}

/// Every rule `input` selects, following `NextToken` to the last page.
async fn list_all_rules(em: &Emulator, input: &Value) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = input.clone();
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = em.post("/list-pricing-rules", &request).await;
        assert_eq!(
            status, 200,
            "ListPricingRules {request}: expected 200, got {status} with {body}"
        );
        collected.extend(body["PricingRules"].as_array().cloned().unwrap_or_default());
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
}

/// The stored rule with this ARN, read back through `ListPricingRules`.
async fn stored_rule(em: &Emulator, arn: &str) -> Value {
    let listed = list_all_rules(em, &json!({"Filters": {"Arns": [arn]}})).await;
    assert_eq!(
        listed.len(),
        1,
        "expected exactly the rule {arn:?} to be reported, got {listed:?}"
    );
    listed.into_iter().next().unwrap_or(Value::Null)
}

/// The `Name` of every element, in the order they were reported.
fn names(elements: &[Value]) -> Vec<String> {
    elements
        .iter()
        .map(|element| element["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the create table: a request body that holds together, whatever its
/// `Scope`/`Type` combination.
fn accepted_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "GLOBAL MARKUP, the smallest request the model accepts",
            json!({"Name": "GlobalMarkup", "Scope": "GLOBAL", "Type": "MARKUP",
                   "ModifierPercentage": 10.5}),
        ),
        (
            "GLOBAL DISCOUNT with a description and tags",
            json!({"Name": "GlobalDiscount", "Scope": "GLOBAL", "Type": "DISCOUNT",
                   "ModifierPercentage": 25.0, "Description": "a discount",
                   "Tags": {"team": "finance"}}),
        ),
        (
            "GLOBAL MARKUP at the lower modelled ModifierPercentage bound",
            json!({"Name": "GlobalZero", "Scope": "GLOBAL", "Type": "MARKUP",
                   "ModifierPercentage": 0.0, "Description": ""}),
        ),
        (
            "SERVICE MARKUP naming the service it applies to",
            json!({"Name": "Service.Markup", "Scope": "SERVICE", "Type": "MARKUP",
                   "ModifierPercentage": 5.0, "Service": "AmazonEC2"}),
        ),
        (
            "SKU DISCOUNT naming the service, the usage type and the operation",
            json!({"Name": "Sku-Discount", "Scope": "SKU", "Type": "DISCOUNT",
                   "ModifierPercentage": 1.25, "Service": "AmazonEC2",
                   "UsageType": "USW2-BoxUsage:m2.2xlarge", "Operation": "RunInstances:0202"}),
        ),
        (
            "BILLING_ENTITY MARKUP naming the billing entity",
            json!({"Name": "Marketplace+Markup", "Scope": "BILLING_ENTITY", "Type": "MARKUP",
                   "ModifierPercentage": 7.0, "BillingEntity": "AWS Marketplace"}),
        ),
        (
            "GLOBAL TIERING with a free tier",
            json!({"Name": "FreeTierRule", "Scope": "GLOBAL", "Type": "TIERING",
                   "Tiering": {"FreeTier": {"Activated": true}}}),
        ),
        (
            "SERVICE TIERING with two custom tiers, the second unbounded above",
            json!({"Name": "CustomTiers@EC2", "Scope": "SERVICE", "Type": "TIERING",
                   "Service": "AmazonEC2",
                   "Tiering": {"CustomTiers": [
                       {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0,
                        "RateValue": 0.5},
                       {"BeginRangeInclusive": 100.0, "RateValue": 0.25}]}}),
        ),
        (
            "GLOBAL TIERING with a deactivated free tier and one custom tier",
            json!({"Name": "Mixed_Tiering", "Scope": "GLOBAL", "Type": "TIERING",
                   "Tiering": {"FreeTier": {"Activated": false},
                               "CustomTiers": [{"BeginRangeInclusive": 0.0,
                                                "RateValue": 1.0}]}}),
        ),
    ]
}

/// RULE: `CreatePricingRule` mints a fresh `PricingRuleArn` and PERSISTS the rule —
/// every member the request carried reads back out of state through
/// `ListPricingRules` unchanged, with the creation and last-modified instants the
/// create stamped and an association count derived from the pricing plans.
///
/// The rows span every `Scope` and every `Type` the model enumerates and differ
/// along which optional members the request carries and how the tiering
/// configuration is shaped.
#[tokio::test]
async fn a_created_pricing_rule_reads_back_out_of_state_member_for_member() {
    let em = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for (case, request) in accepted_rows() {
        let arn = create_rule(&em, case, &request).await;
        assert!(
            !minted.contains(&arn),
            "case {case:?}: expected a fresh ARN, got {arn:?} which was already minted"
        );
        minted.push(arn.clone());

        let stored = stored_rule(&em, &arn).await;
        assert_eq!(
            stored["Arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored rule to carry the minted ARN, got {stored}"
        );
        for member in ECHOED_MEMBERS {
            let sent = request.get(member).cloned();
            let reported = stored.get(member).cloned();
            assert_eq!(
                reported, sent,
                "case {case:?}: expected the stored {member} to be the one the request sent \
                 ({sent:?}), got {reported:?}"
            );
        }
        let created = stored["CreationTime"].as_i64().unwrap_or_default();
        let modified = stored["LastModifiedTime"].as_i64().unwrap_or_default();
        assert!(
            created > 0 && modified == created,
            "case {case:?}: expected one creation instant in epoch millis, also reported as \
             the last modification of a rule nothing has modified, got CreationTime {created} \
             and LastModifiedTime {modified}"
        );
        assert_eq!(
            stored["AssociatedPricingPlanCount"].as_i64(),
            Some(0),
            "case {case:?}: a rule no pricing plan names is associated with none, got {stored}"
        );
    }

    // The whole table is stored, in the creation order the minted ARNs carry.
    let listed = list_all_rules(&em, &json!({})).await;
    let expected: Vec<String> = accepted_rows()
        .iter()
        .map(|(_, request)| request["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names(&listed),
        expected,
        "expected every created rule to be reported in creation order, got {listed:?}"
    );
}

/// RULE: a `Scope`/`Type` combination must carry the members the model's own
/// documentation conditions on it — `Service` on `SERVICE`/`SKU`, `UsageType` and
/// `Operation` on `SKU`, `BillingEntity` on `BILLING_ENTITY`, `ModifierPercentage`
/// on `MARKUP`/`DISCOUNT` and `Tiering` on `TIERING` — and a request that leaves one
/// out is refused with the modelled `ValidationException` (HTTP 400) under the
/// `ValidationExceptionReason` the model enumerates for that member, naming it in
/// `Fields`, and stores nothing.
///
/// The rows differ along which member is missing and along the scope and the type
/// that make it required, so every conditioned member is covered. The model states no
/// condition that REFUSES one of these members on another combination, so this suite
/// asserts only the direction the documentation states.
#[tokio::test]
async fn a_combination_missing_a_member_it_keys_on_is_refused() {
    // (case, body, expected Reason, expected member at fault)
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "MARKUP without a percentage modifier",
            json!({"Name": "NoModifier", "Scope": "GLOBAL", "Type": "MARKUP"}),
            "ILLEGAL_MODIFIER_PERCENTAGE",
            "ModifierPercentage",
        ),
        (
            "DISCOUNT without a percentage modifier",
            json!({"Name": "NoModifierDiscount", "Scope": "GLOBAL", "Type": "DISCOUNT"}),
            "ILLEGAL_MODIFIER_PERCENTAGE",
            "ModifierPercentage",
        ),
        (
            "SERVICE-scoped DISCOUNT without a percentage modifier",
            json!({"Name": "ServiceNoModifier", "Scope": "SERVICE", "Type": "DISCOUNT",
                   "Service": "AmazonEC2"}),
            "ILLEGAL_MODIFIER_PERCENTAGE",
            "ModifierPercentage",
        ),
        (
            "SERVICE scope without a service",
            json!({"Name": "ServiceScopeNoService", "Scope": "SERVICE", "Type": "MARKUP",
                   "ModifierPercentage": 5.0}),
            "ILLEGAL_SERVICE",
            "Service",
        ),
        (
            "SKU scope without a service",
            json!({"Name": "SkuScopeNoService", "Scope": "SKU", "Type": "DISCOUNT",
                   "ModifierPercentage": 5.0, "UsageType": "USW2-BoxUsage:m2.2xlarge",
                   "Operation": "RunInstances:0202"}),
            "ILLEGAL_SERVICE",
            "Service",
        ),
        (
            "SKU scope without a usage type",
            json!({"Name": "SkuNoUsageType", "Scope": "SKU", "Type": "DISCOUNT",
                   "ModifierPercentage": 5.0, "Service": "AmazonEC2",
                   "Operation": "RunInstances:0202"}),
            "ILLEGAL_USAGE_TYPE",
            "UsageType",
        ),
        (
            "SKU scope without an operation",
            json!({"Name": "SkuNoOperation", "Scope": "SKU", "Type": "DISCOUNT",
                   "ModifierPercentage": 5.0, "Service": "AmazonEC2",
                   "UsageType": "USW2-BoxUsage:m2.2xlarge"}),
            "ILLEGAL_OPERATION",
            "Operation",
        ),
        (
            "BILLING_ENTITY scope without a billing entity",
            json!({"Name": "EntityScopeNoEntity", "Scope": "BILLING_ENTITY", "Type": "MARKUP",
                   "ModifierPercentage": 5.0}),
            "ILLEGAL_BILLING_ENTITY",
            "BillingEntity",
        ),
        (
            "TIERING without a tiering configuration",
            json!({"Name": "TieringNoTiering", "Scope": "GLOBAL", "Type": "TIERING"}),
            "ILLEGAL_TIERING_INPUT",
            "Tiering",
        ),
        (
            "SKU-scoped TIERING without a tiering configuration",
            json!({"Name": "SkuTieringNoTiering", "Scope": "SKU", "Type": "TIERING",
                   "Service": "AmazonEC2", "UsageType": "USW2-BoxUsage:m2.2xlarge",
                   "Operation": "RunInstances:0202"}),
            "ILLEGAL_TIERING_INPUT",
            "Tiering",
        ),
    ];

    let em = Emulator::new();
    for (case, body, reason, field) in &rows {
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
            Some(*field),
            "case {case:?}: expected the refusal to name the member {field:?}, got {answered}"
        );
        assert!(
            answered["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the model marks Message @required, got {answered}"
        );
    }

    // A refused create stores nothing: the whole table left the store empty.
    let listed = list_all_rules(&em, &json!({})).await;
    assert!(
        listed.is_empty(),
        "expected no rule to be stored by a refused create, got {listed:?}"
    );
}

/// RULE: "The names must be unique to each pricing rule" — a second create under a
/// name a stored rule already carries is refused with the modelled
/// `ConflictException` (HTTP 409) naming the rule that holds the name, and no second
/// rule is stored.
///
/// The rows differ along the name (each character class the modelled pattern admits,
/// and both of its length bounds) and along how far the second request's body differs
/// from the first.
#[tokio::test]
async fn a_pricing_rule_name_is_unique() {
    let longest = "N".repeat(128);
    let names_taken = ["Rule_1", "a+b=c.d-e@f", "x", longest.as_str()];

    let em = Emulator::new();
    for name in names_taken {
        let first = json!({"Name": name, "Scope": "GLOBAL", "Type": "MARKUP",
                           "ModifierPercentage": 1.0});
        let arn = create_rule(&em, &format!("first create of {name:?}"), &first).await;

        // The second request differs from the first in everything but the name, so
        // the refusal is the name's and not the body's.
        let second = json!({"Name": name, "Scope": "SERVICE", "Type": "TIERING",
                            "Service": "AmazonS3",
                            "Tiering": {"FreeTier": {"Activated": true}}});
        let (status, answered) = em.post("/create-pricing-rule", &second).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected HTTP 409 for a name already taken, got {status} with \
             {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ConflictException",
            "case {name:?}: expected the modelled ConflictException, got {answered}"
        );
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: expected the conflict to name the rule holding the name, got \
             {answered}"
        );
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some("pricingrule"),
            "case {name:?}: expected the conflict to name the resource type, got {answered}"
        );
        assert_eq!(
            answered["Reason"].as_str(),
            Some("RESOURCE_NAME_CONFLICT"),
            "case {name:?}: expected the enumerated conflict reason, got {answered}"
        );

        // The stored rule is still the FIRST one, unchanged by the refused create.
        let stored = stored_rule(&em, &arn).await;
        assert_eq!(
            (stored["Scope"].as_str(), stored["Type"].as_str()),
            (Some("GLOBAL"), Some("MARKUP")),
            "case {name:?}: expected the first rule to be untouched, got {stored}"
        );
    }

    let listed = list_all_rules(&em, &json!({})).await;
    assert_eq!(
        listed.len(),
        names_taken.len(),
        "expected one rule per name and no second rule for any of them, got {listed:?}"
    );
}

/// RULE: "if the original request completes successfully, any subsequent retries
/// complete successfully without performing any further actions" — a create replayed
/// under the same `ClientToken` answers with the ARN the first call minted and stores
/// no second rule, while a different token mints a new one.
///
/// The rows differ along the token (each character class the modelled pattern admits
/// and both of its length bounds) and along how far the replayed body differs.
#[tokio::test]
async fn a_client_token_replay_creates_no_second_pricing_rule() {
    // `ClientToken`: len 1..64, pattern ^[a-zA-Z0-9-]+$.
    let longest = "t".repeat(64);
    let tokens = ["t", "Token-0123", longest.as_str()];

    let em = Emulator::new();
    for (index, token) in tokens.iter().enumerate() {
        let name = format!("Replayed{index}");
        let first = json!({"Name": name, "Scope": "GLOBAL", "Type": "MARKUP",
                           "ModifierPercentage": 2.0});
        let (status, answered) = em
            .post_with_token("/create-pricing-rule", &first, Some(token))
            .await;
        assert_eq!(
            status, 200,
            "case token {token:?}: expected the first create to be served, got {status} with \
             {answered}"
        );
        let arn = answered["Arn"].as_str().unwrap_or_default().to_string();

        // The replay carries a DIFFERENT body under the same token: the documented
        // contract is that the retry performs no further action, so it is served the
        // rule the first call minted.
        let replay = json!({"Name": format!("{name}Retried"), "Scope": "SERVICE",
                            "Type": "DISCOUNT", "ModifierPercentage": 9.0,
                            "Service": "AmazonS3"});
        let (status, replayed) = em
            .post_with_token("/create-pricing-rule", &replay, Some(token))
            .await;
        assert_eq!(
            status, 200,
            "case token {token:?}: expected the replay to be served, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["Arn"].as_str(),
            Some(arn.as_str()),
            "case token {token:?}: expected the replay to answer with the ARN the first call \
             minted ({arn:?}), got {replayed}"
        );
        let stored = stored_rule(&em, &arn).await;
        assert_eq!(
            stored["Name"].as_str(),
            Some(name.as_str()),
            "case token {token:?}: expected the replay to perform no further action, got {stored}"
        );
    }

    // One rule per token, and a fresh token creates another one.
    let listed = list_all_rules(&em, &json!({})).await;
    assert_eq!(
        listed.len(),
        tokens.len(),
        "expected exactly one rule per client token, got {listed:?}"
    );
    let fresh = json!({"Name": "FreshToken", "Scope": "GLOBAL", "Type": "MARKUP",
                       "ModifierPercentage": 2.0});
    let (status, answered) = em
        .post_with_token("/create-pricing-rule", &fresh, Some("another-token"))
        .await;
    assert_eq!(
        status, 200,
        "expected a create under an unused token to be served, got {status} with {answered}"
    );
    let listed = list_all_rules(&em, &json!({})).await;
    assert_eq!(
        listed.len(),
        tokens.len() + 1,
        "expected an unused client token to mint a new rule, got {listed:?}"
    );
}

/// RULE: `AssociatedPricingPlanCount` is "the pricing plans count that this pricing
/// rule is associated with" — it is computed from the plans that name the rule in
/// their `PricingRuleArns`, in either spelling the modelled ARN pattern admits, and
/// counts each such plan exactly once.
///
/// The rows differ along how many plans name the rule (none, one, two) and along the
/// spelling they name it by (full ARN, bare 10-character id).
#[tokio::test]
async fn the_association_count_of_a_rule_is_derived_from_the_plans_that_name_it() {
    let em = Emulator::new();
    let mut arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let body = json!({"Name": format!("Counted{index}"), "Scope": "GLOBAL",
                          "Type": "MARKUP", "ModifierPercentage": 1.0});
        arns.push(create_rule(&em, &format!("rule {index}"), &body).await);
    }

    // One plan names rule 0 by its full ARN; another names rule 0 by its bare id and
    // rule 1 by its full ARN; a third names no rule at all.
    let plans = [
        json!({"Name": "PlanFullArn", "PricingRuleArns": [arns[0]]}),
        json!({"Name": "PlanBareId",
               "PricingRuleArns": [bare_id(&arns[0]), arns[1]]}),
        json!({"Name": "PlanNoRules"}),
    ];
    for plan in &plans {
        let (status, answered) = em.post("/create-pricing-plan", plan).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {plan}: expected 200, got {status} with {answered}"
        );
    }

    let expected = [(0_usize, 2_i64), (1, 1), (2, 0)];
    for (index, count) in expected {
        let stored = stored_rule(&em, &arns[index]).await;
        assert_eq!(
            stored["AssociatedPricingPlanCount"].as_i64(),
            Some(count),
            "case rule {index}: expected {count} associated pricing plan(s), got {stored}"
        );
    }
}

/// RULE: `ListPricingRules` reports the stored rules in creation order, at most
/// `MaxResults` per page with a `NextToken` naming the cut point, and
/// `Filters.Arns` selects exactly the named rules in either spelling the modelled ARN
/// pattern admits — a filter naming a rule that does not exist selects none rather
/// than failing. The reported `BillingPeriod` is the one the request asked for.
///
/// The rows differ along the page size (each bound of the modelled 1..100 range and
/// values between them), the number of pages that takes, and the filter.
#[tokio::test]
async fn list_pricing_rules_pages_in_creation_order_and_filters_by_arn() {
    let em = Emulator::new();
    let mut arns: Vec<String> = Vec::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let name = format!("Paged{index}");
        let body = json!({"Name": name, "Scope": "GLOBAL", "Type": "MARKUP",
                          "ModifierPercentage": f64::from(index)});
        arns.push(create_rule(&em, &format!("rule {index}"), &body).await);
        created.push(name);
    }

    // Every page size reports the same sequence, in the same order, with no element
    // repeated or dropped.
    for max_results in [1_i64, 2, 3, 5, 100] {
        let collected = list_all_rules(&em, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            names(&collected),
            created,
            "case MaxResults={max_results}: expected every rule once, in creation order, got \
             {collected:?}"
        );
    }

    // A page is bounded by MaxResults, and the token is handed back only while rules
    // are left.
    for max_results in [1_i64, 2, 5] {
        let (status, body) = em
            .post("/list-pricing-rules", &json!({"MaxResults": max_results}))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: expected 200, got {status} with {body}"
        );
        let page = body["PricingRules"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            i64::try_from(page.len()).unwrap_or(i64::MAX),
            max_results,
            "case MaxResults={max_results}: expected a page of that size, got {page:?}"
        );
        let has_token = body["NextToken"].as_str().is_some();
        assert_eq!(
            has_token,
            max_results < 5,
            "case MaxResults={max_results}: expected a NextToken only while rules are left, \
             got {body}"
        );
    }

    // `Filters.Arns`: the full ARN, the bare id, both spellings mixed, a rule that
    // does not exist, and an ARN of a rule that exists beside one that does not.
    let absent = "arn:aws:billingconductor::000000000000:pricingrule/ZZZZZZZZZZ";
    let filters: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one full ARN",
            json!([arns[1]]),
            vec![created[1].clone()],
        ),
        (
            "one bare id",
            json!([bare_id(&arns[3])]),
            vec![created[3].clone()],
        ),
        (
            "both spellings of two rules",
            json!([arns[0], bare_id(&arns[4])]),
            vec![created[0].clone(), created[4].clone()],
        ),
        ("a rule that does not exist", json!([absent]), Vec::new()),
        (
            "one stored rule beside one that does not exist",
            json!([absent, arns[2]]),
            vec![created[2].clone()],
        ),
    ];
    for (case, arns_filter, expected) in filters {
        let listed = list_all_rules(&em, &json!({"Filters": {"Arns": arns_filter}})).await;
        assert_eq!(
            names(&listed),
            expected,
            "case {case:?}: expected the filter to select {expected:?}, got {listed:?}"
        );
    }

    // `BillingPeriod`: "the billing period for which the described pricing rules are
    // applicable" is the one the request asked for, and absent when it asked for none.
    // The rows are both forms the modelled pattern ^\d{4}-(0?[1-9]|1[012])$ admits.
    for period in ["2026-07", "2026-7", "2026-12"] {
        let (status, body) = em
            .post("/list-pricing-rules", &json!({"BillingPeriod": period}))
            .await;
        assert_eq!(
            status, 200,
            "case BillingPeriod={period:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["BillingPeriod"].as_str(),
            Some(period),
            "case BillingPeriod={period:?}: expected the period the request asked for, got {body}"
        );
    }
    let (status, body) = em.post("/list-pricing-rules", &json!({})).await;
    assert_eq!(
        status, 200,
        "case no BillingPeriod: expected 200, got {status} with {body}"
    );
    assert!(
        body.get("BillingPeriod").is_none(),
        "case no BillingPeriod: expected no period to be reported, got {body}"
    );
    // The read reported rules without changing what a later caller sees.
    let after = list_all_rules(&em, &json!({})).await;
    assert_eq!(
        names(&after),
        created,
        "expected the reads to have left the store untouched, got {after:?}"
    );
    let object: &Map<String, Value> = body.as_object().expect("a JSON object response");
    assert!(
        object.contains_key("PricingRules"),
        "expected the modelled PricingRules member even when it is empty, got {body}"
    );
}
