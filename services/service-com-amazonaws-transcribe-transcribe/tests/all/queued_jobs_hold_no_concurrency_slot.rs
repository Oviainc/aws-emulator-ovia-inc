// SPDX-License-Identifier: CC0-1.0

//! What a job family's CONCURRENCY ceiling counts, end to end over the wire, for both
//! families whose start documents a queue.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler bodies.
//! The emulator and the call are the transcription-job suite's harness
//! (`crate::transcription_jobs`) and the jobs that fill a ceiling are
//! `crate::job_concurrency_quota`'s, both reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: the ceiling a start operation answers `LimitExceededException`
//! for bounds the jobs of that family the service is actually PROCESSING. A job that is
//! only WAITING in the queue its family's start documents occupies none of those slots —
//! it is waiting for one — so however many records wait, a start made while the service
//! is processing fewer jobs than the ceiling is SERVED, and processes from the moment it
//! is started whether or not it opted into the queue. The model states the ceiling from
//! the queueing side: a request that opts in waits "until the number of jobs falls below
//! the concurrent request limit" (`smithy_query(query="shape",
//! shape="JobExecutionSettings")`), and the sibling family's start says the same ("your
//! job will automatically be added to a queue to be processed once your concurrent job
//! count is below the limit", `smithy_query(query="doc",
//! shape="StartCallAnalyticsJob")`), so a job still waiting for that number to fall
//! cannot itself be part of it. What the queue IS bounded by — its own depth — and the
//! refusal a start gets while the ceiling's worth of jobs really are processing are
//! `crate::transcription_job_queueing`'s, `crate::call_analytics_job_queueing`'s and
//! `crate::job_concurrency_quota`'s subjects.
//!
//! Each test asserts that over a table spanning the class the rule generalises over: the
//! two families (batch transcription, keyed on
//! `JobExecutionSettings.AllowDeferredExecution`, and Call Analytics, keyed on
//! `DataAccessRoleArn`), and for each one a start that opts into the queue and starts
//! that opt into nothing at all — the minimal request, a rich one, and one carrying the
//! whole opt-in structure with the flag explicitly `false` — varying the media URI
//! scheme and format, the language, the output location form and the settings, subtitles,
//! redaction and tags beside them.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::job_concurrency_quota::{call_analytics_request, transcription_request};
use crate::transcription_jobs::{Reply, Transcribe};

/// An IAM role ARN of the form the opt-in members are modelled to accept
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/…$`,
/// `smithy_query(query="shape", shape="JobExecutionSettings")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// When, measured from the first filled start, the jobs that only WAIT are started.
///
/// Inside the span the filled jobs are processing in (one
/// `crate::state::JOB_TRANSITION_DELAY`, 500 ms), so the family is at its ceiling and
/// these starts enter the queue instead of running — and late inside it, so the jobs they
/// wait behind have all finished well before these leave the queue, one span after their
/// own start.
const QUEUE_AT: Duration = Duration::from_millis(300);

/// How long [`jobs_left_waiting`] waits between two looks at the family, and how many
/// looks it takes before giving up: the situation it is waiting for — every filled job
/// finished while records still wait — is POLLED rather than slept for, so the test acts
/// at the first instant it holds instead of guessing an instant and leaving the rest of
/// the span unused. Together comfortably past both spans of a queued job even on a slow
/// machine.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const POLLS_FOR_THE_SITUATION: usize = 60;

/// The most pages [`count_in_status`] reads before giving up, so a listing that kept
/// handing out cursors could not loop for ever. A family holds at most its ceiling of
/// processing jobs and its queue's depth of waiting ones, each a page of
/// `crate::state::MAX_LIST_MAX_RESULTS`, so two pages answer and this is slack above it.
const PAGE_LIMIT: usize = 8;

/// Nothing sleeps inside the service: every transition is due at an instant stored on
/// the record, so the test waits the span out itself. `std::thread::sleep`, because the
/// workspace enables tokio's `macros`, `rt` and `rt-multi-thread` features only, so
/// `tokio::time::sleep` is not available here.
fn wait_until(at: Instant) {
    let now = Instant::now();
    if at > now {
        std::thread::sleep(at - now);
    }
}

/// A batch transcription start that opts into the queue the model documents for this
/// family, over `crate::job_concurrency_quota`'s varying media.
fn deferring_transcription_request(name: &str, index: usize) -> Value {
    let mut request = transcription_request(name, index);
    request["JobExecutionSettings"] = json!({
        "AllowDeferredExecution": true,
        "DataAccessRoleArn": ROLE_ARN
    });
    request
}

/// A Call Analytics start that opts into the queue this family's start documents, over
/// the same varying media.
fn queueing_call_analytics_request(name: &str, index: usize) -> Value {
    let mut request = call_analytics_request(name, index);
    request["DataAccessRoleArn"] = json!(ROLE_ARN);
    request
}

