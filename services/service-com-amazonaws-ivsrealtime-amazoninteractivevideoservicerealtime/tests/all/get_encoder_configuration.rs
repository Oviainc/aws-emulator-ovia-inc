// SPDX-License-Identifier: CC0-1.0

//! `GetEncoderConfiguration` as a REPORTING read of this service's state.
//!
//! `tests/all/encoder_configurations.rs` pins the create → read round trip, the minted
//! ARN and the delete. This suite pins what that one does not: that the read answers
//! from the record this service's state holds AT THE TIME OF THE CALL, and that it tells
//! a request it cannot accept apart from an ARN it holds nothing for.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. LIVE STATE. The configuration reported for an ARN is the one this service's state
//!    holds for it, so a change another operation makes to that record — the tagging
//!    surface writing, replacing or removing a key — is on the next read, while every
//!    member that operation did not touch stays as the create left it.
//! 2. PURE READ. The read itself changes nothing: consecutive reads with no write
//!    between them answer the same configuration, and a read of an ARN this store holds
//!    nothing for leaves it holding nothing.
//! 3. REFUSAL vs ABSENCE. An `arn` outside the member's modelled `@pattern`/`@length`,
//!    or missing altogether, is the modelled `ValidationException` (HTTP 400) naming the
//!    member; a well-formed `arn` this store holds no configuration for is the modelled
//!    `ResourceNotFoundException` (HTTP 404). Neither carries a configuration, and
//!    neither disturbs the configurations the store does hold.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every call of this suite is signed for, unless the row varies it.
const REGION: &str = "us-east-1";

/// `GetEncoderConfigurationRequest$arn`'s `@pattern`, from
/// `smithy_query(query="operation", operation="GetEncoderConfiguration")`.
const ARN_PATTERN: &str = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:encoder-configuration/[a-zA-Z0-9-]+$";

/// The published bounds of that same member's `@length`: `1..128`.
const ARN_LENGTH: (usize, usize) = (1, 128);

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// An operation bound to `POST /<Operation>`, signed for `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] in the region the rows that do not vary it use.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// One of the operations bound to `/tags/{resourceArn}`, carrying the ARN the way a
    /// client does: percent-encoded into one path segment.
    async fn tagging(
        &self,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            REGION,
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an encoder configuration and return the `encoderConfiguration` the service
    /// reported for it.
    async fn create(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("CreateEncoderConfiguration", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }

    /// Read `arn` back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetEncoderConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEncoderConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The `message` member of an error body, as a client reads it.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// A string member, read as the empty string when absent.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The tags of a reported configuration, as a client reads them.
fn tags_of(configuration: &Value) -> HashMap<String, String> {
    serde_json::from_value(configuration["tags"].clone()).unwrap_or_default()
}

/// A configuration with its `tags` dropped: the part of the record the tagging surface
/// must leave exactly as the create left it.
fn without_tags(configuration: &Value) -> Value {
    let mut stripped = configuration.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("tags");
    }
    stripped
}

/// The create requests this suite spans the class of stored configurations with: each row
/// differs in whether a name, a video configuration and tags are carried, and in which
/// video members the request names.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("the smallest request the model allows", json!({})),
        ("a name, no video and no tags", json!({ "name": "studio" })),
        (
            "every video member supplied, no tags",
            json!({
                "name": "full",
                "video": { "width": 854, "height": 480, "framerate": 24, "bitrate": 1_200_000 },
            }),
        ),
        (
            "one video member supplied and tags already attached",
            json!({ "video": { "width": 640 }, "tags": { "env": "test" } }),
        ),
        (
            "the documented portrait maximum resolution, with tags",
            json!({
                "name": "portrait-max",
                "video": { "width": 1080, "height": 1920 },
                "tags": { "env": "test", "team": "video" },
            }),
        ),
    ]
}

