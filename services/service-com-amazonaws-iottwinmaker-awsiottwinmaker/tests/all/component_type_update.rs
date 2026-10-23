// SPDX-License-Identifier: CC0-1.0

//! `UpdateComponentType`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`,
//! the same entry the served emulator uses — so a route, a label, a status code
//! and a serialized body are all exercised, not just a handler body. What the
//! update persisted is read back through `GetComponentType`, so the suite
//! proves the store changed rather than that one response was assembled.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: an update PATCHES its top-level members — every member it names
//! REPLACES the stored value for that member whole (a map member included, so
//! an entry the request leaves out is dropped) and every member it does not
//! name keeps the value already persisted; the update answers the non-terminal
//! `UPDATING` and the CLOCK settles the component type back in `ACTIVE`; a
//! label naming something the store does not hold is a 404; and a patch that
//! would leave a definition no read can report is refused with the stored
//! record untouched.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    COMPONENT_TYPE_SETTLE_SPAN, STATE_ACTIVE, STATE_UPDATING, TABULAR_GROUP_TYPE,
};

use crate::harness::{component_type_arn, Emulator, Resp};

/// The workspace every case in this suite works in. Each test builds its own
/// emulator, so the id is shared without the cases sharing state.
const WORKSPACE: &str = "updateCtWorkspace";

/// Block until the transition a create or an update owes has fallen due.
///
/// The emulator stores the instant and never sleeps for it, so a test that
/// wants to observe the settled state is the one that has to wait; the margin
/// keeps the wait from racing the span's own boundary.
fn wait_until_settled() {
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));
}

/// The create every patch case starts from: a component type carrying one of
/// every member an update can name, so each case can assert both what its
/// patch changed and what it left alone.
fn baseline() -> Value {
    json!({
        "componentTypeName": "Baseline sensor",
        "description": "the description the create persisted",
        "isSingleton": false,
        "propertyDefinitions": {
            "serial": {
                "dataType": {"type": "STRING"},
                "isExternalId": true,
                "displayName": "Serial",
            },
            "temperature": {
                "dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"},
                "isTimeSeries": true,
            },
        },
        "functions": {
            "dataReader": {"scope": "ENTITY", "requiredProperties": ["serial"]},
        },
        "propertyGroups": {
            "readings": {"groupType": TABULAR_GROUP_TYPE, "propertyNames": ["serial"]},
        },
        "compositeComponentTypes": {
            "inlet": {"componentTypeId": "com.example.inlet"},
        },
    })
}

/// An emulator holding the one workspace the cases are created in.
async fn baseline_emulator() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    emulator
}

/// The `status.state` a read reports for a component type.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("status")["state"].as_str()
}

/// One numeric member of a response — the epoch-second instants this service
/// reports — or a failure naming the case that did not carry it.
fn numeric(resp: &Resp, name: &str, case: &str) -> f64 {
    match resp.member(name).as_f64() {
        Some(value) => value,
        None => panic!("case {case:?}: expected a numeric {name}, got {resp:?}"),
    }
}

/// One patch case: its name, the request body it sends, and the `(JSON
/// pointer, value)` pairs the read afterwards must report — `None` where the
/// patch must have left nothing at that path at all.
type PatchCase = (&'static str, Value, Vec<(&'static str, Option<Value>)>);

