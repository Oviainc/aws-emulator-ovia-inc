Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.column-statistics-task-settings-duplicate-create-rejected glue.delete-session-while-statement-running-accepted glue.deploy-job-missing-output-permission-rejected glue.integration-resource-property-lifecycle glue.integration-table-properties-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.column-statistics-task-settings-duplicate-create-rejected|run#31|AssertionError|assert runs, "expected GetColumnStatisticsTaskRuns to show a run after a successful Create, got none"
Issue-Key: glue.delete-session-while-statement-running-accepted|_wait_statement_state#8|AssertionError|raise AssertionError(f"statement {statement_id} never reached {states}; last state={last}")
Issue-Key: glue.deploy-job-missing-output-permission-rejected|run#19|AssertionError|assert good_run["JobRunState"] == "SUCCEEDED", good_run.get("ErrorMessage")
Issue-Key: glue.integration-resource-property-lifecycle|run#21|AssertionError|assert redelete_message == expected, f"expected message {expected!r}, got {redelete_message!r}"
Issue-Key: glue.integration-table-properties-lifecycle|run#15|AssertionError|assert delete_message == expected_message, (
Issue-Commit: 2f9e8ff46e4d80859bfa918f10daaf4eec221ac1

# Glue: 5 net-new root causes — CreateColumnStatisticsTaskSettings starts no task run, session statements and job runs never execute, and the two integration-property delete refusals use the connection-ARN message for a database ARN

