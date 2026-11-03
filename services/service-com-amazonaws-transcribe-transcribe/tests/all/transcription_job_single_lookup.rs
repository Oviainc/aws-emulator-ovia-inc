// SPDX-License-Identifier: CC0-1.0

//! The ONE lookup the transcription-job family's reporting and deleting operations
//! share, driven end to end over the wire through both of them.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope and the handlers'
//! own refusals — not just the handler bodies. The emulator and the call are the
//! transcription-job suite's (`crate::transcription_jobs`), reused rather than stood
//! up a second time.
//!
//! THE RULE UNDER TEST: a transcription job is looked up in one place, which
//! materialises every status transition the clock already owed it BEFORE the caller
//! decides anything from its status — so `GetTranscriptionJob` and
//! `DeleteTranscriptionJob` can never disagree about the status a stored job is in,
//! nor about a name the Region holds no job under. One of them keeping the
//! clock-owed transition while the other lost it would show up here as a job that
//! reads back `COMPLETED` and is still refused deletion for a status it had left (or
//! the reverse).
//!
//! The refusals themselves carry no status — this service words both of them as fixed
//! sentences — so what the two operations are held to agreeing on here is WHICH answer
//! each gives: while the read reports a non-terminal status the delete refuses, and
//! once the read reports the terminal one the same delete goes through.
//!
//! Asserted over a table whose rows vary along what the rule generalises over: what
//! the request asked for (an explicit language against automatic identification, where
//! the output goes, which transcripts the job was asked to produce, whether it deferred
//! its execution) and the Region the call is signed for — not along the job name alone.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::Transcribe;

/// Long enough for every clock-owed transition of a job started during the test to
/// have fallen due: a deferred job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans
/// of `crate::state::JOB_TRANSITION_DELAY` (500 ms each).
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// An IAM role ARN of the form `DataAccessRoleArn` is modelled to accept
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/…$`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/TranscribeAccess";

/// The statuses a transcription job passes through, in the order the clock moves it
/// along them: `QUEUED` only for a job that entered the queue its family's concurrency
/// ceiling feeds, then `IN_PROGRESS`, then the terminal `COMPLETED`
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")` lists
/// `QUEUED`, `IN_PROGRESS`, `FAILED` and `COMPLETED`; `FAILED` is the other terminal
/// one and no emulated job reaches it).
const STATUS_ORDER: &[&str] = &["QUEUED", "IN_PROGRESS", "COMPLETED"];

/// Where `status` sits in [`STATUS_ORDER`], so two observations of one job can be
/// compared: the clock only ever moves a job forward along it.
fn rank(case: &str, status: &str) -> usize {
    STATUS_ORDER
        .iter()
        .position(|known| *known == status)
        .unwrap_or_else(|| panic!("case {case:?}: {status:?} is not a modelled job status"))
}

