// SPDX-License-Identifier: CC0-1.0

//! `UpdateComponentType`'s referential integrity, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`,
//! the same entry the served emulator uses — and reads back through
//! `GetComponentType`, so the suite proves what the store holds afterwards
//! rather than that one response was assembled.
//!
//! The RULE under test: a write of a component type's declared inheritance,
//! composition or property groups must be RESOLVABLE — every id the request
//! puts in `extendsFrom` and in `compositeComponentTypes[*].componentTypeId`
//! has to name a component type the service holds (one stored in the workspace,
//! or one of the service's built-in types), that inheritance must not reach
//! back to the component type being written, and every
//! `propertyGroups[*].propertyNames` entry has to name a property the component
//! type defines as the write leaves it (its own or an inherited one) —
//! otherwise the request is refused with the modelled `ValidationException`
//! (HTTP 400) and the stored record is left exactly as it was.
//!
//! The two id members are resolved only where the request NAMES them (a
//! reference an earlier write left dangling is not this call's to refuse),
//! while the property groups are checked over the whole patched record whenever
//! the request touches what they resolve against — the groups, the own
//! definitions or the inheritance — because a patch that drops a property a
//! stored group names is the call that strands that group.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    COMPONENT_TYPE_SETTLE_SPAN, TABULAR_GROUP_TYPE,
};

use crate::harness::Emulator;

/// The workspace every case in this suite works in. Each test builds its own
/// emulator, so the id is shared without the cases sharing state.
const WORKSPACE: &str = "updateCtRefsWorkspace";

/// Two of the component types IoT TwinMaker SHIPS, from the user guide's
/// "built-in component types": they are published under the service's reserved
/// `com.amazon.` namespace (`state::BUILT_IN_COMPONENT_TYPE_ID_PREFIX`) and
/// neither is created in the workspace below, so a reference to one resolves
/// against the service rather than against the store.
const BUILT_IN_ALARM: &str = "com.amazon.iottwinmaker.alarm.basic";
/// See [`BUILT_IN_ALARM`].
const BUILT_IN_CONNECTOR: &str = "com.amazon.iotsitewise.connector";

/// Block until the transition a create or an update owes has fallen due, so a
/// read before a patch and a read after it cannot straddle it.
fn wait_until_settled() {
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));
}

/// An emulator holding the one workspace the cases are created in, plus the
/// component types the cases reference: a base type carrying one property
/// definition, and a two-link chain above it.
async fn emulator_with_references() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    for (component_type_id, body) in [
        (
            "com.example.base",
            json!({"propertyDefinitions": {"baseSerial": {"dataType": {"type": "STRING"}}}}),
        ),
        ("com.example.mid", json!({"extendsFrom": ["com.example.base"]})),
        ("com.example.leaf", json!({"extendsFrom": ["com.example.mid"]})),
    ] {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "the referenced component type {component_type_id:?} must exist, got {created:?}"
        );
    }
    emulator
}

/// Create the component type a row patches, under an id of its own so no row
/// depends on what another row left behind.
async fn create_target(emulator: &Emulator, case: &str, body: Value) -> String {
    let component_type_id = format!("com.example.target.{case}");
    let created = emulator
        .create_component_type(WORKSPACE, &component_type_id, body)
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateComponentType, got {created:?}"
    );
    component_type_id
}

/// One refused case: its name, the body the target is created with, the patch,
/// and the id or property name the refusal has to name.
type RefusedCase = (&'static str, Value, Value, &'static str);

