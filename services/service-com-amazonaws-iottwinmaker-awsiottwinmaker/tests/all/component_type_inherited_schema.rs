// SPDX-License-Identifier: CC0-1.0

//! A component TYPE's effective schema, end to end over HTTP.
//!
//! RULE under test: `GetComponentType` reports the schema the component type
//! EFFECTIVELY has — every property definition, function, property group and
//! composite component type the type itself or some component type on its
//! `extendsFrom` chain declares — with each entry resolved over the declarations
//! that reach it, nearest first (so a declaration that refines an inherited one,
//! restating only the members it changes, is reported with the inherited values
//! of the rest), and marked `isInherited` exactly when the chain declares it.
//!
//! The rows differ along what the rule generalises over: WHERE a declaration
//! comes from (the type itself, its parent, its grandparent, a parent the
//! service ships, a parent this emulator does not model, none at all), HOW MANY
//! parents a type declares (none, one, two), the DEPTH the chain is resolved
//! through (one hop, two hops), WHICH part of the schema carries it (property
//! definition, function, property group, composite component type), and whether
//! the type RESTATES an inherited declaration whole or refines it.
//!
//! The expectations for the parent the SERVICE ships are read off the catalogue
//! `CreateWorkspace` provisions (`state::built_in_component_types`) rather than
//! restated, so the rows stay the service's own declaration of it.

use std::collections::{BTreeMap, BTreeSet};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    built_in_component_types, COMPONENT_TYPE_SETTLE_SPAN, SCHEMA_INITIALIZER_FUNCTION,
};

use crate::harness::{Emulator, Resp, REGION};

/// The workspace every case below is created in.
const WORKSPACE: &str = "ctInherit";

/// The component type a composite entry refers to: it exists only to be
/// referenced, so it declares nothing.
const LEAF: &str = "com.example.leaf";
/// The root of the chain: it declares one of every part of a schema.
const BASE: &str = "com.example.base";
/// One hop above the root, adding a property definition of its own.
const MIDDLE: &str = "com.example.middle";
/// Two hops above the root: it REFINES an inherited property definition (no
/// `dataType`, only the member it changes) and an inherited function, and
/// declares one property of its own.
const CHILD: &str = "com.example.child";
/// A type that extends TWO parents at once, one of which declares nothing.
const TWO_PARENTS: &str = "com.example.twoParents";
/// A type extending the alarm component type the SERVICE ships — the pattern the
/// user guide publishes.
const ALARM_CHILD: &str = "com.example.alarm";
/// The built-in parent [`ALARM_CHILD`] extends.
const ALARM_BASIC: &str = "com.amazon.iottwinmaker.alarm.basic";
/// A type whose parent lies in the service's reserved namespace but is NOT one
/// of the built-ins this emulator models, so no declaration of it is held.
const UNMODELLED_PARENT_CHILD: &str = "com.example.unmodelledParent";
/// The parent [`UNMODELLED_PARENT_CHILD`] declares.
const UNMODELLED_PARENT: &str = "com.amazon.iottwinmaker.notmodelled";
/// A type that extends nothing: the boundary the rule sits on.
const SOLO: &str = "com.example.solo";

/// The lambda the base type's data connector is implemented by.
const READER_ARN: &str = "arn:aws:lambda:us-west-2:000000000000:function:reader";

