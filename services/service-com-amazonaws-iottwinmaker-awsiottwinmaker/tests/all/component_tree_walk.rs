// SPDX-License-Identifier: CC0-1.0

//! The level-by-level walk `ListComponents` serves, end to end over HTTP.
//!
//! RULE under test: one level of an entity's component tree holds one row per
//! DISTINCT next path segment the entity holds a composite component at or
//! beneath it — the stored record when one stands at that segment, else the path
//! step itself, carrying the component type the declaration its path resolves to
//! names and no description or property group of its own — so walking a level at
//! a time from the entity's top-level components reaches exactly the composite
//! component paths the entity holds, and no more.
//!
//! The rows differ along what the rule generalises over: HOW MANY steps of a
//! stored path were never created (one, two, the whole chain), the DEPTH the
//! uncreated step sits at, whether created and uncreated steps share a level,
//! and how many stored paths run through one uncreated step.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp, REGION};

/// The workspace every case below is created in.
const WORKSPACE: &str = "treewalk";

/// The component the entities below hang their composite components under.
const COMPONENT: &str = "Sensor";

/// The component types the entities instantiate: a chain deep enough for a path
/// of four segments, so a case can leave one, two or three of its steps
/// uncreated.
///
/// `com.example.sensor` declares `inner` (a leaf type), `nested`
/// (`com.example.deep`, which declares `leaf`) and `chain`
/// (`com.example.deeper`, which declares `mid` -> `com.example.deep`).
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            "com.example.inner",
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            "com.example.deep",
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": "com.example.inner"}}}),
        ),
        (
            "com.example.deeper",
            json!({"compositeComponentTypes": {"mid": {"componentTypeId": "com.example.deep"}}}),
        ),
        (
            "com.example.sensor",
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": "com.example.inner"},
                "nested": {"componentTypeId": "com.example.deep"},
                "chain": {"componentTypeId": "com.example.deeper"},
            }}),
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

/// Create one entity whose only composite components are `composite_paths`,
/// each created with no members of its own.
async fn seed_entity(emulator: &Emulator, entity_id: &str, composite_paths: &[&str]) {
    let mut composite = serde_json::Map::new();
    for path in composite_paths {
        composite.insert((*path).to_string(), json!({}));
    }
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": "com.example.sensor"}},
                "compositeComponents": Value::Object(composite),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity {entity_id:?}: expected 200, got {created:?}"
    );
}

