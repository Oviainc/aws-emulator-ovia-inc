Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.delete-registry-nonempty-rejected schemas.export-schema-json-schema-draft4-conversion schemas.create-registry-duplicate-name-rejected-conflict schemas.create-schema-duplicate-name-rejected-bad-request schemas.concurrent-schema-updates-produce-unique-sequential-versions
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas.delete-registry-nonempty-rejected|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: schemas.export-schema-json-schema-draft4-conversion|_find_aws_managed_schema#2|AssertionError|assert registries, "no AWS-managed schema registries visible to this account"
Issue-Key: schemas.create-registry-duplicate-name-rejected-conflict|run#7|AssertionError|assert err["Message"] == f"Registry with name {registry} already exists.", f"unexpected create_registry_dup message: {err['Message']!r}"
Issue-Key: schemas.create-schema-duplicate-name-rejected-bad-request|run#11|AssertionError|assert err["Message"] == f"Schema with name {schema_name} already exists.", f"unexpected create_schema_dup message: {err['Message']!r}"
Issue-Key: schemas.concurrent-schema-updates-produce-unique-sequential-versions|run#16|AssertionError|assert len(successes) >= 1, "at least one concurrent update must win"
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# schemas registry/schema fidelity: DeleteRegistry deletes a non-empty registry, no AWS-managed registries exist, duplicate-create conflict messages use the wrong template, and UpdateSchema mints no versions (4 causes)

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRegistry

## Description
Four independent root causes in the schemas service, all grounded in failing behavioral checks. A fixer must close all four before submitting.

=== Cause 1 of 4: DeleteRegistry destroys a non-empty registry instead of refusing it ===
Failing check: schemas.delete-registry-nonempty-rejected.
AWS: recorded behavior only (the Smithy model for DeleteRegistry declares BadRequestException 400, ForbiddenException 403, InternalServerErrorException 500, NotFoundException 404, ServiceUnavailableException 503, UnauthorizedException 401 — it does not model the conflict, so the referential-integrity guard is recorded, not modeled). Real AWS refuses DeleteRegistry while the registry still contains a schema, answering error code ConflictException (HTTP 409); recorded message, as an example: 'The registry you tried to delete is not empty.' The registry and its schema both survive the refusal, and the identical DeleteRegistry call succeeds (HTTP 204) once the last schema is deleted.
Emulator: DeleteRegistry returns HTTP 204 success and removes the registry together with every schema nested in it. services/service-com-amazonaws-schemas-schemas/src/ops/delete_registry.rs:171 does `state.registries.remove(&input.registry_name)` and only distinguishes the not-found case; there is no emptiness check.
Why: a client that relies on AWS's guard loses data silently. Code that calls DeleteRegistry defensively (expecting a 409 to tell it schemas are still present) deletes a production registry and every schema version in it, and never sees the error it was written to handle.
Fix: the DeleteRegistry handler's precondition check (services/service-com-amazonaws-schemas-schemas/src/ops/delete_registry.rs) — refuse with the 409 conflict error when the registry still owns schemas, before mutating state.

=== Cause 2 of 4: no AWS-managed registries exist, so ListRegistries(Scope='AWS') is empty and ExportSchema's supported path is unreachable ===
Failing check: schemas.export-schema-json-schema-draft4-conversion.
AWS: recorded behavior only. Every account sees service-owned, AWS-managed schema registries through ListRegistries with Scope='AWS' (the model exposes Scope as an httpQuery parameter on ListRegistries), each holding AWS event schemas. Those schemas are the only ones ExportSchema will convert: ExportSchema(Type='JSONSchemaDraft4') on an AWS-managed schema returns 200 with Content whose $schema names draft-04, while the same call on a hand-created schema is refused with ForbiddenException (403, modeled) and a non-enum Type is refused with BadRequestException (400, modeled).
Emulator: ListRegistries(Scope='AWS') returns an empty Registries list, so the check cannot reach ExportSchema at all ('no AWS-managed schema registries visible to this account'). The AWS-managed catalog is never seeded, and consequently neither ExportSchema's success path nor its two rejection classes are exercisable.
Why: a client that discovers AWS event schemas (the normal way to generate code against AWS events) sees an empty catalog and concludes the account has no AWS schemas, and any ExportSchema flow is dead.
Fix: the schemas service's seeded state (services/service-com-amazonaws-schemas-schemas/src/state.rs) must contain the AWS-managed registry/schema catalog with Scope='AWS', and the ExportSchema handler must honor the draft-4 conversion for those schemas plus the BadRequestException/ForbiddenException preconditions. Distinct from the already-filed stub issue for TagResource that blocks schemas.list-registries-scope-filter-local-vs-aws.

