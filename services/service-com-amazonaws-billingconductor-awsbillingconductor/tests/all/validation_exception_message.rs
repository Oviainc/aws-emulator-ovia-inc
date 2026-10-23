// SPDX-License-Identifier: CC0-1.0

//! The modelled `ValidationException` this service refuses a request with when a value
//! breaks a rule its model declares.
//!
//! RULE under test: a request that violates a constraint the model declares is refused
//! with the modelled `ValidationException` (`@httpError 400`), whose `@required Message`
//! is built FROM THE REQUEST — `<N> validation error(s) detected: ` over one
//! `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clause per
//! failing member, joined with `; ` and ordered by the member path. A violation found on
//! a list ELEMENT is stated at the list member, with the element's requirement nested
//! (`Member must satisfy constraint: [...]`) and the list itself as the value, so two bad
//! elements of one list are ONE error. A `@pattern` is stated without the `^`/`$`
//! anchors the model spells it with, and a `@sensitive` member is named with NO value.
//!
//! The rows of each table differ along what the rule generalises over: which operation,
//! which member, which constraint class (pattern, range, length, enum, required), how
//! many violations in one request, how many elements in a list, and whether the member
//! is `@sensitive`. The recorded case (`UpdateBillingGroup` with a non-conforming `Arn`)
//! is one row.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::ops::types::{
    BILLING_GROUP_STATUS_VALUES, PRICING_RULE_SCOPE_VALUES,
};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A live emulator holding only this service.
///
/// Local to this suite rather than shared with `billing_group_core_loop`: that suite's
/// harness only POSTs, and the batch operations under test here are modelled `PUT`.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — see the note on `billing_group_core_loop`'s
    /// harness for why a payer-gated service needs one designated, and
    /// `tests/all/payer_account_gate.rs` for the denial every other caller gets. The
    /// designation does not affect the refusals this suite is about: a modelled
    /// constraint is refused at the boundary AHEAD of that gate. It is what makes the
    /// rows that assert a CONFORMING request is served reachable.
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

    /// Send a JSON body to one of this service's routes with the method the model binds
    /// that route to, and return the status plus the parsed response body.
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

/// The modelled `Message` member of a response body, or `None` when the body carries
/// none at all — which is the defect this suite pins.
fn message(body: &Value) -> Option<&str> {
    body.get("Message").and_then(Value::as_str)
}

/// The `BillingGroupArn` pattern, as the model declares it and the message states it
/// (without Smithy's `^`/`$` anchors).
/// Source: smithy_query(query="operation", operation="UpdateBillingGroup") — member
/// `Arn`, target `BillingGroupArn`.
const BILLING_GROUP_ARN_PATTERN: &str =
    r"(arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}";

/// The `AccountId` pattern. Source: smithy_query(query="shape", shape="AccountIdList") —
/// the list's member targets `AccountId`.
const ACCOUNT_ID_PATTERN: &str = "[0-9]{12}";

/// The `CustomLineItemAssociationElement` pattern. Source: smithy_query(query="shape",
/// shape="CustomLineItemBatchAssociationsList") — the list's member.
const RESOURCE_ARN_PATTERN: &str =
    r"(arn:aws(-cn)?:billingconductor::[0-9]{12}:(customlineitem|billinggroup)/)?[a-zA-Z0-9]{10,12}";

/// The `BillingGroupName` / `CustomLineItemName` / `PricingPlanName` pattern — every one
/// of those shapes is `@sensitive`. Source: smithy_query(query="shape",
/// shape="BillingGroupName,CustomLineItemName,PricingPlanName").
const NAME_PATTERN: &str = r"[a-zA-Z0-9_\+=\.\-@]+";

/// The `BillingPeriod` pattern. Source: smithy_query(query="operation",
/// operation="ListBillingGroups") — member `BillingPeriod`.
const BILLING_PERIOD_PATTERN: &str = r"\d{4}-(0?[1-9]|1[012])";

/// A value that satisfies `BillingGroupArn` in its bare-id form, so a row can break ONE
/// member and leave the rest of the request conforming.
const VALID_GROUP_ID: &str = "ABCDEFGHIJ";

/// Is `message` one this rule produced — i.e. does it open with the count envelope?
fn is_constraint_message(message: &str) -> bool {
    message.starts_with("1 validation error detected: ")
        || (message.contains(" validation errors detected: ")
            && message
                .split_once(' ')
                .is_some_and(|(count, _)| count.parse::<u32>().is_ok()))
}

