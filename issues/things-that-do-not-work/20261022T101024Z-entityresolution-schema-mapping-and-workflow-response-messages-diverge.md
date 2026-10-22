Issue-Source: trace-sweep-divergence
Issue-Trace: entityresolution-schema-mapping-lifecycle
Issue-Step: teardown#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: entityresolution-schema-mapping-lifecycle entityresolution-schema-mapping-tagging entityresolution-error-paths-misc
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|DeleteSchemaMapping|2xx->2xx|$.message
Issue-Key: entityresolution|DeleteMatchingWorkflow|4xx->4xx|$.message
Issue-Key: entityresolution|GetMatchingWorkflow|4xx->4xx|$.message
Issue-Key: entityresolution|CreateSchemaMapping|4xx->4xx|$.message
Issue-Key: entityresolution|GetSchemaMapping|4xx->4xx|$.message
Issue-Commit: 0cb111ce1f72d72d1bf8c7c3dcacf1ff8624e565

# entityresolution schema-mapping and workflow response messages diverge from AWS

> _Note: this issue was identified against the code base at commit `0cb111ce1f72d72d1bf8c7c3dcacf1ff8624e565`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSchemaMapping

## Description
The entityresolution emulator returns the right HTTP status and the right error shape on these paths, but it synthesizes its own body text instead of the text AWS produces. One case is a modeled, @required SUCCESS-response member (not an error message); the other four are the fixed/templated strings AWS uses for not-found and already-exists. All of them are stable strings with no host, id, timestamp or account in them. There are 5 distinct causes in this issue and ALL FIVE must be closed.

