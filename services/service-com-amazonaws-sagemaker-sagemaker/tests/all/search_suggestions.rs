// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `GetSearchSuggestions`, the autocomplete behind the
//! console's search box.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, not a second copy of it.
//!
//! The RULE under test: for the requested `ResourceType`, the operation answers the
//! INDEXED property names of that resource that the `PropertyNameHint` begins — the whole
//! name or one of its dot-separated segments ("Text that begins a property's name") — and
//! nothing else. A resource's indexed property names are, for every key the account's
//! stored resources of that type carry in a KEYED member of that resource's searchable
//! entity, the dotted path that key sits at with the containing member LOWERCASED
//! (`tags.<key>`, `hyperparameters.<key>`, `environment.<key>`, `parameters.<key>`) — the
//! operation's own doc: "Provides suggestions for HyperParameters, Tags, and Metrics" —
//! and the caller's key verbatim. A key is never offered
//! bare, and a FIXED member of the entity (`ExperimentName`, `TrainingJobStatus`, …) is
//! never offered at all: it is schema, not an indexed property. The tests assert that rule
//! over a TABLE of resource types, hints and stored keys, and sweep every property name of
//! every `ResourceType` value for the prefix invariant rather than trusting one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{one_variant_config, primary_container, Emulator};
use crate::training_job_admission::{image_specification, training_body};

/// The facet segment the keys of a training job's `HyperParameters` map are answered
/// under: LOWERCASE, which is the form SageMaker answers (and `Search`'s `Filter.Name`
/// accepts) for that facet, whatever case the modelled member name carries.
const HYPER_PARAMETERS_FACET: &str = "hyperparameters";

/// The most property names one answer carries, as the handler documents it. Read from the
/// crate rather than copied, so the test tracks the bound the operation enforces.
const CAP: usize =
    service_com_amazonaws_sagemaker_sagemaker::ops::get_search_suggestions::MAX_PROPERTY_NAME_SUGGESTIONS;

/// Every value of the modelled `ResourceType` enum
/// (smithy_query query="shape" shape="ResourceType"), which is the set of resources this
/// operation can be asked about.
const RESOURCE_TYPES: [&str; 20] = [
    "TrainingJob",
    "Experiment",
    "ExperimentTrial",
    "ExperimentTrialComponent",
    "Endpoint",
    "Model",
    "ModelPackage",
    "ModelPackageGroup",
    "Pipeline",
    "PipelineExecution",
    "FeatureGroup",
    "FeatureMetadata",
    "Image",
    "ImageVersion",
    "Project",
    "HyperParameterTuningJob",
    "ModelCard",
    "PipelineVersion",
    "Job",
    "HubContent",
];

/// Every caller-chosen key [`seeded`] stores, spelled bare. None of them is a property
/// name a `Search` filter accepts — `Filter`'s `Name` takes the path the key sits at
/// (`tags.Project`, not `Project`) — so none may be suggested on its own.
const BARE_KEYS: [&str; 19] = [
    "momentum",
    "VERBOSITY",
    "unit",
    "learning_rate",
    "batch_size",
    "optimiser",
    "größe",
    "dataset",
    "metrics_file",
    "Project",
    "team",
    "dept",
    "tier",
    "env",
    "owner",
    "squad",
    "domain",
    "phase",
    "cohort",
];

/// One FIXED member of a resource's searchable entity, per resource type the model binds
/// one to: the resource type and the member's name as the model spells it
/// (smithy_query query="shape" shape="<entity>"). None of them is an indexed property, so
/// none may be suggested — not even when the hint is the member's own name.
///
/// The rows cover every kind of fixed member the entity declares: a scalar, a list, a
/// nested structure, and the two keyed members whose ENTRIES are indexed (`Tags`,
/// `HyperParameters`) — the container itself is still schema.
const FIXED_MEMBERS: [(&str, &str); 23] = [
    ("TrainingJob", "TrainingJobName"),
    ("TrainingJob", "TrainingJobStatus"),
    ("TrainingJob", "HyperParameters"),
    ("TrainingJob", "Environment"),
    ("Experiment", "ExperimentName"),
    ("Experiment", "DisplayName"),
    ("Experiment", "Tags"),
    ("ExperimentTrial", "TrialName"),
    ("ExperimentTrialComponent", "TrialComponentName"),
    ("ExperimentTrialComponent", "Parameters"),
    ("Endpoint", "EndpointName"),
    ("Endpoint", "CreationTime"),
    ("Model", "MonitoringSchedules"),
    ("ModelPackage", "CustomerMetadataProperties"),
    ("ModelPackageGroup", "ModelPackageGroupStatus"),
    ("Pipeline", "PipelineDisplayName"),
    ("PipelineExecution", "PipelineExecutionStatus"),
    ("PipelineVersion", "PipelineVersionId"),
    ("FeatureGroup", "FeatureDefinitions"),
    ("FeatureMetadata", "FeatureType"),
    ("Project", "ProjectId"),
    ("HyperParameterTuningJob", "BestTrainingJob"),
    ("HubContent", "HubContentSearchKeywords"),
];

