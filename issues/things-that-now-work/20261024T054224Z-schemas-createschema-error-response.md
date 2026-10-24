Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# CreateSchema for an existing schema name answers 409 ConflictException instead of 400 BadRequestException

_Filed: 2026-10-24T05:42:24Z_
_Operation: CreateSchema | Class: error-response_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A second CreateSchema under a name the registry already holds is refused by the emulator with HTTP 409 and `x-amzn-errortype: ConflictException` (body `Code: "ResourceAlreadyExist"`). Real AWS refuses the duplicate CreateSchema as a 400 BadRequestException (same message text, 'Schema with name <SchemaName> already exists.'), which is why CreateSchema's modeled error set contains BadRequestException but NOT ConflictException, while CreateRegistry — whose duplicate-create genuinely is a 409 — does declare ConflictException.

Expected: duplicate CreateSchema -> HTTP 400, `x-amzn-errortype: BadRequestException`, message 'Schema with name <SchemaName> already exists.'
Actual: HTTP 409, `x-amzn-errortype: ConflictException`.

A client that branches on the error type (create-if-not-exists loops, idempotent provisioning code, Terraform-style 'already exists' handling) takes a different branch against the emulator than against AWS, and an SDK whose modeled error set for CreateSchema has no ConflictException surfaces the response as an unmodeled/unknown error.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateSchema")` — CreateSchema declares exactly BadRequestException (400), ForbiddenException (403), InternalServerErrorException (500), ServiceUnavailableException (503). ConflictException is NOT in the list; CreateRegistry's list does contain it (409). The handler nevertheless widens its error enum with `extra_errors = ["NotFoundException", "ConflictException"]` (services/service-com-amazonaws-schemas-schemas/src/ops/create_schema.rs:97-100) and returns `ConflictException(crate::state::schema_already_exists(..))` at create_schema.rs:211-216.
Recorded-behavior corroboration: the already-filed (fixed) issue 20261022T034503Z-schemas-registry-schema-fidelity-... carries two sibling sim2real checks for this exact pair of cases, and they are named for the error class each one recorded: `schemas.create-registry-duplicate-name-rejected-conflict` (CreateRegistry, 409) vs `schemas.create-schema-duplicate-name-rejected-bad-request` (CreateSchema, 400). That issue only corrected the message template, not the status/error type for the schema case.
Test coverage: tests/all/error_body_codes.rs:240 and tests/all/aws_managed_and_versions.rs:171-228 currently assert the 409/ConflictException behavior for duplicate CreateSchema, so they encode the divergence and would have to change with the fix.

## Steps to Reproduce

1. POST /v1/registries/name/reg (CreateRegistry) -> 201
2. POST /v1/registries/name/reg/schemas/name/sch with {"Content":"{...valid OpenAPI 3 document...}","Type":"OpenApi3"} -> 201
3. Repeat the identical CreateSchema call
4. Observe HTTP 409 with x-amzn-errortype: ConflictException; AWS answers HTTP 400 with x-amzn-errortype: BadRequestException and the message 'Schema with name sch already exists.'
