// SPDX-License-Identifier: CC0-1.0

//! The media insights pipeline CONFIGURATION lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the wire: the suite builds the real
//! `MultiServiceRouter`, mounts this service on one shared engine and dispatches
//! `http::Request`s, so what it pins is the emulator's HTTP behaviour and not an
//! internal helper.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a configuration is identified by its NAME or its ARN, and an identifier that
//!   resolves to no stored configuration is this service's modelled 404
//!   `NotFoundException` — HTTP 404, `Content-Type: application/json` (restJson1
//!   error framing), an `x-amzn-errortype` header, and a body carrying `Code`
//!   `NotFound`, a `Message` and a `RequestId`;
//! * a create MINTS and PERSISTS a configuration, and every later read of either
//!   identifier renders from the stored record;
//! * a delete is observable by every later read, and deleting twice is the same 404
//!   as deleting something that never existed;
//! * a read writes nothing, so two reads of one configuration answer alike and a
//!   failed delete leaves its neighbours alone;
//! * a repeated `ClientRequestToken` answers with the configuration the first create
//!   minted, and a name already in use is the modelled 400 `BadRequestException`;
//! * `Elements`, the one caller-chosen size the model leaves unbounded, is bounded per
//!   configuration by the service: at or under the maximum a create is served and stored
//!   in full, past it the modelled 400 `BadRequestException` with nothing stored;
//! * the hand-written `MediaInsightsPipelineConfiguration` schema carries exactly the
//!   model's members, in order, with the `date-time` format on its two timestamps.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::ops::types::{
    MediaInsightsPipelineConfiguration, MediaInsightsPipelineConfigurationElement,
};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    service_state, InsightsConfiguration, InsightsConfigurationRefused,
    MAX_INSIGHTS_CONFIGURATION_ELEMENTS,
};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for. The ARN a create derives must carry it, so it is
/// deliberately NOT the emulator's `us-east-1` default.
const REGION: &str = "us-west-2";

/// The canonical placeholder account every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `ErrorCode` value the modelled `NotFoundException` carries in its body `Code`
/// member. Source: `smithy_query(query="shape",shape="ErrorCode")` on this service's
/// model — `BadRequest | Conflict | Forbidden | NotFound | …`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The wire error-shape name restJson1 resolves from `x-amzn-errortype`.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The `ErrorCode` value the modelled `BadRequestException` carries in its body `Code`
/// member, from the same `ErrorCode` enum, and the wire error-shape name beside it.
const ERROR_CODE_BAD_REQUEST: &str = "BadRequest";
pub(crate) const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// One request/response round trip through the router. `pub(crate)` because the sibling
/// suite `media_insights_pipeline_configuration_listing` drives the same wire surface and
/// must not build a second copy of it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch one request and return what the rows assert on (`Value::Null` for an
    /// empty body, which is what a 204 carries).
    pub(crate) async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> Response {
        let payload = body.map_or_else(Vec::new, |b| {
            serde_json::to_vec(&b).expect("a JSON value serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://media-pipelines-chime.{REGION}.amazonaws.com{path_and_query}"
            ))
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
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let error_type = header("x-amzn-errortype");
        let content_type = header("content-type");
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
            content_type,
            body,
        }
    }

    pub(crate) async fn create(&self, input: Value) -> Response {
        self.call(
            "POST",
            "/media-insights-pipeline-configurations",
            Some(input),
        )
        .await
    }

    pub(crate) async fn get(&self, identifier: &str) -> Response {
        self.call(
            "GET",
            &format!(
                "/media-insights-pipeline-configurations/{}",
                encode_label(identifier)
            ),
            None,
        )
        .await
    }

    pub(crate) async fn delete(&self, identifier: &str) -> Response {
        self.call(
            "DELETE",
            &format!(
                "/media-insights-pipeline-configurations/{}",
                encode_label(identifier)
            ),
            None,
        )
        .await
    }
}

/// A non-greedy `@httpLabel` as a client sends it: the characters an ARN carries that
/// are path separators or delimiters are percent-encoded.
fn encode_label(label: &str) -> String {
    label.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    pub(crate) body: Value,
}

impl Response {
    /// The wire error shape name, read from `x-amzn-errortype` (which restJson1
    /// resolves the shape from) and falling back to the body's `__type`.
    pub(crate) fn error_code(&self) -> String {
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
            .unwrap_or_default()
            .to_string()
    }
}

