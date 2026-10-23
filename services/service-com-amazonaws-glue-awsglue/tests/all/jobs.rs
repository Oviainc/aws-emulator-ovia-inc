// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue job slice: `CreateJob` and the `GetJob` that reads
//! back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `CreateJob` persists the job definition under its name —
//! documented as unique in the account — and answers 200 with that name, and `GetJob`
//! reports the stored definition: every member the request carried, unchanged, plus the
//! members the SERVICE fills (the documented defaults for an absent `JobMode`,
//! `GlueVersion`, `ExecutionProperty` and `Timeout`, and the capacity in DPUs the
//! request's worker configuration maps to). The recorded reference-architecture case —
//! a `glueetl` command with a role ARN and a script location — is one row of the tables
//! below; the rows differ along the member set sent, the command kind, the worker type
//! and count, the Glue version and the capacity form.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::{CreateJobInput, JobCommand};
use service_com_amazonaws_glue_awsglue::state::{
    dpu_per_worker, ComAmazonawsGlueAwsglueState, JobEntry, DEFAULT_GLUE_VERSION,
    DEFAULT_JOB_MODE, DEFAULT_MAX_CONCURRENT_RUNS, DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5,
    DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4, MAX_JOBS, MAX_TIMEOUT_MINUTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, ok};

/// The role ARN the recorded reference-architecture workflow passes to `CreateJob`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// A minimal `CreateJob` body: the three `@required` members and nothing else.
fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

