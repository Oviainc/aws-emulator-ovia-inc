// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE derivation that digests a workflow configuration.
//!
//! The rule under test: the digest of a workflow configuration is a single walk over
//! the configuration members a request supplies, in one fixed order, with every field
//! length-delimited — so two configurations that differ in any member, in the size or
//! the order of a list, or only in where one value ends and the next begins, never
//! share a digest, and the same configuration always digests the same.
//!
//! Both operations that digest a configuration drive that one walk, so every row below
//! is driven through BOTH of them:
//!
//! * `CreateWorkflow`, where the digest is what tells a repeated `ClientToken` apart
//!   from the same token reused for a different request: an identical request replays
//!   the first answer, a changed configuration is the modelled `ConflictException`.
//! * `UpdateWorkflow`, where the digest feeds the `WorkflowVersion` the appended
//!   version carries, read back through the implemented `GetWorkflow`.
//!
//! The table spans what the rule generalises over: which optional members a request
//! supplies, the form and length of their values, list sizes, list order, and the split
//! between two adjacent values.

use crate::workflow_lifecycle::{role_arn, Harness};

/// The configuration members `CreateWorkflowRequest` and `UpdateWorkflowRequest` both
/// carry, from
/// `smithy_query(query="operation", operation="CreateWorkflow,UpdateWorkflow")`. Each
/// one must read back through `GetWorkflow` exactly as the request supplied it, and read
/// back ABSENT when the request supplied none.
const SHARED_CONFIGURATION_MEMBERS: &[&str] = &[
    "DefinitionS3Location",
    "Code",
    "RoleArn",
    "Description",
    "LoggingConfiguration",
    "EngineVersion",
    "NetworkConfiguration",
    "TriggerMode",
];

/// The member only `CreateWorkflowRequest` carries (same `smithy_query`), so an update
/// cannot express it and it is stripped from every update this suite sends.
const CREATE_ONLY_MEMBER: &str = "EncryptionConfiguration";

/// A second valid IAM role ARN, as `RoleARN` models it
/// (`smithy_query(query="shape", shape="RoleARN")`).
fn other_role_arn() -> String {
    "arn:aws:iam::000000000000:role/path/to/mwaa-serverless-other".to_string()
}

/// The configuration every row starts from: every member both operations can express,
/// with two ids in each network list so a row can change their size, their order or
/// which list they sit in.
fn base_configuration() -> serde_json::Value {
    serde_json::json!({
        "DefinitionS3Location": {
            "Bucket": "defs",
            "ObjectKey": "dags/base.yaml",
            "VersionId": "s3objectversion1",
        },
        "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "base.zip",
                                "VersionId": "codeversion1"}},
        "RoleArn": role_arn(),
        "Description": "the base configuration",
        "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/base"},
        "EngineVersion": 1,
        "NetworkConfiguration": {
            "SecurityGroupIds": ["sg-1", "sg-2"],
            "SubnetIds": ["subnet-1", "subnet-2"],
        },
        "TriggerMode": "ON_DEMAND",
    })
}

