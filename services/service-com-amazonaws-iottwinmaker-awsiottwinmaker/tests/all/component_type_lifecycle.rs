// SPDX-License-Identifier: CC0-1.0

//! The workspace -> component type lifecycle, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: what a `CreateComponentType` persists is exactly what
//! `GetComponentType` reports, whatever mix of optional members the create
//! carried; the create answers the non-terminal `CREATING` and the clock — not
//! a caller — settles it in `ACTIVE`; a taken id conflicts and leaves the
//! stored record alone; and a component type is part of its workspace's
//! contents.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::TYPE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    COMPONENT_TYPE_SETTLE_SPAN, SCHEMA_INITIALIZER_FUNCTION, STATE_ACTIVE, STATE_CREATING,
    TABULAR_GROUP_TYPE,
};

use crate::harness::{component_type_arn, Emulator, Resp};

/// Block until the `CREATING` -> `ACTIVE` transition a create owes has fallen
/// due.
///
/// The emulator stores the instant and never sleeps for it, so a test that
/// wants to observe the settled state is the one that has to wait; the margin
/// keeps the wait from racing the span's own boundary.
fn wait_until_settled() {
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));
}

/// A `dataType` for one value of the model's `Type` enum.
///
/// `LIST` and `MAP` take a `nestedType` and `RELATIONSHIP` a `relationship`,
/// which is what makes each of those a complete data type rather than a bare
/// tag; the scalar values need neither.
fn data_type(type_value: &str) -> Value {
    match type_value {
        "LIST" | "MAP" => json!({"type": type_value, "nestedType": {"type": "STRING"}}),
        "RELATIONSHIP" => json!({
            "type": type_value,
            "relationship": {
                "targetComponentTypeId": "com.example.target",
                "relationshipType": "belongsTo",
            },
        }),
        _ => json!({"type": type_value}),
    }
}

/// The `status.state` a read reports for a component type.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("status")["state"].as_str()
}

