// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the TEXT SageMaker refuses a CREATE with.
//!
//! A refusal's HTTP status and error code are only part of its contract: the MESSAGE is a
//! requirement too, because a client matches on it or shows it to a user. This suite pins
//! the three rules the create refusals follow:
//!
//! 1. a name already taken is worded per RESOURCE TYPE — the sentence names that resource
//!    type and the identifier it is addressed by, so no create borrows another resource's
//!    sentence;
//! 2. a request whose members contradict each other is refused for THAT, before any
//!    resource it references is resolved against the store;
//! 3. a reference the account cannot resolve quotes the identifier the request SUBMITTED,
//!    never an ARN the service synthesised from it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) on the harness the `inference_deployment_lifecycle` suite stands up, so
//! what these tests exercise is what a real SDK client gets.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, message, one_variant_config, primary_container, Emulator, ACCOUNT,
};
use crate::pipeline_execution::definition_declaring;
use crate::pipeline_lifecycle::{create_body as pipeline_body, definition, step, token};

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerExecutionRole";

/// The three members `TrainingSpecification` marks required
/// (`smithy_query(query="shape",shape="TrainingSpecification")`), with one channel
/// carrying the members `ChannelSpecification` marks required.
fn training_specification(image: &str) -> Value {
    json!({
        "TrainingImage": image,
        "SupportedTrainingInstanceTypes": ["ml.m5.large"],
        "TrainingChannels": [{
            "Name": "train",
            "SupportedContentTypes": ["text/csv"],
            "SupportedInputModes": ["File"],
        }],
    })
}

/// A `CreateFeatureGroup` body carrying the members the operation marks required, plus
/// the online store a group must have one of the two stores to be created with.
fn feature_group_body(name: &str, identifier: &str) -> Value {
    json!({
        "FeatureGroupName": name,
        "RecordIdentifierFeatureName": identifier,
        "EventTimeFeatureName": "event-time",
        "FeatureDefinitions": [
            { "FeatureName": identifier, "FeatureType": "String" },
            { "FeatureName": "event-time", "FeatureType": "String" },
        ],
        "OnlineStoreConfig": { "EnableOnlineStore": true },
    })
}

/// One `InferenceComponentSpecification` naming `model`, with the compute requirement
/// whose `MinMemoryRequiredInMb` member the shape makes required.
fn component_specification(model: &str) -> Value {
    json!({
        "ModelName": model,
        "InstanceType": "ml.m5.large",
        "ComputeResourceRequirements": { "MinMemoryRequiredInMb": 1024 },
    })
}

/// One row of the name-collision table: the operation, the body that TAKES the name and
/// the body that collides with it, and the refusal the second one must answer.
struct Collision {
    case: &'static str,
    /// The region the two calls sign for, so a sentence carrying an ARN is pinned against
    /// a region other than the default too.
    region: &'static str,
    operation: &'static str,
    first: Value,
    second: Value,
    code: &'static str,
    expected: String,
}