/// Create the workspace and every component type the cases read.
///
/// In reference order: `extendsFrom` and a composite entry both have to name a
/// component type the workspace already holds.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (LEAF, json!({})),
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Temperature": {
                        "dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"},
                        "displayName": "Temperature",
                        "isTimeSeries": true,
                    },
                    "Serial": {"dataType": {"type": "STRING"}, "isExternalId": true},
                },
                "propertyGroups": {
                    "Readings": {
                        "groupType": "TABULAR",
                        "propertyNames": ["Serial", "Temperature"],
                    },
                },
                "functions": {
                    "dataReader": {
                        "scope": "ENTITY",
                        "requiredProperties": ["Serial"],
                        "implementedBy": {"lambda": {"arn": READER_ARN}},
                    },
                },
                "compositeComponentTypes": {"inner": {"componentTypeId": LEAF}},
            }),
        ),
        (
            MIDDLE,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}},
            }),
        ),
        (
            CHILD,
            json!({
                "extendsFrom": [MIDDLE],
                // Temperature carries NO dataType: it refines the definition the
                // chain already gives it. Humidity is the child's own.
                "propertyDefinitions": {
                    "Temperature": {"isRequiredInEntity": true},
                    "Humidity": {"dataType": {"type": "DOUBLE"}},
                },
                // The same refinement of a function: a different scope, and the
                // inherited implementation left alone.
                "functions": {"dataReader": {"scope": "WORKSPACE"}},
            }),
        ),
        (TWO_PARENTS, json!({"extendsFrom": [LEAF, MIDDLE]})),
        (ALARM_CHILD, json!({"extendsFrom": [ALARM_BASIC]})),
        (
            UNMODELLED_PARENT_CHILD,
            json!({
                "extendsFrom": [UNMODELLED_PARENT],
                "propertyDefinitions": {"Own": {"dataType": {"type": "STRING"}}},
            }),
        ),
        (
            SOLO,
            json!({"propertyDefinitions": {"Own": {"dataType": {"type": "STRING"}}}}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }
}

/// One property definition the read has to report: its name, its
/// `dataType.type` and whether it is inherited. Owned, because the row for the
/// parent the service ships is read off the catalogue rather than written out.
type ExpectedDefinition = (String, String, bool);

/// One [`ExpectedDefinition`], written the way a row names it.
fn definition(property_name: &str, data_type: &str, is_inherited: bool) -> ExpectedDefinition {
    (
        property_name.to_string(),
        data_type.to_string(),
        is_inherited,
    )
}

/// One case of the rule: the component type read, and the whole schema the read
/// has to report for it.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    component_type_id: &'static str,
    /// Every property definition the type effectively has, in any order.
    definitions: Vec<ExpectedDefinition>,
    /// Every function it effectively has, with whether it is inherited.
    functions: Vec<(&'static str, bool)>,
    /// Every property group it effectively has, with whether it is inherited.
    groups: Vec<(&'static str, bool)>,
    /// Every composite component type it effectively has, with whether it is
    /// inherited and the id it refers to.
    composites: Vec<(&'static str, bool, &'static str)>,
}

/// The property definitions the SERVICE publishes for the built-in alarm type,
/// read off the catalogue a workspace is provisioned with: `(name, type)`, each
/// reported on a child as INHERITED.
fn alarm_definitions() -> Vec<ExpectedDefinition> {
    let catalogue = built_in_component_types(REGION, WORKSPACE, std::time::SystemTime::now());
    let record = catalogue
        .get(ALARM_BASIC)
        .unwrap_or_else(|| panic!("the catalogue must ship {ALARM_BASIC:?}"));
    record
        .property_definitions
        .iter()
        .map(|(property_name, declared)| {
            let data_type = declared
                .data_type
                .as_ref()
                .unwrap_or_else(|| panic!("{property_name}: the catalogue declares a dataType"));
            definition(property_name, &data_type.r#type, true)
        })
        .collect()
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "aTypeThatExtendsNothingReportsOnlyItsOwn",
            component_type_id: BASE,
            definitions: vec![
                definition("Serial", "STRING", false),
                definition("Temperature", "DOUBLE", false),
            ],
            functions: vec![("dataReader", false)],
            groups: vec![("Readings", false)],
            composites: vec![("inner", false, LEAF)],
        },
        Case {
            what: "oneHopInheritsEveryPartOfTheParentsSchema",
            component_type_id: MIDDLE,
            definitions: vec![
                definition("Pressure", "DOUBLE", false),
                definition("Serial", "STRING", true),
                definition("Temperature", "DOUBLE", true),
            ],
            functions: vec![("dataReader", true)],
            groups: vec![("Readings", true)],
            composites: vec![("inner", true, LEAF)],
        },
        Case {
            what: "twoHopsInheritTransitivelyAndRefinementsStayInherited",
            component_type_id: CHILD,
            definitions: vec![
                definition("Humidity", "DOUBLE", false),
                definition("Pressure", "DOUBLE", true),
                definition("Serial", "STRING", true),
                // Refined by the child, and still reached through the chain:
                // its dataType is the one the chain declares.
                definition("Temperature", "DOUBLE", true),
            ],
            functions: vec![("dataReader", true)],
            groups: vec![("Readings", true)],
            composites: vec![("inner", true, LEAF)],
        },
        Case {
            what: "twoParentsOneOfWhichDeclaresNothing",
            component_type_id: TWO_PARENTS,
            definitions: vec![
                definition("Pressure", "DOUBLE", true),
                definition("Serial", "STRING", true),
                definition("Temperature", "DOUBLE", true),
            ],
            functions: vec![("dataReader", true)],
            groups: vec![("Readings", true)],
            composites: vec![("inner", true, LEAF)],
        },
        Case {
            what: "aParentTheServiceShipsIsInheritedFromLikeAnyOther",
            component_type_id: ALARM_CHILD,
            definitions: alarm_definitions(),
            functions: vec![],
            groups: vec![],
            composites: vec![],
        },
        Case {
            what: "aParentThisEmulatorDoesNotModelContributesNothing",
            component_type_id: UNMODELLED_PARENT_CHILD,
            definitions: vec![definition("Own", "STRING", false)],
            functions: vec![],
            groups: vec![],
            composites: vec![],
        },
        Case {
            what: "nothingExtendedIsNothingInherited",
            component_type_id: SOLO,
            definitions: vec![definition("Own", "STRING", false)],
            functions: vec![],
            groups: vec![],
            composites: vec![],
        },
    ]
}

