// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise computation-model slice
//! (`CreateComputationModel` -> `DescribeComputationModel`), driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created computation model is PERSISTED and reads back exactly as it was
//!   created — its name, description, configuration, data binding and dates, the
//!   first version of the model, the ARN form `computationModelArn` documents,
//!   and the actions the computation type its configuration names exposes;
//! * a read scoped to a version answers only from a version the store holds for
//!   that model: an absent filter and `LATEST` name the stored version, `ACTIVE`
//!   names it only once the transition the create owes has fallen due, and an
//!   explicit number names it only when it is that version's own number;
//! * a `computationModelId` no create made is the modelled 404, and writes
//!   nothing;
//! * a create whose configuration names no computation type, references a
//!   variable the data binding does not define, or carries a binding value that
//!   does not set exactly one member, is the modelled 400 and stores nothing;
//! * a retried create carrying the `clientToken` of one that already succeeded
//!   replays that answer instead of minting a second model.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ACTIVATION_DELAY, ANOMALY_DETECTION_ACTION_TYPES, MAX_BINDING_VALUE_DEPTH,
};

use crate::asset_lifecycle::{
    call, error_code, is_modelled_id, router, router_with_engine, REGION,
};

/// `POST|GET /computation-models`, as `OPERATIONS.md` reports the routes.
const COMPUTATION_MODELS_URI: &str = "/computation-models";

/// `GET /computation-models/{computationModelId}`.
fn computation_model_uri(id: &str) -> String {
    format!("{COMPUTATION_MODELS_URI}/{id}")
}

/// The same, scoped to the version a `ComputationModelVersionFilter` names.
fn versioned_uri(id: &str, version: &str) -> String {
    format!(
        "{}?computationModelVersion={version}",
        computation_model_uri(id)
    )
}

/// Long enough that a transition the clock owes has certainly fallen due.
fn settle() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// The ARN form this emulator mints for a computation model, as the
/// `computationModelArn` member documents it.
pub(crate) fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:computation-model/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `CreateComputationModel` with `body`.
pub(crate) async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", COMPUTATION_MODELS_URI, Some(body)).await
}

/// `DescribeComputationModel` on `id`, with no version filter.
pub(crate) async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &computation_model_uri(id), None).await
}

/// A `ComputationModelDataBindingValue` naming an asset model's property. Both
/// members of `AssetModelPropertyBindingValue` are modelled `ID`s.
pub(crate) fn asset_model_property(asset_model_id: &str, property_id: &str) -> Value {
    json!({
        "assetModelProperty": { "assetModelId": asset_model_id, "propertyId": property_id }
    })
}

/// A `ComputationModelDataBindingValue` naming one asset's property.
fn asset_property(asset_id: &str, property_id: &str) -> Value {
    json!({ "assetProperty": { "assetId": asset_id, "propertyId": property_id } })
}

/// A `ComputationModelDataBindingValue` naming a LIST of binding values, the
/// third form `ComputationModelDataBindingValue` carries.
fn binding_list(values: Vec<Value>) -> Value {
    json!({ "list": values })
}

/// A modelled `ID`: a lowercase hyphenated UUID built from `digit`, so each row
/// of a table can name ids of its own without minting any.
pub(crate) fn id_of(digit: char) -> String {
    let repeat = |count: usize| std::iter::repeat_n(digit, count).collect::<String>();
    format!(
        "{}-{}-4{}-8{}-{}",
        repeat(8),
        repeat(4),
        repeat(3),
        repeat(3),
        repeat(12)
    )
}

/// An `anomalyDetection` configuration referencing the binding variables
/// `input` and `result`, in the `${VariableName}` form the members document.
pub(crate) fn anomaly_detection(input: &str, result: &str) -> Value {
    json!({
        "anomalyDetection": {
            "inputProperties": format!("${{{input}}}"),
            "resultProperty": format!("${{{result}}}"),
        }
    })
}

/// One row of the create-and-read-back table: a computation model a caller may
/// ask for, differing from its neighbours in the members the create owns.
#[derive(Debug)]
struct Case {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    configuration: Value,
    binding: Value,
    tags: Option<Value>,
}

