// SPDX-License-Identifier: CC0-1.0

//! `StartJobRun` built from a stored job template, end to end.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` (the harness [`crate::virtual_cluster_job_run_loop`] owns) and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A submission that names a `jobTemplateId` runs the values the template stores:
//!    `JobTemplateData` is "the values of StartJobRun API requests used in job runs
//!    started using the job template" (smithy_query: shape `JobTemplateData`), so the
//!    run's `executionRoleArn`, `releaseLabel`, `jobDriver` and `configurationOverrides`
//!    come from the template and its `jobTags` are applied to the run, with every
//!    `${name}` substituted from `jobTemplateParameters` and, where the request supplies
//!    none, from the `defaultValue` the template's `parameterConfiguration` declares.
//! 2. A template the request cannot be reconciled with is refused with the modelled
//!    `ValidationException` (HTTP 400) and creates nothing: a parameter the template does
//!    not declare, a reference nothing resolves, a value that is not of the declared
//!    `TemplateParameterDataType`, a template whose substituted value cannot be the run's
//!    member (a monitoring destination with no name, a `persistentAppUI` no enum member
//!    names, an `executionRoleArn`, `releaseLabel` or `entryPoint` the run's own
//!    constraint rejects), and an application past a bound the emulator sets on one call
//!    (the substitution budget, the run's modelled tag ceiling).
//! 3. The template is what the run runs with: where the request carries one of the values
//!    the template stores, the template's is the one the run gets ("enforcing certain
//!    values in StartJobRun API request" — smithy_query: operation `CreateJobTemplate`),
//!    while what the template stores nothing about stays the request's.
//! 4. Applying a template neither consumes nor changes it: the `@idempotencyToken`
//!    replayed names the one run that was minted, and the template reads back unchanged.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::start_job_run::MAX_SUBSTITUTED_CHARACTERS;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state::MAX_RESOURCE_TAGS;

use crate::virtual_cluster_job_run_loop::{assert_resource_id, field, text, Answer, Emr};

/// An `IAMRoleArn` the run's own member accepts (smithy_query: shape `IAMRoleArn`).
const ROLE: &str = "arn:aws:iam::000000000000:role/EMRJobRole";

/// `POST /jobtemplates` — `CreateJobTemplate`.
async fn create_template(emr: &Emr, name: &str, token: &str, data: Value) -> Answer {
    emr.send(
        "POST",
        "/jobtemplates",
        Some(json!({"name": name, "clientToken": token, "jobTemplateData": data})),
    )
    .await
}

/// `GET /jobtemplates/{id}` — `DescribeJobTemplate`.
async fn describe_template(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/jobtemplates/{id}"), None).await
}