/// RULE: SageMaker words a create's refusal of a name already taken per RESOURCE TYPE —
/// the sentence names that resource type and the identifier the resource is addressed by
/// (the name the request submitted, or the existing resource's ARN where that type's
/// sentence quotes the ARN) — so a create never answers with another resource's sentence,
/// and in particular not with a catch-all that identifies neither the resource nor the
/// name.
#[tokio::test]
async fn each_create_words_its_own_name_collision_refusal() {
    // Rows span the resource types the rule covers, and within them the name's shape (one
    // character, digits, hyphens), the region signed for, how the second body differs from
    // the first, and whether the sentence quotes the submitted name or the stored ARN. The
    // last row is the control: for an app image config the resource's OWN sentence IS the
    // service's shared one, so the rule leaves it exactly as it is.
    let rows: Vec<Collision> = vec![
        Collision {
            case: "experiment",
            region: "us-east-1",
            operation: "CreateExperiment",
            first: json!({ "ExperimentName": "tracking-run" }),
            second: json!({ "ExperimentName": "tracking-run", "Description": "a second run" }),
            code: "ValidationException",
            expected: "Experiment names must be unique within an AWS account and region. \
                       Experiment with name (tracking-run) already exists."
                .to_string(),
        },
        Collision {
            // `ExperimentName` is "not case-sensitive", so a name differing only in case
            // collides — and the sentence echoes the name THIS request sent, not the
            // spelling the stored record carries.
            case: "experiment-differing-only-in-case",
            region: "eu-west-1",
            operation: "CreateExperiment",
            first: json!({ "ExperimentName": "TrackingRun" }),
            second: json!({ "ExperimentName": "trackingrun" }),
            code: "ValidationException",
            expected: "Experiment names must be unique within an AWS account and region. \
                       Experiment with name (trackingrun) already exists."
                .to_string(),
        },
        Collision {
            case: "ai-workload-config",
            region: "us-east-1",
            operation: "CreateAIWorkloadConfig",
            first: json!({ "AIWorkloadConfigName": "wlc9" }),
            second: json!({
                "AIWorkloadConfigName": "wlc9",
                "AIWorkloadConfigs": { "WorkloadSpec": { "Inline": "concurrency: 2\n" } },
            }),
            code: "ResourceInUse",
            expected: "AIWorkloadConfig with name 'wlc9' already exists for this account."
                .to_string(),
        },
        Collision {
            // The one sentence in the table that quotes the ARN of the resource already
            // holding the name, so this row is signed for a non-default region.
            case: "algorithm",
            region: "ap-southeast-2",
            operation: "CreateAlgorithm",
            first: json!({
                "AlgorithmName": "a",
                "TrainingSpecification": training_specification("demo-training:1"),
            }),
            second: json!({
                "AlgorithmName": "a",
                "TrainingSpecification": training_specification("other-training:9"),
                "CertifyForMarketplace": true,
            }),
            code: "ValidationException",
            expected: format!(
                "Algorithm already exists: arn:aws:sagemaker:ap-southeast-2:{ACCOUNT}:algorithm/a"
            ),
        },
        Collision {
            case: "feature-group",
            region: "us-east-1",
            operation: "CreateFeatureGroup",
            first: feature_group_body("customer-features", "customer-id"),
            second: feature_group_body("customer-features", "order-id"),
            code: "ResourceInUse",
            expected: "Resource Already Exists: FeatureGroup with name customer-features \
                       already exists. Choose a different name."
                .to_string(),
        },
        Collision {
            case: "hub",
            region: "us-east-1",
            operation: "CreateHub",
            first: json!({ "HubName": "h", "HubDescription": "a hub" }),
            second: json!({ "HubName": "h", "HubDescription": "another hub" }),
            code: "ResourceInUse",
            expected: "A hub with name h already exists. Please use a different name and \
                       try again."
                .to_string(),
        },
        Collision {
            case: "image",
            region: "eu-west-1",
            operation: "CreateImage",
            first: json!({ "ImageName": "studio-image-1", "RoleArn": ROLE_ARN }),
            second: json!({
                "ImageName": "studio-image-1",
                "RoleArn": ROLE_ARN,
                "Description": "overwrite",
            }),
            code: "ResourceInUse",
            expected: "The Image with ImageName studio-image-1 already exists. Please use \
                       a different ImageName."
                .to_string(),
        },
        Collision {
            case: "app-image-config",
            region: "us-east-1",
            operation: "CreateAppImageConfig",
            first: json!({ "AppImageConfigName": "kernel-config" }),
            second: json!({ "AppImageConfigName": "kernel-config", "CodeEditorAppImageConfig": {} }),
            code: "ResourceInUse",
            expected: "The ID or Name specified is already in use.".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let (status, created) = emulator.call_in(row.region, row.operation, &row.first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} with \
             {created:?}"
        );

        let (status, refused) = emulator.call_in(row.region, row.operation, &row.second).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, row.code),
            "case {case:?}: expected 400 {}, got {status} with {refused:?}",
            row.code
        );
        assert_eq!(
            message(&refused),
            row.expected,
            "case {case:?}: expected {:?}, got {:?}",
            row.expected,
            message(&refused)
        );
    }

    // No two of these sentences are the same text: a collision refusal identifies the
    // resource type it is about, so a client cannot confuse one for another. (The app
    // image config and the shared sentence are one row, so the whole table is distinct.)
    let mut sentences: Vec<&str> = rows.iter().map(|row| row.expected.as_str()).collect();
    sentences.sort_unstable();
    let distinct = sentences.len();
    sentences.dedup();
    assert_eq!(
        sentences.len(),
        distinct,
        "every resource type must have its own collision sentence, got {sentences:?}"
    );
}