/// A `CreateHyperParameterTuningJob` request body this suite's seed is accepted with: the
/// members the operation requires (a strategy, an objective, resource limits, parameter
/// ranges and exactly one training job definition), carrying this suite's own tag key.
fn tuning_job(name: &str) -> Value {
    json!({
        "HyperParameterTuningJobName": name,
        "HyperParameterTuningJobConfig": {
            "Strategy": "Bayesian",
            "HyperParameterTuningJobObjective": {
                "Type": "Minimize",
                "MetricName": "validation:rmse",
            },
            "ResourceLimits": {
                "MaxNumberOfTrainingJobs": 2,
                "MaxParallelTrainingJobs": 1,
            },
            "ParameterRanges": {
                "IntegerParameterRanges": [{
                    "Name": "max_depth",
                    "MinValue": "3",
                    "MaxValue": "10",
                    "ScalingType": "Auto",
                }],
            },
            "TrainingJobEarlyStoppingType": "Off",
        },
        "TrainingJobDefinition": {
            "AlgorithmSpecification": {
                "TrainingImage": "123456789012.dkr.ecr.us-east-1.amazonaws.com/xgboost:1",
                "TrainingInputMode": "File",
                "MetricDefinitions": [
                    { "Name": "validation:rmse", "Regex": "rmse: ([0-9.]+)" },
                ],
            },
            "RoleArn": "arn:aws:iam::123456789012:role/SageMakerTuningRole",
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
            "OutputDataConfig": { "S3OutputPath": "s3://tuning-output/artifacts/" },
            "ResourceConfig": {
                "InstanceType": "ml.m5.large",
                "InstanceCount": 1,
                "VolumeSizeInGB": 10,
            },
            "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
            "StaticHyperParameters": { "objective": "reg:squarederror" },
        },
        "Tags": [{ "Key": "cohort", "Value": "beta" }],
    })
}

/// The suggestions `GetSearchSuggestions` answers for `resource`, with `hint` when one is
/// given and with no `SuggestionQuery` at all when it is `None`.
async fn suggestions(emulator: &Emulator, resource: &str, hint: Option<&str>) -> Vec<String> {
    let mut body = json!({ "Resource": resource });
    if let Some(hint) = hint {
        body["SuggestionQuery"] = json!({ "PropertyNameQuery": { "PropertyNameHint": hint } });
    }
    let answered = emulator.ok("GetSearchSuggestions", &body).await;
    let listed = answered
        .get("PropertyNameSuggestions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {resource}/{hint:?}: expected a PropertyNameSuggestions list, \
                 got {answered}"
            )
        });
    listed
        .iter()
        .map(|suggestion| {
            suggestion
                .get("PropertyName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {resource}/{hint:?}: a suggestion without a PropertyName: {suggestion}")
                })
                .to_string()
        })
        .collect()
}

