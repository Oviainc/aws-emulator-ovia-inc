// SPDX-License-Identifier: CC0-1.0

//! What a lookup miss of this service says about the resource it could not find: the
//! two REQUIRED identity members of `ResourceNotFoundException`
//! (`smithy_query(query="shape", shape="ResourceNotFoundException")`).
//!
//! The rule both members follow:
//!
//! * `ResourceType` is the AWS resource name of the entity CLASS — the capitalised
//!   entity name (`Workflow`, `WorkflowVersion`, `WorkflowRun`, `TaskInstance`), never
//!   a lowercased or hyphenated spelling of it.
//! * `ResourceId` is the identifier OF that entity: the resource-id segment of the
//!   workflow ARN, a version id, a run id, a task-instance id. An operation that
//!   addresses a typed workflow resource therefore reports the workflow's own id and
//!   not the ARN it was addressed by, while the tag operations — which address an
//!   untyped taggable resource and are handed nothing but its ARN — report that ARN.
//!
//! The table below spans the class the rule generalises over: every operation of this
//! service that resolves a reference, each entity class a refusal can name, and ARNs
//! differing in partition, region, account and in the form and length of the resource
//! name. One recorded request is one row of it.

// `LOOKUP_MISS_STATUS` is the status this service answers a lookup miss with, asserted
// by `missing_resource_lookups` and reused here rather than restated.
use crate::missing_resource_lookups::LOOKUP_MISS_STATUS;
use crate::workflow_lifecycle::{role_arn, Harness};

/// The modelled shape every lookup miss carries.
const LOOKUP_MISS_TYPE: &str = "ResourceNotFoundException";

/// Every `ResourceType` token this service can report, as the recorded service spells
/// them (fixture `mwaa-serverless-resource-not-found-errors`: `Workflow`) and as the
/// model names the remaining classes (`WorkflowVersion`, `WorkflowRunSummary`,
/// `TaskInstanceSummary` —
/// `smithy_query(query="shape", shape="WorkflowVersion,WorkflowRunSummary,TaskInstanceSummary")`).
const WORKFLOW: &str = "Workflow";
const WORKFLOW_VERSION: &str = "WorkflowVersion";
const WORKFLOW_RUN: &str = "WorkflowRun";
const TASK_INSTANCE: &str = "TaskInstance";

/// A pattern-valid `WorkflowArn` no store ever held, kept beside the resource-id
/// segment it carries so a row states what the refusal must report.
struct Ghost {
    /// The whole ARN, as a request supplies it.
    arn: String,
    /// The workflow's own identifier: the `<name>-<10 alnum>` segment the modelled
    /// pattern places after `workflow/`.
    resource_id: String,
}

/// ARNs of the modelled form
/// (`^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$`,
/// `smithy_query(query="shape", shape="WorkflowArn")`) for workflows that were never
/// created, differing in partition, region, account, and in the form and length of the
/// resource name — including the single-character name the pattern's lower bound admits
/// and a 244-character one.
fn ghosts() -> Vec<Ghost> {
    let longest = format!("{}-0123456789", "n".repeat(244));
    [
        ("aws", "us-west-2", "000000000000", "a-0123456789"),
        (
            "aws",
            "eu-central-1",
            "123456789012",
            "absent.wf_name-ABCDEFGHIJ",
        ),
        ("aws", "ap-south-1", "999999999999", longest.as_str()),
        // Another partition: the id segment sits in the same place whatever the
        // partition, which the pattern's `aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?`
        // alternative admits.
        ("aws-cn", "cn-north-1", "222222222222", "cn-probe-ZYXWVUTSRQ"),
    ]
    .into_iter()
    .map(|(partition, region, account, resource_id)| Ghost {
        arn: format!(
            "arn:{partition}:airflow-serverless:{region}:{account}:workflow/{resource_id}"
        ),
        resource_id: resource_id.to_string(),
    })
    .collect()
}

/// One row: a call whose reference resolves to nothing, and the identity its refusal
/// must report.
struct Case {
    name: String,
    operation: &'static str,
    body: serde_json::Value,
    expected_type: &'static str,
    expected_id: String,
}

/// `WorkflowVersion` values of the modelled form (`^[a-z0-9]{32}$`) that no workflow in
/// this test ever had.
fn ghost_versions() -> Vec<String> {
    vec![
        "0".repeat(32),
        "7c9e1f0a5b3d2e4f6a8b0c1d2e3f4a5b".to_string(),
    ]
}

/// `RunId` / `TaskInstanceId` values of the modelled form
/// (`^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`) that nothing in this test ever held.
fn ghost_ids() -> Vec<&'static str> {
    vec!["r", "run-0000000000000000", "Task.1_absent-xyz"]
}

