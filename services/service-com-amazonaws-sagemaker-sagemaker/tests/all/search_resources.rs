// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `Search`, the query surface over this service's stored
//! resources.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` declares, not a second copy of it.
//!
//! The RULES under test, each asserted over a TABLE of inputs rather than one call:
//!
//! * a resource is in the result exactly when the `SearchExpression` holds for it — a
//!   Boolean tree over its MODELLED properties, combined by `And` (the documented
//!   default) or `Or`, with each `Operator` compared against the property's own value
//!   type;
//! * `Results` is an ordered SEQUENCE: the matching resources sorted by the property
//!   `SortBy` names, in the direction `SortOrder` names (`Descending` by default), with
//!   a total order so the same request answers the same sequence;
//! * a property name the resource's record shape does not declare is refused with HTTP
//!   400 `ValidationException`, whether it is written in a filter, in a nested filter or
//!   in `SortBy`;
//! * every `ResourceType` member is searched against the store that holds that kind of
//!   resource, and `TotalHits` counts every match, not just the page;
//! * `MaxResults` and `NextToken` walk that one sequence exactly once.

use std::collections::BTreeSet;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, message, one_variant_config, primary_container, str_field, Emulator,
    CREATE_SPACING,
};
use crate::pipeline_lifecycle::{definition, step, token};
use crate::training_job_admission::{image_specification, training_body};

/// The `RoleArn` `CreateImage` requires, in the form `custom_images` sends it.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/SageMakerRole";

/// The names of the experiments every expression case is asserted against.
const ALPHA: &str = "alpha-experiment";
const BETA: &str = "beta-experiment";
const GAMMA: &str = "gamma-experiment";

/// `Search` for `resource`, with `body` supplying the rest of the request.
async fn search(emulator: &Emulator, resource: &str, extra: Value) -> Value {
    let mut body = json!({ "Resource": resource });
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        body[member] = value.clone();
    }
    emulator.ok("Search", &body).await
}

/// The `Results` of one answer, as the list it is.
fn results(answered: &Value) -> Vec<Value> {
    answered
        .get("Results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a Results list, got {answered}"))
}

/// `TotalHits.Value` of one answer.
fn total_hits(answered: &Value) -> i64 {
    answered
        .get("TotalHits")
        .and_then(|hits| hits.get("Value"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a TotalHits.Value, got {answered}"))
}

/// The `ExperimentName` of every record of one answer, in the order answered.
fn experiment_names(answered: &Value) -> Vec<String> {
    results(answered)
        .iter()
        .map(|record| {
            let experiment = record
                .get("Experiment")
                .unwrap_or_else(|| panic!("expected an Experiment record, got {record}"));
            str_field(experiment, "ExperimentName")
        })
        .collect()
}

/// Three experiments, created in name order with a gap between them so their
/// `CreationTime`s are distinct, differing along display name, description and tags.
async fn seeded_experiments() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateExperiment",
            &json!({
                "ExperimentName": ALPHA,
                "DisplayName": "Forecasting-alpha",
                "Description": "the first run",
                "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "tier", "Value": "1" }],
            }),
        )
        .await;
    std::thread::sleep(CREATE_SPACING);
    emulator
        .ok(
            "CreateExperiment",
            &json!({
                "ExperimentName": BETA,
                "DisplayName": "Forecasting-beta",
                "Tags": [{ "Key": "team", "Value": "research" }],
            }),
        )
        .await;
    std::thread::sleep(CREATE_SPACING);
    emulator
        .ok(
            "CreateExperiment",
            &json!({
                "ExperimentName": GAMMA,
                "DisplayName": "Vision-gamma",
                "Description": "the third run",
                "Tags": [{ "Key": "team", "Value": "vision" }],
            }),
        )
        .await;
    emulator
}

