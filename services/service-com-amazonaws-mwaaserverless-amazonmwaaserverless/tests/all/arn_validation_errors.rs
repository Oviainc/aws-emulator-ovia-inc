// SPDX-License-Identifier: CC0-1.0

//! The ARN validation result this service reports for an ARN-typed input member
//! it refuses.
//!
//! RULE under test: an input member that carries an ARN is validated by this
//! service's OWN ARN validator, whose refusal — not the generic member-constraint
//! rendering — is what a client sees. For every operation and every ARN member:
//!
//! * a member naming a resource of this service (one targeting `WorkflowArn` or
//!   `TaggableResourceArn`) is refused with HTTP 400 `ValidationException`,
//!   `Reason` = `fieldValidationFailed`, a single `FieldList` entry whose `Name`
//!   is the generic field name `Arn` and whose `Message` is the validator's
//!   short ARN message, and a top-level `Message` composed as
//!   `"Validation failed. " + <field message>`;
//! * a member naming an IAM role (one targeting `RoleARN`) is refused with the
//!   same status, error and `Reason`, one `FieldList` entry named as the model
//!   names the member, and a message built from the request — the fixed prefix
//!   `"Invalid Role ARN: "` followed by the rejected value as supplied — which
//!   is also the top-level `Message`.
//!
//! Neither message states the modelled `@pattern`, which is internal.
//!
//! The table spans the class the rule generalises over: the operation, the ARN
//! member, which validator owns it, and how the value is malformed (not an ARN
//! at all, a well-formed ARN of another service or another resource type, a
//! malformed region or account segment, a missing resource suffix, an unknown
//! partition, empty, and past the member's published length maximum). The four
//! cases the issue recorded are four rows among them.
//!
//! Every case drives the real wire path through the shared `Harness` (see
//! `workflow_lifecycle`), so what is asserted is what a client sees.

use crate::validation_errors::{good_definition, valid_workflow_arn, REGION};
use crate::workflow_lifecycle::{role_arn, Harness};

/// The generic field name this service's resource-ARN validator reports.
const ARN_FIELD: &str = "Arn";

/// The field message a malformed resource ARN carries.
const INVALID_ARN: &str = "Invalid arn";

/// How a service-level validation failure composes its top-level `Message`.
const VALIDATION_FAILED_PREFIX: &str = "Validation failed. ";

/// The fixed prefix of the role-ARN validator's message.
const INVALID_ROLE_ARN_PREFIX: &str = "Invalid Role ARN: ";

/// `ErrorMessage`'s modelled length, which both `Message` and a
/// `ValidationExceptionField$Message` must satisfy
/// (`smithy_query(query="shape", shape="ErrorMessage")`).
const MESSAGE_LENGTH: std::ops::RangeInclusive<usize> = 1..=2048;

/// Which validator's answer a row expects.
#[derive(Debug)]
enum Expected {
    /// The resource-ARN validator: field `Arn`, its short message, and the
    /// composed top-level `Message`.
    ResourceArn,
    /// The role-ARN validator: field `RoleArn`, and the prefix followed by the
    /// rejected value exactly as supplied.
    RoleArn(String),
    /// The role-ARN validator for a value too long to echo whole: the prefix is
    /// still there, the message stays inside `ErrorMessage`'s bound, and the
    /// value is not echoed in full.
    RoleArnTooLongToEcho {
        /// How long the supplied value was.
        supplied: usize,
    },
}

/// One row: a label, the operation, the request body, and the validator answer
/// it must produce.
struct Case {
    label: &'static str,
    operation: &'static str,
    body: serde_json::Value,
    expected: Expected,
}

fn case(
    label: &'static str,
    operation: &'static str,
    body: serde_json::Value,
    expected: Expected,
) -> Case {
    Case {
        label,
        operation,
        body,
        expected,
    }
}

/// A `RunId`/`TaskInstanceId` that satisfies `IdString`, so the only thing wrong
/// with a row is its ARN.
const GOOD_RUN_ID: &str = "run-01";
const GOOD_TASK_ID: &str = "task-01";

/// A `TaggableResourceArn` one character past its published maximum of 1011
/// (`smithy_query(query="shape", shape="TaggableResourceArn")`), built so only
/// its length is at issue in the prefix and suffix.
fn resource_arn_past_its_length_maximum() -> String {
    let prefix = format!("arn:aws:airflow-serverless:{REGION}:000000000000:workflow/");
    let suffix = "-0123456789";
    let name = "n".repeat(1012 - prefix.len() - suffix.len());
    format!("{prefix}{name}{suffix}")
}

