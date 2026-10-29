// SPDX-License-Identifier: CC0-1.0

//! The scope of a `componentTypeId` query, asserted over BOTH property readers
//! that accept one, end to end over HTTP.
//!
//! RULE under test: a read addressed to a component TYPE is scoped to every
//! instance of that type in the workspace — a component whose `componentTypeId`
//! is that type or one that extends it, at every path an entity holds a
//! component at, so a composite component of the type is an instance of it too
//! — walked in (entityId, componentName) order, the order the store is keyed
//! in. `GetPropertyValue` and `GetPropertyValueHistory` scope such a read the
//! same way, so the two cannot disagree about which components one component
//! type names.
//!
//! The rows below differ along what the rule generalises over: HOW the instance
//! is reached (a component instantiating the named type, one instantiating a
//! type that extends it, a composite component of it at depth two and at depth
//! three), HOW MANY instances the type has (none, one, several), and how many
//! instances sit in one entity (one, two, and one entity carrying components of
//! several types).

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below reads.
const WORKSPACE: &str = "ctscope";

/// The component type the cases below query for: its own instances, the
/// instances of the type that extends it, and the composite components of it.
const LEAF: &str = "com.example.scopeleaf";
/// A component type that EXTENDS [`LEAF`], so a component instantiating it is
/// an instance of [`LEAF`] too.
const DERIVED: &str = "com.example.scopederived";
/// A component type that declares a COMPOSITE component type of [`LEAF`] under
/// `deeper`, and is itself declared as one: an instance of it is reached at
/// depth two and carries a [`LEAF`] instance at depth three.
const NEST: &str = "com.example.scopenest";
/// The component type an entity's top-level component instantiates: it declares
/// a composite component type of [`NEST`] under `inner`.
const HOLDER: &str = "com.example.scopeholder";
/// A component type no component of the workspace instantiates: the case a
/// component type has no instance at all.
const ORPHAN: &str = "com.example.scopeorphan";

/// The property whose stored value says WHICH component a reported row came out
/// of: every component below carries a different one, its own [`Instance::key`].
const MARKER: &str = "Marker";
/// The time series property every instance below carries a written value of, so
/// a history read reports a series per instance.
const READING: &str = "Reading";

/// The instant every time-series write below carries.
const T0: &str = "2026-05-01T00:00:00Z";

/// The property definitions every component type below declares: the marker and
/// one time series property.
fn property_definitions() -> Value {
    json!({
        MARKER: {"dataType": {"type": "STRING"}},
        READING: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
    })
}

/// A component's `properties` carrying `key` as its marker value.
fn marked(key: &str) -> Value {
    json!({MARKER: {"value": {"stringValue": key}}})
}

/// One component the seeded workspace holds: the entity it sits in and the
/// `componentPath` that addresses it.
#[derive(Debug)]
struct Instance {
    entity_id: &'static str,
    /// The whole `componentPath` of the component — a single segment for a
    /// top-level one.
    path: &'static str,
    /// The component's own name: the LAST segment of its path.
    component_name: &'static str,
    /// Whether it is a COMPOSITE component, which a reported reference names by
    /// its `componentPath` beside its name.
    composite: bool,
}

impl Instance {
    /// How a case names this component, and the value its marker property
    /// carries: the entity and the path together address exactly one component.
    fn key(&self) -> String {
        format!("{}|{}", self.entity_id, self.path)
    }

    /// The `entityPropertyReference` a reader reports for one property of this
    /// component.
    fn reference(&self, property_name: &str) -> Value {
        let mut reference = json!({
            "entityId": self.entity_id,
            "componentName": self.component_name,
            "propertyName": property_name,
        });
        if self.composite {
            reference["componentPath"] = json!(self.path);
        }
        reference
    }
}

