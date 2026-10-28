// SPDX-License-Identifier: CC0-1.0

//! WHICH element of a list of tag keys a refusal names.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so it exercises the same
//! awsJson1_1 path a real SDK takes.
//!
//! The rule under test: a refusal about a constraint one ELEMENT of an input list violates
//! names that element in its input path — the list member, the element's 1-based position,
//! the step into the element, then the member within it for a list of structures — and a
//! request whose list carries several offending elements earns one clause per offender, in
//! position order. The rule is the request boundary's own path spelling, so this suite
//! asserts it from BOTH sides of the same member: the `@length` bound on a tag key, which
//! the boundary walk enforces, and the `@pattern` on that same key, which a handler
//! enforces because the expression's leading look-ahead cannot be compiled. One member
//! cannot read back as two different paths depending on which side caught it.

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

/// One past the modelled `TagKey` maximum length (`@length(min: 1, max: 128)`,
/// `smithy_query` shape `TagKey`): the key the request BOUNDARY refuses, so a refusal from
/// the walk can be compared with one from a handler on the same member.
const PAST_MAXIMUM_LENGTH: usize = 129;

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
/// keys themselves, so the path stops at the collection step (the same module's
/// `names.2.member`).
fn tag_keys_path(position: usize) -> String {
    format!("tagKeys.{position}.member")
}

/// The whole refusal message for tag keys outside the modelled pattern at `paths`: the AWS
/// constraint-violation envelope (`1 validation error detected: …` / `N validation errors
/// detected: …; …`) around one clause per offending element, with the offending value NOT
/// echoed.
fn expected_message(paths: &[String]) -> String {
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

/// Every input path the clauses of `message` name, in the order the message states them.
///
/// Reads the paths back out of the wire message rather than out of the service, so a
/// refusal the WALK worded and one a HANDLER worded are compared on the same footing.
fn clause_paths(message: &str) -> Vec<String> {
    message
        .split("Value at '")
        .skip(1)
        .filter_map(|rest| rest.split('\'').next().map(str::to_string))
        .collect()
}

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged in the store of the service that OWNS data-automation projects: a submission's
/// project reference is resolved against it.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "element-path-receipts");
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
fn tags_of(keys: &[&str]) -> Value {
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

/// How one operation carries a list of tag keys: the request body built from the keys, and
/// the input path an element of that list reports at.
struct Carrier {
    operation: &'static str,
    body: fn(&str, &[&str]) -> Value,
    path: fn(usize) -> String,
}

/// `TagResource` and `InvokeDataAutomationAsync` carry whole tags (`tags`);
/// `UntagResource` carries bare keys (`tagKeys`). Every operation this service has that
/// accepts a tag key is here.
const CARRIERS: [Carrier; 3] = [
    Carrier {
        operation: "TagResource",
        body: |arn, keys| json!({"resourceARN": arn, "tags": tags_of(keys)}),
        path: tags_key_path,
    },
    Carrier {
        operation: "InvokeDataAutomationAsync",
        body: |_, keys| submission("s3://out-bucket/element-path/", tags_of(keys)),
        path: tags_key_path,
    },
    Carrier {
        operation: "UntagResource",
        body: |arn, keys| json!({"resourceARN": arn, "tagKeys": keys}),
        path: tag_keys_path,
    },
];

#[tokio::test]
async fn a_malformed_tag_key_is_refused_at_its_own_element_position() {
    // Rows differ along the list's LENGTH, WHICH positions offend and HOW MANY do — what
    // the rule generalises over. The malformed keys themselves span both halves of the
    // modelled pattern (the reserved-namespace look-ahead and the character class), which
    // the sibling suite `tag_key_refusal` is about.
    //
    // (case, the keys the list carries, the 1-based positions a refusal must name).
    let rows: [(&str, &[&str], &[usize]); 7] = [
        ("one key, malformed", &["aws:owner"], &[1]),
        ("two keys, the second malformed", &["owner", "aws:cost"], &[2]),
        (
            "three keys, the middle malformed",
            &["owner", "100%done", "team"],
            &[2],
        ),
        (
            "five keys, the last malformed",
            &["a", "b", "c", "d", "team[eu]"],
            &[5],
        ),
        (
            "four keys, the first and the third malformed",
            &["aws:a", "b", "100%c", "d"],
            &[1, 3],
        ),
        (
            "three keys, every one malformed",
            &["aws:a", "b%", "c["],
            &[1, 2, 3],
        ),
        (
            "three keys, none malformed",
            &["owner", "cost-center", "team"],
            &[],
        ),
    ];

    let router = service();
    let arn = submit(&router, "s3://out-bucket/element-path/").await;

    for (case, keys, offending) in rows {
        for carrier in &CARRIERS {
            let operation = carrier.operation;
            let answer = call(&router, operation, (carrier.body)(&arn, keys)).await;
            if offending.is_empty() {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} on {operation}: every key satisfies the modelled \
                     pattern, got {} / {}",
                    answer.status, answer.body
                );
                continue;
            }
            let paths: Vec<String> = offending
                .iter()
                .map(|position| (carrier.path)(*position))
                .collect();
            let expected = expected_message(&paths);
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
async fn the_walk_and_the_handler_name_the_same_element_of_the_same_member() {
    // The two halves of the SAME member: `TagKey` carries `@length(min: 1, max: 128)`,
    // which the request boundary enforces, and a `@pattern` whose leading look-ahead the
    // boundary cannot compile, which the handler enforces. A key at `position` breaks one
    // or the other, and both refusals must name that element the same way.
    let too_long = "k".repeat(PAST_MAXIMUM_LENGTH);
    let router = service();
    let arn = submit(&router, "s3://out-bucket/element-path-agreement/").await;

    for position in [1_usize, 2, 3] {
        // A three-key list whose key at `position` is the offender and whose others are
        // ordinary keys.
        let keys = |offender: &str| -> Vec<String> {
            (1..=3)
                .map(|slot| {
                    if slot == position {
                        offender.to_string()
                    } else {
                        format!("ordinary-{slot}")
                    }
                })
                .collect()
        };

        for carrier in &CARRIERS {
            let operation = carrier.operation;
            let expected = vec![(carrier.path)(position)];

            let long_keys = keys(&too_long);
            let borrowed: Vec<&str> = long_keys.iter().map(String::as_str).collect();
            let from_walk = call(&router, operation, (carrier.body)(&arn, &borrowed)).await;
            assert_eq!(
                (from_walk.status, from_walk.error_code()),
                (400, "ValidationException"),
                "case position {position} on {operation}: a key past the modelled maximum \
                 length must be refused, got {} / {}",
                from_walk.status,
                from_walk.body
            );
            assert_eq!(
                clause_paths(from_walk.message()),
                expected,
                "case position {position} on {operation}: the boundary's length refusal \
                 must name the offending element, got {:?}",
                from_walk.message()
            );

            let malformed_keys = keys("aws:reserved");
            let borrowed: Vec<&str> = malformed_keys.iter().map(String::as_str).collect();
            let from_handler = call(&router, operation, (carrier.body)(&arn, &borrowed)).await;
            assert_eq!(
                (from_handler.status, from_handler.error_code()),
                (400, "ValidationException"),
                "case position {position} on {operation}: a key under the reserved prefix \
                 must be refused, got {} / {}",
                from_handler.status,
                from_handler.body
            );
            assert_eq!(
                clause_paths(from_handler.message()),
                expected,
                "case position {position} on {operation}: the handler's pattern refusal \
                 must name the offending element the way the boundary does, got {:?}",
                from_handler.message()
            );
        }
    }
}