/// One start made while the family holds waiting records: the request, and whether it
/// opts into that family's queue at all.
struct Row {
    case: &'static str,
    /// `true` when the request carries the member its family's start documents as the
    /// opt-in into job queueing. Below the ceiling there is nothing to wait for, so such
    /// a start must be processing like any other.
    opts_in: bool,
    request: fn(&str) -> Value,
}

/// One job family that has a documented queue: how a job of it is started, read back and
/// listed, where its name and status live on the wire, the ceiling its starts are bounded
/// by, and the starts to make while it holds waiting records.
struct Family {
    case: &'static str,
    start: &'static str,
    get: &'static str,
    list: &'static str,
    /// The member both the start request and the read request carry the job name in.
    name_member: &'static str,
    /// The member of the read response the job's projection sits under.
    job_member: &'static str,
    /// The member of the listing carrying the page of summaries.
    summaries_member: &'static str,
    /// The member of a projection or summary carrying the job's `*JobStatus`.
    status_member: &'static str,
    ceiling: usize,
    /// A start that is processing straight away, varied by its index in the fill.
    fill: fn(&str, usize) -> Value,
    /// A start that opts into this family's queue, varied the same way.
    queueing: fn(&str, usize) -> Value,
    rows: Vec<Row>,
}

/// Both families whose start documents a queue, each with the ceiling its own start is
/// bounded by read off the service rather than restated here.
fn families() -> Vec<Family> {
    use service_com_amazonaws_transcribe_transcribe::state;
    vec![
        Family {
            case: "batch transcription jobs",
            start: "StartTranscriptionJob",
            get: "GetTranscriptionJob",
            list: "ListTranscriptionJobs",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            summaries_member: "TranscriptionJobSummaries",
            status_member: "TranscriptionJobStatus",
            ceiling: state::MAX_CONCURRENT_TRANSCRIPTION_JOBS,
            fill: transcription_request,
            queueing: deferring_transcription_request,
            rows: vec![
                Row {
                    case: "transcription: no JobExecutionSettings at all, minimal request",
                    opts_in: false,
                    request: |name| {
                        json!({
                            "TranscriptionJobName": name,
                            "LanguageCode": "en-US",
                            "Media": {"MediaFileUri": "s3://example-media/audio.flac"}
                        })
                    },
                },
                Row {
                    case: "transcription: no JobExecutionSettings, output key, subtitles, tags",
                    opts_in: false,
                    request: |name| {
                        json!({
                            "TranscriptionJobName": name,
                            "LanguageCode": "es-US",
                            "MediaFormat": "wav",
                            "Media": {"MediaFileUri": "s3://example-media/lecture.wav"},
                            "OutputBucketName": "example-output",
                            "OutputKey": "transcripts/lecture.json",
                            "Subtitles": {"Formats": ["vtt", "srt"], "OutputStartIndex": 1},
                            "Tags": [
                                {"Key": "team", "Value": "research"},
                                {"Key": "cost", "Value": "7"}
                            ]
                        })
                    },
                },
                Row {
                    case: "transcription: AllowDeferredExecution explicitly false, https media",
                    opts_in: false,
                    request: |name| {
                        json!({
                            "TranscriptionJobName": name,
                            "MediaFormat": "mp4",
                            "Media": {"MediaFileUri": "https://example.invalid/media/clip.mp4"},
                            "IdentifyLanguage": true,
                            "LanguageOptions": ["en-US", "fr-FR"],
                            // Both sub-parameters are required once
                            // `JobExecutionSettings` is included
                            // (`smithy_query(query="shape",
                            // shape="JobExecutionSettings")`).
                            "JobExecutionSettings": {
                                "AllowDeferredExecution": false,
                                "DataAccessRoleArn": ROLE_ARN
                            }
                        })
                    },
                },
                Row {
                    case: "transcription: AllowDeferredExecution true, redacted output",
                    opts_in: true,
                    request: |name| {
                        json!({
                            "TranscriptionJobName": name,
                            "LanguageCode": "de-DE",
                            "MediaFormat": "ogg",
                            "Media": {"MediaFileUri": "s3://example-media/call.ogg"},
                            // Both members of `ContentRedaction` are required
                            // (`smithy_query(query="shape", shape="ContentRedaction")`).
                            "ContentRedaction": {
                                "RedactionType": "PII",
                                "RedactionOutput": "redacted_and_unredacted"
                            },
                            "JobExecutionSettings": {
                                "AllowDeferredExecution": true,
                                "DataAccessRoleArn": ROLE_ARN
                            }
                        })
                    },
                },
            ],
        },
        Family {
            case: "Call Analytics jobs",
            start: "StartCallAnalyticsJob",
            get: "GetCallAnalyticsJob",
            list: "ListCallAnalyticsJobs",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            summaries_member: "CallAnalyticsJobSummaries",
            status_member: "CallAnalyticsJobStatus",
            ceiling: state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS,
            fill: call_analytics_request,
            queueing: queueing_call_analytics_request,
            rows: vec![
                Row {
                    case: "Call Analytics: no role, the family's minimal request",
                    opts_in: false,
                    request: |name| {
                        json!({
                            "CallAnalyticsJobName": name,
                            "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                        })
                    },
                },
                Row {
                    case: "Call Analytics: no role, output location, settings, channels, tags",
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
                            "Tags": [
                                {"Key": "team", "Value": "support"},
                                {"Key": "cost", "Value": "7"}
                            ]
                        })
                    },
                },
                Row {
                    case: "Call Analytics: a role, redacted input audio and an output folder",
                    opts_in: true,
                    request: |name| {
                        json!({
                            "CallAnalyticsJobName": name,
                            "Media": {
                                "RedactedMediaFileUri": "s3://example-media/call-redacted.wav"
                            },
                            "OutputLocation": "s3://example-output/analytics/",
                            "DataAccessRoleArn": "arn:aws-cn:iam::111122223333:role/Queued"
                        })
                    },
                },
            ],
        },
    ]
}