/// RULE: a pipeline definition must declare at least one step, and `CreatePipeline`
/// refuses one whose `Steps` list is empty with the fixed sentence naming the part of the
/// request being judged and the minimum the definition breaks — whatever else the
/// definition carries.
#[tokio::test]
async fn a_definition_declaring_no_steps_is_refused_with_the_minimum_step_requirement() {
    let emulator = Emulator::new();

    // The sentence carries nothing from the request, so the rows vary everything the
    // request CAN vary: the pipeline's name, the parameters the definition declares, and
    // whether the empty `Steps` list sits in a definition that is otherwise minimal.
    let rows: Vec<(&str, String)> = vec![
        ("bare-empty-steps", definition(json!([]))),
        ("empty-steps-with-no-parameters", definition_declaring(&[], json!([]))),
        (
            "empty-steps-beside-declared-parameters",
            definition_declaring(&["Alpha", "Beta"], json!([])),
        ),
    ];
    let expected = "Pipeline definition: At least 1 step must be provided";

    for (case, document) in &rows {
        let (status, refused) = emulator
            .call(
                "CreatePipeline",
                &pipeline_body(case, document, &token(case)),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );

        // Nothing was stored: the refusal is about the definition, not about a pipeline.
        let (status, missing) = emulator
            .call("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert_eq!(
            (status, error_code(&missing).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: a refused create must persist nothing, got {status} with \
             {missing:?}"
        );
    }

    // And ONE step is enough: the refusal is about the empty list, not about the schema.
    let (status, accepted) = emulator
        .call(
            "CreatePipeline",
            &pipeline_body(
                "one-step",
                &definition(json!([step("Only", &[])])),
                &token("one-step"),
            ),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"one-step\": a definition declaring one step is accepted, got {status} \
         with {accepted:?}"
    );
}

/// RULE: `JobType` `Default` runs an instance recommendation and `Advanced` runs a load
/// test, so a `Default` job carrying an `InputConfig` member the model documents as
/// belonging to the load test is refused naming that member — and because the rule judges
/// the request's own members, it is decided before any resource the request references is
/// resolved against the account.
#[tokio::test]
async fn a_default_recommendation_job_refuses_the_load_test_only_input_members() {
    // The model every accepted row benchmarks.
    const MODEL: &str = "recommender-model";

    // The load-test-only members, from their own documentation: `TrafficPattern`
    // "defines the traffic pattern of the load test"
    // (`smithy_query(query="shape",shape="TrafficPattern")`) and `ResourceLimit`
    // "defines the maximum number of load tests"
    // (`shape="RecommendationJobResourceLimit"`).
    let resource_limit = json!({ "MaxNumberOfTests": 5, "MaxParallelOfTests": 2 });
    let traffic_pattern = json!({
        "TrafficType": "PHASES",
        "Phases": [{ "InitialNumberOfUsers": 1, "SpawnRate": 1, "DurationInSeconds": 120 }],
    });

    // Rows differ along the member carried, the job type, and whether the model the
    // request names exists — the axis that shows the ORDER: a `Default` job carrying a
    // load-test member is refused for that member even when its `ModelName` names nothing,
    // and an `Advanced` job carrying the same member is accepted.
    struct Row {
        case: &'static str,
        job_type: &'static str,
        model: &'static str,
        input: Value,
        /// `None` when the create must be accepted.
        expected: Option<&'static str>,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "default-with-resource-limit",
            job_type: "Default",
            model: MODEL,
            input: json!({ "ResourceLimit": resource_limit }),
            expected: Some("Default JobType will not accept ResourceLimit"),
        },
        Row {
            case: "default-with-resource-limit-and-an-unknown-model",
            job_type: "Default",
            model: "never-created-model",
            input: json!({ "ResourceLimit": resource_limit }),
            expected: Some("Default JobType will not accept ResourceLimit"),
        },
        Row {
            case: "default-with-traffic-pattern",
            job_type: "Default",
            model: MODEL,
            input: json!({ "TrafficPattern": traffic_pattern }),
            expected: Some("Default JobType will not accept TrafficPattern"),
        },
        Row {
            case: "default-with-traffic-pattern-and-an-unknown-model",
            job_type: "Default",
            model: "never-created-model",
            input: json!({ "TrafficPattern": traffic_pattern }),
            expected: Some("Default JobType will not accept TrafficPattern"),
        },
        Row {
            case: "advanced-with-resource-limit",
            job_type: "Advanced",
            model: MODEL,
            input: json!({ "ResourceLimit": resource_limit }),
            expected: None,
        },
        Row {
            case: "advanced-with-traffic-pattern",
            job_type: "Advanced",
            model: MODEL,
            input: json!({ "TrafficPattern": traffic_pattern }),
            expected: None,
        },
        Row {
            case: "advanced-with-both",
            job_type: "Advanced",
            model: MODEL,
            input: json!({ "ResourceLimit": resource_limit, "TrafficPattern": traffic_pattern }),
            expected: None,
        },
        Row {
            case: "default-with-neither",
            job_type: "Default",
            model: MODEL,
            input: json!({ "JobDurationInSeconds": 600 }),
            expected: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModel",
                &json!({
                    "ModelName": MODEL,
                    "PrimaryContainer": primary_container("ranker:1"),
                    "ExecutionRoleArn": ROLE_ARN,
                }),
            )
            .await;

        let mut request = json!({
            "JobName": case,
            "JobType": row.job_type,
            "RoleArn": ROLE_ARN,
            "InputConfig": { "ModelName": row.model },
        });
        for (member, value) in row.input.as_object().expect("a JSON object of InputConfig members")
        {
            request["InputConfig"][member] = value.clone();
        }

        let (status, answered) = emulator
            .call("CreateInferenceRecommendationsJob", &request)
            .await;
        match row.expected {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected 400 ValidationException, got {status} with \
                     {answered:?}"
                );
                assert_eq!(
                    message(&answered),
                    expected,
                    "case {case:?}: expected {expected:?}, got {:?}",
                    message(&answered)
                );
                let (status, missing) = emulator
                    .call(
                        "DescribeInferenceRecommendationsJob",
                        &json!({ "JobName": case }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&missing).as_str()),
                    (400, "ResourceNotFound"),
                    "case {case:?}: a refused create must persist nothing, got {status} \
                     with {missing:?}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: a load test may carry the member, got {status} with \
                 {answered:?}"
            ),
        }
    }
}

