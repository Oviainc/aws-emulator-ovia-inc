// SPDX-License-Identifier: CC0-1.0

//! Job QUEUEING at the concurrency ceiling, end to end over the wire, for the one job
//! family whose start documents it.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator and the call are the transcription-job suite's harness
//! (`crate::transcription_jobs`), the family's read helper is
//! `crate::call_analytics_jobs`' and the jobs that fill the ceiling are
//! `crate::job_concurrency_quota`'s, all reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: where a start operation's own documentation says the family has
//! job queueing and names the request member that opts into it, a start made while that
//! family's concurrency ceiling's worth of jobs are in flight is ACCEPTED into the queue
//! — readable in the `QUEUED` status its `*JobStatus` enum models, with no start time —
//! when the request carries that member, and is refused with the modelled
//! `LimitExceededException` when it does not; a queued job then leaves the queue for
//! `IN_PROGRESS` and reaches `COMPLETED` on the clock, and below the ceiling the member
//! queues nothing. A waiting job holds no running slot, so the queue has a depth of its
//! own (`crate::state::MAX_QUEUED_CALL_ANALYTICS_JOBS`) and a start past it is refused
//! the same way. For `StartCallAnalyticsJob` the opt-in member is `DataAccessRoleArn`
//! (`smithy_query(query="doc", shape="StartCallAnalyticsJob")`); the sibling family
//! spells the same opt-in `JobExecutionSettings.AllowDeferredExecution`, which
//! `crate::transcription_jobs` already drives.
//!
//! Each test asserts that over a table spanning the class the rule generalises over:
//! whether the opt-in member is present, which ARN partition it names, which `Media`
//! member names the input audio and under which URI scheme, which documented output
//! location form is asked for, and which settings, channel definitions and tags the
//! rest of the request carries.

use std::time::Duration;

use serde_json::{json, Value};

use crate::call_analytics_jobs::get;
use crate::job_concurrency_quota::{call_analytics_request, MEDIA};
use crate::transcription_jobs::Transcribe;

/// Long enough for both clock-owed transitions of a QUEUED job to have fallen due:
/// `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans of
/// `crate::state::JOB_TRANSITION_DELAY` (500 ms each), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// Long enough for the `QUEUED -> IN_PROGRESS` transition alone to have fallen due,
/// and short of the completion that follows it.
const PAST_LEAVING_THE_QUEUE: Duration = Duration::from_millis(600);

/// The ceiling a Call Analytics start is bounded by, read off the service rather than
/// restated here.
fn ceiling() -> usize {
    service_com_amazonaws_transcribe_transcribe::state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS
}

/// One row: a start request made while the family is at its ceiling, and whether it
/// carries the member the operation documents as opting into the queue.
struct Row {
    case: &'static str,
    /// `true` when the request passes a `DataAccessRoleArn`.
    opts_in: bool,
    request: fn(&str) -> Value,
}

