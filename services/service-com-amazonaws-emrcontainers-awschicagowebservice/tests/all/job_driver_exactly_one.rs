// SPDX-License-Identifier: CC0-1.0

//! The `JobDriver` shape rule, at every place this service accepts one.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rule under test, asserted over a TABLE of `JobDriver` values rather than one
//! request: `JobDriver` is "the driver that the job runs on. Exactly one of the two
//! available job drivers is required, either sparkSqlJobDriver or sparkSubmitJobDriver"
//! (smithy_query: shape `JobDriver`), while BOTH of its members are optional in the model
//! — so a value naming neither driver, and one naming both, has to be refused with
//! `ValidationException` (HTTP 400) wherever such a value is accepted, and nothing stored
//! for it. The two places are the submission that carries a driver (`StartJobRun`) and
//! the job template that stores one for a later submission to run
//! (`CreateJobTemplate`). A value naming exactly one driver is accepted at both and
//! reads back unchanged, whichever of the two it names and whichever of that driver's own
//! optional members it fills in.
//!
//! The rule is about the value the caller SENT, not only about the one the run ends up
//! running: a submission that names a `jobTemplateId` takes the run's driver from the
//! template, and a `jobDriver` the request carries beside it is still a `JobDriver` the
//! caller sent, so it is judged by the same rule.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{field, text, Emr};

/// One row: a `JobDriver` value and whether the rule admits it.
struct DriverCase {
    label: &'static str,
    driver: Value,
    /// True when the value names exactly one of the two drivers.
    accepted: bool,
}

/// The table. Rows differ along which of the two members the value names, how many of
/// them it names, and which of the named driver's own optional members are filled in —
/// the member names and their optionality are the model's (smithy_query: shapes
/// `JobDriver`, `SparkSubmitJobDriver`, `SparkSqlJobDriver`).
fn cases() -> Vec<DriverCase> {
    vec![
        DriverCase {
            label: "a bare sparkSubmitJobDriver",
            driver: json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}}),
            accepted: true,
        },
        DriverCase {
            label: "a sparkSubmitJobDriver with every optional member",
            driver: json!({"sparkSubmitJobDriver": {
                "entryPoint": "s3://bucket/main.py",
                "entryPointArguments": ["--rows", "10"],
                "sparkSubmitParameters": "--conf spark.executor.instances=2",
            }}),
            accepted: true,
        },
        DriverCase {
            label: "a sparkSqlJobDriver naming a file",
            driver: json!({"sparkSqlJobDriver": {"entryPoint": "s3://bucket/query.sql"}}),
            accepted: true,
        },
        DriverCase {
            label: "a sparkSqlJobDriver carrying only its parameters",
            driver: json!({"sparkSqlJobDriver": {"sparkSqlParameters": "-e select 1"}}),
            accepted: true,
        },
        DriverCase {
            // Every member of `SparkSqlJobDriver` is optional, so this still names one of
            // the two drivers and the rule is about the drivers, not their contents.
            label: "a sparkSqlJobDriver with none of its own members",
            driver: json!({"sparkSqlJobDriver": {}}),
            accepted: true,
        },
        DriverCase {
            label: "neither driver",
            driver: json!({}),
            accepted: false,
        },
        DriverCase {
            label: "both drivers",
            driver: json!({
                "sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"},
                "sparkSqlJobDriver": {"entryPoint": "s3://bucket/query.sql"},
            }),
            accepted: false,
        },
        DriverCase {
            label: "both drivers, the second one empty",
            driver: json!({
                "sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"},
                "sparkSqlJobDriver": {},
            }),
            accepted: false,
        },
    ]
}

/// A `jobTemplateData` carrying `driver`: its other two members are `@required` and are
/// not what this suite varies.
fn template_data(driver: &Value) -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": driver,
    })
}

/// Assert a refusal names the member the bad value sat at and both drivers it could have
/// named, so the caller learns which value to repair and what it may say.
fn assert_refusal_names_the_rule(body: &Value, case: &str, member: &str) {
    let reported = text(body, "/message", case);
    for expected in [member, "sparkSqlJobDriver", "sparkSubmitJobDriver"] {
        assert!(
            reported.contains(expected),
            "case {case:?}: the refusal must name {expected}, got {reported:?}"
        );
    }
}