> _Note: this issue was identified against the code base at commit `2f9e8ff46e4d80859bfa918f10daaf4eec221ac1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateColumnStatisticsTaskSettings

## Description
Five (5) net-new root causes in the Glue service. Each is independent and ALL FIVE must be closed before this issue is submitted. None of the five checks below was marked ALREADY FILED or OWNED by the replay report, so no open issue carries their replay keys; where an open issue describes a neighbouring fault it is named in the cause.

Model consulted via smithy_query on services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json.

=== Cause 1 of 5: CreateColumnStatisticsTaskSettings succeeds but starts no column-statistics task run at all ===
AWS: Smithy: CreateColumnStatisticsTaskSettings(DatabaseName, TableName, Role all @required; optional Schedule, ColumnNameList, SampleSize, CatalogID, SecurityConfiguration, Tags), errors AccessDeniedException, AlreadyExistsException, ColumnStatisticsTaskRunningException, EntityNotFoundException, InvalidInputException, OperationTimeoutException, ResourceNumberLimitExceededException. GetColumnStatisticsTaskRuns(DatabaseName, TableName @required) returns ColumnStatisticsTaskRuns. Recorded behavior: a Create with NO Schedule immediately starts a run, so GetColumnStatisticsTaskRuns for that (DatabaseName, TableName) lists exactly one run, carrying the Role submitted to Create and a Status still in flight (STARTING or RUNNING) when polled right after; that in-flight run is what makes a second Create for the same table refusable with ColumnStatisticsTaskRunningException ("Column statistics task for input table has already started") and stoppable with StopColumnStatisticsTaskRun (which drives it to STOPPED).
Emulator: Create returns 200 but GetColumnStatisticsTaskRuns stays EMPTY — the check polled it for 60 s (every 3 s) and never saw a run. Because no run exists, the whole in-flight state machine behind it is unreachable: the duplicate-Create refusal, the no-side-effect assertion on the run list, StopColumnStatisticsTaskRun, and the subsequent Create that must succeed once the run is STOPPED.
Why: a client that submits settings and then waits on GetColumnStatisticsTaskRuns for its statistics job waits forever and can never obtain a ColumnStatisticsTaskRunId; and because there is never an in-flight run, a second Create for the same table is accepted where AWS refuses it, so two conflicting Roles can be registered for one table.
Related but NOT covering: 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md and 20261027T060729Z-glue-a-column-statistics-task-run-auto-started.md both describe a run that IS auto-started and then settles FAILED with an emulator-authored ErrorMessage. On this commit the behaviour has moved: there is now no run to observe at all, which is a different cause in a different place (the auto-start, not the run's terminal state), and neither issue carries this check's replay key.
Fix: the CreateColumnStatisticsTaskSettings handler and the column-statistics state it writes — a Create without a Schedule must register the settings AND enqueue a task run for the table so GetColumnStatisticsTaskRuns lists it with the submitted Role and an in-flight Status.
Traces: glue.column-statistics-task-settings-duplicate-create-rejected.

=== Cause 2 of 5: a statement submitted to a READY session is never executed — it settles ERROR and is never observable RUNNING ===
AWS: Smithy: RunStatement(SessionId, Code @required) -> Id; errors include IllegalSessionStateException, InvalidInputException and SessionBusyException; GetStatement returns Statement.State from StatementState (WAITING|RUNNING|AVAILABLE|CANCELLING|CANCELLED|ERROR). Recorded behavior: code submitted to a READY session runs on the session's Spark runtime, so a long-running cell (`import time; time.sleep(45)`) is observably State=RUNNING for the duration, which is the precondition for the delete-while-running path (DeleteSession mid-statement must succeed and make both GetSession and GetStatement answer EntityNotFoundException).
Emulator: the statement settles State='ERROR' immediately (last observed state ERROR after the 60 s poll for RUNNING), so RUNNING is never produced and the mid-statement delete semantics cannot be exercised at all.
Why: an interactive session exists to run code; every cell errors and no client can observe, cancel or delete an in-flight statement.
Related but NOT covering: 20261029T230701Z-glue-interactive-sessions-5-net-new-root-causes.md describes this same family (its Cause 1) and even lists this check, but it carries the check's OLD replay key (a harness Timeout inside _wait_statement_state); the check now fails fast with AssertionError "statement 1 never reached {'RUNNING'}; last state=ERROR", so the report left it unowned. A fixer closing that issue must re-replay this check too.
Fix: statement execution/settling in the session state the RunStatement handler writes (the statement must become RUNNING while code is in flight, rather than being settled terminally at submission).
Traces: glue.delete-session-while-statement-running-accepted.

=== Cause 3 of 5: a Glue job run executes no script, so a role-permission outcome can never be observed ===
AWS: Smithy: StartJobRun(JobName @required, Arguments, ...) -> JobRunId; GetJobRun -> JobRun{JobRunState, ErrorMessage, ...}. Recorded behavior: a CloudFormation-deployed pythonshell job whose Role grants s3:PutObject runs its ScriptLocation script to completion — JobRunState settles SUCCEEDED, no ErrorMessage, and the object the script writes is readable from S3; the otherwise identical job whose role lacks exactly that grant settles FAILED with ErrorMessage of the form "ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<roleName>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: \"arn:aws:s3:::<bucket>/<key>\" because no identity-based policy allows the s3:PutObject action" (fixed text as quoted; account, role name, bucket and key come from the request/state), and no object is written (S3 answers NoSuchKey / "The specified key does not exist.").
Emulator: the positive-control run does not reach SUCCEEDED; GetJobRun reports the emulator-authored ErrorMessage "This emulator records job runs but executes no job script, so the job run ended without running." — the run is recorded and settled without the script ever executing, so neither the success effect (the written object) nor the AccessDenied failure can ever be produced.
Why: every client that uses Glue to transform data gets no side effects, and the IAM outcome of a job's role is unobservable — a permission defect and a correct grant are indistinguishable.
Related but NOT covering: 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md (and the several other open job-run issues) describe this fault for other checks; none carries this check's replay key, which is why the report left it unowned.
Fix: the job-run execution in this service's job-run state (the StartJobRun/GetJobRun state machine must run the job's Command.ScriptLocation under the job's Role and derive JobRunState/ErrorMessage from that execution, with authorization evaluated against the role).
Traces: glue.deploy-job-missing-output-permission-rejected.

=== Cause 4 of 5: DeleteIntegrationResourceProperty on a DATABASE ARN whose property is gone answers the connection-ARN message ===
AWS: Smithy: DeleteIntegrationResourceProperty(ResourceArn @required, String512 @length 1..512); modeled errors include EntityNotFoundException (client, httpError 400), InvalidInputException, ValidationException, ResourceNotFoundException. Recorded behavior (behavioral recording; the model does not fix the text): after Create -> Get -> Delete for ResourceArn = arn:aws:glue:<region>:<account>:database/<db>, a SECOND Delete on the same ARN is refused 400 EntityNotFoundException with message "The target resource <ResourceArn> is not found." — fixed text "The target resource " + " is not found.", the only variable part being the request's ResourceArn. The Get-after-delete on the same ARN already answers exactly that message and the emulator gets that one right; only the delete leg diverges.
Emulator: 400 EntityNotFoundException (class correct) with message "Failed to find the resource property with arn <ResourceArn> in the table".
Why: a client doing an idempotent delete (or matching the refusal to decide the property was already gone) sees text the service does not emit for this target kind, and the two legs of the same lifecycle disagree with each other.
Note for the fixer: the open WIRE issue 20261104T102303Z-glue-deleteintegrationresourceproperty-refusals-diverge-wrong-error-class-and.md records the opposite direction for a CONNECTION ARN — there AWS's message IS "Failed to find the resource property with arn <arn> in the table". The emulator now emits that connection-ARN template unconditionally. Hypothesis: AWS picks the message by the resolved ARN kind, so the fix is to key the not-found message on the target kind rather than replace one constant with another; a fix that satisfies only one of the two recordings re-breaks the other.
Fix: the DeleteIntegrationResourceProperty handler's not-found refusal message (per-target-kind message selection), not its error class.
Traces: glue.integration-resource-property-lifecycle.

=== Cause 5 of 5: DeleteIntegrationTableProperties on a DATABASE ARN + TableName whose properties are gone answers the sourceTableProperties message ===
AWS: Smithy: DeleteIntegrationTableProperties(ResourceArn @required String512, TableName @required String128 @length 1..128); modeled errors include EntityNotFoundException (client, 400), ValidationException, InvalidInputException, ResourceNotFoundException. Recorded behavior: after Create -> Get -> Delete for ResourceArn = arn:aws:glue:<region>:<account>:database/<db> with TableName = <table>, the repeat Delete is refused 400 EntityNotFoundException with message "The target resource <ResourceArn> is not found." — fixed text "The target resource " + " is not found." with the request's ResourceArn substituted (TableName does not appear).
Emulator: 400 EntityNotFoundException (class correct) with message "Failed to find the sourceTableProperties with sourceArn <ResourceArn> and tableName <TableName>".
Why: same as cause 4 — the repeat-delete refusal a client matches to confirm the properties were already removed carries wording the service does not emit for a database-ARN target.
Note for the fixer: the open WIRE issue 20261030T000205Z-glue-delete-path-refusals-carry-emulator-authored-messages.md records the sourceTableProperties wording as AWS's for a CONNECTION ARN. As in cause 4, the emulator now applies that one template to every target kind; the fix must select the message by resolved target kind so both recordings hold.
Fix: the DeleteIntegrationTableProperties handler's not-found refusal message (per-target-kind message selection).
Traces: glue.integration-table-properties-lifecycle.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator up; bootstrap_signal false; 272/611 passed; glue 194/321).

Cause 1 — glue.column-statistics-task-settings-duplicate-create-rejected [mismatch]:
  AssertionError: expected GetColumnStatisticsTaskRuns to show a run after a successful Create, got none
  (the check polls get_column_statistics_task_runs(DatabaseName, TableName) every 3 s for 60 s after a successful create_column_statistics_task_settings(..., Role=<roleA>); AWS lists one run with Role == <roleA> and Status in (STARTING, RUNNING). Everything downstream — the ColumnStatisticsTaskRunningException duplicate-Create refusal "Column statistics task for input table has already started", the one-run no-side-effect assertion, StopColumnStatisticsTaskRun -> STOPPED, and the re-Create with <roleB> — is unreachable.)

Cause 2 — glue.delete-session-while-statement-running-accepted [mismatch]:
  AssertionError: statement 1 never reached {'RUNNING'}; last state=ERROR
  (run_statement(SessionId=<id>, Code='import time\ntime.sleep(45)') on a READY session; AWS shows State=RUNNING, so DeleteSession mid-statement can be exercised and GetSession/GetStatement then answer EntityNotFoundException.)

Cause 3 — glue.deploy-job-missing-output-permission-rejected [mismatch]:
  AssertionError: This emulator records job runs but executes no job script, so the job run ended without running.
  (raised by `assert good_run["JobRunState"] == "SUCCEEDED", good_run.get("ErrorMessage")`; the quoted string is the emulator's own JobRun.ErrorMessage. AWS: the granted job's run is SUCCEEDED with the script's object readable at s3://<bucket>/results/good.txt; the ungranted job's run is FAILED with the AccessDenied ClientError message quoted in cause 3 and no object written.)

Cause 4 — glue.integration-resource-property-lifecycle [mismatch]:
  expected message 'The target resource arn:aws:glue:<region>:<account>:database/<db> is not found.'
  got      'Failed to find the resource property with arn arn:aws:glue:<region>:<account>:database/<db> in the table'
  (second delete_integration_resource_property on the same ResourceArn; code EntityNotFoundException matched on both sides, and the preceding get-after-delete message matched.)

Cause 5 — glue.integration-table-properties-lifecycle [mismatch]:
  expected repeat Delete message 'The target resource arn:aws:glue:<region>:<account>:database/<db> is not found.'
  got      'Failed to find the sourceTableProperties with sourceArn arn:aws:glue:<region>:<account>:database/<db> and tableName <table>'
  (repeat delete_integration_table_properties(ResourceArn, TableName); code EntityNotFoundException matched.)

In causes 4 and 5 the fixed text is "The target resource " + " is not found."; the ARN (and the region, account id, database and table names inside it) are the request's values from that run and are not part of the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run these five checks: glue.column-statistics-task-settings-duplicate-create-rejected, glue.delete-session-while-statement-running-accepted, glue.deploy-job-missing-output-permission-rejected, glue.integration-resource-property-lifecycle, glue.integration-table-properties-lifecycle. All five must pass before submitting.
3. Cause 1: create a bucket with a small CSV, an IAM role trusted by glue.amazonaws.com with read access to it, a Glue database and an EXTERNAL_TABLE over that prefix; call glue.create_column_statistics_task_settings(DatabaseName, TableName, Role=<roleA>) with NO Schedule; then poll glue.get_column_statistics_task_runs(DatabaseName, TableName) — it must list one run with Role <roleA> and Status STARTING/RUNNING. Then a second create_column_statistics_task_settings(..., Role=<roleB>) must be refused ColumnStatisticsTaskRunningException "Column statistics task for input table has already started" while that run is in flight, stop_column_statistics_task_run must drive it to STOPPED, and the identical Create must then succeed.
4. Cause 2: glue.create_session(Id=<id>, Role=<glue role>, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='5.0'); wait for READY; glue.run_statement(SessionId=<id>, Code='import time\ntime.sleep(45)'); poll glue.get_statement — State must be RUNNING; then glue.delete_session(Id=<id>) must succeed and both get_session and get_statement must answer EntityNotFoundException.
5. Cause 3: deploy the two-job CloudFormation stack from the check (identical AWS::Glue::Job pythonshell jobs differing only in whether their role grants s3:PutObject on the output prefix) with a script that does s3.put_object(Bucket=--bucket, Key=--output_key, Body=b'ok'); start both runs and poll get_job_run: the granted run must be SUCCEEDED with the object readable, the ungranted run FAILED with the AccessDenied ErrorMessage and no object written, and granting the missing statement must make the same job succeed.
6. Cause 4: glue.create_database; glue.create_integration_resource_property(ResourceArn='arn:aws:glue:<region>:<account>:database/<db>', TargetProcessingProperties={'RoleArn': <role>}); get; delete; then delete AGAIN and compare Error.Code/Error.Message with 'EntityNotFoundException' / 'The target resource <ResourceArn> is not found.'.
7. Cause 5: with a database and a table, glue.create_integration_table_properties(ResourceArn='arn:aws:glue:<region>:<account>:database/<db>', TableName=<table>, TargetTableConfig={...}); get; delete; delete AGAIN and compare against 'EntityNotFoundException' / 'The target resource <ResourceArn> is not found.' (the get-after-delete must carry the same message).
8. When fixing causes 4 and 5, keep the connection-ARN recordings in 20261104T102303Z-glue-deleteintegrationresourceproperty-refusals-diverge-wrong-error-class-and.md and 20261030T000205Z-glue-delete-path-refusals-carry-emulator-authored-messages.md passing: the message must be chosen by the resolved target kind.
