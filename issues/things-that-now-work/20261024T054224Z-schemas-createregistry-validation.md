Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# CreateRegistry accepts a RegistryName outside the service's name constraints

_Filed: 2026-10-24T05:42:24Z_
_Operation: CreateRegistry | Class: validation_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler stores whatever `RegistryName` the URI label carries: no character-set and no length check at all. Real EventBridge Schemas validates the registry name before creating anything and refuses a name that does not match the service's pattern (letters, digits, '.', '-', '_'; at most 64 characters) with a 400 BadRequestException whose message follows the service's parameter-pattern template ('Parameter RegistryName doesn't match pattern ...' / a length refusal).

Expected: `PUT`/`POST /v1/registries/name/<bad name>` with a name containing e.g. a space, '/', '@', '!' or a 65+ character name -> HTTP 400 BadRequestException, and NO registry created.
Actual: HTTP 201 with the registry persisted; DescribeRegistry/ListRegistries then report a registry AWS would never have created (and whose ARN the emulator mints from the invalid name).

This matters because the invalid name is then observable forever in the store: a client that (correctly) expects a 400 proceeds as if it had a usable registry, and every later schema it creates lives under a resource AWS could not have.

## Evidence

Spec: `smithy_query(query="shape", shape="CreateRegistryRequest")` shows `RegistryName` targeting the unconstrained `__string` (@httpLabel, @required) — no @length, no @pattern — so the boundary validator enforces nothing and the constraint is the SERVICE's, exactly as this crate already reasons for the `Language` label (src/state.rs:2160-2185 cites the real refusal 'Parameter CodeBindingLanguage doesn't match pattern ^(Java8|TypeScript3|Python36|Go1)$', proving the service does pattern-validate its URI-label names and returns 400 for a mismatch). Well-established service constraint: registry names are limited to 64 characters of [A-Za-z0-9._-] (same constraint CloudFormation AWS::EventSchemas::Registry and the Terraform aws_schemas_registry resource publish for `name`).
Handler: services/service-com-amazonaws-schemas-schemas/src/ops/create_registry.rs:146-201 — the only pre-store checks are existence (409) and the store cap; `input.registry_name` is used verbatim for the key and the ARN.
Tests: no test in tests/all/* exercises an invalid registry name (registry_schema_lifecycle.rs covers only well-formed names, duplicates and prefixes).

## Steps to Reproduce

1. POST /v1/registries/name/my%20registry (a name containing a space) -> observe 201 Created instead of 400 BadRequestException
2. GET /v1/registries/name/my%20registry -> observe 200 with RegistryArn 'arn:aws:schemas:us-east-1:<acct>:registry/my registry'
3. POST /v1/registries/name/<66-character name> -> observe 201 instead of a 400 length refusal
4. GET /v1/registries -> the invalid names are listed as real registries
