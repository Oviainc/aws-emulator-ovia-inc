// SPDX-License-Identifier: CC0-1.0

//! The media capture pipeline lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the wire: the suite builds the real
//! `MultiServiceRouter`, mounts this service on one shared engine, and dispatches
//! `http::Request`s, so what it pins is the emulator's HTTP behaviour and not an
//! internal helper.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a create MINTS and PERSISTS a pipeline, and every later read of that id
//!   renders from the stored record;
//! * a read of an id the store does not hold is the modelled 404, and a value
//!   outside a modelled constraint is the modelled 400;
//! * paging reports every created-and-not-deleted pipeline exactly once, in
//!   creation order, with a resume token present only while more remain;
//! * a repeated `ClientRequestToken` answers with the first pipeline;
//! * a delete is observable by every later read;
//! * the store refuses to grow past its stated maximum, with the modelled quota
//!   error, and keeps serving reads once it is full.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::ops::types::MediaCapturePipeline;
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    INITIALIZING_TO_IN_PROGRESS, MAX_CAPTURE_PIPELINES,
};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for. The ARN a create derives must carry it, so it
/// is deliberately NOT the emulator's `us-east-1` default.
const REGION: &str = "us-west-2";

/// The canonical placeholder account every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed `MediaPipelineId` (36-char GUID) that no create in this suite
/// mints, for the "unknown but well-formed id" rows.
const UNKNOWN_ID: &str = "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e";

/// One request/response round trip through the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one request and return its status and parsed body (`Value::Null`
    /// for an empty body, which is what a 204 carries).
    async fn call(&self, method: &str, path_and_query: &str, body: Option<Value>) -> Response {
        let payload = body.map_or_else(Vec::new, |b| {
            serde_json::to_vec(&b).expect("a JSON value serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://media-pipelines-chime.{REGION}.amazonaws.com{path_and_query}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTSUITE/20261021/{REGION}/chime/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {path_and_query} answered {status} with a non-JSON body: {e}: {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        Response {
            status,
            error_type,
            body,
        }
    }

    async fn create(&self, input: Value) -> Response {
        self.call("POST", "/sdk-media-capture-pipelines", Some(input))
            .await
    }

    async fn get_capture(&self, id: &str) -> Response {
        self.call("GET", &format!("/sdk-media-capture-pipelines/{id}"), None)
            .await
    }

    async fn get_pipeline(&self, id: &str) -> Response {
        self.call("GET", &format!("/sdk-media-pipelines/{id}"), None)
            .await
    }

    async fn delete_capture(&self, id: &str) -> Response {
        self.call(
            "DELETE",
            &format!("/sdk-media-capture-pipelines/{id}"),
            None,
        )
        .await
    }
}

#[derive(Debug)]
struct Response {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Response {
    /// The wire error shape name, read from `x-amzn-errortype` (which restJson1
    /// resolves the shape from) and falling back to the body's `__type`.
    fn error_code(&self) -> String {
        self.error_type
            .clone()
            .or_else(|| {
                self.body
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    fn str_at(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("no string at {pointer} in {:?}", self.body))
            .to_string()
    }
}

/// `MediaPipelineId`'s modelled pattern,
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, checked without a
/// regex crate: five hex groups of 8-4-4-4-12.
fn is_guid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Parse a modelled `Iso8601Timestamp` as the service renders it, returning epoch
/// seconds so two timestamps can be ORDERED. Parsing rather than comparing the
/// text is what makes the assertion sound: the `date-time` rendering omits the
/// fractional part when it is zero, so `"...:54Z"` and `"...:54.500Z"` do not
/// order lexicographically.
fn epoch_seconds(case: &str, value: &str) -> f64 {
    aws_smithy_types::DateTime::from_str(value, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| {
            panic!("case {case:?}: {value:?} is not an ISO-8601 date-time: {e}")
        })
        .as_secs_f64()
}

/// The create requests the lifecycle rows are driven with. They differ along what
/// `CreateMediaCapturePipeline` generalises over: which optional members are
/// present, the shape and length of the ARNs, and the size of the nested
/// configuration — the recorded "minimal" request is just the first row.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "required members only",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/abcd",
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::my-bucket/captures",
            }),
        ),
        (
            "shortest ARNs the pattern admits",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:",
                "SinkType": "S3Bucket",
                "SinkArn": "arn-",
            }),
        ),
        (
            "1024-character SinkArn, the modelled maximum",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:aws:chime:eu-west-1:000000000000:meeting/e2e",
                "SinkType": "S3Bucket",
                "SinkArn": format!("arn:aws:s3:::{}", "b".repeat(1024 - "arn:aws:s3:::".len())),
            }),
        ),
        (
            "with a sink IAM role and server-side encryption parameters",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/kms",
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::encrypted-bucket/captures",
                "SinkIamRoleArn": "arn:aws:iam::000000000000:role/capture",
                "SseAwsKeyManagementParams": {
                    "AwsKmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd",
                    "AwsKmsEncryptionContext": "{\"k\":\"v\"}",
                },
            }),
        ),
        (
            "with a nested ChimeSdkMeetingConfiguration and tags",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/nested",
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::nested-bucket/captures",
                "ChimeSdkMeetingConfiguration": {
                    "SourceConfiguration": {
                        "SelectedVideoStreams": {
                            "AttendeeIds": [
                                "11111111-1111-4111-8111-111111111111",
                                "22222222-2222-4222-8222-222222222222"
                            ],
                            "ExternalUserIds": ["external-one"],
                        }
                    },
                    "ArtifactsConfiguration": {
                        "Audio": { "MuxType": "AudioOnly" },
                        "Video": { "State": "Enabled", "MuxType": "VideoOnly" },
                        "Content": { "State": "Disabled" },
                    }
                },
                "Tags": [{ "Key": "team", "Value": "media" }],
            }),
        ),
        (
            "with a client request token",
            json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/token",
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::token-bucket/captures",
                "ClientRequestToken": "lifecycle-row-token",
            }),
        ),
    ]
}