/// Seed one resource in each store this emulator keeps for a searchable resource type,
/// each carrying caller-chosen keys: a trial component with parameters, artifacts and
/// tags, a tagged model, a tagged endpoint (through its configuration), a tagged model
/// package group, a tagged experiment, a tagged trial, a tagged hyperparameter tuning job,
/// a tagged feature group and a training job carrying a hyperparameter, an environment
/// variable and a tag.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();

    // A training job, whose entity declares three members whose entries the CALLER names:
    // `HyperParameters`, `Environment` and `Tags`.
    let mut job = training_body(
        "suggestions-training-job",
        image_specification(),
        "s3://demo/output/",
    );
    job["HyperParameters"] = json!({ "momentum": "0.9" });
    job["Environment"] = json!({ "VERBOSITY": "2" });
    job["Tags"] = json!([{ "Key": "unit", "Value": "integration" }]);
    emulator.ok("CreateTrainingJob", &job).await;

    emulator
        .ok(
            "CreateTrialComponent",
            &json!({
                "TrialComponentName": "suggestions-component",
                "Parameters": {
                    "learning_rate": { "NumberValue": 0.01 },
                    "batch_size": { "NumberValue": 64.0 },
                    "optimiser": { "StringValue": "adamw" },
                    // A non-ASCII key: the match folds ASCII only, so this one is
                    // matched byte-exactly.
                    "größe": { "NumberValue": 7.0 },
                },
                "InputArtifacts": {
                    "dataset": { "Value": "s3://bucket/train.csv" },
                },
                "OutputArtifacts": {
                    "metrics_file": { "Value": "s3://bucket/metrics.json" },
                },
                "Tags": [
                    { "Key": "Project", "Value": "forecasting" },
                    { "Key": "team", "Value": "research" },
                ],
            }),
        )
        .await;

    emulator
        .ok(
            "CreateModel",
            &json!({
                "ModelName": "suggestions-model",
                "PrimaryContainer": primary_container("demo:1"),
                "Tags": [{ "Key": "dept", "Value": "science" }],
            }),
        )
        .await;

    let mut config = one_variant_config("suggestions-config", "suggestions-model");
    config["Tags"] = json!([{ "Key": "tier", "Value": "gold" }]);
    emulator.ok("CreateEndpointConfig", &config).await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({
                "EndpointName": "suggestions-endpoint",
                "EndpointConfigName": "suggestions-config",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        )
        .await;

    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({
                "ModelPackageGroupName": "suggestions-group",
                "Tags": [{ "Key": "owner", "Value": "mlops" }],
            }),
        )
        .await;

    emulator
        .ok(
            "CreateExperiment",
            &json!({
                "ExperimentName": "suggestions-experiment",
                "Tags": [{ "Key": "squad", "Value": "forecasting" }],
            }),
        )
        .await;

    // A trial, whose searchable entity (`SearchRecord`'s `Trial`) declares `Tags`. It
    // belongs to the experiment created just above, which `CreateTrial` resolves.
    emulator
        .ok(
            "CreateTrial",
            &json!({
                "TrialName": "suggestions-trial",
                "ExperimentName": "suggestions-experiment",
                "Tags": [{ "Key": "phase", "Value": "pilot" }],
            }),
        )
        .await;

    // A hyperparameter tuning job, whose searchable entity
    // (`HyperParameterTuningJobSearchEntity`) declares `Tags` too.
    emulator
        .ok("CreateHyperParameterTuningJob", &tuning_job("suggestions-tuning-job"))
        .await;

    // `CreateFeatureGroup`'s required members, as `feature_group_lifecycle` sends them.
    emulator
        .ok(
            "CreateFeatureGroup",
            &json!({
                "FeatureGroupName": "suggestions-feature-group",
                "RecordIdentifierFeatureName": "customer-id",
                "EventTimeFeatureName": "event-time",
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                ],
                "OnlineStoreConfig": { "EnableOnlineStore": true },
                "Tags": [{ "Key": "domain", "Value": "retail" }],
            }),
        )
        .await;

    emulator
}

/// Every indexed property name [`seeded`] stores, per resource type: the facet path
/// LOWERCASED with the key in the case it was sent. A resource type the seed stores no
/// caller-named key for has none at all.
fn seeded_property_names(resource: &str) -> BTreeSet<String> {
    let names: &[&str] = match resource {
        "TrainingJob" => &[
            "hyperparameters.momentum",
            "environment.VERBOSITY",
            "tags.unit",
        ],
        "Experiment" => &["tags.squad"],
        "ExperimentTrial" => &["tags.phase"],
        "ExperimentTrialComponent" => &[
            "parameters.learning_rate",
            "parameters.batch_size",
            "parameters.optimiser",
            "parameters.größe",
            "inputartifacts.dataset",
            "outputartifacts.metrics_file",
            "tags.Project",
            "tags.team",
        ],
        "Endpoint" => &["tags.env"],
        // `SearchRecord`'s `Model` member targets `ModelDashboardModel`, which carries the
        // model itself under its own `Model` member — so a model's tags sit one level
        // deeper, and the whole path is lowercased.
        "Model" => &["model.tags.dept"],
        "ModelPackageGroup" => &["tags.owner"],
        "HyperParameterTuningJob" => &["tags.cohort"],
        "FeatureGroup" => &["tags.domain"],
        _ => &[],
    };
    names.iter().map(|name| (*name).to_string()).collect()
}

