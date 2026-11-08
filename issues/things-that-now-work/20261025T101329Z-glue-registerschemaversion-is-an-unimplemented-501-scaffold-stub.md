Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.register-version-on-disabled-compatibility-fails
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.register-version-on-disabled-compatibility-fails|run#11|AssertionError|assert code == "InvalidInputException", (
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# glue RegisterSchemaVersion is an unimplemented 501 scaffold stub, so schema evolution never happens and a DISABLED-compatibility schema is never refused with InvalidInputException

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSchemaVersion

## Description
One net-new root cause in the glue service.

Cause 1 of 1: `RegisterSchemaVersion` answers HTTP 501 `NotImplemented` for every request, so neither the rejection path nor the success path of Glue Schema Registry version evolution exists. Failing check ids: glue.register-version-on-disabled-compatibility-fails.

AWS: Smithy model `com.amazonaws.glue#RegisterSchemaVersion` (model path services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) is a real, fully modeled operation: input `SchemaId` (required) + `SchemaDefinition` (SchemaDefinitionString, @length 1..170000, pattern \S, required); output `SchemaVersionId` (36-char uuid pattern), `VersionNumber` (VersionLongNumber range 1..100000), `Status` (SchemaVersionStatus: AVAILABLE | DELETING | FAILURE | PENDING). Its modeled errors are AccessDeniedException, ConcurrentModificationException, EntityNotFoundException, InternalServiceException, InvalidInputException (all HTTP 400) and InternalServiceException (HTTP 500). The model documents the operation as "Adds a new version to the existing schema. Returns an error if new version of schema does not meet the compatibility requirements of the schema set." A schema created with `Compatibility=DISABLED` therefore allows no further versions at all: registering one is refused with HTTP 400 `InvalidInputException` (recorded message example: `Compatibility DISABLED does not allow versioning. SchemaId: SchemaId(schemaArn=<arn>, schemaName=null, registryName=null)` — the message text is an example; the requirement is the 400/InvalidInputException classification for a version registered against a DISABLED schema). Against a schema whose Compatibility is BACKWARD the identical evolved AVRO definition is accepted and reaches `Status=AVAILABLE` with a new `VersionNumber`, and the rejected registration adds NO version (ListSchemaVersions still reports exactly 1).

Emulator: every call returns HTTP 501 with error code `NotImplemented` and the message `operation RegisterSchemaVersion is scaffolded but not implemented in this emulator`. The replay fails with `AssertionError: expected InvalidInputException registering a version under DISABLED, got NotImplemented`, and the positive BACKWARD-compatibility control can never produce a second version either. The handler is still the generated scaffold whose `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("RegisterSchemaVersion"))`.

Why: a client using the Glue Schema Registry cannot evolve a schema at all — no new version is ever created, no `SchemaVersionId`/`VersionNumber`/`Status` is ever returned, and compatibility enforcement is invisible. Worse for correctness testing, code that relies on DISABLED compatibility refusing evolution sees a 501 server-side error instead of the modeled 400 client error, so retry/error-classification logic (501 is not a client fault) takes the wrong branch and a guard that should surface "this schema does not allow versioning" never fires. Note this is NOT covered by the already-fixed issue 20261025T022820Z-glue-createschema-stub (CreateSchema now succeeds; the refusal has moved to RegisterSchemaVersion), nor by the open 20261025T022820Z-glue-checkschemavalidity/CheckSchemaVersionValidity stub issue, which names a different operation.

Fix: implement the operation's handler and its compatibility validation in the Glue schema-registry state — resolve `SchemaId` (SchemaArn, or SchemaName+RegistryName) to the stored schema, refuse with `InvalidInputException` when the resolved schema's `Compatibility` is `DISABLED`, refuse with `EntityNotFoundException` when the schema set does not exist, otherwise mint a new version (uuid `SchemaVersionId`, next `VersionNumber`, terminal `Status=AVAILABLE`) and record it so GetSchemaVersion/ListSchemaVersions observe it; a refused registration must leave the version list unchanged. File citation under Traces.

Traces: failing behavioral check glue.register-version-on-disabled-compatibility-fails ([mismatch]); scaffold handler services/service-com-amazonaws-glue-awsglue/src/ops/register_schema_version.rs lines 3-6 (SCAFFOLDED marker) and line 181 (`Scaffolded::not_implemented`).

## Evidence
Expected (AWS): register_schema_version against a schema created with Compatibility=DISABLED -> HTTP 400, Error.Code = InvalidInputException (example message: "Compatibility DISABLED does not allow versioning. SchemaId: SchemaId(schemaArn=<arn>, schemaName=null, registryName=null)"); ListSchemaVersions for that schema still returns exactly 1 version; the identical evolved definition registered against an otherwise-identical Compatibility=BACKWARD schema succeeds and the new version reaches Status=AVAILABLE.

Actual (emulator): HTTP 501, Error.Code = NotImplemented, message "operation RegisterSchemaVersion is scaffolded but not implemented in this emulator". Replay output:
  File "check_glue_register_version_on_disabled_compatibility_fails.py", line 88, in run
    assert code == "InvalidInputException", (
  AssertionError: expected InvalidInputException registering a version under DISABLED, got NotImplemented
The BACKWARD control leg is unreachable for the same reason, so no version is ever minted.

Source: services/service-com-amazonaws-glue-awsglue/src/ops/register_schema_version.rs:181 returns Scaffolded::not_implemented("RegisterSchemaVersion"); the file still carries the "//! SCAFFOLDED, NOT IMPLEMENTED" marker.

## Steps to Reproduce
1. glue.create_schema(SchemaName=<name-a>, DataFormat="AVRO", Compatibility="DISABLED", SchemaDefinition='{"type":"record","name":"User","fields":[{"name":"id","type":"string"}]}') and note the returned SchemaArn.
2. glue.create_schema(SchemaName=<name-b>, DataFormat="AVRO", Compatibility="BACKWARD", SchemaDefinition=<same base definition>) as the control.
3. glue.register_schema_version(SchemaId={"SchemaArn": <arn-a>}, SchemaDefinition='{"type":"record","name":"User","fields":[{"name":"id","type":"string"},{"name":"email","type":["null","string"],"default":null}]}').
   Expected: ClientError with HTTP 400 and Error.Code == "InvalidInputException". Observed: HTTP 501 with Error.Code == "NotImplemented".
4. glue.list_schema_versions(SchemaId={"SchemaArn": <arn-a>}) -> must still report exactly 1 version.
5. glue.register_schema_version(SchemaId={"SchemaArn": <arn-b>}, SchemaDefinition=<same evolved definition>) -> expected to succeed and, after polling glue.get_schema_version, to reach Status == "AVAILABLE". Observed: HTTP 501 NotImplemented.
Replay shortcut: re-run behavioral check glue.register-version-on-disabled-compatibility-fails.
