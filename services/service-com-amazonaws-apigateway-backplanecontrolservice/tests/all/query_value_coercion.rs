// SPDX-License-Identifier: CC0-1.0

//! How a value bound to the QUERY STRING that cannot be converted to the modelled type
//! of its member is refused: the protocol-level `400 SerializationException` carrying
//! the sentence under the capital-`Message` body member, not the framework's
//! constraint-refusal envelope.
//!
//! A `@httpQuery` value is CONVERTED into its member's modelled type before the request
//! becomes an input at all, so a value that cannot be converted fails in the
//! protocol/deserialization layer, ahead of the operation and ahead of every modelled
//! constraint. That refusal is a different error class from a value that WAS converted
//! and then broke a modelled rule: the first tells a caller to fix its serializer, the
//! second to fix a value, and an SDK maps the two to different exception classes. See
//! `src/service/query_coercion.rs` for the whole argument.
//!
//! The rule is asserted over a SET of inputs rather than the one recorded call. The rows
//! differ along what it generalises over:
//!
//! * the MEMBER, over every `@httpQuery`-bound convertible scalar this service's model
//!   declares — `GetApiKeys$includeValues` and `$limit`, `GetApiKey$includeValue`,
//!   `GetModel$flatten`, `PutRestApi$failOnWarnings`, `ImportRestApi$failOnWarnings`
//!   (`smithy_query(query="operation", operation="GetApiKey,GetApiKeys,GetModel,\
//!   ImportApiKeys,ImportDocumentationParts,ImportRestApi,PutRestApi",
//!   model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/\
//!   smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")`), so the rule
//!   is not keyed to the one member the recording names;
//! * the modelled TYPE, boolean and integer;
//! * the VALUE's form: a word, a number where a boolean is modelled, a differently-cased
//!   boolean word, an empty value, and a number one past the modelled width;
//! * the METHOD and whether the operation is implemented, a stub or would answer a
//!   modelled error of its own.
//!
//! [`Gateway`] and friends are this crate's one wire-test harness, reused from
//! `rest_api_lifecycle`.

use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};

/// The error code a protocol-level conversion failure is answered with, and the code
/// that must NOT appear: `ValidationException` is what the framework answers for a value
/// that was read and then broke a modelled constraint.
const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The code a constraint refusal carries, which a conversion failure is not.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// What a row expects of the answer.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The request is refused by the protocol layer, with this exact sentence — built
    /// from the value the request sent and the type it could not be converted to.
    Unconvertible(&'static str),
    /// The value converts, so this layer must not refuse the request at all; what the
    /// operation then answers is the operation's own business.
    Converted,
}

/// Assert `answer` is the protocol-level conversion refusal stating `message`.
fn assert_unconvertible(answer: &Answer, message: &str, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(SERIALIZATION_EXCEPTION),
        "case {case}: expected the error type {SERIALIZATION_EXCEPTION}, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.body["__type"].as_str(),
        Some(SERIALIZATION_EXCEPTION),
        "case {case}: the body must resolve the same shape as the header, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["Message"].as_str(),
        Some(message),
        "case {case}: expected the sentence {message:?} under `Message`, got {:?}",
        answer.body
    );
    assert!(
        answer.body["message"].is_null(),
        "case {case}: this envelope carries its sentence under `Message` alone, \
         got {:?}",
        answer.body
    );
}