/// RULE: a key a stored resource carries in a keyed member of its searchable entity is an
/// indexed property name of that resource at the dotted PATH it sits at, with the facet
/// LOWERCASED and the key in the case it was stored — never bare, since the path is the
/// form `Filter`'s `Name` accepts — for the resource type that holds it and for no other.
#[tokio::test]
async fn keys_stored_on_a_resource_are_suggested_at_the_lowercased_path_they_sit_at() {
    let emulator = seeded().await;

    // Rows differ along what the rule generalises over: which keyed member the key sits
    // in (`Tags`, `HyperParameters`, `Parameters`, `InputArtifacts`, `OutputArtifacts`),
    // the depth of the path (`tags` on the entity itself versus `model.tags`), the hint's
    // case, the hint's shape (a key, a dotted path prefix, a whole facet) and the key's
    // character set (ASCII, non-ASCII). The answer is the lowercased PATH in every row,
    // whether the hint begins the path or the key.
    let cases: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "hyperparameter-key",
            "TrainingJob",
            "moment",
            &["hyperparameters.momentum"],
        ),
        (
            "hyperparameter-lowercase-facet-path",
            "TrainingJob",
            "hyperparameters.m",
            &["hyperparameters.momentum"],
        ),
        (
            "facet-hint-in-model-casing",
            "TrainingJob",
            "HyperParameters",
            &["hyperparameters.momentum"],
        ),
        (
            "environment-key",
            "TrainingJob",
            "VERBOS",
            &["environment.VERBOSITY"],
        ),
        (
            "training-job-tag-key",
            "TrainingJob",
            "unit",
            &["tags.unit"],
        ),
        (
            "parameter-key",
            "ExperimentTrialComponent",
            "learning",
            &["parameters.learning_rate"],
        ),
        (
            "parameter-path",
            "ExperimentTrialComponent",
            "parameters.learn",
            &["parameters.learning_rate"],
        ),
        (
            "second-parameter-key",
            "ExperimentTrialComponent",
            "batch",
            &["parameters.batch_size"],
        ),
        (
            "parameter-key-case-folded",
            "ExperimentTrialComponent",
            "OPTIMIS",
            &["parameters.optimiser"],
        ),
        (
            "non-ascii-parameter-key",
            "ExperimentTrialComponent",
            "größ",
            &["parameters.größe"],
        ),
        (
            "input-artifact-key",
            "ExperimentTrialComponent",
            "datas",
            &["inputartifacts.dataset"],
        ),
        (
            "input-artifact-path",
            "ExperimentTrialComponent",
            "InputArtifacts.data",
            &["inputartifacts.dataset"],
        ),
        (
            "output-artifact-key",
            "ExperimentTrialComponent",
            "metrics_",
            &["outputartifacts.metrics_file"],
        ),
        (
            "tag-key",
            "ExperimentTrialComponent",
            "Project",
            &["tags.Project"],
        ),
        (
            "tag-path-prefix",
            "ExperimentTrialComponent",
            "tags.Pro",
            &["tags.Project"],
        ),
        (
            "tag-key-case-folded",
            "ExperimentTrialComponent",
            "TEAM",
            &["tags.team"],
        ),
        (
            "facet-hint-answers-every-key-under-it",
            "ExperimentTrialComponent",
            "Tags",
            &["tags.Project", "tags.team"],
        ),
        ("model-tag-key", "Model", "dept", &["model.tags.dept"]),
        (
            "model-nested-tag-path",
            "Model",
            "Model.Tags",
            &["model.tags.dept"],
        ),
        ("endpoint-tag-key", "Endpoint", "env", &["tags.env"]),
        ("endpoint-tag-path", "Endpoint", "tags.e", &["tags.env"]),
        (
            "model-package-group-tag-key",
            "ModelPackageGroup",
            "owner",
            &["tags.owner"],
        ),
        ("experiment-tag-key", "Experiment", "squad", &["tags.squad"]),
        (
            "experiment-tag-path",
            "Experiment",
            "tags.s",
            &["tags.squad"],
        ),
        ("trial-tag-key", "ExperimentTrial", "phase", &["tags.phase"]),
        (
            "trial-tag-path",
            "ExperimentTrial",
            "Tags.ph",
            &["tags.phase"],
        ),
        (
            "tuning-job-tag-key",
            "HyperParameterTuningJob",
            "cohort",
            &["tags.cohort"],
        ),
        (
            "tuning-job-tag-path",
            "HyperParameterTuningJob",
            "tags.c",
            &["tags.cohort"],
        ),
        (
            "feature-group-tag-key",
            "FeatureGroup",
            "domain",
            &["tags.domain"],
        ),
        (
            "feature-group-tag-path",
            "FeatureGroup",
            "tags.d",
            &["tags.domain"],
        ),
    ];

    for (case, resource, hint, expected) in &cases {
        let answered = suggestions(&emulator, resource, Some(hint)).await;
        for name in *expected {
            assert!(
                answered.iter().any(|suggested| suggested == name),
                "case {case}: expected {name} among the suggestions for {resource} \
                 hinted {hint:?}, got {answered:?}"
            );
        }
        // A suggestion is "the property name to use in Search queries", so a key is only
        // ever offered at the path it sits at — never bare.
        for suggested in &answered {
            assert!(
                !BARE_KEYS.contains(&suggested.as_str()),
                "case {case}: {suggested:?} is a bare key, not a Search property name, \
                 got {answered:?}"
            );
        }
    }

    // A key belongs to the resource that stores it: no OTHER resource type suggests it.
    // The hint is the name itself, which begins it.
    let stored: [(&str, &str); 13] = [
        ("TrainingJob", "hyperparameters.momentum"),
        ("TrainingJob", "environment.VERBOSITY"),
        ("TrainingJob", "tags.unit"),
        ("ExperimentTrialComponent", "parameters.learning_rate"),
        ("ExperimentTrialComponent", "inputartifacts.dataset"),
        ("ExperimentTrialComponent", "tags.Project"),
        ("Model", "model.tags.dept"),
        ("Endpoint", "tags.env"),
        ("ModelPackageGroup", "tags.owner"),
        ("Experiment", "tags.squad"),
        ("ExperimentTrial", "tags.phase"),
        ("HyperParameterTuningJob", "tags.cohort"),
        ("FeatureGroup", "tags.domain"),
    ];
    for (owner, name) in stored {
        for other in RESOURCE_TYPES.iter().filter(|other| **other != owner) {
            let elsewhere = suggestions(&emulator, other, Some(name)).await;
            assert!(
                !elsewhere.iter().any(|suggested| suggested == name),
                "case {name}: it is a property of {owner}, but {other} suggested it too: \
                 {elsewhere:?}"
            );
        }
    }
}

