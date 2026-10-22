// SPDX-License-Identifier: CC0-1.0

//! WHERE a refused request's diagnostic is written in the error body, over the wire.
//!
//! The rule: a request outside a modelled input constraint is answered 400
//! `ValidationException` whose body carries the diagnostic in the `Message` member this
//! service's model declares — `ValidationException { Message @required, Reason
//! @required, FieldList }`, and the awsJson codecs key a member by its MEMBER NAME, so
//! the key is `Message` and not the framework's lowercase `message`. The prose is the
//! AWS validation envelope (`N validation error(s) detected: `) over one clause per
//! violation, each naming the member path and the constraint it failed.
//!
//! A violation on an ELEMENT of a collection is reported against the COLLECTION MEMBER,
//! with the element's requirement set bracketed inside `Member must satisfy constraint:
//! [ … ]`; a violation of the collection itself is not bracketed, and neither is one on
//! a member INSIDE a structure element, which keeps its own full path.
//!
//! Each test is a TABLE spanning the class the rule covers — every operation of the
//! lifecycle, every constraint class the model declares on an input member (`@pattern`,
//! `@length` on a string and on a list, `@range`, an enum value set, an absent
//! `@required` member), one and several violations in one request, and both collection
//! shapes (a scalar element and a structure element). The ten calls the issue named are
//! ten rows among them.
//!
//! Every pattern and bound quoted below is the one the model declares, via
//! `smithy_query(query="operation",operation="...",
//! model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")`.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::FULFILLMENT_TYPE_VALUES;

use crate::benefit_application_lifecycle::{error_code, Harness, CATALOG};

/// `BenefitApplicationIdentifier`'s `@pattern`, without the anchors the model spells it
/// with — which is what a clause reporting it states.
const APPLICATION_IDENTIFIER_PATTERN: &str = r"(arn:.+|benappl-[0-9a-z]{14})";

/// The `@pattern` on `CreateBenefitApplicationInput$BenefitIdentifier`, unanchored.
const BENEFIT_IDENTIFIER_PATTERN: &str = r"(arn:.+|ben-[0-9a-z]{14})";

/// `Arn`'s `@pattern` — the shape `ResourceArn` targets — unanchored.
const ARN_PATTERN: &str = r"arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)";

/// `CatalogName`'s `@pattern`, unanchored.
const CATALOG_NAME_PATTERN: &str = "[A-Za-z0-9_-]+";

/// A `ClientToken` every `@pattern` the model puts on one admits (`^[!-~]{1,64}$` on
/// create/update, `^[a-zA-Z0-9-]{1,64}$` on cancel/amend), so a row that is about
/// another member carries no violation of its own.
const CLIENT_TOKEN: &str = "message-probe-token";

/// An application identifier the modelled `BenefitApplicationIdentifier` admits.
const APPLICATION: &str = "benappl-00000000000001";

/// A benefit identifier the modelled pattern admits.
const BENEFIT: &str = "ben-00000000000000";

/// A value no identifier pattern of this service admits.
const MALFORMED: &str = "not-a-valid-identifier!!";

