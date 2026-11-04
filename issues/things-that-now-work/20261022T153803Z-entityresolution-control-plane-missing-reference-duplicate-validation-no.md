Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: entityresolution.create-schema-mapping-duplicate-field-names-rejected entityresolution.create-matching-workflow-invalid-schema-reference-rejected entityresolution.get-schema-mapping-access-denied-without-permission-rejected entityresolution.schema-mapping-create-get-delete-roundtrip entityresolution.delete-schema-mapping-in-use-conflict-rejected
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution.create-schema-mapping-duplicate-field-names-rejected|_error#3|AssertionError|raise AssertionError(f"expected {description} to raise a ClientError, but it SUCCEEDED and returned {result!r}")
Issue-Key: entityresolution.create-matching-workflow-invalid-schema-reference-rejected|_error#8|AssertionError|raise AssertionError(f"expected {description} to raise a ClientError, but it SUCCEEDED and returned {result!r}")
Issue-Key: entityresolution.get-schema-mapping-access-denied-without-permission-rejected|_error_code#9|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: entityresolution.schema-mapping-create-get-delete-roundtrip|run#19|AssertionError|assert update_code == "ResourceNotFoundException", (
Issue-Key: entityresolution.delete-schema-mapping-in-use-conflict-rejected|run#16|AssertionError|assert code_message == expected, f"expected message {expected!r}, got {code_message!r}"
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# entityresolution control plane: missing reference/duplicate validation, no authorization enforcement, UpdateSchemaMapping unimplemented, and unparameterized in-use conflict message

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMatchingWorkflow

## Description
The entityresolution emulator diverges from AWS Entity Resolution on five independent control-plane behaviors found by the behavioral suite. There are 5 distinct causes in this issue and ALL FIVE must be closed. (An open wire issue, 'entityresolution write path skips validation and drops schema-mapping updates' / 20261022T101024Z-entityresolution-write-path-skips-validation-and-drops-schema.md, covers DIFFERENT causes on some of the same operations — the missing UNIQUE_ID-cardinality check on CreateSchemaMapping, the non-assumable-roleArn check on CreateMatchingWorkflow, and UpdateSchemaMapping returning a 200 echo without persisting. None of the five causes below is one of those.)

Cause 1 of 5 — CreateSchemaMapping accepts two mappedInputFields entries with the same fieldName.
AWS: rejects a schema whose mappedInputFields contains duplicate field names with ValidationException. Smithy: CreateSchemaMapping.mappedInputFields is @required, targets SchemaInputAttributes (@length min 2 max 60) whose member SchemaInputAttribute.fieldName is @required (AttributeName, @length max 255, pattern ^[a-zA-Z_0-9- ]*$) and names one distinct input column; CreateSchemaMapping models ValidationException (client error, httpError 400). The duplicate-name rule itself is recorded behavior only: the recording shows the call rejected with ValidationException and nothing created.
Emulator: returns HTTP 200 and stores the mapping with both duplicate entries, e.g. mappedInputFields [{'fieldName':'id','type':'UNIQUE_ID'},{'fieldName':'id','type':'EMAIL_ADDRESS'}].
Why: a client that relies on AWS refusing a self-contradictory schema instead receives a schema mapping it believes is usable; every workflow later built on that mapping behaves differently against the emulator than against AWS, and the rejection branch is unreachable in client tests.
Fix: input validation for CreateSchemaMapping in the entityresolution service crate's CreateSchemaMapping handler — reject duplicate fieldName values across mappedInputFields with ValidationException (400, x-amzn-errortype: ValidationException) before anything is persisted.
Traces: entityresolution.create-schema-mapping-duplicate-field-names-rejected.

Cause 2 of 5 — CreateMatchingWorkflow accepts an inputSourceConfig entry whose schemaName does not exist.
AWS: validates every inputSourceConfig[].schemaName against existing schema mappings and rejects unknown references with ValidationException, creating no workflow (a later GetMatchingWorkflow is a 404 ResourceNotFoundException). Smithy: CreateMatchingWorkflow.inputSourceConfig @required (@length min 1 max 20), models ValidationException (client error, httpError 400); GetMatchingWorkflow models ResourceNotFoundException (404). Recorded message example: 'References schema mapping(s) which does not exist: [<schemaName>]' — the requirement is the 400/ValidationException with the unknown schema name(s) named, not that exact text.
Emulator: the call SUCCEEDS (HTTP 200) and the workflow is created even though no such schema mapping exists.
Why: dangling references are silently accepted, so clients cannot exercise the validation branch and the emulator holds a workflow AWS would never have created; subsequent reads/jobs on that workflow diverge from AWS entirely.
Fix: validation in the CreateMatchingWorkflow handler of the entityresolution service crate — resolve each inputSourceConfig[].schemaName in state and raise ValidationException (400) naming the missing schema mapping(s) instead of creating the workflow.
Traces: entityresolution.create-matching-workflow-invalid-schema-reference-rejected.

