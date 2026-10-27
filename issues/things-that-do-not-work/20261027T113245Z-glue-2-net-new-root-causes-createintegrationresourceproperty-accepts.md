Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-integration-resource-property-conflict-rejected glue.e2e-column-statistics-task-settings-nonexistent-table-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-integration-resource-property-conflict-rejected|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: glue.e2e-column-statistics-task-settings-nonexistent-table-rejected|run#17|AssertionError|assert code == (
Issue-Commit: 861e7723cbd520f91d3c8b29f6571221cf90c671

# Glue: 2 net-new root causes — CreateIntegrationResourceProperty accepts a request with neither processing property (200 instead of ValidationException), and CreateColumnStatisticsTaskSettings's table-not-found message is request-derived instead of AWS's constant

> _Note: this issue was identified against the code base at commit `861e7723cbd520f91d3c8b29f6571221cf90c671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegrationResourceProperty

## Description
This issue carries 2 net-new root causes in the Glue control plane. Neither failing check is marked ALREADY FILED and neither is marked OWNED by an open issue in the replay report; both handlers named below are implemented in this checkout (they are not scaffold stubs), so these are fidelity divergences, not bootstrap gaps.

=== Cause 1 of 2: CreateIntegrationResourceProperty performs no "at least one processing property" validation — a request carrying neither SourceProcessingProperties nor TargetProcessingProperties is accepted with HTTP 200 and persisted, instead of being refused with ValidationException ===

AWS: Smithy com.amazonaws.glue#CreateIntegrationResourceProperty has input ResourceArn (String512, @required, @length 1..512), plus the two OPTIONAL structures SourceProcessingProperties and TargetProcessingProperties and a Tags list; it models ValidationException (client error, httpError 400, documented "A value could not be validated") alongside AccessDeniedException/ConflictException/EntityNotFoundException/InvalidInputException/ResourceNotFoundException. The model marks both processing-property members optional individually, so the "one of the two must be present" rule is not expressible in the model and is enforced by the service — recorded behavior only: a call that passes ONLY ResourceArn (a real, existing Glue database or connection ARN of this account/Region) is refused with HTTP 400, Error.Code "ValidationException" and Error.Message

    Either source processing properties or target processing properties must be provided in the request.

That message is entirely FIXED text — no part of it is derived from the request or from state (no ARN, no member name, no limit is interpolated), trailing period included. The refusal persists nothing, so a following GetIntegrationResourceProperty for that same ResourceArn is refused with Error.Code "EntityNotFoundException" and the message "The target resource <ResourceArn> is not found." — here "The target resource " and " is not found." are the fixed text and <ResourceArn> is the ARN taken verbatim from the request. (The recording's subsequent legs show the op is an upsert once a property IS supplied: a second create for the same ResourceArn overwrites TargetProcessingProperties.RoleArn and keeps the same ResourcePropertyArn.)

Emulator: the handler validates only the SHAPE of the ARN and the existence of the named resource; it never checks that at least one processing property was supplied. A request with neither is accepted, a record is minted in the per-account integration-resource-property map, and the call answers HTTP 200 with ResourceArn echoed and a freshly minted ResourcePropertyArn (replay: `{'ResourceArn': 'arn:aws:glue:<region>:<account>:database/<db>', 'ResourcePropertyArn': 'arn:aws:glue:<region>:<account>:integrationresourceproperty/<hash>', ...}` with HTTPStatusCode 200 — the concrete ARNs/hash are per-run values, the divergence is the 200 itself). Because the empty record is persisted, the check's follow-on assertion that GetIntegrationResourceProperty still 404s with EntityNotFoundException is also broken by construction: the resource now HAS a property record.

Why: a client that omits both processing properties (a config bug, a half-populated template, a zero-ETL wiring step that lost its RoleArn) is told its integration resource is configured when nothing usable was stored. AWS's refusal is the only signal that the role needed to read the source connection or write the target database is missing; against the emulator that misconfiguration is silently accepted, and the stored empty record then shadows the not-found path, so error-handling and "is it configured yet?" probes both answer the opposite of AWS. SDK/IaC layers that branch on ValidationException never see it.

Fix: the request validation of the Glue CreateIntegrationResourceProperty handler — before any write, refuse a request in which both SourceProcessingProperties and TargetProcessingProperties are absent with the modelled ValidationException (HTTP 400) and AWS's constant message above, and persist nothing on that path so GetIntegrationResourceProperty keeps answering EntityNotFoundException for that ResourceArn.

Traces: glue.create-integration-resource-property-conflict-rejected (mismatch: "expected a ClientError, but the call succeeded and returned {...'HTTPStatusCode': 200...}"). Handler in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/create_integration_resource_property.rs — handle() at lines 164-271 goes straight from the ARN parse (lines 187-200) and the resource-existence check (lines 206-219) to the store write (lines 246-254); there is no branch on input.source_processing_properties / input.target_processing_properties anywhere.

=== Cause 2 of 2: CreateColumnStatisticsTaskSettings's EntityNotFoundException for a missing table is rebuilt from the request ("Table <TableName> not found.") instead of AWS's fixed, request-independent text ===

AWS: Smithy com.amazonaws.glue#CreateColumnStatisticsTaskSettings takes DatabaseName, TableName and Role (all @required, NameString @length 1..255) plus Schedule/ColumnNameList/SampleSize/CatalogID/SecurityConfiguration/Tags, and models EntityNotFoundException as a client error with httpError 400 (documentation: "A specified entity does not exist"). The model documents the exception's Message only as free prose, so the exact wording is recorded behavior only. Recorded behavior: a create addressed at a TableName the addressed catalog's database does not hold is refused with HTTP 400, Error.Code "EntityNotFoundException" and the CONSTANT message

    Table passed as input within input database not found.

Every character of that is fixed text; it has NO variable parts and AWS deliberately does NOT echo the requested table name back. (The same recording then expects the absence-of-effect read, GetColumnStatisticsTaskSettings for that same database/table, to be refused with EntityNotFoundException and the equally constant "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist" — the check stops on the create's message assertion, so that leg was not exercised in this replay and is not part of this cause's evidence, but a fix should keep it in view.)

Emulator: the status and the code are right (HTTP 400 / EntityNotFoundException), but the message is synthesised from the request. The handler's table-lookup miss returns not_found(crate::state::messages::table_not_found(&input.table_name)), i.e. the template "Table <TableName> not found." — different fixed text ("Table " ... " not found.") with the request's TableName interpolated in the middle. In the failing replay the emulator answered 'Table sim2real_cs_neg_tbl_18b2a0774b19-does-not-exist not found.' where AWS answers the constant above; that table name is a per-run value generated by the check from the request and is not part of any requirement.

Why: for this error class the message is a fixed constant and is what clients match on — SDK wrappers, error-classification layers, conformance suites, log scrapers and operator runbooks compare Error.Message against the documented constant. Against the emulator such a matcher never recognises the refusal, so "you pointed the statistics schedule at a table that does not exist" surfaces as an unknown failure; and because the status and code are correct the divergence stays silent until something reads the prose. The request-derived form additionally echoes a caller-supplied identifier that AWS withholds, and it is indistinguishable from the handler's separate database-miss message, so a client cannot tell which half of the name was wrong.

Fix: the not-found refusal path of the Glue CreateColumnStatisticsTaskSettings handler and the shared message helper it calls — raise EntityNotFoundException with AWS's constant message and no interpolation of TableName. Any other operation that reuses the same table-lookup helper for this error class must emit the same constant.

Related open issues (neither covers these causes, and neither carries these checks' replay keys): 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md records the column-statistics RUN always settling to FAILED and CreateColumnStatisticsTaskSettings being a NotImplemented stub (it is now implemented; the not-found MESSAGE is not stated there); 20261027T060729Z-glue-a-column-statistics-task-run-auto-started.md covers an auto-started run's ErrorMessage; 20261027T083036Z-glue-getjob-updatejob-entitynotfoundexception-message-is-request-derived.md and 20261025T213800Z-glue-getjobrun-s-entitynotfoundexception-message-echoes-the-request.md record the SAME CLASS of defect for DIFFERENT operations (GetJob/UpdateJob and GetJobRun) and do not cover the catalog table lookup; 20261025T022820Z-glue-createintegrationtableproperties-stub.md is a different operation (CreateIntegrationTableProperties).

Traces: glue.e2e-column-statistics-task-settings-nonexistent-table-rejected (mismatch). Handler in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/create_column_statistics_task_settings.rs lines 247-254 (the table-lookup miss) and the not_found helper at lines 303-312, which wraps crate::state::messages::table_not_found.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 58/223 checks passed (43 mismatched, 122 errored). Both failures below are net-new: neither is marked ALREADY FILED and neither is marked OWNED in the report.

Cause 1 of 2 — glue.create-integration-resource-property-conflict-rejected [mismatch]:
  File "check_glue_create_integration_resource_property_conflict_rejected.py", line 40, in run
  code, message = _error(lambda: glue.create_integration_resource_property(ResourceArn=resource_arn))
  File "check_glue_create_integration_resource_property_conflict_rejected.py", line 82, in _error
  raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResourceArn': 'arn:aws:glue:us-west-2:000000000000:database/sim2real-db-03ea0e3260cb', 'ResourcePropertyArn': 'arn:aws:glue:us-west-2:000000000000:integrationresourceproperty/6a8bc14dee337ee5189a6db5073a809f', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

  Expected (AWS): CreateIntegrationResourceProperty(ResourceArn=<an existing Glue database ARN>) with NEITHER SourceProcessingProperties nor TargetProcessingProperties -> HTTP 400, Error.Code 'ValidationException', Error.Message exactly 'Either source processing properties or target processing properties must be provided in the request.' (entirely fixed text, nothing interpolated). Nothing is persisted, so GetIntegrationResourceProperty(ResourceArn=<same ARN>) is then refused HTTP 400, Error.Code 'EntityNotFoundException', Error.Message 'The target resource <ResourceArn> is not found.' — fixed text 'The target resource ' + ' is not found.' around the request's ARN.
  Observed (emulator): HTTP 200 with ResourceArn echoed and a minted ResourcePropertyArn; the record is persisted, so the subsequent not-found assertion can no longer hold either. The database ARN and the ResourcePropertyArn hash in the output above are per-run values generated by the check/emulator and are not part of the requirement; the requirement is the 400/ValidationException refusal and its constant message.
  Source of the divergence in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/create_integration_resource_property.rs — no validation of the two processing-property members exists in handle() (lines 164-271); the only refusals are a malformed/unknown ARN (InvalidInputException, lines 187-200), an unknown resource (EntityNotFoundException, lines 206-219) and a store-capacity bound (lines 227-244).

Cause 2 of 2 — glue.e2e-column-statistics-task-settings-nonexistent-table-rejected [mismatch]:
  File "check_glue_e2e_column_statistics_task_settings_nonexistent_table_rejected.py", line 66, in run
  assert code == ("EntityNotFoundException", "Table passed as input within input database not found."), repr(code)
  AssertionError: ('EntityNotFoundException', 'Table sim2real_cs_neg_tbl_18b2a0774b19-does-not-exist not found.')

  Expected (AWS): CreateColumnStatisticsTaskSettings(DatabaseName=<existing db>, TableName=<a name no table in that db holds>, Role=<a role trusting glue.amazonaws.com>) -> HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Table passed as input within input database not found.' — entirely fixed text, with the requested TableName NOT echoed.
  Observed (emulator): HTTP 400 and Error.Code 'EntityNotFoundException' (both correct), Error.Message built from the template 'Table <TableName> not found.' — fixed parts 'Table ' and ' not found.' with the request's TableName interpolated. The concrete name in the trace is a per-run value the check generated.
  Source of the divergence in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/create_column_statistics_task_settings.rs:247-254 returns not_found(crate::state::messages::table_not_found(&input.table_name)); the not_found helper at lines 303-312 builds the modelled EntityNotFoundException from that string.

## Steps to Reproduce
Cause 1 of 2 (glue.create-integration-resource-property-conflict-rejected):
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running glue.create-integration-resource-property-conflict-rejected.
2. iam.create_role twice with a trust policy allowing sts:AssumeRole to Service glue.amazonaws.com (the two role ARNs are used by the later upsert legs).
3. glue.create_database(DatabaseInput={'Name': '<db>'}) and glue.tag_resource(ResourceArn='arn:aws:glue:<region>:<account>:database/<db>', TagsToAdd={...}) so the resource exists and is addressable.
4. glue.create_integration_resource_property(ResourceArn='arn:aws:glue:<region>:<account>:database/<db>') — passing NEITHER SourceProcessingProperties nor TargetProcessingProperties.
5. Expect: ClientError, HTTP 400, Error.Code 'ValidationException', Error.Message exactly 'Either source processing properties or target processing properties must be provided in the request.'
6. Observed: HTTP 200 with a minted ResourcePropertyArn, and the record is persisted.
7. glue.get_integration_resource_property(ResourceArn=<same ARN>) must still be refused HTTP 400 'EntityNotFoundException' with 'The target resource <ResourceArn> is not found.' (the rejected create persists nothing). Against the emulator the record written in step 4 makes this read succeed.
8. Positive controls that must keep working after the fix: create_integration_resource_property(ResourceArn=<same ARN>, TargetProcessingProperties={'RoleArn': <role1>}) succeeds and echoes ResourceArn; a second call with {'RoleArn': <role2>} is an UPSERT that succeeds, and get_integration_resource_property then reports TargetProcessingProperties.RoleArn == <role2>.

Cause 2 of 2 (glue.e2e-column-statistics-task-settings-nonexistent-table-rejected):
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running glue.e2e-column-statistics-task-settings-nonexistent-table-rejected.
2. iam.create_role with a glue.amazonaws.com trust policy, attaching arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole and arn:aws:iam::aws:policy/AmazonS3FullAccess.
3. s3.create_bucket and put_object a small CSV under the prefix 'data/'.
4. glue.create_database(DatabaseInput={'Name': '<db>'}), then glue.create_table(DatabaseName='<db>', TableInput={'Name': '<tbl>', 'TableType': 'EXTERNAL_TABLE', 'StorageDescriptor': {'Columns': [{'Name': 'id', 'Type': 'int'}], 'Location': 's3://<bucket>/data/', TextInputFormat/HiveIgnoreKeyTextOutputFormat/LazySimpleSerDe}}).
5. glue.create_column_statistics_task_settings(DatabaseName='<db>', TableName='<tbl>-does-not-exist', Role='<role arn>').
6. Expect: ClientError, HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly 'Table passed as input within input database not found.'
7. Observed: HTTP 400 and 'EntityNotFoundException' are correct, but Error.Message is 'Table <TableName> not found.' with the requested table name interpolated.
8. Then glue.get_column_statistics_task_settings(DatabaseName='<db>', TableName='<tbl>-does-not-exist') must be refused HTTP 400 'EntityNotFoundException' with the constant 'ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist' (this leg was NOT reached in the failing replay), and the positive control create on the real '<tbl>' with Schedule='cron(0 12 1 1 ? 2099)' must succeed and round-trip Role through GetColumnStatisticsTaskSettings.