=== Cause 3 of 4: duplicate-create conflicts use a message template AWS does not use ===
Failing checks: schemas.create-registry-duplicate-name-rejected-conflict, schemas.create-schema-duplicate-name-rejected-bad-request.
AWS: recorded behavior only for the wording; the error class is modeled (CreateRegistry declares ConflictException, HTTP 409). The message is built from the request, so its template is the requirement: re-creating an existing registry answers 'Registry with name <RegistryName> already exists.' and re-creating an existing schema answers 'Schema with name <SchemaName> already exists.' — note the schema message names the schema only, not its registry.
Emulator: the error code is right (ConflictException) but both messages use a different template: 'Registry <RegistryName> already exists.' (missing 'with name') and 'Schema <SchemaName> already exists in registry <RegistryName>.' (dropping 'with name' and appending a registry clause AWS does not emit).
Why: clients and test harnesses that assert on, log-scrape or pattern-match AWS's documented conflict sentence misclassify the emulator's response, and the extra registry clause leaks a request member AWS deliberately keeps out of that message.
Fix: the conflict-message construction shared by the CreateRegistry and CreateSchema handlers (services/service-com-amazonaws-schemas-schemas/src/ops/create_registry.rs, .../create_schema.rs, and the message helpers in .../src/state.rs).

=== Cause 4 of 4: concurrent UpdateSchema calls mint no version and never produce the conflict/uniqueness behavior AWS guarantees ===
Failing check: schemas.concurrent-schema-updates-produce-unique-sequential-versions.
AWS: recorded behavior only. Racing UpdateSchema calls on one schema each either win with a distinct SchemaVersion or are refused with error code ConflictException; no two winners share a version, and ListSchemaVersions afterwards contains exactly version '1' plus one entry per winner.
Emulator: every racer is refused with error code NotImplemented ('operation UpdateSchema is scaffolded but not implemented in this emulator'), so zero updates win and the version counter never advances ('at least one concurrent update must win').
Why: a client cannot evolve a schema at all, and once UpdateSchema is implemented the concurrency contract still has to hold — a non-atomic version counter would silently mint duplicate versions and clobber another writer's content.
Fix: the UpdateSchema handler plus the schema version counter in the service state (services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs, .../src/state.rs), with version minting performed atomically under the state lock and losers refused with the 409 conflict error. The bare 'UpdateSchema is a stub' fact is already filed as its own scaffold-stub issue; what is recorded here and NOT covered there is the concurrency/version-uniqueness contract this check asserts.

Traces: schemas.delete-registry-nonempty-rejected, schemas.export-schema-json-schema-draft4-conversion, schemas.create-registry-duplicate-name-rejected-conflict, schemas.create-schema-duplicate-name-rejected-bad-request, schemas.concurrent-schema-updates-produce-unique-sequential-versions.

## Evidence
Cause 1 — DeleteRegistry on a registry that still holds a schema:
  Expected: ClientError with Code 'ConflictException' (HTTP 409), registry and schema still present afterwards; example message 'The registry you tried to delete is not empty.'
  Actual: success — {'ResponseMetadata': {'HTTPStatusCode': 204, ...}} and the registry plus its schema are gone. AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', ...}, 'RetryAttempts': 0}}
  Code: services/service-com-amazonaws-schemas-schemas/src/ops/delete_registry.rs:171 removes the registry unconditionally.

Cause 2 — ListRegistries(Scope='AWS'):
  Expected: a non-empty Registries list of AWS-managed registries, from which ExportSchema(Type='JSONSchemaDraft4') returns Content with '$schema' containing 'draft-04'.
  Actual: empty list. AssertionError: no AWS-managed schema registries visible to this account

Cause 3 — duplicate creates:
  Expected message template: 'Registry with name <RegistryName> already exists.' / 'Schema with name <SchemaName> already exists.' (Code ConflictException in both cases, which the emulator gets right).
  Actual: AssertionError: unexpected create_registry_dup message: 'Registry sim2real-schemas-reg-2a4bf69b3646 already exists.'
          AssertionError: unexpected create_schema_dup message: 'Schema sim2real-schemas-sch-96b2a5fcf5fb already exists in registry sim2real-schemas-reg-313a49e66629.'

Cause 4 — 5 racing UpdateSchema calls on one schema:
  Expected: >=1 success, each with a distinct SchemaVersion; any loser refused with Code 'ConflictException'; ListSchemaVersions == {'1'} | winners.
  Actual: 0 successes, every racer refused with Code 'NotImplemented' ('operation UpdateSchema is scaffolded but not implemented in this emulator'). AssertionError: at least one concurrent update must win

## Steps to Reproduce
Cause 1: CreateRegistry(R); CreateSchema(R, S, Type='OpenApi3', Content=<openapi3 doc>); DeleteRegistry(R) -> observe HTTP 204 instead of a 409 ConflictException, then DescribeRegistry(R) -> gone. Replay schemas.delete-registry-nonempty-rejected.
Cause 2: ListRegistries(Scope='AWS') -> observe Registries == []; no registry is available to feed ExportSchema(Type='JSONSchemaDraft4'). Replay schemas.export-schema-json-schema-draft4-conversion.
Cause 3: CreateRegistry(R) twice -> inspect the ConflictException message; CreateRegistry(R) + CreateSchema(R,S) twice -> inspect the ConflictException message. Replay schemas.create-registry-duplicate-name-rejected-conflict and schemas.create-schema-duplicate-name-rejected-bad-request.
Cause 4: CreateRegistry(R); CreateSchema(R,S); fire 5 concurrent UpdateSchema(R,S,Type='OpenApi3',Content=<distinct doc>) calls -> observe all five refused with NotImplemented and no version minted. Replay schemas.concurrent-schema-updates-produce-unique-sequential-versions.