/// How many job templates the store holds.
async fn template_count(emr: &Emr, case: &str) -> usize {
    let listed = emr.send("GET", "/jobtemplates", None).await;
    field(listed.ok(case), "/templates", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: templates is not a list"))
        .len()
}

/// How many job runs a cluster holds.
async fn run_count(emr: &Emr, cluster: &str, case: &str) -> usize {
    let listed = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    field(listed.ok(case), "/jobRuns", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: jobRuns is not a list"))
        .len()
}

/// The rule at `CreateJobTemplate`: a template stores the driver a run is started with,
/// so a value naming neither driver or both is refused and no template is stored, while
/// one naming exactly one is stored and read back unchanged through
/// `DescribeJobTemplate`.
#[tokio::test]
async fn a_job_template_stores_a_driver_only_when_it_names_exactly_one() {
    for case in cases() {
        let label = case.label;
        // One emulator per row, so the store counts below are about THIS row.
        let emr = Emr::new();
        let answer = emr
            .send(
                "POST",
                "/jobtemplates",
                Some(json!({
                    "name": "driver.rule",
                    "clientToken": "tok-driver-rule",
                    "jobTemplateData": template_data(&case.driver),
                })),
            )
            .await;

        if !case.accepted {
            answer.refused(label, 400, "ValidationException");
            assert_refusal_names_the_rule(&answer.body, label, "jobTemplateData.jobDriver");
            assert_eq!(
                template_count(&emr, label).await,
                0,
                "case {label:?}: a refused create must store no job template"
            );
            continue;
        }

        let id = text(answer.ok(label), "/id", label);
        let described = emr.send("GET", &format!("/jobtemplates/{id}"), None).await;
        let template = field(described.ok(label), "/jobTemplate", label);
        assert_eq!(
            template.pointer("/jobTemplateData/jobDriver"),
            Some(&case.driver),
            "case {label:?}: the stored driver did not round-trip, got {template}"
        );
        assert_eq!(
            template_count(&emr, label).await,
            1,
            "case {label:?}: the accepted template must be the store's only one"
        );

        // The point of storing the driver: a run started from the template runs it.
        let cluster = emr.cluster("driver-rule", "tok-driver-cluster").await;
        let started = emr
            .start_job(
                &cluster,
                json!({"clientToken": "tok-driver-run", "jobTemplateId": id}),
            )
            .await;
        let run = text(started.ok(label), "/id", label);
        let run_described = emr.describe_job(&cluster, &run).await;
        let job_run = field(run_described.ok(label), "/jobRun", label);
        assert_eq!(
            job_run.get("jobDriver"),
            Some(&case.driver),
            "case {label:?}: the run must carry the template's driver, got {job_run}"
        );
    }
}

/// The same rule at `StartJobRun`, on the driver the submission carries itself: a value
/// naming neither driver or both is refused and no run is created, while one naming
/// exactly one is accepted and reads back unchanged through `DescribeJobRun`.
#[tokio::test]
async fn a_job_run_is_submitted_only_with_a_driver_that_names_exactly_one() {
    for case in cases() {
        let label = case.label;
        let emr = Emr::new();
        let cluster = emr.cluster("driver-rule", "tok-driver-cluster").await;
        let token = "tok-driver-submission";
        let body = json!({
            "clientToken": token,
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.10.0-latest",
            "jobDriver": case.driver,
        });
        let answer = emr.start_job(&cluster, body.clone()).await;

        if !case.accepted {
            answer.refused(label, 400, "ValidationException");
            assert_refusal_names_the_rule(&answer.body, label, "jobDriver");
            assert_eq!(
                run_count(&emr, &cluster, label).await,
                0,
                "case {label:?}: a refused submission must create no job run"
            );
            // Nothing was created, so the token was never spent: the same token carrying
            // a driver that does name one is accepted.
            let repaired = emr
                .start_job(
                    &cluster,
                    json!({
                        "clientToken": token,
                        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                        "releaseLabel": "emr-6.10.0-latest",
                        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/ok.py"}},
                    }),
                )
                .await;
            repaired.ok(label);
            continue;
        }

        let id = text(answer.ok(label), "/id", label);
        let described = emr.describe_job(&cluster, &id).await;
        let job_run = field(described.ok(label), "/jobRun", label);
        assert_eq!(
            job_run.get("jobDriver"),
            Some(&case.driver),
            "case {label:?}: the submitted driver did not round-trip, got {job_run}"
        );
        assert_eq!(
            run_count(&emr, &cluster, label).await,
            1,
            "case {label:?}: the accepted submission must be the cluster's only run"
        );
    }
}

/// The same rule at `StartJobRun` when the submission ALSO names a job template: the run's
/// driver comes from the template, but the `jobDriver` the request carried is a `JobDriver`
/// the caller sent, so a value naming neither driver or both is refused and no run is
/// created, while one naming exactly one is accepted and the run still runs the template's
/// driver.
#[tokio::test]
async fn a_templated_submission_is_refused_for_the_driver_it_carries_itself() {
    // The driver the template stores, so the run always has one that names exactly one
    // driver whatever the request carried: the refusals below are about the request's
    // value alone.
    let stored = json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/template.py"}});
    for case in cases() {
        let label = case.label;
        let emr = Emr::new();
        let cluster = emr.cluster("driver-rule", "tok-driver-cluster").await;
        let created = emr
            .send(
                "POST",
                "/jobtemplates",
                Some(json!({
                    "name": "driver.rule.templated",
                    "clientToken": "tok-driver-templated",
                    "jobTemplateData": template_data(&stored),
                })),
            )
            .await;
        let template = text(created.ok(label), "/id", label);

        let answer = emr
            .start_job(
                &cluster,
                json!({
                    "clientToken": "tok-driver-templated-run",
                    "jobTemplateId": template,
                    "jobDriver": case.driver,
                }),
            )
            .await;

        if !case.accepted {
            answer.refused(label, 400, "ValidationException");
            assert_refusal_names_the_rule(&answer.body, label, "jobDriver");
            assert_eq!(
                run_count(&emr, &cluster, label).await,
                0,
                "case {label:?}: a refused submission must create no job run"
            );
            continue;
        }

        let run = text(answer.ok(label), "/id", label);
        let described = emr.describe_job(&cluster, &run).await;
        let job_run = field(described.ok(label), "/jobRun", label);
        assert_eq!(
            job_run.get("jobDriver"),
            Some(&stored),
            "case {label:?}: the run must carry the template's driver, got {job_run}"
        );
    }
}