/// One configuration per row, each differing from [`base_configuration`] in exactly one
/// member. A `null` in an overlay drops that member, so a row can cover an absent
/// optional as well as a changed one.
fn configuration_cases() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "another role, of a different path and length",
            serde_json::json!({"RoleArn": other_role_arn()}),
        ),
        (
            // `DescriptionString` is `@length(1..1024)`
            // (`smithy_query(query="operation", operation="CreateWorkflow")`), so these
            // two rows sit on its published bounds.
            "a single-character description, the shortest one allowed",
            serde_json::json!({"Description": "d"}),
        ),
        (
            "a 1024-character description, the longest one allowed",
            serde_json::json!({"Description": "d".repeat(1024)}),
        ),
        (
            "no description at all",
            serde_json::json!({"Description": null}),
        ),
        (
            // `TriggerMode` is `@length(1..255)`, `^.*$`: a free string the model
            // documents no value for, so a row walks one of them as text.
            "a trigger mode",
            serde_json::json!({"TriggerMode": "SCHEDULED"}),
        ),
        (
            "no trigger mode at all",
            serde_json::json!({"TriggerMode": null}),
        ),
        (
            // `EngineVersion` is an intEnum whose only value is 1
            // (`smithy_query(query="shape", shape="EngineVersion")`), so presence is the
            // axis this row walks.
            "no engine version at all",
            serde_json::json!({"EngineVersion": null}),
        ),
        (
            "the definition in another bucket",
            serde_json::json!({"DefinitionS3Location": {
                "Bucket": "other-defs",
                "ObjectKey": "dags/base.yaml",
                "VersionId": "s3objectversion1",
            }}),
        ),
        (
            "the definition at another object version",
            serde_json::json!({"DefinitionS3Location": {
                "Bucket": "defs",
                "ObjectKey": "dags/base.yaml",
                "VersionId": "s3objectversion2",
            }}),
        ),
        (
            "the definition with no object version",
            serde_json::json!({"DefinitionS3Location": {
                "Bucket": "defs",
                "ObjectKey": "dags/base.yaml",
            }}),
        ),
        (
            // The bucket and the object key concatenate to exactly what the base row's
            // pair concatenates to, so only length-delimited fields tell the two apart.
            "the definition bucket and object key split one character differently",
            serde_json::json!({"DefinitionS3Location": {
                "Bucket": "def",
                "ObjectKey": "sdags/base.yaml",
                "VersionId": "s3objectversion1",
            }}),
        ),
        (
            "another code object, with no object version",
            serde_json::json!({"Code": {"S3Location": {"Bucket": "code",
                                                       "ObjectKey": "other.zip"}}}),
        ),
        ("no code at all", serde_json::json!({"Code": null})),
        (
            "another log group",
            serde_json::json!({"LoggingConfiguration": {
                "LogGroupName": "/aws/mwaa-serverless/other",
            }}),
        ),
        (
            "no logging configuration at all",
            serde_json::json!({"LoggingConfiguration": null}),
        ),
        (
            "one id per network list instead of two",
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": ["sg-1"],
                "SubnetIds": ["subnet-1"],
            }}),
        ),
        (
            "empty network lists, so a list of size zero reaches the digest",
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": [],
                "SubnetIds": [],
            }}),
        ),
        (
            "the same security group ids in the other order",
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": ["sg-2", "sg-1"],
                "SubnetIds": ["subnet-1", "subnet-2"],
            }}),
        ),
        (
            // As many ids in total as the base row, split differently between the two
            // lists, so only a digest that keeps the lists apart tells them apart. The
            // id forms come from the element patterns the model declares
            // (`^sg-[a-z0-9]*$` and `^subnet-[a-z0-9]*$`).
            "the same number of network ids, split differently between the two lists",
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": ["sg-1", "sg-2", "sg-3"],
                "SubnetIds": ["subnet-1"],
            }}),
        ),
        (
            "no network configuration at all",
            serde_json::json!({"NetworkConfiguration": null}),
        ),
        (
            // `EncryptionType` is `AWS_MANAGED_KEY` or `CUSTOMER_MANAGED_KEY`
            // (`smithy_query(query="shape", shape="EncryptionType")`). Only a create
            // supplies this member, so these rows exercise the create leg's extra field
            // and leave the update leg with the base configuration.
            "an encryption configuration the service manages the key for",
            serde_json::json!({"EncryptionConfiguration": {"Type": "AWS_MANAGED_KEY"}}),
        ),
        (
            "an encryption configuration naming a customer-managed key",
            serde_json::json!({"EncryptionConfiguration": {
                "Type": "CUSTOMER_MANAGED_KEY",
                "KmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd",
            }}),
        ),
    ]
}

/// [`base_configuration`] with `overlay`'s members replaced, and the members whose
/// overlay value is `null` dropped.
fn configuration_with(overlay: &serde_json::Value) -> serde_json::Value {
    let mut configuration = base_configuration();
    let members = configuration
        .as_object_mut()
        .expect("the base configuration is a JSON object");
    for (member, value) in overlay
        .as_object()
        .expect("every overlay is a JSON object")
        .iter()
    {
        if value.is_null() {
            members.remove(member);
        } else {
            members.insert(member.clone(), value.clone());
        }
    }
    configuration
}

/// A `CreateWorkflow` request body for `configuration` under `name` and `token`.
fn create_body(configuration: &serde_json::Value, name: &str, token: &str) -> serde_json::Value {
    let mut body = configuration.clone();
    let members = body.as_object_mut().expect("a configuration is an object");
    members.insert("Name".to_string(), serde_json::json!(name));
    members.insert("ClientToken".to_string(), serde_json::json!(token));
    body
}

/// An `UpdateWorkflow` request body for `configuration` against `workflow_arn`, without
/// the member only a create can express.
fn update_body(configuration: &serde_json::Value, workflow_arn: &str) -> serde_json::Value {
    let mut body = configuration.clone();
    let members = body.as_object_mut().expect("a configuration is an object");
    members.remove(CREATE_ONLY_MEMBER);
    members.insert("WorkflowArn".to_string(), serde_json::json!(workflow_arn));
    body
}

