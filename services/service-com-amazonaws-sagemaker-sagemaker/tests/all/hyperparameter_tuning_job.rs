// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's hyperparameter tuning jobs:
//! `CreateHyperParameterTuningJob` and `DescribeHyperParameterTuningJob`.
//!
//! Every case goes over the wire through the same router the production binary builds,
//! reusing the harness in `inference_deployment_lifecycle` (`Emulator`), so what these
//! tests exercise is what a real SDK client gets.
//!
//! The suites assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * a create persists the job and `Describe` replays every stored member, for every
//!   shape of request the model allows (one definition or a list, each strategy, each
//!   kind of parameter range, warm start, autotune, tags, any signed region);
//! * a request past a published per-tuning-job quota is the modelled
//!   `ResourceLimitExceeded`, tested on each quota's own bound and one past it;
//! * a request breaking a cross-member rule the model's constraints cannot express is a
//!   400 `ValidationException` naming the member, and persists nothing;
//! * a name already in use is the modelled `ResourceInUse` and keeps the stored job;
//! * the job ends `Failed` with a reason saying no training job ran, once the clock owes
//!   that transition — never with a best training job or an objective metric value for
//!   work nothing performed;
//! * a call that CHANGES this service's state leaves a structured audit record naming the
//!   operation, the resource and the access key id the request presented.

use std::time::Duration;

use serde_json::{json, Value};

