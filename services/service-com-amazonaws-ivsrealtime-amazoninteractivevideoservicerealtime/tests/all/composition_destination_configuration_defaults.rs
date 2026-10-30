// SPDX-License-Identifier: CC0-1.0

//! The defaults a composition DESTINATION's stored configuration reports, over the wire.
//!
//! THE RULE: every member of a configuration this service stores whose own model
//! documentation states a default value is reported with that documented default when the
//! request left the member out of an object it supplied — a composition destination's
//! recording and thumbnail configuration exactly as a stage's recording configuration,
//! while a value the request DID send is reported as it arrived. An optional object the
//! request supplied no value for is not conjured: its absence is what the model gives
//! meaning to.
//!
//! The members under test, each from
//! `smithy_query(query="shape", shape="S3DestinationConfiguration,RecordingConfiguration,
//! CompositionRecordingHlsConfiguration,CompositionThumbnailConfiguration")`:
//!
//! * `CompositionRecordingHlsConfiguration$targetSegmentDurationSeconds` — "Defines the
//!   target duration for recorded segments generated when using composite recording.
//!   Default: 2.", `@range(2..10)`.
//! * `CompositionThumbnailConfiguration$targetIntervalSeconds` — "The targeted
//!   thumbnail-generation interval in seconds. Default: 60.", `@range(1..86400)`.
//! * `CompositionThumbnailConfiguration$storage` — "… You can enable both `SEQUENTIAL`
//!   and `LATEST`. Default: `SEQUENTIAL`.", a `@length(0..2)` `@uniqueItems` list.
//!
//! The members that must NOT be filled in are rows here too: `RecordingConfiguration$format`
//! documents no default, and the two optional objects of `S3DestinationConfiguration`
//! ("an optional customer specification", and the list that "allows you to enable/disable
//! the recording of thumbnails for a `Composition`") mean something by being absent.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// `CompositionRecordingHlsConfiguration$targetSegmentDurationSeconds`: "Default: 2."
const DEFAULT_COMPOSITE_SEGMENT_DURATION_SECONDS: i64 = 2;

/// `CompositionThumbnailConfiguration$targetIntervalSeconds`: "Default: 60."
const DEFAULT_THUMBNAIL_INTERVAL_SECONDS: i64 = 60;

/// `CompositionThumbnailConfiguration$storage`: "Default: `SEQUENTIAL`."
const DEFAULT_THUMBNAIL_STORAGE: &str = "SEQUENTIAL";

/// One IVS RealTime emulator over one shared engine, driven through the router that
/// serves production traffic.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
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

    /// `operation`, asserting the modelled success and answering with the member of the
    /// response body `key` names.
    async fn succeed(&self, case: &str, operation: &str, body: &Value, key: &str) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        response[key].clone()
    }

    /// The ARN of a resource this service just minted.
    async fn created_arn(&self, case: &str, operation: &str, body: &Value, key: &str) -> String {
        let created = self.succeed(case, operation, body, key).await;
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted arn, got {created}"))
            .to_string()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The resources every row of this suite refers to, resolved against this store: the
/// stage being composited and the two configurations an S3 destination names, both of
/// which are references to resources of this same service.
struct Referenced {
    stage_arn: String,
    storage_configuration_arn: String,
    encoder_configuration_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            stage_arn: service
                .created_arn(case, "CreateStage", &json!({ "name": "composited" }), "stage")
                .await,
            storage_configuration_arn: service
                .created_arn(
                    case,
                    "CreateStorageConfiguration",
                    &json!({ "s3": { "bucketName": "destination-defaults" } }),
                    "storageConfiguration",
                )
                .await,
            encoder_configuration_arn: service
                .created_arn(
                    case,
                    "CreateEncoderConfiguration",
                    &json!({ "name": "enc" }),
                    "encoderConfiguration",
                )
                .await,
        }
    }

    /// An S3 destination carrying the two `@required` members plus `extra`'s members.
    fn s3(&self, extra: Value) -> Value {
        let mut s3 = json!({
            "storageConfigurationArn": self.storage_configuration_arn,
            "encoderConfigurationArns": [self.encoder_configuration_arn],
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            s3[key] = value;
        }
        json!({ "s3": s3 })
    }
}

/// One row of the destination-configuration class: the destination a `StartComposition`
/// carries, and the whole `configuration` the composition must then report for it.
struct Row {
    case: &'static str,
    requested: Value,
    reported: Value,
}

