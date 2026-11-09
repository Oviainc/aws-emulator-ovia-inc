Issue-Source: trace-sweep-divergence
Issue-Trace: dms-s3-full-load-task-migrates-csv-rows
Issue-Step: task#21
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dms-s3-full-load-task-migrates-csv-rows
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|StartReplicationTask|2xx->2xx|$.ReplicationTask.ReplicationTaskCreationDate
Issue-Key: dms|StartReplicationTask|2xx->2xx|$.ReplicationTask.ReplicationTaskSettings
Issue-Key: dms|StartReplicationTask|2xx->2xx|$.ReplicationTask.ReplicationTaskStartDate
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].ReplicationTaskCreationDate
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].ReplicationTaskSettings
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].ReplicationTaskStartDate
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].ReplicationTaskStats
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].Status
Issue-Key: dms|DescribeReplicationTasks|2xx->2xx|$.ReplicationTasks[*].StopReason
Issue-Key: dms|DeleteReplicationTask|2xx->4xx|status
Issue-Key: dms|DescribeReplicationTasks|4xx->2xx|$
Issue-Key: dms|DescribeReplicationTasks|4xx->2xx|status
Issue-Key: dms|DescribeReplicationInstances|4xx->2xx|$
Issue-Key: dms|DescribeReplicationInstances|4xx->2xx|status
Issue-Commit: 2e3afc6c699aa7e5b0dfd114974fd7ed6d393583

# DMS replication-task lifecycle never completes: task settings are never defaulted, a full-load task is stuck 'running', its delete is refused, and deleted resources are still described

> _Note: this issue was identified against the code base at commit `2e3afc6c699aa7e5b0dfd114974fd7ed6d393583`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplicationTask

## Description
Five net-new wire divergences in the DMS replication-task / replication-instance lifecycle, all from fixture dms-s3-full-load-task-migrates-csv-rows (12 fixtures replayed, emulator serving: /__admin/health 200 and the data listener accepts, so these are fidelity divergences, not a bootstrap gap). The two s3 divergences in the same fixture (ListObjectsV2 task#24, GetObject task#25) are OWNED by open issues 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md and 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md and are not part of this issue. Related open issue: 20261105T072850Z-dms-a-started-full-load-task-never-leaves (behavioral corpus) describes the same non-terminating task from the behavioral side; it does NOT carry these wire replay keys, so Causes 2-4 are filed here with their fixture steps, and Cause 1 and Cause 5 are not covered by it at all.

Cause 1 of 5: a replication task created without (or with a partial) ReplicationTaskSettings is never given DMS's default task-settings document, so every response that carries the task omits ReplicationTaskSettings entirely. Fixture dms-s3-full-load-task-migrates-csv-rows step task#21 (StartReplicationTask).
AWS: Smithy com.amazonaws.databasemigrationservice#ReplicationTask$ReplicationTaskSettings (String) - 'The settings for the replication task'; CreateReplicationTask$ReplicationTaskSettings is OPTIONAL. DMS resolves the omitted/partial setting against its service defaults at create time and thereafter reports the fully populated settings JSON on every ReplicationTask it returns (CreateReplicationTask, StartReplicationTask, DescribeReplicationTasks without WithoutSettings=true, DeleteReplicationTask). The recorded response shows the shape of that document: a JSON object whose top-level keys include Logging (EnableLogging/EnableLogContext false, CloudWatchLogGroup/CloudWatchLogStream null, and one LogComponents entry per DMS log component - DATA_STRUCTURE, COMMUNICATION, IO, COMMON, FILE_FACTORY, FILE_TRANSFER, REST_SERVER, ADDONS, TARGET_LOAD, TARGET_APPLY, SOURCE_UNLOAD, SOURCE_CAPTURE, TRANSFORMATION, SORTER, TASK_MANAGER, TABLES_MANAGER, METADATA_MANAGER, PERFORMANCE, VALIDATOR_EXT - each at Severity LOGGER_SEVERITY_DEFAULT), StreamBufferSettings, ErrorBehavior and the remaining default sections. The individual numbers in one recording are the service defaults, not per-run values.
Emulator: ReplicationTaskSettings is stored as exactly what the caller passed (services/.../src/ops/create_replication_task.rs line 321, `replication_task_settings: input.replication_task_settings`, an Option<String>) and projected straight back out (src/state.rs lines 931-943), so when the caller omits it the member is absent from the response. Replay reports `$.ReplicationTask.ReplicationTaskSettings: missing`. On the same step the harness also flags `$.ReplicationTask.ReplicationTaskCreationDate` and `$.ReplicationTask.ReplicationTaskStartDate` (expected '<shaped-not-asserted>', got the bare epoch float 1794240021.8734615 from `DateTime::from(SystemTime)`); the recording's values are redacted to a placeholder, so this is recorded behavior only - Hypothesis: the emulator's TStamp serialization for these two members does not match the serialization the recording carries. The absent settings document is the unambiguous part of this cluster.
Why: a client that reads back a task's effective settings - the documented way to discover what logging, error behavior and stream buffering the task will actually run with, and the basis for a ModifyReplicationTask round-trip - gets no settings member at all, so any JSON parse of ReplicationTaskSettings fails or silently yields nothing, and a settings round-trip wipes the defaults.
Fix: the CreateReplicationTask handler's task-record construction (resolve the caller's omitted/partial ReplicationTaskSettings against a default task-settings document held in the service's state module, and store the resolved document), so the single ReplicationTaskRecord::to_shape projection reports it on every operation that returns a ReplicationTask.
Traces: dms-s3-full-load-task-migrates-csv-rows#task#21. Code anchors: services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_task.rs line 321; services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/state.rs lines 900-901 and 931-943.

Cause 2 of 5: a started MigrationType='full-load' task never reaches a terminal status - DescribeReplicationTasks reports Status 'running' forever, with no StopReason and no ReplicationTaskStats - so a poll for completion never terminates. Fixture step task#22 (DescribeReplicationTasks).
AWS: Smithy ReplicationTask$Status documents the lifecycle values including 'stopped', and ReplicationTask$StopReason documents 'Stop Reason FULL_LOAD_ONLY_FINISHED' - 'The task completed the full load phase'. A full-load-only task therefore runs its load, stops by itself, and reports Status 'stopped' with StopReason exactly the fixed text 'Stop Reason FULL_LOAD_ONLY_FINISHED' (no variable part), plus ReplicationTask$ReplicationTaskStats describing the finished load (FullLoadProgressPercent 100, TablesLoaded = the tables the task's TableMappings selected, TablesErrored 0, and the full-load start/finish/stop timestamps). The recorded step polls DescribeReplicationTasks and gets that terminal task, settings document included.
Emulator: StartReplicationTask sets status 'starting' and queues exactly one owed transition, Owed::ToStatus(TASK_RUNNING) (src/ops/start_replication_task.rs lines 219-239, with stop_reason cleared at line 223); state.settle() only ever applies Owed::ToStatus / Owed::Removal (src/state.rs lines 1002-1040) and has no completion transition, and ReplicationTaskRecord::to_shape hard-codes `replication_task_stats: None` (src/state.rs lines 960-962). Replay: the step's poll is capped at 180s, and the task it keeps reading is {Status: 'running', no StopReason, no ReplicationTaskStats, no ReplicationTaskSettings}.
Why: the documented completion pattern - poll Status until terminal, then read StopReason and ReplicationTaskStats - never returns against this emulator, so every client that waits for a migration to finish hangs, and no accounting of the load is ever readable.
Fix: the StartReplicationTask handler together with the owed-transition machinery in the service's state module: a started full-load task must be owed a run that TERMINATES - 'starting' -> 'running' -> 'stopped' with stop_reason = 'Stop Reason FULL_LOAD_ONLY_FINISHED' for MigrationType 'full-load' - and the completion must populate the record's replication task stats so to_shape stops reporting None.
Traces: dms-s3-full-load-task-migrates-csv-rows#task#22. Code anchors: src/ops/start_replication_task.rs lines 219-239; src/state.rs lines 960-962 and 1002-1040.

Cause 3 of 5: because the task is permanently 'running', DeleteReplicationTask refuses it with HTTP 400 InvalidResourceStateFault - a full-load task can never be deleted through its documented lifecycle. Fixture step teardown#26 (DeleteReplicationTask).
AWS: Smithy DeleteReplicationTask returns HTTP 200 with the deleted ReplicationTask (whose Status becomes 'deleting', per ReplicationTask$Status: "'deleting' - The task is being deleted in response to running the DeleteReplicationTask operation"); it models InvalidResourceStateFault (HTTP 400) only for a task that is in a state that prevents the delete - i.e. one still running. In the recording the full-load task has already stopped itself by teardown, so the delete is accepted with 200.
Emulator: the handler's UNDELETABLE_STATUSES = [starting, running, deleting] (src/ops/delete_replication_task.rs lines 162-167, 185-189) refuses with 400 InvalidResourceStateFault and the emulator-authored message 'replication task <arn> is <status> and cannot be deleted in that status'. That refusal rule itself matches AWS; it is unreachable-to-escape here because Cause 2 leaves the task 'running' forever. Replay: `status: expected 200, got 400`.
Why: there is no sequence of calls by which a client can delete a full-load task it started - teardown of any DMS scenario is impossible, tasks accumulate until CreateReplicationTask starts answering ResourceQuotaExceededFault, and the client sees an InvalidResourceStateFault for a task AWS considers finished.
Fix: fixing Cause 2 (the task-execution path in the StartReplicationTask handler plus the state module's settle) is what makes this step pass; the DeleteReplicationTask handler's status gate should NOT be loosened, since refusing a genuinely running task is AWS's behavior. Re-replay this step after the completion transition exists.
Traces: dms-s3-full-load-task-migrates-csv-rows#teardown#26. Code anchor: src/ops/delete_replication_task.rs lines 162-167 and 185-189.

Cause 4 of 5: after the delete, DescribeReplicationTasks still reports the task with HTTP 200 where AWS answers HTTP 400 ResourceNotFoundFault for a filter naming a task that no longer exists. Fixture step teardown#27 (DescribeReplicationTasks).
AWS: Smithy DescribeReplicationTasks models exactly one error, ResourceNotFoundFault (HTTP 400, 'The resource could not be found'). A describe filtered on a replication-task ARN/identifier that has been deleted and reclaimed is refused with that error code; the recorded step polls until the error code is ResourceNotFoundFault and treats HTTP 400 as the expected outcome. The message is DMS's own text for a resource that is gone, naming the filter value from the request - not a value from any one run.
Emulator: the step answers HTTP 200 with ReplicationTasks = [the same task, Status 'running'] on all 37 poll attempts until the 180s cap; the delete that should have removed it was refused (Cause 3), so the record is never put into 'deleting' and never reclaimed by settle's Owed::Removal branch. Replay: `status: expected 400, got 200` and `poll: error code None not in ['ResourceNotFoundFault'] after 37 attempt(s)`.
Why: a client polling for its teardown to finish - the documented way to confirm a task is gone - never observes the task leave, so teardown loops forever and a freed identifier is never reusable.
Fix: the task-execution/removal path (Cause 2 + the DeleteReplicationTask -> Owed::Removal reclamation in the state module's settle); additionally the DescribeReplicationTasks handler must answer ResourceNotFoundFault (HTTP 400) when a request's Filters name a replication task the store does not hold, rather than an empty/stale 200.
Traces: dms-s3-full-load-task-migrates-csv-rows#teardown#27. Code anchors: src/ops/delete_replication_task.rs lines 169-176; src/state.rs lines 1014-1028; src/ops/describe_replication_tasks.rs (filter handling).

Cause 5 of 5: a deleted replication instance is never removed from the store, so DescribeReplicationInstances keeps answering HTTP 200 with the instance where AWS answers HTTP 400 ResourceNotFoundFault. Fixture step teardown#31 (DescribeReplicationInstances).
AWS: Smithy DescribeReplicationInstances models exactly one error, ResourceNotFoundFault (HTTP 400). After DeleteReplicationInstance the instance goes to 'deleting' and is then reclaimed; a describe filtered on that instance ARN/identifier is thereafter refused with ResourceNotFoundFault and HTTP 400, which is what the recorded step polls for.
Emulator: the step answers HTTP 200 with ReplicationInstances = [the instance, ReplicationInstanceStatus 'available'] on all 10 attempts until the poll cap. Two emulator defects stand behind it: DeleteReplicationInstance is still an unimplemented scaffold that answers HTTP 501 NotImplemented (src/ops/delete_replication_instance.rs lines 137-155 - that operation's own stub issue is filed by the replay harness and is NOT this issue), AND state.settle() handles only Owed::ToStatus for replication instances, with no Owed::Removal branch for them at all (src/state.rs lines 1003-1012, versus the task branch at lines 1014-1028) - so even once the delete is written, nothing would ever reclaim the instance. Replay: `status: expected 400, got 200` and `poll: error code None not in ['ResourceNotFoundFault'] after 10 attempt(s)`.
Why: a client cannot confirm - or achieve - instance teardown: the instance it deleted stays 'available' forever, so billing-shaped teardown checks, identifier reuse and any 'wait until deleted' loop never complete.
Fix: the replication-instance removal path in the service's state module (give settle an Owed::Removal branch for replication_instances that drops the record and frees instance_arn_by_identifier, mirroring the task branch), plus the DescribeReplicationInstances handler answering ResourceNotFoundFault (HTTP 400) when the request's Filters name an instance the store does not hold.
Traces: dms-s3-full-load-task-migrates-csv-rows#teardown#31. Code anchors: src/state.rs lines 1003-1012 and 1014-1028; src/ops/delete_replication_instance.rs lines 137-155.

## Evidence
Replay of trace set simparity-wire-aws-v1 (12 fixtures, 156 steps, 57 passed, 97 mismatched, 2 blocked, score 36.5%). emulator_up=true, bootstrap_signal=false, emulator_detail: '/__admin/health answered 200 and the :40607 data listener accepts'. All five clusters below come from fixture dms-s3-full-load-task-migrates-csv-rows, service 'dms', and none carries stub_path or owned_by.

[mismatch] task#21 StartReplicationTask - expected 200, got 200, body diverges:
  $.ReplicationTask.ReplicationTaskSettings: missing. Expected: the full DMS default task-settings JSON document - {"Logging":{"EnableLogging":false,"EnableLogContext":false,"LogComponents":[{"Severity":"LOGGER_SEVERITY_DEFAULT","Id":"DATA_STRUCTURE"}, ... one entry per DMS log component ...],"CloudWatchLogGroup":null,"CloudWatchLogStream":null},"StreamBufferSettings":{...},"ErrorBehavior":{...}, ...}. Actual: member absent.
  $.ReplicationTask.ReplicationTaskCreationDate: value_mismatch (expected '<shaped-not-asserted>', got 1794240021.8734615); $.ReplicationTask.ReplicationTaskStartDate likewise flagged.
  divergence_key: service=dms|StartReplicationTask|status=200->200|keys=$.ReplicationTask.ReplicationTaskCreationDate,$.ReplicationTask.ReplicationTaskSettings,$.ReplicationTask.ReplicationTaskStartDate

[mismatch] task#22 DescribeReplicationTasks - expected 200, got 200; the step's poll was capped at 180s.
  Expected (recording): ReplicationTasks[0] with MigrationType 'full-load', the default ReplicationTaskSettings document, a terminal Status, StopReason and ReplicationTaskStats.
  Actual (emulator): ReplicationTasks[0] Status 'running', no StopReason, no ReplicationTaskStats, no ReplicationTaskSettings.
  divergence_key: service=dms|DescribeReplicationTasks|status=200->200|keys=$,$.ReplicationTasks[0].ReplicationTaskCreationDate,$.ReplicationTasks[0].ReplicationTaskSettings,$.ReplicationTasks[0].ReplicationTaskStartDate,$.ReplicationTasks[0].ReplicationTaskStats,$.ReplicationTasks[0].Status,$.ReplicationTasks[0].StopReason

[mismatch] teardown#26 DeleteReplicationTask - status: expected 200, got 400 (InvalidResourceStateFault; handler message 'replication task <arn> is running and cannot be deleted in that status').
  divergence_key: service=dms|DeleteReplicationTask|status=200->400|keys=status

[mismatch] teardown#27 DescribeReplicationTasks - status: expected 400, got 200.
  Expected (recording): HTTP 400 with error code ResourceNotFoundFault once the task is gone.
  Actual: HTTP 200 with the task still present and Status 'running'; 'poll: error code None not in [\'ResourceNotFoundFault\'] after 37 attempt(s)', poll capped at 180s.
  divergence_key: service=dms|DescribeReplicationTasks|status=400->200|keys=$,status

[mismatch] teardown#31 DescribeReplicationInstances - status: expected 400, got 200.
  Expected (recording): HTTP 400 with error code ResourceNotFoundFault once the replication instance is gone.
  Actual: HTTP 200 with ReplicationInstances[0] ReplicationInstanceStatus 'available'; 'poll: error code None not in [\'ResourceNotFoundFault\'] after 10 attempt(s)', poll capped at 180s.
  divergence_key: service=dms|DescribeReplicationInstances|status=400->200|keys=$,status

Model consulted: smithy_query query=operation operation=StartReplicationTask,DescribeReplicationTasks,DeleteReplicationTask,DeleteReplicationInstance,DescribeReplicationInstances,CreateReplicationTask and query=shape shape=ReplicationTask,InvalidResourceStateFault,ResourceNotFoundFault on services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json. DescribeReplicationTasks and DescribeReplicationInstances each model ResourceNotFoundFault at HTTP 400; DeleteReplicationTask returns the deleted ReplicationTask at HTTP 200; ReplicationTask$StopReason documents the fixed text 'Stop Reason FULL_LOAD_ONLY_FINISHED'; CreateReplicationTask$ReplicationTaskSettings is optional while ReplicationTask$ReplicationTaskSettings is reported back.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay wire fixture dms-s3-full-load-task-migrates-csv-rows from trace set simparity-wire-aws-v1; steps task#21, task#22, teardown#26, teardown#27 and teardown#31 diverge.
3. Shorter repro against DMS alone:
   a. CreateReplicationSubnetGroup over two subnets in two AZs; CreateReplicationInstance and wait for ReplicationInstanceStatus 'available'.
   b. CreateEndpoint twice (EndpointType 'source' and 'target', EngineName 's3').
   c. CreateReplicationTask with MigrationType='full-load', one selection rule, and NO ReplicationTaskSettings; wait for Status 'ready'. The response's ReplicationTask has no ReplicationTaskSettings member, where AWS reports the resolved default task-settings document (Cause 1).
   d. StartReplicationTask(StartReplicationTaskType='start-replication'): HTTP 200, still no ReplicationTaskSettings (Cause 1).
   e. Poll DescribeReplicationTasks: Status goes 'starting' -> 'running' and stays 'running'; StopReason and ReplicationTaskStats are never reported, so a wait for a terminal status never returns (Cause 2).
   f. DeleteReplicationTask(ReplicationTaskArn=<task arn>): HTTP 400 InvalidResourceStateFault, where AWS answers HTTP 200 with the task in 'deleting' (Cause 3).
   g. Poll DescribeReplicationTasks with Filters naming that task: HTTP 200 with the task still present, where AWS answers HTTP 400 ResourceNotFoundFault once it is gone (Cause 4).
   h. DeleteReplicationInstance(ReplicationInstanceArn=<instance arn>) (currently a 501 scaffold, its own issue), then poll DescribeReplicationInstances with Filters naming that instance: HTTP 200 with ReplicationInstanceStatus 'available' forever - settle() has no Owed::Removal branch for replication instances - where AWS answers HTTP 400 ResourceNotFoundFault (Cause 5).