/// RULE: an update replaces the stored value of every member the request
/// NAMES, whole, and leaves every member it does not name exactly as it was
/// persisted — so an entry missing from a map the request names is dropped,
/// while a map the request does not name keeps all of its entries.
///
/// The rows differ along which member the patch names and along that member's
/// shape — nothing at all, a string, an emptied string, a boolean, a list, and
/// each of the four maps replaced, narrowed or grown — because that is what
/// the rule generalises over.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_keeps_the_rest() {
    let emulator = baseline_emulator().await;

    // (case, the patch, the members the read must report afterwards)
    let rows: Vec<PatchCase> = vec![
        (
            "names.nothing",
            json!({}),
            vec![
                ("/componentTypeName", Some(json!("Baseline sensor"))),
                (
                    "/description",
                    Some(json!("the description the create persisted")),
                ),
                ("/isSingleton", Some(json!(false))),
                (
                    "/propertyDefinitions/serial/displayName",
                    Some(json!("Serial")),
                ),
                ("/functions/dataReader/scope", Some(json!("ENTITY"))),
                (
                    "/propertyGroups/readings/propertyNames",
                    Some(json!(["serial"])),
                ),
                (
                    "/compositeComponentTypes/inlet/componentTypeId",
                    Some(json!("com.example.inlet")),
                ),
            ],
        ),
        (
            "description.replaced",
            json!({"description": "the description the update persisted"}),
            vec![
                (
                    "/description",
                    Some(json!("the description the update persisted")),
                ),
                ("/componentTypeName", Some(json!("Baseline sensor"))),
            ],
        ),
        (
            "description.emptied",
            // An empty string is a value the request NAMED, not an omission:
            // `Description` is `@length(min: 0)`, so clearing it is legal.
            json!({"description": ""}),
            vec![
                ("/description", Some(json!(""))),
                ("/componentTypeName", Some(json!("Baseline sensor"))),
            ],
        ),
        (
            "componentTypeName.replaced",
            json!({"componentTypeName": "Patched sensor"}),
            vec![
                ("/componentTypeName", Some(json!("Patched sensor"))),
                (
                    "/description",
                    Some(json!("the description the create persisted")),
                ),
            ],
        ),
        (
            "isSingleton.flipped",
            json!({"isSingleton": true}),
            vec![
                ("/isSingleton", Some(json!(true))),
                (
                    "/propertyDefinitions/temperature/dataType/type",
                    Some(json!("DOUBLE")),
                ),
            ],
        ),
        (
            "extendsFrom.replaced",
            json!({"extendsFrom": ["com.example.parent", "com.example.other"]}),
            vec![
                (
                    "/extendsFrom",
                    Some(json!(["com.example.parent", "com.example.other"])),
                ),
                (
                    "/propertyDefinitions/serial/dataType/type",
                    Some(json!("STRING")),
                ),
            ],
        ),
        (
            "propertyDefinitions.replaced",
            // The map the request carries is the map the component type ends up
            // with: `serial` and `temperature` were not restated, so they are
            // gone.
            json!({
                "propertyDefinitions": {
                    "pressure": {"dataType": {"type": "DOUBLE", "unitOfMeasure": "kPa"}},
                },
            }),
            vec![
                (
                    "/propertyDefinitions/pressure/dataType/unitOfMeasure",
                    Some(json!("kPa")),
                ),
                ("/propertyDefinitions/serial", None),
                ("/propertyDefinitions/temperature", None),
                // A map the patch did NOT name is untouched.
                ("/functions/dataReader/scope", Some(json!("ENTITY"))),
            ],
        ),
        (
            "propertyDefinitions.narrowed",
            // One of the two definitions restated, the other dropped: the only
            // way this API can shorten the map.
            json!({
                "propertyDefinitions": {
                    "serial": {
                        "dataType": {"type": "STRING"},
                        "isExternalId": true,
                        "displayName": "Serial number",
                    },
                },
            }),
            vec![
                (
                    "/propertyDefinitions/serial/displayName",
                    Some(json!("Serial number")),
                ),
                ("/propertyDefinitions/serial/isExternalId", Some(json!(true))),
                ("/propertyDefinitions/temperature", None),
            ],
        ),
        (
            "propertyDefinitions.grown",
            // Restating both and adding a third keeps all three.
            json!({
                "propertyDefinitions": {
                    "serial": {"dataType": {"type": "STRING"}},
                    "temperature": {"dataType": {"type": "INTEGER"}},
                    "pressure": {"dataType": {"type": "DOUBLE"}},
                },
            }),
            vec![
                (
                    "/propertyDefinitions/serial/dataType/type",
                    Some(json!("STRING")),
                ),
                (
                    "/propertyDefinitions/temperature/dataType/type",
                    Some(json!("INTEGER")),
                ),
                (
                    "/propertyDefinitions/pressure/dataType/type",
                    Some(json!("DOUBLE")),
                ),
                // A restated definition is replaced, not merged: the members it
                // does not carry take their resolved values, not the ones the
                // create gave them.
                (
                    "/propertyDefinitions/serial/isExternalId",
                    Some(json!(false)),
                ),
                (
                    "/propertyDefinitions/temperature/isTimeSeries",
                    Some(json!(false)),
                ),
            ],
        ),
        (
            "functions.replaced",
            json!({
                "functions": {
                    "schemaInitializer": {"scope": "ENTITY", "implementedBy": {"isNative": true}},
                },
            }),
            vec![
                (
                    "/functions/schemaInitializer/implementedBy/isNative",
                    Some(json!(true)),
                ),
                ("/functions/dataReader", None),
                (
                    "/propertyDefinitions/serial/dataType/type",
                    Some(json!("STRING")),
                ),
            ],
        ),
        (
            "propertyGroups.replaced",
            json!({"propertyGroups": {"spares": {"propertyNames": ["serial"]}}}),
            vec![
                (
                    "/propertyGroups/spares/propertyNames",
                    Some(json!(["serial"])),
                ),
                // `groupType` was not named, so the group reports the only value
                // the model's `GroupType` enum admits.
                (
                    "/propertyGroups/spares/groupType",
                    Some(json!(TABULAR_GROUP_TYPE)),
                ),
                ("/propertyGroups/readings", None),
            ],
        ),
        (
            "compositeComponentTypes.replaced",
            json!({
                "compositeComponentTypes": {
                    "outlet": {"componentTypeId": "com.example.outlet"},
                },
            }),
            vec![
                (
                    "/compositeComponentTypes/outlet/componentTypeId",
                    Some(json!("com.example.outlet")),
                ),
                ("/compositeComponentTypes/inlet", None),
            ],
        ),
        (
            "many.membersAtOnce",
            json!({
                "componentTypeName": "Everything patched",
                "isSingleton": true,
                "propertyDefinitions": {
                    "serial": {"dataType": {"type": "STRING"}, "isRequiredInEntity": true},
                },
                "propertyGroups": {
                    "readings": {"groupType": TABULAR_GROUP_TYPE, "propertyNames": ["serial"]},
                },
            }),
            vec![
                ("/componentTypeName", Some(json!("Everything patched"))),
                ("/isSingleton", Some(json!(true))),
                (
                    "/propertyDefinitions/serial/isRequiredInEntity",
                    Some(json!(true)),
                ),
                ("/propertyDefinitions/temperature", None),
                (
                    "/propertyGroups/readings/propertyNames",
                    Some(json!(["serial"])),
                ),
                // Neither named by the patch nor dropped by it.
                (
                    "/description",
                    Some(json!("the description the create persisted")),
                ),
                ("/functions/dataReader/scope", Some(json!("ENTITY"))),
            ],
        ),
    ];

    for (case, patch, expectations) in rows {
        let created = emulator
            .create_component_type(WORKSPACE, case, baseline())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateComponentType, got {created:?}"
        );

        let updated = emulator
            .update_component_type(WORKSPACE, case, patch.clone())
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateComponentType, got {updated:?}"
        );
        assert_eq!(
            updated.member("workspaceId").as_str(),
            Some(WORKSPACE),
            "case {case:?}: the update echoes the workspace it addressed, got {updated:?}"
        );
        assert_eq!(
            updated.member("componentTypeId").as_str(),
            Some(case),
            "case {case:?}: the update echoes the component type it addressed, got {updated:?}"
        );
        assert_eq!(
            updated.member("arn").as_str(),
            Some(component_type_arn(WORKSPACE, case).as_str()),
            "case {case:?}: the update reports the ARN the create minted, got {updated:?}"
        );

        let read = emulator.get_component_type(WORKSPACE, case).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetComponentType, got {read:?}"
        );
        for (path, expected) in &expectations {
            let actual = read.body.pointer(path);
            assert_eq!(
                actual,
                expected.as_ref(),
                "case {case:?}: expected {path} to be {expected:?}, got {actual:?} in {read:?}"
            );
        }
        // Patching never moves what addresses the resource.
        assert_eq!(
            read.member("arn").as_str(),
            Some(component_type_arn(WORKSPACE, case).as_str()),
            "case {case:?}: a patch does not re-mint the ARN, got {read:?}"
        );
    }
}

