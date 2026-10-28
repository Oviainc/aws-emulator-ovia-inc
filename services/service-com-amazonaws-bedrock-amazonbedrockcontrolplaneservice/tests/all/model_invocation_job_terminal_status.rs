// SPDX-License-Identifier: CC0-1.0

//! WHEN a submitted batch inference job becomes observably finished, asserted by POLLING the
//! emulator the way a client does — no record is settled by hand here, so what this suite
//! measures is the wall clock a caller actually waits through.
//!
//! THE RULE UNDER TEST: a resource this emulator answers in a non-terminal status is owed its
//! terminal status after a span of the emulator's own choosing, and that span is of the same
//! order as the other transitions of the same clock — a few seconds, reachable inside one
//! short polling pass — rather than a wall-clock wait of the size a caller budgets for the
//! whole interaction; and because it is the emulator's span rather than the real service's
//! queue, it does not grow with the work the resource was given. For a batch inference job
//! that is what makes the thing the job exists to produce observable: `GetModelInvocationJob`
//! reports `status` from `ModelInvocationJobStatus`, whose `InProgress` says "this job has
//! begun. You can start viewing the results in the output S3 location" and whose terminal
//! `Completed` says to "view the output files in the output S3 location"
//! (`smithy_query(query="doc",shape="ModelInvocationJobSummary",member="status")`), so a
//! client polling until the status is terminal must reach that status, the counts that
//! account for the records, and the results under the prefix `outputDataConfig` names.
//!
//! The rows differ along the one thing the rule generalises over: how many records the input
//! holds — one, a handful, tens and a hundred of them, which is two orders of magnitude of
//! work — plus which model family and invocation endpoint the records are invoked through, so
//! a row's time to a terminal status cannot come from the size of its input.
//!
//! The harness is the one `model_invocation_job_execution` drives (this service and this
//! emulator's own Amazon S3 on a single engine behind the production router), reused rather
//! than copied.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::model_invocation_job_execution::{
    count, emulator, given_bucket, given_input_object, job_id, llama_input, read_job, record,
    submit, BUCKET,
};
use crate::support::Reply;

/// The whole wall clock a caller gives a submitted job before it concludes the job hung.
///
/// An ABSOLUTE bound rather than one computed from this service's own spans: the point of the
/// rule is that the spans stay small enough for a client, a CI step or a workflow with a step
/// timeout to poll a job to its end, so a span that grew past a caller's budget has to fail
/// here even though every stored due instant would still be internally consistent. It is
/// generous next to the spans the service actually arms, which is room for a slow machine and
/// not room for another order of magnitude.
const POLLING_BUDGET: Duration = Duration::from_secs(15);

/// How long the poll loop waits between two reads of a job — short enough that no status the
/// clock carries a job through for its own span falls between two polls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How much longer the largest input of the table may take to reach a terminal status than
/// the smallest.
///
/// The rows are submitted together and polled in one pass, so what separates their terminal
/// instants is only the order they were submitted in and the in-process inference of the
/// extra records — room for a slow machine, and far less than the wait a span that grew with
/// the records would add over a hundred times the input.
const MAX_SPREAD: Duration = Duration::from_secs(2);

/// The terminal values of `ModelInvocationJobStatus`: the statuses a poll stops on, as the
/// modelled enum documents them — `Completed`, `PartiallyCompleted` and `Failed` end a run,
/// `Stopped` ends a job a user stopped and `Expired` one that timed out
/// (`smithy_query(query="shape",shape="ModelInvocationJobStatus")`).
const TERMINAL_STATUSES: &[&str] = &[
    "Completed",
    "PartiallyCompleted",
    "Failed",
    "Stopped",
    "Expired",
];

/// One row of the table: an input of its own size, invoked through its own endpoint.
struct Row {
    case: &'static str,
    /// `jobName`.
    name: &'static str,
    /// `modelId`: a model the published catalog carries and the inference surface serves.
    model_id: &'static str,
    /// `modelInvocationType`: the endpoint each record's `modelInput` is a body for.
    invocation_type: &'static str,
    /// The key the input object is staged under.
    input_key: &'static str,
    /// How many records that object holds.
    records: usize,
}

