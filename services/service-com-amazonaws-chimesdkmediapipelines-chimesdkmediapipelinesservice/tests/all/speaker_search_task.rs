// SPDX-License-Identifier: CC0-1.0

//! The speaker search TASK surface, end to end over HTTP.
//!
//! The wire harness is the sibling suite's ([`crate::kinesis_video_stream_pool_lifecycle`]):
//! one `MultiServiceRouter` with this service mounted on one shared engine, so every
//! assertion here is about the emulator's HTTP behaviour rather than an internal helper.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * speaker search is a biometric feature an account is enrolled for out of band, so
//!   every voice-analytics task call from an account that is not enrolled is the modelled
//!   403 `ForbiddenException` carrying the service's fixed wording, decided BEFORE either
//!   identifier is resolved — whatever the request names, and storing nothing;
//! * for an enrolled account a start MINTS and PERSISTS a task against the stored media
//!   insights pipeline, and `GetSpeakerSearchTask` reads it back under either identifying
//!   form of that pipeline;
//! * `Initializing` is non-terminal: a caller reading straight back still sees it, and once
//!   the span has elapsed every read sees `InProgress`;
//! * a task is addressed under the pipeline it was started on, so an identifier naming no
//!   stored pipeline — and a task id that pipeline holds no task under — is the modelled
//!   404 `NotFoundException` with this service's fixed, identifier-free message;
//! * a repeated `ClientRequestToken` answers with the task the first start minted.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::ops::types::SpeakerSearchTask;
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    NOT_FOUND_MESSAGE, TASK_INITIALIZING_TO_IN_PROGRESS, TASK_STATUS_INITIALIZING,
    TASK_STATUS_IN_PROGRESS, VOICE_ANALYTICS_NOT_ENABLED_MESSAGE,
};

use crate::kinesis_video_stream_pool_lifecycle::{
    encode_label, is_guid, iso8601_seconds, Emulator, Response, ACCOUNT, REGION,
};
use crate::media_insights_pipeline_status::configuration_arn;

/// The routes this suite drives, from `smithy_query(query="digest")`:
/// `POST /media-insights-pipelines/{Identifier}/speaker-search-tasks?operation=start` and
/// `GET /media-insights-pipelines/{Identifier}/speaker-search-tasks/{SpeakerSearchTaskId}`.
const PIPELINES: &str = "/media-insights-pipelines";
const TASKS: &str = "speaker-search-tasks";

/// The wire error-shape names restJson1 resolves from `x-amzn-errortype`.
const FORBIDDEN_EXCEPTION: &str = "ForbiddenException";
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The `ErrorCode` values the modelled errors carry in their body `Code` member, from
/// `smithy_query(query="shape",shape="ErrorCode")` — `BadRequest | Forbidden | NotFound |
/// ResourceLimitExceeded | ServiceFailure | ServiceUnavailable | Throttling`.
const ERROR_CODE_FORBIDDEN: &str = "Forbidden";
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// A `VoiceProfileDomainArn` satisfying the modelled `Arn` pattern
/// `^arn[\/\:\-\_\.a-zA-Z0-9]+$`.
fn domain_arn(name: &str) -> String {
    format!("arn:aws:chime:{REGION}:{ACCOUNT}:voice-profile-domain/{name}")
}

/// A Kinesis Video Stream ARN satisfying the modelled `KinesisVideoStreamArn` pattern
/// `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`.
const STREAM_ARN: &str =
    "arn:aws:kinesisvideo:us-west-2:000000000000:stream/speaker-stream/1700000000000";

/// A GUID the store minted nothing under, satisfying the modelled `GuidString`.
const UNMINTED_GUID: &str = "11111111-1111-4111-8111-111111111111";