/// One job class: the start request, the Region it is signed for, and the status a
/// job of that class is in before any transition of it has fallen due.
struct Row {
    case: &'static str,
    name: &'static str,
    /// `None` leaves the request unsigned, so the handler sees this service's default
    /// Region; `Some` puts a Region in the SigV4 credential scope.
    region: Option<&'static str>,
    request: Value,
    first_status: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "processing straight away, service-managed output",
            name: "immediate-job",
            region: None,
            request: json!({
                "TranscriptionJobName": "immediate-job",
                "LanguageCode": "en-US",
                "MediaFormat": "flac",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"}
            }),
            first_status: "IN_PROGRESS",
        },
        Row {
            case: "deferred execution, which queues nothing below the ceiling",
            name: "deferred.job-1",
            region: None,
            request: json!({
                "TranscriptionJobName": "deferred.job-1",
                "LanguageCode": "de-DE",
                "MediaFormat": "mp3",
                "Media": {"MediaFileUri": "s3://example-media/meeting.mp3"},
                "OutputBucketName": "example-output",
                "OutputKey": "transcripts/",
                "JobExecutionSettings": {
                    "AllowDeferredExecution": true,
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
            // Nothing of this family is in flight here, so the deferral has no queue
            // to enter: the job is processing already
            // (`crate::transcription_job_queueing` drives the ceiling it queues at).
            first_status: "IN_PROGRESS",
        },
        Row {
            case: "redacted and unredacted transcripts, signed for eu-west-1",
            name: "redacted-job",
            region: Some("eu-west-1"),
            request: json!({
                "TranscriptionJobName": "redacted-job",
                "LanguageCode": "en-GB",
                "Media": {"MediaFileUri": "https://example.invalid/media/clip.mp4"},
                "MediaFormat": "mp4",
                "ContentRedaction": {
                    "RedactionType": "PII",
                    "RedactionOutput": "redacted_and_unredacted"
                },
                "Subtitles": {"Formats": ["vtt", "srt"]}
            }),
            first_status: "IN_PROGRESS",
        },
        Row {
            case: "deferred execution, signed for ap-southeast-2",
            name: "deferred-elsewhere",
            region: Some("ap-southeast-2"),
            request: json!({
                "TranscriptionJobName": "deferred-elsewhere",
                "IdentifyLanguage": true,
                "LanguageOptions": ["en-US", "es-US"],
                "Media": {"MediaFileUri": "s3://example-media/lecture.ogg"},
                "MediaFormat": "ogg",
                "JobExecutionSettings": {
                    "AllowDeferredExecution": true,
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
            // Deferred too, and in another Region, where this family is just as idle.
            first_status: "IN_PROGRESS",
        },
    ]
}

impl Row {
    /// The status `GetTranscriptionJob` reports for this row's job.
    async fn reported_status(&self, transcribe: &Transcribe, case: &str) -> String {
        let reply = transcribe
            .call_in_region(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": self.name}),
                self.region,
            )
            .await;
        let body = reply.ok(case);
        body["TranscriptionJob"]["TranscriptionJobStatus"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a TranscriptionJobStatus, got {body:?}")
            })
            .to_string()
    }
}

/// How this service refuses a delete of a job it has not finished working on: the job
/// families' own fixed sentence for that condition, which names neither the job asked
/// about nor the status it is in.
const NOT_DELETABLE: &str = "The job couldn't be deleted. Make sure it exists and isn't \
     pending.";

/// A job still being processed is refused deletion while the read operation reports it
/// as such: the two go through one lookup, so neither can see a status the other has
/// already left behind.
#[tokio::test]
async fn a_read_and_a_delete_see_the_same_non_terminal_status() {
    for row in rows() {
        let transcribe = Transcribe::new();
        let start = format!("{}: start", row.case);
        transcribe
            .call_in_region("StartTranscriptionJob", row.request.clone(), row.region)
            .await
            .ok_status(&start, 200);

        let before = row.reported_status(&transcribe, row.case).await;
        assert_eq!(
            before, row.first_status,
            "case {:?}: a job just started must read back {:?}, got {before:?}",
            row.case, row.first_status
        );

        // The delete is refused for the non-terminal status the reads either side of it
        // bracket: the clock only moves a job forward, so a delete that had NOT
        // materialised what the clock owed would be refusing for a status older than the
        // earlier read's.
        let refusal = transcribe
            .call_in_region(
                "DeleteTranscriptionJob",
                json!({"TranscriptionJobName": row.name}),
                row.region,
            )
            .await;
        refusal.refused(row.case, 400, "BadRequestException");
        let message = refusal.error_message(row.case);
        assert_eq!(
            message, NOT_DELETABLE,
            "case {:?}: expected the job family's non-terminal delete refusal, got \
             {message:?}",
            row.case
        );
        assert!(
            !message.contains(row.name),
            "case {:?}: the refusal must not echo the job name, got {message:?}",
            row.case
        );
        let after = row.reported_status(&transcribe, row.case).await;
        assert!(
            rank(row.case, &before) <= rank(row.case, &after),
            "case {:?}: the clock only moves a job forward, got {before:?} then \
             {after:?}",
            row.case
        );
    }
}

/// Once every transition the clock owed a job has fallen due, BOTH operations see it:
/// the read reports the terminal status and the delete of that same job goes through
/// instead of refusing it for a status it had left.
#[tokio::test]
async fn a_read_and_a_delete_see_the_same_clock_owed_completion() {
    for row in rows() {
        let transcribe = Transcribe::new();
        let start = format!("{}: start", row.case);
        transcribe
            .call_in_region("StartTranscriptionJob", row.request.clone(), row.region)
            .await
            .ok_status(&start, 200);

        // Nothing sleeps inside the service: each transition is due at an instant
        // stored on the record, so the test waits the span out itself.
        std::thread::sleep(PAST_EVERY_TRANSITION);

        let completed = row.reported_status(&transcribe, row.case).await;
        assert_eq!(
            completed, "COMPLETED",
            "case {:?}: a job whose whole span has elapsed reads back COMPLETED, got \
             {completed:?}",
            row.case
        );

        let deleted = transcribe
            .call_in_region(
                "DeleteTranscriptionJob",
                json!({"TranscriptionJobName": row.name}),
                row.region,
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: a job the clock has already completed is deletable, got {} with \
             body {:?}",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {:?}: DeleteTranscriptionJob answers no members, got {:?}",
            row.case,
            deleted.body
        );

        // And the record is gone, so the read now answers the same not-found refusal
        // the delete would.
        let gone = transcribe
            .call_in_region(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": row.name}),
                row.region,
            )
            .await;
        gone.refused(row.case, 400, "BadRequestException");
    }
}

/// A name the Region holds no job under answers the SAME refusal through both
/// operations — the one arm of the shared lookup that found nothing.
#[tokio::test]
async fn a_name_no_job_is_stored_under_is_refused_alike_by_both_operations() {
    let transcribe = Transcribe::new();

    // One job exists, in one Region, so the rows below also cover a name that IS
    // stored but not in the Region the call is served in.
    transcribe
        .call_in_region(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "stored-in-eu-west-1",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "MediaFormat": "wav"
            }),
            Some("eu-west-1"),
        )
        .await
        .ok_status("the one stored job", 200);

    // The last row is a name of the modelled maximum length
    // (`TranscriptionJobName`, len 1..200, pattern `^[0-9a-zA-Z._-]+$`), so the
    // not-found arm is exercised at the boundary of what a name may be as well as on
    // ordinary names.
    let cases: Vec<(String, String, Option<&str>)> = vec![
        (
            "a name nothing was ever started under".to_string(),
            "no-such-job".to_string(),
            None,
        ),
        (
            "a name whose job lives in another Region".to_string(),
            "stored-in-eu-west-1".to_string(),
            Some("us-east-2"),
        ),
        (
            "a differently cased spelling of a stored name (job names are case \
             sensitive)"
                .to_string(),
            "STORED-IN-EU-WEST-1".to_string(),
            Some("eu-west-1"),
        ),
        (
            "a name of the modelled maximum length".to_string(),
            "a".repeat(200),
            Some("eu-west-1"),
        ),
    ];

    for (case, name, region) in cases {
        let case = case.as_str();
        let name = name.as_str();
        let read = transcribe
            .call_in_region(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": name}),
                region,
            )
            .await;
        let deleted = transcribe
            .call_in_region(
                "DeleteTranscriptionJob",
                json!({"TranscriptionJobName": name}),
                region,
            )
            .await;
        read.refused(case, 400, "BadRequestException");
        deleted.refused(case, 400, "BadRequestException");
        assert_eq!(
            read.error_message(case),
            deleted.error_message(case),
            "case {case:?}: both operations report an absent job with the same message, \
             got {:?} and {:?}",
            read.error_message(case),
            deleted.error_message(case)
        );
    }
}
