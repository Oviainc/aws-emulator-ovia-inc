// SPDX-License-Identifier: CC0-1.0

//! WHAT DECIDES the status a batch inference job reports: the time elapsed since its
//! submission, and nothing else.
//!
//! THE RULE UNDER TEST: the progression `ModelInvocationJobStatus` documents — "Submitted –
//! this job has been submitted to a queue for validation", "Validating", "Scheduled – this
//! job has been validated and is now in a queue", "InProgress – this job has begun. You can
//! start viewing the results in the output S3 location" and then a terminal status
//! (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`) — is driven
//! by the service as the job runs, so the status a read is answered is a function of the time
//! since the submission alone. A client that submits a job, waits and then reads ONCE is
//! answered the status its wait has carried the job to, up to and including the terminal one,
//! with the record counts, the end instant and the result objects under
//! `outputDataConfig.s3OutputDataConfig.s3Uri` that terminal status accounts for; and two
//! clients whose waits are equal read the same status whether one of them polled a dozen
//! times on the way and the other not at all.
//!
//! The rows differ along what the rule generalises over: HOW LONG the client waited before its
//! read — zero, a moment short of each of the clock's own instants, a moment past each of
//! them, and waits far longer than the whole lifecycle (half a minute and an hour, the
//! intervals a client of a long-running job actually sleeps) — WHICH operation it read with
//! (the read of one job, or the listing that carries it), and HOW MANY reads it made over
//! that wait (one, against a dozen).
//!
//! The wall clock each wait stands for is brought by ageing the stored record
//! (`Emulator::age_job`) rather than by sleeping through it.
//!
//! The harness is the one `model_invocation_job_execution` drives (this service and this
//! emulator's own Amazon S3 on a single engine behind the production router), reused rather
//! than copied.

use std::time::Duration;

use serde_json::Value;

use crate::model_invocation_job_execution::{
    completed_at, count, emulator, given_bucket, given_input_object, in_progress_at, job_id,
    llama_input, read_job, record, scheduled_at, submit, validating_at, Emulator, BUCKET,
};
use crate::support::Reply;

/// How far on either side of one of those instants a row's read lands: large enough to swamp
/// the milliseconds an in-process call takes, and several times smaller than every span of the
/// progression, so a read on either side of an instant is inside the stage it belongs to.
const MARGIN: Duration = Duration::from_millis(200);

/// `ModelInvocationJobStatus` values this emulator's lifecycle passes through.
const SUBMITTED: &str = "Submitted";
const VALIDATING: &str = "Validating";
const SCHEDULED: &str = "Scheduled";
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// The model every row's records are invoked on, and the endpoint they are a body for: what
/// varies between rows is the wait rather than the work.
const MODEL_ID: &str = "meta.llama3-1-8b-instruct-v1:0";
const INVOCATION_TYPE: &str = "InvokeModel";

/// How many records each row's input holds.
const RECORDS: usize = 4;

/// Which operation a row reads the job with: both report `status` and the four record counts.
#[derive(Debug, Clone, Copy)]
enum Reader {
    /// `GetModelInvocationJob` on the one job.
    Read,
    /// `ListModelInvocationJobs`, which carries the job as a summary.
    List,
}

/// The input of one row: `RECORDS` records, each a JSON object carrying its own `recordId` and
/// the `modelInput` the endpoint serves.
fn lines() -> Vec<String> {
    (1..=RECORDS)
        .map(|index| {
            record(
                Some(&format!("RECORD{index:05}")),
                llama_input(&format!("record {index}")),
            )
        })
        .collect()
}

/// Submit one row's job over its own staged input, and hand back its ARN and input key.
async fn given_job(emu: &Emulator, stem: &str) -> (String, String) {
    let input_key = format!("input/{stem}.jsonl");
    given_input_object(emu, &input_key, &lines()).await;
    let arn = submit(
        emu,
        &format!("batch-{stem}"),
        MODEL_ID,
        INVOCATION_TYPE,
        &format!("s3://{BUCKET}/{input_key}"),
        &format!("s3://{BUCKET}/output"),
    )
    .await;
    (arn, input_key)
}

/// Read the job through the operation this row uses.
async fn read_once(emu: &Emulator, case: &str, arn: &str, reader: Reader) -> Reply {
    match reader {
        Reader::Read => {
            let reply = read_job(emu, arn).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: the submitted job must be readable, got {} {:?}",
                reply.status, reply.body
            );
            reply
        }
        Reader::List => {
            let listed = emu.bedrock("GET", "/model-invocation-jobs", None).await;
            assert_eq!(
                listed.status, 200,
                "case {case}: the listing must answer 200, got {} {:?}",
                listed.status, listed.body
            );
            let summary = listed
                .get("invocationJobSummaries")
                .as_array()
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| row.get("jobArn").and_then(Value::as_str) == Some(arn))
                        .cloned()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: the listing must carry the submitted job, got {:?}",
                        listed.body
                    )
                });
            Reply {
                status: listed.status,
                content_type: listed.content_type,
                body: summary,
                error: None,
            }
        }
    }
}

/// The status one read reported.
fn status_of(case: &str, reply: &Reply) -> String {
    reply
        .get("status")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read job reports a status, got {:?}", reply.body))
        .to_string()
}