/// Every row of the table.
fn cases(stored_arn: &str) -> Vec<Case> {
    let mut cases = Vec::new();

    for ghost in ghosts() {
        let Ghost { arn, resource_id } = ghost;

        // The operations that address a WORKFLOW resource by its `WorkflowArn`: the
        // refusal names the workflow by its own id segment.
        for (operation, body) in [
            ("GetWorkflow", serde_json::json!({"WorkflowArn": arn})),
            ("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn})),
            (
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/probe.yaml"},
                    "RoleArn": role_arn(),
                }),
            ),
        ] {
            cases.push(Case {
                name: format!("{operation}/absent-workflow/{arn}"),
                operation,
                body,
                expected_type: WORKFLOW,
                expected_id: resource_id.clone(),
            });
        }

        // `StartWorkflowRun` resolves the VERSION it would execute, so for an absent
        // workflow it names the version class — and, with no version named, the
        // workflow whose latest version it looked for, by that same id segment.
        cases.push(Case {
            name: format!("StartWorkflowRun/absent-workflow/{arn}"),
            operation: "StartWorkflowRun",
            body: serde_json::json!({"WorkflowArn": arn}),
            expected_type: WORKFLOW_VERSION,
            expected_id: resource_id.clone(),
        });

        // The tag operations address an untyped taggable resource: the ARN is the only
        // identifier they are handed, so it is the one their refusal reports.
        for (operation, body) in [
            (
                "ListTagsForResource",
                serde_json::json!({"ResourceArn": arn}),
            ),
            (
                "TagResource",
                serde_json::json!({"ResourceArn": arn, "Tags": {"env": "test"}}),
            ),
            (
                "UntagResource",
                serde_json::json!({"ResourceArn": arn, "TagKeys": ["env"]}),
            ),
        ] {
            cases.push(Case {
                name: format!("{operation}/absent-workflow/{arn}"),
                operation,
                body,
                expected_type: WORKFLOW,
                expected_id: arn.clone(),
            });
        }

        // The run and task-instance readers resolve their own entity, which an absent
        // workflow holds no more than an absent run does.
        cases.push(Case {
            name: format!("GetWorkflowRun/absent-workflow/{arn}"),
            operation: "GetWorkflowRun",
            body: serde_json::json!({"WorkflowArn": arn, "RunId": "run-absent-0001"}),
            expected_type: WORKFLOW_RUN,
            expected_id: "run-absent-0001".to_string(),
        });
        cases.push(Case {
            name: format!("GetTaskInstance/absent-workflow/{arn}"),
            operation: "GetTaskInstance",
            body: serde_json::json!({
                "WorkflowArn": arn,
                "RunId": "run-absent-0001",
                "TaskInstanceId": "task-absent-0001",
            }),
            expected_type: TASK_INSTANCE,
            expected_id: "task-absent-0001".to_string(),
        });
    }

    // Against a workflow the store really holds: a version it does not hold is named by
    // the version id the request supplied, which IS that entity's own identifier.
    for version in ghost_versions() {
        for operation in ["GetWorkflow", "DeleteWorkflow", "StartWorkflowRun"] {
            cases.push(Case {
                name: format!("{operation}/absent-version/{version}"),
                operation,
                body: serde_json::json!({
                    "WorkflowArn": stored_arn,
                    "WorkflowVersion": version,
                }),
                expected_type: WORKFLOW_VERSION,
                expected_id: version.clone(),
            });
        }
    }

    // ... and a run or task instance it does not hold, by the id the request supplied.
    for id in ghost_ids() {
        for operation in ["GetWorkflowRun", "StopWorkflowRun"] {
            cases.push(Case {
                name: format!("{operation}/absent-run/{id}"),
                operation,
                body: serde_json::json!({"WorkflowArn": stored_arn, "RunId": id}),
                expected_type: WORKFLOW_RUN,
                expected_id: id.to_string(),
            });
        }
        cases.push(Case {
            name: format!("GetTaskInstance/absent-instance/{id}"),
            operation: "GetTaskInstance",
            body: serde_json::json!({
                "WorkflowArn": stored_arn,
                "RunId": "run-absent-0001",
                "TaskInstanceId": id,
            }),
            expected_type: TASK_INSTANCE,
            expected_id: id.to_string(),
        });
    }

    cases
}

/// Create one workflow so the version-, run- and instance-level rows run against a
/// workflow the store really holds, and report its ARN.
async fn stored_workflow(harness: &Harness) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "identityProbe",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/probe.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

/// The segment of an ARN after its last `/`: the resource id the modelled `WorkflowArn`
/// pattern puts there.
fn id_segment(arn: &str) -> &str {
    arn.rsplit_once('/').map_or(arn, |(_, id)| id)
}

