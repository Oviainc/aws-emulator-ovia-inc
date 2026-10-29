// SPDX-License-Identifier: CC0-1.0

//! WHERE in a list of tag keys a refusal can point, across the whole span of positions
//! the list's modelled `@length` bound admits.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes. The wire harness is restated here rather than shared,
//! the way each sibling suite in this crate states its own.
//!
//! The rule under test: a refusal about a constraint ONE ELEMENT of an input list violates
//! names that element's 1-based position in the request boundary's own input-path form, at
//! every position the list's modelled bound admits — a single-digit one, a multi-digit one
//! and the published maximum alike — while a refusal about the LIST ITSELF names the
//! collection member with no element segment at all. The sibling suite
//! `tag_key_refusal_element_path` spans short lists and the two halves of the modelled
//! `TagKey` expression; this one spans the list's published bound, which is where an
//! un-indexed path would leave a caller with the most elements to search.

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

/// The modelled `TagKey` pattern without the outer anchors a Smithy `@pattern` does not
/// need (`smithy_query` shape `TagKey`), as a refusal's requirement clause prints it.
const TAG_KEY_PATTERN: &str = r"(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*";

/// The most elements either list of tag keys admits: both `TagList` and `TagKeyList` carry
/// `@length(min: 0, max: 200)` (`smithy_query` shapes `TagList` / `TagKeyList`).
const MAX_LIST_LENGTH: usize = 200;

/// A tag key the modelled pattern refuses: it opens with the namespace the expression's
/// leading look-ahead reserves, which is the half the request boundary cannot compile and
/// a handler therefore applies.
const MALFORMED_KEY: &str = "aws:reserved";

/// The input path the `key` of the `tags` element at 1-based `position` occupies.
///
/// Spelled here independently of the service: a list element names its 1-based position
/// and then the collection step (`member`), and a member of that element follows
/// (`aws_smithy_schema_server::constraints::path`, whose own tests pin
/// `keySchema.2.member.keyType` for a struct element of a list).
fn tags_key_path(position: usize) -> String {
    format!("tags.{position}.member.key")
}

/// The input path the `tagKeys` element at 1-based `position` occupies: the list holds the
/// keys themselves, so the path stops at the collection step.
fn tag_keys_path(position: usize) -> String {
    format!("tagKeys.{position}.member")
}

/// The whole refusal message for tag keys outside the modelled pattern at `paths`: the AWS
/// constraint-violation envelope (`1 validation error detected: …` / `N validation errors
/// detected: …; …`) around one clause per offending element, with the offending value NOT
/// echoed.
fn expected_pattern_message(paths: &[String]) -> String {
    let clauses: Vec<String> = paths
        .iter()
        .map(|path| {
            format!(
                "Value at '{path}' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {TAG_KEY_PATTERN}"
            )
        })
        .collect();
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The refusal message a list longer than its modelled maximum earns: the same envelope,
/// around a clause naming the COLLECTION member — no element position, because no single
/// element is at fault.
fn expected_list_length_message(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must have length less than or equal to {MAX_LIST_LENGTH}"
    )
}

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged in the store of the service that OWNS data-automation projects.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "list-bound-receipts");
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
/// extract), with `tags` set to `tags`.
fn submission(output: &str, tags: Value) -> Value {
    json!({
        "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/receipt.pdf"},
        "outputConfiguration": {"s3Uri": output},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        "tags": tags,
    })
}

/// `keys` as the `tags` list a request carries: each key paired with a value the modelled
/// `TagValue` pattern admits.
fn tags_of(keys: &[String]) -> Value {
    Value::Array(
        keys.iter()
            .map(|key| json!({"key": key, "value": "v"}))
            .collect(),
    )
}

