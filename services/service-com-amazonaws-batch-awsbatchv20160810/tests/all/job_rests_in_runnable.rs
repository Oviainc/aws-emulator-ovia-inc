// SPDX-License-Identifier: CC0-1.0

//! Where a submitted AWS Batch job COMES TO REST, end to end over the wire.
//!
//! The rule under test: **a job AWS Batch has accepted leaves `SUBMITTED` on the clock and
//! then waits to be placed on compute capacity — `RUNNABLE` when its dependencies are
//! satisfied, `PENDING` while they are not — and it leaves that status only when a
//! scheduler places it or a caller ends it; with no capacity to place it on it RESTS there
//! indefinitely, reporting no `startedAt`, no `stoppedAt` and no attempt, and never
//! becoming terminal of its own accord.**
//!
//! Nothing in this emulator places a job: there is no container runtime, and the engine
//! contract forbids reporting a status, duration or exit code for work nothing ran. So
//! every job here rests, and these suites assert that over a SET of inputs — the number of
//! jobs in the queue, whether a job has a dependency and whether that dependency has
//! completed, and which call a caller ends the job with (`CancelJob`, `CancelJobs`,
//! `TerminateJob`, `TerminateJobs`) — together with what the queue's own snapshot and the
//! status-filtered `ListJobs` then report about a queue full of resting jobs.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue, Batch,
};

/// One emulator with a queue and a job definition to submit into.
struct Fixture {
    batch: Batch,
    queue_arn: String,
    queue_name: String,
    definition: String,
}

/// A fresh emulator carrying a compute environment, a queue over it and one container job
/// definition, all named after `prefix`.
async fn fixture(prefix: &str) -> Fixture {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, &format!("ce-{prefix}")).await;
    let queue_name = format!("q-{prefix}");
    let queue_arn = job_queue(&batch, &queue_name, &environment_arn).await;
    let (definition, _) =
        job_definition(&batch, &format!("jd-{prefix}"), container_properties()).await;
    Fixture {
        batch,
        queue_arn,
        queue_name,
        definition,
    }
}

/// Submit one job called `name`, optionally depending on other jobs; answer its id.
async fn submit(fixture: &Fixture, name: &str, depends_on: &[&str]) -> String {
    let mut body = json!({
        "jobName": name,
        "jobQueue": &fixture.queue_arn,
        "jobDefinition": &fixture.definition,
    });
    if !depends_on.is_empty() {
        body["dependsOn"] = Value::Array(
            depends_on
                .iter()
                .map(|id| json!({"jobId": id, "type": "SEQUENTIAL"}))
                .collect(),
        );
    }
    let answered = fixture.batch.ok("/v1/submitjob", body).await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {answered}"))
        .to_string()
}

/// Poll `job_id` until it reports `status`, and answer its last `JobDetail`. Driven off
/// OBSERVED state rather than a fixed sleep, so a slow machine delays a case instead of
/// breaking it.
async fn poll_until_status(batch: &Batch, job_id: &str, status: &str) -> Value {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut detail = describe_job(batch, job_id).await;
    for _ in 0..60 {
        if detail["status"].as_str() == Some(status) {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis((span / 20).max(1))).await;
        detail = describe_job(batch, job_id).await;
    }
    detail
}

/// Assert that `detail` reports a job nothing ran: no `startedAt`, no `stoppedAt` and no
/// attempt, whatever status it is in.
fn assert_nothing_ran(case: &str, detail: &Value) {
    for member in ["startedAt", "stoppedAt"] {
        assert_eq!(
            detail[member],
            Value::Null,
            "case {case}: a job nothing ran reports no {member}, got {detail}"
        );
    }
    assert!(
        detail["attempts"].is_null(),
        "case {case}: no attempt is invented for a job nothing placed, got {detail}"
    );
    assert!(
        detail["container"]["exitCode"].is_null(),
        "case {case}: no exit code is invented, got {detail}"
    );
    assert!(
        detail["createdAt"].as_i64().unwrap_or_default() > 0,
        "case {case}: a stored job carries createdAt, got {detail}"
    );
}

/// How a row's jobs depend on each other, which is what decides where each of them rests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Independent jobs: each rests in `RUNNABLE`.
    Independent,
    /// A chain: the first rests in `RUNNABLE` and each later one waits in `PENDING` behind
    /// the one before it.
    Chain,
}