/// Whether `id` is a 36-character GUID, the model's `GuidString`
/// (`^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`).
fn is_guid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
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
/// seconds. Parsing is what makes the assertion sound: it fails both on the epoch
/// NUMBER the protocol default would emit and on any other text.
fn iso8601_seconds(case: &str, value: &str) -> f64 {
    aws_smithy_types::DateTime::from_str(value, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {value:?} is not an ISO-8601 date-time: {e}"))
        .as_secs_f64()
}

/// A `ResourceAccessRoleArn` satisfying the modelled `Arn` shape (len 1..1024,
/// pattern `^arn[\/\:\-\_\.a-zA-Z0-9]+$`).
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-access";

/// A 64-character configuration name: the longest the modelled
/// `MediaInsightsPipelineConfigurationNameString` allows (len 2..64, pattern
/// `^[0-9a-zA-Z._-]+$`).
fn longest_name() -> String {
    "c".repeat(64)
}

/// The create inputs the lifecycle rules are asserted over. The rows differ along
/// what the rules generalise over: the NAME's form and length across the modelled
/// pattern and both of its length bounds, the number and kind of `Elements`, and
/// whether the optional `RealTimeAlertConfiguration` and `Tags` are present.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the shortest name the model allows, one processor and one sink",
            json!({
                "MediaInsightsPipelineConfigurationName": "c1",
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "AmazonTranscribeProcessor",
                        "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
                    },
                    {
                        "Type": "KinesisDataStreamSink",
                        "KinesisDataStreamSinkConfiguration": {
                            "InsightsTarget": "arn:aws:kinesis:us-west-2:000000000000:stream/insights",
                        },
                    },
                ],
            }),
        ),
        (
            "a name carrying every class its pattern allows, with tags",
            json!({
                "MediaInsightsPipelineConfigurationName": "Conf.1_test-2",
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "S3RecordingSink",
                        "S3RecordingSinkConfiguration": {
                            "Destination": "arn:aws:s3:::insights-bucket/recordings",
                            "RecordingFileFormat": "Wav",
                        },
                    },
                ],
                "Tags": [{ "Key": "team", "Value": "voice" }],
            }),
        ),
        (
            "the longest name the model allows, with a real-time alert configuration",
            json!({
                "MediaInsightsPipelineConfigurationName": longest_name(),
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "AmazonTranscribeCallAnalyticsProcessor",
                        "AmazonTranscribeCallAnalyticsProcessorConfiguration": {
                            "LanguageCode": "en-US",
                        },
                    },
                    {
                        "Type": "LambdaFunctionSink",
                        "LambdaFunctionSinkConfiguration": {
                            "InsightsTarget": "arn:aws:lambda:us-west-2:000000000000:function:insights",
                        },
                    },
                    {
                        "Type": "SqsQueueSink",
                        "SqsQueueSinkConfiguration": {
                            "InsightsTarget": "arn:aws:sqs:us-west-2:000000000000:insights",
                        },
                    },
                ],
                "RealTimeAlertConfiguration": {
                    "Disabled": false,
                    "Rules": [
                        {
                            "Type": "KeywordMatch",
                            "KeywordMatchConfiguration": {
                                "RuleName": "escalate",
                                "Keywords": ["manager", "supervisor"],
                                "Negate": false,
                            },
                        },
                    ],
                },
            }),
        ),
        (
            "a numeric name, one sink only",
            json!({
                "MediaInsightsPipelineConfigurationName": "20261023",
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "SnsTopicSink",
                        "SnsTopicSinkConfiguration": {
                            "InsightsTarget": "arn:aws:sns:us-west-2:000000000000:insights",
                        },
                    },
                ],
            }),
        ),
    ]
}

