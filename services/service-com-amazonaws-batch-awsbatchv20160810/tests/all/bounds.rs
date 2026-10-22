// SPDX-License-Identifier: CC0-1.0

//! Everything a caller can make this service STORE is bounded.
//!
//! The rule under test: every store an unauthenticated caller fills carries a maximum —
//! the published AWS Batch quota where there is one, a bound this emulator states where
//! AWS publishes none (`state::limits`) — and a request past it answers the modelled
//! `ClientException` (HTTP 400) instead of growing the store. The jobs store answers one
//! step earlier: it first drops the jobs that have been terminal longest, which no later
//! call can move again, so a long run cannot accumulate records for the emulator's
//! lifetime.
//!
//! One row per store, differing in what grows: a per-Region resource keyed by name
//! (compute environments, job queues), the dependencies one submit may name, and the jobs
//! store, where the bound is kept by eviction rather than by refusal. (The job-definition
//! registry carries its own bound, `state::MAX_JOB_DEFINITIONS`, and its own tests in
//! `job_definition_registry.rs`.)

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::limits;

use crate::job_lifecycle::{container_properties, describe_job, Batch};

/// Every modelled `JobStatus`, so a count over `ListJobs` (which filters by exactly one
/// status) sees every stored job. Source: the model's `com.amazonaws.batch#JobStatus`
/// enum (`smithy_query(query="shape", shape="JobStatus")`).
const JOB_STATUSES: &[&str] = &[
    "SUBMITTED",
    "PENDING",
    "RUNNABLE",
    "STARTING",
    "RUNNING",
    "SUCCEEDED",
    "FAILED",
];

/// The stores a create fills.
#[derive(Debug, Clone, Copy)]
enum Store {
    /// One record per compute environment name.
    ComputeEnvironments,
    /// One record per job queue name.
    JobQueues,
}

/// Create the `index`th resource of `store`, answering the status and body.
async fn create(batch: &Batch, store: Store, index: usize) -> (u16, Value) {
    match store {
        Store::ComputeEnvironments => {
            batch
                .call(
                    "/v1/createcomputeenvironment",
                    json!({"computeEnvironmentName": format!("ce-{index}"), "type": "UNMANAGED"}),
                )
                .await
        }
        Store::JobQueues => {
            batch
                .call(
                    "/v1/createjobqueue",
                    json!({
                        "jobQueueName": format!("q-{index}"),
                        "priority": 1,
                        "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-shared"}],
                    }),
                )
                .await
        }
    }
}

#[tokio::test]
async fn every_store_a_create_fills_stops_at_its_stated_maximum() {
    for (store, maximum) in [
        (Store::ComputeEnvironments, limits::MAX_COMPUTE_ENVIRONMENTS),
        (Store::JobQueues, limits::MAX_JOB_QUEUES),
    ] {
        // One emulator per row: a maximum bounds one store, not the process.
        let batch = Batch::new();
        if matches!(store, Store::JobQueues) {
            batch
                .ok(
                    "/v1/createcomputeenvironment",
                    json!({"computeEnvironmentName": "ce-shared", "type": "UNMANAGED"}),
                )
                .await;
        }

        // Everything up to the maximum is accepted...
        for index in 0..maximum {
            let (status, body) = create(&batch, store, index).await;
            assert_eq!(
                status, 200,
                "case {store:?} create {index} of {maximum}: expected 200, got {status} {body}"
            );
        }

        // ...and the one past it is refused with the modelled 400, naming the maximum.
        let (status, body) = create(&batch, store, maximum).await;
        assert_eq!(
            status, 400,
            "case {store:?} create past {maximum}: expected the modelled ClientException 400, got {status} {body}"
        );
        assert!(
            body.to_string().contains(&maximum.to_string()),
            "case {store:?} create past {maximum}: the 400 must say what the maximum is, got {body}"
        );
    }
}

/// How many jobs the queue still holds, summed over every modelled `JobStatus`.
async fn stored_jobs(batch: &Batch, queue: &str) -> usize {
    let mut total = 0;
    for status in JOB_STATUSES {
        let body = batch
            .ok(
                "/v1/listjobs",
                json!({"jobQueue": queue, "jobStatus": status}),
            )
            .await;
        total += body["jobSummaryList"]
            .as_array()
            .map_or(0, |summaries| summaries.len());
    }
    total
}

/// Whether `job_id` is still stored: `DescribeJobs` reports a known id and silently omits
/// one it does not know.
async fn is_stored(batch: &Batch, job_id: &str) -> bool {
    let body = batch
        .ok("/v1/describejobs", json!({"jobs": [job_id]}))
        .await;
    body["jobs"]
        .as_array()
        .is_some_and(|details| !details.is_empty())
}

