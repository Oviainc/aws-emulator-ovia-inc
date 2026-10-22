// SPDX-License-Identifier: CC0-1.0

//! The refusals the custom-line-item ARN operations and the create-name operations owe:
//! the eight recorded steps of
//! `billingconductor-custom-line-item-arn-validation-errors` and
//! `billingconductor-create-operations-name-validation`, as a rule over a table.
//!
//! RULE: a request whose member breaks a `@pattern` the model declares is refused with
//! the modelled `ValidationException` (`@httpError 400`) whose `@required Message` is
//! built FROM THE REQUEST — `1 validation error detected: <subject> at '<member path>'
//! failed to satisfy constraint: Member must satisfy regular expression pattern:
//! <the member's @pattern without the ^/$ anchors>` — and the subject names the value
//! the caller sent EXCEPT where the member's target shape is `@sensitive`, in which case
//! the clause degrades to `Value at '<member path>'`.
//!
//! The rows differ along the operation (all four custom-line-item operations that take a
//! `CustomLineItemArn`, and all four `Create*` operations that take a `@sensitive`
//! resource name), the member, and the way the value breaks the pattern (wrong shape, too
//! short, too long, empty, an illegal character). The recorded `Arn="not-an-arn"` and the
//! recorded bad names are one row each.
//!
//! `validation_exception_message.rs` asserts the same rule across every constraint class;
//! this suite is the regression cover for the eight recorded steps in particular.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `CustomLineItemArn`'s `@pattern` as the model declares it, minus the `^`/`$` anchors
/// (`smithy_query(query="shape",shape="CustomLineItemArn")`).
const CUSTOM_LINE_ITEM_ARN_PATTERN: &str =
    "(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}";

/// The `@pattern` shared by `BillingGroupName`, `PricingPlanName`, `PricingRuleName` and
/// `CustomLineItemName` — all four `@sensitive` — minus its anchors
/// (`smithy_query(query="shape",shape="BillingGroupName,PricingPlanName,
/// PricingRuleName,CustomLineItemName")`).
const RESOURCE_NAME_PATTERN: &str = r"[a-zA-Z0-9_\+=\.\-@]+";

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

    /// POST a JSON body to one of this service's routes and return the status, the parsed
    /// body, and the `x-amzn-errortype` header restJson1 resolves an error shape from.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value, Option<String>) {
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
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed, error_type)
    }
}

/// Assert one refusal: HTTP 400, the modelled error code in the body and the header, and
/// the whole `Message` the construction rule produces — under the member name the model
/// declares `@required`, never the framework's lowercase `message`.
async fn assert_refused(
    emulator: &Emulator,
    label: &str,
    path: &str,
    body: &Value,
    expected: &str,
    absent: Option<&str>,
) {
    let (status, answer, error_type) = emulator.post(path, body).await;
    assert_eq!(
        status, 400,
        "case {label:?}: expected HTTP 400, got {status} with {answer}"
    );
    assert_eq!(
        answer.get("__type").and_then(Value::as_str),
        Some("ValidationException"),
        "case {label:?}: expected the __type to name the modelled error shape, got {answer}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("ValidationException"),
        "case {label:?}: expected x-amzn-errortype to name the modelled error shape, got \
         {error_type:?}"
    );
    let stated = answer.get("Message").and_then(Value::as_str);
    assert_eq!(
        stated,
        Some(expected),
        "case {label:?}: expected the @required Message member to state {expected:?}, got {answer}"
    );
    let stated = stated.unwrap_or_default();
    assert!(
        answer.get("message").is_none(),
        "case {label:?}: the prose belongs in the modelled `Message` member only, got {answer}"
    );
    assert!(
        !stated.contains('^') && !stated.contains('$'),
        "case {label:?}: a reported @pattern carries no anchors, got {stated:?}"
    );
    if let Some(absent) = absent {
        assert!(
            !stated.contains(absent),
            "case {label:?}: a @sensitive member's value must not be quoted back, got {stated:?}"
        );
    }
}

