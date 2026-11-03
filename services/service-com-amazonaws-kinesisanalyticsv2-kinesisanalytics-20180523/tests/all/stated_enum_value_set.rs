// SPDX-License-Identifier: CC0-1.0

//! WHICH values a boundary refusal names when a request carries a value outside an
//! enumerated member's set.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`.
//!
//! The rules, each asserted over a TABLE rather than over one request:
//!
//! * a refusal of a value outside an enumerated member states the admissible set the
//!   SERVICE names for the enum that member targets, which the public model can declare
//!   more values than — it is published from the SDK's view of the API — and states it in
//!   lexicographic order. The set is a property of the SHAPE, so every member targeting it,
//!   in any operation, is stated the same way;
//! * every other enumerated type of this model is stated as the model declares it, so the
//!   rule is one table entry rather than a sentence per enum;
//! * what the refusal NAMES is not what the boundary ACCEPTS: a runtime the stated set
//!   leaves out is still taken by a create, and is read back on the stored application.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::{
    ARTIFACT_TYPE_VALUES, CONFIGURATION_TYPE_VALUES, KEY_TYPE_VALUES, METRICS_LEVEL_VALUES,
    RUNTIME_ENVIRONMENT_VALUES,
};

use super::harness::{call, create_body, describe, error_code, error_message, router};

/// The admissible set a refusal NAMES for `RuntimeEnvironment` — the runtime families this
/// service enumerates for the member: the SQL runtime, the Flink runtimes through 1_15 and
/// the Zeppelin-Flink notebook runtimes.
///
/// SOURCE: the bracketed set in the service's own refusal of a runtime outside it, which is
/// service data about the enum rather than one request's answer. The published model also
/// declares the later Flink releases ([`RUNTIME_ENVIRONMENT_VALUES`]), which the refusal
/// does not name and which a create nevertheless accepts
/// ([`a_runtime_the_stated_set_leaves_out_is_still_accepted`]).
pub(crate) const NAMED_RUNTIME_ENVIRONMENT_VALUES: &[&str] = &[
    "FLINK-1_11",
    "FLINK-1_13",
    "FLINK-1_15",
    "FLINK-1_6",
    "FLINK-1_8",
    "SQL-1_0",
    "ZEPPELIN-FLINK-1_0",
    "ZEPPELIN-FLINK-2_0",
    "ZEPPELIN-FLINK-3_0",
];

/// This service's Smithy model, as it ships beside this crate. Read at test time rather than
/// embedded: it is megabytes of JSON that no non-test build needs.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json"
);

/// The whole message a refusal of one off-enum value states: the framework's envelope and
/// clause over the set `values`, stated in lexicographic order.
fn expected_message(submitted: &str, path: &str, values: &[&str]) -> String {
    let mut stated: Vec<&str> = values.to_vec();
    stated.sort_unstable();
    format!(
        "1 validation error detected: Value '{submitted}' at '{path}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        stated.join(", ")
    )
}

/// One member a request can carry a value outside its enumerated type's set in: where it
/// sits (`operation`, `path`), the request that carries it, the value submitted, and the set
/// the refusal must name.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    path: &'static str,
    submitted: &'static str,
    values: &'static [&'static str],
}

