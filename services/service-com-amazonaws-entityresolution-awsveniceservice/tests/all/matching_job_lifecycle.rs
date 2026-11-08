// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution MATCHING JOB lifecycle:
//! `StartMatchingJob` and the `GetMatchingJob` that reads the job back.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see.
//!
//! The rules under test:
//!
//! 1. `StartMatchingJob` MINTS a job of an existing matching workflow — a `jobId`
//!    matching the modelled `JobId` pattern `^[a-f0-9]{32}$`, distinct per start — and
//!    PERSISTS it, so `GetMatchingJob` answers that job's status, `startTime` and the
//!    `outputSourceConfig` derived from the workflow's own output configuration.
//! 2. The job's status is CLOCK-DRIVEN and never regresses: it is `QUEUED` when the
//!    caller reads it straight after starting it, then `RUNNING`, then the terminal
//!    status its run reached, carrying `errorDetails` when that run produced nothing
//!    (this suite mounts no AWS Glue Data Catalog, so no workflow here has an input
//!    table to read) and an `endTime` that is set on no earlier status. Polling is a
//!    read: it does not move the job, and two reads of a settled job are identical.
//! 3. A workflow that does not exist is `ResourceNotFoundException` (404) on both
//!    operations; so is a `jobId` the named workflow does not hold — including one that
//!    belongs to another workflow.
//! 4. A workflow with a job still in progress refuses a second start with
//!    `ConflictException` (400) and mints nothing; once that job is terminal, a start
//!    succeeds again with a NEW `jobId`.
//! 5. A job in progress likewise blocks `DeleteMatchingWorkflow` with
//!    `ConflictException` (400) — the state `ConflictException`'s doc calls "Workflow
//!    is currently running" — whose message NAMES the workflow whose job blocks the
//!    delete, and a delete that does go through takes that workflow's job history with
//!    it, so a workflow recreated under the same name inherits no runs.
//!
//! Each table row differs along what the rules generalise over: how the workflow's
//! output source is configured (an S3 path with and without a `KMSArn`, a deep path,
//! and no path at all — `OutputSource$outputS3Path` defaults to the empty string), the
//! `resolutionType`, the number of input sources, and the `workflowName` character
//! classes the modelled `EntityName` pattern `^[a-zA-Z_0-9-]*$` admits. The recorded
//! request is ONE row of these.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::schema_mapping_workflow_lifecycle::{
    glue_table_arn, role_arn, string_member, Emulator, Reply, REGION,
};

/// The `JobStatus` values, in the order the lifecycle reaches them
/// (`smithy_query(query="shape",shape="JobStatus")` gives the set
/// `RUNNING | SUCCEEDED | FAILED | QUEUED`; the order below is this service's
/// lifecycle, with `SUCCEEDED` unreachable in THIS suite because it mounts no catalog
/// for a run to read its input table from — the sibling [`crate::matching_run`] suite
/// is the one that mounts the catalog and the object store and reaches `SUCCEEDED`).
const LIFECYCLE: &[&str] = &["QUEUED", "RUNNING", "FAILED"];

/// How long the whole lifecycle may take before the test gives up. The emulator's own
/// per-status span is well under 1 s, so this is a generous ceiling that keeps the
/// suite deterministic on a loaded machine rather than a second assertion about timing.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the lifecycle is polled while waiting for it to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `DeleteMatchingWorkflow`'s `ConflictException` message when a job of the workflow
/// is still running, as a literal here so this suite pins the TEMPLATE independently
/// of the constant the service crate declares. The workflow name is substituted, so
/// every row of the table checks the same template over a different name.
fn delete_blocked_by_job(workflow_name: &str) -> String {
    format!("Job associated with workflow {workflow_name} is currently running, failed to delete")
}

/// A poll gap this test treats as prompt: shorter than the span a job spends in any
/// one non-terminal status, so a loop polling this often observes every status.
const PROMPT_POLL: Duration = Duration::from_millis(250);