/// The four custom-line-item operations that take a `CustomLineItemArn`: the ARN is not
/// `@sensitive`, so the clause QUOTES the value the caller sent.
#[tokio::test]
async fn a_non_conforming_custom_line_item_arn_is_refused_with_the_value_and_the_pattern() {
    // Every route the model binds to an operation whose only `@required` member is
    // `Arn: CustomLineItemArn` (`smithy_query(query="operation",operation=
    // "UpdateCustomLineItem,DeleteCustomLineItem,ListCustomLineItemVersions,
    // ListResourcesAssociatedToCustomLineItem")`).
    let routes = [
        ("UpdateCustomLineItem", "/update-custom-line-item"),
        ("DeleteCustomLineItem", "/delete-custom-line-item"),
        (
            "ListCustomLineItemVersions",
            "/list-custom-line-item-versions",
        ),
        (
            "ListResourcesAssociatedToCustomLineItem",
            "/list-resources-associated-to-custom-line-item",
        ),
    ];
    // The ways a value breaks the pattern: the recorded one, a bare id too short, a bare
    // id too long, an illegal character in a bare id, a full ARN of the wrong resource
    // type, and the empty string.
    let rejected = [
        "not-an-arn",
        "ABC123",
        "ABCDEFGHIJKL",
        "ABCDEFGHI_",
        "arn:aws:billingconductor::123456789012:billinggroup/ABCDEFGHIJ",
        "",
    ];

    let emulator = Emulator::new();
    for (operation, path) in routes {
        for arn in rejected {
            let expected = format!(
                "1 validation error detected: Value '{arn}' at 'arn' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {CUSTOM_LINE_ITEM_ARN_PATTERN}"
            );
            assert_refused(
                &emulator,
                &format!("{operation} with Arn={arn:?}"),
                path,
                &json!({ "Arn": arn }),
                &expected,
                None,
            )
            .await;
        }
    }
}

/// The four create operations whose `Name` targets a `@sensitive` shape: the clause names
/// the member and the pattern and NOTHING of the name the caller sent.
#[tokio::test]
async fn a_non_conforming_resource_name_is_refused_without_echoing_the_name() {
    // The names break the pattern in a different way per row: a space, a slash, a colon,
    // a percent sign, a quote (which would also have to be escaped in the JSON body) and
    // a non-ASCII letter.
    let rejected = ["bad name", "plan/one", "rule:one", "item%one", "o'brien", "naïve"];

    let emulator = Emulator::new();
    for name in rejected {
        // Each body satisfies every OTHER `@required` member of its operation, so `Name`
        // is the only member outside its constraints.
        let bodies: [(&str, &str, Value); 4] = [
            (
                "CreateBillingGroup",
                "/create-billing-group",
                json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": {
                        "PricingPlanArn":
                            "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan"
                    },
                }),
            ),
            (
                "CreatePricingPlan",
                "/create-pricing-plan",
                json!({ "Name": name }),
            ),
            (
                "CreatePricingRule",
                "/create-pricing-rule",
                json!({ "Name": name, "Scope": "GLOBAL", "Type": "MARKUP" }),
            ),
            (
                "CreateCustomLineItem",
                "/create-custom-line-item",
                json!({
                    "Name": name,
                    "Description": "a custom line item",
                    "BillingGroupArn": "BG0123456789",
                    "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
                }),
            ),
        ];
        let expected = format!(
            "1 validation error detected: Value at 'name' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {RESOURCE_NAME_PATTERN}"
        );
        for (operation, path, body) in bodies {
            assert_refused(
                &emulator,
                &format!("{operation} with Name={name:?}"),
                path,
                &body,
                &expected,
                Some(name),
            )
            .await;
        }
    }
}