/// The `componentSummaries` of one response.
fn summaries(response: &Resp) -> Vec<Value> {
    response
        .member("componentSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `componentName` of each summary, in the order they were reported.
fn names(response: &Resp) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|summary| {
            summary["componentName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every composite component path a caller REACHES by walking one level per
/// call, starting from the entity's top-level components.
///
/// Each row of a level is followed through its own reported `componentPath`,
/// which is the hop the next call is made with, so what this collects is exactly
/// what the documented discipline can see.
async fn walk(emulator: &Emulator, entity_id: &str) -> BTreeSet<String> {
    let top = emulator.list_components(WORKSPACE, entity_id, json!({})).await;
    assert_eq!(
        top.status, 200,
        "{entity_id}: expected 200 for the top level, got {top:?}"
    );
    let mut frontier: Vec<String> = names(&top);
    let mut reached: BTreeSet<String> = BTreeSet::new();
    // The walk is over a tree, so it terminates; the bound only keeps a
    // regression that reports a row under itself from hanging the suite.
    let mut hops = 0;
    while let Some(path) = frontier.pop() {
        hops += 1;
        assert!(
            hops <= 64,
            "{entity_id}: the walk must terminate, reached {reached:?}"
        );
        let level = emulator
            .list_components(WORKSPACE, entity_id, json!({"componentPath": path}))
            .await;
        assert_eq!(
            level.status, 200,
            "{entity_id}: expected 200 at componentPath {path:?}, got {level:?}"
        );
        for summary in summaries(&level) {
            let child = summary["componentPath"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("{entity_id}: a composite component reports its own path, got {summary:?}")
                })
                .to_string();
            assert!(
                child.starts_with(&format!("{path}/")),
                "{entity_id}: a row of {path:?} is one segment below it, got {child:?}"
            );
            assert!(
                reached.insert(child.clone()),
                "{entity_id}: {child:?} was reported by two levels"
            );
            frontier.push(child);
        }
    }
    reached
}

/// Every path a created composite component makes addressable: the path itself
/// and each of its composite ancestors, which are the steps that lead to it.
fn addressable(composite_paths: &[&str]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for path in composite_paths {
        let segments: Vec<&str> = path.split('/').collect();
        for depth in 2..=segments.len() {
            paths.insert(segments[..depth].join("/"));
        }
    }
    paths
}

/// RULE (where a step's component type comes from): a step is reported with the
/// component type the declarations resolve it to, READ at the time of the call —
/// so a step whose declaration has stopped standing is still a row of its level,
/// because the stored record beneath it is still reachable, and it reports no
/// component type rather than hiding the record the walk reaches through it.
#[tokio::test]
async fn a_step_whose_declaration_stopped_standing_is_still_a_row() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    seed_entity(&emulator, "stranded", &["Sensor/nested/leaf"]).await;

    // While the declaration stands, the step reports the type it names.
    let before = emulator
        .list_components(WORKSPACE, "stranded", json!({"componentPath": COMPONENT}))
        .await;
    assert_eq!(
        before.status, 200,
        "expected 200 before the declaration is dropped, got {before:?}"
    );
    let step = summaries(&before)
        .into_iter()
        .find(|summary| summary["componentName"] == "nested")
        .unwrap_or_else(|| panic!("expected the step row, got {before:?}"));
    assert_eq!(
        step["componentTypeId"].as_str(),
        Some("com.example.deep"),
        "expected the declared componentTypeId, got {step:?}"
    );

    // `UpdateComponentType` replaces `compositeComponentTypes` whole, so an
    // empty map takes the declaration the step stood on away.
    let updated = emulator
        .update_component_type(
            WORKSPACE,
            "com.example.sensor",
            json!({"compositeComponentTypes": {}}),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateComponentType, got {updated:?}"
    );

    let after = emulator
        .list_components(WORKSPACE, "stranded", json!({"componentPath": COMPONENT}))
        .await;
    assert_eq!(
        after.status, 200,
        "expected 200 after the declaration is dropped, got {after:?}"
    );
    assert_eq!(
        names(&after),
        vec!["nested".to_string()],
        "the stored record is still reached through this step, so it is still a row, got \
         {after:?}"
    );
    let step = summaries(&after)
        .into_iter()
        .find(|summary| summary["componentName"] == "nested")
        .unwrap_or_else(|| panic!("expected the step row, got {after:?}"));
    assert_eq!(
        step["componentTypeId"].as_str(),
        Some(""),
        "a step with no declaration left names no component type, got {step:?}"
    );
    assert_eq!(
        step["componentPath"].as_str(),
        Some("Sensor/nested"),
        "the step is still addressed by its own whole path, got {step:?}"
    );

    // And the record beneath it is still reachable, reported from the store
    // rather than from the declaration that stopped standing.
    let deeper = emulator
        .list_components(
            WORKSPACE,
            "stranded",
            json!({"componentPath": "Sensor/nested"}),
        )
        .await;
    assert_eq!(
        deeper.status, 200,
        "expected 200 one level deeper, got {deeper:?}"
    );
    assert_eq!(
        names(&deeper),
        vec!["leaf".to_string()],
        "expected the stored composite component, got {deeper:?}"
    );
    let leaf = summaries(&deeper)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("expected the stored row, got {deeper:?}"));
    assert_eq!(
        leaf["componentTypeId"].as_str(),
        Some("com.example.inner"),
        "a stored record reports the type it was created instantiating, got {leaf:?}"
    );
}

/// RULE: walking one level per call reaches exactly the composite component
/// paths the entity holds — every created path, and every step that leads to one
/// — whichever steps of those paths were created themselves.
#[tokio::test]
async fn the_walk_reaches_every_stored_composite_component() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases: Vec<(&str, Vec<&str>)> = vec![
        // Every step created: the walk the rule generalises from.
        (
            "allStepsCreated",
            vec!["Sensor/chain", "Sensor/chain/mid", "Sensor/chain/mid/leaf"],
        ),
        // The last step only: one uncreated step, one level down.
        ("skipOneStep", vec!["Sensor/nested/leaf"]),
        // Two uncreated steps between the component and the created path.
        ("skipTwoSteps", vec!["Sensor/chain/mid/leaf"]),
        // Created and uncreated steps sharing one level, and two created paths
        // running through one uncreated step.
        (
            "mixedLevel",
            vec![
                "Sensor/inner",
                "Sensor/chain/mid",
                "Sensor/nested/leaf",
                "Sensor/chain/mid/leaf",
            ],
        ),
        // Only the deepest path of two branches: two uncreated steps side by
        // side at the same depth.
        (
            "twoUncreatedBranches",
            vec!["Sensor/nested/leaf", "Sensor/chain/mid/leaf"],
        ),
        // No composite component at all: the boundary the rule sits on.
        ("noCompositeComponents", vec![]),
    ];

    for (entity_id, composite_paths) in cases {
        seed_entity(&emulator, entity_id, &composite_paths).await;
        let reached = walk(&emulator, entity_id).await;
        assert_eq!(
            reached,
            addressable(&composite_paths),
            "case {entity_id:?}: the walk must reach every stored composite component and \
             every step that leads to one, created or not"
        );
    }
}