/// RULE: an update answers the non-terminal `UPDATING`, and the CLOCK settles
/// the component type back in `ACTIVE` once the settle span has elapsed — no
/// caller has to poll for the transition, and the update moves
/// `updateDateTime` forward while leaving `creationDateTime` where the create
/// put it.
#[tokio::test]
async fn an_update_settles_back_in_active_once_the_clock_makes_it_due() {
    let emulator = baseline_emulator().await;

    // The rows differ in whether a reader observed the component type during
    // the settle span, because the transition is the clock's and not a read's.
    let rows = [("polled", true), ("unpolled", false)];
    for (case, _) in rows {
        assert_eq!(
            emulator
                .create_component_type(WORKSPACE, case, json!({}))
                .await
                .status,
            200,
            "case {case:?}: expected 200 from CreateComponentType"
        );
    }

    // Start from a settled component type, so what the update reports is the
    // update's own transition and not the create's.
    wait_until_settled();

    let mut created_at: Vec<f64> = Vec::new();
    for (case, poll_while_updating) in rows {
        let before = emulator.get_component_type(WORKSPACE, case).await;
        assert_eq!(
            reported_state(&before),
            Some(STATE_ACTIVE),
            "case {case:?}: the create has settled, so the update starts from ACTIVE, \
             got {before:?}"
        );
        created_at.push(numeric(&before, "creationDateTime", case));

        let updated = emulator
            .update_component_type(WORKSPACE, case, json!({"description": "patched"}))
            .await;
        assert_eq!(
            updated.member("state").as_str(),
            Some(STATE_UPDATING),
            "case {case:?}: an update in flight answers UPDATING, got {updated:?}"
        );
        if poll_while_updating {
            let during = emulator.get_component_type(WORKSPACE, case).await;
            assert_eq!(
                reported_state(&during),
                Some(STATE_UPDATING),
                "case {case:?}: a read inside the settle span still reports UPDATING, \
                 got {during:?}"
            );
        }
    }

    wait_until_settled();

    for (&(case, _), creation_date_time) in rows.iter().zip(created_at) {
        let settled = emulator.get_component_type(WORKSPACE, case).await;
        assert_eq!(
            reported_state(&settled),
            Some(STATE_ACTIVE),
            "case {case:?}: the settle span has elapsed, so the state is ACTIVE, got {settled:?}"
        );
        let settled_creation = numeric(&settled, "creationDateTime", case);
        assert_eq!(
            settled_creation, creation_date_time,
            "case {case:?}: an update is not a create, so the creation instant stands, \
             got {settled:?}"
        );
        let update_date_time = numeric(&settled, "updateDateTime", case);
        assert!(
            update_date_time > creation_date_time,
            "case {case:?}: the update moved the update instant past the creation instant, \
             got {update_date_time:?} against {creation_date_time:?}"
        );
        let again = emulator.get_component_type(WORKSPACE, case).await;
        assert_eq!(
            (
                reported_state(&again),
                numeric(&again, "updateDateTime", case)
            ),
            (Some(STATE_ACTIVE), update_date_time),
            "case {case:?}: settling is not an update, so neither the state nor the instant \
             moves again, got {again:?}"
        );
    }
}

