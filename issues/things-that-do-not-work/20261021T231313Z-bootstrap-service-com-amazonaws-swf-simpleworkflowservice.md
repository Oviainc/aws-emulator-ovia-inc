Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the swf-simpleworkflowservice emulator

_Filed: 2026-10-21T23:13:13Z_
_Operation: service-com-amazonaws-swf-simpleworkflowservice | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 39 handlers under `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no SWF client can do anything at all. Implement a FIRST, SMALL, stateful slice: the domain -> workflow-type -> workflow-execution lifecycle, so a realistic client script runs end to end.

### Happy path to implement (in order; awsJson1_0, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Op>`)

1. **RegisterDomain** — input `name` (required, len 1..256), `workflowExecutionRetentionPeriodInDays` (required, len 1..8), `description`, `tags`. Empty output body (`{}`). Must INSERT a domain record with status `REGISTERED`; a second RegisterDomain with the same name must fail with `DomainAlreadyExistsFault` (HTTP 400), not succeed.
2. **RegisterWorkflowType** — input `domain`, `name`, `version` (all required), plus defaults (`defaultTaskList`, `defaultTaskStartToCloseTimeout`, `defaultExecutionStartToCloseTimeout`, `defaultChildPolicy`, `defaultTaskPriority`, `defaultLambdaRole`, `description`). Empty output. Must store the type keyed by (domain, name, version) with status `REGISTERED` and the supplied defaults; unknown `domain` -> `UnknownResourceFault`, duplicate (name, version) -> `TypeAlreadyExistsFault`.
3. **StartWorkflowExecution** — input `domain`, `workflowId`, `workflowType` (required), plus `taskList`, `input`, `tagList`, `executionStartToCloseTimeout`, `taskStartToCloseTimeout`, `childPolicy`, `taskPriority`, `lambdaRole`. Output `runId` — a freshly generated unique id that MUST be stored, not just generated and discarded. Must create an OPEN execution, seed its history with a `WorkflowExecutionStarted` event (eventId 1) carrying the start parameters, and append a `DecisionTaskScheduled` event (eventId 2). Unknown domain or unregistered workflowType -> `UnknownResourceFault`; a second start with the same `workflowId` while one is still OPEN -> `WorkflowExecutionAlreadyStartedFault`; required timeouts absent on both the request and the type's defaults -> `DefaultUndefinedFault`.
4. **DescribeWorkflowExecution** — input `domain`, `execution` {`workflowId`, `runId`} (required). Output must be computed from the stored execution: `executionInfo` (WorkflowExecutionInfo: `execution`, `workflowType`, `startTimestamp`, `executionStatus` = OPEN/CLOSED, `closeStatus`, `closeTimestamp`, `tagList`, `cancelRequested`), `executionConfiguration` (the effective timeouts/taskList/childPolicy actually resolved at start time, request value winning over the type default), and `openCounts` (WorkflowExecutionOpenCounts: `openActivityTasks`, `openDecisionTasks`, `openTimers`, `openChildWorkflowExecutions`) counted from live state — `openDecisionTasks` must be 1 right after a start, since the DecisionTaskScheduled event is pending. Unknown execution -> `UnknownResourceFault`.
5. **GetWorkflowExecutionHistory** — input `domain`, `execution` (required), `maximumPageSize` (0..1000), `nextPageToken`, `reverseOrder`. Output `events` (list<HistoryEvent> with monotonically increasing `eventId`, correct `eventType` enum and `eventTimestamp`, plus the matching `*EventAttributes` member populated) and `nextPageToken`. Must replay the persisted event log for that run: the events seeded by StartWorkflowExecution and appended by Terminate, honouring `maximumPageSize` and `reverseOrder` and round-tripping `nextPageToken`.
6. **TerminateWorkflowExecution** — input `domain`, `workflowId` (required), `runId`, `reason`, `details`, `childPolicy`. Empty output. Must flip the stored execution to `executionStatus` CLOSED with `closeStatus` TERMINATED and a `closeTimestamp`, append a `WorkflowExecutionTerminated` event (with `reason`/`details`/`childPolicy`), and clear its open decision task. A following DescribeWorkflowExecution must report CLOSED/TERMINATED and GetWorkflowExecutionHistory must show the terminated event. Terminating an already-closed or unknown execution -> `UnknownResourceFault`.

Every one of these six must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), never from canned literals or echoed input. A generated `runId` that is not stored, or a Describe that reflects the request rather than the stored record, counts as a failure of this issue. Also delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each implemented handler.

## State Model

In `src/state.rs` (`ComAmazonawsSwfSimpleworkflowserviceState`), reachable through the shared engine:

- `domains: HashMap<String /*domain name*/, Domain>`
  - `Domain { name: String, description: Option<String>, retention_period_in_days: String, status: RegistrationStatus /*REGISTERED|DEPRECATED*/, arn: String, tags: Vec<ResourceTag> }`
  - `Domain.workflow_types: HashMap<(String /*name*/, String /*version*/), WorkflowTypeRecord>`
    - `WorkflowTypeRecord { name, version, description, status: RegistrationStatus, creation_date: SystemTime, deprecation_date: Option<SystemTime>, default_task_list: Option<String>, default_task_start_to_close_timeout: Option<String>, default_execution_start_to_close_timeout: Option<String>, default_task_priority: Option<String>, default_child_policy: Option<ChildPolicy>, default_lambda_role: Option<String> }`
  - `Domain.activity_types: HashMap<(String, String), ActivityTypeRecord>` (populated later by RegisterActivityType; keep the field so the slice does not need reshaping)
  - `Domain.executions: HashMap<(String /*workflowId*/, String /*runId*/), Execution>`
  - `Domain.open_by_workflow_id: HashMap<String /*workflowId*/, String /*runId*/>` — enforces `WorkflowExecutionAlreadyStartedFault` and resolves a Terminate/Signal/Cancel call that omits `runId`.
