Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# The componentTypeId instance scan is written out twice, once in GetPropertyValue and once in GetPropertyValueHistory

_Filed: 2026-10-28T18:09:32Z_
_Operation: GetPropertyValue | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "the scope of a componentTypeId query is every INSTANCE of that component type in the workspace — a top-level component whose type is in the instance set, plus every composite component whose type is, walked in (entityId, componentName) order and capped at a declared ceiling" is implemented as two independent copies of the same nested loop, one per property-reading handler. The two spans are structurally identical: same `component_type_instance_ids` seed, same outer/inner iteration, same two membership tests, same push-per-match, same `components.len() > CEILING` guard in the same position inside the inner loop. Only three identifiers differ (the two scoped-component constructors, the ceiling constant and the refusal wording).

That is the same shape of duplication this crate has already paid for once: the `componentPath` -> component resolution rule was written out four times until it was hoisted into `crate::state::component_at_path`, and the `maxResults` page-size rule was copied into eight handlers until it was hoisted into `crate::state::page_size`. This copy can drift the same way, and the specific thing it can drift on — whether the scan walks an entity's COMPOSITE components — is a divergence this crate has already shipped elsewhere (ListEntities' componentTypeId filter walks only top-level components while the externalId filter beside it walks composites). A change to this scan today has to be made twice by hand with no compile-time link between the copies.

Fix shape: one helper beside `crate::state::component_at_path` that yields the instances of a component type as (entity id, `ComponentAtPath`) pairs in the store's own key order; each handler keeps its own ceiling, its own refusal message and its own scoped struct by mapping over the helper's output. No response byte changes and no existing test changes.

## Evidence

COPY 1 — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value.rs:496-526:
```
    let instance_type_ids = workspace.component_type_instance_ids(component_type_id);
...
    let mut components = Vec::new();
    for entity in workspace.entities.values() {
        for component in entity.components.values() {
            if component
                .component_type_id
                .as_deref()
                .is_some_and(|instantiated| instance_type_ids.contains(instantiated))
            {
                components.push(scoped_component(&entity.entity_id, component));
            }
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite_component(&entity.entity_id, composite));
                }
            }
            if components.len() > MAX_TABLE_ROWS {
                return Err(invalid(format!(
                    "componentType {component_type_id} has more instances in workspace {} than \
                     the {MAX_TABLE_ROWS} rows one table is built from; name an entityId, a \
                     narrower componentTypeId or a propertyFilter",
                    input.workspace_id
                )));
            }
        }
    }
```

COPY 2 — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value_history.rs:717-742 (the same loop, three identifiers apart):
```
    let instance_type_ids = workspace.component_type_instance_ids(component_type_id);
    let mut components = Vec::new();
    for entity in workspace.entities.values() {
        for component in entity.components.values() {
            if component
                .component_type_id
                .as_deref()
                .is_some_and(|instantiated| instance_type_ids.contains(instantiated))
            {
                components.push(scoped(&entity.entity_id, component));
            }
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite(&entity.entity_id, composite));
                }
            }
            if components.len() > MAX_COMPONENTS_IN_SCOPE {
                return Err(invalid(format!(
                    "componentType {component_type_id} has more instances in workspace {} than \
                     the {MAX_COMPONENTS_IN_SCOPE} one request reads; name an entityId or a \
                     narrower componentTypeId",
                    input.workspace_id
                )));
            }
        }
    }
```

THE PRECEDENT IN THIS CRATE (the read-side path rule, already hoisted) — src/state.rs:3610-3616:
```
/// This is the READ side of the rule
/// [`resolve_composite_component_path`] writes with: every property reader that
/// accepts a `componentPath` resolves it here, so one change to how a path is
/// read reaches all of them at once and they cannot disagree about which
/// component one path names.
```

TESTS THAT COVER BOTH COPIES AND MUST STAY UNCHANGED: tests/all/property_value_read.rs:417 ("RULE: a `componentTypeId` query is answered with a table — a row per ...", with component-type-scoped requests at :513, :571, :804-:855 and a composite instance at :79) and tests/all/time_series_property_values.rs:752 ("RULE: a cross-entity read — one naming a `componentTypeId` and no entity —", request at :785, composite instance at :82, over-ceiling/absent-type cases at :958 and :1138).

## Steps to Reproduce

1. Read services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value.rs:496-526.
2. Read services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value_history.rs:717-742 — the same nested loop, the same two membership tests and the same in-loop ceiling guard, differing only in the two constructor names, the ceiling constant and the refusal wording.
3. Change the scan in one copy (for example, stop walking `component.composite_components`, or move the ceiling guard out of the inner loop) and observe that the other copy still carries the old version, with nothing in the build linking them.
4. Confirm the drift risk is real for this exact rule: src/ops/list_entities.rs:300-308 walks only top-level components for the `componentTypeId` filter while `carries_external_id` beside it (list_entities.rs:321-337) walks composites too.
