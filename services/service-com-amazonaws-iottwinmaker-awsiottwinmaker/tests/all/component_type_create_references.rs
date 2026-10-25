// SPDX-License-Identifier: CC0-1.0

//! `CreateComponentType`'s referential integrity, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`,
//! the same entry the served emulator uses — and reads back through
//! `GetComponentType`, so the suite proves what the store holds afterwards
//! rather than that one response was assembled.
//!
//! The RULE under test: a create of a component type's declared inheritance,
//! composition or property groups must be RESOLVABLE — every id the request
//! puts in `extendsFrom` and in `compositeComponentTypes[*].componentTypeId`
//! has to name a component type the service holds (one stored in the workspace,
//! or one of the service's built-in types), that inheritance must not name the
//! component type being created, and every `propertyGroups[*].propertyNames`
//! entry has to name a property the new component type defines (its own or an
//! inherited one) — otherwise the request is refused with the modelled
//! `ValidationException` (HTTP 400) and NOTHING is persisted, so the id the
//! create asked for is still free.
//!
//! The same rule `UpdateComponentType` meets, so the store cannot come to hold
//! a reference that does not resolve through either writer.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::TABULAR_GROUP_TYPE;

use crate::harness::{component_type_arn, Emulator};

/// The workspace every case in this suite works in. Each test builds its own
/// emulator, so the id is shared without the cases sharing state.
const WORKSPACE: &str = "createCtRefsWorkspace";

/// Two of the component types IoT TwinMaker SHIPS, from the catalogue this
/// crate models in `state::built_in_component_types`: they are published under
/// the service's reserved `com.amazon.` namespace and neither is created by the
/// cases below — the workspace is PROVISIONED with them when it is created, so
/// a reference to one resolves against what the service seeded rather than
/// against anything a caller wrote.
const BUILT_IN_ALARM: &str = "com.amazon.iottwinmaker.alarm.basic";
/// See [`BUILT_IN_ALARM`].
const BUILT_IN_CONNECTOR: &str = "com.amazon.iotsitewise.connector";

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

