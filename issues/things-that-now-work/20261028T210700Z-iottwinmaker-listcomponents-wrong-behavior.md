Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# A composite component whose intermediate path step was never created is unreachable by the level-by-level walk ListComponents promises

_Filed: 2026-10-28T21:07:00Z_
_Operation: ListComponents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListComponents` is documented by its own handler as the operation a caller uses to "walk a nested component tree one level per call". The level it reports is computed by `components_under`, which reports ONLY the composite components whose stored key is exactly one segment longer than the requested path.

The same function, two blocks above, states and implements the OPPOSITE existence rule for the path it is given: an intermediate step of a composite component's path need not have been created itself, so the path names a composite component when one is stored AT it OR anywhere BENEATH it. So for an entity whose only stored composite component is `Sensor/nested/leaf` (nothing stored at `Sensor/nested`):

* `ListComponents(componentPath="Sensor/nested")` answers 200 with `leaf` — the path resolves, because something is stored beneath it;
* `ListComponents(componentPath="Sensor")` answers 200 with an EMPTY `componentSummaries` — the key `nested/leaf` carries a separator, so the filter drops it and nothing stands in for `nested`;
* `GetEntity` on the same entity reports `components.Sensor.compositeComponents` containing that composite component, because `components_response` walks `composite_components` at EVERY depth.

The two readers of one stored record therefore disagree: `GetEntity` says `Sensor` holds a composite component, and the level the walk is supposed to use says `Sensor` holds none. A caller that follows the documented "one level per call" discipline can never reach `Sensor/nested/leaf`, because the only hop that would take it there (`nested`) is never reported, even though the handler itself treats `Sensor/nested` as an existing scope.

The root cause is that `components_under` hand-rolls its own path walk (`split_once` + `contains_key` + `starts_with`) instead of going through `crate::state::component_at_path`, the rule the other four path readers share — which is why its existence rule and its child-listing rule can disagree with each other inside one function.

## Evidence

THE RULE THE HANDLER STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_components.rs:9-14:
```
//! The listed SCOPE is one level of the entity's component tree, which is what
//! `componentPath` selects: a request that names none lists the entity's TOP-LEVEL
//! components, and one that names a path lists the composite components stored directly
//! inside the component that path leads to ("the path to the composite component,
//! starting from the top-level component"), so a caller walks a nested component tree one
//! level per call instead of reading it all back through `GetEntity`, ...
```
and src/ops/list_components.rs:242-244:
```
        // ONE level of the entity's component tree: its top-level components
        // when the request names no path, else the composite components stored
        // directly inside the component the path leads to.
```

THE EXISTENCE RULE THE SAME FUNCTION APPLIES — src/ops/list_components.rs:424-437:
```
            // The path has to name a composite component of this component.
            // Nothing requires the intermediate steps of a composite
            // component's path to have been created themselves, so the path
            // names one when a composite component is stored AT it or anywhere
            // BENEATH it.
            if !component.composite_components.contains_key(below)
                && !component
                    .composite_components
                    .keys()
                    .any(|key| key.starts_with(&nested))
            {
                return None;
            }
            nested
```

THE CHILD-LISTING RULE THAT CONTRADICTS IT — src/ops/list_components.rs:440-450:
```
    Some(
        component
            .composite_components
            .iter()
            .filter(|(key, _)| {
                key.strip_prefix(prefix.as_str())
                    .is_some_and(|name| !name.is_empty() && !name.contains(separator))
            })
            .map(|(_, composite)| Listed::Composite(composite))
            .collect(),
    )
```
For `componentPath="Sensor"`, `prefix` is the empty string (src/ops/list_components.rs:420-422: `let prefix = match below { None => String::new(), ...`), so the stored key `nested/leaf` strips to `nested/leaf`, which `contains(separator)` — it is filtered out, and no entry is reported for `nested`.

THE SIBLING READER THAT DOES REPORT IT — src/ops/get_entity.rs:293-305:
```
            let composite_components: ::std::collections::HashMap<String, ComponentSummary> =
                component
                    .composite_components
                    .values()
                    .map(|composite| {
                        (
                            composite.component_name.clone(),
                            composite_component_summary(workspace, composite, status),
                        )
                    })
                    .collect();
```
(every depth, not one level), so `GetEntity` reports the composite component that `ListComponents` at `Sensor` omits.

THE SHARED PATH RULE THIS FUNCTION BYPASSES — src/state.rs:3651-3657 (`component_at_path`):
```
/// This is the READ side of the rule
/// [`resolve_composite_component_path`] writes with: every property reader that
/// accepts a `componentPath` resolves it here, so one change to how a path is
/// read reaches all of them at once and they cannot disagree about which
/// component one path names.
```
`components_under` does not call it; `list_properties.rs:368-377`, `batch_put_property_values.rs:513-525` and `get_property_value_history.rs` all do.

TEST COVERAGE: the very entity that exposes this is already seeded — tests/all/component_listing.rs:99-109:
```
        (
            // Only the DEEP composite component was created: nothing requires
            // the intermediate step of its path to have been created too.
            "sparse",
            json!({
                "entityId": "sparse",
                "entityName": "Sparse",
                "components": {"Sensor": {"componentTypeId": "com.example.sensor"}},
                "compositeComponents": {"Sensor/nested/leaf": {}},
            }),
        ),
```
and the row table of `a_page_reports_one_level_of_the_component_tree` (tests/all/component_listing.rs:164-253) exercises `"sparse"` only at `Some("Sensor/nested")` ("ancestorOfACreatedPath", lines 230-242). No row lists the `"sparse"` entity at `Some("Sensor")` or with no path at all, so nothing pins the empty level this report is about.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.inner", propertyDefinitions={"Pressure": {"dataType": {"type": "DOUBLE"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.deep", compositeComponentTypes={"leaf": {"componentTypeId": "com.example.inner"}}).
4. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", compositeComponentTypes={"nested": {"componentTypeId": "com.example.deep"}}).
5. CreateEntity(workspaceId="ws", entityId="sparse", entityName="Sparse", components={"Sensor": {"componentTypeId": "com.example.sensor"}}, compositeComponents={"Sensor/nested/leaf": {}}) -> 200 (the intermediate step `Sensor/nested` is deliberately not created).
6. ListComponents(workspaceId="ws", entityId="sparse") -> 200 with one row, `Sensor`.
7. ListComponents(workspaceId="ws", entityId="sparse", componentPath="Sensor") -> 200 with `componentSummaries: []`: the walk has nowhere to go from `Sensor`.
8. ListComponents(workspaceId="ws", entityId="sparse", componentPath="Sensor/nested") -> 200 with one row, `leaf` (componentPath `Sensor/nested/leaf`) — the level the previous call said was empty does have a child, and the handler accepts `Sensor/nested` as an existing scope.
9. GetEntity(workspaceId="ws", entityId="sparse") -> 200 and `components.Sensor.compositeComponents` carries `leaf`, so the stored state the empty level was computed from does hold a composite component under `Sensor`.