/// RULE: a create persists a configuration, both forms of its `Identifier` (the name
/// and the ARN) read the SAME stored record back, and after a delete every read of
/// either form is the modelled 404.
#[tokio::test]
async fn a_created_configuration_reads_back_by_name_and_arn_and_is_gone_after_delete() {
    for (case, input) in create_rows() {
        let emulator = Emulator::new();
        let created = emulator.create(input.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            created.status, created.body
        );

        let configuration = created
            .body
            .get("MediaInsightsPipelineConfiguration")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no MediaInsightsPipelineConfiguration in {:?}",
                    created.body
                )
            })
            .clone();
        let name = input["MediaInsightsPipelineConfigurationName"]
            .as_str()
            .expect("every row names its configuration")
            .to_string();
        let id = created.str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationId");
        assert!(
            is_guid(&id),
            "case {case:?}: expected a 36-character GUID configuration id, got {id:?}"
        );
        let arn = created.str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn");
        assert_eq!(
            arn,
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/{name}"),
            "case {case:?}: the ARN must be derived from the signed region and the name"
        );
        for member in [
            "MediaInsightsPipelineConfigurationName",
            "ResourceAccessRoleArn",
        ] {
            assert_eq!(
                configuration.get(member),
                input.get(member),
                "case {case:?}: {member} must come back as stored"
            );
        }
        // The caller's processors, sinks and alert rules come back as submitted. The
        // comparison is "everything submitted is present with the same value" rather
        // than equality, because the generated shapes materialise a modelled boolean
        // the caller omitted as its `false` default, which the response then carries.
        for member in ["Elements", "RealTimeAlertConfiguration"] {
            assert_submitted_members_are_reported(
                &format!("{case} / {member}"),
                input.get(member),
                configuration.get(member),
            );
        }
        let created_at = created.str_at("/MediaInsightsPipelineConfiguration/CreatedTimestamp");
        assert!(
            !created_at.is_empty(),
            "case {case:?}: a created configuration carries a CreatedTimestamp: {configuration:?}"
        );
        // The modelled `Iso8601Timestamp` carries `@timestampFormat("date-time")`, so
        // the wire form is an ISO-8601 STRING and not epoch seconds.
        let _ = iso8601_seconds(case, &created_at);
        assert_eq!(
            created.str_at("/MediaInsightsPipelineConfiguration/UpdatedTimestamp"),
            created_at,
            "case {case:?}: a configuration nothing has changed yet was updated when it was created"
        );

        // Both identifier forms read the SAME record back, and a read changes nothing:
        // the second read of each answers identically.
        for identifier in [name.as_str(), arn.as_str()] {
            for attempt in 1..=2 {
                let fetched = emulator.get(identifier).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: read {attempt} of {identifier:?} expected 200, got {} with {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.body.get("MediaInsightsPipelineConfiguration"),
                    Some(&configuration),
                    "case {case:?}: read {attempt} of {identifier:?} must return what Create persisted"
                );
            }
        }

        // Delete by the ARN; afterwards every read of either form is the modelled 404,
        // and so is deleting again.
        let deleted = emulator.delete(&arn).await;
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

        for (again_case, again) in [
            ("GetMediaInsightsPipelineConfiguration by name", emulator.get(&name).await),
            ("GetMediaInsightsPipelineConfiguration by ARN", emulator.get(&arn).await),
            ("DeleteMediaInsightsPipelineConfiguration by name", emulator.delete(&name).await),
            ("DeleteMediaInsightsPipelineConfiguration by ARN", emulator.delete(&arn).await),
        ] {
            assert_not_found(&format!("{case} / {again_case}"), &again);
        }
    }
}

/// Assert every member `submitted` carries is reported in `reported` with the same
/// value — recursively, so a nested configuration is checked member by member while a
/// member the response adds (a modelled boolean's `false` default) is tolerated. A
/// scalar must be equal, and a list must have the same length, element for element.
pub(crate) fn assert_submitted_members_are_reported(
    case: &str,
    submitted: Option<&Value>,
    reported: Option<&Value>,
) {
    let (Some(submitted), Some(reported)) = (submitted, reported) else {
        assert_eq!(
            submitted.is_some(),
            reported.is_some(),
            "case {case:?}: expected {submitted:?}, got {reported:?}"
        );
        return;
    };
    match submitted {
        Value::Object(members) => {
            for (name, value) in members {
                assert_submitted_members_are_reported(
                    &format!("{case}/{name}"),
                    Some(value),
                    reported.get(name),
                );
            }
        }
        Value::Array(items) => {
            let reported_items = reported.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected a list, got {reported:?}")
            });
            assert_eq!(
                items.len(),
                reported_items.len(),
                "case {case:?}: expected {} items, got {reported:?}",
                items.len()
            );
            for (index, (item, reported_item)) in items.iter().zip(reported_items).enumerate() {
                assert_submitted_members_are_reported(
                    &format!("{case}[{index}]"),
                    Some(item),
                    Some(reported_item),
                );
            }
        }
        scalar => assert_eq!(
            Some(scalar),
            Some(reported),
            "case {case:?}: expected {scalar:?}, got {reported:?}"
        ),
    }
}