/// The rows the class is spanned with. They differ along what the rule generalises over:
/// which KIND of destination the request carries, which of the two optional configuration
/// objects it supplies, how many members it leaves out of each, and the value it sends for
/// the ones it does supply — at each published bound of that member and between them.
fn rows(referenced: &Referenced) -> Vec<Row> {
    let channel = format!("arn:aws:ivs:{REGION}:000000000000:channel/abcdABCD1234");
    let full_thumbnail = json!({
        "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
        "storage": [DEFAULT_THUMBNAIL_STORAGE],
    });
    let default_hls = json!({
        "targetSegmentDurationSeconds": DEFAULT_COMPOSITE_SEGMENT_DURATION_SECONDS,
    });
    vec![
        Row {
            case: "a channel destination, which carries no member documenting a default",
            requested: json!({ "name": "primary", "channel": { "channelArn": channel } }),
            reported: json!({ "name": "primary", "channel": { "channelArn": channel } }),
        },
        Row {
            case: "an S3 destination supplying neither optional object",
            requested: referenced.s3(json!({})),
            reported: referenced.s3(json!({})),
        },
        Row {
            case: "a recording configuration naming only the format",
            requested: referenced.s3(json!({ "recordingConfiguration": { "format": "HLS" } })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "format": "HLS", "hlsConfiguration": default_hls },
            })),
        },
        Row {
            case: "a recording configuration carrying no member at all",
            requested: referenced.s3(json!({ "recordingConfiguration": {} })),
            reported: referenced
                .s3(json!({ "recordingConfiguration": { "hlsConfiguration": default_hls } })),
        },
        Row {
            case: "a recording configuration whose hls configuration carries no member",
            requested: referenced.s3(json!({
                "recordingConfiguration": { "format": "HLS", "hlsConfiguration": {} },
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "format": "HLS", "hlsConfiguration": default_hls },
            })),
        },
        Row {
            // The published minimum of the member's `@range`, which is also its
            // documented default: a value the request SENT, reported as it arrived.
            case: "a segment duration at the published minimum",
            requested: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 2,
                }},
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 2,
                }},
            })),
        },
        Row {
            case: "a segment duration between the published bounds",
            requested: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 7,
                }},
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 7,
                }},
            })),
        },
        Row {
            case: "a segment duration at the published maximum",
            requested: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 10,
                }},
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "hlsConfiguration": {
                    "targetSegmentDurationSeconds": 10,
                }},
            })),
        },
        Row {
            case: "one thumbnail configuration carrying no member at all",
            requested: referenced.s3(json!({ "thumbnailConfigurations": [{}] })),
            reported: referenced.s3(json!({ "thumbnailConfigurations": [full_thumbnail] })),
        },
        Row {
            case: "a thumbnail interval at the published minimum, with no storage named",
            requested: referenced
                .s3(json!({ "thumbnailConfigurations": [{ "targetIntervalSeconds": 1 }] })),
            reported: referenced.s3(json!({
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": 1,
                    "storage": [DEFAULT_THUMBNAIL_STORAGE],
                }],
            })),
        },
        Row {
            case: "a thumbnail interval at the published maximum, with no storage named",
            requested: referenced
                .s3(json!({ "thumbnailConfigurations": [{ "targetIntervalSeconds": 86_400 }] })),
            reported: referenced.s3(json!({
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": 86_400,
                    "storage": [DEFAULT_THUMBNAIL_STORAGE],
                }],
            })),
        },
        Row {
            // The storage format the issue's example never names, with no interval
            // beside it: the one member the request left out is the one that moves.
            case: "a thumbnail storage naming only the latest format",
            requested: referenced
                .s3(json!({ "thumbnailConfigurations": [{ "storage": ["LATEST"] }] })),
            reported: referenced.s3(json!({
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
                    "storage": ["LATEST"],
                }],
            })),
        },
        Row {
            case: "a thumbnail storage naming both formats, at the list's length maximum",
            requested: referenced.s3(json!({
                "thumbnailConfigurations": [{ "storage": ["SEQUENTIAL", "LATEST"] }],
            })),
            reported: referenced.s3(json!({
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": DEFAULT_THUMBNAIL_INTERVAL_SECONDS,
                    "storage": ["SEQUENTIAL", "LATEST"],
                }],
            })),
        },
        Row {
            // `CompositionThumbnailConfigurationList` is `@length(0..1)`: an empty list
            // holds no configuration to default, so nothing is added to it.
            case: "an empty thumbnail list, at the list's length minimum",
            requested: referenced.s3(json!({ "thumbnailConfigurations": [] })),
            reported: referenced.s3(json!({ "thumbnailConfigurations": [] })),
        },
        Row {
            case: "both optional objects supplied, each missing a different member",
            requested: referenced.s3(json!({
                "recordingConfiguration": { "format": "HLS" },
                "thumbnailConfigurations": [{ "targetIntervalSeconds": 30 }],
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": { "format": "HLS", "hlsConfiguration": default_hls },
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": 30,
                    "storage": [DEFAULT_THUMBNAIL_STORAGE],
                }],
            })),
        },
        Row {
            case: "every member documenting a default supplied, none at that default",
            requested: referenced.s3(json!({
                "recordingConfiguration": {
                    "format": "HLS",
                    "hlsConfiguration": { "targetSegmentDurationSeconds": 3 },
                },
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": 120,
                    "storage": ["LATEST"],
                }],
            })),
            reported: referenced.s3(json!({
                "recordingConfiguration": {
                    "format": "HLS",
                    "hlsConfiguration": { "targetSegmentDurationSeconds": 3 },
                },
                "thumbnailConfigurations": [{
                    "targetIntervalSeconds": 120,
                    "storage": ["LATEST"],
                }],
            })),
        },
    ]
}

