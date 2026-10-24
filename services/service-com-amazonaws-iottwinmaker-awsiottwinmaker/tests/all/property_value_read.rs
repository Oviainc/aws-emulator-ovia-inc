// SPDX-License-Identifier: CC0-1.0

//! `GetPropertyValue`, end to end over HTTP.
//!
//! RULE under test: a property reads as the value written to it, else as the
//! default declared by the definition that gives it its meaning — the
//! component's own definition, else the one it inherits from the component type
//! it instantiates or from a type that type extends — and as nothing at all
//! when neither exists. The properties of ONE component come back as the
//! `propertyValues` map keyed by property name; the instances of a component
//! type come back as a table whose rows `tabularConditions` filters and orders.
//!
//! The rows of each table below differ along what the rule generalises over:
//! the DATA TYPE the value travels in (`STRING`, `DOUBLE`, `INTEGER`, `LONG`,
//! `BOOLEAN`), WHERE the value comes from (written to the property, defaulted by
//! the component's own definition, defaulted by the component type, defaulted by
//! a type it extends, nowhere), and WHICH component carries it (a top-level one
//! and a composite one addressed by its `componentPath`).

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below reads from.
const WORKSPACE: &str = "propvalues";

/// The component type whose property definitions carry the defaults the rule
/// falls back to. Its `Enabled` property declares none, which is the case a
/// property reads as nothing in.
const BASE: &str = "com.example.base";
/// A component type that EXTENDS [`BASE`]: a component instantiating it
/// inherits every default above, and adds one of its own (`Site`).
const DERIVED: &str = "com.example.derived";
/// A component type that declares a COMPOSITE component type of [`BASE`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Spec/inner`.
const ASSEMBLY: &str = "com.example.assembly";

