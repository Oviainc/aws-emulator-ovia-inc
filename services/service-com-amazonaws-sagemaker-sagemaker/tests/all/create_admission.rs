// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for two of the admission rules a SageMaker create answers before
//! it stores anything: the conformance of a caller-supplied CONFIGURATION DOCUMENT to the
//! schema the service publishes (`CreateJob`), and the completeness of a VPC PLACEMENT
//! (`CreateNotebookInstance`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.
//!
//! The rules asserted here are:
//!
//! * a create whose request carries a configuration document the service's schema does not
//!   accept is refused 400 `ValidationException` carrying the schema layer's findings, and
//!   stores nothing — the document is judged against the SAME schema
//!   `DescribeJobSchemaVersion` reports, so a caller that validated against that answer is
//!   accepted;
//! * a create that places a resource in a VPC subnet and names no security group for it is
//!   refused 400 `ValidationException` with the fixed sentence, and stores nothing.
//!
//! The admission rules that resolve a reference in ANOTHER service — the execution role
//! against IAM, the container image against Amazon ECR — can only be driven with that
//! service mounted beside this one, so they are asserted in
//! `core/router/tests/all/sagemaker_create_admission.rs` instead.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/AmazonSageMaker-ExecutionRole";

/// Every member of the `JobCategory` enum.
///
/// Catalog source: `smithy_query(query="operation", operation="CreateJob")` —
/// `JobCategory` is `one of: AgentRFT | AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// The properties the published job configuration schema requires, in the order its
/// findings list them.
///
/// Catalog source: the schema `DescribeJobSchemaVersion` reports, which the first test
/// below reads back and compares against this list rather than assuming it.
const REQUIRED_PROPERTIES: [&str; 5] = [
    "AgentConfig",
    "InputDataConfig",
    "ModelPackageConfig",
    "OutputDataConfig",
    "TrainingConfig",
];

/// A configuration document carrying exactly the properties named in `properties`.
fn document_of(properties: &[&str]) -> String {
    let members: Vec<String> = properties
        .iter()
        .map(|property| format!("\"{property}\":{{}}"))
        .collect();
    format!("{{{}}}", members.join(","))
}

/// A `CreateJob` body carrying `document` as its configuration document.
fn create_job_body(name: &str, category: &str, document: &str) -> Value {
    json!({
        "JobName": name,
        "JobCategory": category,
        "JobConfigSchemaVersion": "1.0.0",
        "JobConfigDocument": document,
        "RoleArn": ROLE_ARN,
    })
}

