Issue-Source: trace-sweep-divergence
Issue-Trace: entityresolution-error-paths-misc
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: entityresolution-error-paths-misc entityresolution-matching-workflow-access-denied entityresolution-schema-mapping-lifecycle
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|CreateSchemaMapping|4xx->2xx|$.message
Issue-Key: entityresolution|CreateSchemaMapping|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: entityresolution|CreateSchemaMapping|4xx->2xx|status
Issue-Key: entityresolution|CreateMatchingWorkflow|4xx->2xx|$.message
Issue-Key: entityresolution|CreateMatchingWorkflow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: entityresolution|CreateMatchingWorkflow|4xx->2xx|status
Issue-Key: entityresolution|DeleteSchemaMapping|2xx->4xx|$.message
Issue-Key: entityresolution|DeleteSchemaMapping|2xx->4xx|status
Issue-Key: entityresolution|GetSchemaMapping|2xx->2xx|$.description
Issue-Key: entityresolution|GetSchemaMapping|2xx->2xx|$.mappedInputFields[*]
Issue-Commit: 0cb111ce1f72d72d1bf8c7c3dcacf1ff8624e565

# entityresolution write path skips validation and drops schema-mapping updates

> _Note: this issue was identified against the code base at commit `0cb111ce1f72d72d1bf8c7c3dcacf1ff8624e565`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchemaMapping

## Description
The entityresolution emulator's create/update path neither rejects requests AWS rejects nor persists the mutations it accepts: two create operations return a 200 success for inputs AWS refuses, and UpdateSchemaMapping leaves stored state untouched. There are 4 distinct causes in this issue and ALL FOUR must be closed.