/// The envelope one clause renders into.
fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The envelope several clauses render into, in the order the walk found them.
fn many(clauses: &[String]) -> String {
    format!(
        "{} validation errors detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// One clause: the member path, then the requirement it failed.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause a `@pattern` violation states.
fn pattern_clause(path: &str, pattern: &str) -> String {
    clause(
        path,
        &format!("Member must satisfy regular expression pattern: {pattern}"),
    )
}

/// The clause a violation of one ELEMENT of a collection states: the collection member,
/// with the element's requirement set bracketed — the constraint it failed, then the
/// requirement that the element be present at all (no list of this model is `@sparse`).
fn element_clause(path: &str, requirement: &str) -> String {
    clause(
        path,
        &format!("Member must satisfy constraint: [{requirement}, Member must not be null]"),
    )
}

/// The requirement an element of `FulfillmentTypes` fails when it names no
/// `FulfillmentType`, over the value set the model declares.
fn fulfillment_enum_requirement() -> String {
    format!(
        "Member must satisfy enum value set: [{}]",
        FULFILLMENT_TYPE_VALUES.join(", ")
    )
}

/// The `Message` member of an error body, or `None` when the body carries none.
fn message_member(body: &Value) -> Option<String> {
    body.get("Message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Call `operation` with `body`, require the 400 `ValidationException` the rule owes,
/// and return the `Message` it carried — failing when the member is absent, and when the
/// diagnostic travelled under the framework's lowercase `message` key instead.
async fn refusal_message(harness: &Harness, case: &str, operation: &str, body: &Value) -> String {
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400, got {status} with {response:?}"
    );
    let code = error_code(&response);
    assert_eq!(
        code, "ValidationException",
        "case {case:?} on {operation}: expected ValidationException, got {code:?} with \
         {response:?}"
    );
    assert!(
        response.get("message").is_none(),
        "case {case:?} on {operation}: the diagnostic must travel in the modelled \
         `Message` member, not in a lowercase `message` key, got {response:?}"
    );
    message_member(&response).unwrap_or_else(|| {
        panic!(
            "case {case:?} on {operation}: the refusal carries no `Message` member: \
             {response:?}"
        )
    })
}

/// Every operation of the benefit-application lifecycle, and every constraint class the
/// model declares on one of their input members: the refusal states its diagnostic in
/// the modelled `Message` member, as the AWS validation envelope.
#[tokio::test]
async fn every_refused_request_states_its_message_in_the_modelled_member() {
    let harness = Harness::new();
    // (case, operation, request, the Message the refusal must carry)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a create whose benefit identifier is malformed",
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "BenefitIdentifier": MALFORMED,
            }),
            one(&pattern_clause("benefitIdentifier", BENEFIT_IDENTIFIER_PATTERN)),
        ),
        (
            "a read whose identifier is malformed",
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "an update whose identifier is malformed",
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
            "a cancel whose identifier is malformed",
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": MALFORMED,
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a recall whose identifier is malformed",
            "RecallBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "Identifier": MALFORMED,
                "Reason": "a message probe",
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "a submit whose identifier is malformed",
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": MALFORMED }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            "an amend whose identifier is malformed",
            "AmendBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": CLIENT_TOKEN,
                "Identifier": MALFORMED,
                "Revision": "1",
                "AmendmentReason": "a message probe",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "probe" }],
            }),
            one(&pattern_clause("identifier", APPLICATION_IDENTIFIER_PATTERN)),
        ),
        (
            // TWO violations in one request: the plural envelope, both clauses, in the
            // order the input shape declares its members.
            "an associate whose identifier and resource are both malformed",
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": MALFORMED,
                "ResourceArn": "not-an-arn",
            }),
            many(&[
                pattern_clause(
                    "benefitApplicationIdentifier",
                    APPLICATION_IDENTIFIER_PATTERN,
                ),
                pattern_clause("resourceArn", ARN_PATTERN),
            ]),
        ),
        (
            "a disassociate whose identifier and resource are both malformed",
            "DisassociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": MALFORMED,
                "ResourceArn": "not-an-arn",
            }),
            many(&[
                pattern_clause(
                    "benefitApplicationIdentifier",
                    APPLICATION_IDENTIFIER_PATTERN,
                ),
                pattern_clause("resourceArn", ARN_PATTERN),
            ]),
        ),
        (
            // The collection bound and the element's own constraint, in one request.
            "a listing whose fulfillment-type filter is too long and names no type",
            "ListBenefitAllocations",
            json!({
                "Catalog": CATALOG,
                "FulfillmentTypes": ["ACCESS", "CASH", "NOT_A_FULFILLMENT_TYPE"],
            }),
            many(&[
                clause(
                    "fulfillmentTypes",
                    "Member must have length less than or equal to 2",
                ),
                element_clause("fulfillmentTypes", &fulfillment_enum_requirement()),
            ]),
        ),
        (
            "an absent required member, reported as the null it was",
            "CreateBenefitApplication",
            json!({ "Catalog": CATALOG, "BenefitIdentifier": BENEFIT }),
            one(
                "Value null at 'clientToken' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a catalog outside the modelled CatalogName pattern",
            "ListBenefitApplications",
            json!({ "Catalog": "not a catalog!" }),
            one(&pattern_clause("catalog", CATALOG_NAME_PATTERN)),
        ),
        (
            "maxResults one past the modelled @range maximum",
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "MaxResults": 101 }),
            one(&clause(
                "maxResults",
                "Member must have value less than or equal to 100",
            )),
        ),
        (
            "an empty nextToken, under the modelled @length minimum",
            "ListBenefitApplications",
            json!({ "Catalog": CATALOG, "NextToken": "" }),
            one(&clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            // A member INSIDE a structure element keeps its own full path: the collapse
            // onto the collection member is for a violation ON an element.
            "a tag key inside a structure element",
            "TagResource",
            json!({
                "resourceArn": format!(
                    "arn:aws:partnercentral:us-west-2:{}:catalog/{CATALOG}/benefit-application/{APPLICATION}",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "tags": [{ "Key": "k".repeat(129), "Value": "v" }],
            }),
            one(&clause(
                "tags.1.member.key",
                "Member must have length less than or equal to 128",
            )),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let message = refusal_message(&harness, case, operation, &body).await;
        assert_eq!(
            message, expected,
            "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
        );
    }
}

/// A refused ELEMENT of a collection is reported against the collection member, with the
/// element's requirement set bracketed — however many elements were refused, and
/// whatever their positions. The collection's OWN bound is reported unbracketed, so the
/// two are distinguishable, and a request within both is not refused at all.
#[tokio::test]
async fn a_refused_collection_element_is_reported_against_the_collection_member() {
    let harness = Harness::new();
    let element = element_clause("fulfillmentTypes", &fulfillment_enum_requirement());
    let too_long = clause(
        "fulfillmentTypes",
        "Member must have length less than or equal to 2",
    );
    // (case, the FulfillmentTypes filter, the Message it must be refused with — `None`
    // for a filter inside every modelled bound, which must not be refused at all; the
    // operation's own answer then stands, which for a stub is the scaffold's 501)
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "one bad element",
            json!(["NOT_A_FULFILLMENT_TYPE"]),
            Some(one(&element)),
        ),
        (
            "a bad element after a good one",
            json!([FULFILLMENT_TYPE_VALUES[0], "NOT_A_FULFILLMENT_TYPE"]),
            Some(one(&element)),
        ),
        (
            // Two refused elements name the member ONCE: the clause they collapse onto
            // is the same clause.
            "two bad elements, within the collection bound",
            json!(["NOT_A_FULFILLMENT_TYPE", "ALSO_NOT_ONE"]),
            Some(one(&element)),
        ),
        (
            "an over-long filter whose elements are all good",
            json!([
                FULFILLMENT_TYPE_VALUES[0],
                FULFILLMENT_TYPE_VALUES[1],
                FULFILLMENT_TYPE_VALUES[2]
            ]),
            Some(one(&too_long)),
        ),
        (
            "an over-long filter with a bad element",
            json!([
                FULFILLMENT_TYPE_VALUES[0],
                FULFILLMENT_TYPE_VALUES[1],
                "NOT_A_FULFILLMENT_TYPE"
            ]),
            Some(many(&[too_long.clone(), element.clone()])),
        ),
        ("an empty filter, at the modelled minimum", json!([]), None),
        (
            "a filter at the modelled maximum, all good",
            json!([FULFILLMENT_TYPE_VALUES[0], FULFILLMENT_TYPE_VALUES[1]]),
            None,
        ),
    ];

    for (case, filter, expected) in rows {
        let body = json!({ "Catalog": CATALOG, "FulfillmentTypes": filter });
        match expected {
            Some(expected) => {
                let message =
                    refusal_message(&harness, case, "ListBenefitAllocations", &body).await;
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
            }
            None => {
                let (status, response) = harness.call("ListBenefitAllocations", &body).await;
                assert_ne!(
                    status, 400,
                    "case {case:?}: a filter inside every modelled bound must not be \
                     refused, got {status} with {response:?}"
                );
                assert!(
                    [200, 404, 501].contains(&status),
                    "case {case:?}: expected the operation's own answer (200, the 404 of a \
                     lookup that found nothing, or the 501 of an operation not implemented \
                     yet), got {status} with {response:?}"
                );
            }
        }
    }
}