/// RULE: a resource is in the result exactly when the `SearchExpression` holds for it —
/// every statement for `And` (the default), one statement for `Or`, each filter comparing
/// the resource's own value for the named property under the filter's `Operator`.
#[tokio::test]
async fn search_expression_selects_exactly_the_resources_it_holds_for() {
    let emulator = seeded_experiments().await;

    // Rows differ along the axes the rule generalises over: which operator, how many
    // statements, which Boolean operator joins them, whether the property is a scalar, a
    // tag addressed by key or absent from some resources, and whether the statements are
    // nested in a sub-expression.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "equals-one-name",
            json!({ "Filters": [{ "Name": "ExperimentName", "Value": ALPHA }] }),
            vec![ALPHA],
        ),
        (
            // "If you specify a Value, but not an Operator, SageMaker uses the equals
            // operator."
            "operator-defaults-to-equals",
            json!({ "Filters": [{ "Name": "ExperimentName", "Operator": "Equals", "Value": BETA }] }),
            vec![BETA],
        ),
        (
            "and-is-the-default-boolean-operator",
            json!({ "Filters": [
                { "Name": "Tags.team", "Value": "research" },
                { "Name": "DisplayName", "Operator": "Contains", "Value": "beta" },
            ] }),
            vec![BETA],
        ),
        (
            "explicit-and",
            json!({
                "Operator": "And",
                "Filters": [
                    { "Name": "Tags.team", "Value": "research" },
                    { "Name": "Description", "Operator": "Exists" },
                ],
            }),
            vec![ALPHA],
        ),
        (
            "or-needs-one-statement",
            json!({
                "Operator": "Or",
                "Filters": [
                    { "Name": "ExperimentName", "Value": ALPHA },
                    { "Name": "ExperimentName", "Value": GAMMA },
                ],
            }),
            vec![ALPHA, GAMMA],
        ),
        (
            // "The value of Name is one of the comma delimited strings in Value."
            "in-takes-a-comma-delimited-list",
            json!({ "Filters": [{
                "Name": "ExperimentName",
                "Operator": "In",
                "Value": format!("{ALPHA},{GAMMA}"),
            }] }),
            vec![ALPHA, GAMMA],
        ),
        (
            "in-with-a-single-item",
            json!({ "Filters": [{ "Name": "ExperimentName", "Operator": "In", "Value": BETA }] }),
            vec![BETA],
        ),
        (
            "in-with-an-item-nothing-carries",
            json!({ "Filters": [{
                "Name": "ExperimentName",
                "Operator": "In",
                "Value": "delta-experiment",
            }] }),
            vec![],
        ),
        (
            "contains-matches-a-substring",
            json!({ "Filters": [{
                "Name": "DisplayName",
                "Operator": "Contains",
                "Value": "Forecasting",
            }] }),
            vec![ALPHA, BETA],
        ),
        (
            "not-equals-excludes-one",
            json!({ "Filters": [{
                "Name": "Tags.team",
                "Operator": "NotEquals",
                "Value": "research",
            }] }),
            vec![GAMMA],
        ),
        (
            // A property the resource does not carry has no value to differ from, so it
            // satisfies neither `Equals` nor `NotEquals`.
            "not-exists-selects-the-resource-without-the-property",
            json!({ "Filters": [{ "Name": "Description", "Operator": "NotExists" }] }),
            vec![BETA],
        ),
        (
            "exists-selects-the-resources-with-it",
            json!({ "Filters": [{ "Name": "Description", "Operator": "Exists" }] }),
            vec![ALPHA, GAMMA],
        ),
        (
            "tag-addressed-by-its-key",
            json!({ "Filters": [{ "Name": "Tags.tier", "Value": "1" }] }),
            vec![ALPHA],
        ),
        (
            "tag-key-no-resource-carries",
            json!({ "Filters": [{ "Name": "Tags.absent", "Operator": "Exists" }] }),
            vec![],
        ),
        (
            "sub-expression-nests-a-second-tree",
            json!({
                "Operator": "Or",
                "Filters": [{ "Name": "ExperimentName", "Value": GAMMA }],
                "SubExpressions": [{
                    "Operator": "And",
                    "Filters": [
                        { "Name": "Tags.team", "Value": "research" },
                        { "Name": "DisplayName", "Operator": "Contains", "Value": "alpha" },
                    ],
                }],
            }),
            vec![ALPHA, GAMMA],
        ),
        (
            // `Filter$Value`'s doc gives the timestamp spelling as
            // `YYYY-mm-dd'T'HH:MM:SS`; the zone-qualified RFC 3339 form is accepted too,
            // and both are compared as the instant the response reports.
            "timestamp-bound-in-the-past",
            json!({ "Filters": [{
                "Name": "CreationTime",
                "Operator": "GreaterThan",
                "Value": "2000-01-01T00:00:00",
            }] }),
            vec![ALPHA, BETA, GAMMA],
        ),
        (
            "timestamp-bound-in-the-past-with-a-zone",
            json!({ "Filters": [{
                "Name": "CreationTime",
                "Operator": "GreaterThanOrEqualTo",
                "Value": "2000-01-01T00:00:00Z",
            }] }),
            vec![ALPHA, BETA, GAMMA],
        ),
        (
            "timestamp-bound-in-the-future",
            json!({ "Filters": [{
                "Name": "CreationTime",
                "Operator": "GreaterThan",
                "Value": "2100-01-01T00:00:00Z",
            }] }),
            vec![],
        ),
        (
            "ordering-operator-on-a-text-property-matches-nothing",
            // "Not supported for text properties": a name has no ordering to compare, so
            // the comparison selects nothing rather than ordering the text.
            json!({ "Filters": [{
                "Name": "ExperimentName",
                "Operator": "GreaterThan",
                "Value": "a",
            }] }),
            vec![],
        ),
    ];

    for (case, expression, expected) in cases {
        let answered = search(
            &emulator,
            "Experiment",
            json!({ "SearchExpression": expression }),
        )
        .await;
        let mut answered_names = experiment_names(&answered);
        answered_names.sort();
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            answered_names, expected,
            "case {case}: expected the experiments {expected:?}, got {answered_names:?}"
        );
        assert_eq!(
            total_hits(&answered),
            expected.len() as i64,
            "case {case}: TotalHits must count every match, got {answered}"
        );
    }
}

