Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the DataSync emulator

_Filed: 2026-10-21T21:38:28Z_
_Operation: service-com-amazonaws-datasync-fmrsservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 53 handlers in `services/service-com-amazonaws-datasync-fmrsservice/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented`, so no DataSync client can do anything. Implement a FIRST SMALL SLICE: the core transfer-task lifecycle, backed by real persisted state.

Protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: FmrsService.<OperationName>` (see README/OPERATIONS.md wire-protocol section).

### Happy path to implement (in order)

1. **CreateLocationS3** — in: `S3BucketArn` (required), `S3Config` (required), `Subdirectory`, `S3StorageClass`, `AgentArns`, `Tags`. Out: `LocationArn`, newly minted and matching the modeled pattern `arn:aws:datasync:<region>:<account>:location/loc-[0-9a-z]{17}`. Store the full location record; do not echo.
2. **CreateTask** — in: `SourceLocationArn` (required), `DestinationLocationArn` (required), `Name`, `Options`, `Includes`/`Excludes`, `Schedule`, `CloudWatchLogGroupArn`, `TaskMode`, `Tags`. Out: `TaskArn` matching `.../task/task-[0-9a-f]{17}`. Both location ARNs must resolve to locations previously created in state; an unknown ARN is `InvalidRequestException` (HTTP 400), not a success.
3. **StartTaskExecution** — in: `TaskArn` (required), optional `OverrideOptions`, `Includes`, `Excludes`. Out: `TaskExecutionArn` = `<TaskArn>/execution/exec-<17 hex>`. Must create a persisted execution record, set the parent task's `Status` to `RUNNING` and its `CurrentTaskExecutionArn` to the new execution ARN. Unknown `TaskArn` → `InvalidRequestException`.
4. **DescribeTaskExecution** — in: `TaskExecutionArn` (required). Out must come from the stored execution: `TaskExecutionArn`, `Status` (one of `QUEUED|LAUNCHING|PREPARING|TRANSFERRING|VERIFYING|SUCCESS|ERROR|CANCELLING`), `StartTime`, the `Options` actually in effect (task options overlaid with `OverrideOptions`), `Includes`/`Excludes`, and the required-with-default counters (`EstimatedFilesToTransfer`, `EstimatedBytesToTransfer`, `FilesTransferred`, `BytesWritten`, `BytesTransferred`, `BytesCompressed`, `FilesDeleted`, `FilesSkipped`, `FilesVerified`, `EstimatedFilesToDelete`, `FilesPrepared`). A simple deterministic progression (e.g. first describe `LAUNCHING`/`TRANSFERRING`, then `SUCCESS` with `EndTime` set) is acceptable; a hardcoded constant body is not.
5. **DescribeTask** — in: `TaskArn` (required). Out from state: `TaskArn`, `Name`, `Status`, `SourceLocationArn`, `DestinationLocationArn`, `Options`, `Includes`/`Excludes`, `Schedule`, `CreationTime`, `CurrentTaskExecutionArn`, `TaskMode` — exactly the values given to CreateTask.
6. **ListTasks** — in: `MaxResults` (0..100), `NextToken`, `Filters`. Out: `Tasks` (`TaskListEntry`: `TaskArn`, `Status`, `Name`, `TaskMode`) for every task in state, honouring `MaxResults` by truncating and returning an opaque `NextToken` (matching `^[a-zA-Z0-9=_-]+$`) that resumes exactly where the page ended; omit `NextToken` on the final page.
7. **DeleteTask** — in: `TaskArn` (required). Empty output. Removes the task from state: a following `DescribeTask`/`ListTasks` must no longer see it (`InvalidRequestException` on describe), and the call must not be a silent no-op for an unknown ARN.

Everything must be computed from persisted state held in the shared engine store (`crate::state::service_state(&*self.engine)`). No canned responses, no echoing the input back, no minting an ARN that is never stored. A create followed by a describe/list must return what was created, across separate HTTP requests.

### State Model

In `src/state.rs`, on `ComAmazonawsDatasyncFmrsserviceState`:

- `locations: HashMap<String /*LocationArn*/, Location>` where `Location { arn, location_uri (e.g. "s3://<bucket>/<subdirectory>"), kind: LocationKind (S3 for this slice), s3_bucket_arn, s3_config, s3_storage_class, subdirectory, agent_arns: Vec<String>, tags: Vec<TagListEntry>, creation_time: SystemTime }`
- `tasks: HashMap<String /*TaskArn*/, Task>` where `Task { arn, name, status: TaskStatus, source_location_arn, destination_location_arn, cloud_watch_log_group_arn, options, includes: Vec<FilterRule>, excludes: Vec<FilterRule>, schedule, task_mode, tags, creation_time, current_task_execution_arn: Option<String>, execution_arns: Vec<String> /* creation order, for ListTaskExecutions later */ }`
- `task_executions: HashMap<String /*TaskExecutionArn*/, TaskExecution>` where `TaskExecution { arn, task_arn, status: TaskExecutionStatus, options (effective), includes, excludes, start_time, launch_time: Option<..>, end_time: Option<..>, counters: ExecutionCounters { estimated_files_to_transfer, estimated_bytes_to_transfer, files_transferred, bytes_written, bytes_transferred, bytes_compressed, files_deleted, files_skipped, files_verified, estimated_files_to_delete, files_prepared }, result: Option<TaskExecutionResultDetail> }`
- counters for deterministic ARN minting: `next_location_seq`, `next_task_seq`, `next_execution_seq` (used to generate the id suffixes that satisfy the modeled ARN patterns — `loc-[0-9a-z]{17}`, `task-[0-9a-f]{17}`, `exec-` hex).
- `list_tokens`/pagination cursor encoding for `ListTasks` (an index or last-seen-ARN encoded into the opaque `NextToken`).

### Done when

An integration test drives CreateLocationS3 (twice: source + destination) → CreateTask → StartTaskExecution → DescribeTaskExecution → DescribeTask → ListTasks → DeleteTask → DescribeTask-is-gone over HTTP, asserting response BODIES (the ARNs round-trip, the task fields equal what was created, the execution belongs to the task, pagination resumes) and not merely 2xx.

All other operations (agents, the other eleven location families, Update*/Tag*/Untag*, CancelTaskExecution, ListLocations, ListAgents, ListTaskExecutions, UpdateTask, UpdateTaskExecution) stay at 501 for now and will be filed as individual gap issues.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-datasync-fmrsservice/src/ops/` matches all 53 operation files, including create_location_s3.rs, create_task.rs, start_task_execution.rs, describe_task_execution.rs, describe_task.rs, list_tasks.rs and delete_task.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. The operation contracts, required members, ARN patterns, enums (`TaskStatus`, `TaskExecutionStatus`, `S3StorageClass`, `TaskMode`), paginated traits (`inputToken=NextToken, outputToken=NextToken, items=Tasks`) and the only two modeled errors (`InternalException` 500, `InvalidRequestException` 400) are taken from services/service-com-amazonaws-datasync-fmrsservice/OPERATIONS.md (lines 93-97 CreateLocationS3, 105-109 CreateTask, 123-127 DeleteTask, 189-193 DescribeLocationS3, 201-211 DescribeTask/DescribeTaskExecution, 241-252 ListTasks/StartTaskExecution) and the wire-protocol section (awsJson1_1, X-Amz-Target FmrsService.<Op>). Create→start→describe→delete statefulness is well-established DataSync behavior: StartTaskExecution sets the task to RUNNING and populates CurrentTaskExecutionArn, and DescribeTask returns the configuration supplied at CreateTask.