/// RULE: a FIXED member of a resource's searchable entity is not an indexed property, so
/// it is never suggested — not even when the hint is that member's own name — and an
/// account that has stored nothing therefore gets no suggestion for any resource type.
#[tokio::test]
async fn no_fixed_member_of_the_searchable_entity_is_suggested() {
    let seeded_emulator = seeded().await;
    let empty = Emulator::new();

    for (resource, member) in FIXED_MEMBERS {
        for (store, emulator) in [("seeded", &seeded_emulator), ("empty", &empty)] {
            let answered = suggestions(emulator, resource, Some(member)).await;
            assert!(
                !answered.iter().any(|suggested| suggested == member),
                "case {store}/{resource}/{member}: {member} is a fixed member of the \
                 searchable entity, not an indexed property, got {answered:?}"
            );
        }
    }

    // Nothing stored, nothing indexed: the entity's schema contributes no name of its own.
    for resource in RESOURCE_TYPES {
        let answered = suggestions(&empty, resource, None).await;
        assert!(
            answered.is_empty(),
            "case {resource}: an empty account has no indexed property to suggest, \
             got {answered:?}"
        );
    }
}

/// RULE: the whole answer for a resource type is the indexed paths its stored resources
/// contribute — deduplicated, in one stable order — and nothing else.
#[tokio::test]
async fn the_answer_is_exactly_the_indexed_paths_the_stored_resources_contribute() {
    let emulator = seeded().await;

    for resource in RESOURCE_TYPES {
        let all = suggestions(&emulator, resource, None).await;
        let expected: Vec<String> = seeded_property_names(resource).into_iter().collect();
        assert_eq!(
            all, expected,
            "case {resource}: the answer must be the indexed paths the seed stored, \
             got {all:?}"
        );

        // An absent `SuggestionQuery` and the empty hint both limit nothing, so they
        // answer the same list.
        let empty_hint = suggestions(&emulator, resource, Some("")).await;
        assert_eq!(
            all, empty_hint,
            "case {resource}: the empty hint limits nothing, so it must answer what no \
             SuggestionQuery does, got {empty_hint:?} and {all:?}"
        );

        // Every name is a facet PATH: a lowercase facet head, at least one dot, and a key
        // that is not offered bare.
        for name in &all {
            let head = name.split('.').next().unwrap_or(name.as_str());
            assert!(
                name.contains('.') && head == head.to_ascii_lowercase(),
                "case {resource}: {name} is not a lowercased facet path, got {all:?}"
            );
        }
    }
}

/// RULE (invariant): the answer for a hint is exactly the property names the hint BEGINS
/// — the whole name or one of its dot-separated segments — case-insensitively for ASCII,
/// never the ones that merely contain it.
///
/// Swept over EVERY property name of EVERY `ResourceType` value rather than one row: each
/// name yields hints from its own prefixes and from its last segment's, in the ASCII case
/// the name carries and in both foldings.
#[tokio::test]
async fn a_hint_answers_exactly_the_names_it_begins() {
    let emulator = seeded().await;

    for resource in RESOURCE_TYPES {
        let all = suggestions(&emulator, resource, None).await;

        for name in &all {
            // Prefixes of the whole name and of its last segment, at 1, 3 and all
            // characters, in the stored case and in both ASCII foldings.
            let leaf = name.rsplit('.').next().unwrap_or(name.as_str()).to_string();
            let mut hints: BTreeSet<String> = BTreeSet::new();
            for text in [name.as_str(), leaf.as_str()] {
                for take in [1usize, 3, text.chars().count()] {
                    let prefix: String = text.chars().take(take).collect();
                    hints.insert(prefix.to_ascii_lowercase());
                    hints.insert(prefix.to_ascii_uppercase());
                    hints.insert(prefix);
                }
            }

            for hint in &hints {
                let answered = suggestions(&emulator, resource, Some(hint)).await;
                let expected: Vec<String> = all
                    .iter()
                    .filter(|candidate| begins(candidate, hint))
                    .cloned()
                    .collect();
                assert_eq!(
                    answered, expected,
                    "case {resource} name {name} hint {hint:?}: the answer must be the \
                     names the hint begins, got {answered:?}"
                );
            }
        }
    }
}