/// Rows whose ARN member names a resource of this service.
fn resource_arn_cases() -> Vec<Case> {
    vec![
        case(
            "GetWorkflow WorkflowArn is not an ARN at all",
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": "not-an-arn"}),
            Expected::ResourceArn,
        ),
        case(
            "DeleteWorkflow WorkflowArn has a malformed region segment",
            "DeleteWorkflow",
            serde_json::json!({
                "WorkflowArn":
                    "arn:aws:airflow-serverless:REGION:000000000000:workflow/demo-0123456789",
            }),
            Expected::ResourceArn,
        ),
        case(
            // The recorded ListWorkflowRuns case.
            "ListWorkflowRuns WorkflowArn is not an ARN at all",
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": "not-an-arn"}),
            Expected::ResourceArn,
        ),
        case(
            "ListWorkflowVersions WorkflowArn is an ARN of another service",
            "ListWorkflowVersions",
            serde_json::json!({"WorkflowArn": "arn:aws:iam::000000000000:role/r"}),
            Expected::ResourceArn,
        ),
        case(
            "ListTaskInstances WorkflowArn has a five-digit account segment",
            "ListTaskInstances",
            serde_json::json!({
                "WorkflowArn":
                    format!("arn:aws:airflow-serverless:{REGION}:12345:workflow/demo-0123456789"),
                "RunId": GOOD_RUN_ID,
            }),
            Expected::ResourceArn,
        ),
        case(
            "GetWorkflowRun WorkflowArn is empty, below the published minimum of 1",
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": "", "RunId": GOOD_RUN_ID}),
            Expected::ResourceArn,
        ),
        case(
            "GetTaskInstance WorkflowArn's resource lacks the ten-character suffix",
            "GetTaskInstance",
            serde_json::json!({
                "WorkflowArn":
                    format!("arn:aws:airflow-serverless:{REGION}:000000000000:workflow/demo"),
                "RunId": GOOD_RUN_ID,
                "TaskInstanceId": GOOD_TASK_ID,
            }),
            Expected::ResourceArn,
        ),
        case(
            "StartWorkflowRun WorkflowArn is a bare resource path",
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": "workflow/demo"}),
            Expected::ResourceArn,
        ),
        case(
            "StopWorkflowRun WorkflowArn names a partition the pattern excludes",
            "StopWorkflowRun",
            serde_json::json!({
                "WorkflowArn":
                    format!(
                        "arn:aws-fake:airflow-serverless:{REGION}:000000000000:workflow/demo-0123456789"
                    ),
                "RunId": GOOD_RUN_ID,
            }),
            Expected::ResourceArn,
        ),
        case(
            "UpdateWorkflow WorkflowArn is not an ARN while its RoleArn is well formed",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": "not-an-arn",
                "DefinitionS3Location": good_definition(),
                "RoleArn": role_arn(),
            }),
            Expected::ResourceArn,
        ),
        case(
            // The resource ARN names the resource the call targets, so it is
            // the one reported when a request gets both ARNs wrong.
            "UpdateWorkflow with both its WorkflowArn and its RoleArn malformed",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": "not-an-arn",
                "DefinitionS3Location": good_definition(),
                "RoleArn": "nope",
            }),
            Expected::ResourceArn,
        ),
        case(
            "ListTagsForResource ResourceArn is an ARN of another service",
            "ListTagsForResource",
            serde_json::json!({"ResourceArn": "arn:aws:s3:::some-bucket"}),
            Expected::ResourceArn,
        ),
        case(
            // The recorded TagResource case.
            "TagResource ResourceArn is not an ARN at all",
            "TagResource",
            serde_json::json!({"ResourceArn": "not-an-arn", "Tags": {"team": "data"}}),
            Expected::ResourceArn,
        ),
        case(
            // `TaggableResourceArn` is @length(1..1011): one past the maximum.
            "TagResource ResourceArn one past TaggableResourceArn's maximum of 1011",
            "TagResource",
            serde_json::json!({
                "ResourceArn": resource_arn_past_its_length_maximum(),
                "Tags": {"team": "data"},
            }),
            Expected::ResourceArn,
        ),
        case(
            // The recorded UntagResource case.
            "UntagResource ResourceArn is not an ARN at all",
            "UntagResource",
            serde_json::json!({"ResourceArn": "not-an-arn", "TagKeys": ["team"]}),
            Expected::ResourceArn,
        ),
    ]
}