// One capture of this service's audit records serves the whole test binary: `tracing`
// allows a single global subscriber, so this suite SHARES the sibling suite's capture
// rather than installing a second one that would silently see nothing.
use crate::hyperpod_cluster_audit_record::audit_capture;
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::TUNING_JOB_SETTLES_AFTER`.
const PAST_SETTLE_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TUNING_JOB_SETTLES_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// An IAM role ARN matching `RoleArn`'s modelled pattern.
const ROLE: &str = "arn:aws:iam::123456789012:role/SageMakerTuningRole";

/// The modelled `MaxNumberOfTrainingJobs` quota this emulator holds every account to.
const MAX_TRAINING_JOBS: i64 =
    service_com_amazonaws_sagemaker_sagemaker::state::MAX_TRAINING_JOBS_PER_TUNING_JOB as i64;
/// The modelled `MaxParallelTrainingJobs` quota.
const MAX_PARALLEL_TRAINING_JOBS: i64 =
    service_com_amazonaws_sagemaker_sagemaker::state::MAX_PARALLEL_TRAINING_JOBS_PER_TUNING_JOB
        as i64;

/// A `HyperParameterAlgorithmSpecification` the model accepts.
fn algorithm_specification() -> Value {
    json!({
        "TrainingImage": "123456789012.dkr.ecr.us-east-1.amazonaws.com/xgboost:1",
        "TrainingInputMode": "File",
        "MetricDefinitions": [{ "Name": "validation:rmse", "Regex": "rmse: ([0-9.]+)" }],
    })
}

/// A `HyperParameterTrainingJobDefinition` writing its artifacts under `s3_output`.
fn training_job_definition(s3_output: &str) -> Value {
    json!({
        "AlgorithmSpecification": algorithm_specification(),
        "RoleArn": ROLE,
        "InputDataConfig": [{
            "ChannelName": "train",
            "DataSource": {
                "S3DataSource": {
                    "S3DataType": "S3Prefix",
                    "S3Uri": "s3://tuning-input/train/",
                    "S3DataDistributionType": "FullyReplicated",
                },
            },
            "ContentType": "text/csv",
        }],
        "OutputDataConfig": { "S3OutputPath": s3_output },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
        "StaticHyperParameters": { "objective": "reg:squarederror" },
    })
}

/// A valid `CreateHyperParameterTuningJob` body: a Bayesian search over one integer and
/// one continuous hyperparameter, with a single training job definition.
fn valid_body(name: &str) -> Value {
    json!({
        "HyperParameterTuningJobName": name,
        "HyperParameterTuningJobConfig": {
            "Strategy": "Bayesian",
            "HyperParameterTuningJobObjective": {
                "Type": "Minimize",
                "MetricName": "validation:rmse",
            },
            "ResourceLimits": {
                "MaxNumberOfTrainingJobs": 4,
                "MaxParallelTrainingJobs": 2,
            },
            "ParameterRanges": {
                "IntegerParameterRanges": [{
                    "Name": "max_depth",
                    "MinValue": "3",
                    "MaxValue": "10",
                    "ScalingType": "Auto",
                }],
                "ContinuousParameterRanges": [{
                    "Name": "eta",
                    "MinValue": "0.01",
                    "MaxValue": "0.3",
                    "ScalingType": "Linear",
                }],
            },
            "TrainingJobEarlyStoppingType": "Off",
        },
        "TrainingJobDefinition": training_job_definition("s3://tuning-output/artifacts/"),
        "Tags": [{ "Key": "team", "Value": "science" }],
    })
}

/// The ARN form `HyperParameterTuningJobArn` is modelled with, for `name` in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:hyper-parameter-tuning-job/{name}")
}

/// Assert that every member `expected` names appears in `actual` with the same value,
/// recursively. Members `actual` carries beyond `expected` are ignored, so a case states
/// only what it is about; a list must match entry for entry.
fn assert_subset(expected: &Value, actual: &Value, case: &str, path: &str) {
    match expected {
        Value::Object(members) => {
            for (key, want) in members {
                let got = actual.get(key).unwrap_or_else(|| {
                    panic!("case {case:?}: expected member {path}.{key}, got {actual}")
                });
                assert_subset(want, got, case, &format!("{path}.{key}"));
            }
        }
        Value::Array(items) => {
            let got = actual.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected {path} to be a list, got {actual}")
            });
            assert_eq!(
                items.len(),
                got.len(),
                "case {case:?}: expected {path} to hold {} entries, got {actual}",
                items.len()
            );
            for (index, want) in items.iter().enumerate() {
                assert_subset(want, &got[index], case, &format!("{path}[{index}]"));
            }
        }
        leaf => assert_eq!(
            leaf, actual,
            "case {case:?}: expected {path} to be {leaf}, got {actual}"
        ),
    }
}

/// The counter members a tuning job reports while no training job has run: every counter
/// present and zero, because nothing was launched to count.
fn zero_counters() -> Value {
    json!({
        "TrainingJobStatusCounters": {
            "Completed": 0,
            "InProgress": 0,
            "RetryableError": 0,
            "NonRetryableError": 0,
            "Stopped": 0,
        },
        "ObjectiveStatusCounters": { "Succeeded": 0, "Pending": 0, "Failed": 0 },
    })
}

/// RULE: a create stores the tuning job and mints its ARN from the signed region, and
/// `Describe` replays every member the create carried, for every request shape the model
/// allows. The rows differ in which alternative members are used (one definition or a
/// list), the search strategy, the kinds of parameter range, the optional members present
/// and the region the caller signed for.
#[tokio::test]
async fn describe_replays_every_member_the_create_stored() {
    let emulator = Emulator::new();

    let mut rows: Vec<(&str, &str, Value)> = Vec::new();

    rows.push((
        "one definition, integer and continuous ranges, tags",
        "us-east-1",
        valid_body("bayesian-mixed-ranges"),
    ));

    let mut multi = valid_body("two-definitions");
    multi
        .as_object_mut()
        .expect("an object body")
        .remove("TrainingJobDefinition");
    multi["TrainingJobDefinitions"] = json!([
        {
            "DefinitionName": "xgboost",
            "TuningObjective": { "Type": "Maximize", "MetricName": "validation:auc" },
            "HyperParameterRanges": {
                "CategoricalParameterRanges": [{
                    "Name": "booster",
                    "Values": ["gbtree", "dart"],
                }],
            },
            "AlgorithmSpecification": algorithm_specification(),
            "RoleArn": ROLE,
            "OutputDataConfig": { "S3OutputPath": "s3://tuning-output/one/" },
            "StoppingCondition": { "MaxRuntimeInSeconds": 1800 },
        },
        {
            "DefinitionName": "linear",
            "TuningObjective": { "Type": "Maximize", "MetricName": "validation:auc" },
            "AlgorithmSpecification": algorithm_specification(),
            "RoleArn": ROLE,
            "OutputDataConfig": { "S3OutputPath": "s3://tuning-output/two/" },
            "StoppingCondition": { "MaxRuntimeInSeconds": 1800 },
            "EnableNetworkIsolation": true,
        },
    ]);
    rows.push(("a list of two definitions, signed elsewhere", "eu-west-2", multi));

    let mut hyperband = valid_body("hyperband-strategy");
    hyperband["HyperParameterTuningJobConfig"]["Strategy"] = json!("Hyperband");
    hyperband["HyperParameterTuningJobConfig"]["StrategyConfig"] = json!({
        "HyperbandStrategyConfig": { "MinResource": 1, "MaxResource": 10 },
    });
    hyperband["HyperParameterTuningJobConfig"]["RandomSeed"] = json!(42);
    rows.push(("Hyperband with its strategy config", "ap-southeast-2", hyperband));

    let mut grid = valid_body("grid-categorical-only");
    grid["HyperParameterTuningJobConfig"]["Strategy"] = json!("Grid");
    grid["HyperParameterTuningJobConfig"]["ParameterRanges"] = json!({
        "CategoricalParameterRanges": [{ "Name": "booster", "Values": ["gbtree", "dart"] }],
    });
    grid["Autotune"] = json!({ "Mode": "Enabled" });
    rows.push(("Grid over categorical values, with Autotune", "us-west-2", grid));

    let mut minimal = valid_body("minimal-random-search");
    minimal["HyperParameterTuningJobConfig"] = json!({
        "Strategy": "Random",
        "ResourceLimits": { "MaxParallelTrainingJobs": 1 },
    });
    minimal
        .as_object_mut()
        .expect("an object body")
        .remove("Tags");
    rows.push((
        "no optional config members and no tags",
        "us-east-1",
        minimal,
    ));

    for (case, region, body) in rows {
        let name = str_field(&body, "HyperParameterTuningJobName");

        let (status, created) = emulator
            .call_in(region, "CreateHyperParameterTuningJob", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        assert_eq!(
            str_field(&created, "HyperParameterTuningJobArn"),
            expected_arn(region, &name),
            "case {case:?}: the minted ARN must name the signed region and the stored name"
        );

        let (status, described) = emulator
            .call_in(
                region,
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the describe, got {status} with {described}"
        );

        // Every member the create carried comes back unchanged...
        let mut expected = body.clone();
        let members = expected.as_object_mut().expect("an object body");
        // ... except `Tags`, which `DescribeHyperParameterTuningJob` does not report (the
        // model binds no `Tags` member on its output; `ListTags` reports them).
        members.remove("Tags");
        assert_subset(&expected, &described, case, "$");

        assert_subset(
            &json!({
                "HyperParameterTuningJobArn": expected_arn(region, &name),
                "HyperParameterTuningJobStatus": "InProgress",
            }),
            &described,
            case,
            "$",
        );
        assert_subset(&zero_counters(), &described, case, "$");
        assert!(
            described.get("BestTrainingJob").is_none()
                && described.get("OverallBestTrainingJob").is_none(),
            "case {case:?}: no training job ran, so no best training job may be reported, \
             got {described}"
        );
        assert!(
            described.get("FailureReason").is_none(),
            "case {case:?}: a job the clock has not settled yet carries no FailureReason, \
             got {described}"
        );
        assert!(
            described.get("HyperParameterTuningEndTime").is_none(),
            "case {case:?}: a job still InProgress has not ended, got {described}"
        );
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {described}"));
        assert!(
            creation_time > 0.0,
            "case {case:?}: expected a CreationTime after the epoch, got {creation_time}"
        );
    }
}

/// RULE: a tuning job is accepted `InProgress` and reaches its terminal status only once
/// the clock owes that transition; because this emulator launches no training job, the
/// terminal status is `Failed` with a reason that says so — never `Completed`, never a
/// best training job, never an objective metric value for work nothing ran. The settled
/// job is then stable across reads.
#[tokio::test]
async fn a_tuning_job_ends_failed_because_no_training_job_ever_runs() {
    let emulator = Emulator::new();
    let name = "settles-to-failed";
    let created = emulator
        .ok("CreateHyperParameterTuningJob", &valid_body(name))
        .await;
    let arn = str_field(&created, "HyperParameterTuningJobArn");

    let describe = json!({ "HyperParameterTuningJobName": name });

    // A caller acting straight after the create still observes the transient status.
    let before = emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    assert_eq!(
        str_field(&before, "HyperParameterTuningJobStatus"),
        "InProgress",
        "case {name:?}: the span before the terminal status is never zero, got {before}"
    );

    tokio::time::sleep(PAST_SETTLE_DUE).await;

    let settled = emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    assert_eq!(
        str_field(&settled, "HyperParameterTuningJobStatus"),
        "Failed",
        "case {name:?}: a tuning job no training runner serves cannot complete, got {settled}"
    );
    let reason = str_field(&settled, "FailureReason");
    assert!(
        reason.contains("training job"),
        "case {name:?}: the failure reason must say no training job ran, got {reason:?}"
    );
    assert_eq!(
        reason,
        service_com_amazonaws_sagemaker_sagemaker::state::NO_TRAINING_RUNNER_FAILURE_REASON,
        "case {name:?}: the reason is the one the service states once, got {reason:?}"
    );
    assert!(
        settled.get("HyperParameterTuningEndTime").is_some(),
        "case {name:?}: a terminal job has an end time, got {settled}"
    );
    assert_subset(&zero_counters(), &settled, name, "$");
    assert!(
        settled.get("BestTrainingJob").is_none()
            && settled.get("OverallBestTrainingJob").is_none()
            && settled.get("TuningJobCompletionDetails").is_none()
            && settled.get("ConsumedResources").is_none(),
        "case {name:?}: nothing ran, so no best job, completion detail or consumed \
         resource may be reported, got {settled}"
    );
    assert_eq!(
        str_field(&settled, "HyperParameterTuningJobArn"),
        arn,
        "case {name:?}: the ARN is stable across reads, got {settled}"
    );

    // A settled job is not re-settled per call: a second read answers exactly the same.
    let reread = emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    assert_eq!(
        reread, settled,
        "case {name:?}: a settled job must read back identically, got {reread}"
    );
}

/// RULE: a request that satisfies every modelled constraint but breaks a rule the model
/// cannot express — the two training-job-definition members are alternatives, a numeric
/// range carries numbers of its own type with its minimum no greater than its maximum,
/// the searched hyperparameters fit the bound the model states per range list, a strategy
/// rules out the members it is incompatible with, and a warm start names tuning jobs that
/// exist — is refused with a 400 `ValidationException` naming the member, and nothing is
/// persisted.
#[tokio::test]
async fn a_rule_the_model_cannot_express_is_refused_with_a_validation_exception() {
    let emulator = Emulator::new();
    let mut rows: Vec<(&str, Value, &str)> = Vec::new();

    let mut body = valid_body("no-definition");
    body.as_object_mut()
        .expect("an object body")
        .remove("TrainingJobDefinition");
    rows.push((
        "neither TrainingJobDefinition nor TrainingJobDefinitions",
        body,
        "Either TrainingJobDefinition or TrainingJobDefinitions",
    ));

    let mut body = valid_body("both-definitions");
    body["TrainingJobDefinitions"] = json!([training_job_definition("s3://tuning-output/x/")]);
    rows.push((
        "both definition members at once",
        body,
        "Only one of TrainingJobDefinition and TrainingJobDefinitions",
    ));

    let mut body = valid_body("integer-reversed");
    body["HyperParameterTuningJobConfig"]["ParameterRanges"]["IntegerParameterRanges"] = json!([{
        "Name": "max-depth",
        "MinValue": "10",
        "MaxValue": "3",
    }]);
    rows.push((
        "an integer range whose MinValue exceeds its MaxValue",
        body,
        "max-depth",
    ));

    let mut body = valid_body("integer-not-integral");
    body["HyperParameterTuningJobConfig"]["ParameterRanges"]["IntegerParameterRanges"] = json!([{
        "Name": "max-depth",
        "MinValue": "0.5",
        "MaxValue": "ten",
    }]);
    rows.push((
        "an integer range carrying values that are not integers",
        body,
        "integer MinValue and MaxValue",
    ));

    let mut body = valid_body("continuous-reversed");
    body["HyperParameterTuningJobConfig"]["ParameterRanges"]["ContinuousParameterRanges"] =
        json!([{ "Name": "eta", "MinValue": "0.9", "MaxValue": "0.1" }]);
    rows.push((
        "a continuous range whose MinValue exceeds its MaxValue",
        body,
        "eta",
    ));

    let mut body = valid_body("continuous-not-numeric");
    body["HyperParameterTuningJobConfig"]["ParameterRanges"]["ContinuousParameterRanges"] =
        json!([{ "Name": "eta", "MinValue": "low", "MaxValue": "0.1" }]);
    rows.push((
        "a continuous range carrying values that are not numbers",
        body,
        "numeric MinValue and MaxValue",
    ));

    let mut body = valid_body("continuous-infinite");
    body["HyperParameterTuningJobConfig"]["ParameterRanges"]["ContinuousParameterRanges"] =
        json!([{ "Name": "eta", "MinValue": "0.1", "MaxValue": "inf" }]);
    rows.push((
        "a continuous range whose MaxValue is not finite",
        body,
        "finite MinValue and MaxValue",
    ));

    // 16 integer plus 15 continuous ranges: each list is inside the length the model
    // allows, but together they search 31 hyperparameters.
    let mut body = valid_body("too-many-hyperparameters");
    let integers: Vec<Value> = (0..16)
        .map(|index| json!({ "Name": format!("int-{index}"), "MinValue": "1", "MaxValue": "2" }))
        .collect();
    let continuous: Vec<Value> = (0..15)
        .map(|index| {
            json!({ "Name": format!("cont-{index}"), "MinValue": "0.1", "MaxValue": "0.2" })
        })
        .collect();
    body["HyperParameterTuningJobConfig"]["ParameterRanges"] = json!({
        "IntegerParameterRanges": integers,
        "ContinuousParameterRanges": continuous,
    });
    rows.push((
        "more searched hyperparameters than the bound the model states",
        body,
        "at most 30 hyperparameters",
    ));

    let mut body = valid_body("grid-with-numeric");
    body["HyperParameterTuningJobConfig"]["Strategy"] = json!("Grid");
    rows.push((
        "the Grid strategy with a numeric parameter range",
        body,
        "Grid strategy searches categorical hyperparameters only",
    ));

    let mut body = valid_body("hyperband-early-stopping");
    body["HyperParameterTuningJobConfig"]["Strategy"] = json!("Hyperband");
    body["HyperParameterTuningJobConfig"]["TrainingJobEarlyStoppingType"] = json!("Auto");
    rows.push((
        "early stopping on with the Hyperband strategy",
        body,
        "TrainingJobEarlyStoppingType must be Off",
    ));

    let mut body = valid_body("hyperband-reversed");
    body["HyperParameterTuningJobConfig"]["Strategy"] = json!("Hyperband");
    body["HyperParameterTuningJobConfig"]["StrategyConfig"] =
        json!({ "HyperbandStrategyConfig": { "MinResource": 9, "MaxResource": 3 } });
    rows.push((
        "a Hyperband config whose MinResource exceeds its MaxResource",
        body,
        "HyperbandStrategyConfig",
    ));

    let mut body = valid_body("parallel-over-total");
    body["HyperParameterTuningJobConfig"]["ResourceLimits"] = json!({
        "MaxNumberOfTrainingJobs": 2,
        "MaxParallelTrainingJobs": 5,
    });
    rows.push((
        "more parallel training jobs than the total it may launch",
        body,
        "MaxParallelTrainingJobs",
    ));

    let mut body = valid_body("wait-under-runtime");
    body["TrainingJobDefinition"]["StoppingCondition"] =
        json!({ "MaxRuntimeInSeconds": 3600, "MaxWaitTimeInSeconds": 600 });
    rows.push((
        "a managed-spot wait shorter than the runtime it covers",
        body,
        "MaxWaitTimeInSeconds",
    ));

    let mut body = valid_body("both-resource-configs");
    body["TrainingJobDefinition"]["HyperParameterTuningResourceConfig"] =
        json!({ "InstanceType": "ml.m5.large", "InstanceCount": 1 });
    rows.push((
        "both a ResourceConfig and a HyperParameterTuningResourceConfig",
        body,
        "HyperParameterTuningResourceConfig",
    ));

    let mut body = valid_body("warm-start-no-parent");
    body["WarmStartConfig"] = json!({
        "WarmStartType": "IdenticalDataAndAlgorithm",
        "ParentHyperParameterTuningJobs": [{
            "HyperParameterTuningJobName": "no-such-parent-job",
        }],
    });
    rows.push((
        "a warm start from a tuning job that does not exist",
        body,
        "no-such-parent-job",
    ));

    for (case, body, fragment) in rows {
        let name = str_field(&body, "HyperParameterTuningJobName");
        let (status, refused) = emulator
            .call("CreateHyperParameterTuningJob", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        let text = message(&refused);
        assert!(
            text.contains(fragment),
            "case {case:?}: expected the message to name {fragment:?}, got {text:?}"
        );

        // A refused create persists nothing.
        let (status, described) = emulator
            .call(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no job, got {status} with {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFound",
            "case {case:?}: a refused create stores no job, got {described}"
        );
    }
}

/// RULE: the capacity a tuning job asks for is held to the published per-tuning-job
/// quotas — a request AT a quota is accepted, a request past it is refused with the
/// modelled `ResourceLimitExceeded` naming the limit. Each quota is tested on its own
/// bound and one past it.
///
/// The last row also pins the PRECEDENCE: a request that is both past a published quota
/// and breaks a cross-member rule (more parallel training jobs than it may launch in
/// total) is answered as the limit failure, because the capacity it asks for is
/// unavailable whatever else its numbers say.
#[tokio::test]
async fn capacity_past_a_published_quota_is_refused_with_resource_limit_exceeded() {
    let emulator = Emulator::new();

    // (case, MaxNumberOfTrainingJobs, MaxParallelTrainingJobs, accepted)
    let rows: Vec<(&str, i64, i64, bool)> = vec![
        ("one training job, one at a time", 1, 1, true),
        (
            "at both quotas",
            MAX_TRAINING_JOBS,
            MAX_PARALLEL_TRAINING_JOBS,
            true,
        ),
        (
            "one training job past the total quota",
            MAX_TRAINING_JOBS + 1,
            MAX_PARALLEL_TRAINING_JOBS,
            false,
        ),
        (
            "one job past the parallel quota",
            MAX_TRAINING_JOBS,
            MAX_PARALLEL_TRAINING_JOBS + 1,
            false,
        ),
        ("far past the total quota", 1_000_000, 1, false),
        // Past the parallel quota AND past its own total, so this row pins which of the
        // two refusals wins: the limit failure (see the handler's note on the order).
        (
            "far past the parallel quota, with a smaller total",
            2,
            1_000,
            false,
        ),
    ];

    for (index, (case, total, parallel, accepted)) in rows.into_iter().enumerate() {
        let name = format!("quota-case-{index}");
        let mut body = valid_body(&name);
        body["HyperParameterTuningJobConfig"]["ResourceLimits"] = json!({
            "MaxNumberOfTrainingJobs": total,
            "MaxParallelTrainingJobs": parallel,
        });

        let (status, answer) = emulator
            .call("CreateHyperParameterTuningJob", &body)
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a request at the quota is accepted, got {status} with {answer}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceLimitExceeded",
            "case {case:?}: a request past a quota is the modelled limit failure, got {answer}"
        );
        let text = message(&answer);
        assert!(
            text.contains("service limit"),
            "case {case:?}: expected the message to name the limit, got {text:?}"
        );

        let (status, described) = emulator
            .call(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no job, got {status} with {described}"
        );
    }
}

/// RULE: a tuning job name is unique within an account and Region, so a second create
/// under a name already taken is the modelled `ResourceInUse` and leaves the stored job
/// exactly as the first create wrote it.
#[tokio::test]
async fn a_name_already_in_use_is_refused_and_keeps_the_stored_job() {
    let emulator = Emulator::new();
    let name = "duplicate-tuning-job";
    emulator
        .ok("CreateHyperParameterTuningJob", &valid_body(name))
        .await;
    let first = emulator
        .ok(
            "DescribeHyperParameterTuningJob",
            &json!({ "HyperParameterTuningJobName": name }),
        )
        .await;

    // A second create differing in its search, so a store that overwrote the record
    // would read back differently.
    let mut second = valid_body(name);
    second["HyperParameterTuningJobConfig"]["Strategy"] = json!("Random");
    let (status, refused) = emulator
        .call("CreateHyperParameterTuningJob", &second)
        .await;
    assert_eq!(
        status, 400,
        "case {name:?}: expected HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceInUse",
        "case {name:?}: a name in use is the modelled conflict, got {refused}"
    );

    let after = emulator
        .ok(
            "DescribeHyperParameterTuningJob",
            &json!({ "HyperParameterTuningJobName": name }),
        )
        .await;
    assert_eq!(
        after, first,
        "case {name:?}: the refused create must leave the stored job untouched, got {after}"
    );
}

/// RULE: a warm start whose parents this service holds is accepted, for every modelled
/// `WarmStartType`, and the configuration is replayed as stored.
#[tokio::test]
async fn a_warm_start_from_stored_parents_is_accepted_and_replayed() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateHyperParameterTuningJob", &valid_body("warm-parent-a"))
        .await;
    emulator
        .ok("CreateHyperParameterTuningJob", &valid_body("warm-parent-b"))
        .await;

    // Every member of `HyperParameterTuningJobWarmStartType`.
    for (index, warm_start_type) in ["IdenticalDataAndAlgorithm", "TransferLearning"]
        .into_iter()
        .enumerate()
    {
        let name = format!("warm-child-{index}");
        let mut body = valid_body(&name);
        let warm_start = json!({
            "WarmStartType": warm_start_type,
            "ParentHyperParameterTuningJobs": [
                { "HyperParameterTuningJobName": "warm-parent-a" },
                { "HyperParameterTuningJobName": "warm-parent-b" },
            ],
        });
        body["WarmStartConfig"] = warm_start.clone();

        let (status, created) = emulator
            .call("CreateHyperParameterTuningJob", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {warm_start_type:?}: expected HTTP 200, got {status} with {created}"
        );

        let described = emulator
            .ok(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": name }),
            )
            .await;
        assert_subset(
            &json!({ "WarmStartConfig": warm_start }),
            &described,
            warm_start_type,
            "$",
        );
    }
}

/// RULE: the searched-hyperparameter bound applies to what ONE training job definition
/// searches (the tuning job config's ranges plus that definition's own), so a
/// multi-algorithm request whose definitions each stay inside the bound is accepted even
/// though their ranges together exceed it, while one definition past the bound is refused.
#[tokio::test]
async fn the_searched_hyperparameter_bound_is_per_training_job_definition() {
    let emulator = Emulator::new();

    // `ParameterRanges`' integer list is bounded at 30 by the model, and the service
    // bound on the hyperparameters searched is the same figure.
    let ranges = |count: usize, prefix: &str| -> Value {
        let integers: Vec<Value> = (0..count)
            .map(|index| {
                json!({ "Name": format!("{prefix}-{index}"), "MinValue": "1", "MaxValue": "2" })
            })
            .collect();
        json!({ "IntegerParameterRanges": integers })
    };

    // Two definitions searching 16 hyperparameters each: 32 together, 16 apiece.
    let mut body = valid_body("per-definition-bound");
    body.as_object_mut()
        .expect("an object body")
        .remove("TrainingJobDefinition");
    body["HyperParameterTuningJobConfig"]
        .as_object_mut()
        .expect("an object config")
        .remove("ParameterRanges");
    let mut first = training_job_definition("s3://tuning-output/one/");
    first["DefinitionName"] = json!("first");
    first["HyperParameterRanges"] = ranges(16, "a");
    let mut second = training_job_definition("s3://tuning-output/two/");
    second["DefinitionName"] = json!("second");
    second["HyperParameterRanges"] = ranges(16, "b");
    body["TrainingJobDefinitions"] = json!([first, second]);

    let (status, answer) = emulator
        .call("CreateHyperParameterTuningJob", &body)
        .await;
    assert_eq!(
        status, 200,
        "case two definitions of 16 ranges each: each stays inside the bound, got {status} \
         with {answer}"
    );

    // One definition searching 31 is past the bound, whichever member carries the ranges.
    let mut body = valid_body("one-definition-over");
    body["HyperParameterTuningJobConfig"]
        .as_object_mut()
        .expect("an object config")
        .remove("ParameterRanges");
    body["TrainingJobDefinition"]["HyperParameterRanges"] = ranges(31, "c");
    let (status, refused) = emulator
        .call("CreateHyperParameterTuningJob", &body)
        .await;
    assert_eq!(
        status, 400,
        "case one definition of 31 ranges: past the bound, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case one definition of 31 ranges: expected a ValidationException, got {refused}"
    );
}

/// RULE: a value outside a constraint the model states on a NESTED member is refused
/// with HTTP 400 just as a top-level one is, and persists nothing. The rows sit one past
/// each published bound inside `HyperParameterTuningJobConfig`.
#[tokio::test]
async fn values_outside_a_nested_modelled_constraint_are_refused() {
    let emulator = Emulator::new();

    // `ResourceLimits$MaxNumberOfTrainingJobs` range 1.., `$MaxParallelTrainingJobs`
    // range 1.., `$MaxRuntimeInSeconds` range 120..15768000, `RandomSeed` range 0..,
    // `IntegerParameterRanges` len 0..30.
    let thirty_one: Vec<Value> = (0..31)
        .map(|index| json!({ "Name": format!("int-{index}"), "MinValue": "1", "MaxValue": "2" }))
        .collect();
    let rows: Vec<(&str, Value)> = vec![
        ("MaxNumberOfTrainingJobs below 1", json!(0)),
        ("MaxParallelTrainingJobs below 1", json!(0)),
        ("MaxRuntimeInSeconds below 120", json!(119)),
        ("MaxRuntimeInSeconds above 15768000", json!(15_768_001)),
        ("RandomSeed below 0", json!(-1)),
        ("thirty-one integer ranges in one list", json!(0)),
    ];

    for (index, (case, value)) in rows.into_iter().enumerate() {
        let name = format!("nested-constraint-{index}");
        let mut body = valid_body(&name);
        match case {
            "MaxNumberOfTrainingJobs below 1" => {
                body["HyperParameterTuningJobConfig"]["ResourceLimits"]
                    ["MaxNumberOfTrainingJobs"] = value;
            }
            "MaxParallelTrainingJobs below 1" => {
                body["HyperParameterTuningJobConfig"]["ResourceLimits"]
                    ["MaxParallelTrainingJobs"] = value;
            }
            "RandomSeed below 0" => {
                body["HyperParameterTuningJobConfig"]["RandomSeed"] = value;
            }
            "thirty-one integer ranges in one list" => {
                body["HyperParameterTuningJobConfig"]["ParameterRanges"] =
                    json!({ "IntegerParameterRanges": thirty_one.clone() });
            }
            _ => {
                body["HyperParameterTuningJobConfig"]["ResourceLimits"]["MaxRuntimeInSeconds"] =
                    value;
            }
        }

        let (status, answer) = emulator
            .call("CreateHyperParameterTuningJob", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the modelled constraint must be refused, got \
             {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer}"
        );

        let (status, described) = emulator
            .call(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no job, got {status} with {described}"
        );
    }
}

/// RULE: `HyperParameterTuningJobName`'s doc states the name "is not case sensitive", so
/// names differing only in case are ONE job: the create refuses the second as a conflict
/// and every read resolves to the first, which keeps the casing the caller gave it.
#[tokio::test]
async fn the_tuning_job_name_identifies_one_job_whatever_its_case() {
    let emulator = Emulator::new();
    let stored = "Mixed-Case-Job";
    let created = emulator
        .ok("CreateHyperParameterTuningJob", &valid_body(stored))
        .await;
    let arn = str_field(&created, "HyperParameterTuningJobArn");
    assert_eq!(
        arn,
        expected_arn("us-east-1", stored),
        "case {stored:?}: the ARN keeps the casing the caller gave, got {created}"
    );

    for case in ["Mixed-Case-Job", "mixed-case-job", "MIXED-CASE-JOB"] {
        let described = emulator
            .ok(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": case }),
            )
            .await;
        assert_eq!(
            str_field(&described, "HyperParameterTuningJobName"),
            stored,
            "case {case:?}: the stored name is reported as written, got {described}"
        );
        assert_eq!(
            str_field(&described, "HyperParameterTuningJobArn"),
            arn,
            "case {case:?}: every casing names the one stored job, got {described}"
        );

        let (status, refused) = emulator
            .call("CreateHyperParameterTuningJob", &valid_body(case))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the name is already in use, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case:?}: a name differing only in case is the same name, got {refused}"
        );
    }
}

/// RULE: a name no create stored is the modelled `ResourceNotFound`, never a fabricated
/// job — whatever shape the name has.
#[tokio::test]
async fn describing_a_tuning_job_that_was_never_created_is_a_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateHyperParameterTuningJob", &valid_body("stored-job"))
        .await;

    // Names the model's pattern accepts, none of them stored: a near miss on the stored
    // name, the shortest and the longest the model allows.
    let cases = ["stored-jobs", "a", "a234567890123456789012345678901b"];
    for case in cases {
        let (status, answer) = emulator
            .call(
                "DescribeHyperParameterTuningJob",
                &json!({ "HyperParameterTuningJobName": case }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFound",
            "case {case:?}: an unknown job is the modelled not-found, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// The audit record a state-changing call leaves.
//
// `state::record_state_change` emits a structured `tracing` event, read back through the
// ONE capture this test binary installs (`hyperpod_cluster_audit_record::audit_capture`).
// Every lookup filters on the ARN of the job THIS suite created, because the capture
// serves every suite in the binary and they run concurrently. Nothing is written to disk.
// ---------------------------------------------------------------------------

/// RULE: a call that CHANGES this service's state leaves exactly one structured audit
/// record naming the operation, the ARN of the resource it changed and the access key id
/// the REQUEST presented — including the status transition a reporting call materialises.
/// A call that changes nothing (a refused create, a describe that settles nothing) leaves
/// none, so the record means a change really happened.
#[tokio::test]
async fn a_state_changing_call_leaves_one_audit_record() {
    let capture = audit_capture();
    let emulator = Emulator::new();
    let name = "audited-tuning-job";

    let created = emulator
        .ok("CreateHyperParameterTuningJob", &valid_body(name))
        .await;
    let arn = str_field(&created, "HyperParameterTuningJobArn");
    // The capture matches on the resource, and it takes the set of resources the calling
    // test owns; this suite owns exactly the one job it created.
    let own = [arn.clone()];
    let records = capture.records_for("CreateHyperParameterTuningJob", &own);
    assert_eq!(
        records.len(),
        1,
        "case {name:?}: the create must leave one audit record, captured {records:?}"
    );
    // The harness signs with `Credential=AKIDEXAMPLE/...`, so that is the id the record
    // attributes the call to — never the emulator's own credentials.
    assert_eq!(
        records[0].field("access_key_id"),
        Some("AKIDEXAMPLE"),
        "case {name:?}: the record attributes the call to the presented access key id, \
         got {records:?}"
    );

    // A refused create changes nothing, so it accounts for nothing.
    let (status, _) = emulator
        .call("CreateHyperParameterTuningJob", &valid_body(name))
        .await;
    assert_eq!(status, 400, "case {name:?}: the duplicate create is refused");
    let records = capture.records_for("CreateHyperParameterTuningJob", &own);
    assert_eq!(
        records.len(),
        1,
        "case {name:?}: a refused create leaves no further record, captured {records:?}"
    );

    // A describe that settles nothing is a pure read.
    let describe = json!({ "HyperParameterTuningJobName": name });
    emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    let reads = capture.records_for("DescribeHyperParameterTuningJob", &own);
    assert!(
        reads.is_empty(),
        "case {name:?}: a describe that settles nothing changes nothing, captured {reads:?}"
    );

    // The transition the clock owed IS a change, so it leaves the same record — once.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    emulator
        .ok("DescribeHyperParameterTuningJob", &describe)
        .await;
    let settles = capture.records_for("DescribeHyperParameterTuningJob", &own);
    assert_eq!(
        settles.len(),
        1,
        "case {name:?}: the settled transition is recorded exactly once, captured {settles:?}"
    );
}