/// RULE (bound): one answer carries at most [`CAP`] property names — the FIRST of them in
/// the stable order this operation answers in — so an account that stored more keys than
/// the cap still gets one deterministic, bounded list rather than its whole store.
#[tokio::test]
async fn one_answer_is_capped_and_still_deterministic() {
    let emulator = Emulator::new();

    // 50 is `TagList`'s modelled maximum (smithy_query query="shape" shape="TagList"), so
    // these 26 tagged experiments carry 26 x 50 = 1300 distinct tag keys, more than the
    // cap: the names past it are the ones the answer must leave out.
    let mut stored: BTreeSet<String> = BTreeSet::new();
    for group in 0..26u32 {
        let tags: Vec<Value> = (0..50u32)
            .map(|index| {
                let key = format!("key-{group:02}-{index:02}");
                stored.insert(format!("tags.{key}"));
                json!({ "Key": key, "Value": "seeded" })
            })
            .collect();
        emulator
            .ok(
                "CreateExperiment",
                &json!({
                    "ExperimentName": format!("capped-experiment-{group:02}"),
                    "Tags": tags,
                }),
            )
            .await;
    }
    assert!(
        stored.len() > CAP,
        "case seed: the seed must exceed the cap to exercise it, got {} names",
        stored.len()
    );

    let answered = suggestions(&emulator, "Experiment", Some("tags.key-")).await;
    assert_eq!(
        answered.len(),
        CAP,
        "case cap: an answer carries at most {CAP} names, got {}",
        answered.len()
    );
    let first: Vec<String> = stored.iter().take(CAP).cloned().collect();
    assert_eq!(
        answered, first,
        "case cap: the answer must be the first {CAP} names of the order it answers in"
    );
    // Capping does not make the answer unstable: the same call answers the same list.
    let again = suggestions(&emulator, "Experiment", Some("tags.key-")).await;
    assert_eq!(
        answered, again,
        "case cap-repeated: two identical calls must answer the same capped list"
    );
}

/// Whether `hint` begins `name` or one of its dot-separated segments, folding ASCII case
/// — the rule the hint's documentation ("Text that begins a property's name") states.
///
/// Spelled out in a DIFFERENT form from the handler's own test deliberately: lower-casing
/// both sides and asking `starts_with` is the obvious reading of "begins", so a sweep
/// built on it cannot agree with a handler that quietly matched on something else (a
/// substring, a suffix, a byte-wise fold of non-ASCII text).
fn begins(name: &str, hint: &str) -> bool {
    let hint = hint.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    name.starts_with(&hint) || name.split('.').any(|segment| segment.starts_with(&hint))
}

/// RULE: a hint that begins no property name answers an empty list — a 200 with nothing
/// in it, not a refusal — and asking for suggestions changes nothing a later caller sees.
#[tokio::test]
async fn an_unmatched_hint_answers_an_empty_list_and_no_read_changes_the_state() {
    let emulator = seeded().await;

    // Rows differ along what the rule generalises over: the resource type, and why the
    // hint begins nothing (a name no resource carries, a stored key of ANOTHER resource
    // type, a path's segments in the wrong order, text that only occurs INSIDE a name,
    // text longer than any name, a fixed member of the entity).
    let unmatched: Vec<(&str, &str, &str)> = vec![
        ("nonsense", "ExperimentTrialComponent", "zzzz-no-such-name"),
        ("other-types-key", "Endpoint", "learning_rate"),
        ("other-types-tag", "Endpoint", "Project"),
        (
            "segment-out-of-order",
            "ExperimentTrialComponent",
            "Project.Tags",
        ),
        // A hint is text that BEGINS a property's name, so a substring of one in any
        // other position begins nothing: `learning_rate` is suggested, `rate` is not.
        ("substring-not-a-prefix", "ExperimentTrialComponent", "rate"),
        (
            "longer-than-any-name",
            "ExperimentTrialComponent",
            "tags.Projective",
        ),
        ("fixed-member-of-an-unseeded-type", "Job", "JobCategory"),
    ];
    for (case, resource, hint) in &unmatched {
        let answered = suggestions(&emulator, resource, Some(hint)).await;
        assert!(
            answered.is_empty(),
            "case {case}: {hint:?} begins no property name of {resource}, got {answered:?}"
        );
    }

    // A read is a read: the same call answers the same list, and the resource it read is
    // unchanged when it is described afterwards.
    let before = emulator
        .ok(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "suggestions-component" }),
        )
        .await;
    let first = suggestions(&emulator, "ExperimentTrialComponent", Some("Tags")).await;
    let second = suggestions(&emulator, "ExperimentTrialComponent", Some("Tags")).await;
    assert_eq!(
        first, second,
        "case repeated-read: two identical calls must answer the same list, got \
         {first:?} and {second:?}"
    );
    let after = emulator
        .ok(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "suggestions-component" }),
        )
        .await;
    assert_eq!(
        before, after,
        "case repeated-read: GetSearchSuggestions must not change the resources it reads"
    );
}

