// SPDX-License-Identifier: CC0-1.0

//! `DescribeThumbnails`, end to end over the restJson1 wire.
//!
//! The rules each suite below asserts, each over a table that spans its class
//! rather than one recorded request:
//!
//! 1. `DescribeThumbnails` reports the LATEST frame — latest by capture time —
//!    that the named pipeline of the named channel captured of the requested
//!    `ThumbnailType`, as one `ThumbnailDetail` carrying that frame's stored
//!    body, content type, kind and capture instant.
//! 2. A channel that has captured no frame the read can report answers an EMPTY
//!    `ThumbnailDetails` list, and a frame is reported only while the pipeline it
//!    came from is up and the channel's thumbnail feature is on.
//! 3. A `ChannelId` naming no stored channel is a 404 `NotFoundException`, and a
//!    request this operation cannot read (a `PipelineId` that is not one of the
//!    channel's pipeline numbers, a `ThumbnailType` outside the modelled enum) is
//!    a 400 `BadRequestException`.
//! 4. The capture log takes a frame only where one could really have been
//!    captured, and is bounded: the oldest frame is dropped once it is full.
//!
//! NOTHING in this emulator captures a thumbnail frame — the real service
//! generates them in the encoder pipelines it runs for a channel
//! (`ThumbnailConfiguration$State`: "The feature generates thumbnails of the
//! incoming video in each pipeline in the channel"), and this emulator encodes no
//! media — so a channel created through the API reports NO thumbnails (asserted
//! below), and the suites that exercise the reporting seed the log through the
//! store's own one writer, `capture_channel_thumbnail`, instead of asking an
//! operation to invent image data.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    self, ThumbnailRecord, MAX_CHANNEL_THUMBNAILS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::support::{epoch_seconds, timestamp};
use crate::wire::{call, error_code, medialive_with_engine};

/// The modelled `ThumbnailType` values (`smithy_query(query="shape",
/// shape="ThumbnailType")`: UNSPECIFIED | CURRENT_ACTIVE) — the whole set of
/// kinds a capture can be of and a request can ask for.
const KINDS: [&str; 2] = ["UNSPECIFIED", "CURRENT_ACTIVE"];

/// The content type a capturer reports for a frame, as the test's capturer
/// reports it; the reader echoes the STORED value rather than choosing one.
const CONTENT_TYPE: &str = "image/jpeg";

/// An instant a fixed number of seconds after the Unix epoch, so an asserted
/// capture instant names an instant the test chose rather than one it has to
/// guess.
fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// One frame of `kind`, captured from `pipeline` `secs` after the Unix epoch,
/// carrying the image data and content type its capturer reported.
fn frame(pipeline: i32, kind: &str, secs: u64, body: &str) -> ThumbnailRecord {
    let mut record = ThumbnailRecord::captured(pipeline, kind, at(secs));
    record.body = Some(body.to_string());
    record.content_type = Some(CONTENT_TYPE.to_string());
    record
}

/// The `EncoderSettings` a channel is created with: the four members the shape
/// requires (`smithy_query(query="shape", shape="EncoderSettings")`:
/// AudioDescriptions, OutputGroups, TimecodeConfig, VideoDescriptions) plus the
/// `ThumbnailConfiguration` whose `State` turns the thumbnail feature on or off
/// ("AUTO turns the feature on, DISABLE turns the feature off"), or no
/// configuration at all when `thumbnail_state` is `None`.
fn encoder_settings(thumbnail_state: Option<&str>) -> Value {
    let mut settings = json!({
        "audioDescriptions": [],
        "outputGroups": [],
        "timecodeConfig": { "source": "EMBEDDED" },
        "videoDescriptions": [],
    });
    if let Some(state) = thumbnail_state {
        settings["thumbnailConfiguration"] = json!({ "state": state });
    }
    settings
}