/// Every component the seeded workspace holds, in no particular order: what the
/// cases below pick their expected instances out of.
const EVERY: &[Instance] = &[
    Instance {
        entity_id: "unit-a",
        path: "Direct",
        component_name: "Direct",
        composite: false,
    },
    Instance {
        entity_id: "unit-b",
        path: "Sub",
        component_name: "Sub",
        composite: false,
    },
    Instance {
        entity_id: "unit-c",
        path: "Assembly",
        component_name: "Assembly",
        composite: false,
    },
    Instance {
        entity_id: "unit-c",
        path: "Assembly/inner",
        component_name: "inner",
        composite: true,
    },
    Instance {
        entity_id: "unit-c",
        path: "Assembly/inner/deeper",
        component_name: "deeper",
        composite: true,
    },
    Instance {
        entity_id: "unit-d",
        path: "Alpha",
        component_name: "Alpha",
        composite: false,
    },
    Instance {
        entity_id: "unit-d",
        path: "Beta",
        component_name: "Beta",
        composite: false,
    },
];

/// The seeded component `key` names.
fn instance(key: &str) -> &'static Instance {
    EVERY
        .iter()
        .find(|instance| instance.key() == key)
        .unwrap_or_else(|| panic!("the seeded workspace holds a component keyed {key:?}"))
}

/// One query: the component type it names, and the instances of that type in
/// the order the scan walks them.
#[derive(Debug)]
struct ScopeCase {
    case: &'static str,
    component_type_id: &'static str,
    /// The [`Instance::key`]s of the instances in scope, in (entityId,
    /// componentName) order — an entity's components by name, each followed by
    /// the composite components inside it in path order.
    expected: &'static [&'static str],
}

/// The queries the rule is asserted over.
const CASES: &[ScopeCase] = &[
    ScopeCase {
        case: "the named type: its own instances, its subtype's, and a composite one",
        component_type_id: LEAF,
        expected: &[
            "unit-a|Direct",
            "unit-b|Sub",
            "unit-c|Assembly/inner/deeper",
            "unit-d|Alpha",
            "unit-d|Beta",
        ],
    },
    ScopeCase {
        case: "a subtype: only the component instantiating it, not its parent's instances",
        component_type_id: DERIVED,
        expected: &["unit-b|Sub"],
    },
    ScopeCase {
        case: "a type reached only as a composite component, at depth two",
        component_type_id: NEST,
        expected: &["unit-c|Assembly/inner"],
    },
    ScopeCase {
        case: "a type only one top-level component instantiates",
        component_type_id: HOLDER,
        expected: &["unit-c|Assembly"],
    },
    ScopeCase {
        case: "a type no component of the workspace instantiates",
        component_type_id: ORPHAN,
        expected: &[],
    },
];

/// The workspace, component types, entities and written values every case
/// reads.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (LEAF, json!({"propertyDefinitions": property_definitions()})),
        (DERIVED, json!({"extendsFrom": [LEAF]})),
        (
            NEST,
            json!({
                "propertyDefinitions": property_definitions(),
                "compositeComponentTypes": {"deeper": {"componentTypeId": LEAF}},
            }),
        ),
        (
            HOLDER,
            json!({
                "propertyDefinitions": property_definitions(),
                "compositeComponentTypes": {"inner": {"componentTypeId": NEST}},
            }),
        ),
        (
            ORPHAN,
            json!({"propertyDefinitions": property_definitions()}),
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

    let entities: Vec<Value> = vec![
        json!({
            "entityId": "unit-a",
            "entityName": "unit-a",
            "components": {"Direct": {
                "componentTypeId": LEAF,
                "properties": marked("unit-a|Direct"),
            }},
        }),
        json!({
            "entityId": "unit-b",
            "entityName": "unit-b",
            "components": {"Sub": {
                "componentTypeId": DERIVED,
                "properties": marked("unit-b|Sub"),
            }},
        }),
        json!({
            "entityId": "unit-c",
            "entityName": "unit-c",
            "components": {"Assembly": {
                "componentTypeId": HOLDER,
                "properties": marked("unit-c|Assembly"),
            }},
            "compositeComponents": {
                "Assembly/inner": {"properties": marked("unit-c|Assembly/inner")},
                "Assembly/inner/deeper": {
                    "properties": marked("unit-c|Assembly/inner/deeper"),
                },
            },
        }),
        json!({
            "entityId": "unit-d",
            "entityName": "unit-d",
            "components": {
                "Alpha": {"componentTypeId": LEAF, "properties": marked("unit-d|Alpha")},
                "Beta": {"componentTypeId": LEAF, "properties": marked("unit-d|Beta")},
            },
        }),
    ];
    for body in entities {
        let entity_id = body["entityId"].as_str().unwrap_or_default().to_string();
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }

    // One written value per component, so a history read reports a series for
    // every instance in scope: a property no value was written to has no
    // history to report.
    let entries: Vec<Value> = EVERY
        .iter()
        .enumerate()
        .map(|(index, instance)| {
            json!({
                "entityPropertyReference": {
                    "entityId": instance.entity_id,
                    "componentPath": instance.path,
                    "propertyName": READING,
                },
                "propertyValues": [{
                    "time": T0,
                    "value": {"doubleValue": index as f64},
                }],
            })
        })
        .collect();
    let written = emulator
        .batch_put_property_values(WORKSPACE, json!(entries))
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues: expected 200, got {written:?}"
    );
    assert_eq!(
        written.member("errorEntries"),
        &json!([]),
        "BatchPutPropertyValues: expected every seeded write to be accepted, got {written:?}"
    );
}

