// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetJobs`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `BatchGetJobs` answers 200 and reports, for EACH name in
//! `JobNames` and in the order the request listed them, either the job definition stored
//! under that name — the same projection `GetJob` reports — or that name in
//! `JobsNotFound`; a name no job holds is not an error, and the call changes nothing a
//! later caller sees. The tables below span the batch along what the rule generalises
//! over: how many names are asked for, how many of them are stored, where the misses sit
//! in the order, and whether a name repeats.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::batch_get_jobs::MAX_BATCH_JOB_NAMES;
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The role ARN every job here is created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// A `CreateJob` body: the three `@required` members, plus a description so each stored
/// job carries a member that distinguishes it from its siblings.
fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": format!("s3://etl-scripts/{name}.py") },
        "Description": format!("job {name}"),
    })
}

/// One `BatchGetJobs` call that must succeed, as `(the reported Jobs' names, JobsNotFound)`.
async fn batch_get(router: &MultiServiceRouter, names: &[&str]) -> (Vec<String>, Vec<String>) {
    let body = ok(router, "BatchGetJobs", &json!({ "JobNames": names })).await;
    (job_names(&body), string_list(&body, "JobsNotFound"))
}

/// The `Name` of every element of the response's `Jobs` list, in order.
fn job_names(body: &Value) -> Vec<String> {
    body.get("Jobs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("BatchGetJobs must report a Jobs list, got {body}"))
        .iter()
        .map(|job| {
            job.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every reported Job carries a Name, got {job}"))
                .to_string()
        })
        .collect()
}

/// The strings of a top-level list member, which must be present even when empty.
fn string_list(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("BatchGetJobs must report {member}, got {body}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("{member} holds strings, got {name}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The batch splits the requested names across the two lists
// ---------------------------------------------------------------------------

/// One row of the batch table: the case name, the names the request asks for, the names
/// the response must report in `Jobs`, and the names it must report in `JobsNotFound`.
type BatchCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// Each requested name is reported in exactly one of the two lists, in the order asked.
///
/// The rows differ along the batch's size, how many of its names are stored, where the
/// misses sit (first, last, interleaved), and whether a name repeats. The recorded
/// reference case — one batch naming the jobs a single role backs — is the "every name
/// stored" row.
#[tokio::test]
async fn every_requested_name_is_reported_exactly_once() {
    let router = emulator();
    for stored in ["alpha", "beta", "gamma"] {
        ok(&router, "CreateJob", &job_body(stored)).await;
    }

    // (case, requested names, expected Jobs names, expected JobsNotFound)
    let cases: Vec<BatchCase> = vec![
        (
            "one stored name",
            vec!["alpha"],
            vec!["alpha"],
            vec![],
        ),
        (
            // The recorded case: a batch whose every name is stored.
            "every name stored",
            vec!["alpha", "beta", "gamma"],
            vec!["alpha", "beta", "gamma"],
            vec![],
        ),
        (
            "no name stored",
            vec!["absent", "missing"],
            vec![],
            vec!["absent", "missing"],
        ),
        (
            "the miss comes first",
            vec!["absent", "beta"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "the miss comes last",
            vec!["beta", "absent"],
            vec!["beta"],
            vec!["absent"],
        ),
        (
            "misses interleaved with hits",
            vec!["absent", "alpha", "missing", "gamma", "nope"],
            vec!["alpha", "gamma"],
            vec!["absent", "missing", "nope"],
        ),
        (
            // `Job$Name` carries no documented case fold, so a different casing is a
            // different job — and so a miss.
            "a stored name in another casing",
            vec!["ALPHA", "alpha"],
            vec!["alpha"],
            vec!["ALPHA"],
        ),
        (
            // One answer per requested name, not per distinct name.
            "a repeated stored name",
            vec!["beta", "beta"],
            vec!["beta", "beta"],
            vec![],
        ),
        (
            "a repeated unknown name",
            vec!["absent", "absent"],
            vec![],
            vec!["absent", "absent"],
        ),
        (
            // A batch with nothing in it: both lists are present and empty.
            "an empty batch",
            vec![],
            vec![],
            vec![],
        ),
    ];

    for (case, requested, expected_jobs, expected_missing) in cases {
        let body = ok(&router, "BatchGetJobs", &json!({ "JobNames": requested })).await;

        let reported = job_names(&body);
        assert_eq!(
            reported, expected_jobs,
            "case {case:?}: expected Jobs {expected_jobs:?}, got {reported:?} in {body}"
        );
        let missing = string_list(&body, "JobsNotFound");
        assert_eq!(
            missing, expected_missing,
            "case {case:?}: expected JobsNotFound {expected_missing:?}, got {missing:?} in {body}"
        );
        // The two lists together account for every requested name, once each.
        assert_eq!(
            reported.len() + missing.len(),
            requested.len(),
            "case {case:?}: every requested name must be reported exactly once, got {body}"
        );

        // A read does not change what the next caller sees: the same batch answers the
        // same thing, and each name is still readable through `GetJob`.
        let again = ok(&router, "BatchGetJobs", &json!({ "JobNames": requested })).await;
        assert_eq!(
            again, body,
            "case {case:?}: BatchGetJobs must be repeatable, got {again}"
        );
        for stored in ["alpha", "beta", "gamma"] {
            let read = ok(&router, "GetJob", &json!({ "JobName": stored })).await;
            assert_eq!(
                read.get("Job").and_then(|job| job.get("Name")),
                Some(&json!(stored)),
                "case {case:?}: the batch read must not have touched {stored:?}, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What it reports is what the store holds
// ---------------------------------------------------------------------------

/// A job the batch reports is byte-for-byte the definition `GetJob` reports for the same
/// name — the batch is a different addressing mode over one stored resource, not a
/// second projection of it.
///
/// The rows differ along the member set the create carried, so the comparison covers a
/// bare job, a worker-configured one and a fully specified one.
#[tokio::test]
async fn a_batched_job_is_the_job_get_job_reports() {
    let router = emulator();

    let creates: Vec<(&str, Value)> = vec![
        ("required members only", job_body("bare")),
        (
            "a worker configuration",
            json!({
                "Name": "workers",
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/w.py" },
                "WorkerType": "G.2X",
                "NumberOfWorkers": 4,
                "GlueVersion": "4.0",
            }),
        ),
        (
            "every optional member this batch cares about",
            json!({
                "Name": "full",
                "JobMode": "VISUAL",
                "Description": "the deploy stage's ETL job",
                "LogUri": "s3://glue-logs/full/",
                "Role": ROLE_ARN,
                "ExecutionProperty": { "MaxConcurrentRuns": 5 },
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://etl/full.py",
                    "PythonVersion": "3",
                },
                "DefaultArguments": { "--job-language": "python" },
                "Connections": { "Connections": ["warehouse"] },
                "MaxRetries": 3,
                "Timeout": 60,
                "MaxCapacity": 1.0,
                "MaintenanceWindow": "Mon:10",
                "Tags": { "owner": "data-platform" },
            }),
        ),
    ];

    let mut names = Vec::new();
    for (case, request) in &creates {
        ok(&router, "CreateJob", request).await;
        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: every row names its job"));
        names.push(name.to_string());
    }

    let requested: Vec<&str> = names.iter().map(String::as_str).collect();
    let body = ok(&router, "BatchGetJobs", &json!({ "JobNames": requested })).await;
    let reported = body
        .get("Jobs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("BatchGetJobs must report a Jobs list, got {body}"))
        .clone();

    for ((case, _), name) in creates.iter().zip(names.iter()) {
        let single = ok(&router, "GetJob", &json!({ "JobName": name })).await;
        let expected = single
            .get("Job")
            .unwrap_or_else(|| panic!("case {case:?}: GetJob must report a Job, got {single}"));
        let actual = reported
            .iter()
            .find(|job| job.get("Name") == Some(&json!(name)))
            .unwrap_or_else(|| panic!("case {case:?}: the batch must report {name:?}, got {body}"));
        assert_eq!(
            actual, expected,
            "case {case:?}: the batched job must be the job GetJob reports, got {actual}"
        );
        // `Tags` is not a member of the `Job` shape, so a batched job carries none.
        assert!(
            actual.get("Tags").is_none(),
            "case {case:?}: Tags is not a member of Job, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled constraint on a name is refused, not reported as a miss
// ---------------------------------------------------------------------------

/// A `JobNames` element outside the modelled `length 1..255` is refused at the request
/// boundary rather than reported in `JobsNotFound` — a name the service cannot address
/// is a bad request, not a job that is absent — and the required member itself must be
/// present.
///
/// Source for the bound: `smithy_query(query="shape",shape="JobNameList")`, whose member
/// targets `NameString` with `length 1..255`. Both bounds are exercised, plus one past
/// each.
#[tokio::test]
async fn a_name_outside_the_modeled_bounds_is_refused() {
    let router = emulator();
    ok(&router, "CreateJob", &job_body("alpha")).await;

    // At each published bound, the name is a legal address — so it is simply unknown,
    // and reported as a miss rather than refused.
    for length in [1usize, 255] {
        let name = "n".repeat(length);
        let body = ok(&router, "BatchGetJobs", &json!({ "JobNames": [name.clone()] })).await;
        assert_eq!(
            string_list(&body, "JobsNotFound"),
            vec![name.clone()],
            "case name of {length} characters: a legal but unknown name is a miss, got {body}"
        );
        assert!(
            job_names(&body).is_empty(),
            "case name of {length} characters: no job is stored under it, got {body}"
        );
    }

    // One past each bound is refused.
    for (case, names) in [
        ("an empty name", json!([""])),
        ("a name of 256 characters", json!(["n".repeat(256)])),
        (
            "a legal name beside an over-long one",
            json!(["alpha", "n".repeat(256)]),
        ),
    ] {
        let (status, answer) = call(&router, "BatchGetJobs", &json!({ "JobNames": names })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            answer.get("__type").is_some(),
            "case {case:?}: the refusal must name a modeled error, got {answer}"
        );
    }

    // `JobNames` is `@required`, so a batch that names it not at all is refused too.
    let (status, answer) = call(&router, "BatchGetJobs", &json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no JobNames member\": expected 400, got {status} with {answer}"
    );
    assert!(
        !error_code(&answer).is_empty(),
        "case \"no JobNames member\": the refusal must name a modeled error, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The batch's own size bound
// ---------------------------------------------------------------------------

/// A batch is served up to `MAX_BATCH_JOB_NAMES` names and refused past it — the answer
/// carries one whole job projection per listed name, repeats included, so the bound is
/// on the request's name COUNT, which the model leaves unconstrained.
///
/// The rows differ along the batch's size (one, a middling batch, the bound exactly, one
/// past it) and along what the names are: a batch at the bound made of distinct stored
/// jobs, one made of the same stored name repeated, and one of unknown names.
#[tokio::test]
async fn a_batch_past_the_name_ceiling_is_refused() {
    let router = emulator();
    // Enough distinct jobs to fill a batch at the bound.
    let stored: Vec<String> = (0..MAX_BATCH_JOB_NAMES)
        .map(|index| format!("batched-{index}"))
        .collect();
    for name in &stored {
        ok(&router, "CreateJob", &job_body(name)).await;
    }

    // At and below the bound the batch is served, whatever its names are.
    for size in [1usize, 25, MAX_BATCH_JOB_NAMES] {
        let distinct: Vec<&str> = stored.iter().take(size).map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &distinct).await;
        assert_eq!(
            found, distinct,
            "case {size} distinct stored names: expected them all reported, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} distinct stored names: none may miss, got {missing:?}"
        );

        let repeated: Vec<&str> = vec!["batched-0"; size];
        let (found, missing) = batch_get(&router, &repeated).await;
        assert_eq!(
            found.len(),
            size,
            "case {size} copies of one stored name: one entry per requested name, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} copies of one stored name: none may miss, got {missing:?}"
        );

        let unknown: Vec<String> = (0..size).map(|index| format!("absent-{index}")).collect();
        let unknown: Vec<&str> = unknown.iter().map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &unknown).await;
        assert!(
            found.is_empty(),
            "case {size} unknown names: nothing may be reported, got {found:?}"
        );
        assert_eq!(
            missing, unknown,
            "case {size} unknown names: all of them miss, got {missing:?}"
        );
    }

    // One past the bound is refused with the operation's modelled input error, and the
    // refusal does not depend on whether the names are stored.
    let over = MAX_BATCH_JOB_NAMES + 1;
    let batches: Vec<(&str, Vec<String>)> = vec![
        (
            "one name too many, all stored",
            stored
                .iter()
                .cloned()
                .chain(std::iter::once("batched-0".to_string()))
                .collect(),
        ),
        (
            "one name too many, none stored",
            (0..over).map(|index| format!("absent-{index}")).collect(),
        ),
        (
            "one name too many, the same name repeated",
            vec!["batched-0".to_string(); over],
        ),
    ];
    for (case, names) in batches {
        assert_eq!(
            names.len(),
            over,
            "case {case:?}: the batch must be one past the bound"
        );
        let (status, answer) = call(&router, "BatchGetJobs", &json!({ "JobNames": names })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {answer}"
        );
    }

    // The refusals changed nothing: a batch at the bound is still served.
    let distinct: Vec<&str> = stored.iter().map(String::as_str).collect();
    let (found, _) = batch_get(&router, &distinct).await;
    assert_eq!(
        found.len(),
        MAX_BATCH_JOB_NAMES,
        "a batch at the bound is still served after a refused one, got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// A deleted job becomes a miss
// ---------------------------------------------------------------------------

/// The batch reports what the store holds AT THE TIME OF THE CALL: a name moves from
/// `Jobs` to `JobsNotFound` when its job leaves the store, and back when it is created
/// again.
///
/// The job leaves the store directly rather than through `DeleteJob`, which is still a
/// scaffold in this crate; what is under test is that the batch reads the store on every
/// call rather than caching its first answer.
#[tokio::test]
async fn the_batch_follows_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    ok(&router, "CreateJob", &job_body("transient")).await;

    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"created\": a stored job is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"created\": a stored job is not missing, got {missing:?}"
    );

    engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .jobs
        .remove("transient");
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert!(
        found.is_empty(),
        "case \"removed\": a job no longer stored is not reported, got {found:?}"
    );
    assert_eq!(
        missing,
        vec!["transient".to_string()],
        "case \"removed\": a job no longer stored is reported missing, got {missing:?}"
    );

    ok(&router, "CreateJob", &job_body("transient")).await;
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"re-created\": a re-created job is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"re-created\": a re-created job is not missing, got {missing:?}"
    );
}