/// One channel created with `encoder_settings`, started and left `RUNNING` when
/// `running`, answering its id.
///
/// The describe at the end is what pins the state the rest of a case runs
/// against, exactly as the pipeline-restart suite's own setup does.
async fn channel(
    router: &MultiServiceRouter,
    case: &str,
    class: &str,
    settings: Option<Value>,
    running: bool,
) -> String {
    let mut request = json!({ "name": format!("thumbnails-{case}"), "channelClass": class });
    if let Some(settings) = settings {
        request["encoderSettings"] = settings;
    }
    let (status, created) = call(router, "POST", "/prod/channels", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: got {status}: {created}"
    );
    let id = created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    wait_for_the_clock();
    if running {
        let (status, started) =
            call(router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartChannel: got {status}: {started}"
        );
        wait_for_the_clock();
    }

    let expected_state = if running { "RUNNING" } else { "IDLE" };
    let (status, channel) = describe(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel: got {status}: {channel}"
    );
    assert_eq!(
        channel["state"],
        json!(expected_state),
        "case {case:?}: the test did not reach {expected_state}, it reached {:?}",
        channel["state"]
    );
    id
}

/// `DescribeThumbnails` on `channel_id`, with `query` as its query string.
async fn describe_thumbnails(
    router: &MultiServiceRouter,
    channel_id: &str,
    query: &str,
) -> (u16, Value) {
    let uri = format!("/prod/channels/{channel_id}/thumbnails{query}");
    call(router, "GET", &uri, None).await
}

/// The same read, for the usual case of naming one pipeline and one kind.
async fn read(
    router: &MultiServiceRouter,
    channel_id: &str,
    pipeline: i32,
    kind: &str,
) -> (u16, Value) {
    describe_thumbnails(
        router,
        channel_id,
        &format!("?pipelineId={pipeline}&thumbnailType={kind}"),
    )
    .await
}

/// Seed `frames` into the capture log of `channel_id`, in the order given.
///
/// The store's own writer, so the log the operation reads is the log something
/// captured into: there is no second copy for the two to disagree about.
fn capture(
    engine: &engine::InMemoryEngine,
    channel_id: &str,
    frames: impl IntoIterator<Item = ThumbnailRecord>,
) {
    for taken in try_capture(engine, channel_id, frames) {
        assert!(
            taken,
            "setup: expected channel {channel_id} to take the captured frame"
        );
    }
}

/// The same write, answering whether the store took each frame instead of
/// asserting it — which is what the writer's own refusals are asserted through.
fn try_capture(
    engine: &engine::InMemoryEngine,
    channel_id: &str,
    frames: impl IntoIterator<Item = ThumbnailRecord>,
) -> Vec<bool> {
    let store = state::service_state(engine).expect("the medialive store");
    let mut store = store.lock();
    frames
        .into_iter()
        .map(|frame| store.capture_channel_thumbnail(channel_id, frame))
        .collect()
}

/// Every frame a channel of `class` could have captured, two per pipeline and
/// kind: an older one and a newer one.
///
/// The NEWER frames are seeded FIRST, so the latest frame of every pipeline and
/// kind is never the last one pushed — which is what makes "latest" mean latest
/// by capture time rather than last recorded.
fn two_frames_per_pipeline_and_kind(pipelines: i32) -> Vec<ThumbnailRecord> {
    let mut frames = Vec::new();
    for secs in [NEWER_AT, OLDER_AT] {
        for pipeline in 0..pipelines {
            for kind in KINDS {
                frames.push(frame(pipeline, kind, secs, &body(pipeline, kind, secs)));
            }
        }
    }
    frames
}

/// The capture instants the seeded frames carry.
const OLDER_AT: u64 = 1_700_000_000;
const NEWER_AT: u64 = 1_700_000_900;

/// The image data a seeded frame carries, distinct per pipeline, kind and
/// instant, so an assertion can tell exactly WHICH frame was reported.
fn body(pipeline: i32, kind: &str, secs: u64) -> String {
    format!("frame-{pipeline}-{kind}-{secs}")
}

/// The one `ThumbnailDetail` a response carries, with the detail count asserted.
fn single_detail(case: &str, body: &Value) -> Value {
    let details = body["thumbnailDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        details.len(),
        1,
        "case {case:?}: expected exactly one thumbnailDetail, got {:?}",
        body["thumbnailDetails"]
    );
    details[0].clone()
}