/// The marker value of each row of a `GetPropertyValue` table, in the order the
/// table reports them: which components the query was scoped to, as the table
/// sees them.
fn table_markers(read: &crate::harness::Resp) -> Vec<String> {
    read.member("tabularPropertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|table| table.as_array().cloned().unwrap_or_default())
        .map(|row| {
            row[MARKER]["stringValue"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The component each series of a `GetPropertyValueHistory` answer came out of,
/// as the [`Instance::key`] its reference names: which components the read was
/// scoped to, as the history sees them.
fn history_keys(read: &crate::harness::Resp) -> Vec<String> {
    read.member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|series| {
            let reference = &series["entityPropertyReference"];
            let path = reference["componentPath"]
                .as_str()
                .or_else(|| reference["componentName"].as_str())
                .unwrap_or_default();
            format!(
                "{}|{path}",
                reference["entityId"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// RULE: both property readers scope a `componentTypeId` query to the same
/// instances of that component type — a component instantiating it, one
/// instantiating a type that extends it, and a composite component of it at any
/// depth — in (entityId, componentName) order.
#[tokio::test]
async fn both_property_readers_scope_a_component_type_query_to_the_same_instances() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for ScopeCase {
        case,
        component_type_id,
        expected,
    } in CASES
    {
        let expected_keys: Vec<String> =
            expected.iter().map(|key| (*key).to_string()).collect();

        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "componentTypeId": component_type_id,
                    "selectedProperties": [MARKER],
                    "tabularConditions": {},
                }),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPropertyValue, got {read:?}"
        );
        let markers = table_markers(&read);
        assert_eq!(
            markers, expected_keys,
            "case {case:?}: expected GetPropertyValue to build a row per instance of \
             {component_type_id:?}, got {markers:?}"
        );

        let history = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "componentTypeId": component_type_id,
                    "selectedProperties": [READING],
                }),
            )
            .await;
        assert_eq!(
            history.status, 200,
            "case {case:?}: expected 200 from GetPropertyValueHistory, got {history:?}"
        );
        let keys = history_keys(&history);
        assert_eq!(
            keys, expected_keys,
            "case {case:?}: expected GetPropertyValueHistory to report a series per instance of \
             {component_type_id:?}, got {keys:?}"
        );
        assert_eq!(
            keys, markers,
            "case {case:?}: expected both readers to be scoped to the same instances of \
             {component_type_id:?}, got {keys:?} against {markers:?}"
        );

        // Each series is named by the reference of the component it came out
        // of: a composite one by its `componentPath` beside its own name, a
        // top-level one by its name alone.
        let references: Vec<Value> = history
            .member("propertyValues")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|series| series["entityPropertyReference"].clone())
            .collect();
        let expected_references: Vec<Value> = expected
            .iter()
            .map(|key| instance(key).reference(READING))
            .collect();
        assert_eq!(
            references, expected_references,
            "case {case:?}: expected each series to name the component it came out of, got \
             {references:?}"
        );

        // The scan walks the store's own key order, so a second read of
        // unchanged state answers exactly as the first did.
        let again = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "componentTypeId": component_type_id,
                    "selectedProperties": [READING],
                }),
            )
            .await;
        assert_eq!(
            again.body, history.body,
            "case {case:?}: expected a second read of unchanged state to answer as the first \
             did, got {again:?} against {history:?}"
        );
    }
}
