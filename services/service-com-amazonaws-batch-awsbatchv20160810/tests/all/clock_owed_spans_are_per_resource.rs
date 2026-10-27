// SPDX-License-Identifier: CC0-1.0

//! WHEN a resource this service hands back in a non-terminal status settles, end to end
//! over the wire.
//!
//! The RULE under test: every transition this service's clock owes falls due one span
//! ([`state::JOB_PHASE_MILLIS`]) after the CALL THAT STARTED IT — that resource's own call
//! — and nothing materialises it earlier, so a read bracketed entirely before that instant
//! reports the transient status and a read bracketed entirely after it reports the settled
//! one. The span is therefore PER RESOURCE: one resource having settled is no evidence at
//! all about another whose own call was made at a different instant, and a case that
//! asserts a status it inferred from a sibling asserts nothing about the emulator — it
//! asserts how fast the machine under it happens to be.
//!
//! The rows span the resource KINDS whose status the clock owes a move out of (the span's
//! own doc names them: a compute environment or a job queue leaving `CREATING`, and each
//! move a submitted job makes) and, for each kind, the GAP between the calls that start a
//! PAIR of them: none at all, a quarter of a span, a whole span — by which time the first
//! has certainly settled while the second certainly has not, in one and the same sequence
//! of reads — and two spans. Every assertion is judged against the bracket of that
//! resource's OWN starting call, so a case asserts only what its own reads make certain.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    assert_clock_owed_status, bracketed, container_properties, describe_compute_environment,
    describe_job, describe_job_queue, job_definition, Asserted, Batch, Bracketed,
};

/// How many times [`until_settled`] looks before it gives up: each resource owes ONE move
/// here, so this is many spans of headroom on a slow machine.
const SETTLE_POLLS: usize = 240;

/// A resource kind whose non-terminal status the clock owes a move out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `CreateComputeEnvironment`: `CREATING` -> `VALID`.
    ComputeEnvironment,
    /// `CreateJobQueue`: `CREATING` -> `VALID`.
    JobQueue,
    /// `SubmitJob`: `SUBMITTED` -> `RUNNABLE`, where a job with nothing to place it on
    /// rests.
    Job,
}

/// Every kind the rows run over.
const KINDS: [Kind; 3] = [Kind::ComputeEnvironment, Kind::JobQueue, Kind::Job];

impl Kind {
    /// What a case calls this kind.
    fn label(self) -> &'static str {
        match self {
            Kind::ComputeEnvironment => "compute environment",
            Kind::JobQueue => "job queue",
            Kind::Job => "job",
        }
    }

    /// The prefix of the names this kind's resources are created under.
    fn prefix(self) -> &'static str {
        match self {
            Kind::ComputeEnvironment => "ce-span",
            Kind::JobQueue => "q-span",
            Kind::Job => "j-span",
        }
    }

    /// The status the starting call hands the resource back in.
    fn transient(self) -> &'static str {
        match self {
            Kind::ComputeEnvironment | Kind::JobQueue => state::STATUS_CREATING,
            Kind::Job => state::JOB_SUBMITTED,
        }
    }

    /// The status one span later, where this kind comes to rest.
    fn settled(self) -> &'static str {
        match self {
            Kind::ComputeEnvironment | Kind::JobQueue => state::STATUS_VALID,
            Kind::Job => state::JOB_RUNNABLE,
        }
    }
}

/// The emulator a row runs against, with the compute environment, queue and job definition
/// the started resources are built over.
struct Fixture {
    batch: Batch,
    /// The compute environment the row's job queues order.
    environment_arn: String,
    /// The queue the row's jobs are submitted to.
    queue_arn: String,
    /// The definition the row's jobs are submitted through.
    definition: String,
}

/// One emulator carrying a settled compute environment, a queue over it and a container
/// job definition.
async fn fixture() -> Fixture {
    let batch = Batch::new();
    let created = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-span-base", "type": "UNMANAGED", "state": "ENABLED"}),
        )
        .await;
    let environment_arn = created["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let queue = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "q-span-base",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment_arn}],
            }),
        )
        .await;
    let queue_arn = queue["jobQueueArn"].as_str().unwrap_or_default().to_string();
    let (definition, _) = job_definition(&batch, "jd-span", container_properties()).await;
    Fixture {
        batch,
        environment_arn,
        queue_arn,
        definition,
    }
}

/// One started resource: the bracket of the call that started its span, and the identifier
/// its reader resolves it by.
struct Started {
    case: String,
    kind: Kind,
    started: Bracketed<Value>,
    identifier: String,
}

