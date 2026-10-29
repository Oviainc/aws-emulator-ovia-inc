// SPDX-License-Identifier: CC0-1.0

//! The media insights PIPELINE lifecycle and its status changes, end to end over HTTP.
//!
//! The wire harness is the sibling suite's ([`crate::kinesis_video_stream_pool_lifecycle`]):
//! one `MultiServiceRouter` with this service mounted on one shared engine, so every
//! assertion here is about the emulator's HTTP behaviour rather than an internal helper.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a create runs a STORED media insights pipeline configuration, so a create whose
//!   configuration the store does not hold is the modelled 404 `NotFoundException`;
//! * a create MINTS and PERSISTS a pipeline — its own `MediaPipelineId`, an ARN derived
//!   from the signed region and that id, the caller's runtime configurations stored as
//!   submitted — and `GetMediaPipeline`/`ListMediaPipelines` read it back as a media
//!   pipeline of its own kind, beside the capture pipelines;
//! * a pipeline is identified by its ID or its ARN, and an identifier that resolves to no
//!   stored pipeline is the modelled 404 on the status update;
//! * `Pause` is made from the running status and `Resume` from the paused one, each
//!   observable by every later read; every other current status is the modelled 409
//!   `ConflictException`, which names the status it clashed with.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    INITIALIZING_TO_IN_PROGRESS, STATUS_INITIALIZING, STATUS_IN_PROGRESS, STATUS_PAUSED,
};

use crate::kinesis_video_stream_pool_lifecycle::{encode_label, Emulator, Response, ACCOUNT, REGION};

/// The routes this suite drives, from `smithy_query(query="digest")`.
const CONFIGURATIONS: &str = "/media-insights-pipeline-configurations";
const PIPELINES: &str = "/media-insights-pipelines";
const PIPELINE_STATUS: &str = "/media-insights-pipeline-status";
const MEDIA_PIPELINES: &str = "/sdk-media-pipelines";

/// The `ErrorCode` value the modelled `NotFoundException` carries in its body `Code`
/// member, from `smithy_query(query="shape",shape="ErrorCode")`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The wire error-shape names restJson1 resolves from `x-amzn-errortype`.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// A `ResourceAccessRoleArn` satisfying the modelled `Arn` shape.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-access";

/// A Kinesis Video Stream ARN satisfying the modelled `KinesisVideoStreamArn` pattern
/// `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`.
const STREAM_ARN: &str =
    "arn:aws:kinesisvideo:us-west-2:000000000000:stream/insights-stream/1700000000000";