/// RULE: every constraint class, on every operation, is refused as 400
/// `ValidationException` whose `Message` names the member and the rule it broke.
#[tokio::test]
async fn a_constraint_violation_is_refused_with_the_required_message() {
    let em = Emulator::new();

    // (case, method, path, body, expected Message)
    let cases: Vec<(&str, &str, &str, Value, String)> = vec![
        (
            // The recorded case: fixture billingconductor-billing-group-arn-validation-errors, task#1.
            "UpdateBillingGroup with a non-conforming Arn",
            "POST",
            "/update-billing-group",
            json!({"Arn": "not-an-arn"}),
            format!(
                "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {BILLING_GROUP_ARN_PATTERN}"
            ),
        ),
        (
            "GetBillingGroupCostReport with the same non-conforming Arn",
            "POST",
            "/get-billing-group-cost-report",
            json!({"Arn": "not-an-arn"}),
            format!(
                "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {BILLING_GROUP_ARN_PATTERN}"
            ),
        ),
        (
            "DeleteBillingGroup with an Arn that is one character too short",
            "POST",
            "/delete-billing-group",
            json!({"Arn": "TOOSHORT9"}),
            format!(
                "1 validation error detected: Value 'TOOSHORT9' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {BILLING_GROUP_ARN_PATTERN}"
            ),
        ),
        (
            "AssociateAccounts with a bad account id and a bad Arn: two members, two errors",
            "POST",
            "/associate-accounts",
            json!({"Arn": "not-an-arn", "AccountIds": ["not-an-id"]}),
            format!(
                "2 validation errors detected: Value '[not-an-id]' at 'accountIds' failed to \
                 satisfy constraint: Member must satisfy constraint: [Member must satisfy \
                 regular expression pattern: {ACCOUNT_ID_PATTERN}]; Value 'not-an-arn' at \
                 'arn' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {BILLING_GROUP_ARN_PATTERN}"
            ),
        ),
        (
            // TWO bad elements of ONE list are ONE error, stated with the whole list.
            "DisassociateAccounts with two bad account ids and a bad Arn",
            "POST",
            "/disassociate-accounts",
            json!({"Arn": "not-an-arn", "AccountIds": ["not-an-id", "12345678901"]}),
            format!(
                "2 validation errors detected: Value '[not-an-id, 12345678901]' at 'accountIds' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy regular expression pattern: {ACCOUNT_ID_PATTERN}]; Value 'not-an-arn' \
                 at 'arn' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {BILLING_GROUP_ARN_PATTERN}"
            ),
        ),
        (
            // A conforming element beside a bad one: the clause states the list as sent.
            "AssociateAccounts with one good and one bad account id",
            "POST",
            "/associate-accounts",
            json!({"Arn": VALID_GROUP_ID, "AccountIds": ["111122223333", "not-an-id"]}),
            format!(
                "1 validation error detected: Value '[111122223333, not-an-id]' at 'accountIds' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy regular expression pattern: {ACCOUNT_ID_PATTERN}]"
            ),
        ),
        (
            // fixture billingconductor-batch-resource-association-validation-errors, task#1.
            "BatchAssociateResourcesToCustomLineItem with a non-billingconductor resource ARN",
            "PUT",
            "/batch-associate-resources-to-custom-line-item",
            json!({
                "TargetArn": VALID_GROUP_ID,
                "ResourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            format!(
                "1 validation error detected: Value \
                 '[arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0]' at \
                 'resourceArns' failed to satisfy constraint: Member must satisfy constraint: \
                 [Member must satisfy regular expression pattern: {RESOURCE_ARN_PATTERN}]"
            ),
        ),
        (
            "BatchDisassociateResourcesFromCustomLineItem with the same resource ARN",
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            json!({
                "TargetArn": VALID_GROUP_ID,
                "ResourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            format!(
                "1 validation error detected: Value \
                 '[arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0]' at \
                 'resourceArns' failed to satisfy constraint: Member must satisfy constraint: \
                 [Member must satisfy regular expression pattern: {RESOURCE_ARN_PATTERN}]"
            ),
        ),
        (
            // A member the shape requires, absent: found while READING the request, stated
            // the same way.
            "UpdateBillingGroup with no Arn at all",
            "POST",
            "/update-billing-group",
            json!({}),
            "1 validation error detected: Value null at 'arn' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            // A @range member below its published minimum.
            "ListBillingGroups with MaxResults below the modelled minimum",
            "POST",
            "/list-billing-groups",
            json!({"MaxResults": 0}),
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
             constraint: Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            // …and above its published maximum.
            "ListBillingGroups with MaxResults above the modelled maximum",
            "POST",
            "/list-billing-groups",
            json!({"MaxResults": 101}),
            "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
             constraint: Member must have value less than or equal to 100"
                .to_string(),
        ),
        (
            // A pattern on another member of another operation entirely.
            "ListBillingGroups with a BillingPeriod that is not YYYY-MM",
            "POST",
            "/list-billing-groups",
            json!({"BillingPeriod": "2026-13"}),
            format!(
                "1 validation error detected: Value '2026-13' at 'billingPeriod' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 {BILLING_PERIOD_PATTERN}"
            ),
        ),
    ];

    for (case, method, path, body, expected) in cases {
        let (status, response) = em.send(method, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {response:?}"
        );
        assert_eq!(
            message(&response),
            Some(expected.as_str()),
            "case {case:?}: expected the Message {expected:?}, got {response:?}"
        );
        assert_eq!(
            response.get("Reason"),
            None,
            "case {case:?}: a constraint refusal names no modelled Reason, got {response:?}"
        );
    }
}

/// One row of the enumerated-member table: the case name, the HTTP method, the path,
/// the request body, the member path the clause must name, and the value set the model
/// declares for that member.
type EnumCase<'a> = (&'a str, &'a str, &'a str, Value, &'a str, &'a [&'a str]);

/// RULE: a member outside an enumerated type's value set is refused with that member's
/// WHOLE value set — the values the model declares, in the order it declares them.
#[tokio::test]
async fn an_enumerated_member_is_refused_with_its_value_set() {
    let em = Emulator::new();

    // Rows differ along the operation and the enumerated member. The expected values
    // come from the same model the constraint does (`<Enum>_VALUES`, generated into this
    // crate's `ops::types`), so the row cannot drift from the model; the ORDER is the
    // model's own declaration order, which those sorted constants do not carry, so the
    // sets are compared as sets.
    let cases: Vec<EnumCase<'_>> = vec![
        (
            "UpdateBillingGroup with a Status outside the enumerated set",
            "POST",
            "/update-billing-group",
            json!({"Arn": VALID_GROUP_ID, "Status": "NOT_A_STATUS"}),
            "status",
            BILLING_GROUP_STATUS_VALUES,
        ),
        (
            "CreatePricingRule with a Scope outside the enumerated set",
            "POST",
            "/create-pricing-rule",
            json!({"Name": "ScopeRule", "Scope": "NOT_A_SCOPE", "Type": "MARKUP"}),
            "scope",
            PRICING_RULE_SCOPE_VALUES,
        ),
    ];

    for (case, method, path, body, member, values) in cases {
        let (status, response) = em.send(method, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response:?}"
        );
        let text = message(&response).unwrap_or_default();
        let opening = format!(
            "1 validation error detected: Value 'NOT_A_{}' at '{member}' failed to satisfy \
             constraint: Member must satisfy enum value set: [",
            member.to_uppercase()
        );
        assert!(
            text.starts_with(&opening) && text.ends_with(']'),
            "case {case:?}: expected a clause opening {opening:?}, got {text:?}"
        );
        let mut reported: Vec<&str> = text
            .trim_start_matches(&opening)
            .trim_end_matches(']')
            .split(", ")
            .collect();
        reported.sort_unstable();
        let mut expected = values.to_vec();
        expected.sort_unstable();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the modelled value set {expected:?}, got {reported:?} in \
             {text:?}"
        );
    }
}

