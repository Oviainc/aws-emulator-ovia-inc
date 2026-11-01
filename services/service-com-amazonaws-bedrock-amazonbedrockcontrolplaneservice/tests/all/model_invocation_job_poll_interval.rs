// SPDX-License-Identifier: CC0-1.0

//! What a client polling a batch inference job READS, whatever interval it polls at.
//!
//! THE RULE UNDER TEST: a batch inference job's progression is driven by the SERVICE as the
//! job runs, so which status a read is answered is decided by the time elapsed since the
//! submission and by nothing else — never by how many reads a caller has made, nor by how
//! long it waited between two of them. `ModelInvocationJobStatus` documents the progression
//! "Submitted – this job has been submitted to a queue for validation", "Validating",
//! "Scheduled – this job has been validated and is now in a queue", "InProgress – this job
//! has begun. You can start viewing the results in the output S3 location" and then a
//! terminal status
//! (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`): those
//! intermediate statuses are OBSERVABLE — a client polling tighter than the spans reads the
//! job in each of them — and never waypoints a caller must poll through, so a client that
//! waits and reads once is answered the status its wait has carried the job to, up to and
//! including the terminal one. With the terminal status come the record counts and the result
//! objects under `outputDataConfig.s3OutputDataConfig.s3Uri`, for the very read that first
//! reports it; and `StopModelInvocationJob`
//! (`POST /model-invocation-job/{jobIdentifier}/stop`) on a job the elapsed time has not
//! finished winds it down to `Stopping` and then the terminal `Stopped`.
//!
//! The rows differ along what the rule generalises over: the INTERVAL between the client's
//! reads — from a tenth of a second to an hour, four orders of magnitude, including intervals
//! longer than every span of the lifecycle put together, which is what a client polling a real
//! batch job every ten seconds or every minute does — WHICH operation does the polling, the
//! read of one job or the listing that carries it, and (for the stop) HOW LONG the client
//! waited before its single read, one row per stage of the progression.
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
    completed_at, count, emulator, given_bucket, given_input_object, in_progress_at, job_id,
    llama_input, read_job, record, scheduled_at, submit, validating_at, Emulator, BUCKET,
};
use crate::support::{label, Reply};

/// How long after a stop was accepted the job is owed its move to `Stopped`.
const STOPPED_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_STOPPED_AFTER;

/// The whole wall clock a job nobody stops takes from its submission to its terminal status,
/// read off the shared harness so this suite cannot drift from the others when a span changes.
fn lifespan() -> Duration {
    completed_at()
}

/// How far into a stage's own window a row's single read lands: long enough that the stage
/// before it is certainly behind the job, short enough that the stage after it is certainly
/// still ahead — every span of the progression is several times this.
const INTO_THE_STAGE: Duration = Duration::from_millis(100);

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
/// loop, far above the reads the shortest interval of the table needs to span the lifecycle.
const MAX_READS: usize = 256;

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
async fn a_client_polling_at_any_interval_reaches_the_terminal_status_and_the_results() {
    let emu = emulator();
    given_bucket(&emu).await;

    for (index, (case, interval, poller)) in rows().into_iter().enumerate() {
        let (arn, input_key) = given_job(&emu, &format!("poll-{index}")).await;

        // The client's own loop: read, then wait its interval, until the job is finished.
        let mut seen: Vec<String> = Vec::new();
        let mut terminal: Option<Reply> = None;
        let mut reads = 0_usize;
        let mut waited = Duration::ZERO;
        for _ in 0..MAX_READS {
            let reply = poll_once(&emu, &case, &arn, poller).await;
            reads += 1;
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
            waited += interval;
        }

        // Whatever the interval, the statuses the client read are statuses of the documented
        // progression, in its own order and none of them twice: a job never goes backwards and
        // no poller is answered a status the progression does not carry.
        let mut stages = PROGRESSION.iter();
        for status in &seen {
            assert!(
                stages.any(|stage| stage == status),
                "case {case:?}: the statuses a poller reads must be a subsequence of \
                 {PROGRESSION:?}, in order, saw {seen:?}"
            );
        }
        assert_eq!(
            seen.first().map(String::as_str),
            Some(SUBMITTED),
            "case {case:?}: the read of a job just submitted must answer the queued status, \
             saw {seen:?}"
        );
        let terminal = terminal.unwrap_or_else(|| {
            panic!(
                "case {case:?}: the job must reach its terminal status inside {MAX_READS} reads, \
                 saw {seen:?}"
            )
        });

        // The ELAPSED time is what carried the job there, not the number of reads: the first
        // read taken once the whole lifespan had passed answered the terminal status, so no
        // client needs more reads than its own interval divides that lifespan into.
        let lifespan = lifespan();
        let reads_needed = lifespan.as_nanos() / interval.as_nanos() + 2;
        assert!(
            u128::try_from(reads).expect("a small read count") <= reads_needed,
            "case {case:?}: the progression must be carried by elapsed time rather than by \
             reads, took {reads} reads over {waited:?} of waiting where the elapsed time needs \
             at most {reads_needed} reads, saw {seen:?}"
        );

        // A client polling tighter than every span of the lifecycle is the one that observes
        // each intermediate status; a coarse one is simply answered the status its wait owes.
        if interval <= INTERVALS[0] {
            assert_eq!(
                seen, PROGRESSION,
                "case {case:?}: a client polling tighter than every span must read the job in \
                 every status of the progression, in order, saw {seen:?}"
            );
        }

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
        // submission named, one per input record — there for the read that first reported the
        // terminal status, however coarsely the client polled.
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

/// One row of the single-read table: how long the client waited before its ONE read, and the
/// status that wait owes the job.
///
/// The waits are the stages' own instants, each a little way into the stage's window, so the
/// rows together cover every status a job that has not finished carries.
fn single_read_rows() -> Vec<(String, Duration, &'static str)> {
    let mut rows = Vec::new();
    for (wait, expected) in [
        (Duration::ZERO, SUBMITTED),
        (validating_at() + INTO_THE_STAGE, VALIDATING),
        (scheduled_at() + INTO_THE_STAGE, SCHEDULED),
        (in_progress_at() + INTO_THE_STAGE, IN_PROGRESS),
    ] {
        rows.push((format!("one read {wait:?} after the submission"), wait, expected));
    }
    rows
}

#[tokio::test]
async fn a_stop_winds_down_the_job_the_elapsed_time_has_not_finished() {
    let emu = emulator();
    given_bucket(&emu).await;

    for (index, (case, wait, expected)) in single_read_rows().into_iter().enumerate() {
        for poller in [Poller::Read, Poller::List] {
            let case = format!("{case}, polled by {poller:?}");
            let (arn, _) = given_job(&emu, &format!("stop-{index}-{poller:?}")).await;

            // The client waits, then reads ONCE: the status it is answered is the one its own
            // wait owes the job, whichever operation it read with.
            emu.age_job(&arn, wait);
            let before = poll_once(&emu, &case, &arn, poller).await;
            assert_eq!(
                status_of(&case, &before),
                expected,
                "case {case:?}: a single read after {wait:?} must answer the status that wait \
                 owes the job, got {:?}",
                before.body
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
                "case {case:?}: a stop of the unfinished job must be accepted, got {} {:?}",
                stopped.status, stopped.body
            );

            // The stop landed on a job that had not finished, so the caller observes the
            // transient status and then the terminal one the stop owes.
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
            // polling it — including past the instant an unstopped job would have finished.
            for _ in 0..3 {
                emu.age_job(&arn, lifespan());
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
}