/// RULE: a create mints and persists the pipeline; every later read of that id
/// renders the SAME stored record; a delete is observable by every later read.
#[tokio::test]
async fn a_created_pipeline_is_read_back_listed_and_then_gone_after_delete() {
    for (case, input) in create_rows() {
        let emulator = Emulator::new();
        let created = emulator.create(input.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            created.status, created.body
        );

        let pipeline = created
            .body
            .get("MediaCapturePipeline")
            .unwrap_or_else(|| panic!("case {case:?}: no MediaCapturePipeline in {:?}", created.body))
            .clone();
        let id = created.str_at("/MediaCapturePipeline/MediaPipelineId");
        assert!(
            is_guid(&id),
            "case {case:?}: expected a 36-character GUID MediaPipelineId, got {id:?}"
        );
        assert_eq!(
            created.str_at("/MediaCapturePipeline/MediaPipelineArn"),
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{id}"),
            "case {case:?}: the ARN must be derived from the signed region and the minted id"
        );
        assert_eq!(
            created.str_at("/MediaCapturePipeline/Status"),
            "Initializing",
            "case {case:?}: a fresh pipeline is Initializing"
        );
        for member in ["SourceType", "SourceArn", "SinkType", "SinkArn"] {
            assert_eq!(
                pipeline.get(member),
                input.get(member),
                "case {case:?}: {member} must come back as stored"
            );
        }
        for member in ["SinkIamRoleArn", "ChimeSdkMeetingConfiguration", "SseAwsKeyManagementParams"] {
            assert_eq!(
                pipeline.get(member),
                input.get(member),
                "case {case:?}: optional member {member} must come back as stored"
            );
        }
        let created_at = created.str_at("/MediaCapturePipeline/CreatedTimestamp");
        assert_eq!(
            created.str_at("/MediaCapturePipeline/UpdatedTimestamp"),
            created_at,
            "case {case:?}: a pipeline nothing has changed yet was updated when it was created"
        );
        // Parses as the modelled `@timestampFormat("date-time")` rendering.
        let _ = epoch_seconds(case, &created_at);

        // Read back through GetMediaCapturePipeline: the same structure.
        let fetched = emulator.get_capture(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetMediaCapturePipeline, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body.get("MediaCapturePipeline"),
            Some(&pipeline),
            "case {case:?}: GetMediaCapturePipeline must return what Create persisted"
        );

        // And through GetMediaPipeline, where ONLY the capture member is set.
        let polymorphic = emulator.get_pipeline(&id).await;
        assert_eq!(
            polymorphic.status, 200,
            "case {case:?}: expected 200 from GetMediaPipeline, got {} with {:?}",
            polymorphic.status, polymorphic.body
        );
        assert_eq!(
            polymorphic.body.pointer("/MediaPipeline/MediaCapturePipeline"),
            Some(&pipeline),
            "case {case:?}: GetMediaPipeline must carry the stored capture pipeline"
        );
        for absent in [
            "MediaLiveConnectorPipeline",
            "MediaConcatenationPipeline",
            "MediaInsightsPipeline",
            "MediaStreamPipeline",
        ] {
            assert_eq!(
                polymorphic.body.pointer(&format!("/MediaPipeline/{absent}")),
                None,
                "case {case:?}: {absent} must be absent for a capture pipeline"
            );
        }

        // Both list operations report it exactly once, with no resume token.
        for (path, items_key) in [
            ("/sdk-media-capture-pipelines", "MediaCapturePipelines"),
            ("/sdk-media-pipelines", "MediaPipelines"),
        ] {
            let listed = emulator.call("GET", path, None).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected 200 from GET {path}, got {} with {:?}",
                listed.status, listed.body
            );
            let ids: Vec<&str> = listed.body[items_key]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no {items_key} array in {:?}", listed.body))
                .iter()
                .map(|item| item["MediaPipelineId"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                ids,
                vec![id.as_str()],
                "case {case:?}: GET {path} must report the created pipeline exactly once"
            );
            assert_eq!(
                listed.body.get("NextToken"),
                None,
                "case {case:?}: GET {path} must not emit a token for a last page"
            );
        }

        // Delete, then every read of that id is a 404 and both lists are empty.
        let deleted = emulator.delete_capture(&id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected 204 from Delete, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {case:?}: a 204 carries an empty body"
        );

        for again in [
            emulator.get_capture(&id).await,
            emulator.get_pipeline(&id).await,
            emulator.delete_capture(&id).await,
        ] {
            assert_eq!(
                again.status, 404,
                "case {case:?}: after Delete every read of {id} is 404, got {} with {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code(),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {:?}",
                again.body
            );
        }

        for (path, items_key) in [
            ("/sdk-media-capture-pipelines", "MediaCapturePipelines"),
            ("/sdk-media-pipelines", "MediaPipelines"),
        ] {
            let listed = emulator.call("GET", path, None).await;
            assert_eq!(
                listed.body[items_key].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: GET {path} must not report a deleted pipeline: {:?}",
                listed.body
            );
        }
    }
}