- `Execution { workflow_id: String, run_id: String, workflow_type: (String, String), status: ExecutionStatus /*OPEN|CLOSED*/, close_status: Option<CloseStatus>, start_timestamp: SystemTime, close_timestamp: Option<SystemTime>, tag_list: Vec<String>, cancel_requested: bool, input: Option<String>, effective_config: ExecutionConfig, history: Vec<HistoryEventRecord>, next_event_id: i64, open_decision_tasks: u32, open_activity_tasks: u32, open_timers: u32, open_child_executions: u32, latest_execution_context: Option<String> }`
- `ExecutionConfig { task_list: String, task_priority: Option<String>, task_start_to_close_timeout: Option<String>, execution_start_to_close_timeout: Option<String>, child_policy: ChildPolicy, lambda_role: Option<String> }` — resolved once at start (request value else type default) and returned verbatim by DescribeWorkflowExecution.
- `HistoryEventRecord { event_id: i64, event_timestamp: SystemTime, event_type: EventType, attributes: EventAttributes }` where `EventAttributes` is an enum with at least `WorkflowExecutionStarted`, `DecisionTaskScheduled`, `WorkflowExecutionTerminated` variants for this slice; `next_event_id` is the single allocator so ids stay unique and increasing within a run.

The remaining 33 operations (PollForDecisionTask, RespondDecisionTaskCompleted, PollForActivityTask, the Respond*/Count*/List*/Deprecate*/Tag* families, SignalWorkflowExecution, RequestCancelWorkflowExecution, ...) stay at 501 for now and are tracked as individual gap issues; this state model is shaped so they can be layered on without rewriting it.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/` matches all 39 operation files (register_domain.rs, register_workflow_type.rs, start_workflow_execution.rs, describe_workflow_execution.rs, get_workflow_execution_history.rs, terminate_workflow_execution.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501, so the service holds no state at all. Field names, requiredness, length/range constraints and modeled errors above are taken from the crate's generated `OPERATIONS.md` (sections RegisterDomain, RegisterWorkflowType, StartWorkflowExecution, DescribeWorkflowExecution, GetWorkflowExecutionHistory, TerminateWorkflowExecution) and from the Smithy model shapes `DomainInfo`, `DomainConfiguration`, `WorkflowExecution`, `WorkflowExecutionInfo`, `WorkflowExecutionOpenCounts` and `HistoryEvent` (`smithy-model/com-amazonaws-swf-simpleworkflowservice.json`). `README.md` gives the wire protocol: awsJson1_0, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`. No tests exercise any of these operations today (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an SWF client (aws CLI `aws swf ...` or boto3 `client('swf')`) at it.
2. RegisterDomain name=demo-domain workflowExecutionRetentionPeriodInDays=1 -> expect HTTP 200 with an empty JSON body (today: 501 NotImplemented).
3. RegisterDomain name=demo-domain again -> expect HTTP 400 DomainAlreadyExistsFault.
4. RegisterWorkflowType domain=demo-domain name=demo-wf version=1.0 defaultTaskList={name:'demo-tasks'} defaultTaskStartToCloseTimeout=60 defaultExecutionStartToCloseTimeout=300 defaultChildPolicy=TERMINATE -> expect 200 empty body; repeat -> expect TypeAlreadyExistsFault.
5. StartWorkflowExecution domain=demo-domain workflowId=wf-1 workflowType={name:'demo-wf',version:'1.0'} input='{"n":1}' tagList=['t1'] -> expect 200 with a non-empty unique runId; record it.
6. StartWorkflowExecution again with workflowId=wf-1 -> expect HTTP 400 WorkflowExecutionAlreadyStartedFault.
7. DescribeWorkflowExecution domain=demo-domain execution={workflowId:'wf-1',runId:<runId>} -> expect executionInfo.executionStatus=OPEN, executionInfo.workflowType={demo-wf,1.0}, executionInfo.tagList=['t1'], a startTimestamp, executionConfiguration reflecting the type defaults (taskList demo-tasks, childPolicy TERMINATE, the two timeouts) and openCounts.openDecisionTasks=1.
8. GetWorkflowExecutionHistory domain=demo-domain execution={workflowId:'wf-1',runId:<runId>} -> expect events[0].eventType=WorkflowExecutionStarted with eventId 1 and workflowExecutionStartedEventAttributes carrying input/taskList/workflowType, and events[1].eventType=DecisionTaskScheduled with eventId 2.
9. GetWorkflowExecutionHistory with maximumPageSize=1 -> expect exactly 1 event plus a nextPageToken; pass that token back and expect the next event (no repeats, no loss). Repeat with reverseOrder=true and expect the events newest-first.
10. TerminateWorkflowExecution domain=demo-domain workflowId=wf-1 runId=<runId> reason='done' details='bye' -> expect 200 empty body.
11. DescribeWorkflowExecution again -> expect executionStatus=CLOSED, closeStatus=TERMINATED, a closeTimestamp, openCounts.openDecisionTasks=0.
12. GetWorkflowExecutionHistory again -> expect a trailing WorkflowExecutionTerminated event whose attributes carry reason='done' and details='bye'.
13. DescribeWorkflowExecution with a bogus runId, and RegisterWorkflowType in a domain that was never registered -> expect HTTP 400 UnknownResourceFault in both cases.