/// RULE: an absent `SearchExpression` selects every resource of the type, and a
/// `SearchExpression` carrying no statement at all is refused rather than taken as one.
#[tokio::test]
async fn an_absent_search_expression_selects_every_resource_and_an_empty_one_is_refused() {
    let emulator = seeded_experiments().await;

    let answered = search(&emulator, "Experiment", json!({})).await;
    let mut names = experiment_names(&answered);
    names.sort();
    assert_eq!(
        names,
        vec![ALPHA, BETA, GAMMA],
        "case no-expression: every stored experiment must be answered, got {answered}"
    );

    let (status, parsed) = emulator
        .call(
            "Search",
            &json!({ "Resource": "Experiment", "SearchExpression": {} }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case empty-expression: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case empty-expression: error code, got {parsed}"
    );
}

/// RULE: `Results` is an ordered sequence — the matches sorted by the property `SortBy`
/// names in the direction `SortOrder` names, `Descending` by default.
#[tokio::test]
async fn results_are_ordered_by_the_sort_property_in_the_requested_direction() {
    let emulator = seeded_experiments().await;

    // The three experiments were created in name order, so `CreationTime` ascending and
    // `ExperimentName` ascending agree — which is what lets one expected sequence be
    // written per (property, direction) row.
    let ascending = vec![ALPHA, BETA, GAMMA];
    let descending = vec![GAMMA, BETA, ALPHA];

    // Rows differ along the sort property (a timestamp, two different text properties)
    // and the direction, including the two documented defaults.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "defaults-to-last-modified-time-descending",
            json!({}),
            descending.clone(),
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            ascending.clone(),
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            descending.clone(),
        ),
        (
            "name-ascending",
            json!({ "SortBy": "ExperimentName", "SortOrder": "Ascending" }),
            ascending.clone(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "ExperimentName", "SortOrder": "Descending" }),
            descending.clone(),
        ),
        (
            "display-name-ascending",
            json!({ "SortBy": "DisplayName", "SortOrder": "Ascending" }),
            // "Forecasting-alpha" < "Forecasting-beta" < "Vision-gamma".
            ascending.clone(),
        ),
        (
            // A property two of the three do not carry: a resource without a value for
            // the sort property still has one place in the sequence, below the ones that
            // carry one, and the tie-break keeps the rest total.
            "sorted-by-a-property-some-resources-lack",
            json!({ "SortBy": "Description", "SortOrder": "Ascending" }),
            vec![BETA, ALPHA, GAMMA],
        ),
        (
            "default-sort-order-is-descending",
            json!({ "SortBy": "ExperimentName" }),
            descending.clone(),
        ),
    ];

    for (case, sort, expected) in cases {
        let answered = search(&emulator, "Experiment", sort.clone()).await;
        let answered_names = experiment_names(&answered);
        assert_eq!(
            answered_names, expected,
            "case {case}: expected the sequence {expected:?}, got {answered_names:?}"
        );
        // The same request answers the same sequence: the order is a property of the
        // result set, not of the call.
        let again = search(&emulator, "Experiment", sort).await;
        assert_eq!(
            experiment_names(&again),
            answered_names,
            "case {case}: a second identical search must answer the same sequence, got \
             {again}"
        );
    }
}