/// RULE: an id the store does not hold is the modelled 404, whatever the
/// operation — a well-formed id that was never created reads the same as one that
/// was deleted.
#[tokio::test]
async fn an_unknown_but_well_formed_id_is_not_found() {
    let emulator = Emulator::new();
    for (case, response) in [
        ("GetMediaCapturePipeline", emulator.get_capture(UNKNOWN_ID).await),
        ("GetMediaPipeline", emulator.get_pipeline(UNKNOWN_ID).await),
        ("DeleteMediaCapturePipeline", emulator.delete_capture(UNKNOWN_ID).await),
    ] {
        assert_eq!(
            response.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            response.status, response.body
        );
        assert_eq!(
            response.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            response.body
        );
    }
}

/// RULE: a value outside a modelled constraint is refused with this service's own
/// 400 `BadRequestException`, never a 500 and never a 501.
///
/// The rows walk each kind of constraint the model declares on these operations:
/// an enum value set, the ARN pattern, the ARN length bound (and one character
/// past it), the GUID path label, and the `max-results` range at both published
/// bounds and one past each.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_a_bad_request() {
    let emulator = Emulator::new();
    let valid = json!({
        "SourceType": "ChimeSdkMeeting",
        "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/ok",
        "SinkType": "S3Bucket",
        "SinkArn": "arn:aws:s3:::ok-bucket/captures",
    });

    let mut refused: Vec<(String, Response)> = Vec::new();
    for (case, member, bad) in [
        ("SourceType outside its enum value set", "SourceType", json!("Bogus")),
        ("SinkType outside its enum value set", "SinkType", json!("KinesisVideoStream")),
        ("SourceArn without the arn prefix", "SourceArn", json!("not-an-arn")),
        ("SinkArn with a character the pattern excludes", "SinkArn", json!("arn:aws:s3:::bucket/with space")),
        ("SourceArn below the 1-character minimum", "SourceArn", json!("")),
        (
            "SinkArn one character past the 1024 maximum",
            "SinkArn",
            json!(format!("arn:aws:s3:::{}", "b".repeat(1025 - "arn:aws:s3:::".len()))),
        ),
        ("ClientRequestToken shorter than its 2-character minimum", "ClientRequestToken", json!("x")),
    ] {
        let mut input = valid.clone();
        input[member] = bad;
        refused.push((case.to_string(), emulator.create(input).await));
    }

    // The GUID path label, and the paging query parameters.
    refused.push((
        "a MediaPipelineId path label that is not a GUID".to_string(),
        emulator.get_capture("not-a-guid").await,
    ));
    refused.push((
        "a MediaPipelineId path label of the right length but not hex".to_string(),
        emulator.get_capture("zzzzzzzz-ceea-467a-9ba1-3d1e2b3c4d5e").await,
    ));
    for bad in ["0", "101", "-1"] {
        refused.push((
            format!("max-results of {bad}, outside the modelled 1..100 range"),
            emulator
                .call(
                    "GET",
                    &format!("/sdk-media-capture-pipelines?max-results={bad}"),
                    None,
                )
                .await,
        ));
        refused.push((
            format!("max-results of {bad} on ListMediaPipelines"),
            emulator
                .call("GET", &format!("/sdk-media-pipelines?max-results={bad}"), None)
                .await,
        ));
    }
    refused.push((
        "a next-token this service never issued".to_string(),
        emulator
            .call(
                "GET",
                "/sdk-media-capture-pipelines?next-token=not-a-token",
                None,
            )
            .await,
    ));

    for (case, response) in refused {
        assert_eq!(
            response.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            response.status, response.body
        );
        assert_eq!(
            response.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            response.body
        );
    }

    // And the published bounds themselves are accepted.
    for good in ["1", "100"] {
        for path in ["/sdk-media-capture-pipelines", "/sdk-media-pipelines"] {
            let response = emulator
                .call("GET", &format!("{path}?max-results={good}"), None)
                .await;
            assert_eq!(
                response.status, 200,
                "case {path:?} with max-results={good}: expected 200, got {} with {:?}",
                response.status, response.body
            );
        }
    }
}

