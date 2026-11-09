// SPDX-License-Identifier: CC0-1.0

//! The error class this service refuses a MODELLED input constraint with.
//!
//! The rule: a request this service refuses for a value its model rejects — a number outside
//! a `@range`, a value outside an enum's value set, a required member left out — is answered
//! with the one bad-input error class the service models, `InvalidParameterValueException`
//! (HTTP 400). The model declares no `ValidationException`, so the generic class the request
//! boundary would otherwise name is an error code no client of this API can catch, whichever
//! operation and whichever member the refusal came from.
//!
//! So the table below spans the refusal, not one call: several operations, several violation
//! kinds (above a range, below a range, outside an enum, absent required member) and
//! paging-size bounds of two different widths — each one past a bound the model publishes,
//! with the bound itself accepted. The refusal also NAMES the input parameter at fault, which
//! each row states. Every call goes through the shared wire harness, so what is asserted is
//! the code, the status and the message a client reads.

use serde_json::{json, Value};

use crate::harness::{call, emulator, error_code, instance_arn, opt_in, ACCOUNT};

/// The ONE bad-input error class this service models: "The value supplied for the input
/// parameter is out of range or not valid" (HTTP 400). Every operation that takes an input
/// lists it, and no operation lists a `ValidationException`.
const MODELLED_BAD_INPUT: &str = "InvalidParameterValueException";

/// A request refused for a modelled constraint is refused with the service's own bad-input
/// class, whatever the operation and whatever the constraint.
#[tokio::test]
async fn a_modelled_constraint_violation_is_refused_with_the_services_own_error_class() {
    let router = emulator();
    opt_in(&router).await;

    // (case, operation, request, the message the refusal states)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a paging size one above IdleMaxResults' published maximum of 100",
            "GetIdleRecommendations",
            json!({"maxResults": 101}),
            "Invalid Max Results.",
        ),
        (
            "a paging size one below IdleMaxResults' published minimum of 0",
            "GetIdleRecommendations",
            json!({"maxResults": -1}),
            "Invalid Max Results.",
        ),
        (
            "a paging size one above MaxResults' published maximum of 1000",
            "GetLambdaFunctionRecommendations",
            json!({"maxResults": 1001}),
            "Invalid Max Results.",
        ),
        (
            "a paging size one below MaxResults' published minimum of 0",
            "GetEnrollmentStatusesForOrganization",
            json!({"maxResults": -1}),
            "Invalid Max Results.",
        ),
        (
            "a filter name outside IdleRecommendationFilterName",
            "GetIdleRecommendations",
            json!({"filters": [{"name": "tag:Owner", "values": ["TeamA"]}]}),
            "Invalid Name.",
        ),
        (
            "a sort order outside Order",
            "GetIdleRecommendations",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Descending"}}),
            "Invalid Order.",
        ),
        (
            "a resource type outside ResourceType",
            "PutRecommendationPreferences",
            json!({
                "resourceType": "Ec2InstanceFleet",
                "scope": {"name": "AccountId", "value": ACCOUNT},
            }),
            "Invalid Resource Type.",
        ),
        (
            "a file format outside FileFormat",
            "ExportEC2InstanceRecommendations",
            json!({
                "s3DestinationConfig": {"bucket": "a-destination-bucket"},
                "fileFormat": "Parquet",
            }),
            "Invalid File Format.",
        ),
        (
            "an enrollment status outside Status",
            "UpdateEnrollmentStatus",
            json!({"status": "Enrolled"}),
            "Invalid Status.",
        ),
        (
            "the required status member left out",
            "UpdateEnrollmentStatus",
            json!({}),
            "Invalid Status.",
        ),
        (
            "the required resourceType member left out",
            "GetRecommendationPreferences",
            json!({}),
            "Invalid Resource Type.",
        ),
        (
            "a scope name outside ScopeName",
            "PutRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "InstanceArn", "value": instance_arn("i-0abcdef1234567890")},
            }),
            "Invalid Name.",
        ),
    ];

    for (case, operation, request, wanted_message) in rows {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse {request} with 400, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            MODELLED_BAD_INPUT,
            "case {case:?}: expected {MODELLED_BAD_INPUT}, got {}: {body}",
            error_code(&body)
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, wanted_message,
            "case {case:?}: expected the refusal to state {wanted_message:?}, got {message:?}"
        );
    }
}

/// The bound itself is INSIDE the range: a paging size equal to a published maximum, and one
/// equal to a published minimum, are answered rather than refused — so the refusal above is
/// the constraint talking, not a rejection of every paging size.
#[tokio::test]
async fn a_paging_size_on_a_published_bound_is_answered() {
    let router = emulator();
    opt_in(&router).await;

    // (case, operation, maxResults, the list member the answer carries)
    let rows: Vec<(&str, &str, i64, &str)> = vec![
        (
            "IdleMaxResults' maximum",
            "GetIdleRecommendations",
            100,
            "idleRecommendations",
        ),
        (
            "IdleMaxResults' minimum",
            "GetIdleRecommendations",
            0,
            "idleRecommendations",
        ),
        (
            "MaxResults' maximum",
            "GetLambdaFunctionRecommendations",
            1000,
            "lambdaFunctionRecommendations",
        ),
        (
            "MaxResults' minimum",
            "GetLambdaFunctionRecommendations",
            0,
            "lambdaFunctionRecommendations",
        ),
    ];

    for (case, operation, max_results, member) in rows {
        let (status, body) = call(&router, operation, json!({"maxResults": max_results})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer maxResults={max_results}, got \
             {status}: {body}"
        );
        assert!(
            body[member].is_array(),
            "case {case:?}: expected {member} to be a list, got {body}"
        );
    }
}