/// The runs `ListJobRuns` reports for `cluster`.
async fn listed_runs(emr: &Emr, cluster: &str, case: &str) -> Vec<Value> {
    let answer = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns"), None)
        .await;
    field(answer.ok(case), "/jobRuns", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: jobRuns is not a list"))
        .clone()
}

/// One `StartJobRun` body naming a template, with or without parameters for it.
fn templated_body(token: &str, template: &str, parameters: Option<Value>) -> Value {
    let mut body = json!({"clientToken": token, "jobTemplateId": template});
    if let Some(parameters) = parameters {
        body["jobTemplateParameters"] = parameters;
    }
    body
}

/// One row of RULE 1: a stored template, the parameters the request supplies for it, and
/// the members the started run must then carry.
struct Applied {
    label: &'static str,
    data: Value,
    parameters: Option<Value>,
    /// The members of the described `jobRun` this row pins, each compared whole.
    expected: Value,
}

/// The rows differ along what the rule generalises over: whether the template references
/// parameters at all, where the value comes from (the request or the `defaultValue`), the
/// declared type, how many references one value carries, which `JobDriver` member the
/// template stores, and how deep in the `configurationOverrides` tree a reference sits.
fn applied_rows() -> Vec<Applied> {
    vec![
        Applied {
            label: "a template that references no parameter",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/plain.py"}},
            }),
            parameters: None,
            expected: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/plain.py"}},
            }),
        },
        Applied {
            label: "a STRING parameter the request supplies",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "${releaseVersion}",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {
                    "releaseVersion": {"type": "STRING", "defaultValue": "emr-6.15.0-latest"},
                },
            }),
            parameters: Some(json!({"releaseVersion": "emr-7.0.0-latest"})),
            expected: json!({"releaseLabel": "emr-7.0.0-latest"}),
        },
        Applied {
            label: "the same parameter falling back to its defaultValue",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "${releaseVersion}",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {
                    "releaseVersion": {"type": "STRING", "defaultValue": "emr-6.15.0-latest"},
                },
            }),
            parameters: None,
            expected: json!({"releaseLabel": "emr-6.15.0-latest"}),
        },
        Applied {
            label: "two references inside one longer value, one NUMBER and one STRING",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/j.py",
                    "sparkSubmitParameters": "--conf spark.executor.instances=${instances} --conf spark.executor.memory=${memory}",
                }},
                "parameterConfiguration": {
                    "instances": {"type": "NUMBER", "defaultValue": "2"},
                    "memory": {"type": "STRING", "defaultValue": "2G"},
                },
            }),
            parameters: Some(json!({"instances": "5"})),
            expected: json!({"jobDriver": {"sparkSubmitJobDriver": {
                "entryPoint": "s3://bucket/j.py",
                "sparkSubmitParameters": "--conf spark.executor.instances=5 --conf spark.executor.memory=2G",
            }}}),
        },
        Applied {
            label: "a reference in a list element and one used twice",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://${bucket}/jobs/${script}.py",
                    "entryPointArguments": ["--rows", "${rows}", "--bucket", "${bucket}"],
                }},
                "parameterConfiguration": {
                    "bucket": {"type": "STRING"},
                    "script": {"type": "STRING", "defaultValue": "nightly"},
                    "rows": {"type": "NUMBER", "defaultValue": "10"},
                },
            }),
            parameters: Some(json!({"bucket": "data-lake", "rows": "20"})),
            expected: json!({"jobDriver": {"sparkSubmitJobDriver": {
                "entryPoint": "s3://data-lake/jobs/nightly.py",
                "entryPointArguments": ["--rows", "20", "--bucket", "data-lake"],
            }}}),
        },
        Applied {
            label: "the other JobDriver member the model declares",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSqlJobDriver": {
                    "entryPoint": "s3://bucket/${query}.sql",
                    "sparkSqlParameters": "--conf spark.sql.shuffle.partitions=${parts}",
                }},
                "parameterConfiguration": {
                    "query": {"type": "STRING"},
                    "parts": {"type": "NUMBER", "defaultValue": "10"},
                },
            }),
            parameters: Some(json!({"query": "rollup"})),
            expected: json!({"jobDriver": {"sparkSqlJobDriver": {
                "entryPoint": "s3://bucket/rollup.sql",
                "sparkSqlParameters": "--conf spark.sql.shuffle.partitions=10",
            }}}),
        },
        Applied {
            label: "references throughout the configuration overrides and the job tags",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "configurationOverrides": {
                    "applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor.memory": "${memory}"},
                        "configurations": [{
                            "classification": "nested",
                            "properties": {"spark.driver.memory": "${memory}"},
                        }],
                    }],
                    "monitoringConfiguration": {
                        "persistentAppUI": "${ui}",
                        "cloudWatchMonitoringConfiguration": {
                            "logGroupName": "/emr/${stage}",
                            "logStreamNamePrefix": "${stage}",
                        },
                        "s3MonitoringConfiguration": {"logUri": "s3://logs/${stage}"},
                    },
                },
                "parameterConfiguration": {
                    "memory": {"type": "STRING", "defaultValue": "4G"},
                    "ui": {"type": "STRING", "defaultValue": "ENABLED"},
                    "stage": {"type": "STRING"},
                },
                "jobTags": {"pipeline": "${stage}"},
            }),
            parameters: Some(json!({"stage": "prod"})),
            expected: json!({
                "configurationOverrides": {
                    "applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor.memory": "4G"},
                        "configurations": [{
                            "classification": "nested",
                            "properties": {"spark.driver.memory": "4G"},
                        }],
                    }],
                    "monitoringConfiguration": {
                        "persistentAppUI": "ENABLED",
                        "cloudWatchMonitoringConfiguration": {
                            "logGroupName": "/emr/prod",
                            "logStreamNamePrefix": "prod",
                        },
                        "s3MonitoringConfiguration": {"logUri": "s3://logs/prod"},
                    },
                },
                "tags": {"pipeline": "prod"},
            }),
        },
        Applied {
            label: "a parametric executionRoleArn",
            data: json!({
                "executionRoleArn": "${roleArn}",
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {"roleArn": {"type": "STRING"}},
            }),
            parameters: Some(json!({"roleArn": "arn:aws:iam::000000000000:role/OtherRole"})),
            expected: json!({"executionRoleArn": "arn:aws:iam::000000000000:role/OtherRole"}),
        },
    ]
}