/// RULE: every job a queue accepts comes to rest — `RUNNABLE` with no outstanding
/// dependency, `PENDING` with one — and STAYS there over further spans of the clock, with
/// nothing reported for work nothing ran.
///
/// Rows differ along how many jobs the queue holds and how they depend on each other.
#[tokio::test]
async fn an_accepted_job_comes_to_rest_and_stays_there() {
    // (case, how many jobs, how they depend on each other)
    let rows: &[(&str, usize, Shape)] = &[
        ("one independent job", 1, Shape::Independent),
        ("three independent jobs", 3, Shape::Independent),
        ("a chain of two", 2, Shape::Chain),
        ("a chain of four", 4, Shape::Chain),
    ];

    for (index, (case, count, shape)) in rows.iter().enumerate() {
        let fixture = fixture(&format!("rest{index}")).await;
        let mut ids: Vec<String> = Vec::new();
        for position in 0..*count {
            let depends: Vec<&str> = match (shape, ids.last()) {
                (Shape::Chain, Some(previous)) => vec![previous.as_str()],
                _ => Vec::new(),
            };
            ids.push(submit(&fixture, &format!("j{position}"), &depends).await);
        }

        // Where each job must come to rest: RUNNABLE unless something ahead of it in the
        // chain has not completed.
        let resting: Vec<&str> = ids
            .iter()
            .enumerate()
            .map(|(position, _)| {
                if *shape == Shape::Chain && position > 0 {
                    state::JOB_PENDING
                } else {
                    state::JOB_RUNNABLE
                }
            })
            .collect();

        for (position, id) in ids.iter().enumerate() {
            let detail = poll_until_status(&fixture.batch, id, resting[position]).await;
            assert_eq!(
                detail["status"].as_str(),
                Some(resting[position]),
                "case {case}: job {position} must come to rest in {}, got {detail}",
                resting[position]
            );
            assert_nothing_ran(case, &detail);
        }

        // RESTING means staying: several further spans of the clock move none of them, and
        // no reason is invented for a job that simply has nowhere to go.
        let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
        tokio::time::sleep(Duration::from_millis(span * 4)).await;
        for (position, id) in ids.iter().enumerate() {
            let detail = describe_job(&fixture.batch, id).await;
            assert_eq!(
                detail["status"].as_str(),
                Some(resting[position]),
                "case {case}: job {position} must still be {} after four more spans, got \
                 {detail}",
                resting[position]
            );
            assert_eq!(
                detail["statusReason"],
                Value::Null,
                "case {case}: a resting job reports no statusReason, got {detail}"
            );
            assert_nothing_ran(case, &detail);
        }
    }
}

