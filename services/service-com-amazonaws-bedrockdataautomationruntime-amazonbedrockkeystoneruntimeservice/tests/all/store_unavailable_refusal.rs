// SPDX-License-Identifier: CC0-1.0

//! The one answer every implemented operation here gives when the engine it is mounted on
//! holds no store of this service's type, driven through each of them.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes. The wire harness is restated here rather than shared,
//! the way each sibling suite in this crate states its own.
//!
//! The rule under test: reaching this service's store is ONE decision for the whole
//! service — an engine that holds the store serves the operation that reached for it, and
//! an engine that holds none earns exactly the same refusal (HTTP status, error code and
//! message) whichever operation was called. The store-bearing half is what every other
//! suite in this crate drives; this one adds the half no engine this emulator mounts can
//! reach, so the five handlers cannot come to disagree about it unnoticed.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// A legal `dataAutomationConfiguration.dataAutomationProjectArn` (pattern from
/// `smithy_query` shape `DataAutomationProjectArn`): what tells the service WHAT to
/// extract, which every accepted submission has to name.
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/statements";

/// The error code an engine holding no store of this service's type earns (`smithy_query`
/// operations `InvokeDataAutomationAsync` / `GetDataAutomationStatus` / `TagResource` /
/// `ListTagsForResource` / `UntagResource`, which all declare
/// `InternalServerException`).
const UNAVAILABLE_CODE: &str = "InternalServerException";

/// The HTTP status that refusal is answered with: the one the `InternalServerException`
/// shape's own `smithy.api#httpError` declares (500, per each operation's modelled
/// contract). Spelled out here so a change to the status decision fails this suite.
const UNAVAILABLE_STATUS: u16 = 500;

/// The message that refusal carries: this service's own fixed store-unavailable string,
/// which echoes nothing from the request. Spelled out rather than read off the crate, so a
/// wording change fails here.
const UNAVAILABLE_MESSAGE: &str = "the data automation runtime store is unavailable on this engine";

/// An engine kind this emulator does not mount, so it holds no store of any service's
/// type: what a handler's reach-or-refuse step has to answer the refusal for.
#[derive(Debug)]
struct StorelessEngine;

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code: the body's `__type`, local name only.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split('#')
            .next_back()
            .unwrap_or_default()
    }

    /// The error message the body carries, or the empty string.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The whole refusal a caller branches on: status, error code and message.
    fn refusal(&self) -> (u16, &str, &str) {
        (self.status, self.error_code(), self.message())
    }
}

/// A router serving only this service over an engine that holds no store at all.
fn storeless_service() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::new(StorelessEngine)));
    router
}

/// A router serving only this service over the shared in-memory engine, with
/// [`PROJECT_ARN`] arranged in the store of the service that OWNS data-automation
/// projects.
fn stored_service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "store-reach-statements");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// Call one operation on `router` the way an SDK would.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// The submission body every row below uses to reach a handler: well formed, so the
/// boundary walk passes it and the handler's own store reach is what decides the answer.
fn submission() -> Value {
    json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/statement.pdf"},
        "outputConfiguration": {"s3Uri": "s3://out-bucket/store-reach/"},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    })
}

