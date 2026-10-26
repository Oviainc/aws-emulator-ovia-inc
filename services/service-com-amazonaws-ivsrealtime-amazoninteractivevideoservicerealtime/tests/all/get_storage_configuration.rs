// SPDX-License-Identifier: CC0-1.0

//! `GetStorageConfiguration` as a REPORTING read of this service's state.
//!
//! `tests/all/storage_configurations.rs` pins the create → read round trip, the minted
//! ARN and the listing. This suite pins what that one does not: that the read is
//! IMPLEMENTED (it answers from the record this service's state holds AT THE TIME OF THE
//! CALL, never "not implemented"), that it writes nothing, and that it tells a request it
//! cannot accept from an ARN it holds nothing for.
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
//! 4. ONE READ, ONE RESOURCE. With several configurations stored at once, each ARN reads
//!    back its own record — a read never answers a sibling's.

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

/// `GetStorageConfigurationRequest$arn`'s `@pattern`, from
/// `smithy_query(query="operation", operation="GetStorageConfiguration")`.
const ARN_PATTERN: &str = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:storage-configuration/[a-zA-Z0-9-]+$";

/// The published maximum of that same member's `@length` (`1..128`). The minimum, 1, is
/// exercised by the empty-string row below, which is one past it on the low side; an ARN
/// of exactly one character cannot be in bounds for the pattern as well, so the pattern
/// rows carry the low end.
const ARN_MAX_LENGTH: usize = 128;

/// The published bounds of `S3StorageConfiguration$bucketName`'s `@length`, from
/// `smithy_query(query="shape", shape="S3StorageConfiguration")`: `3..63`.
const BUCKET_NAME_LENGTH: (usize, usize) = (3, 63);

