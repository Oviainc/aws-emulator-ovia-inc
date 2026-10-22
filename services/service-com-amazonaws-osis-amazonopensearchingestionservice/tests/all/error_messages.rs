// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, driven end to end over HTTP.
//!
//! Two rules are under test, each over a table whose rows differ along what the
//! rule generalises over rather than repeating one recorded request:
//!
//! 1. A member that is a collection of constrained scalars is reported ONCE per
//!    member — the collection member's own path, the refused values as a
//!    bracketed list, and the element shape's whole constraint set enumerated,
//!    whichever of those constraints the request failed — and the entries of the
//!    message are ordered by path. The rows vary the operation, the nesting
//!    depth of the member, how many lists and how many elements are refused and
//!    which constraint each one fails. A member that is NOT such a collection
//!    keeps the framework's own per-member clause, anchors and all.
//! 2. A not-found failure is worded per operation: `DeletePipeline` states the
//!    delete it could not carry out and names the pipeline the request named,
//!    while `GetPipeline` states only that the pipeline was not found and echoes
//!    nothing back. The rows vary the pipeline name's length and shape, and
//!    whether the name was never created or created and then deleted.
//!
//! The constraint catalogue the expected entries are built from is written out
//! here from the model, so a message is compared against the model rather than
//! against the code that renders it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is not the emulator default, so nothing under test can depend
/// on a default-region code path.
const REGION: &str = "eu-west-2";

/// A well-formed `PipelineArn` (`@length(46, 76)`, `^arn:(aws|…):osis:.+:pipeline\/.+$`),
/// so a `CreatePipelineEndpoint` row is refused for its `VpcOptions` alone.
const PIPELINE_ARN: &str = "arn:aws:osis:eu-west-2:000000000000:pipeline/endpoint-pipeline";

/// What one element shape requires, as the model declares it.
///
/// Source: `smithy_query(query="shape", shape="com.amazonaws.osis#SubnetId,com.amazonaws.osis#SecurityGroupId,com.amazonaws.osis#PipelineEndpointId")`
/// — `@length(min, max)` and `@pattern`. `pattern` is written here in the
/// unanchored form a validation message states it in.
struct ElementShape {
    max_length: i64,
    min_length: i64,
    pattern: &'static str,
}

/// `com.amazonaws.osis#SubnetId`: `@length(15, 24)`, `@pattern("^subnet-\w{8}(\w{9})?$")`.
const SUBNET_ID: ElementShape = ElementShape {
    max_length: 24,
    min_length: 15,
    pattern: r"subnet-\w{8}(\w{9})?.*",
};

/// `com.amazonaws.osis#SecurityGroupId`: `@length(11, 20)`, `@pattern("^sg-\w{8}(\w{9})?$")`.
const SECURITY_GROUP_ID: ElementShape = ElementShape {
    max_length: 20,
    min_length: 11,
    pattern: r"sg-\w{8}(\w{9})?.*",
};

/// `com.amazonaws.osis#PipelineEndpointId`: `@length(3, 512)`,
/// `@pattern("^[a-zA-Z0-9][a-zA-Z0-9-_]+$")`.
const PIPELINE_ENDPOINT_ID: ElementShape = ElementShape {
    max_length: 512,
    min_length: 3,
    pattern: r"[a-zA-Z0-9][a-zA-Z0-9-_]+.*",
};

/// A security group id the model accepts, for a row that refuses only the other
/// list.
const VALID_SECURITY_GROUP_ID: &str = "sg-12345678";

/// A subnet id the model accepts, for the same reason.
const VALID_SUBNET_ID: &str = "subnet-12345678";