/// The class of start a caller can make at the ceiling.
///
/// The role-carrying rows walk the partitions `DataAccessRoleArn`'s own `@pattern`
/// admits — `^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/…$`
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`) — so the
/// opt-in is recognised by the member being present and not by one recorded ARN; the
/// role-less rows span a minimal request and a rich one, so what decides the answer is
/// that member alone and not how much else the request says.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no role, the family's minimal request",
            opts_in: false,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                })
            },
        },
        Row {
            case: "no role, output location, settings, channel definitions and two tags",
            opts_in: false,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/two-channel.ogg"},
                    "OutputLocation": "s3://example-output/analytics/",
                    "Settings": {"LanguageOptions": ["en-US", "es-US"]},
                    "ChannelDefinitions": [
                        {"ChannelId": 0, "ParticipantRole": "AGENT"},
                        {"ChannelId": 1, "ParticipantRole": "CUSTOMER"}
                    ],
                    "Tags": [{"Key": "team", "Value": "support"}, {"Key": "cost", "Value": "7"}]
                })
            },
        },
        Row {
            case: "an aws-partition role, minimal otherwise",
            opts_in: true,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                    "DataAccessRoleArn": "arn:aws:iam::111122223333:role/CallAnalyticsAccess"
                })
            },
        },
        Row {
            case: "an aws-cn-partition role, redacted input audio and an output folder",
            opts_in: true,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"RedactedMediaFileUri": "s3://example-media/call-redacted.wav"},
                    "OutputLocation": "s3://example-output/analytics/",
                    "DataAccessRoleArn": "arn:aws-cn:iam::111122223333:role/Queued"
                })
            },
        },
        Row {
            case: "an aws-us-gov-partition role with a path, https media, content redaction",
            opts_in: true,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "https://example.invalid/media/call.mp4"},
                    "OutputLocation": "s3://example-output/analytics/queued.json",
                    "DataAccessRoleArn": "arn:aws-us-gov:iam::111122223333:role/support/Analytics",
                    // Both members of `ContentRedaction` are required
                    // (`smithy_query(query="shape", shape="ContentRedaction")`).
                    "Settings": {
                        "ContentRedaction": {
                            "RedactionType": "PII",
                            "RedactionOutput": "redacted_and_unredacted"
                        }
                    }
                })
            },
        },
        Row {
            case: "an aws-iso-partition role, a bucket-only output location and one tag",
            opts_in: true,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                    "OutputLocation": "s3://example-output",
                    "DataAccessRoleArn": "arn:aws-iso:iam::111122223333:role/Queued",
                    "Tags": [{"Key": "queued", "Value": "yes"}]
                })
            },
        },
        Row {
            case: "an aws-iso-b-partition role, channel definitions and summarization",
            opts_in: true,
            request: |name| {
                json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/two-channel.ogg"},
                    "DataAccessRoleArn": "arn:aws-iso-b:iam::111122223333:role/Queued",
                    "Settings": {"Summarization": {"GenerateAbstractiveSummary": true}},
                    "ChannelDefinitions": [
                        {"ChannelId": 1, "ParticipantRole": "AGENT"},
                        {"ChannelId": 0, "ParticipantRole": "CUSTOMER"}
                    ]
                })
            },
        },
    ]
}

/// Start the ceiling's worth of Call Analytics jobs, none of which opts into the queue,
/// so the family is exactly at its limit with every slot held by a job in flight.
async fn fill_to_the_ceiling(transcribe: &Transcribe) {
    for index in 0..ceiling() {
        let case = format!("job {index} of the Call Analytics ceiling");
        transcribe
            .call(
                "StartCallAnalyticsJob",
                call_analytics_request(&format!("fill-{index}"), index),
            )
            .await
            .ok_status(&case, 200);
    }
}

/// The `CallAnalyticsJob` projection a read of `name` reports.
async fn job_of(transcribe: &Transcribe, case: &str, name: &str) -> Value {
    get(transcribe, name).await.ok(case)["CallAnalyticsJob"].clone()
}

#[tokio::test]
async fn a_start_at_the_ceiling_is_queued_only_when_it_opts_in() {
    for (index, row) in rows().into_iter().enumerate() {
        // A fresh emulator per row, so each row meets a family that is exactly at its
        // ceiling with nothing queued behind it.
        let transcribe = Transcribe::new();
        fill_to_the_ceiling(&transcribe).await;

        let case = row.case;
        let name = format!("at-the-ceiling-{index}");
        let request = (row.request)(&name);
        let reply = transcribe.call("StartCallAnalyticsJob", request.clone()).await;

        if !row.opts_in {
            reply.refused(case, 400, "LimitExceededException");
            let message = reply.error_message(case);
            assert!(
                message.contains(&ceiling().to_string()),
                "case {case:?}: the refusal must name the quota it exceeded, got {message:?}"
            );
            // A refused start stores nothing: the name it asked for is still unknown.
            get(&transcribe, &name)
                .await
                .refused(case, 400, "BadRequestException");
            continue;
        }

        // Accepted into the documented queue, and the answer IS the stored record.
        let started = reply.ok(case)["CallAnalyticsJob"].clone();
        for (where_read, job) in [
            ("the start's own answer", started),
            ("a read of the job", job_of(&transcribe, case, &name).await),
        ] {
            assert_eq!(
                job["CallAnalyticsJobStatus"],
                json!("QUEUED"),
                "case {case:?}: {where_read} must report a queued job, got {job:?}"
            );
            assert!(
                job["StartTime"].is_null(),
                "case {case:?}: {where_read} must report no StartTime for a job that has \
                 not begun processing, got {:?}",
                job["StartTime"]
            );
            assert!(
                !job["CreationTime"].is_null(),
                "case {case:?}: {where_read} must report when the job was created, got \
                 {job:?}"
            );
            assert!(
                job["Transcript"].is_null() && job["CompletionTime"].is_null(),
                "case {case:?}: {where_read} must report nothing a finished job reports, \
                 got {job:?}"
            );
            assert_eq!(
                job["DataAccessRoleArn"], request["DataAccessRoleArn"],
                "case {case:?}: {where_read} must report the role the start opted in \
                 with, got {:?}",
                job["DataAccessRoleArn"]
            );
        }

        // The queued job is listable in the status it is in, which is what makes the
        // documented `QUEUED` state observable to a caller that polls the family.
        let listing = transcribe
            .call(
                "ListCallAnalyticsJobs",
                json!({"Status": "QUEUED", "MaxResults": 100}),
            )
            .await;
        let summaries = listing.ok(case)["CallAnalyticsJobSummaries"].clone();
        let names: Vec<&str> = summaries
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {summaries:?}"))
            .iter()
            .filter_map(|summary| summary["CallAnalyticsJobName"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![name.as_str()],
            "case {case:?}: exactly the queued job must be listed as QUEUED, got \
             {summaries:?}"
        );
    }
}

#[tokio::test]
async fn a_queued_job_leaves_the_queue_and_completes_on_the_clock() {
    for (index, row) in rows()
        .into_iter()
        .filter(|row| row.opts_in)
        .enumerate()
    {
        let transcribe = Transcribe::new();
        fill_to_the_ceiling(&transcribe).await;

        let case = row.case;
        let name = format!("queued-{index}");
        transcribe
            .call("StartCallAnalyticsJob", (row.request)(&name))
            .await
            .ok_status(case, 200);

        // Nothing sleeps inside the service: each transition is due at an instant
        // stored on the record, so the test waits the span out itself.
        std::thread::sleep(PAST_LEAVING_THE_QUEUE);
        let processing = job_of(&transcribe, case, &name).await;
        assert_eq!(
            processing["CallAnalyticsJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a job the clock has taken out of the queue must be \
             processing, got {processing:?}"
        );
        let start_time = processing["StartTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a job that has begun processing must report a StartTime, \
                 got {processing:?}"
            )
        });

        std::thread::sleep(PAST_EVERY_TRANSITION - PAST_LEAVING_THE_QUEUE);
        let finished = job_of(&transcribe, case, &name).await;
        assert_eq!(
            finished["CallAnalyticsJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: a job the clock has finished must report COMPLETED, got \
             {finished:?}"
        );
        assert_eq!(
            finished["StartTime"].as_f64(),
            Some(start_time),
            "case {case:?}: the instant the job began processing must not move between \
             reads, got {:?}",
            finished["StartTime"]
        );
        let completion_time = finished["CompletionTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: a finished job must report a CompletionTime, got {finished:?}")
        });
        assert!(
            completion_time >= start_time,
            "case {case:?}: a job cannot finish before it started, got {completion_time} \
             after {start_time}"
        );
        assert!(
            !finished["Transcript"].is_null(),
            "case {case:?}: a finished job must report where its transcript would be \
             written, got {finished:?}"
        );
    }
}

#[tokio::test]
async fn the_queue_itself_has_a_bound() {
    use service_com_amazonaws_transcribe_transcribe::state;

    // A waiting job holds no running slot, so the ceiling cannot cap how many of them
    // one caller accumulates: the queue's own depth does, and a start past it is
    // refused exactly as a start that opted into nothing is.
    let depth = state::MAX_QUEUED_CALL_ANALYTICS_JOBS;
    let row = rows()
        .into_iter()
        .find(|row| row.opts_in)
        .expect("a row that opts into the queue");
    let transcribe = Transcribe::new();
    fill_to_the_ceiling(&transcribe).await;

    for index in 0..depth {
        let case = format!("job {index} of the queue's depth");
        let name = format!("waiting-{index}");
        let reply = transcribe
            .call("StartCallAnalyticsJob", (row.request)(&name))
            .await;
        assert_eq!(
            reply.ok(&case)["CallAnalyticsJob"]["CallAnalyticsJobStatus"],
            json!("QUEUED"),
            "case {case:?}: every start up to the queue's depth must be accepted into \
             it, got {:?}",
            reply.body
        );
    }

    let case = "one start past the queue's depth";
    let refused = transcribe
        .call("StartCallAnalyticsJob", (row.request)("past-the-queue"))
        .await;
    refused.refused(case, 400, "LimitExceededException");
    let message = refused.error_message(case);
    assert!(
        message.contains(&ceiling().to_string()),
        "case {case:?}: the refusal must name the quota it exceeded, got {message:?}"
    );
    // Nothing was stored for it, and the queue is no deeper than its bound.
    get(&transcribe, "past-the-queue")
        .await
        .refused(case, 400, "BadRequestException");
    let listing = transcribe
        .call(
            "ListCallAnalyticsJobs",
            json!({"Status": "QUEUED", "MaxResults": 100}),
        )
        .await;
    let page = listing.ok(case)["CallAnalyticsJobSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    // Exactly the queue's depth is waiting — one page's worth of it, since the listing
    // serves at most `MAX_LIST_MAX_RESULTS` summaries at a time.
    let expected = depth.min(state::MAX_LIST_MAX_RESULTS);
    assert_eq!(
        page, expected,
        "case {case:?}: the queue must hold exactly its depth of {depth} waiting jobs, \
         a page of it reported {page}"
    );
}

#[tokio::test]
async fn the_opt_in_member_queues_nothing_below_the_ceiling() {
    // One emulator for every row: the rows below start far fewer jobs than the
    // ceiling, so none of them is ever at the limit.
    let transcribe = Transcribe::new();
    for (index, row) in rows()
        .into_iter()
        .filter(|row| row.opts_in)
        .enumerate()
    {
        let case = row.case;
        let name = format!("below-the-ceiling-{index}");
        let reply = transcribe
            .call("StartCallAnalyticsJob", (row.request)(&name))
            .await;
        let job = reply.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a start below the concurrency limit has no queue to enter, \
             so it must be processing already, got {job:?}"
        );
        assert!(
            !job["StartTime"].is_null(),
            "case {case:?}: a job that is processing must report when it began, got \
             {job:?}"
        );
    }

    // And the jobs those starts left in flight are counted the same way: a role-less
    // start is still served while the family is below its ceiling.
    let case = "a role-less start below the ceiling";
    let served = transcribe
        .call(
            "StartCallAnalyticsJob",
            call_analytics_request("role-less-below", MEDIA.len()),
        )
        .await;
    assert_eq!(
        served.ok(case)["CallAnalyticsJob"]["CallAnalyticsJobStatus"],
        json!("IN_PROGRESS"),
        "case {case:?}: nothing queues a start the ceiling does not refuse, got {:?}",
        served.body
    );
}