/// Every member of a destination's configuration that documents a default is reported
/// with it when the request left it out, and every value the request sent is reported as
/// it arrived — in the start answer AND, because the defaults are persisted rather than
/// shaped onto the response, in every later read of the composition.
#[tokio::test]
async fn a_destination_reports_the_documented_default_of_every_member_left_out() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "destination defaults").await;

    for Row {
        case,
        requested,
        reported,
    } in rows(&referenced)
    {
        let request = json!({
            "stageArn": referenced.stage_arn,
            "destinations": [requested],
        });
        let started = service
            .succeed(case, "StartComposition", &request, "composition")
            .await;
        let answered = &started["destinations"][0]["configuration"];
        assert_eq!(
            answered, &reported,
            "case {case:?}: expected StartComposition to report the configuration \
             {reported}, got {answered}"
        );

        // ROUND TRIP: a read answers with the stored destination, so the defaults are
        // part of what was persisted and not a shape put on one response.
        let arn = started["arn"].as_str().unwrap_or_default();
        let read = service
            .succeed(case, "GetComposition", &json!({ "arn": arn }), "composition")
            .await;
        let answered = &read["destinations"][0]["configuration"];
        assert_eq!(
            answered, &reported,
            "case {case:?}: expected GetComposition to report the configuration \
             {reported}, got {answered}"
        );
    }
}

/// A documented default fills a member the request left OUT; it never rescues a value the
/// member's own bound excludes. Every row is one step past a published bound of a member
/// under test, and is the modelled `ValidationException` (HTTP 400).
#[tokio::test]
async fn a_value_past_a_published_bound_is_still_refused() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "refusal").await;

    // Bounds from `smithy_query(query="shape",
    // shape="CompositionRecordingTargetSegmentDurationSeconds,ThumbnailIntervalSeconds,
    // ThumbnailStorageTypeList,CompositionThumbnailConfigurationList,
    // RecordingConfigurationFormat")`.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a segment duration one below the published minimum",
            json!({ "recordingConfiguration": { "hlsConfiguration": {
                "targetSegmentDurationSeconds": 1,
            }}}),
        ),
        (
            "a segment duration one above the published maximum",
            json!({ "recordingConfiguration": { "hlsConfiguration": {
                "targetSegmentDurationSeconds": 11,
            }}}),
        ),
        (
            "a recording format outside the model's value set",
            json!({ "recordingConfiguration": { "format": "MP4" } }),
        ),
        (
            "a thumbnail interval one below the published minimum",
            json!({ "thumbnailConfigurations": [{ "targetIntervalSeconds": 0 }] }),
        ),
        (
            "a thumbnail interval one above the published maximum",
            json!({ "thumbnailConfigurations": [{ "targetIntervalSeconds": 86_401 }] }),
        ),
        (
            "a thumbnail storage format outside the model's value set",
            json!({ "thumbnailConfigurations": [{ "storage": ["FIRST"] }] }),
        ),
        (
            "two thumbnail configurations, one past the list's length maximum",
            json!({ "thumbnailConfigurations": [{}, { "targetIntervalSeconds": 60 }] }),
        ),
    ];

    for (case, extra) in refused {
        let (status, response) = service
            .call(
                "StartComposition",
                &json!({
                    "stageArn": referenced.stage_arn,
                    "destinations": [referenced.s3(extra)],
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}