/// The modelled `WorkflowVersion`: `^[a-z0-9]{32}$`, length 32..32.
fn is_workflow_version(version: &str) -> bool {
    version.len() == 32 && version.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

#[tokio::test]
async fn one_configuration_digest_serves_both_the_create_token_ledger_and_the_update_version() {
    for (index, (case, overlay)) in configuration_cases().into_iter().enumerate() {
        // A fresh emulator per row, so one row's workflow and token ledger cannot
        // answer another's call.
        let harness = Harness::new("us-west-2");
        let base = base_configuration();
        let changed = configuration_with(&overlay);
        let name = format!("digested{index}");
        let token = format!("digest-token-{index}");

        // CREATE, first leg: the base configuration under this row's token.
        let (status, first) = harness
            .call("CreateWorkflow", create_body(&base, &name, &token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}"
        );
        let arn = first["WorkflowArn"].as_str().unwrap_or_default().to_string();
        let created_version = first["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The SAME configuration under the same token digests the same, so the first
        // answer is replayed rather than a second workflow minted.
        let (status, replay) = harness
            .call("CreateWorkflow", create_body(&base, &name, &token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the token replay to succeed, got {status} {replay:?}"
        );
        assert_eq!(
            replay, first,
            "case {case:?}: an unchanged request must replay the first response, got {replay:?}"
        );

        // This row's configuration under the same token digests differently, so it is
        // the modelled conflict — which is what shows the digest covers the member this
        // row changed.
        let (status, conflict) = harness
            .call("CreateWorkflow", create_body(&changed, &name, &token))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected ConflictException (409) for the same token with a \
             changed configuration, got {status} {conflict:?}"
        );
        assert!(
            conflict["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ConflictException")),
            "case {case:?}: expected a ConflictException, got {:?}",
            conflict["__type"]
        );
        assert!(
            conflict["Message"]
                .as_str()
                .is_some_and(|message| message.contains(&token)),
            "case {case:?}: expected the conflict to name the reused ClientToken, got {:?}",
            conflict["Message"]
        );
        assert_eq!(
            conflict["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the conflict to name the workflow the token created, \
             got {:?}",
            conflict["ResourceId"]
        );

        // And the row's configuration IS an acceptable request: under its own name and
        // token it creates a workflow, so the conflict above is the digest talking and
        // not a refusal of the value.
        let (status, accepted) = harness
            .call(
                "CreateWorkflow",
                create_body(&changed, &format!("{name}.other"), &format!("{token}-other")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the changed configuration to be createable on its own, \
             got {status} {accepted:?}"
        );

        // UPDATE: the same walk digests the configuration an update supplies, feeding
        // the version it appends.
        let body = update_body(&changed, &arn);
        let (status, updated) = harness.call("UpdateWorkflow", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let updated_version = updated["WorkflowVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_workflow_version(&updated_version),
            "case {case:?}: expected a 32-character lowercase-alphanumeric WorkflowVersion, \
             got {updated_version:?}"
        );
        assert_ne!(
            updated_version, created_version,
            "case {case:?}: an update must mint a version the workflow does not already hold, \
             got {updated_version:?}"
        );

        // ROUND TRIP: the appended version carries exactly the configuration the update
        // supplied, read back through `GetWorkflow`.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(updated_version.as_str()),
            "case {case:?}: GetWorkflow must report the appended version, got {:?}",
            fetched["WorkflowVersion"]
        );
        for member in SHARED_CONFIGURATION_MEMBERS {
            let supplied = &body[*member];
            if supplied.is_null() {
                assert!(
                    fetched[*member].is_null(),
                    "case {case:?}: the update supplied no {member}, so the appended version \
                     must carry none, got {:?}",
                    fetched[*member]
                );
            } else {
                assert_eq!(
                    &fetched[*member], supplied,
                    "case {case:?}: GetWorkflow must report the {member} the update supplied, \
                     got {:?}",
                    fetched[*member]
                );
            }
        }
        // An update cannot express the create-only member, so the workflow keeps the one
        // its create supplied.
        assert_eq!(
            fetched[CREATE_ONLY_MEMBER], base[CREATE_ONLY_MEMBER],
            "case {case:?}: an update must not change {CREATE_ONLY_MEMBER}, got {:?}",
            fetched[CREATE_ONLY_MEMBER]
        );

        // Exactly two versions are held: the create's and the update's.
        let stored = harness.stored_versions(&arn);
        assert_eq!(
            stored,
            vec![
                (created_version.clone(), false),
                (updated_version.clone(), true)
            ],
            "case {case:?}: expected the created version to be kept and only the appended one \
             to be latest, got {stored:?}"
        );
    }
}