/// RULE 1: the run a template starts carries the template's values, substituted.
#[tokio::test]
async fn a_templated_run_carries_the_templates_substituted_values() {
    let emr = Emr::new();
    let cluster = emr.cluster("templated-runs", "tok-templated-runs").await;

    for (index, row) in applied_rows().into_iter().enumerate() {
        let case = row.label;
        let created = create_template(
            &emr,
            &format!("applied-{index}"),
            &format!("tok-applied-{index}"),
            row.data,
        )
        .await;
        let template = text(created.ok(case), "/id", case);

        let started = emr
            .start_job(
                &cluster,
                templated_body(&format!("run-applied-{index}"), &template, row.parameters),
            )
            .await;
        let run = text(started.ok(case), "/id", case);
        assert_resource_id(case, "the templated job run id", &run);

        let described = emr.describe_job(&cluster, &run).await;
        let job_run = field(described.ok(case), "/jobRun", case).clone();
        for (member, wanted) in row
            .expected
            .as_object()
            .expect("the row's expectation is a JSON object")
        {
            let actual = job_run.get(member);
            assert_eq!(
                actual,
                Some(wanted),
                "case {case:?}: the run's {member} must come from the template, got {actual:?}"
            );
        }

        // The other reader of the same record answers the same run: the values are
        // stored, not recomputed per reader.
        let listed = listed_runs(&emr, &cluster, case).await;
        let same = listed
            .iter()
            .find(|candidate| candidate.get("id").and_then(Value::as_str) == Some(run.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: ListJobRuns does not report run {run}"));
        assert_eq!(
            same, &job_run,
            "case {case:?}: ListJobRuns and DescribeJobRun must report the same run, got {same}"
        );
    }
}

/// One row of RULE 2: a template plus parameters a run cannot be built from, and the
/// name the refusal has to carry because the refusal is built from the request and the
/// template rather than canned.
struct Unapplicable {
    label: &'static str,
    data: Value,
    parameters: Option<Value>,
    named: &'static str,
}

/// The rows differ in WHICH reconciliation fails: an undeclared parameter, a reference
/// with no value from either source, a value of the wrong declared type (from either
/// source), and a substituted value that cannot be the run's member.
fn unapplicable_rows() -> Vec<Unapplicable> {
    let minimal = json!({
        "executionRoleArn": ROLE,
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
    });
    vec![
        Unapplicable {
            label: "a parameter the template declares nothing about",
            data: minimal.clone(),
            parameters: Some(json!({"rows": "10"})),
            named: "rows",
        },
        Unapplicable {
            label: "a declared parameter with neither a supplied value nor a default",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "${releaseVersion}",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {"releaseVersion": {"type": "STRING"}},
            }),
            parameters: None,
            named: "releaseVersion",
        },
        Unapplicable {
            label: "a reference the template declares no parameter for",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/${script}.py"}},
            }),
            parameters: None,
            named: "script",
        },
        Unapplicable {
            label: "a NUMBER parameter the request supplies a non-number for",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/j.py",
                    "sparkSubmitParameters": "--conf spark.executor.instances=${instances}",
                }},
                "parameterConfiguration": {"instances": {"type": "NUMBER"}},
            }),
            parameters: Some(json!({"instances": "five"})),
            named: "instances",
        },
        Unapplicable {
            label: "a NUMBER parameter whose defaultValue is not a number",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/j.py",
                    "sparkSubmitParameters": "--conf spark.executor.instances=${instances}",
                }},
                "parameterConfiguration": {"instances": {"type": "NUMBER", "defaultValue": "lots"}},
            }),
            parameters: None,
            named: "instances",
        },
        Unapplicable {
            label: "a CloudWatch destination the template never names",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "configurationOverrides": {"monitoringConfiguration": {
                    "cloudWatchMonitoringConfiguration": {"logStreamNamePrefix": "jobs"},
                }},
            }),
            parameters: None,
            named: "logGroupName",
        },
        Unapplicable {
            label: "an Amazon S3 destination the template never names",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "configurationOverrides": {"monitoringConfiguration": {
                    "s3MonitoringConfiguration": {},
                }},
            }),
            parameters: None,
            named: "logUri",
        },
        Unapplicable {
            label: "a persistentAppUI no PersistentAppUI member names",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "configurationOverrides": {"monitoringConfiguration": {
                    "persistentAppUI": "${ui}",
                }},
                "parameterConfiguration": {"ui": {"type": "STRING", "defaultValue": "MAYBE"}},
            }),
            parameters: Some(json!({"ui": "MAYBE"})),
            named: "persistentAppUI",
        },
        Unapplicable {
            label: "a substituted executionRoleArn that is not an IAM role ARN",
            data: json!({
                "executionRoleArn": "${role}",
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {"role": {"type": "STRING"}},
            }),
            parameters: Some(json!({"role": "arn:aws:iam::000000000000:user/NotARole"})),
            named: "executionRoleArn",
        },
        Unapplicable {
            label: "a substituted releaseLabel the run's own pattern rejects",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "${label}",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "parameterConfiguration": {"label": {"type": "STRING"}},
            }),
            parameters: Some(json!({"label": "emr-6.15.0-latest!"})),
            named: "releaseLabel",
        },
        Unapplicable {
            label: "a substituted entryPoint longer than the run's own member admits",
            data: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                // `EntryPointPath` is len 1..256 on the run, while a parameter value may
                // be 1024 characters (smithy_query: shapes `EntryPointPath`,
                // `TemplateParameterInputMap`).
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/${script}"}},
                "parameterConfiguration": {"script": {"type": "STRING"}},
            }),
            parameters: Some(json!({"script": "j".repeat(300)})),
            named: "jobDriver.sparkSubmitJobDriver.entryPoint",
        },
    ]
}