/// The `(name, isInherited)` pairs of one reported map of the schema.
fn reported_inheritance(read: &Resp, member: &str) -> BTreeMap<String, Option<bool>> {
    read.member(member)
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(name, entry)| (name, entry["isInherited"].as_bool()))
        .collect()
}

/// The `(name, isInherited)` pairs a case expects under one member.
fn expected_inheritance(rows: &[(&str, bool)]) -> BTreeMap<String, Option<bool>> {
    rows.iter()
        .map(|(name, is_inherited)| ((*name).to_string(), Some(*is_inherited)))
        .collect()
}

/// RULE: a read reports every part of the schema the component type effectively
/// has — its own declarations and the ones its `extendsFrom` chain contributes —
/// each marked `isInherited` exactly when the chain declares it.
#[tokio::test]
async fn a_component_type_reports_the_schema_it_effectively_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let read = emulator
            .get_component_type(WORKSPACE, case.component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetComponentType, got {read:?}",
            case.what
        );

        // Exactly these property definitions, and nothing else.
        let reported: BTreeSet<String> = read
            .member("propertyDefinitions")
            .as_object()
            .cloned()
            .unwrap_or_default()
            .keys()
            .cloned()
            .collect();
        let wanted: BTreeSet<String> = case
            .definitions
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect();
        assert_eq!(
            reported, wanted,
            "case {:?}: expected exactly these property definitions, got {read:?}",
            case.what
        );

        for (property_name, data_type, is_inherited) in &case.definitions {
            let reported = &read.member("propertyDefinitions")[property_name];
            assert_eq!(
                reported["dataType"]["type"].as_str(),
                Some(data_type.as_str()),
                "case {:?} property {property_name:?}: wrong dataType, got {reported:?}",
                case.what
            );
            assert_eq!(
                reported["isInherited"].as_bool(),
                Some(*is_inherited),
                "case {:?} property {property_name:?}: expected isInherited {is_inherited}, got \
                 {reported:?}",
                case.what
            );
        }

        for (member, expected) in [
            ("functions", expected_inheritance(&case.functions)),
            ("propertyGroups", expected_inheritance(&case.groups)),
        ] {
            assert_eq!(
                reported_inheritance(&read, member),
                expected,
                "case {:?}: expected exactly these {member}, each marked as resolved, got {read:?}",
                case.what
            );
        }

        assert_eq!(
            reported_inheritance(&read, "compositeComponentTypes"),
            case.composites
                .iter()
                .map(|(name, is_inherited, _)| ((*name).to_string(), Some(*is_inherited)))
                .collect::<BTreeMap<String, Option<bool>>>(),
            "case {:?}: expected exactly these compositeComponentTypes, got {read:?}",
            case.what
        );
        for (composite_name, _, referenced) in &case.composites {
            let composite = &read.member("compositeComponentTypes")[composite_name];
            assert_eq!(
                composite["componentTypeId"].as_str(),
                Some(*referenced),
                "case {:?} composite {composite_name:?}: expected the referenced component type, \
                 got {composite:?}",
                case.what
            );
        }
    }
}

