Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# The registry -> schema -> version read resolution is open-coded in four handlers

_Filed: 2026-10-27T16:50:29Z_
_Operation: DescribeSchema | Class: maintainability_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every read that names a schema version performs the same three-step resolution and the same choice of refusal for each step: look the registry up (else `registry_not_found`, 404), look the schema up inside it (else `schema_not_found`, 404), then resolve `SchemaVersion` through `Schema::version_for` (else `schema_version_not_found`, 404). That rule is copied verbatim into four handlers, with two more carrying truncated copies of it, so the copies can drift — a handler that changed which refusal a step answers, or resolved the version before the schema, would diverge silently from the others.

The crate already made the same consolidation for the WRITE side: `ComAmazonawsSchemasSchemasState::writable_registry` exists precisely because the resolve-then-refuse step for writes was open-coded per handler and had drifted (state.rs:190-217: "Resolving and refusing together is what keeps the second step from being forgotten by a handler that remembers the first"). The read side has no such helper, so the identical rule stays replicated.

A fix is a read counterpart of `writable_registry` in `crate::state` returning the resolved schema/version or an enum naming which step failed, with each handler mapping that enum into its own modelled error set exactly as it already maps `RegistryWriteError`. No response and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-schemas-schemas/src/ops/describe_schema.rs:171-194:
```
        let registry = state.registries.get(&input.registry_name).ok_or_else(|| { ... crate::state::registry_not_found(&input.registry_name) ... })?;
        let schema = registry.schemas.get(&input.schema_name).ok_or_else(|| { ... crate::state::schema_not_found(&input.schema_name) ... })?;
        let version = schema.version_for(input.schema_version.as_deref()).ok_or_else(|| { ... crate::state::schema_version_not_found(&input.registry_name, &input.schema_name, input.schema_version.as_deref()) ... })?;
```
Copy 2 — src/ops/export_schema.rs:189-212: the same three lookups, the same three helpers, in the same order.
Copy 3 — src/ops/describe_code_binding.rs:190-214: the same three lookups, the same three helpers, in the same order.
Copy 4 — src/ops/put_code_binding.rs:187-215: the same three lookups (`get_mut`), the same three helpers, in the same order.
Partial copies — src/ops/list_schema_versions.rs:181-190 (registry + schema steps) and src/ops/delete_schema_version.rs:208-215 (schema step, after `writable_registry`).

The already-consolidated counterpart it should mirror — src/state.rs:190-217:
```
    /// Every write this service serves that names a registry ... takes the
    /// same two steps before it changes anything ... Resolving and refusing together is
    /// what keeps the second step from being forgotten by a handler that remembers the first.
    pub fn writable_registry(&mut self, name: &str) -> Result<&mut Registry, RegistryWriteError> {
```

Tests that cover the behaviour and must stay unchanged: tests/all/registry_schema_lifecycle.rs (describe/list 404s for a missing registry and schema), tests/all/schema_version_deletion.rs (the version-step 404), tests/all/code_binding_lifecycle.rs:488 `a_put_against_a_missing_schema_or_version_is_not_found`, tests/all/export_reads_the_stored_schema.rs, and tests/all/error_body_codes.rs (the wire `Code`/`Message` of each refusal).

## Steps to Reproduce

1. Open services/service-com-amazonaws-schemas-schemas/src/ops/describe_schema.rs:171-194 and read the registry -> schema -> version resolution with its three refusal helpers
2. Open src/ops/export_schema.rs:189-212, src/ops/describe_code_binding.rs:190-214 and src/ops/put_code_binding.rs:187-215 and observe the identical three-step sequence repeated
3. Compare with src/state.rs:190-217, where the analogous write-side resolution was already consolidated into `writable_registry` after it had drifted across handlers
4. Observe that no helper in `crate::state` resolves the read path, so the rule exists in four separately maintained copies