/// RULE 2: a template a run cannot be built from is refused, and nothing is created.
#[tokio::test]
async fn a_template_a_run_cannot_be_built_from_is_refused() {
    let emr = Emr::new();

    for (index, row) in unapplicable_rows().into_iter().enumerate() {
        let case = row.label;
        // One cluster per row, so the run count below is about THIS row's submission.
        let cluster = emr
            .cluster(
                &format!("unapplicable-{index}"),
                &format!("tok-cluster-{index}"),
            )
            .await;
        let created = create_template(
            &emr,
            &format!("unapplicable-{index}"),
            &format!("tok-unapplicable-{index}"),
            row.data,
        )
        .await;
        let template = text(created.ok(case), "/id", case);

        let token = format!("run-unapplicable-{index}");
        let answer = emr
            .start_job(&cluster, templated_body(&token, &template, row.parameters))
            .await;
        answer.refused(case, 400, "ValidationException");
        let reported = text(&answer.body, "/message", case);
        assert!(
            reported.contains(row.named),
            "case {case:?}: the refusal must name {:?}, got {reported:?}",
            row.named
        );

        let listed = listed_runs(&emr, &cluster, case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused submission must create no job run, got {listed:?}"
        );

        // Nothing was created, so the idempotency token was never spent: the same token
        // now naming a template a run CAN be built from is accepted and readable.
        let repaired = create_template(
            &emr,
            &format!("repaired-{index}"),
            &format!("tok-repaired-{index}"),
            json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
            }),
        )
        .await;
        let repaired = text(repaired.ok(case), "/id", case);
        let started = emr
            .start_job(&cluster, templated_body(&token, &repaired, None))
            .await;
        let run = text(started.ok(case), "/id", case);
        let described = emr.describe_job(&cluster, &run).await;
        assert_eq!(
            text(field(described.ok(case), "/jobRun", case), "/id", case),
            run,
            "case {case:?}: the repaired submission must be readable"
        );
        assert_eq!(
            listed_runs(&emr, &cluster, case).await.len(),
            1,
            "case {case:?}: the repaired submission must be the cluster's only run"
        );
    }
}