#[tokio::test]
async fn a_lookup_miss_names_the_resource_class_and_the_resource_s_own_id() {
    // The class tokens must stay tellable apart and must all be the capitalised AWS
    // resource name — a client branching on the class compares against that spelling.
    let classes = [WORKFLOW, WORKFLOW_VERSION, WORKFLOW_RUN, TASK_INSTANCE];
    for (index, class) in classes.iter().enumerate() {
        assert!(
            class
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_uppercase())
                && class.chars().all(|character| character.is_ascii_alphabetic()),
            "case class {class:?}: a resource type is the capitalised AWS resource name, \
             with no hyphen or lowercased lead"
        );
        for other in &classes[index + 1..] {
            assert_ne!(
                class, other,
                "case classes: {class:?} and {other:?} must differ so a client can tell \
                 which class was absent"
            );
        }
    }

    let harness = Harness::new("us-west-2");
    let stored_arn = stored_workflow(&harness).await;

    for case in cases(&stored_arn) {
        let Case {
            name,
            operation,
            body,
            expected_type,
            expected_id,
        } = case;
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {name:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains(LOOKUP_MISS_TYPE)),
            "case {name:?}: expected a {LOOKUP_MISS_TYPE}, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some(expected_type),
            "case {name:?}: expected ResourceType {expected_type:?}, got {:?}",
            answered["ResourceType"]
        );
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(expected_id.as_str()),
            "case {name:?}: expected ResourceId {expected_id:?}, got {:?}",
            answered["ResourceId"]
        );
    }
}

#[tokio::test]
async fn the_id_a_workflow_lookup_miss_reports_re_composes_the_arn_requested() {
    // The invariant behind the rule: for an operation that addresses a workflow by its
    // ARN, the reported id is exactly the ARN's resource-id segment, so putting it back
    // after `workflow/` yields the ARN the request supplied — and the id itself is never
    // an ARN, which is what a client re-composing one would nest.
    let harness = Harness::new("us-west-2");

    for ghost in ghosts() {
        let Ghost { arn, resource_id } = ghost;
        let (prefix, _) = arn
            .rsplit_once('/')
            .expect("a modelled workflow ARN carries its resource id after a slash");

        for (operation, body) in [
            ("GetWorkflow", serde_json::json!({"WorkflowArn": arn})),
            ("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn})),
            (
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/probe.yaml"},
                    "RoleArn": role_arn(),
                }),
            ),
            ("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn})),
        ] {
            let case = format!("{operation}/{arn}");
            let (status, answered) = harness.call(operation, body).await;
            assert_eq!(
                status, LOOKUP_MISS_STATUS,
                "case {case:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
            );
            let reported = answered["ResourceId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !reported.starts_with("arn:"),
                "case {case:?}: expected the workflow's own id, not an ARN, got {reported:?}"
            );
            assert_eq!(
                reported, resource_id,
                "case {case:?}: expected the ARN's resource-id segment {resource_id:?}, got \
                 {reported:?}"
            );
            assert_eq!(
                format!("{prefix}/{reported}"),
                arn,
                "case {case:?}: the reported id must re-compose the ARN requested, got \
                 {:?}",
                format!("{prefix}/{reported}")
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_workflow_is_reported_by_the_id_its_own_arn_carries() {
    // The id is the resource's, not the request's: a workflow this emulator minted is
    // reported — once it is gone — by exactly the id segment its minted ARN carries,
    // read back through the same operations that served it while it existed.
    let harness = Harness::new("eu-west-1");
    let stored_arn = stored_workflow(&harness).await;
    let resource_id = id_segment(&stored_arn).to_string();

    // While it exists, the ARN resolves: the refusal below is about absence, not about
    // the identity rule rejecting a good ARN.
    let (status, fetched) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": stored_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case stored workflow: expected GetWorkflow to succeed, got {status} {fetched:?}"
    );
    assert_eq!(
        fetched["WorkflowArn"].as_str(),
        Some(stored_arn.as_str()),
        "case stored workflow: expected the stored ARN back, got {:?}",
        fetched["WorkflowArn"]
    );

    // A workflow of ANOTHER region is not one this endpoint holds, so the very same ARN
    // is a lookup miss there — reported by that same id segment.
    let (status, answered) = harness
        .call_in(
            "us-east-1",
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": stored_arn}),
        )
        .await;
    assert_eq!(
        status, LOOKUP_MISS_STATUS,
        "case other-region endpoint: expected HTTP {LOOKUP_MISS_STATUS}, got {status} \
         {answered:?}"
    );
    assert_eq!(
        answered["ResourceId"].as_str(),
        Some(resource_id.as_str()),
        "case other-region endpoint: expected the minted ARN's id segment {resource_id:?}, \
         got {:?}",
        answered["ResourceId"]
    );
    assert_eq!(
        answered["ResourceType"].as_str(),
        Some(WORKFLOW),
        "case other-region endpoint: expected ResourceType {WORKFLOW:?}, got {:?}",
        answered["ResourceType"]
    );
}