/// A compute environment, a queue over it and a container job definition, all named
/// `<prefix>`; answers the queue's ARN.
async fn queue_with_definition(batch: &Batch, prefix: &str) -> String {
    batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": format!("ce-{prefix}"), "type": "UNMANAGED"}),
        )
        .await;
    let queue = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": format!("q-{prefix}"),
                "priority": 1,
                "computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": format!("ce-{prefix}")},
                ],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": format!("jd-{prefix}"),
                "type": "container",
                "containerProperties": container_properties(),
            }),
        )
        .await;
    queue["jobQueueArn"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn one_submit_may_name_at_most_the_documented_number_of_dependencies() {
    let batch = Batch::new();
    let queue_arn = queue_with_definition(&batch, "deps").await;

    // One more real job than a submit may depend on, so the refusal is about the COUNT
    // and not about an id the store does not hold.
    let mut ids = Vec::with_capacity(limits::MAX_JOB_DEPENDENCIES + 1);
    for index in 0..=limits::MAX_JOB_DEPENDENCIES {
        let body = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("dep-{index}"),
                    "jobQueue": &queue_arn,
                    "jobDefinition": "jd-deps",
                }),
            )
            .await;
        ids.push(body["jobId"].as_str().unwrap_or_default().to_string());
    }
    let depends_on = |count: usize| -> Vec<Value> {
        ids.iter()
            .take(count)
            .map(|id| json!({"jobId": id}))
            .collect()
    };

    // Exactly the maximum is accepted...
    let (status, body) = batch
        .call(
            "/v1/submitjob",
            json!({
                "jobName": "at-the-maximum",
                "jobQueue": &queue_arn,
                "jobDefinition": "jd-deps",
                "dependsOn": depends_on(limits::MAX_JOB_DEPENDENCIES),
            }),
        )
        .await;
    assert_eq!(
        status,
        200,
        "case dependsOn with {} entries: expected 200, got {status} {body}",
        limits::MAX_JOB_DEPENDENCIES
    );

    // ...and one past it is refused with the modelled 400.
    let (status, body) = batch
        .call(
            "/v1/submitjob",
            json!({
                "jobName": "past-the-maximum",
                "jobQueue": &queue_arn,
                "jobDefinition": "jd-deps",
                "dependsOn": depends_on(limits::MAX_JOB_DEPENDENCIES + 1),
            }),
        )
        .await;
    assert_eq!(
        status,
        400,
        "case dependsOn with {} entries: expected the modelled ClientException 400, got {status} {body}",
        limits::MAX_JOB_DEPENDENCIES + 1
    );
    assert!(
        body.to_string()
            .contains(&limits::MAX_JOB_DEPENDENCIES.to_string()),
        "case dependsOn past the maximum: the 400 must say what the maximum is, got {body}"
    );
}

#[tokio::test]
async fn the_jobs_store_evicts_the_longest_terminal_job_instead_of_growing() {
    let batch = Batch::new();
    batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-jobs", "type": "UNMANAGED"}),
        )
        .await;
    batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "q-jobs",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-jobs"}],
            }),
        )
        .await;
    batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": "jd-jobs",
                "type": "container",
                "containerProperties": container_properties(),
            }),
        )
        .await;

    // Fill the store to its maximum, terminating each job as it is submitted so every
    // record is in a terminal status — the only kind eviction may drop.
    let mut ids = Vec::with_capacity(limits::MAX_JOBS);
    for index in 0..limits::MAX_JOBS {
        let body = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("j-{index}"),
                    "jobQueue": "q-jobs",
                    "jobDefinition": "jd-jobs",
                }),
            )
            .await;
        let id = body["jobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "case submit {index}: expected a job id, got {body}"
        );
        batch
            .ok(
                "/v1/terminatejob",
                json!({"jobId": id, "reason": "making room"}),
            )
            .await;
        ids.push(id);
        // The first two jobs stop in distinct milliseconds, so "terminal longest" has one
        // unambiguous answer whatever the rest of the loop costs.
        if index < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    assert_eq!(
        stored_jobs(&batch, "q-jobs").await,
        limits::MAX_JOBS,
        "case filled store: expected exactly the maximum, got a different count"
    );

    // One more submit is ACCEPTED: the store makes room rather than growing or refusing.
    let body = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "j-last",
                "jobQueue": "q-jobs",
                "jobDefinition": "jd-jobs",
            }),
        )
        .await;
    let newest = body["jobId"].as_str().unwrap_or_default().to_string();

    let stored = stored_jobs(&batch, "q-jobs").await;
    assert_eq!(
        stored,
        limits::MAX_JOBS,
        "case submit past the maximum: the store must stay at {}, got {stored}",
        limits::MAX_JOBS
    );
    let detail = describe_job(&batch, &newest).await;
    assert_eq!(
        detail["jobName"].as_str(),
        Some("j-last"),
        "case submit past the maximum: the newest job must be the one that is stored, got {detail}"
    );
    assert!(
        !is_stored(&batch, &ids[0]).await,
        "case submit past the maximum: the job terminal longest ({}) must be the one evicted",
        ids[0]
    );
    assert!(
        is_stored(&batch, &ids[1]).await,
        "case submit past the maximum: only the job terminal longest goes, but {} is gone too",
        ids[1]
    );
}