/// The one `Thumbnail` a detail carries, with the thumbnail count asserted.
fn single_thumbnail(case: &str, detail: &Value) -> Value {
    let thumbnails = detail["thumbnails"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        thumbnails.len(),
        1,
        "case {case:?}: expected exactly one thumbnail, got {:?}",
        detail["thumbnails"]
    );
    thumbnails[0].clone()
}

/// Assert that a response reports no thumbnail at all: the member is the EMPTY
/// list, not an absent one.
fn assert_no_thumbnails(case: &str, status: u16, body: &Value) {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert_eq!(
        body["thumbnailDetails"],
        json!([]),
        "case {case:?}: expected an empty thumbnailDetails list, got {:?}",
        body["thumbnailDetails"]
    );
}

/// One row of the reporting table.
struct Row {
    case: &'static str,
    /// The `ChannelClass` the channel is created with.
    class: &'static str,
    /// The `PipelineId` the read names.
    pipeline: i32,
    /// The `ThumbnailType` the read asks for.
    kind: &'static str,
}

/// Rule: `DescribeThumbnails` reports the latest frame — latest by CAPTURE TIME
/// — that the named pipeline captured of the named kind, as one
/// `ThumbnailDetail` whose `PipelineId` is the pipeline the frame came from and
/// whose single `Thumbnail` carries that frame's stored body, content type, kind
/// and capture instant; frames of the other pipelines and the other kinds are
/// not reported.
///
/// The rows differ along what the rule generalises over: which pipeline is read,
/// which `ThumbnailType` is asked for (the whole modelled enum) and how many
/// pipelines the channel's class has.
#[tokio::test]
async fn the_latest_captured_frame_of_the_named_pipeline_and_kind_is_reported() {
    let table = [
        Row {
            case: "standard, pipeline 0, current active",
            class: "STANDARD",
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
        },
        Row {
            case: "standard, pipeline 0, unspecified",
            class: "STANDARD",
            pipeline: 0,
            kind: "UNSPECIFIED",
        },
        Row {
            case: "standard, pipeline 1, current active",
            class: "STANDARD",
            pipeline: 1,
            kind: "CURRENT_ACTIVE",
        },
        Row {
            case: "standard, pipeline 1, unspecified",
            class: "STANDARD",
            pipeline: 1,
            kind: "UNSPECIFIED",
        },
        Row {
            case: "single pipeline channel, current active",
            class: "SINGLE_PIPELINE",
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
        },
    ];

    for Row {
        case,
        class,
        pipeline,
        kind,
    } in table
    {
        let (router, engine) = medialive_with_engine();
        let id = channel(
            &router,
            case,
            class,
            Some(encoder_settings(Some("AUTO"))),
            true,
        )
        .await;
        let pipelines = if class == "SINGLE_PIPELINE" { 1 } else { 2 };
        capture(&engine, &id, two_frames_per_pipeline_and_kind(pipelines));

        let (status, reported) = read(&router, &id, pipeline, kind).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reported}");

        let detail = single_detail(case, &reported);
        assert_eq!(
            detail["pipelineId"],
            json!(pipeline.to_string()),
            "case {case:?}: expected the detail to name pipeline {pipeline}, got {:?}",
            detail["pipelineId"]
        );

        let thumbnail = single_thumbnail(case, &detail);
        assert_eq!(
            thumbnail["body"],
            json!(body(pipeline, kind, NEWER_AT)),
            "case {case:?}: expected the latest captured frame's body, got {:?}",
            thumbnail["body"]
        );
        assert_eq!(
            thumbnail["contentType"],
            json!(CONTENT_TYPE),
            "case {case:?}: expected the captured content type, got {:?}",
            thumbnail["contentType"]
        );
        assert_eq!(
            thumbnail["thumbnailType"],
            json!(kind),
            "case {case:?}: expected the frame's own kind, got {:?}",
            thumbnail["thumbnailType"]
        );
        let reported_at = epoch_seconds(timestamp(case, "timeStamp", &thumbnail));
        assert!(
            (reported_at - NEWER_AT as f64).abs() < 1.0,
            "case {case:?}: expected the latest capture instant {NEWER_AT}, got {reported_at}"
        );

        // A read changes nothing a later read sees: the same frame is still the
        // latest one, and reporting it did not consume it.
        let (status, again) = read(&router, &id, pipeline, kind).await;
        assert_eq!(status, 200, "case {case:?}: second read: got {status}: {again}");
        assert_eq!(
            again, reported,
            "case {case:?}: a read changed what the next read sees: {reported} then {again}"
        );
    }
}

