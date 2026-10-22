// SPDX-License-Identifier: CC0-1.0

//! `RestartChannelPipelines`, end to end over the restJson1 wire.
//!
//! The suites are tables: the recorded case in the issue (a running channel whose
//! pipelines restart into `RECOVERING`) is ONE row of the first one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe, wait_for_the_clock};
use crate::wire::{call, error_code, medialive};

/// One row of the restart table: a named struct rather than a tuple, so the rows
/// stay readable (and the type stays simple) as the table grows.
struct Row {
    case: &'static str,
    /// The `ChannelClass` the channel is created with.
    class: &'static str,
    /// The `pipelineIds` member the restart request carries, if any.
    pipeline_ids: Option<Value>,
    /// The `PipelineId`s expected to stay healthy while the channel recovers.
    recovering: &'static [&'static str],
    /// The `PipelineId`s expected to be healthy once the restart completes.
    restarted: &'static [&'static str],
}

/// Create a channel of `class` and start it, returning the id of a channel that
/// is `RUNNING`.
async fn running_channel(router: &MultiServiceRouter, case: &str, class: &str) -> String {
    let request = json!({ "name": format!("restart-{case}"), "channelClass": class });
    let (status, created) = call(router, "POST", "/prod/channels", Some(request)).await;
    assert_eq!(status, 201, "case {case:?}: CreateChannel: got {status}: {created}");
    let id = created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    wait_for_the_clock();
    let (status, started) = call(router, "POST", &format!("/prod/channels/{id}/start"), None).await;
    assert_eq!(status, 200, "case {case:?}: StartChannel: got {status}: {started}");
    wait_for_the_clock();

    let (status, running) = describe(router, &id).await;
    assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {running}");
    assert_eq!(
        running["state"],
        json!("RUNNING"),
        "case {case:?}: the test did not reach RUNNING, it reached {:?}",
        running["state"]
    );
    id
}

async fn restart(router: &MultiServiceRouter, id: &str, pipeline_ids: Option<Value>) -> (u16, Value) {
    let body = pipeline_ids.map(|ids| json!({ "pipelineIds": ids }));
    call(
        router,
        "POST",
        &format!("/prod/channels/{id}/restartChannelPipelines"),
        body,
    )
    .await
}

