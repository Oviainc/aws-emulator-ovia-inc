Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.column-statistics-task-run-respects-schedule-interval glue.create-column-statistics-task-settings-while-running-rejected glue.get-unfiltered-partition-metadata-not-found-exact-code
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.column-statistics-task-run-respects-schedule-interval|run#17|AssertionError|assert message == "Column Statistics task for input table has already started", message
Issue-Key: glue.create-column-statistics-task-settings-while-running-rejected|_error#15|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: glue.get-unfiltered-partition-metadata-not-found-exact-code|run#19|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# Glue: 3 net-new root causes — StartColumnStatisticsTaskRun rewrites its ColumnStatisticsTaskRunningException message, CreateColumnStatisticsTaskSettings accepts a create while a run is in flight, and GetUnfilteredPartitionMetadata is still a 501 scaffold instead of its recorded AccessDeniedException

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartColumnStatisticsTaskRun

## Description
This issue bundles THREE distinct net-new root causes in the Glue control plane. Cause count: 3. All three must be closed before submitting; a fixer should re-replay every check id in trace_ids.

=== Cause 1 of 3: StartColumnStatisticsTaskRun's in-flight refusal carries emulator-authored, request/state-derived text instead of AWS's fixed message ===
AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models com.amazonaws.glue#StartColumnStatisticsTaskRun with DatabaseName/TableName/Role @required and the error ColumnStatisticsTaskRunningException (errorKind client, httpError 400), documented "An exception thrown when you try to start another job while running a column stats generation job". Recorded behavior: while a column-statistics task run for that same table is in flight (STARTING/RUNNING), a second StartColumnStatisticsTaskRun for it is refused HTTP 400 with Error.Code "ColumnStatisticsTaskRunningException" and Error.Message "Column Statistics task for input table has already started". That message is ENTIRELY fixed text — it has no variable parts at all: it names neither the database, nor the table, nor the in-flight run id.
Emulator: the error CODE is right, but the message is rewritten: "A column statistics generation job is already running for table <database>.<table>", i.e. fixed emulator prose whose two variable parts are the database name and table name taken from the in-flight run in state. See services/service-com-amazonaws-glue-awsglue/src/ops/start_column_statistics_task_run.rs lines 237-255, which builds the message from crate::state::messages::column_statistics_task_already_running(&running.database_name, &running.table_name).
Why: an error message is part of the contract. A client (or an operator runbook, log matcher, or test) that recognises AWS's fixed sentence does not recognise the emulator's, so a legitimate transient conflict that should be retried after the run drains reads as an unknown failure; and because the emulator's text embeds names from state, no client can match on it stably either.
Fix: the refusal's message in the StartColumnStatisticsTaskRun handler (and the shared message helper it calls) — emit AWS's fixed text for this error class and stop interpolating the database/table names.
Traces: glue.column-statistics-task-run-respects-schedule-interval (mismatch: `assert message == "Column Statistics task for input table has already started"` -> AssertionError: "A column statistics generation job is already running for table sim2real-glue-db-<id>.sim2real-glue-tbl-<id>"; the db/table ids in that line are per-run values, the requirement is the fixed sentence).

=== Cause 2 of 3: CreateColumnStatisticsTaskSettings returns 200 and persists settings while a column-statistics run is in flight, instead of refusing with ColumnStatisticsTaskRunningException ===
AWS: Smithy models com.amazonaws.glue#CreateColumnStatisticsTaskSettings (DatabaseName/TableName/Role @required, optional Schedule/ColumnNameList/SampleSize/CatalogID/SecurityConfiguration/Tags; EMPTY output) with the error set AccessDeniedException, AlreadyExistsException, ColumnStatisticsTaskRunningException, EntityNotFoundException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException (all httpError 400). Recorded behavior: with a column-statistics task run in flight for that table, the call is refused HTTP 400, Error.Code "ColumnStatisticsTaskRunningException", Error.Message "Column statistics task for input table has already started" — again entirely fixed text with no variable parts (note this operation's recorded wording differs from StartColumnStatisticsTaskRun's only in the case of "statistics"). The refusal persists NOTHING, so a following GetColumnStatisticsTaskSettings for the same DatabaseName/TableName is refused HTTP 400 EntityNotFoundException with the fixed message "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist"; once the run reaches a terminal state the same create (with a Schedule) succeeds and the settings round-trip the submitted Role.
Emulator: the call SUCCEEDS — HTTP 200 with an empty JSON body (content-length 2) — and stores the settings. The handler services/service-com-amazonaws-glue-awsglue/src/ops/create_column_statistics_task_settings.rs validates the foreign catalog, the Schedule's cron form, database/table existence and an already-existing settings record (lines 184-253), but has NO in-flight-run guard at all, even though the ColumnStatisticsTaskRunningException variant is declared for it (line 113). Nothing consults crate::state's running-task-run lookup the way start_column_statistics_task_run.rs does.
Why: the invariant this error exists to protect — a statistics schedule must not be written over a table whose statistics job is already running — is unenforced, so a client that relies on the refusal to serialise its own writes silently wins a race AWS refuses, and the effect-absent guarantee (a refused create leaves GetColumnStatisticsTaskSettings 404ing) is inverted: the emulator leaves a settings record where AWS leaves none.
Adjacent open issue (does NOT cover this): 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md recorded THIS check id failing because the operation was a NotImplemented stub. The stub has since been implemented, so that diagnosis no longer reproduces; the implementation omits the conflict guard and the check now fails on an ACCEPTANCE (200) rather than on an unmodelled refusal. This is a new defect in a now-implemented handler, not a reopening of a refused call.
Fix: the CreateColumnStatisticsTaskSettings handler's validation — before persisting, refuse with ColumnStatisticsTaskRunningException and its fixed message when a run for that catalog/database/table is in flight, and persist nothing on that refusal.
Traces: glue.create-column-statistics-task-settings-while-running-rejected (mismatch: "expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2'}}").