/// The published maximum of `StorageConfigurationName`'s `@length`, from
/// `smithy_query(query="shape", shape="StorageConfigurationName")`: `0..128`.
const NAME_MAX_LENGTH: usize = 128;

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

    /// Create a storage configuration in `region` and return the `storageConfiguration`
    /// the service reported for it.
    async fn create_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self
            .call_in(region, "CreateStorageConfiguration", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {body} in {region} must answer \
             HTTP 200, got {status} {response}"
        );
        response["storageConfiguration"].clone()
    }

    /// [`Self::create_in`] in the region the rows that do not vary it use.
    async fn create(&self, case: &str, body: &Value) -> Value {
        self.create_in(case, REGION, body).await
    }

    /// Read `arn` back in `region`, asserting the read succeeded.
    async fn get_in(&self, case: &str, region: &str, arn: &str) -> Value {
        let (status, response) = self
            .call_in(region, "GetStorageConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStorageConfiguration on {arn} in {region} must answer \
             HTTP 200, got {status} {response}"
        );
        response["storageConfiguration"].clone()
    }

    /// [`Self::get_in`] in the region the rows that do not vary it use.
    async fn get(&self, case: &str, arn: &str) -> Value {
        self.get_in(case, REGION, arn).await
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
/// differs in whether a name and tags are carried, in the form and length of the bucket
/// name, and in which character classes the name uses — including each published `@length`
/// bound of `bucketName` and the maximum of `name`.
fn create_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest request the model allows",
            json!({ "s3": { "bucketName": "a".repeat(BUCKET_NAME_LENGTH.0) } }),
        ),
        (
            "a name, and no tags",
            json!({ "s3": { "bucketName": "recordings-bucket" }, "name": "studio" }),
        ),
        (
            "a dotted bucket name and an explicitly empty tag map",
            json!({ "s3": { "bucketName": "my.recorded.videos" }, "tags": {} }),
        ),
        (
            "a bucket name at the modelled maximum length, with tags",
            json!({
                "s3": { "bucketName": "b".repeat(BUCKET_NAME_LENGTH.1) },
                "name": "max-bucket",
                "tags": { "env": "test" },
            }),
        ),
        (
            "a name using every character class the name pattern allows",
            json!({ "s3": { "bucketName": "mixed-case-name" }, "name": "Studio_9-A" }),
        ),
        (
            "a name at the modelled maximum length, with several tags",
            json!({
                "s3": { "bucketName": "long-name-bucket" },
                "name": "n".repeat(NAME_MAX_LENGTH),
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
        ("a value replaced", TagWrite::Add(json!({ "owner": "studio" }))),
        ("a key removed", TagWrite::Remove("tier")),
        (
            "a key the resource does not carry removed",
            TagWrite::Remove("tier"),
        ),
        ("another key removed", TagWrite::Remove("stack")),
        (
            "the last key this suite added removed",
            TagWrite::Remove("owner"),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the read reports live state, and changes nothing
// ---------------------------------------------------------------------------

/// What `GetStorageConfiguration` reports is the record as the service's state holds it
/// when the call arrives: each tagging write is on the next read, every other member is
/// the one the create fixed, and two reads with no write between them agree.
#[tokio::test]
async fn get_storage_configuration_reports_the_record_as_other_operations_leave_it() {
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
                        .tagging("DELETE", &configuration_arn, &format!("?tagKeys={key}"), None)
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
                "case {case:?}: a second GetStorageConfiguration must answer the first, \
                 expected {read}, got {again}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: with several configurations stored, each ARN reads back its own
// ---------------------------------------------------------------------------

/// Every row of the input class stored in ONE account at once: each ARN reads back the
/// configuration created under it, in any order, and reading one leaves the others as
/// they were — the read selects by the ARN it was given and nothing else.
#[tokio::test]
async fn each_stored_storage_configuration_reads_back_under_its_own_arn() {
    let emulator = RealTime::new();

    let mut stored: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in create_cases() {
        let created = emulator.create(case, &request).await;
        stored.push((case, member(&created, "arn"), created));
    }

    // Forwards, then backwards: the answer depends on the ARN, not on the order of calls.
    for (case, configuration_arn, created) in stored.iter().chain(stored.iter().rev()) {
        let read = emulator.get(case, configuration_arn).await;
        assert_eq!(
            &read, created,
            "case {case:?}: expected {configuration_arn} to report its own configuration, \
             expected {created}, got {read}"
        );
    }

    // No two rows share an ARN, so none of the reads above could have answered a sibling.
    for (case, configuration_arn, _) in &stored {
        let clashes = stored
            .iter()
            .filter(|(_, other, _)| other == configuration_arn)
            .count();
        assert_eq!(
            clashes, 1,
            "case {case:?}: expected {configuration_arn} to name exactly one stored \
             configuration, got {clashes}"
        );
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
async fn get_storage_configuration_tells_a_refused_arn_from_one_it_holds_nothing_for() {
    let emulator = RealTime::new();

    // Live configurations, so "not found" is never merely "the store is empty".
    let mut live: Vec<(&str, String, Value)> = Vec::new();
    for (case, request) in create_cases() {
        let created = emulator.create(case, &request).await;
        live.push((case, member(&created, "arn"), created));
    }

    let live_id = live[0]
        .1
        .rsplit('/')
        .next()
        .expect("a minted arn ends in its id")
        .to_string();

    // Each row is outside the member's modelled `@pattern` or `@length`, or absent. The
    // expected message is `Some` where exactly one constraint is violated and the
    // service's wording is therefore pinned to that constraint.
    let pattern_refusal =
        format!("GetStorageConfiguration.arn does not match the required pattern {ARN_PATTERN}");
    let refused: Vec<(&str, Value, Option<String>)> = vec![
        ("no arn at all", json!({}), None),
        ("the empty string", json!({ "arn": "" }), None),
        (
            "one character past the modelled maximum length",
            json!({
                "arn": arn(
                    REGION,
                    "storage-configuration",
                    &"a".repeat(
                        ARN_MAX_LENGTH + 1 - arn(REGION, "storage-configuration", "").len(),
                    ),
                ),
            }),
            None,
        ),
        (
            "not an arn at all",
            json!({ "arn": "storage-configuration/abcdef123456" }),
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
                "arn": format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:storage-configuration/abcdef123456"),
            }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an underscore in the id, which the pattern excludes",
            json!({ "arn": arn(REGION, "storage-configuration", "abc_def") }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an upper-case region, which the pattern excludes",
            json!({ "arn": arn("US-EAST-1", "storage-configuration", "abcdef123456") }),
            Some(pattern_refusal.clone()),
        ),
        (
            "an account that is not digits, which the pattern excludes",
            json!({
                "arn": format!("arn:aws:ivs:{REGION}:acct:storage-configuration/abcdef123456"),
            }),
            Some(pattern_refusal.clone()),
        ),
    ];
    for (case, request, expected_message) in &refused {
        let (status, response) = emulator.call("GetStorageConfiguration", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: GetStorageConfiguration {request} must answer HTTP 400, \
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
                message.starts_with("GetStorageConfiguration.arn"),
                "case {case:?}: expected a refusal naming GetStorageConfiguration.arn, \
                 got {message:?}"
            ),
        }
        assert!(
            response.get("storageConfiguration").is_none(),
            "case {case:?}: a refused read must report no configuration, got {response}"
        );
    }

    // Each row is a well-formed ARN this store holds no configuration for.
    let missing: Vec<(&str, String)> = vec![
        (
            "an id this store never minted",
            arn(REGION, "storage-configuration", "abcdef123456"),
        ),
        (
            "the id of a live configuration, in another region",
            arn("eu-west-1", "storage-configuration", &live_id),
        ),
        (
            "an id at the modelled maximum arn length",
            arn(
                REGION,
                "storage-configuration",
                &"a".repeat(ARN_MAX_LENGTH - arn(REGION, "storage-configuration", "").len()),
            ),
        ),
        (
            "the shortest id the pattern admits",
            arn(REGION, "storage-configuration", "a"),
        ),
    ];
    for (case, missing_arn) in &missing {
        // Twice: a read of an ARN the store holds nothing for leaves it holding nothing,
        // so the second read is the same refusal rather than a found resource.
        for attempt in 1..=2 {
            let (status, response) = emulator
                .call("GetStorageConfiguration", &json!({ "arn": missing_arn }))
                .await;
            assert_eq!(
                status, 404,
                "case {case:?} (read {attempt}): GetStorageConfiguration on {missing_arn} \
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
                response.get("storageConfiguration").is_none(),
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
async fn a_storage_configuration_reads_back_in_the_region_it_was_created_in() {
    let emulator = RealTime::new();

    for region in ["us-east-1", "eu-west-1", "ap-northeast-3"] {
        for (case, request) in create_cases() {
            let created = emulator.create_in(case, region, &request).await;
            let configuration_arn = member(&created, "arn");
            assert_eq!(
                configuration_arn.split(':').nth(3).unwrap_or_default(),
                region,
                "case {case:?}: expected the minted arn to name {region}, \
                 got {configuration_arn:?}"
            );

            let read = emulator.get_in(case, region, &configuration_arn).await;
            assert_eq!(
                read, created,
                "case {case:?}: a read in {region} must report the created configuration, \
                 expected {created}, got {read}"
            );
        }
    }
}