/// RULE: both of this operation's labels address a resource that must already
/// exist, so a label naming something the store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) and nothing is created by the
/// attempt.
#[tokio::test]
async fn an_update_of_something_that_does_not_exist_is_a_404() {
    let emulator = baseline_emulator().await;
    assert_eq!(
        emulator
            .create_component_type(WORKSPACE, "com.example.present", json!({}))
            .await
            .status,
        200,
        "the component type the present case patches must exist"
    );

    // The rows differ in WHICH label is unresolvable: the workspace, or the
    // component type inside a workspace that does exist.
    let rows = [
        ("absent workspace", "absentWorkspace", "com.example.present"),
        ("absent component type", WORKSPACE, "com.example.absent"),
    ];
    for (case, workspace_id, component_type_id) in rows {
        let refused = emulator
            .update_component_type(
                workspace_id,
                component_type_id,
                json!({"description": "patched"}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404, got {refused:?}"
        );
        assert!(
            refused.member("message").is_string(),
            "case {case:?}: a refusal must say why, got {refused:?}"
        );
        // The refusal created nothing: the resource is still absent.
        let read = emulator
            .get_component_type(workspace_id, component_type_id)
            .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused update must not have created anything, got {read:?}"
        );
    }
}

/// RULE: a patch that would leave the component type holding a property
/// definition no read can report — one with no `dataType` and nothing to
/// inherit one from — is refused with the modelled `ValidationException` (HTTP
/// 400), and the stored record is left exactly as it was.
#[tokio::test]
async fn a_patch_that_would_leave_an_unreportable_definition_is_refused() {
    let emulator = baseline_emulator().await;

    // The rows differ in HOW the patch would strand a definition: a map entry
    // carrying no `dataType`, a restated entry that dropped the `dataType` the
    // create gave it, and a patch that takes away the `extendsFrom` an untyped
    // definition was relying on.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "new entry, nothing to inherit from",
            json!({"propertyDefinitions": {"serial": {"dataType": {"type": "STRING"}}}}),
            json!({"propertyDefinitions": {"untyped": {"isTimeSeries": true}}}),
        ),
        (
            "restated entry that dropped its dataType",
            json!({"propertyDefinitions": {"serial": {"dataType": {"type": "STRING"}}}}),
            json!({"propertyDefinitions": {"serial": {"displayName": "Serial number"}}}),
        ),
        (
            "extendsFrom cleared under an untyped definition",
            json!({
                "extendsFrom": ["com.example.parent"],
                "propertyDefinitions": {"inheritedOnly": {"displayName": "Inherited"}},
            }),
            json!({"extendsFrom": []}),
        ),
    ];

    for (index, (case, create, patch)) in rows.iter().enumerate() {
        let component_type_id = format!("com.example.refused{index}");
        assert_eq!(
            emulator
                .create_component_type(WORKSPACE, &component_type_id, create.clone())
                .await
                .status,
            200,
            "case {case:?}: expected 200 from CreateComponentType"
        );
        // From a SETTLED record, so the only thing that could move the state or
        // the instants between the two reads below is the refused patch.
        wait_until_settled();
        let before = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;

        let refused = emulator
            .update_component_type(WORKSPACE, &component_type_id, patch.clone())
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains("dataType")),
            "case {case:?}: the refusal must name the constraint it refused on, got {refused:?}"
        );

        let after = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;
        assert_eq!(
            after.member("extendsFrom"),
            before.member("extendsFrom"),
            "case {case:?}: a refused patch leaves extendsFrom as it was, got {after:?}"
        );
        assert_eq!(
            after.member("propertyDefinitions"),
            before.member("propertyDefinitions"),
            "case {case:?}: a refused patch leaves the definitions as they were, got {after:?}"
        );
        assert_eq!(
            after.member("updateDateTime").as_f64(),
            before.member("updateDateTime").as_f64(),
            "case {case:?}: a refused patch does not move the update instant, got {after:?}"
        );
        assert_eq!(
            reported_state(&after),
            reported_state(&before),
            "case {case:?}: a refused patch does not open an update window, got {after:?}"
        );
    }
}
