Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# The componentPath -> component resolution rule is written out four times, once per property-reading handler

_Filed: 2026-10-27T14:56:24Z_
_Operation: BatchPutPropertyValues | Class: maintainability_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule decides which stored component a `componentPath` addresses: a path of one segment names the entity's top-level component, and a longer one names the composite component held under the rest of the path inside it — the key `ComponentRecord::composite_components` uses. The crate already owns the write side of this rule in one place (`crate::state::resolve_composite_component_path`, used by CreateEntity and UpdateEntity), but the READ side is open-coded four times, in four handlers, with the same `split_once(crate::state::COMPONENT_PATH_SEPARATOR)` match, the same two lookups and the same four-line doc comment restating the rule. Only the local struct each copy builds differs (`Addressed`, `AddressedComponent`, `ScopedComponent`, `Scoped`), and every one of those carries the same two things the resolution produces: the component type the component instantiates and its property map.

Four copies of one rule can drift: a change to how a path is read (a trailing separator, an empty segment, a deeper nesting) has to be made in four places or the four property readers start disagreeing about which component a path names. Hoisting the resolution into `crate::state` — answering the `(component_type_id, properties)` pair the callers already reduce it to, with each handler keeping its own struct construction — leaves every response and every existing test byte-identical.

## Evidence

COPY 1 — src/ops/batch_put_property_values.rs:514-529:
  "fn at_path<'a>(entity: &'a crate::state::Entity, component_path: &str) -> Option<Addressed<'a>> {
       match component_path.split_once(crate::state::COMPONENT_PATH_SEPARATOR) {
           None => Some(of_component(entity, entity.components.get(component_path)?)),
           Some((component_name, composite_path)) => {
               let component = entity.components.get(component_name)?;
               let composite = component.composite_components.get(composite_path)?;
               Some(of_composite(entity, component, composite, composite_path))
           }
       }
   }"

COPY 2 — src/ops/get_property_value_history.rs:750-764:
  "fn at_path<'a>(entity: &'a crate::state::Entity, component_path: &str) -> Option<Scoped<'a>> {
       match component_path.split_once(crate::state::COMPONENT_PATH_SEPARATOR) {
           None => Some(scoped(&entity.entity_id, entity.components.get(component_path)?)),
           Some((component_name, composite_path)) => {
               let component = entity.components.get(component_name)?;
               Some(scoped_composite(&entity.entity_id, component.composite_components.get(composite_path)?))
           }
       }
   }"

COPY 3 — src/ops/get_property_value.rs:401-418:
  "fn component_at_path<'a>(entity: &'a crate::state::Entity, component_path: &str) -> Option<ScopedComponent<'a>> {
       match component_path.split_once(crate::state::COMPONENT_PATH_SEPARATOR) {
           None => Some(scoped_component(&entity.entity_id, entity.components.get(component_path)?)),
           Some((component_name, composite_path)) => {
               let component = entity.components.get(component_name)?;
               Some(scoped_composite_component(&entity.entity_id, component.composite_components.get(composite_path)?))
           }
       }
   }"

COPY 4 — src/ops/list_properties.rs:373-397:
  "fn component_at_path<'a>(entity: &'a crate::state::Entity, component_path: &str) -> Option<AddressedComponent<'a>> {
       match component_path.split_once(crate::state::COMPONENT_PATH_SEPARATOR) {
           None => { let component = entity.components.get(component_path)?; Some(AddressedComponent { component_type_id: component.component_type_id.as_deref(), properties: &component.properties }) }
           Some((component_name, composite_path)) => {
               let composite = entity.components.get(component_name)?.composite_components.get(composite_path)?;
               Some(AddressedComponent { component_type_id: Some(&composite.component_type_id), properties: &composite.properties })
           }
       }
   }"

The rule each copy restates is the same sentence, e.g. src/ops/list_properties.rs:328-332 and src/ops/get_property_value.rs:394-400: "A path of one segment names a top-level component (\"the path to the composite component, STARTING FROM the top-level component\" ...); a longer one names the composite component stored under the rest of the path inside it, which is the key `crate::state::ComponentRecord::composite_components` holds it under."

The WRITE side already lives in one place: `crate::state::resolve_composite_component_path`, called from src/ops/create_entity.rs:373-378 and src/ops/update_entity.rs:488-494.

TESTS THAT COVER THE FOUR COPIES (and must stay unchanged): tests/all/time_series_property_refusal.rs (composite addressed by `"componentPath": "Assembly/inner"` for both BatchPutPropertyValues and GetPropertyValueHistory), tests/all/time_series_property_values.rs, tests/all/property_value_read.rs, tests/all/property_listing.rs:699-711, tests/all/entity_composite_components.rs.

## Steps to Reproduce

1. grep -n 'split_once(crate::state::COMPONENT_PATH_SEPARATOR)' services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/ — four handler-local copies of one resolution appear (batch_put_property_values.rs:518, get_property_value_history.rs:751, get_property_value.rs:405, list_properties.rs:377).
2. Read each: the match arms, the two map lookups and the restated doc sentence are the same; only the local struct built differs.
3. Note that the write side of the same rule is already centralised as `crate::state::resolve_composite_component_path` (create_entity.rs:373, update_entity.rs:488), so the crate already has the place this read-side rule belongs.
4. Change how a path is read in one copy only (for example to tolerate a trailing separator) and the four property readers disagree about which component the same `componentPath` names.
