// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION a Glue job definition lives in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, which puts
//! the Region in the request's SigV4 credential scope — the field the service reads a
//! request's Region from — so what is asserted here is what a client pinned to one
//! Region's endpoint sees.
//!
//! The rule under test: Glue is a regional service, so a job definition exists only in
//! the Region it was created in. Every operation that addresses a job by name resolves
//! it in the endpoint's Region alone — `StartJobRun`, `GetJob` and `UpdateJob` refuse a
//! job of another Region with `EntityNotFoundException`, `BatchGetJobs` reports the name
//! in `JobsNotFound` and `ListJobs` does not list it — and the same name in two Regions
//! names two independent jobs.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    JOB_METADATA_MISSING, JOB_RUNS_JOB_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, REGION};

/// The role ARN these jobs are created with. An IAM role is account-global, so the same
/// one backs a job in every Region.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The Regions, other than the harness's own, these tests address an endpoint in. They
/// span three partitions' worth of prefixes, so nothing here depends on one Region name.
const OTHER_REGIONS: [&str; 3] = ["us-west-2", "eu-west-1", "ap-southeast-1"];

/// A `CreateJob` body for `name`, pointed at a script in Amazon S3.
fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

/// Create `name` through `region`'s endpoint.
async fn create_job_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, body) = call_in_region(router, region, "CreateJob", &job_body(name)).await;
    assert_eq!(
        status, 200,
        "case {region}: CreateJob must answer 200, got {status} with {body}"
    );
}

/// The `(Jobs, JobsNotFound)` names `BatchGetJobs` reports in `region` for `names`.
async fn batch_get_in(
    router: &MultiServiceRouter,
    region: &str,
    names: &[&str],
) -> (Vec<String>, Vec<String>) {
    let (status, body) = call_in_region(
        router,
        region,
        "BatchGetJobs",
        &json!({ "JobNames": names }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region}: BatchGetJobs must answer 200, got {status} with {body}"
    );
    let found = body["Jobs"]
        .as_array()
        .expect("Jobs is a list")
        .iter()
        .map(|job| job["Name"].as_str().expect("Job.Name is a string").to_string())
        .collect();
    let missing = body["JobsNotFound"]
        .as_array()
        .expect("JobsNotFound is a list")
        .iter()
        .map(|name| name.as_str().expect("a name is a string").to_string())
        .collect();
    (found, missing)
}

/// The names `ListJobs` reports in `region`.
async fn list_jobs_in(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = call_in_region(router, region, "ListJobs", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {region}: ListJobs must answer 200, got {status} with {body}"
    );
    body["JobNames"]
        .as_array()
        .expect("JobNames is a list")
        .iter()
        .map(|name| name.as_str().expect("a name is a string").to_string())
        .collect()
}

