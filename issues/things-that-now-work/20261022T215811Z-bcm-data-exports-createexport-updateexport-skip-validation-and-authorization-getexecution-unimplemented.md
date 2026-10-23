Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.create-export-invalid-name-pattern-rejected bcm-data-exports.create-export-invalid-query-statement-rejected bcm-data-exports.create-export-without-iam-permission-rejected bcm-data-exports.get-execution-not-found-for-bogus-execution-id bcm-data-exports.get-export-cross-account-arn-isolated bcm-data-exports.get-table-invalid-table-name-rejected bcm-data-exports.tag-resource-exceeds-max-tag-count-rejected bcm-data-exports.update-export-full-replace-does-not-merge-table-configurations
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports.create-export-invalid-name-pattern-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bcm-data-exports.create-export-invalid-query-statement-rejected|_create_export_rejected#5|AssertionError|raise AssertionError(
Issue-Key: bcm-data-exports.create-export-without-iam-permission-rejected|_error#5|AssertionError|raise AssertionError(f"expected rejection, but call succeeded and returned {result!r}")
Issue-Key: bcm-data-exports.get-execution-not-found-for-bogus-execution-id|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: bcm-data-exports.get-export-cross-account-arn-isolated|run#8|AssertionError|assert tag_code == "ValidationException", (
Issue-Key: bcm-data-exports.get-table-invalid-table-name-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bcm-data-exports.tag-resource-exceeds-max-tag-count-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bcm-data-exports.update-export-full-replace-does-not-merge-table-configurations|_error_code#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# CreateExport/UpdateExport skip validation and authorization; GetExecution unimplemented; validation-message and tagging error classification diverge

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
The bcm-data-exports control plane diverges from AWS on its rejection paths: two mutating operations accept requests AWS refuses, one operation is not implemented at all, the tagging ARN check is classified as the wrong error, and three validation messages are rendered by a rule AWS does not use. 8 causes.

Model consulted throughout: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model/com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports.json (awsJson1_1, sigv4, sdkId 'BCM Data Exports'). Service-common errors: InternalServerException, ThrottlingException, ValidationException(@httpError 400, Message required).

NOTE on an existing open issue: 20261022T061241Z-bcm-data-exports-export-lifecycle-errors-validation-and-tag-state-diverge (wire corpus) describes closely related symptoms for Cause 1 and Cause 2 below from WIRE fixtures. It does not carry these behavioral check ids, so these causes are filed here with their behavioral replay keys; a fixer closing that issue should re-replay the check ids listed under each cause here as well.

Cause 1 of 8: CreateExport's pattern-constraint validation message renders the member pattern WITH the regex anchors AWS strips.
AWS: the standard AWS validation message is built from the request and names the member path plus the constraint it failed. Smithy: Export.Name -> ExportName, @required, @length(min 1, max 128), @pattern("^[0-9A-Za-z\\-_]+$"); rejection is the service-common ValidationException (@httpError 400). Recorded behavior shows AWS rendering that constraint with the anchors removed, e.g. "1 validation error detected: Value 'bad name! (not allowed)' at 'export.name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z\\-_]+" (recorded text is an example; the RULE -- message built from the member path and the pattern rendered without ^/$ -- is the requirement).
Emulator: same HTTP 400 and same ValidationException code, but emits the raw anchored modeled pattern: "... regular expression pattern: ^[0-9A-Za-z\\-_]+$".
Why: clients, SDK test suites and UIs that parse or display the failed constraint out of the validation message receive a pattern string AWS never emits, so constraint-driven retry/repair logic and golden-message assertions break.
Fix: the shared validation-message renderer for @pattern constraints in this service's input validation -- strip a leading ^ and trailing $ when rendering a member's pattern.
Traces: bcm-data-exports.create-export-invalid-name-pattern-rejected.

