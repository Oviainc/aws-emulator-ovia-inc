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
//! property names of that resource that the `PropertyNameHint` begins — the whole name or
//! one of its dot-separated segments ("Text that begins a property's name") — and nothing
//! else. A resource's property names are the members the model binds that resource type
//! to in `SearchRecord` (`Filter`'s `Name` doc: "For valid property names, see
//! SearchRecord") plus, for every key the account's stored resources of that type carry
//! in a keyed member of that entity, the dotted PATH that key sits at (the operation's
//! doc: "Provides suggestions for HyperParameters, Tags, and Metrics"). A key is never
//! offered bare: the path is the form `Filter`'s `Name` accepts. The tests assert that
//! rule over a TABLE of resource types, hints and stored keys, and sweep every property
//! name of every `ResourceType` value for the prefix invariant rather than trusting one
//! row.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{one_variant_config, primary_container, Emulator};

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

/// The two `ResourceType` values `SearchRecord` declares NO member for
/// (smithy_query query="shape" shape="SearchRecord" lists every other one), so the model
/// names no searchable property for them at all.
const WITHOUT_A_SEARCH_RECORD_MEMBER: [&str; 2] = ["Image", "ImageVersion"];

/// Every caller-chosen key [`seeded`] stores, spelled bare. None of them is a property
/// name a `Search` filter accepts — `Filter`'s `Name` takes the path the key sits at
/// (`Tags.Project`, not `Project`) — so none may be suggested on its own.
const BARE_KEYS: [&str; 16] = [
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
/// package group, a tagged experiment, a tagged trial, a tagged hyperparameter tuning job
/// and a tagged feature group.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();

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

/// RULE: a suggestion for a resource type is one of the members the model binds that
/// type to in `SearchRecord`, whatever the hint's case.
#[tokio::test]
async fn modelled_member_names_are_suggested_for_every_resource_type() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: the resource type (including the
    // two whose `SearchRecord` member is not named after it, `ExperimentTrial` and
    // `ExperimentTrialComponent`, and `Model`, whose entity is `ModelDashboardModel`),
    // the hint's case and length, and the kind of member the hint lands on (a scalar, a
    // map, a list, a nested structure). Every expected name below is a member of that
    // entity in the model (smithy_query query="shape" shape="<entity>").
    let cases: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "training-job-hyper-parameters",
            "TrainingJob",
            "Hyper",
            &["HyperParameters"],
        ),
        (
            "training-job-lowercase-hint",
            "TrainingJob",
            "training",
            &["TrainingJobName", "TrainingJobArn", "TrainingJobStatus"],
        ),
        (
            "experiment-display-name",
            "Experiment",
            "Display",
            &["DisplayName"],
        ),
        (
            "experiment-trial-is-the-trial-entity",
            "ExperimentTrial",
            "TrialName",
            &["TrialName"],
        ),
        (
            "experiment-trial-component-entity",
            "ExperimentTrialComponent",
            "trial",
            &["TrialComponentName", "TrialComponentArn"],
        ),
        (
            "endpoint-entity",
            "Endpoint",
            "ENDPOINT",
            &[
                "EndpointName",
                "EndpointArn",
                "EndpointConfigName",
                "EndpointStatus",
            ],
        ),
        (
            "model-is-the-dashboard-entity",
            "Model",
            "Monitoring",
            &["MonitoringSchedules"],
        ),
        (
            "model-package-customer-metadata",
            "ModelPackage",
            "Customer",
            &["CustomerMetadataProperties"],
        ),
        (
            "model-package-group-entity",
            "ModelPackageGroup",
            "ModelPackageGroup",
            &[
                "ModelPackageGroupName",
                "ModelPackageGroupArn",
                "ModelPackageGroupStatus",
            ],
        ),
        (
            "pipeline-entity",
            "Pipeline",
            "pipelined",
            &["PipelineDisplayName", "PipelineDescription"],
        ),
        (
            "pipeline-execution-entity",
            "PipelineExecution",
            "PipelineExecutionS",
            &["PipelineExecutionStatus"],
        ),
        (
            "pipeline-version-entity",
            "PipelineVersion",
            "PipelineVersionI",
            &["PipelineVersionId"],
        ),
        (
            "feature-group-entity",
            "FeatureGroup",
            "feature",
            &["FeatureGroupName", "FeatureDefinitions"],
        ),
        (
            "feature-metadata-entity",
            "FeatureMetadata",
            "FeatureT",
            &["FeatureType"],
        ),
        ("project-entity", "Project", "ProjectI", &["ProjectId"]),
        (
            "tuning-job-search-entity",
            "HyperParameterTuningJob",
            "BestT",
            &["BestTrainingJob"],
        ),
        (
            "model-card-entity",
            "ModelCard",
            "Risk",
            &["RiskRating"],
        ),
        ("job-entity", "Job", "JobCat", &["JobCategory"]),
        (
            "hub-content-entity",
            "HubContent",
            "HubContentSearch",
            &["HubContentSearchKeywords"],
        ),
        // The whole name is a prefix of itself, so the exact name is always suggested.
        ("exact-name", "Endpoint", "CreationTime", &["CreationTime"]),
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
        for suggested in &answered {
            assert!(
                begins(suggested, hint),
                "case {case}: the hint {hint:?} begins neither {suggested:?} nor any of \
                 its segments, got {answered:?}"
            );
        }
    }
}