/// RULE: a component type read back reports exactly the members its create
/// persisted — the ones the create named, and the resolved value of the ones it
/// did not — under the ARN the create minted.
///
/// The rows differ along which optional members the create carries and along
/// the shape of the id, because that is what the rule generalises over.
#[tokio::test]
async fn a_component_type_reads_back_exactly_what_was_created() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("ctWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    let rows: Vec<(&str, Value)> = vec![
        ("bare", json!({})),
        (
            "named.and.described",
            json!({"componentTypeName": "A sensor", "description": "a described type"}),
        ),
        (
            "singleton-type",
            json!({"isSingleton": true, "description": ""}),
        ),
        (
            "com.example.withProperties",
            json!({
                "propertyDefinitions": {
                    "serial": {
                        "dataType": {"type": "STRING"},
                        "isExternalId": true,
                        "displayName": "Serial number",
                    },
                    "temperature": {
                        "dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"},
                        "isTimeSeries": true,
                        "isRequiredInEntity": false,
                    },
                    "limit": {
                        "dataType": {"type": "INTEGER"},
                        "defaultValue": {"integerValue": 42},
                    },
                },
            }),
        ),
        (
            "com.example.withGroupsAndFunctions",
            json!({
                "propertyDefinitions": {
                    "alarmKey": {"dataType": {"type": "STRING"}},
                    "alarmStatus": {"dataType": {"type": "STRING"}},
                },
                "propertyGroups": {
                    "tabularGroup": {
                        "groupType": TABULAR_GROUP_TYPE,
                        "propertyNames": ["alarmKey", "alarmStatus"],
                    },
                    "groupWithoutType": {"propertyNames": ["alarmKey"]},
                },
                "functions": {
                    "dataReader": {
                        "scope": "ENTITY",
                        "requiredProperties": ["alarmKey"],
                        "implementedBy": {
                            "lambda": {
                                "arn": "arn:aws:lambda:us-west-2:000000000000:function:reader",
                            },
                        },
                    },
                },
            }),
        ),
        (
            "com.example.derived",
            json!({
                "extendsFrom": ["com.amazonaws.iottwinmaker.alarm.basic", "com.example.base"],
                "componentTypeName": "Derived",
                "compositeComponentTypes": {
                    "inner": {"componentTypeId": "com.example.inner"},
                },
            }),
        ),
        (
            "tagged-type",
            json!({"tags": {"team": "plant", "env": "test"}}),
        ),
    ];

    for (component_type_id, request) in &rows {
        let created = emulator
            .create_component_type("ctWorkspace", component_type_id, request.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: expected 200 from CreateComponentType, got {created:?}"
        );
        assert_eq!(
            created.member("arn").as_str(),
            Some(component_type_arn("ctWorkspace", component_type_id).as_str()),
            "case {component_type_id:?}: expected the minted ARN, got {created:?}"
        );
        assert_eq!(
            created.member("state").as_str(),
            Some(STATE_CREATING),
            "case {component_type_id:?}: a create answers the non-terminal state, got {created:?}"
        );
        let creation_date_time = created.member("creationDateTime").as_f64();
        assert!(
            creation_date_time.is_some_and(|seconds| seconds > 0.0),
            "case {component_type_id:?}: expected an epoch-seconds creationDateTime, \
             got {created:?}"
        );

        let read = emulator
            .get_component_type("ctWorkspace", component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {component_type_id:?}: expected 200 from GetComponentType, got {read:?}"
        );
        assert_eq!(
            (
                read.member("workspaceId").as_str(),
                read.member("componentTypeId").as_str(),
                read.member("arn").as_str(),
            ),
            (
                Some("ctWorkspace"),
                Some(*component_type_id),
                Some(component_type_arn("ctWorkspace", component_type_id).as_str()),
            ),
            "case {component_type_id:?}: expected the created identity, got {read:?}"
        );
        assert_eq!(
            read.member("creationDateTime").as_f64(),
            creation_date_time,
            "case {component_type_id:?}: the read must report the create's own instant, \
             got {read:?}"
        );
        assert_eq!(
            read.member("updateDateTime").as_f64(),
            creation_date_time,
            "case {component_type_id:?}: nothing updated it, so updateDateTime is the \
             creation instant, got {read:?}"
        );

        // Every member the create named is reported back verbatim; a member it
        // did not name is reported resolved (`isSingleton` false, the
        // collections empty) rather than absent.
        assert_eq!(
            read.member("componentTypeName").as_str(),
            request["componentTypeName"].as_str(),
            "case {component_type_id:?}: expected the created componentTypeName, got {read:?}"
        );
        assert_eq!(
            read.member("description").as_str(),
            request["description"].as_str(),
            "case {component_type_id:?}: expected the created description, got {read:?}"
        );
        assert_eq!(
            read.member("isSingleton").as_bool(),
            Some(request["isSingleton"].as_bool().unwrap_or(false)),
            "case {component_type_id:?}: expected the created isSingleton, got {read:?}"
        );
        assert_eq!(
            read.member("extendsFrom"),
            &json!(request["extendsFrom"]
                .as_array()
                .cloned()
                .unwrap_or_default()),
            "case {component_type_id:?}: expected the created extendsFrom, got {read:?}"
        );
        assert_eq!(
            read.member("isAbstract").as_bool(),
            Some(false),
            "case {component_type_id:?}: a component type created through this API is \
             concrete, got {read:?}"
        );
        assert_eq!(
            reported_state(&read),
            Some(STATE_CREATING),
            "case {component_type_id:?}: a read in the settle span still reports CREATING, \
             got {read:?}"
        );
        assert_eq!(
            read.member("status")["error"],
            json!({}),
            "case {component_type_id:?}: a status carries its error structure, got {read:?}"
        );

        // The property definitions: every name the create declared, each with
        // the dataType it declared and the resolved value of each flag.
        let declared = request["propertyDefinitions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let reported = read
            .member("propertyDefinitions")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.keys().collect::<BTreeSet<_>>(),
            declared.keys().collect::<BTreeSet<_>>(),
            "case {component_type_id:?}: expected the created property names, got {read:?}"
        );
        for (property_name, definition) in &declared {
            let response = &reported[property_name];
            assert_eq!(
                &response["dataType"], &definition["dataType"],
                "case {component_type_id:?}/{property_name}: expected the created dataType, \
                 got {response:?}"
            );
            for flag in [
                "isTimeSeries",
                "isRequiredInEntity",
                "isExternalId",
                "isStoredExternally",
            ] {
                assert_eq!(
                    response[flag].as_bool(),
                    Some(definition[flag].as_bool().unwrap_or(false)),
                    "case {component_type_id:?}/{property_name}: expected the created {flag}, \
                     got {response:?}"
                );
            }
            for inherited in ["isImported", "isFinal", "isInherited"] {
                assert_eq!(
                    response[inherited].as_bool(),
                    Some(false),
                    "case {component_type_id:?}/{property_name}: a definition written by this \
                     create came from nowhere else, got {response:?}"
                );
            }
            assert_eq!(
                response["defaultValue"], definition["defaultValue"],
                "case {component_type_id:?}/{property_name}: expected the created defaultValue, \
                 got {response:?}"
            );
            assert_eq!(
                response["displayName"], definition["displayName"],
                "case {component_type_id:?}/{property_name}: expected the created displayName, \
                 got {response:?}"
            );
        }

        // The functions, reported with the implementation the create gave them.
        let declared_functions = request["functions"].as_object().cloned().unwrap_or_default();
        let reported_functions = read.member("functions").as_object().cloned().unwrap_or_default();
        assert_eq!(
            reported_functions.keys().collect::<BTreeSet<_>>(),
            declared_functions.keys().collect::<BTreeSet<_>>(),
            "case {component_type_id:?}: expected the created function names, got {read:?}"
        );
        for (function_name, function) in &declared_functions {
            let response = &reported_functions[function_name];
            assert_eq!(
                (&response["scope"], &response["implementedBy"]),
                (&function["scope"], &function["implementedBy"]),
                "case {component_type_id:?}/{function_name}: expected the created function, \
                 got {response:?}"
            );
            assert_eq!(
                response["isInherited"].as_bool(),
                Some(false),
                "case {component_type_id:?}/{function_name}: this function is the type's own, \
                 got {response:?}"
            );
        }

        // The property groups. `groupType` is optional on the request and
        // required on the response, so a group created without one reports the
        // only value the model's `GroupType` enum admits.
        let declared_groups = request["propertyGroups"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let reported_groups = read
            .member("propertyGroups")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported_groups.keys().collect::<BTreeSet<_>>(),
            declared_groups.keys().collect::<BTreeSet<_>>(),
            "case {component_type_id:?}: expected the created group names, got {read:?}"
        );
        for (group_name, group) in &declared_groups {
            let response = &reported_groups[group_name];
            assert_eq!(
                response["groupType"].as_str(),
                Some(group["groupType"].as_str().unwrap_or(TABULAR_GROUP_TYPE)),
                "case {component_type_id:?}/{group_name}: expected the created groupType, \
                 got {response:?}"
            );
            assert_eq!(
                response["propertyNames"], group["propertyNames"],
                "case {component_type_id:?}/{group_name}: expected the created propertyNames, \
                 got {response:?}"
            );
        }

        // The composite component types, each reporting the id it references.
        let declared_composites = request["compositeComponentTypes"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let reported_composites = read
            .member("compositeComponentTypes")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported_composites.keys().collect::<BTreeSet<_>>(),
            declared_composites.keys().collect::<BTreeSet<_>>(),
            "case {component_type_id:?}: expected the created composite names, got {read:?}"
        );
        for (composite_name, composite) in &declared_composites {
            assert_eq!(
                reported_composites[composite_name]["componentTypeId"],
                composite["componentTypeId"],
                "case {component_type_id:?}/{composite_name}: expected the created \
                 componentTypeId, got {read:?}"
            );
        }
    }
}

