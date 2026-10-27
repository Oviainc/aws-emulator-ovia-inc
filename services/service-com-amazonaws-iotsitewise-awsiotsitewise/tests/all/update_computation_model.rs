// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateComputationModel`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * an accepted update REPLACES the stored definition of a computation model
//!   with the one the request states — its name, description, configuration and
//!   data binding — counts the write as the model's next version, leaves its id,
//!   ARN and creation date alone, and reports a status of `UPDATING` until the
//!   transition the call owes falls due, after which the model is `ACTIVE`;
//! * a read scoped to a version answers only from the version the store holds,
//!   so after an update the version the create wrote is gone;
//! * a configuration and data binding that do not describe one model are the
//!   modelled 400, and the stored model is left exactly as it was;
//! * a `computationModelId` no create made is the modelled 404, and writes
//!   nothing;
//! * a model still settling a call it already accepted is the modelled 409, and
//!   a retry once that transition has fallen due succeeds;
//! * a retried update carrying the `clientToken` of one that already succeeded
//!   replays that answer and counts no second version, while a DIFFERENT request
//!   reusing that token is the modelled 409 and changes nothing.
//!
//! The HTTP harness (`call`, `router`, `error_code`) and the computation-model
//! bodies (`create`, `describe`, `anomaly_detection`, the binding-value forms)
//! are the ones the sibling suites already define; this suite reuses them rather
//! than growing a second copy.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::computation_model_lifecycle::{
    anomaly_detection, asset_model_property, asset_property, binding_list, create, describe,
    expected_arn, id_of,
};

/// Long enough that a transition the clock owes has certainly fallen due.
fn settle() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// `POST /computation-models/{computationModelId}`, as `OPERATIONS.md` reports
/// the route `UpdateComputationModel` is bound to.
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/computation-models/{id}"),
        Some(body),
    )
    .await
}

/// `DescribeComputationModel` on `id`, scoped to the version
/// `ComputationModelVersionFilter` names.
async fn describe_version(
    router: &MultiServiceRouter,
    id: &str,
    version: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/computation-models/{id}?computationModelVersion={version}"),
        None,
    )
    .await
}

/// A `ClientToken`-shaped value built from `seed`.
///
/// Source: the `clientToken` member this operation declares —
/// `smithy_query(query="operation", operation="UpdateComputationModel")` reports
/// target `ClientToken`, `length 36..64`, `pattern ^\S{36,64}$` — so a short,
/// readable seed is padded with a non-whitespace filler up to the minimum.
fn client_token(seed: &str) -> String {
    let token = format!("{seed:-<36}");
    assert!(
        (36..=64).contains(&token.len()),
        "case {seed:?}: a ClientToken is 36..64 characters, built {token:?}"
    );
    token
}

/// One computation-model definition: the members a create and an update both
/// state in full.
#[derive(Debug, Clone)]
struct Definition {
    name: &'static str,
    description: Option<&'static str>,
    configuration: Value,
    binding: Value,
}

impl Definition {
    /// The JSON body a create or an update sends for this definition, with an
    /// absent description OMITTED rather than sent as null.
    fn to_json(&self) -> Value {
        let mut body = json!({
            "computationModelName": self.name,
            "computationModelConfiguration": self.configuration.clone(),
            "computationModelDataBinding": self.binding.clone(),
        });
        if let Some(description) = self.description {
            body.as_object_mut()
                .expect("an object body")
                .insert(
                    "computationModelDescription".to_string(),
                    json!(description),
                );
        }
        body
    }
}

/// One row of the update table: the definition a model is created with and the
/// definition an update states for it.
#[derive(Debug)]
struct Case {
    label: &'static str,
    before: Definition,
    after: Definition,
}