/// A KMS ARN, as `OutputSource$KMSArn` models it (`^arn:aws:kms:.*:[0-9]+:.*$`).
fn kms_arn() -> String {
    format!("arn:aws:kms:{REGION}:000000000000:key/11111111-2222-3333-4444-555555555555")
}

/// `POST /matchingworkflows/{workflowName}/jobs` — `StartMatchingJob`. The operation
/// models no body members at all (`workflowName` is its only input, an @httpLabel), so
/// the request carries no payload.
async fn start_matching_job(emulator: &Emulator, workflow_name: &str) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/jobs"),
            None,
        )
        .await
}

/// `GET /matchingworkflows/{workflowName}/jobs/{jobId}` — `GetMatchingJob`.
async fn get_matching_job(emulator: &Emulator, workflow_name: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/matchingworkflows/{workflow_name}/jobs/{job_id}"),
            None,
        )
        .await
}

/// One workflow to run jobs against, plus what the rule says a job of it must report
/// as its `outputSourceConfig`.
struct JobCase {
    case: &'static str,
    workflow_name: &'static str,
    request: Value,
    /// The single `JobOutputSource` a job of this workflow reports, or `None` when the
    /// workflow's output configuration carries no S3 path for a job to write to.
    expected_output_source: Option<Value>,
}

/// The schema mappings every row's `inputSourceConfig` references.
fn schema_names() -> [&'static str; 3] {
    ["customers", "orders-1", "devices_0"]
}

/// One minimal `CreateSchemaMapping` request. `mappedInputFields` is `length 2..60`
/// (`smithy_query(query="operation",operation="CreateSchemaMapping")`), so two fields
/// is the modelled minimum.
fn schema_request(name: &str) -> Value {
    json!({
        "schemaName": name,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "name", "type": "NAME" },
        ],
    })
}

/// The `StartMatchingJob` / `GetMatchingJob` table.
fn job_cases() -> Vec<JobCase> {
    vec![
        JobCase {
            case: "rule matching, one output source with an S3 path and no KMS key",
            workflow_name: "job-wf-rules",
            request: json!({
                "workflowName": "job-wf-rules",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers" },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/rules",
                    "output": [{ "name": "id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleBasedProperties": {
                        "rules": [{ "ruleName": "by name", "matchingKeys": ["name"] }],
                        "attributeMatchingModel": "ONE_TO_ONE",
                    },
                },
                "roleArn": role_arn(),
            }),
            expected_output_source: Some(json!({
                "roleArn": role_arn(),
                "outputS3Path": "s3://er-output/rules",
            })),
        },
        JobCase {
            case: "ML matching over three input sources, output source with a KMS key",
            workflow_name: "job_wf_ml_0",
            request: json!({
                "workflowName": "job_wf_ml_0",
                "description": "nightly consolidation",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers" },
                    { "inputSourceARN": glue_table_arn("orders"), "schemaName": "orders-1" },
                    { "inputSourceARN": glue_table_arn("devices"), "schemaName": "devices_0" },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/ml",
                    "KMSArn": kms_arn(),
                    "output": [{ "name": "id" }, { "name": "name", "hashed": true }],
                    "applyNormalization": true,
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            expected_output_source: Some(json!({
                "roleArn": role_arn(),
                "outputS3Path": "s3://er-output/ml",
                "KMSArn": kms_arn(),
            })),
        },
        JobCase {
            case: "an uppercase workflow name and a deep output path with a KMS key",
            workflow_name: "JOBWF2",
            request: json!({
                "workflowName": "JOBWF2",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("devices"), "schemaName": "devices_0" },
                ],
                // `OutputSourceConfig` is itself `length 1..1`
                // (`smithy_query(query="shape",shape="OutputSourceConfig")`), so a
                // workflow carries exactly one output source and a job's own
                // `length 1..1` config is derived from that one.
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output-2/matches/2026/01",
                    "KMSArn": kms_arn(),
                    "output": [{ "name": "id" }, { "name": "name" }, { "name": "email" }],
                    "applyNormalization": false,
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            expected_output_source: Some(json!({
                "roleArn": role_arn(),
                "outputS3Path": "s3://er-output-2/matches/2026/01",
                "KMSArn": kms_arn(),
            })),
        },
        JobCase {
            case: "no output S3 path configured: a job reports no outputSourceConfig",
            workflow_name: "job3",
            request: json!({
                "workflowName": "job3",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("people"), "schemaName": "orders-1" },
                ],
                // `outputS3Path` is modelled optional with the empty string as its
                // default, so a workflow can exist with no path for a job to write to.
                "outputSourceConfig": [{ "output": [{ "name": "id" }] }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "roleArn": role_arn(),
            }),
            expected_output_source: None,
        },
    ]
}

/// Create the schema mappings and the workflows of `job_cases`, so each row runs
/// against a store that holds them.
async fn seed(emulator: &Emulator) {
    for name in schema_names() {
        let reply = emulator.create_schema_mapping(&schema_request(name)).await;
        assert_eq!(
            reply.status, 200,
            "seeding schema mapping {name:?} failed: {reply:?}"
        );
    }
    for case in job_cases() {
        let reply = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: seeding the matching workflow failed: {reply:?}",
            case.case
        );
    }
}

/// The index of `status` in [`LIFECYCLE`], or `None` when it is not a status this
/// lifecycle ever reaches.
fn lifecycle_position(status: &str) -> Option<usize> {
    LIFECYCLE.iter().position(|known| *known == status)
}

fn status_of(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from GetMatchingJob, got {reply:?}"
    );
    string_member(&reply.body, "status")
}