/// RULE: a property definition round-trips for EVERY value the model's `Type`
/// enum defines — the catalog is `ops::types::TYPE_VALUES`, generated from the
/// model's `com.amazonaws.iottwinmaker#Type` enum, so a value added to the
/// model is a row here without this test changing.
#[tokio::test]
async fn every_modelled_data_type_round_trips() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("typeWorkspace").await.status,
        200,
        "the workspace the catalog is created in must exist"
    );
    assert!(
        TYPE_VALUES.len() >= 8,
        "the model's Type enum must still carry its values, got {TYPE_VALUES:?}"
    );

    let mut definitions = serde_json::Map::new();
    for type_value in TYPE_VALUES {
        definitions.insert(
            format!("prop_{type_value}"),
            json!({"dataType": data_type(type_value)}),
        );
    }
    let created = emulator
        .create_component_type(
            "typeWorkspace",
            "com.example.allTypes",
            json!({"propertyDefinitions": Value::Object(definitions)}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"allTypes\": expected 200 from CreateComponentType, got {created:?}"
    );

    let read = emulator
        .get_component_type("typeWorkspace", "com.example.allTypes")
        .await;
    assert_eq!(
        read.status, 200,
        "case \"allTypes\": expected 200 from GetComponentType, got {read:?}"
    );
    for type_value in TYPE_VALUES {
        let reported = &read.member("propertyDefinitions")[format!("prop_{type_value}")];
        assert_eq!(
            &reported["dataType"],
            &data_type(type_value),
            "case {type_value:?}: expected the created dataType, got {reported:?}"
        );
    }
}