/// RULE: a numeric bound compares as a number — the instant a timestamp property reports
/// is a number of seconds, so a bound taken from one record selects it and everything
/// after it.
#[tokio::test]
async fn a_numeric_bound_on_a_timestamp_property_selects_from_that_instant_on() {
    let emulator = seeded_experiments().await;

    let ordered = search(
        &emulator,
        "Experiment",
        json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
    )
    .await;
    let records = results(&ordered);
    assert_eq!(
        records.len(),
        3,
        "case bound: three experiments must be stored, got {ordered}"
    );
    let middle = f64_field(
        records[1]
            .get("Experiment")
            .expect("an Experiment record in the second result"),
        "CreationTime",
    );

    // Rows differ along the operator and so along which side of the bound is selected.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("at-or-after", "GreaterThanOrEqualTo", vec![BETA, GAMMA]),
        ("strictly-after", "GreaterThan", vec![GAMMA]),
        ("at-or-before", "LessThanOrEqualTo", vec![ALPHA, BETA]),
        ("strictly-before", "LessThan", vec![ALPHA]),
        ("exactly-at", "Equals", vec![BETA]),
    ];

    for (case, operator, expected) in cases {
        let answered = search(
            &emulator,
            "Experiment",
            json!({ "SearchExpression": { "Filters": [{
                "Name": "CreationTime",
                "Operator": operator,
                "Value": middle.to_string(),
            }] } }),
        )
        .await;
        let mut answered_names = experiment_names(&answered);
        answered_names.sort();
        assert_eq!(
            answered_names, expected,
            "case {case}: expected {expected:?} for a {operator} bound, got {answered_names:?}"
        );
    }

    // The same bound, written as the ISO 8601 instant the doc gives, selects the same
    // records once the sub-second part is dropped by the format: the rows above pin the
    // comparison, this one pins that the two spellings reach it.
    let rendered = DateTime::from_secs_f64(middle)
        .fmt(Format::DateTime)
        .expect("the instant renders as RFC 3339");
    let answered = search(
        &emulator,
        "Experiment",
        json!({ "SearchExpression": { "Filters": [{
            "Name": "CreationTime",
            "Operator": "GreaterThanOrEqualTo",
            "Value": rendered,
        }] } }),
    )
    .await;
    assert!(
        !experiment_names(&answered).is_empty(),
        "case iso-bound: an ISO 8601 bound must be read as an instant, got {answered}"
    );
}

