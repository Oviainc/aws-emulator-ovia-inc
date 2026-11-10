// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `BatchGetJob`.
//!
//! The rule under test: a batch read reports the outcome of EVERY requested identifier
//! independently, in the order the request listed them — an identifier naming a job the
//! service holds yields an item carrying exactly what the single-resource read reports,
//! and an identifier naming a resource the service does not hold yields an error entry
//! echoing that identifier — and the call itself still answers HTTP 200.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` already drives the service with,
//! so these tests go through routing, deserialization, constraint validation, the handler
//! and serialization, exactly as the router does in production.

use std::time::Duration;

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};

/// The path `BatchGetJob` is bound to, from
/// `smithy_query(query="operation", operation="BatchGetJob")`: `POST /2023-10-12/batch-get-job`.
const BATCH_GET_JOB: &str = "/2023-10-12/batch-get-job";

/// What the service is expected to report for one requested identifier.
#[derive(Debug, Clone)]
enum Expect {
    /// The identifier names a held job, which comes back as an item under `jobs`.
    Job(&'static str),
    /// The identifier names a resource the service does not hold at the given level of
    /// `farm -> queue -> job`, which comes back as an entry under `errors`.
    Missing {
        resource_type: &'static str,
        resource_id: String,
    },
}

fn identifier(farm_id: &str, queue_id: &str, job_id: &str) -> Value {
    json!({"farmId": farm_id, "queueId": queue_id, "jobId": job_id})
}

fn array_at(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Call `BatchGetJob` with the identifiers of `rows` and assert each one's expected
/// outcome, in order.
async fn assert_batch(emulator: &Emulator, case: &str, rows: &[(Value, Expect)]) {
    let identifiers: Vec<Value> = rows.iter().map(|(id, _)| id.clone()).collect();
    let (status, body) = emulator
        .post(BATCH_GET_JOB, json!({"identifiers": identifiers}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected BatchGetJob to answer 200, got {status} with {body}"
    );

    let jobs = array_at(&body, "jobs");
    let errors = array_at(&body, "errors");
    assert_eq!(
        jobs.len() + errors.len(),
        rows.len(),
        "case {case:?}: expected one result per identifier ({}), got {} jobs and {} errors in {body}",
        rows.len(),
        jobs.len(),
        errors.len()
    );

    let mut next_job = 0usize;
    let mut next_error = 0usize;
    for (identifier, expect) in rows {
        match expect {
            Expect::Job(name) => {
                let Some(item) = jobs.get(next_job) else {
                    panic!("case {case:?}: expected a job at index {next_job}, got {body}");
                };
                for member in ["farmId", "queueId", "jobId"] {
                    assert_eq!(
                        item.get(member),
                        identifier.get(member),
                        "case {case:?}: job at index {next_job} reports another identifier's \
                         {member}, got {item}"
                    );
                }
                assert_eq!(
                    string_at(item, "name"),
                    *name,
                    "case {case:?}: expected the job named {name:?} at index {next_job}, \
                     got {item}"
                );
                next_job += 1;
            }
            Expect::Missing {
                resource_type,
                resource_id,
            } => {
                let Some(entry) = errors.get(next_error) else {
                    panic!("case {case:?}: expected an error at index {next_error}, got {body}");
                };
                for member in ["farmId", "queueId", "jobId"] {
                    assert_eq!(
                        entry.get(member),
                        identifier.get(member),
                        "case {case:?}: error at index {next_error} echoes another \
                         identifier's {member}, got {entry}"
                    );
                }
                assert_eq!(
                    string_at(entry, "code"),
                    "ResourceNotFoundException",
                    "case {case:?}: expected the missing-resource code at index \
                     {next_error}, got {entry}"
                );
                let expected =
                    format!("Resource of type {resource_type} with id {resource_id} does not exist.");
                assert_eq!(
                    string_at(entry, "message"),
                    expected,
                    "case {case:?}: expected the message {expected:?} at index \
                     {next_error}, got {entry}"
                );
                next_error += 1;
            }
        }
    }
}

#[tokio::test]
async fn batch_get_job_reports_one_result_per_identifier() {
    let emulator = Emulator::new();
    let farm = emulator.farm("batch").await;
    let queue = emulator.queue(&farm, "batch").await;
    let alpha = emulator.job(&farm, &queue, "alpha", 10).await;
    let beta = emulator.job(&farm, &queue, "beta", 50).await;
    let gamma = emulator.job(&farm, &queue, "gamma", 100).await;
    // A second hierarchy, so "held, but not under the farm and queue you asked with" is
    // one of the rows rather than an untested corner.
    let other_farm = emulator.farm("batch-other").await;
    let other_queue = emulator.queue(&other_farm, "batch-other").await;
    let delta = emulator.job(&other_farm, &other_queue, "delta", 7).await;

    let no_farm = absent_id("farm", "00000000000000000000000000000001");
    let no_queue = absent_id("queue", "00000000000000000000000000000002");
    let no_job = absent_id("job", "00000000000000000000000000000003");

    // The rows differ along what the rule generalises over: how many identifiers, which
    // of them the service holds, which level of the hierarchy is missing, how the found
    // and the missing are interleaved, and whether an identifier repeats.
    let cases: Vec<(&str, Vec<(Value, Expect)>)> = vec![
        (
            "a single held job",
            vec![(identifier(&farm, &queue, &alpha), Expect::Job("alpha"))],
        ),
        (
            "every job of one queue, answered in request order",
            vec![
                (identifier(&farm, &queue, &gamma), Expect::Job("gamma")),
                (identifier(&farm, &queue, &alpha), Expect::Job("alpha")),
                (identifier(&farm, &queue, &beta), Expect::Job("beta")),
            ],
        ),
        (
            "jobs of two different farms in one request",
            vec![
                (identifier(&farm, &queue, &beta), Expect::Job("beta")),
                (
                    identifier(&other_farm, &other_queue, &delta),
                    Expect::Job("delta"),
                ),
            ],
        ),
        (
            "the same identifier twice",
            vec![
                (identifier(&farm, &queue, &alpha), Expect::Job("alpha")),
                (identifier(&farm, &queue, &alpha), Expect::Job("alpha")),
            ],
        ),
        (
            "partial failure: held, absent job, held",
            vec![
                (identifier(&farm, &queue, &alpha), Expect::Job("alpha")),
                (
                    identifier(&farm, &queue, &no_job),
                    Expect::Missing {
                        resource_type: "job",
                        resource_id: no_job.clone(),
                    },
                ),
                (identifier(&farm, &queue, &gamma), Expect::Job("gamma")),
            ],
        ),
        (
            "an absent queue under a held farm",
            vec![(
                identifier(&farm, &no_queue, &alpha),
                Expect::Missing {
                    resource_type: "queue",
                    resource_id: no_queue.clone(),
                },
            )],
        ),
        (
            "an absent farm",
            vec![(
                identifier(&no_farm, &queue, &alpha),
                Expect::Missing {
                    resource_type: "farm",
                    resource_id: no_farm.clone(),
                },
            )],
        ),
        (
            "a queue that belongs to another farm",
            vec![(
                identifier(&farm, &other_queue, &delta),
                Expect::Missing {
                    resource_type: "queue",
                    resource_id: other_queue.clone(),
                },
            )],
        ),
        (
            "a job that belongs to another queue",
            vec![(
                identifier(&farm, &queue, &delta),
                Expect::Missing {
                    resource_type: "job",
                    resource_id: delta.clone(),
                },
            )],
        ),
        (
            "every identifier absent, one per level",
            vec![
                (
                    identifier(&no_farm, &no_queue, &no_job),
                    Expect::Missing {
                        resource_type: "farm",
                        resource_id: no_farm.clone(),
                    },
                ),
                (
                    identifier(&farm, &no_queue, &no_job),
                    Expect::Missing {
                        resource_type: "queue",
                        resource_id: no_queue.clone(),
                    },
                ),
                (
                    identifier(&farm, &queue, &no_job),
                    Expect::Missing {
                        resource_type: "job",
                        resource_id: no_job.clone(),
                    },
                ),
            ],
        ),
    ];

    for (case, rows) in cases {
        assert_batch(&emulator, case, &rows).await;
    }
}

#[tokio::test]
async fn batch_get_job_honours_the_modeled_identifier_count() {
    let emulator = Emulator::new();
    let farm = emulator.farm("counts").await;
    let queue = emulator.queue(&farm, "counts").await;
    let job = emulator.job(&farm, &queue, "counted", 50).await;

    // `identifiers` is `len 1..100`, from
    // `smithy_query(query="operation", operation="BatchGetJob")`: both published bounds
    // are served, and one past each is refused by the boundary validator.
    let one = identifier(&farm, &queue, &job);
    for count in [1usize, 100] {
        let identifiers = vec![one.clone(); count];
        let (status, body) = emulator
            .post(BATCH_GET_JOB, json!({"identifiers": identifiers}))
            .await;
        assert_eq!(
            status, 200,
            "case {count} identifiers: expected 200, got {status} with {body}"
        );
        assert_eq!(
            array_at(&body, "jobs").len(),
            count,
            "case {count} identifiers: expected one job per identifier, got {body}"
        );
        assert!(
            array_at(&body, "errors").is_empty(),
            "case {count} identifiers: expected no errors for held jobs, got {body}"
        );
    }

    for count in [0usize, 101] {
        let identifiers = vec![one.clone(); count];
        let (status, body) = emulator
            .post(BATCH_GET_JOB, json!({"identifiers": identifiers}))
            .await;
        assert_eq!(
            status, 400,
            "case {count} identifiers: expected the modeled length refusal, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {count} identifiers: expected a ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_batch_item_reports_what_the_single_job_read_reports() {
    let emulator = Emulator::new();
    let farm = emulator.farm("agreement").await;
    let queue = emulator.queue(&farm, "agreement").await;

    // Rows differ along the members a job can carry, so the agreement is asserted over a
    // set of jobs rather than one shape of job.
    let requests: Vec<(&str, Value)> = vec![
        (
            "minimal",
            json!({"priority": 50, "nameOverride": "minimal"}),
        ),
        (
            "described and bounded",
            json!({
                "priority": 0,
                "nameOverride": "bounded",
                "descriptionOverride": "a described job",
                "maxFailedTasksCount": 0,
                "maxRetriesPerTask": 11,
                "maxWorkerCount": -1,
                "targetTaskRunStatus": "SUSPENDED",
            }),
        ),
        (
            "at the top priority",
            json!({"priority": 100, "nameOverride": "urgent"}),
        ),
    ];

    for (case, request) in requests {
        let (status, created) = emulator
            .post(
                &format!("/2023-10-12/farms/{farm}/queues/{queue}/jobs"),
                request,
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateJob to answer 201, got {status} with {created}"
        );
        let job_id = string_at(&created, "jobId");
        let one = identifier(&farm, &queue, &job_id);

        // The batch read settles the clock-owed lifecycle transition exactly as the
        // single read does: before the span it still reports the non-terminal status.
        let (_, body) = emulator
            .post(BATCH_GET_JOB, json!({"identifiers": [one.clone()]}))
            .await;
        let item = array_at(&body, "jobs").first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_at(&item, "lifecycleStatus"),
            "CREATE_IN_PROGRESS",
            "case {case:?}: immediately after CreateJob, expected the non-terminal status, \
             got {item}"
        );

        tokio::time::sleep(Duration::from_millis(900)).await;
        let (_, body) = emulator
            .post(BATCH_GET_JOB, json!({"identifiers": [one.clone()]}))
            .await;
        let item = array_at(&body, "jobs").first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_at(&item, "lifecycleStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: after the create span, expected the settled status, got {item}"
        );

        // Every member GetJob reports, the batch item reports identically.
        let (status, single) = emulator
            .get(&format!(
                "/2023-10-12/farms/{farm}/queues/{queue}/jobs/{job_id}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetJob to answer 200, got {status} with {single}"
        );
        let members = single
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a GetJob object, got {single}"));
        for (member, value) in members {
            assert_eq!(
                item.get(member),
                Some(value),
                "case {case:?}: expected the batch item's {member} to match GetJob's \
                 {value}, got {item}"
            );
        }

        // A read does not change what the next reader sees.
        let (_, again) = emulator
            .post(BATCH_GET_JOB, json!({"identifiers": [one]}))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: two batch reads of one job disagreed: {body} then {again}"
        );
    }
}
