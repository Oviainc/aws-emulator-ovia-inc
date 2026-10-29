Issue-Source: trace-sweep-divergence
Issue-Trace: rds-blue-green-deployment-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-blue-green-deployment-errors rds-describe-blue-green-deployments-identifier-not-found rds-db-shard-group-error-paths rds-db-shard-group-lifecycle rds-db-snapshot-and-export-task-error-paths rds-export-task-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DeleteBlueGreenDeployment|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DescribeBlueGreenDeployments|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBShardGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBShardGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteDBShardGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|RebootDBShardGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CancelExportTask|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS not-found refusals for shard groups, export tasks and blue/green deployments use emulator wording

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBlueGreenDeployment

## Description
Seven not-found refusals answer the correct 404 and error code but replace AWS's sentence with a generic '<Type> <id> not found.' template the emulator generates centrally. 7 causes.

Cause 1 of 7: DeleteBlueGreenDeployment on a missing deployment.
AWS: recorded behavior only — 404, Message "Blue Green Deployment '<BlueGreenDeploymentIdentifier>' doesn't exist." Fixed text: "Blue Green Deployment '" + "' doesn't exist."; variable part is the request identifier, single-quoted.
Emulator: 404 with 'BlueGreenDeployment <id> not found.' — unquoted, concatenated type name, different sentence.
Why: callers matching the documented sentence do not recognise the missing-deployment case.
Fix: the DeleteBlueGreenDeployment handler's existence check message.
Traces: rds-blue-green-deployment-errors#task#3.

Cause 2 of 7: DescribeBlueGreenDeployments filtered on a missing deployment.
AWS: recorded behavior only — 404, same sentence as cause 1.
Emulator: 'BlueGreenDeployment <id> not found.'
Why: same.
Fix: the DescribeBlueGreenDeployments handler's existence check message.
Traces: rds-describe-blue-green-deployments-identifier-not-found#task#2.

Cause 3 of 7: CreateDBShardGroup naming a cluster that does not exist.
AWS: Smithy CreateDBShardGroup errors DBClusterNotFoundFault, awsQueryError DBClusterNotFoundFault, httpError 404. Recorded Message: "The DB cluster doesn't exist." — entirely fixed text, with NO identifier.
Emulator: 404 with 'DBCluster <DBClusterIdentifier> not found.' — interpolates an identifier AWS omits.
Why: the emulator leaks the request identifier into a constant message.
Fix: the CreateDBShardGroup handler's cluster-existence check.
Traces: rds-db-shard-group-error-paths#task#1; also rds-db-shard-group-lifecycle#task#2.

Cause 4 of 7: ModifyDBShardGroup on a missing shard group.
AWS: recorded behavior only — 404, Message "The DB shard group with requested identifier in requested account doesn't exist." — fixed text, no identifier.
Emulator: 'DBShardGroup <id> not found.'
Why: same.
Fix: the ModifyDBShardGroup handler's existence check.
Traces: rds-db-shard-group-error-paths#task#3; also rds-db-shard-group-lifecycle#task#3.

Cause 5 of 7: DeleteDBShardGroup on a missing shard group.
AWS: recorded behavior only — 404, Message 'DB shard group not found or can not be accessed' — fixed text, no identifier, no terminal period.
Emulator: 'DBShardGroup <id> not found.'
Why: same.
Fix: the DeleteDBShardGroup handler's existence check.
Traces: rds-db-shard-group-error-paths#task#4; also rds-db-shard-group-lifecycle#task#4.

Cause 6 of 7: RebootDBShardGroup on a missing shard group.
AWS: recorded behavior only — 404, same fixed sentence as cause 5.
Emulator: 'DBShardGroup <id> not found.'
Why: same.
Fix: the RebootDBShardGroup handler's existence check.
Traces: rds-db-shard-group-error-paths#task#5; also rds-db-shard-group-lifecycle#task#5.

Cause 7 of 7: CancelExportTask on a missing export task.
AWS: recorded behavior only — 404, Message "The export task <ExportTaskIdentifier> doesn't exist." Fixed text: "The export task " + " doesn't exist."; variable part is the request identifier, unquoted.
Emulator: 'ExportTask <id> not found.'
Why: same message-matching break.
Fix: the CancelExportTask handler's existence check.
Traces: rds-db-snapshot-and-export-task-error-paths#task#11; also rds-export-task-lifecycle#task#4, #task#5.

## Evidence
Replay cluster details, expected vs actual $.ErrorResponse.Error.Message (all status matched; only the Message diverged):
1. rds-blue-green-deployment-errors#task#3 DeleteBlueGreenDeployment 404: expected "Blue Green Deployment 'bgd-000000000000000' doesn't exist.", got 'BlueGreenDeployment bgd-000000000000000 not found.'
2. rds-describe-blue-green-deployments-identifier-not-found#task#2 DescribeBlueGreenDeployments 404: expected "Blue Green Deployment 'bgd-sim2real-5b09be76' doesn't exist.", got 'BlueGreenDeployment bgd-sim2real-5b09be76 not found.'
3. rds-db-shard-group-error-paths#task#1 CreateDBShardGroup 404: expected "The DB cluster doesn't exist.", got 'DBCluster sim2real-probe-rds-rdsnoexistcluster-5a55343a not found.'
4. rds-db-shard-group-error-paths#task#3 ModifyDBShardGroup 404: expected "The DB shard group with requested identifier in requested account doesn't exist.", got 'DBShardGroup sim2real-probe-rds-rdsshardgroup-5a55343a not found.'
5. rds-db-shard-group-error-paths#task#4 DeleteDBShardGroup 404: expected 'DB shard group not found or can not be accessed', got 'DBShardGroup ... not found.'
6. rds-db-shard-group-error-paths#task#5 RebootDBShardGroup 404: expected 'DB shard group not found or can not be accessed', got 'DBShardGroup ... not found.'
7. rds-db-snapshot-and-export-task-error-paths#task#11 CancelExportTask 404: expected "The export task sim2real-probe-rds-missing-export-task-263e26a6 doesn't exist.", got 'ExportTask ... not found.'
(The identifiers shown are this run's generated names.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run the representative step rds-blue-green-deployment-errors#task#3 and the steps in trace_ids; each calls the operation with an identifier that was never created and compares $.ErrorResponse.Error.Message.
