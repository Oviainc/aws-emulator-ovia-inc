// SPDX-License-Identifier: CC0-1.0

//! The ARN a refusal of this service REPORTS BACK, driven end to end over this service's
//! wire protocol: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson
//! body through the same `MultiServiceRouter` the production router uses, so what these
//! tests read is what an SDK client reads.
//!
//! The rule under test: a value this service reports in a modelled member is one that
//! member can hold. Every member of every error shape of this model that names the
//! resource an operation was asked about targets `InvoiceUnitArnString`,
//! `@length(min: 1, max: 256)` (`smithy_query(query="shape",
//! shape="ResourceNotFoundException,InvoiceUnitArnString")`), while the tag plane accepts
//! its `ResourceArn` as a `TagrisArn`, `@length(min: 20, max: 2048)`
//! (`smithy_query(query="shape", shape="TagrisArn")`) — so one legal request can name a
//! resource eight times longer than the member reporting it. An ARN that member cannot
//! hold leaves it ABSENT: an identifier is not prose, so unlike the explanatory text
//! (`request_derived_refusal_message_bound.rs`) it is never cut down to the bound, because
//! a cut-down ARN names a different resource. An ARN the member CAN hold is reported
//! whole, which is what every plane whose own input the model already caps at 256
//! answers.
//!
//! The rows span the LENGTH the rule generalises over — the shortest ARN each plane's
//! input admits, both sides of the reporting member's maximum, and that input's own
//! maximum — crossed with the operations of the tag plane, which all three reach the same
//! refusal.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The longest ARN a member of this model that reports one may carry: every such member
/// targets `InvoiceUnitArnString`, `@length(min: 1, max: 256)`, counted in CHARACTERS.
const MAX_REPORTED_ARN_LENGTH: usize = 256;

/// The longest `ResourceArn` the tag plane ACCEPTS: `TagrisArn`, `@length(min: 20, max:
/// 2048)` — eight times the bound of the member that reports it back.
const MAX_TAG_PLANE_ARN_LENGTH: usize = 2048;

/// The message a `ResourceNotFoundException` of the invoice-unit and tag planes carries:
/// the sentence the shape documents (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), whatever was asked for.
const NOT_FOUND_MESSAGE: &str = "The resource could not be found.";

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200 from {operation}, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a backend stamps in front of it.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text of a failure body, under either of the keys this service's planes
/// write it under.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN a refusal reports, if it reports one: the model's `resourceName`, which every
/// recorded body of this service keys `resourceId`.
fn reported_arn(body: &Value) -> Option<String> {
    body.get("resourceId")
        .map(|value| value.as_str().unwrap_or_default().to_string())
}

/// The emulator's own account id, which every ARN of this service names.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// An ARN of exactly `length` characters starting at `prefix`, padded in the resource
/// segment with a character every ARN pattern of this model admits (`[-a-zA-Z0-9/:_]`).
fn padded_arn(prefix: &str, length: usize) -> String {
    assert!(
        length > prefix.len(),
        "case {prefix}: a {length}-character ARN has no room for its own prefix"
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// A `ResourceArn` of exactly `length` characters in the form `TagrisArn`'s `@pattern`
/// spells (`^arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+$`), naming no
/// resource this service holds.
fn tag_plane_arn(length: usize) -> String {
    padded_arn(&format!("arn:aws:invoicing::{}:", account()), length)
}

/// An `InvoiceUnitArn` of exactly `length` characters, naming no unit this service holds.
fn invoice_unit_arn(length: usize) -> String {
    padded_arn(
        &format!("arn:aws:invoicing::{}:invoice-unit/", account()),
        length,
    )
}

/// A `ProcurementPortalPreferenceArn` of exactly `length` characters, in the form that
/// shape's `@pattern` spells, naming no preference this service holds.
fn preference_arn(length: usize) -> String {
    padded_arn(
        &format!(
            "arn:aws:invoicing::{}:procurement-portal-preference/",
            account()
        ),
        length,
    )
}

/// The requests of the tag plane that name one resource by ARN: every operation of that
/// plane, so the rule is asserted over the plane rather than over the one reported
/// operation.
fn tag_plane_requests(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({"ResourceArn": arn})),
        (
            "TagResource",
            json!({"ResourceArn": arn, "ResourceTags": [{"Key": "k", "Value": "v"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn, "ResourceTagKeys": ["k"]}),
        ),
    ]
}

/// RULE: the tag plane's 404 reports the ARN it was asked about only while that ARN is one
/// the reporting member holds — at most [`MAX_REPORTED_ARN_LENGTH`] characters — and
/// leaves the member absent for a longer one, rather than answering a value the model says
/// it never holds. The status, the error code and the documented sentence are the same for
/// every row: how long the ARN is decides what is REPORTED, never what is refused.
#[tokio::test]
async fn the_tag_plane_reports_an_arn_only_while_the_member_can_hold_it() {
    let router = emulator();

    // Lengths spanning what the rule generalises over: the shortest ARN of this form, a
    // middling one, both sides of the reporting member's maximum, the length of the
    // reported case, and `TagrisArn`'s own maximum.
    let shortest = format!("arn:aws:invoicing::{}:", account()).chars().count() + 1;
    let lengths = [
        shortest,
        100,
        MAX_REPORTED_ARN_LENGTH - 1,
        MAX_REPORTED_ARN_LENGTH,
        MAX_REPORTED_ARN_LENGTH + 1,
        MAX_REPORTED_ARN_LENGTH + 2,
        332,
        1024,
        MAX_TAG_PLANE_ARN_LENGTH,
    ];

    for length in lengths {
        let arn = tag_plane_arn(length);
        assert_eq!(
            arn.chars().count(),
            length,
            "case {length} characters: the fixture ARN is the length the row asks for, \
             got {actual}",
            actual = arn.chars().count()
        );
        for (operation, request) in tag_plane_requests(&arn) {
            let case = format!("{operation} on a {length}-character ARN");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: expected a ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                NOT_FOUND_MESSAGE,
                "case {case}: expected the documented sentence, got {body}"
            );

            let reported = reported_arn(&body);
            if length <= MAX_REPORTED_ARN_LENGTH {
                assert_eq!(
                    reported.as_deref(),
                    Some(arn.as_str()),
                    "case {case}: an ARN the member holds is reported whole, got {reported:?}"
                );
            } else {
                assert_eq!(
                    reported, None,
                    "case {case}: an ARN past {MAX_REPORTED_ARN_LENGTH} characters leaves the \
                     member absent, got {reported:?}"
                );
            }
            // The invariant behind both branches, stated on whatever was reported.
            let stated = reported.as_deref().map(|value| value.chars().count());
            assert!(
                stated.is_none_or(|count| (1..=MAX_REPORTED_ARN_LENGTH).contains(&count)),
                "case {case}: a reported ARN is within 1..={MAX_REPORTED_ARN_LENGTH} \
                 characters, got {stated:?}"
            );
        }
    }

    // The control: holding the reported member to its bound changes nothing about WHICH
    // resources resolve — a resource this service holds is still served, with its tags.
    let created = ok(
        &router,
        "a resource that exists",
        "CreateInvoiceUnit",
        &json!({
            "Name": "ReportedArnUnit",
            "InvoiceReceiver": "123456789012",
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": ["210987654321"]},
            "ResourceTags": [{"Key": "k", "Value": "v"}],
        }),
    )
    .await;
    let minted = created["InvoiceUnitArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn, got {created}"))
        .to_string();
    let listed = ok(
        &router,
        "a resource that exists",
        "ListTagsForResource",
        &json!({"ResourceArn": minted}),
    )
    .await;
    assert_eq!(
        listed["ResourceTags"],
        json!([{"Key": "k", "Value": "v"}]),
        "case a resource that exists: expected the tags it was created with, got {listed}"
    );
}