/// Create one media insights pipeline and answer its id and ARN: the resource a speaker
/// search task runs on, so every row starts from one.
async fn pipeline(emulator: &Emulator, name: &str) -> (String, String) {
    let configuration = configuration_arn(emulator, name).await;
    let created = emulator
        .call(
            "POST",
            PIPELINES,
            Some(json!({ "MediaInsightsPipelineConfigurationArn": configuration })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the media insights pipeline {name:?} must be created: {:?}",
        created.body
    );
    (
        created.str_at("/MediaInsightsPipeline/MediaPipelineId"),
        created.str_at("/MediaInsightsPipeline/MediaPipelineArn"),
    )
}

/// Start a speaker search task on the pipeline `identifier` names.
async fn start(emulator: &Emulator, identifier: &str, input: Value) -> Response {
    emulator
        .call(
            "POST",
            &format!(
                "{PIPELINES}/{}/{TASKS}?operation=start",
                encode_label(identifier)
            ),
            Some(input),
        )
        .await
}

/// Read the speaker search task `task_id` names on the pipeline `identifier` names.
async fn get(emulator: &Emulator, identifier: &str, task_id: &str) -> Response {
    emulator
        .call(
            "GET",
            &format!("{PIPELINES}/{}/{TASKS}/{task_id}", encode_label(identifier)),
            None,
        )
        .await
}

/// Record that this account is enrolled for the voice-analytics features. The enrolment is
/// made out of band — no modelled operation of this service carries it — so a suite that
/// drives the enrolled behaviour writes it to the store the handlers read.
fn enrol(emulator: &Emulator) {
    emulator.store().lock().set_voice_analytics_enabled(true);
}

/// The start inputs the rules are asserted over. The rows differ along what the rules
/// generalise over: which voice profile domain the task matches against, whether the
/// optional Kinesis video stream source task configuration is present and which of the
/// modelled 0..1 channels and fragment numbers it names, and whether a
/// `ClientRequestToken` is carried.
fn start_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the voice profile domain alone, nothing optional",
            json!({ "VoiceProfileDomainArn": domain_arn("domain-a") }),
        ),
        (
            "a stream source on the first channel",
            json!({
                "VoiceProfileDomainArn": domain_arn("domain-b"),
                "KinesisVideoStreamSourceTaskConfiguration": {
                    "StreamArn": STREAM_ARN,
                    // The bottom of the modelled 0..1 `ChannelId` range.
                    "ChannelId": 0,
                },
            }),
        ),
        (
            "a stream source on the top channel, from a fragment, with a request token",
            json!({
                "VoiceProfileDomainArn": domain_arn("domain.c-1_2"),
                "KinesisVideoStreamSourceTaskConfiguration": {
                    "StreamArn": STREAM_ARN,
                    // The top of the modelled 0..1 `ChannelId` range.
                    "ChannelId": 1,
                    "FragmentNumber": "91343852333181432392682062607124",
                },
                "ClientRequestToken": "speaker-search-token-1",
            }),
        ),
    ]
}

/// RULE: speaker search is a biometric feature an account is enrolled for out of band, so
/// a call from an account that is not enrolled is the modelled 403 `ForbiddenException`
/// with the service's fixed wording — decided before either identifier is resolved, so it
/// is the answer whatever the request names, and nothing is started.
#[tokio::test]
async fn an_account_not_enrolled_for_voice_analytics_is_forbidden_whatever_it_names() {
    let emulator = Emulator::new();
    let (live_id, live_arn) = pipeline(&emulator, "forbidden-conf").await;

    // The rows differ along what the gate generalises over: whether the identifier names a
    // stored pipeline at all, which identifying form it uses, and whether it is an
    // identifier form of this service in the first place.
    let identifiers = vec![
        ("the id of a stored pipeline", live_id.clone()),
        ("the ARN of a stored pipeline", live_arn),
        ("a GUID nothing was created under", UNMINTED_GUID.to_string()),
        (
            "this service's own ARN form for such a GUID",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{UNMINTED_GUID}"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"),
        ),
        ("a value that is no identifier form", "nope".to_string()),
    ];

    for (identified, identifier) in &identifiers {
        for (row, input) in start_rows() {
            let case = format!("start / {identified} / {row}");
            let refused = start(&emulator, identifier, input).await;
            assert_forbidden(&case, &refused);
        }
        for task in [UNMINTED_GUID, "22222222-2222-4222-8222-222222222222"] {
            let case = format!("get / {identified} / task {task}");
            assert_forbidden(&case, &get(&emulator, identifier, task).await);
        }
    }

    // Nothing was started by any refused call: once the account IS enrolled, the stored
    // pipeline holds no task under any of the ids a refused start could have minted.
    enrol(&emulator);
    for task in [UNMINTED_GUID, "22222222-2222-4222-8222-222222222222"] {
        assert_not_found(
            &format!("a refused start stored nothing / task {task}"),
            &get(&emulator, &live_id, task).await,
        );
    }
}

