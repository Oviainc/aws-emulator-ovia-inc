// SPDX-License-Identifier: CC0-1.0

//! What a client polling a batch inference job READS, whatever interval it polls at.
//!
//! THE RULE UNDER TEST: every status `ModelInvocationJobStatus` documents for a job that has
//! not finished is a status a caller reads the job in — "Submitted – this job has been
//! submitted to a queue for validation", "Validating", "Scheduled – this job has been
//! validated and is now in a queue", "InProgress – this job has begun. You can start viewing
//! the results in the output S3 location"
//! (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`) — so which
//! of them a caller gets to see, and gets to act on, is decided by the job's own progression
//! and never by how long that caller waits between two of its reads. A client polling a
//! submitted job therefore reads `Submitted`, `Validating`, `Scheduled`, `InProgress` and then
//! the terminal status, in that order, whatever its interval; `StopModelInvocationJob`
//! (`POST /model-invocation-job/{jobIdentifier}/stop`) on the job it just read running winds
//! that job down to `Stopping` and then the terminal `Stopped`; and the terminal status, the
//! record counts and the result objects under `outputDataConfig.s3OutputDataConfig.s3Uri` are
//! all reachable by the same poller.
//!
//! The rows differ along what the rule generalises over: the INTERVAL between the client's
//! reads — from a tenth of a second to an hour, four orders of magnitude, including intervals
//! longer than every span of the lifecycle put together, which is what a client polling a real
//! batch job every ten seconds or every minute does — and WHICH operation does the polling,
//! the read of one job or the listing that carries it.
//!
//! The wall clock a long interval stands for is brought by ageing the stored record between
//! reads (`Emulator::age_job`) rather than by sleeping through it, so the row that stands for
//! an hour between polls runs as fast as the one that stands for a tenth of a second.
//!
//! The harness is the one `model_invocation_job_execution` drives (this service and this
//! emulator's own Amazon S3 on a single engine behind the production router), reused rather
//! than copied.

use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::model_invocation_job_execution::{
    count, emulator, given_bucket, given_input_object, job_id, llama_input, read_job, record,
    submit, Emulator, BUCKET,
};
use crate::support::{label, Reply};

/// How long after a stop was accepted the job is owed its move to `Stopped`.
const STOPPED_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_STOPPED_AFTER;

/// `ModelInvocationJobStatus` values this emulator's lifecycle passes through.
const SUBMITTED: &str = "Submitted";
const VALIDATING: &str = "Validating";
const SCHEDULED: &str = "Scheduled";
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";
const STOPPING: &str = "Stopping";
const STOPPED: &str = "Stopped";

/// The whole progression an unstopped job walks, in the order the `status` member's
/// documentation lists it.
const PROGRESSION: &[&str] = &[SUBMITTED, VALIDATING, SCHEDULED, IN_PROGRESS, COMPLETED];

/// The most reads one row makes before it concludes the progression is stuck: a bound on the
/// loop, far above the reads the shortest interval of the table needs.
const MAX_READS: usize = 64;

/// The model every row's records are invoked on, and the endpoint they are a body for: one
/// pair this emulator's inference surface serves, because what varies between rows is the
/// polling interval rather than the work.
const MODEL_ID: &str = "meta.llama3-1-8b-instruct-v1:0";
const INVOCATION_TYPE: &str = "InvokeModel";

/// How many records each row's input holds.
const RECORDS: usize = 3;

/// Which operation a row polls the job with: both report `status` and the four record counts,
/// and both observe the job.
#[derive(Debug, Clone, Copy)]
enum Poller {
    /// `GetModelInvocationJob` on the one job.
    Read,
    /// `ListModelInvocationJobs`, which carries the job as a summary.
    List,
}

/// The intervals a client polls at. Each stands for the wall clock between two of its reads,
/// brought by ageing the stored record rather than by waiting.
const INTERVALS: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(600),
    Duration::from_secs(1),
    Duration::from_secs(10),
    Duration::from_secs(60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(60 * 60),
];