/// One row of the already-bounded table: an operation whose own ARN input the model caps
/// at [`MAX_REPORTED_ARN_LENGTH`], so every ARN it accepts is one the reporting member
/// holds and is reported WHOLE — the bound removes nothing a client could otherwise read.
struct BoundedInputCase {
    case: String,
    operation: &'static str,
    arn: String,
    request: Value,
}

/// RULE, on the planes the model already bounds: the same reporting rule reports their
/// ARNs whole, at the longest value their own input admits. These rows are what makes the
/// rule a bound on the REPORTED member rather than a cap on what any plane may name.
#[tokio::test]
async fn a_plane_whose_input_is_already_bounded_reports_its_arn_whole() {
    let router = emulator();

    let mut cases = Vec::new();
    for length in [MAX_REPORTED_ARN_LENGTH - 1, MAX_REPORTED_ARN_LENGTH] {
        let arn = invoice_unit_arn(length);
        cases.push(BoundedInputCase {
            case: format!("GetInvoiceUnit on a {length}-character ARN"),
            operation: "GetInvoiceUnit",
            arn: arn.clone(),
            request: json!({"InvoiceUnitArn": arn}),
        });
        cases.push(BoundedInputCase {
            case: format!("UpdateInvoiceUnit on a {length}-character ARN"),
            operation: "UpdateInvoiceUnit",
            arn: arn.clone(),
            request: json!({"InvoiceUnitArn": arn, "Description": "nope"}),
        });
        cases.push(BoundedInputCase {
            case: format!("DeleteInvoiceUnit on a {length}-character ARN"),
            operation: "DeleteInvoiceUnit",
            arn: arn.clone(),
            request: json!({"InvoiceUnitArn": arn}),
        });

        let preference = preference_arn(length);
        cases.push(BoundedInputCase {
            case: format!("GetProcurementPortalPreference on a {length}-character ARN"),
            operation: "GetProcurementPortalPreference",
            arn: preference.clone(),
            request: json!({"ProcurementPortalPreferenceArn": preference}),
        });
        cases.push(BoundedInputCase {
            case: format!("DeleteProcurementPortalPreference on a {length}-character ARN"),
            operation: "DeleteProcurementPortalPreference",
            arn: preference.clone(),
            request: json!({"ProcurementPortalPreferenceArn": preference}),
        });
    }

    for case in cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 404,
            "case {}: expected HTTP 404, got {status} {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {}: expected a ResourceNotFoundException, got {body}",
            case.case
        );
        let reported = reported_arn(&body);
        assert_eq!(
            reported.as_deref(),
            Some(case.arn.as_str()),
            "case {}: an ARN the member holds is reported whole, got {reported:?}",
            case.case
        );
    }
}