/// Assert `response` is the modelled 403 `ForbiddenException`, carrying this service's
/// fixed wording for an account that is not enrolled for voice analytics.
fn assert_forbidden(case: &str, response: &Response) {
    assert_eq!(
        response.status, 403,
        "case {case:?}: expected the modelled 403, got {} body {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        FORBIDDEN_EXCEPTION,
        "case {case:?}: expected the modelled error shape, got {:?}",
        response.body
    );
    assert_eq!(
        response.str_at("/Code"),
        ERROR_CODE_FORBIDDEN,
        "case {case:?}: expected the modelled ErrorCode, got {:?}",
        response.body
    );
    assert_eq!(
        response.str_at("/Message"),
        VOICE_ANALYTICS_NOT_ENABLED_MESSAGE,
        "case {case:?}: expected the service's fixed wording, got {:?}",
        response.body
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
}

/// Assert `response` is the modelled 404 `NotFoundException`, with this service's fixed
/// message — which echoes no identifier from the request.
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
    assert_eq!(
        response.str_at("/Message"),
        NOT_FOUND_MESSAGE,
        "case {case:?}: expected the service's fixed message, got {:?}",
        response.body
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
}

/// RULE: for an enrolled account a start mints and persists a task against the stored
/// pipeline, and every later read renders it from the stored record under either
/// identifying form of that pipeline.
#[tokio::test]
async fn a_started_task_is_minted_persisted_and_read_back() {
    for (case, input) in start_rows() {
        let emulator = Emulator::new();
        enrol(&emulator);
        let (id, arn) = pipeline(&emulator, "started-conf").await;
        // A second pipeline, so a read proves the task is addressed under the pipeline it
        // was started on rather than found anywhere in the store.
        let (other_id, _) = pipeline(&emulator, "other-conf").await;

        let started = start(&emulator, &id, input).await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: expected the modelled 201, got {} body {:?}",
            started.status, started.body
        );
        let task = "/SpeakerSearchTask";
        let task_id = started.str_at(&format!("{task}/SpeakerSearchTaskId"));
        assert!(
            is_guid(&task_id),
            "case {case:?}: expected a minted speaker search task id, got {task_id:?}"
        );
        assert_eq!(
            started.str_at(&format!("{task}/SpeakerSearchTaskStatus")),
            TASK_STATUS_INITIALIZING,
            "case {case:?}: a freshly started task is in the transient status: {:?}",
            started.body
        );
        let created_at = iso8601_seconds(case, &started.str_at(&format!("{task}/CreatedTimestamp")));
        assert_eq!(
            iso8601_seconds(case, &started.str_at(&format!("{task}/UpdatedTimestamp"))),
            created_at,
            "case {case:?}: a task that has not changed yet reports one instant for both: {:?}",
            started.body
        );

        // Read back under both identifying forms of the pipeline: the same stored record.
        for (identified, identifier) in [("by id", &id), ("by ARN", &arn)] {
            let read = get(&emulator, identifier, &task_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} / {identified}: the started task must be served, got {} body {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body.pointer(task),
                started.body.pointer(task),
                "case {case:?} / {identified}: the read must render the stored record"
            );
        }

        // The task belongs to the pipeline it was started on, and to no other; and that
        // pipeline holds no task under an id nothing minted.
        assert_not_found(
            &format!("{case} / the same task id under another pipeline"),
            &get(&emulator, &other_id, &task_id).await,
        );
        assert_not_found(
            &format!("{case} / an id nothing minted under the right pipeline"),
            &get(&emulator, &id, UNMINTED_GUID).await,
        );
    }
}