/// RULE: a property name the resource's record shape does not declare is refused with
/// HTTP 400 `ValidationException` naming it, wherever the request writes it; a name the
/// shape does declare is accepted.
#[tokio::test]
async fn a_property_the_resource_does_not_declare_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": ALPHA }))
        .await;
    emulator
        .ok(
            "CreateTrainingJob",
            &training_body("search-job", image_specification(), "s3://demo/output/"),
        )
        .await;
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "search-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;

    // Rows differ along the resource type (whose record shape declares a different set of
    // properties), where in the request the name is written, and the shape of the unknown
    // name: a bare word, a property of ANOTHER resource type, and a dotted path whose
    // first segment names nothing.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "experiment-filter",
            "Experiment",
            "NotAProperty",
            json!({ "SearchExpression": { "Filters": [{ "Name": "NotAProperty", "Value": "x" }] } }),
        ),
        (
            "experiment-filter-names-another-resources-property",
            "Experiment",
            "TrainingJobName",
            json!({ "SearchExpression": { "Filters": [{
                "Name": "TrainingJobName",
                "Value": "x",
            }] } }),
        ),
        (
            "experiment-sort-by",
            "Experiment",
            "NotAProperty",
            json!({ "SortBy": "NotAProperty" }),
        ),
        (
            "training-job-nested-property",
            "TrainingJob",
            "NotAConfig",
            json!({ "SearchExpression": { "NestedFilters": [{
                "NestedPropertyName": "NotAConfig",
                "Filters": [{ "Name": "NotAConfig.ChannelName", "Value": "train" }],
            }] } }),
        ),
        (
            "training-job-nested-inner-filter",
            "TrainingJob",
            "NotAProperty.ChannelName",
            json!({ "SearchExpression": { "NestedFilters": [{
                "NestedPropertyName": "InputDataConfig",
                "Filters": [{ "Name": "NotAProperty.ChannelName", "Value": "train" }],
            }] } }),
        ),
        (
            "training-job-dotted-head",
            "TrainingJob",
            "Nope.InstanceCount",
            json!({ "SearchExpression": { "Filters": [{
                "Name": "Nope.InstanceCount",
                "Operator": "Exists",
            }] } }),
        ),
        (
            // `SearchRecord` carries a model as a `ModelDashboardModel`, so the model's
            // own members sit under `Model`; the bare name is not a property of the
            // record.
            "model-bare-member-of-the-nested-model",
            "Model",
            "ModelName",
            json!({ "SearchExpression": { "Filters": [{ "Name": "ModelName", "Value": "x" }] } }),
        ),
        (
            "sub-expression-filter",
            "Experiment",
            "NotAProperty",
            json!({ "SearchExpression": {
                "Filters": [{ "Name": "ExperimentName", "Value": ALPHA }],
                "SubExpressions": [{ "Filters": [{ "Name": "NotAProperty", "Value": "x" }] }],
            } }),
        ),
    ];

    for (case, resource, name, extra) in cases {
        let mut body = json!({ "Resource": resource });
        for (member, value) in extra.as_object().expect("a JSON object of extra members") {
            body[member] = value.clone();
        }
        let (status, parsed) = emulator.call("Search", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed}"
        );
        assert_eq!(
            message(&parsed),
            format!("Unknown property name: {name}"),
            "case {case}: the refusal must name the property, got {parsed}"
        );
    }

    // The same places, with a property each record shape DOES declare: the check refuses
    // unknown names rather than every name.
    let accepted: Vec<(&str, &str, Value)> = vec![
        (
            "experiment-filter",
            "Experiment",
            json!({ "SearchExpression": { "Filters": [{ "Name": "ExperimentName", "Value": ALPHA }] } }),
        ),
        (
            "experiment-sort-by",
            "Experiment",
            json!({ "SortBy": "DisplayName" }),
        ),
        (
            "training-job-nested-property",
            "TrainingJob",
            json!({ "SearchExpression": { "NestedFilters": [{
                "NestedPropertyName": "InputDataConfig",
                "Filters": [{ "Name": "InputDataConfig.ChannelName", "Value": "train" }],
            }] } }),
        ),
        (
            "model-member-under-its-record",
            "Model",
            json!({ "SearchExpression": { "Filters": [{
                "Name": "Model.ModelName",
                "Value": "search-model",
            }] } }),
        ),
    ];
    for (case, resource, extra) in accepted {
        let answered = search(&emulator, resource, extra).await;
        assert_eq!(
            results(&answered).len(),
            1,
            "case {case}: a declared property must be searched, got {answered}"
        );
    }
}