/// RULE: a create answers the non-terminal `CREATING`, and the CLOCK settles
/// the component type in `ACTIVE` once the settle span has elapsed — no caller
/// has to poll for the transition and no read undoes it.
#[tokio::test]
async fn a_component_type_settles_in_active_once_the_clock_makes_it_due() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("settleWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The rows differ in whether a reader observed the component type during
    // the settle span, because the transition is the clock's and not a read's.
    let rows = [("polled", true), ("unpolled", false)];
    for (case, poll_while_creating) in rows {
        let created = emulator
            .create_component_type("settleWorkspace", case, json!({}))
            .await;
        assert_eq!(
            created.member("state").as_str(),
            Some(STATE_CREATING),
            "case {case:?}: a create answers CREATING, got {created:?}"
        );
        if poll_while_creating {
            let during = emulator.get_component_type("settleWorkspace", case).await;
            assert_eq!(
                reported_state(&during),
                Some(STATE_CREATING),
                "case {case:?}: a read inside the settle span still reports CREATING, \
                 got {during:?}"
            );
        }
    }

    wait_until_settled();

    for (case, _) in rows {
        let settled = emulator.get_component_type("settleWorkspace", case).await;
        assert_eq!(
            reported_state(&settled),
            Some(STATE_ACTIVE),
            "case {case:?}: the settle span has elapsed, so the state is ACTIVE, got {settled:?}"
        );
        let again = emulator.get_component_type("settleWorkspace", case).await;
        assert_eq!(
            reported_state(&again),
            Some(STATE_ACTIVE),
            "case {case:?}: ACTIVE is terminal, so a second read reports it too, got {again:?}"
        );
        assert_eq!(
            again.member("creationDateTime").as_f64(),
            settled.member("creationDateTime").as_f64(),
            "case {case:?}: settling is not an update, so the instants do not move, got {again:?}"
        );
    }
}

/// RULE: `isSchemaInitialized` is true exactly when the component type carries
/// a schema-initializer function AND that initializer has run — which is once
/// the create has settled. (The member's own documentation: "whether the
/// component type has a schema initializer and that the schema initializer has
/// run".)
#[tokio::test]
async fn is_schema_initialized_reports_a_schema_initializer_that_has_run() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("schemaWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    let initializer = json!({
        SCHEMA_INITIALIZER_FUNCTION: {
            "scope": "ENTITY",
            "implementedBy": {"isNative": true},
        },
    });
    // The rows differ in which functions the create declares: none, one that is
    // not the schema initializer, and the schema initializer itself.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("noFunctions", json!({}), false),
        (
            "otherFunction",
            json!({"functions": {"dataReader": {"scope": "ENTITY"}}}),
            false,
        ),
        (
            "schemaInitializer",
            json!({"functions": initializer}),
            true,
        ),
    ];

    for (case, request, _) in &rows {
        assert_eq!(
            emulator
                .create_component_type("schemaWorkspace", case, request.clone())
                .await
                .status,
            200,
            "case {case:?}: expected 200 from CreateComponentType"
        );
        let during = emulator.get_component_type("schemaWorkspace", case).await;
        assert_eq!(
            during.member("isSchemaInitialized").as_bool(),
            Some(false),
            "case {case:?}: an initializer has not run while the create is still CREATING, \
             got {during:?}"
        );
    }

    wait_until_settled();

    for (case, _, expected) in &rows {
        let settled = emulator.get_component_type("schemaWorkspace", case).await;
        assert_eq!(
            settled.member("isSchemaInitialized").as_bool(),
            Some(*expected),
            "case {case:?}: expected isSchemaInitialized {expected}, got {settled:?}"
        );
    }
}

/// RULE: a `componentTypeId` already taken in the workspace conflicts, and the
/// refused create leaves the stored component type exactly as it was.
#[tokio::test]
async fn a_taken_component_type_id_conflicts_and_changes_nothing() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("conflictWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The rows differ in how much the second create carries, because the id —
    // not the body — is what conflicts.
    let rows: Vec<(&str, Value)> = vec![
        ("sameBody", json!({"description": "the first one"})),
        ("emptyBody", json!({})),
        (
            "richerBody",
            json!({
                "description": "the second one",
                "componentTypeName": "Second",
                "propertyDefinitions": {"extra": {"dataType": {"type": "STRING"}}},
            }),
        ),
    ];
    assert_eq!(
        emulator
            .create_component_type(
                "conflictWorkspace",
                "com.example.taken",
                json!({"description": "the first one"}),
            )
            .await
            .status,
        200,
        "the component type every row re-creates must exist first"
    );

    for (case, request) in &rows {
        let again = emulator
            .create_component_type("conflictWorkspace", "com.example.taken", request.clone())
            .await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: re-creating a taken id must conflict, got {again:?}"
        );
        let read = emulator
            .get_component_type("conflictWorkspace", "com.example.taken")
            .await;
        assert_eq!(
            read.member("description").as_str(),
            Some("the first one"),
            "case {case:?}: a refused create must not overwrite the stored type, got {read:?}"
        );
        assert_eq!(
            read.member("propertyDefinitions"),
            &json!({}),
            "case {case:?}: a refused create must persist nothing, got {read:?}"
        );
    }
}

