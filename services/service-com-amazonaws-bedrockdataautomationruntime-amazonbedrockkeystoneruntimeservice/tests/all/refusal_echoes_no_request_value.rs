// SPDX-License-Identifier: CC0-1.0

//! Every refusal this service emits, asserted against one rule.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes: `POST /` with
//! `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.<Op>`.
//!
//! The rule under test: a refusal this service answers with states the rule that was
//! broken and carries NOTHING from the request — no ARN, no tag key, no S3 URI the caller
//! sent is interpolated into the message, so the message of a given refusal class is the
//! same text whatever request earned it. The rows below drive one refusal class each —
//! the resource-not-found answer of all four ARN-resolving operations, the tag-key
//! pattern refusal, the boundary walk's length and pattern refusals, an unresolved
//! project reference, and the tag quota — each with a distinctive, recognisable value in
//! the request, and assert that value is absent from the answer.

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
/// `smithy_query` shape `DataAutomationProjectArn`), arranged in the owning service's
/// store by [`service`]: what tells the service WHAT to extract, which every accepted
/// submission has to name.
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/statements";

/// The most tags one resource can carry: the modelled `TagList` bound
/// (`@length(min: 0, max: 200)`, `smithy_query` shape `TagList`). Spelled out here rather
/// than read from the crate, so the rows below pin the bound as well as the wording.
const MAX_TAGS: usize = 200;

/// The message the tag quota refusal must read as: AWS's own fixed text for that error
/// class, which names no resource, no count and no limit. Spelled out here rather than
/// read from the crate, so a wrong constant in the implementation cannot make this pass.
fn expected_tag_quota_message() -> String {
    "The number of the provided tags is beyond the limit, or the number of total tags you \
     are trying to attach to the specified resource exceeds the limit."
        .to_string()
}

/// A router serving only this service over one shared engine.
///
/// [`PROJECT_ARN`] is arranged in the store of the service that OWNS data-automation
/// projects first: a submission's project reference is resolved against it, so a
/// submission naming a project nobody created is refused before any rule this suite is
/// about can be reached.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "refusal-wording-statements");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

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

/// A submission carrying every required member at its minimum, naming `project` as what
/// to extract and `tags` as its tags.
fn submission(output: &str, project: &str, tags: Value) -> Value {
    json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/statement.pdf"},
        "outputConfiguration": {"s3Uri": output},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": project},
        "tags": tags,
    })
}