/// RULE: a filter resolves the property against the resource's own modelled document, so
/// a nested path addresses the members of a nested structure, a nested filter needs ONE
/// element of a list to satisfy all of its filters, and a facet the service publishes
/// lowercased is accepted under either spelling.
#[tokio::test]
async fn nested_properties_resolve_against_the_resources_own_document() {
    let emulator = Emulator::new();
    let mut job = training_body("nested-job", image_specification(), "s3://demo/output/");
    job["HyperParameters"] = json!({ "learning_rate": "0.05" });
    emulator.ok("CreateTrainingJob", &job).await;

    // Rows differ along the shape of the path (a nested structure's member, a map entry
    // under each of its two accepted spellings, a numeric member inside a structure) and
    // along whether the value matches.
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "nested-structure-member",
            json!({ "Filters": [{
                "Name": "ResourceConfig.InstanceType",
                "Value": "ml.m5.large",
            }] }),
            1,
        ),
        (
            "nested-numeric-member",
            json!({ "Filters": [{
                "Name": "ResourceConfig.InstanceCount",
                "Operator": "GreaterThanOrEqualTo",
                "Value": "1",
            }] }),
            1,
        ),
        (
            "nested-numeric-member-out-of-range",
            json!({ "Filters": [{
                "Name": "ResourceConfig.InstanceCount",
                "Operator": "GreaterThan",
                "Value": "1",
            }] }),
            0,
        ),
        (
            "through-a-list-to-a-channels-member",
            json!({ "Filters": [{ "Name": "InputDataConfig.ChannelName", "Value": "train" }] }),
            1,
        ),
        (
            "through-a-list-to-a-value-no-element-carries",
            json!({ "Filters": [{ "Name": "InputDataConfig.ChannelName", "Value": "validation" }] }),
            0,
        ),
        (
            "nested-filters-need-one-element-to-satisfy-all",
            json!({ "NestedFilters": [{
                "NestedPropertyName": "InputDataConfig",
                "Filters": [
                    { "Name": "InputDataConfig.ChannelName", "Value": "train" },
                    {
                        "Name": "InputDataConfig.DataSource.S3DataSource.S3Uri",
                        "Operator": "Contains",
                        "Value": "demo/train",
                    },
                ],
            }] }),
            1,
        ),
        (
            "nested-filters-with-one-statement-no-element-satisfies",
            json!({ "NestedFilters": [{
                "NestedPropertyName": "InputDataConfig",
                "Filters": [
                    { "Name": "InputDataConfig.ChannelName", "Value": "train" },
                    { "Name": "InputDataConfig.ContentType", "Value": "application/json" },
                ],
            }] }),
            0,
        ),
        (
            "map-entry-under-the-modelled-member-name",
            json!({ "Filters": [{ "Name": "HyperParameters.learning_rate", "Value": "0.05" }] }),
            1,
        ),
        (
            // The facet spelling `GetSearchSuggestions` answers for a hyperparameter.
            "map-entry-under-the-published-facet",
            json!({ "Filters": [{ "Name": "hyperparameters.learning_rate", "Value": "0.05" }] }),
            1,
        ),
        (
            "map-entry-no-job-carries",
            json!({ "Filters": [{ "Name": "hyperparameters.momentum", "Operator": "Exists" }] }),
            0,
        ),
    ];

    for (case, expression, expected) in cases {
        let answered = search(
            &emulator,
            "TrainingJob",
            json!({ "SearchExpression": expression }),
        )
        .await;
        assert_eq!(
            results(&answered).len(),
            expected,
            "case {case}: expected {expected} record(s), got {answered}"
        );
    }
}

