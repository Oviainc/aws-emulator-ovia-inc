// SPDX-License-Identifier: CC0-1.0

//! WHICH value a constraint refusal of this service quotes, for a member the model marks
//! as one whose value is not to be disclosed.
//!
//! The rule: a clause quotes the value the request carried at the refused member — for
//! EVERY member, including one the model marks `@sensitive`. The validating walk withholds
//! such a value (it hands the violation over with none and says so), so the value a clause
//! states is recovered by re-reading the request, which is what
//! `service_common::validation` collects the submitted echo for. A member the walk
//! withheld is not a member the request left out, and the two are not stated the same way:
//! an ABSENT member keeps the `Value null at '<member>'` it earns.
//!
//! The rows span what the rule generalises over: the constraint class (a pattern, a length
//! maximum, a length minimum), where the member sits (a top-level member and a member
//! inside a list element), the length of the value quoted back, and whether the model marks
//! the member at all. Which members it marks comes from the model
//! (`smithy_query(query="operation", operation="SignUp")` and
//! `smithy_query(query="shape", shape="PasswordType,SecretHashType,ClientIdType,\
//! UsernameType,AttributeType")`): `Password`, `SecretHash`, `ClientId`, `Username` and
//! `AttributeType$Value` carry the trait, and `AttributeType$Name` does not.
//!
//! The harness is the one `log_delivery_configuration.rs` builds for this service, reached
//! through the test crate root rather than copied.

use serde_json::{json, Value};

use super::log_delivery_configuration::{call, error_code, error_message, router};

/// A `ClientId` every row that is not about that member carries: within `@length` 1..128
/// and inside the `^[\w+]+$` the model declares for it.
const WELL_FORMED_CLIENT_ID: &str = "1h57kf5cpq17m0eml12EXAMPLE";

/// A `Username` every row that is not about that member carries.
const WELL_FORMED_USERNAME: &str = "jane.doe";