/// Assert one answer is this service's modelled not-found refusal, in restJson1's own
/// error framing: that is the whole rule, so every row of every table checks it here.
fn assert_not_found(case: &str, response: &Response) {
    assert_eq!(
        response.status, 404,
        "case {case:?}: expected 404, got {} with {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled {NOT_FOUND_EXCEPTION}, got {:?}",
        response.body
    );
    assert_eq!(
        response.content_type.as_deref(),
        Some("application/json"),
        "case {case:?}: a restJson1 error is framed as application/json, got {:?}",
        response.content_type
    );
    assert_eq!(
        response.body.get("Code").and_then(Value::as_str),
        Some(ERROR_CODE_NOT_FOUND),
        "case {case:?}: expected body Code {ERROR_CODE_NOT_FOUND:?}, got {:?}",
        response.body
    );
    for member in ["Message", "RequestId"] {
        let value = response.body.get(member).and_then(Value::as_str);
        assert!(
            value.is_some_and(|text| !text.is_empty()),
            "case {case:?}: the modelled error carries a non-empty {member}, got {:?}",
            response.body
        );
    }
}

/// Identifiers no create in this suite mints, spanning the forms a caller sends: a
/// name, this service's own ARN form, another service's ARN, a GUID, a name at each
/// end of the name shape's length bounds, and an identifier at the `Identifier`
/// label's own 1024-character maximum.
fn unknown_identifiers() -> Vec<(&'static str, String)> {
    vec![
        ("a well-formed name nothing was created under", "no-such-configuration".to_string()),
        (
            "this service's own ARN form for a name nothing holds",
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/no-such-configuration"
            ),
        ),
        (
            "an ARN of another resource type in the same namespace",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        ("a GUID, which is not an identifier form", "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e".to_string()),
        ("the shortest name the name shape allows", "ab".to_string()),
        ("the longest name the name shape allows", longest_name()),
        (
            // The label's own modelled maximum: `NonEmptyString` is len 0..1024.
            "an identifier at the Identifier label's 1024-character maximum",
            "z".repeat(1024),
        ),
    ]
}

/// RULE: an `Identifier` that resolves to no stored configuration is the modelled 404
/// `NotFoundException` — for the read and for the delete alike, whatever form the
/// identifier takes. This is the issue's own call: a delete of an unknown identifier
/// is a terminal client error, never a server fault.
#[tokio::test]
async fn an_unknown_identifier_is_the_modelled_not_found_for_every_identifier_form() {
    let emulator = Emulator::new();

    // One live configuration, so the store is not merely empty: a miss must be decided
    // by the identifier and not by there being nothing at all.
    let live = emulator
        .create(json!({
            "MediaInsightsPipelineConfigurationName": "live-configuration",
            "ResourceAccessRoleArn": ROLE_ARN,
            "Elements": [
                {
                    "Type": "AmazonTranscribeProcessor",
                    "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
                },
            ],
        }))
        .await;
    assert_eq!(
        live.status, 201,
        "the live configuration must be created, got {} with {:?}",
        live.status, live.body
    );
    let live_body = live
        .body
        .get("MediaInsightsPipelineConfiguration")
        .cloned()
        .expect("the create answers with the configuration");

    for (case, identifier) in unknown_identifiers() {
        for (operation, response) in [
            ("GetMediaInsightsPipelineConfiguration", emulator.get(&identifier).await),
            ("DeleteMediaInsightsPipelineConfiguration", emulator.delete(&identifier).await),
        ] {
            assert_not_found(&format!("{operation} on {case}"), &response);
        }
    }

    // None of those refusals touched the store: the live configuration still reads.
    let still_there = emulator.get("live-configuration").await;
    assert_eq!(
        still_there.status, 200,
        "a refused read or delete must leave its neighbours alone, got {} with {:?}",
        still_there.status, still_there.body
    );
    assert_eq!(
        still_there.body.get("MediaInsightsPipelineConfiguration"),
        Some(&live_body),
        "a refused read or delete must not change the configurations it did not name"
    );
}