/// One refused case: its name, the id the create asks for, the request body,
/// and the id or property name the refusal has to name.
type RefusedCase = (&'static str, &'static str, Value, &'static str);

/// RULE (refusal half): a create declaring a reference the service cannot
/// resolve — an `extendsFrom` id or a `compositeComponentTypes` id naming no
/// component type the service holds, inheritance naming the component type
/// being created, or a property group naming a property the new component type
/// neither defines nor inherits — is refused with the modelled
/// `ValidationException` (HTTP 400), and nothing is persisted: the id is still
/// free afterwards.
///
/// The rows differ along WHICH member carries the unresolvable reference, where
/// in the member it sits (alone, first, last), how many references the member
/// carries, the shape of the id, and what the named property is missing from
/// (never defined anywhere, or defined only off the inheritance chain the
/// create declares) — because that is what the rule generalises over.
#[tokio::test]
async fn a_create_refuses_a_declared_reference_the_service_does_not_hold() {
    let emulator = emulator_with_references().await;

    let typed = json!({
        "serial": {"dataType": {"type": "STRING"}},
        "pressure": {"dataType": {"type": "DOUBLE"}},
    });
    let rows: Vec<RefusedCase> = vec![
        (
            "extendsFrom.unknownAlone",
            "com.example.child.unknownAlone",
            json!({"extendsFrom": ["com.example.nothing"]}),
            "com.example.nothing",
        ),
        (
            "extendsFrom.unknownLastOfTwo",
            "com.example.child.unknownLastOfTwo",
            json!({"extendsFrom": ["com.example.base", "com.example.nothing"]}),
            "com.example.nothing",
        ),
        (
            "extendsFrom.unknownFirstOfThree",
            "com.example.child.unknownFirstOfThree",
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
            "com.example.child.outsideTheNamespace",
            json!({"extendsFrom": ["com.amazonaws.iottwinmaker.alarm.basic"]}),
            "com.amazonaws.iottwinmaker.alarm.basic",
        ),
        (
            // A colon-separated id, which `ComponentTypeId`'s pattern admits.
            "extendsFrom.unknownColonId",
            "com.example.child.colonId",
            json!({"extendsFrom": ["ns:unknown-type_1"]}),
            "ns:unknown-type_1",
        ),
        (
            // Inheritance naming the component type this very create mints:
            // nothing could ever be inherited through it, so the chain above it
            // could not be walked.
            "extendsFrom.itself",
            "com.example.child.itself",
            json!({"extendsFrom": ["com.example.child.itself"]}),
            "com.example.child.itself",
        ),
        (
            // A dataType-less definition is licensed by a non-empty
            // `extendsFrom` — but only one that resolves, so this is refused on
            // the parent rather than accepted and silently stranded.
            "extendsFrom.unknownUnderAnUntypedDefinition",
            "com.example.child.untypedUnderUnknown",
            json!({
                "extendsFrom": ["com.example.nothing"],
                "propertyDefinitions": {"temperature": {"isRequiredInEntity": true}},
            }),
            "com.example.nothing",
        ),
        (
            "compositeComponentTypes.unknownAlone",
            "com.example.child.compositeUnknown",
            json!({
                "compositeComponentTypes": {
                    "inlet": {"componentTypeId": "com.example.nothing"},
                },
            }),
            "com.example.nothing",
        ),
        (
            "compositeComponentTypes.unknownBesideAStoredOne",
            "com.example.child.compositeBesideStored",
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
            "com.example.child.compositeBesideBuiltIn",
            json!({
                "compositeComponentTypes": {
                    "alarm": {"componentTypeId": BUILT_IN_ALARM},
                    "outlet": {"componentTypeId": "com.example.absent"},
                },
            }),
            "com.example.absent",
        ),
        (
            "propertyGroups.namesAPropertyThatIsNotDefined",
            "com.example.child.groupUndefined",
            json!({
                "propertyDefinitions": typed.clone(),
                "propertyGroups": {
                    "readings": {
                        "groupType": TABULAR_GROUP_TYPE,
                        "propertyNames": ["notAProperty"],
                    },
                },
            }),
            "notAProperty",
        ),
        (
            // Several names in one group, only the last of them undefined.
            "propertyGroups.lastNameUndefined",
            "com.example.child.groupLastUndefined",
            json!({
                "propertyDefinitions": typed.clone(),
                "propertyGroups": {
                    "readings": {"propertyNames": ["serial", "pressure", "humidity"]},
                },
            }),
            "humidity",
        ),
        (
            // Inherited names count, but only the ones the declared chain really
            // carries: `com.example.base` defines `baseSerial`, not this.
            "propertyGroups.namesAPropertyNoAncestorDefines",
            "com.example.child.groupNoAncestor",
            json!({
                "extendsFrom": ["com.example.mid"],
                "propertyGroups": {"inherited": {"propertyNames": ["baseMissing"]}},
            }),
            "baseMissing",
        ),
    ];

    for (case, component_type_id, body, named) in rows {
        let refused = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
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

        // Nothing was persisted: the component type is not readable, and the
        // ARN the create would have minted does not resolve either.
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create persists nothing, got {read:?}"
        );
        let arn = component_type_arn(WORKSPACE, component_type_id);
        let tags = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create mints no ARN, got {tags:?}"
        );
    }

    // The id every refused row asked for is still free: the create that was
    // refused took nothing, so one whose references resolve can have it.
    let retaken = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.child.unknownAlone",
            json!({"extendsFrom": ["com.example.base"]}),
        )
        .await;
    assert_eq!(
        retaken.status, 200,
        "case \"retakenAfterRefusal\": the id a refused create asked for is still free, \
         got {retaken:?}"
    );
}

/// One accepted case: its name, the id the create asks for, the request body,
/// and the `(JSON pointer, what it is, the expected value)` triples the read
/// afterwards must report.
type AcceptedCase = (
    &'static str,
    &'static str,
    Value,
    Vec<(&'static str, &'static str, Value)>,
);

