Issue-Source: trace-sweep-divergence
Issue-Trace: glue-job-trigger-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-job-trigger-error-paths glue-trigger-duplicate-create-error glue-trigger-error-paths glue-table-optimizer-error-paths glue-table-optimizer-lifecycle glue-table-optimizer-nonexistent-table-vs-ineligible-table glue-asset-type-lifecycle-put-get-delete glue-form-type-and-asset-type-lifecycle glue-formtype-assettype-lifecycle glue-formtype-delete-then-not-found glue-not-found-error-paths
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateTrigger|4xx->4xx|$.Message
Issue-Key: glue|CreateTrigger|4xx->4xx|$.__type
Issue-Key: glue|GetTableOptimizer|4xx->4xx|$.Message
Issue-Key: glue|GetTableOptimizer|4xx->4xx|$.__type
Issue-Key: glue|CreateTableOptimizer|4xx->4xx|$.Message
Issue-Key: glue|GetAssetType|4xx->4xx|$.Message
Issue-Key: glue|DeleteFormType|2xx->4xx|$
Issue-Key: glue|DeleteFormType|2xx->4xx|status
Issue-Commit: c912e232849d0cf345219eaaac304bd62b7f7616

# Glue error classification picks the wrong exception for validation versus not-found

> _Note: this issue was identified against the code base at commit `c912e232849d0cf345219eaaac304bd62b7f7616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrigger

## Description
This issue bundles 7 net-new causes with one shared fault: when a request is invalid, the emulator reports the resource as missing (EntityNotFoundException) or conflicting (ConflictException) instead of the validation error AWS raises, and vice versa — so the error CODE, not just the text, is wrong. Two of the seven also fail or refuse an operation AWS accepts. HIGH because clients branch on the error code.

Cause 1 of 7: CreateTrigger naming the same job in both Predicate and Actions.
AWS: Smithy: CreateTrigger errors include InvalidInputException and EntityNotFoundException (both client, httpError 400). A CONDITIONAL trigger whose predicate job is also its action job is a validation failure: __type InvalidInputException, Message the fixed text 'Job cannot be a Predicate and Action for Conditional Trigger.'
Emulator: __type EntityNotFoundException, Message 'Job not found.' — it reports the (existing) job as missing.
Why: a client that creates the job lazily retries forever on EntityNotFoundException, when the real fault is its trigger definition.
Fix: CreateTrigger validation ordering/classification (validate predicate-vs-action overlap before the job lookup).
Traces: glue-job-trigger-error-paths#task#4 (also glue-trigger-duplicate-create-error#task#3).

Cause 2 of 7: CreateTrigger of type CONDITIONAL with no Predicate.
AWS: Smithy: InvalidInputException (client, 400). Message is the fixed text 'Predicate cannot be null or empty'.
Emulator: right code, emulator-authored Message 'Predicate is required when the trigger type is CONDITIONAL'.
Why: message assertions fail; the text is not AWS's.
Fix: CreateTrigger validation message.
Traces: glue-trigger-error-paths#task#4.

Cause 3 of 7: GetTableOptimizer for a table whose type cannot be optimized.
AWS: Smithy: GetTableOptimizer errors include InvalidInputException and EntityNotFoundException (client, 400). An ineligible table type is a validation failure: __type InvalidInputException, Message the fixed text 'Request Validation Failed: Unsupported Table type' plus AWS's trailing '(Service: glue; Status Code: 400; Error Code: InvalidInputException; Request ID: <requestId>; Proxy: null)' clause.
Emulator: __type EntityNotFoundException, Message 'Table optimizer of type <type> for table <tableName> not found.'
Why: the client is told to create an optimizer that can never exist, instead of being told the table is unsupported.
Fix: GetTableOptimizer handler's error classification (table-type check before optimizer lookup).
Traces: glue-table-optimizer-error-paths#task#3.

Cause 4 of 7: GetTableOptimizer for an eligible table with no optimizer of that type.
AWS: EntityNotFoundException is correct here, but the Message follows the template 'No optimizer of type <type> found for the table: <tableName>' plus AWS's trailing '(Service: glue; Status Code: 404; Error Code: EntityNotFoundException; Request ID: <requestId>; Proxy: null)' clause — note AWS reports Status Code 404 inside that clause while the HTTP status stays 400.
Emulator: Message 'Table optimizer of type <type> for table <tableName> not found.'
Why: message assertions fail and the embedded diagnostic clause AWS includes is absent.
Fix: GetTableOptimizer not-found message assembly.
Traces: glue-table-optimizer-lifecycle#task#3 (also glue-table-optimizer-nonexistent-table-vs-ineligible-table#task#4).

Cause 5 of 7: CreateTableOptimizer naming a table that does not exist.
AWS: __type EntityNotFoundException, Message the nested fixed text 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <innerRequestId>; Proxy: null)' followed by the outer '(Service: glue; Status Code: 404; Error Code: EntityNotFoundException; Request ID: <outerRequestId>; Proxy: null)' clause — i.e. AWS surfaces the Data Catalog's own refusal wrapped in Glue's.
Emulator: Message 'Table <tableName> not found.'
Why: clients parsing the wrapped-service diagnostic see none of it.
Fix: CreateTableOptimizer not-found message assembly (propagate the catalog refusal).
Traces: glue-table-optimizer-nonexistent-table-vs-ineligible-table#task#3.

Cause 6 of 7: GetAssetType for a deleted asset type returns an unwrapped message.
AWS: Smithy: GetAssetType error EntityNotFoundException (client, httpError 400). The metadata-catalog operations JSON-wrap their message: Message is the literal string '{"Message":"Asset type <assetTypeName> not found."}'.
Emulator: right code and status, but Message is the bare sentence 'Asset type <assetTypeName> not found' — no JSON envelope and no trailing period.
Why: a client that json-parses Message (required for the metadata-catalog family) fails.
Fix: the metadata-catalog error serializer used by GetAssetType (wrap Message as JSON).
Traces: glue-asset-type-lifecycle-put-get-delete#task#5 (cluster also covers glue-form-type-and-asset-type-lifecycle#teardown#11, glue-formtype-assettype-lifecycle#teardown#6, glue-formtype-delete-then-not-found#task#7, glue-not-found-error-paths#task#2).

Cause 7 of 7: DeleteFormType refuses with a spurious dependency conflict.
AWS: DeleteFormType of a form type whose only reference is the asset type being torn down in the same flow succeeds: HTTP 200, empty body.
Emulator: HTTP 400 with __type ConflictException and Message '{"Message":"Form type <name> cannot be deleted because of the following dependencies: ..."}' — it counts a dependency AWS does not.
Why: teardown cannot complete; the form type can never be deleted.
Fix: DeleteFormType handler's dependency check (which references actually block a delete).
Traces: glue-formtype-delete-then-not-found#task#3.

## Evidence
Replay of simparity-wire-aws-v1:
1. CreateTrigger 400/400: $.__type expected 'InvalidInputException' got 'EntityNotFoundException'; $.Message expected 'Job cannot be a Predicate and Action for Conditional Trigger.' got 'Job not found.'
2. CreateTrigger 400/400: $.Message expected 'Predicate cannot be null or empty' got 'Predicate is required when the trigger type is CONDITIONAL'.
3. GetTableOptimizer 400/400: $.__type expected 'InvalidInputException' got 'EntityNotFoundException'; $.Message expected 'Request Validation Failed: Unsupported Table type (Service: glue; Status Code: 400; Error Code: InvalidInputException; Request ID: ...; Proxy: null)' got 'Table optimizer of type compaction for table sim2real-probe-glue-opt_tbl-326629c3 not found.'
4. GetTableOptimizer 400/400: $.Message expected 'No optimizer of type compaction found for the table: sim2real-probe-glue-glue-tbl-60585968 (Service: glue; Status Code: 404; Error Code: EntityNotFoundException; Request ID: ...; Proxy: null)' got 'Table optimizer of type compaction for table ... not found.'
5. CreateTableOptimizer 400/400: $.Message expected 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: ...; Proxy: null) (Service: glue; Status Code: 404; ...)' got 'Table sim2real-glue-optnf-missing-0b00aac8 not found.'
6. GetAssetType 400/400: $.Message expected '{"Message":"Asset type SimBehavAssetTypeLCPGDb9552bebc5 not found."}' got 'Asset type SimBehavAssetTypeLCPGDb9552bebc5 not found'.
7. DeleteFormType: status expected 200, got 400; expected body {}, got {'__type': 'ConflictException', 'Message': '{"Message":"Form type FormTypeRetention0d613640 cannot be deleted because of the following dependencies: gluetype glueAsset#..."}'}.
(Table/type names and request ids above are that run's values; per-call request ids are not part of the requirement.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Re-replay: glue-job-trigger-error-paths#task#4, glue-trigger-error-paths#task#4, glue-table-optimizer-error-paths#task#3, glue-table-optimizer-lifecycle#task#3, glue-table-optimizer-nonexistent-table-vs-ineligible-table#task#3, glue-asset-type-lifecycle-put-get-delete#task#5, glue-formtype-delete-then-not-found#task#3.
3. Compare $.__type, HTTP status and $.Message against the AWS values quoted per cause.