/// Submit one invocation that carries no tags and return its `invocationArn`.
async fn submit(router: &MultiServiceRouter, output: &str) -> String {
    let answer = call(router, "InvokeDataAutomationAsync", submission(output, json!([]))).await;
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

/// How one operation carries a list of tag keys: the request body built from the keys, the
/// input path an element of that list reports at, and the collection member's own name.
struct Carrier {
    operation: &'static str,
    body: fn(&str, &[String]) -> Value,
    path: fn(usize) -> String,
    member: &'static str,
}

/// `TagResource` and `InvokeDataAutomationAsync` carry whole tags (`tags`);
/// `UntagResource` carries bare keys (`tagKeys`). Every operation this service has that
/// accepts a tag key is here.
const CARRIERS: [Carrier; 3] = [
    Carrier {
        operation: "TagResource",
        body: |arn, keys| json!({"resourceARN": arn, "tags": tags_of(keys)}),
        path: tags_key_path,
        member: "tags",
    },
    Carrier {
        operation: "InvokeDataAutomationAsync",
        body: |_, keys| submission("s3://out-bucket/list-bound/", tags_of(keys)),
        path: tags_key_path,
        member: "tags",
    },
    Carrier {
        operation: "UntagResource",
        body: |arn, keys| json!({"resourceARN": arn, "tagKeys": keys}),
        path: tag_keys_path,
        member: "tagKeys",
    },
];

/// A list of `length` tag keys in which the keys at the 1-based positions `offending` are
/// malformed and every other key is an ordinary, distinct one.
fn keys_with_offenders_at(length: usize, offending: &[usize]) -> Vec<String> {
    (1..=length)
        .map(|slot| {
            if offending.contains(&slot) {
                MALFORMED_KEY.to_string()
            } else {
                format!("ordinary-{slot}")
            }
        })
        .collect()
}

#[tokio::test]
async fn a_malformed_tag_key_is_named_at_every_position_the_list_bound_admits() {
    // Rows differ along the list's LENGTH, HOW FAR into it the offender sits and HOW MANY
    // offenders there are — what the rule generalises over. The span runs from the first
    // position to the modelled list maximum, so it covers a single-digit, a two-digit and
    // a three-digit position as well as the published bound itself.
    //
    // (case, the list's length, the 1-based positions a refusal must name).
    let rows: [(&str, usize, &[usize]); 6] = [
        ("the only key is malformed", 1, &[1]),
        ("ten keys, the last malformed", 10, &[10]),
        ("forty-two keys, the last malformed", 42, &[42]),
        ("a hundred keys, the middle one malformed", 100, &[50]),
        (
            "the list at its modelled maximum, the last key malformed",
            MAX_LIST_LENGTH,
            &[MAX_LIST_LENGTH],
        ),
        (
            "the list at its modelled maximum, the first, a middle and the last malformed",
            MAX_LIST_LENGTH,
            &[1, 100, MAX_LIST_LENGTH],
        ),
    ];

    let router = service();
    let arn = submit(&router, "s3://out-bucket/list-bound/").await;

    for (case, length, offending) in rows {
        let keys = keys_with_offenders_at(length, offending);
        for carrier in &CARRIERS {
            let operation = carrier.operation;
            let paths: Vec<String> = offending
                .iter()
                .map(|position| (carrier.path)(*position))
                .collect();
            let expected = expected_pattern_message(&paths);
            let answer = call(&router, operation, (carrier.body)(&arn, &keys)).await;
            assert_eq!(
                (answer.status, answer.error_code(), answer.message()),
                (400, "ValidationException", expected.as_str()),
                "case {case:?} on {operation}: expected {:?}, got {} / {}",
                (400, "ValidationException", &expected),
                answer.status,
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_list_at_its_modelled_maximum_is_admitted_and_one_past_it_names_the_collection() {
    // The bound itself, from both sides: a list of well-formed keys AT the modelled maximum
    // is admitted, and one element past it is refused — on the COLLECTION member, with no
    // element position, because the list's length is what failed rather than any one of its
    // elements. That is the same path rule read at the other end: a path names an element
    // exactly when an element is at fault.
    let router = service();

    for carrier in &CARRIERS {
        let operation = carrier.operation;

        // A fresh resource per carrier: `TagResource` at the bound fills the resource's tag
        // list, so a shared one would make a later carrier's request a quota refusal rather
        // than the admission under test.
        let arn = submit(&router, &format!("s3://out-bucket/list-bound/{operation}/")).await;

        let at_bound = keys_with_offenders_at(MAX_LIST_LENGTH, &[]);
        let answer = call(&router, operation, (carrier.body)(&arn, &at_bound)).await;
        assert_eq!(
            answer.status,
            200,
            "case the list at its modelled maximum on {operation}: {MAX_LIST_LENGTH} \
             well-formed keys are within the modelled bound, got {} / {}",
            answer.status,
            answer.body
        );

        let past_bound = keys_with_offenders_at(MAX_LIST_LENGTH + 1, &[]);
        let expected = expected_list_length_message(carrier.member);
        let answer = call(&router, operation, (carrier.body)(&arn, &past_bound)).await;
        assert_eq!(
            (answer.status, answer.error_code(), answer.message()),
            (400, "ValidationException", expected.as_str()),
            "case one past the modelled maximum on {operation}: expected {:?}, got {} / {}",
            (400, "ValidationException", &expected),
            answer.status,
            answer.body
        );
    }
}
