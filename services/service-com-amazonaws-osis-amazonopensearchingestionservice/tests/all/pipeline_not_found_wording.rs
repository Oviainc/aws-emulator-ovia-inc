// SPDX-License-Identifier: CC0-1.0

//! How a name-addressed pipeline operation words its lookup miss, driven end to
//! end over HTTP.
//!
//! The rule under test: a lookup-miss refusal carries the wording the service uses
//! for THAT operation, and its only variable parts are the ones that wording has —
//! an operation whose wording is fixed text echoes nothing the request submitted,
//! neither the pipeline name nor any other member. `UpdatePipeline` is such an
//! operation: its 404 is one fixed sentence, so no name shape and no update body
//! can change it.
//!
//! The rows differ along what the rule generalises over: the operation, the
//! pipeline name's length and shape (both published `PipelineName` bounds among
//! them), which optional update members the request carries and their values, and
//! whether the name was never used or created and then deleted.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is not the emulator default, so nothing under test can depend on
/// a default-region code path.
const REGION: &str = "us-east-2";

/// The transition span the service itself uses, so a setup that waits for a
/// transition is tied to the implementation's own constant.
const TRANSITION_SPAN: Duration =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN;

/// Comfortably past [`TRANSITION_SPAN`], so a transition the clock owes has come
/// due by the time the next call runs.
const PAST_TRANSITION: Duration = TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// The wording `UpdatePipeline` answers a lookup miss with: entirely fixed text,
/// with no variable part for the request to fill.
const UPDATE_NOT_FOUND: &str = "Service returned error code ResourceNotFoundException";

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error message the response carries, under either casing a protocol may
    /// render the `message` member with.
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
    // elsewhere; these cases are about this service's behaviour.
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

async fn update_pipeline(router: &MultiServiceRouter, name: &str, body: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/updatePipeline/{name}"),
        Some(body),
    )
    .await
}

async fn get_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipeline/{name}"),
        None,
    )
    .await
}

