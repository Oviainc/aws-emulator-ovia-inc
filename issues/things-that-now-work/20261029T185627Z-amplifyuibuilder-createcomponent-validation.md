Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# CreateComponent stores a reserved `tags` property key inside children and variants, which the model forbids

_Filed: 2026-10-29T18:56:27Z_
_Operation: CreateComponent | Class: validation_

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler enforces the model's "You can't specify `tags` as a valid property" rule on exactly four maps of `CreateComponentData` (`properties`, `overrides`, `bindingProperties`, `collectionProperties`) and on nothing else. The Smithy model documents the SAME rule for three further maps the create payload carries: `ComponentChild$properties` (reachable through `componentToCreate.children`, recursively), `ComponentVariant$variantValues` and `ComponentVariant$overrides` (reachable through `componentToCreate.variants`). A create naming `tags` in any of those answers HTTP 200 and the key is persisted and reported back by `GetComponent`/`ExportComponents`, whereas the identical key one level up is refused with 400.

The crate is not merely silent about this: `crate::state::carries_reserved_name` documents the omission on a factual claim the model contradicts, asserting that `ComponentVariant$overrides` and `ComponentChild$properties` "carry no such statement". They do carry it, verbatim. The helper that implements the check is generic (`carries_reserved_name<V>`), so both nested maps can be passed to the existing helper; only the call sites are missing.

## Evidence

Smithy model (`smithy_query(query="shape", shape="ComponentChild")`), member `properties`:
  "Describes the properties of the child component. You can't specify <code>tags</code> as a valid property for <code>properties</code>."

Smithy model (`smithy_query(query="shape", shape="ComponentVariant")`), member `variantValues`:
  "The combination of variants that comprise this variant. You can't specify <code>tags</code> as a valid property for <code>variantValues</code>."
and member `overrides`:
  "The properties of the component variant that can be overriden when customizing an instance of the component. You can't specify <code>tags</code> as a valid property for <code>overrides</code>."

Handler, services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/create_component.rs:162-185 — only four maps are inspected, and `children`/`variants` are not among them:
  "// `Component$properties`, `$overrides`, `$bindingProperties` and\n// `$collectionProperties` each document \"You can't specify `tags` as a\n// valid property\" for that map ... so a request naming it is refused rather than stored."
  `for (member, carries) in [("properties", ...), ("overrides", ...), ("bindingProperties", ...), ("collectionProperties", ...)]`
and create_component.rs:265-266 stores the unchecked maps as given:
  `children: data.children,`
  `variants: data.variants,`

The crate's own contradicting claim, services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/state.rs:1053-1056:
  "/// Only the four maps the model documents the rule for are checked by the\n/// handlers; `ComponentVariant$overrides` and `ComponentChild$properties` carry\n/// no such statement, so a key named `tags` there is stored as given rather than\n/// refused on a rule the model does not make."

Test coverage: tests/all/component_lifecycle.rs:1059 `a_reserved_property_name_is_refused` pins the 400 for the four top-level maps only — tests/all/component_lifecycle.rs:1064: "// One row per map the model documents the rule for; the value shape differs per map." No test asserts that a nested `tags` key is accepted, so nothing in the crate pins the current behaviour.

## Steps to Reproduce

1. POST /app/d1234abcd/environment/staging/components with componentToCreate = {"name":"Nested","componentType":"View","properties":{"label":{"value":"hi"}},"variants":[],"overrides":{},"bindingProperties":{},"children":[{"componentType":"Text","name":"Child","properties":{"tags":{"value":"x"}}}]}
2. Observe HTTP 200 and an `entity` whose `children[0].properties` carries the `tags` key; the same key placed in `componentToCreate.properties` is refused with HTTP 400 by create_component.rs:187
3. Repeat with componentToCreate.variants = [{"variantValues":{"tags":"x"},"overrides":{"tags":{"color":"red"}}}] — also accepted and persisted
4. GET /app/d1234abcd/environment/staging/components/{id} and see the reserved key reported back
