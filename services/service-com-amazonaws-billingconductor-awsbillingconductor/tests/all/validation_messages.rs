// SPDX-License-Identifier: CC0-1.0

//! The modelled `ValidationException` on the PRICING-PLAN and PRICING-RULE surface, and
//! the two parts of the rule the billing-group suite
//! (`validation_exception_message.rs`) cannot pin: the clause ORDER a multi-violation
//! request states, and that EVERY operation the model declares answers a constraint
//! refusal in the modelled shape rather than the framework's generic envelope.
//!
//! RULE under test: a request that breaks a constraint the model declares is refused with
//! the modelled `ValidationException` (`@httpError 400`, error code `ValidationException`),
//! whose `@required Message` is built FROM THE REQUEST —
//! `<N> validation error(s) detected: ` over one
//! `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clause per
//! failing member, joined with `; `. A violation on a list's ELEMENTS is stated once at
//! the list member, with the element's requirement nested and the list itself as the
//! value; a `@pattern` is stated without the anchors the model spells it with; a
//! `@sensitive` member is named with no value; and the clauses come out in the REVERSE of
//! the order the boundary walk finds them, which is the reverse of the order the model
//! declares the members.
//!
//! The rows span what the rule generalises over rather than one recorded call: the
//! operation (ten of them), the HTTP binding the offending member travels in (JSON body,
//! `@httpLabel`, `@httpHeader`), the constraint class (`@pattern`, `@length` maximum,
//! `@range` at each published bound and one step past it, an absent `@required` member),
//! the member's name (`arn`, `pricingPlanArn`, `pricingRuleArn`, `resourceArn`,
//! `maxResults`, `clientToken`, `name`, and a list member's elements), how many elements
//! of a list are at fault, and how many violations one request carries (one, two, three).
//! The recorded cases (`UpdatePricingPlan` with a non-conforming `Arn`, and
//! `AssociatePricingRules` with both a bad plan ARN and a bad rule ARN) are two rows.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// `PricingPlanArn`'s `@pattern`, as a message states it: the model's expression without
/// the `^`/`$` anchors (`smithy_query(query="shape",shape="PricingPlanArn")`).
const PRICING_PLAN_ARN: &str = "(arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})";

/// `PricingRuleArn`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="PricingRuleArn")`).
const PRICING_RULE_ARN: &str =
    "(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}";

/// `BillingPeriod`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="BillingPeriod")`).
const BILLING_PERIOD: &str = r"\d{4}-(0?[1-9]|1[012])";

/// `PricingPlanName`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="PricingPlanName")`). The shape is `@sensitive`, so
/// a clause about it states no value.
const PRICING_NAME: &str = r"[a-zA-Z0-9_\+=\.\-@]+";

/// `ClientToken`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="ClientToken")`).
const CLIENT_TOKEN: &str = "[a-zA-Z0-9-]+";

/// The tagging operations' `ResourceArn` `@pattern`, with only the OUTER anchors stripped
/// (`smithy_query(query="operation",operation="ListTagsForResource")`): each alternative
/// of this expression carries its own `^`/`$`, and only the expression's own leading `^`
/// and trailing `$` are the anchoring the model adds to make the match total.
const RESOURCE_ARN: &str = concat!(
    r"arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/?[0-9]{12}$",
    r"|^arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingplan/[a-zA-Z0-9]{10}$",
    r"|^arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/[a-zA-Z0-9]{10}$",
    r"|^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}",
);

/// A pricing-plan ARN the `PricingPlanArn` pattern admits, in its full-ARN form.
const VALID_PLAN_ARN: &str = "arn:aws:billingconductor::123456789012:pricingplan/ABCDEFGHIJ";

/// A pricing-rule ARN the `PricingRuleArn` pattern admits, in its bare-id form.
const VALID_RULE_ARN: &str = "KLMNOPQRST";

/// This service's Smithy model, read at compile time so a model-derived test cannot drift
/// from the model this crate is generated from.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-billingconductor-awsbillingconductor.json"
));

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
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
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
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let payload = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the body")),
            None => Bytes::new(),
        };
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

    /// POST a JSON body, the shape most of this service's operations take.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.call("POST", path, Some(body), &[]).await
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