/// Drive every row and assert the refusal: HTTP 400, `ValidationException`, and the whole
/// message the row expects.
async fn assert_rows(rows: Vec<Row>) {
    let router = router();
    // One stored application, so a row about an UPDATE is about the value it carries rather
    // than about an application nothing holds.
    let (status, created) = call(
        &router,
        "CreateApplication",
        &create_body("enum-probe", "FLINK-1_20"),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication got {status} {created}");

    for Row {
        case,
        operation,
        body,
        path,
        submitted,
        values,
    } in rows
    {
        let (status, refused) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        let expected = expected_message(submitted, path, values);
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A `CreateApplication` body whose `ApplicationConfiguration` is `configuration`, so a row
/// varies exactly the nested member it is about.
fn configured(configuration: Value) -> Value {
    let mut body = create_body("enum-probe-2", "FLINK-1_20");
    body["ApplicationConfiguration"] = configuration;
    body
}

/// Every value outside `RuntimeEnvironment` is refused with the set the SERVICE names for
/// that shape — not the whole set the published model declares.
///
/// The rows differ along the member that reaches the shape (the runtime a create names and
/// the runtime an update moves an application to, in two operations) and along the FORM of
/// the rejected value: a release no runtime family published, a published runtime in the
/// wrong case, the empty string, a runtime family this service does not run, and a later
/// release of a family it does.
#[tokio::test]
async fn an_enumerated_set_is_stated_as_the_service_names_it() {
    let rows = vec![
        Row {
            case: "a release no runtime family publishes, on a create",
            operation: "CreateApplication",
            body: create_body("enum-probe-2", "FLINK-99_9"),
            path: "runtimeEnvironment",
            submitted: "FLINK-99_9",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        Row {
            case: "a published runtime in the wrong case, on a create",
            operation: "CreateApplication",
            body: create_body("enum-probe-2", "flink-1_20"),
            path: "runtimeEnvironment",
            submitted: "flink-1_20",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        Row {
            case: "the empty string, on a create",
            operation: "CreateApplication",
            body: create_body("enum-probe-2", ""),
            path: "runtimeEnvironment",
            submitted: "",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        Row {
            case: "a runtime family this service does not run, on a create",
            operation: "CreateApplication",
            body: create_body("enum-probe-2", "SPARK-3_5"),
            path: "runtimeEnvironment",
            submitted: "SPARK-3_5",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        Row {
            case: "a later release of the SQL family, on an update",
            operation: "UpdateApplication",
            body: json!({
                "ApplicationName": "enum-probe",
                "CurrentApplicationVersionId": 1,
                "RuntimeEnvironmentUpdate": "SQL-2_0",
            }),
            path: "runtimeEnvironmentUpdate",
            submitted: "SQL-2_0",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        Row {
            case: "a notebook release nothing publishes, on an update",
            operation: "UpdateApplication",
            body: json!({
                "ApplicationName": "enum-probe",
                "ConditionalToken": "tok",
                "RuntimeEnvironmentUpdate": "ZEPPELIN-FLINK-9_0",
            }),
            path: "runtimeEnvironmentUpdate",
            submitted: "ZEPPELIN-FLINK-9_0",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
    ];

    assert_rows(rows).await;
}

/// Every OTHER enumerated type of this model is stated as the model declares it, so the one
/// shape the service names differently is the exception and not the rule.
///
/// The rows differ along the enumerated type and along the FORM the member takes: a required
/// member of a nested structure, an optional one beside it, a member two structures deep, and
/// a member of a structure inside a LIST — whose nested path the framework renders.
#[tokio::test]
async fn every_other_enumerated_type_is_stated_as_the_model_declares_it() {
    let rows = vec![
        Row {
            case: "the metrics level of a monitoring configuration",
            operation: "CreateApplication",
            body: configured(json!({
                "FlinkApplicationConfiguration": {
                    "MonitoringConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "MetricsLevel": "CLUSTER"
                    }
                }
            })),
            path: "applicationConfiguration.flinkApplicationConfiguration.\
                   monitoringConfiguration.metricsLevel",
            submitted: "CLUSTER",
            values: METRICS_LEVEL_VALUES,
        },
        Row {
            case: "a required nested member: the monitoring configuration type",
            operation: "CreateApplication",
            body: configured(json!({
                "FlinkApplicationConfiguration": {
                    "MonitoringConfiguration": {"ConfigurationType": "INHERIT"}
                }
            })),
            path: "applicationConfiguration.flinkApplicationConfiguration.\
                   monitoringConfiguration.configurationType",
            submitted: "INHERIT",
            values: CONFIGURATION_TYPE_VALUES,
        },
        Row {
            case: "the key type of an encryption configuration",
            operation: "CreateApplication",
            body: configured(json!({
                "ApplicationEncryptionConfiguration": {"KeyType": "CUSTOMER_KEY"}
            })),
            path: "applicationConfiguration.applicationEncryptionConfiguration.keyType",
            submitted: "CUSTOMER_KEY",
            values: KEY_TYPE_VALUES,
        },
        Row {
            case: "a member of a list element: a custom artifact's type",
            operation: "CreateApplication",
            body: configured(json!({
                "ZeppelinApplicationConfiguration": {
                    "CustomArtifactsConfiguration": [{"ArtifactType": "JAR"}]
                }
            })),
            path: "applicationConfiguration.zeppelinApplicationConfiguration.\
                   customArtifactsConfiguration.1.member.artifactType",
            submitted: "JAR",
            values: ARTIFACT_TYPE_VALUES,
        },
    ];

    assert_rows(rows).await;
}

/// Every enumerated type this model declares, as (shape id, the values it declares).
///
/// Both spellings are read: a Smithy 2.0 `enum` shape, whose members carry
/// `smithy.api#enumValue`, and the older `smithy.api#enum` trait on a string shape.
fn modelled_enums() -> Vec<(String, BTreeSet<String>)> {
    let model = std::fs::read_to_string(MODEL_PATH)
        .expect("the model that generated this crate ships beside it");
    let model: Value =
        serde_json::from_str(&model).expect("the model that generated this crate is JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("a Smithy model declares shapes")
        .clone();
    shapes
        .into_iter()
        .filter_map(|(id, shape)| {
            let declared: BTreeSet<String> = if let Some(values) =
                shape["traits"]["smithy.api#enum"].as_array()
            {
                values
                    .iter()
                    .filter_map(|entry| entry["value"].as_str().map(str::to_string))
                    .collect()
            } else if shape["type"].as_str() == Some("enum") {
                shape["members"]
                    .as_object()?
                    .values()
                    .filter_map(|member| {
                        member["traits"]["smithy.api#enumValue"]
                            .as_str()
                            .map(str::to_string)
                    })
                    .collect()
            } else {
                return None;
            };
            (!declared.is_empty()).then_some((id, declared))
        })
        .collect()
}

/// GUARD on how the service's table identifies an enum: a runtime violation carries the
/// value SET rather than the shape it came from, so the set keyed on must be one exactly ONE
/// enum of this model declares — otherwise a second enum sharing it would be stated with the
/// wrong set. And what the service names must be values the model declares: a refusal may
/// name fewer runtimes than the model publishes, never one the model does not have.
#[test]
fn the_named_set_belongs_to_one_enum_of_the_model_and_names_only_values_it_declares() {
    let enums = modelled_enums();
    assert!(
        enums.len() >= 10,
        "case \"the model's enums\": expected this service's enumerated types, got {enums:?}"
    );

    let declared: BTreeSet<String> = RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let matching: Vec<&str> = enums
        .iter()
        .filter(|(_, values)| *values == declared)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        matching,
        vec!["com.amazonaws.kinesisanalyticsv2#RuntimeEnvironment"],
        "case \"the keyed set\": exactly one enum of this model may declare it, got {matching:?}"
    );

    let named: Vec<&str> = NAMED_RUNTIME_ENVIRONMENT_VALUES.to_vec();
    let mut sorted = named.clone();
    sorted.sort_unstable();
    assert_eq!(
        named, sorted,
        "case \"the named set's order\": expected {sorted:?}, got {named:?}"
    );
    let undeclared: Vec<&str> = named
        .iter()
        .copied()
        .filter(|value| !declared.contains(*value))
        .collect();
    assert!(
        undeclared.is_empty(),
        "case \"the named set's values\": expected every named runtime to be one the model \
         declares, got {undeclared:?}"
    );
}

/// What a refusal NAMES does not narrow what the boundary ACCEPTS: a create naming any
/// runtime the model declares is taken — stored and read back under that runtime — including
/// the later Flink releases the stated set leaves out. The retired SQL family is the one
/// exception, and it is refused by the handler with its modelled
/// `UnsupportedOperationException` rather than as an invalid value.
#[tokio::test]
async fn a_runtime_the_stated_set_leaves_out_is_still_accepted() {
    let router = router();
    let mut accepted_beyond_the_stated_set: Vec<&str> = Vec::new();

    for runtime in RUNTIME_ENVIRONMENT_VALUES {
        let name = format!("probe-{runtime}");
        let (status, body) = call(&router, "CreateApplication", &create_body(&name, runtime)).await;

        if runtime.starts_with("SQL-") {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "UnsupportedOperationException"),
                "case {runtime:?}: expected the retired SQL family to be refused as \
                 unsupported, got {status} {body}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {runtime:?}: expected the create to be accepted, got {status} {body}"
        );
        let stored = describe(&router, &name).await;
        assert_eq!(
            stored["RuntimeEnvironment"].as_str(),
            Some(*runtime),
            "case {runtime:?}: expected the stored application to run on it, got {stored}"
        );
        if !NAMED_RUNTIME_ENVIRONMENT_VALUES.contains(runtime) {
            accepted_beyond_the_stated_set.push(runtime);
        }
    }

    assert!(
        !accepted_beyond_the_stated_set.is_empty(),
        "case \"beyond the stated set\": expected the model to declare runtimes the refusal \
         does not name, got none"
    );
}
