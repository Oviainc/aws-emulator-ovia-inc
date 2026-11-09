// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how the Inference Recommender surfaces REFUSE:
//! `GetScalingConfigurationRecommendation` and `DescribeInferenceRecommendationsJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! recommendation is picked by exactly one of `RecommendationId` and `EndpointName`, a
//! rule about the REQUEST that is judged before anything is looked up, so a request
//! naming neither is refused with the sentence that names both members whether or not
//! the job exists; a job this account holds none of is refused with the modelled
//! `ResourceNotFound` naming that job; and the job's own reader words a job it cannot
//! find with the account it looked in, while a surface reporting over a job words it
//! without one.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, Emulator, ACCOUNT};

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerRecommenderRole";

/// The model every job benchmarks.
const MODEL: &str = "recommender-model";

/// How a surface that reports OVER a recommendation job words a job it cannot find.
fn job_not_found(name: &str) -> String {
    format!("Inference recommendations job {name} was not found.")
}

/// How the job's own reader words a job it cannot find: qualified by the account it
/// looked in.
fn job_not_found_for_account(name: &str) -> String {
    format!("Inference recommendations job {name} for {ACCOUNT} not found.")
}

/// Register the model and a recommendation job named `job`, both in `us-east-1`.
async fn seed_job(emulator: &Emulator, job: &str) {
    emulator
        .ok(
            "CreateModel",
            &json!({
                "ModelName": MODEL,
                "PrimaryContainer": {
                    "Image": "000000000000.dkr.ecr.us-east-1.amazonaws.com/ranker:1",
                },
                "ExecutionRoleArn": ROLE_ARN,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInferenceRecommendationsJob",
            &json!({
                "JobName": job,
                "JobType": "Default",
                "RoleArn": ROLE_ARN,
                "InputConfig": { "ModelName": MODEL },
            }),
        )
        .await;
}

/// RULE: `GetScalingConfigurationRecommendation` refuses a job this account holds none of
/// with the modelled `ResourceNotFound` naming that job — whichever selector the request
/// carries.
#[tokio::test]
async fn an_unknown_job_is_refused_for_the_job_whatever_selector_the_request_carries() {
    let emulator = Emulator::new();
    seed_job(&emulator, "held-job").await;

    // Rows differ along the selector (an endpoint name, a recommendation id) and along the
    // shape of the job name that names no job (`RecommendationJobName`
    // is len 1..64, `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,63}$` — smithy_query
    // operation=GetScalingConfigurationRecommendation).
    let longest = format!("j{}", "a".repeat(63));
    let names = [
        "j".to_string(),
        "absent-job".to_string(),
        "Held-Job".to_string(),
        longest,
    ];
    let selectors: Vec<(&str, Value)> = vec![
        ("an endpoint name", json!({ "EndpointName": "ep-a" })),
        ("a recommendation id", json!({ "RecommendationId": "r-1" })),
    ];

    for name in &names {
        for (label, selector) in &selectors {
            let case = format!("{name}/{label}");
            let mut body = json!({ "InferenceRecommendationsJobName": name });
            for (member, value) in selector.as_object().expect("a JSON object of selectors") {
                body[member] = value.clone();
            }
            let (status, refused) = emulator
                .call("GetScalingConfigurationRecommendation", &body)
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFound",
                "case {case}: error code, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                job_not_found(name),
                "case {case}: the refusal must name the job the request asked about, \
                 got {refused:?}"
            );
        }
    }
}

/// RULE: the recommendation is picked by exactly one of `RecommendationId` and
/// `EndpointName` — a rule about the REQUEST, so a request naming neither is refused with
/// a 400 `ValidationException` whose message names both members the caller may supply,
/// whether or not the job it names exists.
#[tokio::test]
async fn a_request_naming_no_selector_is_refused_for_the_selector() {
    // Rows differ along whether the account holds the job the request names, and along
    // what else the request carries: nothing, a utilisation target, a traffic objective —
    // none of which can pick a recommendation.
    let extras: Vec<(&str, Value)> = vec![
        ("nothing else", json!({})),
        (
            "a utilisation target",
            json!({ "TargetCpuUtilizationPerCore": 60 }),
        ),
        (
            "a traffic objective",
            json!({
                "ScalingPolicyObjective": {
                    "MinInvocationsPerMinute": 10,
                    "MaxInvocationsPerMinute": 100,
                },
            }),
        ),
    ];

    for (label, extra) in &extras {
        for job in ["selector-job", "no-such-job"] {
            let case = format!("{label}/{job}");
            let emulator = Emulator::new();
            seed_job(&emulator, "selector-job").await;
            let mut body = json!({ "InferenceRecommendationsJobName": job });
            for (member, value) in extra.as_object().expect("a JSON object of extra members") {
                body[member] = value.clone();
            }
            let (status, refused) = emulator
                .call("GetScalingConfigurationRecommendation", &body)
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: error code, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                "Neither RecommendationId or EndpointName were provided in the input. Please \
                 provide a RecommendationId or EndpointName in the request.",
                "case {case:?}: the refusal must name both selectors, got {refused:?}"
            );
        }
    }
}

/// RULE: the job's own reader words a job it cannot find with the account it looked in,
/// while a surface that reports over a job words the same fact without one — so a client
/// can tell the two refusals apart whatever name it asked about.
#[tokio::test]
async fn each_recommendation_surface_words_an_absent_job_its_own_way() {
    let emulator = Emulator::new();
    seed_job(&emulator, "worded-job").await;

    // Rows: the name's shape, plus the name of a job that IS held but is asked about in a
    // different case (the name is matched as a whole).
    for name in ["j", "absent", "Worded-Job", "a-job-9"] {
        let (status, refused) = emulator
            .call(
                "DescribeInferenceRecommendationsJob",
                &json!({ "JobName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {name}: DescribeInferenceRecommendationsJob, got {status} with {refused}"
        );
        assert_eq!(
            message(&refused),
            job_not_found_for_account(name),
            "case {name}: the reader's refusal names the account it looked in, \
             got {refused:?}"
        );

        let (status, over) = emulator
            .call(
                "GetScalingConfigurationRecommendation",
                &json!({
                    "InferenceRecommendationsJobName": name,
                    "EndpointName": "ep-a",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&over).as_str()),
            (400, "ResourceNotFound"),
            "case {name}: GetScalingConfigurationRecommendation, got {status} with {over}"
        );
        assert_eq!(
            message(&over),
            job_not_found(name),
            "case {name}: a surface over a job words it without the account, got {over:?}"
        );
    }

    // The job that IS held is still reported, so the refusals above are about the name.
    let described = emulator
        .ok(
            "DescribeInferenceRecommendationsJob",
            &json!({ "JobName": "worded-job" }),
        )
        .await;
    assert_eq!(
        described.get("JobName").and_then(Value::as_str),
        Some("worded-job"),
        "the held job must still be reported, got {described:?}"
    );
}