## Steps to Reproduce

1. POST / with X-Amz-Target: FmrsService.CreateLocationS3 and body {"S3BucketArn":"arn:aws:s3:::src-bucket","S3Config":{"BucketAccessRoleArn":"arn:aws:iam::123456789012:role/datasync"},"Subdirectory":"/in"} — expect 200 with a LocationArn; today HTTP 501 NotImplemented.
2. Repeat for a destination bucket to get a second LocationArn.
3. POST FmrsService.CreateTask with {"SourceLocationArn":<loc1>,"DestinationLocationArn":<loc2>,"Name":"demo-task"} — expect 200 with a TaskArn matching task/task-[0-9a-f]{17}.
4. POST FmrsService.StartTaskExecution with {"TaskArn":<taskArn>} — expect 200 with TaskExecutionArn = <taskArn>/execution/exec-...
5. POST FmrsService.DescribeTaskExecution with that TaskExecutionArn — expect the stored execution: matching ARN, a valid Status, StartTime and the required counter fields.
6. POST FmrsService.DescribeTask with <taskArn> — expect Name "demo-task", the two source/destination ARNs created in step 1-2, CreationTime and CurrentTaskExecutionArn equal to the execution started in step 4.
7. POST FmrsService.ListTasks with {"MaxResults":1} — expect one TaskListEntry plus a NextToken; follow the token to get the remaining task(s) with no duplicates.
8. POST FmrsService.DeleteTask with <taskArn>, then POST FmrsService.DescribeTask with the same ARN — expect InvalidRequestException (HTTP 400) and the task absent from ListTasks.
9. POST FmrsService.CreateTask with a SourceLocationArn that was never created — expect InvalidRequestException (HTTP 400), not a success.
