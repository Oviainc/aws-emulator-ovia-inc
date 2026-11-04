Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# UpdateComponent can introduce a reserved `tags` property key through children and variants

_Filed: 2026-10-29T18:56:27Z_
_Operation: UpdateComponent | Class: validation_

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateComponent` re-applies the reserved-name rule its own comment calls "The same reserved-name rule `CreateComponent` enforces", but, like the create, only over `properties`, `overrides`, `bindingProperties` and `collectionProperties`. A patch whose `children` or `variants` carry a `tags` key is accepted and merged into the stored record, so an update can introduce exactly the property name the model forbids for `ComponentChild$properties`, `ComponentVariant$variantValues` and `ComponentVariant$overrides` — after which `GetComponent` reports it. The check for the four top-level maps and the missing check for the nested ones sit in the same loop, and the helper they would use (`crate::state::carries_reserved_name<V>`) is already generic over the map's value type.

## Evidence

Smithy model (`smithy_query(query="shape", shape="ComponentChild")`), member `properties`:
  "Describes the properties of the child component. You can't specify <code>tags</code> as a valid property for <code>properties</code>."
Smithy model (`smithy_query(query="shape", shape="ComponentVariant")`), member `overrides`:
  "... You can't specify <code>tags</code> as a valid property for <code>overrides</code>."
and member `variantValues`:
  "... You can't specify <code>tags</code> as a valid property for <code>variantValues</code>."

Handler, services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/update_component.rs:200-231:
  "// The same reserved-name rule `CreateComponent` enforces: a component's\n// property maps may not carry a `tags` entry, so an update may not\n// introduce one either."
  `for (member, carries) in [("properties", ...), ("overrides", ...), ("bindingProperties", ...), ("collectionProperties", ...)]`
and the unchecked merges at update_component.rs:314-328:
  `if let Some(variants) = patch.variants { component.variants = variants; }`
  `if patch.children.is_some() { component.children = patch.children; }`

The crate's contradicting claim, services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1053-1056:
  "... `ComponentVariant$overrides` and `ComponentChild$properties` carry no such statement, so a key named `tags` there is stored as given rather than refused on a rule the model does not make."

Test coverage: tests/all/component_lifecycle.rs:1099-1116 drives the update half of `a_reserved_property_name_is_refused` over the same four rows only ("// The same rule on an update, judged against the stored record it would leave behind."); no test pins acceptance of a nested `tags` key.

## Steps to Reproduce

1. Create a component: POST /app/d1234abcd/environment/staging/components with the six required `CreateComponentData` members
2. PATCH /app/d1234abcd/environment/staging/components/{id} with updatedComponent = {"children":[{"componentType":"Text","name":"Child","properties":{"tags":{"value":"x"}}}]}
3. Observe HTTP 200, while the same key sent as updatedComponent.properties is refused with HTTP 400 at update_component.rs:234
4. GET the component and see `children[0].properties.tags` persisted
5. Repeat with updatedComponent = {"variants":[{"variantValues":{"tags":"x"}}]} — also accepted