/// Assert `answer` is NOT a conversion refusal and not a constraint refusal either: the
/// value was read, so neither layer rejected the request.
fn assert_converted(answer: &Answer, case: &str) {
    assert_ne!(
        answer.error.as_deref(),
        Some(SERIALIZATION_EXCEPTION),
        "case {case}: a value that converts must not be refused as unconvertible, \
         got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_ne!(
        answer.error.as_deref(),
        Some(VALIDATION_EXCEPTION),
        "case {case}: a value that converts must reach the operation, \
         got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_ne!(
        answer.status, 400,
        "case {case}: a value that converts must not be a bad request, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_query_value_that_cannot_be_converted_is_a_protocol_level_serialization_failure() {
    let gateway = Gateway::new();
    // One stored key and one stored API, so a row that converts reaches an operation
    // that answers about a real record rather than a missing one.
    let created = gateway
        .call("POST", "/apikeys", Some(json!({ "name": "coercion" })))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateApiKey: expected 201, got {} with {:?}",
        created.status, created.body
    );
    let key = string_member(&created.body, "id", "CreateApiKey");
    let (api, _) = create_api(&gateway, "coercion-api").await;

    // (case, method, uri, body, expectation)
    let rows: Vec<(String, &str, String, Option<Value>, Expect)> = vec![
        // GetApiKeys$includeValues: a NullableBoolean bound to `includeValues`.
        (
            "GetApiKeys includeValues=maybe".to_string(),
            "GET",
            "/apikeys?includeValues=maybe".to_string(),
            None,
            Expect::Unconvertible("'maybe' can not be converted to Boolean"),
        ),
        (
            "GetApiKeys includeValues=1".to_string(),
            "GET",
            "/apikeys?includeValues=1".to_string(),
            None,
            Expect::Unconvertible("'1' can not be converted to Boolean"),
        ),
        (
            "GetApiKeys includeValues=TRUE".to_string(),
            "GET",
            "/apikeys?includeValues=TRUE".to_string(),
            None,
            Expect::Unconvertible("'TRUE' can not be converted to Boolean"),
        ),
        (
            "GetApiKeys includeValues with no value".to_string(),
            "GET",
            "/apikeys?includeValues=".to_string(),
            None,
            Expect::Unconvertible("'' can not be converted to Boolean"),
        ),
        (
            "GetApiKeys includeValues=true".to_string(),
            "GET",
            "/apikeys?includeValues=true".to_string(),
            None,
            Expect::Converted,
        ),
        (
            "GetApiKeys includeValues=false".to_string(),
            "GET",
            "/apikeys?includeValues=false".to_string(),
            None,
            Expect::Converted,
        ),
        // GetApiKeys$limit: a NullableInteger bound to `limit` — the same rule, a
        // different modelled type, named by that type.
        (
            "GetApiKeys limit=lots".to_string(),
            "GET",
            "/apikeys?limit=lots".to_string(),
            None,
            Expect::Unconvertible("'lots' can not be converted to Integer"),
        ),
        (
            "GetApiKeys limit one past the modelled width".to_string(),
            "GET",
            "/apikeys?limit=2147483648".to_string(),
            None,
            Expect::Unconvertible("'2147483648' can not be converted to Integer"),
        ),
        (
            "GetApiKeys limit=2147483647".to_string(),
            "GET",
            "/apikeys?limit=2147483647".to_string(),
            None,
            Expect::Converted,
        ),
        // GetApiKey$includeValue, on a key that EXISTS and on one that does not: the
        // conversion failure precedes the operation, so it precedes the modelled 404.
        (
            "GetApiKey includeValue=yes".to_string(),
            "GET",
            format!("/apikeys/{key}?includeValue=yes"),
            None,
            Expect::Unconvertible("'yes' can not be converted to Boolean"),
        ),
        (
            "GetApiKey includeValue=yes on an unknown key".to_string(),
            "GET",
            "/apikeys/zzzzzzzzzz?includeValue=yes".to_string(),
            None,
            Expect::Unconvertible("'yes' can not be converted to Boolean"),
        ),
        (
            "GetApiKey includeValue=true".to_string(),
            "GET",
            format!("/apikeys/{key}?includeValue=true"),
            None,
            Expect::Converted,
        ),
        // GetRestApis$limit, a different operation reading the same modelled type.
        (
            "GetRestApis limit=abc".to_string(),
            "GET",
            "/restapis?limit=abc".to_string(),
            None,
            Expect::Unconvertible("'abc' can not be converted to Integer"),
        ),
        (
            "GetRestApis limit=1".to_string(),
            "GET",
            "/restapis?limit=1".to_string(),
            None,
            Expect::Converted,
        ),
        // GetResources$limit, under a URI with labels before the query string.
        (
            "GetResources limit=none".to_string(),
            "GET",
            format!("/restapis/{api}/resources?limit=none"),
            None,
            Expect::Unconvertible("'none' can not be converted to Integer"),
        ),
        (
            "GetResources limit=2".to_string(),
            "GET",
            format!("/restapis/{api}/resources?limit=2"),
            None,
            Expect::Converted,
        ),
        // GetModel$flatten: a Boolean with a declared default, which changes nothing
        // about converting the value a request DOES send.
        (
            "GetModel flatten=sure".to_string(),
            "GET",
            format!("/restapis/{api}/models/Empty?flatten=sure"),
            None,
            Expect::Unconvertible("'sure' can not be converted to Boolean"),
        ),
        // PutRestApi$failOnWarnings, bound to the lowercase query name
        // `failonwarnings` — the NAME is the model's, not the member's spelling.
        (
            "PutRestApi failonwarnings=nope".to_string(),
            "PUT",
            format!("/restapis/{api}?mode=merge&failonwarnings=nope"),
            Some(json!({ "swagger": "2.0" })),
            Expect::Unconvertible("'nope' can not be converted to Boolean"),
        ),
        // ImportRestApi$failOnWarnings, on a POST whose body is the payload: the
        // conversion failure precedes the body being read at all.
        (
            "ImportRestApi failonwarnings=sure".to_string(),
            "POST",
            "/restapis?mode=import&failonwarnings=sure".to_string(),
            None,
            Expect::Unconvertible("'sure' can not be converted to Boolean"),
        ),
        // A value carrying a percent escape is decoded before it is converted, so the
        // sentence names what the caller sent.
        (
            "GetApiKeys includeValues with an escaped value".to_string(),
            "GET",
            "/apikeys?includeValues=tr%75e".to_string(),
            None,
            Expect::Converted,
        ),
        (
            "GetApiKeys includeValues=may%20be".to_string(),
            "GET",
            "/apikeys?includeValues=may%20be".to_string(),
            None,
            Expect::Unconvertible("'may be' can not be converted to Boolean"),
        ),
        // Only the FIRST value bound to a name is converted, so a second one is never
        // refused — and a first one that cannot be converted is, whatever follows it.
        (
            "GetApiKeys includeValues=true then maybe".to_string(),
            "GET",
            "/apikeys?includeValues=true&includeValues=maybe".to_string(),
            None,
            Expect::Converted,
        ),
        (
            "GetApiKeys includeValues=maybe then true".to_string(),
            "GET",
            "/apikeys?includeValues=maybe&includeValues=true".to_string(),
            None,
            Expect::Unconvertible("'maybe' can not be converted to Boolean"),
        ),
        // A query parameter the operation binds no member to is not this layer's
        // business, whatever it carries.
        (
            "GetApiKeys with an unmodelled parameter".to_string(),
            "GET",
            "/apikeys?somethingElse=maybe".to_string(),
            None,
            Expect::Converted,
        ),
        // A string-typed query member converts from any text at all.
        (
            "GetApiKeys name=maybe".to_string(),
            "GET",
            "/apikeys?name=maybe".to_string(),
            None,
            Expect::Converted,
        ),
    ];

    for (case, method, uri, body, expect) in rows {
        let answer = gateway.call(method, &uri, body).await;
        match expect {
            Expect::Unconvertible(message) => {
                assert_unconvertible(&answer, message, &format!("{case} [{method} {uri}]"));
            }
            Expect::Converted => {
                assert_converted(&answer, &format!("{case} [{method} {uri}]"));
            }
        }
    }
}

#[tokio::test]
async fn two_unconvertible_values_are_answered_for_one_of_them_in_the_same_envelope() {
    let gateway = Gateway::new();
    // When more than one value cannot be converted, the refusal names ONE of them — the
    // member the reader reaches first — in the same envelope. Either sentence is the
    // rule applied; what must not happen is a constraint envelope or no `Message`.
    let answer = gateway
        .call("GET", "/apikeys?includeValues=maybe&limit=lots", None)
        .await;
    let case = "GetApiKeys includeValues=maybe&limit=lots";
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some(SERIALIZATION_EXCEPTION),
        "case {case}: expected {SERIALIZATION_EXCEPTION}, got {:?}",
        answer.error
    );
    let stated = answer.body["Message"].as_str().unwrap_or_default();
    let expected = [
        "'maybe' can not be converted to Boolean",
        "'lots' can not be converted to Integer",
    ];
    assert!(
        expected.contains(&stated),
        "case {case}: expected one of {expected:?} under `Message`, got {stated:?}"
    );
}