/// The workspace, component types and entities every case below reads.
///
/// Four entities, each a different source for the value a read reports:
/// `unit-1` writes one property and leaves the rest to their defaults; `unit-2`
/// writes none; `unit-3` instantiates the DERIVED type and writes one inherited
/// property; `unit-4` carries a composite component of the base type.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Tag": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "unset-tag"}},
                    "Threshold": {"dataType": {"type": "DOUBLE"}, "defaultValue": {"doubleValue": 1.5}},
                    "Count": {"dataType": {"type": "INTEGER"}, "defaultValue": {"integerValue": 7}},
                    "Serial": {"dataType": {"type": "LONG"}, "defaultValue": {"longValue": 42}},
                    "Enabled": {"dataType": {"type": "BOOLEAN"}},
                },
                "propertyGroups": {
                    "Readings": {"groupType": "TABULAR", "propertyNames": ["Tag", "Threshold"]},
                },
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {
                    "Site": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "plant-a"}},
                },
            }),
        ),
        (
            ASSEMBLY,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": BASE}}}),
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

    let entities: Vec<(&str, Value)> = vec![
        (
            "unit-1",
            json!({
                "entityId": "unit-1",
                "entityName": "unit-1",
                "components": {"Spec": {
                    "componentTypeId": BASE,
                    "properties": {"Tag": {"value": {"stringValue": "alpha"}}},
                }},
            }),
        ),
        (
            "unit-2",
            json!({
                "entityId": "unit-2",
                "entityName": "unit-2",
                "components": {"Spec": {"componentTypeId": BASE}},
            }),
        ),
        (
            "unit-3",
            json!({
                "entityId": "unit-3",
                "entityName": "unit-3",
                "components": {"Spec": {
                    "componentTypeId": DERIVED,
                    "properties": {"Threshold": {"value": {"doubleValue": 9.5}}},
                }},
            }),
        ),
        (
            "unit-4",
            json!({
                "entityId": "unit-4",
                "entityName": "unit-4",
                "components": {"Spec": {"componentTypeId": ASSEMBLY}},
                "compositeComponents": {"Spec/inner": {}},
            }),
        ),
    ];
    for (entity_id, body) in entities {
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

/// One case of the rule: which component, which property, and the `DataValue`
/// member the reported value has to travel in.
#[derive(Debug)]
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    entity_id: &'static str,
    /// The addressing member: a top-level component by name, or a composite
    /// component by path.
    component_name: Option<&'static str>,
    component_path: Option<&'static str>,
    property: &'static str,
    /// The `DataValue` member the value travels in and the value itself, or
    /// `None` where the property reads as nothing.
    expected: Option<(&'static str, Value)>,
}

/// RULE: a selected property reads as the value written to it, else as the
/// default its definition declares (the component's own, else the component
/// type's, else that of a type the component type extends), else as nothing.
#[tokio::test]
async fn a_property_reads_as_its_value_then_its_default() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases = vec![
        Case {
            what: "a value written to the property wins over its default",
            entity_id: "unit-1",
            component_name: Some("Spec"),
            component_path: None,
            property: "Tag",
            expected: Some(("stringValue", json!("alpha"))),
        },
        Case {
            what: "a DOUBLE property nothing wrote to reads as the component type's default",
            entity_id: "unit-1",
            component_name: Some("Spec"),
            component_path: None,
            property: "Threshold",
            expected: Some(("doubleValue", json!(1.5))),
        },
        Case {
            what: "a STRING default, on a component that wrote no property at all",
            entity_id: "unit-2",
            component_name: Some("Spec"),
            component_path: None,
            property: "Tag",
            expected: Some(("stringValue", json!("unset-tag"))),
        },
        Case {
            what: "an INTEGER default",
            entity_id: "unit-2",
            component_name: Some("Spec"),
            component_path: None,
            property: "Count",
            expected: Some(("integerValue", json!(7))),
        },
        Case {
            what: "a LONG default",
            entity_id: "unit-2",
            component_name: Some("Spec"),
            component_path: None,
            property: "Serial",
            expected: Some(("longValue", json!(42))),
        },
        Case {
            what: "a property with no value and no default reads as nothing",
            entity_id: "unit-2",
            component_name: Some("Spec"),
            component_path: None,
            property: "Enabled",
            expected: None,
        },
        Case {
            what: "a default inherited through extendsFrom",
            entity_id: "unit-3",
            component_name: Some("Spec"),
            component_path: None,
            property: "Count",
            expected: Some(("integerValue", json!(7))),
        },
        Case {
            what: "the extending type's own default",
            entity_id: "unit-3",
            component_name: Some("Spec"),
            component_path: None,
            property: "Site",
            expected: Some(("stringValue", json!("plant-a"))),
        },
        Case {
            what: "a value written to an inherited property",
            entity_id: "unit-3",
            component_name: Some("Spec"),
            component_path: None,
            property: "Threshold",
            expected: Some(("doubleValue", json!(9.5))),
        },
        Case {
            what: "a composite component addressed by its componentPath",
            entity_id: "unit-4",
            component_name: None,
            component_path: Some("Spec/inner"),
            property: "Tag",
            expected: Some(("stringValue", json!("unset-tag"))),
        },
    ];

    for case in cases {
        let mut body = json!({"selectedProperties": [case.property]});
        body["entityId"] = json!(case.entity_id);
        if let Some(component_name) = case.component_name {
            body["componentName"] = json!(component_name);
        }
        if let Some(component_path) = case.component_path {
            body["componentPath"] = json!(component_path);
        }
        let read = emulator.get_property_value(WORKSPACE, body).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200, got {read:?}",
            case.what
        );

        let latest = &read.member("propertyValues")[case.property];
        assert!(
            latest.is_object(),
            "case {:?}: expected propertyValues.{} to be reported, got {read:?}",
            case.what,
            case.property
        );
        assert_eq!(
            latest["propertyReference"]["propertyName"],
            json!(case.property),
            "case {:?}: expected the reference to name the property, got {latest:?}",
            case.what
        );
        assert_eq!(
            latest["propertyReference"]["entityId"],
            json!(case.entity_id),
            "case {:?}: expected the reference to name the entity, got {latest:?}",
            case.what
        );
        // A composite component is reported with the path it was addressed by;
        // a top-level one carries no path.
        assert_eq!(
            latest["propertyReference"]["componentPath"],
            case.component_path.map_or(Value::Null, |path| json!(path)),
            "case {:?}: expected the reference's componentPath to be the one addressed, got \
             {latest:?}",
            case.what
        );
        match &case.expected {
            Some((member, value)) => assert_eq!(
                latest["propertyValue"][member],
                *value,
                "case {:?}: expected propertyValue.{member} to be {value}, got {latest:?}",
                case.what
            ),
            None => assert!(
                latest["propertyValue"].is_null(),
                "case {:?}: expected no propertyValue, got {latest:?}",
                case.what
            ),
        }
    }
}