/// One operation that reaches this service's store: its name, and a well-formed body for
/// it carrying `arn` wherever it names an invocation.
type StoreReachingOperation = (&'static str, fn(&str) -> Value);

/// Every implemented operation of this service — one per former copy of the
/// store-unavailable branch.
const STORE_REACHING_OPERATIONS: [StoreReachingOperation; 5] = [
    ("InvokeDataAutomationAsync", |_| submission()),
    ("GetDataAutomationStatus", |arn| {
        json!({"invocationArn": arn})
    }),
    ("ListTagsForResource", |arn| json!({"resourceARN": arn})),
    ("TagResource", |arn| {
        json!({"resourceARN": arn, "tags": [{"key": "team", "value": "finance"}]})
    }),
    ("UntagResource", |arn| {
        json!({"resourceARN": arn, "tagKeys": ["team"]})
    }),
];

#[tokio::test]
async fn one_store_unavailable_refusal_is_answered_by_every_operation() {
    let router = storeless_service();
    let account = emulator_server::EMULATOR_ACCOUNT_ID;

    // Rows are ARNs the modelled `InvocationArn` / `TaggableResourceArn` pattern and
    // `@length` admit, so the request boundary passes them to a handler. They differ along
    // partition, region, account and resource-id form, and `InvokeDataAutomationAsync`
    // carries none of them at all: whatever the request named, an engine with no store of
    // this service's type can resolve nothing, so the answer is the same refusal.
    let arns: [(&str, String); 4] = [
        (
            "this region, a plain id",
            format!("arn:aws:bedrock:{REGION}:{account}:data-automation-invocation/absent"),
        ),
        (
            "another region, a single-digit id",
            format!("arn:aws:bedrock:eu-central-1:{account}:data-automation-invocation/7"),
        ),
        (
            "the GovCloud partition, every id character class",
            format!(
                "arn:aws-us-gov:bedrock:us-gov-east-1:{account}:data-automation-invocation/aZ_9-x"
            ),
        ),
        (
            "an account this emulator does not mint for",
            format!("arn:aws:bedrock:{REGION}:210987654321:data-automation-invocation/absent"),
        ),
    ];

    let expected = (UNAVAILABLE_STATUS, UNAVAILABLE_CODE, UNAVAILABLE_MESSAGE);
    for (case, arn) in &arns {
        // Every operation's answer is collected, so the assertion is both "it is this
        // refusal" and "it is the SAME refusal across the operations".
        let mut answers: Vec<(&str, (u16, String, String))> = Vec::new();
        for (operation, build) in STORE_REACHING_OPERATIONS {
            let answer = call(&router, operation, build(arn)).await;
            assert_eq!(
                answer.refusal(),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?} / {}",
                answer.refusal(),
                answer.body
            );
            let (status, code, message) = answer.refusal();
            answers.push((operation, (status, code.to_string(), message.to_string())));
        }
        let (first_operation, first) = &answers[0];
        for (operation, refusal) in &answers[1..] {
            assert_eq!(
                refusal, first,
                "case {case:?}: {operation} must answer exactly what {first_operation} \
                 answers on an engine holding no store, got {refusal:?} against {first:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_operation_is_served_when_the_engine_holds_the_store() {
    // The other half of the reach-or-refuse step: on the engine this emulator does mount,
    // each of the five operations that answered the refusal above reaches the store and is
    // served from it, so no row can be passing above because its handler stopped early.
    let router = stored_service();

    let submitted = call(&router, "InvokeDataAutomationAsync", submission()).await;
    assert_eq!(
        submitted.status, 200,
        "case the store-bearing engine on InvokeDataAutomationAsync: expected 200, got {} / {}",
        submitted.status, submitted.body
    );
    let arn = submitted.body["invocationArn"]
        .as_str()
        .expect("the submission must answer with an ARN")
        .to_string();

    for (operation, build) in STORE_REACHING_OPERATIONS {
        if operation == "InvokeDataAutomationAsync" {
            continue;
        }
        let answer = call(&router, operation, build(&arn)).await;
        assert_eq!(
            answer.status, 200,
            "case the store-bearing engine on {operation}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.error_code(),
            UNAVAILABLE_CODE,
            "case the store-bearing engine on {operation}: the store must be reached, got {}",
            answer.body
        );
    }

    // The tag the table's `TagResource` row merged is readable afterwards, and the one its
    // `UntagResource` row removed is gone: the operations above were served FROM the store,
    // not merely answered 200.
    let listed = call(&router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        listed.body["tags"],
        json!([]),
        "case the store-bearing engine after TagResource then UntagResource: the edits must \
         be visible in the store, got {}",
        listed.body
    );
}