/// A job created in one Region does not exist in any other: every operation that
/// addresses it by name answers for THAT Region's namespace, and the job stays served
/// where it was created.
///
/// The rows are the operations that resolve a job name, each tried through a Region that
/// does not hold the job.
#[tokio::test]
async fn a_job_exists_only_in_the_region_it_was_created_in() {
    let router = emulator();
    let name = "regional-etl-job";
    create_job_in(&router, REGION, name).await;

    for region in OTHER_REGIONS {
        // The refusals: a start, a read and an update of a job this Region does not hold.
        let refused: Vec<(&str, Value)> = vec![
            ("StartJobRun", json!({ "JobName": name })),
            ("GetJob", json!({ "JobName": name })),
            ("GetJobRuns", json!({ "JobName": name })),
            (
                "UpdateJob",
                json!({
                    "JobName": name,
                    "JobUpdate": {
                        "Role": ROLE_ARN,
                        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/other.py" },
                    },
                }),
            ),
        ];
        for (operation, body) in &refused {
            let (status, answer) = call_in_region(&router, region, operation, body).await;
            assert_eq!(
                status, 400,
                "case {region} / {operation}: expected 400, got {status} with {answer}"
            );
            assert!(
                error_code(&answer).contains("EntityNotFoundException"),
                "case {region} / {operation}: expected EntityNotFoundException, got {}",
                error_code(&answer)
            );
        }

        // The batch read reports the name as one it could not resolve, which is the
        // documented signal a caller uses to detect "this job does not exist here".
        let (found, missing) = batch_get_in(&router, region, &[name]).await;
        assert!(
            found.is_empty(),
            "case {region}: BatchGetJobs must find no job of another Region, got {found:?}"
        );
        assert_eq!(
            missing,
            vec![name.to_string()],
            "case {region}: BatchGetJobs must report the name in JobsNotFound, got {missing:?}"
        );

        // And the listing of that Region is empty: no phantom record to enumerate.
        let listed = list_jobs_in(&router, region).await;
        assert!(
            listed.is_empty(),
            "case {region}: ListJobs must not list another Region's jobs, got {listed:?}"
        );
    }

    // The job is still served where it was created, and nothing started a run of it.
    let job = ok(&router, "GetJob", &json!({ "JobName": name })).await;
    assert_eq!(
        job["Job"]["Name"].as_str(),
        Some(name),
        "the owning Region must still serve the job, got {job}"
    );
    let history = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    assert_eq!(
        history["JobRuns"].as_array().map(Vec::len),
        Some(0),
        "the owning Region's job must have an empty run history, got {history}"
    );
    let (found, missing) = batch_get_in(&router, REGION, &[name]).await;
    assert_eq!(
        found,
        vec![name.to_string()],
        "the owning Region must resolve the name, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "the owning Region misses nothing, got {missing:?}"
    );
}

/// `StartJobRun` against a Region that does not hold the job is refused as the service
/// words that refusal — after the metadata the start could not read — and records no run
/// anywhere.
#[tokio::test]
async fn a_cross_region_start_records_no_run() {
    let router = emulator();
    let name = "cross-region-start";
    create_job_in(&router, REGION, name).await;

    let (status, answer) = call_in_region(
        &router,
        OTHER_REGIONS[0],
        "StartJobRun",
        &json!({ "JobName": name }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a cross-Region start must be refused, got {status} with {answer}"
    );
    assert_eq!(
        error_message(&answer),
        JOB_METADATA_MISSING,
        "expected the service's wording for a start with no job metadata, got {}",
        error_message(&answer)
    );

    // No run exists in either Region: the refused start wrote nothing, and the owning
    // Region's job still has an empty run history.
    for region in [REGION, OTHER_REGIONS[0]] {
        let (status, answer) = call_in_region(
            &router,
            region,
            "GetJobRun",
            &json!({ "JobName": name, "RunId": "jr_0000000000000000000000000000000000000000000000000000000000000000" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region}: no run may be found, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {region}: expected EntityNotFoundException, got {}",
            error_code(&answer)
        );
    }

    // A start through the OWNING Region's endpoint does run, so the refusal above is
    // about the Region and not about the job.
    let started = ok(&router, "StartJobRun", &json!({ "JobName": name })).await;
    let run_id = started["JobRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {started}"))
        .to_string();
    // And that run is readable only through the Region that holds the job.
    let run = ok(
        &router,
        "GetJobRun",
        &json!({ "JobName": name, "RunId": run_id }),
    )
    .await;
    assert_eq!(
        run["JobRun"]["Id"].as_str(),
        Some(run_id.as_str()),
        "the owning Region must serve the run it started, got {run}"
    );
    let (status, answer) = call_in_region(
        &router,
        OTHER_REGIONS[0],
        "GetJobRun",
        &json!({ "JobName": name, "RunId": run_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a run of another Region's job is not found, got {status} with {answer}"
    );
}

/// `GetJobRuns` lists the runs of ONE job of the endpoint's Region, newest first, one
/// bounded page at a time; a `JobName` that Region holds no job for is refused as the
/// service words that refusal.
///
/// The rows differ along the page size, which is what the listing generalises over: the
/// whole history at once, and every page size from one run up to one past the count.
#[tokio::test]
async fn the_run_history_is_listed_newest_first_for_the_region_that_holds_the_job() {
    let router = emulator();
    let name = "run-history";
    // "The maximum number of concurrent runs allowed for the job": the history below
    // holds several runs at once, so the job admits them.
    let mut body = job_body(name);
    body["ExecutionProperty"] = json!({ "MaxConcurrentRuns": 4 });
    ok(&router, "CreateJob", &body).await;

    // Started in order, so the listing must report them reversed.
    let mut started: Vec<String> = Vec::new();
    for _ in 0..4 {
        let answer = ok(&router, "StartJobRun", &json!({ "JobName": name })).await;
        started.push(
            answer["JobRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {answer}"))
                .to_string(),
        );
    }
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();

    for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(5)] {
        let case = format!("MaxResults {page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "JobName": name });
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(cursor) = token.as_ref() {
                body["NextToken"] = json!(cursor);
            }
            let answer = ok(&router, "GetJobRuns", &body).await;
            let page = answer["JobRuns"].as_array().expect("JobRuns is a list");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case}: a page must hold at most {size} runs, got {}",
                    page.len()
                );
            }
            for run in page {
                assert_eq!(
                    run["JobName"].as_str(),
                    Some(name),
                    "case {case}: every run is a run of the addressed job, got {run}"
                );
                seen.push(
                    run["Id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: a run carries an Id, got {run}"))
                        .to_string(),
                );
            }
            token = answer["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, newest_first,
            "case {case}: the runs must be listed newest first, got {seen:?}"
        );
    }

    // A name this Region holds no job for is refused with the service's own wording.
    let (status, answer) = call(&router, "GetJobRuns", &json!({ "JobName": "never-created" })).await;
    assert_eq!(
        status, 400,
        "a listing for an unknown job is refused, got {status} with {answer}"
    );
    assert!(
        error_code(&answer).contains("EntityNotFoundException"),
        "expected EntityNotFoundException, got {}",
        error_code(&answer)
    );
    assert_eq!(
        error_message(&answer),
        JOB_RUNS_JOB_NOT_FOUND,
        "expected the service's wording for a run listing of an unknown job, got {}",
        error_message(&answer)
    );
}