/// A `CreateTrainingJob` body naming `name` and carrying `hyper_parameters`, so one seed
/// row differs from the next only in the keys under test.
fn training_job_with(name: &str, hyper_parameters: Value) -> Value {
    let mut body = training_body(name, image_specification(), "s3://demo/output/");
    body["HyperParameters"] = hyper_parameters;
    body
}

/// RULE: every key a stored resource carries in a caller-named map of its searchable
/// entity is suggested as the dotted property name a `Search` filter accepts for it — the
/// facet segment as SageMaker names that facet, then the caller's key VERBATIM — so each
/// key supplied in a training job's `HyperParameters` is answered as
/// `hyperparameters.<key>`, lowercase facet and key untouched.
#[tokio::test]
async fn a_training_jobs_hyperparameter_keys_are_suggested_under_the_lowercase_facet() {
    let emulator = Emulator::new();

    // The longest key `HyperParameterKey` admits (smithy_query query="shape"
    // shape="HyperParameterKey": len 0..256, pattern `.*`), so a key at the published
    // bound is offered like any other.
    let longest = "k".repeat(256);
    // Rows differ along what the rule generalises over: the key's case, its characters
    // (ASCII, non-ASCII, an underscore, an embedded dot), and its length (one character,
    // the modelled maximum) — the recorded case is one row among them.
    let keys: Vec<(&str, String)> = vec![
        ("plain", "momentum".to_string()),
        ("mixed-case", "LearningRate".to_string()),
        ("upper-case", "WEIGHT_DECAY".to_string()),
        ("underscored", "max_depth".to_string()),
        // A non-ASCII key: only ASCII case is folded, so this one is matched byte-exactly.
        ("non-ascii", "größe_hp".to_string()),
        ("one-character", "n".to_string()),
        ("dotted", "tree.depth".to_string()),
        ("longest", longest.clone()),
    ];

    // Every key goes on its own job, so the answer is the keys of EVERY stored job rather
    // than of one, and a key supplied with the empty string rides along: SageMaker keeps
    // no such entry, so there is no key to suggest for it.
    for (index, (_, key)) in keys.iter().enumerate() {
        let mut supplied = serde_json::Map::new();
        supplied.insert(key.clone(), json!("0.5"));
        supplied.insert("unset".to_string(), json!(""));
        let body =
            training_job_with(&format!("suggestions-job-{index}"), Value::Object(supplied));
        emulator.ok("CreateTrainingJob", &body).await;
    }
    // A key that is the empty string is admitted by the model (len 0..256) and names no
    // property, so it contributes no path.
    let mut empty_key = serde_json::Map::new();
    empty_key.insert(String::new(), json!("0.5"));
    emulator
        .ok(
            "CreateTrainingJob",
            &training_job_with("suggestions-job-empty-key", Value::Object(empty_key)),
        )
        .await;

    // `PropertyNameHint` is modelled len 0..100 (smithy_query query="shape"
    // shape="PropertyNameQuery"), so a hint for a key longer than that is the first 100
    // characters of the path — which still BEGINS it.
    let bounded = |hint: String| -> String { hint.chars().take(100).collect() };

    for (case, key) in &keys {
        let expected = format!("{HYPER_PARAMETERS_FACET}.{key}");
        let leaf = key.rsplit('.').next().unwrap_or(key.as_str()).to_string();
        let cut: String = expected
            .chars()
            .take(expected.chars().count().saturating_sub(2))
            .collect();
        // Hints differ along how far through the property name the caller has typed and
        // which case they typed the facet in: the whole path, the path cut inside the key
        // (what an autocompleting console sends), the key's last segment on its own, the
        // facet alone, and the facet in the modelled member's casing.
        let hints = [
            bounded(expected.clone()),
            bounded(cut),
            bounded(leaf),
            HYPER_PARAMETERS_FACET.to_string(),
            bounded(format!("HyperParameters.{key}")),
        ];
        for hint in &hints {
            let answered = suggestions(&emulator, "TrainingJob", Some(hint)).await;
            assert!(
                answered.contains(&expected),
                "case {case:?} hint {hint:?}: expected {expected:?} among the \
                 suggestions, got {answered:?}"
            );
            // The facet is answered lowercase and the key verbatim: neither the modelled
            // casing of the member nor a folded key is a property name SageMaker answers.
            for wrong in [
                format!("HyperParameters.{key}"),
                format!("{HYPER_PARAMETERS_FACET}.{}", key.to_lowercase()),
            ] {
                assert!(
                    wrong == expected || !answered.contains(&wrong),
                    "case {case:?} hint {hint:?}: {wrong:?} is not the property name for \
                     this key, got {answered:?}"
                );
            }
            // A key is never offered bare: the path is the form `Filter`'s `Name` accepts.
            assert!(
                !answered.iter().any(|suggested| suggested == key),
                "case {case:?} hint {hint:?}: {key:?} is a bare key, not a Search \
                 property name, got {answered:?}"
            );
        }
    }

    // Neither an entry supplied with the empty string nor one whose key is empty names a
    // property, so no path is offered for either.
    let under_the_facet = suggestions(&emulator, "TrainingJob", Some(HYPER_PARAMETERS_FACET)).await;
    for absent in [
        format!("{HYPER_PARAMETERS_FACET}.unset"),
        format!("{HYPER_PARAMETERS_FACET}."),
        HYPER_PARAMETERS_FACET.to_string(),
    ] {
        assert!(
            !under_the_facet.contains(&absent),
            "case not-a-property {absent:?}: it names no stored hyperparameter, got \
             {under_the_facet:?}"
        );
    }
    // Every key that IS stored is under that facet, and the answer is stable across reads.
    let again = suggestions(&emulator, "TrainingJob", Some(HYPER_PARAMETERS_FACET)).await;
    assert_eq!(
        under_the_facet, again,
        "case repeated-read: two identical calls must answer the same list, got \
         {under_the_facet:?} and {again:?}"
    );
    for (case, key) in &keys {
        let expected = format!("{HYPER_PARAMETERS_FACET}.{key}");
        assert!(
            under_the_facet.contains(&expected),
            "case {case:?}: the facet hint must answer every stored key, {expected:?} is \
             missing from {under_the_facet:?}"
        );
    }

    // A hyperparameter key belongs to the training jobs that store it: no OTHER resource
    // type suggests it, whatever its searchable entity declares.
    for (case, key) in &keys {
        let expected = format!("{HYPER_PARAMETERS_FACET}.{key}");
        let hint = bounded(expected.clone());
        for other in RESOURCE_TYPES.iter().filter(|other| **other != "TrainingJob") {
            let elsewhere = suggestions(&emulator, other, Some(&hint)).await;
            assert!(
                !elsewhere.contains(&expected),
                "case {case:?}: {expected:?} is a property of TrainingJob, but {other} \
                 suggested it too: {elsewhere:?}"
            );
        }
    }
}