=== Cause 3 of 3: GetUnfilteredPartitionMetadata is still an unimplemented 501 scaffold, so its recorded AccessDeniedException refusal never happens ===
AWS: Smithy models com.amazonaws.glue#GetUnfilteredPartitionMetadata (CatalogId/DatabaseName/TableName/PartitionValues/SupportedPermissionTypes @required) with the error set EntityNotFoundException, FederationSourceException, FederationSourceRetryableException, GlueEncryptionException, InvalidInputException, OperationTimeoutException, PermissionTypeMismatchException (400) and InternalServiceException (500). `NotImplemented` is not a member of that set and 501 is not a status this operation answers. Recorded behavior (not stated by the model, so recorded-only): this is a Lake Formation application-integration API meant to be called by a query engine through a vended session, and for a plain IAM caller it is refused HTTP 400 with Error.Code "AccessDeniedException" and an EMPTY Error.Message — identically for a partition that exists and for one that was never created, and still so after the caller has been made a Lake Formation Data Lake Administrator AND granted DESCRIBE/SELECT on the database and table. The contract therefore is: the authorization gate is evaluated BEFORE and REGARDLESS of partition existence, and the refusal never leaks a not-found signal or a different error class. The ordinary, non-Lake-Formation-gated GetPartition is unaffected and still refuses an absent partition with EntityNotFoundException and the fixed message "Cannot find partition."
Emulator: the call is refused HTTP 501 with Error.Code "NotImplemented" and the message "operation GetUnfilteredPartitionMetadata is scaffolded but not implemented in this emulator" — fixed scaffold template whose only variable part is the operation name; it echoes nothing from the request or from state. services/service-com-amazonaws-glue-awsglue/src/ops/get_unfiltered_partition_metadata.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker (line 4) and its handle() returns crate::scaffolded::Scaffolded::not_implemented("GetUnfilteredPartitionMetadata") (line 189).
Why: an SDK maps a Glue error code onto a modelled exception class, so a caller that catches AccessDeniedException to mean "this API is not reachable with these credentials; go through a vended session" instead receives an unmodelled code at an unmodelled status and cannot branch. Worse, an emulator that later implements this by answering EntityNotFoundException for an absent partition would leak existence information that AWS withholds from a denied caller — the ordering of the authorization gate relative to the existence check is the thing this operation owes.
Adjacent open issue (does NOT cover this): 20261025T022820Z-glue-10-net-new-root-causes-identity-center.md cause 4 of 10 covers GetUnfilteredTableMetadata, the TABLE variant and its FederationSourceRetryableException; the PARTITION variant and the AccessDeniedException gate below are named nowhere.
Fix: implement services/service-com-amazonaws-glue-awsglue/src/ops/get_unfiltered_partition_metadata.rs, evaluating the Lake Formation authorization gate before any catalog lookup and refusing a plain IAM caller with AccessDeniedException and an empty message regardless of whether the partition exists; delete the SCAFFOLDED marker.
Traces: glue.get-unfiltered-partition-metadata-not-found-exact-code (mismatch: `assert code == "AccessDeniedException"` -> AssertionError: expected AccessDeniedException, got NotImplemented).

## Evidence
Cause 1 of 3 — glue.column-statistics-task-run-respects-schedule-interval [mismatch]
Expected (AWS / recording): second StartColumnStatisticsTaskRun for a table whose run is still STARTING/RUNNING -> HTTP 400, Error.Code "ColumnStatisticsTaskRunningException", Error.Message "Column Statistics task for input table has already started" (the whole message is fixed text; it carries no request- or state-derived values).
Actual (emulator): the code matches, the message does not:
  AssertionError: A column statistics generation job is already running for table sim2real-glue-db-ee78ee470b8e.sim2real-glue-tbl-83c01dfd3e5a
raised at `assert message == "Column Statistics task for input table has already started"`. Fixed part of the emulator's message: "A column statistics generation job is already running for table "; variable parts: the database and table names read out of the in-flight run in state (the ids shown are this run's generated names and are not part of the requirement).
Source: services/service-com-amazonaws-glue-awsglue/src/ops/start_column_statistics_task_run.rs lines 237-255 (crate::state::messages::column_statistics_task_already_running(db, table)).