/// Rows whose ARN member names an IAM role.
fn role_arn_cases() -> Vec<Case> {
    let create = |role: serde_json::Value| {
        serde_json::json!({
            "Name": "arnRow",
            "DefinitionS3Location": good_definition(),
            "RoleArn": role,
        })
    };
    vec![
        case(
            // The recorded CreateWorkflow case.
            "CreateWorkflow RoleArn is not an ARN at all",
            "CreateWorkflow",
            create(serde_json::json!("not-a-valid-role-arn")),
            Expected::RoleArn("not-a-valid-role-arn".to_string()),
        ),
        case(
            "CreateWorkflow RoleArn is an IAM user, not a role",
            "CreateWorkflow",
            create(serde_json::json!("arn:aws:iam::000000000000:user/not-a-role")),
            Expected::RoleArn("arn:aws:iam::000000000000:user/not-a-role".to_string()),
        ),
        case(
            "CreateWorkflow RoleArn has a five-digit account segment",
            "CreateWorkflow",
            create(serde_json::json!("arn:aws:iam::12345:role/short-account")),
            Expected::RoleArn("arn:aws:iam::12345:role/short-account".to_string()),
        ),
        case(
            "CreateWorkflow RoleArn is the service's own resource ARN",
            "CreateWorkflow",
            create(serde_json::json!(valid_workflow_arn())),
            Expected::RoleArn(valid_workflow_arn()),
        ),
        case(
            "CreateWorkflow RoleArn is empty, below RoleARN's published minimum of 1",
            "CreateWorkflow",
            create(serde_json::json!("")),
            Expected::RoleArn(String::new()),
        ),
        case(
            // The ARN validator answers before any other member's constraint is
            // rendered, so a request that also breaks `Name`'s pattern still
            // reports the ARN failure alone.
            "CreateWorkflow with both its Name and its RoleArn malformed",
            "CreateWorkflow",
            serde_json::json!({
                "Name": "-bad-name",
                "DefinitionS3Location": good_definition(),
                "RoleArn": "nope",
            }),
            Expected::RoleArn("nope".to_string()),
        ),
        case(
            "UpdateWorkflow RoleArn is an IAM user while its WorkflowArn is well formed",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": valid_workflow_arn(),
                "DefinitionS3Location": good_definition(),
                "RoleArn": "arn:aws:iam::000000000000:user/not-a-role",
            }),
            Expected::RoleArn("arn:aws:iam::000000000000:user/not-a-role".to_string()),
        ),
        case(
            // `RoleARN` is @length(1..2048); a value past it is also too long to
            // echo whole into a message.
            "CreateWorkflow RoleArn is one past RoleARN's maximum of 2048",
            "CreateWorkflow",
            create(serde_json::json!(format!(
                "arn:aws:iam::000000000000:role/{}",
                "r".repeat(2049 - "arn:aws:iam::000000000000:role/".len())
            ))),
            Expected::RoleArnTooLongToEcho { supplied: 2049 },
        ),
    ]
}