/// RULE: a create that cannot resolve a hosting resource the request references refuses
/// with `Could not find <kind> "<identifier as submitted>".` — the identifier the request
/// carried, never an ARN the service synthesised from it, because a resource the account
/// does not hold has no ARN the caller could match against what it sent. References are
/// resolved outside in, so the request's own top-level reference is the one reported when
/// more than one does not resolve.
#[tokio::test]
async fn an_unresolvable_reference_quotes_the_identifier_the_request_submitted() {
    // Rows differ along the resource kind that is missing (endpoint, model, both), the
    // identifier's shape — one character, digits, hyphens, a name that differs from an
    // existing one only in case, and the modelled maximum of
    // `EndpointName`/`ModelName` (len 1..63) — and which reference is reported when two
    // of them are unresolvable.
    let long_name = format!("a{}b", "2345678901234567890123456789012345678901234567890123456789");
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "endpoint-one-character",
            "endpoint",
            "e".to_string(),
            "endpoint".to_string(),
        ),
        (
            "endpoint-with-hyphens",
            "endpoint",
            "never-created-endpoint".to_string(),
            "endpoint".to_string(),
        ),
        (
            "endpoint-differing-only-in-case",
            "endpoint",
            "Hosted-Ep".to_string(),
            "endpoint".to_string(),
        ),
        (
            "endpoint-at-the-modelled-maximum",
            "endpoint",
            long_name.clone(),
            "endpoint".to_string(),
        ),
        (
            "model-with-digits",
            "model",
            "ranker9".to_string(),
            "model".to_string(),
        ),
        (
            "model-at-the-modelled-maximum",
            "model",
            long_name.clone(),
            "model".to_string(),
        ),
        (
            // Both references are unresolvable: the endpoint is the request's own
            // top-level member, so that is the one the refusal reports.
            "endpoint-and-model-both-missing",
            "both",
            "absent-ep".to_string(),
            "endpoint".to_string(),
        ),
    ];

    for (case, missing, identifier, kind) in &rows {
        let emulator = Emulator::new();
        // A model and an endpoint the account DOES hold, so the refusal below is about
        // the identifier the request named and not about an empty account. The endpoint's
        // name differs from the "differing-only-in-case" row's only in case.
        emulator
            .ok(
                "CreateModel",
                &json!({
                    "ModelName": "hosted-model",
                    "PrimaryContainer": primary_container("demo:1"),
                }),
            )
            .await;
        emulator
            .ok(
                "CreateEndpointConfig",
                &one_variant_config("hosted-cfg", "hosted-model"),
            )
            .await;
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": "hosted-ep", "EndpointConfigName": "hosted-cfg" }),
            )
            .await;

        // Only the referenced resource under test is absent — except in the row where
        // both are, which is what pins the order the two are resolved in.
        let (endpoint, model) = match *missing {
            "endpoint" => (identifier.as_str(), "hosted-model"),
            "both" => (identifier.as_str(), "absent-model"),
            _ => ("hosted-ep", identifier.as_str()),
        };
        let (status, refused) = emulator
            .call(
                "CreateInferenceComponent",
                &json!({
                    "InferenceComponentName": "component",
                    "EndpointName": endpoint,
                    "Specification": component_specification(model),
                    "RuntimeConfig": { "CopyCount": 1 },
                }),
            )
            .await;

        let expected = format!("Could not find {kind} \"{identifier}\".");
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
        assert!(
            !message(&refused).contains("arn:"),
            "case {case:?}: the refusal must quote the submitted identifier rather than \
             an ARN, got {:?}",
            message(&refused)
        );
    }

    // The SAME rule on the other create that resolves these two hosting references, so
    // the sentence is pinned per RULE rather than per operation. Rows differ along the
    // reference member (`InputConfig.ModelName`, `InputConfig.Endpoints[].EndpointName`),
    // the job type, and the identifier's shape.
    let job_rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "recommendations-job-missing-model",
            "Default",
            json!({ "ModelName": "absent-ranker" }),
            "Could not find model \"absent-ranker\".".to_string(),
        ),
        (
            "recommendations-job-missing-model-at-the-modelled-maximum",
            "Advanced",
            json!({ "ModelName": long_name }),
            format!("Could not find model \"{long_name}\"."),
        ),
        (
            "recommendations-job-missing-endpoint",
            "Advanced",
            json!({
                "ModelName": "hosted-model",
                "Endpoints": [{ "EndpointName": "absent-benchmark-ep" }],
            }),
            "Could not find endpoint \"absent-benchmark-ep\".".to_string(),
        ),
    ];

    for (case, job_type, input_config, expected) in &job_rows {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateModel",
                &json!({
                    "ModelName": "hosted-model",
                    "PrimaryContainer": primary_container("demo:1"),
                }),
            )
            .await;

        let (status, refused) = emulator
            .call(
                "CreateInferenceRecommendationsJob",
                &json!({
                    "JobName": "benchmark",
                    "JobType": job_type,
                    "RoleArn": ROLE_ARN,
                    "InputConfig": input_config,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {refused:?}"
        );
        assert_eq!(
            &message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
        assert!(
            !message(&refused).contains("arn:"),
            "case {case:?}: the refusal must quote the submitted identifier rather than \
             an ARN, got {:?}",
            message(&refused)
        );
    }
}