/// RULE (the refining half): a declaration that restates only the members it
/// changes is reported with the inherited values of every member it left alone —
/// which is why a writer accepts a property definition with no `dataType`
/// wherever one is inherited — and refining it changes nothing for the type it
/// was inherited from.
#[tokio::test]
async fn a_refining_declaration_carries_the_inherited_members_it_left_alone() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let child = emulator.get_component_type(WORKSPACE, CHILD).await;
    assert_eq!(
        child.status, 200,
        "expected 200 from GetComponentType, got {child:?}"
    );

    // The refined property definition: the member the child restated is the
    // child's, every other member is the one the chain declared.
    let refined = &child.member("propertyDefinitions")["Temperature"];
    let rows: Vec<(&str, Value)> = vec![
        ("dataType", json!({"type": "DOUBLE", "unitOfMeasure": "Celsius"})),
        ("displayName", json!("Temperature")),
        ("isTimeSeries", json!(true)),
        // Restated by the child, where the chain leaves it unset.
        ("isRequiredInEntity", json!(true)),
        // Set by neither, so it reads as the resolved false.
        ("isStoredExternally", json!(false)),
    ];
    for (member, expected) in rows {
        assert_eq!(
            refined[member], expected,
            "case {member:?}: expected the resolved value of the refined definition, got {refined:?}"
        );
    }

    // The same fold on a function: the scope the child restated, the
    // implementation and the required properties it inherited.
    let function = &child.member("functions")["dataReader"];
    assert_eq!(
        (
            function["scope"].as_str(),
            function["implementedBy"]["lambda"]["arn"].as_str(),
            function["requiredProperties"].clone(),
        ),
        (Some("WORKSPACE"), Some(READER_ARN), json!(["Serial"])),
        "expected the refined scope beside the inherited implementation, got {function:?}"
    );

    // The group the chain declares is reported with the contents it declared.
    let group = &child.member("propertyGroups")["Readings"];
    assert_eq!(
        (
            group["groupType"].as_str(),
            group["propertyNames"].clone(),
        ),
        (Some("TABULAR"), json!(["Serial", "Temperature"])),
        "expected the inherited group's own contents, got {group:?}"
    );

    // The parent is untouched by the child's refinement: its own definition
    // still reads as it declared it, and as its own.
    let base = emulator.get_component_type(WORKSPACE, BASE).await;
    let parent_definition = &base.member("propertyDefinitions")["Temperature"];
    assert_eq!(
        (
            parent_definition["isRequiredInEntity"].as_bool(),
            parent_definition["isInherited"].as_bool(),
        ),
        (Some(false), Some(false)),
        "expected the parent's own definition to be unchanged by the child, got \
         {parent_definition:?}"
    );
    let base_function = &base.member("functions")["dataReader"];
    assert_eq!(
        (
            base_function["scope"].as_str(),
            base_function["isInherited"].as_bool(),
        ),
        (Some("ENTITY"), Some(false)),
        "expected the parent's own function to be unchanged by the child, got {base_function:?}"
    );
}

