// SPDX-License-Identifier: CC0-1.0

//! How Comprehend words a request its model refuses before any handler runs.
//!
//! The refusal is a `ValidationException` (HTTP 400) carrying the
//! `<N> validation error(s) detected: ` envelope over one clause per violated
//! member. Which member is named, and whether the rejected value is echoed,
//! follows the violation's CLASS: a value outside a numeric `@range`, and a value
//! longer than a member's `@length` MAXIMUM, are reported WITHOUT it, naming only
//! the member and the bound; every other class (an absent required member, a
//! `@length` MINIMUM, a `@pattern`, an enumerated value set) echoes the value the
//! request sent.
//!
//! Each suite asserts that rule over a SET of members and operations rather than
//! one request: both `@range` members the model declares, both bound directions,
//! values at each published bound and past it, and every `@length` maximum a
//! top-level input member of an implemented operation carries.

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The error code every refusal in this file answers with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The envelope a single-clause refusal carries.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a numeric `@range` violation states: the member and the bound, with
/// no value echoed.
fn range_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// Every numeric member the Comprehend model bounds with `@range`, from
/// `smithy_query(query="shape", shape="InferenceUnitsInteger,MaxResultsInteger")`:
///
/// * `CreateEndpoint.DesiredInferenceUnits` -> `InferenceUnitsInteger`,
///   `@range` min 1 (no maximum);
/// * `ListSentimentDetectionJobs.MaxResults` -> `MaxResultsInteger`, `@range`
///   min 1, max 500.
///
/// Each row is `(case, operation, body, member path, requirement)`.
fn range_violations() -> Vec<(&'static str, &'static str, Value, &'static str, String)> {
    let min_one = "Member must have value greater than or equal to 1".to_string();
    let max_five_hundred = "Member must have value less than or equal to 500".to_string();
    vec![
        (
            "DesiredInferenceUnits at zero",
            "CreateEndpoint",
            json!({ "EndpointName": "endpoint", "DesiredInferenceUnits": 0 }),
            "desiredInferenceUnits",
            min_one.clone(),
        ),
        (
            "DesiredInferenceUnits far below the minimum",
            "CreateEndpoint",
            json!({ "EndpointName": "endpoint", "DesiredInferenceUnits": -4096 }),
            "desiredInferenceUnits",
            min_one.clone(),
        ),
        (
            "MaxResults one below the minimum",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 0 }),
            "maxResults",
            min_one.clone(),
        ),
        (
            "MaxResults negative",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": -1 }),
            "maxResults",
            min_one,
        ),
        (
            "MaxResults one above the maximum",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 501 }),
            "maxResults",
            max_five_hundred.clone(),
        ),
        (
            "MaxResults far above the maximum",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 100_000 }),
            "maxResults",
            max_five_hundred,
        ),
    ]
}