/// RULE: paging reports every created-and-not-deleted pipeline exactly once, in
/// creation order, and emits a resume token only while more remain.
///
/// Asserted as an invariant over every page size from 1 to one past the number of
/// pipelines, for both list operations, including after a deletion in the middle
/// of the sequence.
#[tokio::test]
async fn paging_reports_every_pipeline_exactly_once_in_creation_order() {
    const PIPELINES: usize = 7;
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for n in 0..PIPELINES {
        let response = emulator
            .create(json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": format!("arn:aws:chime:us-west-2:000000000000:meeting/m{n}"),
                "SinkType": "S3Bucket",
                "SinkArn": format!("arn:aws:s3:::bucket-{n}/captures"),
            }))
            .await;
        assert_eq!(
            response.status, 201,
            "case {n}: expected 201, got {} with {:?}",
            response.status, response.body
        );
        created.push(response.str_at("/MediaCapturePipeline/MediaPipelineId"));
    }

    // Remove one from the middle: paging must skip it without stranding the rest.
    let removed = created.remove(PIPELINES / 2);
    assert_eq!(
        emulator.delete_capture(&removed).await.status,
        204,
        "the mid-sequence pipeline must delete"
    );

    for (path, items_key) in [
        ("/sdk-media-capture-pipelines", "MediaCapturePipelines"),
        ("/sdk-media-pipelines", "MediaPipelines"),
    ] {
        for page_size in 1..=created.len() + 1 {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                pages += 1;
                assert!(
                    pages <= created.len() + 2,
                    "case {path:?}/page_size {page_size}: paging did not terminate, saw {seen:?}"
                );
                let query = match &token {
                    Some(t) => format!("{path}?max-results={page_size}&next-token={t}"),
                    None => format!("{path}?max-results={page_size}"),
                };
                let response = emulator.call("GET", &query, None).await;
                assert_eq!(
                    response.status, 200,
                    "case {path:?}/page_size {page_size}: expected 200, got {} with {:?}",
                    response.status, response.body
                );
                let items = response.body[items_key]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {path:?}/page_size {page_size}: no {items_key} in {:?}",
                            response.body
                        )
                    })
                    .clone();
                assert!(
                    items.len() <= page_size,
                    "case {path:?}/page_size {page_size}: a page returned {} items",
                    items.len()
                );
                for item in &items {
                    let id = item["MediaPipelineId"].as_str().unwrap_or_default();
                    assert_eq!(
                        item["MediaPipelineArn"].as_str().unwrap_or_default(),
                        format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{id}"),
                        "case {path:?}/page_size {page_size}: a summary's ARN must match its id"
                    );
                    seen.push(id.to_string());
                }
                token = response
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                match &token {
                    // A token is emitted only while something remains.
                    Some(_) => assert!(
                        seen.len() < created.len(),
                        "case {path:?}/page_size {page_size}: a token was emitted on the last page"
                    ),
                    None => break,
                }
            }
            assert_eq!(
                seen, created,
                "case {path:?}/page_size {page_size}: paging must report every pipeline exactly once, in creation order"
            );
        }
    }
}