/// RULE: the schema `DescribeJobSchemaVersion` reports IS the schema `CreateJob` admits a
/// document against — it requires exactly the properties the create's findings name, and a
/// version the service does not publish is refused rather than answered with a schema.
#[tokio::test]
async fn the_reported_schema_is_the_one_the_create_admits_a_document_against() {
    let emulator = Emulator::new();

    // Rows differ along the category asked about and whether the request names the version
    // at all ("If you don't specify a schema version, the latest version is returned").
    for category in JOB_CATEGORIES {
        for named_version in [false, true] {
            let case = format!("{category}/named-version={named_version}");
            let mut body = json!({ "JobCategory": category });
            if named_version {
                body["JobConfigSchemaVersion"] = json!("1.0.0");
            }
            let (status, reported) = emulator.call("DescribeJobSchemaVersion", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {reported}"
            );
            assert_eq!(
                str_field(&reported, "JobCategory"),
                category,
                "case {case:?}: expected the category asked about, got {reported}"
            );
            assert_eq!(
                str_field(&reported, "JobConfigSchemaVersion"),
                "1.0.0",
                "case {case:?}: expected the published version, got {reported}"
            );

            let schema: Value = ::serde_json::from_str(&str_field(&reported, "JobConfigSchema"))
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the reported schema must be JSON, got {error}")
                });
            let required: Vec<&str> = schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the schema must list its required set"))
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert_eq!(
                required, REQUIRED_PROPERTIES,
                "case {case:?}: expected the schema to require {REQUIRED_PROPERTIES:?}, got \
                 {required:?}"
            );

            // A document built from the schema the service just reported is accepted, so
            // the reader and the admission cannot describe different schemas.
            let name = format!("job-{category}-{named_version}");
            let accepted = create_job_body(&name, category, &document_of(&required));
            let (status, created) = emulator.call("CreateJob", &accepted).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a document built from the reported schema to be \
                 accepted, got {status} with {created}"
            );
        }
    }

    // A version the service does not publish names a schema it does not hold.
    let (status, refused) = emulator
        .call(
            "DescribeJobSchemaVersion",
            &json!({ "JobCategory": JOB_CATEGORIES[0], "JobConfigSchemaVersion": "9.9.9" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceNotFound"),
        "case unpublished-version: expected 400 ResourceNotFound, got {status} with {refused}"
    );
}

/// RULE: a `JobConfigDocument` the published schema does not accept is refused 400
/// `ValidationException` carrying the schema layer's findings — the properties the schema
/// does not allow, then the required properties the document is missing — and no job is
/// stored.
#[tokio::test]
async fn a_document_the_schema_does_not_accept_is_refused_and_stores_nothing() {
    // (case, the document, the fragments the findings have to carry)
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "an empty object",
            "{}".to_string(),
            vec![format!(
                "object has missing required properties ({})",
                quoted(&REQUIRED_PROPERTIES)
            )],
        ),
        (
            "one required property only",
            document_of(&REQUIRED_PROPERTIES[..1]),
            vec![format!(
                "object has missing required properties ({})",
                quoted(&REQUIRED_PROPERTIES[1..])
            )],
        ),
        (
            "every required property but one",
            document_of(&REQUIRED_PROPERTIES[..4]),
            vec![format!(
                "object has missing required properties ({})",
                quoted(&REQUIRED_PROPERTIES[4..])
            )],
        ),
        (
            "a property the schema does not allow, beside every required one",
            document_of(&[
                "AgentConfig",
                "InputDataConfig",
                "ModelPackageConfig",
                "OutputDataConfig",
                "TrainingConfig",
                "RewardConfig",
            ]),
            vec![
                "object instance has properties which are not allowed by the schema: \
                 [\"RewardConfig\"]"
                    .to_string(),
            ],
        ),
        (
            "both findings at once",
            document_of(&["baseModel"]),
            vec![
                "object instance has properties which are not allowed by the schema: \
                 [\"baseModel\"]"
                    .to_string(),
                format!(
                    "object has missing required properties ({})",
                    quoted(&REQUIRED_PROPERTIES)
                ),
            ],
        ),
        (
            "a document that is not JSON at all",
            "not a json document".to_string(),
            vec!["instance type (string) does not match any allowed primitive type \
                  (allowed: [\"object\"])"
                .to_string()],
        ),
        (
            "a JSON array where the schema requires an object",
            "[]".to_string(),
            vec!["instance type (array) does not match any allowed primitive type \
                  (allowed: [\"object\"])"
                .to_string()],
        ),
        (
            "a JSON number where the schema requires an object",
            "7".to_string(),
            vec!["instance type (integer) does not match any allowed primitive type \
                  (allowed: [\"object\"])"
                .to_string()],
        ),
    ];

    for (case, document, fragments) in rows {
        for category in JOB_CATEGORIES {
            let emulator = Emulator::new();
            let body = create_job_body("rollout-1", category, &document);
            let (status, refused) = emulator.call("CreateJob", &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case:?}/{category}: expected 400 ValidationException, got {status} with \
                 {refused}"
            );
            let stated = message(&refused);
            assert!(
                stated.starts_with("Job config document failed schema validation: "),
                "case {case:?}/{category}: expected the schema-validation opening, got {stated:?}"
            );
            for fragment in &fragments {
                assert!(
                    stated.contains(fragment.as_str()),
                    "case {case:?}/{category}: expected the finding {fragment:?}, got {stated:?}"
                );
            }

            // Nothing was stored: the reader cannot find the job the refused create named.
            let (status, read) = emulator
                .call(
                    "DescribeJob",
                    &json!({ "JobName": "rollout-1", "JobCategory": category }),
                )
                .await;
            assert_eq!(
                (status, error_code(&read).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{category}: a refused create must store nothing, got {status} \
                 with {read}"
            );
        }
    }
}