Cause 2 of 8: CreateExport accepts a DataQuery.QueryStatement that names a table outside the data-exports catalog and returns 200.
AWS: Smithy CreateExport errors = AccessDeniedException, ServiceQuotaExceededException + service-common ValidationException(400). Recorded behavior: AWS rejects a QueryStatement whose FROM names a table that is not in ListTables with HTTP 400 / code ValidationException and a Message naming the offending table (recorded example: "Table NOT_A_REAL_TABLE_XYZ is invalid"); nothing is created.
Emulator: HTTP 200 with an ExportArn -- the export is persisted with an unsatisfiable query.
Why: the client's error handling for an unsupported table never runs; it believes it owns a working export, and the failure surfaces later as missing deliveries (or never). It also leaves junk definitions in ListExports.
Fix: CreateExport input validation (the create_export handler's validation step) -- resolve the query's table name against the same table catalog ListTables/GetTable serve and raise ValidationException(400) naming the table.
Traces: bcm-data-exports.create-export-invalid-query-statement-rejected.

Cause 3 of 8: CreateExport performs no authorization -- a caller with no IAM permission for bcm-data-exports succeeds instead of being denied.
AWS: Smithy CreateExport models AccessDeniedException (@httpError 403, Message required) as an operation error, and the service is sigv4-authenticated. Recorded behavior: a principal with no bcm-data-exports permissions gets HTTP 403 / code AccessDeniedException and no export is created.
Emulator: HTTP 200 with an ExportArn for the restricted principal; the export is created.
Why: this is an overly-permissive divergence. Any test or product that validates least-privilege policy for data exports passes against the emulator and fails in AWS, and callers never exercise their AccessDenied path.
Fix: the authorization check in front of the create_export handler (identity/permission evaluation for this service), returning AccessDeniedException(403) before any state mutation.
Traces: bcm-data-exports.create-export-without-iam-permission-rejected.

Cause 4 of 8: GetExecution is not implemented -- it answers with a NotImplemented error instead of the modeled not-found behavior.
AWS: Smithy GetExecution(GetExecutionRequest -> GetExecutionResponse) errors = ResourceNotFoundException (@httpError 404 modeled; Message/ResourceId/ResourceType required) + service-common errors. Recorded behavior: an unknown ExecutionId against an otherwise valid ExportArn is answered with code ResourceNotFoundException.
Emulator: error code 'NotImplemented' ("operation ... is scaffolded but not implemented in this emulator") for every GetExecution call, including the valid-export/bogus-execution-id case.
Why: execution-history polling is the normal way a client confirms an export ran; a NotImplemented code is not in the operation's modeled error set, so SDK error handling and retry classification have no branch for it and the client cannot distinguish 'no such execution' from a broken service.
Fix: implement the GetExecution handler in this service's ops module (services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/) against the execution state, including the ResourceNotFoundException path for an unknown ExecutionId.
Traces: bcm-data-exports.get-execution-not-found-for-bogus-execution-id.

Cause 5 of 8: ListTagsForResource misclassifies a non-taggable / cross-account ResourceArn as ResourceNotFoundException instead of ValidationException.
AWS: Smithy ListTagsForResource.ResourceArn -> Arn, @required, @pattern("^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$"); errors = AccessDeniedException, ResourceNotFoundException + service-common ValidationException(400). Recorded behavior: AWS validates the ResourceArn (account/resource shape) BEFORE any existence lookup, so an ARN naming a different account / a resource form that does not support tagging is rejected with code ValidationException (status 400), recorded Message example "Resource doesn't support tagging".
Emulator: code ResourceNotFoundException for that same ARN. (GetExport on the same cross-account ARN is correctly ResourceNotFoundException -- only the tagging path is misclassified.)
Why: the two codes mean different things to a client: ResourceNotFound invites a create-then-retry, ValidationException says the ARN itself is unusable. It also makes the emulator's answer depend on existence for an ARN AWS never looks up, which is the cross-tenant probe this check exists to rule out.
Fix: the ResourceArn validation step of the list_tags_for_resource handler -- validate ARN account/resource form first and raise ValidationException(400) before consulting the export store.
Traces: bcm-data-exports.get-export-cross-account-arn-isolated.

Cause 6 of 8: GetTable's unknown-TableName rejection message does not follow AWS's rule.
AWS: Smithy GetTable has no operation-specific errors, so the rejection is the service-common ValidationException(@httpError 400). Recorded behavior: AWS answers code ValidationException with a message built from the request that names the table and states it was not found -- recorded example "Table NOT_A_REAL_TABLE_XYZ not found". The rule (message naming the requested TableName and reporting it as not found) is what is being replayed.
Emulator: correct code ValidationException, but a different sentence that reports the table as unsupported and adds a remediation pointer: "Table NOT_A_REAL_TABLE_XYZ is not supported. Call ListTables for the tables available in data exports."
Why: lowest-severity cause in this bundle -- status and code are right. It matters only for clients and conformance suites that assert on the request-derived message; the recorded sentence is quoted as an example, the request-derived form is the requirement.
Fix: the message construction in the get_table handler's TableName validation.
Traces: bcm-data-exports.get-table-invalid-table-name-rejected.

Cause 7 of 8: TagResource's over-limit ResourceTags message does not follow AWS's length-constraint rendering.
AWS: Smithy ResourceTagList = list<ResourceTag> with @length(min 0, max 200); TagResourceRequest.ResourceTags @required. A 201-element list is rejected with the service-common ValidationException(400) and AWS's standard constraint message built from the request: "1 validation error detected: Value '[<rendered list>]' at 'resourceTags' failed to satisfy constraint: Member must have length less than or equal to 200", where each element renders as ResourceTag(key=..., value=...). The rule (member path 'resourceTags' + the modeled max length) is the requirement, not the recorded element text.
Emulator: the status and code are right (ValidationException), but the Message does not match that rendering -- the suite's diff for this check was truncated before the emulator's text, so only the divergence is established, not the exact substitute wording.
Why: same class of breakage as Cause 1 -- the rendered constraint is how a client learns WHICH member and WHICH limit it violated when batching tags.
Fix: the validation-message renderer for @length constraints on list members (same component as Cause 1), used by the tag_resource input validation.
Traces: bcm-data-exports.tag-resource-exceeds-max-tag-count-rejected.

Cause 8 of 8: UpdateExport accepts a COST_AND_USAGE_REPORT export definition that omits DataQuery.TableConfigurations and returns 200, silently keeping/ignoring the prior value.
AWS: Smithy UpdateExport doc is explicit -- "Updates an existing data export by overwriting all export parameters. All export parameters must be specified in the UpdateExport request." Errors = ResourceNotFoundException + service-common ValidationException(400). Recorded behavior: an UpdateExport whose DataQuery names COST_AND_USAGE_REPORT but omits TableConfigurations is rejected with code ValidationException (recorded Message example: "'TableConfigurations' must be provided when updating a COST_AND_USAGE_REPORT export") and the stored definition is left unchanged.
Emulator: HTTP 200 with an ExportArn -- the update is accepted.
Why: UpdateExport is a full replace, so a client that round-trips a definition and drops TableConfigurations gets no error and cannot tell whether the per-table properties (granularity, resource inclusion, billing view) were cleared or retained. That silently changes what the export delivers.
Fix: the update_export handler's validation of the submitted Export -- require TableConfigurations for a COST_AND_USAGE_REPORT DataQuery and reject with ValidationException(400) before mutating state.
Traces: bcm-data-exports.update-export-full-replace-does-not-merge-table-configurations.

Additional observation (not filed as a separate cause, no assertion reached it): every ExportArn minted in the replay output carries the CALLING region, e.g. 'arn:aws:bcm-data-exports:us-west-2:000000000000:export/<name>'. Several checks in this suite document that bcm-data-exports is region-pinned and that AWS mints export ARNs in us-east-1 regardless of the client's region. If that pinning is intended to come from the engine's effective_region(), the create_export ARN minting is not using it; once Causes 2/3/8 are fixed the positive-control assertions that read these ARNs will exercise it.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator booted from mainline: 10/21 passed (47.6%); bcm-data-exports 10/21, 11 failed (8 mismatches clustered here; 3 errored checks are blocked by UNIMPLEMENTED TagResource and are ALREADY FILED, so they are excluded).

Cause 1 — bcm-data-exports.create-export-invalid-name-pattern-rejected: AssertionError: expected message "1 validation error detected: Value 'bad name! (not allowed)' at 'export.name' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9A-Za-z\\-_]+", got "... regular expression pattern: ^[0-9A-Za-z\\-_]+$". Code was ValidationException as expected.

Cause 2 — bcm-data-exports.create-export-invalid-query-statement-rejected: QueryStatement "SELECT some_column FROM NOT_A_REAL_TABLE_XYZ". AssertionError: expected CreateExport to reject 'sim2real-bcmde-badquery-...', but it succeeded and returned {'ExportArn': 'arn:aws:bcm-data-exports:us-west-2:000000000000:export/sim2real-bcmde-badquery-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.

Cause 3 — bcm-data-exports.create-export-without-iam-permission-rejected: a restricted client with no bcm-data-exports permissions. AssertionError: expected rejection, but call succeeded and returned {'ExportArn': 'arn:aws:bcm-data-exports:us-west-2:000000000000:export/sim2real-bcmde-denied-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected code AccessDeniedException (HTTP 403).

Cause 4 — bcm-data-exports.get-execution-not-found-for-bogus-execution-id: AssertionError: expected ResourceNotFoundException, got 'NotImplemented'. The export itself was created successfully first, so the valid-ExportArn precondition held.

Cause 5 — bcm-data-exports.get-export-cross-account-arn-isolated: ResourceArn 'arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000'. GetExport correctly answered ResourceNotFoundException with 'No matching definition found'; ListTagsForResource then failed: AssertionError: expected ValidationException for a cross-account ListTagsForResource, got 'ResourceNotFoundException' (expected Message example "Resource doesn't support tagging").

Cause 6 — bcm-data-exports.get-table-invalid-table-name-rejected: positive control GetTable(TableName=COST_AND_USAGE_REPORT) succeeded. AssertionError: expected message 'Table NOT_A_REAL_TABLE_XYZ not found', got 'Table NOT_A_REAL_TABLE_XYZ is not supported. Call ListTables for the tables available in data exports.' Code ValidationException matched.

Cause 7 — bcm-data-exports.tag-resource-exceeds-max-tag-count-rejected: 201 tags via TagResource(ResourceArn=<own export>, ResourceTags=[{k0,v0}..{k200,v200}]). Code assertion (ValidationException) passed; the failure is at the message assertion (line 29): expected "1 validation error detected: Value '[ResourceTag(key=k0, value=v0), ... ]' at 'resourceTags' failed to satisfy constraint: Member must have length less than or equal to 200". The suite's rendering of the emulator's actual message was truncated by the report's size limit.

Cause 8 — bcm-data-exports.update-export-full-replace-does-not-merge-table-configurations: export created with TableConfigurations {'COST_AND_USAGE_REPORT': {'TIME_GRANULARITY': 'DAILY'}}; UpdateExport sent with a COST_AND_USAGE_REPORT QueryStatement and TableConfigurations omitted entirely. AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ExportArn': 'arn:aws:bcm-data-exports:us-west-2:000000000000:export/sim2real-bcmde-export-5bb1c1353408', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.

Model consulted: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model — Export.Name(@required, @length 1..128, @pattern ^[0-9A-Za-z\-_]+$), ResourceTagList(@length 0..200), TagResourceRequest.ResourceArn/ResourceTags(@required), ListTagsForResourceRequest.ResourceArn(@required, Arn @pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):...$), GetTableRequest.TableName(@required), ValidationException(client, httpError 400, Message required), ResourceNotFoundException(client, httpError 404), AccessDeniedException(client, httpError 403), CreateExport errors [AccessDeniedException, ServiceQuotaExceededException], GetExecution/UpdateExport errors [ResourceNotFoundException], UpdateExport doc 'overwriting all export parameters. All export parameters must be specified'.

## Steps to Reproduce
1. git clone the mainline checkout and boot the emulator from it.
2. Replay the behavioral corpus simparity-behavioral-aws-v1 (service bcm-data-exports). Each cause below is one check id; re-replay exactly the ids in trace_ids to confirm the fix.
3. Cause 1: run check bcm-data-exports.create-export-invalid-name-pattern-rejected — CreateExport with Export.Name='bad name! (not allowed)'; observe the ValidationException message quoting the anchored pattern '^[0-9A-Za-z\-_]+$'.
4. Cause 2: run bcm-data-exports.create-export-invalid-query-statement-rejected — CreateExport with DataQuery.QueryStatement 'SELECT some_column FROM NOT_A_REAL_TABLE_XYZ'; observe HTTP 200 instead of ValidationException(400), and the bogus export appearing in ListExports.
5. Cause 3: run bcm-data-exports.create-export-without-iam-permission-rejected — call CreateExport with a principal that has no bcm-data-exports permissions; observe HTTP 200 instead of AccessDeniedException(403).
6. Cause 4: run bcm-data-exports.get-execution-not-found-for-bogus-execution-id — create an export, then GetExecution(ExportArn=<that arn>, ExecutionId=<bogus>); observe error code 'NotImplemented' instead of ResourceNotFoundException.
7. Cause 5: run bcm-data-exports.get-export-cross-account-arn-isolated — ListTagsForResource(ResourceArn='arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000'); observe ResourceNotFoundException instead of ValidationException(400).
8. Cause 6: run bcm-data-exports.get-table-invalid-table-name-rejected — GetTable(TableName='NOT_A_REAL_TABLE_XYZ'); observe the 'is not supported. Call ListTables...' message instead of the request-derived 'not found' form.
9. Cause 7: run bcm-data-exports.tag-resource-exceeds-max-tag-count-rejected — TagResource on an owned export with 201 ResourceTags; observe the ValidationException Message not matching the standard 'at 'resourceTags' failed to satisfy constraint: Member must have length less than or equal to 200' rendering.
10. Cause 8: run bcm-data-exports.update-export-full-replace-does-not-merge-table-configurations — create an export with TableConfigurations {'COST_AND_USAGE_REPORT': {'TIME_GRANULARITY': 'DAILY'}}, then UpdateExport with a COST_AND_USAGE_REPORT QueryStatement and TableConfigurations omitted; observe HTTP 200 instead of ValidationException(400).