/// RULE (the same one, over the OTHER caller-named members a training job's entity
/// declares): a key stored in the job's `Environment` or `Tags` is answered at the path it
/// sits at, with the facet lowercased, so the facet a key is offered under is the one the
/// request put it in.
#[tokio::test]
async fn a_training_jobs_environment_and_tag_keys_are_suggested_at_their_own_paths() {
    let emulator = Emulator::new();

    let mut body = training_body(
        "suggestions-facets",
        image_specification(),
        "s3://demo/output/",
    );
    // `TrainingEnvironmentKey` is `^[a-zA-Z_][a-zA-Z0-9_]*$` (smithy_query query="shape"
    // shape="TrainingEnvironmentKey"), so the rows below span its case and its leading
    // underscore rather than inventing characters it refuses.
    body["HyperParameters"] = json!({ "eta": "0.2" });
    body["Environment"] = json!({ "VERBOSITY": "2", "_private": "1", "mixedCase": "x" });
    body["Tags"] = json!([
        { "Key": "unit", "Value": "integration" },
        { "Key": "Owner.Team", "Value": "science" },
    ]);
    emulator.ok("CreateTrainingJob", &body).await;

    // Rows differ along the facet the key sits in and the key's own shape; each is checked
    // with the full path and with a prefix of it.
    let rows: Vec<(&str, &str)> = vec![
        ("hyperparameter", "hyperparameters.eta"),
        ("environment-upper", "environment.VERBOSITY"),
        ("environment-leading-underscore", "environment._private"),
        ("environment-mixed-case", "environment.mixedCase"),
        ("tag", "tags.unit"),
        ("tag-with-a-dot-in-the-key", "tags.Owner.Team"),
    ];
    for (case, expected) in &rows {
        // The whole path, and the path one CHARACTER short of it (char-wise, so a row
        // carrying non-ASCII text cannot split a character).
        let short: String = expected
            .chars()
            .take(expected.chars().count().saturating_sub(1))
            .collect();
        for hint in [expected.to_string(), short] {
            let answered = suggestions(&emulator, "TrainingJob", Some(&hint)).await;
            assert!(
                answered.iter().any(|suggested| suggested == expected),
                "case {case:?} hint {hint:?}: expected {expected:?} among the \
                 suggestions, got {answered:?}"
            );
        }
    }
}