Cause 1 of 5 — DeleteSchemaMapping success message (fixture entityresolution-schema-mapping-lifecycle, step teardown#7; cluster also covers entityresolution-schema-mapping-tagging#teardown#6).
AWS: DeleteSchemaMapping returns HTTP 200 with output member message, Smithy: DeleteSchemaMappingOutput.message @required, target String, doc 'A successful operation message'. The value AWS returns is the fixed string 'SchemaMapping Deleted.' — a value of a modeled required output member, not an error message.
Emulator: HTTP 200 but $.message is a constructed sentence naming the resource, e.g. 'Successfully deleted <schemaName>'.
Why: a required modeled output field carries a value no AWS caller would ever see; a client that asserts on or logs the operation message (and anything that round-trips the response shape) diverges.
Fix: the DeleteSchemaMapping serializer/handler in the entityresolution service crate — emit the service's constant success message for this member.

Cause 2 of 5 — GetSchemaMapping not-found message (fixture entityresolution-schema-mapping-lifecycle, step teardown#8).
AWS: HTTP 404 ResourceNotFoundException (Smithy: ResourceNotFoundException, client error, httpError 404, member message). Recorded behavior only for the text: AWS uses the constant 'SchemaMapping does not exist' and does NOT name the requested schema.
Emulator: 404 ResourceNotFoundException (status and shape correct) with 'Schema mapping <schemaName> not found'.
Why: the status and error shape are right, so this is a body-fidelity gap, but it is a deterministic constant that recorded clients compare against.
Fix: the GetSchemaMapping handler's not-found path in the entityresolution service crate.

Cause 3 of 5 — CreateSchemaMapping already-exists message (fixture entityresolution-schema-mapping-lifecycle, step task#6).
AWS: HTTP 400 ConflictException (Smithy: ConflictException, client error, httpError 400, doc 'The request couldn't be processed because of conflict in the current state of the resource. Example: Workflow already exists, Schema already exists...'). Recorded behavior only for the text: the constant 'SchemaMapping already exists.', with no resource name.
Emulator: 400 with 'Schema mapping <schemaName> already exists'.
Why: same as cause 2 — correct status/shape, diverging constant body text on the duplicate-create path.
Fix: the CreateSchemaMapping conflict path in the entityresolution service crate.

Cause 4 of 5 — GetMatchingWorkflow not-found message (fixture entityresolution-error-paths-misc, step task#5).
AWS: HTTP 404 ResourceNotFoundException. Recorded behavior only for the text: the constant 'MatchingWorkflow Not Found' (note AWS uses a DIFFERENT wording here than on DeleteMatchingWorkflow, cause 5 — they must not be collapsed into one shared string).
Emulator: 404 with 'Matching workflow <workflowName> not found'.
Why: correct status/shape, diverging constant text.
Fix: the GetMatchingWorkflow handler's not-found path in the entityresolution service crate.

Cause 5 of 5 — DeleteMatchingWorkflow not-found message (fixture entityresolution-error-paths-misc, step task#1).
AWS: HTTP 404 ResourceNotFoundException with a message built from the request: 'MatchingWorkflow <workflowName> does not exist', where <workflowName> is the httpLabel input member (Smithy: DeleteMatchingWorkflow.workflowName @required, @httpLabel). The rule is that template, not the recorded name.
Emulator: 404 with 'Matching workflow <workflowName> not found'.
Why: correct status/shape, but the message does not follow the documented-by-recording template AWS uses for this operation.
Fix: the DeleteMatchingWorkflow handler's not-found path in the entityresolution service crate.

No existing open issue covers any of these (list_existing_issues found no entityresolution issues). Severity is MEDIUM because cause 1 is a wrong value in a @required modeled output member; causes 2-5 are error-body text on paths whose status and error shape already match.

Traces (examples): entityresolution-schema-mapping-lifecycle#teardown#7 and entityresolution-schema-mapping-tagging#teardown#6 (cause 1), entityresolution-schema-mapping-lifecycle#teardown#8 (cause 2), entityresolution-schema-mapping-lifecycle#task#6 (cause 3), entityresolution-error-paths-misc#task#5 (cause 4), entityresolution-error-paths-misc#task#1 (cause 5).

## Evidence
Cause 1 — DeleteSchemaMapping, entityresolution-schema-mapping-lifecycle#teardown#7 (affects 2 steps, also entityresolution-schema-mapping-tagging#teardown#6): HTTP 200 expected and got, but '$.message: value_mismatch (expected "SchemaMapping Deleted.", got "Successfully deleted <schemaName>")'. Smithy: DeleteSchemaMappingOutput.message @required, doc 'A successful operation message'.
Cause 2 — GetSchemaMapping, entityresolution-schema-mapping-lifecycle#teardown#8: HTTP 404 both sides, '$.message: value_mismatch (expected "SchemaMapping does not exist", got "Schema mapping <schemaName> not found")'.
Cause 3 — CreateSchemaMapping, entityresolution-schema-mapping-lifecycle#task#6: HTTP 400 both sides, '$.message: value_mismatch (expected "SchemaMapping already exists.", got "Schema mapping <schemaName> already exists")'.
Cause 4 — GetMatchingWorkflow, entityresolution-error-paths-misc#task#5: HTTP 404 both sides, '$.message: value_mismatch (expected "MatchingWorkflow Not Found", got "Matching workflow <workflowName> not found")'.
Cause 5 — DeleteMatchingWorkflow, entityresolution-error-paths-misc#task#1: HTTP 404 both sides, '$.message: value_mismatch (expected "MatchingWorkflow <workflowName> does not exist", got "Matching workflow <workflowName> not found")'.
Smithy citations: ResourceNotFoundException (client, httpError 404, member message, @length 1..2048); ConflictException (client, httpError 400, doc names 'Schema already exists'); DeleteSchemaMappingOutput.message @required; DeleteMatchingWorkflow.workflowName @required @httpLabel.
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture entityresolution-schema-mapping-lifecycle, step teardown#7 — DELETE /schemas/{schemaName} for an existing, unreferenced mapping; compare $.message with the constant AWS returns. Same divergence at entityresolution-schema-mapping-tagging#teardown#6.
4. Cause 2: same fixture, step teardown#8 — GET /schemas/{schemaName} for a deleted mapping; 404 body message differs.
5. Cause 3: same fixture, step task#6 — POST /schemas re-creating an existing schemaName; 400 ConflictException body message differs.
6. Cause 4: fixture entityresolution-error-paths-misc, step task#5 — GET /matchingworkflows/{workflowName} for a missing workflow; 404 body message differs.
7. Cause 5: same fixture, step task#1 — DELETE /matchingworkflows/{workflowName} for a missing workflow; 404 body message differs, and note AWS's wording here differs from cause 4.