/// The table: updates differing in which members change (the name alone, the
/// description, the configuration's references, the binding), in how many
/// variables the binding defines, in which form each binding value takes, and in
/// whether the description is added, replaced or cleared.
fn cases() -> Vec<Case> {
    let two_model_properties = json!({
        "input": asset_model_property(&id_of('1'), &id_of('2')),
        "result": asset_model_property(&id_of('1'), &id_of('3')),
    });
    vec![
        Case {
            label: "the name alone changes",
            before: Definition {
                name: "turbine-anomaly",
                description: None,
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
            after: Definition {
                name: "turbine-anomaly renamed",
                description: None,
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
        },
        Case {
            label: "a description is added to a model created without one",
            before: Definition {
                name: "Pump 4 anomaly #1",
                description: None,
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
            after: Definition {
                name: "Pump 4 anomaly #1",
                description: Some("Detects anomalies in pump 4"),
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
        },
        Case {
            label: "the description is CLEARED by an update that omits it",
            before: Definition {
                name: "fleet_anomaly-2024",
                description: Some("Fleet wide model"),
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
            after: Definition {
                name: "fleet_anomaly-2024",
                description: None,
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
        },
        Case {
            label: "the binding is re-bound to asset properties under other variable names",
            before: Definition {
                name: "line-3 anomaly",
                description: Some("Line 3"),
                configuration: anomaly_detection("input", "result"),
                binding: two_model_properties.clone(),
            },
            after: Definition {
                name: "line-3 anomaly",
                description: Some("Line 3 rebound"),
                configuration: anomaly_detection("inputs_0", "result_value"),
                binding: json!({
                    "inputs_0": asset_property(&id_of('4'), &id_of('5')),
                    "result_value": asset_property(&id_of('4'), &id_of('6')),
                }),
            },
        },
        Case {
            label: "the binding grows a list value and a variable nothing references",
            before: Definition {
                name: "fleet anomaly",
                description: None,
                configuration: anomaly_detection("inputs_0", "result_value"),
                binding: json!({
                    "inputs_0": asset_property(&id_of('7'), &id_of('8')),
                    "result_value": asset_property(&id_of('7'), &id_of('b')),
                }),
            },
            after: Definition {
                name: "fleet anomaly",
                description: None,
                configuration: anomaly_detection("inputs_0", "result_value"),
                binding: json!({
                    "inputs_0": binding_list(vec![
                        asset_property(&id_of('7'), &id_of('8')),
                        asset_model_property(&id_of('9'), &id_of('a')),
                    ]),
                    "result_value": asset_property(&id_of('7'), &id_of('b')),
                    "spare": asset_model_property(&id_of('9'), &id_of('c')),
                }),
            },
        },
    ]
}

/// Create the model `definition` states and wait out the transition the create
/// owes, so the model is ACTIVE and can be updated.
async fn create_active(router: &MultiServiceRouter, definition: &Definition) -> String {
    let (status, created) = create(router, definition.to_json()).await;
    assert_eq!(
        status, 202,
        "create of {:?} answered {status} {created}",
        definition.name
    );
    let id = created["computationModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no computationModelId in {created}"))
        .to_string();
    tokio::time::sleep(settle()).await;
    id
}

/// The description a read must report for `definition`.
fn assert_description(described: &Value, definition: &Definition, label: &str) {
    match definition.description {
        Some(description) => assert_eq!(
            described["computationModelDescription"].as_str().unwrap_or(""),
            description,
            "case {label:?}: expected the description {description:?}, got {described}"
        ),
        None => assert!(
            described["computationModelDescription"].is_null(),
            "case {label:?}: expected no description, got {described}"
        ),
    }
}

#[tokio::test]
async fn an_accepted_update_replaces_the_stored_definition_and_reads_back() {
    let router = router();

    for case in cases() {
        let id = create_active(&router, &case.before).await;

        let (status, before) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read before the update answered {status} {before}",
            case.label
        );
        assert_eq!(
            before["computationModelVersion"].as_str().unwrap_or(""),
            "1",
            "case {:?}: a created model reports {before}",
            case.label
        );
        let created_at = before["computationModelCreationDate"].as_f64();
        let actions_before = before["actionDefinitions"].clone();

        let (status, accepted) = update(&router, &id, case.after.to_json()).await;
        assert_eq!(
            status, 202,
            "case {:?}: the update answered {status} {accepted}",
            case.label
        );
        assert_eq!(
            accepted["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "UPDATING",
            "case {:?}: an accepted update reports {accepted}",
            case.label
        );

        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read after the update answered {status} {described}",
            case.label
        );
        assert_eq!(
            described["computationModelId"].as_str().unwrap_or(""),
            id,
            "case {:?}: the update moved the model's id: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelArn"].as_str().unwrap_or(""),
            expected_arn(&id),
            "case {:?}: the update moved the model's ARN: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelName"].as_str().unwrap_or(""),
            case.after.name,
            "case {:?}: unexpected name in {described}",
            case.label
        );
        assert_description(&described, &case.after, case.label);
        assert_eq!(
            described["computationModelConfiguration"], case.after.configuration,
            "case {:?}: unexpected configuration in {described}",
            case.label
        );
        assert_eq!(
            described["computationModelDataBinding"], case.after.binding,
            "case {:?}: unexpected data binding in {described}",
            case.label
        );
        assert_eq!(
            described["computationModelVersion"].as_str().unwrap_or(""),
            "2",
            "case {:?}: an updated model reports {described}",
            case.label
        );
        assert_eq!(
            described["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "UPDATING",
            "case {:?}: a model read before its owed transition reports {described}",
            case.label
        );
        assert_eq!(
            described["computationModelCreationDate"].as_f64(),
            created_at,
            "case {:?}: the update moved the creation date: {described}",
            case.label
        );
        let updated_at = described["computationModelLastUpdateDate"].as_f64();
        assert!(
            updated_at.is_some_and(|at| created_at.is_some_and(|created| at >= created)),
            "case {:?}: expected a last-update instant at or after {created_at:?}, got \
             {updated_at:?}",
            case.label
        );
        // The actions a model publishes follow from its computation type, which
        // none of these updates changes, so their ids are the ones the create
        // minted.
        assert_eq!(
            described["actionDefinitions"], actions_before,
            "case {:?}: the update re-minted the model's actions: {described}",
            case.label
        );

        // The store keeps the one version, so the version the create wrote is no
        // longer a version a read can be answered from.
        for (version, expected) in [("1", 404), ("2", 200), ("LATEST", 200), ("ACTIVE", 404)] {
            let (status, body) = describe_version(&router, &id, version).await;
            assert_eq!(
                status, expected,
                "case {:?}: a read scoped to {version:?} while UPDATING answered {status} \
                 {body}",
                case.label
            );
        }

        tokio::time::sleep(settle()).await;
        let (status, settled) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the settled read answered {status} {settled}",
            case.label
        );
        assert_eq!(
            settled["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "ACTIVE",
            "case {:?}: a model after its owed transition reports {settled}",
            case.label
        );
        let (status, body) = describe_version(&router, &id, "ACTIVE").await;
        assert_eq!(
            status, 200,
            "case {:?}: a read scoped to ACTIVE once settled answered {status} {body}",
            case.label
        );
        assert_eq!(
            body["computationModelVersion"].as_str().unwrap_or(""),
            "2",
            "case {:?}: the active version reads back as {body}",
            case.label
        );
    }
}

/// One row of the refusal table: a body whose configuration and data binding do
/// not describe one computation model.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    configuration: Value,
    binding: Value,
}

/// The rows span the three things the one configuration-and-binding rule
/// requires: that the configuration name a computation type, that every
/// `${VariableName}` it references be a variable the binding defines, and that
/// every binding value set exactly one of its members.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            label: "the configuration names no computation type",
            configuration: json!({}),
            binding: json!({ "input": asset_model_property(&id_of('1'), &id_of('2')) }),
        },
        RefusalCase {
            label: "a referenced variable the binding does not define",
            configuration: anomaly_detection("input", "missing"),
            binding: json!({ "input": asset_model_property(&id_of('1'), &id_of('2')) }),
        },
        RefusalCase {
            label: "a reference that is not in the ${VariableName} form",
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
                "input": json!({}),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
        },
        RefusalCase {
            label: "a binding value setting two members",
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": json!({
                    "assetModelProperty": { "assetModelId": id_of('1'), "propertyId": id_of('2') },
                    "assetProperty": { "assetId": id_of('4'), "propertyId": id_of('5') },
                }),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
        },
    ]
}