/// RULE: a read reports what is stored and changes nothing, so two reads of
/// unchanged state answer the same, and the component the value was read from
/// still reports it through `GetEntity`.
#[tokio::test]
async fn reading_a_property_value_changes_nothing() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let request = json!({
        "entityId": "unit-1",
        "componentName": "Spec",
        "selectedProperties": ["Tag", "Threshold"],
    });
    let first = emulator
        .get_property_value(WORKSPACE, request.clone())
        .await;
    let second = emulator
        .get_property_value(WORKSPACE, request.clone())
        .await;
    assert_eq!(
        first.body, second.body,
        "two reads of unchanged state: expected the same answer, got {first:?} then {second:?}"
    );

    let entity = emulator.get_entity(WORKSPACE, "unit-1").await;
    assert_eq!(
        entity.member("components")["Spec"]["properties"]["Tag"]["value"]["stringValue"],
        json!("alpha"),
        "GetEntity after the reads: expected the written value to be unchanged, got {entity:?}"
    );
}

/// RULE: the `propertyValues` answer is keyed by property name, walked in name
/// order and paged — `maxResults` caps the page and the `nextToken` resumes at
/// the next property, so following the tokens reports every selected property
/// exactly once.
#[tokio::test]
async fn the_latest_value_answer_is_paged_over_the_selection() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Five properties, asked for in no particular order, in pages of two: the
    // answer is keyed by name, so the pages are the name-ordered selection cut
    // into threes.
    let selected = json!(["Tag", "Threshold", "Count", "Serial", "Enabled"]);
    let expected_pages: Vec<Vec<&str>> = vec![
        vec!["Count", "Enabled"],
        vec!["Serial", "Tag"],
        vec!["Threshold"],
    ];

    let mut next_token: Option<String> = None;
    for (index, expected) in expected_pages.iter().enumerate() {
        let mut body = json!({
            "entityId": "unit-2",
            "componentName": "Spec",
            "selectedProperties": selected,
            "maxResults": 2,
        });
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let page = emulator.get_property_value(WORKSPACE, body).await;
        assert_eq!(
            page.status, 200,
            "page {index}: expected 200, got {page:?}"
        );

        let mut reported: Vec<String> = page
            .member("propertyValues")
            .as_object()
            .unwrap_or_else(|| panic!("page {index}: expected a propertyValues map, got {page:?}"))
            .keys()
            .cloned()
            .collect();
        reported.sort();
        assert_eq!(
            reported,
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "page {index}: expected exactly these properties, got {page:?}"
        );

        next_token = page
            .member("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        let is_last = index + 1 == expected_pages.len();
        assert_eq!(
            next_token.is_none(),
            is_last,
            "page {index}: expected a nextToken only before the last page, got {next_token:?}"
        );
    }
}