/// RULE: a `@sensitive` member is named with NO value — the constraint it broke is the
/// whole of what the clause states, so caller input never comes back in the body.
#[tokio::test]
async fn a_sensitive_member_is_named_without_its_value() {
    let em = Emulator::new();

    // Rows differ along the operation and the @sensitive member: a Name whose pattern
    // fails, and a Description whose length does.
    let secret = "a name nobody may log!";
    let long_secret = "x".repeat(2000);
    let cases: Vec<(&str, &str, &str, Value, String, &str)> = vec![
        (
            "UpdateBillingGroup with a Name that breaks its pattern",
            "POST",
            "/update-billing-group",
            json!({"Arn": VALID_GROUP_ID, "Name": secret}),
            format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {NAME_PATTERN}"
            ),
            secret,
        ),
        (
            "UpdatePricingPlan with a Name that breaks its pattern",
            "PUT",
            "/update-pricing-plan",
            json!({"Arn": "BasicPricingPlan", "Name": secret}),
            format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {NAME_PATTERN}"
            ),
            secret,
        ),
        (
            "UpdateBillingGroup with a Description over its modelled length",
            "POST",
            "/update-billing-group",
            json!({"Arn": VALID_GROUP_ID, "Description": long_secret.clone()}),
            "1 validation error detected: Value at 'description' failed to satisfy constraint: \
             Member must have length less than or equal to 1024"
                .to_string(),
            long_secret.as_str(),
        ),
    ];

    for (case, method, path, body, expected, hidden) in cases {
        let (status, response) = em.send(method, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response:?}"
        );
        assert_eq!(
            message(&response),
            Some(expected.as_str()),
            "case {case:?}: expected the Message {expected:?}, got {response:?}"
        );
        let rendered = response.to_string();
        assert!(
            !rendered.contains(hidden),
            "case {case:?}: the body must not echo a @sensitive value, got {rendered:?}"
        );
    }
}