/// What polling a job to its terminal status observed.
struct Settled {
    /// The last reply, i.e. the terminal job.
    reply: Reply,
    /// Every DISTINCT status observed, in the order they were observed.
    seen: Vec<String>,
    /// The longest gap between two consecutive polls. The emulator advances a job on
    /// the clock, so a poll loop that was itself starved for longer than a status
    /// lasts can miss an intermediate status; the caller uses this to tell "the status
    /// never happened" from "this test was too slow to look".
    max_gap: Duration,
}

/// Poll `GetMatchingJob` until the job is terminal, asserting as it goes that the
/// status only ever moves FORWARD through [`LIFECYCLE`] and that `endTime` and
/// `errorDetails` appear only on the terminal status.
async fn settle(case: &str, emulator: &Emulator, workflow_name: &str, job_id: &str) -> Settled {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    let mut max_gap = Duration::ZERO;
    let mut last_poll: Option<Instant> = None;
    loop {
        let reply = get_matching_job(emulator, workflow_name, job_id).await;
        let now = Instant::now();
        if let Some(previous) = last_poll {
            max_gap = max_gap.max(now.duration_since(previous));
        }
        last_poll = Some(now);
        let status = status_of(case, &reply);
        let position = lifecycle_position(&status).unwrap_or_else(|| {
            panic!("case {case:?}: {status:?} is not a status of this lifecycle: {LIFECYCLE:?}")
        });
        if let Some(previous) = seen.last() {
            let before = lifecycle_position(previous).expect("an observed status is in the table");
            assert!(
                position >= before,
                "case {case:?}: the status went backwards, from {previous:?} to {status:?}"
            );
        }
        if seen.last().map(String::as_str) != Some(status.as_str()) {
            seen.push(status.clone());
        }

        let terminal = status == "FAILED" || status == "SUCCEEDED";
        assert_eq!(
            reply.body.get("endTime").is_some(),
            terminal,
            "case {case:?}: endTime must be present exactly on a terminal status, saw \
             status {status:?} in {:?}",
            reply.body
        );
        assert_eq!(
            reply.body.get("errorDetails").is_some(),
            terminal,
            "case {case:?}: errorDetails must be present exactly on the failed status, \
             saw status {status:?} in {:?}",
            reply.body
        );
        if terminal {
            return Settled {
                reply,
                seen,
                max_gap,
            };
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job did not reach a terminal status within {SETTLE_TIMEOUT:?}, \
             saw {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn start_matching_job_mints_a_job_get_matching_job_reads_it_back() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let mut job_ids: BTreeSet<String> = BTreeSet::new();
    for case in job_cases() {
        let label = case.case;
        let name = case.workflow_name;

        let started = start_matching_job(&emulator, name).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: expected HTTP 200 from StartMatchingJob, got {started:?}"
        );
        let job_id = string_member(&started.body, "jobId");
        assert!(
            job_id.len() == 32 && job_id.chars().all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch)),
            "case {label:?}: jobId must match the modelled ^[a-f0-9]{{32}}$, got {job_id:?}"
        );
        assert!(
            job_ids.insert(job_id.clone()),
            "case {label:?}: jobId {job_id:?} was already minted for another job"
        );

        // Read straight after the start: the first status is the one the job was
        // started in, so a caller acting immediately still observes it.
        let read = get_matching_job(&emulator, name, &job_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetMatchingJob, got {read:?}"
        );
        assert_eq!(
            string_member(&read.body, "jobId"),
            job_id,
            "case {label:?}: GetMatchingJob must answer the job that was started, got {:?}",
            read.body
        );
        assert_eq!(
            string_member(&read.body, "status"),
            "QUEUED",
            "case {label:?}: a job just started must read QUEUED, got {:?}",
            read.body
        );
        let start_time = read
            .body
            .get("startTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: startTime is not a timestamp in {:?}", read.body));
        assert!(
            start_time > 0.0,
            "case {label:?}: startTime must be the instant the job started, got {start_time}"
        );
        assert_eq!(
            read.body.get("outputSourceConfig").cloned(),
            case.expected_output_source
                .clone()
                .map(|source| Value::Array(vec![source])),
            "case {label:?}: outputSourceConfig must be derived from the workflow, got {:?}",
            read.body
        );
        // Nothing ran yet: a job just started has counted nothing.
        assert!(
            read.body.get("metrics").is_none(),
            "case {label:?}: a job whose run has not finished must report no metrics, got {:?}",
            read.body
        );

        // The clock-owed lifecycle, and the terminal failure that says why no match
        // output exists.
        let settled = settle(label, &emulator, name, &job_id).await;
        // Every status the lifecycle passes through, when this test's own polling was
        // prompt enough to see them. The guard only skips the INTERMEDIATE status
        // check: the emulator advances a job on the clock, so a starved poll loop can
        // step over `RUNNING`, which is a fact about the test, not about the service.
        if settled.max_gap < PROMPT_POLL {
            assert_eq!(
                settled.seen,
                LIFECYCLE,
                "case {label:?}: a job must pass through every status of the lifecycle in \
                 order (longest poll gap {:?})",
                settled.max_gap
            );
        }
        let settled = settled.reply;
        assert_eq!(
            string_member(&settled.body, "status"),
            "FAILED",
            "case {label:?}: a job this emulator cannot execute must end FAILED, got {:?}",
            settled.body
        );
        let end_time = settled
            .body
            .get("endTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: endTime is not a timestamp in {:?}", settled.body));
        assert!(
            end_time >= start_time,
            "case {label:?}: endTime {end_time} must not precede startTime {start_time}"
        );
        let error_message = settled
            .body
            .get("errorDetails")
            .map(|details| string_member(details, "errorMessage"))
            .unwrap_or_else(|| panic!("case {label:?}: no errorDetails in {:?}", settled.body));
        assert!(
            !error_message.is_empty() && error_message.len() <= 2048,
            "case {label:?}: errorMessage must satisfy the modelled length 1..2048, got \
             {error_message:?}"
        );
        assert_eq!(
            settled.body.get("outputSourceConfig").cloned(),
            case.expected_output_source
                .clone()
                .map(|source| Value::Array(vec![source])),
            "case {label:?}: the settled job must keep the outputSourceConfig it started \
             with, got {:?}",
            settled.body
        );

        // A settled job is stable: a second read answers exactly the same body, so
        // reading never changes what a later caller sees.
        let again = get_matching_job(&emulator, name, &job_id).await;
        assert_eq!(
            again.body, settled.body,
            "case {label:?}: two reads of a settled job must be identical"
        );
    }
}