/// RULE: every `ResourceType` member is searched against the store that holds that kind
/// of resource, and each match is carried in the `SearchRecord` member the model binds
/// that type to.
#[tokio::test]
async fn each_resource_type_is_searched_against_its_own_store() {
    let emulator = Emulator::new();

    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": "an-experiment" }))
        .await;
    emulator
        .ok(
            "CreateTrial",
            &json!({ "TrialName": "a-trial", "ExperimentName": "an-experiment" }),
        )
        .await;
    emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "a-component" }),
        )
        .await;
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "a-model", "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
        .ok("CreateEndpointConfig", &one_variant_config("a-config", "a-model"))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "an-endpoint", "EndpointConfigName": "a-config" }),
        )
        .await;
    emulator
        .ok(
            "CreateModelPackageGroup",
            &json!({ "ModelPackageGroupName": "a-group" }),
        )
        .await;
    emulator
        .ok(
            "CreatePipeline",
            &json!({
                "PipelineName": "a-pipeline",
                "PipelineDefinition": definition(json!([step("Only", &[])])),
                "RoleArn": "arn:aws:iam::123456789012:role/SageMakerRole",
                "ClientRequestToken": token("search-pipeline"),
            }),
        )
        .await;
    emulator
        .ok(
            "CreateFeatureGroup",
            &json!({
                "FeatureGroupName": "a-feature-group",
                "RecordIdentifierFeatureName": "customer-id",
                "EventTimeFeatureName": "event-time",
                "FeatureDefinitions": [
                    { "FeatureName": "customer-id", "FeatureType": "String" },
                    { "FeatureName": "event-time", "FeatureType": "String" },
                ],
                "OnlineStoreConfig": { "EnableOnlineStore": true },
            }),
        )
        .await;
    emulator
        .ok(
            "CreateTrainingJob",
            &training_body("a-training-job", image_specification(), "s3://demo/output/"),
        )
        .await;
    // An image: a `ResourceType` member `SearchRecord` declares no member for, so one
    // can be stored and still never be carried in a result.
    emulator
        .ok(
            "CreateImage",
            &json!({ "ImageName": "an-image", "RoleArn": ROLE_ARN }),
        )
        .await;

    // Rows differ along the resource type, so along the record member each match is
    // carried in and the member that names it.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("Experiment", "Experiment", "ExperimentName", "an-experiment"),
        ("ExperimentTrial", "Trial", "TrialName", "a-trial"),
        (
            "ExperimentTrialComponent",
            "TrialComponent",
            "TrialComponentName",
            "a-component",
        ),
        ("Endpoint", "Endpoint", "EndpointName", "an-endpoint"),
        (
            "ModelPackageGroup",
            "ModelPackageGroup",
            "ModelPackageGroupName",
            "a-group",
        ),
        ("Pipeline", "Pipeline", "PipelineName", "a-pipeline"),
        (
            "FeatureGroup",
            "FeatureGroup",
            "FeatureGroupName",
            "a-feature-group",
        ),
        (
            "TrainingJob",
            "TrainingJob",
            "TrainingJobName",
            "a-training-job",
        ),
    ];

    for (resource, member, names, expected) in cases {
        let answered = search(&emulator, resource, json!({})).await;
        let records = results(&answered);
        assert_eq!(
            records.len(),
            1,
            "case {resource}: expected exactly the one stored resource, got {answered}"
        );
        let record = records[0]
            .get(member)
            .unwrap_or_else(|| panic!("case {resource}: expected a {member} record, got {answered}"));
        assert_eq!(
            str_field(record, names),
            expected,
            "case {resource}: {names}, got {answered}"
        );
        assert_eq!(
            total_hits(&answered),
            1,
            "case {resource}: TotalHits, got {answered}"
        );
    }

    // A model is carried as a `ModelDashboardModel`, which nests the model itself and the
    // endpoints hosting it — both read off this store.
    let answered = search(&emulator, "Model", json!({})).await;
    let records = results(&answered);
    assert_eq!(
        records.len(),
        1,
        "case Model: expected the one stored model, got {answered}"
    );
    let dashboard = records[0]
        .get("Model")
        .unwrap_or_else(|| panic!("case Model: expected a Model record, got {answered}"));
    assert_eq!(
        str_field(
            dashboard
                .get("Model")
                .unwrap_or_else(|| panic!("case Model: expected a nested Model, got {answered}")),
            "ModelName"
        ),
        "a-model",
        "case Model: ModelName, got {answered}"
    );
    let hosting = dashboard
        .get("Endpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case Model: expected the hosting Endpoints, got {answered}"));
    assert_eq!(
        hosting.len(),
        1,
        "case Model: the endpoint hosting the model must be reported, got {answered}"
    );
    assert_eq!(
        str_field(&hosting[0], "EndpointName"),
        "an-endpoint",
        "case Model: hosting EndpointName, got {answered}"
    );

    // Rows for the resource types this emulator holds nothing of: an empty result and a
    // zero count, not a refusal.
    for resource in [
        "Image",
        "ImageVersion",
        "Project",
        "FeatureMetadata",
        "PipelineVersion",
        "ModelCard",
        "ModelPackage",
        "PipelineExecution",
        "HyperParameterTuningJob",
        "Job",
        "HubContent",
    ] {
        let answered = search(&emulator, resource, json!({})).await;
        assert!(
            results(&answered).is_empty(),
            "case {resource}: nothing of this type is stored, got {answered}"
        );
        assert_eq!(
            total_hits(&answered),
            0,
            "case {resource}: TotalHits, got {answered}"
        );
    }

    // A `Resource` that is not a member of the modelled enum is refused.
    let (status, parsed) = emulator
        .call("Search", &json!({ "Resource": "NotAResourceType" }))
        .await;
    assert_eq!(
        status, 400,
        "case unknown-resource: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case unknown-resource: error code, got {parsed}"
    );
}