/// RULE (refusal half): a patch declaring a reference the service cannot
/// resolve — an `extendsFrom` id or a `compositeComponentTypes` id naming no
/// component type the service holds, or a property group left naming a property
/// the patched component type neither defines nor inherits — is refused with
/// the modelled `ValidationException` (HTTP 400), and the stored record is left
/// exactly as it was.
///
/// The rows differ along WHICH member carries the unresolvable reference, where
/// in the member it sits (alone, first, last), how many references the member
/// carries, the shape of the id, and whether the named property was never
/// defined, is dropped by the very same patch, or is stranded by a patch that
/// names no group at all — because that is what the rule generalises over.
#[tokio::test]
async fn an_update_refuses_a_declared_reference_the_service_does_not_hold() {
    let emulator = emulator_with_references().await;

    let typed = json!({
        "propertyDefinitions": {
            "serial": {"dataType": {"type": "STRING"}},
            "pressure": {"dataType": {"type": "DOUBLE"}},
        },
    });
    let rows: Vec<RefusedCase> = vec![
        (
            "extendsFrom.unknownAlone",
            typed.clone(),
            json!({"extendsFrom": ["com.example.nothing"]}),
            "com.example.nothing",
        ),
        (
            "extendsFrom.unknownLastOfTwo",
            typed.clone(),
            json!({"extendsFrom": ["com.example.base", "com.example.nothing"]}),
            "com.example.nothing",
        ),
        (
            "extendsFrom.unknownFirstOfThree",
            typed.clone(),
            json!({
                "extendsFrom": ["com.example.absent", "com.example.base", "com.example.mid"],
            }),
            "com.example.absent",
        ),
        (
            // An id shaped like one of the service's own, but OUTSIDE the
            // `com.amazon.` namespace the built-in types live in: nothing holds
            // it, so it does not resolve.
            "extendsFrom.outsideTheBuiltInNamespace",
            typed.clone(),
            json!({"extendsFrom": ["com.amazonaws.iottwinmaker.alarm.basic"]}),
            "com.amazonaws.iottwinmaker.alarm.basic",
        ),
        (
            // A colon-separated id, which `ComponentTypeId`'s pattern admits.
            "extendsFrom.unknownColonId",
            typed.clone(),
            json!({"extendsFrom": ["ns:unknown-type_1"]}),
            "ns:unknown-type_1",
        ),
        (
            "compositeComponentTypes.unknownAlone",
            typed.clone(),
            json!({"compositeComponentTypes": {"inlet": {"componentTypeId": "com.example.nothing"}}}),
            "com.example.nothing",
        ),
        (
            "compositeComponentTypes.unknownBesideAStoredOne",
            typed.clone(),
            json!({
                "compositeComponentTypes": {
                    "inlet": {"componentTypeId": "com.example.base"},
                    "outlet": {"componentTypeId": "com.example.absent"},
                },
            }),
            "com.example.absent",
        ),
        (
            "compositeComponentTypes.unknownBesideABuiltIn",
            typed.clone(),
            json!({
                "compositeComponentTypes": {
                    "alarm": {"componentTypeId": BUILT_IN_ALARM},
                    "outlet": {"componentTypeId": "com.example.absent"},
                },
            }),
            "com.example.absent",
        ),
        (
            "propertyGroups.namesAPropertyThatWasNeverDefined",
            typed.clone(),
            json!({
                "propertyGroups": {
                    "readings": {"groupType": TABULAR_GROUP_TYPE, "propertyNames": ["notAProperty"]},
                },
            }),
            "notAProperty",
        ),
        (
            // The group's name IS a definition the record holds — until this
            // same patch replaces the definitions without it.
            "propertyGroups.namesAPropertyThisPatchDrops",
            typed.clone(),
            json!({
                "propertyDefinitions": {"pressure": {"dataType": {"type": "DOUBLE"}}},
                "propertyGroups": {"readings": {"propertyNames": ["serial"]}},
            }),
            "serial",
        ),
        (
            // Inherited names count, but only the ones the chain really
            // carries: `com.example.base` defines `baseSerial`, not this.
            "propertyGroups.namesAPropertyNoAncestorDefines",
            json!({"extendsFrom": ["com.example.mid"]}),
            json!({"propertyGroups": {"readings": {"propertyNames": ["baseMissing"]}}}),
            "baseMissing",
        ),
        (
            // Several names in one group, only the last of them undefined.
            "propertyGroups.lastNameUndefined",
            typed.clone(),
            json!({
                "propertyGroups": {
                    "readings": {"propertyNames": ["serial", "pressure", "humidity"]},
                },
            }),
            "humidity",
        ),
        (
            // The patch names no property group at all — it drops a definition
            // the STORED group names, which is this call stranding that group.
            "propertyDefinitions.droppedUnderAStoredGroup",
            json!({
                "propertyDefinitions": {
                    "serial": {"dataType": {"type": "STRING"}},
                    "pressure": {"dataType": {"type": "DOUBLE"}},
                },
                "propertyGroups": {"readings": {"propertyNames": ["serial"]}},
            }),
            json!({"propertyDefinitions": {"pressure": {"dataType": {"type": "DOUBLE"}}}}),
            "serial",
        ),
        (
            // The same stranding through the other member: clearing
            // `extendsFrom` takes away the inheritance the stored group's name
            // was resolving through.
            "extendsFrom.clearedUnderAStoredGroup",
            json!({
                "extendsFrom": ["com.example.mid"],
                "propertyGroups": {"inherited": {"propertyNames": ["baseSerial"]}},
            }),
            json!({"extendsFrom": []}),
            "baseSerial",
        ),
    ];

    let mut cases = Vec::new();
    for (case, create, patch, named) in rows {
        let component_type_id = create_target(&emulator, case, create).await;
        cases.push((case, component_type_id, patch, named));
    }
    // From SETTLED records, so the only thing that could move what a read
    // reports between the two reads below is the refused patch itself.
    wait_until_settled();

    for (case, component_type_id, patch, named) in cases {
        let before = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the component type the patch addresses must read back, got {before:?}"
        );

        let refused = emulator
            .update_component_type(WORKSPACE, &component_type_id, patch)
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        let message = refused.member("message").as_str().unwrap_or_default();
        assert!(
            message.contains(named),
            "case {case:?}: the refusal must name the reference it could not resolve \
             ({named:?}), got {message:?}"
        );

        let after = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused patch leaves the stored record exactly as it was, \
             got {after:?} against {before:?}"
        );
    }
}