/// RULE 2 at the bounds the emulator sets on one application: the substitution budget
/// ([`MAX_SUBSTITUTED_CHARACTERS`]) and the run's tag ceiling ([`MAX_RESOURCE_TAGS`], the modelled
/// `TagMap` maximum). The rows are the published bound and one past it.
#[tokio::test]
async fn an_application_past_a_bound_is_refused() {
    let emr = Emr::new();
    let cluster = emr.cluster("bounded", "tok-bounded").await;

    // The longest value `TemplateParameterInputMap` admits is `String1024`, so one
    // reference can render at most this many characters (smithy_query: shape
    // `TemplateParameterInputMap`).
    let value_length = 1024usize;
    let value = "x".repeat(value_length);
    // One reference per configuration property, so the budget is driven by the number of
    // configurations: `ConfigurationList` is len 0..100 and `SensitivePropertiesMap`
    // 0..100 (smithy_query: shapes `ConfigurationList`, `SensitivePropertiesMap`), and
    // each property's rendered value is exactly what its own `String1024` member admits,
    // so only the budget decides these rows.
    let properties_per_configuration = 100usize;
    let configurations = |count: usize| -> Value {
        (0..count)
            .map(|index| {
                json!({
                    "classification": format!("spark-defaults-{index}"),
                    "properties": (0..properties_per_configuration)
                        .map(|property| (format!("p{property}"), Value::from("${pad}")))
                        .collect::<serde_json::Map<String, Value>>(),
                })
            })
            .collect::<Vec<Value>>()
            .into()
    };
    let per_configuration = properties_per_configuration * value_length;
    let budget_rows = [
        (
            "configurations rendering just inside the budget",
            MAX_SUBSTITUTED_CHARACTERS / per_configuration,
            true,
        ),
        (
            "one configuration more than the budget pays for",
            MAX_SUBSTITUTED_CHARACTERS / per_configuration + 1,
            false,
        ),
    ];
    for (index, (case, count, accepted)) in budget_rows.into_iter().enumerate() {
        let created = create_template(
            &emr,
            &format!("budget-{index}"),
            &format!("tok-budget-{index}"),
            json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "configurationOverrides": {"applicationConfiguration": configurations(count)},
                "parameterConfiguration": {"pad": {"type": "STRING"}},
            }),
        )
        .await;
        let template = text(created.ok(case), "/id", case);
        let answer = emr
            .start_job(
                &cluster,
                templated_body(
                    &format!("run-budget-{index}"),
                    &template,
                    Some(json!({"pad": value})),
                ),
            )
            .await;
        if accepted {
            let run = text(answer.ok(case), "/id", case);
            assert_resource_id(case, "the job run id", &run);
        } else {
            answer.refused(case, 400, "ValidationException");
            let reported = text(&answer.body, "/message", case);
            assert!(
                reported.contains(&MAX_SUBSTITUTED_CHARACTERS.to_string()),
                "case {case:?}: the refusal must name the budget, got {reported:?}"
            );
        }
    }

    // The tag ceiling is the run's, so it is checked after the template's `jobTags` are
    // applied to the request's `tags` — each map on its own is within the modelled
    // maximum.
    let tag_rows = [
        (
            "tags and jobTags together at the ceiling",
            MAX_RESOURCE_TAGS / 2,
            true,
        ),
        (
            "one tag more than a job run may carry",
            MAX_RESOURCE_TAGS / 2 + 1,
            false,
        ),
    ];
    for (index, (case, per_side, accepted)) in tag_rows.into_iter().enumerate() {
        let job_tags: Value = (0..per_side)
            .map(|tag| (format!("template{tag}"), Value::from("t")))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let tags: Value = (0..per_side)
            .map(|tag| (format!("request{tag}"), Value::from("r")))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let created = create_template(
            &emr,
            &format!("tagged-{index}"),
            &format!("tok-tagged-{index}"),
            json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/j.py"}},
                "jobTags": job_tags,
            }),
        )
        .await;
        let template = text(created.ok(case), "/id", case);
        let answer = emr
            .start_job(
                &cluster,
                json!({
                    "clientToken": format!("run-tagged-{index}"),
                    "jobTemplateId": template,
                    "tags": tags,
                }),
            )
            .await;
        if accepted {
            let run = text(answer.ok(case), "/id", case);
            let described = emr.describe_job(&cluster, &run).await;
            let reported = field(described.ok(case), "/jobRun", case)
                .get("tags")
                .and_then(Value::as_object)
                .map(serde_json::Map::len)
                .unwrap_or_default();
            assert_eq!(
                reported,
                per_side * 2,
                "case {case:?}: the run must carry every tag from both sides, got {reported}"
            );
        } else {
            answer.refused(case, 400, "ValidationException");
            let reported = text(&answer.body, "/message", case);
            assert!(
                reported.contains(&MAX_RESOURCE_TAGS.to_string()),
                "case {case:?}: the refusal must name the ceiling, got {reported:?}"
            );
        }
    }
}