/// Create a pipeline, let the transition the clock owes come due, then delete it,
/// so the name exists in no state by the time the row under test runs.
async fn created_and_deleted(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(json!({
            "PipelineName": name,
            "MinUnits": 1,
            "MaxUnits": 2,
            "PipelineConfigurationBody": "version: \"2\"\n",
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected the pipeline to be created, got {} / {:?}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let deleted = call(
        router,
        "DELETE",
        &format!("/2022-01-01/osis/deletePipeline/{name}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {name:?}: expected the pipeline to be deleted, got {} / {:?}",
        deleted.status, deleted.body
    );
}

/// `UpdatePipeline` against a name that resolves to nothing is refused 404 with
/// one fixed sentence, whatever the name and whatever the body carries.
#[tokio::test]
async fn an_update_of_a_missing_pipeline_is_refused_with_fixed_wording() {
    let router = new_router();

    // Rows vary the name (both `@length(3, 28)` bounds among them, and a
    // hyphen-and-digit shape), which optional members the body carries, and
    // whether the name was never used or created and then deleted. The values
    // submitted are deliberately distinct per row, so an echoed one would show.
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "never created, both counts named",
            "ghost-pipeline",
            json!({ "MinUnits": 2, "MaxUnits": 2 }),
            false,
        ),
        (
            "shortest legal name, only the maximum named",
            "abc",
            json!({ "MaxUnits": 3 }),
            false,
        ),
        (
            "longest legal name, only the minimum named",
            "a-very-long-pipeline-name-28",
            json!({ "MinUnits": 7 }),
            false,
        ),
        (
            "hyphen-and-digit name, an empty body",
            "p-1-2-3",
            json!({}),
            false,
        ),
        (
            "a configuration body and a role",
            "absent-with-config",
            json!({
                "PipelineConfigurationBody": "version: \"2\"\nlog-pipeline:\n  source:\n",
                "PipelineRoleArn": "arn:aws:iam::123456789012:role/osis-not-found-probe",
            }),
            false,
        ),
        (
            "created and then deleted",
            "recycled-pipeline",
            json!({ "MinUnits": 4, "MaxUnits": 9 }),
            true,
        ),
    ];

    for (case, name, body, create_first) in cases {
        if create_first {
            created_and_deleted(&router, name).await;
        }

        let refused = update_pipeline(&router, name, body.clone()).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected status 404, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        let message = refused.message();
        assert_eq!(
            message, UPDATE_NOT_FOUND,
            "case {case:?}: expected {UPDATE_NOT_FOUND:?}, got {message:?}"
        );
        // The wording has no variable part: nothing the request submitted — the
        // name on the path or any member of the body — appears in it.
        assert!(
            !message.contains(name),
            "case {case:?}: this wording names no pipeline, got {message:?}"
        );
        for submitted in body.as_object().expect("an object body").values() {
            let rendered = match submitted {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            assert!(
                !message.contains(&rendered),
                "case {case:?}: {rendered:?} is not part of this wording, got {message:?}"
            );
        }

        // The refused update created nothing: the name still resolves to nothing.
        let read = get_pipeline(&router, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused update must create no pipeline, got {} / {:?}",
            read.status, read.body
        );
    }
}

/// One row of the per-operation wording table: the call to make, the message it
/// must answer with, and whether that wording has the pipeline name as a variable
/// part.
struct Row {
    operation: &'static str,
    method: &'static str,
    segment: &'static str,
    body: Option<Value>,
    expected: String,
    names_pipeline: bool,
}

/// A lookup miss is worded per operation: the four name-addressed pipeline
/// operations answer the same missing name with four different messages, and only
/// the ones whose wording has a variable part name the pipeline.
#[tokio::test]
async fn each_operation_words_its_own_lookup_miss() {
    let router = new_router();

    // Rows run over two names of different length and shape, so a wording's
    // fixed text is distinguished from its variable part.
    for name in ["mis", "missing-pipeline-name-of-28"] {
        let rows: Vec<Row> = vec![
            Row {
                operation: "UpdatePipeline",
                method: "PUT",
                segment: "updatePipeline",
                body: Some(json!({ "MinUnits": 1, "MaxUnits": 1 })),
                expected: UPDATE_NOT_FOUND.to_string(),
                names_pipeline: false,
            },
            Row {
                operation: "StartPipeline",
                method: "PUT",
                segment: "startPipeline",
                body: None,
                expected: "No pipeline settings found".to_string(),
                names_pipeline: false,
            },
            Row {
                operation: "StopPipeline",
                method: "PUT",
                segment: "stopPipeline",
                body: None,
                expected: format!(
                    "The pipeline {name} could not be stopped because it does not exist"
                ),
                names_pipeline: true,
            },
            Row {
                operation: "DeletePipeline",
                method: "DELETE",
                segment: "deletePipeline",
                body: None,
                expected: format!(
                    "The pipeline {name} could not be deleted because it does not exist"
                ),
                names_pipeline: true,
            },
            Row {
                operation: "GetPipeline",
                method: "GET",
                segment: "getPipeline",
                body: None,
                expected: "The pipeline could not be found".to_string(),
                names_pipeline: false,
            },
        ];

        let mut seen: Vec<String> = Vec::new();
        for row in rows {
            let Row {
                operation,
                method,
                segment,
                body,
                expected,
                names_pipeline,
            } = row;
            let case = format!("{operation} on {name:?}");
            let refused = call(
                &router,
                method,
                &format!("/2022-01-01/osis/{segment}/{name}"),
                body,
            )
            .await;
            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected status 404, got {} / {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            let message = refused.message();
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert_eq!(
                message.contains(name),
                names_pipeline,
                "case {case:?}: expected the wording to name the pipeline = \
                 {names_pipeline}, got {message:?}"
            );
            assert!(
                !seen.contains(&message),
                "case {case:?}: this wording is shared with another operation, got {message:?}"
            );
            seen.push(message);
        }
    }
}