#[tokio::test]
async fn a_workflow_with_a_job_in_progress_refuses_a_second_start_until_it_settles() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in job_cases() {
        let label = case.case;
        let name = case.workflow_name;

        let first = start_matching_job(&emulator, name).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from the first StartMatchingJob, got {first:?}"
        );
        let first_id = string_member(&first.body, "jobId");

        let refused = start_matching_job(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: a second start while a job is in progress must be refused, got \
             {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {refused:?}"
        );
        assert!(
            refused.body.get("jobId").is_none(),
            "case {label:?}: a refused start must mint no job, got {:?}",
            refused.body
        );

        // The refusal is about the job that is running, not about the workflow: the
        // first job is still there and still readable.
        let read = get_matching_job(&emulator, name, &first_id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the in-progress job must still be readable, got {read:?}"
        );

        settle(label, &emulator, name, &first_id).await;

        // Once the first job is terminal the slot is free again, and the new job is a
        // DIFFERENT one.
        let second = start_matching_job(&emulator, name).await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: a start after the previous job settled must succeed, got {second:?}"
        );
        let second_id = string_member(&second.body, "jobId");
        assert_ne!(
            second_id, first_id,
            "case {label:?}: a new start must mint a new jobId"
        );
        assert_eq!(
            string_member(&get_matching_job(&emulator, name, &second_id).await.body, "status"),
            "QUEUED",
            "case {label:?}: the new job must start its own lifecycle"
        );
        // And the earlier job is untouched by the new one.
        assert_eq!(
            string_member(&get_matching_job(&emulator, name, &first_id).await.body, "status"),
            "FAILED",
            "case {label:?}: starting a new job must not revive the previous one"
        );
    }
}