/// The table: the same job lifecycle over inputs two orders of magnitude apart in size.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single record",
            name: "batch-one-record",
            model_id: "meta.llama3-1-8b-instruct-v1:0",
            invocation_type: "InvokeModel",
            input_key: "input/one.jsonl",
            records: 1,
        },
        Row {
            case: "a handful of records",
            name: "batch-seven-records",
            model_id: "amazon.titan-text-lite-v1",
            invocation_type: "InvokeModel",
            input_key: "input/seven.jsonl",
            records: 7,
        },
        Row {
            case: "tens of records through the Converse endpoint",
            name: "batch-forty-records",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            invocation_type: "Converse",
            input_key: "input/forty.jsonl",
            records: 40,
        },
        Row {
            case: "a hundred records, a hundred times the first row's work",
            name: "batch-hundred-records",
            model_id: "meta.llama3-1-8b-instruct-v1:0",
            invocation_type: "InvokeModel",
            input_key: "input/hundred.jsonl",
            records: 100,
        },
    ]
}

/// The lines of an input object holding `records` records, each a JSON object carrying its own
/// `recordId` and the `modelInput` its endpoint serves.
fn lines_for(row: &Row) -> Vec<String> {
    (1..=row.records)
        .map(|index| {
            let record_id = format!("RECORD{index:05}");
            let prompt = format!("record {index}");
            let model_input = if row.invocation_type == "Converse" {
                serde_json::json!({
                    "messages": [{"role": "user", "content": [{"text": prompt}]}],
                    "inferenceConfig": {"maxTokens": 32},
                })
            } else if row.model_id.starts_with("amazon.titan") {
                serde_json::json!({"inputText": prompt})
            } else {
                llama_input(&prompt)
            };
            record(Some(&record_id), model_input)
        })
        .collect()
}

/// What polling one job observed: every distinct status it was read in, in order, how long
/// after the poll loop started it first answered a terminal one, and that answer.
struct Observed {
    statuses: Vec<String>,
    terminal_after: Option<Duration>,
    terminal_reply: Option<Reply>,
}

