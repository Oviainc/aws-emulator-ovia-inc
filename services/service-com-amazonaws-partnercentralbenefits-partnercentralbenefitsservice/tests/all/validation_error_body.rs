// SPDX-License-Identifier: CC0-1.0

//! The BODY a refused request is answered with, over the wire.
//!
//! The rule: a request that violates a modelled input constraint is refused with HTTP 400
//! in this service's OWN modelled `ValidationException` shape, so the body carries that
//! shape's members under the names the model declares — `Message`, stating the AWS
//! validation envelope over one clause per violation, and the `Reason` the model marks
//! @required beside it — and never the framework's generic refusal envelope, whose only
//! member is a lowercase `message`.
//!
//! Model evidence (`smithy_query(query="shape",shape="ValidationException",
//! model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")`):
//! `ValidationException { Message @required, Reason @required (ValidationExceptionReason),
//! FieldList }` with `@error("client")` and `@httpError(400)`. awsJson1_0 keys a member by
//! its Smithy member name, so `Message` is the key — a required member that is never
//! absent and never lowercase.
//!
//! The tables below span the class the rule covers rather than the calls the issue named:
//! EVERY operation the service declares (an input constraint is a shape fact, so the
//! operation a refused request happened to name changes nothing), every constraint class
//! the model puts on an input member (`@pattern`, `@length` on a string, `@length` on a
//! list, `@range`, `@required`, and a constraint on a member nested inside a list), and
//! both the singular and the plural envelope. The five calls this issue reports — the
//! tagging operations' `resourceArn`, `ListBenefitApplications$Stages`,
//! `GetBenefit$Identifier` and `GetBenefitAllocation$Identifier` — are single rows among
//! them.
//!
//! The wording of the message is pinned in `tests/all/service_error_rules.rs`; what these
//! tests pin is the SHAPE of the body that carries it.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::{
    FULFILLMENT_TYPE_VALUES, VALIDATION_EXCEPTION_REASON_VALUES,
};

use crate::benefit_application_lifecycle::{error_code, Harness, CATALOG};
use crate::service_error_rules::{
    assert_validation_invariants, clause, message_of, one, pattern_clause, taggable_arn,
    well_formed_requests, ALLOCATION_IDENTIFIER_PATTERN, APPLICATION_IDENTIFIER_PATTERN,
    ARN_PATTERN, BENEFIT_IDENTIFIER_PATTERN, CLIENT_TOKEN, TAGGABLE_ARN_PATTERN,
    UNKNOWN_APPLICATION, UNKNOWN_BENEFIT,
};

/// The `ValidationExceptionReason` a refusal of one input member states. A member of the
/// model's own `ValidationExceptionReason` enum (asserted against the model-derived
/// `VALIDATION_EXCEPTION_REASON_VALUES` below), and the one that names what happened: a
/// field failed validation.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// A value no modelled identifier `@pattern` admits, so it violates every one of them.
const MALFORMED: &str = "not-a-valid-identifier!!";

/// Call `operation`, require HTTP 400 `ValidationException`, and return the whole body —
/// the body is what these tests are about, so nothing of it is dropped on the way back.
async fn refused_body(harness: &Harness, case: &str, operation: &str, body: &Value) -> Value {
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {response:?}"
    );
    response
}

/// Every member the model requires on `ValidationException`, present under its modelled
/// name — and the framework's lowercase `message` nowhere, which is the only way the
/// required `Message` can go missing.
fn assert_modelled_exception(case: &str, response: &Value, expected_message: &str) {
    let message = message_of(response).unwrap_or_else(|| {
        panic!("case {case:?}: the body carries no modelled Message member: {response:?}")
    });
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );
    assert_validation_invariants(case, &message);
    assert!(
        response.get("message").is_none(),
        "case {case:?}: the body spells the message under the framework's generic \
         lowercase key: {response:?}"
    );
    let reason = response
        .get("Reason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the body carries no modelled Reason member: {response:?}")
        });
    assert_eq!(
        reason, REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: expected the reason {REASON_FIELD_VALIDATION_FAILED:?}, got \
         {reason:?}"
    );
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?}: {reason:?} is not a value the modelled ValidationExceptionReason \
         enum declares ({VALIDATION_EXCEPTION_REASON_VALUES:?})"
    );
}

