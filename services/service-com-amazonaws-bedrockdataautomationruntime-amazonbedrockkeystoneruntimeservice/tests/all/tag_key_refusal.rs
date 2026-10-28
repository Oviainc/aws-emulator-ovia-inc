// SPDX-License-Identifier: CC0-1.0

//! The modelled `TagKey` refusal, asserted over every operation that accepts a tag key.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes.
//!
//! The rule under test: a request carrying a tag key outside the modelled `TagKey`
//! pattern is refused with `ValidationException` (HTTP 400) whose message names the input
//! path the key arrived at — including the 1-based position of the list element it came in
//! (`tag_key_refusal_element_path` is the suite that spans those positions) — and that
//! refusal is the SAME for every operation that accepts a tag key — the submission that mints a resource, the operation that adds tags and the
//! one that removes them. The three handlers reach it through one helper, so the rows
//! below drive that one path from each of them and compare the answers to each other, not
//! only to a constant.

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
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/receipts";

/// The refusal message a malformed tag key at input path `path` must read as: the
/// `ValidationException` envelope around one clause that names the member and the
/// constraint it failed, with the declared `TagKey` pattern (`smithy_query` shape
/// `TagKey`) printed without the outer anchors a Smithy `@pattern` does not need, and the
/// offending value NOT echoed.
fn expected_refusal(path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: \
         (?!aws:)[\\p{{L}}\\p{{Z}}\\p{{N}}_.:/=+\\-@]*"
    )
}

/// A router serving only this service over one shared engine.
///
/// [`PROJECT_ARN`] is arranged in the store of the service that OWNS data-automation
/// projects first: a submission's project reference is resolved against it, so a
/// submission naming a project nobody created is refused before any rule this suite is
/// about can be reached.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "tag-key-refusal-receipts");
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

/// A submission carrying every required member (and the project that says what to
/// extract) at its minimum, optionally with `tags`.
fn submission(output: &str, tags: Option<Value>) -> Value {
    let mut body = json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/receipt.pdf"},
        "outputConfiguration": {"s3Uri": output},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    body
}