/// RULE (acceptance half): a reference the service CAN resolve is stored by the
/// create and read back — a component type stored in the workspace, one of the
/// service's built-in types, the component type the create itself mints (for a
/// composite entry, which this call is what makes resolvable), a property the
/// new record defines itself, and a property it inherits from the component
/// types it extends.
///
/// The rows differ along what the reference resolves against (the store, the
/// service's built-in namespace, the record being created, its own definitions,
/// an inherited definition), along how many references the member carries, and
/// along the member carrying them — because that is what the rule generalises
/// over.
#[tokio::test]
async fn a_create_accepts_declared_references_the_service_holds() {
    let emulator = emulator_with_references().await;

    let typed = json!({
        "serial": {"dataType": {"type": "STRING"}},
        "pressure": {"dataType": {"type": "DOUBLE"}},
    });
    let rows: Vec<AcceptedCase> = vec![
        (
            "extendsFrom.oneStoredParent",
            "com.example.ok.oneStoredParent",
            json!({"extendsFrom": ["com.example.base"]}),
            vec![(
                "/extendsFrom",
                "the stored parent",
                json!(["com.example.base"]),
            )],
        ),
        (
            "extendsFrom.twoStoredParents",
            "com.example.ok.twoStoredParents",
            json!({"extendsFrom": ["com.example.base", "com.example.mid"]}),
            vec![(
                "/extendsFrom",
                "both stored parents, in the order declared",
                json!(["com.example.base", "com.example.mid"]),
            )],
        ),
        (
            "extendsFrom.aBuiltInType",
            "com.example.ok.builtInParent",
            json!({"extendsFrom": [BUILT_IN_ALARM]}),
            vec![(
                "/extendsFrom",
                "the built-in parent",
                json!([BUILT_IN_ALARM]),
            )],
        ),
        (
            "extendsFrom.aBuiltInBesideAStoredOne",
            "com.example.ok.builtInBesideStored",
            json!({"extendsFrom": [BUILT_IN_CONNECTOR, "com.example.leaf"]}),
            vec![(
                "/extendsFrom",
                "both parents, in the order declared",
                json!([BUILT_IN_CONNECTOR, "com.example.leaf"]),
            )],
        ),
        (
            // No `extendsFrom` at all declares no reference to resolve.
            "extendsFrom.absent",
            "com.example.ok.noParent",
            json!({"propertyDefinitions": typed.clone()}),
            vec![("/extendsFrom", "no parents", json!([]))],
        ),
        (
            // A definition carrying no `dataType`, licensed by a parent that
            // RESOLVES: the parent's definitions are what a reader resolves it
            // through.
            "extendsFrom.licensesAnUntypedDefinition",
            "com.example.ok.untypedUnderAParent",
            json!({
                "extendsFrom": ["com.example.base"],
                "propertyDefinitions": {"baseSerial": {"isRequiredInEntity": true}},
            }),
            vec![(
                "/extendsFrom",
                "the parent the definition refines",
                json!(["com.example.base"]),
            )],
        ),
        (
            "compositeComponentTypes.aStoredType",
            "com.example.ok.compositeStored",
            json!({
                "compositeComponentTypes": {
                    "inlet": {"componentTypeId": "com.example.base"},
                },
            }),
            vec![(
                "/compositeComponentTypes/inlet/componentTypeId",
                "the stored composite type",
                json!("com.example.base"),
            )],
        ),
        (
            "compositeComponentTypes.aBuiltInAndAStoredType",
            "com.example.ok.compositeBuiltInAndStored",
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
            // A composite entry naming the component type this create mints:
            // the reference resolves, because this very call is what leaves the
            // workspace holding it.
            "compositeComponentTypes.theTypeBeingCreated",
            "com.example.ok.selfComposite",
            json!({
                "compositeComponentTypes": {
                    "inner": {"componentTypeId": "com.example.ok.selfComposite"},
                },
            }),
            vec![(
                "/compositeComponentTypes/inner/componentTypeId",
                "the component type the create minted",
                json!("com.example.ok.selfComposite"),
            )],
        ),
        (
            // `CompositeComponentTypeRequest.componentTypeId` is optional, so
            // an entry that names none declares no reference to resolve.
            "compositeComponentTypes.entryWithoutAnId",
            "com.example.ok.compositeWithoutAnId",
            json!({"compositeComponentTypes": {"inlet": {}}}),
            vec![(
                "/compositeComponentTypes/inlet/componentTypeId",
                "no referenced id",
                Value::Null,
            )],
        ),
        (
            "propertyGroups.namesTheRecordsOwnProperties",
            "com.example.ok.groupOwnProperties",
            json!({
                "propertyDefinitions": typed.clone(),
                "propertyGroups": {
                    "readings": {
                        "groupType": TABULAR_GROUP_TYPE,
                        "propertyNames": ["serial", "pressure"],
                    },
                },
            }),
            vec![(
                "/propertyGroups/readings/propertyNames",
                "the grouped own properties",
                json!(["serial", "pressure"]),
            )],
        ),
        (
            // `baseSerial` is defined two links up the chain, not here.
            "propertyGroups.namesAnInheritedProperty",
            "com.example.ok.groupInherited",
            json!({
                "extendsFrom": ["com.example.mid"],
                "propertyGroups": {"inherited": {"propertyNames": ["baseSerial"]}},
            }),
            vec![(
                "/propertyGroups/inherited/propertyNames",
                "the inherited property",
                json!(["baseSerial"]),
            )],
        ),
        (
            // The chain reaches a BUILT-IN type the workspace was provisioned
            // with, so the group's name resolves against the definitions that
            // built-in declares.
            "propertyGroups.underABuiltInParent",
            "com.example.ok.groupUnderBuiltIn",
            json!({
                "extendsFrom": [BUILT_IN_ALARM],
                "propertyGroups": {"alarms": {"propertyNames": ["alarm_status"]}},
            }),
            vec![(
                "/propertyGroups/alarms/propertyNames",
                "the group under a built-in parent",
                json!(["alarm_status"]),
            )],
        ),
    ];

    for (case, component_type_id, body, expectations) in rows {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateComponentType, got {created:?}"
        );

        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
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

/// RULE (reach half): what a create may reference is what the SERVICE holds at
/// that moment, so a component type created earlier in the same workspace is a
/// legal parent while the same id in another workspace is not — a reference
/// never reaches across a workspace.
#[tokio::test]
async fn a_create_resolves_references_inside_its_own_workspace_only() {
    let emulator = Emulator::new();
    for workspace_id in ["refsHere", "refsElsewhere"] {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} must exist"
        );
    }
    let parent = "com.example.parentOverThere";
    assert_eq!(
        emulator
            .create_component_type("refsElsewhere", parent, json!({}))
            .await
            .status,
        200,
        "the parent in the other workspace must exist"
    );

    let refused = emulator
        .create_component_type(
            "refsHere",
            "com.example.childOverHere",
            json!({"extendsFrom": [parent]}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"parentInAnotherWorkspace\": expected the modelled refusal, got {refused:?}"
    );
    assert!(
        refused
            .member("message")
            .as_str()
            .is_some_and(|message| message.contains(parent) && message.contains("refsHere")),
        "case \"parentInAnotherWorkspace\": the refusal must name the id and the workspace \
         it was looked for in, got {refused:?}"
    );

    // The same id, created here, is a parent that resolves.
    assert_eq!(
        emulator
            .create_component_type("refsHere", parent, json!({}))
            .await
            .status,
        200,
        "case \"parentCreatedHere\": the parent must be creatable in this workspace"
    );
    let accepted = emulator
        .create_component_type(
            "refsHere",
            "com.example.childOverHere",
            json!({"extendsFrom": [parent]}),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"parentCreatedHere\": a parent this workspace holds resolves, got {accepted:?}"
    );
    let read = emulator
        .get_component_type("refsHere", "com.example.childOverHere")
        .await;
    assert_eq!(
        read.member("extendsFrom"),
        &json!([parent]),
        "case \"parentCreatedHere\": the accepted inheritance is what the read reports, \
         got {read:?}"
    );
}