/// The `Job` structure `GetJob` reports for `name`.
async fn get_job(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetJob", &json!({ "JobName": name })).await;
    body.get("Job")
        .cloned()
        .unwrap_or_else(|| panic!("GetJob must report a Job, got {body}"))
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateJob` persists is what `GetJob` reports.
///
/// Each row is a request plus the members the service derives for it; the request's own
/// members must round-trip unchanged and the derived ones must match the documented
/// rule. The rows differ along the member set sent (the bare required members, every
/// optional member), the command kind (`glueetl`, `pythonshell`, `gluestreaming`,
/// `glueray`), the capacity form (worker type and count, an explicit `MaxCapacity`, the
/// deprecated `AllocatedCapacity`, none at all) and the Glue version, which is what the
/// `Timeout` default turns on.
#[tokio::test]
async fn created_jobs_are_read_back_from_the_store() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            // The recorded reference-architecture case: a Spark ETL job pointed at a
            // script in Amazon S3, with no capacity or version named.
            "required members only",
            job_body("sim2real-etl-job"),
            json!({
                // "When the JobMode field is missing or null, SCRIPT is assigned as
                // the default value."
                "JobMode": DEFAULT_JOB_MODE,
                // "Jobs that are created without specifying a Glue version default to
                // Glue 5.1."
                "GlueVersion": DEFAULT_GLUE_VERSION,
                // "the timeout is defaulted to … 480 minutes for Glue version 5.0 and
                // later."
                "Timeout": DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5,
                // "The maximum number of concurrent runs allowed for the job. The
                // default is 1."
                "ExecutionProperty": { "MaxConcurrentRuns": DEFAULT_MAX_CONCURRENT_RUNS },
                // The model's own `@default` for the two deprecated/absent integers.
                "MaxRetries": 0,
                "AllocatedCapacity": 0,
                // No capacity was named and no worker configuration maps to one.
                "MaxCapacity": Value::Null,
                "ProfileName": Value::Null,
            }),
        ),
        (
            // Two G.1X workers: 1 DPU each, so the job's capacity is 2 DPUs. Glue 4.0
            // takes the longer documented default timeout.
            "worker configuration on glue 4.0",
            json!({
                "Name": "worker-config",
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/j.py" },
                "GlueVersion": "4.0",
                "WorkerType": "G.1X",
                "NumberOfWorkers": 2,
            }),
            json!({
                "MaxCapacity": 2.0,
                "AllocatedCapacity": 2,
                "Timeout": DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4,
                "GlueVersion": "4.0",
            }),
        ),
        (
            // A Python shell job may allocate a fractional DPU: "you can allocate
            // either 0.0625 or 1 DPU".
            "python shell with a fractional max capacity",
            json!({
                "Name": "python-shell",
                "Role": "GlueJobRole",
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://etl/shell.py",
                    "PythonVersion": "3",
                },
                "MaxCapacity": 0.0625,
            }),
            json!({
                "MaxCapacity": 0.0625,
                // A fraction of a DPU is no whole DPU.
                "AllocatedCapacity": 0,
            }),
        ),
        (
            "streaming job with G.025X workers",
            json!({
                "Name": "streaming",
                "Role": "GlueJobRole",
                "Command": { "Name": "gluestreaming", "ScriptLocation": "s3://etl/stream.py" },
                "WorkerType": "G.025X",
                "NumberOfWorkers": 4,
                "GlueVersion": "3.0",
            }),
            // 0.25 DPU per worker x 4 workers.
            json!({ "MaxCapacity": 1.0, "AllocatedCapacity": 1 }),
        ),
        (
            "ray job with Z.2X workers",
            json!({
                "Name": "ray-job",
                "Role": "GlueJobRole",
                "Command": {
                    "Name": "glueray",
                    "ScriptLocation": "s3://etl/ray.py",
                    "Runtime": "Ray2.4",
                },
                "WorkerType": "Z.2X",
                "NumberOfWorkers": 3,
                "GlueVersion": "4.0",
            }),
            json!({ "MaxCapacity": 6.0, "AllocatedCapacity": 3 * 2 }),
        ),
        (
            // "This parameter is deprecated. Use MaxCapacity instead" — the two name
            // one quantity, so the job reports the same number for both.
            "the deprecated allocated capacity",
            json!({
                "Name": "allocated-capacity",
                "Role": "GlueJobRole",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/old.py" },
                "AllocatedCapacity": 10,
                "GlueVersion": "1.0",
            }),
            json!({ "MaxCapacity": 10.0, "AllocatedCapacity": 10 }),
        ),
        (
            "every optional member",
            json!({
                "Name": "fully-specified",
                "JobMode": "VISUAL",
                "JobRunQueuingEnabled": true,
                "Description": "the deploy stage's ETL job",
                "LogUri": "s3://glue-logs/fully-specified/",
                "Role": ROLE_ARN,
                "ExecutionProperty": { "MaxConcurrentRuns": 5 },
                "Command": {
                    "Name": "glueetl",
                    "ScriptLocation": "s3://etl-scripts/full.py",
                    "PythonVersion": "3",
                },
                "DefaultArguments": {
                    "--job-language": "python",
                    "--enable-metrics": "true",
                },
                "NonOverridableArguments": { "--locked": "yes" },
                "Connections": { "Connections": ["warehouse", "lake"] },
                "MaxRetries": 3,
                "Timeout": 60,
                "SecurityConfiguration": "glue-security",
                "NotificationProperty": { "NotifyDelayAfter": 10 },
                "GlueVersion": "5.0",
                "NumberOfWorkers": 10,
                "WorkerType": "G.2X",
                "ExecutionClass": "FLEX",
                "SourceControlDetails": {
                    "Provider": "AWS_CODE_COMMIT",
                    "Repository": "etl-source",
                    "Branch": "main",
                    "Folder": "jobs",
                    "AuthStrategy": "AWS_SECRETS_MANAGER",
                },
                "MaintenanceWindow": "Mon:10",
                "Tags": { "owner": "data-platform" },
            }),
            json!({ "MaxCapacity": 20.0, "AllocatedCapacity": 20, "Timeout": 60 }),
        ),
    ];

    for (case, request, derived) in cases {
        let created = ok(&router, "CreateJob", &request).await;
        assert_eq!(
            created.get("Name"),
            request.get("Name"),
            "case {case:?}: CreateJob must answer with the name it was given, got {created}"
        );

        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .expect("every row names its job");
        let job = get_job(&router, name).await;

        // Everything the request carried comes back unchanged. `Tags` is not a member
        // of the `Job` shape — it is read back through the tagging operations — so it
        // is not part of the echo.
        let mut echoed = request.clone();
        echoed
            .as_object_mut()
            .expect("the request is a structure")
            .remove("Tags");
        assert_echoed(case, &echoed, &job);
        assert!(
            job.get("Tags").is_none(),
            "case {case:?}: Tags is not a member of Job, got {job}"
        );

        // And the members the service fills are the documented ones; a `null`
        // expectation means the member carries no value.
        assert_echoed(case, &derived, &job);

        // Both server-set instants are present, and a job that has not been updated
        // was last modified when it was created.
        let created_on = job
            .get("CreatedOn")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: CreatedOn must be a timestamp, got {job}"));
        assert_eq!(
            job.get("LastModifiedOn").and_then(Value::as_f64),
            Some(created_on),
            "case {case:?}: a job nothing has updated was last modified at its create"
        );

        // A read does not change what the next caller sees.
        let again = get_job(&router, name).await;
        assert_eq!(again, job, "case {case:?}: GetJob must be repeatable");
    }
}

/// Every predefined worker type maps a worker count to the DPUs the model quotes for
/// that worker, and a worker type the model gives no per-worker DPU figure for derives
/// no capacity at all.
///
/// The catalog of worker types is `smithy_query(query="shape",shape="WorkerType")` and
/// the per-worker DPU figures are from `CreateJob$WorkerType`'s own documentation
/// ("For the `G.1X` worker type, each worker maps to 1 DPU", and so on for G.2X, G.4X,
/// G.8X, G.025X and Z.2X); `Standard` is in the enum but carries no such figure.
#[tokio::test]
async fn capacity_is_derived_from_every_documented_worker_type() {
    let router = emulator();

    // (worker type, DPUs per worker as the model documents it, or None)
    let catalog: Vec<(&str, Option<f64>)> = vec![
        ("G.025X", Some(0.25)),
        ("G.1X", Some(1.0)),
        ("G.2X", Some(2.0)),
        ("G.4X", Some(4.0)),
        ("G.8X", Some(8.0)),
        ("Z.2X", Some(2.0)),
        ("Standard", None),
    ];

    for (worker_type, per_worker) in catalog {
        assert_eq!(
            dpu_per_worker(worker_type),
            per_worker,
            "worker type {worker_type:?}: the per-worker DPU figure the model documents"
        );
        for workers in [1i64, 2, 7, 299] {
            let name = format!("capacity-{worker_type}-{workers}");
            let case = name.clone();
            ok(
                &router,
                "CreateJob",
                &json!({
                    "Name": name,
                    "Role": ROLE_ARN,
                    "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/c.py" },
                    "WorkerType": worker_type,
                    "NumberOfWorkers": workers,
                }),
            )
            .await;
            let job = get_job(&router, &name).await;
            let expected = per_worker.map(|dpu| dpu * workers as f64);
            assert_eq!(
                job.get("MaxCapacity").and_then(Value::as_f64),
                expected,
                "case {case:?}: {workers} workers of {worker_type:?} must map to {expected:?} DPUs, got {job}"
            );
            assert_eq!(
                job.get("AllocatedCapacity").and_then(Value::as_i64),
                Some(expected.map_or(0, |dpus| dpus as i64)),
                "case {case:?}: AllocatedCapacity reports the same whole DPUs as MaxCapacity"
            );
            // The worker configuration itself round-trips whatever is derived from it.
            assert_eq!(
                job.get("WorkerType").and_then(Value::as_str),
                Some(worker_type),
                "case {case:?}: WorkerType must round-trip"
            );
            assert_eq!(
                job.get("NumberOfWorkers").and_then(Value::as_i64),
                Some(workers),
                "case {case:?}: NumberOfWorkers must round-trip"
            );
        }
    }
}

/// `Timeout` is accepted at each documented bound and refused past the upper one:
/// "Jobs must have timeout values less than 7 days or 10080 minutes. Otherwise, the
/// jobs will throw an exception", and the model's own `range 1..` bounds it below.
#[tokio::test]
async fn the_timeout_bounds_are_enforced() {
    let router = emulator();

    for timeout in [1, 60, MAX_TIMEOUT_MINUTES] {
        let name = format!("timeout-{timeout}");
        let mut body = job_body(&name);
        body["Timeout"] = json!(timeout);
        ok(&router, "CreateJob", &body).await;
        let job = get_job(&router, &name).await;
        assert_eq!(
            job.get("Timeout").and_then(Value::as_i64),
            Some(i64::from(timeout)),
            "case timeout {timeout}: a timeout within the documented bounds round-trips"
        );
    }

    // One past the documented maximum.
    let mut body = job_body("timeout-too-long");
    body["Timeout"] = json!(MAX_TIMEOUT_MINUTES + 1);
    let (status, answer) = call(&router, "CreateJob", &body).await;
    assert_eq!(
        status, 400,
        "case timeout {}: expected 400, got {status} with {answer}",
        MAX_TIMEOUT_MINUTES + 1
    );
    assert!(
        error_code(&answer).contains("InvalidInputException"),
        "case timeout {}: expected InvalidInputException, got {answer}",
        MAX_TIMEOUT_MINUTES + 1
    );

    // One below the modeled minimum: `Timeout`'s `range 1..` is refused at the request
    // boundary, before this handler runs, so only the refusal itself is asserted here.
    let mut body = job_body("timeout-zero");
    body["Timeout"] = json!(0);
    let (status, answer) = call(&router, "CreateJob", &body).await;
    assert_eq!(
        status, 400,
        "case timeout 0: expected 400, got {status} with {answer}"
    );
    assert!(
        answer.get("__type").is_some(),
        "case timeout 0: the refusal must name a modeled error, got {answer}"
    );
    let (status, answer) = call(&router, "GetJob", &json!({ "JobName": "timeout-zero" })).await;
    assert_eq!(
        status, 400,
        "case timeout 0: the refused create must have stored nothing, got {status} {answer}"
    );
}

/// A request `CreateJob` cannot serve is refused with the error the model names for it,
/// and nothing is stored. The rows differ along which part of the request is at fault:
/// the name's uniqueness, the command, the role, and the two ways of naming capacity.
#[tokio::test]
async fn an_unservable_create_is_refused_with_the_modeled_error() {
    let router = emulator();
    ok(&router, "CreateJob", &job_body("taken")).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            // "It must be unique in your account."
            "a name already taken",
            job_body("taken"),
            "AlreadyExistsException",
        ),
        (
            // "For an Apache Spark ETL job, this must be glueetl. For a Python shell
            // job, it must be pythonshell. … gluestreaming … glueray."
            "a command the service does not run",
            json!({
                "Name": "bad-command",
                "Role": ROLE_ARN,
                "Command": { "Name": "sparkmagic", "ScriptLocation": "s3://etl/x.py" },
            }),
            "InvalidInputException",
        ),
        (
            "a command naming no job kind at all",
            json!({
                "Name": "no-command-name",
                "Role": ROLE_ARN,
                "Command": { "ScriptLocation": "s3://etl/x.py" },
            }),
            "InvalidInputException",
        ),
        (
            "a blank role",
            json!({
                "Name": "blank-role",
                "Role": "   ",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
            }),
            "InvalidInputException",
        ),
        (
            // "Do not set MaxCapacity if using WorkerType and NumberOfWorkers."
            "a capacity named twice",
            json!({
                "Name": "two-capacities",
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
                "MaxCapacity": 10.0,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 10,
            }),
            "InvalidInputException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, answer) = call(&router, "CreateJob", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected),
            "case {case:?}: expected {expected}, got {answer}"
        );
        // The refused create stored nothing: the name is still unknown (except
        // "taken", which the successful create above owns).
        let name = body.get("Name").and_then(Value::as_str).expect("a name");
        if name != "taken" {
            let (status, answer) = call(&router, "GetJob", &json!({ "JobName": name })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store nothing, got {status} {answer}"
            );
            assert!(
                error_code(&answer).contains("EntityNotFoundException"),
                "case {case:?}: an unstored job is not found, got {answer}"
            );
        }
    }

    // The job the first create stored is still exactly what it stored.
    let job = get_job(&router, "taken").await;
    assert_eq!(
        job.get("Role").and_then(Value::as_str),
        Some(ROLE_ARN),
        "the refusals must not have touched the stored job, got {job}"
    );
}

/// `GetJob` answers the modelled not-found error for a name no job in the account
/// holds — never an empty success, which a caller would read as a job with no members.
#[tokio::test]
async fn getting_a_job_that_does_not_exist_is_not_found() {
    let router = emulator();
    ok(&router, "CreateJob", &job_body("exists")).await;

    // The rows differ along how the name misses: never created, deleted casing, and a
    // name that is a prefix of a stored one. `Job$Name` carries no documented case
    // fold, so a different casing is a different job.
    for name in ["absent", "EXISTS", "exist"] {
        let (status, answer) = call(&router, "GetJob", &json!({ "JobName": name })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {name:?}: expected EntityNotFoundException, got {answer}"
        );
    }
}

/// The store holds a bounded number of job definitions: the create at the ceiling
/// succeeds, the one past it is refused with the modelled limit error, and freeing a
/// slot lets a create succeed again — the ceiling bounds what is stored rather than
/// latching how many creates were made.
#[tokio::test]
async fn a_create_past_the_job_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_JOBS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateJobInput {
                name: name.clone(),
                role: "GlueJobRole".to_string(),
                command: JobCommand {
                    name: Some("glueetl".to_string()),
                    ..JobCommand::default()
                },
                ..CreateJobInput::default()
            };
            let entry = JobEntry::from_create_input(&input, SystemTime::UNIX_EPOCH);
            store.jobs.insert(name, entry);
        }
    }

    ok(&router, "CreateJob", &job_body("last-job")).await;
    let (status, body) = call(&router, "CreateJob", &job_body("one-too-many")).await;
    assert_eq!(
        status, 400,
        "case job {} of {MAX_JOBS}: expected 400, got {status} with {body}",
        MAX_JOBS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one job too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().jobs.remove("seeded-0");
    }
    ok(&router, "CreateJob", &job_body("one-too-many")).await;
}