/// RULE: a create that places a resource in a VPC subnet and names no security group is
/// refused 400 `ValidationException` with the fixed sentence and stores nothing; one that
/// names a group, and one that asks for no VPC placement at all, is accepted.
#[tokio::test]
async fn a_subnet_placement_without_a_security_group_is_refused() {
    // Rows differ along the pair the rule relates: whether a subnet is named, and how many
    // security groups come with it — `SecurityGroupIds` is `len 0..5`
    // (smithy_query operation=CreateNotebookInstance), so the table sits on the empty list,
    // one group and the published maximum.
    let five: Vec<String> = (1..=5).map(|index| format!("sg-0000000{index}")).collect();
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no VPC placement at all", json!({}), true),
        (
            "a subnet and no SecurityGroupIds member",
            json!({ "SubnetId": "subnet-0abc1234" }),
            false,
        ),
        (
            "a subnet and an empty SecurityGroupIds list",
            json!({ "SubnetId": "subnet-0abc1234", "SecurityGroupIds": [] }),
            false,
        ),
        (
            "a subnet and one group",
            json!({ "SubnetId": "subnet-0abc1234", "SecurityGroupIds": ["sg-00000001"] }),
            true,
        ),
        (
            "a subnet and the five groups the model allows",
            json!({ "SubnetId": "subnet-0abc1234", "SecurityGroupIds": five }),
            true,
        ),
        (
            // Security groups without a placement ask for no VPC placement at all.
            "groups and no subnet",
            json!({ "SecurityGroupIds": ["sg-00000001"] }),
            true,
        ),
        (
            "another subnet id shape, with a group",
            json!({ "SubnetId": "subnet-abcdef0123456789", "SecurityGroupIds": ["sg-00000002"] }),
            true,
        ),
    ];

    for (case, placement, accepted) in rows {
        for region in ["us-east-1", "eu-west-1"] {
            let emulator = Emulator::new();
            let name = "notebook-1";
            let mut body = json!({
                "NotebookInstanceName": name,
                // A member of `InstanceType` (smithy_query operation=CreateNotebookInstance).
                "InstanceType": "ml.t3.medium",
                "RoleArn": ROLE_ARN,
            });
            for (member, value) in placement.as_object().expect("an object of members") {
                body[member] = value.clone();
            }

            let (status, answered) = emulator
                .call_in(region, "CreateNotebookInstance", &body)
                .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}/{region}: expected HTTP 200, got {status} with {answered}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}/{region}: expected 400 ValidationException, got {status} with \
                 {answered}"
            );
            assert_eq!(
                message(&answered),
                "Check if the security groups are specified",
                "case {case:?}/{region}: expected the fixed sentence, got {answered}"
            );

            // Nothing was stored: the reader cannot find the instance the create named.
            let (status, read) = emulator
                .call_in(
                    region,
                    "DescribeNotebookInstance",
                    &json!({ "NotebookInstanceName": name }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}/{region}: a refused create must store nothing, got {status} with \
                 {read}"
            );
        }
    }
}

/// A list of property names as the schema layer renders one: each quoted,
/// comma-separated, in brackets.
fn quoted(properties: &[&str]) -> String {
    let quoted: Vec<String> = properties
        .iter()
        .map(|property| format!("\"{property}\""))
        .collect();
    format!("[{}]", quoted.join(","))
}