/// RULE: `Initializing` is a non-terminal `MediaPipelineTaskStatus`, so the move to
/// `InProgress` is the clock's: a caller reading straight back still sees the transient
/// status, and once the span has elapsed every read sees the running one, timestamped when
/// the transition fell due rather than when it was observed.
#[tokio::test]
async fn the_task_reaches_the_running_status_the_clock_owes() {
    let emulator = Emulator::new();
    enrol(&emulator);
    let (id, _) = pipeline(&emulator, "clock-conf").await;
    let started = start(
        &emulator,
        &id,
        json!({ "VoiceProfileDomainArn": domain_arn("clock-domain") }),
    )
    .await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    let task_id = started.str_at("/SpeakerSearchTask/SpeakerSearchTaskId");

    let straight_back = get(&emulator, &id, &task_id).await;
    assert_eq!(
        straight_back.str_at("/SpeakerSearchTask/SpeakerSearchTaskStatus"),
        TASK_STATUS_INITIALIZING,
        "a caller reading straight back must still see the transient status: {:?}",
        straight_back.body
    );

    tokio::time::sleep(TASK_INITIALIZING_TO_IN_PROGRESS + Duration::from_millis(200)).await;
    let settled = get(&emulator, &id, &task_id).await;
    assert_eq!(
        settled.str_at("/SpeakerSearchTask/SpeakerSearchTaskStatus"),
        TASK_STATUS_IN_PROGRESS,
        "the owed transition must have been materialised: {:?}",
        settled.body
    );
    let created_at = iso8601_seconds(
        "the owed transition",
        &settled.str_at("/SpeakerSearchTask/CreatedTimestamp"),
    );
    let updated_at = iso8601_seconds(
        "the owed transition",
        &settled.str_at("/SpeakerSearchTask/UpdatedTimestamp"),
    );
    assert!(
        updated_at >= created_at + TASK_INITIALIZING_TO_IN_PROGRESS.as_secs_f64(),
        "the transition must be timestamped when it fell due, got created {created_at} \
         updated {updated_at}"
    );
    assert_eq!(
        iso8601_seconds(
            "a settled read is stable",
            &get(&emulator, &id, &task_id)
                .await
                .str_at("/SpeakerSearchTask/UpdatedTimestamp")
        ),
        updated_at,
        "a later read must not move the instant the transition was materialised at"
    );
}

/// RULE: a task runs ON a media insights pipeline, so a start whose identifier resolves to
/// no stored pipeline is the modelled 404 `NotFoundException` and starts nothing.
#[tokio::test]
async fn a_start_naming_no_stored_pipeline_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    enrol(&emulator);
    let (live_id, _) = pipeline(&emulator, "probe-conf").await;
    // A media pipeline of another kind and a configuration, so the refusals are about
    // resolving a media INSIGHTS pipeline rather than about an empty store.
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
    assert_eq!(capture.status, 201, "{:?}", capture.body);
    let configuration = configuration_arn(&emulator, "probe-only-conf").await;

    // The rows differ along what the resolution generalises over: a GUID nothing was
    // created under, this service's own ARN form for it, the id and the ARN of a media
    // pipeline of ANOTHER kind, a configuration's ARN, and a value that is no identifier
    // form at all.
    let rows = vec![
        (
            "a GUID nothing was created under",
            UNMINTED_GUID.to_string(),
        ),
        (
            "this service's own ARN form for such a GUID",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{UNMINTED_GUID}"),
        ),
        (
            "the id of a media pipeline of another kind",
            capture.str_at("/MediaCapturePipeline/MediaPipelineId"),
        ),
        (
            "the ARN of a media pipeline of another kind",
            capture.str_at("/MediaCapturePipeline/MediaPipelineArn"),
        ),
        ("a configuration's own ARN", configuration),
        ("a value that is no identifier form", "nope".to_string()),
    ];

    for (case, identifier) in rows {
        for (row, input) in start_rows() {
            let refused = start(&emulator, &identifier, input).await;
            assert_not_found(&format!("{case} / {row}"), &refused);
        }
    }

    // And the stored pipeline still serves a start, which the refusals left room for.
    let served = start(
        &emulator,
        &live_id,
        json!({ "VoiceProfileDomainArn": domain_arn("probe-domain") }),
    )
    .await;
    assert_eq!(
        served.status, 201,
        "the stored pipeline must still serve a start: {:?}",
        served.body
    );
}