/// The same job name in two Regions names two independent job definitions: each Region
/// reports its own, a create in one does not collide with the other, and a read from
/// either resolves to the definition that Region stored.
#[tokio::test]
async fn the_same_name_in_two_regions_names_two_jobs() {
    let router = emulator();
    let name = "shared-job-name";

    create_job_in(&router, REGION, name).await;
    for region in OTHER_REGIONS {
        // No `AlreadyExistsException`: the name is free in every other Region.
        create_job_in(&router, region, name).await;
    }

    // Each Region's own script location is what that Region reports, so the two records
    // are not one shared record.
    for (index, region) in OTHER_REGIONS.iter().enumerate() {
        let script = format!("s3://etl-scripts/{region}-{index}.py");
        let (status, answer) = call_in_region(
            &router,
            region,
            "UpdateJob",
            &json!({
                "JobName": name,
                "JobUpdate": {
                    "Role": ROLE_ARN,
                    "Command": { "Name": "glueetl", "ScriptLocation": script },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: the Region's own job is updatable, got {status} with {answer}"
        );

        let (status, answer) = call_in_region(&router, region, "GetJob", &json!({ "JobName": name }))
            .await;
        assert_eq!(status, 200, "case {region}: expected 200, got {answer}");
        assert_eq!(
            answer["Job"]["Command"]["ScriptLocation"].as_str(),
            Some(script.as_str()),
            "case {region}: the Region must report its own definition, got {answer}"
        );
        let listed = list_jobs_in(&router, region).await;
        assert_eq!(
            listed,
            vec![name.to_string()],
            "case {region}: the Region lists its own job and no other, got {listed:?}"
        );
    }

    // The Region the first create used still reports the definition it stored.
    let job = ok(&router, "GetJob", &json!({ "JobName": name })).await;
    assert_eq!(
        job["Job"]["Command"]["ScriptLocation"].as_str(),
        Some("s3://etl-scripts/etl.py"),
        "the first Region's definition must be untouched by the others, got {job}"
    );
}