/// RULE: a create replayed with the same `ClientRequestToken` answers with the
/// pipeline the first one minted instead of minting a second.
#[tokio::test]
async fn a_repeated_client_request_token_does_not_mint_a_second_pipeline() {
    let emulator = Emulator::new();
    // Rows differ along the token (length, alphabet) and along whether the replay
    // repeats the same body or sends a different one under the same token.
    for (case, token, replay_source) in [
        ("shortest token the model admits", "ab", None),
        ("64-character token, the modelled maximum", &*"t".repeat(64), None),
        ("token with the punctuation the pattern admits", "a-b_C9", None),
        (
            "same token, different body",
            "replay-with-other-body",
            Some("arn:aws:chime:us-west-2:000000000000:meeting/other"),
        ),
    ] {
        let source = "arn:aws:chime:us-west-2:000000000000:meeting/first";
        let first = emulator
            .create(json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": source,
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::idempotent/captures",
                "ClientRequestToken": token,
            }))
            .await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            first.status, first.body
        );
        let id = first.str_at("/MediaCapturePipeline/MediaPipelineId");

        let replay = emulator
            .create(json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": replay_source.unwrap_or(source),
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::idempotent/captures",
                "ClientRequestToken": token,
            }))
            .await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: expected 201 on replay, got {} with {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body.get("MediaCapturePipeline"),
            first.body.get("MediaCapturePipeline"),
            "case {case:?}: a replayed token must answer with the pipeline the first create minted"
        );

        let listed = emulator
            .call("GET", "/sdk-media-capture-pipelines", None)
            .await;
        let matching = listed.body["MediaCapturePipelines"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no list in {:?}", listed.body))
            .iter()
            .filter(|item| item["MediaPipelineId"].as_str() == Some(id.as_str()))
            .count();
        assert_eq!(
            matching, 1,
            "case {case:?}: the replay must not have minted a second pipeline: {:?}",
            listed.body
        );

        // Leave the store empty for the next row.
        assert_eq!(emulator.delete_capture(&id).await.status, 204);
    }
}

