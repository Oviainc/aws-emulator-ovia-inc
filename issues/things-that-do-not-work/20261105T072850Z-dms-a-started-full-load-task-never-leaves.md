Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dms.s3-full-load-migration
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms.s3-full-load-migration|_await#47|Timeout|time.sleep(poll_s)
Issue-Commit: deff4a6e6d31627acea4018c09886a861433b132

# DMS: a started full-load task never leaves 'running' — no FULL_LOAD_ONLY_FINISHED stop, no ReplicationTaskStats, no data moved, and DescribeTableStatistics is still a 501 stub

> _Note: this issue was identified against the code base at commit `deff4a6e6d31627acea4018c09886a861433b132`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplicationTask

## Description
Two net-new root causes in the DMS replication-task execution path, both grounded in the behavioral check dms.s3-full-load-migration (which now gets past the EC2 VPC-endpoint setup that open issue 20261104T113449Z-dms-the-s3-full-load-migration-scenario-cannot describes, and stalls further downstream, inside DMS itself). The other failing dms check in this run (dms.subnet-group-requires-two-azs) is OWNED by 20261031T070440Z and is not part of this issue.

Cause 1 of 2: StartReplicationTask moves a task to 'starting' and then only ever to 'running'; the task never completes, so a full-load-only migration never terminates. Failing check ids: dms.s3-full-load-migration.
AWS: Smithy com.amazonaws.databasemigrationservice#ReplicationTask$Status documents the task lifecycle values, including "stopped" ('the task is stopped'), and ReplicationTask$StopReason documents "Stop Reason FULL_LOAD_ONLY_FINISHED" - 'The task completed the full load phase'. A task created with MigrationType 'full-load' therefore runs the full load, STOPS by itself, and reports StopReason exactly "Stop Reason FULL_LOAD_ONLY_FINISHED" (fixed text, no variable part). ReplicationTask$ReplicationTaskStats (shape ReplicationTaskStats) then carries FullLoadProgressPercent (100 for a finished load), TablesLoaded (the number of tables the table mappings selected, 1 here), TablesErrored (0), TablesLoading/TablesQueued (0), plus FullLoadStartDate/FullLoadFinishDate/StopDate. For an S3 source to an S3 target, the data plane is AWS's too: the rows read from <BucketFolder>/<schema>/<table>/LOAD*.csv at the source are written to the target endpoint's <BucketFolder>/<schema>/<table>/ prefix as a single full-load object named LOAD00000001.csv whose bytes are the migrated rows.
Emulator: StartReplicationTask sets status='starting' and queues exactly one owed transition, Owed::ToStatus(TASK_RUNNING); the clock-settle pass materialises that and nothing further. The task stays 'running' forever: it never reaches 'stopped', StopReason stays unset (StartReplicationTask explicitly clears it), ReplicationTaskStats is never produced with a finished full load, and nothing is ever read from the source bucket or written to the target bucket. The check polls DescribeReplicationTasks for a terminal status and is cut off by the harness - the replayed verdict is 'Check timed out after 60.0s' with no AWS call having failed, i.e. the state machine simply never arrives.
Why: a client that starts a full-load task and waits for it to finish (the documented pattern: poll Status, then read StopReason and ReplicationTaskStats) waits forever against this emulator, and no migrated data ever appears at the target. Every assertion about a completed migration - the FULL_LOAD_ONLY_FINISHED stop reason, FullLoadProgressPercent==100, TablesLoaded/TablesErrored, and the single LOAD00000001.csv landing under the target prefix with the source bytes - is unreachable, so the whole DMS data plane is unusable and ungradeable.
Fix: the StartReplicationTask handler together with the owed-transition machinery in the service's state module (the Pending/Owed/settle slice): a started full-load task must be owed a run that terminates - advance 'starting' -> 'running' -> 'stopped' with stop_reason = "Stop Reason FULL_LOAD_ONLY_FINISHED" for MigrationType 'full-load', and populate the task's ReplicationTaskStats (FullLoadProgressPercent 100, TablesLoaded = tables selected by the task's TableMappings, TablesErrored 0, the full-load start/finish/stop timestamps) as part of that completion. The S3-to-S3 full-load data movement belongs in the same task-execution path, writing the migrated object under the target endpoint's bucket/folder as LOAD00000001.csv.
Traces: dms.s3-full-load-migration. Code anchors: services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/start_replication_task.rs lines 219-239 (only Owed::ToStatus(TASK_RUNNING) is queued; stop_reason cleared at line 223) and services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/state.rs lines ~955-992 (settle only applies Owed::ToStatus / Owed::Removal; no completion transition, no stats, no stop reason exists anywhere in the crate).

Cause 2 of 2: DescribeTableStatistics is an unimplemented scaffold that answers HTTP 501 NotImplemented, so even once a task completes its per-table accounting cannot be read. Failing check ids: dms.s3-full-load-migration.
AWS: Smithy com.amazonaws.databasemigrationservice#DescribeTableStatistics returns TableStatistics entries for the task's tables; TableStatistics documents SchemaName, TableName, FullLoadRows ('The number of rows added during the full load operation'), FullLoadErrorRows, FullLoadStartTime/FullLoadEndTime and TableState, whose documented valid states include 'Table completed'. After a finished full load AWS answers HTTP 200 with one entry per migrated table, naming the schema and table from the task's table mappings, FullLoadRows equal to the rows loaded, FullLoadErrorRows 0 and TableState 'Table completed'. The operation models only ResourceNotFoundFault and InvalidResourceStateFault - there is no error class under which a 501 is permissible.
Emulator: the handler file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns HTTP 501 with error code NotImplemented, so the request is refused outright. (The check times out at Cause 1 before issuing this call, so this cause is grounded in the handler's own scaffold marker plus the check's final assertions, which cannot pass while it stands.)
Why: a client reconciling what the task reports against what landed at the target - the standard way to validate a migration - gets a 5xx that no SDK models for this operation, instead of per-table statistics.
Fix: the DescribeTableStatistics handler - project the per-table full-load accounting the task-execution path records (schema, table, FullLoadRows, FullLoadErrorRows, TableState) out of the replication-task state, with the operation's documented filtering/paging, and drop the scaffold marker.
Traces: dms.s3-full-load-migration. Code anchor: services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/describe_table_statistics.rs lines 3-6.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator serving (dms 1/3 checks passed; the emulator answers DMS calls, so this is a fidelity divergence, not a bootstrap gap).

[mismatch] dms.s3-full-load-migration:
  Error: Check timed out after 60.0s

Where it stalls: the check creates the VPC/subnets/S3 gateway endpoint, the replication subnet group, the replication instance (reaches 'available'), both s3 endpoints and the full-load task (reaches 'ready'), calls StartReplicationTask(StartReplicationTaskType='start-replication'), and then polls DescribeReplicationTasks for a terminal status. No AWS call fails; the task status never becomes 'stopped', so the poll loop runs until the harness cuts the check off.

Expected (AWS), for a MigrationType='full-load' task over an S3 source and S3 target:
  - DescribeReplicationTasks eventually reports Status 'stopped'
  - StopReason == 'Stop Reason FULL_LOAD_ONLY_FINISHED' (fixed text; no variable part)
  - ReplicationTaskStats.FullLoadProgressPercent == 100, TablesLoaded == <tables selected by TableMappings>, TablesErrored == 0
  - the target bucket holds exactly one object under <TargetBucketFolder>/<schema>/<table>/, named LOAD00000001.csv, whose body is the source rows
  - DescribeTableStatistics -> HTTP 200 with one TableStatistics entry per migrated table: SchemaName/TableName from the task's mappings, FullLoadRows == rows loaded, FullLoadErrorRows == 0, TableState == 'Table completed'
Actual (emulator):
  - Status stays 'running' indefinitely (StartReplicationTask queues only the 'starting' -> 'running' transition)
  - StopReason remains absent (it is cleared on start and never set)
  - ReplicationTaskStats never reports a finished full load
  - nothing is read from the source bucket or written to the target bucket
  - DescribeTableStatistics answers HTTP 501 / NotImplemented (handler still marked 'SCAFFOLDED, NOT IMPLEMENTED')

Model consulted: smithy_query query=shape shape=ReplicationTask,ReplicationTaskStats,TableStatistics on services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json - ReplicationTask$Status includes 'stopped'; ReplicationTask$StopReason documents 'Stop Reason FULL_LOAD_ONLY_FINISHED'; TableStatistics$TableState documents 'Table completed'.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the behavioral suite simparity-behavioral-aws-v1, check dms.s3-full-load-migration.
3. Shorter repro against DMS alone: create a replication subnet group over two subnets in two AZs, CreateReplicationInstance and wait for 'available', CreateEndpoint twice (EndpointType 'source' and 'target', EngineName 's3'), CreateReplicationTask with MigrationType='full-load' and a selection rule for one table, wait for Status 'ready', then StartReplicationTask(StartReplicationTaskType='start-replication').
4. Poll DescribeReplicationTasks: the task reaches 'running' and stays there. It never reaches 'stopped', StopReason is never set to 'Stop Reason FULL_LOAD_ONLY_FINISHED', and ReplicationTaskStats never reports FullLoadProgressPercent 100 / TablesLoaded 1 / TablesErrored 0.
5. List the target endpoint's bucket under <BucketFolder>/<schema>/<table>/: it is empty, where AWS has written LOAD00000001.csv containing the source rows.
6. Call DescribeTableStatistics(ReplicationTaskArn=<task arn>): HTTP 501 with error code NotImplemented, where AWS answers HTTP 200 with one TableStatistics entry (TableState 'Table completed', FullLoadRows == rows loaded, FullLoadErrorRows 0).