/// RULE: a request that names something this service does not hold, or a
/// property definition it could never report back, is refused with the modelled
/// error for that refusal — and the refusal says why.
#[tokio::test]
async fn a_component_type_request_the_service_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("refusalWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    assert_eq!(
        emulator
            .create_component_type("refusalWorkspace", "com.example.present", json!({}))
            .await
            .status,
        200,
        "the component type the found case reads must exist"
    );

    // A create into a workspace that does not exist is a ValidationException,
    // not a 404: CreateComponentType models no ResourceNotFoundException.
    let missing_workspace = emulator
        .create_component_type("absentWorkspace", "com.example.orphan", json!({}))
        .await;
    assert_eq!(
        (
            missing_workspace.status,
            missing_workspace.error_type.as_deref()
        ),
        (400, Some("ValidationException")),
        "case \"absent workspace\": expected the modelled refusal, got {missing_workspace:?}"
    );
    assert!(
        missing_workspace.member("message").is_string(),
        "case \"absent workspace\": a refusal must say why, got {missing_workspace:?}"
    );

    // A read of either resource this service does not hold is a 404.
    for (case, workspace_id, component_type_id) in [
        ("absent workspace", "absentWorkspace", "com.example.present"),
        ("absent component type", "refusalWorkspace", "com.example.absent"),
    ] {
        let read = emulator
            .get_component_type(workspace_id, component_type_id)
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404, got {read:?}"
        );
        assert!(
            read.member("message").is_string(),
            "case {case:?}: a refusal must say why, got {read:?}"
        );
    }

    // `dataType` is required on the definition a read reports, so a definition
    // that omits it is refused unless the component type has a parent to
    // inherit one from.
    let definitions = json!({"propertyDefinitions": {"untyped": {"isTimeSeries": true}}});
    let refused = emulator
        .create_component_type("refusalWorkspace", "com.example.untyped", definitions.clone())
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"no dataType, no parent\": expected the modelled refusal, got {refused:?}"
    );
    assert!(
        refused
            .member("message")
            .as_str()
            .is_some_and(|message| message.contains("untyped")),
        "case \"no dataType, no parent\": the refusal must name the member, got {refused:?}"
    );
    let gone = emulator
        .get_component_type("refusalWorkspace", "com.example.untyped")
        .await;
    assert_eq!(
        gone.status, 404,
        "case \"no dataType, no parent\": a refused create persists nothing, got {gone:?}"
    );

    let mut inheriting = definitions;
    inheriting["extendsFrom"] = json!(["com.example.present"]);
    let accepted = emulator
        .create_component_type("refusalWorkspace", "com.example.inheriting", inheriting)
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"no dataType, with a parent\": the definition may refine an inherited one, \
         got {accepted:?}"
    );
}

/// RULE: a component type is part of its workspace's contents — the workspace
/// that holds one is not deletable, and every component type created in a
/// workspace is reachable under its own id.
#[tokio::test]
async fn a_workspace_that_still_holds_component_types_is_not_deleted() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("contents").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The rows differ in how many component types the workspace holds.
    for count in 1..=3u32 {
        let component_type_id = format!("com.example.held{count}");
        assert_eq!(
            emulator
                .create_component_type("contents", &component_type_id, json!({}))
                .await
                .status,
            200,
            "case {count}: expected 200 from CreateComponentType"
        );
        let refused = emulator.delete_workspace("contents").await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {count}: a workspace holding component types must not be deletable, \
             got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(&count.to_string())),
            "case {count}: the refusal must count what the workspace still holds, got {refused:?}"
        );
        for held in 1..=count {
            let read = emulator
                .get_component_type("contents", &format!("com.example.held{held}"))
                .await;
            assert_eq!(
                read.status, 200,
                "case {count}: component type {held} must still be readable, got {read:?}"
            );
        }
    }
}