impl Case {
    /// The JSON body this row's create sends, with every absent member OMITTED
    /// rather than sent as null.
    fn to_json(&self) -> Value {
        let mut body = json!({
            "computationModelName": self.name,
            "computationModelConfiguration": self.configuration.clone(),
            "computationModelDataBinding": self.binding.clone(),
        });
        let object = body.as_object_mut().expect("an object body");
        if let Some(description) = self.description {
            object.insert(
                "computationModelDescription".to_string(),
                json!(description),
            );
        }
        if let Some(tags) = self.tags.clone() {
            object.insert("tags".to_string(), tags);
        }
        body
    }
}

/// The table: computation models differing in how many variables they bind, in
/// which form each binding value takes, in whether they carry a description and
/// in whether they carry tags.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one asset-model property, no description, no tags",
            name: "turbine-anomaly",
            description: None,
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
            tags: None,
        },
        Case {
            label: "asset properties, a description and tags",
            name: "Pump 4 anomaly #1",
            description: Some("Detects anomalies in pump 4"),
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_property(&id_of('4'), &id_of('5')),
                "result": asset_property(&id_of('4'), &id_of('6')),
            }),
            tags: Some(json!({ "line": "packaging", "owner": "controls" })),
        },
        Case {
            label: "a list of binding values and a longer binding",
            name: "fleet_anomaly-2024",
            description: Some("Fleet wide model"),
            configuration: anomaly_detection("inputs_0", "result_value"),
            binding: json!({
                "inputs_0": binding_list(vec![
                    asset_property(&id_of('7'), &id_of('8')),
                    asset_model_property(&id_of('9'), &id_of('a')),
                ]),
                "result_value": asset_property(&id_of('7'), &id_of('b')),
                "spare": asset_model_property(&id_of('9'), &id_of('c')),
            }),
            tags: None,
        },
    ]
}