Cause 2 of 3 — glue.create-column-statistics-task-settings-while-running-rejected [mismatch]
Expected (AWS / recording): with a run in flight, CreateColumnStatisticsTaskSettings(DatabaseName, TableName, Role) with no Schedule -> HTTP 400, Error.Code "ColumnStatisticsTaskRunningException", Error.Message "Column statistics task for input table has already started" (entirely fixed text); nothing persisted, so GetColumnStatisticsTaskSettings(DatabaseName, TableName) -> HTTP 400 EntityNotFoundException "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist" (also entirely fixed text).
Actual (emulator): HTTP 200, empty body:
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}
The settings are persisted, so the check's effect-absent assertion on GetColumnStatisticsTaskSettings was never reached.
Source: services/service-com-amazonaws-glue-awsglue/src/ops/create_column_statistics_task_settings.rs — guards at lines 189-253 cover foreign catalog, cron form, missing database, missing table and an existing settings record; there is no in-flight-run check, although ColumnStatisticsTaskRunningException is declared for the operation at line 113.

Cause 3 of 3 — glue.get-unfiltered-partition-metadata-not-found-exact-code [mismatch]
Expected (recording, both for an existing partition and for one never created): HTTP 400, Error.Code "AccessDeniedException", Error.Message "" (empty) — uniformly, even for a caller registered as a Lake Formation Data Lake Administrator and granted DESCRIBE/SELECT on the database and table. The positive controls in the same check are unaffected: GetPartition returns the existing partition, and for the absent partition values GetPartition -> EntityNotFoundException with the fixed message "Cannot find partition."
Actual (emulator): HTTP 501, Error.Code "NotImplemented", Error.Message "operation GetUnfilteredPartitionMetadata is scaffolded but not implemented in this emulator":
  AssertionError: expected AccessDeniedException, got NotImplemented
Fixed part of the emulator's message: "operation <Op> is scaffolded but not implemented in this emulator"; the only variable part is the operation name. Neither the code nor the 501 status is modelled for this operation.
Source: services/service-com-amazonaws-glue-awsglue/src/ops/get_unfiltered_partition_metadata.rs line 4 (SCAFFOLDED marker) and line 189 (Scaffolded::not_implemented).

## Steps to Reproduce
Cause 1 of 3:
1. Create an IAM role trusting glue.amazonaws.com, a Glue database, and a table with a StorageDescriptor Location under an S3 prefix the role can read.
2. glue.create_column_statistics_task_settings(DatabaseName, TableName, Role, Schedule="cron(0 0 * * ? *)"); confirm get_column_statistics_task_settings reports Schedule.ScheduleExpression and State SCHEDULED.
3. glue.start_column_statistics_task_run(DatabaseName, TableName, Role) and confirm via get_column_statistics_task_run that the run is STARTING/RUNNING.
4. Call glue.start_column_statistics_task_run again for the same table.
   -> AWS: 400 ColumnStatisticsTaskRunningException, message exactly "Column Statistics task for input table has already started".
   -> Emulator: correct code, message "A column statistics generation job is already running for table <db>.<table>".
Replay: glue.column-statistics-task-run-respects-schedule-interval

Cause 2 of 3:
1. Same bucket/role/database/table setup (one string column suffices).
2. glue.start_column_statistics_task_run(DatabaseName, TableName, Role, ColumnNameList=["col1"]); confirm the run is in flight.
3. While it is in flight, glue.create_column_statistics_task_settings(DatabaseName, TableName, Role) with NO Schedule.
   -> AWS: 400 ColumnStatisticsTaskRunningException "Column statistics task for input table has already started".
   -> Emulator: HTTP 200 with an empty body, and the settings are stored.
4. glue.get_column_statistics_task_settings(DatabaseName, TableName).
   -> AWS: 400 EntityNotFoundException "ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist" (the refused create persisted nothing).
   -> Emulator: returns the settings the rejected call should never have written.
Replay: glue.create-column-statistics-task-settings-while-running-rejected

Cause 3 of 3:
1. lakeformation.put_data_lake_settings adding the caller's IAM role ARN to DataLakeAdmins; lakeformation.grant_permissions DESCRIBE on the database and DESCRIBE/SELECT on the table.
2. glue.create_database, glue.create_table with PartitionKeys=[{"Name":"dt","Type":"string"}], glue.create_partition with Values=["2024-01-01"].
3. glue.get_partition(DatabaseName, TableName, PartitionValues=["2024-01-01"]) -> succeeds (positive control).
4. glue.get_unfiltered_partition_metadata(CatalogId=<account>, DatabaseName, TableName, PartitionValues=["2024-01-01"], SupportedPermissionTypes=["COLUMN_PERMISSION"]).
   -> AWS: 400 AccessDeniedException with an EMPTY message. Emulator: 501 NotImplemented.
5. Repeat step 4 with PartitionValues=["1999-12-31"] (never created) -> AWS: the SAME 400 AccessDeniedException with an empty message, never a not-found.
6. glue.get_partition(DatabaseName, TableName, PartitionValues=["1999-12-31"]) -> 400 EntityNotFoundException "Cannot find partition." (unchanged by the denied calls).
Replay: glue.get-unfiltered-partition-metadata-not-found-exact-code