/// RULE: a resource the service returns in a NON-TERMINAL status carries the
/// transition the clock owes — a caller reading straight back still sees
/// `Initializing`, and once the span has elapsed every read sees `InProgress`
/// with `UpdatedTimestamp` moved past `CreatedTimestamp`.
#[tokio::test]
async fn the_initializing_status_moves_to_in_progress_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = emulator
        .create(json!({
            "SourceType": "ChimeSdkMeeting",
            "SourceArn": "arn:aws:chime:us-west-2:000000000000:meeting/clock",
            "SinkType": "S3Bucket",
            "SinkArn": "arn:aws:s3:::clock-bucket/captures",
        }))
        .await;
    assert_eq!(created.status, 201, "expected 201, got {:?}", created.body);
    let id = created.str_at("/MediaCapturePipeline/MediaPipelineId");
    let created_at = created.str_at("/MediaCapturePipeline/CreatedTimestamp");

    // Straight back: the transient status is still observable.
    let immediate = emulator.get_capture(&id).await;
    assert_eq!(
        immediate.str_at("/MediaCapturePipeline/Status"),
        "Initializing",
        "a read taken straight after the create must still see the transient status"
    );

    // Past the span the transition is due in — derived from the span itself, not a
    // magic number, so the test follows the constant if it ever changes.
    tokio::time::sleep(INITIALIZING_TO_IN_PROGRESS + Duration::from_millis(200)).await;
    for (case, response) in [
        ("GetMediaCapturePipeline", emulator.get_capture(&id).await),
        ("GetMediaPipeline", emulator.get_pipeline(&id).await),
    ] {
        let pointer = if case == "GetMediaPipeline" {
            "/MediaPipeline/MediaCapturePipeline"
        } else {
            "/MediaCapturePipeline"
        };
        let pipeline = response
            .body
            .pointer(pointer)
            .unwrap_or_else(|| panic!("case {case:?}: nothing at {pointer} in {:?}", response.body));
        assert_eq!(
            pipeline["Status"].as_str(),
            Some("InProgress"),
            "case {case:?}: the owed transition must have been materialised: {pipeline:?}"
        );
        let updated = pipeline["UpdatedTimestamp"].as_str().unwrap_or_default();
        assert!(
            epoch_seconds(case, updated) > epoch_seconds(case, &created_at),
            "case {case:?}: expected UpdatedTimestamp after CreatedTimestamp {created_at:?}, got {updated:?}"
        );
        assert_eq!(
            pipeline["CreatedTimestamp"].as_str(),
            Some(created_at.as_str()),
            "case {case:?}: CreatedTimestamp must not move"
        );
    }
}

/// RULE: the store never grows past its stated maximum — a create that would push
/// it past `MAX_CAPTURE_PIPELINES` is refused with the quota error the model
/// declares for the operation, and every read keeps working while it is full.
///
/// The rows are the boundary itself: the last create that fits, the first that does
/// not, one more after that (so the refusal is not a one-off), and — after freeing
/// exactly one slot — a create that fits again.
#[tokio::test]
async fn the_store_refuses_to_grow_past_its_stated_maximum() {
    let emulator = Emulator::new();

    let mut ids: Vec<String> = Vec::with_capacity(MAX_CAPTURE_PIPELINES);
    for n in 0..MAX_CAPTURE_PIPELINES {
        let response = emulator.create(filler_create(n)).await;
        assert_eq!(
            response.status, 201,
            "case {n} of {MAX_CAPTURE_PIPELINES}: a create inside the bound must succeed, got {} with {:?}",
            response.status, response.body
        );
        ids.push(response.str_at("/MediaCapturePipeline/MediaPipelineId"));
    }

    for case in ["first create past the bound", "second create past the bound"] {
        let refused = emulator.create(filler_create(MAX_CAPTURE_PIPELINES)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceLimitExceededException",
            "case {case:?}: expected the modelled quota error, got {:?}",
            refused.body
        );
    }

    // A full store still serves reads, and still pages.
    let full = emulator
        .call("GET", "/sdk-media-capture-pipelines?max-results=1", None)
        .await;
    assert_eq!(
        full.status, 200,
        "a full store must still serve ListMediaCapturePipelines, got {} with {:?}",
        full.status, full.body
    );
    assert!(
        full.body.get("NextToken").is_some(),
        "a full store paged at 1 must hand back a resume token: {:?}",
        full.body
    );
    assert_eq!(
        emulator.get_capture(&ids[0]).await.status,
        200,
        "a full store must still serve GetMediaCapturePipeline"
    );

    // Free exactly one slot: the next create fits again, and only one does.
    assert_eq!(
        emulator.delete_capture(&ids[0]).await.status,
        204,
        "deleting from a full store must succeed"
    );
    let after_delete = emulator.create(filler_create(MAX_CAPTURE_PIPELINES)).await;
    assert_eq!(
        after_delete.status, 201,
        "case \"create into the freed slot\": expected 201, got {} with {:?}",
        after_delete.status, after_delete.body
    );
    let refused_again = emulator.create(filler_create(MAX_CAPTURE_PIPELINES + 1)).await;
    assert_eq!(
        refused_again.status, 400,
        "case \"create past the bound again\": expected 400, got {} with {:?}",
        refused_again.status, refused_again.body
    );
}

