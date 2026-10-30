// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the TEXT SageMaker refuses a resource the account does not
//! hold with.
//!
//! A refusal's HTTP status and error code are only part of its contract: the MESSAGE is a
//! requirement too, because a client matches on it or shows it to a user. Each resource
//! family has its own sentence, and every operation that resolves a resource of that
//! family words it identically — so each test here asserts ONE family's sentence over a
//! TABLE of operations, identifier forms, Regions and reasons the resource is missing,
//! rather than one recorded call:
//!
//! 1. the experiment-tracking and lineage readers, deletes and membership operations say
//!    `<Entity> '<identifier>' does not exist.`, single-quoted;
//! 2. the AI benchmarking family says `<Entity> '<identifier>' not found.`, quoting the
//!    identifier the REQUEST carried; a CREATE resolving a reference of that family adds
//!    ` for this account`, and a DELETE relays the inner service's rendering of it;
//! 3. the cluster reader and delete say `Cluster with name <identifier> not found` — no
//!    quoting, no closing period;
//! 4. the model-group policy read and delete say
//!    `Cannot find resource policy for: <group arn>` — a bare ARN, no closing period.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) on the harness the `inference_deployment_lifecycle` suite stands up, so
//! what these tests exercise is what a real SDK client gets.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A `arn:aws:sagemaker:<region>:<account>:<kind>/<name>` ARN, as this service mints one.
fn arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// The ARN kind segments the lineage and experiment-tracking entities carry, from their
/// modelled ARN patterns (`smithy_query(query="shape",shape="TrialComponentArn")`,
/// `shape="ArtifactArn"`, `shape="ContextArn"`).
const COMPONENT_KIND: &str = "experiment-trial-component";
const ARTIFACT_KIND: &str = "artifact";
const CONTEXT_KIND: &str = "context";

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (`smithy_query(query="operation",operation="CreateAIBenchmarkJob")`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerBenchmarkRole";

/// An `ExecutionRole` for a cluster instance group, same modelled `RoleArn` pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/SageMakerClusterRole";

/// Create the trial component `component` in `region`.
async fn seed_component(emulator: &Emulator, region: &str, component: &str) {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateTrialComponent",
            &json!({ "TrialComponentName": component }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding component {component}: expected HTTP 200, got {status} with {created:?}"
    );
}

/// Create the trial `trial`, and the experiment it belongs to, in `region`.
async fn seed_trial(emulator: &Emulator, region: &str, trial: &str) {
    let experiment = format!("{trial}-experiment");
    for (target, body) in [
        ("CreateExperiment", json!({ "ExperimentName": experiment })),
        (
            "CreateTrial",
            json!({ "TrialName": trial, "ExperimentName": experiment }),
        ),
    ] {
        let (status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            status, 200,
            "seeding {target} {trial}: expected HTTP 200, got {status} with {created:?}"
        );
    }
}

/// RULE: every operation that cannot resolve an experiment-tracking or lineage entity
/// refuses with the modelled `ResourceNotFound` (HTTP 400) whose message is the entity
/// label, then the identifier the request resolved to IN SINGLE QUOTES, then
/// `does not exist.` — a bare name resolved to the ARN against the Region the request
/// signed for, and an identifier that arrived as an ARN or a source URI reported as it
/// arrived.
#[tokio::test]
async fn a_missing_tracking_entity_is_reported_as_not_existing() {
    // Rows differ along the entity kind, the operation that resolves it (read, delete,
    // update, and both membership calls), the identifier's form (bare name, ARN, source
    // URI), the Region signed for, and why the entity is missing (never created, a near
    // miss of one that exists, an ARN naming another entity kind). Each row names the
    // operation, the body, and the sentence the family words.
    struct Row {
        case: &'static str,
        region: &'static str,
        operation: &'static str,
        body: Value,
        expected: String,
    }

    let absent_component_arn = arn("us-east-1", COMPONENT_KIND, "absent-component");
    let rows = vec![
        Row {
            case: "read a component by name",
            region: "us-east-1",
            operation: "DescribeTrialComponent",
            body: json!({ "TrialComponentName": "absent-component" }),
            expected: format!("Trial component '{absent_component_arn}' does not exist."),
        },
        Row {
            case: "read a component by its ARN",
            region: "us-east-1",
            operation: "DescribeTrialComponent",
            body: json!({ "TrialComponentName": absent_component_arn.clone() }),
            expected: format!("Trial component '{absent_component_arn}' does not exist."),
        },
        Row {
            case: "read a component by an ARN of another entity kind",
            region: "us-east-1",
            operation: "DescribeTrialComponent",
            body: json!({ "TrialComponentName": arn("us-east-1", "experiment-trial", "stored") }),
            expected: format!(
                "Trial component '{}' does not exist.",
                arn("us-east-1", "experiment-trial", "stored")
            ),
        },
        Row {
            case: "read a component in another Region",
            region: "eu-west-1",
            operation: "DescribeTrialComponent",
            body: json!({ "TrialComponentName": "absent-component" }),
            expected: format!(
                "Trial component '{}' does not exist.",
                arn("eu-west-1", COMPONENT_KIND, "absent-component")
            ),
        },
        Row {
            case: "delete a component by name",
            region: "ap-south-1",
            operation: "DeleteTrialComponent",
            body: json!({ "TrialComponentName": "absent-component" }),
            expected: format!(
                "Trial component '{}' does not exist.",
                arn("ap-south-1", COMPONENT_KIND, "absent-component")
            ),
        },
        Row {
            case: "associate a component with a stored trial",
            region: "us-east-1",
            operation: "AssociateTrialComponent",
            body: json!({
                "TrialComponentName": "absent-component",
                "TrialName": "stored-trial",
            }),
            expected: format!("Trial component '{absent_component_arn}' does not exist."),
        },
        Row {
            case: "disassociate a component from a stored trial",
            region: "us-east-1",
            operation: "DisassociateTrialComponent",
            body: json!({
                "TrialComponentName": "absent-component",
                "TrialName": "stored-trial",
            }),
            expected: format!("Trial component '{absent_component_arn}' does not exist."),
        },
        Row {
            case: "read an artifact by its ARN",
            region: "us-east-1",
            operation: "DescribeArtifact",
            body: json!({ "ArtifactArn": arn("us-east-1", ARTIFACT_KIND, "absent-artifact") }),
            expected: format!(
                "Artifact '{}' does not exist.",
                arn("us-east-1", ARTIFACT_KIND, "absent-artifact")
            ),
        },
        Row {
            case: "delete an artifact by its ARN",
            region: "eu-west-1",
            operation: "DeleteArtifact",
            body: json!({ "ArtifactArn": arn("eu-west-1", ARTIFACT_KIND, "absent-artifact") }),
            expected: format!(
                "Artifact '{}' does not exist.",
                arn("eu-west-1", ARTIFACT_KIND, "absent-artifact")
            ),
        },
        Row {
            case: "delete an artifact by a source URI nothing was created from",
            region: "us-east-1",
            operation: "DeleteArtifact",
            body: json!({ "Source": { "SourceUri": "s3://bucket/absent/object" } }),
            expected: "Artifact 's3://bucket/absent/object' does not exist.".to_string(),
        },
        Row {
            case: "read a context by name",
            region: "us-east-1",
            operation: "DescribeContext",
            body: json!({ "ContextName": "absent-context" }),
            expected: format!(
                "Context '{}' does not exist.",
                arn("us-east-1", CONTEXT_KIND, "absent-context")
            ),
        },
        Row {
            case: "read a context by its ARN",
            region: "us-east-1",
            operation: "DescribeContext",
            body: json!({ "ContextName": arn("us-west-2", CONTEXT_KIND, "absent-context") }),
            expected: format!(
                "Context '{}' does not exist.",
                arn("us-west-2", CONTEXT_KIND, "absent-context")
            ),
        },
        Row {
            case: "delete a context by name",
            region: "ap-south-1",
            operation: "DeleteContext",
            body: json!({ "ContextName": "absent-context" }),
            expected: format!(
                "Context '{}' does not exist.",
                arn("ap-south-1", CONTEXT_KIND, "absent-context")
            ),
        },
        Row {
            case: "update a context by name",
            region: "us-east-1",
            operation: "UpdateContext",
            body: json!({ "ContextName": "absent-context", "Description": "no such context" }),
            expected: format!(
                "Context '{}' does not exist.",
                arn("us-east-1", CONTEXT_KIND, "absent-context")
            ),
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        // A component, a trial and a context that DO exist throughout, so a refusal is
        // about the identifier the row names rather than about an empty store, and the
        // membership rows get past their trial lookup to the component one.
        seed_component(&emulator, row.region, "stored").await;
        seed_trial(&emulator, row.region, "stored-trial").await;
        let (status, refused) = emulator
            .call_in(row.region, row.operation, &row.body)
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {:?}: expected 400 ResourceNotFound, got {status} with {refused:?}",
            row.case
        );
        assert_eq!(
            message(&refused),
            row.expected,
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            row.expected,
            message(&refused)
        );
    }
}

/// RULE: an entity that was DELETED is as missing as one nobody created, and every reader
/// of it words the same sentence — the identifier the delete answered, single-quoted.
#[tokio::test]
async fn a_deleted_tracking_entity_is_reported_as_not_existing() {
    let emulator = Emulator::new();

    // Rows differ along the entity kind and therefore along the create, delete and read
    // triple that addresses it.
    seed_component(&emulator, "us-east-1", "doomed-component").await;
    let component_arn = str_field(
        &emulator
            .ok(
                "DeleteTrialComponent",
                &json!({ "TrialComponentName": "doomed-component" }),
            )
            .await,
        "TrialComponentArn",
    );

    let created_artifact = emulator
        .ok(
            "CreateArtifact",
            &json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/doomed/object" },
            }),
        )
        .await;
    let artifact_arn = str_field(&created_artifact, "ArtifactArn");
    emulator
        .ok("DeleteArtifact", &json!({ "ArtifactArn": artifact_arn }))
        .await;

    let created_context = emulator
        .ok(
            "CreateContext",
            &json!({
                "ContextName": "doomed-context",
                "ContextType": "Endpoint",
                "Source": { "SourceUri": "https://example.test/contexts/1" },
            }),
        )
        .await;
    let context_arn = str_field(&created_context, "ContextArn");
    emulator
        .ok("DeleteContext", &json!({ "ContextName": "doomed-context" }))
        .await;

    let rows: [(&str, &str, Value, String); 3] = [
        (
            "a deleted trial component",
            "DescribeTrialComponent",
            json!({ "TrialComponentName": "doomed-component" }),
            format!("Trial component '{component_arn}' does not exist."),
        ),
        (
            "a deleted artifact",
            "DescribeArtifact",
            json!({ "ArtifactArn": str_field(&created_artifact, "ArtifactArn") }),
            format!(
                "Artifact '{}' does not exist.",
                str_field(&created_artifact, "ArtifactArn")
            ),
        ),
        (
            "a deleted context",
            "DescribeContext",
            json!({ "ContextName": "doomed-context" }),
            format!("Context '{context_arn}' does not exist."),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, refused) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// A `CreateAIBenchmarkJob` body naming `config` as the workload configuration.
fn benchmark_body(name: &str, config: &str) -> Value {
    json!({
        "AIBenchmarkJobName": name,
        "AIWorkloadConfigIdentifier": config,
        "RoleArn": ROLE_ARN,
        "BenchmarkTarget": { "Endpoint": { "Identifier": "an-endpoint" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
    })
}

/// A `CreateAIRecommendationJob` body naming `config` as the workload configuration.
fn recommendation_body(name: &str, config: &str) -> Value {
    json!({
        "AIRecommendationJobName": name,
        "AIWorkloadConfigIdentifier": config,
        "RoleArn": ROLE_ARN,
        "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/llama/" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/recommendations/" },
        "PerformanceTarget": { "Constraints": [{ "Metric": "ttft-ms" }] },
    })
}

/// RULE: the AI benchmarking family refuses a resource the account does not hold with the
/// modelled `ResourceNotFound` (HTTP 400) whose message is the entity label, then the
/// identifier THE REQUEST CARRIED in single quotes, then `not found.` — and a create that
/// cannot resolve a reference of the family states the scope it searched,
/// `not found for this account.`
#[tokio::test]
async fn a_missing_ai_resource_is_reported_as_not_found() {
    // Rows differ along the resource kind (workload configuration, benchmark job,
    // recommendation job), the operation (read, delete, and the two creates that resolve a
    // configuration reference), the identifier's form (a bare name, the configuration's
    // ARN, a near miss of a name that exists), and the Region signed for.
    let config_arn = arn("us-east-1", "ai-workload-config", "absent-config");
    let rows: [(&str, &str, &str, Value, String); 9] = [
        (
            "read a configuration by name",
            "us-east-1",
            "DescribeAIWorkloadConfig",
            json!({ "AIWorkloadConfigName": "absent-config" }),
            "AIWorkloadConfig 'absent-config' not found.".to_string(),
        ),
        (
            "read a configuration whose name differs only in case",
            "us-east-1",
            "DescribeAIWorkloadConfig",
            json!({ "AIWorkloadConfigName": "STORED" }),
            "AIWorkloadConfig 'STORED' not found.".to_string(),
        ),
        (
            "read a configuration in another Region",
            "eu-west-1",
            "DescribeAIWorkloadConfig",
            json!({ "AIWorkloadConfigName": "absent-config" }),
            "AIWorkloadConfig 'absent-config' not found.".to_string(),
        ),
        (
            "stop a recommendation job by name",
            "us-east-1",
            "StopAIRecommendationJob",
            json!({ "AIRecommendationJobName": "absent-job" }),
            "Job 'absent-job' not found.".to_string(),
        ),
        (
            "read a benchmark job by name",
            "us-east-1",
            "DescribeAIBenchmarkJob",
            json!({ "AIBenchmarkJobName": "absent-job" }),
            "AIBenchmarkJob 'absent-job' not found.".to_string(),
        ),
        (
            "read a recommendation job by name",
            "ap-south-1",
            "DescribeAIRecommendationJob",
            json!({ "AIRecommendationJobName": "absent-job" }),
            "Job 'absent-job' not found.".to_string(),
        ),
        (
            "create a benchmark job on a configuration named by name",
            "us-east-1",
            "CreateAIBenchmarkJob",
            benchmark_body("fresh-bench", "absent-config"),
            "AIWorkloadConfig 'absent-config' not found for this account.".to_string(),
        ),
        (
            "create a benchmark job on a configuration named by ARN",
            "us-east-1",
            "CreateAIBenchmarkJob",
            benchmark_body("fresh-bench", &config_arn),
            format!("AIWorkloadConfig '{config_arn}' not found for this account."),
        ),
        (
            "create a recommendation job on a configuration named by name",
            "eu-west-1",
            "CreateAIRecommendationJob",
            recommendation_body("fresh-rec", "absent-config"),
            "AIWorkloadConfig 'absent-config' not found for this account.".to_string(),
        ),
    ];

    for (case, region, operation, body, expected) in rows {
        let emulator = Emulator::new();
        // A configuration that DOES exist throughout, so a refusal is about the identifier
        // the row names rather than about an empty store.
        let (status, seeded) = emulator
            .call_in(
                region,
                "CreateAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": "stored" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding the stored configuration must answer 200, got {status} \
             with {seeded:?}"
        );

        let (status, refused) = emulator.call_in(region, operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex shape every AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    let shaped = [8usize, 4, 4, 4, 12].iter().all(|width| {
        groups.next().is_some_and(|group| {
            group.len() == *width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    });
    shaped && groups.next().is_none()
}

/// RULE: the AI family's DELETE operations front an internal service and report THAT
/// service's rendering of the failure: the family's `<Entity> '<name>' not found.`
/// sentence, then the service that raised it, the status it was raised with, the request id
/// of that call and its attempt count — the id minted per call, so two refusals of the same
/// name do not report one recorded value.
#[tokio::test]
async fn a_missing_ai_resource_is_relayed_with_the_inner_service_tail() {
    // Rows differ along the resource kind the delete addresses and the name's shape,
    // including both published bounds of `AIEntityName`'s `len 1..63` (smithy_query
    // shape=AIEntityName).
    let longest = format!("a{}z", "b".repeat(61));
    let rows: [(&str, &str, &str, &str); 6] = [
        (
            "delete a configuration",
            "DeleteAIWorkloadConfig",
            "AIWorkloadConfigName",
            "absent-config",
        ),
        (
            "delete a configuration named by one character",
            "DeleteAIWorkloadConfig",
            "AIWorkloadConfigName",
            "a",
        ),
        (
            "delete a configuration named at the length maximum",
            "DeleteAIWorkloadConfig",
            "AIWorkloadConfigName",
            &longest,
        ),
        (
            "delete a recommendation job",
            "DeleteAIRecommendationJob",
            "AIRecommendationJobName",
            "absent-job",
        ),
        (
            "delete a recommendation job whose name differs only in case",
            "DeleteAIRecommendationJob",
            "AIRecommendationJobName",
            "STORED",
        ),
        (
            "delete a recommendation job named at the length maximum",
            "DeleteAIRecommendationJob",
            "AIRecommendationJobName",
            &longest,
        ),
    ];

    for (case, operation, member, name) in rows {
        let emulator = Emulator::new();
        let entity = if operation == "DeleteAIWorkloadConfig" {
            "AIWorkloadConfig"
        } else {
            "Job"
        };
        let (status, refused) = emulator.call(operation, &json!({ member: name })).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused:?}"
        );

        let reported = message(&refused);
        let sentence = format!("{entity} '{name}' not found.");
        let (head, tail) = reported.split_once(" (Service: ").unwrap_or((&reported, ""));
        assert_eq!(
            head, sentence,
            "case {case:?}: expected the family's sentence first, got {reported:?}"
        );
        let tail_head = "SageMakerLumen, Status Code: 400, Request ID: ";
        let suffix = ") (SDK Attempt Count: 1)";
        assert!(
            tail.starts_with(tail_head) && tail.ends_with(suffix),
            "case {case:?}: expected the inner service's tail, got {reported:?}"
        );
        let request_id = &tail[tail_head.len()..tail.len() - suffix.len()];
        assert!(
            is_request_id(request_id),
            "case {case:?}: expected a minted request id, got {request_id:?}"
        );

        // Minted per call, never a value a recording fixed: a second refusal of the same
        // name reports a different id and the same sentence.
        let (_, again) = emulator.call(operation, &json!({ member: name })).await;
        assert_ne!(
            message(&again),
            reported,
            "case {case:?}: each refusal must report its own request id, got {reported:?} twice"
        );
        assert!(
            message(&again).starts_with(&sentence),
            "case {case:?}: the sentence must not change between refusals, got {:?}",
            message(&again)
        );
    }
}

/// RULE: the operations whose subject IS a HyperPod cluster refuse an identifier the
/// account holds no cluster under with the modelled `ResourceNotFound` (HTTP 400) whose
/// message names that identifier as the request carried it — unquoted, and with no closing
/// period.
#[tokio::test]
async fn a_missing_cluster_is_reported_by_the_identifier_the_request_carried() {
    // Rows differ along the operation (read, delete), the identifier's form (a bare name,
    // a well-formed `ClusterArn` naming no cluster — `cluster/[a-z0-9]{12}` — and a name
    // that differs from a stored one only in case), and the Region signed for.
    let absent_arn = arn("us-east-1", "cluster", "0a1b2c3d4e5f");
    let rows: [(&str, &str, &str, String); 6] = [
        ("read by name", "us-east-1", "DescribeCluster", "absent-cluster".to_string()),
        ("read by ARN", "us-east-1", "DescribeCluster", absent_arn.clone()),
        (
            "read a name differing only in case",
            "us-east-1",
            "DescribeCluster",
            "STORED".to_string(),
        ),
        ("read in another Region", "eu-west-1", "DescribeCluster", "absent-cluster".to_string()),
        ("delete by name", "us-east-1", "DeleteCluster", "absent-cluster".to_string()),
        ("delete by ARN", "ap-south-1", "DeleteCluster", arn("ap-south-1", "cluster", "9f8e7d6c5b4a")),
    ];

    for (case, region, operation, identifier) in rows {
        let emulator = Emulator::new();
        // A cluster that DOES exist throughout, so a refusal is about the identifier the
        // row names rather than about an empty store.
        let (status, seeded) = emulator
            .call_in(
                region,
                "CreateCluster",
                &json!({
                    "ClusterName": "stored",
                    "InstanceGroups": [{
                        "InstanceGroupName": "workers",
                        "InstanceCount": 1,
                        "InstanceType": "ml.m5.large",
                        "ExecutionRole": EXECUTION_ROLE,
                    }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding the stored cluster must answer 200, got {status} with \
             {seeded:?}"
        );

        let (status, refused) = emulator
            .call_in(region, operation, &json!({ "ClusterName": identifier }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Cluster with name {identifier} not found"),
            "case {case:?}: expected the identifier as sent, got {:?}",
            message(&refused)
        );
    }
}

/// A policy document naming `owner`, so a stored document is identifiable.
fn policy_for(owner: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"Policy{}","Effect":"Allow","Principal":{{"AWS":"*"}},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}}]}}"#,
        owner.replace(['-', '_'], "")
    )
}

/// RULE: a read or a delete of a model group's resource policy that finds none is refused
/// HTTP 400 `ValidationException` whose message is `Cannot find resource policy for: ` and
/// then the group's ARN — resolved from the requested name against the Region the request
/// signed for — BARE: no quotes around it and no closing period.
#[tokio::test]
async fn a_missing_group_policy_names_the_group_arn_bare() {
    // Rows differ along the operation (read, delete), why there is no policy (no such
    // group, a group that never carried one, a group whose policy was deleted), the group
    // name's shape (including both published `EntityName` length bounds, 1..63), and the
    // Region signed for.
    let longest = format!("a{}z", "b".repeat(61));
    let rows: [(&str, &str, &str, &str, bool, bool); 8] = [
        ("read, no such group", "us-east-1", "GetModelPackageGroupPolicy", "absent", false, false),
        (
            "read, a group carrying no policy",
            "us-east-1",
            "GetModelPackageGroupPolicy",
            "bare-group",
            true,
            false,
        ),
        (
            "read, a group whose policy was deleted",
            "us-east-1",
            "GetModelPackageGroupPolicy",
            "released-group",
            true,
            true,
        ),
        (
            "read, a one-character name",
            "eu-west-1",
            "GetModelPackageGroupPolicy",
            "g",
            false,
            false,
        ),
        (
            "delete, no such group",
            "us-east-1",
            "DeleteModelPackageGroupPolicy",
            "absent",
            false,
            false,
        ),
        (
            "delete, a group carrying no policy",
            "ap-south-1",
            "DeleteModelPackageGroupPolicy",
            "bare-group",
            true,
            false,
        ),
        (
            "delete, a group whose policy was deleted",
            "us-east-1",
            "DeleteModelPackageGroupPolicy",
            "released-group",
            true,
            true,
        ),
        (
            "delete, a 63-character name",
            "us-east-1",
            "DeleteModelPackageGroupPolicy",
            "placeholder",
            false,
            false,
        ),
    ];

    for (case, region, operation, name, create_group, attach_then_release) in rows {
        let emulator = Emulator::new();
        let name = if case.ends_with("63-character name") {
            longest.clone()
        } else {
            name.to_string()
        };
        assert!(
            !name.is_empty() && name.len() <= 63,
            "case {case:?}: the row's name must satisfy EntityName's 1..63 bounds, got {name:?}"
        );

        if create_group {
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: seeding the group must answer 200, got {status} with {created:?}"
            );
        }
        if attach_then_release {
            for (target, body) in [
                (
                    "PutModelPackageGroupPolicy",
                    json!({
                        "ModelPackageGroupName": name,
                        "ResourcePolicy": policy_for(&name),
                    }),
                ),
                (
                    "DeleteModelPackageGroupPolicy",
                    json!({ "ModelPackageGroupName": name }),
                ),
            ] {
                let (status, answered) = emulator.call_in(region, target, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {target} must answer 200, got {status} with {answered:?}"
                );
            }
        }

        let (status, refused) = emulator
            .call_in(region, operation, &json!({ "ModelPackageGroupName": name }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "Cannot find resource policy for: {}",
                arn(region, "model-package-group", &name)
            ),
            "case {case:?}: expected the bare group ARN, got {:?}",
            message(&refused)
        );
    }
}
