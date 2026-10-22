// SPDX-License-Identifier: CC0-1.0

//! The pattern a `ValidationException` QUOTES, driven end to end over HTTP.
//!
//! The rule under test: when this service refuses a value for its member's
//! `@pattern`, the clause quotes the pattern the service PUBLISHES for that
//! string shape — the model's regex with its anchors dropped, a redundantly
//! escaped `-` inside a character class written literally, and a trailing
//! one-or-more repetition replaced by the explicit repetition range the shape's
//! `@length` bounds pin it down to — never the model's regex literal. Where the
//! model pins no range down (no trailing one-or-more repetition, or no length
//! maximum) the unanchored find-form is quoted and the length bounds are stated
//! by their own clauses.
//!
//! The rows span every patterned string shape this service declares, the
//! operations that expose them (a URI label, a body member, a nested body member
//! and a query parameter), and both sides of the rule: a bound that lands on the
//! repetition and a pattern that pins none down.
//!
//! The published patterns asserted here are written out from the model, so a
//! message is compared against the model rather than against the code that
//! renders it. Source:
//! `smithy_query(query="shape", shape="com.amazonaws.osis#PipelineName,com.amazonaws.osis#PipelineEndpointId,com.amazonaws.osis#LogGroup,com.amazonaws.osis#PipelineArn,com.amazonaws.osis#PipelineRoleArn")`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is not the emulator default, so nothing under test can depend
/// on a default-region code path.
const REGION: &str = "eu-west-2";

/// `com.amazonaws.osis#PipelineName`: `@length(3, 28)`, `@pattern("^[a-z][a-z0-9\-]+$")`.
/// One leading character plus 2..27 more is the 3..28 the length bound states.
const PIPELINE_NAME: &str = r"[a-z][a-z0-9-]{2,27}";

/// `com.amazonaws.osis#PipelineEndpointId`: `@length(3, 512)`,
/// `@pattern("^[a-zA-Z0-9][a-zA-Z0-9-_]+$")`.
const PIPELINE_ENDPOINT_ID: &str = r"[a-zA-Z0-9][a-zA-Z0-9-_]{2,511}";

/// `com.amazonaws.osis#LogGroup`: `@length(1, 512)`,
/// `@pattern("^\/aws\/vendedlogs\/[\.\-_/#A-Za-z0-9]+$")`. The fixed prefix is 16
/// characters, so 512 leaves 496 repetitions, and the minimum of 1 is already
/// covered by the prefix. The `\-` in the middle of the class stays escaped:
/// unescaped there it would read as a range.
const LOG_GROUP: &str = r"\/aws\/vendedlogs\/[\.\-_/#A-Za-z0-9]{1,496}";

/// `com.amazonaws.osis#PipelineArn`: `@length(46, 76)`,
/// `@pattern("^arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b):osis:.+:pipeline\/.+$")`.
/// The alternation leaves no fixed width, so no repetition range is stated.
const PIPELINE_ARN: &str =
    r"arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b):osis:.+:pipeline\/.+";

/// `com.amazonaws.osis#PipelineRoleArn`: `@length(20, 2048)`,
/// `@pattern("^arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b|aws\-iso\-e|aws\-iso\-f):iam::[0-9]+:role\/.*$")`.
/// Its tail is optional (`.*`), not a one-or-more repetition, so it is left as
/// the model writes it.
const PIPELINE_ROLE_ARN: &str = r"arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b|aws\-iso\-e|aws\-iso\-f):iam::[0-9]+:role\/.*";

/// A `PipelineConfigurationBody` the model accepts, so a `CreatePipeline` row is
/// refused for the member under test alone.
const CONFIGURATION_BODY: &str = "version: \"2\"\n";

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

/// The clause a pattern refusal states for one member.
fn pattern_clause(value: &str, path: &str, published: &str) -> String {
    format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {published}"
    )
}

