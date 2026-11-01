// SPDX-License-Identifier: CC0-1.0

//! The one refusal every operation here that resolves a caller-supplied invocation ARN
//! answers when that ARN names no retained invocation, driven through each of them.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes. The wire harness is restated here rather than shared,
//! the way each sibling suite in this crate states its own.
//!
//! The rule under test: resolving a caller-supplied ARN is ONE rule for the whole service
//! — an ARN that names a retained invocation is served by the operation that carries it,
//! and an ARN that names none earns exactly the same refusal (HTTP status, error code and
//! message) whichever of those operations was called. The sibling suite
//! `data_automation_invocations` asserts that refusal's own values; this one asserts the
//! four operations cannot come to disagree about them, which is what a single resolve-or-
//! refuse step buys.

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

/// The error code an ARN naming no retained invocation earns (`smithy_query` operations
/// `GetDataAutomationStatus` / `TagResource` / `ListTagsForResource` / `UntagResource`,
/// which all declare `ResourceNotFoundException`).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The HTTP status this awsJson1_1 service answers that client error with — not the 404
/// the shape's advisory `smithy.api#httpError` declares. Spelled out here so a change to
/// the status decision fails this suite.
const NOT_FOUND_STATUS: u16 = 400;

/// The message that refusal carries: this service's own fixed not-found string, which
/// echoes nothing from the request. Spelled out rather than read off the crate, so a
/// wording change fails here.
const NOT_FOUND_MESSAGE: &str = "Data Automation Invocation ARN not found.";

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

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged in the store of the service that OWNS data-automation projects.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "unknown-arn-statements");
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

/// Submit one invocation and return its `invocationArn`.
async fn submit(router: &MultiServiceRouter, output: &str) -> String {
    let answer = call(
        router,
        "InvokeDataAutomationAsync",
        json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/statement.pdf"},
            "outputConfiguration": {"s3Uri": output},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a well-formed submission must be accepted, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .expect("the submission must answer with an ARN")
        .to_string()
}

/// One operation that resolves a caller-supplied invocation ARN: its name, and how it
/// carries that ARN.
type ArnKeyedOperation = (&'static str, fn(&str) -> Value);

/// Every operation this service has that resolves a caller-supplied invocation ARN.
const ARN_KEYED_OPERATIONS: [ArnKeyedOperation; 4] = [
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
async fn one_unknown_invocation_refusal_is_answered_by_every_arn_keyed_operation() {
    let router = service();
    let account = emulator_server::EMULATOR_ACCOUNT_ID;

    // A retained invocation, so one row can differ from a REAL ARN by a single character:
    // resolution is by the whole ARN, not by a prefix of it.
    let retained = submit(&router, "s3://out-bucket/unknown-arn/").await;
    let near_miss = format!("{retained}x");

    // Rows are ARNs the modelled `InvocationArn` pattern and `@length` admit — so the
    // request boundary passes them to a handler — that name no retained invocation. They
    // differ along partition, region, account, resource-id form and length, and the last
    // row is one character from a retained ARN.
    let unknown: [(&str, String); 6] = [
        (
            "this region, an id nothing minted",
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
            "the China partition, a long id",
            format!(
                "arn:aws-cn:bedrock:cn-northwest-1:{account}:data-automation-invocation/{}",
                "7".repeat(40)
            ),
        ),
        (
            "an account this emulator does not mint for",
            format!("arn:aws:bedrock:{REGION}:210987654321:data-automation-invocation/absent"),
        ),
        ("one character past a retained ARN", near_miss),
    ];

    let expected = (NOT_FOUND_STATUS, NOT_FOUND_CODE, NOT_FOUND_MESSAGE);
    for (case, arn) in &unknown {
        // Every operation's answer is collected, so the assertion is both "it is this
        // refusal" and "it is the SAME refusal across the operations".
        let mut answers: Vec<(&str, (u16, String, String))> = Vec::new();
        for (operation, build) in ARN_KEYED_OPERATIONS {
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
                 answers for one unknown ARN, got {refusal:?} against {first:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_retained_invocation_is_served_by_every_arn_keyed_operation() {
    // The other half of the resolve-or-refuse step: the ARN of an invocation this emulator
    // minted is RESOLVED, by each of the operations that answered the refusal above, and
    // the edits two of them make are visible to the reader afterwards.
    let router = service();
    let arn = submit(&router, "s3://out-bucket/retained/").await;

    let status = call(
        &router,
        "GetDataAutomationStatus",
        json!({"invocationArn": arn}),
    )
    .await;
    assert_eq!(
        status.status, 200,
        "case a retained ARN on GetDataAutomationStatus: expected 200, got {} / {}",
        status.status, status.body
    );
    assert_eq!(
        status.body["outputConfiguration"]["s3Uri"].as_str(),
        Some(crate::support::job_metadata_uri("s3://out-bucket/retained/", &arn).as_str()),
        "case a retained ARN on GetDataAutomationStatus: the record's own output \
         configuration must be reported, got {}",
        status.body
    );

    let tagged = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": "team", "value": "finance"}]}),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "case a retained ARN on TagResource: expected 200, got {} / {}",
        tagged.status, tagged.body
    );

    let listed = call(&router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        listed.status, 200,
        "case a retained ARN on ListTagsForResource: expected 200, got {} / {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["tags"],
        json!([{"key": "team", "value": "finance"}]),
        "case a retained ARN on ListTagsForResource: the tag TagResource merged must be \
         read back, got {}",
        listed.body
    );

    let untagged = call(
        &router,
        "UntagResource",
        json!({"resourceARN": arn, "tagKeys": ["team"]}),
    )
    .await;
    assert_eq!(
        untagged.status, 200,
        "case a retained ARN on UntagResource: expected 200, got {} / {}",
        untagged.status, untagged.body
    );

    let listed = call(&router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        listed.body["tags"],
        json!([]),
        "case a retained ARN after UntagResource: the removed tag must be gone, got {}",
        listed.body
    );
}
