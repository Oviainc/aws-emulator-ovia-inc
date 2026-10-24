Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Deleting a table in CREATING or UPDATING answers ConflictException instead of the documented ResourceInUseException

_Filed: 2026-10-24T03:46:38Z_
_Operation: DeleteTable | Class: error-response_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The DeleteTable documentation in the service model is explicit about the three in-flight cases: "If the table is in the ACTIVE state, you can delete it. If a table is either in the CREATING or UPDATING states, then Amazon Keyspaces returns a ResourceInUseException. If the specified table does not exist, Amazon Keyspaces returns a ResourceNotFoundException. If the table is already in the DELETING state, no error is returned."

The emulator implements the ACTIVE, DELETING and missing-table cases correctly, but answers the CREATING/UPDATING case with `ConflictException` (HTTP 409) rather than the documented `ResourceInUseException`. A client that branches on the error code (the common "wait until the table settles, then retry the delete" loop keys off ResourceInUseException) takes the wrong branch. Note the Keyspaces model declares no ResourceInUseException shape, so reproducing AWS here means emitting that error code/envelope even though it is unmodeled — which is what the service's own documentation describes.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.keyspaces#DeleteTable")` returns the sentence naming ResourceInUseException for the CREATING/UPDATING states.

Handler: services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_table.rs, the `in_progress =>` arm of the status match (around lines 214-229), returns `DeleteTableErrorModeled::ConflictException`, with a comment acknowledging that the documented shape is ResourceInUseException and that this model does not declare it.

Tests: tests/all/keyspace_table_lifecycle.rs exercises the delete lifecycle but no test asserts the error code for a delete against a CREATING/UPDATING table, so nothing pins the current substitution.

## Steps to Reproduce

1. CreateKeyspace {keyspaceName: ks}; wait for it to settle
2. CreateTable {keyspaceName: ks, tableName: t, schemaDefinition: {...}}
3. Immediately (while GetTable still reports status CREATING) call DeleteTable {keyspaceName: ks, tableName: t}
4. Expected per the DeleteTable documentation: a ResourceInUseException; actual: 409 ConflictException
5. Same for UPDATING: after the table is ACTIVE, call UpdateTable {.., defaultTimeToLive: 60} and then DeleteTable while the table is still UPDATING