#[tokio::test]
async fn a_created_computation_model_reads_back_through_describe() {
    let (router, engine) = router_with_engine();

    for case in cases() {
        let (status, created) = create(&router, case.to_json()).await;
        assert_eq!(
            status, 202,
            "case {:?}: create answered {status} {created}",
            case.label
        );
        let id = created["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no computationModelId in {created}", case.label))
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: expected a modelled ID, got {id:?}",
            case.label
        );
        assert_eq!(
            created["computationModelArn"].as_str().unwrap_or(""),
            expected_arn(&id),
            "case {:?}: unexpected ARN in {created}",
            case.label
        );
        assert_eq!(
            created["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "CREATING",
            "case {:?}: a fresh create reports {created}",
            case.label
        );

        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: describe answered {status} {described}",
            case.label
        );
        assert_eq!(
            described["computationModelId"].as_str().unwrap_or(""),
            id,
            "case {:?}: describe reported another model: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelArn"].as_str().unwrap_or(""),
            expected_arn(&id),
            "case {:?}: unexpected ARN in {described}",
            case.label
        );
        assert_eq!(
            described["computationModelName"].as_str().unwrap_or(""),
            case.name,
            "case {:?}: unexpected name in {described}",
            case.label
        );
        match case.description {
            Some(description) => assert_eq!(
                described["computationModelDescription"].as_str().unwrap_or(""),
                description,
                "case {:?}: unexpected description in {described}",
                case.label
            ),
            None => assert!(
                described["computationModelDescription"].is_null(),
                "case {:?}: a model created without a description reports {described}",
                case.label
            ),
        }
        assert_eq!(
            described["computationModelConfiguration"], case.configuration,
            "case {:?}: unexpected configuration in {described}",
            case.label
        );
        assert_eq!(
            described["computationModelDataBinding"], case.binding,
            "case {:?}: unexpected data binding in {described}",
            case.label
        );
        assert_eq!(
            described["computationModelVersion"].as_str().unwrap_or(""),
            "1",
            "case {:?}: a created model reports {described}",
            case.label
        );
        assert_eq!(
            described["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "CREATING",
            "case {:?}: a model read before its owed transition reports {described}",
            case.label
        );
        let created_at = described["computationModelCreationDate"].as_f64();
        let updated_at = described["computationModelLastUpdateDate"].as_f64();
        assert!(
            created_at.is_some_and(|at| at > 0.0) && created_at == updated_at,
            "case {:?}: expected one epoch-seconds instant for both dates, got \
             {created_at:?} and {updated_at:?}",
            case.label
        );

        // The actions the model exposes follow from its computation type, each with
        // an id of its own that every read reports unchanged.
        let actions = described["actionDefinitions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no actionDefinitions in {described}", case.label))
            .clone();
        let mut reported: Vec<(String, String, String)> = actions
            .iter()
            .map(|action| {
                (
                    action["actionType"].as_str().unwrap_or("").to_string(),
                    action["actionName"].as_str().unwrap_or("").to_string(),
                    action["actionDefinitionId"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        reported.sort();
        let mut expected: Vec<String> = ANOMALY_DETECTION_ACTION_TYPES
            .iter()
            .map(|action_type| (*action_type).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            reported
                .iter()
                .map(|(action_type, _, _)| action_type.clone())
                .collect::<Vec<_>>(),
            expected,
            "case {:?}: unexpected action types in {described}",
            case.label
        );
        for (action_type, action_name, action_id) in &reported {
            assert_eq!(
                action_name, action_type,
                "case {:?}: action {action_type} is named {action_name:?}",
                case.label
            );
            assert!(
                is_modelled_id(action_id),
                "case {:?}: action {action_type} carries the id {action_id:?}",
                case.label
            );
        }
        let distinct: std::collections::BTreeSet<&String> =
            reported.iter().map(|(_, _, id)| id).collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {:?}: two actions share an id in {described}",
            case.label
        );

        // Read twice: the ids the create minted are stored, not re-minted per read.
        let (_, again) = describe(&router, &id).await;
        assert_eq!(
            again["actionDefinitions"], described["actionDefinitions"],
            "case {:?}: a second read reports other actions: {again}",
            case.label
        );

        // The tags the create carried are the ones the store holds for the model.
        let expected_tags: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = case
                .tags
                .as_ref()
                .and_then(Value::as_object)
                .map(|tags| {
                    tags.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            pairs.sort();
            pairs
        };
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let mut stored_tags: Vec<(String, String)> = state
            .lock()
            .computation_models
            .get(&id)
            .unwrap_or_else(|| {
                panic!("case {:?}: model {id} was not persisted", case.label)
            })
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        stored_tags.sort();
        assert_eq!(
            stored_tags, expected_tags,
            "case {:?}: unexpected stored tags for model {id}",
            case.label
        );

        // And the same set through the API: a computation model's ARN resolves to
        // that one tag set, so the tagging operations and the create share it.
        let (status, listed) = crate::resource_tags::list_tags(&router, &expected_arn(&id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource on the model answered {status} {listed}",
            case.label
        );
        let mut reported_tags: Vec<(String, String)> = listed["tags"]
            .as_object()
            .unwrap_or_else(|| panic!("case {:?}: no tags map in {listed}", case.label))
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect();
        reported_tags.sort();
        assert_eq!(
            reported_tags, expected_tags,
            "case {:?}: ListTagsForResource reported other tags for model {id}",
            case.label
        );
    }

    // Every row settles into ACTIVE once the transition its create owes falls due.
    tokio::time::sleep(settle()).await;
    let ids: Vec<String> = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        ids.len(),
        cases().len(),
        "expected one stored model per row, got {ids:?}"
    );
    for id in ids {
        let (status, described) = describe(&router, &id).await;
        assert_eq!(status, 200, "settled read of {id}: got {status} {described}");
        assert_eq!(
            described["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "ACTIVE",
            "model {id} after its owed transition reports {described}"
        );
    }
}

/// One row of the version-filter table: the `ComputationModelVersionFilter` a
/// read carries, and the status it answers with before and after the transition
/// the create owes has fallen due.
#[derive(Debug)]
struct VersionCase {
    filter: &'static str,
    while_creating: u16,
    once_active: u16,
}

#[tokio::test]
async fn a_read_answers_only_from_a_version_the_store_holds() {
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "computationModelName": "version-filter",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        }),
    )
    .await;
    assert_eq!(status, 202, "create: got {status} {created}");
    let id = created["computationModelId"]
        .as_str()
        .expect("a minted computationModelId")
        .to_string();

    // The filter's own values come from the modelled
    // `ComputationModelVersionFilter` (`^(LATEST|ACTIVE|[1-9]{1}\d{0,9})$`): the two
    // named versions, the stored version's own number, a number no version of this
    // model carries, and the largest number the pattern admits.
    let rows = [
        VersionCase { filter: "LATEST", while_creating: 200, once_active: 200 },
        VersionCase { filter: "1", while_creating: 200, once_active: 200 },
        VersionCase { filter: "ACTIVE", while_creating: 404, once_active: 200 },
        VersionCase { filter: "2", while_creating: 404, once_active: 404 },
        VersionCase { filter: "9999999999", while_creating: 404, once_active: 404 },
    ];

    for row in &rows {
        let (status, body) = call(&router, "GET", &versioned_uri(&id, row.filter), None).await;
        assert_eq!(
            status, row.while_creating,
            "case {row:?}: a read taken while the model is CREATING answered {status} {body}"
        );
        if status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {row:?}: unexpected error code in {body}"
            );
        }
    }

    // An unscoped read is the LATEST version, which this store always holds.
    let (status, body) = describe(&router, &id).await;
    assert_eq!(status, 200, "an unscoped read answered {status} {body}");

    tokio::time::sleep(settle()).await;

    for row in &rows {
        let (status, body) = call(&router, "GET", &versioned_uri(&id, row.filter), None).await;
        assert_eq!(
            status, row.once_active,
            "case {row:?}: a read taken once the model is ACTIVE answered {status} {body}"
        );
        if status == 200 {
            assert_eq!(
                body["computationModelVersion"].as_str().unwrap_or(""),
                "1",
                "case {row:?}: the served version is reported as {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_computation_model_no_create_made_is_a_modelled_refusal() {
    let (router, engine) = router_with_engine();
    let unknown = id_of('e');

    let for_each_filter = ["", "LATEST", "ACTIVE", "1"];
    for filter in for_each_filter {
        let uri = if filter.is_empty() {
            computation_model_uri(&unknown)
        } else {
            versioned_uri(&unknown, filter)
        };
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {filter:?}: reading an unknown model answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {filter:?}: unexpected error code in {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(&unknown),
            "case {filter:?}: the refusal does not name the requested id: {body}"
        );
    }

    assert!(
        service_state(&*engine)
            .expect("the IoT SiteWise store")
            .lock()
            .computation_models
            .is_empty(),
        "reading an unknown computation model wrote one"
    );
}

/// One row of the refused-create table: a create body that describes no one
/// computation model, and why.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    configuration: Value,
    binding: Value,
}

#[tokio::test]
async fn a_create_that_describes_no_model_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();

    let rows = vec![
        RefusalCase {
            label: "a configuration naming no computation type",
            configuration: json!({}),
            binding: json!({ "input": asset_model_property(&id_of('1'), &id_of('2')) }),
        },
        RefusalCase {
            label: "a reference to a variable the binding does not define",
            configuration: anomaly_detection("input", "missing"),
            binding: json!({ "input": asset_model_property(&id_of('1'), &id_of('2')) }),
        },
        RefusalCase {
            label: "a reference that is not in ${VariableName} form",
            configuration: json!({
                "anomalyDetection": { "inputProperties": "input", "resultProperty": "${result}" }
            }),
            binding: json!({
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
        },
        RefusalCase {
            label: "a binding value setting no member",
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": json!({}),
            }),
        },
        RefusalCase {
            label: "a binding value setting two members",
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": {
                    "assetModelProperty": { "assetModelId": id_of('1'), "propertyId": id_of('2') },
                    "assetProperty": { "assetId": id_of('4'), "propertyId": id_of('5') },
                },
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
        },
        RefusalCase {
            label: "a list element setting no member",
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": binding_list(vec![json!({})]),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
        },
    ];

    for row in &rows {
        let (status, body) = create(
            &router,
            json!({
                "computationModelName": "refused",
                "computationModelConfiguration": row.configuration.clone(),
                "computationModelDataBinding": row.binding.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the create answered {status} {body}",
            row.label
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {:?}: unexpected error code in {body}",
            row.label
        );
        assert!(
            service_state(&*engine)
                .expect("the IoT SiteWise store")
                .lock()
                .computation_models
                .is_empty(),
            "case {:?}: a refused create stored a model",
            row.label
        );
    }
}

/// A binding value whose `list` members nest `depth` levels deep: the innermost
/// level holds one asset-property value, so `depth` is that value's own nesting
/// level and `depth == 1` is a value with no list at all.
fn nested_binding(depth: usize) -> Value {
    let mut value = asset_property(&id_of('7'), &id_of('8'));
    for _ in 1..depth {
        value = binding_list(vec![value]);
    }
    value
}

#[tokio::test]
async fn a_binding_value_may_nest_lists_up_to_the_bound() {
    let (router, engine) = router_with_engine();

    // The published bound, one inside it, and one past it.
    for depth in [1, MAX_BINDING_VALUE_DEPTH - 1, MAX_BINDING_VALUE_DEPTH] {
        let binding = json!({
            "input": nested_binding(depth),
            "result": asset_property(&id_of('7'), &id_of('b')),
        });
        let (status, created) = create(
            &router,
            json!({
                "computationModelName": "nested",
                "computationModelConfiguration": anomaly_detection("input", "result"),
                "computationModelDataBinding": binding.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {depth} levels: the create answered {status} {created}"
        );
        let id = created["computationModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {depth} levels: no id in {created}"))
            .to_string();
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {depth} levels: the read answered {status} {described}"
        );
        assert_eq!(
            described["computationModelDataBinding"], binding,
            "case {depth} levels: the binding read back as {described}"
        );
    }

    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .len();

    let too_deep = MAX_BINDING_VALUE_DEPTH + 1;
    let (status, body) = create(
        &router,
        json!({
            "computationModelName": "nested",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": nested_binding(too_deep),
                "result": asset_property(&id_of('7'), &id_of('b')),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {too_deep} levels: the create answered {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "case {too_deep} levels: unexpected error code in {body}"
    );
    let after = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .len();
    assert_eq!(
        after, held,
        "case {too_deep} levels: a refused create stored a model"
    );
}

#[tokio::test]
async fn a_stored_computation_model_is_a_listable_execution_target() {
    let router = router();
    let (status, created) = create(
        &router,
        json!({
            "computationModelName": "listable",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        }),
    )
    .await;
    assert_eq!(status, 202, "create: got {status} {created}");
    let id = created["computationModelId"]
        .as_str()
        .expect("a minted computationModelId")
        .to_string();

    // `ListExecutions` resolves its target against the same store, so a model
    // this service holds is a target that EXISTS — it has run nothing yet, which
    // is an empty page, not a refusal.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/executions?targetResourceType=COMPUTATION_MODEL&targetResourceId={id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a stored model\": listing its executions answered {status} {listed}"
    );
    let summaries = listed["executionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no executionSummaries list in {listed}"));
    assert!(
        summaries.is_empty(),
        "case \"a stored model\": expected no executions, got {listed}"
    );

    // An id no create made is still the modelled refusal.
    let unknown = id_of('d');
    let (status, body) = call(
        &router,
        "GET",
        &format!("/executions?targetResourceType=COMPUTATION_MODEL&targetResourceId={unknown}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"an unknown model\": listing its executions answered {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"an unknown model\": unexpected error code in {body}"
    );
}

#[tokio::test]
async fn a_retried_create_mints_no_second_model() {
    let (router, engine) = router_with_engine();
    let body = |token: &str| {
        json!({
            "computationModelName": "retried",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
            "clientToken": token,
        })
    };
    let token = "0f8d1a32-dc1b-4f39-9a2a-5f1b0c9e7d41-retry";
    let other = "7c2b4e55-91aa-4d0e-bb77-2f6c8e3a9b10-retry";

    let (status, first) = create(&router, body(token)).await;
    assert_eq!(status, 202, "first create: got {status} {first}");
    let (status, replayed) = create(&router, body(token)).await;
    assert_eq!(status, 202, "retried create: got {status} {replayed}");
    assert_eq!(
        replayed["computationModelId"], first["computationModelId"],
        "case \"same clientToken\": the retry answered with another model: {replayed}"
    );
    assert_eq!(
        replayed["computationModelArn"], first["computationModelArn"],
        "case \"same clientToken\": the retry answered with another ARN: {replayed}"
    );

    let (status, second) = create(&router, body(other)).await;
    assert_eq!(status, 202, "create with another token: got {status} {second}");
    assert_ne!(
        second["computationModelId"], first["computationModelId"],
        "case \"another clientToken\": the create replayed the first model: {second}"
    );

    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .len();
    assert_eq!(
        held, 2,
        "expected one model per distinct clientToken, the store holds {held}"
    );
}