/// Submit one invocation already carrying `tag_count` distinct tags, and return its
/// `invocationArn`.
async fn submit_with_tags(
    router: &MultiServiceRouter,
    case: &str,
    output: &str,
    tag_count: usize,
) -> String {
    let tags: Vec<Value> = (0..tag_count)
        .map(|i| json!({"key": format!("key{i:03}"), "value": format!("value{i:03}")}))
        .collect();
    let answer = call(
        router,
        "InvokeDataAutomationAsync",
        submission(output, PROJECT_ARN, json!(tags)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a well-formed submission must be accepted, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the submission must answer with an ARN"))
        .to_string()
}

/// How many tags the resource `arn` currently carries.
async fn tag_count(router: &MultiServiceRouter, case: &str, arn: &str) -> usize {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the resource's tags must read back, got {} / {}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry a tags list"))
        .len()
}

/// One refusal row: the case, the operation, its request body, the expected
/// `(status, error code)`, and the caller-supplied values the answer must not contain.
type RefusalCase = (String, &'static str, Value, (u16, &'static str), Vec<String>);

#[tokio::test]
async fn no_refusal_echoes_a_value_the_caller_sent() {
    let router = service();

    // Two resources, each already at the modelled `TagList` bound, reached under two
    // DIFFERENT ARNs: whatever resource the quota refusal is about, and however many
    // keys would take it past the bound, the refusal has to read the same.
    let first = submit_with_tags(&router, "the first full resource", "s3://out/a/", MAX_TAGS).await;
    let second =
        submit_with_tags(&router, "the second full resource", "s3://out/b/", MAX_TAGS).await;
    assert_ne!(
        first, second,
        "the two submissions must be two resources: got {first:?} twice"
    );

    // A well-formed ARN of the modelled shape that names no stored invocation — the
    // `TaggableResourceArn` / `InvocationArn` pattern, with an id this store never minted.
    let unknown =
        "arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/absent-job-ffffffff"
            .to_string();
    // A project reference of the modelled shape that no store holds.
    let unknown_project =
        "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/absent-project-ffffffff"
            .to_string();

    let mut cases: Vec<RefusalCase> = Vec::new();

    // The tag quota, from two different resources and two different overflowing edits:
    // the value the caller sent that the refusal could echo is the `resourceARN` and the
    // keys, so both vary between the rows.
    for (label, arn, extra) in [
        ("one key past the bound", &first, 1usize),
        ("several keys past the bound", &second, 3usize),
    ] {
        let tags: Vec<Value> = (0..extra)
            .map(|i| json!({"key": format!("overflow-key-{i}"), "value": "x"}))
            .collect();
        cases.push((
            format!("the tag quota, {label}"),
            "TagResource",
            json!({"resourceARN": arn, "tags": tags}),
            (402, "ServiceQuotaExceededException"),
            vec![
                arn.clone(),
                "overflow-key-0".to_string(),
                "data-automation-invocation/".to_string(),
            ],
        ));
    }

    // The resource-not-found answer of every operation that resolves a caller-supplied
    // ARN: four operations, one rule, and the ARN is what each of them could echo.
    for (operation, member) in [
        ("GetDataAutomationStatus", "invocationArn"),
        ("TagResource", "resourceARN"),
        ("ListTagsForResource", "resourceARN"),
        ("UntagResource", "resourceARN"),
    ] {
        let mut body = json!({member: unknown.clone()});
        if operation == "TagResource" {
            body["tags"] = json!([{"key": "owner", "value": "fin"}]);
        }
        if operation == "UntagResource" {
            body["tagKeys"] = json!(["owner"]);
        }
        cases.push((
            format!("an ARN naming no stored invocation, on {operation}"),
            operation,
            body,
            (400, "ResourceNotFoundException"),
            vec![unknown.clone(), "absent-job-ffffffff".to_string()],
        ));
    }

    // The tag-key pattern refusal, which the handler applies itself: the key is the
    // offending value, and the three operations that accept one must not echo it.
    for (operation, body) in [
        (
            "TagResource",
            json!({"resourceARN": first, "tags": [{"key": "aws:echoed-key", "value": "v"}]}),
        ),
        (
            "UntagResource",
            json!({"resourceARN": first, "tagKeys": ["aws:echoed-key"]}),
        ),
        (
            "InvokeDataAutomationAsync",
            submission(
                "s3://out/keys/",
                PROJECT_ARN,
                json!([{"key": "aws:echoed-key", "value": "v"}]),
            ),
        ),
    ] {
        cases.push((
            format!("a tag key outside the modelled pattern, on {operation}"),
            operation,
            body,
            (400, "ValidationException"),
            vec!["aws:echoed-key".to_string()],
        ));
    }

    // The boundary walk's own refusals, reworded by this service: a value too short for
    // the modelled `@length`, and one outside the modelled `@pattern`.
    cases.push((
        "an ARN under the modelled minimum length".to_string(),
        "ListTagsForResource",
        json!({"resourceARN": "echoed-short-arn"}),
        (400, "ValidationException"),
        vec!["echoed-short-arn".to_string()],
    ));
    cases.push((
        "an ARN outside the modelled pattern".to_string(),
        "ListTagsForResource",
        json!({"resourceARN": "arn:aws:s3:::echoed-bucket-name-not-an-invocation"}),
        (400, "ValidationException"),
        vec!["echoed-bucket-name-not-an-invocation".to_string()],
    ));

    // A reference to a resource another service owns, which no store holds: the refusal
    // names the reference's kind, not the ARN the caller sent.
    cases.push((
        "a submission naming a project no store holds".to_string(),
        "InvokeDataAutomationAsync",
        submission("s3://out/refs/", &unknown_project, json!([])),
        (400, "ValidationException"),
        vec![
            unknown_project.clone(),
            "absent-project-ffffffff".to_string(),
        ],
    ));

    for (case, operation, body, expected, forbidden) in &cases {
        let answer = call(&router, operation, body.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            *expected,
            "case {case:?}: expected {expected:?}, got {} / {}",
            answer.status,
            answer.body
        );
        let message = answer.message();
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must carry a message, got {}",
            answer.body
        );
        for value in forbidden {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the refusal must not echo the caller's {value:?}, got \
                 {message:?}"
            );
        }
    }

    // The quota refusal reads as this service's own fixed text, character for character,
    // for both resources: one rule, one message, no request value in it.
    for (case, operation, body, _, _) in cases.iter().take(2) {
        let answer = call(&router, operation, body.clone()).await;
        assert_eq!(
            answer.message(),
            expected_tag_quota_message(),
            "case {case:?}: expected the fixed quota wording, got {:?}",
            answer.message()
        );
    }

    // And a refused edit changed nothing: both resources still carry exactly the bound.
    for (case, arn) in [("the first full resource", &first), ("the second", &second)] {
        assert_eq!(
            tag_count(&router, case, arn).await,
            MAX_TAGS,
            "case {case:?}: a refused TagResource must leave the stored tags untouched"
        );
    }
}
