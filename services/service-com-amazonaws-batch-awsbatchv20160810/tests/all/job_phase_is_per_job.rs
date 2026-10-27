// SPDX-License-Identifier: CC0-1.0

//! WHEN a submitted job leaves `SUBMITTED`, end to end over the wire.
//!
//! The RULE under test: the move out of the non-terminal status a submit stores is owed by
//! the CLOCK, and it falls due one span ([`state::JOB_PHASE_MILLIS`]) after the instant of
//! the call that started it — that job's own `createdAt`. So the span is per JOB: a read
//! before a job's own due instant reports the status its submit stored, a read after it
//! reports `RUNNABLE` (where a job with nothing to place it on rests), and one job having
//! got there is no evidence at all about another job submitted at a different instant.
//!
//! That is what a case asserting a job's status must respect: it has to OBSERVE the job it
//! talks about, not infer it from a sibling. `consumable_resource_release.rs` inferred it,
//! and so read a witness job back in `SUBMITTED` on a machine fast enough to finish the
//! release within the few milliseconds between the two submits.
//!
//! The rows span the gap between one submit and the next: two jobs submitted in the same
//! instant, one a quarter of a span later, one a whole span later (by which time the rows
//! before it are already resting) and one two spans later. Every read is bracketed by the
//! test's own reading of the same clock the emulator reads, so each case asserts the
//! status that bracket makes certain and nothing more: a read that straddles a job's due
//! instant may legitimately answer either status, and asserts nothing.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_job, job_definition, job_queue,
    until_status, Batch,
};

/// One emulator with a queue and a container job definition to submit into.
struct Fixture {
    batch: Batch,
    queue_arn: String,
    definition: String,
}

/// A fresh emulator carrying a compute environment, a queue over it and one container job
/// definition.
async fn fixture() -> Fixture {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-phase").await;
    let queue_arn = job_queue(&batch, "q-phase", &environment_arn).await;
    let (definition, _) = job_definition(&batch, "jd-phase", container_properties()).await;
    Fixture {
        batch,
        queue_arn,
        definition,
    }
}

/// Submit one job called `job_name`; answer its id.
async fn submit(fixture: &Fixture, job_name: &str) -> String {
    let answered = fixture
        .batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": job_name,
                "jobQueue": &fixture.queue_arn,
                "jobDefinition": &fixture.definition,
            }),
        )
        .await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {job_name}: expected a jobId, got {answered}"))
        .to_string()
}

/// One `DescribeJobs` read, bracketed by the instant the test read the clock before the
/// call and the instant it read it after: the emulator read its own clock somewhere
/// between the two, so `before <= the instant the answer describes <= after`.
struct Observation {
    detail: Value,
    before: i64,
    after: i64,
}

/// Read `job_id` back, bracketing the call.
async fn observe(batch: &Batch, job_id: &str) -> Observation {
    let before = state::now_millis();
    let detail = describe_job(batch, job_id).await;
    let after = state::now_millis();
    Observation {
        detail,
        before,
        after,
    }
}

/// The `createdAt` of a read job: the instant its own span starts.
fn created_at(case: &str, detail: &Value) -> i64 {
    detail["createdAt"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: a stored job carries createdAt, got {detail}"))
}

/// Assert that `observation` reports the job where its OWN span puts it: still `SUBMITTED`
/// when the read is certainly before its due instant, and `RUNNABLE` when the read is
/// certainly after it.
fn assert_phase(case: &str, observation: &Observation, created_at: i64) {
    let due = created_at + state::JOB_PHASE_MILLIS;
    let status = observation.detail["status"].as_str();
    let detail = &observation.detail;
    if observation.after < due {
        assert_eq!(
            status,
            Some(state::JOB_SUBMITTED),
            "case {case}: a job read {} ms before its own span elapsed has not moved yet, \
             got {detail}",
            due - observation.after
        );
    } else if observation.before >= due {
        assert_eq!(
            status,
            Some(state::JOB_RUNNABLE),
            "case {case}: a job read {} ms after its own span elapsed rests in RUNNABLE, got \
             {detail}",
            observation.before - due
        );
    }
}

/// RULE: each job's clock-owed move out of `SUBMITTED` falls due one span after THAT job's
/// own `createdAt`, so the jobs of one queue are each exactly where their own span puts
/// them — one of them resting in `RUNNABLE` says nothing about another submitted at a
/// different instant.
#[tokio::test]
async fn every_job_leaves_submitted_one_span_after_its_own_submit() {
    let span = state::JOB_PHASE_MILLIS;
    // (case, how long after the row before it this job is submitted)
    let rows: [(&str, i64); 5] = [
        ("together-a", 0),
        ("together-b", 0),
        ("a-quarter-span-later", span / 4),
        ("a-span-later", span),
        ("two-spans-later", span * 2),
    ];
    let fixture = fixture().await;

    let mut submitted: Vec<(&str, String, i64)> = Vec::new();
    for (case, gap) in rows {
        if gap > 0 {
            tokio::time::sleep(Duration::from_millis(u64::try_from(gap).unwrap_or(500))).await;
        }
        let job_id = submit(&fixture, case).await;
        let observation = observe(&fixture.batch, &job_id).await;
        let created = created_at(case, &observation.detail);
        assert_phase(case, &observation, created);
        submitted.push((case, job_id, created));

        // Every job the store already holds is read again: each is where ITS OWN span puts
        // it, which is the whole rule — the rows submitted earlier may have left
        // `SUBMITTED` while the newest has not, in the very same sequence of reads.
        for (earlier, earlier_id, earlier_created) in &submitted {
            let observation = observe(&fixture.batch, earlier_id).await;
            assert_eq!(
                created_at(earlier, &observation.detail),
                *earlier_created,
                "case {earlier}: the instant a job's span starts does not change from one \
                 read to the next, got {}",
                observation.detail
            );
            assert_phase(earlier, &observation, *earlier_created);
        }
    }

    // Every one of them gets there on its own span and then RESTS: nothing is reported for
    // work nothing ran.
    for (case, job_id, created) in &submitted {
        let detail = until_status(&fixture.batch, case, job_id, state::JOB_RUNNABLE).await;
        let at = state::now_millis();
        assert!(
            at >= created + span,
            "case {case}: a job may not be reported RUNNABLE before its own span elapses \
             (due at {}), observed at {at}",
            created + span
        );
        for member in ["startedAt", "stoppedAt", "statusReason"] {
            assert_eq!(
                detail[member],
                Value::Null,
                "case {case}: a job nothing placed reports no {member}, got {detail}"
            );
        }
    }
}