/// A conforming value on the same members is NOT refused: the rule rejects what the
/// pattern rejects and nothing else. These operations are still scaffolded, so what
/// matters is that the answer is not a constraint refusal — asserted on the error code
/// rather than only on the status, so a 500 could not pass for an acceptance.
#[tokio::test]
async fn a_conforming_arn_or_name_is_not_refused() {
    let emulator = Emulator::new();
    // Both forms the `CustomLineItemArn` pattern admits: the bare 10-character id and the
    // full ARN.
    for arn in [
        "ABCDEFGHIJ",
        "arn:aws:billingconductor::123456789012:customlineitem/ABCDEFGHIJ",
    ] {
        let (status, body, error_type) = emulator
            .post("/update-custom-line-item", &json!({ "Arn": arn }))
            .await;
        assert_not_a_refusal(&format!("Arn={arn:?}"), status, &body, error_type.as_deref());
    }
    // Every character class the resource-name pattern admits.
    for name in ["plan", "Plan_1", "plan+one", "plan=one", "plan.one", "plan-one", "plan@one"] {
        let (status, body, error_type) = emulator
            .post("/create-pricing-plan", &json!({ "Name": name }))
            .await;
        assert_not_a_refusal(
            &format!("Name={name:?}"),
            status,
            &body,
            error_type.as_deref(),
        );
    }
}

/// A request the pattern admits is answered by whoever serves the operation, never by the
/// constraint refusal: neither the status nor the error code of one.
fn assert_not_a_refusal(label: &str, status: u16, body: &Value, error_type: Option<&str>) {
    assert_ne!(
        status, 400,
        "case {label}: the pattern admits this value, so it must not be refused, got {status} \
         with {body}"
    );
    assert_ne!(
        body.get("__type").and_then(Value::as_str),
        Some("ValidationException"),
        "case {label}: the pattern admits this value, so no constraint refusal may be \
         returned, got {status} with {body}"
    );
    assert_ne!(
        error_type,
        Some("ValidationException"),
        "case {label}: the pattern admits this value, so no constraint refusal may be \
         returned, got {status} with {body}"
    );
}

/// The clause for a LIST member is bounded however long the list the caller sent: one
/// request cannot amplify into an arbitrarily long 400 body, and the message still says
/// how long the value really was.
///
/// The rows differ along the number of elements and the length of each, so the bound
/// holds for a long list of short values and a short list of long ones alike. Every
/// element breaks `CustomLineItemArn`'s pattern, so the walk reports one violation per
/// element and they are stated as the ONE list member they belong to.
#[tokio::test]
async fn the_clause_for_a_list_member_is_bounded() {
    let emulator = Emulator::new();
    for (count, width) in [(40_usize, 300_usize), (4, 4096), (120, 12)] {
        let label = format!("{count} elements of {width} characters");
        let elements: Vec<String> = (0..count).map(|_| "z".repeat(width)).collect();
        let (status, body, _) = emulator
            .post(
                "/list-custom-line-items",
                &json!({ "Filters": { "Arns": elements } }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} with {body}"
        );
        let stated = body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            stated.starts_with("1 validation error detected: Value '[")
                && stated.contains("at 'filters.arns' failed to satisfy constraint:"),
            "case {label}: expected one clause stated at the list member, got {stated:?}"
        );
        // The untruncated rendering would be `[` + the elements joined by `, ` + `]`.
        // What stays is the echoed value's 256-byte prefix plus the clause's own fixed
        // text (the member path and the requirement, both model-sized).
        let total = 2 + count * width + 2 * (count - 1);
        assert!(
            stated.len() < 1024 && stated.contains(&format!("... [{total} bytes]")),
            "case {label}: expected a bounded value reporting {total} bytes, got a {}-byte \
             Message {stated:?}",
            stated.len()
        );
    }

    // A list whose FIRST element renders empty: short enough to be stated whole, so the
    // separator accounting is visible in the clause rather than only in a reported length.
    let (status, body, _) = emulator
        .post(
            "/list-custom-line-items",
            &json!({ "Filters": { "Arns": ["", "x"] } }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an empty first element: expected HTTP 400, got {status} with {body}"
    );
    let stated = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        stated.starts_with("1 validation error detected: Value '[, x]' at 'filters.arns' "),
        "case an empty first element: expected the list stated as the caller sent it, got \
         {stated:?}"
    );
}