/// RULE: a queue of resting jobs is what the queue's own reports describe — its snapshot
/// lists the `RUNNABLE` jobs in the order it would dispatch them (submission order), each
/// with the instant it reached its position, and `ListJobs` reports them under the
/// `RUNNABLE` filter while its DEFAULT selects only `RUNNING` jobs, of which there are
/// none.
#[tokio::test]
async fn a_queue_of_resting_jobs_is_reported_by_the_snapshot_and_by_list_jobs() {
    let fixture = fixture("snapshot").await;
    let mut ids: Vec<String> = Vec::new();
    for position in 0..3 {
        ids.push(submit(&fixture, &format!("s{position}"), &[]).await);
    }
    for id in &ids {
        poll_until_status(&fixture.batch, id, state::JOB_RUNNABLE).await;
    }
    let arns: Vec<String> = {
        let mut arns = Vec::new();
        for id in &ids {
            let detail = describe_job(&fixture.batch, id).await;
            arns.push(detail["jobArn"].as_str().unwrap_or_default().to_string());
        }
        arns
    };

    // The snapshot, addressed by the queue's NAME and by its ARN — both of which the
    // member documents ("the job queue's name or full queue Amazon Resource Name (ARN)").
    for identifier in [fixture.queue_name.clone(), fixture.queue_arn.clone()] {
        let snapshot = fixture
            .batch
            .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": identifier}))
            .await;
        let listed: Vec<String> = snapshot["frontOfQueue"]["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|job| job["jobArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed, arns,
            "case {identifier}: expected the snapshot to list the queue in dispatch order, \
             got {snapshot}"
        );
        for (position, job) in snapshot["frontOfQueue"]["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            assert!(
                job["earliestTimeAtPosition"].as_i64().unwrap_or_default() > 0,
                "case {identifier}: job {position} must report when it reached its position, \
                 got {job}"
            );
        }
        // Nothing is dispatched, so no capacity is in use.
        assert_eq!(
            snapshot["queueUtilization"]["totalCapacityUsage"],
            json!([]),
            "case {identifier}: expected no capacity in use, got {snapshot}"
        );
        // A read changes nothing a later caller sees.
        let again = fixture
            .batch
            .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": identifier}))
            .await;
        assert_eq!(
            again["frontOfQueue"]["jobs"], snapshot["frontOfQueue"]["jobs"],
            "case {identifier}: a second snapshot must report the same queue, got {again}"
        );
    }

    // `ListJobs$jobStatus`: "If you don't specify a status, only RUNNING jobs are
    // returned." Nothing here runs, so the default selects nothing while the RUNNABLE
    // filter selects the whole queue.
    let defaulted = fixture
        .batch
        .ok("/v1/listjobs", json!({"jobQueue": &fixture.queue_arn}))
        .await;
    assert_eq!(
        defaulted["jobSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case the default filter: nothing is RUNNING, got {defaulted}"
    );
    let runnable = fixture
        .batch
        .ok(
            "/v1/listjobs",
            json!({"jobQueue": &fixture.queue_arn, "jobStatus": state::JOB_RUNNABLE}),
        )
        .await;
    let selected: Vec<String> = runnable["jobSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect();
    for id in &ids {
        assert!(
            selected.contains(id),
            "case the RUNNABLE filter: expected {id} in {selected:?}, got {runnable}"
        );
    }
}

/// Which call a row ends its resting jobs with.
#[derive(Debug, Clone, Copy)]
struct Ending {
    case: &'static str,
    /// The URI the call is bound to.
    uri: &'static str,
    /// Whether the call takes a LIST of job ids (the bulk form) or one id.
    bulk: bool,
    /// Whether the ended job reports `isTerminated` rather than `isCancelled`: a job that
    /// had not progressed past `RUNNABLE` is CANCELLED by either call, so neither of these
    /// rows terminates one.
    terminated: bool,
}

const ENDINGS: &[Ending] = &[
    Ending {
        case: "CancelJob, one id at a time",
        uri: "/v1/canceljob",
        bulk: false,
        terminated: false,
    },
    Ending {
        case: "TerminateJob, one id at a time",
        uri: "/v1/terminatejob",
        bulk: false,
        terminated: false,
    },
    Ending {
        case: "CancelJobs, the whole queue at once",
        uri: "/v1/canceljobs",
        bulk: true,
        terminated: false,
    },
    Ending {
        case: "TerminateJobs, the whole queue at once",
        uri: "/v1/terminatejobs",
        bulk: true,
        terminated: false,
    },
];

/// RULE: a resting job leaves `RUNNABLE` only when a CALLER ends it, and then it ends
/// `FAILED` carrying that caller's reason — still with no `startedAt` and no `stoppedAt`,
/// those being the `STARTING` -> `RUNNING` and `RUNNING` -> terminal transitions a job
/// nothing placed never made. The bulk forms report every id they were given as
/// successful, including one no job exists for ("Jobs that can't be found are treated as
/// successfully processed").
#[tokio::test]
async fn only_a_caller_takes_a_resting_job_terminal() {
    const REASON: &str = "end the resting jobs";
    /// A job id of the form this service mints that no job was ever created under.
    const UNKNOWN: &str = "00000000-0000-4000-8000-000000000000";

    for (index, ending) in ENDINGS.iter().enumerate() {
        let case = ending.case;
        let fixture = fixture(&format!("end{index}")).await;
        let mut ids: Vec<String> = Vec::new();
        for position in 0..2 {
            ids.push(submit(&fixture, &format!("e{position}"), &[]).await);
        }
        for id in &ids {
            poll_until_status(&fixture.batch, id, state::JOB_RUNNABLE).await;
        }

        if ending.bulk {
            let mut requested: Vec<String> = ids.clone();
            requested.push(UNKNOWN.to_string());
            let answered = fixture
                .batch
                .ok(
                    ending.uri,
                    json!({"jobs": &requested, "reason": REASON}),
                )
                .await;
            let successful: Vec<String> = answered["successful"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                successful, requested,
                "case {case}: every requested id is reported as processed, got {answered}"
            );
            assert_eq!(
                answered["errors"],
                json!([]),
                "case {case}: nothing the service could address failed, got {answered}"
            );
        } else {
            for id in &ids {
                let (status, answered) = fixture
                    .batch
                    .call(ending.uri, json!({"jobId": id, "reason": REASON}))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected HTTP 200, got {status} with {answered}"
                );
            }
        }

        for (position, id) in ids.iter().enumerate() {
            let detail = describe_job(&fixture.batch, id).await;
            assert_eq!(
                detail["status"].as_str(),
                Some(state::JOB_FAILED),
                "case {case}: job {position} must end FAILED, got {detail}"
            );
            assert_eq!(
                detail["statusReason"].as_str(),
                Some(REASON),
                "case {case}: the caller's reason is what the job records, got {detail}"
            );
            assert_eq!(
                detail["isTerminated"].as_bool(),
                Some(ending.terminated),
                "case {case}: a job that had not progressed past RUNNABLE is cancelled, got \
                 {detail}"
            );
            assert_eq!(
                detail["isCancelled"].as_bool(),
                Some(!ending.terminated),
                "case {case}: expected isCancelled, got {detail}"
            );
            assert_nothing_ran(case, &detail);
        }

        // A terminal job is FROZEN: repeating the same call changes nothing at all.
        let before = describe_job(&fixture.batch, &ids[0]).await;
        if ending.bulk {
            fixture
                .batch
                .ok(
                    ending.uri,
                    json!({"jobs": [&ids[0]], "reason": "a second reason"}),
                )
                .await;
        } else {
            fixture
                .batch
                .ok(
                    ending.uri,
                    json!({"jobId": &ids[0], "reason": "a second reason"}),
                )
                .await;
        }
        let after = describe_job(&fixture.batch, &ids[0]).await;
        assert_eq!(
            after, before,
            "case {case}: a terminal job is left exactly as it is, got {after}"
        );

        // And the queue's snapshot reports an empty front of queue once nothing rests in
        // it any more.
        let snapshot = fixture
            .batch
            .ok(
                "/v1/getjobqueuesnapshot",
                json!({"jobQueue": &fixture.queue_arn}),
            )
            .await;
        assert_eq!(
            snapshot["frontOfQueue"]["jobs"],
            json!([]),
            "case {case}: no job rests in the queue any more, got {snapshot}"
        );
    }
}
