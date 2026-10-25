// SPDX-License-Identifier: CC0-1.0

//! The quota a job start is refused `LimitExceededException` for, end to end over the
//! wire, for every job family this service has.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator and the call are the transcription-job suite's
//! (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: the ceiling a start operation answers
//! `LimitExceededException` for bounds the jobs of that family the service is still
//! WORKING ON — the records in a non-terminal status — and not the job records the
//! account holds. So:
//!
//! * a job that has reached a terminal status releases its slot: once the ceiling's
//!   worth of jobs have all finished, a further start succeeds and the finished
//!   records stay readable, which leaves the account holding MORE records than the
//!   ceiling;
//! * a start made while the ceiling's worth of jobs are still in flight IS refused,
//!   with the modelled error and nothing stored under the refused name, and the very
//!   same request succeeds once those jobs have finished.
//!
//! Asserted over a table whose rows are the four job families — batch transcription,
//! medical transcription, Call Analytics and Medical Scribe — each with its own
//! ceiling constant, start and read operations and request shape, and whose filled
//! jobs vary along the media URI scheme, the media format and each family's own
//! settings axis rather than along the job name alone.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for every clock-owed transition of a job started during the test to
/// have fallen due: a deferred job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans
/// of `crate::state::JOB_TRANSITION_DELAY` (500 ms each).
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// Long enough for a job that started processing straight away to have completed:
/// one span of `crate::state::JOB_TRANSITION_DELAY`, waited out the same way.
const PAST_A_COMPLETION: Duration = Duration::from_millis(700);