/// Submit one invocation that carries no tags and return its `invocationArn`.
async fn submit(router: &MultiServiceRouter, case: &str, output: &str) -> String {
    let answer = call(router, "InvokeDataAutomationAsync", submission(output, None)).await;
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

/// The tag keys the resource `arn` currently carries, sorted.
async fn tag_keys(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<String> {
    let answer = call(router, "ListTagsForResource", json!({"resourceARN": arn})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the resource's tags must read back, got {} / {}",
        answer.status, answer.body
    );
    let mut keys: Vec<String> = answer.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry a tags list"))
        .iter()
        .map(|tag| {
            tag["key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every reported tag has a key"))
                .to_string()
        })
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn every_tag_key_accepting_operation_states_the_tag_key_refusal_identically() {
    // Rows span both halves of the modelled `TagKey` pattern
    // `^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$` (`smithy_query` shape `TagKey`): the
    // reserved-namespace lookahead (and near-misses of it, which it does NOT exclude),
    // and characters inside and outside the character class — Unicode letters, numbers
    // and separators it admits, and symbols, control characters and ASCII punctuation it
    // does not. Keys cited by the report are not what the rows turn on: each row below
    // is a different way to be inside or outside the pattern.
    //
    // The modelled `TagKey` upper length bound (`@length(min: 1, max: 128)`,
    // `smithy_query` shape `TagKey`): a key AT the bound is still admitted, and the walk
    // refuses one past it with its own length refusal, which is not this rule.
    let at_maximum_length = "k".repeat(128);

    // (case, tag key, whether the pattern admits it).
    let keys: [(&str, &str, bool); 14] = [
        ("a plain ASCII key", "owner", true),
        (
            "every punctuation character the class lists",
            "a_b.c:d/e=f+g-h@i",
            true,
        ),
        ("a CJK letter key (\\p{L})", "請求書", true),
        ("a number-category key (\\p{N}, No)", "share\u{bc}", true),
        (
            "a non-breaking separator inside the key (\\p{Z}, Zs)",
            "cost\u{a0}center",
            true,
        ),
        (
            // The lookahead excludes `aws:` only: the same letters without the colon,
            // and the same prefix in another case, are ordinary keys.
            "the reserved letters without the reserved colon",
            "awsome",
            true,
        ),
        ("the reserved prefix in another case", "AWS:team", true),
        (
            "the key at the modelled maximum length",
            at_maximum_length.as_str(),
            true,
        ),
        ("the reserved prefix exactly", "aws:", false),
        (
            "a key under the reserved prefix",
            "aws:cloudformation:stack-id",
            false,
        ),
        (
            // A control character is White_Space but not `\p{Z}`.
            "a tab inside the key, outside the class",
            "cost\tcenter",
            false,
        ),
        ("a percent sign, outside the class", "100%done", false),
        ("a bracket, outside the class", "team[eu]", false),
        (
            // A symbol (`Sm`) is in none of the three categories the class names.
            "a math symbol, outside the class",
            "budget\u{2264}cap",
            false,
        ),
    ];

    let router = service();
    let arn = submit(&router, "tag key refusal", "s3://out-bucket/tag-key-refusal/").await;
    let mut admitted_keys: Vec<String> = Vec::new();

    for (case, key, admitted) in keys {
        // One row per operation that accepts a tag key, with the input path that
        // operation's request carries it at. `InvokeDataAutomationAsync` and
        // `TagResource` carry whole tags; `UntagResource` carries bare keys.
        // Every row below submits a ONE-element list, so the element the path names is at
        // position 1; the positions themselves are spanned by
        // `tag_key_refusal_element_path`.
        let operations: [(&str, Value, &str); 3] = [
            (
                "InvokeDataAutomationAsync",
                submission(
                    "s3://out-bucket/tag-key-refusal/",
                    Some(json!([{"key": key, "value": "v"}])),
                ),
                "tags.1.member.key",
            ),
            (
                "TagResource",
                json!({"resourceARN": arn, "tags": [{"key": key, "value": "v"}]}),
                "tags.1.member.key",
            ),
            (
                "UntagResource",
                json!({"resourceARN": arn, "tagKeys": [key]}),
                "tagKeys.1.member",
            ),
        ];

        for (operation, body, path) in operations {
            let answer = call(&router, operation, body).await;
            if admitted {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} ({key:?}) on {operation}: the modelled TagKey pattern \
                     admits this key, got {} / {}",
                    answer.status,
                    answer.body
                );
                continue;
            }
            assert_eq!(
                (answer.status, answer.error_code(), answer.message()),
                (400, "ValidationException", expected_refusal(path).as_str()),
                "case {case:?} ({key:?}) on {operation}: expected {:?}, got {} / {}",
                (400, "ValidationException", expected_refusal(path)),
                answer.status,
                answer.body
            );
        }

        if admitted {
            admitted_keys.push(key.to_string());
        }
    }

    // The two operations that carry whole tags state the refusal character for character
    // alike — one rule, one wording, whichever operation caught it.
    let refused = "aws:refused-by-both";
    let from_submission = call(
        &router,
        "InvokeDataAutomationAsync",
        submission(
            "s3://out-bucket/tag-key-refusal/",
            Some(json!([{"key": refused, "value": "v"}])),
        ),
    )
    .await;
    let from_tagging = call(
        &router,
        "TagResource",
        json!({"resourceARN": arn, "tags": [{"key": refused, "value": "v"}]}),
    )
    .await;
    assert_eq!(
        (from_submission.status, from_submission.message()),
        (from_tagging.status, from_tagging.message()),
        "case {refused:?}: InvokeDataAutomationAsync and TagResource must state the same \
         refusal, got {} / {:?} and {} / {:?}",
        from_submission.status,
        from_submission.message(),
        from_tagging.status,
        from_tagging.message()
    );

    // Every admitted key reached the tag map and every refused one did not: the last
    // `UntagResource` row removed each admitted key again, so re-adding them all and
    // reading them back is what the resource must report.
    let answer = call(
        &router,
        "TagResource",
        json!({
            "resourceARN": arn,
            "tags": admitted_keys
                .iter()
                .map(|key| json!({"key": key, "value": "v"}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "re-adding every admitted key must be accepted, got {} / {}",
        answer.status, answer.body
    );
    let mut expected = admitted_keys.clone();
    expected.sort();
    assert_eq!(
        tag_keys(&router, "tag key refusal", &arn).await,
        expected,
        "the resource must carry exactly the admitted keys, and no refused one"
    );
}