#[tokio::test]
async fn a_range_violation_names_the_member_and_the_bound_and_echoes_no_value() {
    let comprehend = Comprehend::new();

    for (case, operation, body, member_path, requirement) in range_violations() {
        let (status, response) = comprehend.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            one_error(&range_clause(member_path, &requirement)),
            "case {case:?}: unexpected validation message"
        );
        // The same rule, stated as the property it is: the submitted magnitude —
        // the one number in the request — appears nowhere in the refusal.
        let submitted = body
            .as_object()
            .and_then(|members| members.values().find(|value| value.is_number()))
            .map(Value::to_string)
            .unwrap_or_default();
        assert!(
            !submitted.is_empty() && !message.contains(&submitted),
            "case {case:?}: the refusal echoed the rejected value {submitted:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_value_inside_a_published_range_bound_is_not_refused() {
    let comprehend = Comprehend::new();

    // Each published bound of each `@range` member, and a value between them: a
    // bound is INCLUSIVE, so none of these is a validation error. Each row states
    // the status it must get instead: `ListSentimentDetectionJobs` is implemented
    // and answers 200, while `CreateEndpoint` gets past validation and is then
    // refused for the model it names — a classifier this store does not hold — with
    // `ResourceNotFoundException`. Either way the request was not refused as
    // invalid.
    let absent_model = format!(
        "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/absent-classifier"
    );
    for (case, operation, body, expected_status, expected_code) in [
        (
            "MaxResults at the minimum",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 1 }),
            200,
            None,
        ),
        (
            "MaxResults at the maximum",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 500 }),
            200,
            None,
        ),
        (
            "MaxResults between the bounds",
            "ListSentimentDetectionJobs",
            json!({ "MaxResults": 250 }),
            200,
            None,
        ),
        (
            "DesiredInferenceUnits at the minimum",
            "CreateEndpoint",
            json!({
                "EndpointName": "endpoint",
                "ModelArn": absent_model.clone(),
                "DesiredInferenceUnits": 1,
            }),
            400,
            Some("ResourceNotFoundException"),
        ),
        (
            "DesiredInferenceUnits far above the minimum",
            "CreateEndpoint",
            json!({
                "EndpointName": "endpoint",
                "ModelArn": absent_model.clone(),
                "DesiredInferenceUnits": 4096,
            }),
            400,
            Some("ResourceNotFoundException"),
        ),
    ] {
        let (status, response) = comprehend.call(operation, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a value inside the published bound must not be refused, \
             expected {expected_status}, got {status}: {response}"
        );
        if let Some(code) = expected_code {
            assert_eq!(
                error_code(&response),
                code,
                "case {case:?}: expected {code}, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_violation_of_another_class_still_echoes_the_rejected_value() {
    let comprehend = Comprehend::new();

    // A `@length` MINIMUM and an absent required member: both state the value the
    // request held, exactly as the framework's validator renders them. The maximum
    // direction is the one class that echoes nothing — see
    // [`a_length_maximum_violation_names_the_member_and_the_limit_and_echoes_no_value`].
    for (case, operation, body, expected) in [
        (
            "NextToken shorter than its @length minimum",
            "ListSentimentDetectionJobs",
            json!({ "NextToken": "" }),
            one_error(
                "Value '' at 'nextToken' failed to satisfy constraint: Member must have \
                 length greater than or equal to 1",
            ),
        ),
        (
            "DesiredInferenceUnits absent",
            "CreateEndpoint",
            json!({ "EndpointName": "endpoint" }),
            one_error(
                "Value null at 'desiredInferenceUnits' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
    ] {
        let (status, response) = comprehend.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {response}"
        );
        assert_eq!(
            error_message(&response),
            expected,
            "case {case:?}: unexpected validation message"
        );
    }

    // A `@pattern` violation echoes the value too; the pattern it names is the
    // model's own, so only the echoed prefix is pinned here.
    let (status, response) = comprehend
        .call(
            "CreateEndpoint",
            &json!({ "EndpointName": "not a valid name!!", "DesiredInferenceUnits": 1 }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"EndpointName outside its @pattern\": expected 400, got {status}: {response}"
    );
    let message = error_message(&response);
    assert!(
        message.starts_with(&one_error(
            "Value 'not a valid name!!' at 'endpointName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: "
        )),
        "case \"EndpointName outside its @pattern\": expected the rejected value to be \
         echoed, got {message:?}"
    );
}

/// The clause a `@length` maximum violation states: the member and the limit, with
/// no value echoed.
fn length_clause(member_path: &str, max: usize) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must have length \
         less than or equal to {max}"
    )
}

/// A Comprehend ARN of exactly `total` characters naming `resource_type`, whose local
/// name is pattern-conforming — so the only constraint it breaks is the `@length`
/// maximum of the member it is sent in.
fn arn_of_length(resource_type: &str, total: usize) -> String {
    let prefix = format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:{resource_type}/");
    let name_length = total
        .checked_sub(prefix.len())
        .expect("the requested length leaves room for the ARN's local name");
    format!("{prefix}{}", "a".repeat(name_length))
}

#[tokio::test]
async fn a_length_maximum_violation_names_the_member_and_the_limit_and_echoes_no_value() {
    let comprehend = Comprehend::new();

    // Every `@length` maximum a top-level input member of an implemented operation
    // carries, from `smithy_query(query="operation",
    // operation="CreateEndpoint,DescribeEndpoint")`: `CreateEndpoint.EndpointName` ->
    // `ComprehendEndpointName` max 40, `CreateEndpoint.ClientRequestToken` ->
    // `ClientRequestTokenString` max 64, `CreateEndpoint.ModelArn` ->
    // `ComprehendModelArn` max 256 and `DescribeEndpoint.EndpointArn` ->
    // `ComprehendEndpointArn` max 256. Rows differ along the member, the operation,
    // the limit and how far past it the value reaches — one character past, the
    // length the issue cited, and far past — and every value also conforms to its
    // member's `@pattern`, so the length is the only constraint it breaks.
    let hyphenated = format!("{}-{}", "a".repeat(20), "b".repeat(22));
    let rows: Vec<(&str, &str, Value, &str, usize, String)> = vec![
        (
            "EndpointName one character past its maximum",
            "CreateEndpoint",
            json!({ "EndpointName": "a".repeat(41), "DesiredInferenceUnits": 1 }),
            "endpointName",
            40,
            "a".repeat(41),
        ),
        (
            "EndpointName at the length the issue cited",
            "CreateEndpoint",
            json!({ "EndpointName": &hyphenated, "DesiredInferenceUnits": 1 }),
            "endpointName",
            40,
            hyphenated.clone(),
        ),
        (
            "EndpointName far past its maximum",
            "CreateEndpoint",
            json!({ "EndpointName": "a".repeat(400), "DesiredInferenceUnits": 1 }),
            "endpointName",
            40,
            "a".repeat(400),
        ),
        (
            "ClientRequestToken one character past its maximum",
            "CreateEndpoint",
            json!({
                "EndpointName": "endpoint",
                "DesiredInferenceUnits": 1,
                "ClientRequestToken": "a".repeat(65),
            }),
            "clientRequestToken",
            64,
            "a".repeat(65),
        ),
        (
            "ModelArn one character past its maximum",
            "CreateEndpoint",
            json!({
                "EndpointName": "endpoint",
                "DesiredInferenceUnits": 1,
                "ModelArn": arn_of_length("document-classifier", 257),
            }),
            "modelArn",
            256,
            arn_of_length("document-classifier", 257),
        ),
        (
            "EndpointArn one character past its maximum",
            "DescribeEndpoint",
            json!({ "EndpointArn": arn_of_length("document-classifier-endpoint", 257) }),
            "endpointArn",
            256,
            arn_of_length("document-classifier-endpoint", 257),
        ),
    ];

    for (case, operation, body, member_path, max, submitted) in rows {
        let (status, response) = comprehend.call(operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            one_error(&length_clause(member_path, max)),
            "case {case:?}: unexpected validation message"
        );
        // The same rule, stated as the property it is: the oversized value the
        // request carried appears nowhere in the refusal.
        assert!(
            !message.contains(&submitted),
            "case {case:?}: the refusal echoed the rejected value {submitted:?}: \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_value_at_a_published_length_maximum_is_not_refused_for_its_length() {
    let comprehend = Comprehend::new();

    // The other side of the same rule: a `@length` maximum is INCLUSIVE, so a value
    // exactly at it is not a validation error. `CreateEndpoint` gets past validation
    // and is then refused for the model it names — a classifier this store does not
    // hold — so the request was not refused as invalid.
    //
    // The discriminator is the CODE, not the status: both refusals are HTTP 400 here,
    // and `ResourceNotFoundException` can only come from the handler, which runs only
    // after validation passed. The rows that prove validation is applied to these
    // members at all — rather than skipped — are the oversized ones above, which this
    // test is the inclusive-bound half of.
    let absent_model = arn_of_length("document-classifier", 100);
    for (case, body) in [
        (
            "EndpointName exactly at its maximum",
            json!({
                "EndpointName": "a".repeat(40),
                "ModelArn": absent_model.clone(),
                "DesiredInferenceUnits": 1,
            }),
        ),
        (
            "ClientRequestToken exactly at its maximum",
            json!({
                "EndpointName": "endpoint",
                "ModelArn": absent_model.clone(),
                "DesiredInferenceUnits": 1,
                "ClientRequestToken": "a".repeat(64),
            }),
        ),
        (
            "ModelArn exactly at its maximum",
            json!({
                "EndpointName": "endpoint",
                "ModelArn": arn_of_length("document-classifier", 256),
                "DesiredInferenceUnits": 1,
            }),
        ),
    ] {
        let (status, response) = comprehend.call("CreateEndpoint", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: a value at the published maximum must not be refused as \
             invalid, got {response}"
        );
    }
}