/// The `PipelineId`s a channel description reports as healthy, in order.
fn reported_pipelines(channel: &Value) -> Vec<String> {
    channel["pipelineDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| detail["pipelineId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Rule: restarting the pipelines of a running channel moves it to `RECOVERING`,
/// leaves exactly the pipelines it did NOT take down healthy
/// (`PipelinesRunningCount` is "the number of currently healthy pipelines", and
/// `PipelineDetails` carries one entry per healthy pipeline), and the restart
/// completes into `RUNNING` with every pipeline of the channel's class healthy
/// again.
///
/// The rows differ along which pipelines the request names and how many the
/// channel's class has, which is what the rule generalises over: the whole
/// `ChannelPipelineIdToRestart` enum (`smithy_query(query="shape",
/// shape="ChannelPipelineIdToRestart")`: PIPELINE_0 | PIPELINE_1), the absent and
/// empty lists, a repeated entry, and both `ChannelClass` values.
#[tokio::test]
async fn restarting_pipelines_recovers_only_the_pipelines_it_took_down() {
    // (case, channel class, pipelineIds sent, pipelines healthy while RECOVERING,
    //  pipelines healthy once the restart completes)
    let table = [
        Row {
            case: "standard, all pipelines",
            class: "STANDARD",
            pipeline_ids: None,
            recovering: &[],
            restarted: &["0", "1"],
        },
        Row {
            case: "standard, empty list",
            class: "STANDARD",
            pipeline_ids: Some(json!([])),
            recovering: &[],
            restarted: &["0", "1"],
        },
        Row {
            case: "standard, first pipeline",
            class: "STANDARD",
            pipeline_ids: Some(json!(["PIPELINE_0"])),
            recovering: &["1"],
            restarted: &["0", "1"],
        },
        Row {
            case: "standard, second pipeline",
            class: "STANDARD",
            pipeline_ids: Some(json!(["PIPELINE_1"])),
            recovering: &["0"],
            restarted: &["0", "1"],
        },
        Row {
            case: "standard, both pipelines",
            class: "STANDARD",
            pipeline_ids: Some(json!(["PIPELINE_0", "PIPELINE_1"])),
            recovering: &[],
            restarted: &["0", "1"],
        },
        Row {
            case: "standard, one pipeline named twice",
            class: "STANDARD",
            pipeline_ids: Some(json!(["PIPELINE_1", "PIPELINE_1"])),
            recovering: &["0"],
            restarted: &["0", "1"],
        },
        Row {
            case: "single pipeline channel",
            class: "SINGLE_PIPELINE",
            pipeline_ids: Some(json!(["PIPELINE_0"])),
            recovering: &[],
            restarted: &["0"],
        },
    ];

    for row in table {
        let Row {
            case,
            class,
            pipeline_ids,
            recovering: recovering_pipelines,
            restarted: restarted_pipelines,
        } = row;
        let router = medialive();
        let id = running_channel(&router, case, class).await;

        let (status, restarting) = restart(&router, &id, pipeline_ids).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RestartChannelPipelines: got {status}: {restarting}"
        );
        assert_eq!(
            restarting["state"],
            json!("RECOVERING"),
            "case {case:?}: expected a restarted channel to be RECOVERING, got {:?}",
            restarting["state"]
        );
        assert_eq!(
            restarting["id"],
            json!(id),
            "case {case:?}: expected the restarted channel's own id, got {:?}",
            restarting["id"]
        );
        assert_eq!(
            reported_pipelines(&restarting),
            recovering_pipelines,
            "case {case:?}: expected pipelines {recovering_pipelines:?} to stay healthy, got {:?}",
            restarting["pipelineDetails"]
        );
        assert_eq!(
            restarting["pipelinesRunningCount"],
            json!(recovering_pipelines.len()),
            "case {case:?}: expected {} healthy pipeline(s) while RECOVERING, got {:?}",
            recovering_pipelines.len(),
            restarting["pipelinesRunningCount"]
        );

        // The reader sees the same recovering channel: a describe neither hurries
        // the restart along nor reports a state the restart call did not set.
        let (status, recovering) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {recovering}");
        assert_eq!(
            recovering["state"],
            json!("RECOVERING"),
            "case {case:?}: expected the describe to report RECOVERING too, got {:?}",
            recovering["state"]
        );
        assert_eq!(
            reported_pipelines(&recovering),
            recovering_pipelines,
            "case {case:?}: expected the describe to report pipelines {recovering_pipelines:?}, got {:?}",
            recovering["pipelineDetails"]
        );

        wait_for_the_clock();
        let (status, restarted) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {restarted}");
        assert_eq!(
            restarted["state"],
            json!("RUNNING"),
            "case {case:?}: expected a completed restart to leave the channel RUNNING, got {:?}",
            restarted["state"]
        );
        assert_eq!(
            reported_pipelines(&restarted),
            restarted_pipelines,
            "case {case:?}: expected every pipeline healthy again ({restarted_pipelines:?}), got {:?}",
            restarted["pipelineDetails"]
        );
        assert_eq!(
            restarted["pipelinesRunningCount"],
            json!(restarted_pipelines.len()),
            "case {case:?}: expected {} healthy pipeline(s) once restarted, got {:?}",
            restarted_pipelines.len(),
            restarted["pipelinesRunningCount"]
        );

        // The channel the restart answered with is the stored channel, not the
        // request: the members the create supplied are still there.
        assert_eq!(
            restarting["name"],
            json!(format!("restart-{case}")),
            "case {case:?}: expected the stored name, got {:?}",
            restarting["name"]
        );
        assert_eq!(
            restarting["channelClass"],
            restarted["channelClass"],
            "case {case:?}: the class changed across the restart: {:?} then {:?}",
            restarting["channelClass"],
            restarted["channelClass"]
        );
    }
}

/// Rule: a restart of a channel that is not currently running is refused with the
/// operation's `ConflictException` (409), one of a channel that does not exist
/// with its `NotFoundException` (404), and neither leaves the channel recovering.
///
/// The rows span every non-running state this slice can put a channel in.
#[tokio::test]
async fn restarting_pipelines_is_refused_unless_the_channel_is_running() {
    let router = medialive();
    let (status, body) = restart(&router, "0000000", None).await;
    assert_eq!(status, 404, "case unknown channel: expected 404, got {status}: {body}");
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "case unknown channel: expected NotFoundException, got {body}"
    );

    // (case, the state the channel is in when the restart is attempted)
    let states: [(&str, &str); 4] = [
        ("while CREATING", "CREATING"),
        ("while IDLE", "IDLE"),
        ("while STARTING", "STARTING"),
        ("while STOPPING", "STOPPING"),
    ];

    for (case, state) in states {
        let router = medialive();
        let (status, created) = call(
            &router,
            "POST",
            "/prod/channels",
            Some(json!({ "name": format!("restart-{case}") })),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: CreateChannel: got {status}: {created}");
        let id = created["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if state != "CREATING" {
            wait_for_the_clock();
        }
        if state == "STARTING" || state == "STOPPING" {
            let (status, started) =
                call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
            assert_eq!(status, 200, "case {case:?}: StartChannel: got {status}: {started}");
        }
        if state == "STOPPING" {
            wait_for_the_clock();
            let (status, stopped) =
                call(&router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
            assert_eq!(status, 200, "case {case:?}: StopChannel: got {status}: {stopped}");
        }

        let (status, observed) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {observed}");
        assert_eq!(
            observed["state"],
            json!(state),
            "case {case:?}: the test did not reach {state}, it reached {:?}",
            observed["state"]
        );

        let (status, body) = restart(&router, &id, None).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );

        let (status, after) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {after}");
        assert_ne!(
            after["state"],
            json!("RECOVERING"),
            "case {case:?}: a refused restart must not have moved the channel, got {:?}",
            after["state"]
        );
    }
}

/// Rule: a `PipelineIds` entry that does not name a pipeline the channel's class
/// gives it is refused with a 400 rather than restarting something else, and the
/// channel keeps running.
///
/// `PIPELINE_1` is a modelled `ChannelPipelineIdToRestart` value, so a
/// single-pipeline channel refuses it with the operation's own
/// `BadRequestException`; `PIPELINE_2` is not a modelled value at all, so the
/// boundary refuses it before the handler runs — both are 400s, which is what the
/// rule asserts.
#[tokio::test]
async fn a_pipeline_the_channel_does_not_have_is_refused() {
    // (case, channel class, pipelineIds sent, expected error code if the handler owns it)
    let table: [(&str, &str, Value, Option<&str>); 3] = [
        (
            "PIPELINE_1 on a single pipeline channel",
            "SINGLE_PIPELINE",
            json!(["PIPELINE_1"]),
            Some("BadRequestException"),
        ),
        (
            "PIPELINE_0 and PIPELINE_1 on a single pipeline channel",
            "SINGLE_PIPELINE",
            json!(["PIPELINE_0", "PIPELINE_1"]),
            Some("BadRequestException"),
        ),
        (
            "a value the enum does not model",
            "STANDARD",
            json!(["PIPELINE_2"]),
            None,
        ),
    ];

    for (case, class, pipeline_ids, expected_code) in table {
        let router = medialive();
        let id = running_channel(&router, case, class).await;

        let (status, body) = restart(&router, &id, Some(pipeline_ids)).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        if let Some(code) = expected_code {
            assert_eq!(
                error_code(status, &body),
                code,
                "case {case:?}: expected {code}, got {body}"
            );
        }

        let (status, after) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {after}");
        assert_eq!(
            after["state"],
            json!("RUNNING"),
            "case {case:?}: a refused restart must leave the channel RUNNING, got {:?}",
            after["state"]
        );
        assert_eq!(
            after["pipelinesRunningCount"],
            json!(if class == "SINGLE_PIPELINE" { 1 } else { 2 }),
            "case {case:?}: a refused restart must leave every pipeline healthy, got {:?}",
            after["pipelinesRunningCount"]
        );
    }
}