/// One tabular case: the conditions the query carries, how many rows they
/// select, and the column the first row has to carry.
#[derive(Debug)]
struct TableCase {
    what: &'static str,
    conditions: Value,
    expected_rows: usize,
    /// The column of the FIRST row and the `DataValue` it has to be, where the
    /// case pins one.
    expected_first: Option<(&'static str, Value)>,
}

/// RULE: a `componentTypeId` query is answered with a table — a row per
/// instance of that component type in the workspace, counting a component whose
/// type EXTENDS it and a composite component of it — whose rows
/// `tabularConditions` filters and orders.
#[tokio::test]
async fn a_component_type_query_is_answered_with_a_filtered_ordered_table() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The instances of the base type: unit-1's and unit-2's components, unit-3's
    // component of the type that extends it, and unit-4's composite component of
    // it. Each row carries the two selected properties.
    let cases = vec![
        TableCase {
            what: "no conditions: a row per instance of the component type",
            conditions: json!({}),
            expected_rows: 4,
            expected_first: None,
        },
        TableCase {
            what: "ordered by a DOUBLE property, descending: the largest value first",
            conditions: json!({"orderBy": [{"propertyName": "Threshold", "order": "DESCENDING"}]}),
            expected_rows: 4,
            expected_first: Some(("Threshold", json!({"doubleValue": 9.5}))),
        },
        TableCase {
            what: "ordered ascending by default: the smallest value first",
            conditions: json!({"orderBy": [{"propertyName": "Threshold"}]}),
            expected_rows: 4,
            expected_first: Some(("Threshold", json!({"doubleValue": 1.5}))),
        },
        TableCase {
            what: "filtered with >: only the row above the threshold",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Threshold", "operator": ">", "value": {"doubleValue": 2.0}},
            ]}),
            expected_rows: 1,
            expected_first: Some(("Threshold", json!({"doubleValue": 9.5}))),
        },
        TableCase {
            what: "filtered with <=: the rows at or below it",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Threshold", "operator": "<=", "value": {"doubleValue": 1.5}},
            ]}),
            expected_rows: 3,
            expected_first: Some(("Threshold", json!({"doubleValue": 1.5}))),
        },
        TableCase {
            what: "filtered on a STRING value: only the row that wrote it",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Tag", "operator": "=", "value": {"stringValue": "alpha"}},
            ]}),
            expected_rows: 1,
            expected_first: Some(("Tag", json!({"stringValue": "alpha"}))),
        },
        TableCase {
            what: "filtered with !=: every row that did not write it",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Tag", "operator": "!=", "value": {"stringValue": "alpha"}},
            ]}),
            expected_rows: 3,
            expected_first: Some(("Tag", json!({"stringValue": "unset-tag"}))),
        },
        TableCase {
            // `Count` is not selected and every instance leaves it at the
            // INTEGER default 7, compared here against a DOUBLE: the two widths
            // are one ordered domain.
            what: "filtered on a property the rows do not carry, across numeric widths",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Count", "operator": "=", "value": {"doubleValue": 7.0}},
            ]}),
            expected_rows: 4,
            expected_first: None,
        },
        TableCase {
            what: "two filters: both have to hold",
            conditions: json!({"propertyFilters": [
                {"propertyName": "Threshold", "operator": "<=", "value": {"doubleValue": 1.5}},
                {"propertyName": "Tag", "operator": "=", "value": {"stringValue": "alpha"}},
            ]}),
            expected_rows: 1,
            expected_first: Some(("Tag", json!({"stringValue": "alpha"}))),
        },
    ];

    for TableCase {
        what,
        conditions,
        expected_rows,
        expected_first,
    } in cases
    {
        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "componentTypeId": BASE,
                    "selectedProperties": ["Tag", "Threshold"],
                    "tabularConditions": conditions,
                }),
            )
            .await;
        assert_eq!(read.status, 200, "case {what:?}: expected 200, got {read:?}");

        let tables = read.member("tabularPropertyValues");
        let table = tables
            .as_array()
            .and_then(|tables| tables.first())
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            table.len(),
            expected_rows,
            "case {what:?}: expected {expected_rows} row(s), got {read:?}"
        );
        assert!(
            read.member("propertyValues").is_null(),
            "case {what:?}: expected a table and no propertyValues map, got {read:?}"
        );
        if let Some((column, value)) = expected_first {
            assert_eq!(
                table[0][column], value,
                "case {what:?}: expected the first row's {column} to be {value}, got {read:?}"
            );
        }
        // A row carries the SELECTED columns and nothing else, even where a
        // filter or an ordering key named another property.
        for (index, row) in table.iter().enumerate() {
            let columns: Vec<String> = row
                .as_object()
                .map(|row| row.keys().cloned().collect())
                .unwrap_or_default();
            assert!(
                columns
                    .iter()
                    .all(|column| column == "Tag" || column == "Threshold"),
                "case {what:?}, row {index}: expected only the selected columns, got {row:?}"
            );
        }
    }
}

/// RULE: a tabular answer is paged too — `maxResults` caps the rows a page
/// carries and the `nextToken` resumes at the next row of the same sequence.
#[tokio::test]
async fn a_tabular_answer_is_paged_over_its_rows() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let mut seen = 0usize;
    let mut next_token: Option<String> = None;
    for page_index in 0..4 {
        let mut body = json!({
            "componentTypeId": BASE,
            "selectedProperties": ["Tag", "Threshold"],
            "tabularConditions": {"orderBy": [{"propertyName": "Threshold"}]},
            "maxResults": 2,
        });
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let page = emulator.get_property_value(WORKSPACE, body).await;
        assert_eq!(
            page.status, 200,
            "page {page_index}: expected 200, got {page:?}"
        );
        let rows = page.member("tabularPropertyValues")[0]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            rows.len() <= 2,
            "page {page_index}: expected at most the 2 rows asked for, got {page:?}"
        );
        seen += rows.len();
        next_token = page
            .member("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, 4,
        "following every nextToken: expected all 4 rows exactly once, got {seen}"
    );
    assert!(
        next_token.is_none(),
        "following every nextToken: expected the last page to carry none, got {next_token:?}"
    );
}