/// RULE: a shape's schema carries exactly the members the model declares for it, in one
/// order, each at its own index, with the wire traits the model gives them.
///
/// `SpeakerSearchTask`'s schema is hand-written (see the comment on it in
/// `src/ops/types/shared_types.rs`) because the derive drops `@timestampFormat`.
/// Hand-written means it can drift from the model, so this test pins it: the member list
/// below is the model's, from `smithy_query(query="shape", shape="SpeakerSearchTask")`, and
/// the two timestamp members must report the `date-time` format that is the whole reason
/// the block exists.
#[test]
fn the_hand_written_speaker_search_task_schema_matches_the_model() {
    // From `smithy_query(query="shape", shape="SpeakerSearchTask")`: every member the model
    // declares, in declaration order.
    const MODEL_MEMBERS: [&str; 4] = [
        "SpeakerSearchTaskId",
        "SpeakerSearchTaskStatus",
        "CreatedTimestamp",
        "UpdatedTimestamp",
    ];
    // The members the model targets at `Iso8601Timestamp`, which carries
    // `@timestampFormat("date-time")`.
    const DATE_TIME_MEMBERS: [&str; 2] = ["CreatedTimestamp", "UpdatedTimestamp"];

    let members = SpeakerSearchTask::SCHEMA.members();
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

/// RULE: `ClientRequestToken` is an idempotency token, so a repeated start under one token
/// answers with the task the first one minted rather than minting a second.
#[tokio::test]
async fn a_repeated_client_request_token_answers_with_the_first_task() {
    let emulator = Emulator::new();
    enrol(&emulator);
    let (id, arn) = pipeline(&emulator, "token-conf").await;

    // The rows differ along what the replay generalises over: the identifying form the
    // replay is addressed by, and the parameters it carries beside the token.
    let token = "repeated-token-1";
    let first = start(
        &emulator,
        &id,
        json!({
            "VoiceProfileDomainArn": domain_arn("token-domain"),
            "ClientRequestToken": token,
        }),
    )
    .await;
    assert_eq!(first.status, 201, "{:?}", first.body);
    let first_id = first.str_at("/SpeakerSearchTask/SpeakerSearchTaskId");

    for (case, identifier, input) in [
        (
            "the same request, by id",
            id.clone(),
            json!({
                "VoiceProfileDomainArn": domain_arn("token-domain"),
                "ClientRequestToken": token,
            }),
        ),
        (
            "other parameters under the same token, by ARN",
            arn.clone(),
            json!({
                "VoiceProfileDomainArn": domain_arn("another-domain"),
                "KinesisVideoStreamSourceTaskConfiguration": {
                    "StreamArn": STREAM_ARN,
                    "ChannelId": 1,
                },
                "ClientRequestToken": token,
            }),
        ),
    ] {
        let replayed = start(&emulator, &identifier, input).await;
        assert_eq!(
            replayed.status, 201,
            "case {case:?}: expected the modelled 201, got {} body {:?}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.str_at("/SpeakerSearchTask/SpeakerSearchTaskId"),
            first_id,
            "case {case:?}: a replayed token must answer with the first task: {:?}",
            replayed.body
        );
    }

    // A different token on the same pipeline starts its own task.
    let other = start(
        &emulator,
        &id,
        json!({
            "VoiceProfileDomainArn": domain_arn("token-domain"),
            "ClientRequestToken": "another-token-2",
        }),
    )
    .await;
    assert_eq!(other.status, 201, "{:?}", other.body);
    let other_id = other.str_at("/SpeakerSearchTask/SpeakerSearchTaskId");
    assert_ne!(
        other_id, first_id,
        "a different token must start its own task: {:?}",
        other.body
    );
    // Both tasks are readable, so the replay did not replace the first.
    for (case, task_id) in [("the first task", &first_id), ("the second task", &other_id)] {
        assert_eq!(
            get(&emulator, &id, task_id).await.status,
            200,
            "case {case:?}: both started tasks must remain readable"
        );
    }
}