/// One row of the table: a request, and the whole message the service must answer with.
struct Case {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    headers: Vec<(&'static str, &'static str)>,
    message: String,
}

/// THE RULE on the pricing surface, over the class of refusable requests.
#[tokio::test]
async fn a_pricing_constraint_violation_is_refused_with_the_required_message() {
    let long_name = "N".repeat(129);
    let cases = vec![
        // @pattern on a required body ARN, across four operations and two ARN shapes.
        Case {
            // The recorded case: fixture
            // billingconductor-pricing-plan-arn-validation-errors, task#1.
            label: "UpdatePricingPlan with a non-conforming Arn",
            method: "PUT",
            uri: "/update-pricing-plan".to_string(),
            body: Some(json!({ "Arn": "not-an-arn" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {PRICING_PLAN_ARN}"
            ),
        },
        Case {
            label: "DeletePricingPlan with a plan id one character too short",
            method: "POST",
            uri: "/delete-pricing-plan".to_string(),
            body: Some(json!({ "Arn": "ABCDEFGHI" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'ABCDEFGHI' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {PRICING_PLAN_ARN}"
            ),
        },
        Case {
            label: "UpdatePricingRule with a non-conforming Arn",
            method: "PUT",
            uri: "/update-pricing-rule".to_string(),
            body: Some(json!({ "Arn": "not-an-arn" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {PRICING_RULE_ARN}"
            ),
        },
        Case {
            label: "DeletePricingRule with a rule id one character too long",
            method: "POST",
            uri: "/delete-pricing-rule".to_string(),
            body: Some(json!({ "Arn": "ABCDEFGHIJK" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'ABCDEFGHIJK' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {PRICING_RULE_ARN}"
            ),
        },
        // The same violation on a member the model does NOT call `arn`: the clause names
        // the member the request carried it in.
        Case {
            label: "ListPricingRulesAssociatedToPricingPlan names pricingPlanArn",
            method: "POST",
            uri: "/list-pricing-rules-associated-to-pricing-plan".to_string(),
            body: Some(json!({ "PricingPlanArn": "not-an-arn" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'not-an-arn' at 'pricingPlanArn' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {PRICING_PLAN_ARN}"
            ),
        },
        Case {
            label: "ListPricingPlansAssociatedWithPricingRule names pricingRuleArn",
            method: "POST",
            uri: "/list-pricing-plans-associated-with-pricing-rule".to_string(),
            body: Some(json!({ "PricingRuleArn": "not-an-arn" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'not-an-arn' at 'pricingRuleArn' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {PRICING_RULE_ARN}"
            ),
        },
        // An absent @required member: stated as the null it was, with no value to echo.
        Case {
            label: "DeletePricingPlan with no Arn at all",
            method: "POST",
            uri: "/delete-pricing-plan".to_string(),
            body: Some(json!({})),
            headers: Vec::new(),
            message: "1 validation error detected: Value null at 'arn' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
        // A @sensitive member (`PricingPlanName`): the clause names the member and the
        // requirement, and NOT what was sent.
        Case {
            label: "UpdatePricingPlan with a Name the pattern refuses",
            method: "PUT",
            uri: "/update-pricing-plan".to_string(),
            body: Some(json!({ "Arn": VALID_PLAN_ARN, "Name": "has a space" })),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {PRICING_NAME}"
            ),
        },
        // @length's published maximum (PricingPlanName is 1..128), one character past it.
        Case {
            label: "UpdatePricingPlan with a Name one character over the length maximum",
            method: "PUT",
            uri: "/update-pricing-plan".to_string(),
            body: Some(json!({ "Arn": VALID_PLAN_ARN, "Name": long_name })),
            headers: Vec::new(),
            message: "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                      Member must have length less than or equal to 128"
                .to_string(),
        },
        // @range at each published bound of MaxPricingPlanResults (1..100), one step past.
        Case {
            label: "ListPricingRulesAssociatedToPricingPlan with MaxResults below the minimum",
            method: "POST",
            uri: "/list-pricing-rules-associated-to-pricing-plan".to_string(),
            body: Some(json!({ "PricingPlanArn": VALID_PLAN_ARN, "MaxResults": 0 })),
            headers: Vec::new(),
            message: "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                      constraint: Member must have value greater than or equal to 1"
                .to_string(),
        },
        Case {
            label: "ListPricingRulesAssociatedToPricingPlan with MaxResults above the maximum",
            method: "POST",
            uri: "/list-pricing-rules-associated-to-pricing-plan".to_string(),
            body: Some(json!({ "PricingPlanArn": VALID_PLAN_ARN, "MaxResults": 101 })),
            headers: Vec::new(),
            message: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 100"
                .to_string(),
        },
        // A violation on the scalar ELEMENTS of a list member is stated once, at the list
        // member, with the list as the caller sent it and the element's requirement
        // nested — so a conforming element beside a bad one is still ONE error.
        Case {
            label: "AssociatePricingRules with one non-conforming rule ARN in the list",
            method: "PUT",
            uri: "/associate-pricing-rules".to_string(),
            body: Some(
                json!({ "Arn": VALID_PLAN_ARN, "PricingRuleArns": [VALID_RULE_ARN, "nope"] }),
            ),
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value '[{VALID_RULE_ARN}, nope]' at \
                 'pricingRuleArns' failed to satisfy constraint: Member must satisfy \
                 constraint: [Member must satisfy regular expression pattern: \
                 {PRICING_RULE_ARN}]"
            ),
        },
        // Two violations in one request: counted in the prefix, joined by `; `, and in the
        // reverse of the order the model declares the members (`Arn`, then
        // `PricingRuleArns`) — which is the order the recorded answer states them in.
        Case {
            // The recorded case: fixture
            // billingconductor-pricing-plan-arn-validation-errors, task#3.
            label: "AssociatePricingRules with a bad plan ARN and a bad rule ARN",
            method: "PUT",
            uri: "/associate-pricing-rules".to_string(),
            body: Some(json!({ "Arn": "not-an-arn", "PricingRuleArns": ["not-an-arn-2"] })),
            headers: Vec::new(),
            message: format!(
                "2 validation errors detected: Value '[not-an-arn-2]' at 'pricingRuleArns' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy regular expression pattern: {PRICING_RULE_ARN}]; Value 'not-an-arn' at \
                 'arn' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {PRICING_PLAN_ARN}"
            ),
        },
        Case {
            label: "DisassociatePricingRules with two bad rule ARNs and a bad plan ARN",
            method: "PUT",
            uri: "/disassociate-pricing-rules".to_string(),
            body: Some(json!({ "Arn": "not-an-arn", "PricingRuleArns": ["bad-one", "bad-two"] })),
            headers: Vec::new(),
            message: format!(
                "2 validation errors detected: Value '[bad-one, bad-two]' at 'pricingRuleArns' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy regular expression pattern: {PRICING_RULE_ARN}]; Value 'not-an-arn' at \
                 'arn' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {PRICING_PLAN_ARN}"
            ),
        },
        // THREE violations, across three constraint classes, on an input whose members the
        // model declares in the order `BillingPeriod`, `PricingPlanArn`, `MaxResults`: the
        // clauses come out in the reverse of that order.
        Case {
            label: "ListPricingRulesAssociatedToPricingPlan breaking three constraints",
            method: "POST",
            uri: "/list-pricing-rules-associated-to-pricing-plan".to_string(),
            body: Some(json!({
                "BillingPeriod": "2026-13",
                "PricingPlanArn": "not-an-arn",
                "MaxResults": 0,
            })),
            headers: Vec::new(),
            message: format!(
                "3 validation errors detected: Value '0' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1; Value \
                 'not-an-arn' at 'pricingPlanArn' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: {PRICING_PLAN_ARN}; Value '2026-13' at \
                 'billingPeriod' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {BILLING_PERIOD}"
            ),
        },
        // A member bound to the request URI rather than the body.
        Case {
            label: "ListTagsForResource with a non-conforming resource ARN label",
            method: "GET",
            uri: "/tags/short".to_string(),
            body: None,
            headers: Vec::new(),
            message: format!(
                "1 validation error detected: Value 'short' at 'resourceArn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {RESOURCE_ARN}"
            ),
        },
        // …and one bound to a request HEADER.
        Case {
            label: "CreatePricingPlan with a client token the pattern refuses",
            method: "POST",
            uri: "/create-pricing-plan".to_string(),
            body: Some(json!({ "Name": "ValidPlanName" })),
            headers: vec![("x-amzn-client-token", "bad token")],
            message: format!(
                "1 validation error detected: Value 'bad token' at 'clientToken' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {CLIENT_TOKEN}"
            ),
        },
    ];

    let em = Emulator::new();
    for case in &cases {
        let (status, body) = em
            .call(case.method, &case.uri, case.body.as_ref(), &case.headers)
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected the modelled ValidationException, got {body}",
            case.label
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(case.message.as_str()),
            "case {:?}: expected the Message {:?}, got {:?}",
            case.label,
            case.message,
            body["Message"]
        );
        // The model names the member `Message`; the framework's generic envelope names it
        // `message`. A body carrying the lower-case spelling is the generic one, from
        // which an SDK reads the required member as null.
        assert!(
            body.get("message").is_none(),
            "case {:?}: expected no lower-case message member, got {body}",
            case.label
        );
    }
}

/// RULE: a request within every modelled constraint is NOT refused — it reaches the
/// operation, which answers from its own behaviour: a 200 for an implemented operation,
/// the scaffold's 501 for one nobody has written yet, never a 400.
///
/// The rows differ along the operation, which optional members the request carries, the
/// binding they travel in, and the id form a conforming ARN takes (full ARN, bare id, and
/// the AWS-managed plan ids the `PricingPlanArn` pattern enumerates).
#[tokio::test]
async fn a_request_within_every_modelled_constraint_reaches_the_operation() {
    let em = Emulator::new();

    // An implemented operation still serves a conforming request, including a @sensitive
    // name at each end of its @length bound and a client token the pattern admits.
    let accepted = [
        (
            "CreatePricingPlan with the shortest legal name",
            json!({ "Name": "A" }),
            Vec::new(),
        ),
        (
            "CreatePricingPlan with a 128-character name and a conforming client token",
            json!({ "Name": "N".repeat(128), "Description": "" }),
            vec![("x-amzn-client-token", "token-0123")],
        ),
        (
            "CreatePricingPlan with an empty rule list (the lower modelled bound)",
            json!({ "Name": "EmptyRuleList", "PricingRuleArns": [] }),
            Vec::new(),
        ),
    ];
    for (case, body, headers) in &accepted {
        let (status, answered) = em
            .call("POST", "/create-pricing-plan", Some(body), headers)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the conforming request to be served, got {status} with \
             {answered}"
        );
    }

    // And what a conforming request STORES reads back out of state through an implemented
    // reader, so the validation seam did not change what a served request does.
    let (status, created) = em
        .post("/create-pricing-plan", &json!({ "Name": "RoundTripPlan" }))
        .await;
    assert_eq!(
        status, 200,
        "expected the create to be served, got {status} with {created}"
    );
    let arn = created["Arn"].as_str().unwrap_or_default().to_string();
    let (status, group) = em
        .post(
            "/create-billing-group",
            &json!({
                "Name": "RoundTripGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": arn },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the group naming the minted plan to be served, got {status} with {group}"
    );
    let (status, listed) = em.post("/list-billing-groups", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListBillingGroups to be served, got {status} with {listed}"
    );
    let stored = listed["BillingGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        stored.iter().any(|element| {
            element["Name"].as_str() == Some("RoundTripGroup")
                && element["ComputationPreference"]["PricingPlanArn"].as_str() == Some(arn.as_str())
        }),
        "expected the stored group to name the minted plan {arn:?}, got {stored:?}"
    );

    // A conforming request REACHES the operation: one nobody has written yet answers
    // the scaffold's 501, and an implemented one answers from its own state (here, the
    // modelled 404 for a rule ARN nothing created). Either way the answer comes from the
    // operation, not from this seam: it refuses input, it does not implement operations.
    let reaches_the_operation = [
        (
            "UpdatePricingPlan with a conforming full-ARN plan",
            "PUT",
            "/update-pricing-plan",
            json!({ "Arn": VALID_PLAN_ARN, "Name": "StillScaffolded" }),
            501,
        ),
        (
            "UpdatePricingRule with a conforming bare rule id",
            "PUT",
            "/update-pricing-rule",
            json!({ "Arn": VALID_RULE_ARN, "Name": "StillScaffolded" }),
            // Implemented: no rule answers to this id, so the operation's own
            // `ResourceNotFoundException` is what comes back.
            404,
        ),
        (
            "AssociatePricingRules with an AWS-managed plan id and a rule list",
            "PUT",
            "/associate-pricing-rules",
            json!({ "Arn": "BasicPricingPlan", "PricingRuleArns": [VALID_RULE_ARN] }),
            501,
        ),
        (
            "ListPricingRulesAssociatedToPricingPlan at the lower MaxResults bound",
            "POST",
            "/list-pricing-rules-associated-to-pricing-plan",
            json!({ "PricingPlanArn": VALID_PLAN_ARN, "MaxResults": 1, "BillingPeriod": "2026-1" }),
            501,
        ),
        (
            "ListPricingPlansAssociatedWithPricingRule at the upper MaxResults bound",
            "POST",
            "/list-pricing-plans-associated-with-pricing-rule",
            json!({ "PricingRuleArn": VALID_RULE_ARN, "MaxResults": 100 }),
            501,
        ),
    ];
    for (case, method, path, body, expected) in &reaches_the_operation {
        let (status, answered) = em.call(method, path, Some(body), &[]).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected the conforming request to reach the operation \
             (HTTP {expected}), got {status} with {answered}"
        );
    }
}

/// RULE: EVERY operation this service's model declares answers a constraint refusal in
/// the modelled shape — the body carries `Message`, never the generic envelope's
/// lower-case `message`.
///
/// Derived from the model rather than from a hand-written list: the service registers its
/// operations behind the validation wrapper one line at a time, and an operation left out
/// of that list silently reverts to the framework's generic envelope. This drives EVERY
/// operation the model declares, at the route the model binds it to, with an empty JSON
/// body — which breaks a `@required` member of most of them and is accepted by the rest —
/// and holds every answer that IS a `ValidationException` to the modelled shape.
#[tokio::test]
async fn every_modelled_operation_is_validated_in_the_modelled_shape() {
    let model: Value = serde_json::from_str(MODEL).expect("parse the Smithy model");
    let shapes = model["shapes"].as_object().expect("the model's shapes");

    // (operation name, method, uri) for every operation shape in the model, with each
    // `{Label}` of the URI filled with a placeholder the ARN patterns refuse — a label is
    // part of the route, so it has to carry something.
    let mut operations: BTreeSet<(String, String, String)> = BTreeSet::new();
    for (id, shape) in shapes {
        if shape.get("type").and_then(Value::as_str) != Some("operation") {
            continue;
        }
        let http = shape
            .get("traits")
            .and_then(|traits| traits.get("smithy.api#http"))
            .unwrap_or_else(|| panic!("operation {id} declares no @http binding"));
        let method = http["method"].as_str().expect("an @http method").to_string();
        let uri = http["uri"].as_str().expect("an @http uri");
        let filled: Vec<String> = uri
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "short".to_string()
                } else {
                    segment.to_string()
                }
            })
            .collect();
        let name = id.rsplit('#').next().unwrap_or(id).to_string();
        operations.insert((name, method, filled.join("/")));
    }
    assert!(
        operations.len() >= 34,
        "expected every modelled operation to be exercised, got {}",
        operations.len()
    );

    let em = Emulator::new();
    let mut refused = 0_usize;
    for (name, method, uri) in &operations {
        let (status, body) = em.call(method, uri, Some(&json!({})), &[]).await;
        if error_code(&body) != "ValidationException" {
            // The model accepts an empty body for this operation (or the request failed
            // for another reason entirely, which this rule says nothing about).
            continue;
        }
        refused += 1;
        assert_eq!(
            status, 400,
            "case {name:?}: a ValidationException is served on 400, got {status} with {body}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|message| message
                .starts_with("1 validation error detected: ")
                || message.contains(" validation errors detected: ")),
            "case {name:?}: expected the modelled Message in AWS's envelope, got {body}"
        );
        assert!(
            body.get("message").is_none(),
            "case {name:?}: the generic envelope's lower-case message means this operation is \
             not served behind the validation wrapper; got {body}"
        );
    }
    // The check is only worth as much as the number of operations it actually refused:
    // every operation with a @required member is one of them.
    assert!(
        refused >= 20,
        "expected an empty body to be refused by most operations, got {refused} refusals over \
         {} operations",
        operations.len()
    );
}
