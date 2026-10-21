Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Migration Hub emulator

_Filed: 2026-10-21T22:07:59Z_
_Operation: service-com-amazonaws-migrationhub-awsmigrationhub | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in `src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. Implement a FIRST minimal happy-path slice so a realistic client script works end to end against the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: AWSMigrationHub.<OperationName>`).

### Happy path to implement (in order)

1. **CreateProgressUpdateStream** — input `ProgressUpdateStreamName` (string 1..50, `^[^/:|\000-\037]+$`, required), `DryRun` (boolean, default false). Empty output. Must persist a new progress-update stream keyed by name. `DryRun=true` must NOT persist anything and must fail with `DryRunOperation`.
2. **ImportMigrationTask** — input `ProgressUpdateStream` (required), `MigrationTaskName` (string 1..256, `^[^:|]+$`, required), `DryRun`. Empty output. Must create an empty migration task inside the named stream; if the stream does not exist, return `ResourceNotFoundException`.
3. **NotifyMigrationTaskState** — input `ProgressUpdateStream`, `MigrationTaskName`, `Task` (Task{`Status` required enum, `StatusDetail` 0..2500, `ProgressPercent` 0..100}), `UpdateDateTime` (timestamp, required), `NextUpdateSeconds` (integer >=0, default 0), `DryRun`. Empty output. Must OVERWRITE the stored task's `Task`/`UpdateDateTime` and APPEND a `MigrationTaskUpdate{UpdateDateTime, UpdateType, MigrationTaskState}` entry to that task's update history. Unknown stream or task → `ResourceNotFoundException`.
4. **DescribeMigrationTask** — input `ProgressUpdateStream`, `MigrationTaskName`. Output `MigrationTask{ProgressUpdateStream, MigrationTaskName, Task, UpdateDateTime, ResourceAttributeList}`. Must return exactly the values last written by step 3 (and step 6 if called) — read from the store, never echo the request or fabricate defaults. Unknown task → `ResourceNotFoundException`.
5. **ListMigrationTasks** — input `NextToken` (0..2048, `^[a-zA-Z0-9/+=]{0,2048}$`), `MaxResults` (1..100), `ResourceName`. Output `MigrationTaskSummaryList` (list of MigrationTaskSummary{ProgressUpdateStream, MigrationTaskName, Status, ProgressPercent, StatusDetail, UpdateDateTime}) + `NextToken`. Must enumerate the persisted tasks across all streams, honour `MaxResults` by truncating and returning an opaque `NextToken` that resumes exactly where the previous page stopped (and omit `NextToken` on the last page). Summary fields must be derived from the stored `Task`, not canned.
6. **PutResourceAttributes** — input `ProgressUpdateStream`, `MigrationTaskName`, `ResourceAttributeList` (list<ResourceAttribute{Type required enum, Value required 1..256}>, len 1..100), `DryRun`. Empty output. Must persist the attribute list on the task so step 4 returns it in `MigrationTask.ResourceAttributeList`.
7. **DeleteProgressUpdateStream** — input `ProgressUpdateStreamName`, `DryRun`. Empty output. Must remove the stream AND all migration tasks/updates/attributes under it, so a subsequent `DescribeMigrationTask` returns `ResourceNotFoundException` and `ListMigrationTasks` no longer lists them. Unknown stream → `ResourceNotFoundException`.

Every one of these must compute its response from persisted state held in the single shared engine store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). No canned bodies, no echoing the input back, no generating an identifier that is never stored. A create followed by a describe must return what was created, and a delete must be observable by the next reader.

## State Model

In `src/state.rs` (`ComAmazonawsMigrationhubAwsmigrationhubState`):

- `progress_update_streams: BTreeMap<String /* ProgressUpdateStreamName */, ProgressUpdateStream>` — insertion-ordered-by-name registry, backs `ListProgressUpdateStreams` later.
- `ProgressUpdateStream { name: String, created_at: SystemTime, tasks: BTreeMap<String /* MigrationTaskName */, MigrationTask> }`
- `MigrationTask { progress_update_stream: String, migration_task_name: String, task: Option<Task>, update_date_time: Option<SystemTime>, resource_attributes: Vec<ResourceAttribute>, updates: Vec<MigrationTaskUpdate> }`
- `Task { status: Status /* NOT_STARTED | IN_PROGRESS | COMPLETED | FAILED */, status_detail: Option<String>, progress_percent: Option<i32> }`
- `MigrationTaskUpdate { update_date_time: SystemTime, update_type: UpdateType, migration_task_state: Task }` — append-only history, newest-last; backs `ListMigrationTaskUpdates` later.
- `ResourceAttribute { r#type: ResourceAttributeType, value: String }`