Cause 3 of 5 — GetSchemaMapping performs no authorization check.
AWS: a principal from an assumed role with no entityresolution permissions is denied with AccessDeniedException; granting that role the single missing action makes the identical call succeed. Smithy: GetSchemaMapping errors include AccessDeniedException (client error, httpError 403, doc 'You do not have sufficient access to perform this action').
Emulator: returns HTTP 200 with the full schema mapping (schemaName, schemaArn, mappedInputFields, createdAt/updatedAt, tags, hasWorkflows) for the unprivileged assumed-role caller — the request's credentials are ignored.
Why: the emulator is overly permissive on the authorization axis: a client that tests least-privilege policies, or that depends on AccessDeniedException to detect a missing IAM grant, sees success where AWS denies, so policy-related failures are impossible to reproduce.
Fix: authorization in the entityresolution read path (GetSchemaMapping handler, shared request-authorization hook) — evaluate the caller's identity against the emulator's IAM state for entityresolution:GetSchemaMapping and return AccessDeniedException (403, x-amzn-errortype: AccessDeniedException) when the action is not allowed.
Traces: entityresolution.get-schema-mapping-access-denied-without-permission-rejected.

Cause 4 of 5 — UpdateSchemaMapping is a scaffolded stub and answers NotImplemented.
AWS: UpdateSchemaMapping (PUT /schemas/{schemaName}, @idempotent) replaces description and the @required mappedInputFields of an existing mapping, and for a schemaName that does not exist (e.g. just deleted) it fails with ResourceNotFoundException. Smithy: UpdateSchemaMapping errors include ResourceNotFoundException (client error, httpError 404) and ValidationException (400); output carries schemaName/schemaArn/mappedInputFields (@required). Recorded message example for the absent resource: 'SchemaMapping does not exist'.
Emulator: the operation returns error code 'NotImplemented' ('operation UpdateSchemaMapping is scaffolded but not implemented in this emulator') instead of the modeled 404 ResourceNotFoundException (and, for an existing mapping, instead of applying the update).
Why: no client can update a schema mapping against the emulator, and the not-found branch is reported with an unmodeled error code that no SDK error handler matches, so retry/branching logic written for AWS falls through.
Fix: implement the UpdateSchemaMapping handler in the entityresolution service crate — look the schemaName up in state, return ResourceNotFoundException (404) when it is absent, and otherwise write description/mappedInputFields back and refresh updatedAt.
Traces: entityresolution.schema-mapping-create-get-delete-roundtrip.