/// RULE: a request that names a `propertyGroupName` reads the properties of
/// that group, and a selection outside it is refused.
#[tokio::test]
async fn a_property_group_restricts_the_selection() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let inside = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "propertyGroupName": "Readings",
                "selectedProperties": ["Tag", "Threshold"],
            }),
        )
        .await;
    assert_eq!(
        inside.status, 200,
        "a selection inside the group: expected 200, got {inside:?}"
    );
    assert_eq!(
        inside.member("propertyValues")["Tag"]["propertyValue"]["stringValue"],
        json!("alpha"),
        "a selection inside the group: expected the written value, got {inside:?}"
    );

    let outside = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "propertyGroupName": "Readings",
                "selectedProperties": ["Count"],
            }),
        )
        .await;
    assert_eq!(
        outside.status, 400,
        "a selection outside the group: expected 400, got {outside:?}"
    );
}

/// One refused request: what it asks for, and the status and error code it has
/// to be answered with.
#[derive(Debug)]
struct Refusal {
    what: &'static str,
    workspace_id: &'static str,
    body: Value,
    status: u16,
    error: &'static str,
}

/// RULE: a request that names something the store does not hold is a 404
/// `ResourceNotFoundException`; one whose members cannot address a property, or
/// carry a condition this service does not recognise, is a 400
/// `ValidationException`. Neither is a 501, and neither reports an empty value.
#[tokio::test]
async fn a_request_that_addresses_nothing_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let refusals = vec![
        Refusal {
            what: "an unknown workspace",
            workspace_id: "no-such-workspace",
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "an unknown entity",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "no-such-entity",
                "componentName": "Spec",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "an unknown component of a known entity",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-1",
                "componentName": "NoSuchComponent",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "a componentPath that addresses no composite component",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-4",
                "componentPath": "Spec/missing",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "an unknown component type",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": "com.example.absent",
                "selectedProperties": ["Tag"],
                "tabularConditions": {},
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "an unknown property group",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "propertyGroupName": "NoSuchGroup",
                "selectedProperties": ["Tag"],
            }),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            what: "an entity with no component named",
            workspace_id: WORKSPACE,
            body: json!({"entityId": "unit-1", "selectedProperties": ["Tag"]}),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "neither an entity nor a component type",
            workspace_id: WORKSPACE,
            body: json!({"selectedProperties": ["Tag"]}),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a selected property the addressed component does not have",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "selectedProperties": ["NoSuchProperty"],
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a component type query with no tabularConditions",
            workspace_id: WORKSPACE,
            body: json!({"componentTypeId": BASE, "selectedProperties": ["Tag"]}),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "maxResults below the documented minimum of 1",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "selectedProperties": ["Tag"],
                "maxResults": 0,
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a nextToken this query did not mint",
            workspace_id: WORKSPACE,
            body: json!({
                "entityId": "unit-1",
                "componentName": "Spec",
                "selectedProperties": ["Tag"],
                "nextToken": "6e6f742d612d746f6b656e",
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a filter operator this service does not recognise",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": BASE,
                "selectedProperties": ["Tag"],
                "tabularConditions": {"propertyFilters": [
                    {"propertyName": "Tag", "operator": "~=", "value": {"stringValue": "a"}},
                ]},
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a filter with no value to compare against",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": BASE,
                "selectedProperties": ["Tag"],
                "tabularConditions": {"propertyFilters": [
                    {"propertyName": "Tag", "operator": "="},
                ]},
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "a filter on a property the addressed scope does not have",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": BASE,
                "selectedProperties": ["Tag"],
                "tabularConditions": {"propertyFilters": [
                    {"propertyName": "NoSuchProperty", "operator": "=",
                     "value": {"stringValue": "a"}},
                ]},
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "an orderBy on a property the addressed scope does not have",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": BASE,
                "selectedProperties": ["Tag"],
                "tabularConditions": {"orderBy": [{"propertyName": "NoSuchProperty"}]},
            }),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            what: "an orderBy order outside the modelled Order enum",
            workspace_id: WORKSPACE,
            body: json!({
                "componentTypeId": BASE,
                "selectedProperties": ["Tag"],
                "tabularConditions": {"orderBy": [
                    {"propertyName": "Tag", "order": "SIDEWAYS"},
                ]},
            }),
            status: 400,
            error: "ValidationException",
        },
    ];

    for refusal in refusals {
        let read = emulator
            .get_property_value(refusal.workspace_id, refusal.body.clone())
            .await;
        assert_eq!(
            read.status, refusal.status,
            "case {:?}: expected status {}, got {read:?}",
            refusal.what, refusal.status
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some(refusal.error),
            "case {:?}: expected the error code {}, got {read:?}",
            refusal.what,
            refusal.error
        );
    }
}

