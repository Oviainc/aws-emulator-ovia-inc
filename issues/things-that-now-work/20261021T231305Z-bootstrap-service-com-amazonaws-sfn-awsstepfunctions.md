Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the AWS Step Functions (SFN) emulator

_Filed: 2026-10-21T23:13:05Z_
_Operation: service-com-amazonaws-sfn-awsstepfunctions | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 operations in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. No client workflow can work.

This issue scopes the FIRST buildable slice: the core state-machine lifecycle + one execution, implemented statefully over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: AWSStepFunctions.<Operation>`).

### Happy path to implement (in order)

1. **CreateStateMachine** — input `name` (required, 1..80), `definition` (required ASL JSON), `roleArn` (required), optional `type` (STANDARD|EXPRESS, default STANDARD), `tags`, `loggingConfiguration`, `tracingConfiguration`, `publish`. Must mint and persist a state machine under ARN `arn:aws:states:<region>:<account>:stateMachine:<name>` and return `stateMachineArn` + `creationDate` (the stored creation time). Re-creating the same name with a *different* definition/role must fail `StateMachineAlreadyExists`; the `@idempotent` same-input repeat returns the existing ARN.
2. **DescribeStateMachine** — input `stateMachineArn`. Must return the PERSISTED `name`, `definition`, `roleArn`, `type`, `creationDate`, `status: ACTIVE`, plus `loggingConfiguration`/`tracingConfiguration` exactly as supplied at create time — not echoed from this request and not canned. Unknown ARN -> `StateMachineDoesNotExist`.
3. **StartExecution** — input `stateMachineArn` (required), optional `name` (auto-generate a UUID when absent), `input` (JSON string, default `{}`), `traceHeader`. Must reject an unknown state machine with `StateMachineDoesNotExist`, reject a duplicate execution name on the same state machine with `ExecutionAlreadyExists`, store a new Execution record, and return `executionArn` = `arn:aws:states:<region>:<account>:execution:<smName>:<execName>` plus `startDate`. The execution must then be driven to a terminal state by interpreting a minimal ASL subset (`Pass`, `Succeed`, `Fail`, plus `StartAt`/`Next`/`End`): `status` becomes `SUCCEEDED` with `output` derived from the input/`Result`, or `FAILED` with `error`/`cause` for a `Fail` state. A `Wait`/`Task` definition may legitimately stay `RUNNING` for now.
4. **DescribeExecution** — input `executionArn`. Must return the stored `executionArn`, `stateMachineArn`, `name`, `status`, `startDate`, and — once terminal — `stopDate`, `output`, and `error`/`cause`; `input` must be byte-identical to what StartExecution was given. Unknown ARN -> `ExecutionDoesNotExist`.
5. **ListExecutions** — input `stateMachineArn`, optional `statusFilter`, `maxResults` (0..1000, 0 = default page size), `nextToken`. Must return `executions` (`ExecutionListItem`: `executionArn`, `stateMachineArn`, `name`, `status`, `startDate`, `stopDate`) built from stored executions of that state machine, newest-first, honoring `statusFilter` and emitting `nextToken` when truncated. Unknown ARN -> `StateMachineDoesNotExist`.
6. **DeleteStateMachine** — input `stateMachineArn`. Must remove the state machine from state so a following `DescribeStateMachine` fails `StateMachineDoesNotExist` and `ListStateMachines` no longer lists it. Deleting an absent ARN is a success (idempotent, per real SFN).

Optionally round out the slice with **ListStateMachines** (`stateMachines` list of `StateMachineListItem{stateMachineArn,name,type,creationDate}` from state) and **StopExecution** (sets `status: ABORTED`, `stopDate`, `error`/`cause`, returns the stored `stopDate`), both trivial once the state model exists.

### NEVER STUBS
Every operation above must compute its response from persisted state. Hardcoded outputs, echoing the request back, or minting an ARN without storing the record are failures of this issue: `Create -> Describe` must return what was created, and `Delete -> Describe` must fail.

## State Model

Service state lives in `ComAmazonawsSfnAwsstepfunctionsState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)` (one shared engine; never construct an engine in a handler):

- `state_machines: HashMap<String /* stateMachineArn */, StateMachine>` where
  `StateMachine { arn, name, definition: String, role_arn: String, sm_type: StateMachineType /* STANDARD|EXPRESS */, status: StateMachineStatus /* ACTIVE|DELETING */, creation_date: SystemTime, update_date: SystemTime, logging_configuration: Option<LoggingConfiguration>, tracing_configuration: Option<TracingConfiguration>, encryption_configuration: Option<EncryptionConfiguration>, revision_id: Option<String>, description: Option<String>, tags: Vec<Tag> }`
- `sm_name_index: HashMap<String /* name */, String /* arn */>` for `StateMachineAlreadyExists` / name->ARN lookup.
- `executions: HashMap<String /* executionArn */, Execution>` where
  `Execution { arn, state_machine_arn, name, status: ExecutionStatus /* RUNNING|SUCCEEDED|FAILED|TIMED_OUT|ABORTED|PENDING_REDRIVE */, start_date: SystemTime, stop_date: Option<SystemTime>, input: String, output: Option<String>, error: Option<String>, cause: Option<String>, trace_header: Option<String>, redrive_count: u32, history: Vec<HistoryEvent> }`
- `executions_by_sm: HashMap<String /* stateMachineArn */, Vec<String /* executionArn */>>` in start order, so ListExecutions can page newest-first without scanning everything.
- `history` on each Execution is appended as the interpreter runs (`ExecutionStarted`, `PassStateEntered/Exited`, `ExecutionSucceeded`/`ExecutionFailed`) so a later GetExecutionHistory slice reads real events rather than fabricating them.
- `activities: HashMap<String /* activityArn */, Activity{arn,name,creation_date,tags}>` and `task_tokens: HashMap<String /* taskToken */, TaskToken{execution_arn, activity_arn, ...}>` may be declared now but left empty — the activity/task-token loop is out of scope for this slice.

ARN synthesis should use the request's region/account from `ctx` (falling back to a fixed test account) and be stable across calls so clients can round-trip ARNs.

### Out of scope (stay 501, filed separately later)
Versions/aliases (`PublishStateMachineVersion`, `*StateMachineAlias*`, `DeleteStateMachineVersion`), activities and the task-token loop (`CreateActivity`, `GetActivityTask`, `SendTask*`), map runs (`DescribeMapRun`, `ListMapRuns`, `UpdateMapRun`), `StartSyncExecution`, `RedriveExecution`, `TestState`, `ValidateStateMachineDefinition`, tagging (`TagResource`, `UntagResource`, `ListTagsForResource`), `UpdateStateMachine`, `DescribeStateMachineForExecution`, `GetExecutionHistory`, `DescribeActivity`, `DeleteActivity`, `ListActivities`.

## Evidence

All 37 handler files under `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/` match `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:'` (full list confirmed: create_state_machine.rs, start_execution.rs, describe_execution.rs, list_executions.rs, describe_state_machine.rs, delete_state_machine.rs, ... every file in the directory), so every operation answers HTTP 501 and no state is kept.