/// An IAM role ARN of the form `DataAccessRoleArn` is modelled to accept
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/…$`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The media the filled jobs rotate through, so the jobs counted against a family's
/// quota differ in their input and not only in their name: the URI scheme varies, and
/// each format is one of the modelled `MediaFormat` values
/// (`smithy_query(query="shape", shape="MediaFormat")`).
const MEDIA: &[(&str, &str)] = &[
    ("s3://example-media/audio.wav", "wav"),
    ("s3://example-media/meeting.mp3", "mp3"),
    ("https://example.invalid/media/clip.mp4", "mp4"),
    ("s3://example-media/lecture.flac", "flac"),
    ("s3://example-media/call.ogg", "ogg"),
];

/// One job family: how a job of it is started and read back, where its name and its
/// status live on the wire, and the ceiling its starts are bounded by.
struct Family {
    case: &'static str,
    start: &'static str,
    get: &'static str,
    /// The member both the start request and the read request carry the job name in.
    name_member: &'static str,
    /// The member of the read response the job's projection sits under.
    job_member: &'static str,
    /// The member of that projection carrying the job's `*JobStatus`.
    status_member: &'static str,
    ceiling: usize,
    /// A start request for the job named `name`, varied by its index in the fill.
    request: fn(&str, usize) -> Value,
}

/// A batch transcription start. Every fifth job defers its execution, so the family's
/// queued-then-processing path is counted against the quota too, and the rest start
/// processing straight away.
fn transcription_request(name: &str, index: usize) -> Value {
    let (uri, format) = MEDIA[index % MEDIA.len()];
    let mut request = json!({
        "TranscriptionJobName": name,
        "LanguageCode": "en-US",
        "MediaFormat": format,
        "Media": {"MediaFileUri": uri}
    });
    if index.is_multiple_of(5) {
        request["JobExecutionSettings"] = json!({
            "AllowDeferredExecution": true,
            "DataAccessRoleArn": ROLE_ARN
        });
    }
    request
}

/// A medical transcription start. `Specialty` has the one modelled value
/// `PRIMARYCARE` (`smithy_query(query="shape", shape="Specialty")`); `Type` alternates
/// between the two the model carries
/// (`smithy_query(query="shape", shape="Type")`).
fn medical_transcription_request(name: &str, index: usize) -> Value {
    let (uri, format) = MEDIA[index % MEDIA.len()];
    json!({
        "MedicalTranscriptionJobName": name,
        "LanguageCode": "en-US",
        "MediaFormat": format,
        "Media": {"MediaFileUri": uri},
        "OutputBucketName": "example-medical-output",
        "Specialty": "PRIMARYCARE",
        "Type": if index.is_multiple_of(2) { "DICTATION" } else { "CONVERSATION" }
    })
}

/// A Call Analytics start, the family's minimal request plus the varying media.
fn call_analytics_request(name: &str, index: usize) -> Value {
    let (uri, format) = MEDIA[index % MEDIA.len()];
    json!({
        "CallAnalyticsJobName": name,
        "MediaFormat": format,
        "Media": {"MediaFileUri": uri}
    })
}

/// A Medical Scribe start. `MaxSpeakerLabels` alternates between the published bounds
/// of its range (`@range(min: 2, max: 30)`).
fn medical_scribe_request(name: &str, index: usize) -> Value {
    let (uri, _) = MEDIA[index % MEDIA.len()];
    json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": uri},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": {
            "ShowSpeakerLabels": true,
            "MaxSpeakerLabels": if index.is_multiple_of(2) { 2 } else { 30 }
        }
    })
}

/// Every job family, each with the ceiling its own start is bounded by.
fn families() -> Vec<Family> {
    use service_com_amazonaws_transcribe_transcribe::state;
    vec![
        Family {
            case: "batch transcription jobs",
            start: "StartTranscriptionJob",
            get: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            status_member: "TranscriptionJobStatus",
            ceiling: state::MAX_CONCURRENT_TRANSCRIPTION_JOBS,
            request: transcription_request,
        },
        Family {
            case: "medical transcription jobs",
            start: "StartMedicalTranscriptionJob",
            get: "GetMedicalTranscriptionJob",
            name_member: "MedicalTranscriptionJobName",
            job_member: "MedicalTranscriptionJob",
            status_member: "TranscriptionJobStatus",
            ceiling: state::MAX_CONCURRENT_MEDICAL_TRANSCRIPTION_JOBS,
            request: medical_transcription_request,
        },
        Family {
            case: "Call Analytics jobs",
            start: "StartCallAnalyticsJob",
            get: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            status_member: "CallAnalyticsJobStatus",
            ceiling: state::MAX_CONCURRENT_CALL_ANALYTICS_JOBS,
            request: call_analytics_request,
        },
        Family {
            case: "Medical Scribe jobs",
            start: "StartMedicalScribeJob",
            get: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
            job_member: "MedicalScribeJob",
            status_member: "MedicalScribeJobStatus",
            ceiling: state::MAX_CONCURRENT_MEDICAL_SCRIBE_JOBS,
            request: medical_scribe_request,
        },
    ]
}

impl Family {
    /// Start one job of this family named `name`, as the `index`-th of a fill.
    async fn start_job(&self, transcribe: &Transcribe, name: &str, index: usize) -> Reply {
        transcribe
            .call(self.start, (self.request)(name, index))
            .await
    }

    /// Read one job of this family back.
    async fn get_job(&self, transcribe: &Transcribe, name: &str) -> Reply {
        transcribe
            .call(self.get, json!({self.name_member: name}))
            .await
    }

    /// The status the family's own read operation reports for `name`.
    async fn status_of(&self, transcribe: &Transcribe, name: &str) -> String {
        let case = format!("{}: reading back {name}", self.case);
        let reply = self.get_job(transcribe, name).await;
        let body = reply.ok(&case);
        body.get(self.job_member)
            .and_then(|job| job.get(self.status_member))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} in the answer, got {body:?}",
                    self.status_member
                )
            })
            .to_string()
    }

    /// Start this family's ceiling's worth of jobs, each of which must be accepted.
    async fn fill_to_the_ceiling(&self, transcribe: &Transcribe) -> Vec<String> {
        let mut names = Vec::with_capacity(self.ceiling);
        for index in 0..self.ceiling {
            let name = format!("fill-{index}");
            let case = format!("{}: job {index} of the ceiling", self.case);
            self.start_job(transcribe, &name, index)
                .await
                .ok_status(&case, 200);
            names.push(name);
        }
        names
    }
}

/// A job the service has finished with no longer holds a slot: once the ceiling's
/// worth of jobs are all terminal, further starts are accepted — leaving the account
/// holding more job records than the ceiling, every one of them still readable.
#[tokio::test]
async fn a_finished_job_releases_its_concurrency_slot() {
    for family in families() {
        let transcribe = Transcribe::new();
        let filled = family.fill_to_the_ceiling(&transcribe).await;

        // Nothing sleeps inside the service: each transition is due at an instant
        // stored on the record, so the test waits the span out itself.
        std::thread::sleep(PAST_EVERY_TRANSITION);

        let first = filled.first().expect("a filled job").clone();
        let last = filled.last().expect("a filled job").clone();
        for name in [&first, &last] {
            let status = family.status_of(&transcribe, name).await;
            assert_eq!(
                status, "COMPLETED",
                "case {:?}: the filled job {name} must have finished before the quota is \
                 re-tested, got {status:?}",
                family.case
            );
        }

        // Past the ceiling in RECORDS, but with nothing in flight: every one of these
        // starts is served.
        let mut extra = Vec::new();
        for offset in 0..3 {
            let name = format!("after-the-ceiling-{offset}");
            let case = format!("{}: start {offset} past the ceiling in records", family.case);
            family
                .start_job(&transcribe, &name, offset)
                .await
                .ok_status(&case, 200);
            let status = family.status_of(&transcribe, &name).await;
            assert!(
                status == "QUEUED" || status == "IN_PROGRESS",
                "case {case:?}: a job just started must be readable in a non-terminal \
                 status, got {status:?}"
            );
            extra.push(name);
        }

        // The finished records were kept, not evicted to make room: the account now
        // holds more than the ceiling.
        for name in filled.iter().take(2).chain(extra.iter()) {
            let reply = family.get_job(&transcribe, name).await;
            let case = format!("{}: {name} after the ceiling was passed", family.case);
            assert_eq!(
                reply.ok(&case)[family.job_member][family.name_member],
                json!(name),
                "case {case:?}: the record must still be readable, got {:?}",
                reply.body
            );
        }
    }
}

/// A start made while the ceiling's worth of jobs are IN FLIGHT is refused with the
/// modelled `LimitExceededException`, stores nothing — and the identical request is
/// served once those jobs have finished.
#[tokio::test]
async fn a_start_at_the_concurrency_limit_is_refused_until_a_job_finishes() {
    for family in families() {
        let transcribe = Transcribe::new();
        family.fill_to_the_ceiling(&transcribe).await;

        let case = format!("{}: one start past the concurrency limit", family.case);
        let refused = family.start_job(&transcribe, "over-the-limit", 0).await;
        refused.refused(&case, 400, "LimitExceededException");
        let message = refused.error_message(&case);
        assert!(
            message.contains(&family.ceiling.to_string()),
            "case {case:?}: the refusal must name the quota it exceeded, got {message:?}"
        );
        assert!(
            !message.contains("Delete"),
            "case {case:?}: a concurrency quota is released by a job finishing, not by \
             deleting a record, got {message:?}"
        );

        // The refusal stored nothing: the name is not readable.
        family
            .get_job(&transcribe, "over-the-limit")
            .await
            .refused(&case, 400, "BadRequestException");

        std::thread::sleep(PAST_EVERY_TRANSITION);

        let served = family.start_job(&transcribe, "over-the-limit", 0).await;
        served.ok_status(
            &format!("{}: the same start once the family is idle", family.case),
            200,
        );
        let status = family.status_of(&transcribe, "over-the-limit").await;
        assert!(
            status == "QUEUED" || status == "IN_PROGRESS",
            "case {:?}: the once-refused start must now hold a readable job, got \
             {status:?}",
            family.case
        );
    }
}

/// Counting only the jobs in flight is what the QUOTA does; the store is kept bounded
/// instead by retiring the records the service has already finished with, so a caller
/// that keeps starting jobs never grows it without end — and never loses a job that
/// is still running.
#[tokio::test]
async fn the_store_retires_the_oldest_finished_records_rather_than_growing() {
    use service_com_amazonaws_transcribe_transcribe::state;

    let family = families()
        .into_iter()
        .find(|family| family.start == "StartTranscriptionJob")
        .expect("the batch transcription family");
    let transcribe = Transcribe::new();
    let kept = state::MAX_RETAINED_JOB_RECORDS;
    let total = kept + family.ceiling;

    // Started in rounds of the concurrency quota, waiting each round out, since the
    // quota — not the retention bound — is what refuses a start.
    for index in 0..total {
        let name = format!("bound-{index}");
        let case = format!("job {index} of {total}");
        transcribe
            .call(
                family.start,
                json!({
                    "TranscriptionJobName": name,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": MEDIA[index % MEDIA.len()].0}
                }),
            )
            .await
            .ok_status(&case, 200);
        if (index + 1) % family.ceiling == 0 && index + 1 < total {
            std::thread::sleep(PAST_A_COMPLETION);
        }
    }

    // The oldest records made way for the newest: exactly `total - kept` of them.
    for index in [0, family.ceiling / 2, family.ceiling - 1] {
        let name = format!("bound-{index}");
        let case = format!("the retired record {name}");
        family
            .get_job(&transcribe, &name)
            .await
            .refused(&case, 400, "BadRequestException");
    }

    // Everything within the retention bound is still readable — including the jobs of
    // the last round, which were in flight while those starts were retiring records.
    for index in [family.ceiling, total / 2, total - 1] {
        let name = format!("bound-{index}");
        let case = format!("the retained record {name}");
        let reply = family.get_job(&transcribe, &name).await;
        assert_eq!(
            reply.ok(&case)[family.job_member][family.name_member],
            json!(name),
            "case {case:?}: a record within the retention bound must still be \
             readable, got {:?}",
            reply.body
        );
    }
}