/// RULE 3: the template is what the run runs with, and what it stores nothing about stays
/// the request's.
#[tokio::test]
async fn the_template_supplies_the_values_the_request_also_carries() {
    let emr = Emr::new();
    let cluster = emr.cluster("enforced", "tok-enforced").await;
    let case = "a request that carries the values the template stores";
    let created = create_template(
        &emr,
        "enforcing",
        "tok-enforcing",
        json!({
            "executionRoleArn": ROLE,
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/template.py"}},
            "jobTags": {"pipeline": "nightly", "owner": "platform"},
        }),
    )
    .await;
    let template = text(created.ok(case), "/id", case);

    let started = emr
        .start_job(
            &cluster,
            json!({
                "name": "enforced-run",
                "clientToken": "run-enforced",
                "jobTemplateId": template,
                "executionRoleArn": "arn:aws:iam::000000000000:role/RequestRole",
                "releaseLabel": "emr-5.36.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/request.py"}},
                "tags": {"owner": "alice", "cost": "center"},
            }),
        )
        .await;
    let run = text(started.ok(case), "/id", case);
    let described = emr.describe_job(&cluster, &run).await;
    let job_run = field(described.ok(case), "/jobRun", case).clone();

    for (member, wanted) in [
        ("executionRoleArn", json!(ROLE)),
        ("releaseLabel", json!("emr-6.15.0-latest")),
        (
            "jobDriver",
            json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/template.py"}}),
        ),
        // `jobTags` are applied to the run, winning a key the request also set; a tag
        // only the request set stays.
        (
            "tags",
            json!({"pipeline": "nightly", "owner": "platform", "cost": "center"}),
        ),
        // The template stores nothing about `name`, so the request's is the run's.
        ("name", json!("enforced-run")),
    ] {
        let actual = job_run.get(member);
        assert_eq!(
            actual,
            Some(&wanted),
            "case {case:?}: the run's {member} must be {wanted}, got {actual:?}"
        );
    }

    // `configurationOverrides` is optional on the template: one that overrides nothing
    // leaves the request's overrides in place.
    let case = "a template that stores no configurationOverrides";
    let bare = create_template(
        &emr,
        "bare",
        "tok-bare",
        json!({
            "executionRoleArn": ROLE,
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/bare.py"}},
        }),
    )
    .await;
    let bare = text(bare.ok(case), "/id", case);
    let overrides = json!({"monitoringConfiguration": {
        "s3MonitoringConfiguration": {"logUri": "s3://bucket/request-logs"},
    }});
    let started = emr
        .start_job(
            &cluster,
            json!({
                "clientToken": "run-bare",
                "jobTemplateId": bare,
                "configurationOverrides": overrides,
            }),
        )
        .await;
    let run = text(started.ok(case), "/id", case);
    let described = emr.describe_job(&cluster, &run).await;
    let job_run = field(described.ok(case), "/jobRun", case);
    let actual = job_run.get("configurationOverrides");
    assert_eq!(
        actual,
        Some(&overrides),
        "case {case:?}: the request's configurationOverrides must stay, got {actual:?}"
    );
}

/// RULE 4: applying a template neither mints a second run for a replayed token nor
/// changes the template it read.
#[tokio::test]
async fn applying_a_template_neither_replays_twice_nor_changes_the_template() {
    let emr = Emr::new();
    let cluster = emr.cluster("replayed", "tok-replayed-run").await;
    let case = "a templated submission replayed";
    let created = create_template(
        &emr,
        "replayable",
        "tok-replayable",
        json!({
            "executionRoleArn": ROLE,
            "releaseLabel": "${releaseVersion}",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/${script}.py"}},
            "parameterConfiguration": {
                "releaseVersion": {"type": "STRING", "defaultValue": "emr-6.15.0-latest"},
                "script": {"type": "STRING", "defaultValue": "nightly"},
            },
            "jobTags": {"pipeline": "nightly"},
        }),
    )
    .await;
    let template = text(created.ok(case), "/id", case);
    let before = field(
        describe_template(&emr, &template).await.ok(case),
        "/jobTemplate",
        case,
    )
    .clone();

    let body = templated_body(
        "run-replayed",
        &template,
        Some(json!({"script": "rollup"})),
    );
    let mut ids = Vec::new();
    for attempt in 0..3 {
        let answer = emr.start_job(&cluster, body.clone()).await;
        ids.push(text(answer.ok(case), "/id", case));
        assert_eq!(
            listed_runs(&emr, &cluster, case).await.len(),
            1,
            "case {case:?}: attempt {attempt} must not mint a second run"
        );
    }
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "case {case:?}: a replayed clientToken must name one run, got {ids:?}"
    );

    // The template is a resource this submission only READ: it reads back byte for byte
    // as it was stored, references and all.
    let after = field(
        describe_template(&emr, &template).await.ok(case),
        "/jobTemplate",
        case,
    )
    .clone();
    assert_eq!(
        after, before,
        "case {case:?}: starting a run must leave the template unchanged, got {after}"
    );
}