Contracts for the chosen slice come from the crate's `OPERATIONS.md` (rendered from `smithy-model/com-amazonaws-sfn-awsstepfunctions.json`): protocol `awsJson1_0`, target prefix `AWSStepFunctions.`, sigv4 name `states`. CreateStateMachine requires `name`/`definition`/`roleArn` and returns `stateMachineArn`+`creationDate` with `StateMachineAlreadyExists`/`InvalidDefinition` errors; DescribeStateMachine returns `name`/`definition`/`roleArn`/`type`/`creationDate`/`status` with `StateMachineDoesNotExist`; StartExecution takes `stateMachineArn`/`name`/`input` and returns `executionArn`+`startDate` with `ExecutionAlreadyExists`/`StateMachineDoesNotExist`; DescribeExecution returns `status`/`startDate`/`stopDate`/`input`/`output`/`error`/`cause` with `ExecutionDoesNotExist`; ListExecutions is `@paginated(inputToken=nextToken, outputToken=nextToken, items=executions)` with `statusFilter` and `maxResults` 0..1000; DeleteStateMachine takes `stateMachineArn` and returns an empty body. Well-established SFN semantics ground the rest: delete is idempotent, execution ARNs are `...:execution:<smName>:<execName>`, and a Pass/Succeed-only definition completes immediately as SUCCEEDED.

No tests exercise these operations (only the scaffold `tests/state_roundtrip.rs` example exists), and `list_existing_issues` reports no prior SFN issues.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `stepfunctions` client at it (endpoint-url = emulator, region us-east-1).
2. CreateStateMachine with name='demo', roleArn='arn:aws:iam::123456789012:role/svc', definition='{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}' — expect 200 with a stateMachineArn ending in ':stateMachine:demo' and a creationDate. Today: HTTP 501 NotImplemented.
3. DescribeStateMachine with that ARN — expect the definition, roleArn, type=STANDARD, status=ACTIVE and the SAME creationDate returned by step 2.
4. StartExecution with that ARN, name='run1', input='{"a":1}' — expect 200 with executionArn '...:execution:demo:run1' and a startDate.
5. Repeat the same StartExecution — expect the modeled ExecutionAlreadyExists error, not a second execution.
6. DescribeExecution with the executionArn — expect name='run1', input exactly '{"a":1}', status=SUCCEEDED, a stopDate and output '{"msg":"hi"}'.
7. ListExecutions for the state machine ARN — expect one ExecutionListItem for 'run1' with matching status/startDate; repeat with statusFilter=FAILED and expect an empty list.
8. DeleteStateMachine with the ARN, then DescribeStateMachine again — expect StateMachineDoesNotExist, and ListStateMachines no longer contains 'demo'.
9. DescribeExecution / DescribeStateMachine with a syntactically valid but unknown ARN — expect ExecutionDoesNotExist / StateMachineDoesNotExist rather than 501 or a hollow 200.