/// Whatever a caller sends, the refusal body never hands it back — the rule holds for the
/// `Message` member exactly as it did for the framework's wording, so the fix cannot have
/// started echoing input.
#[tokio::test]
async fn the_modelled_message_never_echoes_the_rejected_value() {
    let harness = Harness::new();
    // (case, operation, request, the distinctive value the request carries)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a rejected identifier",
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "benappl-sensitivevalue!!" }),
            "sensitivevalue".to_string(),
        ),
        (
            "a rejected resource arn",
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": APPLICATION,
                "ResourceArn": "arn:private:customer-secret",
            }),
            "customer-secret".to_string(),
        ),
        (
            "a rejected enum element",
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "FulfillmentTypes": ["NOT_A_FULFILLMENT_TYPE"] }),
            "NOT_A_FULFILLMENT_TYPE".to_string(),
        ),
        (
            "a rejected number",
            "ListBenefitAllocations",
            json!({ "Catalog": CATALOG, "MaxResults": 424_242 }),
            "424242".to_string(),
        ),
        (
            "a rejected value longer than any echo cap",
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": format!("benappl-{}", "+".repeat(400)) }),
            "+".repeat(64),
        ),
    ];

    for (case, operation, body, secret) in rows {
        let message = refusal_message(&harness, case, operation, &body).await;
        assert!(
            !message.contains(&secret),
            "case {case:?} on {operation}: the Message echoes {secret:?}: {message:?}"
        );
    }
}