/// The error this service answers a request its modelled constraints refuse.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The envelope the framework states one broken constraint in.
fn one_validation_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The envelope the framework states SEVERAL broken constraints in: the count it announces
/// and the clauses joined the way it joins them, in walk order.
fn validation_errors(clauses: &[String]) -> String {
    format!(
        "{} validation errors detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// The clause a refused member earns: the value the request carried, the member, the
/// requirement.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause an ABSENT member earns: there was no value to quote.
fn absent_clause(path: &str, requirement: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: {requirement}")
}

/// One row: the request body a member's own constraint refuses, and the message the
/// refusal states.
struct Row {
    case: &'static str,
    body: Value,
    expected_message: String,
}

#[tokio::test]
async fn a_clause_quotes_the_value_the_request_carried_at_a_sensitive_member() {
    let router = router();

    // The requirement halves, as the framework words each class.
    let at_most_128 = "Member must have length less than or equal to 128";
    let at_most_32 = "Member must have length less than or equal to 32";
    let at_least_1 = "Member must have length greater than or equal to 1";
    // A `@pattern` requirement quotes the expression without the whole-value anchors the
    // model writes it with, which is this service's rendering of every pattern.
    let password_pattern = r"Member must satisfy regular expression pattern: [\S]+";
    let client_id_pattern = r"Member must satisfy regular expression pattern: [\w+]+";
    let username_pattern =
        r"Member must satisfy regular expression pattern: [\p{L}\p{M}\p{S}\p{N}\p{P}]+";

    let over_long_secret_hash = "a".repeat(129);
    let over_long_attribute_name = "n".repeat(33);

    let rows = vec![
        Row {
            case: "a password carrying a space, outside its pattern",
            body: json!({
                "ClientId": WELL_FORMED_CLIENT_ID,
                "Username": WELL_FORMED_USERNAME,
                "Password": "two words",
            }),
            expected_message: one_validation_error(&clause(
                "two words",
                "password",
                password_pattern,
            )),
        },
        Row {
            case: "a secret hash past its length maximum",
            body: json!({
                "ClientId": WELL_FORMED_CLIENT_ID,
                "Username": WELL_FORMED_USERNAME,
                "SecretHash": over_long_secret_hash.clone(),
            }),
            expected_message: one_validation_error(&clause(
                &over_long_secret_hash,
                "secretHash",
                at_most_128,
            )),
        },
        Row {
            case: "a client id carrying spaces, outside its pattern",
            body: json!({
                "ClientId": "not a client id",
                "Username": WELL_FORMED_USERNAME,
            }),
            expected_message: one_validation_error(&clause(
                "not a client id",
                "clientId",
                client_id_pattern,
            )),
        },
        Row {
            case: "an empty username, under its length minimum and outside its pattern",
            body: json!({
                "ClientId": WELL_FORMED_CLIENT_ID,
                "Username": "",
            }),
            // The empty string breaks both of the member's constraints, so the refusal
            // states a clause for each, both quoting the value the request carried.
            expected_message: validation_errors(&[
                clause("", "username", at_least_1),
                clause("", "username", username_pattern),
            ]),
        },
        Row {
            case: "a user attribute's NAME past its length maximum, which the model does not mark",
            body: json!({
                "ClientId": WELL_FORMED_CLIENT_ID,
                "Username": WELL_FORMED_USERNAME,
                "UserAttributes": [{ "Name": over_long_attribute_name, "Value": "jane@example" }],
            }),
            expected_message: one_validation_error(&clause(
                &over_long_attribute_name,
                "userAttributes.1.member.name",
                at_most_32,
            )),
        },
        Row {
            case: "an absent required member, which has no value to quote",
            body: json!({ "Username": WELL_FORMED_USERNAME }),
            expected_message: one_validation_error(&absent_clause(
                "clientId",
                "Member must not be null",
            )),
        },
    ];

    for row in rows {
        let (status, payload) = call(&router, "SignUp", &row.body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, INVALID_PARAMETER),
            "case {:?}: expected a 400 {INVALID_PARAMETER}, got {status} with {payload}",
            row.case
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.expected_message,
            "case {:?}: expected {:?}, got {message:?}",
            row.case, row.expected_message
        );
    }
}

#[tokio::test]
async fn a_long_withheld_value_is_still_the_value_the_request_carried() {
    let router = router();

    // The one case where the value cannot be stated whole: breaking a member's length
    // MAXIMUM takes a value past the ceiling a clause quotes at, so the clause opens with
    // a prefix of what the request carried rather than all of it. What the rule fixes is
    // that the value is the SUBMITTED one — asserted as the clause's two ends, so the
    // ceiling itself stays the shared layer's to choose.
    //
    // `AttributeValueType` is `@length(max: 2048)` and `@sensitive`
    // (`smithy_query(query="shape", shape="AttributeType")`), so a value one character
    // past it is both over the bound and withheld by the walk.
    let submitted = "v".repeat(2049);
    let (status, payload) = call(
        &router,
        "SignUp",
        &json!({
            "ClientId": WELL_FORMED_CLIENT_ID,
            "Username": WELL_FORMED_USERNAME,
            "UserAttributes": [{ "Name": "email", "Value": submitted }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, INVALID_PARAMETER),
        "expected a 400 {INVALID_PARAMETER}, got {status} with {payload}"
    );
    let message = error_message(&payload);
    // Short enough to be inside any ceiling the layer may state, so this holds whether
    // the clause quotes the value whole or elides it.
    let opening = format!("1 validation error detected: Value '{}", "v".repeat(64));
    assert!(
        message.starts_with(&opening),
        "the clause must open with the value the request carried, got {message:?}"
    );
    let requirement = "at 'userAttributes.1.member.value' failed to satisfy constraint: \
                       Member must have length less than or equal to 2048";
    assert!(
        message.ends_with(requirement),
        "the clause must name the member and the bound it broke, got {message:?}"
    );
}
