Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# ExportSchema refuses every caller-owned schema with 403 instead of converting an OpenApi3 document

_Filed: 2026-10-24T06:03:47Z_
_Operation: ExportSchema | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler decides whether a schema may be exported from WHO OWNS THE REGISTRY, not from the stored schema's type: any schema that is not in the seeded AWS-managed registry (`aws.events`) is refused with HTTP 403 ForbiddenException and the emulator-authored sentence "ExportSchema is supported only for AWS-managed schemas; a schema in a registry you created cannot be exported."

Expected: ExportSchema converts the stored document of ANY schema the caller can describe. The real refusal is a property of the stored SCHEMA TYPE, not of registry ownership — an `OpenApi3` schema exports to `JSONSchemaDraft4` (the whole point of the operation: this is the documented way to obtain the JSON Schema draft-4 rendering of a schema, including every schema EventBridge discovery writes, which lives in the caller-scope `discovered-schemas` registry, i.e. a registry ListRegistries reports under Scope=LOCAL), while a schema stored as `JSONSchemaDraft4` has nothing to convert and is the case the modeled ForbiddenException covers.

Actual: `CreateRegistry("mine")` + `CreateSchema("mine","own",Type=OpenApi3,...)` + `ExportSchema("mine","own",type=JSONSchemaDraft4)` -> 403 ForbiddenException, with no conversion performed, even though the same document stored in `aws.events` converts and returns 200. A client that exports its own (or a discovered) OpenApi3 schema to generate code or draft-4 validators gets a hard 403 against the emulator and a 200 with the converted document against AWS.

Secondary symptom of the same gate (same operation/class, listed only so the fix covers it): because the only gate is registry ownership, `type=OpenApi3` against an AWS-managed schema answers 200 with the stored document echoed back; the type dimension of the refusal is never evaluated at all.

## Evidence

Spec: `smithy_query(query="operation", operation="ExportSchema")` — input is only RegistryName/SchemaName/SchemaVersion/Type; nothing in the model scopes the operation to service-owned registries, and ForbiddenException (403) is declared beside NotFoundException (404), so the 403 exists for a condition other than "the resource is not yours to read" (reads of a caller's own schema are plainly allowed — DescribeSchema on the same resource answers 200).
Well-established service behavior: schema discovery writes its inferred schemas as OpenApi3 into the account's own `discovered-schemas` registry (a LOCAL-scope registry, not `aws.events`), and exporting those to JSON Schema draft 4 is the documented consumer workflow; AWS's own refusal for a non-exportable schema names the schema TYPE, not the registry's owner.
Handler: services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs:205-215 — `if !registry.scope.is_managed() { return Err(ForbiddenException(forbidden("ExportSchema is supported only for AWS-managed schemas..."))) }`, reached for every registry created through CreateRegistry; the conversion at export_schema.rs:217-241 is only ever applied to seeded `aws.events` documents. `crate::state::RegistryScope::Aws` is set only by the seeded catalogue (src/state.rs, `aws_managed_registries`), so every caller-created registry is Local.
Test coverage: tests/all/aws_managed_and_versions.rs:413-453 (`exporting_what_cannot_be_exported_is_refused`) currently ENCODES this divergence — rows "a caller's own schema" and "a caller's own schema, exported as its own type" both assert 403 — so the test has to change with the fix. The recorded check referenced by issues/things-that-now-work/20261022T034503Z-schemas-registry-schema-fidelity-...md (`schemas.export-schema-json-schema-draft4-conversion`) only ever exports an AWS-managed schema, so nothing recorded pins the caller-owned case.

## Steps to Reproduce

1. POST /v1/registries/name/mine -> 201 (CreateRegistry)
2. POST /v1/registries/name/mine/schemas/name/own with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\",\"info\":{\"title\":\"Order Placed\"},\"components\":{\"schemas\":{\"AWSEvent\":{\"type\":\"object\"}}}}"} -> 201 (CreateSchema)
3. GET /v1/registries/name/mine/schemas/name/own/export?type=JSONSchemaDraft4
4. Observe HTTP 403 x-amzn-errortype: ForbiddenException with no Content; AWS answers 200 with Content carrying the draft-04 rendering ($schema http://json-schema.org/draft-04/schema#, definitions built from components.schemas), Type=JSONSchemaDraft4, SchemaName=own and the schema's ARN
5. For contrast run GET /v1/registries/name/aws.events/schemas/name/aws.ecs@ECSTaskStateChange/export?type=JSONSchemaDraft4 -> 200, showing the only difference is who owns the registry, not what is stored