/// RULE: a key a stored resource carries in a keyed member of its searchable entity is a
/// property name of that resource at the dotted PATH it sits at — never bare, since the
/// path is the form `Filter`'s `Name` accepts — for the resource type that holds it and
/// for no other.
#[tokio::test]
async fn keys_stored_on_a_resource_are_suggested_at_the_path_they_sit_at() {
    let emulator = seeded().await;

    // Rows differ along what the rule generalises over: which keyed member the key sits
    // in (`Tags`, `Parameters`, `InputArtifacts`, `OutputArtifacts`), the depth of the
    // path (`Tags` on the entity itself versus `Model.Tags`, since `SearchRecord`'s
    // `Model` member targets `ModelDashboardModel`, which nests the model), the hint's
    // case, the hint's shape (a key, a dotted path prefix, a whole container) and the
    // key's character set (ASCII, non-ASCII). The answer is the PATH in every row — the
    // form `Filter`'s `Name` accepts — whether the hint begins the path or the key.
    let cases: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "parameter-key",
            "ExperimentTrialComponent",
            "learning",
            &["Parameters.learning_rate"],
        ),
        (
            "parameter-path",
            "ExperimentTrialComponent",
            "Parameters.learn",
            &["Parameters.learning_rate"],
        ),
        (
            "second-parameter-key",
            "ExperimentTrialComponent",
            "batch",
            &["Parameters.batch_size"],
        ),
        (
            "parameter-key-case-folded",
            "ExperimentTrialComponent",
            "OPTIMIS",
            &["Parameters.optimiser"],
        ),
        (
            "non-ascii-parameter-key",
            "ExperimentTrialComponent",
            "größ",
            &["Parameters.größe"],
        ),
        (
            "input-artifact-key",
            "ExperimentTrialComponent",
            "datas",
            &["InputArtifacts.dataset"],
        ),
        (
            "input-artifact-path",
            "ExperimentTrialComponent",
            "InputArtifacts.data",
            &["InputArtifacts.dataset"],
        ),
        (
            "output-artifact-key",
            "ExperimentTrialComponent",
            "metrics_",
            &["OutputArtifacts.metrics_file"],
        ),
        (
            "tag-key",
            "ExperimentTrialComponent",
            "Project",
            &["Tags.Project"],
        ),
        (
            "tag-path-prefix",
            "ExperimentTrialComponent",
            "Tags.Pro",
            &["Tags.Project"],
        ),
        (
            "tag-key-case-folded",
            "ExperimentTrialComponent",
            "TEAM",
            &["Tags.team"],
        ),
        (
            "container-hint-answers-the-member-and-every-key-under-it",
            "ExperimentTrialComponent",
            "Tags",
            &["Tags", "Tags.Project", "Tags.team"],
        ),
        ("model-tag-key", "Model", "dept", &["Model.Tags.dept"]),
        (
            "model-nested-tag-path",
            "Model",
            "Model.Tags",
            &["Model.Tags.dept"],
        ),
        ("endpoint-tag-key", "Endpoint", "env", &["Tags.env"]),
        ("endpoint-tag-path", "Endpoint", "Tags.e", &["Tags.env"]),
        (
            "model-package-group-tag-key",
            "ModelPackageGroup",
            "owner",
            &["Tags.owner"],
        ),
        ("experiment-tag-key", "Experiment", "squad", &["Tags.squad"]),
        (
            "experiment-tag-path",
            "Experiment",
            "Tags.s",
            &["Tags.squad"],
        ),
        ("trial-tag-key", "ExperimentTrial", "phase", &["Tags.phase"]),
        (
            "trial-tag-path",
            "ExperimentTrial",
            "Tags.ph",
            &["Tags.phase"],
        ),
        (
            "tuning-job-tag-key",
            "HyperParameterTuningJob",
            "cohort",
            &["Tags.cohort"],
        ),
        (
            "tuning-job-tag-path",
            "HyperParameterTuningJob",
            "Tags.c",
            &["Tags.cohort"],
        ),
        (
            "feature-group-tag-key",
            "FeatureGroup",
            "domain",
            &["Tags.domain"],
        ),
        (
            "feature-group-tag-path",
            "FeatureGroup",
            "Tags.d",
            &["Tags.domain"],
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
    let stored: [(&str, &str); 10] = [
        ("ExperimentTrialComponent", "Parameters.learning_rate"),
        ("ExperimentTrialComponent", "InputArtifacts.dataset"),
        ("ExperimentTrialComponent", "Tags.Project"),
        ("Model", "Model.Tags.dept"),
        ("Endpoint", "Tags.env"),
        ("ModelPackageGroup", "Tags.owner"),
        ("Experiment", "Tags.squad"),
        ("ExperimentTrial", "Tags.phase"),
        ("HyperParameterTuningJob", "Tags.cohort"),
        ("FeatureGroup", "Tags.domain"),
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

/// RULE: a suggestion is a member of the resource's searchable entity, or the dotted path
/// of a key a stored resource carries under one of those members. Nothing else is a
/// property name of that resource — in particular a key is never offered bare — and a
/// resource type the model binds no `SearchRecord` member to has no property name at all.
#[tokio::test]
async fn every_suggestion_is_a_modelled_member_or_a_path_under_one() {
    let emulator = seeded().await;
    // A second emulator with an EMPTY store: what it answers is the modelled surface
    // alone, since no stored resource can contribute a key to it.
    let empty = Emulator::new();

    for resource in RESOURCE_TYPES {
        let all = suggestions(&emulator, resource, None).await;
        let modelled = suggestions(&empty, resource, None).await;

        // An absent `SuggestionQuery` and the empty hint both limit nothing, so they
        // answer the same list.
        let empty_hint = suggestions(&emulator, resource, Some("")).await;
        assert_eq!(
            all, empty_hint,
            "case {resource}: the empty hint limits nothing, so it must answer what no \
             SuggestionQuery does, got {empty_hint:?} and {all:?}"
        );

        // The list is deduplicated and in one stable order, so two reads agree.
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            all, sorted,
            "case {resource}: suggestions must be deduplicated and ordered, got {all:?}"
        );

        if WITHOUT_A_SEARCH_RECORD_MEMBER.contains(&resource) {
            assert!(
                all.is_empty() && modelled.is_empty(),
                "case {resource}: SearchRecord declares no member for it, so the model \
                 names no property to suggest, got {all:?}"
            );
            continue;
        }
        assert!(
            !modelled.is_empty(),
            "case {resource}: SearchRecord binds it to an entity, so its members must be \
             suggested, got {modelled:?}"
        );
        // Storing resources only ADDS property names; it never takes a modelled one away.
        for name in &modelled {
            assert!(
                all.contains(name),
                "case {resource}: the modelled member {name} must still be suggested, \
                 got {all:?}"
            );
        }

        for name in &all {
            if modelled.contains(name) {
                continue;
            }
            // Every name the stored resources added is a PATH under a modelled member:
            // the key's own spelling is never a suggestion on its own, because it is not
            // the name a `Search` filter accepts.
            let head = name.split('.').next().unwrap_or(name.as_str());
            assert!(
                name.contains('.') && modelled.iter().any(|member| member == head),
                "case {resource}: {name} is neither a modelled member nor a path under \
                 one: {modelled:?}"
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
                stored.insert(format!("Tags.{key}"));
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

    let answered = suggestions(&emulator, "Experiment", Some("Tags.key-")).await;
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
    let again = suggestions(&emulator, "Experiment", Some("Tags.key-")).await;
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
    // hint begins nothing (a name no entity carries, a stored key of ANOTHER resource
    // type, a path's segments in the wrong order, text that only occurs INSIDE a name,
    // text longer than any name).
    let unmatched: Vec<(&str, &str, &str)> = vec![
        ("nonsense", "ExperimentTrialComponent", "zzzz-no-such-name"),
        ("other-types-key", "TrainingJob", "learning_rate"),
        ("other-types-tag", "Endpoint", "Project"),
        (
            "segment-out-of-order",
            "ExperimentTrialComponent",
            "Project.Tags",
        ),
        // A hint is text that BEGINS a property's name, so a substring of one in any
        // other position begins nothing: `learning_rate` is suggested, `rate` is not.
        ("substring-not-a-prefix", "ExperimentTrialComponent", "rate"),
        ("longer-than-any-name", "Job", "JobCategoryAndThenSome"),
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