Pagination tokens for `ListMigrationTasks` should be an opaque, base64-ish cursor matching `^[a-zA-Z0-9/+=]{0,2048}$` encoding the last-returned `(stream, task)` key, so paging is stable and round-trippable.

The remaining 14 operations (AssociateCreatedArtifact, AssociateDiscoveredResource, AssociateSourceResource, the three Disassociate*, DescribeApplicationState, NotifyApplicationState, ListApplicationStates, ListCreatedArtifacts, ListDiscoveredResources, ListMigrationTaskUpdates, ListProgressUpdateStreams, ListSourceResources) stay at 501 for now and will be filed as individual gap issues; the state model above is deliberately shaped so they can be layered on without reshaping it.

Done = an integration test driving the 7 calls above succeeds with correct, stateful bodies — not merely 2xx.

## Evidence

OPERATIONS.md (crate root) documents all 21 operations with the exact input/output members and errors cited above: CreateProgressUpdateStream (line 45), DeleteProgressUpdateStream (line 51), DescribeMigrationTask (line 63), ImportMigrationTask (line 87), ListMigrationTasks (line 114, @paginated inputToken=NextToken/outputToken=NextToken/items=MigrationTaskSummaryList), NotifyMigrationTaskState (line 148), PutResourceAttributes (line 154). Shape members for MigrationTask, Task, MigrationTaskSummary, MigrationTaskUpdate and ResourceAttribute come from smithy_query on the model at services/service-com-amazonaws-migrationhub-awsmigrationhub/smithy-model/com-amazonaws-migrationhub-awsmigrationhub.json. `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 21 handler files (e.g. src/ops/create_progress_update_stream.rs:4, src/ops/describe_migration_task.rs:4), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No tests exercise any operation; only the generated tests/state_roundtrip.rs exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `mgh` client at it (awsJson1_1, X-Amz-Target: AWSMigrationHub.<Op>).
2. CreateProgressUpdateStream{ProgressUpdateStreamName: "MyTool"} → expect 200 empty body; today 501 NotImplemented.
3. ImportMigrationTask{ProgressUpdateStream: "MyTool", MigrationTaskName: "task-1"} → expect 200.
4. NotifyMigrationTaskState{ProgressUpdateStream: "MyTool", MigrationTaskName: "task-1", Task: {Status: "IN_PROGRESS", StatusDetail: "copying", ProgressPercent: 42}, UpdateDateTime: <now>, NextUpdateSeconds: 60} → expect 200.
5. PutResourceAttributes{ProgressUpdateStream: "MyTool", MigrationTaskName: "task-1", ResourceAttributeList: [{Type: "IPV4_ADDRESS", Value: "10.0.0.7"}]} → expect 200.
6. DescribeMigrationTask{ProgressUpdateStream: "MyTool", MigrationTaskName: "task-1"} → expect MigrationTask.Task.Status == IN_PROGRESS, ProgressPercent == 42, StatusDetail == "copying", UpdateDateTime == the value sent, and ResourceAttributeList containing 10.0.0.7.
7. ListMigrationTasks{MaxResults: 1} → expect exactly one MigrationTaskSummary for MyTool/task-1 with Status/ProgressPercent/UpdateDateTime matching the stored task; add a second task and confirm MaxResults=1 returns a NextToken whose follow-up page returns the other task and no NextToken.
8. DescribeMigrationTask on an unknown MigrationTaskName → expect ResourceNotFoundException (HTTP 400), not 501/200.
9. DeleteProgressUpdateStream{ProgressUpdateStreamName: "MyTool"} → expect 200, then DescribeMigrationTask for MyTool/task-1 → ResourceNotFoundException and ListMigrationTasks → empty list.
