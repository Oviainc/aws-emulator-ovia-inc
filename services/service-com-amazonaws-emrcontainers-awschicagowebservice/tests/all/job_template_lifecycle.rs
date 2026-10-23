// SPDX-License-Identifier: CC0-1.0

//! The job-template rules the other suites in this crate do not pin, end to end.
//!
//! `job_template_idempotency.rs` covers the dedup contract itself (racing creates, the
//! replay, distinct tokens, the round trip, delete and paging). This suite covers the
//! parts of ONE rule that a table of "change one member" rows cannot reach:
//!
//! 1. `CreateJobTemplate`'s `@idempotencyToken` identity is the request's VALUES, not a
//!    rendering of them: two requests whose characters merely REGROUP across a boundary
//!    (a tag's key/value split, two tags, an argument boundary, a nested configuration
//!    property, a template parameter's name and default) are different requests, so the
//!    second one under a spent token is refused with the modelled `ValidationException`
//!    (HTTP 400) instead of being answered as a replay.
//! 2. That identity reaches the DEEPEST members of `jobTemplateData` — a nested
//!    `Configuration`'s properties, the monitoring configuration, a renamed template
//!    parameter — and a member being REMOVED counts as a difference just as a changed
//!    one does.
//! 3. `DescribeJobTemplate` resolves its id out of state at both published
//!    `ResourceIdString` length bounds, so an id of either bound that names no stored
//!    template is refused with the modelled `ResourceNotFoundException` (HTTP 400).
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` that the shared harness
//! [`crate::virtual_cluster_job_run_loop::Emr`] builds, and driven with restJson1 HTTP
//! requests, so routing, deserialization, constraint validation, the handler and
//! serialization all run exactly as they do for an AWS SDK client.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{text, Answer, Emr};

/// `POST /jobtemplates` — `CreateJobTemplate`.
async fn create_template(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/jobtemplates", Some(body)).await
}

/// `GET /jobtemplates/{id}` — `DescribeJobTemplate`.
async fn describe_template(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/jobtemplates/{id}"), None).await
}

/// The smallest `jobTemplateData` the model accepts: `executionRoleArn`, `releaseLabel`
/// and `jobDriver` are all `@required` (smithy_query: shape `JobTemplateData`).
fn minimal_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
    })
}

/// One row: two `CreateJobTemplate` bodies that must NOT be the same request. The row
/// supplies them without a `clientToken`; the test spends one token on `first` and then
/// offers `second` under that same token.
struct PairCase {
    label: &'static str,
    first: Value,
    second: Value,
}

/// RULE 1: the values a request carried are its identity, so two requests whose
/// characters only regroup across a boundary are DIFFERENT requests — a spent token
/// offered the second one is refused, never answered with the first one's template.
///
/// Every row is a pair a rendering that simply concatenated its parts would collapse: the
/// same characters, split at another place. The rows walk each kind of boundary the
/// request has — inside a map entry, between two map entries, between two list items, and
/// inside a nested structure.
#[tokio::test]
async fn one_client_token_is_told_apart_from_a_request_whose_values_only_regroup() {
    let cases = [
        PairCase {
            label: "a tag's key/value split",
            first: json!({"name": "regroup", "jobTemplateData": minimal_data(), "tags": {"owner": "a.b"}}),
            second: json!({"name": "regroup", "jobTemplateData": minimal_data(), "tags": {"owner.a": "b"}}),
        },
        PairCase {
            label: "two tags regrouped",
            first: json!({
                "name": "regroup",
                "jobTemplateData": minimal_data(),
                "tags": {"a": "bc", "d": "e"},
            }),
            second: json!({
                "name": "regroup",
                "jobTemplateData": minimal_data(),
                "tags": {"ab": "c", "d": "e"},
            }),
        },
        PairCase {
            label: "the name borrowing the release label's characters",
            first: json!({
                "name": "report",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                },
            }),
            second: json!({
                "name": "report-emr",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                },
            }),
        },
        PairCase {
            label: "two driver arguments regrouped",
            first: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {
                        "entryPoint": "s3://bucket/a.py",
                        "entryPointArguments": ["--rows", "10"],
                    }},
                },
            }),
            second: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {
                        "entryPoint": "s3://bucket/a.py",
                        "entryPointArguments": ["--rows1", "0"],
                    }},
                },
            }),
        },
        PairCase {
            label: "a nested configuration property's key/value split",
            first: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                    "configurationOverrides": {"applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor": "memory"},
                    }]},
                },
            }),
            second: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                    "configurationOverrides": {"applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor.memor": "y"},
                    }]},
                },
            }),
        },
        PairCase {
            label: "a template parameter's name and default regrouped",
            first: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                    "parameterConfiguration": {"rows": {"type": "NUMBER", "defaultValue": "10"}},
                },
            }),
            second: json!({
                "name": "regroup",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                    "parameterConfiguration": {"rows1": {"type": "NUMBER", "defaultValue": "0"}},
                },
            }),
        },
    ];

    for case in &cases {
        let label = case.label;
        assert_ne!(
            case.first, case.second,
            "case {label:?}: the row must offer two different requests"
        );
        // One emulator per row, so each row's store holds only its own templates.
        let emr = Emr::new();
        const TOKEN: &str = "regroup-tok";

        let mut first = case.first.clone();
        first["clientToken"] = json!(TOKEN);
        let created = create_template(&emr, first.clone()).await;
        let first_id = text(created.ok(label), "/id", label);

        let mut second = case.second.clone();
        second["clientToken"] = json!(TOKEN);
        create_template(&emr, second)
            .await
            .refused(label, 400, "ValidationException");

        // The refusal left the token naming the template it minted, and that template is
        // still readable.
        let replay = create_template(&emr, first).await;
        assert_eq!(
            text(replay.ok(label), "/id", label),
            first_id,
            "case {label:?}: a refused regrouping must leave the token naming the first template"
        );
        describe_template(&emr, &first_id).await.ok(label);
    }
}

/// One row: the member of [`deep_body`] it changes, by JSON pointer, and the value it
/// changes it to — `Value::Null` meaning "remove this member".
struct DeepCase {
    label: &'static str,
    pointer: &'static str,
    value: Value,
}

/// A request carrying the deepest members `jobTemplateData` has: a nested
/// `Configuration`, a full monitoring configuration and a declared template parameter.
fn deep_body(token: &str) -> Value {
    json!({
        "name": "deep.identity",
        "clientToken": token,
        "tags": {"team": "data", "env": "test"},
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {
                "entryPoint": "s3://bucket/a.py",
                "sparkSubmitParameters": "--conf spark.executor.instances=2",
            }},
            "configurationOverrides": {
                "applicationConfiguration": [{
                    "classification": "spark-defaults",
                    "properties": {"spark.executor.memory": "2G"},
                    "configurations": [{
                        "classification": "nested",
                        "properties": {"nested": "yes"},
                    }],
                }],
                "monitoringConfiguration": {
                    "persistentAppUI": "ENABLED",
                    "cloudWatchMonitoringConfiguration": {
                        "logGroupName": "/emr-containers/jobs",
                        "logStreamNamePrefix": "deep",
                    },
                    "s3MonitoringConfiguration": {"logUri": "s3://bucket/logs"},
                },
            },
            "parameterConfiguration": {"rows": {"type": "NUMBER", "defaultValue": "10"}},
        },
    })
}

/// RULE 2: the token's identity covers the DEEPEST member of the request, and a member
/// the second call REMOVES is as much a difference as one it changes.
///
/// The rows reach what a top-level table does not: inside a nested `Configuration`, each
/// branch of the monitoring configuration, the spark-submit parameters, a renamed
/// template parameter, and a removed tag.
#[tokio::test]
async fn the_deepest_members_of_a_request_are_part_of_its_client_token_identity() {
    let cases = [
        DeepCase {
            label: "a nested configuration property",
            pointer: "/jobTemplateData/configurationOverrides/applicationConfiguration/0/configurations/0/properties/nested",
            value: json!("no"),
        },
        DeepCase {
            label: "the nested configuration list removed",
            pointer: "/jobTemplateData/configurationOverrides/applicationConfiguration/0/configurations",
            value: Value::Null,
        },
        DeepCase {
            label: "the CloudWatch log stream prefix",
            pointer: "/jobTemplateData/configurationOverrides/monitoringConfiguration/cloudWatchMonitoringConfiguration/logStreamNamePrefix",
            value: json!("other"),
        },
        DeepCase {
            label: "the S3 monitoring configuration removed",
            pointer: "/jobTemplateData/configurationOverrides/monitoringConfiguration/s3MonitoringConfiguration",
            value: Value::Null,
        },
        DeepCase {
            label: "the whole monitoring configuration removed",
            pointer: "/jobTemplateData/configurationOverrides/monitoringConfiguration",
            value: Value::Null,
        },
        DeepCase {
            label: "the persistent app UI setting",
            pointer: "/jobTemplateData/configurationOverrides/monitoringConfiguration/persistentAppUI",
            value: json!("DISABLED"),
        },
        DeepCase {
            label: "the spark submit parameters",
            pointer: "/jobTemplateData/jobDriver/sparkSubmitJobDriver/sparkSubmitParameters",
            value: json!("--conf spark.executor.instances=3"),
        },
        DeepCase {
            label: "the spark submit parameters removed",
            pointer: "/jobTemplateData/jobDriver/sparkSubmitJobDriver/sparkSubmitParameters",
            value: Value::Null,
        },
        DeepCase {
            label: "a renamed template parameter",
            pointer: "/jobTemplateData/parameterConfiguration",
            value: json!({"columns": {"type": "NUMBER", "defaultValue": "10"}}),
        },
        DeepCase {
            label: "a tag removed",
            pointer: "/tags/env",
            value: Value::Null,
        },
    ];

    for case in &cases {
        let label = case.label;
        let emr = Emr::new();
        let base = deep_body("deep-tok");
        let created = create_template(&emr, base.clone()).await;
        let first_id = text(created.ok(label), "/id", label);

        let mut differing = base.clone();
        apply(&mut differing, case.pointer, case.value.clone(), label);
        assert_ne!(
            differing, base,
            "case {label:?}: the row must change the request it is a mutation of"
        );
        create_template(&emr, differing)
            .await
            .refused(label, 400, "ValidationException");

        let replay = create_template(&emr, base).await;
        assert_eq!(
            text(replay.ok(label), "/id", label),
            first_id,
            "case {label:?}: a refused difference must leave the token naming the first template"
        );
    }
}

/// Set the member `pointer` names to `value`, or REMOVE it when `value` is
/// `Value::Null`. The pointer's segments are object keys, except a numeric one, which
/// indexes an array.
fn apply(body: &mut Value, pointer: &str, value: Value, case: &str) {
    let segments: Vec<&str> = pointer.split('/').skip(1).collect();
    let (last, parents) = segments
        .split_last()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} names no member"));
    let mut cursor = body;
    for segment in parents {
        cursor = match segment.parse::<usize>() {
            Ok(index) => cursor
                .get_mut(index)
                .unwrap_or_else(|| panic!("case {case:?}: {pointer} has no item {index}")),
            Err(_) => cursor
                .get_mut(*segment)
                .unwrap_or_else(|| panic!("case {case:?}: {pointer} has no member {segment}")),
        };
    }
    let object = cursor
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} does not name an object member"));
    if value.is_null() {
        assert!(
            object.remove(*last).is_some(),
            "case {case:?}: {pointer} was not there to remove"
        );
    } else {
        object.insert((*last).to_string(), value);
    }
}

/// RULE 3: `DescribeJobTemplate` answers out of state at both published
/// `ResourceIdString` bounds — length 1 and length 64 (smithy_query:
/// `DescribeJobTemplate` input `id`, `@length 1..64`, `^[0-9a-z]+$`) — so an id of either
/// bound that names no stored template is not found, while the template that does exist
/// still is.
#[tokio::test]
async fn an_unknown_job_template_id_is_refused_at_both_length_bounds() {
    let emr = Emr::new();
    let created = create_template(
        &emr,
        json!({
            "name": "present",
            "clientToken": "tok-present",
            "jobTemplateData": minimal_data(),
        }),
    )
    .await;
    let present = text(created.ok("the stored template"), "/id", "the stored template");

    let unknown = [
        ("the shortest id the model allows", "a".to_string()),
        ("a mid-length id", "0zzzz9".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
    ];
    for (label, id) in &unknown {
        assert_ne!(
            id, &present,
            "case {label:?}: the row must name an id no template was minted for"
        );
        describe_template(&emr, id)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }

    describe_template(&emr, &present)
        .await
        .ok("the stored template is still found");
}