/// Start one resource of `kind` called `name`, bracketing the call that starts its span.
async fn start(fixture: &Fixture, kind: Kind, case: String, name: &str) -> Started {
    let batch = &fixture.batch;
    let started = match kind {
        Kind::ComputeEnvironment => {
            bracketed(batch.ok(
                "/v1/createcomputeenvironment",
                json!({"computeEnvironmentName": name, "type": "UNMANAGED", "state": "ENABLED"}),
            ))
            .await
        }
        Kind::JobQueue => {
            bracketed(batch.ok(
                "/v1/createjobqueue",
                json!({
                    "jobQueueName": name,
                    "priority": 1,
                    "computeEnvironmentOrder": [
                        {"order": 1, "computeEnvironment": &fixture.environment_arn},
                    ],
                }),
            ))
            .await
        }
        Kind::Job => {
            bracketed(batch.ok(
                "/v1/submitjob",
                json!({
                    "jobName": name,
                    "jobQueue": &fixture.queue_arn,
                    "jobDefinition": &fixture.definition,
                }),
            ))
            .await
        }
    };
    // A compute environment and a queue are addressed by the name the request gave them; a
    // job by the id the submit minted.
    let identifier = match kind {
        Kind::Job => started.value["jobId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a jobId, got {}",
                    started.value
                )
            })
            .to_string(),
        _ => name.to_string(),
    };
    Started {
        case,
        kind,
        started,
        identifier,
    }
}

/// The one record this kind's reader reports for `identifier`.
async fn record(fixture: &Fixture, kind: Kind, identifier: &str) -> Value {
    match kind {
        Kind::ComputeEnvironment => describe_compute_environment(&fixture.batch, identifier).await,
        Kind::JobQueue => describe_job_queue(&fixture.batch, identifier).await,
        Kind::Job => describe_job(&fixture.batch, identifier).await,
    }
}

/// Read `resource` back and assert it reports the status its OWN span puts it in at the
/// instant of that read; answer which of the two the read's own bracket required.
async fn assert_where_its_own_span_puts_it(fixture: &Fixture, resource: &Started) -> Asserted {
    let read = bracketed(record(fixture, resource.kind, &resource.identifier)).await;
    assert_clock_owed_status(
        &resource.case,
        &read,
        resource.started.due_window(),
        resource.kind.transient(),
        Some(resource.kind.settled()),
    )
}

/// Poll `resource` until its reader reports the settled status, then answer that record;
/// panic naming the case and the last record seen if it never gets there.
async fn until_settled(fixture: &Fixture, resource: &Started) -> Value {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    for _ in 0..SETTLE_POLLS {
        let seen = record(fixture, resource.kind, &resource.identifier).await;
        if seen["status"].as_str() == Some(resource.kind.settled()) {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis((span / 10).max(5))).await;
    }
    let seen = record(fixture, resource.kind, &resource.identifier).await;
    panic!(
        "case {}: expected the resource to settle into {}, got {seen}",
        resource.case,
        resource.kind.settled()
    );
}

/// RULE: every clock-owed transition falls due one span after the call that started THAT
/// resource, so each resource reports the status its own span puts it in — the transient
/// one before its due instant, the settled one after — whatever any other resource of the
/// same kind is doing.
#[tokio::test]
async fn every_clock_owed_transition_falls_due_one_span_after_its_own_call() {
    let span = state::JOB_PHASE_MILLIS;
    // (case, how long after the first call of the pair the second one is made)
    let gaps: [(&str, i64); 4] = [
        ("started together", 0),
        ("a quarter span apart", span / 4),
        ("a span apart", span),
        ("two spans apart", span * 2),
    ];
    let fixture = fixture().await;

    let mut started: Vec<Started> = Vec::new();
    // What the reads' own brackets REQUIRED, so the table can be shown not to be vacuous.
    let mut required: Vec<Asserted> = Vec::new();
    for kind in KINDS {
        for (row, (gap, millis)) in gaps.iter().enumerate() {
            let first = start(
                &fixture,
                kind,
                format!("{} {gap}: the first", kind.label()),
                &format!("{}-{row}-a", kind.prefix()),
            )
            .await;
            if *millis > 0 {
                tokio::time::sleep(Duration::from_millis(u64::try_from(*millis).unwrap_or(500)))
                    .await;
            }
            let second = start(
                &fixture,
                kind,
                format!("{} {gap}: the second", kind.label()),
                &format!("{}-{row}-b", kind.prefix()),
            )
            .await;

            // Each of the pair is where ITS OWN span puts it, in one and the same sequence
            // of reads: for the rows a span or more apart the first has certainly settled
            // while the second certainly has not. Read in both orders, because a read moves
            // nothing — which of them is read first cannot decide what either answers.
            required.push(assert_where_its_own_span_puts_it(&fixture, &second).await);
            required.push(assert_where_its_own_span_puts_it(&fixture, &first).await);
            required.push(assert_where_its_own_span_puts_it(&fixture, &second).await);

            started.push(first);
            started.push(second);
        }
    }

    // The rows a span or more apart make both answers certain, so a run in which no read
    // required either of them asserted nothing at all about the emulator.
    for wanted in [Asserted::Transient, Asserted::Settled] {
        assert!(
            required.contains(&wanted),
            "case the table: no read's bracket required {wanted:?}, so the rows asserted \
             nothing; got {required:?}"
        );
    }

    // Every one of them settles on its own span and STAYS settled: the clock owes the move
    // once, and a read never owes it again.
    for resource in &started {
        let settled = until_settled(&fixture, resource).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(resource.kind.settled()),
            "case {}: expected the settled status, got {settled}",
            resource.case
        );
        let again = record(&fixture, resource.kind, &resource.identifier).await;
        assert_eq!(
            again["status"].as_str(),
            Some(resource.kind.settled()),
            "case {}: a settled resource stays settled from one read to the next, got {again}",
            resource.case
        );
    }
}