/// Create one media insights pipeline configuration and answer its ARN: the resource a
/// pipeline runs, so every pipeline row starts from one.
pub(crate) async fn configuration_arn(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .call(
            "POST",
            CONFIGURATIONS,
            Some(json!({
                "MediaInsightsPipelineConfigurationName": name,
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "AmazonTranscribeProcessor",
                        "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
                    },
                    {
                        "Type": "KinesisDataStreamSink",
                        "KinesisDataStreamSinkConfiguration": {
                            "InsightsTarget": format!(
                                "arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"
                            ),
                        },
                    },
                ],
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the configuration {name:?} must be created: {:?}",
        created.body
    );
    created.str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn")
}

/// The pipeline create inputs the rules are asserted over, as a function of the
/// configuration ARN the row runs. The rows differ along what the rules generalise over:
/// which of the optional runtime configurations the create carries, how many streams the
/// source configuration names, and whether runtime metadata and tags are present.
fn pipeline_rows(configuration_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "the configuration alone, nothing optional",
            json!({ "MediaInsightsPipelineConfigurationArn": configuration_arn }),
        ),
        (
            "one source stream, with runtime metadata",
            json!({
                "MediaInsightsPipelineConfigurationArn": configuration_arn,
                "KinesisVideoStreamSourceRuntimeConfiguration": {
                    "Streams": [{
                        "StreamArn": STREAM_ARN,
                        "StreamChannelDefinition": { "NumberOfChannels": 1 },
                    }],
                    "MediaEncoding": "pcm",
                    "MediaSampleRate": 8000,
                },
                "MediaInsightsRuntimeMetadata": { "CallId": "call-1", "Agent": "a-1" },
            }),
        ),
        (
            "the two streams the model's list allows, both channels, an S3 recording sink",
            json!({
                "MediaInsightsPipelineConfigurationArn": configuration_arn,
                "KinesisVideoStreamSourceRuntimeConfiguration": {
                    "Streams": [
                        {
                            "StreamArn": STREAM_ARN,
                            "FragmentNumber": "91343852333181432392682062607124",
                            "StreamChannelDefinition": {
                                "NumberOfChannels": 2,
                                "ChannelDefinitions": [
                                    { "ChannelId": 0, "ParticipantRole": "AGENT" },
                                    { "ChannelId": 1, "ParticipantRole": "CUSTOMER" },
                                ],
                            },
                        },
                        {
                            "StreamArn": STREAM_ARN,
                            "StreamChannelDefinition": { "NumberOfChannels": 1 },
                        },
                    ],
                    "MediaEncoding": "pcm",
                    // The top of the modelled 8000..48000 range.
                    "MediaSampleRate": 48000,
                },
                "S3RecordingSinkRuntimeConfiguration": {
                    "Destination": "arn:aws:s3:::insights-bucket/recordings",
                    "RecordingFileFormat": "Wav",
                },
                "Tags": [{ "Key": "team", "Value": "voice" }],
            }),
        ),
    ]
}