Cause 5 of 5 — DeleteSchemaMapping's in-use ConflictException does not name the referencing workflow.
AWS: deleting a schema mapping still referenced by a matching workflow fails with ConflictException and the message identifies the referencing workflow, i.e. it is built from server state as 'SchemaMapping associated with Matching Workflow: <workflowName>'. Smithy: DeleteSchemaMapping errors include ConflictException (client error, httpError 400, doc 'The request couldn't be processed because of conflict in the current state of the resource...'); ConflictException.message is ErrorMessage (@length 1..2048).
Emulator: the status and error code are correct (ConflictException) but the message is a fixed generic string, 'SchemaMapping is referenced by the InputSourceConfig of a MatchingWorkflow', with no workflow name — so the identity of the blocking workflow is lost.
Why: callers (and operators reading logs) use that name to find and delete the referencing workflow before retrying; with a constant message they cannot tell which workflow blocks the delete.
Fix: the DeleteSchemaMapping handler/error construction in the entityresolution service crate — format the ConflictException message from the referencing workflow found in state rather than a constant.
Traces: entityresolution.delete-schema-mapping-in-use-conflict-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, entityresolution 4/22 passed, 18 failed (12 errored behind already-filed unimplemented operations, 6 mismatches). All five causes below are from the mismatch set.

Cause 1 — entityresolution.create-schema-mapping-duplicate-field-names-rejected: 'expected the call to raise a ClientError, but it SUCCEEDED and returned {"ResponseMetadata": {"HTTPStatusCode": 200, ...}, "schemaName": "sim2real-er-dupfields-...", "schemaArn": "arn:aws:entityresolution:us-west-2:000000000000:schemamapping/sim2real-er-dupfields-...", "description": "", "mappedInputFields": [{"fieldName": "id", "type": "UNIQUE_ID"}, {"fieldName": "id", "type": "EMAIL_ADDRESS"}]}'. Expected: ValidationException, HTTP 400.

Cause 2 — entityresolution.create-matching-workflow-invalid-schema-reference-rejected: 'expected create_matching_workflow with a missing schemaName to raise a ClientError, but it SUCCEEDED and returned None'. Expected: ValidationException (HTTP 400), message example 'References schema mapping(s) which does not exist: [<missing schema>]', and GetMatchingWorkflow afterwards a ResourceNotFoundException (404).

Cause 3 — entityresolution.get-schema-mapping-access-denied-without-permission-rejected: 'expected the call to be denied, but it SUCCEEDED and returned {"ResponseMetadata": {"HTTPStatusCode": 200, ...}, "schemaName": "sim2real-er-authz-...", "mappedInputFields": [...], "hasWorkflows": false, ...}' for a client built from an assumed role with no entityresolution permissions. Expected: AccessDeniedException, HTTP 403 (the fully-permitted caller's read in the same check succeeded, so the credentials, not the resource, are the variable).

Cause 4 — entityresolution.schema-mapping-create-get-delete-roundtrip: 'expected UpdateSchemaMapping on deleted schema to raise ResourceNotFoundException, got "NotImplemented"'. Server text: 'operation UpdateSchemaMapping is scaffolded but not implemented in this emulator'. Expected: ResourceNotFoundException, HTTP 404 (message example 'SchemaMapping does not exist'). Earlier assertions in the same check (create/get/hasWorkflows=false/delete/get-404) passed.

Cause 5 — entityresolution.delete-schema-mapping-in-use-conflict-rejected: code ConflictException matched; 'expected message "SchemaMapping associated with Matching Workflow: sim2real-er-inuse-wf-74058c0114d9", got "SchemaMapping is referenced by the InputSourceConfig of a MatchingWorkflow"'. The workflow name in the expected text is this run's generated name — the requirement is that the message names the referencing workflow, not that literal string.

Smithy citations (services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json): CreateSchemaMapping POST /schemas, mappedInputFields @required -> SchemaInputAttributes, errors include ValidationException(400)/ConflictException(400)/AccessDeniedException(403); SchemaInputAttribute.fieldName @required; CreateMatchingWorkflow POST /matchingworkflows, inputSourceConfig @required (1..20), errors include ValidationException(400); GetSchemaMapping GET /schemas/{schemaName} @readonly, errors include AccessDeniedException(403) and ResourceNotFoundException(404); UpdateSchemaMapping PUT /schemas/{schemaName} @idempotent, errors include ResourceNotFoundException(404); DeleteSchemaMapping DELETE /schemas/{schemaName}, errors include ConflictException(400); ConflictException.message ErrorMessage @length 1..2048.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from it (the behavioral suite does this; bootstrap_signal was false and 4 entityresolution checks passed, so the service is serving).
2. Replay the behavioral suite simparity-behavioral-aws-v1 and look at the entityresolution mismatches.
3. Cause 1: POST /schemas (create_schema_mapping) with mappedInputFields=[{fieldName:'id',type:'UNIQUE_ID'},{fieldName:'id',type:'EMAIL_ADDRESS'}] — emulator answers 200 and stores the mapping; expected 400 ValidationException with nothing created (check entityresolution.create-schema-mapping-duplicate-field-names-rejected).
4. Cause 2: POST /matchingworkflows (create_matching_workflow) with inputSourceConfig=[{inputSourceARN:<any glue table arn>, schemaName:<a schema mapping that was never created>}] — emulator answers 200; expected 400 ValidationException naming the missing schema mapping, and a following GET /matchingworkflows/{name} must be 404 (check entityresolution.create-matching-workflow-invalid-schema-reference-rejected).
5. Cause 3: create a schema mapping with the default credentials, then sts:AssumeRole into a role with no entityresolution permissions and call GET /schemas/{schemaName} with that session — emulator answers 200; expected 403 AccessDeniedException, and after attaching a policy granting entityresolution:GetSchemaMapping to the same role the identical call must succeed (check entityresolution.get-schema-mapping-access-denied-without-permission-rejected).
6. Cause 4: create then DELETE /schemas/{schemaName}, then PUT /schemas/{schemaName} (update_schema_mapping) — emulator answers error code NotImplemented; expected 404 ResourceNotFoundException. Also PUT against an existing mapping to see the operation is unimplemented at all (check entityresolution.schema-mapping-create-get-delete-roundtrip).
7. Cause 5: create a schema mapping, create a matching workflow whose inputSourceConfig references it, then DELETE /schemas/{schemaName} — status 400/ConflictException is correct but the message is a constant and does not name the referencing workflow (check entityresolution.delete-schema-mapping-in-use-conflict-rejected).
