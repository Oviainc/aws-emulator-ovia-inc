Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.create-schema-duplicate-name-rejected-bad-request
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas.create-schema-duplicate-name-rejected-bad-request|run#10|AssertionError|assert err["Code"] == "ConflictException", f"unexpected error: {err!r}"
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Duplicate CreateSchema is refused as 400 BadRequestException instead of the recorded ConflictException

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchema

## Description
REOPENS 20261024T054224Z-schemas-createschema-error-response.md: its fix left the call refused. That (now resolved) issue reasoned from CreateSchema's modeled error list that a duplicate schema name must answer 400 BadRequestException, and the handler was changed accordingly. The recorded behavior says the opposite, so the service now diverges in the other direction and this is not a new defect but the same refusal re-classified.

Cause 1 of 1: duplicate-name refusal of CreateSchema carries the wrong error type.

AWS: recorded behavior only for the error TYPE. Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json) lists CreateSchema's errors as BadRequestException (400), ForbiddenException (403), InternalServerErrorException (500), ServiceUnavailableException (503) and does NOT declare ConflictException — but the model is silent on which of those a name collision takes, and the recording shows real EventBridge Schemas refusing a second CreateSchema on an existing name with error code `ConflictException` (an error shape the same model does declare on CreateRegistry, httpError 409). The recorded message, `Schema with name <SchemaName> already exists.`, is built from the request (it interpolates the requested SchemaName), so the rule — the message names the colliding schema name — is a requirement; the exact recorded sentence is the example of it. After the refusal the pre-existing schema must be untouched: DescribeSchema still reports SchemaVersion "1" and the original Content.

Emulator: the duplicate create is refused with error code `BadRequestException` (observed body `{'Message': 'Schema with name sim2real-schemas-sch-<id> already exists.', 'Code': 'BadRequestException'}`), i.e. the right message and the wrong error type/status class. Everything else the check asserts passes: the existing schema's version and content are intact and a genuinely new name in the same registry still creates at version 1, so only the refusal's classification diverges.

Why: a client that branches on the error type — create-if-not-exists loops, idempotent provisioning, Terraform-style 'already exists' handling, any retry policy that treats 409 as 'resource is there, carry on' and 400 as 'my request is malformed, do not retry' — takes a different branch against the emulator than against AWS. Against AWS the collision is a conflict the caller can absorb; against the emulator it reads as a malformed request and typically aborts provisioning.

Fix: the duplicate-name branch of the CreateSchema handler's refusal classification, services/service-com-amazonaws-schemas-schemas/src/ops/create_schema.rs (the `registry.schemas.contains_key(&input.schema_name)` arm and the operation's error enum, which currently widens only with NotFoundException). The error shape must be the conflict shape the service model already defines, so the operation's error channel needs ConflictException available again; keep the `crate::state::schema_already_exists` message helper unchanged. The crate's own tests encode the current classification (tests/all/error_body_codes.rs and tests/all/aws_managed_and_versions.rs assert the duplicate-create error type) and must move with the handler.

Traces: schemas.create-schema-duplicate-name-rejected-bad-request (the check id is named for the error class the emulator was expected to drop; its assertion is `err["Code"] == "ConflictException"`).

## Evidence
Failing check: schemas.create-schema-duplicate-name-rejected-bad-request [mismatch]

Assertion and observed value from the replay:
  assert err["Code"] == "ConflictException", f"unexpected error: {err!r}"
  AssertionError: unexpected error: {'Message': 'Schema with name sim2real-schemas-sch-b42eb6fe8582 already exists.', 'Code': 'BadRequestException'}

Expected (recorded AWS): CreateSchema on an existing SchemaName -> error code ConflictException, message 'Schema with name <SchemaName> already exists.'; the stored schema stays at SchemaVersion "1" with its original Content.
Actual (emulator): error code BadRequestException with the same message; state is otherwise correct.

Model corroboration (smithy_query query="operation" operation="CreateSchema,CreateRegistry" on services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json): ConflictException is a shape this service models (errorKind client, httpError 409, declared on CreateRegistry); CreateSchema's declared error list omits it, which is why the earlier fix chose BadRequestException. The recording overrides that inference.

Code: services/service-com-amazonaws-schemas-schemas/src/ops/create_schema.rs:101-110 deliberately does NOT widen the error enum with ConflictException, and lines 217-229 return `CreateSchemaErrorModeled::BadRequestException(crate::state::schema_already_exists(&input.schema_name))` for the collision.

Related open issue, different cause (do not fold): 20261024T054224Z-schemas-createschema-validation.md (SchemaName pattern/length not validated).

## Steps to Reproduce
1. POST /v1/registries/name/<reg> (CreateRegistry) -> 201
2. POST /v1/registries/name/<reg>/schemas/name/<sch> with {"Content":"<valid OpenAPI 3 document>","Type":"OpenApi3"} -> 201, SchemaVersion "1"
3. Repeat the CreateSchema call on the same <reg>/<sch> with slightly different Content
4. Observe the refusal body's Code is BadRequestException; AWS answers the conflict shape (Code ConflictException, 409) with the message 'Schema with name <sch> already exists.'
5. GET /v1/registries/name/<reg>/schemas/name/<sch> (DescribeSchema) -> still SchemaVersion "1" with the original Content (this part already matches)
6. CreateSchema a different, unused name in the same registry -> 201 at SchemaVersion "1" (positive control, already matches)