/// One write the tagging surface makes to a configuration between two reads.
#[derive(Debug)]
enum TagWrite {
    /// `TagResource` with this map.
    Add(Value),
    /// `UntagResource` naming this single key.
    Remove(&'static str),
}

/// The writes every row is driven through, in order: a first key, several more at once, a
/// replacement of a value, a removal, and a removal of a key the resource does not carry
/// (`UntagResource` is `@idempotent`).
fn tag_writes() -> Vec<(&'static str, TagWrite)> {
    vec![
        ("one key added", TagWrite::Add(json!({ "owner": "platform" }))),
        (
            "several keys added at once",
            TagWrite::Add(json!({ "tier": "gold", "stack": "prod" })),
        ),
        (
            "a value replaced",
            TagWrite::Add(json!({ "owner": "studio" })),
        ),
        ("a key removed", TagWrite::Remove("tier")),
        (
            "a key the resource does not carry removed",
            TagWrite::Remove("tier"),
        ),
        ("another key removed", TagWrite::Remove("stack")),
        ("the last key this suite added removed", TagWrite::Remove("owner")),
    ]
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the read reports live state, and changes nothing
// ---------------------------------------------------------------------------

/// What `GetEncoderConfiguration` reports is the record as the service's state holds it
/// when the call arrives: each tagging write is on the next read, every other member is
/// the one the create fixed, and two reads with no write between them agree.
#[tokio::test]
async fn get_encoder_configuration_reports_the_record_as_other_operations_leave_it() {
    let emulator = RealTime::new();

    for (case, request) in create_cases() {
        let created = emulator.create(case, &request).await;
        let configuration_arn = member(&created, "arn");
        let fixed_members = without_tags(&created);
        let mut expected_tags = tags_of(&created);

        // The read before any write is the created configuration.
        let read = emulator.get(case, &configuration_arn).await;
        assert_eq!(
            read, created,
            "case {case:?}: the first read must report the created configuration, \
             expected {created}, got {read}"
        );

        for (write_case, write) in tag_writes() {
            let case = format!("{case} / {write_case}");
            match &write {
                TagWrite::Add(tags) => {
                    let (status, response) = emulator
                        .tagging("POST", &configuration_arn, "", Some(json!({ "tags": tags })))
                        .await;
                    assert_eq!(
                        status, 204,
                        "case {case:?}: TagResource {tags} must answer HTTP 204, \
                         got {status} {response}"
                    );
                    for (key, value) in tags.as_object().expect("the row's tags are a map") {
                        expected_tags.insert(
                            key.clone(),
                            value.as_str().expect("a tag value is a string").to_string(),
                        );
                    }
                }
                TagWrite::Remove(key) => {
                    let (status, response) = emulator
                        .tagging(
                            "DELETE",
                            &configuration_arn,
                            &format!("?tagKeys={key}"),
                            None,
                        )
                        .await;
                    assert_eq!(
                        status, 204,
                        "case {case:?}: UntagResource of {key:?} must answer HTTP 204, \
                         got {status} {response}"
                    );
                    expected_tags.remove(*key);
                }
            }

            // The write is on the next read, and only the tags moved.
            let read = emulator.get(&case, &configuration_arn).await;
            let actual_tags = tags_of(&read);
            assert_eq!(
                actual_tags, expected_tags,
                "case {case:?}: expected tags {expected_tags:?}, got {actual_tags:?}"
            );
            let actual_fixed = without_tags(&read);
            assert_eq!(
                actual_fixed, fixed_members,
                "case {case:?}: a tagging write must leave every other member as created, \
                 expected {fixed_members}, got {actual_fixed}"
            );

            // The read changed nothing: with no write between them, the next read agrees.
            let again = emulator.get(&case, &configuration_arn).await;
            assert_eq!(
                again, read,
                "case {case:?}: a second GetEncoderConfiguration must answer the first, \
                 expected {read}, got {again}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a refused request and a missing resource are different answers
// ---------------------------------------------------------------------------

/// An `arn` the member's modelled constraints do not admit is HTTP 400
/// `ValidationException` naming the member; a well-formed one this store holds no
/// configuration for is HTTP 404 `ResourceNotFoundException` naming it. Neither carries a
/// configuration, neither puts anything in the store, and the live configurations are
/// untouched by both.
#[tokio::test]
async fn get_encoder_configuration_tells_a_refused_arn_from_one_it_holds_nothing_for() {
    let emulator = RealTime::new();

    // Live configurations, so "not found" is never merely "the store is empty", plus one
    // that has been deleted, so a stale ARN is a row too.
    let mut live: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in create_cases() {
        let created = emulator.create(case, &request).await;
        live.push((case, member(&created, "arn"), created));
    }
    let doomed = member(&emulator.create("a deleted configuration", &json!({})).await, "arn");
    let (status, response) = emulator
        .call("DeleteEncoderConfiguration", &json!({ "arn": doomed }))
        .await;
    assert_eq!(
        status, 200,
        "DeleteEncoderConfiguration on {doomed} must answer HTTP 200, got {status} {response}"
    );

    let live_id = live[0]
        .1
        .rsplit('/')
        .next()
        .expect("a minted arn ends in its id")
        .to_string();

    // Each row is outside the member's modelled `@pattern` or `@length`, or absent. The
    // expected message is `Some` where exactly one constraint is violated and the service's
    // wording is therefore pinned to that constraint.
    let pattern_refusal = format!("GetEncoderConfiguration.arn does not match the required pattern {ARN_PATTERN}");
    let refused: Vec<(&str, Value, Option<String>)> = vec![
        ("no arn at all", json!({}), None),
        ("the empty string", json!({ "arn": "" }), None),
        (
            "one character past the modelled maximum length",
            json!({
                "arn": arn(
                    REGION,
                    "encoder-configuration",
                    &"a".repeat(ARN_LENGTH.1 + 1 - arn(REGION, "encoder-configuration", "").len()),
                ),
            }),
            None,
        ),
        (
            "not an arn at all",
            json!({ "arn": "encoder-configuration/abcdef123456" }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an arn of another resource type",
            json!({ "arn": arn(REGION, "stage", "abcdef123456") }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an arn of a sibling service",
            json!({
                "arn": format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:encoder-configuration/abcdef123456"),
            }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an underscore in the id, which the pattern excludes",
            json!({ "arn": arn(REGION, "encoder-configuration", "abc_def") }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an upper-case region, which the pattern excludes",
            json!({ "arn": arn("US-EAST-1", "encoder-configuration", "abcdef123456") }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an account that is not digits, which the pattern excludes",
            json!({ "arn": format!("arn:aws:ivs:{REGION}:acct:encoder-configuration/abcdef123456") }),
            Some(pattern_refusal.clone()),
        ),
    ];
    for (case, request, expected_message) in &refused {
        let (status, response) = emulator.call("GetEncoderConfiguration", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetEncoderConfiguration {request} must answer HTTP 400, \
             got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} in {response}"
        );
        let message = error_message(&response);
        match expected_message {
            Some(expected) => assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            ),
            // Several constraints are violated at once, so only the member the refusal is
            // about is pinned here.
            None => assert!(
                message.starts_with("GetEncoderConfiguration.arn"),
                "case {case:?}: expected a refusal naming GetEncoderConfiguration.arn, \
                 got {message:?}"
            ),
        }
        assert!(
            response.get("encoderConfiguration").is_none(),
            "case {case:?}: a refused read must report no configuration, got {response}"
        );
    }

    // Each row is a well-formed ARN this store holds no configuration for.
    let missing: Vec<(&str, String)> = vec![
        (
            "an id this store never minted",
            arn(REGION, "encoder-configuration", "abcdef123456"),
        ),
        ("an already-deleted configuration", doomed.clone()),
        (
            "the id of a live configuration, in another region",
            arn("eu-west-1", "encoder-configuration", &live_id),
        ),
        (
            "an id at the modelled maximum arn length",
            arn(
                REGION,
                "encoder-configuration",
                &"a".repeat(ARN_LENGTH.1 - arn(REGION, "encoder-configuration", "").len()),
            ),
        ),
        (
            "the shortest id the pattern admits",
            arn(REGION, "encoder-configuration", "a"),
        ),
    ];
    for (case, missing_arn) in &missing {
        // Twice: a read of an ARN the store holds nothing for leaves it holding nothing,
        // so the second read is the same refusal rather than a found resource.
        for attempt in 1..=2 {
            let (status, response) = emulator
                .call("GetEncoderConfiguration", &json!({ "arn": missing_arn }))
                .await;
            assert_eq!(
                status, 404,
                "case {case:?} (read {attempt}): GetEncoderConfiguration on {missing_arn} \
                 must answer HTTP 404, got {status} {response}"
            );
            let code = error_code(&response);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?} (read {attempt}): expected ResourceNotFoundException, \
                 got {code:?} in {response}"
            );
            let message = error_message(&response);
            let expected = format!("Resource: {missing_arn} not found");
            assert_eq!(
                message, expected,
                "case {case:?} (read {attempt}): expected the message {expected:?}, \
                 got {message:?}"
            );
            assert!(
                response.get("encoderConfiguration").is_none(),
                "case {case:?} (read {attempt}): a refused read must report no \
                 configuration, got {response}"
            );
        }
    }

    // Every configuration the store does hold still reads back exactly as created: the
    // refused and missing reads disturbed nothing.
    for (case, configuration_arn, created) in &live {
        let read = emulator.get(case, configuration_arn).await;
        assert_eq!(
            &read, created,
            "case {case:?}: the refused reads must leave {configuration_arn} as created, \
             expected {created}, got {read}"
        );
    }
}

/// A configuration created in one region reads back, in that region, as the create
/// reported it: the record the read answers from is keyed by the ARN the create minted,
/// so the rule holds whatever region the resource lives in.
#[tokio::test]
async fn an_encoder_configuration_reads_back_in_the_region_it_was_created_in() {
    let emulator = RealTime::new();

    for region in ["us-east-1", "eu-west-1", "ap-northeast-3"] {
        for (case, request) in create_cases() {
            let (status, response) = emulator
                .call_in(region, "CreateEncoderConfiguration", &request)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateEncoderConfiguration in {region} must answer HTTP 200, \
                 got {status} {response}"
            );
            let created = response["encoderConfiguration"].clone();
            let configuration_arn = member(&created, "arn");

            let (status, response) = emulator
                .call_in(
                    region,
                    "GetEncoderConfiguration",
                    &json!({ "arn": configuration_arn }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a read in {region} must answer HTTP 200, got {status} {response}"
            );
            let read = response["encoderConfiguration"].clone();
            assert_eq!(
                read, created,
                "case {case:?}: a read in {region} must report the created configuration, \
                 expected {created}, got {read}"
            );
        }
    }
}