/// The workspace the two cases below seed for themselves: one property whose
/// stored values differ in KIND across the instances, and two `LONG` values a
/// double cannot tell apart.
const EDGE_WORKSPACE: &str = "propedges";

/// A workspace holding one component type with one property, and one entity per
/// `value` given — each writing that value to the property.
async fn seed_values(emulator: &Emulator, property: &str, values: &[(&str, Value)]) {
    let created = emulator.create_workspace(EDGE_WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );
    let created = emulator
        .create_component_type(
            EDGE_WORKSPACE,
            "com.example.edge",
            json!({"propertyDefinitions": {property: {"dataType": {"type": "STRING"}}}}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType: expected 200, got {created:?}"
    );
    for (entity_id, value) in values {
        let created = emulator
            .create_entity(
                EDGE_WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {"Spec": {
                        "componentTypeId": "com.example.edge",
                        "properties": {property: {"value": value}},
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

/// RULE: an ordering places every row, whatever kinds the stored values are, and
/// two reads of unchanged state order them the same way — values of one kind
/// order by value, and kinds order against each other (numbers, then text, then
/// flags).
#[tokio::test]
async fn an_ordering_over_values_of_different_kinds_is_total() {
    let emulator = Emulator::new();
    seed_values(
        &emulator,
        "Reading",
        &[
            ("flag", json!({"booleanValue": true})),
            ("number", json!({"doubleValue": 3.0})),
            ("small", json!({"integerValue": 1})),
            ("text", json!({"stringValue": "high"})),
        ],
    )
    .await;

    let request = json!({
        "componentTypeId": "com.example.edge",
        "selectedProperties": ["Reading"],
        "tabularConditions": {"orderBy": [{"propertyName": "Reading", "order": "ASCENDING"}]},
    });
    let read = emulator
        .get_property_value(EDGE_WORKSPACE, request.clone())
        .await;
    assert_eq!(read.status, 200, "mixed kinds: expected 200, got {read:?}");

    let rows = read.member("tabularPropertyValues")[0]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rows.len(),
        4,
        "mixed kinds: expected a row per instance, got {read:?}"
    );
    let expected: Vec<Value> = vec![
        json!({"integerValue": 1}),
        json!({"doubleValue": 3.0}),
        json!({"stringValue": "high"}),
        json!({"booleanValue": true}),
    ];
    for (index, value) in expected.iter().enumerate() {
        assert_eq!(
            rows[index]["Reading"], *value,
            "mixed kinds, row {index}: expected {value}, got {read:?}"
        );
    }

    let again = emulator
        .get_property_value(EDGE_WORKSPACE, request.clone())
        .await;
    assert_eq!(
        read.body, again.body,
        "mixed kinds: expected two reads to order the rows the same, got {read:?} then {again:?}"
    );
}

/// RULE: a filter compares the values themselves, so two `LONG`s a double
/// cannot tell apart are still the distinct values they are.
#[tokio::test]
async fn a_filter_compares_long_values_exactly() {
    let emulator = Emulator::new();
    // 2^53 and 2^53 + 1: the first pair of whole numbers a double cannot
    // distinguish.
    seed_values(
        &emulator,
        "Serial",
        &[
            ("exact", json!({"longValue": 9_007_199_254_740_993i64})),
            ("neighbour", json!({"longValue": 9_007_199_254_740_992i64})),
        ],
    )
    .await;

    let read = emulator
        .get_property_value(
            EDGE_WORKSPACE,
            json!({
                "componentTypeId": "com.example.edge",
                "selectedProperties": ["Serial"],
                "tabularConditions": {"propertyFilters": [
                    {"propertyName": "Serial", "operator": "=",
                     "value": {"longValue": 9_007_199_254_740_993i64}},
                ]},
            }),
        )
        .await;
    assert_eq!(read.status, 200, "exact LONG: expected 200, got {read:?}");

    let rows = read.member("tabularPropertyValues")[0]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rows.len(),
        1,
        "exact LONG: expected only the row carrying that value, got {read:?}"
    );
    assert_eq!(
        rows[0]["Serial"],
        json!({"longValue": 9_007_199_254_740_993i64}),
        "exact LONG: expected the row to carry the value filtered for, got {read:?}"
    );
}