/// One refused request per operation the service declares, drawn from every constraint
/// class the model puts on an input member: each is answered in the modelled
/// `ValidationException`, carrying `Message` and `Reason` under their modelled names.
#[tokio::test]
async fn every_operation_states_a_refused_input_in_the_modelled_exception() {
    let harness = Harness::new();
    // (case, operation, request body, the Message the refusal must carry)
    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            // The recorded instance of cause 1: UntagResource's resourceArn.
            "UntagResource with a resourceArn outside the catalog-ARN pattern".to_string(),
            "UntagResource",
            json!({ "resourceArn": "not-an-arn", "tagKeys": ["probe"] }),
            one(&pattern_clause("resourceArn", TAGGABLE_ARN_PATTERN)),
        ),
        (
            // Cause 5: the same member on TagResource.
            "TagResource with a resourceArn of another service".to_string(),
            "TagResource",
            json!({
                "resourceArn": "arn:aws:s3:::not-a-partnercentral-resource",
                "tags": [{ "Key": "probe", "Value": "body" }],
            }),
            one(&pattern_clause("resourceArn", TAGGABLE_ARN_PATTERN)),
        ),
        (
            "ListTagsForResource with a resourceArn that is not an ARN at all".to_string(),
            "ListTagsForResource",
            json!({ "resourceArn": MALFORMED }),
            one(&pattern_clause("resourceArn", TAGGABLE_ARN_PATTERN)),
        ),
        (
            // Cause 2: a collection bound, not a string one.
            "ListBenefitApplications with two Stages, past the modelled maximum of 1"
                .to_string(),
            "ListBenefitApplications",
            json!({ "Catalog": CATALOG, "Stages": ["one", "two"] }),
            one(&clause(
                "stages",
                "Member must have length less than or equal to 1",
            )),
        ),
        (
            // Cause 3.
            "GetBenefit with a malformed Identifier".to_string(),
            "GetBenefit",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", BENEFIT_IDENTIFIER_PATTERN)),
        ),
        (
            // Cause 4.
            "GetBenefitAllocation with a malformed Identifier".to_string(),
            "GetBenefitAllocation",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", ALLOCATION_IDENTIFIER_PATTERN)),
        ),
        (
            "GetBenefitApplication with a malformed Identifier".to_string(),
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "SubmitBenefitApplication with a malformed Identifier".to_string(),
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "RecallBenefitApplication with a malformed Identifier".to_string(),
            "RecallBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "Identifier": MALFORMED,
                "Reason": "the body is what is under test",
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "CancelBenefitApplication with a malformed Identifier".to_string(),
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": MALFORMED,
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "UpdateBenefitApplication with a malformed Identifier".to_string(),
            "UpdateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": MALFORMED,
                "Revision": "1",
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "AmendBenefitApplication with a malformed Identifier".to_string(),
            "AmendBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": MALFORMED,
                "Revision": "1",
                "AmendmentReason": "the body is what is under test",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "a probe" }],
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "CreateBenefitApplication with a malformed BenefitIdentifier".to_string(),
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": MALFORMED,
                "Name": "a probe",
            }),
            one(&pattern_clause(
                "benefitIdentifier",
                BENEFIT_IDENTIFIER_PATTERN,
            )),
        ),
        (
            "AssociateBenefitApplicationResource with a ResourceArn that is not an ARN"
                .to_string(),
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": UNKNOWN_APPLICATION,
                "ResourceArn": MALFORMED,
            }),
            one(&pattern_clause("resourceArn", ARN_PATTERN)),
        ),
        (
            "DisassociateBenefitApplicationResource with a malformed identifier".to_string(),
            "DisassociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": MALFORMED,
                "ResourceArn": "arn:aws:s3:::a-probe-bucket",
            }),
            one(&pattern_clause(
                "benefitApplicationIdentifier",
                APPLICATION_IDENTIFIER_PATTERN,
            )),
        ),
        (
            "ListBenefits with maxResults one past the modelled maximum".to_string(),
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 101 }),
            one(&clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )),
        ),
        (
            // Every `FulfillmentType` the enum declares at once is three values, one past
            // the modelled maximum of two, and each value itself is admissible — so the
            // collection bound is the only violation.
            format!(
                "ListBenefitAllocations with all {} FulfillmentTypes, past the maximum of 2",
                FULFILLMENT_TYPE_VALUES.len()
            ),
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "FulfillmentTypes": FULFILLMENT_TYPE_VALUES }),
            one(&clause(
                "fulfillmentTypes",
                "Member must have length less than or equal to 2",
            )),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let response = refused_body(&harness, &case, operation, &body).await;
        assert_modelled_exception(&case, &response, &expected);
    }
}