/// RULE (cycle half): inheritance a patch declares must not reach back to the
/// component type it is patching — directly or through the component types its
/// declared parents themselves extend — so the chain above any component type
/// can always be walked up. Refused with the modelled `ValidationException`
/// (HTTP 400), the stored record untouched.
///
/// The rows differ along the LENGTH of the path back: naming the component type
/// itself, a child of it, a grandchild, a great-grandchild — plus one row that
/// extends the same chain from OUTSIDE it, which closes no cycle and is
/// accepted.
#[tokio::test]
async fn an_update_refuses_inheritance_that_closes_a_cycle() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the chain is created in must exist"
    );

    // A chain of four, each extending the one before it:
    // `chain0` <- `chain1` <- `chain2` <- `chain3`.
    let chain: Vec<String> = (0..4).map(|link| format!("com.example.chain{link}")).collect();
    for (link, component_type_id) in chain.iter().enumerate() {
        let body = match link {
            0 => json!({"propertyDefinitions": {"root": {"dataType": {"type": "STRING"}}}}),
            _ => json!({"extendsFrom": [chain[link - 1]]}),
        };
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "the chain link {component_type_id:?} must exist, got {created:?}"
        );
    }
    wait_until_settled();

    // (case, the link being patched, the link its new `extendsFrom` names)
    let rows = [
        ("itself", 0, 0),
        ("itsChild", 0, 1),
        ("itsGrandchild", 0, 2),
        ("itsGreatGrandchild", 0, 3),
        ("aMiddleLinksChild", 1, 2),
        ("aMiddleLinksGrandchild", 1, 3),
        ("theLeafItself", 3, 3),
    ];
    for (case, patched, named) in rows {
        let before = emulator.get_component_type(WORKSPACE, &chain[patched]).await;
        let refused = emulator
            .update_component_type(
                WORKSPACE,
                &chain[patched],
                json!({"extendsFrom": [chain[named]]}),
            )
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
                .is_some_and(|message| message.contains(&chain[patched])),
            "case {case:?}: the refusal must name the component type the cycle closes on, \
             got {refused:?}"
        );
        let after = emulator.get_component_type(WORKSPACE, &chain[patched]).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused patch leaves the stored record exactly as it was, \
             got {after:?} against {before:?}"
        );
    }

    // Extending the chain from outside it closes nothing: the walk up from
    // `chain3` never reaches `com.example.outsider`.
    let outsider = "com.example.outsider";
    assert_eq!(
        emulator
            .create_component_type(WORKSPACE, outsider, json!({}))
            .await
            .status,
        200,
        "the outsider must exist"
    );
    let accepted = emulator
        .update_component_type(WORKSPACE, outsider, json!({"extendsFrom": [chain[3]]}))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"outsiderExtendsTheLeaf\": a chain that does not reach back is accepted, \
         got {accepted:?}"
    );
    let read = emulator.get_component_type(WORKSPACE, outsider).await;
    assert_eq!(
        read.member("extendsFrom"),
        &json!([chain[3]]),
        "case \"outsiderExtendsTheLeaf\": the accepted inheritance is what the read reports, \
         got {read:?}"
    );
}