/// A client that submits a batch inference job and polls it reaches a terminal status, the
/// counts and the stored results inside one short polling pass, whatever its input holds.
#[tokio::test]
async fn a_polled_batch_inference_job_reaches_its_terminal_status_within_a_short_pass() {
    let emu = emulator();
    given_bucket(&emu).await;

    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for row in &rows {
        given_input_object(&emu, row.input_key, &lines_for(row)).await;
        arns.push(
            submit(
                &emu,
                row.name,
                row.model_id,
                row.invocation_type,
                &format!("s3://{BUCKET}/{}", row.input_key),
                &format!("s3://{BUCKET}/output"),
            )
            .await,
        );
    }

    // Every row is polled in the same pass, so no row waits through another row's lifecycle
    // and the time each one took is the time a client submitting that one job would wait.
    let started = Instant::now();
    let mut observed: Vec<Observed> = rows
        .iter()
        .map(|_| Observed {
            statuses: Vec::new(),
            terminal_after: None,
            terminal_reply: None,
        })
        .collect();
    loop {
        let mut pending: Vec<&str> = Vec::new();
        for (index, arn) in arns.iter().enumerate() {
            if observed[index].terminal_after.is_some() {
                continue;
            }
            let case = rows[index].case;
            let reply = read_job(&emu, arn).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a submitted job must stay readable while it runs, got {} {:?}",
                reply.status, reply.body
            );
            let status = reply.text("status");
            if observed[index].statuses.last() != Some(&status) {
                observed[index].statuses.push(status.clone());
            }
            if TERMINAL_STATUSES.contains(&status.as_str()) {
                observed[index].terminal_after = Some(started.elapsed());
                observed[index].terminal_reply = Some(reply);
            } else {
                pending.push(case);
            }
        }
        if pending.is_empty() {
            break;
        }
        let waited = started.elapsed();
        assert!(
            waited < POLLING_BUDGET,
            "cases {pending:?}: a submitted job must reach a terminal status inside a short \
             polling pass, but after {waited:?} of polling (budget {POLLING_BUDGET:?}) the \
             statuses observed were {:?}",
            observed
                .iter()
                .map(|seen| seen.statuses.clone())
                .collect::<Vec<_>>()
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let seen = &observed[index];
        let waited = seen
            .terminal_after
            .expect("the loop only ends once every row answered a terminal status");
        assert!(
            waited < POLLING_BUDGET,
            "case {case:?}: the terminal status must come inside the budget, got {waited:?} \
             after {POLLING_BUDGET:?}"
        );

        // The progression stayed OBSERVABLE: the first read of a submitted job answers a
        // non-terminal status, so no span collapsed to nothing, and the status that says the
        // job has begun is one the poll loop read it in rather than one it was carried past.
        let first = seen
            .statuses
            .first()
            .expect("a polled job was read at least once");
        assert!(
            !TERMINAL_STATUSES.contains(&first.as_str()),
            "case {case:?}: a job just submitted must still be read in a non-terminal status, \
             got {first:?}"
        );
        assert!(
            seen.statuses.iter().any(|status| status == "InProgress"),
            "case {case:?}: the running status must dwell long enough to be polled, got {:?}",
            seen.statuses
        );
        let last = seen
            .statuses
            .last()
            .expect("a polled job was read at least once");
        assert_eq!(
            last, "Completed",
            "case {case:?}: every record of this input can be invoked, so the job ends \
             Completed, got {last:?}"
        );
        let distinct: BTreeSet<&String> = seen.statuses.iter().collect();
        assert_eq!(
            distinct.len(),
            seen.statuses.len(),
            "case {case:?}: a job must not be read back in a status it already left, got {:?}",
            seen.statuses
        );

        // The counts the terminal read reports account for every record of the input.
        let reply = seen
            .terminal_reply
            .as_ref()
            .expect("the terminal read is kept with the row");
        let total = i64::try_from(row.records).expect("a small record count");
        for (member, expected) in [
            ("totalRecordCount", total),
            ("processedRecordCount", total),
            ("successRecordCount", total),
            ("errorRecordCount", 0),
        ] {
            let actual = count(reply, member);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: {member} must be {expected} for an input of {total} records, \
                 got {actual:?}"
            );
        }
        assert!(
            reply.get("endTime").is_string(),
            "case {case:?}: a job that has ended reports when it ended, got {:?}",
            reply.get("endTime")
        );

        // And the results are where the job was told to write them, one per input record.
        let input_name = row
            .input_key
            .rsplit('/')
            .next()
            .expect("the key has a last segment");
        let key = format!("output/{}/{input_name}.out", job_id(&arns[index]));
        let (status, body) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a client that polled the job to its end must find its results at \
             {BUCKET}/{key}, got {status}"
        );
        assert!(
            !body.is_empty(),
            "case {case:?}: the result object at {BUCKET}/{key} must not be empty"
        );
        let text = String::from_utf8(body.to_vec()).expect("the results are UTF-8 text");
        let results: Vec<Value> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("case {case:?}: {line:?} is not JSON: {error}"))
            })
            .collect();
        assert_eq!(
            results.len(),
            row.records,
            "case {case:?}: one result record per input record, expected {}, got {}",
            row.records,
            results.len()
        );
        for (position, result) in results.iter().enumerate() {
            let model_output = result.get("modelOutput");
            assert!(
                model_output.is_some_and(Value::is_object),
                "case {case:?}: result {position} must carry what the model answered, got \
                 {result:?}"
            );
        }
    }

    // The rule's own point: the wait does not scale with the work. The largest input is a
    // hundred times the smallest, so a wait that came from the records would be visibly
    // longer.
    let waits: Vec<Duration> = observed
        .iter()
        .map(|seen| seen.terminal_after.expect("every row finished"))
        .collect();
    let spread = waits
        .iter()
        .max()
        .expect("the table has rows")
        .saturating_sub(*waits.iter().min().expect("the table has rows"));
    assert!(
        spread < MAX_SPREAD,
        "the time to a terminal status must not scale with the input: the rows differ by \
         {spread:?}, past {MAX_SPREAD:?}, waits {waits:?}"
    );
}