/// A minimal, distinct create request, for filling the store to its bound.
fn filler_create(n: usize) -> Value {
    json!({
        "SourceType": "ChimeSdkMeeting",
        "SourceArn": format!("arn:aws:chime:us-west-2:000000000000:meeting/fill-{n}"),
        "SinkType": "S3Bucket",
        "SinkArn": format!("arn:aws:s3:::fill-{n}/captures"),
    })
}

/// RULE: a shape's schema carries exactly the members the model declares for it, in
/// one order, each at its own index, with the wire traits the model gives them.
///
/// `MediaCapturePipeline`'s schema is hand-written (see the comment on it in
/// `src/ops/types/shared_types.rs`) because the derive drops `@timestampFormat`.
/// Hand-written means it can drift from the model, so this test pins it: the member
/// list below is the model's, from
/// `smithy_query(query="shape", shape="MediaCapturePipeline")`, and the two
/// timestamp members must report the `date-time` format that is the whole reason
/// the block exists. A dropped, renamed, reordered or re-indexed member fails here
/// rather than silently changing the wire.
#[test]
fn the_hand_written_media_capture_pipeline_schema_matches_the_model() {
    // From `smithy_query(query="shape", shape="MediaCapturePipeline")`: every member
    // the model declares, in declaration order.
    const MODEL_MEMBERS: [&str; 12] = [
        "MediaPipelineId",
        "MediaPipelineArn",
        "SourceType",
        "SourceArn",
        "Status",
        "SinkType",
        "SinkArn",
        "CreatedTimestamp",
        "UpdatedTimestamp",
        "ChimeSdkMeetingConfiguration",
        "SseAwsKeyManagementParams",
        "SinkIamRoleArn",
    ];
    // The members the model targets at `Iso8601Timestamp`, which carries
    // `@timestampFormat("date-time")`.
    const DATE_TIME_MEMBERS: [&str; 2] = ["CreatedTimestamp", "UpdatedTimestamp"];

    let members = MediaCapturePipeline::SCHEMA.members();
    let names: Vec<&str> = members
        .iter()
        .map(|member| member.member_name().unwrap_or("<unnamed>"))
        .collect();
    assert_eq!(
        names,
        MODEL_MEMBERS.to_vec(),
        "case \"member list\": the schema must carry exactly the model's members, in order"
    );

    for (position, member) in members.iter().enumerate() {
        let name = member.member_name().unwrap_or("<unnamed>");
        assert_eq!(
            member.member_index(),
            Some(position),
            "case {name:?}: member_index must equal the member's position, got {:?}",
            member.member_index()
        );
        let format = member.timestamp_format().map(|t| t.format());
        if DATE_TIME_MEMBERS.contains(&name) {
            assert_eq!(
                format,
                Some(aws_smithy_schema::traits::TimestampFormat::DateTime),
                "case {name:?}: the model gives this member @timestampFormat(\"date-time\"), got {format:?}"
            );
        } else {
            assert_eq!(
                format, None,
                "case {name:?}: the model gives this member no @timestampFormat, got {format:?}"
            );
        }
    }
}