#[tokio::test]
async fn an_unknown_workflow_or_job_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // A well-formed `jobId` (`^[a-f0-9]{32}$`) this service never minted.
    let unminted = "0123456789abcdef0123456789abcdef";

    // One job per workflow, so a job id can be offered to the WRONG workflow.
    let mut minted: Vec<(&'static str, String)> = Vec::new();
    for case in job_cases() {
        let reply = start_matching_job(&emulator, case.workflow_name).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200 from StartMatchingJob, got {reply:?}",
            case.case
        );
        minted.push((case.workflow_name, string_member(&reply.body, "jobId")));
    }

    // Workflow names that exist nowhere in the store, spanning the character classes
    // the modelled `EntityName` admits plus its `length` bounds (1 and 255).
    let absent_workflows = [
        "absent",
        "ABSENT-0",
        "absent_workflow-9",
        "x",
        &"a".repeat(255),
    ];
    for workflow_name in absent_workflows {
        let started = start_matching_job(&emulator, workflow_name).await;
        assert_eq!(
            started.status, 404,
            "case {workflow_name:?}: StartMatchingJob on an absent workflow must answer 404, \
             got {started:?}"
        );
        assert_eq!(
            started.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {workflow_name:?}: expected ResourceNotFoundException, got {started:?}"
        );

        let read = get_matching_job(&emulator, workflow_name, unminted).await;
        assert_eq!(
            read.status, 404,
            "case {workflow_name:?}: GetMatchingJob on an absent workflow must answer 404, \
             got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {workflow_name:?}: expected ResourceNotFoundException, got {read:?}"
        );
    }

    for (workflow_name, job_id) in &minted {
        // A job id nobody minted, on a workflow that DOES exist.
        let read = get_matching_job(&emulator, workflow_name, unminted).await;
        assert_eq!(
            read.status, 404,
            "case {workflow_name:?}: an unminted jobId must answer 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {workflow_name:?}: expected ResourceNotFoundException, got {read:?}"
        );

        // A job id that exists, offered to every OTHER workflow: a job belongs to the
        // one workflow it was started on.
        for (other, _) in &minted {
            if other == workflow_name {
                continue;
            }
            let crossed = get_matching_job(&emulator, other, job_id).await;
            assert_eq!(
                crossed.status, 404,
                "case {other:?}: the job of {workflow_name:?} must not be readable here, got \
                 {crossed:?}"
            );
            assert_eq!(
                crossed.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {other:?}: expected ResourceNotFoundException, got {crossed:?}"
            );
        }

        // And it IS readable on its own workflow.
        let own = get_matching_job(&emulator, workflow_name, job_id).await;
        assert_eq!(
            own.status, 200,
            "case {workflow_name:?}: its own job must be readable, got {own:?}"
        );
    }
}