/// RULE: `MaxResults` caps one page and `NextToken` resumes after its last record, so a
/// paginating client walks the one ordered result set exactly once; `TotalHits` counts
/// the whole set on every page.
#[tokio::test]
async fn max_results_and_next_token_walk_the_result_set_once() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..5).map(|index| format!("paged-{index}")).collect();
    for name in &names {
        emulator
            .ok("CreateExperiment", &json!({ "ExperimentName": name }))
            .await;
        std::thread::sleep(Duration::from_millis(2));
    }

    // Rows differ along the page size, including one that divides the set evenly, one
    // that does not, one page holding everything and the smallest page there is.
    for page_size in [1, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "Resource": "Experiment",
                "SortBy": "ExperimentName",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let answered = emulator.ok("Search", &body).await;
            pages += 1;
            assert!(
                pages <= 10,
                "case page-size={page_size}: pagination must terminate, saw {seen:?}"
            );
            let page = experiment_names(&answered);
            assert!(
                page.len() <= page_size.min(names.len()),
                "case page-size={page_size}: a page may not exceed MaxResults, got {answered}"
            );
            assert_eq!(
                total_hits(&answered),
                names.len() as i64,
                "case page-size={page_size}: TotalHits counts the whole set, got {answered}"
            );
            seen.extend(page);
            match answered.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case page-size={page_size}: the pages must walk the sorted set exactly once, \
             got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<&String>>().len(),
            names.len(),
            "case page-size={page_size}: no record may be answered twice, got {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the walk.
    let (status, parsed) = emulator
        .call(
            "Search",
            &json!({ "Resource": "Experiment", "NextToken": "not-a-token" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case stale-token: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case stale-token: error code, got {parsed}"
    );
}

/// RULE: a search is a READ — it answers the same records however many times it runs, and
/// the resources it reports are the ones the implemented readers report.
#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateExperiment",
            &json!({ "ExperimentName": ALPHA, "Description": "the first run" }),
        )
        .await;

    let first = search(&emulator, "Experiment", json!({})).await;
    let second = search(&emulator, "Experiment", json!({})).await;
    assert_eq!(
        first, second,
        "case repeated-search: two identical searches must answer the same thing, got \
         {second}"
    );

    let described = emulator
        .ok("DescribeExperiment", &json!({ "ExperimentName": ALPHA }))
        .await;
    let record = results(&first)[0]
        .get("Experiment")
        .expect("an Experiment record")
        .clone();
    for member in ["ExperimentName", "ExperimentArn", "DisplayName", "Description"] {
        assert_eq!(
            str_field(&record, member),
            str_field(&described, member),
            "case {member}: the search record must report what the reader does, got {record}"
        );
    }
    assert_eq!(
        f64_field(&record, "CreationTime"),
        f64_field(&described, "CreationTime"),
        "case CreationTime: the search record must report what the reader does, got {record}"
    );
}

/// RULE: `CrossAccountFilterOption` selects which account's resources are searched —
/// `CrossAccount` the ones other accounts made discoverable to this one, of which this
/// emulator holds none, and `SameAccount` or the null default this account's own; and a
/// `VisibilityConditions` entry excludes a resource only when it carries that tag key
/// with another value.
#[tokio::test]
async fn cross_account_and_visibility_conditions_select_which_resources_are_searched() {
    let emulator = seeded_experiments().await;

    // Rows differ along the option and so along which account's resources are selected.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("absent", json!({}), 3),
        (
            "same-account",
            json!({ "CrossAccountFilterOption": "SameAccount" }),
            3,
        ),
        (
            "cross-account",
            json!({ "CrossAccountFilterOption": "CrossAccount" }),
            0,
        ),
    ];
    for (case, extra, expected) in cases {
        let answered = search(&emulator, "Experiment", extra).await;
        assert_eq!(
            results(&answered).len(),
            expected,
            "case {case}: expected {expected} record(s), got {answered}"
        );
    }

    // Rows differ along whether the condition's key is one the resources carry and
    // whether its value matches.
    let conditions: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "matching-value-keeps-the-resources-carrying-it",
            json!([{ "Key": "Tags.team", "Value": "research" }]),
            // `GAMMA` carries the key with another value, so it is excluded.
            vec![ALPHA, BETA],
        ),
        (
            "other-value-excludes-every-resource-carrying-the-key",
            json!([{ "Key": "Tags.team", "Value": "nobody" }]),
            vec![],
        ),
        (
            "key-no-resource-carries-excludes-nothing",
            json!([{ "Key": "Tags.absent", "Value": "anything" }]),
            vec![ALPHA, BETA, GAMMA],
        ),
        (
            "two-conditions-must-both-hold",
            json!([
                { "Key": "Tags.team", "Value": "research" },
                { "Key": "Tags.tier", "Value": "1" },
            ]),
            vec![ALPHA, BETA],
        ),
    ];
    for (case, visibility, expected) in conditions {
        let answered = search(
            &emulator,
            "Experiment",
            json!({ "VisibilityConditions": visibility }),
        )
        .await;
        let mut answered_names = experiment_names(&answered);
        answered_names.sort();
        assert_eq!(
            answered_names, expected,
            "case {case}: expected {expected:?}, got {answered_names:?}"
        );
    }

    // A key not in the documented `Tags.<key>` form is refused.
    let (status, parsed) = emulator
        .call(
            "Search",
            &json!({
                "Resource": "Experiment",
                "VisibilityConditions": [{ "Key": "team", "Value": "research" }],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case bare-key: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bare-key: error code, got {parsed}"
    );
}