#[tokio::test]
async fn an_update_that_does_not_describe_one_model_is_refused_and_changes_nothing() {
    let router = router();
    let stored = Definition {
        name: "refusal-subject",
        description: Some("Unchanged by every refused update"),
        configuration: anomaly_detection("input", "result"),
        binding: json!({
            "input": asset_model_property(&id_of('1'), &id_of('2')),
            "result": asset_model_property(&id_of('1'), &id_of('3')),
        }),
    };
    let id = create_active(&router, &stored).await;

    for case in refusal_cases() {
        let (status, body) = update(
            &router,
            &id,
            json!({
                "computationModelName": "refused",
                "computationModelConfiguration": case.configuration.clone(),
                "computationModelDataBinding": case.binding.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: the refused update answered {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {:?}: unexpected error code in {body}",
            case.label
        );

        // Nothing moved: the model still reads back the definition it was created
        // with, at the version the create wrote, in the state it was already in.
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read after the refusal answered {status} {described}",
            case.label
        );
        assert_eq!(
            described["computationModelName"].as_str().unwrap_or(""),
            stored.name,
            "case {:?}: the refused update renamed the model: {described}",
            case.label
        );
        assert_description(&described, &stored, case.label);
        assert_eq!(
            described["computationModelConfiguration"], stored.configuration,
            "case {:?}: the refused update rewrote the configuration: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelDataBinding"], stored.binding,
            "case {:?}: the refused update rewrote the data binding: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelVersion"].as_str().unwrap_or(""),
            "1",
            "case {:?}: the refused update counted a version: {described}",
            case.label
        );
        assert_eq!(
            described["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "ACTIVE",
            "case {:?}: the refused update moved the model's state: {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn updating_a_computation_model_no_create_made_is_a_modelled_refusal() {
    let (router, engine) = router_with_engine();

    // Every row is a modelled `ID` the store holds no model under, so the id the
    // request addresses is the only thing that varies.
    for unknown in [id_of('e'), id_of('d'), id_of('7')] {
        let (status, body) = update(
            &router,
            &unknown,
            json!({
                "computationModelName": "nowhere",
                "computationModelConfiguration": anomaly_detection("input", "result"),
                "computationModelDataBinding": {
                    "input": asset_model_property(&id_of('1'), &id_of('2')),
                    "result": asset_model_property(&id_of('1'), &id_of('3')),
                },
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: updating an unknown model answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(&unknown),
            "case {unknown:?}: the refusal does not name the requested id: {body}"
        );
    }

    let stored = service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .computation_models
        .len();
    assert_eq!(
        stored, 0,
        "a refused update wrote a computation model: {stored} stored"
    );
}

#[tokio::test]
async fn a_model_still_settling_an_accepted_call_refuses_a_second_one() {
    let router = router();
    let definition = Definition {
        name: "settling-subject",
        description: None,
        configuration: anomaly_detection("input", "result"),
        binding: json!({
            "input": asset_model_property(&id_of('1'), &id_of('2')),
            "result": asset_model_property(&id_of('1'), &id_of('3')),
        }),
    };

    // Both non-terminal states a mutating call leaves behind: the CREATING the
    // create owes, and the UPDATING an accepted update owes.
    let (status, created) = create(&router, definition.to_json()).await;
    assert_eq!(status, 202, "create answered {status} {created}");
    let id = created["computationModelId"]
        .as_str()
        .expect("a minted computationModelId")
        .to_string();

    for settling in ["CREATING", "UPDATING"] {
        let renamed = Definition {
            name: "settling-subject renamed",
            ..definition.clone()
        };
        let (status, body) = update(&router, &id, renamed.to_json()).await;
        assert_eq!(
            status, 409,
            "case {settling:?}: an update of a settling model answered {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictingOperationException",
            "case {settling:?}: unexpected error code in {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(settling),
            "case {settling:?}: the refusal does not name the state it refused in: {body}"
        );
        assert_eq!(
            body["resourceId"].as_str().unwrap_or(""),
            id,
            "case {settling:?}: the refusal names another resource: {body}"
        );
        assert_eq!(
            body["resourceArn"].as_str().unwrap_or(""),
            expected_arn(&id),
            "case {settling:?}: the refusal names another ARN: {body}"
        );

        // Once the owed transition has fallen due the same call succeeds — which
        // is what leaves the model UPDATING for the next row.
        tokio::time::sleep(settle()).await;
        let (status, accepted) = update(&router, &id, renamed.to_json()).await;
        assert_eq!(
            status, 202,
            "case {settling:?}: the retry once settled answered {status} {accepted}"
        );
        assert_eq!(
            accepted["computationModelStatus"]["state"]
                .as_str()
                .unwrap_or(""),
            "UPDATING",
            "case {settling:?}: the accepted retry reports {accepted}"
        );
    }
}

#[tokio::test]
async fn a_client_token_serves_one_update_and_replays_its_retry() {
    let router = router();
    let definition = Definition {
        name: "token-subject",
        description: None,
        configuration: anomaly_detection("input", "result"),
        binding: json!({
            "input": asset_model_property(&id_of('1'), &id_of('2')),
            "result": asset_model_property(&id_of('1'), &id_of('3')),
        }),
    };
    let id = create_active(&router, &definition).await;
    let token = client_token("update-computation-model");

    let requested = Definition {
        name: "token-subject renamed",
        description: Some("Written once"),
        ..definition.clone()
    };
    let mut body = requested.to_json();
    body["clientToken"] = json!(token);

    let (status, accepted) = update(&router, &id, body.clone()).await;
    assert_eq!(status, 202, "the first update answered {status} {accepted}");

    // The SAME request under the SAME token is a RETRY: it is answered from what
    // the first call left behind and counts no second version. The data binding
    // is a map, so the retry sends its pairs in another order — a retry is the
    // same request however its map was built.
    let mut reordered = body.clone();
    reordered["computationModelDataBinding"] = json!({
        "result": asset_model_property(&id_of('1'), &id_of('3')),
        "input": asset_model_property(&id_of('1'), &id_of('2')),
    });
    let (status, replayed) = update(&router, &id, reordered).await;
    assert_eq!(status, 202, "the retry answered {status} {replayed}");
    assert_eq!(
        replayed["computationModelStatus"]["state"]
            .as_str()
            .unwrap_or(""),
        "UPDATING",
        "the retry reports {replayed}"
    );
    let (status, described) = describe(&router, &id).await;
    assert_eq!(status, 200, "the read after the retry answered {status} {described}");
    assert_eq!(
        described["computationModelVersion"].as_str().unwrap_or(""),
        "2",
        "the retry counted a second version: {described}"
    );
    assert_eq!(
        described["computationModelName"].as_str().unwrap_or(""),
        requested.name,
        "the retry rewrote the model: {described}"
    );

    // Wait out the transition the accepted call owes, so the refusal below is
    // the token's and not the settling state's.
    tokio::time::sleep(settle()).await;

    // A DIFFERENT request reusing that spent token belongs to another call: the
    // modelled conflict, naming the model the first call acted on, and it
    // changes nothing.
    let mut mismatched = body.clone();
    mismatched["computationModelName"] = json!("token-subject renamed again");
    let (status, refused) = update(&router, &id, mismatched).await;
    assert_eq!(
        status, 409,
        "a different request reusing the token answered {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "unexpected error code in {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or("")
            .contains("clientToken"),
        "the refusal does not name the dimension that collided: {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str().unwrap_or(""),
        id,
        "the refusal names another resource: {refused}"
    );
    let (status, unchanged) = describe(&router, &id).await;
    assert_eq!(status, 200, "the read after the refusal answered {status} {unchanged}");
    assert_eq!(
        unchanged["computationModelName"].as_str().unwrap_or(""),
        requested.name,
        "the refused request rewrote the model: {unchanged}"
    );
    assert_eq!(
        unchanged["computationModelVersion"].as_str().unwrap_or(""),
        "2",
        "the refused request counted a version: {unchanged}"
    );

    // A token of its own serves that same different request.
    let mut fresh = body.clone();
    fresh["computationModelName"] = json!("token-subject renamed again");
    fresh["clientToken"] = json!(client_token("update-computation-model-2"));
    let (status, accepted) = update(&router, &id, fresh).await;
    assert_eq!(status, 202, "the update under a fresh token answered {status} {accepted}");
    let (status, described) = describe(&router, &id).await;
    assert_eq!(status, 200, "the read after it answered {status} {described}");
    assert_eq!(
        described["computationModelVersion"].as_str().unwrap_or(""),
        "3",
        "a second accepted update reports {described}"
    );
    assert_eq!(
        described["computationModelName"].as_str().unwrap_or(""),
        "token-subject renamed again",
        "the second accepted update did not land: {described}"
    );

    // A token is scoped to the operation it was sent to, so the value spent on
    // this update does not stop a create from being served under it.
    let mut created_under_token = definition.to_json();
    created_under_token["clientToken"] = json!(token);
    let (status, created) = create(&router, created_under_token).await;
    assert_eq!(
        status, 202,
        "a create under the token this update spent answered {status} {created}"
    );
}