impl Family {
    /// Start one job of this family, as the `index`-th of a fill.
    async fn start_fill(&self, transcribe: &Transcribe, name: &str, index: usize) -> Reply {
        transcribe.call(self.start, (self.fill)(name, index)).await
    }

    /// Start one job of this family that opts into its queue.
    async fn start_queueing(&self, transcribe: &Transcribe, name: &str, index: usize) -> Reply {
        transcribe
            .call(self.start, (self.queueing)(name, index))
            .await
    }

    /// Read one job of this family back.
    async fn get_job(&self, transcribe: &Transcribe, name: &str) -> Reply {
        transcribe
            .call(self.get, json!({self.name_member: name}))
            .await
    }

    /// The status this family's own read operation reports for `name`.
    async fn status_of(&self, transcribe: &Transcribe, case: &str, name: &str) -> String {
        let reply = self.get_job(transcribe, name).await;
        let body = reply.ok(case).clone();
        body[self.job_member][self.status_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} for {name} in the answer, got {body:?}",
                    self.status_member
                )
            })
            .to_string()
    }
}

/// How many of the family's jobs a caller sees in `status`, over every page of its
/// listing: the listing's own documented `Status` filter is what makes the split between
/// the jobs being processed and the jobs only waiting observable from outside.
async fn count_in_status(transcribe: &Transcribe, family: &Family, status: &str) -> usize {
    let case = format!("{}: counting the jobs in {status}", family.case);
    let mut counted = 0;
    let mut token: Option<String> = None;
    for _ in 0..PAGE_LIMIT {
        let mut request = json!({"Status": status, "MaxResults": 100});
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let reply = transcribe.call(family.list, request).await;
        let page = reply.ok(&case).clone();
        counted += page[family.summaries_member]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return counted;
        }
    }
    panic!("case {case:?}: the listing handed out more than {PAGE_LIMIT} pages of cursors");
}

