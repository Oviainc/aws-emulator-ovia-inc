Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b056d947c24faab0377cbe866250e0547cccc526

# The "Type is one of the modelled Type values" check and its refusal are implemented twice, once against TYPE_VALUES and once against hand-written literals

_Filed: 2026-10-28T07:34:55Z_
_Operation: ExportSchema | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b056d947c24faab0377cbe866250e0547cccc526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two handlers take a schema `Type` the framework cannot validate for them (`ExportSchema$Type` is modelled `__string` on the query string, `GetDiscoveredSchema$Type` is read as a raw string by the handler) and each implements the same rule: reject a value that is not one of the modelled `Type` enum values, with the identical refusal sentence built from `TYPE_VALUES`.

ExportSchema tests membership directly against `crate::ops::types::TYPE_VALUES`. GetDiscoveredSchema tests membership against a second, hand-written copy of the same value set (`Dialect::from_modeled`, which matches the bare literals `"OpenApi3"` and `"JSONSchemaDraft4"`) and then words the refusal from `TYPE_VALUES`. So the admitted set and the sentence that names it are read from two different sources in the same crate: if the model's `Type` enum ever gains or renames a value, `TYPE_VALUES` and the refusal text move while `Dialect::from_modeled` does not, and `GetDiscoveredSchema` would refuse a value its own message says is admitted. ExportSchema additionally re-declares the same two literals as private constants (`JSON_SCHEMA_DRAFT4_TYPE`, `OPEN_API_3_TYPE`), a third spelling of the set.

The fix is to put the one membership-and-refuse rule in `crate::state` beside the other shared refusal helpers (where `validate_content`, `json_type_name` and the name constraints already live) and have both handlers map `TYPE_VALUES` to their own dialect/branch from it. Every response and every existing test stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs:159-171:
```
        // `Type` is modelled `__string` on this operation, not the `Type` enum, so the
        // boundary validator admits anything: the handler is what refuses a value that
        // is not a schema language this service knows. Named members and admitted
        // values, as a validation refusal does.
        if !TYPE_VALUES.contains(&input.r#type.as_str()) {
            return Err(ExportSchemaErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "Type must be one of {}.",
                    TYPE_VALUES.join(", ")
                )),
            )
            .into());
        }
```

Copy 2 — services/service-com-amazonaws-schemas-schemas/src/ops/get_discovered_schema.rs:756-763:
```
        let dialect = Dialect::from_modeled(&input.r#type).ok_or_else(|| {
            GetDiscoveredSchemaError::from(GetDiscoveredSchemaErrorModeled::BadRequestException(
                crate::state::bad_request(format!(
                    "Type must be one of {}.",
                    TYPE_VALUES.join(", ")
                )),
            ))
        })?;
```
whose admitted set is a second, independent spelling — get_discovered_schema.rs:196-213:
```
/// The two values are [`TYPE_VALUES`], the model's `Type` enum. They differ in more
/// than the envelope: each dialect spells a nullable position its own way.
...
    fn from_modeled(r#type: &str) -> Option<Self> {
        match r#type {
            "OpenApi3" => Some(Self::OpenApi3),
            "JSONSchemaDraft4" => Some(Self::JsonSchemaDraft4),
            _ => None,
        }
    }
```
The single source it claims to follow — services/service-com-amazonaws-schemas-schemas/src/ops/types/shared_types.rs:158:
```
pub const TYPE_VALUES: &[&str] = &["JSONSchemaDraft4", "OpenApi3"];
```
And the third spelling — export_schema.rs:258-262:
```
/// The two schema languages this service stores, as [`TYPE_VALUES`] spells them.
const JSON_SCHEMA_DRAFT4_TYPE: &str = "JSONSchemaDraft4";

/// See [`JSON_SCHEMA_DRAFT4_TYPE`].
const OPEN_API_3_TYPE: &str = "OpenApi3";
```

Tests that pin both copies and must stay unchanged: tests/all/export_reads_the_stored_schema.rs:155-156 and :413/:454 iterate `TYPE_VALUES` for ExportSchema, and tests/all/discovered_schema_inference.rs:22 plus :270/:353/:398/:435/:515/:558/:615/:668/:690/:747 iterate `TYPE_VALUES` for GetDiscoveredSchema.

## Steps to Reproduce

1. Open services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs:163-171 and note the membership test against `TYPE_VALUES` plus the `"Type must be one of {}."` refusal.
2. Open services/service-com-amazonaws-schemas-schemas/src/ops/get_discovered_schema.rs:756-763 and note the identical refusal, whose membership test is `Dialect::from_modeled` at get_discovered_schema.rs:207-213.
3. Observe that `Dialect::from_modeled` hard-codes `"OpenApi3"`/`"JSONSchemaDraft4"` rather than reading `crate::ops::types::TYPE_VALUES` (shared_types.rs:158), so the set tested and the set named in the message come from two places that can drift.
4. Run the existing suites (tests/all/export_reads_the_stored_schema.rs, tests/all/discovered_schema_inference.rs) to confirm both paths are covered and that hoisting the one rule into crate::state leaves every response unchanged.