/// Long enough after a create that the `CREATING` -> `ACTIVE` transition the
/// clock owes has come due, tied to the service's own span rather than to a copy
/// of it, so the delete in the round-trip row is not refused as a conflict.
const PAST_TRANSITION: std::time::Duration =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN
        .saturating_add(std::time::Duration::from_millis(50));

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol
    /// may render the `message` member with.
    fn message(&self) -> String {
        for key in ["message", "Message"] {
            if let Some(text) = self.body.get(key).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        panic!("expected an error message in {:?}", self.body)
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised
    // elsewhere; these cases are about this service's wording.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The entry a validation message states for one refused collection member.
fn member_entry(path: &str, values: &[&str], element: &ElementShape) -> String {
    format!(
        "Value '[{}]' at '{path}' failed to satisfy constraint: \
         Member must satisfy constraint: \
         [Member must have length less than or equal to {}, \
         Member must have length greater than or equal to {}, \
         Member must satisfy regular expression pattern: {}]",
        values.join(", "),
        element.max_length,
        element.min_length,
        element.pattern,
    )
}

/// The envelope a `ValidationException` opens with, counting the entries.
fn envelope(entries: &[String]) -> String {
    match entries.len() {
        1 => format!("1 validation error detected: {}", entries[0]),
        n => format!("{n} validation errors detected: {}", entries.join("; ")),
    }
}

/// A constrained collection member is refused once per MEMBER, at the member's
/// own path, with every constraint its element shape declares.
#[tokio::test]
async fn a_constrained_collection_member_is_refused_once_with_its_whole_contract() {
    // Rows differ in the operation called, in whether the member is nested or
    // top-level, in how many members and how many elements are refused, and in
    // which constraint each element fails (pattern only, pattern and length
    // minimum, length minimum only).
    let cases: Vec<(&str, &str, &str, Value, Vec<String>)> = vec![
        (
            "both lists of a nested member refused",
            "POST",
            "/2022-01-01/osis/createPipelineEndpoint",
            json!({
                "PipelineArn": PIPELINE_ARN,
                "VpcOptions": {
                    "SubnetIds": ["not-a-valid-subnet-id"],
                    "SecurityGroupIds": ["not-a-valid-sg-id"],
                },
            }),
            // Ordered by path: `securityGroupIds` before `subnetIds`, which is
            // NOT the order the model declares them in.
            vec![
                member_entry(
                    "vpcOptions.securityGroupIds",
                    &["not-a-valid-sg-id"],
                    &SECURITY_GROUP_ID,
                ),
                member_entry(
                    "vpcOptions.subnetIds",
                    &["not-a-valid-subnet-id"],
                    &SUBNET_ID,
                ),
            ],
        ),
        (
            "one list refused while the other is accepted",
            "POST",
            "/2022-01-01/osis/createPipelineEndpoint",
            json!({
                "PipelineArn": PIPELINE_ARN,
                "VpcOptions": {
                    "SubnetIds": ["not-a-valid-subnet-id"],
                    "SecurityGroupIds": [VALID_SECURITY_GROUP_ID],
                },
            }),
            vec![member_entry(
                "vpcOptions.subnetIds",
                &["not-a-valid-subnet-id"],
                &SUBNET_ID,
            )],
        ),
        (
            "several elements of one list are still one entry",
            "POST",
            "/2022-01-01/osis/createPipelineEndpoint",
            json!({
                "PipelineArn": PIPELINE_ARN,
                "VpcOptions": {
                    // Each is both too short and outside the pattern, and the
                    // entry names the member once either way.
                    "SubnetIds": ["sub-a", "sub-b", "sub-c"],
                    "SecurityGroupIds": [VALID_SECURITY_GROUP_ID],
                },
            }),
            vec![member_entry(
                "vpcOptions.subnetIds",
                &["sub-a", "sub-b", "sub-c"],
                &SUBNET_ID,
            )],
        ),
        (
            "the same nested member under another operation",
            "POST",
            "/2022-01-01/osis/createPipeline",
            json!({
                "PipelineName": "endpoint-pipeline",
                "MinUnits": 1,
                "MaxUnits": 2,
                "PipelineConfigurationBody": "version: \"2\"\n",
                "VpcOptions": {
                    "SubnetIds": [VALID_SUBNET_ID, "second-subnet-is-bad"],
                    "SecurityGroupIds": ["also-bad"],
                },
            }),
            vec![
                member_entry(
                    "vpcOptions.securityGroupIds",
                    &["also-bad"],
                    &SECURITY_GROUP_ID,
                ),
                member_entry(
                    "vpcOptions.subnetIds",
                    &["second-subnet-is-bad"],
                    &SUBNET_ID,
                ),
            ],
        ),
        (
            "a top-level list, refused on a constraint that is not the pattern",
            "POST",
            "/2022-01-01/osis/revokePipelineEndpointConnections",
            json!({
                "PipelineArn": PIPELINE_ARN,
                // `ab` satisfies the pattern and is one character short of the
                // length minimum: the entry still enumerates all three
                // constraints.
                "EndpointIds": ["ab"],
            }),
            vec![member_entry("endpointIds", &["ab"], &PIPELINE_ENDPOINT_ID)],
        ),
    ];

    let router = new_router();
    for (case, method, path, body, entries) in cases {
        let answer = call(&router, method, path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a refused request must answer 400, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_type
        );
        let message = answer.message();
        assert_eq!(
            message,
            envelope(&entries),
            "case {case:?}: expected the member-level entries, got {message:?}"
        );
        assert!(
            !message.contains(".member"),
            "case {case:?}: a message must name the collection member, not an element path, \
             got {message:?}"
        );
        assert!(
            !message.contains('^') && !message.contains("?$"),
            "case {case:?}: a pattern must be stated unanchored, got {message:?}"
        );
    }
}

/// A member that is not a constrained collection keeps the framework's own
/// per-member clause: no bracketed value, no enumerated contract, anchors as the
/// model writes them.
#[tokio::test]
async fn a_scalar_member_keeps_the_frameworks_own_clause() {
    let router = new_router();
    let answer = call(
        &router,
        "POST",
        "/2022-01-01/osis/createPipelineEndpoint",
        Some(json!({
            "PipelineArn": "not-an-arn",
            "VpcOptions": {
                "SubnetIds": [VALID_SUBNET_ID],
                "SecurityGroupIds": [VALID_SECURITY_GROUP_ID],
            },
        })),
    )
    .await;
    let case = "a scalar member outside its length and its pattern";
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {:?}",
        answer.status, answer.body
    );
    let message = answer.message();
    assert!(
        message.contains("Value 'not-an-arn' at 'pipelineArn' failed to satisfy constraint: \
                          Member must have length greater than or equal to 46"),
        "case {case:?}: expected the framework's per-member clause, got {message:?}"
    );
    assert!(
        message.contains("pattern: ^arn:"),
        "case {case:?}: a scalar member's pattern stays as the model writes it, got {message:?}"
    );
    assert!(
        !message.contains("Member must satisfy constraint: ["),
        "case {case:?}: only a collection member enumerates its contract, got {message:?}"
    );
}

/// Not-found is worded per operation: the delete names the pipeline it could not
/// delete, the read names no pipeline at all.
#[tokio::test]
async fn a_missing_pipeline_is_worded_by_the_operation_that_missed_it() {
    // Names on both published `PipelineName` length bounds (`@length(3, 28)`)
    // and differing in shape, plus one that existed and was deleted.
    let cases = [
        ("shortest legal name", "abc", false),
        (
            "longest legal name",
            "a-very-long-pipeline-name-28",
            false,
        ),
        ("hyphen-and-digit name", "p-1-2-3", false),
        ("created and then deleted", "deleted-pipeline", true),
    ];

    let router = new_router();
    for (case, name, create_first) in cases {
        if create_first {
            let created = call(
                &router,
                "POST",
                "/2022-01-01/osis/createPipeline",
                Some(json!({
                    "PipelineName": name,
                    "MinUnits": 1,
                    "MaxUnits": 1,
                    "PipelineConfigurationBody": "version: \"2\"\n",
                })),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the pipeline to be created, got {} / {:?}",
                created.status, created.body
            );
            // Let the transition the clock owes come due, so the delete is not
            // refused as a conflict.
            tokio::time::sleep(PAST_TRANSITION).await;
            let deleted = call(
                &router,
                "DELETE",
                &format!("/2022-01-01/osis/deletePipeline/{name}"),
                None,
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected the pipeline to be deleted, got {} / {:?}",
                deleted.status, deleted.body
            );
        }

        let read = call(
            &router,
            "GET",
            &format!("/2022-01-01/osis/getPipeline/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected 404 from GetPipeline, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error_type
        );
        assert_eq!(
            read.message(),
            "The pipeline could not be found",
            "case {case:?}: expected the read's own wording, got {:?}",
            read.message()
        );
        assert!(
            !read.message().contains(name),
            "case {case:?}: the read must not echo the requested name, got {:?}",
            read.message()
        );

        let deleted_again = call(
            &router,
            "DELETE",
            &format!("/2022-01-01/osis/deletePipeline/{name}"),
            None,
        )
        .await;
        assert_eq!(
            deleted_again.status, 404,
            "case {case:?}: expected 404 from DeletePipeline, got {} / {:?}",
            deleted_again.status, deleted_again.body
        );
        assert_eq!(
            deleted_again.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            deleted_again.error_type
        );
        assert_eq!(
            deleted_again.message(),
            format!("The pipeline {name} could not be deleted because it does not exist"),
            "case {case:?}: expected the delete's own wording, got {:?}",
            deleted_again.message()
        );
    }
}
