// SPDX-License-Identifier: CC0-1.0

//! What a `StartJobRun` that names no job template has to carry.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request: a job
//! run cannot exist without the values `JobTemplateData` marks `@required`
//! (`executionRoleArn`, `releaseLabel`, `jobDriver`), so a submission that names no
//! `jobTemplateId` to supply them must carry all three itself; one missing any of them is
//! refused with `ValidationException` (HTTP 400) and nothing is created. The same members
//! are NOT required of a submission that does name a template.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{field, runnable_body, text, Emr};

/// The members a run cannot exist without: the `@required` members of `JobTemplateData`,
/// "the values of StartJobRun API requests used in job runs started using the job
/// template" (smithy_query: shape `JobTemplateData`).
const REQUIRED: [&str; 3] = ["executionRoleArn", "releaseLabel", "jobDriver"];

/// The `JobDriver` union members the model declares (smithy_query: shape `JobDriver`), so
/// the rule is asserted about the members that are ABSENT and not about one driver shape.
fn drivers() -> Vec<(&'static str, Value)> {
    vec![
        (
            "spark-submit",
            json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}}),
        ),
        (
            "spark-sql",
            json!({"sparkSqlJobDriver": {"entryPoint": "s3://bucket/query.sql"}}),
        ),
    ]
}

/// Every non-empty subset of [`REQUIRED`], as (label, omitted members): the rule holds for
/// one missing member, for two, and for all three.
fn omissions() -> Vec<(String, Vec<&'static str>)> {
    let mut rows = Vec::new();
    for mask in 1u8..(1 << REQUIRED.len()) {
        let omitted: Vec<&'static str> = REQUIRED
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, member)| *member)
            .collect();
        rows.push((format!("without {}", omitted.join(" and ")), omitted));
    }
    rows
}

/// How many job runs the cluster holds.
async fn run_count(emr: &Emr, cluster: &str, case: &str) -> usize {
    let listed = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    field(listed.ok(case), "/jobRuns", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: jobRuns is not a list"))
        .len()
}

#[tokio::test]
async fn a_submission_missing_a_required_member_is_refused_and_creates_nothing() {
    let emr = Emr::new();

    for (driver_label, driver) in drivers() {
        for (index, (omission_label, omitted)) in omissions().into_iter().enumerate() {
            let case = format!("{driver_label}, {omission_label}");
            // One cluster per row, so the run count below is about THIS row's submission.
            let cluster = emr
                .cluster(
                    &format!("required-{driver_label}-{index}"),
                    &format!("tok-required-{driver_label}-{index}"),
                )
                .await;
            let token = format!("tok-{driver_label}-{index}");
            let mut body = runnable_body(&token, driver.clone());
            for member in &omitted {
                body.as_object_mut()
                    .expect("the body is a JSON object")
                    .remove(*member);
            }

            let answer = emr.start_job(&cluster, body).await;
            answer.refused(&case, 400, "ValidationException");
            // The refusal is built from the request, so it names exactly the members this
            // row left out.
            let reported = text(&answer.body, "/message", &case);
            let named: Vec<&str> = reported
                .split_once("missing: ")
                .map(|(_, tail)| tail.split(", ").collect())
                .unwrap_or_default();
            assert_eq!(
                named, omitted,
                "case {case:?}: the refusal must name exactly the missing members, got {reported:?}"
            );
            assert_eq!(
                run_count(&emr, &cluster, &case).await,
                0,
                "case {case:?}: a refused submission must create no job run"
            );

            // Nothing was created, so the idempotency token was never spent: the same
            // token now carrying a complete submission must be accepted and readable.
            let repaired = emr
                .start_job(&cluster, runnable_body(&token, driver.clone()))
                .await;
            let id = text(repaired.ok(&case), "/id", &case);
            let described = emr.describe_job(&cluster, &id).await;
            let job_run = field(described.ok(&case), "/jobRun", &case);
            assert_eq!(
                text(job_run, "/id", &case),
                id,
                "case {case:?}: the described run is not the repaired submission"
            );
            for member in REQUIRED {
                assert!(
                    job_run.get(member).is_some(),
                    "case {case:?}: the stored run must carry {member}, got {job_run}"
                );
            }
            assert_eq!(
                run_count(&emr, &cluster, &case).await,
                1,
                "case {case:?}: the repaired submission must be the cluster's only run"
            );
        }
    }
}

#[tokio::test]
async fn a_submission_naming_a_job_template_is_not_refused_for_those_members() {
    let emr = Emr::new();
    let cluster = emr.cluster("templated", "tok-templated").await;
    let case = "a submission that names a job template";
    let created = emr
        .send(
            "POST",
            "/jobtemplates",
            Some(json!({
                "name": "supplies-the-values",
                "clientToken": "tok-template-values",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.10.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
                },
            })),
        )
        .await;
    let template = text(created.ok(case), "/id", case);

    // A template carries the three values, so a request naming one is not subject to the
    // rule. This emulator does not apply a template's `jobTemplateData` to a run yet, so
    // such a submission is refused for THAT reason — never for members a template supplies.
    let answer = emr
        .start_job(
            &cluster,
            json!({"clientToken": "tok-templated-run", "jobTemplateId": template}),
        )
        .await;
    if answer.status != 200 {
        let reported = text(&answer.body, "/message", case);
        for member in REQUIRED {
            assert!(
                !reported.contains(&format!("missing: {member}")),
                "case {case:?}: a template supplies {member}, so the refusal must not call it missing, got {reported:?}"
            );
        }
    }
}