/// RULE: the NAME identifies a configuration in the account — so a second create under
/// a live name is refused with a modelled 400 (the model declares no `ConflictException`
/// for this operation), while a repeated `ClientRequestToken` answers with the
/// configuration the first create minted rather than a second one.
#[tokio::test]
async fn a_name_identifies_one_configuration_and_a_repeated_token_mints_nothing() {
    for (case, name, token) in [
        ("a short name", "dup-1", "token-dup-1"),
        ("a name with every allowed class", "Dup.2_x-3", "token_dup_2"),
    ] {
        let emulator = Emulator::new();
        let input = json!({
            "MediaInsightsPipelineConfigurationName": name,
            "ResourceAccessRoleArn": ROLE_ARN,
            "Elements": [
                {
                    "Type": "AmazonTranscribeProcessor",
                    "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
                },
            ],
            "ClientRequestToken": token,
        });

        let first = emulator.create(input.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            first.status, first.body
        );

        // The same token: the first configuration, not a second one.
        let replay = emulator.create(input.clone()).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: expected 201 on replay, got {} with {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body.get("MediaInsightsPipelineConfiguration"),
            first.body.get("MediaInsightsPipelineConfiguration"),
            "case {case:?}: a replayed token must answer with the configuration the first create minted"
        );

        // A DIFFERENT token under the same name is a new create of a name in use.
        let mut other = input.clone();
        other["ClientRequestToken"] = json!(format!("{token}-other"));
        let refused = emulator.create(other).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 for a name already in use, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.body
        );

        // And once the name is free, it can be taken again — with a new identity.
        let first_id =
            first.str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationId");
        assert_eq!(
            emulator.delete(name).await.status,
            204,
            "case {case:?}: the configuration must delete"
        );
        let recreated = emulator.create(input).await;
        assert_eq!(
            recreated.status, 201,
            "case {case:?}: a freed name must be usable again, got {} with {:?}",
            recreated.status, recreated.body
        );
        let second_id = recreated
            .str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationId");
        assert_ne!(
            first_id, second_id,
            "case {case:?}: a re-created configuration is a new resource, so its id must differ"
        );
    }
}

/// One sink element, the smallest the model allows: a `Type` plus the one ARN its
/// configuration carries. `index` only keeps the ARNs distinct.
fn sink_element(index: usize) -> Value {
    json!({
        "Type": "SqsQueueSink",
        "SqsQueueSinkConfiguration": {
            "InsightsTarget": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:insights-{index}"),
        },
    })
}

/// RULE: a request member whose size the caller chooses and the model does not bound is
/// bounded by the service before it is stored — so a create whose `Elements` list is at or
/// under the per-configuration maximum is served and persisted in full, and one past it is
/// refused with this service's modelled 400 `BadRequestException`, with nothing stored.
///
/// `Elements` is that member here: `smithy_query(query="shape",
/// shape="MediaInsightsPipelineConfigurationElements")` reports a list with NO `@length`,
/// while every member inside an element is `@length`-bounded — so the count is what the
/// service caps, and `MAX_INSIGHTS_CONFIGURATION_ELEMENTS` is that cap. The rows span the
/// class: one element, the nine types the model declares, exactly the cap, one past it,
/// and far past it.
#[tokio::test]
async fn the_element_list_is_bounded_per_configuration_and_a_refusal_stores_nothing() {
    let cap = MAX_INSIGHTS_CONFIGURATION_ELEMENTS;
    let rows: [(&str, usize, u16); 5] = [
        ("one element", 1, 201),
        ("as many elements as the model declares types", 9, 201),
        ("exactly the per-configuration maximum", cap, 201),
        ("one element past the maximum", cap + 1, 400),
        ("far past the maximum", cap * 16, 400),
    ];

    for (case, count, expected_status) in rows {
        let emulator = Emulator::new();
        let name = format!("elements-{count}");
        let elements: Vec<Value> = (0..count).map(sink_element).collect();
        let response = emulator
            .create(json!({
                "MediaInsightsPipelineConfigurationName": name,
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": elements,
            }))
            .await;
        assert_eq!(
            response.status, expected_status,
            "case {case:?} ({count} elements): expected {expected_status}, got {} with {:?}",
            response.status, response.body
        );

        if expected_status == 201 {
            // Served means PERSISTED: the read reports every element back.
            let fetched = emulator.get(&name).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: the created configuration must read back, got {} with {:?}",
                fetched.status, fetched.body
            );
            let stored = fetched
                .body
                .pointer("/MediaInsightsPipelineConfiguration/Elements")
                .and_then(Value::as_array)
                .map(Vec::len);
            assert_eq!(
                stored,
                Some(count),
                "case {case:?}: the stored configuration must report all {count} elements, got {stored:?}"
            );
        } else {
            assert_eq!(
                response.error_code(),
                BAD_REQUEST_EXCEPTION,
                "case {case:?}: expected the modelled {BAD_REQUEST_EXCEPTION}, got {:?}",
                response.body
            );
            assert_eq!(
                response.content_type.as_deref(),
                Some("application/json"),
                "case {case:?}: a restJson1 error is framed as application/json, got {:?}",
                response.content_type
            );
            assert_eq!(
                response.body.get("Code").and_then(Value::as_str),
                Some(ERROR_CODE_BAD_REQUEST),
                "case {case:?}: expected body Code {ERROR_CODE_BAD_REQUEST:?}, got {:?}",
                response.body
            );
            // Refused before anything was built or locked: nothing is stored under the
            // name, so the read is the modelled 404.
            assert_not_found(
                &format!("{case} / the refused name"),
                &emulator.get(&name).await,
            );
        }
    }
}