#[tokio::test]
async fn jobs_of_different_workflows_run_independently() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Every workflow gets a job: one workflow's in-progress job must not block
    // another's start, and each job keeps its own workflow's output configuration.
    let mut jobs: Vec<(&'static str, String, Option<Value>)> = Vec::new();
    for case in job_cases() {
        let reply = start_matching_job(&emulator, case.workflow_name).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: a job of another workflow must not block this start, got {reply:?}",
            case.case
        );
        jobs.push((
            case.workflow_name,
            string_member(&reply.body, "jobId"),
            case.expected_output_source.clone(),
        ));
    }

    for (workflow_name, job_id, expected_output_source) in &jobs {
        let settled = settle(workflow_name, &emulator, workflow_name, job_id)
            .await
            .reply;
        assert_eq!(
            string_member(&settled.body, "jobId"),
            *job_id,
            "case {workflow_name:?}: each workflow must read back its OWN job"
        );
        assert_eq!(
            settled.body.get("outputSourceConfig").cloned(),
            expected_output_source
                .clone()
                .map(|source| Value::Array(vec![source])),
            "case {workflow_name:?}: each job keeps its own workflow's output config, got {:?}",
            settled.body
        );
    }
}

#[tokio::test]
async fn a_workflow_with_a_job_in_progress_cannot_be_deleted_until_the_job_settles() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in job_cases() {
        let label = case.case;
        let name = case.workflow_name;

        let started = start_matching_job(&emulator, name).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: expected HTTP 200 from StartMatchingJob, got {started:?}"
        );
        let job_id = string_member(&started.body, "jobId");

        let refused = emulator.delete_matching_workflow(name).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: deleting a workflow whose job is in progress must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {refused:?}"
        );
        assert_eq!(
            string_member(&refused.body, "message"),
            delete_blocked_by_job(name),
            "case {label:?}: the refusal must name the workflow whose job blocks the delete"
        );

        // A refused delete removes nothing: the workflow and its job are both still
        // there.
        assert_eq!(
            emulator.get_matching_workflow(name).await.status,
            200,
            "case {label:?}: a refused delete must leave the workflow in place"
        );
        assert_eq!(
            get_matching_job(&emulator, name, &job_id).await.status,
            200,
            "case {label:?}: a refused delete must leave the job in place"
        );

        // The conflict clears itself once the run is over — nothing else has to happen.
        settle(label, &emulator, name, &job_id).await;
        let deleted = emulator.delete_matching_workflow(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: deleting a workflow whose jobs have settled must succeed, got \
             {deleted:?}"
        );
        assert_eq!(
            emulator.get_matching_workflow(name).await.status,
            404,
            "case {label:?}: the deleted workflow must be gone"
        );

        // The workflow's job history went with it, so a workflow recreated under the
        // same name inherits no runs.
        let recreated = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: the workflow must be creatable again, got {recreated:?}"
        );
        let orphan = get_matching_job(&emulator, name, &job_id).await;
        assert_eq!(
            orphan.status, 404,
            "case {label:?}: the deleted workflow's job must not come back with it, got \
             {orphan:?}"
        );
        assert_eq!(
            orphan.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {orphan:?}"
        );
        // And the recreated workflow can be run again straight away: no job of the old
        // one is holding its slot.
        let restarted = start_matching_job(&emulator, name).await;
        assert_eq!(
            restarted.status, 200,
            "case {label:?}: the recreated workflow must be startable, got {restarted:?}"
        );
        assert_ne!(
            string_member(&restarted.body, "jobId"),
            job_id,
            "case {label:?}: the recreated workflow's job must be a new one"
        );
    }
}