/// RULE (what a row of a level says): a step that was never created is reported
/// as the step it is — its own name, its whole path, the component type its
/// declaration names — and carries no description, while a created record is
/// reported from what was stored.
#[tokio::test]
async fn an_uncreated_step_is_reported_as_the_step_it_is() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "reported",
                "entityName": "Reported",
                "components": {COMPONENT: {"componentTypeId": "com.example.sensor"}},
                "compositeComponents": {
                    "Sensor/inner": {"description": "created inline"},
                    "Sensor/nested/leaf": {"description": "two levels down"},
                    "Sensor/chain/mid/leaf": {},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity: expected 200, got {created:?}"
    );

    // Each row: the level it is reported in, its own name, the whole path it is
    // addressed by, the component type the declarations resolve it to, and the
    // description a created record carries (absent for a step nothing was
    // stored at).
    let rows: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        (
            "Sensor",
            "chain",
            "Sensor/chain",
            "com.example.deeper",
            None,
        ),
        (
            "Sensor",
            "inner",
            "Sensor/inner",
            "com.example.inner",
            Some("created inline"),
        ),
        ("Sensor", "nested", "Sensor/nested", "com.example.deep", None),
        (
            "Sensor/chain",
            "mid",
            "Sensor/chain/mid",
            "com.example.deep",
            None,
        ),
        (
            "Sensor/chain/mid",
            "leaf",
            "Sensor/chain/mid/leaf",
            "com.example.inner",
            None,
        ),
        (
            "Sensor/nested",
            "leaf",
            "Sensor/nested/leaf",
            "com.example.inner",
            Some("two levels down"),
        ),
    ];

    // The whole level each row belongs to, so a level is pinned to exactly the
    // rows the rule gives it rather than to at-least those rows.
    for level_path in ["Sensor", "Sensor/chain", "Sensor/chain/mid", "Sensor/nested"] {
        let level = emulator
            .list_components(
                WORKSPACE,
                "reported",
                json!({"componentPath": level_path}),
            )
            .await;
        assert_eq!(
            level.status, 200,
            "level {level_path:?}: expected 200, got {level:?}"
        );
        let expected: Vec<String> = rows
            .iter()
            .filter(|(path, ..)| *path == level_path)
            .map(|(_, name, ..)| (*name).to_string())
            .collect();
        assert_eq!(
            names(&level),
            expected,
            "level {level_path:?}: expected exactly this level in name order, got {level:?}"
        );

        for (_, name, component_path, component_type_id, description) in
            rows.iter().filter(|(path, ..)| *path == level_path)
        {
            let summary = summaries(&level)
                .into_iter()
                .find(|summary| summary["componentName"] == *name)
                .unwrap_or_else(|| panic!("level {level_path:?}: expected {name:?}, got {level:?}"));
            assert_eq!(
                summary["componentPath"].as_str(),
                Some(*component_path),
                "level {level_path:?} {name:?}: wrong componentPath, got {summary:?}"
            );
            assert_eq!(
                summary["componentTypeId"].as_str(),
                Some(*component_type_id),
                "level {level_path:?} {name:?}: wrong componentTypeId, got {summary:?}"
            );
            assert_eq!(
                summary["description"].as_str(),
                *description,
                "level {level_path:?} {name:?}: wrong description, got {summary:?}"
            );
            // A component has no lifecycle of its own, so every row reports the
            // status of the entity holding it.
            assert_eq!(
                summary["status"]["state"].as_str(),
                Some("ACTIVE"),
                "level {level_path:?} {name:?}: expected the entity's own state, got {summary:?}"
            );
        }
    }

    // The composite components `GetEntity` reports for the holding component are
    // the ones the walk reaches: one stored record says the same thing to both
    // readers.
    let read = emulator.get_entity(WORKSPACE, "reported").await;
    assert_eq!(read.status, 200, "expected 200 from GetEntity, got {read:?}");
    let reported: BTreeSet<String> = read.member("components")[COMPONENT]["compositeComponents"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .values()
        .map(|summary| {
            summary["componentPath"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let reached = walk(&emulator, "reported").await;
    assert!(
        reported.is_subset(&reached),
        "expected every composite component GetEntity reports to be reachable by the walk, \
         reported {reported:?}, reached {reached:?}"
    );

    // Serving those levels wrote nothing: the entity still holds exactly the
    // composite components its create persisted, so the steps the walk reported
    // were computed and not materialised.
    let state = emulator.state();
    let state = state.lock();
    let stored: Vec<&String> = state.region(REGION).workspaces[WORKSPACE].entities["reported"]
        .components[COMPONENT]
        .composite_components
        .keys()
        .collect();
    assert_eq!(
        stored,
        vec!["chain/mid/leaf", "inner", "nested/leaf"],
        "expected walking the tree to write nothing, got {stored:?}"
    );
}