/// A value refused for its member's pattern alone quotes the published pattern.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_published_pattern() {
    // Every row's value satisfies its member's length bounds and fails only the
    // pattern, so the message states exactly one error. The rows vary the
    // operation, where the member is bound (URI label, body member, nested body
    // member, query parameter), the shape under test and how the value offends.
    let cases: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "a label member: an uppercase name",
            "DELETE",
            "/2022-01-01/osis/deletePipeline/Sim2Real-Behav-Invalid".to_string(),
            None,
            pattern_clause("Sim2Real-Behav-Invalid", "pipelineName", PIPELINE_NAME),
        ),
        (
            "the same shape under a read",
            "GET",
            "/2022-01-01/osis/getPipeline/name_with_underscore".to_string(),
            None,
            pattern_clause("name_with_underscore", "pipelineName", PIPELINE_NAME),
        ),
        (
            "the same shape under a state change",
            "PUT",
            "/2022-01-01/osis/startPipeline/0-leads-with-a-digit".to_string(),
            None,
            pattern_clause("0-leads-with-a-digit", "pipelineName", PIPELINE_NAME),
        ),
        (
            "a body member rather than a label",
            "POST",
            "/2022-01-01/osis/createPipeline".to_string(),
            Some(json!({
                "PipelineName": "Upper-Case-Pipeline",
                "MinUnits": 1,
                "MaxUnits": 2,
                "PipelineConfigurationBody": CONFIGURATION_BODY,
            })),
            pattern_clause("Upper-Case-Pipeline", "pipelineName", PIPELINE_NAME),
        ),
        (
            "another shape whose bound lands on the repetition",
            "DELETE",
            "/2022-01-01/osis/deletePipelineEndpoint/_leading-underscore".to_string(),
            None,
            pattern_clause("_leading-underscore", "endpointId", PIPELINE_ENDPOINT_ID),
        ),
        (
            "a nested body member with a long fixed prefix",
            "POST",
            "/2022-01-01/osis/createPipeline".to_string(),
            Some(json!({
                "PipelineName": "logging-pipeline",
                "MinUnits": 1,
                "MaxUnits": 2,
                "PipelineConfigurationBody": CONFIGURATION_BODY,
                "LogPublishingOptions": {
                    "IsLoggingEnabled": true,
                    "CloudWatchLogDestination": {"LogGroup": "/aws/wrongprefix/group"},
                },
            })),
            pattern_clause(
                "/aws/wrongprefix/group",
                "logPublishingOptions.cloudWatchLogDestination.logGroup",
                LOG_GROUP,
            ),
        ),
        (
            "a member whose pattern pins no repetition down",
            "POST",
            "/2022-01-01/osis/createPipeline".to_string(),
            Some(json!({
                "PipelineName": "role-pipeline",
                "MinUnits": 1,
                "MaxUnits": 2,
                "PipelineConfigurationBody": CONFIGURATION_BODY,
                // Long enough for `@length(20, 2048)`, and no `:role/` part.
                "PipelineRoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            })),
            pattern_clause(
                "arn:aws:iam::000000000000:user/not-a-role",
                "pipelineRoleArn",
                PIPELINE_ROLE_ARN,
            ),
        ),
        (
            "a query-parameter member whose pattern pins no repetition down",
            "GET",
            // Length 49, inside `@length(46, 76)`, and not an OSIS pipeline arn.
            "/2022-01-01/osis/listTagsForResource?arn=arn:aws:ec2:eu-west-2:000000000000:instance/i-abc"
                .to_string(),
            None,
            pattern_clause(
                "arn:aws:ec2:eu-west-2:000000000000:instance/i-abc",
                "arn",
                PIPELINE_ARN,
            ),
        ),
    ];

    let router = new_router();
    for (case, method, path, body, clause) in cases {
        let answer = call(&router, method, &path, body).await;
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
            format!("1 validation error detected: {clause}"),
            "case {case:?}: expected the published pattern, got {message:?}"
        );
        assert!(
            !message.contains('^') && !message.contains("+$"),
            "case {case:?}: a published pattern carries no anchors, got {message:?}"
        );
    }
}

/// A value that fails its length bound as well states both clauses: the length
/// bounds keep their own clauses, and the pattern clause is still the published
/// one.
#[tokio::test]
async fn a_length_refusal_keeps_its_own_clause_beside_the_published_pattern() {
    let cases = [
        (
            "shorter than the minimum, and outside the pattern",
            "DELETE",
            "/2022-01-01/osis/deletePipeline/A",
            "A",
            "pipelineName",
            PIPELINE_NAME,
            "Member must have length greater than or equal to 3",
        ),
        (
            "longer than the maximum, and outside the pattern",
            "DELETE",
            "/2022-01-01/osis/deletePipeline/A-Pipeline-Name-Of-Twenty-Nine",
            "A-Pipeline-Name-Of-Twenty-Nine",
            "pipelineName",
            PIPELINE_NAME,
            "Member must have length less than or equal to 28",
        ),
        (
            "a member whose pattern pins no repetition down",
            "GET",
            "/2022-01-01/osis/listTagsForResource?arn=not-an-arn",
            "not-an-arn",
            "arn",
            PIPELINE_ARN,
            "Member must have length greater than or equal to 46",
        ),
    ];

    let router = new_router();
    for (case, method, path, value, member, published, length_clause) in cases {
        let answer = call(&router, method, path, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a refused request must answer 400, got {} / {:?}",
            answer.status, answer.body
        );
        let message = answer.message();
        assert!(
            message.starts_with("2 validation errors detected: "),
            "case {case:?}: expected both clauses in one message, got {message:?}"
        );
        assert!(
            message.contains(&format!(
                "Value '{value}' at '{member}' failed to satisfy constraint: {length_clause}"
            )),
            "case {case:?}: expected the length clause, got {message:?}"
        );
        assert!(
            message.contains(&pattern_clause(value, member, published)),
            "case {case:?}: expected the published pattern, got {message:?}"
        );
    }
}