/// A stored record carrying `elements` elements, for the store-level row below. Only the
/// element count varies; everything else is a well-formed value of its modelled shape.
fn stored_configuration(name: &str, elements: usize) -> InsightsConfiguration {
    InsightsConfiguration {
        seq: 0,
        name: name.to_string(),
        arn: format!(
            "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/{name}"
        ),
        id: "00000000-0000-4000-8000-000000000000".to_string(),
        resource_access_role_arn: ROLE_ARN.to_string(),
        real_time_alert_configuration: None,
        elements: (0..elements)
            .map(|_| MediaInsightsPipelineConfigurationElement {
                r#type: "SqsQueueSink".to_string(),
                ..Default::default()
            })
            .collect(),
        created_timestamp: aws_smithy_types::DateTime::from_secs(0),
        updated_timestamp: aws_smithy_types::DateTime::from_secs(0),
    }
}

/// RULE, at the STORE: the per-record bound is the STORE's admission rule, not only the
/// handler's, so an insert past the element maximum is refused there as well — the bound
/// does not depend on one call path being the only way into the collection, and the
/// constant cannot fall out of use while this is the gate.
#[test]
fn the_store_refuses_a_configuration_past_the_element_maximum() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = service_state(&*engine).expect("the shared engine holds this service's store");
    let cap = MAX_INSIGHTS_CONFIGURATION_ELEMENTS;

    // (case, element count, whether the store must refuse it)
    for (case, count, refused) in [
        ("no elements at all", 0, false),
        ("exactly the maximum", cap, false),
        ("one past the maximum", cap + 1, true),
        ("far past the maximum", cap * 16, true),
    ] {
        let mut store = state.lock();
        let result = store
            .insert_insights_configuration(
                stored_configuration(&format!("store-{case}"), count),
                None,
            )
            .map(|_| ());
        assert_eq!(
            result.is_err(),
            refused,
            "case {case:?} ({count} elements): expected refused={refused}, got {result:?}"
        );
        if refused {
            assert_eq!(
                result,
                Err(InsightsConfigurationRefused::TooManyElements(count)),
                "case {case:?}: expected the element-maximum refusal, got {result:?}"
            );
        }
    }
}

/// RULE: a shape's schema carries exactly the members the model declares for it, in one
/// order, each at its own index, with the wire traits the model gives them.
///
/// `MediaInsightsPipelineConfiguration`'s schema is hand-written (see the comment on it
/// in `src/ops/types/shared_types.rs`) because the derive drops `@timestampFormat`.
/// Hand-written means it can drift from the model, so this test pins it: the member list
/// below is the model's, from
/// `smithy_query(query="shape", shape="MediaInsightsPipelineConfiguration")`, and the two
/// timestamp members must report the `date-time` format that is the whole reason the
/// block exists.
#[test]
fn the_hand_written_media_insights_pipeline_configuration_schema_matches_the_model() {
    // From `smithy_query(query="shape", shape="MediaInsightsPipelineConfiguration")`:
    // every member the model declares, in declaration order.
    const MODEL_MEMBERS: [&str; 8] = [
        "MediaInsightsPipelineConfigurationName",
        "MediaInsightsPipelineConfigurationArn",
        "ResourceAccessRoleArn",
        "RealTimeAlertConfiguration",
        "Elements",
        "MediaInsightsPipelineConfigurationId",
        "CreatedTimestamp",
        "UpdatedTimestamp",
    ];
    // The members the model targets at `Iso8601Timestamp`, which carries
    // `@timestampFormat("date-time")`.
    const DATE_TIME_MEMBERS: [&str; 2] = ["CreatedTimestamp", "UpdatedTimestamp"];

    let members = MediaInsightsPipelineConfiguration::SCHEMA.members();
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