/// One accepted case: its name, the body the target is created with, the patch,
/// and the `(JSON pointer, value)` pairs the read afterwards must report.
type AcceptedCase = (
    &'static str,
    Value,
    Value,
    Vec<(&'static str, &'static str, Value)>,
);

/// RULE (acceptance half): a reference the service CAN resolve is stored and
/// read back — a component type stored in the workspace, one of the service's
/// built-in types, a property the record defines itself, and a property it
/// inherits from the component types it extends.
///
/// The rows differ along what the reference resolves against (the store, the
/// service's built-in namespace, the record's own definitions, an inherited
/// definition), along how many references the member carries, and along the
/// member carrying them — because that is what the rule generalises over. Each
/// row patches a component type created for it alone, so what one row stores
/// cannot carry into the next.
#[tokio::test]
async fn an_update_accepts_declared_references_the_service_holds() {
    let emulator = emulator_with_references().await;

    let typed = json!({
        "propertyDefinitions": {
            "serial": {"dataType": {"type": "STRING"}},
            "pressure": {"dataType": {"type": "DOUBLE"}},
        },
    });
    // (case, the create, the patch, (pointer, what it is, the expected value))
    let rows: Vec<AcceptedCase> = vec![
        (
            "extendsFrom.oneStoredParent",
            typed.clone(),
            json!({"extendsFrom": ["com.example.base"]}),
            vec![(
                "/extendsFrom",
                "the stored parent",
                json!(["com.example.base"]),
            )],
        ),
        (
            "extendsFrom.twoStoredParents",
            typed.clone(),
            json!({"extendsFrom": ["com.example.base", "com.example.mid"]}),
            vec![(
                "/extendsFrom",
                "both stored parents, in the order declared",
                json!(["com.example.base", "com.example.mid"]),
            )],
        ),
        (
            "extendsFrom.aBuiltInType",
            typed.clone(),
            json!({"extendsFrom": [BUILT_IN_ALARM]}),
            vec![(
                "/extendsFrom",
                "the built-in parent",
                json!([BUILT_IN_ALARM]),
            )],
        ),
        (
            "extendsFrom.aBuiltInBesideAStoredOne",
            typed.clone(),
            json!({"extendsFrom": [BUILT_IN_CONNECTOR, "com.example.leaf"]}),
            vec![(
                "/extendsFrom",
                "both parents",
                json!([BUILT_IN_CONNECTOR, "com.example.leaf"]),
            )],
        ),
        (
            // An emptied list declares no reference at all, so there is none to
            // resolve — and the record's own definitions still carry a
            // `dataType`, so nothing is stranded by the clearing.
            "extendsFrom.emptied",
            json!({"extendsFrom": ["com.example.base"], "propertyDefinitions": {
                "serial": {"dataType": {"type": "STRING"}},
            }}),
            json!({"extendsFrom": []}),
            vec![("/extendsFrom", "no parents", json!([]))],
        ),
        (
            "compositeComponentTypes.aStoredType",
            typed.clone(),
            json!({"compositeComponentTypes": {"inlet": {"componentTypeId": "com.example.base"}}}),
            vec![(
                "/compositeComponentTypes/inlet/componentTypeId",
                "the stored composite type",
                json!("com.example.base"),
            )],
        ),
        (
            "compositeComponentTypes.aBuiltInAndAStoredType",
            typed.clone(),
            json!({
                "compositeComponentTypes": {
                    "alarm": {"componentTypeId": BUILT_IN_ALARM},
                    "inlet": {"componentTypeId": "com.example.leaf"},
                },
            }),
            vec![
                (
                    "/compositeComponentTypes/alarm/componentTypeId",
                    "the built-in composite type",
                    json!(BUILT_IN_ALARM),
                ),
                (
                    "/compositeComponentTypes/inlet/componentTypeId",
                    "the stored composite type",
                    json!("com.example.leaf"),
                ),
            ],
        ),
        (
            // `CompositeComponentTypeRequest.componentTypeId` is optional, so an
            // entry that names none declares no reference to resolve.
            "compositeComponentTypes.entryWithoutAnId",
            typed.clone(),
            json!({"compositeComponentTypes": {"inlet": {}}}),
            vec![(
                "/compositeComponentTypes/inlet/componentTypeId",
                "no referenced id",
                Value::Null,
            )],
        ),
        (
            "propertyGroups.namesTheRecordsOwnProperty",
            typed.clone(),
            json!({
                "propertyGroups": {
                    "readings": {"groupType": TABULAR_GROUP_TYPE, "propertyNames": ["serial", "pressure"]},
                },
            }),
            vec![(
                "/propertyGroups/readings/propertyNames",
                "the grouped own properties",
                json!(["serial", "pressure"]),
            )],
        ),
        (
            "propertyGroups.namesAPropertyThisPatchDefines",
            typed.clone(),
            json!({
                "propertyDefinitions": {"humidity": {"dataType": {"type": "DOUBLE"}}},
                "propertyGroups": {"readings": {"propertyNames": ["humidity"]}},
            }),
            vec![
                (
                    "/propertyGroups/readings/propertyNames",
                    "the property the same patch defined",
                    json!(["humidity"]),
                ),
                (
                    "/propertyDefinitions/humidity/dataType/type",
                    "the definition the same patch carried",
                    json!("DOUBLE"),
                ),
            ],
        ),
        (
            // `baseSerial` is defined two links up the chain, not here.
            "propertyGroups.namesAnInheritedProperty",
            json!({"extendsFrom": ["com.example.mid"]}),
            json!({"propertyGroups": {"inherited": {"propertyNames": ["baseSerial"]}}}),
            vec![(
                "/propertyGroups/inherited/propertyNames",
                "the inherited property",
                json!(["baseSerial"]),
            )],
        ),
        (
            // The chain reaches a BUILT-IN type, whose own definitions this
            // emulator does not hold, so a name it cannot account for is not
            // one it can call undefined.
            "propertyGroups.underABuiltInParent",
            json!({"extendsFrom": [BUILT_IN_ALARM]}),
            json!({"propertyGroups": {"alarms": {"propertyNames": ["alarm_status"]}}}),
            vec![(
                "/propertyGroups/alarms/propertyNames",
                "the group under a built-in parent",
                json!(["alarm_status"]),
            )],
        ),
    ];

    for (case, create, patch, expectations) in rows {
        let component_type_id = create_target(&emulator, case, create).await;
        let updated = emulator
            .update_component_type(WORKSPACE, &component_type_id, patch)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateComponentType, got {updated:?}"
        );

        let read = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetComponentType, got {read:?}"
        );
        for (pointer, what, expected) in &expectations {
            let actual = read.body.pointer(pointer).unwrap_or(&Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} to report {what} as {expected:?}, \
                 got {actual:?} in {read:?}"
            );
        }
    }
}