/// Bring a family to the state the rule is about: its ceiling's worth of jobs started and
/// then finished, with its queue holding records that are still only WAITING. Answers
/// how many jobs the service is processing and how many are waiting, as a caller sees
/// them through the family's own listing.
async fn jobs_left_waiting(transcribe: &Transcribe, family: &Family) -> (usize, usize) {
    let started = Instant::now();
    for index in 0..family.ceiling {
        let case = format!("{}: job {index} of the ceiling", family.case);
        family
            .start_fill(transcribe, &format!("fill-{index}"), index)
            .await
            .ok_status(&case, 200);
    }

    // At the ceiling a start that opts in is accepted into the queue, so these records
    // are the ones that only wait. The queue's depth is this family's own ceiling.
    wait_until(started + QUEUE_AT);
    for index in 0..family.ceiling {
        let case = format!("{}: waiting job {index}", family.case);
        family
            .start_queueing(transcribe, &format!("waiting-{index}"), index)
            .await
            .ok_status(&case, 200);
    }

    // The situation the rule is about, as the family's own listing reports it: the
    // service is processing fewer jobs than its ceiling, records are still only waiting,
    // and counting those waiting records would put the family AT the ceiling — so what
    // the next start is answered with says which of the two the ceiling bounds.
    let mut last = (0, 0);
    for _ in 0..POLLS_FOR_THE_SITUATION {
        let processing = count_in_status(transcribe, family, "IN_PROGRESS").await;
        let waiting = count_in_status(transcribe, family, "QUEUED").await;
        last = (processing, waiting);
        if processing < family.ceiling && waiting > 0 && processing + waiting >= family.ceiling {
            return (processing, waiting);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let (processing, waiting) = last;
    panic!(
        "case {:?}: the family never reached the situation the rule is about — fewer than \
         its ceiling of {} jobs processing with records still waiting; last saw \
         {processing} processing and {waiting} waiting",
        family.case, family.ceiling
    );
}

/// A start made while the service is processing fewer jobs than the ceiling is served,
/// however many of that family's records are only waiting in its queue.
#[tokio::test]
async fn a_waiting_job_does_not_exhaust_the_concurrency_ceiling() {
    for family in families() {
        for row in &family.rows {
            // A fresh emulator per row, so each row meets a family whose queue holds
            // waiting records and whose running jobs have all finished.
            let transcribe = Transcribe::new();
            let (processing, waiting) = jobs_left_waiting(&transcribe, &family).await;

            let case = row.case;
            let name = "served-while-others-wait";
            let reply = transcribe.call(family.start, (row.request)(name)).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: with {processing} jobs processing below the ceiling of \
                 {} and {waiting} only waiting, the start must be served, got {} with \
                 body {:?}",
                family.ceiling,
                reply.status,
                reply.body
            );
            let job = reply.ok(case)[family.job_member].clone();
            assert_eq!(
                job[family.status_member],
                json!("IN_PROGRESS"),
                "case {case:?}: a start below the concurrency ceiling has nothing to \
                 wait for, so it must be processing already{}, got {job:?}",
                if row.opts_in {
                    " even though it opted into the queue"
                } else {
                    ""
                }
            );
            assert!(
                !job["StartTime"].is_null(),
                "case {case:?}: a job that is processing must report when it began, got \
                 {job:?}"
            );

            // The answer IS the stored record: the job is readable back in the status
            // the start reported.
            let read_back = family.status_of(&transcribe, case, name).await;
            assert_eq!(
                read_back, "IN_PROGRESS",
                "case {case:?}: a read of the served job must report the status its \
                 start did, got {read_back:?}"
            );

            // And serving it took nothing away from the records that are waiting: they
            // are still waiting, in the status the listing filters on.
            let still_waiting = count_in_status(&transcribe, &family, "QUEUED").await;
            assert_eq!(
                still_waiting, waiting,
                "case {case:?}: serving a start must not disturb the waiting records, \
                 {waiting} were waiting and {still_waiting} are"
            );
        }
    }
}

/// The other half of the same rule, on the same setup: a job the service is PROCESSING
/// does hold a slot, so once the served starts have taken the family back to its
/// ceiling, a start that opts into nothing is refused again — the waiting records are
/// what does not count, not the quota itself.
#[tokio::test]
async fn processing_jobs_still_fill_the_ceiling_while_others_wait() {
    for family in families() {
        let transcribe = Transcribe::new();
        let (processing, waiting) = jobs_left_waiting(&transcribe, &family).await;

        let case = format!(
            "{}: back to the ceiling with records still waiting",
            family.case
        );
        // Take the family back to its ceiling with jobs that really are processing:
        // keep starting them until one is refused. How many that takes is not fixed —
        // the jobs that filled the ceiling before are finishing as this runs — so the
        // loop is bounded by twice the ceiling rather than by a count read beforehand,
        // and every start before the refusal must be served.
        let mut past_the_ceiling = None;
        let mut served = 0;
        for index in 0..2 * family.ceiling {
            let name = format!("processing-{index}");
            let reply = family.start_fill(&transcribe, &name, index).await;
            if reply.status != 200 {
                past_the_ceiling = Some((name, reply));
                break;
            }
            served += 1;
        }
        let (refused_name, refused) = past_the_ceiling.unwrap_or_else(|| {
            panic!(
                "case {case:?}: a start must be refused once the service is processing \
                 its ceiling of {} jobs, {processing} were processing and {waiting} \
                 waiting when the top-up began",
                family.ceiling
            )
        });
        assert!(
            served > 0,
            "case {case:?}: with only {processing} of the ceiling's {} slots held by a \
             job the service is processing, the first of these starts must be served \
             whatever the {waiting} waiting records do",
            family.ceiling
        );
        refused.refused(&case, 400, "LimitExceededException");
        let message = refused.error_message(&case);
        assert!(
            message.contains(&family.ceiling.to_string()),
            "case {case:?}: the refusal must name the quota it exceeded, got {message:?}"
        );
        // The refusal stored nothing, and it was the processing jobs that caused it:
        // the records that only wait are still waiting.
        family
            .get_job(&transcribe, &refused_name)
            .await
            .refused(&case, 400, "BadRequestException");
        let still_waiting = count_in_status(&transcribe, &family, "QUEUED").await;
        assert!(
            still_waiting > 0,
            "case {case:?}: the family must still hold waiting records for this to be \
             about the processing ones, {waiting} were waiting and {still_waiting} are"
        );
    }
}