/// One row: how long the client waits between reads, and which operation it reads with.
fn rows() -> Vec<(String, Duration, Poller)> {
    let mut rows = Vec::new();
    for interval in INTERVALS {
        rows.push((
            format!("a read every {interval:?}"),
            *interval,
            Poller::Read,
        ));
    }
    // The listing is polled at the two ends of the range, because it observes a job exactly as
    // a read of that one job does.
    for interval in [INTERVALS[0], *INTERVALS.last().expect("the table has rows")] {
        rows.push((
            format!("a listing every {interval:?}"),
            interval,
            Poller::List,
        ));
    }
    rows
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

/// Read the job the way this row's client does, through the operation it polls with.
async fn poll_once(emu: &Emulator, case: &str, arn: &str, poller: Poller) -> Reply {
    match poller {
        Poller::Read => {
            let reply = read_job(emu, arn).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: the job must stay readable while it is polled, got {} {:?}",
                reply.status, reply.body
            );
            reply
        }
        Poller::List => {
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

/// The status one poll reported.
fn status_of(case: &str, reply: &Reply) -> String {
    reply
        .get("status")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a polled job reports a status, got {:?}", reply.body))
        .to_string()
}

#[tokio::test]
async fn a_client_polling_at_any_interval_reads_every_stage_and_then_the_results() {
    let emu = emulator();
    given_bucket(&emu).await;

    for (index, (case, interval, poller)) in rows().into_iter().enumerate() {
        let (arn, input_key) = given_job(&emu, &format!("poll-{index}")).await;

        // The client's own loop: read, then wait its interval, until the job is finished.
        let mut seen: Vec<String> = Vec::new();
        let mut terminal: Option<Reply> = None;
        for _ in 0..MAX_READS {
            let reply = poll_once(&emu, &case, &arn, poller).await;
            let status = status_of(&case, &reply);
            if seen.last() != Some(&status) {
                seen.push(status.clone());
            }
            if status == COMPLETED {
                terminal = Some(reply);
                break;
            }
            // Waiting the interval, brought by ageing the record rather than by sleeping.
            emu.age_job(&arn, interval);
        }

        assert_eq!(
            seen, PROGRESSION,
            "case {case:?}: a client polling the job must read it in every status of the \
             progression, in order, saw {seen:?}"
        );
        let terminal = terminal.unwrap_or_else(|| {
            panic!(
                "case {case:?}: the job must reach its terminal status inside {MAX_READS} reads, \
                 saw {seen:?}"
            )
        });

        // What the terminal read reports: the counts that account for every record of the
        // input, and the instant the job ended.
        let total = i64::try_from(RECORDS).expect("a small record count");
        for (member, expected) in [
            ("totalRecordCount", total),
            ("processedRecordCount", total),
            ("successRecordCount", total),
            ("errorRecordCount", 0),
        ] {
            let actual = count(&terminal, member);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: {member} must be {expected} for an input of {total} records, \
                 got {actual:?}"
            );
        }
        assert!(
            terminal.get("endTime").is_string(),
            "case {case:?}: a job that has ended reports when it ended, got {:?}",
            terminal.get("endTime")
        );

        // And the results the status told the client to view are under the prefix the
        // submission named, one per input record.
        let input_name = input_key
            .rsplit('/')
            .next()
            .expect("the key has a last segment");
        let key = format!("output/{}/{input_name}.out", job_id(&arn));
        let (status, body) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a client that polled the job to its end must find its results at \
             {BUCKET}/{key}, got {status}"
        );
        let text = String::from_utf8(body.to_vec()).expect("the results are UTF-8 text");
        let results: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(
            results.len(),
            RECORDS,
            "case {case:?}: one result record per input record, expected {RECORDS}, got {} in \
             {text:?}",
            results.len()
        );
    }
}

#[tokio::test]
async fn a_stop_of_the_job_a_poller_just_read_running_winds_it_down() {
    let emu = emulator();
    given_bucket(&emu).await;

    for (index, (case, interval, poller)) in rows().into_iter().enumerate() {
        let (arn, _) = given_job(&emu, &format!("stop-{index}")).await;

        // The documented control flow: poll until the job is running, then cancel it.
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..MAX_READS {
            let reply = poll_once(&emu, &case, &arn, poller).await;
            let status = status_of(&case, &reply);
            if seen.last() != Some(&status) {
                seen.push(status.clone());
            }
            if status == IN_PROGRESS {
                break;
            }
            emu.age_job(&arn, interval);
        }
        assert_eq!(
            seen.last().map(String::as_str),
            Some(IN_PROGRESS),
            "case {case:?}: a client polling the job must read it running before it stops it, \
             saw {seen:?}"
        );

        let stopped = emu
            .bedrock(
                "POST",
                &format!("/model-invocation-job/{}/stop", label(&arn)),
                None,
            )
            .await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: a stop of the running job must be accepted, got {} {:?}",
            stopped.status, stopped.body
        );

        // The stop landed on a job that had not finished, so the caller observes the transient
        // status and then the terminal one the stop owes.
        let stopping = read_job(&emu, &arn).await;
        assert_eq!(
            status_of(&case, &stopping),
            STOPPING,
            "case {case:?}: the stopped job must be winding down, got {:?}",
            stopping.body
        );
        emu.age_job(&arn, STOPPED_AFTER);
        let done = read_job(&emu, &arn).await;
        assert_eq!(
            status_of(&case, &done),
            STOPPED,
            "case {case:?}: the stop must settle on {STOPPED}, got {:?}",
            done.body
        );
        assert!(
            done.get("endTime").is_string(),
            "case {case:?}: a job the stop ended reports when it ended, got {:?}",
            done.get("endTime")
        );

        // A stopped job is owed no completion of its own, however long the client goes on
        // polling it.
        for _ in 0..3 {
            emu.age_job(&arn, interval);
            let after = read_job(&emu, &arn).await;
            assert_eq!(
                status_of(&case, &after),
                STOPPED,
                "case {case:?}: a stopped job never reports {COMPLETED}, got {:?}",
                after.body
            );
        }
    }
}