/// One row of the nothing-to-report table.
struct EmptyRow {
    case: &'static str,
    class: &'static str,
    /// The `ThumbnailConfiguration$State` the channel is created with, or `None`
    /// for a channel whose encoder settings carry no configuration; the whole
    /// row's `encoderSettings` is absent when this is `None` and `settings` is
    /// false.
    thumbnail_state: Option<&'static str>,
    /// Whether the create carries `EncoderSettings` at all.
    settings: bool,
    /// Whether the channel is left `RUNNING`.
    running: bool,
}

/// Rule: a channel with no frame the read can report answers an EMPTY
/// `ThumbnailDetails` list and a 200 — never an error, and never an invented
/// thumbnail. Which is every channel created through this emulator's API, because
/// nothing in it captures a frame.
///
/// The rows span what could make a channel have nothing to report: the feature
/// on but nothing captured, the feature `DISABLED`, no `ThumbnailConfiguration`,
/// no `EncoderSettings` at all, a channel that never started, and both
/// `ChannelClass` values.
#[tokio::test]
async fn a_channel_that_captured_nothing_reports_an_empty_list() {
    let table = [
        EmptyRow {
            case: "running, feature on, nothing captured",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            settings: true,
            running: true,
        },
        EmptyRow {
            case: "running, feature disabled",
            class: "STANDARD",
            thumbnail_state: Some("DISABLED"),
            settings: true,
            running: true,
        },
        EmptyRow {
            case: "running, no thumbnail configuration",
            class: "STANDARD",
            thumbnail_state: None,
            settings: true,
            running: true,
        },
        EmptyRow {
            case: "running, no encoder settings",
            class: "STANDARD",
            thumbnail_state: None,
            settings: false,
            running: true,
        },
        EmptyRow {
            case: "idle, feature on",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            settings: true,
            running: false,
        },
        EmptyRow {
            case: "single pipeline channel, feature on",
            class: "SINGLE_PIPELINE",
            thumbnail_state: Some("AUTO"),
            settings: true,
            running: true,
        },
    ];

    for EmptyRow {
        case,
        class,
        thumbnail_state,
        settings,
        running,
    } in table
    {
        let (router, _engine) = medialive_with_engine();
        let settings = settings.then(|| encoder_settings(thumbnail_state));
        let id = channel(&router, case, class, settings, running).await;

        for kind in KINDS {
            let (status, reported) = read(&router, &id, 0, kind).await;
            assert_no_thumbnails(&format!("{case}, {kind}"), status, &reported);
        }
    }
}