/// RULE: the value a clause echoes is bounded, however long the list a caller sent —
/// one request can never amplify into an arbitrarily long 400 body.
#[tokio::test]
async fn the_echoed_list_value_is_bounded() {
    let em = Emulator::new();

    // Rows differ along how far past the modelled maximum (30 elements) the list goes.
    for count in [31_usize, 60, 120] {
        let case = format!("AccountIds with {count} elements");
        let accounts: Vec<String> = (0..count).map(|index| format!("{index:012}")).collect();
        let (status, response) = em
            .send(
                "POST",
                "/associate-accounts",
                &json!({"Arn": VALID_GROUP_ID, "AccountIds": accounts}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response:?}"
        );
        let text = message(&response).unwrap_or_default();
        assert!(
            text.starts_with("1 validation error detected: Value '[")
                && text.ends_with(
                    "at 'accountIds' failed to satisfy constraint: Member must have length less \
                     than or equal to 30"
                ),
            "case {case:?}: expected the collection-length clause, got {text:?}"
        );
        // The whole clause stays within one screenful whatever the request held: the
        // echoed value is elided, and says how long the original was.
        assert!(
            text.len() < 512 && text.contains("... ["),
            "case {case:?}: expected a bounded, elided value, got a {}-byte Message {text:?}",
            text.len()
        );
    }
}

/// RULE: a request the model ACCEPTS is not refused — the boundary states only what the
/// model's own constraints reject, and everything else is served by the operation.
#[tokio::test]
async fn a_request_the_model_accepts_is_not_refused() {
    let em = Emulator::new();

    // Rows differ along the operation and the member that is being driven to its
    // modelled bound: both published ends of MaxResults, a bare-id ARN, an empty list,
    // and a conforming @sensitive Name.
    let accepted: Vec<(&str, &str, &str, Value)> = vec![
        (
            "ListBillingGroups at the minimum MaxResults",
            "POST",
            "/list-billing-groups",
            json!({"MaxResults": 1}),
        ),
        (
            "ListBillingGroups at the maximum MaxResults",
            "POST",
            "/list-billing-groups",
            json!({"MaxResults": 100}),
        ),
        (
            "CreatePricingPlan with a conforming @sensitive Name",
            "POST",
            "/create-pricing-plan",
            json!({"Name": "Accepted_Plan.1"}),
        ),
        (
            "UpdateBillingGroup with a bare-id Arn the pattern allows",
            "POST",
            "/update-billing-group",
            json!({"Arn": VALID_GROUP_ID}),
        ),
        (
            "AssociateAccounts with the empty list the modelled minimum allows",
            "POST",
            "/associate-accounts",
            json!({"Arn": VALID_GROUP_ID, "AccountIds": []}),
        ),
    ];

    for (case, method, path, body) in accepted {
        let (status, response) = em.send(method, path, &body).await;
        let text = message(&response).unwrap_or_default();
        assert!(
            !is_constraint_message(text),
            "case {case:?}: the model accepts this request, so no constraint refusal may be \
             returned; got {status} with {response:?}"
        );
    }

    // And the happy path still answers from state: an implemented operation serves a
    // conforming request exactly as before.
    let (status, response) = em
        .send("POST", "/create-pricing-plan", &json!({"Name": "LivePlan"}))
        .await;
    assert_eq!(
        status, 200,
        "CreatePricingPlan with a conforming input: expected HTTP 200, got {status} with \
         {response:?}"
    );
}