/// The other constraint classes, on the operations that declare them: a `@length` bound on
/// a string, an absent @required member, a constraint on a member nested inside a list,
/// and two violations in one request — each still answered in the modelled shape.
#[tokio::test]
async fn every_constraint_class_is_stated_in_the_modelled_exception() {
    let harness = Harness::new();
    // (case, operation, request body, the Message the refusal must carry)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a nextToken under the modelled 1-character minimum",
            "ListBenefits",
            json!({ "Catalog": CATALOG, "NextToken": "" }),
            one(&clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            "a nextToken one character past the 2048 maximum",
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "NextToken": "a".repeat(2049) }),
            one(&clause(
                "nextToken",
                "Member must have length less than or equal to 2048",
            )),
        ),
        (
            "an absent @required member",
            "CreateBenefitApplication",
            json!({ "Catalog": CATALOG, "BenefitIdentifier": UNKNOWN_BENEFIT }),
            one(
                "Value null at 'clientToken' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a tag key past the modelled 128-character maximum, inside the tags list",
            "TagResource",
            json!({
                "resourceArn": taggable_arn(),
                "tags": [{ "Key": "k".repeat(129), "Value": "body" }],
            }),
            one(&clause(
                "tags.1.member.key",
                "Member must have length less than or equal to 128",
            )),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let response = refused_body(&harness, case, operation, &body).await;
        assert_modelled_exception(case, &response, &expected);
    }

    // Two violations in one request: the plural envelope, still one `Message` member.
    let case = "a malformed identifier and a maxResults out of range";
    let response = refused_body(
        &harness,
        case,
        "ListBenefitApplications",
        &json!({ "Catalog": CATALOG, "NextToken": "", "MaxResults": 424_242 }),
    )
    .await;
    let message = message_of(&response).unwrap_or_else(|| {
        panic!("case {case:?}: the body carries no modelled Message member: {response:?}")
    });
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural envelope, got {message:?}"
    );
    for expected in [
        clause(
            "nextToken",
            "Member must have length greater than or equal to 1",
        ),
        clause(
            "maxResults",
            "Member must have value less than or equal to 100",
        ),
    ] {
        assert!(
            message.contains(&expected),
            "case {case:?}: expected a clause {expected:?} in {message:?}"
        );
    }
    assert_modelled_exception(case, &response, &message);
}

/// The front that states the refusal is transparent to a request that violates nothing:
/// every well-formed request is still answered by its own operation, never as a
/// validation failure.
#[tokio::test]
async fn a_request_that_violates_nothing_is_not_refused() {
    let harness = Harness::new();
    for (operation, body) in well_formed_requests() {
        let (status, response) = harness.call(operation, &body).await;
        assert_ne!(
            status, 400,
            "case {operation:?}: a well-formed request must not be refused as invalid, \
             got {status} with {response:?}"
        );
        assert_ne!(
            error_code(&response),
            "ValidationException",
            "case {operation:?}: a well-formed request must not be refused as invalid, \
             got {status} with {response:?}"
        );
    }
}

/// And it is transparent to the data path too: a create still round-trips through the
/// front to the read that reports it.
#[tokio::test]
async fn a_create_still_round_trips_through_the_front() {
    let harness = Harness::new();
    let benefit = harness.active_benefit().await;
    let benefit_id = benefit["Id"]
        .as_str()
        .expect("a seeded benefit carries an Id");
    let created = harness
        .ok(
            "a create through the validation front",
            "CreateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "round-trip-through-the-front",
                "BenefitIdentifier": benefit_id,
                "Name": "an application made through the front",
            }),
        )
        .await;
    let identifier = created["Id"]
        .as_str()
        .expect("a create reports the id it minted");
    let read = harness
        .get("a read through the validation front", identifier)
        .await;
    assert_eq!(
        read["Id"].as_str(),
        Some(identifier),
        "the read must report the application the create minted, got {read:?}"
    );
    assert_eq!(
        read["Name"].as_str(),
        Some("an application made through the front"),
        "the read must report the name the create supplied, got {read:?}"
    );
}