Cause 1 of 4 — CreateSchemaMapping accepts a schema with no UNIQUE_ID attribute (fixture entityresolution-error-paths-misc, step task#4).
AWS: CreateSchemaMapping requires the schema to declare exactly one attribute of type UNIQUE_ID. Smithy: CreateSchemaMapping.mappedInputFields is @required, targets SchemaInputAttributes (@length min 2 max 60) whose member SchemaInputAttribute.type targets the SchemaAttributeType enum containing UNIQUE_ID; the operation models ValidationException (client error, httpError 400). The recorded response is HTTP 400 with header x-amzn-errortype: ValidationException and message 'SchemaMapping must contain 1 attribute with type: UNIQUE_ID' (that text is an example of the rule).
Emulator: returns HTTP 200 with a created schema mapping; no $.message and no x-amzn-errortype header.
Why: a client that relies on AWS rejecting a malformed schema instead gets a resource it believes is usable; the invalid mapping is then persisted and every later workflow built on it behaves differently against the emulator than against AWS, and error-handling code paths are never exercised.
Fix: input validation for CreateSchemaMapping in the entityresolution service crate — add the UNIQUE_ID-cardinality check over mappedInputFields and raise ValidationException (400, x-amzn-errortype: ValidationException) before the schema mapping is stored.

Cause 2 of 4 — CreateMatchingWorkflow accepts a roleArn it cannot assume (fixture entityresolution-matching-workflow-access-denied, step task#3).
AWS: CreateMatchingWorkflow.roleArn is @required and the service assumes that role before creating the workflow; when assumption fails the call is denied. Smithy: CreateMatchingWorkflow models AccessDeniedException (client error, httpError 403, doc 'You do not have sufficient access to perform this action'). The recorded response is HTTP 403 with message 'Exception in assuming the passed role: <roleArn>' (example text; the fixture's assertion on x-amzn-errortype is '<shaped-not-asserted>'). The requirement here is the 403 status and the AccessDeniedException shape, not the message string or the recorded ARN.
Emulator: returns HTTP 200 and creates the workflow; no $.message and no error header.
Why: the access-denied branch of CreateMatchingWorkflow is unreachable against the emulator, so clients cannot test or handle a role that is not assumable, and the emulator ends up holding a workflow AWS never created.
Fix: the CreateMatchingWorkflow handler/validation in the entityresolution service crate — verify the passed roleArn is assumable (resolvable in the emulator's IAM state) and return AccessDeniedException (403) instead of creating the workflow.

Cause 3 of 4 — DeleteSchemaMapping fails during teardown because of the wrongly created workflow (fixture entityresolution-matching-workflow-access-denied, step teardown#4).
AWS: with the matching workflow never created (cause 2), the schema mapping has no referencing workflow and DeleteSchemaMapping succeeds with HTTP 200 (output member message, @required, doc 'A successful operation message'; recorded value 'SchemaMapping Deleted.').
Emulator: returns HTTP 400 with message 'Schema mapping <name> is referenced by the inputSourceConfig of a matching workflow'. The in-use/ConflictException check itself matches AWS behavior for a mapping that really is referenced; it fires here only because cause 2 created a workflow AWS refused to create.
Why: the fixture cannot tear down its own resources, so a client's cleanup path fails with a conflict that would never occur on AWS and resources leak across runs.
Fix: expected to fall out of cause 2 (CreateMatchingWorkflow must not create the workflow); re-replay this step to confirm, and only touch the DeleteSchemaMapping reference check if it still diverges.

Cause 4 of 4 — UpdateSchemaMapping does not persist the update (fixture entityresolution-schema-mapping-lifecycle, step task#5).
AWS: UpdateSchemaMapping (PUT /schemas/{schemaName}, idempotent) replaces description and the @required mappedInputFields list, and a subsequent GetSchemaMapping returns the updated values. Smithy: UpdateSchemaMapping.description @length max 255, UpdateSchemaMapping.mappedInputFields @required; GetSchemaMappingOutput returns description, mappedInputFields (@required) and updatedAt (@required).
Emulator: GetSchemaMapping after the update still returns the pre-update state — $.description is the old value ('probe schema mapping' instead of 'updated probe schema mapping') and $.mappedInputFields[3] is absent (the newly added attribute, recorded as {'fieldName': 'phone', 'type': 'PHONE_NUMBER'}, was never stored), i.e. the list is short by the appended element.
Why: the update appears to succeed (200) but is a no-op, so any client that updates a schema mapping and then reads it back sees stale fields; read-after-write consistency, which AWS guarantees here, is broken.
Fix: the UpdateSchemaMapping handler/state in the entityresolution service crate — write description and mappedInputFields back into the stored schema mapping (and refresh updatedAt) instead of returning the request echo without mutating state.

Traces (examples): entityresolution-error-paths-misc#task#4 (cause 1), entityresolution-matching-workflow-access-denied#task#3 (cause 2), entityresolution-matching-workflow-access-denied#teardown#4 (cause 3), entityresolution-schema-mapping-lifecycle#task#5 (cause 4).

## Evidence
Cause 1 — CreateSchemaMapping, entityresolution-error-paths-misc#task#4: expected HTTP 400, got 200. 'status: expected 400, got 200; $.message: missing (expected "SchemaMapping must contain 1 attribute with type: UNIQUE_ID", got None); $headers.x-amzn-errortype: missing (expected "ValidationException", got None)'.
Cause 2 — CreateMatchingWorkflow, entityresolution-matching-workflow-access-denied#task#3: expected HTTP 403, got 200. '$.message: missing (expected "Exception in assuming the passed role: arn:aws:iam::000000000000:role/...", got None); $headers.x-amzn-errortype: missing (expected "<shaped-not-asserted>", got None)'. Status 403 + AccessDeniedException is the requirement; the message and ARN are examples.
Cause 3 — DeleteSchemaMapping, entityresolution-matching-workflow-access-denied#teardown#4: expected HTTP 200, got 400. '$.message: value_mismatch (expected "SchemaMapping Deleted.", got "Schema mapping <name> is referenced by the inputSourceConfig of a matching workflow")'. Downstream of cause 2.
Cause 4 — GetSchemaMapping after UpdateSchemaMapping, entityresolution-schema-mapping-lifecycle#task#5: HTTP 200 both sides but '$.description: value_mismatch (expected "updated probe schema mapping", got "probe schema mapping"); $.mappedInputFields[3]: missing (expected {"fieldName": "phone", "type": "PHONE_NUMBER"}, got None)'.
Smithy citations: CreateSchemaMapping errors include ValidationException (400); CreateMatchingWorkflow errors include AccessDeniedException (403); DeleteSchemaMappingOutput.message @required, doc 'A successful operation message'; SchemaAttributeType enum includes UNIQUE_ID; UpdateSchemaMapping.mappedInputFields @required.
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0, score 23.6%.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health is 200.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture entityresolution-error-paths-misc, step task#4 — POST /schemas with mappedInputFields containing no attribute of type UNIQUE_ID; emulator answers 200 instead of 400 ValidationException.
4. Cause 2: replay fixture entityresolution-matching-workflow-access-denied, step task#3 — POST /matchingworkflows with a roleArn that cannot be assumed; emulator answers 200 instead of 403 AccessDeniedException.
5. Cause 3: in the same fixture, step teardown#4 — DELETE /schemas/{schemaName}; emulator answers 400 (referenced by a matching workflow) instead of 200, because step task#3 created a workflow AWS refused.
6. Cause 4: replay fixture entityresolution-schema-mapping-lifecycle, step task#5 — PUT /schemas/{schemaName} with a changed description and an extra mapped input field, then GET /schemas/{schemaName}; the response still carries the pre-update description and omits the appended attribute.