/// Rule: a captured frame is reported only while the pipeline it came from is UP
/// — a `ThumbnailDetail` is "Thumbnail details for one pipeline of a running
/// channel" — so a channel that has been stopped reports nothing, and a pipeline
/// a restart has taken down reports nothing while the pipelines still up go on
/// reporting.
#[tokio::test]
async fn a_frame_is_reported_only_while_its_pipeline_is_up() {
    let kind = "CURRENT_ACTIVE";

    // Stopping the channel takes every pipeline down, so neither pipeline has a
    // frame to report — in `STOPPING` and in the `IDLE` the clock settles it to.
    let case = "stopped channel";
    let (router, engine) = medialive_with_engine();
    let id = channel(
        &router,
        case,
        "STANDARD",
        Some(encoder_settings(Some("AUTO"))),
        true,
    )
    .await;
    capture(&engine, &id, two_frames_per_pipeline_and_kind(2));
    for pipeline in 0..2 {
        let (status, reported) = read(&router, &id, pipeline, kind).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reported}");
        single_thumbnail(case, &single_detail(case, &reported));
    }

    let (status, stopped) = call(&router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
    assert_eq!(status, 200, "case {case:?}: StopChannel: got {status}: {stopped}");
    for pipeline in 0..2 {
        let (status, reported) = read(&router, &id, pipeline, kind).await;
        assert_no_thumbnails(&format!("{case}, stopping, pipeline {pipeline}"), status, &reported);
    }
    wait_for_the_clock();
    for pipeline in 0..2 {
        let (status, reported) = read(&router, &id, pipeline, kind).await;
        assert_no_thumbnails(&format!("{case}, idle, pipeline {pipeline}"), status, &reported);
    }

    // A restart takes ONE pipeline down: that pipeline reports nothing while it
    // is down, the other goes on reporting its latest frame, and once the restart
    // completes both pipelines are up again.
    let case = "restarted pipeline";
    let (router, engine) = medialive_with_engine();
    let id = channel(
        &router,
        case,
        "STANDARD",
        Some(encoder_settings(Some("AUTO"))),
        true,
    )
    .await;
    capture(&engine, &id, two_frames_per_pipeline_and_kind(2));

    let (status, restarting) = call(
        &router,
        "POST",
        &format!("/prod/channels/{id}/restartChannelPipelines"),
        Some(json!({ "pipelineIds": ["PIPELINE_0"] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RestartChannelPipelines: got {status}: {restarting}"
    );

    let (status, down) = read(&router, &id, 0, kind).await;
    assert_no_thumbnails(&format!("{case}, the pipeline that is down"), status, &down);

    let (status, up) = read(&router, &id, 1, kind).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {up}");
    let thumbnail = single_thumbnail(case, &single_detail(case, &up));
    assert_eq!(
        thumbnail["body"],
        json!(body(1, kind, NEWER_AT)),
        "case {case:?}: expected the pipeline still up to report its latest frame, got {:?}",
        thumbnail["body"]
    );

    wait_for_the_clock();
    for pipeline in 0..2 {
        let (status, reported) = read(&router, &id, pipeline, kind).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {reported}"
        );
        let thumbnail = single_thumbnail(case, &single_detail(case, &reported));
        assert_eq!(
            thumbnail["body"],
            json!(body(pipeline, kind, NEWER_AT)),
            "case {case:?}: expected pipeline {pipeline} to report again once the restart completed, got {:?}",
            thumbnail["body"]
        );
    }
}

/// One row of the refusal table.
struct RefusalRow {
    case: &'static str,
    /// Whether the read is aimed at the single-pipeline channel rather than the
    /// standard one.
    single_pipeline: bool,
    /// The query string the read carries.
    query: &'static str,
    /// The error code the refusal carries, when this operation owns the refusal.
    ///
    /// Every row here is one: `PipelineId` and `ThumbnailType` are `@required`
    /// query parameters, so a request that omits one names no route at all and
    /// never reaches this operation — which is the framework's routing, not this
    /// operation's rule, so no row asserts it.
    code: Option<&'static str>,
}

/// Rule: a `ChannelId` naming no stored channel is this operation's 404
/// `NotFoundException`; a `PipelineId` that is not the decimal spelling of one of
/// the channel's own pipelines (`PipelineId` is documented `Pipeline ID ("0" or
/// "1")`, and the class fixes which of those the channel has) and a
/// `ThumbnailType` outside the modelled enum are its 400 `BadRequestException`;
/// and a refused read leaves what a later read reports untouched.
#[tokio::test]
async fn a_request_this_operation_cannot_read_is_refused() {
    let (router, engine) = medialive_with_engine();

    let (status, missing) = read(&router, "0000000", 0, "CURRENT_ACTIVE").await;
    assert_eq!(
        status, 404,
        "case unknown channel: expected 404, got {status}: {missing}"
    );
    assert_eq!(
        error_code(status, &missing),
        "NotFoundException",
        "case unknown channel: expected NotFoundException, got {missing}"
    );

    let standard = channel(
        &router,
        "refusals, standard",
        "STANDARD",
        Some(encoder_settings(Some("AUTO"))),
        true,
    )
    .await;
    capture(&engine, &standard, two_frames_per_pipeline_and_kind(2));
    let single = channel(
        &router,
        "refusals, single pipeline",
        "SINGLE_PIPELINE",
        Some(encoder_settings(Some("AUTO"))),
        true,
    )
    .await;
    capture(&engine, &single, two_frames_per_pipeline_and_kind(1));

    let table = [
        RefusalRow {
            case: "pipeline 1 of a single pipeline channel",
            single_pipeline: true,
            query: "?pipelineId=1&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a pipeline past the standard class's two",
            single_pipeline: false,
            query: "?pipelineId=2&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a negative pipeline",
            single_pipeline: false,
            query: "?pipelineId=-1&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a pipeline that is not a number",
            single_pipeline: false,
            query: "?pipelineId=PIPELINE_0&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a zero-padded spelling of a pipeline the channel has",
            single_pipeline: false,
            query: "?pipelineId=01&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a signed spelling of a pipeline the channel has",
            single_pipeline: false,
            query: "?pipelineId=%2B1&thumbnailType=CURRENT_ACTIVE",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a thumbnail kind the enum does not model",
            single_pipeline: false,
            query: "?pipelineId=0&thumbnailType=CURRENT",
            code: Some("BadRequestException"),
        },
        RefusalRow {
            case: "a thumbnail kind in the wrong case",
            single_pipeline: false,
            query: "?pipelineId=0&thumbnailType=current_active",
            code: Some("BadRequestException"),
        },
    ];

    for RefusalRow {
        case,
        single_pipeline,
        query,
        code,
    } in table
    {
        let id = if single_pipeline { &single } else { &standard };
        let (status, refused) = describe_thumbnails(&router, id, query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {refused}");
        if let Some(code) = code {
            assert_eq!(
                error_code(status, &refused),
                code,
                "case {case:?}: expected {code}, got {refused}"
            );
        }

        // The refusal reported nothing and changed nothing: the frame the
        // channel did capture is still the one a readable request reports.
        let (status, reported) = read(&router, id, 0, "CURRENT_ACTIVE").await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reported}");
        let thumbnail = single_thumbnail(case, &single_detail(case, &reported));
        assert_eq!(
            thumbnail["body"],
            json!(body(0, "CURRENT_ACTIVE", NEWER_AT)),
            "case {case:?}: a refused read changed what a later read reports, got {:?}",
            thumbnail["body"]
        );
    }
}

/// One row of the capture-refusal table.
struct CaptureRow {
    case: &'static str,
    class: &'static str,
    thumbnail_state: Option<&'static str>,
    running: bool,
    /// The pipeline the frame claims to have come from.
    pipeline: i32,
    /// The kind the frame claims to be.
    kind: &'static str,
    /// Whether the store should take the frame.
    taken: bool,
}

/// Rule: the capture log takes a frame only where one could really have been
/// captured — the channel exists, its thumbnail feature is on, the kind is a
/// modelled `ThumbnailType` and the pipeline is one of the channel's pipelines
/// that is up — and `DescribeThumbnails` then reports exactly the frames it took.
///
/// The rows span each condition on its own, so no single refusal can stand in for
/// another.
#[tokio::test]
async fn the_capture_log_takes_only_a_frame_something_could_have_captured() {
    let table = [
        CaptureRow {
            case: "running, feature on, pipeline 0",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            running: true,
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
            taken: true,
        },
        CaptureRow {
            case: "running, feature on, pipeline 1",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            running: true,
            pipeline: 1,
            kind: "UNSPECIFIED",
            taken: true,
        },
        CaptureRow {
            case: "pipeline 1 of a single pipeline channel",
            class: "SINGLE_PIPELINE",
            thumbnail_state: Some("AUTO"),
            running: true,
            pipeline: 1,
            kind: "CURRENT_ACTIVE",
            taken: false,
        },
        CaptureRow {
            case: "a pipeline past the standard class's two",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            running: true,
            pipeline: 2,
            kind: "CURRENT_ACTIVE",
            taken: false,
        },
        CaptureRow {
            case: "feature disabled",
            class: "STANDARD",
            thumbnail_state: Some("DISABLED"),
            running: true,
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
            taken: false,
        },
        CaptureRow {
            case: "no thumbnail configuration",
            class: "STANDARD",
            thumbnail_state: None,
            running: true,
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
            taken: false,
        },
        CaptureRow {
            case: "a channel that is not running",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            running: false,
            pipeline: 0,
            kind: "CURRENT_ACTIVE",
            taken: false,
        },
        CaptureRow {
            case: "a kind the enum does not model",
            class: "STANDARD",
            thumbnail_state: Some("AUTO"),
            running: true,
            pipeline: 0,
            kind: "CURRENT",
            taken: false,
        },
    ];

    for CaptureRow {
        case,
        class,
        thumbnail_state,
        running,
        pipeline,
        kind,
        taken: expected,
    } in table
    {
        let (router, engine) = medialive_with_engine();
        let id = channel(
            &router,
            case,
            class,
            Some(encoder_settings(thumbnail_state)),
            running,
        )
        .await;

        let captured = try_capture(
            &engine,
            &id,
            [frame(pipeline, kind, NEWER_AT, &body(pipeline, kind, NEWER_AT))],
        );
        assert_eq!(
            captured,
            vec![expected],
            "case {case:?}: expected the store to take the frame: {expected}, got {captured:?}"
        );

        let (status, reported) = read(&router, &id, pipeline, kind).await;
        if expected {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reported}");
            let thumbnail = single_thumbnail(case, &single_detail(case, &reported));
            assert_eq!(
                thumbnail["body"],
                json!(body(pipeline, kind, NEWER_AT)),
                "case {case:?}: expected the captured frame to be reported, got {:?}",
                thumbnail["body"]
            );
        } else if status == 200 {
            assert_no_thumbnails(case, status, &reported);
        } else {
            // The same frame the store refused also names a pipeline or a kind
            // no readable request can ask for, so the read is a 400.
            assert_eq!(status, 400, "case {case:?}: expected 200 or 400, got {status}: {reported}");
        }
    }

    // An unknown channel takes nothing at all.
    let (_router, engine) = medialive_with_engine();
    let captured = try_capture(
        &engine,
        "0000000",
        [frame(0, "CURRENT_ACTIVE", NEWER_AT, "frame")],
    );
    assert_eq!(
        captured,
        vec![false],
        "case unknown channel: expected the store to take nothing, got {captured:?}"
    );
}

/// Rule: the capture log is bounded — a channel keeps at most
/// `MAX_CHANNEL_THUMBNAILS` frames and drops the OLDEST first — so the newest
/// frame of a pipeline is still reported after the log has overflowed, while a
/// frame the overflow evicted is not.
#[tokio::test]
async fn the_capture_log_is_bounded_and_keeps_the_newest_frames() {
    let case = "an overflowing capture log";
    let kind = "CURRENT_ACTIVE";
    let (router, engine) = medialive_with_engine();
    let id = channel(
        &router,
        case,
        "STANDARD",
        Some(encoder_settings(Some("AUTO"))),
        true,
    )
    .await;

    // The first frame in, from the OTHER pipeline, so the overflow's eviction can
    // be observed as that pipeline losing its only frame.
    capture(&engine, &id, [frame(1, kind, OLDER_AT, "the evicted frame")]);
    let newest = MAX_CHANNEL_THUMBNAILS as u64;
    capture(
        &engine,
        &id,
        (1..=newest).map(|step| {
            let secs = OLDER_AT + step;
            frame(0, kind, secs, &body(0, kind, secs))
        }),
    );

    let (status, reported) = read(&router, &id, 0, kind).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reported}");
    let thumbnail = single_thumbnail(case, &single_detail(case, &reported));
    let newest_secs = OLDER_AT + newest;
    assert_eq!(
        thumbnail["body"],
        json!(body(0, kind, newest_secs)),
        "case {case:?}: expected the newest frame to survive the bound, got {:?}",
        thumbnail["body"]
    );

    let (status, evicted) = read(&router, &id, 1, kind).await;
    assert_no_thumbnails(&format!("{case}, the evicted pipeline"), status, &evicted);
}