/// RULE: a create mints and persists a media insights pipeline, and every later read of
/// the media-pipeline surface renders it from the stored record.
#[tokio::test]
async fn a_media_insights_pipeline_is_created_and_read_back_as_a_media_pipeline() {
    for (case, input) in pipeline_rows("placeholder") {
        let emulator = Emulator::new();
        let arn = configuration_arn(&emulator, "insights-conf").await;
        let mut input = input;
        input["MediaInsightsPipelineConfigurationArn"] = json!(arn);

        let created = emulator.call("POST", PIPELINES, Some(input.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected the modelled 201, got {} body {:?}",
            created.status, created.body
        );

        let pipeline = "/MediaInsightsPipeline";
        let id = created.str_at(&format!("{pipeline}/MediaPipelineId"));
        assert!(
            is_guid(&id),
            "case {case:?}: expected a minted media pipeline id, got {id:?}"
        );
        assert_eq!(
            created.str_at(&format!("{pipeline}/MediaPipelineArn")),
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{id}"),
            "case {case:?}: the ARN must be derived from the signed region and the minted id"
        );
        assert_eq!(
            created.str_at(&format!("{pipeline}/MediaInsightsPipelineConfigurationArn")),
            arn,
            "case {case:?}: the pipeline must report the configuration it runs"
        );
        assert_eq!(
            created.str_at(&format!("{pipeline}/Status")),
            STATUS_INITIALIZING,
            "case {case:?}: a freshly created pipeline is in the transient status: {:?}",
            created.body
        );
        // What the caller submitted is stored and reported as submitted.
        for member in [
            "KinesisVideoStreamSourceRuntimeConfiguration",
            "MediaInsightsRuntimeMetadata",
            "S3RecordingSinkRuntimeConfiguration",
        ] {
            assert_eq!(
                created.body.pointer(&format!("{pipeline}/{member}")),
                input.get(member),
                "case {case:?} / {member}: expected it reported exactly as submitted"
            );
        }

        // Read back through the media-pipeline surface: the insights member is the one
        // populated, and it renders the stored record.
        let read = emulator
            .call("GET", &format!("{MEDIA_PIPELINES}/{id}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetMediaPipeline must serve the minted id, got {} body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body.pointer("/MediaPipeline/MediaInsightsPipeline"),
            created.body.pointer(pipeline),
            "case {case:?}: GetMediaPipeline must render the stored record"
        );
        for absent in [
            "MediaCapturePipeline",
            "MediaConcatenationPipeline",
            "MediaLiveConnectorPipeline",
            "MediaStreamPipeline",
        ] {
            assert_eq!(
                read.body.pointer(&format!("/MediaPipeline/{absent}")),
                None,
                "case {case:?}: the id names an insights pipeline, so {absent} stays absent"
            );
        }

        // And it is listed beside the other kinds of media pipeline.
        let capture = emulator
            .call(
                "POST",
                "/sdk-media-capture-pipelines",
                Some(json!({
                    "SourceType": "ChimeSdkMeeting",
                    "SourceArn": format!("arn:aws:chime:{REGION}:{ACCOUNT}:meeting/m-1"),
                    "SinkType": "S3Bucket",
                    "SinkArn": "arn:aws:s3:::captures-bucket/captures",
                })),
            )
            .await;
        assert_eq!(capture.status, 201, "case {case:?}: {:?}", capture.body);
        let capture_id = capture.str_at("/MediaCapturePipeline/MediaPipelineId");
        let listed = emulator.call("GET", MEDIA_PIPELINES, None).await;
        assert_eq!(
            listed_ids(&listed),
            BTreeSet::from([id.clone(), capture_id]),
            "case {case:?}: ListMediaPipelines reports every kind of media pipeline: {:?}",
            listed.body
        );
    }
}

/// Every `MediaPipelineId` a `ListMediaPipelines` page reports.
fn listed_ids(response: &Response) -> BTreeSet<String> {
    response
        .body
        .pointer("/MediaPipelines")
        .and_then(Value::as_array)
        .map(|pipelines| {
            pipelines
                .iter()
                .map(|pipeline| {
                    pipeline["MediaPipelineId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `id` is a 36-character GUID, the model's `GuidString`.
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

/// RULE: a pipeline runs a STORED configuration, so a create naming one the store does not
/// hold is the modelled 404 `NotFoundException` and stores nothing.
#[tokio::test]
async fn a_create_naming_no_stored_configuration_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let live = configuration_arn(&emulator, "live-conf").await;

    // The rows differ along what the resolution generalises over: this service's own ARN
    // form for a configuration nothing holds, an ARN of another resource type in the same
    // namespace, an ARN of another service, and the ARN of a configuration that WAS held
    // and has since been deleted.
    let deleted = configuration_arn(&emulator, "deleted-conf").await;
    assert_eq!(
        emulator
            .call(
                "DELETE",
                &format!("{CONFIGURATIONS}/{}", encode_label(&deleted)),
                None
            )
            .await
            .status,
        204
    );
    let rows = vec![
        (
            "this service's own ARN form for a configuration nothing holds",
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/no-such-conf"
            ),
        ),
        (
            "an ARN of another resource type in the same namespace",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"),
        ),
        ("the ARN of a configuration since deleted", deleted),
    ];

    for (case, configuration) in rows {
        let refused = emulator
            .call(
                "POST",
                PIPELINES,
                Some(json!({ "MediaInsightsPipelineConfigurationArn": configuration })),
            )
            .await;
        assert_not_found(case, &refused);
    }

    // Nothing was stored for any of them, while the live configuration still serves.
    let listed = emulator.call("GET", MEDIA_PIPELINES, None).await;
    assert_eq!(
        listed_ids(&listed),
        BTreeSet::new(),
        "a refused create must store no pipeline: {:?}",
        listed.body
    );
    let served = emulator
        .call(
            "POST",
            PIPELINES,
            Some(json!({ "MediaInsightsPipelineConfigurationArn": live })),
        )
        .await;
    assert_eq!(
        served.status, 201,
        "the live configuration must still serve a create: {:?}",
        served.body
    );
}

/// Assert `response` is this service's modelled 404 `NotFoundException`, in the restJson1
/// error framing.
fn assert_not_found(case: &str, response: &Response) {
    assert_eq!(
        response.status, 404,
        "case {case:?}: expected the modelled 404, got {} body {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled error shape, got {:?}",
        response.body
    );
    assert_eq!(
        response.str_at("/Code"),
        ERROR_CODE_NOT_FOUND,
        "case {case:?}: expected the modelled ErrorCode, got {:?}",
        response.body
    );
    assert!(
        !response.str_at("/Message").is_empty(),
        "case {case:?}: the refusal must say what was not found: {:?}",
        response.body
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
}

/// RULE: `UpdateMediaInsightsPipelineStatus` resolves the pipeline by its ID or its ARN,
/// and an identifier that resolves to none is the modelled 404 `NotFoundException` — the
/// not-found probe a client makes before it acts.
#[tokio::test]
async fn a_status_update_on_an_identifier_that_names_no_pipeline_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let configuration = configuration_arn(&emulator, "probe-conf").await;
    // One live pipeline and one live capture pipeline, so the refusals are about
    // resolution rather than about an empty store.
    let live = emulator
        .call(
            "POST",
            PIPELINES,
            Some(json!({ "MediaInsightsPipelineConfigurationArn": configuration.clone() })),
        )
        .await;
    assert_eq!(live.status, 201, "{:?}", live.body);
    let capture = emulator
        .call(
            "POST",
            "/sdk-media-capture-pipelines",
            Some(json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": format!("arn:aws:chime:{REGION}:{ACCOUNT}:meeting/m-2"),
                "SinkType": "S3Bucket",
                "SinkArn": "arn:aws:s3:::captures-bucket/captures",
            })),
        )
        .await;
    assert_eq!(capture.status, 201, "{:?}", capture.body);

    // The rows differ along what the resolution generalises over: a GUID nothing was
    // created under, this service's own ARN form for such a GUID, the ARN and the id of a
    // media pipeline of ANOTHER kind, the configuration's own ARN, and a value that is no
    // identifier form at all.
    let rows = vec![
        (
            "a GUID nothing was created under",
            "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e".to_string(),
        ),
        (
            "this service's own ARN form for such a GUID",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        (
            "the id of a media pipeline of another kind",
            capture.str_at("/MediaCapturePipeline/MediaPipelineId"),
        ),
        (
            "the ARN of a media pipeline of another kind",
            capture.str_at("/MediaCapturePipeline/MediaPipelineArn"),
        ),
        ("the configuration's own ARN", configuration),
        ("a value that is no identifier form", "nope".to_string()),
    ];

    for (case, identifier) in rows {
        for update in ["Pause", "Resume"] {
            let refused = emulator
                .call(
                    "PUT",
                    &format!("{PIPELINE_STATUS}/{}", encode_label(&identifier)),
                    Some(json!({ "UpdateStatus": update })),
                )
                .await;
            assert_not_found(&format!("{case} / {update}"), &refused);
        }
    }

    // The live pipeline is untouched by any of those refusals.
    let id = live.str_at("/MediaInsightsPipeline/MediaPipelineId");
    let read = emulator
        .call("GET", &format!("{MEDIA_PIPELINES}/{id}"), None)
        .await;
    assert_eq!(
        read.status, 200,
        "a refused status update must leave its neighbours alone: {:?}",
        read.body
    );
}

/// RULE: `Pause` is made from the running status and `Resume` from the paused one, each
/// observable by every later read and addressable by either identifying form; every other
/// current status is the modelled 409 `ConflictException` naming the status it clashed with,
/// and leaves the pipeline as it was.
#[tokio::test]
async fn pause_and_resume_move_the_status_and_clash_with_every_other_one() {
    for (case, identify_by_arn) in [("by id", false), ("by ARN", true)] {
        let emulator = Emulator::new();
        let configuration = configuration_arn(&emulator, "status-conf").await;
        let created = emulator
            .call(
                "POST",
                PIPELINES,
                Some(json!({ "MediaInsightsPipelineConfigurationArn": configuration })),
            )
            .await;
        assert_eq!(created.status, 201, "case {case:?}: {:?}", created.body);
        let id = created.str_at("/MediaInsightsPipeline/MediaPipelineId");
        let identifier = if identify_by_arn {
            created.str_at("/MediaInsightsPipeline/MediaPipelineArn")
        } else {
            id.clone()
        };

        // A pause asked for while the pipeline is still initialising clashes with that
        // status: the transition to the running one is the clock's, and it is not due yet.
        let too_early = update_status(&emulator, &identifier, "Pause").await;
        assert_conflict(
            &format!("{case} / a pause while still initialising"),
            &too_early,
            STATUS_INITIALIZING,
        );
        assert_eq!(
            reported_status(&emulator, &id).await,
            STATUS_INITIALIZING,
            "case {case:?}: a refused pause must leave the status alone"
        );

        // Past the span the owed transition falls due in, the pipeline is running.
        tokio::time::sleep(INITIALIZING_TO_IN_PROGRESS + Duration::from_millis(200)).await;
        assert_eq!(
            reported_status(&emulator, &id).await,
            STATUS_IN_PROGRESS,
            "case {case:?}: the owed transition must have been materialised"
        );

        // Resuming a running pipeline clashes with the running status.
        assert_conflict(
            &format!("{case} / a resume while running"),
            &update_status(&emulator, &identifier, "Resume").await,
            STATUS_IN_PROGRESS,
        );

        // Pause: served, and observable by every later read.
        let paused = update_status(&emulator, &identifier, "Pause").await;
        assert_eq!(
            paused.status, 200,
            "case {case:?}: expected the modelled 200, got {} body {:?}",
            paused.status, paused.body
        );
        assert_eq!(
            reported_status(&emulator, &id).await,
            STATUS_PAUSED,
            "case {case:?}: the pause must be observable by a later read"
        );

        // Pausing again clashes with the paused status.
        assert_conflict(
            &format!("{case} / a pause while paused"),
            &update_status(&emulator, &identifier, "Pause").await,
            STATUS_PAUSED,
        );

        // Resume: served, and back to the running status.
        assert_eq!(
            update_status(&emulator, &identifier, "Resume").await.status,
            200,
            "case {case:?}: a resume from the paused status is served"
        );
        assert_eq!(
            reported_status(&emulator, &id).await,
            STATUS_IN_PROGRESS,
            "case {case:?}: the resume must be observable by a later read"
        );
    }
}

/// The `Status` `GetMediaPipeline` reports for the media insights pipeline `id` names.
async fn reported_status(emulator: &Emulator, id: &str) -> String {
    emulator
        .call("GET", &format!("{MEDIA_PIPELINES}/{id}"), None)
        .await
        .str_at("/MediaPipeline/MediaInsightsPipeline/Status")
}

/// Ask for a status change (`Pause` / `Resume`) on the pipeline `identifier` names.
async fn update_status(emulator: &Emulator, identifier: &str, to: &str) -> Response {
    emulator
        .call(
            "PUT",
            &format!("{PIPELINE_STATUS}/{}", encode_label(identifier)),
            Some(json!({ "UpdateStatus": to })),
        )
        .await
}

/// Assert `response` is the modelled 409 `ConflictException`, and that it names the status
/// the requested change clashed with.
fn assert_conflict(case: &str, response: &Response, clashing_status: &str) {
    assert_eq!(
        response.status, 409,
        "case {case:?}: expected the modelled 409, got {} body {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        CONFLICT_EXCEPTION,
        "case {case:?}: expected the modelled error shape, got {:?}",
        response.body
    );
    let message = response.str_at("/Message");
    assert!(
        message.contains(clashing_status),
        "case {case:?}: the refusal must name the status it clashed with ({clashing_status}), got {message:?}"
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
}