/// RULE (what follows from the resolved schema): a derived member of the schema
/// is read off the functions the component type EFFECTIVELY has, so a schema
/// initializer it inherits counts as much as one it declares itself — and like
/// its own, it has only run once the component type has settled.
///
/// The rows differ along where the initializer is declared (the type itself, its
/// parent, its grandparent) and whether anything in the chain declares one at
/// all.
#[tokio::test]
async fn an_inherited_schema_initializer_is_the_types_own() {
    let emulator = Emulator::new();
    let created = emulator.create_workspace("ctInitializer").await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let initializer = json!({
        SCHEMA_INITIALIZER_FUNCTION: {"scope": "ENTITY", "implementedBy": {"isNative": true}},
    });
    // (the component type, its create, whether the chain gives it an initializer)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("com.example.root", json!({"functions": initializer}), true),
        (
            "com.example.heir",
            json!({"extendsFrom": ["com.example.root"]}),
            true,
        ),
        (
            "com.example.grandheir",
            json!({"extendsFrom": ["com.example.heir"]}),
            true,
        ),
        ("com.example.plain", json!({}), false),
        (
            "com.example.plainHeir",
            json!({"extendsFrom": ["com.example.plain"]}),
            false,
        ),
    ];
    for (component_type_id, body, _) in &rows {
        let created = emulator
            .create_component_type("ctInitializer", component_type_id, body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: expected 200 from CreateComponentType, got {created:?}"
        );
        let during = emulator
            .get_component_type("ctInitializer", component_type_id)
            .await;
        assert_eq!(
            during.member("isSchemaInitialized").as_bool(),
            Some(false),
            "case {component_type_id:?}: nothing has run while the create is still in flight, \
             got {during:?}"
        );
    }

    // The emulator stores the instant and never sleeps for it, so the test is
    // what waits for the transition the clock owes.
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));

    for (component_type_id, body, expected) in &rows {
        let settled = emulator
            .get_component_type("ctInitializer", component_type_id)
            .await;
        assert_eq!(
            settled.member("isSchemaInitialized").as_bool(),
            Some(*expected),
            "case {component_type_id:?}: expected isSchemaInitialized {expected}, got {settled:?}"
        );
        let function = &settled.member("functions")[SCHEMA_INITIALIZER_FUNCTION];
        if *expected {
            assert_eq!(
                function["isInherited"].as_bool(),
                // The type that declares it owns it; every type below it in the
                // chain inherits it.
                Some(body["functions"].is_null()),
                "case {component_type_id:?}: expected the initializer to be reported as resolved, \
                 got {function:?}"
            );
        } else {
            assert!(
                function.is_null(),
                "case {component_type_id:?}: nothing in the chain declares an initializer, \
                 got {function:?}"
            );
        }
    }
}

/// RULE (one schema, one answer): a COMPONENT of a component type that refines
#[tokio::test]
async fn a_component_of_a_refining_type_reads_the_resolved_definition() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "refined",
                "entityName": "refined",
                "components": {"Spec": {"componentTypeId": CHILD}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    let read = emulator.get_entity(WORKSPACE, "refined").await;
    assert_eq!(
        read.status, 200,
        "expected 200 from GetEntity, got {read:?}"
    );
    let definition = &read.member("components")["Spec"]["properties"]["Temperature"]["definition"];
    assert_eq!(
        (
            definition["dataType"]["type"].as_str(),
            definition["isRequiredInEntity"].as_bool(),
            definition["isTimeSeries"].as_bool(),
        ),
        (Some("DOUBLE"), Some(true), Some(true)),
        "expected the resolved definition on the component, got {definition:?}"
    );

    // `ListProperties` resolves it the same way.
    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "refined", "componentName": "Spec"}),
        )
        .await;
    let row = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row["propertyName"] == "Temperature")
        .unwrap_or_else(|| panic!("expected a row for Temperature, got {listed:?}"));
    assert_eq!(
        row["definition"], *definition,
        "expected ListProperties and GetEntity to report one definition, got {row:?}"
    );
}