/// The waits of the table, each with the status the clock owes a job that has waited it.
///
/// The instants come off this service's own spans rather than from a figure written here, and
/// every one of them is covered on both sides — a margin short of the instant and a margin
/// past it — plus two waits far longer than the whole lifecycle.
fn waits() -> Vec<(String, Duration, &'static str)> {
    let rows = vec![
        (Duration::ZERO, SUBMITTED),
        (validating_at().saturating_sub(MARGIN), SUBMITTED),
        (validating_at() + MARGIN, VALIDATING),
        (scheduled_at().saturating_sub(MARGIN), VALIDATING),
        (scheduled_at() + MARGIN, SCHEDULED),
        (in_progress_at().saturating_sub(MARGIN), SCHEDULED),
        (in_progress_at() + MARGIN, IN_PROGRESS),
        (completed_at().saturating_sub(MARGIN), IN_PROGRESS),
        (completed_at() + MARGIN, COMPLETED),
        (Duration::from_secs(30), COMPLETED),
        (Duration::from_secs(60 * 60), COMPLETED),
    ];
    rows.into_iter()
        .map(|(wait, expected)| (format!("a single read {wait:?} after the submission"), wait, expected))
        .collect()
}

#[tokio::test]
async fn one_read_after_a_wait_answers_the_status_that_wait_owes_the_job() {
    let emu = emulator();
    given_bucket(&emu).await;

    for (index, (case, wait, expected)) in waits().into_iter().enumerate() {
        for reader in [Reader::Read, Reader::List] {
            let case = format!("{case}, read by {reader:?}");
            let (arn, input_key) = given_job(&emu, &format!("wait-{index}-{reader:?}")).await;

            // The client's whole interaction: submit, wait, read ONCE.
            emu.age_job(&arn, wait);
            let reply = read_once(&emu, &case, &arn, reader).await;
            let status = status_of(&case, &reply);
            assert_eq!(
                status, expected,
                "case {case:?}: the status is decided by the time since the submission, so a \
                 wait of {wait:?} owes {expected}, got {status} in {:?}",
                reply.body
            );

            // A read is not what carries the job: reading again at the same instant reports
            // the same status.
            let again = read_once(&emu, &case, &arn, reader).await;
            assert_eq!(
                status_of(&case, &again),
                status,
                "case {case:?}: a second read at the same instant must report the same status, \
                 got {:?}",
                again.body
            );

            if expected != COMPLETED {
                assert_eq!(
                    reply.get("endTime"),
                    Value::Null,
                    "case {case:?}: a job that has not finished reports no end, got {:?}",
                    reply.get("endTime")
                );
                continue;
            }

            // The terminal status came with everything it accounts for, on that ONE read: the
            // counts over the whole input, the end instant, and the result records under the
            // prefix the submission named.
            let total = i64::try_from(RECORDS).expect("a small record count");
            for (member, want) in [
                ("totalRecordCount", total),
                ("processedRecordCount", total),
                ("successRecordCount", total),
                ("errorRecordCount", 0),
            ] {
                let actual = count(&reply, member);
                assert_eq!(
                    actual,
                    Some(want),
                    "case {case:?}: {member} must be {want} for an input of {total} records, \
                     got {actual:?}"
                );
            }
            assert!(
                reply.get("endTime").is_string(),
                "case {case:?}: a job that has ended reports when it ended, got {:?}",
                reply.get("endTime")
            );

            let input_name = input_key
                .rsplit('/')
                .next()
                .expect("the key has a last segment");
            let key = format!("output/{}/{input_name}.out", job_id(&arn));
            let (status, body) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read that first reported the terminal status must leave the \
                 results at {BUCKET}/{key}, got {status}"
            );
            let text = String::from_utf8(body.to_vec()).expect("the results are UTF-8 text");
            let results: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
            assert_eq!(
                results.len(),
                RECORDS,
                "case {case:?}: one result record per input record, expected {RECORDS}, got {} \
                 in {text:?}",
                results.len()
            );
        }
    }
}

/// How many reads the polling client of the test below makes over its wait.
const POLLS: usize = 12;

#[tokio::test]
async fn the_number_of_reads_over_a_wait_changes_nothing_about_the_status() {
    let emu = emulator();
    given_bucket(&emu).await;

    // One row per wait of the table: two jobs submitted alike, one of them polled a dozen
    // times over that wait and the other read only at its end. The progression belongs to the
    // job rather than to the caller, so both are answered the same status.
    for (index, (case, wait, expected)) in waits().into_iter().enumerate() {
        let (polled, _) = given_job(&emu, &format!("polled-{index}")).await;
        let (patient, _) = given_job(&emu, &format!("patient-{index}")).await;

        let step = wait / u32::try_from(POLLS).expect("a small poll count");
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..POLLS {
            emu.age_job(&polled, step);
            emu.age_job(&patient, step);
            let reply = read_job(&emu, &polled).await;
            let status = status_of(&case, &reply);
            if seen.last() != Some(&status) {
                seen.push(status);
            }
        }

        let polled_status = status_of(
            &case,
            &read_job(&emu, &polled).await,
        );
        let patient_status = status_of(&case, &read_job(&emu, &patient).await);
        assert_eq!(
            polled_status, patient_status,
            "case {case:?}: a job polled {POLLS} times over the wait and one read once at its \
             end must report the same status, got {polled_status} against {patient_status} \
             (the poller saw {seen:?})"
        );
        assert_eq!(
            patient_status, expected,
            "case {case:?}: the single read after {wait:?} must answer {expected}, got \
             {patient_status}"
        );
    }
}