#[tokio::test]
async fn a_malformed_arn_is_refused_with_the_service_s_own_arn_validation_result() {
    let harness = Harness::new(REGION);

    let mut cases = resource_arn_cases();
    cases.extend(role_arn_cases());

    for Case {
        label,
        operation,
        body,
        expected,
    } in cases
    {
        let (status, answered) = harness.call(operation, body).await;

        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            answered["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected __type ValidationException, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {label:?}: expected Reason fieldValidationFailed, got {:?}",
            answered["Reason"]
        );

        let message = answered["Message"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: expected a Message member, got {answered:?}")
        });
        let field_list = answered["FieldList"].as_array().unwrap_or_else(|| {
            panic!("case {label:?}: expected a FieldList member, got {answered:?}")
        });
        assert_eq!(
            field_list.len(),
            1,
            "case {label:?}: an ARN failure reports exactly one field, got {field_list:?}"
        );
        let name = field_list[0]["Name"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: every FieldList entry needs a Name, got {field_list:?}")
        });
        let field_message = field_list[0]["Message"].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: every FieldList entry needs a Message, got {field_list:?}")
        });

        for (which, text) in [("Message", message), ("FieldList[0].Message", field_message)] {
            assert!(
                MESSAGE_LENGTH.contains(&text.chars().count()),
                "case {label:?}: {which} must satisfy ErrorMessage's length 1..2048, got {} \
                 characters: {text:?}",
                text.chars().count()
            );
            // The modelled `@pattern` is internal: an ARN validation message
            // never dumps it.
            assert!(
                !text.contains("regular expression pattern"),
                "case {label:?}: {which} must not state the modelled pattern, got {text:?}"
            );
        }

        match &expected {
            Expected::ResourceArn => {
                assert_eq!(
                    name, ARN_FIELD,
                    "case {label:?}: expected FieldList[0].Name {ARN_FIELD:?}, got {name:?}"
                );
                assert_eq!(
                    field_message, INVALID_ARN,
                    "case {label:?}: expected FieldList[0].Message {INVALID_ARN:?}, got \
                     {field_message:?}"
                );
                let composed = format!("{VALIDATION_FAILED_PREFIX}{INVALID_ARN}");
                assert_eq!(
                    message, composed,
                    "case {label:?}: expected Message {composed:?}, got {message:?}"
                );
            }
            Expected::RoleArn(supplied) => {
                assert_eq!(
                    name, "RoleArn",
                    "case {label:?}: expected FieldList[0].Name \"RoleArn\", got {name:?}"
                );
                let composed = format!("{INVALID_ROLE_ARN_PREFIX}{supplied}");
                assert_eq!(
                    field_message, composed,
                    "case {label:?}: expected FieldList[0].Message {composed:?}, got \
                     {field_message:?}"
                );
                assert_eq!(
                    message, composed,
                    "case {label:?}: expected Message {composed:?}, got {message:?}"
                );
            }
            Expected::RoleArnTooLongToEcho { supplied } => {
                assert_eq!(
                    name, "RoleArn",
                    "case {label:?}: expected FieldList[0].Name \"RoleArn\", got {name:?}"
                );
                assert!(
                    field_message.starts_with(INVALID_ROLE_ARN_PREFIX),
                    "case {label:?}: expected FieldList[0].Message to open with \
                     {INVALID_ROLE_ARN_PREFIX:?}, got {field_message:?}"
                );
                assert_eq!(
                    message, field_message,
                    "case {label:?}: expected Message to be the field message \
                     {field_message:?}, got {message:?}"
                );
                assert!(
                    field_message.len() < *supplied,
                    "case {label:?}: a value of {supplied} bytes must not be echoed whole, got \
                     {} bytes: {field_message:?}",
                    field_message.len()
                );
            }
        }
    }
}

/// The ARN validators refuse only what the model's ARN patterns refuse: a
/// well-formed ARN of each published partition is accepted, whatever the
/// operation then answers.
///
/// The partitions are the ones the modelled `WorkflowArn`/`RoleARN` patterns
/// list (`smithy_query(query="shape", shape="WorkflowArn,RoleARN")`):
/// `aws`, `aws-cn`, `aws-us-gov`, `aws-iso`, `aws-iso-b`, `aws-iso-e`,
/// `aws-iso-f`.
const PARTITIONS: &[&str] = &[
    "aws",
    "aws-cn",
    "aws-us-gov",
    "aws-iso",
    "aws-iso-b",
    "aws-iso-e",
    "aws-iso-f",
];

#[tokio::test]
async fn a_well_formed_arn_of_any_published_partition_is_not_an_arn_validation_failure() {
    let harness = Harness::new(REGION);

    for partition in PARTITIONS {
        // A resource ARN of this service, and a role ARN, in this partition.
        let workflow_arn = format!(
            "arn:{partition}:airflow-serverless:{REGION}:000000000000:workflow/demo-0123456789"
        );
        let role = format!("arn:{partition}:iam::000000000000:role/mwaa-exec");

        let rows: Vec<(String, &str, serde_json::Value)> = vec![
            (
                format!("GetWorkflow with a {partition} workflow ARN"),
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": workflow_arn.clone()}),
            ),
            (
                format!("ListTagsForResource with a {partition} workflow ARN"),
                "ListTagsForResource",
                serde_json::json!({"ResourceArn": workflow_arn.clone()}),
            ),
            (
                format!("UntagResource with a {partition} workflow ARN"),
                "UntagResource",
                serde_json::json!({"ResourceArn": workflow_arn.clone(), "TagKeys": ["team"]}),
            ),
            (
                format!("CreateWorkflow with a {partition} role ARN"),
                "CreateWorkflow",
                serde_json::json!({
                    "Name": format!("part{}", partition.replace('-', "")),
                    "DefinitionS3Location": good_definition(),
                    "RoleArn": role.clone(),
                }),
            ),
        ];

        for (label, operation, body) in rows {
            let (status, answered) = harness.call(operation, body).await;
            let refused_as_validation = status == 400
                && answered["__type"]
                    .as_str()
                    .is_some_and(|wire| wire.contains("ValidationException"));
            assert!(
                !refused_as_validation,
                "case {label:?}: a well-formed ARN must not be refused as a validation \
                 failure, got {status} {answered:?}"
            );
        }
    }
}
