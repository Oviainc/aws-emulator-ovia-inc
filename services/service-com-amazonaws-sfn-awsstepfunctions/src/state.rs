// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sfn-awsstepfunctions service.
//!
//! Six resources live here, and every operation answers from them:
//!
//! * a **state machine**, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:stateMachine:<name>`). That ARN is a total
//!   function of the region and the name, so the ARN-keyed map IS the name index
//!   `CreateStateMachine`'s idempotency check needs — and a name is therefore scoped
//!   per region and account, the way the service scopes it, with no second copy of
//!   the record to keep in step;
//! * a **state machine version**, a numbered snapshot of a state machine's
//!   configuration taken when `publish` was set, keyed by its own qualified ARN
//!   (`<stateMachineArn>:<version>`), so one state machine's versions sit
//!   contiguously under its ARN and go with it when it is deleted;
//! * a **state machine alias**, a name that points at one or two versions of ONE
//!   state machine, keyed by its own qualified ARN (`<stateMachineArn>:<aliasName>`)
//!   so one state machine's aliases sit contiguously under its ARN and go with it
//!   when it is deleted, exactly as its versions do;
//! * an **activity**, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:activity:<name>`). That ARN is the same total
//!   function of the region and the name, so the ARN-keyed map IS the name index
//!   `CreateActivity`'s idempotency check needs, and an activity name is scoped per
//!   region and account exactly as a state machine name is;
//! * an **execution**, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:execution:<stateMachineName>:<executionName>`),
//!   with `executions_by_sm` listing one state machine's executions in start order so
//!   `ListExecutions` pages without walking the whole store. Each execution carries
//!   the state machine configuration it RAN ([`RanConfiguration`]), because an update
//!   to the state machine leaves an execution already started on "the previous
//!   definition and roleArn";
//! * a **map run**, the resource a Distributed Map state starts, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:mapRun:<stateMachineName>/<label>:<id>`),
//!   with `map_runs_by_execution` listing one execution's map runs in the order their
//!   states were entered so `ListMapRuns` pages the same way;
//! * a **child workflow execution**, the execution ONE item of a Distributed Map
//!   state ran as. It is an execution like any other — the same record, keyed by its
//!   own ARN
//!   (`arn:aws:states:<region>:<account>:execution:<stateMachineName>/<label>:<id>`)
//!   — and `executions_by_map_run` lists one map run's children in the order their
//!   items started, which is the index `ListExecutions(mapRunArn)` pages. A child is
//!   indexed there and NOT under its state machine, so `ListExecutions` for the
//!   parent's state machine reports the executions started on it and none of them.
//!
//! ## What the clock owes
//!
//! Four transitions here are owed to the clock rather than applied in the call that
//! starts them, because that is what the service reports:
//!
//! * an execution starts `RUNNING` and reaches its terminal status later. The run is
//!   interpreted eagerly by [`asl::run`] at `StartExecution` — and again at
//!   `RedriveExecution`, which re-runs the same definition over the same input — and
//!   parked in [`PendingOutcome`], owed at
//!   `<the attempt's start> + EXECUTION_SETTLE_DELAY + <every Wait>`.
//! * an execution whose walk reached an ACTIVITY task has no outcome at all yet: the
//!   task is queued under its activity's ARN as an [`ActivityTask`], a worker claims
//!   it with `GetActivityTask`, and the clock closes it — failing the execution — if
//!   the `TimeoutSeconds` or `HeartbeatSeconds` of the scheduling state runs out
//!   first.
//! * an execution whose walk PARKED at the job a run-a-job (`.sync`) integration
//!   started has no outcome yet either: the job is held as a [`JobTask`] and polled
//!   through its own service's reporting call ([`run_a_job`]) until that service says
//!   it finished, which is what resolves the task. This is NOT a transition the clock
//!   owes — only the invoked service can say whether its job is done — but the TIME
//!   the task is allowed is: a job unfinished past [`JobTask::times_out_at`] fails the
//!   execution with the language's own timeout error.
//! * `DeleteStateMachine` is documented as asynchronous: it "sets the state machine's
//!   status to `DELETING` and begins the deletion process", so the record moves to
//!   `DELETING` now and is removed once [`DELETE_SETTLE_DELAY`] has passed.
//! * a map run a Distributed Map state started is `RUNNING` until the instant its
//!   items were due, so a caller that lists or describes it straight after the start
//!   sees it running, exactly as the execution around it is — and so is each child
//!   workflow execution it started, until the instant its own item finished.
//!
//! Nothing sleeps: the due instant is stored and [`ComAmazonawsSfnAwsstepfunctionsState::settle`]
//! materialises whatever the clock has already made due. That is the only write a
//! reporting operation makes.
//!
//! ## ARNs are validated before anything is looked up
//!
//! Every operation reads its ARN members through [`read_state_machine_arn`],
//! [`read_execution_arn`], [`read_activity_arn`], [`read_taggable_arn`] or
//! [`read_map_run_arn`], each of
//! which judges the ARN itself — its syntax, its service and the region the
//! request's endpoint serves — BEFORE the store is consulted, and answers the
//! message the modelled `InvalidArn` carries. A defect in the ARN is therefore
//! never reported as a resource that is absent.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    EncryptionConfiguration, LoggingConfiguration, RoutingConfigurationListItem, Tag,
    TracingConfiguration, ENCRYPTION_TYPE_VALUES,
};

/// The ARN service namespace, sigv4 signing name and endpoint prefix of this
/// service (`OPERATIONS.md`, "Wire protocol").
pub const ARN_SERVICE: &str = "states";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource type a state machine ARN carries.
pub const STATE_MACHINE_RESOURCE: &str = "stateMachine";

/// The resource type an execution ARN carries.
pub const EXECUTION_RESOURCE: &str = "execution";

/// The resource type a SYNCHRONOUS Express execution ARN carries.
///
/// `StartSyncExecution` starts "a Synchronous Express state machine execution" and
/// answers the execution's own ARN, which names the `express` resource rather than
/// the `execution` one and carries a trailing id field:
/// `arn:<partition>:states:<region>:<account>:express:<stateMachineName>:<executionName>:<executionId>`.
/// That id is what tells two runs that reused one name apart — which Express
/// workflows allow, since `StartExecution`'s `name` documents that "for EXPRESS
/// workflows, execution names can be reused immediately" — so a synchronous start is
/// never refused for a name another run already used.
pub const EXPRESS_EXECUTION_RESOURCE: &str = "express";

/// The resource type an activity ARN carries.
///
/// A Step Functions tagging call takes "the ARN for the Step Functions state
/// machine or activity", so this is the second resource type those calls accept,
/// and `CreateActivity`/`DescribeActivity` read it through
/// [`read_activity_arn`].
pub const ACTIVITY_RESOURCE: &str = "activity";

/// The resource type a Map Run ARN carries (`ListExecutions`' `mapRunArn`).
pub const MAP_RUN_RESOURCE: &str = "mapRun";

/// `ValidationExceptionReason`'s member for an API that does not accept a map-run
/// labeled state machine ARN (`smithy_query(query="shape",
/// shape="ValidationExceptionReason")`: `API_DOES_NOT_SUPPORT_LABELED_ARNS`,
/// `MISSING_REQUIRED_PARAMETER`, `CANNOT_UPDATE_COMPLETED_MAP_RUN`,
/// `INVALID_ROUTING_CONFIGURATION`).
pub const VALIDATION_REASON_LABELED_ARN: &str = "API_DOES_NOT_SUPPORT_LABELED_ARNS";

/// `ValidationExceptionReason`'s member for a request that left out a parameter the
/// operation requires, from the same enum.
pub const VALIDATION_REASON_MISSING_PARAMETER: &str = "MISSING_REQUIRED_PARAMETER";

/// `ValidationExceptionReason`'s member for a routing configuration an alias cannot
/// point at, from the same enum.
pub const VALIDATION_REASON_INVALID_ROUTING_CONFIGURATION: &str =
    "INVALID_ROUTING_CONFIGURATION";

/// `ValidationExceptionReason`'s member for a configuration change asked of a Map
/// Run that has already finished, from the same enum.
pub const VALIDATION_REASON_COMPLETED_MAP_RUN: &str = "CANNOT_UPDATE_COMPLETED_MAP_RUN";

/// What the modelled `ValidationException` says when the Map Run a configuration
/// change was asked of has already reached a terminal status: `UpdateMapRun`
/// "updates an IN-PROGRESS Map Run's configuration", so there is nothing left for
/// the change to govern.
///
/// RECORDED text: the condition, and then the Map Run the request ADDRESSED — the
/// `mapRunArn` member it carried — so a caller retuning many Map Runs at once reads
/// which one had already finished out of the refusal itself, rather than having to
/// re-read each of them. Where [`LABELED_ARN_MESSAGE`] states a rule that no value
/// of the request appears in, this one identifies the resource it refused.
pub fn completed_map_run_message(map_run_arn: &str) -> String {
    format!("Cannot update completed MapRun {map_run_arn}")
}

/// What the modelled `ValidationException` says when a configuration change names
/// none of the settings it can change: `UpdateMapRun`'s three threshold members are
/// each optional with a null default, so a request that carries none of them states
/// no change at all. It names nothing from the request, because no member of it is
/// what was refused.
pub const EMPTY_MAP_RUN_UPDATE_MESSAGE: &str =
    "Either the max concurrency, the tolerated failure count, or the tolerated failure \
     percentage must be specified";

/// What the modelled `ValidationException` says about a map-run labeled state
/// machine ARN, on every operation that documents refusing one. It names nothing
/// from the request, so every such operation answers exactly this.
pub const LABELED_ARN_MESSAGE: &str = "Labeled State Machine ARNs are not supported for this API";

/// Returned when the shared engine does not carry this service's store, which can
/// only happen on an engine this crate was not mounted on. The model declares no
/// server-fault error for any operation in this slice, so the refusal goes out
/// through the one non-modelled channel the crate has.
pub const STORE_UNAVAILABLE: &str = "the Step Functions store is unavailable on this engine";

/// Returned when the blocking thread the definition is INTERPRETED on did not
/// answer — a cancelled runtime, or a panic the interpreter itself raised.
///
/// The model declares no server-fault error for `StartExecution` or
/// `RedriveExecution`, so this refusal goes out through the one non-modelled channel
/// the crate has rather than being reported as a started execution that was never
/// planned.
pub const INTERPRETER_UNAVAILABLE: &str =
    "the state machine definition could not be interpreted on this engine";

// ---------------------------------------------------------------------------
// Wire values, from the model's own enums (`crate::ops::types::*_VALUES`).
// ---------------------------------------------------------------------------

/// `StateMachineType`'s default, from the `type` member's documentation: "The
/// default is STANDARD."
pub const STATE_MACHINE_TYPE_STANDARD: &str = "STANDARD";

/// The Express workflow type. `DescribeExecution`, `ListExecutions` and
/// `StopExecution` each document that they do not serve executions of one.
pub const STATE_MACHINE_TYPE_EXPRESS: &str = "EXPRESS";

/// A state machine that is ready to start executions.
pub const STATE_MACHINE_STATUS_ACTIVE: &str = "ACTIVE";

/// A state machine `DeleteStateMachine` has begun deleting.
pub const STATE_MACHINE_STATUS_DELETING: &str = "DELETING";

/// `EncryptionType`'s default: the key Step Functions encrypts with when a request
/// configures no customer-managed key at all ("For additional control over
/// security, you can encrypt your data using a customer-managed key" — absent that,
/// the data is encrypted under the Amazon Web Services owned key).
pub const ENCRYPTION_TYPE_AWS_OWNED_KEY: &str = "AWS_OWNED_KEY";

/// The `EncryptionType` that encrypts with a KMS key the caller names, so
/// `kmsKeyId` and `kmsDataKeyReusePeriodSeconds` only mean anything under it
/// (`kmsDataKeyReusePeriodSeconds`: "Only applies to customer managed keys").
pub const ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// `LogLevel`'s default, from the `loggingConfiguration` member's own
/// documentation: "By default, the level is set to OFF."
pub const LOG_LEVEL_OFF: &str = "OFF";

/// The least `kmsDataKeyReusePeriodSeconds` the modelled
/// `KmsDataKeyReusePeriodSeconds` accepts (range 60..900).
pub const MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS: i32 = 60;

/// The most `kmsDataKeyReusePeriodSeconds` the modelled
/// `KmsDataKeyReusePeriodSeconds` accepts (range 60..900).
pub const MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS: i32 = 900;

/// An execution that has not reached a terminal status yet.
pub const EXECUTION_STATUS_RUNNING: &str = "RUNNING";

/// An execution that ran to a `Succeed` state or to `"End": true`.
pub const EXECUTION_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// An execution that ran to a `Fail` state, or that could not be run.
pub const EXECUTION_STATUS_FAILED: &str = "FAILED";

/// An execution `StopExecution` stopped.
pub const EXECUTION_STATUS_ABORTED: &str = "ABORTED";

/// An execution that ran for longer than the `TimeoutSeconds` its definition
/// declares.
pub const EXECUTION_STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// A Map Run whose items have not all finished yet, one of the values the modelled
/// `MapRunStatus` carries.
pub const MAP_RUN_STATUS_RUNNING: &str = "RUNNING";

/// A Map Run every item of which its iteration produced a result for, or whose
/// failures stayed inside what its `Map` state tolerates.
pub const MAP_RUN_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// A Map Run more of whose items failed than its `Map` state tolerates.
pub const MAP_RUN_STATUS_FAILED: &str = "FAILED";

/// A Map Run whose parent execution was stopped before its items finished.
pub const MAP_RUN_STATUS_ABORTED: &str = "ABORTED";

/// `ValidateStateMachineDefinitionResultCode`'s value for a definition that
/// carries no error: "The result value will be OK when no syntax errors are
/// found", and "the result can be OK even when diagnostic warnings are present".
pub const VALIDATION_RESULT_OK: &str = "OK";

/// `ValidateStateMachineDefinitionResultCode`'s value for a definition that
/// cannot be created: "FAIL if the workflow definition does not pass
/// verification".
pub const VALIDATION_RESULT_FAIL: &str = "FAIL";

/// `ValidateStateMachineDefinitionSeverity`'s error level, which is also the
/// `severity` member's default: "ERROR returns only ERROR diagnostics ... The
/// default is ERROR." A diagnostic of this severity means "you cannot create or
/// update a state machine with this definition".
pub const SEVERITY_ERROR: &str = "ERROR";

/// `ValidateStateMachineDefinitionSeverity`'s warning level: "WARNING returns
/// both WARNING and ERROR diagnostics", and a `WARNING` diagnostic alerts to a
/// potential issue that "will not prevent you from creating or updating your
/// state machine".
pub const SEVERITY_WARNING: &str = "WARNING";

/// `IncludedData`'s metadata-only member: see [`reported_definition`].
pub const INCLUDED_DATA_METADATA_ONLY: &str = "METADATA_ONLY";

/// `IncludedData`'s every-member value, which reports the definition in full.
pub const INCLUDED_DATA_ALL_DATA: &str = "ALL_DATA";

/// What the weights of ONE routing configuration must add up to.
///
/// From `RoutingConfigurationListItem`'s `weight` documentation: "The percentage of
/// traffic you want to route to a state machine version. The sum of the weights in
/// the routing configuration must be equal to 100."
pub const ROUTING_WEIGHT_TOTAL: i32 = 100;

/// What a metadata-only read reports for a state machine `definition`.
///
/// From the `definition` output member's own documentation: "If called with
/// includedData = METADATA_ONLY, the returned definition will be {}."
pub const METADATA_ONLY_DEFINITION: &str = "{}";

// ---------------------------------------------------------------------------
// Redrive
//
// `DescribeExecution` reports, for every execution, whether `RedriveExecution`
// would redrive it (`redriveStatus`) and — when it would not — the reason why
// (`redriveStatusReason`). Both are answered from ONE predicate,
// [`ComAmazonawsSfnAwsstepfunctionsState::not_redrivable_reason`], so the field a
// client reads to decide whether to call `RedriveExecution` cannot disagree with
// what that call then does.
// ---------------------------------------------------------------------------

/// `ExecutionRedriveStatus`'s value for an execution `RedriveExecution` would
/// redrive.
pub const REDRIVE_STATUS_REDRIVABLE: &str = "REDRIVABLE";

/// `ExecutionRedriveStatus`'s value for an execution it would refuse: "redriveStatus
/// is NOT_REDRIVABLE if calling the RedriveExecution API action would return the
/// ExecutionNotRedrivable error".
pub const REDRIVE_STATUS_NOT_REDRIVABLE: &str = "NOT_REDRIVABLE";

/// `ExecutionRedriveStatus`'s value for a CHILD workflow execution of a Distributed
/// Map state that its Map Run could redrive: "for a Distributed Map that includes
/// child workflows [...] redriveStatus indicates whether or not the Map Run can
/// redrive child workflow executions". A child is redriven by redriving the Map Run
/// that started it, so this is what one reports instead of `REDRIVABLE`.
pub const REDRIVE_STATUS_REDRIVABLE_BY_MAP_RUN: &str = "REDRIVABLE_BY_MAP_RUN";

// The `redriveStatusReason` values below are the ones that member's own
// documentation lists verbatim, so the reason this emulator reports is a reason the
// service publishes rather than wording of its own.

/// "Execution is RUNNING and cannot be redriven."
pub const NOT_REDRIVABLE_RUNNING: &str = "Execution is RUNNING and cannot be redriven";

/// "Execution is SUCCEEDED and cannot be redriven."
pub const NOT_REDRIVABLE_SUCCEEDED: &str = "Execution is SUCCEEDED and cannot be redriven";

/// "State machine is in DELETING status."
pub const NOT_REDRIVABLE_DELETING: &str = "State machine is in DELETING status";

/// "Execution history event limit exceeded."
pub const NOT_REDRIVABLE_EVENT_LIMIT: &str = "Execution history event limit exceeded";

/// "Execution has exceeded the max execution time."
pub const NOT_REDRIVABLE_MAX_EXECUTION_TIME: &str =
    "Execution has exceeded the max execution time";

/// "Execution redrivable period exceeded."
pub const NOT_REDRIVABLE_PERIOD: &str = "Execution redrivable period exceeded";

/// How long after an execution completes it can still be redriven.
///
/// An AWS-documented duration, from `RedriveExecution`'s own documentation: "Your
/// workflow execution has not exceeded the redrivable period of 14 days. Redrivable
/// period refers to the time during which you can redrive a given execution. This
/// period starts from the day a state machine completes its execution."
pub const REDRIVABLE_PERIOD: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The longest an execution may be open and still be redrivable.
///
/// An AWS-documented duration, from the same list: "The workflow execution has not
/// exceeded the maximum open time of one year."
pub const MAX_OPEN_DURATION: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The history event count at which an execution stops being redrivable.
///
/// From the same list: "The execution event history count is less than 24,999.
/// Redriven executions append their event history to the existing event history."
pub const MAX_REDRIVABLE_HISTORY_EVENTS: usize = 24_999;

/// How many client tokens of successful redrives one execution remembers.
///
/// From `RedriveExecution`'s `clientToken` documentation: "The API will return
/// idempotent responses for the last 10 client tokens used to successfully redrive
/// the execution."
pub const MAX_REDRIVE_CLIENT_TOKENS: usize = 10;

/// How long one of those client tokens keeps answering idempotently.
///
/// An AWS-documented duration, from the same member: "These client tokens are valid
/// for up to 15 minutes after they are first used."
pub const REDRIVE_CLIENT_TOKEN_TTL: Duration = Duration::from_secs(15 * 60);

/// The history event an execution's FIRST attempt opens with.
pub const EVENT_EXECUTION_STARTED: &str = "ExecutionStarted";

/// The history event a redrive opens with, before the re-run's own events:
/// `RedriveExecution` documents that "redriven executions append their event history
/// to the existing event history" and names "the ExecutionRedriven history event".
pub const EVENT_EXECUTION_REDRIVEN: &str = "ExecutionRedriven";

// ---------------------------------------------------------------------------
// Spans the clock owes
// ---------------------------------------------------------------------------

/// How long after `StartExecution` an execution's terminal status becomes due.
///
/// EMULATOR-CHOSEN, not an AWS duration: the service publishes no figure for how
/// long an execution takes, so this is the span this emulator holds a started
/// execution in `RUNNING` for. It is deliberately non-zero, so a caller that
/// describes an execution straight after starting it still observes `RUNNING` and
/// `StartExecution`'s documented same-name idempotency is observable.
pub const EXECUTION_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteStateMachine` the state machine is removed.
///
/// EMULATOR-CHOSEN, for the same reason: `DeleteStateMachine` documents only that
/// it "sets the state machine's status to DELETING and begins the deletion
/// process", with no published duration.
pub const DELETE_SETTLE_DELAY: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Activity tasks
//
// An activity is "a worker — a program — that you host": a `Task` state whose
// `Resource` is an activity ARN SCHEDULES a task, which stays outstanding until a
// worker claims it with `GetActivityTask` and answers it. Everything one needs is
// in [`ActivityTask`], keyed by the token the claim hands the worker.
// ---------------------------------------------------------------------------

/// How long `GetActivityTask` holds a poll open before answering with no task.
///
/// EMULATOR-CHOSEN, well inside the service's published maximum: "the maximum time
/// the service holds on to the request before responding is 60 seconds", and it
/// "responds as soon as a task becomes available", so a poll may answer at any point
/// inside that maximum. This is a third of it — long enough that a worker's poll is a
/// LONG poll rather than a busy loop, which is what the service's own guidance on
/// avoiding latency when polling asks of a worker, and far enough inside the 65
/// seconds the service asks a client to allow its socket that no answer arrives late.
pub const ACTIVITY_POLL_HOLD: Duration = Duration::from_secs(20);

/// How often a held poll looks again for a task.
///
/// EMULATOR-CHOSEN: short enough that a task scheduled while a worker is polling is
/// answered promptly, long enough that holding a poll is not a busy loop on the
/// store's lock. A task that is already there is answered by the look this operation
/// makes BEFORE it holds anything, so this is the delay of a task that arrives
/// mid-poll and never of one that is waiting.
pub const ACTIVITY_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The most workers that may hold a poll of ONE activity at the same time, refused
/// with the modelled `ActivityWorkerLimitExceeded`.
///
/// Emulator resource guard: each open poll holds a request for up to
/// [`ACTIVITY_POLL_HOLD`], so this bounds how many of them one activity can tie up.
/// It sits below the quota Step Functions publishes for open activity pollers per
/// activity ARN (1,000).
pub const MAX_ACTIVITY_POLLERS: usize = 100;

/// The most polls the WHOLE store may hold open at the same time, refused with the
/// same modelled error.
///
/// Emulator resource guard, for the reason every other cap here is on the whole
/// store: a per-activity cap alone is multiplied by the number of activities, and a
/// held poll is a request — and a connection — kept open for up to
/// [`ACTIVITY_POLL_HOLD`]. With this, the requests this operation holds are bounded
/// however many activities a caller creates.
pub const MAX_OPEN_ACTIVITY_POLLS: usize = 1_000;

/// The ASL error name an activity task that ran out of time fails its execution
/// with.
///
/// The Amazon States Language states it for both bounds a `Task` state declares:
/// "TimeoutSeconds ... If the task runs longer than the specified seconds, this
/// state fails with a States.Timeout error name", and "HeartbeatSeconds ... If more
/// time than the specified seconds elapses between heartbeats from the task, this
/// state fails with a States.Timeout error name."
pub const TASK_TIMEOUT_ERROR: &str = asl::TIMEOUT_ERROR;

/// The history event a `Task` state writes when a worker claims its task.
pub const EVENT_ACTIVITY_STARTED: &str = "ActivityStarted";

/// The history event an activity task that ran out of time writes.
pub const EVENT_ACTIVITY_TIMED_OUT: &str = "ActivityTimedOut";

/// The history event an activity task a worker reported as FAILED writes.
pub const EVENT_ACTIVITY_FAILED: &str = "ActivityFailed";

/// The history event a `Task` state writes when it schedules a task for its
/// activity, which a `Retry` writes again for each attempt it schedules.
pub const EVENT_ACTIVITY_SCHEDULED: &str = "ActivityScheduled";

/// The history event a worker's successful reply to an activity task writes.
pub const EVENT_ACTIVITY_SUCCEEDED: &str = "ActivitySucceeded";

/// The history event a `Task` state writes when the work its `Resource` names is
/// handed over, which a `Retry` writes again for each attempt it makes.
pub const EVENT_TASK_SCHEDULED: &str = "TaskScheduled";

/// The history event a `Task` state writes when that work starts.
pub const EVENT_TASK_STARTED: &str = "TaskStarted";

/// The history event a `Task` state writes when that work answered.
pub const EVENT_TASK_SUCCEEDED: &str = "TaskSucceeded";

/// The history event a `Task` state writes when that work failed.
pub const EVENT_TASK_FAILED: &str = "TaskFailed";

/// The history event a run-a-job (`.sync`) `Task` state writes when the call it makes
/// has STARTED the job, carrying "the response from a resource when a task has
/// started" — which is what names the job the task is then parked at.
pub const EVENT_TASK_SUBMITTED: &str = "TaskSubmitted";

/// The history event such a state writes when that call did NOT start a job: "details
/// about a task that failed to submit during an execution".
pub const EVENT_TASK_SUBMIT_FAILED: &str = "TaskSubmitFailed";

/// The history event a `Task` state writes when that work ran out of the time the
/// state allowed it, which the language reports under [`TASK_TIMEOUT_ERROR`].
pub const EVENT_TASK_TIMED_OUT: &str = "TaskTimedOut";

/// The history event a `Map` state writes when it starts iterating, carrying how many
/// items it is iterating; written again for each attempt a `Retry` makes.
pub const EVENT_MAP_STATE_STARTED: &str = "MapStateStarted";

/// The history event a `Map` state writes when every one of its iterations the state
/// needed finished.
pub const EVENT_MAP_STATE_SUCCEEDED: &str = "MapStateSucceeded";

/// The history event a `Map` state writes when more of its iterations failed than it
/// tolerates.
pub const EVENT_MAP_STATE_FAILED: &str = "MapStateFailed";

/// The history events ONE iteration of a `Map` state writes, each carrying the state's
/// name and the index of the item the iteration was handed.
pub const EVENT_MAP_ITERATION_STARTED: &str = "MapIterationStarted";
pub const EVENT_MAP_ITERATION_SUCCEEDED: &str = "MapIterationSucceeded";
pub const EVENT_MAP_ITERATION_FAILED: &str = "MapIterationFailed";
pub const EVENT_MAP_ITERATION_ABORTED: &str = "MapIterationAborted";

/// The history event a `Parallel` state writes when its branches start, written again
/// for each attempt a `Retry` makes.
pub const EVENT_PARALLEL_STATE_STARTED: &str = "ParallelStateStarted";

/// The history event a `Parallel` state writes when every branch completed.
pub const EVENT_PARALLEL_STATE_SUCCEEDED: &str = "ParallelStateSucceeded";

/// The history event a `Parallel` state writes when one of its branches failed.
pub const EVENT_PARALLEL_STATE_FAILED: &str = "ParallelStateFailed";

/// The history event an execution that one of its states failed ends on.
pub const EVENT_EXECUTION_FAILED: &str = "ExecutionFailed";

/// The history event an execution that reached its end ends on.
pub const EVENT_EXECUTION_SUCCEEDED: &str = "ExecutionSucceeded";

/// The history event a STOPPED execution ends on.
pub const EVENT_EXECUTION_ABORTED: &str = "ExecutionAborted";

/// The history event an execution that ran past its definition's `TimeoutSeconds`
/// ends on.
pub const EVENT_EXECUTION_TIMED_OUT: &str = "ExecutionTimedOut";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteStateMachine`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. `name`, `definition`, `roleArn`,
// `input`, `error`, `cause`, `traceHeader`, `maxResults` and `nextToken` are all
// bounded by the model, so the centralized walk refuses anything outside them
// before a handler runs.
//
// Each cap below bounds a COUNT of records, and a count alone does not bound the
// memory: every record carries caller-sized text (a definition up to the modelled
// 1 MiB, an input up to 256 KiB), so the PRODUCT of the counts and those sizes is
// what has to be bounded. [`MAX_STORE_BYTES`] is that bound: a create or a start
// that would take the store past it is refused with the operation's own modelled
// limit error, whatever the counts say. Nothing here depends on a value the caller
// chooses — in particular, the state-machine cap is on the WHOLE store, not per
// credential-scope region, so varying the signed region cannot mint a fresh quota.
// ---------------------------------------------------------------------------

/// The most bytes the store retains for every state machine and execution
/// together, refused with the operation's own modelled limit error.
///
/// Emulator resource guard, NOT an AWS quota. This is the bound on the PRODUCT of
/// the record counts below and the caller-sized text each record holds: with it,
/// the store's memory is bounded however the counts are distributed.
pub const MAX_STORE_BYTES: usize = 64 * 1024 * 1024;

/// The most state machines the WHOLE store holds, refused with the modelled
/// `StateMachineLimitExceeded`.
///
/// Emulator resource guard, NOT an AWS quota: Step Functions publishes 10,000
/// registered state machines per account per Region, and this guard sits below it
/// and counts across every region, so a caller that varies the region it signs for
/// cannot grow the store past this.
pub const MAX_STATE_MACHINES: usize = 1_000;

/// The most executions this emulator keeps for one state machine, refused with the
/// modelled `ExecutionLimitExceeded`.
///
/// Emulator resource guard, NOT an AWS quota (AWS publishes 1,000,000 open
/// executions, which no in-memory store should hold).
pub const MAX_EXECUTIONS_PER_STATE_MACHINE: usize = 1_000;

/// The most executions the WHOLE store holds, for the same reason as
/// [`MAX_STATE_MACHINES`]: a per-state-machine cap alone is multiplied by the
/// number of state machines.
pub const MAX_EXECUTIONS: usize = 10_000;

/// The most aliases one state machine may carry, refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// An AWS-PUBLISHED quota rather than an emulator guard, from
/// `CreateStateMachineAlias`'s own documentation: "You can create up to 100 aliases
/// for each state machine. You must delete unused aliases using the
/// DeleteStateMachineAlias API action."
pub const MAX_ALIASES_PER_STATE_MACHINE: usize = 100;

/// The fewest characters a state machine alias name may carry, from the modelled
/// `CharacterRestrictedName` length (1..80). See [`is_valid_alias_name`].
pub const MIN_ALIAS_NAME_CHARS: usize = 1;

/// The most characters a state machine alias name may carry, from the same modelled
/// length.
pub const MAX_ALIAS_NAME_CHARS: usize = 80;

/// The most activities the WHOLE store holds, refused with the modelled
/// `ActivityLimitExceeded`.
///
/// Emulator resource guard, NOT an AWS quota, and set for the same reason as
/// [`MAX_STATE_MACHINES`]: it counts across every region, so a caller that varies
/// the region it signs for cannot mint a fresh quota.
pub const MAX_ACTIVITIES: usize = 1_000;

/// The most versions one state machine keeps, refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// `PublishStateMachineVersion`'s own documentation: "You can publish up to 1000
/// versions for each state machine. You must manually delete unused versions using
/// the DeleteStateMachineVersion API action."
pub const MAX_VERSIONS_PER_STATE_MACHINE: usize = 1_000;

/// The `revisionId` a caller names to mean "this state machine has no revision ID
/// assigned yet".
///
/// From `PublishStateMachineVersion`'s `revisionId`: "To specify an initial revision
/// ID for a state machine with no revision ID assigned, specify the string INITIAL
/// for the revisionId parameter."
pub const INITIAL_REVISION_ID: &str = "INITIAL";

/// The most tags one state machine or activity may carry, refused with the modelled
/// `TooManyTags`. The AWS-wide per-resource tag limit.
pub const MAX_TAGS: usize = 50;

/// The text the service's `TooManyTags` refusal carries.
///
/// A FIXED service string, NOT a message rebuilt from the request: the recorded
/// Step Functions responses to a tag write past the limit carry exactly this text,
/// whatever the request's tag count, so echoing the count or the limit into it
/// would make the emulator's message vary where the service's does not.
pub const TOO_MANY_TAGS_MESSAGE: &str = "Too many tags provided.";

/// The most history events one execution keeps.
///
/// This is the interpreter's own ceiling, not a separate guess: a transition emits
/// the pair its state was entered and left with plus the lifecycle events of the work
/// between them — the handover, start and outcome of a `Task`'s work, the start and
/// outcome of a `Map`'s or a `Parallel`'s, the start and outcome of one `Map`
/// iteration — which is at most [`EVENTS_PER_TRANSITION`] per transition;
/// [`asl::MAX_STATE_TRANSITIONS`] bounds the transitions, and `ExecutionStarted` plus
/// one terminal event bracket them. Each event carries a state name, which
/// [`asl::MAX_STATE_NAME_CHARS`] bounds, so the history's BYTES are bounded too.
pub const MAX_HISTORY_EVENTS: usize = EVENTS_PER_TRANSITION * asl::MAX_STATE_TRANSITIONS + 2;

/// The most history events one state transition writes: the `<Type>StateEntered` and
/// `<Type>StateExited` pair, and the at most six lifecycle events the work between
/// them can write — a `Map` iteration's start and outcome, and, for the attempt that
/// iteration belongs to, the enclosing state's own start and outcome plus the
/// handover and start of a `Task`'s work.
const EVENTS_PER_TRANSITION: usize = 8;

/// What [`Execution::measure`] reserves for a stop that has not happened yet.
///
/// `StopExecution` writes the caller's `error` and `cause` onto a running execution
/// TWICE — as the record's own terminal values, and on the `ExecutionAborted` event,
/// which keeps what the stop reported however a later attempt moves the record — plus
/// that event itself. Reserving BOTH copies of their maxima (the modelled
/// `SensitiveError` and `SensitiveCause` lengths) while the execution is still running
/// means a re-measurement after the stop can only shrink the store's total, so
/// admission is the ONLY place the store grows and [`MAX_STORE_BYTES`] bounds what
/// is retained at every instant.
pub const STOP_RESERVE_BYTES: usize = 2 * (asl::MAX_ERROR_BYTES + asl::MAX_CAUSE_BYTES)
    + std::mem::size_of::<HistoryEvent>()
    + EVENT_EXECUTION_ABORTED.len();

/// Page size used when `maxResults` is absent or `0`.
///
/// From `ListExecutions`' and `ListStateMachines`' own `maxResults` documentation:
/// "The default is 100 and the maximum allowed page size is 1000. A value of 0 uses
/// the default."
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most bytes one of this service's generated ARNs holds, which is the maximum
/// the modelled `Arn` shape (len 1..256) carries: every ARN this emulator mints is
/// built from a state machine name, an execution name and a Map state label, each of
/// which the Amazon States Language and this API bound well inside it.
pub const MAX_ARN_BYTES: usize = 256;

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One stored state machine: every member `DescribeStateMachine` reports, plus the
/// fingerprint `CreateStateMachine`'s idempotency check compares and the instant a
/// started deletion becomes due.
#[derive(Debug, Clone)]
pub struct StateMachine {
    pub arn: String,
    pub name: String,
    pub definition: String,
    pub role_arn: String,
    pub sm_type: String,
    pub status: String,
    pub creation_date: SystemTime,
    pub update_date: SystemTime,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub tracing_configuration: Option<TracingConfiguration>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub revision_id: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    /// Whether the create that minted this state machine asked to publish a
    /// version. `CreateStateMachine`'s idempotency check reads `publish`, so the
    /// flag the create carried is part of what a repeat is compared against.
    pub publish: bool,
    /// The version ARN the create that minted this state machine returned, if it
    /// published one: an idempotent repeat answers the FIRST response, so it
    /// reports that same version rather than publishing a second one.
    pub published_version_arn: Option<String>,
    /// How many versions have been published of this state machine, so the next
    /// one takes the next number and a number is never reused.
    pub version_counter: u32,
    /// When `DeleteStateMachine`'s removal becomes due; `None` while `ACTIVE`.
    pub delete_due_at: Option<SystemTime>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`].
    ///
    /// Set by [`ComAmazonawsSfnAwsstepfunctionsState::insert_state_machine`] from
    /// [`StateMachine::measure`], and moved by the one operation that changes what a
    /// stored state machine holds:
    /// [`ComAmazonawsSfnAwsstepfunctionsState::merge_tags`] re-counts
    /// the tag set it wrote. A constructor may leave it `0`.
    pub retained_bytes: usize,
}

impl StateMachine {
    /// Whether `DeleteStateMachine` has already begun deleting this machine.
    pub fn is_deleting(&self) -> bool {
        self.delete_due_at.is_some()
    }

    /// The members `CreateStateMachine`'s idempotency check reads, rendered from
    /// this record's CURRENT configuration: see [`configuration_fingerprint`].
    ///
    /// Computed rather than stored, so a repeat is always compared against what the
    /// state machine holds now and no second copy can drift from the record.
    pub fn fingerprint(&self) -> String {
        configuration_fingerprint(
            &self.definition,
            &self.sm_type,
            self.logging_configuration.as_ref(),
            self.tracing_configuration.as_ref(),
            self.encryption_configuration.as_ref(),
            self.publish,
            self.description.as_deref(),
        )
    }

    /// The bytes this record would retain: its own layout plus every caller-sized
    /// string it holds. This is what a create is admitted against, so the store's
    /// memory is bounded by [`MAX_STORE_BYTES`] and not only by a record count.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.name.len()
            + self.definition.len()
            + self.role_arn.len()
            + self.sm_type.len()
            + self.status.len()
            + self.published_version_arn.as_ref().map_or(0, String::len)
            + self.revision_id.as_ref().map_or(0, String::len)
            + self.description.as_ref().map_or(0, String::len)
            + tags_bytes(&self.tags)
    }
}

/// One published version of a state machine: the configuration the state machine
/// held at the instant it was published, under its own qualified ARN
/// (`...:stateMachine:<name>:<version>`).
///
/// A version is immutable ("a version is a numbered snapshot of a state machine"),
/// so nothing here changes once it is stored; it disappears with the state machine
/// it belongs to.
#[derive(Debug, Clone)]
pub struct StateMachineVersion {
    pub arn: String,
    pub state_machine_arn: String,
    pub version: u32,
    pub name: String,
    pub definition: String,
    pub role_arn: String,
    pub sm_type: String,
    pub creation_date: SystemTime,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub tracing_configuration: Option<TracingConfiguration>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub revision_id: Option<String>,
    pub description: Option<String>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set
    /// by [`ComAmazonawsSfnAwsstepfunctionsState::publish_state_machine_version`].
    pub retained_bytes: usize,
}

impl StateMachineVersion {
    /// The bytes this record would retain; see [`StateMachine::measure`].
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.state_machine_arn.len()
            + self.name.len()
            + self.definition.len()
            + self.role_arn.len()
            + self.sm_type.len()
            + self.revision_id.as_ref().map_or(0, String::len)
            + self.description.as_ref().map_or(0, String::len)
    }
}

/// One stored state machine alias: "an alias for a state machine that points to one
/// or two versions of the same state machine", under its own qualified ARN
/// (`<stateMachineArn>:<aliasName>`).
///
/// The alias is a pointer, not a snapshot: it holds the routing configuration the
/// caller gave, so the versions it routes to are resolved through their own records
/// and an alias never answers for a configuration a version no longer holds.
#[derive(Debug, Clone)]
pub struct StateMachineAlias {
    pub arn: String,
    pub state_machine_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub routing_configuration: Vec<RoutingConfigurationListItem>,
    pub creation_date: SystemTime,
    /// "The date the state machine alias was last updated. For a newly created
    /// state machine, this is the same as the creation date."
    pub update_date: SystemTime,
    /// Which create minted this alias, counted by the store: `1` for the first alias
    /// it ever accepted, `2` for the next, and so on. Stamped by
    /// [`ComAmazonawsSfnAwsstepfunctionsState::insert_state_machine_alias`].
    ///
    /// This is what orders a listing of one state machine's aliases. The stored
    /// instant cannot do it alone: two creates that land inside one clock tick carry
    /// the same instant (and the wire reports it in whole milliseconds), so an order
    /// keyed on it is not total, while the sequence the store hands out is — and it is
    /// the order the creates were accepted in however coarse the clock is. Nothing but
    /// a create sets it, so an alias keeps its place when it is updated.
    pub creation_sequence: u64,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set
    /// by [`ComAmazonawsSfnAwsstepfunctionsState::insert_state_machine_alias`].
    pub retained_bytes: usize,
}

impl StateMachineAlias {
    /// The bytes this record would retain; see [`StateMachine::measure`].
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.state_machine_arn.len()
            + self.name.len()
            + self.description.as_ref().map_or(0, String::len)
            + routing_bytes(&self.routing_configuration)
    }

    /// Whether a create carrying `description` and `routing_configuration` is the
    /// SAME request as the one that minted this alias.
    ///
    /// "CreateStateMachineAlias is an idempotent API. Step Functions bases the
    /// idempotency check on the stateMachineArn, description, name, and
    /// routingConfiguration parameters." The ARN this record is keyed by is a total
    /// function of the state machine and the name, so what is left to compare is the
    /// description and the routing configuration — the latter entry by entry, since
    /// two configurations that give one version a different weight are different
    /// parameters.
    pub fn was_created_with(
        &self,
        description: Option<&str>,
        routing_configuration: &[RoutingConfigurationListItem],
    ) -> bool {
        self.description.as_deref() == description
            && same_routing(&self.routing_configuration, routing_configuration)
    }

    /// Whether this alias's routing configuration names the version `version_arn`.
    ///
    /// The ONE place that question is answered, so every operation that asks it —
    /// listing the aliases a version is reached through, and refusing to delete a
    /// version an alias still points at — reads the same records the same way.
    pub fn routes_to(&self, version_arn: &str) -> bool {
        self.routing_configuration
            .iter()
            .any(|item| item.state_machine_version_arn == version_arn)
    }
}

/// Whether two routing configurations name the same versions at the same weights, in
/// the same order.
fn same_routing(
    left: &[RoutingConfigurationListItem],
    right: &[RoutingConfigurationListItem],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.state_machine_version_arn == right.state_machine_version_arn
                && left.weight == right.weight
        })
}

/// The bytes one routing configuration holds.
fn routing_bytes(routing: &[RoutingConfigurationListItem]) -> usize {
    routing
        .iter()
        .map(|item| {
            std::mem::size_of::<RoutingConfigurationListItem>()
                + item.state_machine_version_arn.len()
        })
        .sum()
}

/// What a state machine ARN REFERS to: the state machine it resolves to, and the
/// version or alias its qualifier named.
///
/// A state machine ARN may be qualified with "either a version or an alias", and
/// both qualified forms are references to the state machine rather than resources
/// of their own: `StartExecution`'s `stateMachineArn` "accepts one of the following
/// inputs: an unqualified state machine ARN [...] a state machine version ARN [...]
/// a state machine alias ARN", and `ListExecutions` "can also return a list of
/// executions associated with a specific alias or version, by specifying an alias
/// ARN or a version ARN". So the qualifier is resolved to the state machine that
/// owns it, and the version or alias it named is what the call is SCOPED to.
#[derive(Debug, Clone)]
pub struct StateMachineReference {
    /// The unqualified ARN of the state machine this reference resolves to.
    pub state_machine_arn: String,
    /// The version ARN, when the qualifier named a published version.
    pub version_arn: Option<String>,
    /// The alias ARN, when the qualifier named an alias.
    pub alias_arn: Option<String>,
}

/// What a start operation needs to run ONE execution: the state machine the
/// request's ARN resolved to, the configuration that execution interprets, and the
/// version and alias the execution is associated with.
///
/// A version is "a numbered snapshot of a state machine", so an execution started
/// through a version ARN — or through an alias that routed to a version — runs the
/// definition and role that version holds, not whatever the state machine has been
/// updated to since.
#[derive(Debug, Clone)]
pub struct RunTarget {
    /// The unqualified ARN of the state machine being run.
    pub state_machine_arn: String,
    /// That state machine's name, which its execution ARNs carry.
    pub name: String,
    /// `STANDARD` or `EXPRESS`.
    pub sm_type: String,
    /// The configuration this run interprets, recorded onto the execution as
    /// [`Execution::ran`].
    pub configuration: RanConfiguration,
    /// The version this run is attributed to, if any — see
    /// [`Execution::state_machine_version_arn`].
    pub version_arn: Option<String>,
    /// The alias this run was started through, if any — see
    /// [`Execution::state_machine_alias_arn`].
    pub alias_arn: Option<String>,
}

/// The state machine configuration ONE execution runs, as it stood when that
/// execution started.
///
/// An execution is PINNED to it: an update to the state machine "will not affect
/// currently running executions", which keep "the previous definition and roleArn",
/// so what an execution runs is recorded on the execution itself rather than read
/// back from a record that has since moved on. That is what
/// `DescribeStateMachineForExecution` reports, and what a redrive — which "uses the
/// same state machine definition [...] as the original execution attempt" — re-runs.
#[derive(Debug, Clone)]
pub struct RanConfiguration {
    /// The Amazon States Language definition this execution interprets.
    pub definition: String,
    /// The execution role it runs under.
    pub role_arn: String,
    /// The revision of the state machine this configuration IS, which is what lets a
    /// caller "compare the revision of a state machine with the configuration of the
    /// state machine used for executions"; `None` while nothing has revised it.
    pub revision_id: Option<String>,
    /// The instant this configuration was written: the state machine's own
    /// `update_date` when the start resolved the state machine itself, and the
    /// version's creation date when it resolved a version — a version being a
    /// snapshot nothing updates after the publish that took it.
    pub update_date: SystemTime,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub tracing_configuration: Option<TracingConfiguration>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
}

impl RanConfiguration {
    /// The configuration the state machine `machine` currently holds.
    pub fn of_state_machine(machine: &StateMachine) -> Self {
        Self {
            definition: machine.definition.clone(),
            role_arn: machine.role_arn.clone(),
            revision_id: machine.revision_id.clone(),
            update_date: machine.update_date,
            logging_configuration: machine.logging_configuration.clone(),
            tracing_configuration: machine.tracing_configuration.clone(),
            encryption_configuration: machine.encryption_configuration.clone(),
        }
    }

    /// The configuration the published version `version` holds.
    pub fn of_version(version: &StateMachineVersion) -> Self {
        Self {
            definition: version.definition.clone(),
            role_arn: version.role_arn.clone(),
            revision_id: version.revision_id.clone(),
            update_date: version.creation_date,
            logging_configuration: version.logging_configuration.clone(),
            tracing_configuration: version.tracing_configuration.clone(),
            encryption_configuration: version.encryption_configuration.clone(),
        }
    }

    /// The caller-sized bytes this snapshot retains, for the record carrying it; the
    /// layout itself is counted by that record's own `size_of`.
    pub fn measure(&self) -> usize {
        self.definition.len()
            + self.role_arn.len()
            + self.revision_id.as_ref().map_or(0, String::len)
    }
}

/// One stored activity: every member `DescribeActivity` reports, plus the tag set
/// the tagging calls read and write.
///
/// An activity is "a task that you write in any programming language and host on any
/// machine": the service holds its identity alone — no definition, no status and no
/// transition the clock owes — so this record never changes after it is stored,
/// except for its tags.
#[derive(Debug, Clone)]
pub struct Activity {
    pub arn: String,
    pub name: String,
    pub creation_date: SystemTime,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub tags: Vec<Tag>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set
    /// by [`ComAmazonawsSfnAwsstepfunctionsState::insert_activity`] and moved by a
    /// tag write. A constructor may leave it `0`.
    pub retained_bytes: usize,
}

impl Activity {
    /// The bytes this record would retain; see [`StateMachine::measure`].
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>() + self.arn.len() + self.name.len() + tags_bytes(&self.tags)
    }
}

/// One activity task an execution scheduled: outstanding from the instant the walk
/// reached the `Task` state to the instant a worker's reply — or the clock — closes
/// it.
///
/// The record is keyed by the `taskToken` a claim hands the worker, because that is
/// the only name the worker has for it: "a token that identifies the scheduled task.
/// This token must be copied and included in subsequent calls to SendTaskHeartbeat,
/// SendTaskSuccess or SendTaskFailure."
#[derive(Debug, Clone)]
pub struct ActivityTask {
    pub token: String,
    /// The activity this task is queued under, which is the `activityArn` a worker
    /// polls with.
    pub activity_arn: String,
    /// The execution waiting on it, and the state it is waiting at, so a reply
    /// resumes exactly that state of exactly that execution.
    pub execution_arn: String,
    pub state_name: String,
    /// "The string that contains the JSON input data for the task": the effective
    /// input the scheduling state computed.
    pub input: String,
    /// The RAW input of the state that scheduled it, before `InputPath` and the
    /// `Parameters` template. The state's `ResultPath` combines a worker's reply with
    /// its raw input, so answering the task needs the document the state was entered
    /// with and not only the one the worker was handed.
    pub state_input: String,
    /// When a poll may first claim it: the instant the execution's walk REACHED the
    /// state, which is its start plus every `Wait` before it. Nothing sleeps for
    /// those waits, so this is what makes them observable to a worker.
    pub available_at: SystemTime,
    /// The `workerName` of the poll that claimed it, when one has — "an arbitrary
    /// name in order to identify the worker that the task is assigned to".
    pub worker_name: Option<String>,
    /// When it was claimed; `None` while it is still queued.
    pub started_at: Option<SystemTime>,
    /// When the heartbeat window last restarted: the claim, and then each heartbeat.
    /// `None` while the task is queued, because a task nobody holds misses no
    /// heartbeat.
    pub heartbeat_at: Option<SystemTime>,
    /// The scheduling state's `TimeoutSeconds` and `HeartbeatSeconds`, in seconds;
    /// `None` where the state declares none, which is a task with no such bound.
    pub timeout_seconds: Option<u64>,
    pub heartbeat_seconds: Option<u64>,
    /// How many attempts each retrier of the scheduling state's `Retry` has already
    /// spent on this task, in the order the state declares them.
    ///
    /// Empty for the first attempt, which has spent none. A retrier's `MaxAttempts` is
    /// its own ("MaxAttempts ... default 3"), so the counts are per retrier rather than
    /// per task, and they travel with the task a retry re-schedules so the attempt
    /// after it is the next one of that retrier and not the first again.
    pub retries_taken: Vec<u64>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set
    /// by [`ComAmazonawsSfnAwsstepfunctionsState::schedule_activity_task`] exactly as
    /// [`StateMachine::retained_bytes`] is.
    pub retained_bytes: usize,
}

impl ActivityTask {
    /// Whether no worker holds this task yet, so a poll may claim it.
    pub fn is_queued(&self) -> bool {
        self.started_at.is_none()
    }

    /// When this task's `TimeoutSeconds` makes it due to close.
    ///
    /// Measured from the instant a worker CLAIMED it, which is when an activity task's
    /// clock starts: "for Activities, the count begins when GetActivityTask receives a
    /// token and ActivityStarted is logged in the execution event history." A task no
    /// worker holds is therefore bounded by nothing — there is no run to be longer than
    /// the member allows — and stays outstanding until one claims it.
    fn timeout_due_at(&self) -> Option<SystemTime> {
        let since = self.started_at?;
        self.timeout_seconds
            .map(|seconds| after(since, Duration::from_secs(seconds)))
    }

    /// When this task's `HeartbeatSeconds` makes it due to close: that span after the
    /// last heartbeat, which the claim starts. A task no worker holds has no heartbeat
    /// window open.
    fn heartbeat_due_at(&self) -> Option<SystemTime> {
        let since = self.heartbeat_at?;
        self.heartbeat_seconds
            .map(|seconds| after(since, Duration::from_secs(seconds)))
    }

    /// The instant the clock closes this task at: whichever of its two bounds comes
    /// first, or `None` for a task the state bounded neither way, which stays
    /// outstanding until a worker answers it.
    pub fn closes_at(&self) -> Option<SystemTime> {
        match (self.timeout_due_at(), self.heartbeat_due_at()) {
            (Some(timeout), Some(heartbeat)) => Some(timeout.min(heartbeat)),
            (timeout, heartbeat) => timeout.or(heartbeat),
        }
    }

    /// Why the clock closes this task: its heartbeat window rather than the whole
    /// task's timeout. Reads the same two instants [`ActivityTask::closes_at`] chose
    /// between, so the reason cannot disagree with the instant.
    fn closed_by_heartbeat(&self) -> bool {
        match (self.heartbeat_due_at(), self.timeout_due_at()) {
            (Some(heartbeat), Some(timeout)) => heartbeat <= timeout,
            (heartbeat, _) => heartbeat.is_some(),
        }
    }

    /// The bytes this record would retain; see [`StateMachine::measure`].
    ///
    /// The `workerName` a later claim may write is RESERVED rather than counted (see
    /// [`WORKER_NAME_RESERVE_BYTES`]), so a claim moves nothing: the bytes a task
    /// retains are fixed when it is scheduled, which is when the store admitted them.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.token.len()
            + self.activity_arn.len()
            + self.execution_arn.len()
            + self.state_name.len()
            + self.input.len()
            + self.state_input.len()
            + self.retries_taken.len() * std::mem::size_of::<u64>()
            + WORKER_NAME_RESERVE_BYTES
    }
}

/// What a task's record reserves for the `workerName` a claim may write: the maximum
/// the modelled `Name` shape carries (len 1..80), reserved when the task is
/// scheduled for the reason a running execution reserves what a stop may write (see
/// [`STOP_RESERVE_BYTES`]).
const WORKER_NAME_RESERVE_BYTES: usize = 80;

/// The activity task a planned run scheduled, with the walk's offsets turned into
/// instants.
///
/// This is what [`plan_run`] reports for a run that SUSPENDED, and what
/// [`ComAmazonawsSfnAwsstepfunctionsState::schedule_activity_task`] stores once the
/// execution it belongs to has been admitted — the token is minted there, so no two
/// tasks can be handed the same one.
#[derive(Debug, Clone)]
pub struct PlannedActivityTask {
    pub activity_arn: String,
    pub state_name: String,
    pub input: String,
    /// The raw input of the state that scheduled it; see [`ActivityTask::state_input`].
    pub state_input: String,
    pub available_at: SystemTime,
    pub timeout_seconds: Option<u64>,
    pub heartbeat_seconds: Option<u64>,
}

impl PlannedActivityTask {
    /// The bytes the task this plans will retain, so the execution that schedules it
    /// is admitted against its own bytes AND these: admission stays the only place
    /// the store grows.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<ActivityTask>()
            + TASK_TOKEN_CHARS
            + WORKER_NAME_RESERVE_BYTES
            + ACTIVITY_TASK_EVENT_RESERVE_BYTES
            + self.activity_arn.len()
            + self.state_name.len()
            + self.input.len()
            + self.state_input.len()
    }
}

/// The characters a minted `taskToken` carries, reserved by
/// [`PlannedActivityTask::measure`] before the token itself is minted: the base64 of
/// one version-4 UUID, the separator and an activity ARN (see [`task_token`]), whose
/// own maximum is [`MAX_ARN_BYTES`]. Base64 carries three bytes in four characters,
/// and pads to a multiple of four.
const TASK_TOKEN_CHARS: usize = 4 * (36 + 1 + MAX_ARN_BYTES).div_ceil(3) + 4;

/// What a scheduled task reserves for the history events its own life appends to the
/// execution waiting on it: the `ActivityStarted` a claim writes, and the two a
/// closure writes — whether the clock closed it or a worker's reply did. Reserved
/// where the task is admitted, so those writes cannot take the store past
/// [`MAX_STORE_BYTES`] either.
const ACTIVITY_TASK_EVENT_RESERVE_BYTES: usize =
    3 * (std::mem::size_of::<HistoryEvent>() + "ActivityTimedOut".len());

/// One job an execution is PARKED at: the work a `Task` state's run-a-job (`.sync`)
/// integration started, which the task is not finished until the invoked service
/// reports finished.
///
/// Keyed in the store by the execution's own ARN, because a walk parks at one state
/// at a time: the state that started the job is where the execution resumes, with the
/// raw input that state was entered with ([`JobTask::state_input`]).
#[derive(Debug, Clone)]
pub struct JobTask {
    pub execution_arn: String,
    /// The state whose integration started the job.
    pub state_name: String,
    /// The RAW input of that state, which its `ResultPath` combines the job's report
    /// with once the job has ended.
    pub state_input: String,
    /// The integration the job belongs to, and the service whose API reports and
    /// stops it.
    pub job: &'static integration::RunAJob,
    pub api: &'static integration::Api,
    /// Whether the `Resource` asked for the pattern's second revision.
    pub parsed_output: bool,
    /// The job's own identifier, as the call that started it reported it.
    pub job_id: String,
    /// What the call that STARTED the job said it belongs to, which the calls that
    /// report and stop it carry beside the identifier for an integration whose job
    /// that identifier does not name on its own
    /// ([`integration::RunAJob::names_job_with`]).
    pub naming: integration::JobNaming,
    /// The Region the calls that report and stop the job are made in: the
    /// execution's own, which is the Region its state machine's ARN names.
    pub region: String,
    /// The instant the state started the job, which the state's `TimeoutSeconds` —
    /// and this emulator's own ceiling on an unfinished job — are measured from.
    pub parked_at: SystemTime,
    /// `TimeoutSeconds` of the state that started it, when it declares one.
    pub timeout_seconds: Option<u64>,
    /// The bytes the store counts for this record, as every other record's are.
    pub retained_bytes: usize,
}

impl JobTask {
    /// When this job must have finished by, after which the task that started it has
    /// run out of the time it was allowed.
    ///
    /// The state's own `TimeoutSeconds` when it declares one — "if the task runs
    /// longer than the specified seconds, this state fails with a States.Timeout
    /// error name", and for this pattern the task runs for as long as its job does —
    /// else this emulator's own ceiling on how long it polls one job
    /// ([`MAX_PARKED_JOB_DURATION`]).
    pub fn times_out_at(&self) -> SystemTime {
        let allowed = self
            .timeout_seconds
            .map_or(MAX_PARKED_JOB_DURATION, Duration::from_secs);
        after(self.parked_at, allowed.min(MAX_PARKED_JOB_DURATION))
    }
}

/// The bytes a job's naming holds, counted as every other stored text is: the member
/// names are this crate's own constants, so what a store retains for one is the text
/// the starting request named the job with.
fn naming_bytes(naming: &integration::JobNaming) -> usize {
    naming
        .iter()
        .map(|(member, value)| member.len() + value.len())
        .sum()
}

/// The job a planned run parked at, ready for the store to record: this is
/// [`asl::ParkedJob`] with the walk's offset turned into an instant against the
/// execution's own start.
#[derive(Debug, Clone)]
pub struct PlannedJob {
    pub state_name: String,
    pub state_input: String,
    pub job: &'static integration::RunAJob,
    pub api: &'static integration::Api,
    pub parsed_output: bool,
    pub job_id: String,
    pub naming: integration::JobNaming,
    pub parked_at: SystemTime,
    pub timeout_seconds: Option<u64>,
}

impl PlannedJob {
    /// The bytes the record this will become retains, so an execution is admitted
    /// against the whole of what its start adds to the store.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<JobTask>()
            + MAX_ARN_BYTES
            + self.state_name.len()
            + self.state_input.len()
            + self.job_id.len()
            + naming_bytes(&self.naming)
            // The two history events resolving the job appends to the execution
            // waiting at it: the one that reports the job's outcome and the one the
            // execution ends on.
            + 2 * (std::mem::size_of::<HistoryEvent>()
                + EVENT_TASK_FAILED.len()
                + asl::MAX_ERROR_BYTES
                + asl::MAX_CAUSE_BYTES)
    }
}

/// The longest this emulator polls ONE job of a run-a-job integration before the task
/// that started it reports the Amazon States Language's own timeout error.
///
/// Emulator bound, NOT an AWS limit: Step Functions polls a run-a-job task's job for
/// as long as the task's own `TimeoutSeconds` allows, and a task that declares none
/// has no bound at all. A job served inside this process is a resource this process
/// holds — a parked execution and the task polling it — so one that never reaches a
/// terminal status is let go of here rather than polled forever. It is the ceiling on
/// a declared `TimeoutSeconds` too, for the same reason.
pub const MAX_PARKED_JOB_DURATION: Duration = Duration::from_secs(300);

/// How often a parked job is asked whether it has finished.
///
/// Emulator interval: the job is served in THIS process, so its own reporting call
/// answers in microseconds, and the interval is what decides how soon after a job
/// ends the execution waiting at it carries on. It is the same order as
/// [`EXECUTION_SETTLE_DELAY`], so a parked execution is not reported finished sooner
/// than an unparked one would be.
pub const PARKED_JOB_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The bytes a tag set holds, so a tag write can move the store's total by what it
/// changed instead of re-measuring a record whose definition is up to 1 MiB.
fn tags_bytes(tags: &[Tag]) -> usize {    tags.iter()
        .map(|tag| {
            std::mem::size_of::<Tag>()
                + tag.key.as_ref().map_or(0, String::len)
                + tag.value.as_ref().map_or(0, String::len)
        })
        .sum()
}

/// One event of an execution's history, appended as the interpreter runs so a later
/// `GetExecutionHistory` reads events that really happened rather than fabricating
/// them.
#[derive(Debug, Clone)]
pub struct HistoryEvent {
    pub id: i64,
    pub previous_event_id: i64,
    pub event_type: String,
    pub timestamp: SystemTime,
    pub state_name: Option<String>,
    /// What the call that WROTE this event knew about it, which is what
    /// `GetExecutionHistory` reports as that event type's own detail member.
    pub details: EventDetails,
}

/// The `previousEventId` that means "no previous event".
///
/// `EventId` is modelled with a default of `0` and events are "numbered
/// sequentially, starting at one", so `0` names no event at all.
pub const NO_PREVIOUS_EVENT: i64 = 0;

/// The `previousEventId` an event written after the event `(previous_id,
/// previous_type)` reports.
///
/// Every event names the event before it, EXCEPT one written after an execution's
/// own `ExecutionStarted`: the start is the execution beginning rather than a step
/// of its walk, so the first event written after it has no predecessor and reports
/// [`NO_PREVIOUS_EVENT`], exactly as the `ExecutionStarted` event itself does.
pub fn follows_event(previous_id: i64, previous_type: &str) -> i64 {
    if previous_type == EVENT_EXECUTION_STARTED {
        NO_PREVIOUS_EVENT
    } else {
        previous_id
    }
}

/// What one history event carries besides its identity.
///
/// Every member is a value the call that wrote the event already had in hand — the
/// activity a task was scheduled for and the input it was scheduled with, the worker a
/// claim handed it to, the output a reply carried, the error and cause a failure
/// reported — so the history reports an event's detail members from the record instead
/// of leaving them unset. A member no writer knew stays `None`, because a detail this
/// emulator does not record is not reported at all.
#[derive(Debug, Clone, Default)]
pub struct EventDetails {
    /// The resource the event's work names: the activity ARN of a scheduled task, or
    /// the action a `Task` state's integration called.
    pub resource: Option<String>,
    /// The kind of resource that action belongs to, which a `Task` state's own events
    /// report beside it.
    pub resource_type: Option<String>,
    /// The input that work was handed.
    pub input: Option<String>,
    /// The output it answered.
    pub output: Option<String>,
    /// The `workerName` of the poll that claimed it.
    pub worker_name: Option<String>,
    /// The error and cause a failure reported.
    pub error: Option<String>,
    pub cause: Option<String>,
    /// The bounds the scheduling state declared for it.
    pub timeout_seconds: Option<u64>,
    pub heartbeat_seconds: Option<u64>,
    /// How many times the execution had been redriven when the event was written,
    /// which is what an `ExecutionRedriven` event reports. The record's own count
    /// moves again at the NEXT redrive, so the count belongs to the event rather
    /// than being read back off the execution.
    pub redrive_count: Option<i32>,
    /// The index of the item one `Map` iteration was handed.
    pub index: Option<i32>,
    /// How many items a `Map` state is iterating.
    pub length: Option<i32>,
}

impl EventDetails {
    /// The bytes this record holds beyond its own size.
    fn measure(&self) -> usize {
        let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
        text(&self.resource)
            + text(&self.resource_type)
            + text(&self.input)
            + text(&self.output)
            + text(&self.worker_name)
            + text(&self.error)
            + text(&self.cause)
    }

    /// The detail an activity task's event carries, from the task itself.
    fn of_scheduled_task(task: &ActivityTask) -> Self {
        Self {
            resource: Some(task.activity_arn.clone()),
            input: Some(task.input.clone()),
            timeout_seconds: task.timeout_seconds,
            heartbeat_seconds: task.heartbeat_seconds,
            ..Self::default()
        }
    }

    /// The detail a failure's event carries.
    fn of_failure(error: Option<String>, cause: Option<String>) -> Self {
        Self {
            error,
            cause,
            ..Self::default()
        }
    }

    /// The detail the TERMINAL event of one attempt carries: what that attempt ended
    /// with.
    ///
    /// An execution's own `output`, `error` and `cause` are those of its LATEST
    /// attempt — a redrive clears them and the re-run computes its own — while the
    /// event an earlier attempt ended on stays in the history, so the values it
    /// reports are kept on the event itself.
    fn of_outcome(
        output: Option<String>,
        error: Option<String>,
        cause: Option<String>,
    ) -> Self {
        Self {
            output,
            error,
            cause,
            ..Self::default()
        }
    }
}

/// What the store already holding an execution under the ARN a `StartExecution`
/// would mint means for that request
/// ([`ComAmazonawsSfnAwsstepfunctionsState::started_already`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedAlready {
    /// The documented idempotent hit: the first request's response is answered
    /// again, carrying the execution's own `startDate`, and nothing is run a second
    /// time.
    Idempotent { start_date: SystemTime },
    /// Everything else: the execution name is in use, which this operation's
    /// modelled `ExecutionAlreadyExists` reports.
    NameTaken,
}

/// The terminal result [`asl::run`] already computed for an execution, owed to the
/// clock: it becomes the execution's reported status at `due_at` and not before.
#[derive(Debug, Clone)]
pub struct PendingOutcome {
    pub due_at: SystemTime,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub events: Vec<HistoryEvent>,
}

impl PendingOutcome {
    /// The bytes this outcome holds, for [`Execution::measure`].
    fn measure(&self) -> usize {
        self.status.len()
            + self.output.as_ref().map_or(0, String::len)
            + self.error.as_ref().map_or(0, String::len)
            + self.cause.as_ref().map_or(0, String::len)
            + history_bytes(&self.events)
    }
}

/// The bytes a run of history events holds.
fn history_bytes(events: &[HistoryEvent]) -> usize {
    events
        .iter()
        .map(|event| {
            std::mem::size_of::<HistoryEvent>()
                + event.event_type.len()
                + event.state_name.as_ref().map_or(0, String::len)
                + event.details.measure()
        })
        .sum()
}

/// The bytes one execution's remembered redrive client tokens hold.
fn redrive_tokens_bytes(tokens: &[RedriveToken]) -> usize {
    tokens
        .iter()
        .map(|remembered| std::mem::size_of::<RedriveToken>() + remembered.token.len())
        .sum()
}

/// One client token a successful `RedriveExecution` was performed under, with the
/// `redriveDate` that call answered.
///
/// From `RedriveExecution`'s `clientToken` documentation: "The API will return
/// idempotent responses for the last [`MAX_REDRIVE_CLIENT_TOKENS`] client tokens
/// used to successfully redrive the execution. These client tokens are valid for up
/// to [`REDRIVE_CLIENT_TOKEN_TTL`] after they are first used."
#[derive(Debug, Clone)]
pub struct RedriveToken {
    pub token: String,
    /// The instant the redrive this token performed happened — both the response it
    /// keeps answering and the start of its validity.
    pub redrive_date: SystemTime,
}

/// One stored execution: every member `DescribeExecution` and `ListExecutions`
/// report.
///
/// Both the execution a `StartExecution` began and the CHILD workflow execution one
/// item of a Distributed Map state ran as are this one record: a child is an
/// execution in its own right, reached by its own ARN, and [`Execution::map_run_arn`]
/// is what tells the two apart.
#[derive(Debug, Clone)]
pub struct Execution {
    pub arn: String,
    pub state_machine_arn: String,
    pub name: String,
    pub status: String,
    pub start_date: SystemTime,
    pub stop_date: Option<SystemTime>,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub trace_header: Option<String>,
    /// The state machine VERSION this execution runs, when the ARN it was started
    /// with named a version or an alias that routed to one; `None` for an execution
    /// started with an unqualified ARN, which "Step Functions doesn't associate [...]
    /// with a version".
    pub state_machine_version_arn: Option<String>,
    /// The state machine ALIAS this execution was started through, when the ARN it
    /// was started with named one: "Step Functions associates executions that you
    /// start with an alias ARN with that alias and the state machine version used
    /// for that execution". `None` for an execution started with a version ARN,
    /// which it "doesn't associate [...] with any aliases that point to that
    /// version", and for one started with an unqualified ARN.
    pub state_machine_alias_arn: Option<String>,
    /// The state machine configuration this execution RUNS, recorded by the start
    /// that resolved it.
    ///
    /// An execution is pinned to it — "all currently running executions will
    /// continue to use the previous definition and roleArn" when the state machine is
    /// updated — so this is what `DescribeStateMachineForExecution` reports and what
    /// a redrive re-runs, however the state machine (or the version it was published
    /// from) has moved since.
    ///
    /// SHARED: the child workflow executions a Distributed Map state of this
    /// execution started run the same state machine this one does, so each of them
    /// holds this very snapshot rather than a copy of a definition the model bounds
    /// at 1 MiB. That is why only the record that does not belong to a Map Run counts
    /// its bytes in [`Execution::measure`].
    pub ran: std::sync::Arc<RanConfiguration>,
    /// The Map Run that started this execution, for a CHILD workflow execution one of
    /// a Distributed Map state's items ran as; `None` for an execution a
    /// `StartExecution` began.
    ///
    /// It is what `ListExecutions` enumerates a Map Run's children by, and what
    /// `DescribeExecution` reports in `mapRunArn`.
    pub map_run_arn: Option<String>,
    /// "The total number of items processed in a child workflow execution", for a
    /// child; `None` for an execution that is not one, which processes no items of a
    /// Map Run.
    pub item_count: Option<i32>,
    pub redrive_count: i32,
    /// The instant the last successful `RedriveExecution` redrove this execution;
    /// `None` until one has, which is what `redriveDate`'s own documentation says it
    /// reports ("If you have not yet redriven an execution, the redriveDate is
    /// null").
    pub redrive_date: Option<SystemTime>,
    /// The client tokens the last [`MAX_REDRIVE_CLIENT_TOKENS`] successful redrives
    /// carried, oldest first, so a repeat of one of them answers what it answered
    /// the first time instead of redriving a second time.
    pub redrive_tokens: Vec<RedriveToken>,
    pub history: Vec<HistoryEvent>,
    /// The step a `RedriveExecution` would continue this execution from: the state the
    /// current attempt stopped at and the raw input it was entered with — the state it
    /// was waiting at, or the state of the definition its walk failed at.
    ///
    /// `None` for an execution no attempt has stopped at a step of — one that is still
    /// running, and one that could not be run at all — which a redrive then walks from
    /// the definition's `StartAt`. Cleared by the redrive that consumes it, which
    /// records the next attempt's own step if that attempt stops at one.
    pub redrive_step: Option<asl::RedriveStep>,
    /// The outcome the interpreter computed, until the clock makes it due.
    pub pending: Option<PendingOutcome>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set
    /// by [`ComAmazonawsSfnAwsstepfunctionsState::insert_execution`] exactly as
    /// [`StateMachine::retained_bytes`] is, and re-measured by any operation that
    /// changes what the record holds (`StopExecution` writes a caller-sized `error`
    /// and `cause`), so the store's total always matches what is retained.
    ///
    /// The measurement RESERVES what a later `StopExecution` may add (see
    /// [`STOP_RESERVE_BYTES`]), so a re-measurement can only ever shrink the total:
    /// admission is the only place the store can grow, and the ceiling therefore
    /// bounds what is retained at every instant, not only at admission.
    pub retained_bytes: usize,
    /// How far a read of this execution's history has HANDED cursors out, one entry
    /// per read ([`HistoryRead`]) and at most [`MAX_HISTORY_PAGE_CURSORS`] of them.
    ///
    /// A `nextToken` is a handle this service MINTS, as an activity task's token is,
    /// so what it was minted for is remembered rather than guessed at from the
    /// request: "Make the call again using the returned token to retrieve the next
    /// page. Keep all other arguments unchanged", which is what makes a cursor
    /// minted for another execution, the other sort order or another page size this
    /// read's `InvalidToken` rather than a page of it. The cursor cannot carry that
    /// binding itself: the recorded responses show it as the position alone.
    ///
    /// One entry per read, holding the FURTHEST position that read has handed a
    /// cursor out for, is all it takes: a read's pages begin at the multiples of its
    /// own page size, so the cursors at or before that position are exactly the ones
    /// it issued. Two callers walking the same read therefore share one entry and
    /// neither can displace the other's cursor, and a cursor stays good for as long
    /// as the record does.
    ///
    /// The bytes of the whole list are RESERVED by [`Execution::measure`] at
    /// admission, so remembering a cursor never grows what the store accounts for.
    pub issued_page_cursors: Vec<HistoryPageCursor>,
}

/// How many distinct READS one execution's history remembers handing cursors out
/// for.
///
/// A paginator keeps "all other arguments unchanged", so one walk is one read
/// however many pages it takes; this bounds how many different sort-order and
/// page-size combinations are remembered at once, exactly as
/// [`MAX_REDRIVE_CLIENT_TOKENS`] bounds the remembered redrive client tokens, so one
/// execution retains the same bytes however many pages are read from it. The oldest
/// read is dropped past the bound, and a cursor of a read no longer remembered is
/// refused like one that has aged out ("each pagination token expires after 24
/// hours. Using an expired pagination token will return an HTTP 400 InvalidToken
/// error").
pub const MAX_HISTORY_PAGE_CURSORS: usize = 32;

/// How far ONE read of a history has handed cursors out.
///
/// The members are what decide which events the next page holds: the sort order the
/// read walks, the size of its pages, and the furthest position in that read's own
/// order it has handed a cursor out for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPageCursor {
    pub reverse_order: bool,
    pub page: usize,
    pub offset: usize,
}

impl HistoryPageCursor {
    /// Whether this is a cursor of `read`.
    fn of(&self, read: HistoryRead) -> bool {
        self.reverse_order == read.reverse_order && self.page == read.page
    }
}

/// The READ a page cursor belongs to: the sort order and the page size of the
/// `GetExecutionHistory` call it would be handed back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryRead {
    pub reverse_order: bool,
    pub page: usize,
}

impl HistoryRead {
    /// This read's cursor at `offset`, a position in this read's own order.
    pub fn at(self, offset: usize) -> HistoryPageCursor {
        HistoryPageCursor {
            reverse_order: self.reverse_order,
            page: self.page,
            offset,
        }
    }
}

impl Execution {
    /// Whether this execution has reached a terminal status.
    pub fn is_terminal(&self) -> bool {
        self.status != EXECUTION_STATUS_RUNNING
    }

    /// Remember that a read of this history handed a cursor out at `cursor`'s
    /// position, so the call that comes back with it is served and a cursor this
    /// history never handed out is not.
    ///
    /// One entry per read, holding the furthest position that read reached; only the
    /// last [`MAX_HISTORY_PAGE_CURSORS`] reads are remembered, the oldest dropped,
    /// exactly as the remembered redrive client tokens are.
    pub fn remember_page_cursor(&mut self, cursor: HistoryPageCursor) {
        let read = HistoryRead {
            reverse_order: cursor.reverse_order,
            page: cursor.page,
        };
        if let Some(reached) = self
            .issued_page_cursors
            .iter_mut()
            .find(|remembered| remembered.of(read))
        {
            reached.offset = reached.offset.max(cursor.offset);
            return;
        }
        while self.issued_page_cursors.len() >= MAX_HISTORY_PAGE_CURSORS {
            self.issued_page_cursors.remove(0);
        }
        self.issued_page_cursors.push(cursor);
    }

    /// Whether this is a CHILD workflow execution a Distributed Map state started,
    /// which is the one a Map Run enumerates and redrives.
    pub fn is_map_run_child(&self) -> bool {
        self.map_run_arn.is_some()
    }

    /// The bytes this record would retain, including the outcome the interpreter
    /// already computed for it: that outcome is allocated at `StartExecution` and
    /// only moved into the record later, so it counts from the moment the execution
    /// is admitted.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.state_machine_arn.len()
            + self.name.len()
            + self.status.len()
            + self.input.len()
            + self.output.as_ref().map_or(0, String::len)
            + self.error.as_ref().map_or(0, String::len)
            + self.cause.as_ref().map_or(0, String::len)
            + self.trace_header.as_ref().map_or(0, String::len)
            + self
                .state_machine_version_arn
                .as_ref()
                .map_or(0, String::len)
            + self.state_machine_alias_arn.as_ref().map_or(0, String::len)
            + self.map_run_arn.as_ref().map_or(0, String::len)
            // The configuration is SHARED with the child workflow executions this
            // execution's Distributed Map states started, so its bytes are counted
            // once, on the record that owns it.
            + if self.is_map_run_child() {
                0
            } else {
                self.ran.measure()
            }
            + redrive_tokens_bytes(&self.redrive_tokens)
            + history_bytes(&self.history)
            + self.redrive_step.as_ref().map_or(0, asl::RedriveStep::measure)
            + self.pending.as_ref().map_or(0, PendingOutcome::measure)
            // A running execution can still be stopped, and a stop writes a
            // caller-sized `error` and `cause` plus one event. Those bytes are
            // RESERVED here, at admission, so no later write can take the store past
            // [`MAX_STORE_BYTES`]: once the execution is terminal the reserve is
            // released and what the stop actually wrote is counted, which is never
            // more than the reserve.
            + if self.is_terminal() {
                0
            } else {
                STOP_RESERVE_BYTES
            }
            // The page cursors a read of this history may hand out are RESERVED here
            // too, and in full: the list is capped at [`MAX_HISTORY_PAGE_CURSORS`],
            // so remembering one can never take the store past [`MAX_STORE_BYTES`]
            // and a read never grows what the store accounts for.
            + MAX_HISTORY_PAGE_CURSORS * std::mem::size_of::<HistoryPageCursor>()
    }

    /// Apply the outcome the clock now owes.
    ///
    /// `stopDate` is the instant the transition was DUE, not the instant somebody
    /// happened to look: two callers reading at different times must see the same
    /// value.
    fn apply_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.status = pending.status;
        self.stop_date = Some(pending.due_at);
        self.output = pending.output;
        self.error = pending.error;
        self.cause = pending.cause;
        // [`MAX_HISTORY_EVENTS`] is the interpreter's own ceiling, so a run cannot
        // produce more than fits; should the history ever be at the budget anyway,
        // the TERMINAL event is the one kept, so the stored history never ends on a
        // state transition while the record reports a terminal status.
        let mut events = pending.events;
        let room = MAX_HISTORY_EVENTS.saturating_sub(self.history.len());
        if events.len() > room {
            if room == 0 {
                return;
            }
            let terminal = events.pop();
            events.truncate(room - 1);
            events.extend(terminal);
        }
        self.history.extend(events);
    }

    /// Append one event, keeping the history within [`MAX_HISTORY_EVENTS`].
    pub fn push_event(&mut self, event_type: &str, timestamp: SystemTime) {
        self.push_detailed_event(event_type, timestamp, EventDetails::default());
    }

    /// Append one event carrying what its writer knew about it.
    pub fn push_detailed_event(
        &mut self,
        event_type: &str,
        timestamp: SystemTime,
        details: EventDetails,
    ) {
        if self.history.len() >= MAX_HISTORY_EVENTS {
            return;
        }
        let previous = self.history.last();
        let last_id = previous.map_or(0, |event| event.id);
        let previous_event_id = previous
            .map_or(NO_PREVIOUS_EVENT, |event| {
                follows_event(event.id, &event.event_type)
            });
        self.history.push(HistoryEvent {
            id: last_id + 1,
            previous_event_id,
            event_type: event_type.to_string(),
            timestamp,
            state_name: None,
            details,
        });
    }

    /// The id of the last event this execution's history holds, so a later attempt's
    /// events continue the numbering rather than restarting it.
    pub fn last_event_id(&self) -> i64 {
        self.history.last().map_or(0, |event| event.id)
    }

    /// The `redriveDate` a successful redrive under `client_token` already answered,
    /// when that token is still one of the remembered, unexpired ones.
    ///
    /// This is what makes `RedriveExecution` idempotent per client token, for the
    /// window and the count its own documentation publishes.
    pub fn redrive_for_token(&self, client_token: &str, now: SystemTime) -> Option<SystemTime> {
        self.redrive_tokens
            .iter()
            .rev()
            .find(|remembered| {
                remembered.token == client_token
                    && remembered
                        .redrive_date
                        .checked_add(REDRIVE_CLIENT_TOKEN_TTL)
                        .is_none_or(|expiry| expiry > now)
            })
            .map(|remembered| remembered.redrive_date)
    }

    /// Record a redrive: the execution runs again from `now`, so it reports `RUNNING`
    /// until the clock makes `pending` due, its `redriveCount` and `redriveDate`
    /// move, and `opening` (the `ExecutionRedriven` event) is appended to the
    /// history the first attempt left — "redriven executions append their event
    /// history to the existing event history".
    ///
    /// The terminal values of the attempt being redriven are cleared, because the
    /// execution has no terminal values again until the re-run reaches its own.
    fn apply_redrive(
        &mut self,
        now: SystemTime,
        client_token: Option<&str>,
        opening: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
    ) {
        self.status = EXECUTION_STATUS_RUNNING.to_string();
        self.stop_date = None;
        self.output = None;
        self.error = None;
        self.cause = None;
        self.redrive_count = self.redrive_count.saturating_add(1);
        self.redrive_date = Some(now);
        // The step this redrive continued from has been consumed: the attempt it
        // begins records its own if it stops at one.
        self.redrive_step = None;
        if let Some(token) = client_token {
            // Only the last [`MAX_REDRIVE_CLIENT_TOKENS`] are remembered, so this
            // list is bounded however many redrives one execution takes.
            while self.redrive_tokens.len() >= MAX_REDRIVE_CLIENT_TOKENS {
                self.redrive_tokens.remove(0);
            }
            self.redrive_tokens.push(RedriveToken {
                token: token.to_string(),
                redrive_date: now,
            });
        }
        for mut event in opening {
            if self.history.len() >= MAX_HISTORY_EVENTS {
                break;
            }
            // "The number of times you've redriven an execution": the count THIS
            // redrive reached, kept on the event it opens with, because the record's
            // own count moves again at the next one.
            if event.event_type == EVENT_EXECUTION_REDRIVEN {
                event.details.redrive_count = Some(self.redrive_count);
            }
            self.history.push(event);
        }
        self.pending = pending;
    }

    /// Record a worker's reply to the activity task this execution was suspended at:
    /// the events the continuation has already produced are APPENDED to the history
    /// its earlier states left, and the outcome that continuation computed is owed to
    /// the clock exactly as a start's is.
    ///
    /// This CONTINUES the attempt rather than starting another, so nothing the
    /// attempt already recorded is cleared and neither `redriveCount` nor
    /// `redriveDate` moves: the execution has been `RUNNING` since its start and
    /// still is.
    fn apply_continuation(&mut self, events: Vec<HistoryEvent>, pending: Option<PendingOutcome>) {
        for event in events {
            if self.history.len() >= MAX_HISTORY_EVENTS {
                break;
            }
            self.history.push(event);
        }
        self.pending = pending;
    }
}

/// One stored Map Run: the resource a Distributed Map state starts, which
/// `ListMapRuns` reports for the execution that started it.
///
/// A Map Run is NOT keyed by its `(execution, label)` pair in the store, because it
/// is reached by its own ARN; `map_runs_by_execution` is what lists one execution's
/// Map Runs in the order their states were entered.
#[derive(Debug, Clone)]
pub struct MapRun {
    pub arn: String,
    /// The execution whose Distributed Map state started it.
    pub execution_arn: String,
    /// The state machine that execution ran.
    pub state_machine_arn: String,
    /// The `Label` the Map state declares, else that state's own name: the field of
    /// the ARN that tells two Map Runs of one execution apart.
    pub label: String,
    pub status: String,
    pub start_date: SystemTime,
    pub stop_date: Option<SystemTime>,
    /// The terminal status the walk computed and the instant the clock owes it at;
    /// `None` once it has been applied, or once a stop overtook it.
    pub pending: Option<PendingMapRunOutcome>,
    pub max_concurrency: i32,
    pub tolerated_failure_count: i64,
    pub tolerated_failure_percentage: f32,
    pub total_items: i64,
    /// How many of the items the clock has already finished produced a result, and
    /// how many failed: the counts GROW as each item comes due, so what a caller
    /// reads is what the Map Run has done so far rather than what it will have done.
    pub succeeded_items: i64,
    pub failed_items: i64,
    /// The items the clock has NOT finished yet, in the order they finish. Drained by
    /// [`ComAmazonawsSfnAwsstepfunctionsState::settle`] into the counts above, which
    /// is also where the failure budget is weighed.
    pub items: Vec<MapRunItem>,
    /// How many times the parent execution has been redriven since this Map Run was
    /// created, which re-ran the Map state this record belongs to.
    pub redrive_count: i32,
    pub redrive_date: Option<SystemTime>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`], set by
    /// [`ComAmazonawsSfnAwsstepfunctionsState::record_map_runs`] exactly as
    /// [`Execution::retained_bytes`] is.
    pub retained_bytes: usize,
}

/// The terminal status [`asl::run`] computed for a Map Run, owed to the clock: it
/// becomes the Map Run's reported status at `due_at` and not before.
#[derive(Debug, Clone)]
pub struct PendingMapRunOutcome {
    pub due_at: SystemTime,
    pub status: String,
}

/// One item of a Map Run, as the clock owes it: the instant its iteration finished
/// and whether it failed.
///
/// This is [`asl::MapItemOutcome`] with the walk's offset turned into an instant. A
/// Map Run counts an item from this instant on, so the counts it reports are what its
/// items have done SO FAR — the item ran in the walk, but nothing it produced is
/// reported before the clock reaches the instant it produced it at.
#[derive(Debug, Clone, Copy)]
pub struct MapRunItem {
    pub due_at: SystemTime,
    pub failed: bool,
}

/// What a Map Run reports as its `maxConcurrency` for a declared limit of `limit`:
/// the limit itself, and the largest value the modelled integer carries for a limit
/// of `0`.
///
/// `MaxConcurrency`'s own default is `0`, which the Amazon States Language defines as
/// NO limit, and a Map Run reports its limit through a modelled integer: an unlimited
/// Map Run therefore reports the largest value that integer carries, rather than a
/// `0` a client would read as "none at a time". RECORDED: `DescribeMapRun` on a `Map`
/// state that declares no `MaxConcurrency` answers `2147483647`
/// (`sfn.describe-map-run-default-fields-and-execution-counts`).
///
/// One rule for both the `Map` state that declares the limit ([`asl`]) and the
/// `UpdateMapRun` that changes it, so one Map Run never reports "no limit" two ways.
pub fn reported_max_concurrency(limit: i64) -> i32 {
    if limit <= 0 {
        i32::MAX
    } else {
        i32::try_from(limit).unwrap_or(i32::MAX)
    }
}

/// Whether a Map Run whose items turned out this way stayed INSIDE the failure
/// thresholds it ran under.
///
/// `toleratedFailureCount` is "the maximum number of failed items before the Map Run
/// fails" and `toleratedFailurePercentage` "the maximum percentage of failed items"
/// before it does, so either threshold on its own is enough to tolerate the
/// failures: a run is past what it tolerates only when more items failed than the
/// count AND than that share of them. A percentage of `0` — the member's own default
/// — is no share at all, so it tolerates nothing by itself.
///
/// The interpreter decides a `Map` state's outcome with this ([`asl`]), which is also
/// what the Map Run's status reports.
pub fn within_tolerated_failures(
    failed: i64,
    total: i64,
    tolerated_count: i64,
    tolerated_percentage: f64,
) -> bool {
    failed <= tolerated_count
        || (tolerated_percentage > 0.0
            && (failed as f64) * 100.0 <= tolerated_percentage * (total as f64))
}

/// Why an `UpdateMapRun` was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::update_map_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapRunUpdateRejection {
    /// The store holds no Map Run under that ARN — the modelled `ResourceNotFound`.
    NotFound,
    /// The Map Run has already reached a terminal status, so it has no in-progress
    /// configuration to change — the modelled `ValidationException` carrying
    /// [`VALIDATION_REASON_COMPLETED_MAP_RUN`].
    Completed,
}

impl MapRun {
    /// Whether this Map Run has reached a terminal status.
    pub fn is_terminal(&self) -> bool {
        self.status != MAP_RUN_STATUS_RUNNING
    }

    /// The bytes this record would retain.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.arn.len()
            + self.execution_arn.len()
            + self.state_machine_arn.len()
            + self.label.len()
            + self.status.len()
            + self.items.len().saturating_mul(std::mem::size_of::<MapRunItem>())
            + self
                .pending
                .as_ref()
                .map_or(0, |pending| pending.status.len())
    }

    /// Count every item the clock has already finished, and leave `RUNNING` at the
    /// failure that BREACHES the failure budget this record carries.
    ///
    /// Both are owed whether or not anybody called. An item is counted under the
    /// status its iteration reached from the instant it finished, so a Map Run reports
    /// what its items have done so far; and the thresholds a Map Run is running under
    /// are a failure budget, weighed against the failures counted so far
    /// ([`within_tolerated_failures`]) each time one is counted — the thresholds the
    /// record carries NOW, which is what `UpdateMapRun` writes. A Map Run whose
    /// failures are inside that budget keeps running; one whose budget a counted
    /// failure breaches is FAILED from the instant of that failure, rather than only
    /// once every one of its items has finished.
    ///
    /// The budget decides WHEN this Map Run stopped running, and the status it stops
    /// at is the one the walk computed for it: the execution around it reached its own
    /// outcome through the same `Map` state, so the two never come to contradict each
    /// other.
    fn advance(&mut self, now: SystemTime) {
        let fails_on_budget = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.status == MAP_RUN_STATUS_FAILED);
        let mut breached_at: Option<SystemTime> = None;
        let mut counted = 0usize;
        while let Some(item) = self.items.get(counted).copied() {
            if item.due_at > now {
                break;
            }
            counted += 1;
            if !item.failed {
                self.succeeded_items = self.succeeded_items.saturating_add(1);
                continue;
            }
            self.failed_items = self.failed_items.saturating_add(1);
            if breached_at.is_none()
                && fails_on_budget
                && !within_tolerated_failures(
                    self.failed_items,
                    self.total_items,
                    self.tolerated_failure_count,
                    f64::from(self.tolerated_failure_percentage),
                )
            {
                breached_at = Some(item.due_at);
            }
        }
        self.items.drain(..counted);
        if let Some(breached_at) = breached_at.filter(|_| !self.is_terminal()) {
            self.status = MAP_RUN_STATUS_FAILED.to_string();
            self.stop_date = Some(breached_at);
            // The clock owes this Map Run no further status: the budget decided it
            // here, at the failure that breached it.
            self.pending = None;
            return;
        }
        // A Map Run no failure took past its budget reaches the status the walk
        // computed, once its items are all counted and the clock makes it due.
        if self.items.is_empty()
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.due_at <= now)
        {
            self.apply_pending();
        }
    }

    /// Apply the terminal status the clock now owes.
    ///
    /// `stopDate` is the instant the transition was DUE, not the instant somebody
    /// happened to look, so two callers reading at different times see one value.
    fn apply_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.status = pending.status;
        self.stop_date = Some(pending.due_at);
    }
}

/// Per-service state for com-amazonaws-sfn-awsstepfunctions. A handler reaches it
/// through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSfnAwsstepfunctionsState {
    /// Every state machine, keyed by its ARN. A `BTreeMap` so `ListStateMachines`
    /// walks one stable, total order and a page token can name an offset into it.
    ///
    /// This is also the name index: [`state_machine_arn`] maps one region and name to
    /// exactly one key, so `CreateStateMachine` detects a name already in use by
    /// looking up the ARN it would mint, and a name in one region never answers for a
    /// name in another.
    pub state_machines: BTreeMap<String, StateMachine>,
    /// Every published state machine version, keyed by its own qualified ARN
    /// (`<stateMachineArn>:<version>`). A `BTreeMap` so one state machine's
    /// versions sit contiguously under the `<stateMachineArn>:` prefix and go with
    /// it when it is removed.
    ///
    /// Versions live BESIDE the state machines rather than inside them, because a
    /// version is reached by its own ARN: `DescribeStateMachine` on a qualified ARN
    /// is one lookup, and `ListStateMachines` — which reports state machines, not
    /// versions — still walks `state_machines` alone.
    pub state_machine_versions: BTreeMap<String, StateMachineVersion>,
    /// Every state machine alias, keyed by its own qualified ARN
    /// (`<stateMachineArn>:<aliasName>`). A `BTreeMap` for the same reason as the
    /// versions: one state machine's aliases sit contiguously under the
    /// `<stateMachineArn>:` prefix and go with it when it is removed.
    ///
    /// Aliases live in their OWN map rather than beside the versions, because an
    /// alias name and a version number share the qualifier slot of the ARN: keeping
    /// them apart means a lookup of either resolves in one map and never answers
    /// with a record of the other kind.
    pub state_machine_aliases: BTreeMap<String, StateMachineAlias>,
    /// Every activity, keyed by its ARN. A `BTreeMap` for the same reasons the state
    /// machines are in one: one stable, total order for a paged list, and the ARN
    /// key doubling as the name index `CreateActivity`'s idempotency check reads.
    pub activities: BTreeMap<String, Activity>,
    /// Every execution, keyed by its ARN.
    pub executions: BTreeMap<String, Execution>,
    /// One state machine's execution ARNs in start order.
    ///
    /// The CHILD workflow executions a Distributed Map state started are NOT listed
    /// here: "`ListExecutions` [...] returns the child workflow executions of a Map
    /// Run" only when it is asked for that Map Run, so a child is indexed under the
    /// Map Run that started it ([`Self::executions_by_map_run`]) and the parent state
    /// machine's listing holds the executions that were started on it.
    pub executions_by_sm: HashMap<String, Vec<String>>,
    /// Every Map Run a Distributed Map state started, keyed by its own ARN — the one
    /// name `DescribeMapRun` and `UpdateMapRun` reach it by.
    pub map_runs: BTreeMap<String, MapRun>,
    /// One execution's Map Run ARNs in the order their `Map` states were entered, so
    /// `ListMapRuns` pages one execution's Map Runs without walking the whole store.
    pub map_runs_by_execution: HashMap<String, Vec<String>>,
    /// One Map Run's CHILD workflow execution ARNs in the order their items were
    /// started, so `ListExecutions` pages one Map Run's children without walking the
    /// whole store — the index `executions_by_sm` is for a state machine's own
    /// executions.
    pub executions_by_map_run: HashMap<String, Vec<String>>,
    /// Every outstanding activity task, keyed by the `taskToken` that names it — the
    /// one name the worker holding it has, and what `SendTaskHeartbeat`,
    /// `SendTaskSuccess` and `SendTaskFailure` resolve it by.
    pub activity_tasks: BTreeMap<String, ActivityTask>,
    /// Every job an execution is PARKED at, keyed by that execution's own ARN: the
    /// work a `Task` state's run-a-job (`.sync`) integration started, which the task
    /// is not finished until the invoked service reports finished. A walk parks at one
    /// state at a time, so one execution has at most one.
    pub job_tasks: BTreeMap<String, JobTask>,
    /// One activity's outstanding task tokens in SCHEDULE order, so a poll claims the
    /// task that has waited longest and two executions' tasks cannot cross.
    pub activity_tasks_by_activity: HashMap<String, Vec<String>>,
    /// How many workers are holding a poll of each activity, against
    /// [`MAX_ACTIVITY_POLLERS`]. Counted up while a poll is held and back down when
    /// it ends, however it ends (see [`ActivityPoll`]).
    activity_pollers: HashMap<String, usize>,
    /// How many polls are held open across every activity, against
    /// [`MAX_OPEN_ACTIVITY_POLLS`]: the same count summed, kept here so the whole
    /// store's bound does not depend on walking the map.
    open_activity_polls: usize,
    /// The execution ARNs that still carry a [`PendingOutcome`], so settling is
    /// proportional to what is actually owed rather than to the whole store.
    pending_executions: Vec<String>,
    /// The Map Run ARNs that still carry a [`PendingMapRunOutcome`], for the same
    /// reason.
    pending_map_runs: Vec<String>,
    /// The state machine ARNs that are `DELETING`, for the same reason.
    deleting_state_machines: Vec<String>,
    /// The bytes every stored record holds together, so the bound on the store is a
    /// bound on the PRODUCT of the record counts and the caller-sized text each
    /// record carries, not on each factor alone. See [`MAX_STORE_BYTES`].
    retained_bytes: usize,
    /// Mixed into each minted id so two mints in the same nanosecond differ. See
    /// [`ComAmazonawsSfnAwsstepfunctionsState::fresh_execution_name`] and
    /// [`ComAmazonawsSfnAwsstepfunctionsState::fresh_revision_id`].
    name_counter: u64,
    /// How many state machine aliases this store has ever accepted, which stamps each
    /// one's [`StateMachineAlias::creation_sequence`] and so orders a listing of them.
    /// It only ever increases, so a sequence a deleted alias held is never handed out
    /// again and the aliases that remain keep their relative order.
    alias_sequence: u64,
}

impl ComAmazonawsSfnAwsstepfunctionsState {
    /// Materialise every transition the clock has already made due.
    ///
    /// This is the ONE write a reporting operation performs, and it is owed whether
    /// or not anybody called: an execution whose outcome came due is reported
    /// terminal, an activity task that ran out of time is closed and the execution
    /// waiting at it has failed, a map run whose items were due reports the status
    /// they produced, and a state machine whose deletion came due is gone.
    /// Tasks settle before executions and executions before deletions, so an outcome
    /// owed before the machine disappeared is still recorded.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_activity_tasks(now);
        self.settle_job_tasks(now);
        let mut still_pending = Vec::with_capacity(self.pending_executions.len());
        for arn in std::mem::take(&mut self.pending_executions) {
            let Some(execution) = self.executions.get_mut(&arn) else {
                continue;
            };
            let Some(pending) = execution.pending.as_ref() else {
                continue;
            };
            if pending.due_at > now {
                still_pending.push(arn);
                continue;
            }
            execution.apply_pending();
        }
        self.pending_executions = still_pending;

        let mut still_running = Vec::with_capacity(self.pending_map_runs.len());
        for arn in std::mem::take(&mut self.pending_map_runs) {
            let Some(map_run) = self.map_runs.get_mut(&arn) else {
                continue;
            };
            let before = map_run.retained_bytes;
            // The items the clock finished are counted, and a Map Run whose budget
            // one of them breached stops running here.
            map_run.advance(now);
            map_run.retained_bytes = map_run.measure();
            let after = map_run.retained_bytes;
            // A Map Run is still owed something while it holds an item the clock has
            // not reached, or the status its walk computed.
            if map_run.pending.is_some() || !map_run.items.is_empty() {
                still_running.push(arn);
            }
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
        }
        self.pending_map_runs = still_running;

        let mut still_deleting = Vec::with_capacity(self.deleting_state_machines.len());
        for arn in std::mem::take(&mut self.deleting_state_machines) {
            let due = self
                .state_machines
                .get(&arn)
                .and_then(|machine| machine.delete_due_at);
            let Some(due) = due else {
                continue;
            };
            if due > now {
                still_deleting.push(arn);
                continue;
            }
            self.remove_state_machine(&arn);
        }
        self.deleting_state_machines = still_deleting;
    }

    // -----------------------------------------------------------------------
    // Activity tasks
    // -----------------------------------------------------------------------

    /// Close every outstanding activity task the clock has made due, and drop every
    /// one no execution is waiting on any more.
    ///
    /// Both are owed whether or not anybody called: a task whose `TimeoutSeconds` or
    /// `HeartbeatSeconds` ran out has failed its execution at the instant it ran out,
    /// and a task whose execution was stopped — or went with the state machine it ran
    /// on — can never be answered, so no later poll may claim it.
    fn settle_activity_tasks(&mut self, now: SystemTime) {
        // ONE scan over the outstanding tasks, which collects only the tokens this
        // instant changes something for.
        let mut timed_out: Vec<(String, SystemTime)> = Vec::new();
        let mut orphaned: Vec<String> = Vec::new();
        for task in self.activity_tasks.values() {
            if self
                .executions
                .get(&task.execution_arn)
                .is_none_or(Execution::is_terminal)
            {
                orphaned.push(task.token.clone());
                continue;
            }
            if let Some(due) = task.closes_at().filter(|due| *due <= now) {
                timed_out.push((task.token.clone(), due));
            }
        }
        for token in orphaned {
            self.discard_activity_task(&token);
        }
        for (token, due) in timed_out {
            self.time_out_activity_task(&token, due);
        }
    }

    /// Store the task a planned run scheduled for `execution_arn`, minting the
    /// `taskToken` that names it, and answer that token.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include this task's ([`PlannedActivityTask::measure`]).
    pub fn schedule_activity_task(
        &mut self,
        execution_arn: &str,
        planned: PlannedActivityTask,
    ) -> String {
        // The token NAMES the task: a minted id, and the activity it belongs to, so a
        // reply to a token whose task has since been closed can still say which
        // activity that task was of. See [`task_token`].
        let id = self.fresh_uuid();
        let token = task_token(&id, &planned.activity_arn);
        let task = ActivityTask {
            token: token.clone(),
            activity_arn: planned.activity_arn,
            execution_arn: execution_arn.to_string(),
            state_name: planned.state_name,
            input: planned.input,
            state_input: planned.state_input,
            available_at: planned.available_at,
            worker_name: None,
            started_at: None,
            heartbeat_at: None,
            timeout_seconds: planned.timeout_seconds,
            heartbeat_seconds: planned.heartbeat_seconds,
            // The first attempt of this task has spent no retrier's attempts.
            retries_taken: Vec::new(),
            retained_bytes: 0,
        };
        self.store_activity_task(task);
        token
    }

    /// Queue `task` under its activity, counting what its record retains.
    ///
    /// The one place a task record enters the store, so a task a `Retry` scheduled is
    /// queued and counted exactly as the attempt before it was.
    fn store_activity_task(&mut self, mut task: ActivityTask) {
        task.retained_bytes = task.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(task.retained_bytes);
        let token = task.token.clone();
        self.activity_tasks_by_activity
            .entry(task.activity_arn.clone())
            .or_default()
            .push(token.clone());
        self.activity_tasks.insert(token, task);
    }

    /// Hand the longest-waiting available task of `activity_arn` to a worker, and
    /// report the task as it now stands — or `None` when the activity has none to
    /// hand out.
    ///
    /// This is the one write `GetActivityTask` makes: the operation exists to assign
    /// a task to a worker ("you can provide an arbitrary name in order to identify
    /// the worker that the task is assigned to"), so the task moves from scheduled to
    /// started, its heartbeat window opens, and the execution's history records it.
    /// A task already handed out is never handed out twice, and one whose execution
    /// has not reached its state yet (every `Wait` before it is still owed) is not
    /// available.
    pub fn claim_activity_task(
        &mut self,
        activity_arn: &str,
        worker_name: Option<&str>,
        now: SystemTime,
    ) -> Option<ActivityTask> {
        let queue = self.activity_tasks_by_activity.get(activity_arn)?;
        let token = queue
            .iter()
            .find(|token| {
                self.activity_tasks
                    .get(*token)
                    .is_some_and(|task| task.is_queued() && task.available_at <= now)
            })
            .cloned()?;
        let task = self.activity_tasks.get_mut(&token)?;
        task.worker_name = worker_name.map(str::to_string);
        task.started_at = Some(now);
        // The claim starts the heartbeat window: a worker that has just been handed a
        // task has not missed a heartbeat.
        task.heartbeat_at = Some(now);
        let claimed = task.clone();
        if let Some(execution) = self.executions.get_mut(&claimed.execution_arn) {
            let before = execution.retained_bytes;
            // "You can provide an arbitrary name in order to identify the worker that
            // the task is assigned to": the event the claim writes is where that name
            // is recorded.
            execution.push_detailed_event(
                EVENT_ACTIVITY_STARTED,
                now,
                EventDetails {
                    worker_name: claimed.worker_name.clone(),
                    ..EventDetails::default()
                },
            );
            execution.retained_bytes = execution.measure();
            let after = execution.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
        }
        Some(claimed)
    }

    /// Restart the heartbeat window of the task `token` names at `now`, and report
    /// whether the store held one.
    ///
    /// "This action resets the Heartbeat clock": the window the scheduling state's
    /// `HeartbeatSeconds` opens is measured from the last heartbeat, which the claim
    /// starts and this moves on.
    /// The task's `TimeoutSeconds` is read from the instant the state scheduled it and
    /// is deliberately NOT touched, because "the Timeout of a task ... is its maximum
    /// allowed duration, regardless of the number of SendTaskHeartbeat requests
    /// received". No byte of the record changes size, so the store's total is
    /// unaffected.
    pub fn restart_heartbeat_window(&mut self, token: &str, now: SystemTime) -> bool {
        let Some(task) = self.activity_tasks.get_mut(token) else {
            return false;
        };
        task.heartbeat_at = Some(now);
        true
    }

    /// Drop the task `token` names, giving back what its record retained, and report
    /// it. `None` when the store holds no task under that token.
    pub fn discard_activity_task(&mut self, token: &str) -> Option<ActivityTask> {
        let task = self.activity_tasks.remove(token)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(task.retained_bytes);
        if let Some(queue) = self.activity_tasks_by_activity.get_mut(&task.activity_arn) {
            queue.retain(|queued| queued != token);
            if queue.is_empty() {
                self.activity_tasks_by_activity.remove(&task.activity_arn);
            }
        }
        Some(task)
    }

    /// Record a worker's reply to the task `token` names: the task is CLOSED and the
    /// execution waiting at it carries on with what the continuation produced.
    ///
    /// `events` and `pending` are what [`plan_reply`] computed for the walk that
    /// continued from the reply, and `extra_bytes` what the next activity task that
    /// walk stopped at — and the Map Runs it started — will retain, so a reply is
    /// admitted against the store's ceiling exactly as a start is rather than growing
    /// it past [`MAX_STORE_BYTES`].
    ///
    /// The task's record is dropped, giving back the bytes it held: a token answers
    /// ONE task, so a later reply under the same token resolves nothing.
    /// `stopped_at` is the unsuccessful step the continuation's own walk stopped at,
    /// when it stopped at one ([`Planned::stopped_at`]), which is the step a later
    /// `RedriveExecution` continues the execution from.
    pub fn answer_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        stopped_at: Option<asl::RedriveStep>,
    ) -> Result<(), TaskReplyRejection> {
        self.close_activity_task(token, events, pending, extra_bytes, stopped_at, false)
    }

    /// Record the reply that reports the task `token` names FAILED.
    ///
    /// The same close as [`Self::answer_activity_task`], plus the one thing a failure
    /// leaves behind: the attempt stopped AT this task's state, so that state and the
    /// raw input it was entered with are recorded as the step a later
    /// `RedriveExecution` continues the execution from.
    pub fn fail_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
    ) -> Result<(), TaskReplyRejection> {
        self.close_activity_task(token, events, pending, 0, None, true)
    }

    /// Close the task `token` names, recording what the call that closed it computed.
    ///
    /// `stopped_at_task` is whether the attempt STOPPED at this task's state (the task
    /// failed) rather than carrying on past it (the task was answered with a result).
    fn close_activity_task(
        &mut self,
        token: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        walked_stopped_at: Option<asl::RedriveStep>,
        stopped_at_task: bool,
    ) -> Result<(), TaskReplyRejection> {
        let Some(task) = self.activity_tasks.get(token) else {
            return Err(TaskReplyRejection::Gone);
        };
        let execution_arn = task.execution_arn.clone();
        // The attempt stopped either AT this task's state — the worker reported it
        // failed and no retrier answered — or at the step the continuation's own walk
        // failed at, if it failed at one.
        let stopped_at = if stopped_at_task {
            Some(asl::RedriveStep {
                state_name: task.state_name.clone(),
                state_input: task.state_input.clone(),
            })
        } else {
            walked_stopped_at
        };
        self.continue_execution(&execution_arn, events, pending, extra_bytes, stopped_at)?;
        self.discard_activity_task(token);
        Ok(())
    }

    /// Carry the execution `execution_arn` names on with what a continuation walk
    /// produced: its events are APPENDED to the history its earlier states left and
    /// the outcome it computed is owed to the clock.
    ///
    /// The one place a continuation reaches an execution, whatever the execution was
    /// waiting at — a worker's reply to an activity task, or the job a run-a-job
    /// integration started — so both are admitted against the store's ceiling the
    /// same way: `extra_bytes` is what the walk's own next task and Map Runs will
    /// retain, and a continuation the store has no room for changes nothing.
    ///
    /// `stopped_at` is the step a later `RedriveExecution` continues the execution
    /// from, for an attempt that STOPPED at the state it was waiting at.
    fn continue_execution(
        &mut self,
        execution_arn: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        stopped_at: Option<asl::RedriveStep>,
    ) -> Result<(), TaskReplyRejection> {
        // A task whose execution is terminal — stopped, or failed when the clock
        // closed an earlier bound — is answered by nobody: `settle` discards it, and
        // one that is still here when this runs is in that same state.
        let Some(execution) = self
            .executions
            .get(execution_arn)
            .filter(|execution| !execution.is_terminal())
        else {
            return Err(TaskReplyRejection::Gone);
        };
        let before = execution.retained_bytes;
        let mut candidate = execution.clone();
        candidate.apply_continuation(events, pending);
        if let Some(stopped_at) = stopped_at {
            candidate.redrive_step = Some(stopped_at);
        }
        let after = candidate.measure();
        if after + extra_bytes > before && !self.has_room_for(after + extra_bytes - before) {
            return Err(TaskReplyRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        // The continuation's outcome is owed to the clock, so this execution joins the
        // settle queue; one that stopped at ANOTHER activity task — or parked at
        // another job — owes the clock nothing and does not.
        self.pending_executions
            .retain(|pending| pending != execution_arn);
        if candidate.pending.is_some() {
            self.pending_executions.push(execution_arn.to_string());
        }
        self.executions
            .insert(execution_arn.to_string(), candidate);
        Ok(())
    }

    /// Close `token`'s task at `due`, the instant its own bound made it due, and fail
    /// the execution waiting at it with the error the Amazon States Language names
    /// for a task that ran out of time — unless the scheduling state RETRIES it.
    ///
    /// `due` is the instant the bound was reached, not the instant somebody looked:
    /// two callers reading afterwards see the same `stopDate`.
    fn time_out_activity_task(&mut self, token: &str, due: SystemTime) {
        // "If the task runs longer than the specified seconds, this state fails with a
        // States.Timeout error name": the state's own `Retry` answers for that error
        // before the execution does, and the attempt it schedules is a task a worker
        // polls for exactly as this one was.
        if self.retry_activity_task(token, TASK_TIMEOUT_ERROR, EVENT_ACTIVITY_TIMED_OUT, due) {
            return;
        }
        let Some(task) = self.discard_activity_task(token) else {
            return;
        };
        let cause = activity_task_timeout_cause(&task);
        let Some(execution) = self.executions.get_mut(&task.execution_arn) else {
            return;
        };
        if execution.is_terminal() {
            return;
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_FAILED.to_string();
        execution.stop_date = Some(due);
        execution.error = Some(TASK_TIMEOUT_ERROR.to_string());
        execution.cause = Some(cause);
        // The attempt stopped AT this task's state, so that is the step a later
        // redrive continues from — the same step a worker's reported failure leaves.
        execution.redrive_step = Some(asl::RedriveStep {
            state_name: task.state_name.clone(),
            state_input: task.state_input.clone(),
        });
        execution.push_detailed_event(
            EVENT_ACTIVITY_TIMED_OUT,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.push_detailed_event(
            EVENT_EXECUTION_FAILED,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        // The record is re-measured exactly as a stop re-measures it: the execution
        // is terminal now, so the bytes a stop had reserved are given back and the
        // events written here are counted.
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        let execution_arn = task.execution_arn;
        self.pending_executions
            .retain(|pending| pending != &execution_arn);
    }

    /// Re-schedule the task `token` names for its next attempt, when the state that
    /// scheduled it declares a `Retry` that answers for `error`, and report whether it
    /// did.
    ///
    /// "Retry […] an array of objects, called retriers": the first retrier whose
    /// `ErrorEquals` names the error decides, it makes at most its own `MaxAttempts`
    /// attempts, and the next one is due `IntervalSeconds` scaled by `BackoffRate`
    /// once per attempt already taken and capped by `MaxDelaySeconds` after the
    /// failure ([`asl::activity_retry`]).
    ///
    /// The attempt that failed is CLOSED: its token answers nothing afterwards, so the
    /// worker that held it cannot report on the attempt that replaces it. An identical
    /// task is queued in its place, available once the retrier's interval has passed
    /// and carrying one more spent attempt of that retrier, and the execution's history
    /// records both the failure and the scheduling of the next attempt. The execution
    /// stays `RUNNING`, because the state has not failed yet.
    fn retry_activity_task(
        &mut self,
        token: &str,
        error: &str,
        failure_event: &str,
        at: SystemTime,
    ) -> bool {
        let Some(task) = self.activity_tasks.get(token).cloned() else {
            return false;
        };
        let Some(definition) = self.definition_of_execution(&task.execution_arn) else {
            return false;
        };
        let Some((retrier, delay)) =
            asl::activity_retry(&definition, &task.state_name, error, &task.retries_taken)
        else {
            return false;
        };
        // A task whose execution is no longer running is retried by nobody: it was
        // stopped, or an earlier bound of the execution already failed it.
        let Some(execution) = self
            .executions
            .get(&task.execution_arn)
            .filter(|execution| !execution.is_terminal())
            .cloned()
        else {
            return false;
        };
        let available_at = after(at, delay);
        let mut queued = task.clone();
        queued.token = task_token(&self.fresh_uuid(), &task.activity_arn);
        queued.worker_name = None;
        queued.started_at = None;
        queued.heartbeat_at = None;
        queued.available_at = available_at;
        if queued.retries_taken.len() <= retrier {
            queued.retries_taken.resize(retrier + 1, 0);
        }
        if let Some(spent) = queued.retries_taken.get_mut(retrier) {
            *spent = spent.saturating_add(1);
        }
        let mut candidate = execution.clone();
        // The failure happened when the bound was reached; the next attempt is
        // scheduled when its interval has passed, which is also when a poll may first
        // claim it.
        candidate.push_detailed_event(
            failure_event,
            at,
            EventDetails::of_failure(Some(error.to_string()), None),
        );
        candidate.push_detailed_event(
            EVENT_ACTIVITY_SCHEDULED,
            available_at,
            EventDetails::of_scheduled_task(&queued),
        );
        let before = execution.retained_bytes;
        let after_bytes = candidate.measure();
        // What the retry adds is the two events and whatever the next attempt's record
        // holds beyond the one it replaces, which is admitted against the store's
        // ceiling exactly as the first attempt was: a retry the store has no room for
        // leaves the failure to the execution rather than growing past the ceiling.
        let grows = (after_bytes + queued.measure())
            .saturating_sub(before + task.retained_bytes);
        if grows > 0 && !self.has_room_for(grows) {
            return false;
        }
        candidate.retained_bytes = after_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after_bytes)
            .saturating_sub(before);
        self.executions
            .insert(task.execution_arn.clone(), candidate);
        self.discard_activity_task(token);
        self.store_activity_task(queued);
        true
    }

    /// Re-schedule the task `token` names after the worker holding it reported it
    /// FAILED with `error`, and report whether it was retried.
    ///
    /// The error a worker reports is what the scheduling state's `Retry` is matched
    /// against, exactly as the `States.Timeout` the clock reports is: a task the state
    /// retries is queued for its next attempt and the execution stays `RUNNING`, and
    /// one it does not leaves the reply to fail the execution.
    pub fn retry_failed_activity_task(
        &mut self,
        token: &str,
        error: &str,
        at: SystemTime,
    ) -> bool {
        self.retry_activity_task(token, error, EVENT_ACTIVITY_FAILED, at)
    }

    // -----------------------------------------------------------------------
    // The jobs a run-a-job (`.sync`) integration started
    // -----------------------------------------------------------------------

    /// Fail every parked job that has outlasted the time its task was allowed, and
    /// drop every one no execution is waiting at any more.
    ///
    /// Both are owed whether or not anybody called, exactly as an activity task's own
    /// bounds are: a task whose job did not finish inside its `TimeoutSeconds` failed
    /// its execution at the instant that span ran out, and a job whose execution was
    /// stopped — or went with the state machine it ran on — resolves nothing.
    fn settle_job_tasks(&mut self, now: SystemTime) {
        let mut timed_out: Vec<(String, SystemTime)> = Vec::new();
        let mut orphaned: Vec<String> = Vec::new();
        for task in self.job_tasks.values() {
            if self
                .executions
                .get(&task.execution_arn)
                .is_none_or(Execution::is_terminal)
            {
                orphaned.push(task.execution_arn.clone());
                continue;
            }
            let due = task.times_out_at();
            if due <= now {
                timed_out.push((task.execution_arn.clone(), due));
            }
        }
        for execution_arn in orphaned {
            self.discard_job_task(&execution_arn);
        }
        for (execution_arn, due) in timed_out {
            self.time_out_job_task(&execution_arn, due);
        }
    }

    /// Record the job a planned run parked at, so the execution waits at that state
    /// until the job finishes.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include this record's ([`PlannedJob::measure`]).
    pub fn park_job(&mut self, execution_arn: &str, planned: PlannedJob, region: &str) {
        let mut task = JobTask {
            execution_arn: execution_arn.to_string(),
            state_name: planned.state_name,
            state_input: planned.state_input,
            job: planned.job,
            api: planned.api,
            parsed_output: planned.parsed_output,
            job_id: planned.job_id,
            naming: planned.naming,
            region: region.to_string(),
            parked_at: planned.parked_at,
            timeout_seconds: planned.timeout_seconds,
            retained_bytes: 0,
        };
        task.retained_bytes = std::mem::size_of::<JobTask>()
            + task.execution_arn.len()
            + task.state_name.len()
            + task.state_input.len()
            + task.job_id.len()
            + naming_bytes(&task.naming)
            + task.region.len();
        self.retained_bytes = self.retained_bytes.saturating_add(task.retained_bytes);
        self.job_tasks.insert(execution_arn.to_string(), task);
    }

    /// Drop the job the execution `execution_arn` is parked at, giving back what its
    /// record retained, and report it. `None` when the execution is parked at none.
    pub fn discard_job_task(&mut self, execution_arn: &str) -> Option<JobTask> {
        let task = self.job_tasks.remove(execution_arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(task.retained_bytes);
        Some(task)
    }

    /// Record what the job an execution was parked at ENDED as: the job is closed and
    /// the execution waiting at it carries on with what the continuation produced.
    ///
    /// `events` and `pending` are what [`plan_job_reply`] computed for the walk that
    /// continued from the job's own report, admitted against the store's ceiling
    /// exactly as a worker's reply is. `stopped_at_task` is whether the attempt
    /// STOPPED at the state that started the job — the job ended without doing its
    /// work and no `Catch` carried the execution on — and `walked_stopped_at` the
    /// unsuccessful step the continuation's own walk stopped at otherwise; either way
    /// it is the step a later `RedriveExecution` continues from.
    pub fn resolve_job_task(
        &mut self,
        execution_arn: &str,
        events: Vec<HistoryEvent>,
        pending: Option<PendingOutcome>,
        extra_bytes: usize,
        walked_stopped_at: Option<asl::RedriveStep>,
        stopped_at_task: bool,
    ) -> Result<(), TaskReplyRejection> {
        let Some(task) = self.job_tasks.get(execution_arn) else {
            return Err(TaskReplyRejection::Gone);
        };
        let stopped_at = if stopped_at_task {
            Some(asl::RedriveStep {
                state_name: task.state_name.clone(),
                state_input: task.state_input.clone(),
            })
        } else {
            walked_stopped_at
        };
        self.continue_execution(execution_arn, events, pending, extra_bytes, stopped_at)?;
        self.discard_job_task(execution_arn);
        Ok(())
    }

    /// Close the job the execution `execution_arn` is parked at because its task ran
    /// out of the time it was allowed, and fail that execution with the error the
    /// Amazon States Language names for a task that ran out of time.
    ///
    /// `due` is the instant the bound was reached, not the instant somebody looked, so
    /// two callers reading afterwards see the same `stopDate`.
    fn time_out_job_task(&mut self, execution_arn: &str, due: SystemTime) {
        let Some(task) = self.discard_job_task(execution_arn) else {
            return;
        };
        let cause = job_task_timeout_cause(&task);
        let Some(execution) = self.executions.get_mut(execution_arn) else {
            return;
        };
        if execution.is_terminal() {
            return;
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_FAILED.to_string();
        execution.stop_date = Some(due);
        execution.error = Some(TASK_TIMEOUT_ERROR.to_string());
        execution.cause = Some(cause);
        // The attempt stopped AT the state that started the job, so that is the step a
        // later redrive continues from — the same step a failed task leaves.
        execution.redrive_step = Some(asl::RedriveStep {
            state_name: task.state_name.clone(),
            state_input: task.state_input.clone(),
        });
        execution.push_detailed_event(
            EVENT_TASK_TIMED_OUT,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.push_detailed_event(
            EVENT_EXECUTION_FAILED,
            due,
            EventDetails::of_failure(
                Some(TASK_TIMEOUT_ERROR.to_string()),
                execution.cause.clone(),
            ),
        );
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.pending_executions
            .retain(|pending| pending != execution_arn);
    }

    /// The definition the execution `arn` RUNS, which is where its states are read
    /// from.
    ///
    /// An execution started through a version ARN — or through an alias that routed to
    /// a version — runs that version's immutable snapshot; every other runs what the
    /// state machine holds. The same choice `plan_reply`'s callers make, so a retried
    /// task's `Retry` is read from the definition the execution is running.
    fn definition_of_execution(&self, arn: &str) -> Option<String> {
        let execution = self.executions.get(arn)?;
        if let Some(version) = execution
            .state_machine_version_arn
            .as_deref()
            .and_then(|version_arn| self.state_machine_versions.get(version_arn))
        {
            return Some(version.definition.clone());
        }
        self.state_machines
            .get(&execution.state_machine_arn)
            .map(|machine| machine.definition.clone())
    }

    /// Drop every outstanding task of the executions `arns` names, for a removal that
    /// takes those executions with it.
    fn discard_tasks_of_executions(&mut self, arns: &[String]) {
        let tokens: Vec<String> = self
            .activity_tasks
            .values()
            .filter(|task| arns.contains(&task.execution_arn))
            .map(|task| task.token.clone())
            .collect();
        for token in tokens {
            self.discard_activity_task(&token);
        }
        for arn in arns {
            self.discard_job_task(arn);
        }
    }

    /// The bytes every stored record holds together, against [`MAX_STORE_BYTES`].
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// A fresh execution name for a `StartExecution` that did not give one.
    ///
    /// From the `name` member's own documentation: "If you don't provide a name for
    /// the execution, Step Functions automatically generates a universally unique
    /// identifier (UUID) as the execution name." The value is a version-4 UUID:
    /// 122 bits drawn from the process clock mixed with this store's own counter
    /// through SplitMix64, with RFC 4122's version and variant bits set, rendered
    /// 8-4-4-4-12 in lowercase hex. The counter is what makes two mints in the same
    /// nanosecond differ.
    pub fn fresh_execution_name(&mut self) -> String {
        self.fresh_uuid()
    }

    /// A fresh id for the trailing field of one Express execution ARN (see
    /// [`EXPRESS_EXECUTION_RESOURCE`]).
    ///
    /// Minted from the same generator as an execution name, so two synchronous runs
    /// that reused one name still differ — the id is what makes the ARN unique, and
    /// the counter is what makes two mints in the same nanosecond differ.
    pub fn fresh_execution_id(&mut self) -> String {
        self.fresh_uuid()
    }

    /// One freshly drawn version-4 UUID: the single mint behind every id this store
    /// hands out, so there is one implementation of the draw.
    fn fresh_uuid(&mut self) -> String {
        self.name_counter = self.name_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.name_counter.rotate_left(29);
        let high = splitmix64(&mut seed);
        let low = splitmix64(&mut seed);
        uuid_v4(high, low)
    }

    /// One draw in `0..bound`, from the same generator every id this store mints is
    /// drawn from; `0` for an empty span.
    ///
    /// What a weighted routing configuration is sampled with
    /// ([`Self::routed_version`]): the counter is what makes two draws in the same
    /// nanosecond differ, so consecutive starts through one alias do not all route
    /// to the same version.
    fn draw_below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.name_counter = self.name_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.name_counter.rotate_left(29);
        splitmix64(&mut seed) % bound
    }

    /// A fresh revision id for a state machine `UpdateStateMachine` just changed.
    ///
    /// `revisionId` is "the revision identifier for the updated state machine", and
    /// it has to differ on every update so a client can order revisions, so it is
    /// minted from the same UUID generator as an execution name.
    pub fn fresh_revision_id(&mut self) -> String {
        self.fresh_uuid()
    }

    /// Whether the store can still take a record of `bytes`.
    ///
    /// Every growth site asks this BEFORE inserting, so a create or a start that
    /// would take the store past [`MAX_STORE_BYTES`] is refused with the operation's
    /// own modelled limit error instead of growing it.
    pub fn has_room_for(&self, bytes: usize) -> bool {
        self.retained_bytes.saturating_add(bytes) <= MAX_STORE_BYTES
    }

    /// Store a newly created state machine under its own ARN, counting what it
    /// retains.
    pub fn insert_state_machine(&mut self, mut machine: StateMachine) {
        machine.retained_bytes = machine.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(machine.retained_bytes);
        self.state_machines.insert(machine.arn.clone(), machine);
    }

    /// Store a newly created activity under its own ARN, counting what it retains.
    pub fn insert_activity(&mut self, mut activity: Activity) {
        activity.retained_bytes = activity.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(activity.retained_bytes);
        self.activities.insert(activity.arn.clone(), activity);
    }

    /// Remove the activity `arn` names, giving back what its record retained, and
    /// report the record that was removed.
    ///
    /// `None` when the store held no activity under that ARN, which `DeleteActivity`
    /// — whose model declares no does-not-exist error — treats as the state the call
    /// asked for rather than as a failure.
    pub fn remove_activity(&mut self, arn: &str) -> Option<Activity> {
        let activity = self.activities.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(activity.retained_bytes);
        Some(activity)
    }

    /// The activities of one region, in ARN order.
    ///
    /// A regional endpoint reports only its own region's resources, and every
    /// activity ARN begins with [`activity_arn_prefix`] for its region, so the
    /// `BTreeMap` holds one region's records contiguously and this is a range scan
    /// rather than a filter over the whole store — exactly as
    /// [`Self::state_machines_in_region`] reads the other ARN-keyed resource.
    pub fn activities_in_region(&self, region: &str) -> Vec<&Activity> {
        let prefix = activity_arn_prefix(region);
        self.activities
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, activity)| activity)
            .collect()
    }

    /// Why the store cannot take one more activity of `bytes`, or `None` when it
    /// can. The caps are on the WHOLE store, like every other cap here.
    pub fn activity_rejection(&self, bytes: usize) -> Option<String> {
        if self.activities.len() >= MAX_ACTIVITIES {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_ACTIVITIES} activities"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this activity would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// The tags recorded under a taggable ARN — an EMPTY list when none are.
    ///
    /// This is a read of the tag set, not a resolution of the resource: a tagging
    /// read reports "an array of tags associated with the resource", and the array
    /// of an ARN nothing is tagged under is empty, whether the resource carries no
    /// tags or the store holds no resource under that ARN at all. The write path
    /// ([`Self::merge_tags`]) is where a resource has to exist.
    ///
    /// Both taggable resource types answer here: a tagging call takes "the ARN for
    /// the Step Functions state machine or activity", and one ARN names at most one
    /// of them, since the resource type is a field of the ARN itself.
    pub fn tags_of(&self, arn: &str) -> Vec<Tag> {
        self.tags_at(arn).cloned().unwrap_or_default()
    }

    /// The tag set stored under `arn`, whichever taggable resource type holds it, or
    /// `None` when the store holds no resource under that ARN.
    fn tags_at(&self, arn: &str) -> Option<&Vec<Tag>> {
        if let Some(machine) = self.state_machines.get(arn) {
            return Some(&machine.tags);
        }
        self.activities.get(arn).map(|activity| &activity.tags)
    }

    /// The tag set and the record measurement of the taggable resource `arn` names,
    /// for the two writes below, so each of them states the merge rule ONCE rather
    /// than once per resource type.
    fn taggable_record_mut(&mut self, arn: &str) -> Option<(&mut Vec<Tag>, &mut usize)> {
        if let Some(machine) = self.state_machines.get_mut(arn) {
            return Some((&mut machine.tags, &mut machine.retained_bytes));
        }
        self.activities
            .get_mut(arn)
            .map(|activity| (&mut activity.tags, &mut activity.retained_bytes))
    }

    /// Merge `incoming` into the tags of the taggable resource `arn` names.
    ///
    /// A resource's tags are a MAP keyed by tag key: a key the resource already
    /// carries takes the incoming value in place, and a key it does not carry is
    /// appended, so re-tagging one key never adds a second entry for it and the
    /// order the keys were first written in is the same for every later reader.
    /// Nothing is written when the merge is rejected, so a refused call leaves the
    /// resource exactly as it was.
    ///
    /// The WORK is bounded before any of it is done: `incoming` is caller-sized and
    /// the merge scans the set per entry, so a list longer than [`MAX_TAGS`] — which
    /// could only ever be refused — is refused up front, leaving at most
    /// `MAX_TAGS` iterations over at most `2 * MAX_TAGS` entries however long the
    /// request's list is.
    pub fn merge_tags(&mut self, arn: &str, incoming: &[Tag]) -> Result<(), TagWriteRejection> {
        let Some(current) = self.tags_at(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        if incoming.len() > MAX_TAGS {
            return Err(TagWriteRejection::TooManyTags(incoming.len()));
        }

        let tags_before = tags_bytes(current);
        let mut merged = current.clone();
        for tag in incoming {
            match merged.iter_mut().find(|existing| existing.key == tag.key) {
                Some(existing) => existing.value = tag.value.clone(),
                None => merged.push(tag.clone()),
            }
        }
        if merged.len() > MAX_TAGS {
            return Err(TagWriteRejection::TooManyTags(merged.len()));
        }

        // The tags are caller-sized, so the record's measurement moves by what the
        // write changed and a write that would take the store past
        // [`MAX_STORE_BYTES`] is refused instead of growing it.
        let tags_after = tags_bytes(&merged);
        if tags_after > tags_before && !self.has_room_for(tags_after - tags_before) {
            return Err(TagWriteRejection::NoRoom);
        }

        let Some((tags, retained)) = self.taggable_record_mut(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        *tags = merged;
        *retained = retained.saturating_sub(tags_before).saturating_add(tags_after);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(tags_before)
            .saturating_add(tags_after);
        Ok(())
    }

    /// Remove the tags `keys` names from the taggable resource `arn` names.
    ///
    /// Keyed by tag key, like the merge: a key the resource does not carry is
    /// nothing to remove, which is not a failure, and a key removed twice is
    /// removed once. `false` when the store holds no resource under `arn` —
    /// the modelled `ResourceNotFound` of the write path.
    ///
    /// `keys` is caller-sized, so the WORK is bounded by what the resource holds
    /// rather than by the request: the resource's at most [`MAX_TAGS`] keys are read
    /// into a set, the request's list is walked ONCE against it, and only the keys
    /// that actually match are kept — so the allocation is at most `MAX_TAGS`
    /// entries however long the request's list is.
    pub fn remove_tags(&mut self, arn: &str, keys: &[String]) -> bool {
        let Some(current) = self.tags_at(arn) else {
            return false;
        };
        let present: std::collections::HashSet<&str> =
            current.iter().filter_map(|tag| tag.key.as_deref()).collect();
        let removing: std::collections::HashSet<String> = keys
            .iter()
            .filter(|key| present.contains(key.as_str()))
            .cloned()
            .collect();
        if removing.is_empty() {
            // Nothing the resource carries was named, so there is nothing to write
            // and the record's measurement cannot have moved.
            return true;
        }
        let tags_before = tags_bytes(current);

        let Some((tags, retained)) = self.taggable_record_mut(arn) else {
            return false;
        };
        tags.retain(|tag| tag.key.as_deref().is_none_or(|key| !removing.contains(key)));
        // The record's measurement moves by what the write changed; a removal can
        // only shrink it, so it can never take the store past [`MAX_STORE_BYTES`].
        let tags_after = tags_bytes(tags);
        *retained = retained.saturating_sub(tags_before).saturating_add(tags_after);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(tags_before)
            .saturating_add(tags_after);
        true
    }

    /// Publish the CURRENT configuration of the state machine `arn` names as its
    /// next version, and report the version that was stored. `None` when the store
    /// holds no state machine under `arn`.    ///
    /// "If you set the publish parameter of this API action to true, it publishes
    /// version 1 as the first revision of the state machine" — so the number comes
    /// from the state machine's own counter and is never reused, and the version
    /// holds a snapshot of the configuration, not a reference to a record that can
    /// still change.
    ///
    /// The caller admits the version's bytes BEFORE calling (see
    /// [`ComAmazonawsSfnAwsstepfunctionsState::state_machine_rejection`]), so this
    /// only counts them.
    pub fn publish_state_machine_version(
        &mut self,
        arn: &str,
        description: Option<String>,
        now: SystemTime,
    ) -> Option<StateMachineVersion> {
        let machine = self.state_machines.get(arn)?;
        let version = machine.version_counter.saturating_add(1);
        let mut candidate = StateMachineVersion {
            arn: state_machine_version_arn(arn, version),
            state_machine_arn: machine.arn.clone(),
            version,
            name: machine.name.clone(),
            definition: machine.definition.clone(),
            role_arn: machine.role_arn.clone(),
            sm_type: machine.sm_type.clone(),
            creation_date: now,
            logging_configuration: machine.logging_configuration.clone(),
            tracing_configuration: machine.tracing_configuration.clone(),
            encryption_configuration: machine.encryption_configuration.clone(),
            revision_id: machine.revision_id.clone(),
            description,
            retained_bytes: 0,
        };
        candidate.retained_bytes = candidate.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(candidate.retained_bytes);

        let machine = self.state_machines.get_mut(arn)?;
        machine.version_counter = version;
        self.state_machine_versions
            .insert(candidate.arn.clone(), candidate.clone());
        Some(candidate)
    }

    /// What one more published version of the state machine `arn` names would
    /// retain, so a caller can admit it against [`MAX_STORE_BYTES`] BEFORE asking
    /// for it: a version snapshots the record's caller-sized text, so the record's
    /// own measurement bounds it from above (a version carries no tags).
    pub fn version_reserve(&self, arn: &str) -> usize {
        self.state_machines
            .get(arn)
            .map_or(0, |machine| machine.retained_bytes)
    }

    /// The version of one state machine that snapshots the revision `revision_id`
    /// names (`None` for a state machine that has never been updated), if one was
    /// ever published from it.
    ///
    /// This is what makes publishing idempotent: "It doesn't create a duplicate
    /// state machine version if it already exists for the current revision. Step
    /// Functions bases PublishStateMachineVersion's idempotency check on the
    /// stateMachineArn, name, and revisionId parameters" — the state machine (which
    /// its ARN and name identify) plus the revision, and nothing else, so the
    /// description a repeat carries does not make it a different request. The
    /// HIGHEST-numbered match is reported, so the answer is the same whichever order
    /// the store was filled in.
    pub fn version_of_revision(
        &self,
        state_machine_arn: &str,
        revision_id: Option<&str>,
    ) -> Option<&StateMachineVersion> {
        self.versions_of(state_machine_arn)
            .into_iter()
            .rev()
            .find(|version| version.revision_id.as_deref() == revision_id)
    }

    /// How many versions of one state machine the store holds, which
    /// [`MAX_VERSIONS_PER_STATE_MACHINE`] bounds.
    pub fn version_count(&self, state_machine_arn: &str) -> usize {
        self.versions_unordered(state_machine_arn).len()
    }

    /// The published versions of one state machine, in version order.
    pub fn versions_of(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let mut versions = self.versions_unordered(state_machine_arn);
        versions.sort_by_key(|version| version.version);
        versions
    }

    /// The published versions of one state machine, NEWEST FIRST.
    ///
    /// From `ListStateMachineVersions`' own documentation: "The results are sorted in
    /// descending order of the version creation time." The comparator is keyed on
    /// that documented sort key — the creation time — rather than on the version
    /// order [`ComAmazonawsSfnAwsstepfunctionsState::versions_of`] reversed, so the
    /// order still follows the clock if a version is ever stored out of number
    /// order; two versions published inside one clock tick carry the same creation
    /// time, so the version number — which only ever increases
    /// ([`ComAmazonawsSfnAwsstepfunctionsState::publish_state_machine_version`]) —
    /// breaks the tie, keeping the order total so a page token means the same thing
    /// on the next call.
    pub fn versions_newest_first(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let mut versions = self.versions_unordered(state_machine_arn);
        versions.sort_by(|left, right| {
            right
                .creation_date
                .cmp(&left.creation_date)
                .then_with(|| right.version.cmp(&left.version))
        });
        versions
    }

    /// Remove one published version, uncounting what it retained.
    ///
    /// A version is an immutable snapshot keyed by its own qualified ARN, so this
    /// touches nothing else: "deleting a state machine version won't terminate its
    /// in-progress executions", and the state machine's version counter only ever
    /// increases, so the number this frees is never handed out again.
    pub fn remove_state_machine_version(
        &mut self,
        version_arn: &str,
    ) -> Option<StateMachineVersion> {
        let version = self.state_machine_versions.remove(version_arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(version.retained_bytes);
        Some(version)
    }

    /// The published versions of one state machine, in no particular order.
    ///
    /// Every version ARN of one state machine begins with its ARN and a colon, so
    /// this is a range scan over the contiguous block rather than a filter over
    /// every version in the store. It is what both ordered accessors above sort, so
    /// each of them sorts the scan exactly once.
    fn versions_unordered(&self, state_machine_arn: &str) -> Vec<&StateMachineVersion> {
        let prefix = format!("{state_machine_arn}:");
        self.state_machine_versions
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, version)| version)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Aliases
    // -----------------------------------------------------------------------

    /// Store a newly created state machine alias under its own ARN, counting what it
    /// retains and stamping the create's place in the order the store accepted them.
    pub fn insert_state_machine_alias(&mut self, mut alias: StateMachineAlias) {
        self.alias_sequence = self.alias_sequence.saturating_add(1);
        alias.creation_sequence = self.alias_sequence;
        alias.retained_bytes = alias.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(alias.retained_bytes);
        self.state_machine_aliases.insert(alias.arn.clone(), alias);
    }

    /// Apply an `UpdateStateMachineAlias` to the alias `arn` names, and report the
    /// instant it reports as its `updateDate`.
    ///
    /// The record is mutated IN PLACE: an update "modif[ies] its description or
    /// routingConfiguration" and mints nothing, so the alias keeps its ARN, its
    /// creation date and its place in the order the store accepted the creates (see
    /// [`StateMachineAlias::creation_sequence`]). Only the members the request named
    /// are written — `None` leaves what the alias holds.
    ///
    /// "UpdateStateMachineAlias is an idempotent API [...] Requests with the same
    /// parameters return an idempotent response": a request that would write what the
    /// record already holds answers the `updateDate` it already reports and changes
    /// nothing, so a repeat does not keep moving that instant.
    pub fn update_state_machine_alias(
        &mut self,
        arn: &str,
        description: Option<String>,
        routing_configuration: Option<Vec<RoutingConfigurationListItem>>,
        now: SystemTime,
    ) -> Result<SystemTime, AliasUpdateRejection> {
        let Some(alias) = self.state_machine_aliases.get(arn) else {
            return Err(AliasUpdateRejection::NotFound);
        };
        let before = alias.retained_bytes;
        let mut candidate = alias.clone();
        if let Some(description) = description {
            candidate.description = Some(description);
        }
        if let Some(routing_configuration) = routing_configuration {
            candidate.routing_configuration = routing_configuration;
        }
        if candidate.description == alias.description
            && same_routing(
                &candidate.routing_configuration,
                &alias.routing_configuration,
            )
        {
            return Ok(alias.update_date);
        }
        // "The date the state machine alias was last updated" moves only when this
        // call really updated it.
        candidate.update_date = now;
        let after = candidate.measure();
        if after > before && !self.has_room_for(after - before) {
            return Err(AliasUpdateRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.state_machine_aliases
            .insert(arn.to_string(), candidate);
        Ok(now)
    }

    /// Drop the alias `arn` names, giving back the bytes its record held, and report
    /// it. `None` when the store holds no alias under that ARN.
    ///
    /// "When you delete a state machine alias, Step Functions doesn't delete the state
    /// machine versions that alias references": an alias is a pointer, so this touches
    /// nothing but the pointer. The store's creation sequence only ever increases, so
    /// the place this frees is never handed out again and the aliases that remain keep
    /// their relative order.
    pub fn remove_state_machine_alias(&mut self, arn: &str) -> Option<StateMachineAlias> {
        let alias = self.state_machine_aliases.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(alias.retained_bytes);
        Some(alias)
    }

    /// What the modelled `ResourceNotFound` says when an alias MUTATION resolves no
    /// alias under `alias_arn`, worded for WHICH resource the ARN resolved nothing
    /// for.
    ///
    /// An alias ARN names two resources — the state machine it qualifies, and the
    /// alias name inside it ([`state_machine_alias_arn`]) — so resolving one is two
    /// steps, and the refusal states the step that failed. A state machine this store
    /// does not hold resolves nothing at all, and the mutation reports the ARN the
    /// request named, exactly as the alias's own readers do
    /// ([`resource_not_found_message`]): that ARN is what identifies the alias left
    /// dangling by a state machine that was deleted under it. A state machine that IS
    /// held and carries no such alias is the fixed sentence
    /// ([`RESOURCE_DOES_NOT_EXIST`]), which is what the alias itself being gone is
    /// RECORDED answering.
    pub fn missing_alias_message(&self, alias_arn: &str, state_machine_arn: &str) -> String {
        if self.state_machines.contains_key(state_machine_arn) {
            RESOURCE_DOES_NOT_EXIST.to_string()
        } else {
            resource_not_found_message(alias_arn)
        }
    }

    /// The aliases of one state machine, in ARN order — which is alias-name order,
    /// since every one of those ARNs differs only in the name.
    ///
    /// Every alias ARN of one state machine begins with its ARN and a colon, so this
    /// is a range scan over a contiguous block rather than a filter over every alias
    /// in the store, exactly as [`Self::versions_unordered`] is.
    pub fn aliases_of(&self, state_machine_arn: &str) -> Vec<&StateMachineAlias> {
        let prefix = format!("{state_machine_arn}:");
        self.state_machine_aliases
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, alias)| alias)
            .collect()
    }

    /// The aliases of one state machine, in the order they were CREATED — oldest
    /// first.
    ///
    /// This is the order a client observes from `ListStateMachineAliases`: the service
    /// presents one state machine's aliases in the order the creates were accepted,
    /// and that relative order does not move when another alias of the same state
    /// machine is created, updated or deleted. The order is keyed on
    /// [`StateMachineAlias::creation_sequence`], which no two aliases share, so it is
    /// total and two listings of one unchanged state machine agree — which is what a
    /// paged consumer reconciling positions against its own ledger needs.
    pub fn aliases_in_creation_order(&self, state_machine_arn: &str) -> Vec<&StateMachineAlias> {
        let mut aliases = self.aliases_of(state_machine_arn);
        aliases.sort_by_key(|alias| alias.creation_sequence);
        aliases
    }

    /// The aliases of `state_machine_arn` whose routing configuration names the
    /// version `version_arn`, in the order they were created.
    pub fn aliases_routing_to(
        &self,
        state_machine_arn: &str,
        version_arn: &str,
    ) -> Vec<&StateMachineAlias> {
        self.aliases_in_creation_order(state_machine_arn)
            .into_iter()
            .filter(|alias| alias.routes_to(version_arn))
            .collect()
    }

    /// Why the store cannot take one more alias of `bytes` for `state_machine_arn`,
    /// or `None` when it can — the modelled `ServiceQuotaExceededException`.
    pub fn alias_rejection(&self, state_machine_arn: &str, bytes: usize) -> Option<String> {
        let held = self.aliases_of(state_machine_arn).len();
        if held >= MAX_ALIASES_PER_STATE_MACHINE {
            return Some(format!(
                "state machine {state_machine_arn} already holds the maximum of \
                 {MAX_ALIASES_PER_STATE_MACHINE} aliases"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this state machine alias would take the store past the {MAX_STORE_BYTES} bytes \
                 this emulator retains"
            ));
        }
        None
    }

    /// The state machine every version of `routing` belongs to, or why that routing
    /// configuration cannot be pointed at.
    ///
    /// The ONE place the routing-configuration rules live, so every operation that
    /// takes one judges it identically: each entry names a state machine VERSION of
    /// this service in the endpoint's region, "if you specify the ARN of a second
    /// version, it must belong to the same state machine as the first version", "the
    /// sum of the weights in the routing configuration must be equal to 100", and a
    /// version the store does not hold is a resource that cannot be found. The ARNs
    /// are judged BEFORE the store is consulted, like every other ARN here, so a
    /// defect in an ARN is never reported as a version that is absent.
    pub fn read_routing_configuration(
        &self,
        routing: &[RoutingConfigurationListItem],
        endpoint_region: &str,
    ) -> Result<String, RoutingRejection> {
        let mut owner: Option<String> = None;
        for item in routing {
            let arn = item.state_machine_version_arn.as_str();
            let parsed =
                read_state_machine_arn(arn, endpoint_region).map_err(RoutingRejection::InvalidArn)?;
            // A routing configuration routes to VERSIONS: an unqualified ARN, a
            // map-run labeled one and an alias ARN each name something that is not
            // a version, so none of them is a configuration this can be applied to.
            let version = parsed
                .qualifier
                .as_deref()
                .filter(|_| parsed.label().is_none())
                .and_then(qualifier_version);
            if version.is_none() {
                return Err(RoutingRejection::InvalidRouting(format!(
                    "{arn} is not the ARN of a state machine version"
                )));
            }
            // The version's own state machine is its ARN without the qualifier the
            // reader above just took off the end.
            let machine = arn
                .rsplit_once(':')
                .map(|(machine, _)| machine.to_string())
                .unwrap_or_default();
            match owner.as_deref() {
                Some(first) if first != machine => {
                    return Err(RoutingRejection::InvalidRouting(format!(
                        "every version of a routing configuration must belong to one state \
                         machine, got {first} and {machine}"
                    )))
                }
                Some(_) => {}
                None => owner = Some(machine),
            }
        }
        let Some(owner) = owner else {
            // UNREACHABLE through the wire: `routingConfiguration` is modelled
            // 1..2, so the centralized walk has already refused an empty list.
            return Err(RoutingRejection::InvalidRouting(
                "routingConfiguration must name at least one state machine version".to_string(),
            ));
        };

        let weights: Vec<i32> = routing.iter().map(|item| item.weight).collect();
        let total = weights.iter().copied().fold(0_i32, i32::saturating_add);
        if total != ROUTING_WEIGHT_TOTAL {
            return Err(RoutingRejection::InvalidRouting(routing_weight_sum_message(
                &weights,
            )));
        }

        for item in routing {
            if !self
                .state_machine_versions
                .contains_key(&item.state_machine_version_arn)
            {
                return Err(RoutingRejection::VersionNotFound(
                    item.state_machine_version_arn.clone(),
                ));
            }
        }
        // A version is removed with the state machine it belongs to, so by here the
        // state machine is stored; one whose deletion has begun can take no more
        // configuration.
        if self
            .state_machines
            .get(&owner)
            .is_some_and(StateMachine::is_deleting)
        {
            return Err(RoutingRejection::Deleting(owner));
        }
        Ok(owner)
    }

    /// What the state machine ARN `arn` refers to, or `None` when the store holds no
    /// record it names.
    ///
    /// The ONE place a qualified state machine ARN is resolved, so every operation
    /// that takes one reads it identically: a qualifier of nothing but digits is a
    /// VERSION ([`qualifier_version`]) and any other qualifier is an ALIAS name, each
    /// stored under the qualified ARN itself, and both resolve to the state machine
    /// that owns them. A reference the store holds nothing for is a resource that is
    /// absent, which is what the caller's own not-found error reports; the ARN itself
    /// is judged earlier, by [`read_state_machine_arn`].
    pub fn read_state_machine_reference(
        &self,
        arn: &str,
        parsed: &StateMachineArn,
    ) -> Option<StateMachineReference> {
        // A map-run LABELED ARN names a Distributed Map state inside a state
        // machine, so its trailing field is not a qualifier and nothing here
        // answers for it.
        let qualifier = parsed
            .qualifier
            .as_deref()
            .filter(|_| parsed.label().is_none());
        let Some(qualifier) = qualifier else {
            let machine = self.state_machines.get(arn)?;
            return Some(StateMachineReference {
                state_machine_arn: machine.arn.clone(),
                version_arn: None,
                alias_arn: None,
            });
        };
        if qualifier_version(qualifier).is_some() {
            let version = self.state_machine_versions.get(arn)?;
            return Some(StateMachineReference {
                state_machine_arn: version.state_machine_arn.clone(),
                version_arn: Some(version.arn.clone()),
                alias_arn: None,
            });
        }
        let alias = self.state_machine_aliases.get(arn)?;
        Some(StateMachineReference {
            state_machine_arn: alias.state_machine_arn.clone(),
            version_arn: None,
            alias_arn: Some(alias.arn.clone()),
        })
    }

    /// The version one alias routes an execution to, drawn in proportion to the
    /// weights of its routing configuration, or `None` when the store holds no
    /// such alias.
    ///
    /// "You can also map an alias to split StartExecution requests between two
    /// versions of a state machine. [...] Step Functions randomly chooses which
    /// version runs a given execution based on the percentage you specify" — so the
    /// draw is made once per start, and an entry's share of the total weight is its
    /// chance of being drawn. An entry at weight 0 is therefore never drawn.
    pub fn routed_version(&mut self, alias_arn: &str) -> Option<String> {
        let routing: Vec<(String, u64)> = self
            .state_machine_aliases
            .get(alias_arn)?
            .routing_configuration
            .iter()
            .map(|item| {
                (
                    item.state_machine_version_arn.clone(),
                    u64::try_from(item.weight).unwrap_or(0),
                )
            })
            .collect();
        let total: u64 = routing.iter().map(|(_, weight)| weight).sum();
        // "The sum of the weights in the routing configuration must be equal to
        // 100", so the span below is positive; a configuration that somehow adds up
        // to nothing still routes, to the first version it names.
        let draw = self.draw_below(total);
        let mut reached = 0_u64;
        routing
            .iter()
            .find_map(|(version_arn, weight)| {
                reached = reached.saturating_add(*weight);
                (draw < reached).then(|| version_arn.clone())
            })
            .or_else(|| routing.first().map(|(version_arn, _)| version_arn.clone()))
    }

    /// The run the state machine ARN `arn` names, or `None` when the store holds no
    /// record it refers to.
    ///
    /// This is what both start operations resolve their `stateMachineArn` through: an
    /// unqualified ARN runs the state machine's CURRENT configuration, a version ARN
    /// runs that version's snapshot, and an alias ARN runs the version its routing
    /// configuration draws ([`Self::routed_version`]) — and the version and alias the
    /// ARN named are carried onto the execution, which is what every execution report
    /// attributes it by.
    pub fn read_run_target(&mut self, arn: &str, parsed: &StateMachineArn) -> Option<RunTarget> {
        let reference = self.read_state_machine_reference(arn, parsed)?;
        let alias_arn = reference.alias_arn.clone();
        let version_arn = match alias_arn.as_deref() {
            Some(alias_arn) => self.routed_version(alias_arn),
            None => reference.version_arn.clone(),
        };
        if alias_arn.is_some() || version_arn.is_some() {
            // A version is immutable, so what it holds IS what this run executes.
            let version = self.state_machine_versions.get(version_arn.as_deref()?)?;
            return Some(RunTarget {
                state_machine_arn: version.state_machine_arn.clone(),
                name: version.name.clone(),
                sm_type: version.sm_type.clone(),
                configuration: RanConfiguration::of_version(version),
                version_arn: Some(version.arn.clone()),
                alias_arn,
            });
        }
        let machine = self.state_machines.get(&reference.state_machine_arn)?;
        Some(RunTarget {
            state_machine_arn: machine.arn.clone(),
            name: machine.name.clone(),
            sm_type: machine.sm_type.clone(),
            configuration: RanConfiguration::of_state_machine(machine),
            version_arn: None,
            alias_arn: None,
        })
    }

    /// Apply `update` to the state machine `arn` names: every member the update
    /// carries replaces the stored one, and every member it leaves out keeps the
    /// value the state machine holds.
    ///
    /// "Updates an existing state machine by modifying its definition, roleArn,
    /// loggingConfiguration, or EncryptionConfiguration" — so this is the write that
    /// makes the state machine's CURRENT configuration differ from the one it was
    /// created with, which is what a later `DescribeStateMachine` reports and what
    /// `CreateStateMachine`'s idempotency check compares a repeat against.
    ///
    /// The definition is caller-sized, so the record's measurement moves by what the
    /// write changed and an update that would take the store past
    /// [`MAX_STORE_BYTES`] is refused instead of growing it, leaving the state
    /// machine exactly as it was.
    pub fn apply_state_machine_update(
        &mut self,
        arn: &str,
        update: StateMachineUpdate,
    ) -> Result<(), StateMachineUpdateRejection> {
        let Some(machine) = self.state_machines.get(arn) else {
            return Err(StateMachineUpdateRejection::NotFound);
        };
        if machine.is_deleting() {
            return Err(StateMachineUpdateRejection::Deleting);
        }

        let mut updated = machine.clone();
        if let Some(definition) = update.definition {
            updated.definition = definition;
        }
        if let Some(role_arn) = update.role_arn {
            updated.role_arn = role_arn;
        }
        if update.logging_configuration.is_some() {
            updated.logging_configuration = update.logging_configuration;
        }
        if update.tracing_configuration.is_some() {
            updated.tracing_configuration = update.tracing_configuration;
        }
        if update.encryption_configuration.is_some() {
            updated.encryption_configuration = update.encryption_configuration;
        }
        updated.revision_id = Some(update.revision_id);
        updated.update_date = update.now;

        let before = machine.retained_bytes;
        let after = updated.measure();
        if after > before && !self.has_room_for(after - before) {
            return Err(StateMachineUpdateRejection::NoRoom);
        }
        updated.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.state_machines.insert(arn.to_string(), updated);
        Ok(())
    }

    /// Begin deleting the state machine `arn` names: it reports `DELETING` from now
    /// and is removed once [`DELETE_SETTLE_DELAY`] has passed. `false` when nothing
    /// matched, which `DeleteStateMachine` answers as a success all the same.
    pub fn begin_delete_state_machine(&mut self, arn: &str, now: SystemTime) -> bool {
        let Some(machine) = self.state_machines.get_mut(arn) else {
            return false;
        };
        if machine.is_deleting() {
            return true;
        }
        machine.status = STATE_MACHINE_STATUS_DELETING.to_string();
        machine.delete_due_at = Some(now + DELETE_SETTLE_DELAY);
        self.deleting_state_machines.push(arn.to_string());
        true
    }

    /// Remove a state machine, its versions, its aliases and its executions.
    ///
    /// The executions go with it, so no execution ARN is left naming a state machine
    /// that no longer exists, and so do its published versions: "when you delete a
    /// state machine, its versions are deleted too", and a version ARN that outlived
    /// its state machine would answer for a resource that is gone. An alias points at
    /// versions of this one state machine, so it cannot outlive them either.
    pub fn remove_state_machine(&mut self, arn: &str) -> Option<StateMachine> {
        let machine = self.state_machines.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(machine.retained_bytes);
        let version_arns: Vec<String> = self
            .versions_of(arn)
            .into_iter()
            .map(|version| version.arn.clone())
            .collect();
        for version_arn in version_arns {
            if let Some(version) = self.state_machine_versions.remove(&version_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(version.retained_bytes);
            }
        }
        let alias_arns: Vec<String> = self
            .aliases_of(arn)
            .into_iter()
            .map(|alias| alias.arn.clone())
            .collect();
        for alias_arn in alias_arns {
            if let Some(alias) = self.state_machine_aliases.remove(&alias_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(alias.retained_bytes);
            }
        }
        for execution_arn in self.executions_by_sm.remove(arn).unwrap_or_default() {
            if let Some(execution) = self.executions.remove(&execution_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(execution.retained_bytes);
            }
            // An outstanding activity task names the execution it belongs to, so it
            // goes with it: no token is left for a worker to answer against an
            // execution that no longer exists.
            self.discard_tasks_of_executions(std::slice::from_ref(&execution_arn));
            // A Map Run names the execution that started it, for the same reason.
            self.discard_map_runs_of(&execution_arn);
        }
        Some(machine)
    }

    /// The state machines of one region, in ARN order.
    ///
    /// A regional endpoint reports only its own region's resources, and every state
    /// machine ARN begins with [`state_machine_arn_prefix`] for its region, so the
    /// `BTreeMap` holds one region's records contiguously and this is a range scan
    /// rather than a filter over the whole store.
    pub fn state_machines_in_region(&self, region: &str) -> Vec<&StateMachine> {
        let prefix = state_machine_arn_prefix(region);
        self.state_machines
            .range(prefix.clone()..)
            .take_while(|(arn, _)| arn.starts_with(&prefix))
            .map(|(_, machine)| machine)
            .collect()
    }

    /// Why the store cannot take one more state machine of `bytes`, or `None` when
    /// it can. The caps are on the WHOLE store, so no value a caller chooses — the
    /// signed region included — can mint a fresh quota.
    pub fn state_machine_rejection(&self, bytes: usize) -> Option<String> {
        if self.state_machines.len() >= MAX_STATE_MACHINES {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_STATE_MACHINES} state machines"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this state machine would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// What a `StartExecution` whose execution ARN the store ALREADY holds answers,
    /// or `None` when the ARN is free.
    ///
    /// "StartExecution is idempotent for STANDARD workflows. [...] if you call
    /// StartExecution with the same name and input as a running execution, the call
    /// succeeds and return the same response as the original request. If the
    /// execution is closed or if the input is different, it returns a 400
    /// ExecutionAlreadyExists error." It "isn't idempotent for EXPRESS workflows".
    ///
    /// The decision lives HERE because `StartExecution` takes it twice: once before
    /// the definition is interpreted, so a repeat of an idempotent start does not
    /// make a `Task` state's service integration calls a second time, and once with
    /// the admitting lock held, because the name could have been taken while the
    /// walk ran.
    pub fn started_already(
        &self,
        execution_arn: &str,
        standard: bool,
        input: &str,
    ) -> Option<StartedAlready> {
        let existing = self.executions.get(execution_arn)?;
        if standard && !existing.is_terminal() && existing.input == input {
            return Some(StartedAlready::Idempotent {
                start_date: existing.start_date,
            });
        }
        Some(StartedAlready::NameTaken)
    }

    /// Why the store cannot take one more execution of `bytes` for
    /// `state_machine_arn`, or `None` when it can.
    pub fn execution_rejection(&self, state_machine_arn: &str, bytes: usize) -> Option<String> {
        if self.execution_count(state_machine_arn) >= MAX_EXECUTIONS_PER_STATE_MACHINE {
            return Some(format!(
                "state machine {state_machine_arn} already holds the maximum of \
                 {MAX_EXECUTIONS_PER_STATE_MACHINE} executions"
            ));
        }
        if self.executions.len() >= MAX_EXECUTIONS {
            return Some(format!(
                "this emulator already holds the maximum of {MAX_EXECUTIONS} executions"
            ));
        }
        if !self.has_room_for(bytes) {
            return Some(format!(
                "this execution would take the store past the {MAX_STORE_BYTES} bytes this \
                 emulator retains"
            ));
        }
        None
    }

    /// Store a newly started execution and index it under its state machine,
    /// counting what it retains.
    ///
    /// The ARN must not already be stored: `StartExecution` refuses a name already in
    /// use before reaching here, so there is no duplicate to guard against and
    /// `executions_by_sm` is appended to unconditionally.
    pub fn insert_execution(&mut self, mut execution: Execution) {
        execution.retained_bytes = execution.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(execution.retained_bytes);
        if execution.pending.is_some() {
            self.pending_executions.push(execution.arn.clone());
        }
        self.executions_by_sm
            .entry(execution.state_machine_arn.clone())
            .or_default()
            .push(execution.arn.clone());
        self.executions.insert(execution.arn.clone(), execution);
    }

    // -----------------------------------------------------------------------
    // Map runs
    // -----------------------------------------------------------------------

    /// Store the Map Runs one attempt of `execution_arn`'s definition started, and
    /// index them under that execution.
    ///
    /// Called once the execution has been ADMITTED, against bytes that already
    /// include these records' ([`PlannedMapRun::measure`]).
    ///
    /// One attempt's Map Runs are POSITIONAL: the walk reports them in the order it
    /// entered their `Map` states, and a redrive re-runs the same definition over the
    /// same input, so its walk enters the same states in the same order. The record
    /// already at a position therefore belongs to the same `Map` state and MOVES —
    /// keeping its ARN, which a client may be holding, and counting the redrive —
    /// while a position no earlier attempt reached gets a Map Run of its own. The
    /// position is what identifies it, not the label, because one walk can enter one
    /// `Map` state many times (a Distributed Map nested inside another runs once per
    /// outer item) and each of those entries is a Map Run in its own right.
    ///
    /// A record an earlier attempt left at a position this one did not reach, or at
    /// one this one entered a DIFFERENT state at, is no longer a Map Run of this
    /// execution and is dropped rather than left listed under it.
    pub fn record_map_runs(
        &mut self,
        execution_arn: &str,
        state_machine_arn: &str,
        state_machine_name: &str,
        region: &str,
        planned: Vec<PlannedMapRun>,
    ) {
        // An attempt that entered no Distributed Map state reports nothing, and says
        // nothing about the records an earlier attempt left.
        if planned.is_empty() {
            return;
        }
        let previous: Vec<String> = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default();
        let mut recorded: Vec<String> = Vec::with_capacity(planned.len());
        for (position, plan) in planned.into_iter().enumerate() {
            let reused = previous
                .get(position)
                .filter(|arn| {
                    self.map_runs
                        .get(*arn)
                        .is_some_and(|map_run| map_run.label == plan.label)
                })
                .cloned();
            let arn = match reused {
                Some(arn) => arn,
                None => {
                    let id = self.fresh_uuid();
                    map_run_arn(region, state_machine_name, &plan.label, &id)
                }
            };
            self.put_map_run(arn.clone(), execution_arn, state_machine_arn, plan);
            recorded.push(arn);
        }
        for arn in previous {
            if recorded.contains(&arn) {
                continue;
            }
            if let Some(map_run) = self.map_runs.remove(&arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(map_run.retained_bytes);
            }
            self.pending_map_runs.retain(|pending| pending != &arn);
            // A record that is no longer a Map Run of this execution takes the child
            // workflow executions it had started with it.
            self.discard_child_executions_of(&arn);
        }
        self.map_runs_by_execution
            .insert(execution_arn.to_string(), recorded);
    }

    /// Record the Map Runs a CONTINUATION of `execution_arn` started — the states a
    /// worker's reply to an activity task carried the walk on through — keeping the
    /// ones the attempt's earlier states already started.
    ///
    /// A reply continues ONE attempt rather than starting another, so the records
    /// already listed under this execution stand: they belong to states this walk
    /// passed before the task, which happened once and are not re-entered. Each Map
    /// Run here is therefore a position no earlier call reached, appended after them
    /// with an ARN of its own.
    pub fn append_map_runs(
        &mut self,
        execution_arn: &str,
        state_machine_arn: &str,
        state_machine_name: &str,
        region: &str,
        planned: Vec<PlannedMapRun>,
    ) {
        if planned.is_empty() {
            return;
        }
        let mut recorded: Vec<String> = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default();
        for plan in planned {
            let id = self.fresh_uuid();
            let arn = map_run_arn(region, state_machine_name, &plan.label, &id);
            self.put_map_run(arn.clone(), execution_arn, state_machine_arn, plan);
            recorded.push(arn);
        }
        self.map_runs_by_execution
            .insert(execution_arn.to_string(), recorded);
    }

    /// Store one Map Run under `arn`, counting what it retains and queueing the
    /// terminal status its items produced for the clock.
    ///
    /// A record already at that ARN is the SAME Map Run running again — which is what
    /// a redrive of its execution does — so it keeps the ARN a client may be holding
    /// and counts the redrive; the bytes it held are given back and the new record's
    /// are counted.
    fn put_map_run(
        &mut self,
        arn: String,
        execution_arn: &str,
        state_machine_arn: &str,
        plan: PlannedMapRun,
    ) {
        let (redrive_count, retained_before) = match self.map_runs.get(&arn) {
            Some(map_run) => (
                map_run.redrive_count.saturating_add(1),
                map_run.retained_bytes,
            ),
            None => (0, 0),
        };
        let mut record = MapRun {
            arn: arn.clone(),
            execution_arn: execution_arn.to_string(),
            state_machine_arn: state_machine_arn.to_string(),
            label: plan.label,
            // The items ran in the walk, but the Map Run reaches its terminal
            // status when the clock says it did, exactly as its execution does:
            // a caller reading straight after the start sees it RUNNING.
            status: MAP_RUN_STATUS_RUNNING.to_string(),
            start_date: plan.start_date,
            stop_date: None,
            pending: Some(PendingMapRunOutcome {
                due_at: plan.due_at,
                status: plan.status,
            }),
            max_concurrency: plan.max_concurrency,
            tolerated_failure_count: plan.tolerated_failure_count,
            tolerated_failure_percentage: plan.tolerated_failure_percentage,
            total_items: plan.total_items,
            // Nothing this Map Run iterated is counted yet: each item is counted from
            // the instant its own iteration finished, which the clock has still to
            // reach, exactly as the Map Run's status is.
            succeeded_items: 0,
            failed_items: 0,
            items: plan.items,
            redrive_count,
            // "If you have not yet redriven [...] the redriveDate is null": the
            // instant the re-run that produced this record began.
            redrive_date: (redrive_count > 0).then_some(plan.start_date),
            retained_bytes: 0,
        };
        record.retained_bytes = record.measure();
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(record.retained_bytes)
            .saturating_sub(retained_before);
        self.pending_map_runs.retain(|pending| pending != &arn);
        self.pending_map_runs.push(arn.clone());
        self.map_runs.insert(arn.clone(), record);
        // The children of the attempt this record is REPLACING belong to a run of
        // this `Map` state that happened before: a redrive re-ran the state, so the
        // executions it started then are gone and the ones it started now take their
        // place under the same Map Run.
        self.discard_child_executions_of(&arn);
        self.record_child_executions(&arn, execution_arn, plan.children);
    }

    /// Store the CHILD workflow executions one Map Run's items ran as, and index them
    /// under that Map Run.
    ///
    /// Each one is an execution in its own right, with the input its item was handed
    /// and the terminal status its iteration reached owed to the clock exactly as the
    /// Map Run's own status is — so a caller reading one straight after the start sees
    /// it `RUNNING`. It runs the state machine its PARENT ran, which is the state
    /// machine the Map Run reports, so it holds that parent's configuration snapshot
    /// rather than a copy of it.
    ///
    /// Called with the bytes already admitted ([`PlannedChildExecution::measure`]).
    fn record_child_executions(
        &mut self,
        map_run_arn: &str,
        execution_arn: &str,
        children: Vec<PlannedChildExecution>,
    ) {
        if children.is_empty() {
            return;
        }
        // The parent is stored before its Map Runs are, so what it RAN is in hand;
        // without it there is no state machine for a child to have run and nothing to
        // record.
        let Some(parent) = self.executions.get(execution_arn) else {
            return;
        };
        let ran = std::sync::Arc::clone(&parent.ran);
        let state_machine_arn = parent.state_machine_arn.clone();
        let version_arn = parent.state_machine_version_arn.clone();
        let alias_arn = parent.state_machine_alias_arn.clone();
        let trace_header = parent.trace_header.clone();
        let mut recorded = Vec::with_capacity(children.len());
        for child in children {
            let opened = HistoryEvent {
                id: 1,
                previous_event_id: 0,
                event_type: EVENT_EXECUTION_STARTED.to_string(),
                timestamp: child.start_date,
                state_name: None,
                details: EventDetails::default(),
            };
            let terminal = HistoryEvent {
                id: 2,
                // The event after an `ExecutionStarted` has no predecessor — see
                // [`follows_event`].
                previous_event_id: follows_event(opened.id, &opened.event_type),
                event_type: if child.status == EXECUTION_STATUS_SUCCEEDED {
                    EVENT_EXECUTION_SUCCEEDED.to_string()
                } else {
                    EVENT_EXECUTION_FAILED.to_string()
                },
                timestamp: child.due_at,
                state_name: None,
                details: EventDetails::of_outcome(
                    child.output.clone(),
                    child.error.clone(),
                    child.cause.clone(),
                ),
            };
            let record = Execution {
                arn: child.arn.clone(),
                state_machine_arn: state_machine_arn.clone(),
                name: child.name,
                status: EXECUTION_STATUS_RUNNING.to_string(),
                start_date: child.start_date,
                stop_date: None,
                input: child.input,
                output: None,
                error: None,
                cause: None,
                trace_header: trace_header.clone(),
                state_machine_version_arn: version_arn.clone(),
                state_machine_alias_arn: alias_arn.clone(),
                ran: std::sync::Arc::clone(&ran),
                redrive_count: 0,
                redrive_date: None,
                redrive_tokens: Vec::new(),
                history: vec![opened],
                redrive_step: None,
                pending: Some(PendingOutcome {
                    due_at: child.due_at,
                    status: child.status,
                    output: child.output,
                    error: child.error,
                    cause: child.cause,
                    events: vec![terminal],
                }),
                map_run_arn: Some(map_run_arn.to_string()),
                item_count: Some(child.item_count),
                // Measured and counted below, exactly as `insert_execution` does.
                retained_bytes: 0,
                // No read of this child's history has handed a page cursor out yet.
                issued_page_cursors: Vec::new(),
            };
            recorded.push(record.arn.clone());
            self.insert_child_execution(record);
        }
        self.executions_by_map_run
            .insert(map_run_arn.to_string(), recorded);
    }

    /// Store one child workflow execution and count what it retains.
    ///
    /// It is indexed under its Map Run by its caller, NOT under its state machine:
    /// `ListExecutions` reports a Map Run's children for that Map Run alone.
    fn insert_child_execution(&mut self, mut execution: Execution) {
        execution.retained_bytes = execution.measure();
        self.retained_bytes = self.retained_bytes.saturating_add(execution.retained_bytes);
        if execution.pending.is_some() {
            self.pending_executions.push(execution.arn.clone());
        }
        self.executions.insert(execution.arn.clone(), execution);
    }

    /// Drop every child workflow execution of the Map Run `map_run_arn` names, so no
    /// execution is left under a Map Run whose items it no longer belongs to.
    fn discard_child_executions_of(&mut self, map_run_arn: &str) {
        for arn in self
            .executions_by_map_run
            .remove(map_run_arn)
            .unwrap_or_default()
        {
            if let Some(execution) = self.executions.remove(&arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(execution.retained_bytes);
            }
            self.pending_executions.retain(|pending| pending != &arn);
            self.discard_tasks_of_executions(std::slice::from_ref(&arn));
        }
    }

    /// One Map Run's child workflow executions, most recent first.
    ///
    /// The same order `ListExecutions` reports a state machine's executions in
    /// ([`Self::executions_newest_first`]), because it is the same listing: "results
    /// are sorted by time, with the most recent execution first".
    pub fn child_executions_newest_first(&self, map_run_arn: &str) -> Vec<&Execution> {
        let mut children: Vec<&Execution> = self
            .executions_by_map_run
            .get(map_run_arn)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|arn| self.executions.get(arn))
            .collect();
        children.sort_by(|left, right| newest_first(left, right));
        children
    }

    /// Apply `UpdateMapRun`'s configuration to the Map Run `arn` names.
    ///
    /// Only the members the request CARRIED are changed; the others keep what the
    /// `Map` state declared, because the request states a change to the settings it
    /// names and says nothing about the rest.
    ///
    /// What the Map Run's items PRODUCED is not touched, and neither is the status
    /// they left it owed: the iterations are not re-run by a configuration change,
    /// and the execution around the Map Run reached its own outcome through the same
    /// `Map` state — so re-deciding the Map Run's status here alone would leave the
    /// two contradicting each other, with a Map Run reported as succeeded inside an
    /// execution reported as failed at it.
    ///
    /// The thresholds this writes ARE the failure budget the Map Run goes on running
    /// under: [`MapRun::advance`] weighs the failures it counts against what this
    /// record carries at that instant, so a budget a caller raised keeps a Map Run
    /// running over failures the budget it started with would have stopped it on.
    ///
    /// Call [`Self::settle`] first, so a Map Run the clock has already finished is
    /// refused as the completed Map Run it is.
    pub fn update_map_run(
        &mut self,
        arn: &str,
        max_concurrency: Option<i32>,
        tolerated_failure_count: Option<i64>,
        tolerated_failure_percentage: Option<f32>,
    ) -> Result<(), MapRunUpdateRejection> {
        let Some(map_run) = self.map_runs.get_mut(arn) else {
            return Err(MapRunUpdateRejection::NotFound);
        };
        // "Updates an IN-PROGRESS Map Run's configuration": a Map Run that has
        // reached a terminal status has no configuration left to govern it.
        if map_run.is_terminal() {
            return Err(MapRunUpdateRejection::Completed);
        }
        let before = map_run.retained_bytes;
        if let Some(max_concurrency) = max_concurrency {
            // A caller's `0` is the same "no limit" the `Map` state's own default is,
            // so the record reports it the one way a Map Run reports no limit.
            map_run.max_concurrency = reported_max_concurrency(i64::from(max_concurrency));
        }
        if let Some(tolerated_failure_count) = tolerated_failure_count {
            map_run.tolerated_failure_count = tolerated_failure_count;
        }
        if let Some(tolerated_failure_percentage) = tolerated_failure_percentage {
            map_run.tolerated_failure_percentage = tolerated_failure_percentage;
        }
        // The settings a caller sends cost no bytes of their own, but the record is
        // re-measured exactly as every other write to one is, so the store's total
        // never depends on which write path touched it last.
        map_run.retained_bytes = map_run.measure();
        let after = map_run.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        Ok(())
    }

    /// One execution's Map Runs, in the order their `Map` states were entered.
    pub fn map_runs_of(&self, execution_arn: &str) -> Vec<&MapRun> {
        self.map_runs_by_execution
            .get(execution_arn)
            .map(|arns| {
                arns.iter()
                    .filter_map(|arn| self.map_runs.get(arn))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stop every Map Run of `execution_arn` that has not finished, at `now`, and
    /// every child workflow execution of theirs that has not finished either.
    ///
    /// A Map Run runs INSIDE its execution, so an execution that was stopped before
    /// its Map Run's items were due stopped them too: the terminal status the walk
    /// had computed is discarded along with its place in the settle queue, exactly as
    /// the execution's own is.
    fn abort_map_runs_of(&mut self, execution_arn: &str, now: SystemTime) {
        let arns = self
            .map_runs_by_execution
            .get(execution_arn)
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(map_run) = self.map_runs.get_mut(&arn) else {
                continue;
            };
            if map_run.is_terminal() {
                continue;
            }
            let before = map_run.retained_bytes;
            map_run.pending = None;
            // The items the clock had not reached are never going to finish: the
            // execution that was iterating them was stopped.
            map_run.items.clear();
            map_run.status = MAP_RUN_STATUS_ABORTED.to_string();
            map_run.stop_date = Some(now);
            map_run.retained_bytes = map_run.measure();
            let after = map_run.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
            self.pending_map_runs.retain(|pending| pending != &arn);
            self.abort_child_executions_of(&arn, now);
        }
    }

    /// Stop every child workflow execution of the Map Run `map_run_arn` names that
    /// has not reached a terminal status, at `now`.
    ///
    /// A child runs INSIDE its Map Run, so the outcome the walk computed for one the
    /// clock had not reached is discarded along with its place in the settle queue,
    /// exactly as the Map Run's own status and the execution's around it are.
    fn abort_child_executions_of(&mut self, map_run_arn: &str, now: SystemTime) {
        let arns = self
            .executions_by_map_run
            .get(map_run_arn)
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(child) = self.executions.get_mut(&arn) else {
                continue;
            };
            if child.stop_date.is_some() {
                continue;
            }
            let before = child.retained_bytes;
            child.pending = None;
            child.status = EXECUTION_STATUS_ABORTED.to_string();
            child.stop_date = Some(now);
            child.push_event(EVENT_EXECUTION_ABORTED, now);
            child.retained_bytes = child.measure();
            let after = child.retained_bytes;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(after)
                .saturating_sub(before);
            self.pending_executions.retain(|pending| pending != &arn);
        }
    }

    /// Drop every Map Run of `execution_arn`, so no Map Run ARN is left naming one
    /// whose execution no longer exists.
    fn discard_map_runs_of(&mut self, execution_arn: &str) {
        for arn in self
            .map_runs_by_execution
            .remove(execution_arn)
            .unwrap_or_default()
        {
            if let Some(map_run) = self.map_runs.remove(&arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(map_run.retained_bytes);
            }
            self.pending_map_runs.retain(|pending| pending != &arn);
            // The child workflow executions it started go with it, for the same
            // reason: no execution ARN is left naming one whose Map Run is gone.
            self.discard_child_executions_of(&arn);
        }
    }

    /// Stop the execution `arn` names, and report the `stopDate` it carries together
    /// with the job it was PARKED at, if any.
    ///
    /// The outcome the interpreter had computed is DISCARDED along with its place in
    /// the settle queue: the execution was stopped before the clock made that outcome
    /// due, so it never reached it. An execution that has already reached a terminal
    /// status is not moved — its own `stopDate` comes back, so two callers that stop
    /// the same execution see the same instant. `None` when nothing matched.
    ///
    /// A job a run-a-job task started is work of ANOTHER service, which this store
    /// cannot reach: the record of it comes back so the operation that stopped the
    /// execution cancels that job across the service boundary, which is what stopping
    /// an execution parked inside such a task does.
    pub fn stop_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        error: Option<String>,
        cause: Option<String>,
    ) -> Option<Stopped> {
        let execution = self.executions.get_mut(arn)?;
        if let Some(stop_date) = execution.stop_date {
            return Some(Stopped {
                stop_date,
                parked: None,
            });
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_ABORTED.to_string();
        execution.stop_date = Some(now);
        execution.error = error;
        execution.cause = cause;
        // The event carries the error and the cause the STOP reported, so the history
        // keeps them whatever a later attempt of this execution computes.
        execution.push_detailed_event(
            EVENT_EXECUTION_ABORTED,
            now,
            EventDetails::of_failure(execution.error.clone(), execution.cause.clone()),
        );
        // The caller's `error` and `cause` are caller-sized, so the record is
        // re-measured and the store's total moved by the difference. The record
        // already reserved their maxima while it was running (see
        // [`STOP_RESERVE_BYTES`]), so this difference can only ever be a shrink and
        // the stop path cannot take the store past [`MAX_STORE_BYTES`].
        execution.retained_bytes = execution.measure();
        let after = execution.retained_bytes;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        self.pending_executions.retain(|pending| pending != arn);
        // A Map Run of this execution had not finished either, so it stopped here too.
        self.abort_map_runs_of(arn, now);
        // The job this execution was parked at is nobody's work any more: the record
        // leaves the store here — the next `settle` would drop it as orphaned anyway
        // — and comes back so its own service is told to stop it. An activity task
        // the execution was waiting at is dropped by that same settle, as it is for
        // an execution that reached a terminal status any other way.
        Some(Stopped {
            stop_date: now,
            parked: self.discard_job_task(arn),
        })
    }

    /// Why `RedriveExecution` would refuse `execution` at `now`, in the words
    /// `DescribeExecution`'s `redriveStatusReason` documents — or `None` when it
    /// would redrive it, which is exactly when `redriveStatus` is `REDRIVABLE`
    /// ("redriveStatus is NOT_REDRIVABLE if calling the RedriveExecution API action
    /// would return the ExecutionNotRedrivable error").
    ///
    /// The conditions are `RedriveExecution`'s own: "You can redrive executions if
    /// your original execution meets the following conditions: The execution status
    /// isn't SUCCEEDED. Your workflow execution has not exceeded the redrivable
    /// period of 14 days [...] The workflow execution has not exceeded the maximum
    /// open time of one year [...] The execution event history count is less than
    /// 24,999." An execution that has not reached a terminal status has not finished,
    /// so there is nothing to continue from, and a state machine that is being
    /// deleted can run nothing: both are reasons that member's list names too.
    pub fn not_redrivable_reason(
        &self,
        execution: &Execution,
        now: SystemTime,
    ) -> Option<&'static str> {
        if !execution.is_terminal() {
            return Some(NOT_REDRIVABLE_RUNNING);
        }
        if execution.status == EXECUTION_STATUS_SUCCEEDED {
            return Some(NOT_REDRIVABLE_SUCCEEDED);
        }
        // A state machine whose deletion has begun runs nothing; one the store no
        // longer holds at all is past the end of that same deletion.
        if self
            .state_machines
            .get(&execution.state_machine_arn)
            .is_none_or(StateMachine::is_deleting)
        {
            return Some(NOT_REDRIVABLE_DELETING);
        }
        // "Redriven executions append their event history to the existing event
        // history. Make sure your workflow execution contains less than 24,999
        // events to accommodate the ExecutionRedriven history event and at least one
        // other history event." This emulator's own history ceiling is lower, and an
        // execution already at it has no room for that event either, so whichever is
        // reached first is this reason.
        if execution.history.len() + 2 > MAX_REDRIVABLE_HISTORY_EVENTS.min(MAX_HISTORY_EVENTS) {
            return Some(NOT_REDRIVABLE_EVENT_LIMIT);
        }
        if now
            .duration_since(execution.start_date)
            .is_ok_and(|open| open > MAX_OPEN_DURATION)
        {
            return Some(NOT_REDRIVABLE_MAX_EXECUTION_TIME);
        }
        // The redrivable period "starts from the day a state machine completes its
        // execution", which is the execution's own `stopDate`.
        if execution
            .stop_date
            .and_then(|completed| now.duration_since(completed).ok())
            .is_some_and(|since| since > REDRIVABLE_PERIOD)
        {
            return Some(NOT_REDRIVABLE_PERIOD);
        }
        None
    }

    /// Record a redrive of the execution `arn` names, with the opening history event
    /// and the outcome a re-run of its definition computed.
    ///
    /// The execution reports `RUNNING` again until the clock makes that outcome due,
    /// its `redriveCount` and `redriveDate` move, and `client_token` — when the
    /// caller supplied one — is remembered so a repeat answers the same
    /// `redriveDate`. Answers the `redriveDate` the redrive carries.
    ///
    /// `extra_bytes` is what the activity task the re-run stopped at and the Map Runs
    /// it re-entered will retain, so a redrive is admitted against the whole of what
    /// it adds.
    pub fn redrive_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        client_token: Option<&str>,
        attempt: RedrivenAttempt,
    ) -> Result<SystemTime, RedriveRejection> {
        let RedrivenAttempt {
            opening,
            pending,
            extra_bytes,
            stopped_at,
        } = attempt;
        let Some(execution) = self.executions.get(arn) else {
            return Err(RedriveRejection::NotFound);
        };
        // A redrive GROWS the record — the outcome the re-run computed, the events it
        // appends and one remembered client token — so it is admitted against the
        // store's ceiling exactly as a start is, rather than growing it past
        // [`MAX_STORE_BYTES`].
        let before = execution.retained_bytes;
        let mut candidate = execution.clone();
        candidate.apply_redrive(now, client_token, opening, pending);
        // The step THIS attempt stopped at, which the next redrive continues from;
        // `apply_redrive` consumed the one the attempt before it left.
        candidate.redrive_step = stopped_at;
        let after = candidate.measure();
        if after + extra_bytes > before && !self.has_room_for(after + extra_bytes - before) {
            return Err(RedriveRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        // The re-run's outcome is owed to the clock, so this execution joins the
        // settle queue again; it left the queue when its first attempt settled. A
        // re-run that SUSPENDED at an activity task owes the clock nothing, so it
        // does not rejoin it.
        self.pending_executions.retain(|pending| pending != arn);
        if candidate.pending.is_some() {
            self.pending_executions.push(arn.to_string());
        }
        self.executions.insert(arn.to_string(), candidate);
        Ok(now)
    }

    /// How many executions this state machine already holds.
    pub fn execution_count(&self, state_machine_arn: &str) -> usize {
        self.executions_by_sm
            .get(state_machine_arn)
            .map_or(0, Vec::len)
    }

    /// One state machine's executions, most recent first ([`newest_first`]); the
    /// child workflow executions of its Map Runs are not among them.
    pub fn executions_newest_first(&self, state_machine_arn: &str) -> Vec<&Execution> {
        let mut executions: Vec<&Execution> = self
            .executions_by_sm
            .get(state_machine_arn)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|arn| self.executions.get(arn))
            .collect();
        executions.sort_by(|left, right| newest_first(left, right));
        executions
    }
}

/// How `ListExecutions` orders two executions: "results are sorted by time, with the
/// most recent execution first. Running executions are sorted by their `startDate`
/// [...] and other executions are sorted by their `stopDate`."
///
/// The sort key is therefore an execution's `stopDate` once it has one and its
/// `startDate` until then, descending; the ARN breaks ties so the order is total and
/// a page token means the same thing on the next call. ONE comparator for both
/// listings, so a Map Run's children and a state machine's executions are never
/// ordered two ways.
fn newest_first(left: &Execution, right: &Execution) -> std::cmp::Ordering {
    let left_key = left.stop_date.unwrap_or(left.start_date);
    let right_key = right.stop_date.unwrap_or(right.start_date);
    right_key
        .cmp(&left_key)
        .then_with(|| right.start_date.cmp(&left.start_date))
        .then_with(|| left.arn.cmp(&right.arn))
}

/// Why the clock closed an activity task, for the `cause` the execution it failed
/// reports.
///
/// The error NAME is the language's own ([`TASK_TIMEOUT_ERROR`]); this says which of
/// the scheduling state's two bounds ran out and what that bound was, which is built
/// from the definition rather than from any one request.
fn activity_task_timeout_cause(task: &ActivityTask) -> String {
    if task.closed_by_heartbeat() {
        let seconds = task.heartbeat_seconds.unwrap_or_default();
        return format!(
            "state '{}' sent no heartbeat for its activity task within the {seconds} seconds its \
             'HeartbeatSeconds' allows",
            task.state_name
        );
    }
    let seconds = task.timeout_seconds.unwrap_or_default();
    format!(
        "state '{}' ran its activity task for longer than the {seconds} seconds its \
         'TimeoutSeconds' allows",
        task.state_name
    )
}

/// Why the task that started a parked job ran out of time, for the `cause` the
/// execution it failed reports.
///
/// The error NAME is the language's own ([`TASK_TIMEOUT_ERROR`]); this says which
/// span ran out — the state's own `TimeoutSeconds` where it declares one, else how
/// long this emulator polls one job for — and how long that span was.
fn job_task_timeout_cause(task: &JobTask) -> String {
    match task.timeout_seconds {
        Some(seconds) => format!(
            "state '{}' ran the '{}' job it started for longer than the {seconds} seconds its \
             'TimeoutSeconds' allows",
            task.state_name, task.job.starts
        ),
        None => format!(
            "state '{}' ran the '{}' job it started for longer than the {} seconds this emulator \
             waits for one to finish",
            task.state_name,
            task.job.starts,
            MAX_PARKED_JOB_DURATION.as_secs()
        ),
    }
}

/// What stopping an execution left: the `stopDate` it carries, and the job it was
/// PARKED at when the stop reached it.
#[derive(Debug, Clone)]
pub struct Stopped {
    pub stop_date: SystemTime,
    /// The job a run-a-job task of this execution had started and nothing has
    /// finished, for the caller to stop through that job's own service.
    pub parked: Option<JobTask>,
}

/// One worker's open poll of an activity, counted against [`MAX_ACTIVITY_POLLERS`].
///
/// That count is what the modelled `ActivityWorkerLimitExceeded` reports — "the
/// maximum number of workers concurrently polling for activity tasks has been
/// reached" — so it has to fall again however the poll ends: this gives it back when
/// it is DROPPED, which covers the answer, every early return and a caller that went
/// away while its poll was held.
pub struct ActivityPoll {
    state: ::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
    activity_arn: String,
}

impl ActivityPoll {
    /// Count one more open poll of `activity_arn`, or answer `None` when
    /// [`MAX_ACTIVITY_POLLERS`] are already open on it — or
    /// [`MAX_OPEN_ACTIVITY_POLLS`] across the whole store.
    pub fn open(
        state: &::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
        activity_arn: &str,
    ) -> Option<Self> {
        {
            let mut store = state.lock();
            let open = store
                .activity_pollers
                .get(activity_arn)
                .copied()
                .unwrap_or_default();
            if open >= MAX_ACTIVITY_POLLERS
                || store.open_activity_polls >= MAX_OPEN_ACTIVITY_POLLS
            {
                return None;
            }
            store
                .activity_pollers
                .insert(activity_arn.to_string(), open + 1);
            store.open_activity_polls = store.open_activity_polls.saturating_add(1);
        }
        Some(Self {
            state: state.clone(),
            activity_arn: activity_arn.to_string(),
        })
    }
}

impl Drop for ActivityPoll {
    fn drop(&mut self) {
        let mut store = self.state.lock();
        store.open_activity_polls = store.open_activity_polls.saturating_sub(1);
        let open = store
            .activity_pollers
            .get(&self.activity_arn)
            .copied()
            .unwrap_or_default()
            .saturating_sub(1);
        if open == 0 {
            store.activity_pollers.remove(&self.activity_arn);
        } else {
            store
                .activity_pollers
                .insert(self.activity_arn.clone(), open);
        }
    }
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// What one `UpdateStateMachine` changes about a stored state machine.
///
/// Every member is optional because the operation changes only what the request
/// carries: "you must include at least one of definition or roleArn", and the
/// configuration structures it leaves out keep the values the state machine holds.
#[derive(Debug, Clone)]
pub struct StateMachineUpdate {
    pub definition: Option<String>,
    pub role_arn: Option<String>,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub tracing_configuration: Option<TracingConfiguration>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// The revision the update moves the state machine to; see
    /// [`ComAmazonawsSfnAwsstepfunctionsState::fresh_revision_id`].
    pub revision_id: String,
    /// When the update happened, reported as `updateDate`.
    pub now: SystemTime,
}

/// Why a state machine update was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::apply_state_machine_update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateMachineUpdateRejection {
    /// The store holds no state machine under that ARN — the modelled
    /// `StateMachineDoesNotExist`.
    NotFound,
    /// The state machine is being deleted — the modelled `StateMachineDeleting`.
    Deleting,
    /// The updated record would take the store past [`MAX_STORE_BYTES`].
    NoRoom,
}

/// What a Step Functions tagging call named in its `resourceArn`.
///
/// From `TagResource`'s and `ListTagsForResource`'s own `resourceArn`
/// documentation: "The Amazon Resource Name (ARN) for the Step Functions state
/// machine or activity." Those two resource types are the taggable ones, and an ARN
/// naming anything else — another service, another Step Functions resource type —
/// is what the modelled `InvalidArn` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggableResource {
    /// `arn:<partition>:states:<region>:<account>:stateMachine:<name>`, qualified or
    /// not.
    StateMachine(StateMachineArn),
    /// `arn:<partition>:states:<region>:<account>:activity:<name>`.
    Activity(String),
}

/// The taggable resource `arn` names, read as an ARN of this service in
/// `endpoint_region`, or the message the modelled `InvalidArn` carries.
pub fn read_taggable_arn(arn: &str, endpoint_region: &str) -> Result<TaggableResource, String> {
    let resource = states_arn_resource(arn, endpoint_region)?;
    let mut parts = resource.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    if kind == STATE_MACHINE_RESOURCE {
        return Ok(TaggableResource::StateMachine(state_machine_resource(
            arn, &mut parts,
        )?));
    }
    if kind != ACTIVITY_RESOURCE {
        return Err(resource_type_not_valid_message(kind));
    }
    Ok(TaggableResource::Activity(activity_resource(
        resource, &mut parts,
    )?))
}

/// The activity NAME the remaining resource fields name, shared by the readers that
/// accept an activity ARN.
fn activity_resource<'a>(
    resource: &str,
    parts: &mut impl Iterator<Item = &'a str>,
) -> Result<String, String> {
    let name = parts.next().unwrap_or_default();
    // An activity ARN carries no qualifier: only versions and aliases of a state
    // machine do.
    if name.is_empty() || parts.next().is_some() {
        return Err(invalid_arn_message(format!(
            "Invalid ARN {ACTIVITY_RESOURCE} resource: {resource}"
        )));
    }
    Ok(name.to_string())
}

/// Whether `arn` is an activity ARN of this service, in WHATEVER region it names.
///
/// A `Task` state's `Resource` names its activity by that activity's own full ARN,
/// which carries the region the activity lives in, and a definition is INTERPRETED
/// with no request endpoint in hand — so the region is read from the ARN itself here
/// rather than compared with an endpoint's. The task is then queued under that same
/// ARN, so a `Resource` naming a region this emulator holds no activity in queues a
/// task that no poll of this endpoint's ARNs can claim, which is what the store says.
pub fn names_an_activity(arn: &str) -> bool {
    read_activity_arn(arn, &arn_region(arn)).is_ok()
}

/// The activity NAME `arn` names, read as an activity ARN of this service in
/// `endpoint_region`, or the message the modelled `InvalidArn` carries.
pub fn read_activity_arn(arn: &str, endpoint_region: &str) -> Result<String, String> {
    let resource = states_arn_resource(arn, endpoint_region)?;
    let mut parts = resource.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    if kind != ACTIVITY_RESOURCE {
        return Err(resource_type_not_valid_message(kind));
    }
    activity_resource(resource, &mut parts)
}

/// `ResourceNotFound`'s condition, as its message spells it, for
/// [`resource_error_message`]. RECORDED text: the service states this condition in
/// SENTENCE case (`Resource not found: '<arn>'`) rather than in the spaced Title
/// Case its sibling conditions use.
pub const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// What the modelled `ResourceNotFound` says about an ARN the store holds no
/// resource under, so every operation words that refusal the same way.
///
/// The same rule as every other refusal that reports one value of the request —
/// the condition, then that value quoted ([`resource_error_message`]) — with the
/// ARN the request named as the quoted value.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    resource_error_message(RESOURCE_NOT_FOUND, resource_arn)
}

/// What the modelled `ResourceNotFound` says where the service states the condition
/// as a SENTENCE OF ITS OWN instead of naming the ARN: the resource the request
/// referenced is not there.
///
/// RECORDED text: `UpdateStateMachineAlias` answers this fixed sentence for an ALIAS
/// that is gone from a state machine the store still holds, where the alias's own
/// readers (`DescribeStateMachineAlias`, `DeleteStateMachineAlias`) quote the ARN
/// through [`resource_not_found_message`] — see
/// [`ComAmazonawsSfnAwsstepfunctionsState::missing_alias_message`] for which of the
/// two the mutation states. No value of the request appears in it — the ARN travels
/// in the shape's `resourceName` member, which every raiser of this error fills — so
/// a caller reads the refused resource from there rather than out of the sentence.
pub const RESOURCE_DOES_NOT_EXIST: &str = "Request references a resource that does not exist.";

/// What the modelled `ValidationException` says when the weights of a routing
/// configuration do not add up to [`ROUTING_WEIGHT_TOTAL`].
///
/// RECORDED text: the service states the RULE the configuration broke — "the sum of
/// the weights in the routing configuration must be equal to 100" — and then lists,
/// in brackets, the weights the request submitted, in the order it listed them, so a
/// caller sees which numbers it sent. The listed values are the request's own; the
/// total they add up to is not stated, because the weights are what the caller has
/// to change.
pub fn routing_weight_sum_message(weights: &[i32]) -> String {
    format!(
        "Sum of routing configuration weights must equal {ROUTING_WEIGHT_TOTAL}. Received: [{}]",
        weights
            .iter()
            .map(i32::to_string)
            .collect::<Vec<String>>()
            .join(", ")
    )
}

/// What the modelled `ResourceNotFound` says when a routing configuration names a
/// state machine version that was never published.
///
/// The alias operations do not resolve a routing configuration entry by a generic
/// resource lookup: the rule being enforced is the one the routing configuration
/// itself carries — every entry must reference a version that already exists — so
/// the refusal states THAT rule, in a sentence of its own that no value of the
/// request appears in. RECORDED text.
pub const ROUTING_CONFIGURATION_VERSION_NOT_FOUND: &str =
    "Routing configuration must contain state machine version ARNs that already exist.";

/// What the modelled `ActivityWorkerLimitExceeded` says: the activity whose pollers
/// are all open, and the number of them.
///
/// The error reports a LIMIT rather than a value of the request, which is why it is
/// worded here rather than through [`resource_error_message`] — the same way
/// `ExecutionLimitExceeded` and `StateMachineLimitExceeded` are. It names the
/// activity because a worker polling several of them needs to know which poll was
/// refused.
pub fn activity_worker_limit_message(activity_arn: &str) -> String {
    format!(
        "activity {activity_arn} already has the maximum of {MAX_ACTIVITY_POLLERS} workers \
         polling it, or this emulator is holding the {MAX_OPEN_ACTIVITY_POLLS} polls it holds at \
         once"
    )
}

// ---------------------------------------------------------------------------
// Error messages
// ---------------------------------------------------------------------------

/// The message a Step Functions client error carries: the CONDITION the error
/// names, in spaced Title Case, then `: ` and the value of the request that
/// condition is about, quoted.
///
/// This is built from the request through ONE rule — the recorded service answers
/// `State Machine Does Not Exist: '<arn>'` for `StateMachineDoesNotExist`,
/// `Execution Already Exists: '<arn>'` for `ExecutionAlreadyExists` and
/// `Invalid Name: '<name>'` for `InvalidName` — so every operation here words a
/// refusal through this, and none of them invents a sentence of its own. The
/// quoted value is whatever the condition is ABOUT: usually an ARN, but the name
/// the caller sent for `InvalidName`, and the definition diagnostic for
/// `InvalidDefinition`.
///
/// Errors whose subject is NOT a value of the request keep their own wording:
/// `StateMachineLimitExceeded`, `TooManyTags` and `ExecutionLimitExceeded`
/// report a limit, and `ValidationException` carries the standard validation
/// envelope (see `crate::service`'s `validation` module).
/// `StateMachineTypeNotSupported` carries no value at all — see
/// [`STATE_MACHINE_TYPE_NOT_SUPPORTED`] — and `ExecutionNotRedrivable` names
/// the execution AND a reason, which is its own construction, see
/// [`not_redrivable_message`].
pub fn resource_error_message(error_name: &str, arn: &str) -> String {
    format!("{error_name}: '{arn}'")
}

/// What the modelled `ExecutionNotRedrivable` says: the EXECUTION the request
/// named, and then why that execution is not eligible for a redrive.
///
/// The second construction this service builds from the request, for the one
/// refusal whose subject is a value of the request AND whose detail is a reason
/// of its own rather than that value: the recorded service answers
/// `Execution <executionArn> invalid for redrive as: <reason>`
/// (`states-execution-failure-and-redrive#task#15`), so the refusal says WHICH
/// execution it is about — what a caller redriving several of them needs to
/// attribute the failure to one.
///
/// `reason` is one of the `NOT_REDRIVABLE_*` values above, the same reason
/// [`ComAmazonawsSfnAwsstepfunctionsState::not_redrivable_reason`] gives
/// `DescribeExecution` for `redriveStatusReason` — which that member documents
/// on its own, with no execution named, and so carries WITHOUT this wrapper.
pub fn not_redrivable_message(execution_arn: &str, reason: &str) -> String {
    format!("Execution {execution_arn} invalid for redrive as: {reason}")
}

/// What the modelled `ConflictException` says when a publish pins a `revisionId`
/// that is not the one the state machine currently holds.
///
/// RECORDED text, and the third construction this service builds from the request:
/// the service answers this refusal with two sentences — the revision the caller
/// ASKED to publish, then the revision the state machine actually holds — because
/// `ConflictException` carries nothing but `message`, so the body is the only place
/// a caller using `revisionId` as an optimistic-concurrency guard can read which
/// revision to re-drive the publish from. The fixed text is the service's;
/// `requested` is the request's own `revisionId` member and `current` is the
/// revision `DescribeStateMachine` reports, which is [`INITIAL_REVISION_ID`] for a
/// state machine no update has revised yet — the name that API gives that state.
pub fn stale_revision_message(requested: &str, current: Option<&str>) -> String {
    let current = current.unwrap_or(INITIAL_REVISION_ID);
    format!(
        "Failed to publish the State Machine version for revision {requested}. The current \
         State Machine revision is {current}."
    )
}

/// `StateMachineDoesNotExist`'s own name, as its message spells it. RECORDED text.
pub const STATE_MACHINE_DOES_NOT_EXIST: &str = "State Machine Does Not Exist";

/// What the same error says when the ARN named a state machine VERSION that the store
/// holds nothing for. RECORDED text.
///
/// One rule, stated by [`resource_error_message`]: the condition names the resource
/// the request's ARN REFERRED to, and a version-qualified ARN refers to a version
/// rather than to the state machine behind it — so the condition is the version's,
/// even though the error shape is the state machine's (the model declares no
/// version-specific one).
pub const STATE_MACHINE_VERSION_DOES_NOT_EXIST: &str = "State Machine Version Does Not Exist";

/// The condition a resource-absent refusal names for the state machine ARN `parsed`:
/// the VERSION condition for an ARN qualified with a version number, and the state
/// machine's own otherwise.
///
/// The ONE place that choice is made, so every operation that resolves a qualified
/// ARN words the refusal the same way. An alias qualifier keeps the state machine's
/// condition: an alias is a route to a version rather than a resource the request
/// asked to read.
pub fn absent_state_machine_condition(parsed: &StateMachineArn) -> &'static str {
    match parsed.qualifier.as_deref() {
        Some(qualifier)
            if parsed.label().is_none() && qualifier_version(qualifier).is_some() =>
        {
            STATE_MACHINE_VERSION_DOES_NOT_EXIST
        }
        _ => STATE_MACHINE_DOES_NOT_EXIST,
    }
}

/// `StateMachineDeleting`'s, for [`resource_error_message`]. RECORDED text: the
/// service words this condition as a SENTENCE rather than as the words of the
/// shape name (`sfn-state-machine-deleting-rejections#task#6` records
/// `State Machine is being deleted: '<arn>'`).
pub const STATE_MACHINE_DELETING: &str = "State Machine is being deleted";

/// `ExecutionDoesNotExist`'s, for [`resource_error_message`]. The error's own name
/// under the rule above, not a recorded message.
pub const EXECUTION_DOES_NOT_EXIST: &str = "Execution Does Not Exist";

/// `ExecutionAlreadyExists`'s, for [`resource_error_message`]. RECORDED text.
pub const EXECUTION_ALREADY_EXISTS: &str = "Execution Already Exists";

/// `StateMachineAlreadyExists`'s, for [`resource_error_message`]. RECORDED text.
pub const STATE_MACHINE_ALREADY_EXISTS: &str = "State Machine Already Exists";

/// What the modelled `ConflictException` says when a `CreateStateMachineAlias`
/// names an alias that already exists under other parameters.
///
/// RECORDED text, and a FIXED sentence: the create is idempotent only for
/// "requests that contain the same values for these parameters", so a create
/// naming an alias that exists under another description or routing configuration
/// is this conflict — against an alias clients may already be running executions
/// through, which `UpdateStateMachineAlias` is the operation for. The sentence
/// carries no value of the request: the shape has nothing but `message`, and the
/// name the caller sent is the one it sent.
pub const ALIAS_ROUTING_CONFLICT: &str = "Failed to create alias because an alias with the same \
                                          name and a different routing configuration already \
                                          exists.";

/// What the modelled `ConflictException` says when a `DeleteStateMachineVersion`
/// names a version one or more aliases still route to.
///
/// RECORDED text: the service states the rule — "you can't delete a state machine
/// version currently referenced by one or more aliases" — and then lists, in
/// brackets, the aliases that reference it so the caller knows which ones to
/// delete or repoint. The listed values are the aliases' NAMES as the store holds
/// them, not their ARNs, and `alias_names` is read from the store rather than from
/// the request.
pub fn version_referenced_by_aliases_message(alias_names: &[String]) -> String {
    format!(
        "Version to be deleted must not be referenced by an alias. Current list of aliases \
         referencing this version: [{}]",
        alias_names.join(", ")
    )
}

/// What the modelled `MissingRequiredParameter` says when an `UpdateStateMachine`
/// names no member that would change the state machine.
///
/// RECORDED text, and a FIXED sentence: it states the ALTERNATIVES an update may
/// name — the definition, the role ARN, the logging configuration or the tracing
/// configuration — and carries no value of the request, since the request's defect
/// is everything it did not send.
pub const UPDATE_NAMES_NO_MEMBER: &str = "Either the definition, the role ARN, the \
                                          LoggingConfiguration, or the TracingConfiguration must \
                                          be specified";

/// `ActivityDoesNotExist`'s, for [`resource_error_message`]. The error's own name
/// under the rule above — the condition in spaced Title Case, then the ARN the
/// request named — not a recorded message.
pub const ACTIVITY_DOES_NOT_EXIST: &str = "Activity Does Not Exist";

/// `ActivityAlreadyExists`'s, for [`resource_error_message`], under the same rule.
pub const ACTIVITY_ALREADY_EXISTS: &str = "Activity Already Exists";

/// `InvalidName`'s, for [`resource_error_message`]. RECORDED text
/// (`states-creation-input-validation-errors#task#8` records
/// `Invalid Name: '<name>'`): the quoted value is the NAME the caller sent, not
/// an ARN, and not which character of it failed.
pub const INVALID_NAME: &str = "Invalid Name";

/// `InvalidDefinition`'s, for [`resource_error_message`]. RECORDED text: the
/// service names the thing that is invalid rather than spelling the shape name,
/// and the quoted value is the definition's diagnostics, one subject per error as
/// `<code>: <detail> at <location>` (see [`asl::validate`]).
pub const INVALID_STATE_MACHINE_DEFINITION: &str = "Invalid State Machine Definition";

/// `InvalidExecutionInput`'s, for [`resource_error_message`]. RECORDED text
/// (`states-creation-input-validation-errors#task#11` records
/// `Invalid State Machine Execution Input: '<detail>'`): the service names the
/// thing that is invalid — exactly as it does for a definition — and the quoted
/// value is the diagnostic for the input document it could not read, so a parse
/// failure is never reported as a bare parser string.
pub const INVALID_STATE_MACHINE_EXECUTION_INPUT: &str = "Invalid State Machine Execution Input";

/// What a refusal about a document the request carried quotes: where that document
/// stopped parsing, in the words this service states it in.
///
/// The sentence is framed in [`json_diagnostic`], around the tree's ONE reading of a
/// JSON parse fault ([`::service_common::json_parse_fault`]), and re-exported here
/// so every operation reaches it where it reaches the rest of this service's
/// messages — and so there is one name for it rather than two.
pub use json_diagnostic::diagnostic as json_parse_diagnostic;

/// What a refusal about a state machine DEFINITION quotes: the same reading, with the
/// definition the parse was reading quoted in each of its clauses and the position
/// the parse ended at stated after the sentence.
///
/// Framed in [`json_diagnostic`] beside the construction above, and re-exported here
/// for the same reason: one name for it, reached where every other message of this
/// service is reached.
pub use json_diagnostic::definition_diagnostic as json_parse_definition_diagnostic;

/// WHERE a refused definition stopped parsing, as the `location` of the diagnostic
/// that reports it — the clause [`json_parse_definition_diagnostic`] also states
/// after its sentence.
pub use json_diagnostic::definition_source_location as json_parse_definition_location;

/// What `StateMachineTypeNotSupported` says, whatever operation raises it and
/// whatever state machine it was raised for.
///
/// A FIXED service string, not the template above: the error shape's own
/// documentation is "State machine type is not supported" and the recorded
/// service answers one constant sentence that carries no value of the request,
/// so this message does not vary with the operation, the workflow type or the
/// ARN. Every operation that refuses a workflow type states it through this.
pub const STATE_MACHINE_TYPE_NOT_SUPPORTED: &str =
    "This operation is not supported by this type of state machine";

/// `InvalidToken`'s own name, as its message spells it, for
/// [`resource_error_message`]. RECORDED text.
pub const INVALID_TOKEN: &str = "Invalid Token";

/// The detail `InvalidToken`'s message quotes.
///
/// RECORDED text (`sfn.error-invalid-token-list-map-runs` records
/// `Invalid Token: 'Invalid token'`): the service does NOT echo the token it
/// refused, so the quoted value is the condition itself and no caller text reaches
/// the message. A pagination token is opaque and may be long, which is the same
/// reason the service has none to quote.
pub const INVALID_TOKEN_DETAIL: &str = "Invalid token";

/// What every operation of this API answers when it is given a pagination token it
/// did not issue: ONE construction, so no two paged lists word the same refusal
/// differently.
pub fn invalid_token_message() -> String {
    resource_error_message(INVALID_TOKEN, INVALID_TOKEN_DETAIL)
}

/// What the EXECUTION HISTORY answers for a pagination token it did not issue: the
/// condition ALONE, with nothing quoted after it.
///
/// RECORDED text (`sfn.get-execution-history-invalid-token` records `Invalid Token`):
/// the history cursor's refusal is the condition name and nothing more — no detail of
/// the service's own and, as everywhere here, no echo of the token — where the paged
/// lists of this API quote [`INVALID_TOKEN_DETAIL`] after it. Each operation words
/// this refusal as the service words it for THAT operation, which is why there are
/// two constructions and not one.
pub fn invalid_history_token_message() -> String {
    INVALID_TOKEN.to_string()
}

/// `TaskTimedOut`'s own condition name, as its message spells it, for
/// [`resource_error_message`]. RECORDED text.
pub const TASK_TIMED_OUT: &str = "Task Timed Out";

/// What every `SendTask*` operation answers for a token whose task is no longer
/// outstanding: ONE construction, so no two of them word that refusal differently.
///
/// RECORDED text: the recorded service answers a reply to a token whose task has been
/// closed with `Task Timed Out: '<the ARN of the activity that task belonged to>'`, so
/// the quoted value is that activity — which the token itself names (see
/// [`task_token`]) and not a value of the request.
pub fn task_timed_out_message(activity_arn: &str) -> String {
    resource_error_message(TASK_TIMED_OUT, activity_arn)
}

/// `InvalidOutput`'s, for [`resource_error_message`]. The error's own condition name
/// under that rule: the shape documents "the provided JSON output data is not valid",
/// and the value the message quotes is the document the caller sent, clipped by
/// [`asl::echoed`] so one request cannot be amplified into a much larger response.
pub const INVALID_OUTPUT: &str = "Invalid Output";

/// What the modelled `InvalidOutput` says: the condition, then the `output` the
/// request carried, and why it was refused.
///
/// Built from the REQUEST, like every other refusal here that is about a value the
/// caller sent (see [`resource_error_message`]): it names the document that is not
/// JSON, so a worker correcting its reply can see which one was rejected.
pub fn invalid_output_message(output: &str) -> String {
    format!(
        "{}: '{}' is not valid JSON.",
        INVALID_OUTPUT,
        asl::echoed(output)
    )
}

/// What separates a task token's id from the activity ARN it names.
///
/// A character [`FORBIDDEN_NAME_CHARS`] keeps out of an activity name, so the two
/// parts of a decoded token are always told apart however the activity was named.
const TASK_TOKEN_SEPARATOR: char = '|';

/// The `taskToken` that names the task `id` identifies under `activity_arn`.
///
/// "Task tokens are generated by Step Functions when tasks are assigned to a worker":
/// the token is the SERVICE's value, and it NAMES the task — the recorded service
/// answers a reply to a token whose task has been closed with the ARN of the activity
/// that task belonged to, which a store that no longer holds the task can only read
/// from the token itself. So a token carries both: a minted id, which is what makes
/// two tasks of one activity differ, and that ARN, encoded as base64 the way this
/// service's own tokens are, well inside the modelled `TaskToken` length (1..2048)
/// since an ARN is at most [`MAX_ARN_BYTES`].
pub fn task_token(id: &str, activity_arn: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{id}{TASK_TOKEN_SEPARATOR}{activity_arn}"))
}

/// The ACTIVITY the `taskToken` `token` names, or `None` for a value this service
/// never issued — the modelled `InvalidToken`.
///
/// Read from the token itself, BEFORE the store is consulted, exactly as every ARN
/// here is read before a lookup: a value that is not a token this service mints is a
/// defect in the request, while a token of this form the store holds no task for names
/// a task that has been closed.
pub fn read_task_token(token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (id, activity_arn) = decoded.split_once(TASK_TOKEN_SEPARATOR)?;
    (is_minted_id(id) && names_an_activity(activity_arn)).then(|| activity_arn.to_string())
}

/// Whether `id` is one of the ids this store mints: a version-4 UUID as
/// [`ComAmazonawsSfnAwsstepfunctionsState::fresh_execution_id`] renders it, 8-4-4-4-12
/// in lowercase hexadecimal.
fn is_minted_id(id: &str) -> bool {
    /// The characters of each group of the 8-4-4-4-12 form a UUID is rendered in.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = id.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        let hexadecimal = group
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if group.len() != width || !hexadecimal {
            return false;
        }
    }
    groups.next().is_none()
}

/// What a reply to an activity task is refused with when the definition the execution
/// now runs no longer reaches the state that task was scheduled at, so there is no
/// continuation to record for it.
pub const TASK_STATE_NOT_REACHED: &str =
    "the state machine definition this execution runs no longer reaches the state its task was \
     scheduled at, so the execution cannot be continued from it";

/// The members of this service's model whose target shape is a FRACTIONAL number
/// (Smithy `float`/`double`), named by their wire (lowerCamel) member name.
///
/// The standard validation message echoes a rejected value as the member's
/// MODELLED TYPE renders it, so a member listed here is reported with a
/// fractional part (`150.0`) where an `integer`/`long` member is not (`150`).
/// Source: the model itself — `smithy_query(query="operation",
/// operation="UpdateMapRun")` reports `toleratedFailurePercentage` as
/// `{"target": "ToleratedFailurePercentage", "range": {"min": 0, "max": 100},
/// "type": "float"}`. `tests/all/error_messages.rs` reads the model and fails if
/// it declares a fractional member this list does not carry, so the list cannot
/// silently fall behind the model.
///
/// The key is the member's WIRE NAME, because that is all a constraint violation
/// carries — it reports a dotted member path, not the shape the member belongs
/// to. Every structure of this model that spells `toleratedFailurePercentage`
/// (`UpdateMapRunInput`, `DescribeMapRunOutput`, `InspectionData`) targets the
/// same `float` shape, so one name covers them all; a model that ever declared
/// one member name as `float` in one structure and `integer` in another would
/// need the shape/member pair instead, and the test above would not catch it.
pub const FRACTIONAL_MEMBERS: &[&str] = &["toleratedFailurePercentage"];

/// What one redriven attempt leaves for the store to record.
///
/// The members travel together — [`plan_redrive`] produces all of them for the same
/// attempt and none of them means anything without the rest — so they are one
/// argument.
#[derive(Debug)]
pub struct RedrivenAttempt {
    /// The `ExecutionRedriven` event the attempt opens with.
    pub opening: Vec<HistoryEvent>,
    /// The outcome the re-run computed, owed to the clock.
    pub pending: Option<PendingOutcome>,
    /// What the activity task the re-run stopped at and the Map Runs it re-entered
    /// will retain, so the redrive is admitted against the whole of what it adds.
    pub extra_bytes: usize,
    /// The unsuccessful step THIS attempt stopped at, which the next redrive
    /// continues from.
    pub stopped_at: Option<asl::RedriveStep>,
}

/// Why a redrive was not performed; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::redrive_execution`]. A redrive the
/// execution itself is not eligible for is reported by
/// [`ComAmazonawsSfnAwsstepfunctionsState::not_redrivable_reason`] instead, so this
/// carries only what the store itself refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedriveRejection {
    /// The store holds no execution under that ARN — the modelled
    /// `ExecutionDoesNotExist`.
    NotFound,
    /// The re-run would take the store past [`MAX_STORE_BYTES`] — the modelled
    /// `ExecutionLimitExceeded`.
    NoRoom,
}

/// Why a worker's reply to an activity task was not recorded; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::answer_activity_task`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskReplyRejection {
    /// The store holds no outstanding task under that token, or the execution
    /// waiting at it has reached a terminal status — the modelled `TaskTimedOut`:
    /// "the task token has either expired or the task associated with the token has
    /// already been closed".
    Gone,
    /// Continuing the execution would take the store past [`MAX_STORE_BYTES`].
    NoRoom,
}

/// Why an `UpdateStateMachineAlias` was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::update_state_machine_alias`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasUpdateRejection {
    /// The store holds no alias under that ARN — the modelled `ResourceNotFound`.
    NotFound,
    /// The updated record would take the store past [`MAX_STORE_BYTES`].
    NoRoom,
}

/// Why a tag write was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::merge_tags`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagWriteRejection {
    /// The store holds no resource under that ARN — the modelled `ResourceNotFound`.
    NotFound,
    /// The merged tag set would hold this many tags, past [`MAX_TAGS`] — the
    /// modelled `TooManyTags`.
    TooManyTags(usize),
    /// The merged record would take the store past [`MAX_STORE_BYTES`].
    NoRoom,
}

/// Why a routing configuration cannot be pointed at; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::read_routing_configuration`]. Each variant
/// is one of the modelled errors the alias operations declare, carrying what that
/// error reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingRejection {
    /// An entry whose `stateMachineVersionArn` is not an ARN of this service in the
    /// endpoint's region at all — the modelled `InvalidArn`, carrying its message.
    InvalidArn(String),
    /// A configuration whose entries are well-formed ARNs but cannot be pointed at:
    /// an ARN that names no version, versions of two different state machines, or
    /// weights that do not add up — the modelled `ValidationException` with the
    /// `INVALID_ROUTING_CONFIGURATION` reason, carrying its message.
    InvalidRouting(String),
    /// A version the store does not hold — the modelled `ResourceNotFound`, naming
    /// that version's ARN.
    VersionNotFound(String),
    /// The state machine those versions belong to is being deleted — the modelled
    /// `StateMachineDeleting`, naming its ARN.
    Deleting(String),
}

/// The parts of a state machine ARN this service reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMachineArn {
    /// The state machine name field, which on a map-run LABELED ARN carries the
    /// `<name>/<label>` form — see [`StateMachineArn::label`].
    pub name: String,
    /// The `:<version>` or `:<alias>` suffix a qualified ARN carries, if any.
    pub qualifier: Option<String>,
}

impl StateMachineArn {
    /// The map-run label a LABELED state machine ARN carries, or `None` for an ARN
    /// that names a state machine itself.
    ///
    /// From `StartExecution`'s and `DeleteStateMachine`'s own documentation: "The
    /// following qualified state machine ARN refers to a Distributed Map state with
    /// a label `mapStateLabel` in a state machine named `myStateMachine`:
    /// `arn:partition:states:region:account-id:stateMachine:myStateMachine/mapStateLabel`".
    /// The label follows the name after a `/`, which the `name` rule
    /// ([`is_valid_name`]) forbids inside a name, so the two forms never
    /// collide.
    pub fn label(&self) -> Option<&str> {
        self.name.split_once('/').map(|(_, label)| label)
    }
}

/// The ARN of the state machine `name` in `region`.
pub fn state_machine_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{STATE_MACHINE_RESOURCE}:{name}"),
    )
}

/// The ARN of the activity `name` in `region`.
///
/// An activity ARN is the same total function of the region and the name a state
/// machine ARN is ([`state_machine_arn`]), with the activity resource type: that is
/// what makes the ARN-keyed store the name index `CreateActivity`'s idempotency
/// check reads.
pub fn activity_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{ACTIVITY_RESOURCE}:{name}"))
}

/// The prefix every state machine ARN of `region` begins with: the ARN of the empty
/// name, which no state machine can take (`name` is modelled 1..80).
pub fn state_machine_arn_prefix(region: &str) -> String {
    state_machine_arn(region, "")
}

/// The prefix every activity ARN of `region` begins with: the ARN of the empty
/// name, which no activity can take (`name` is modelled 1..80).
pub fn activity_arn_prefix(region: &str) -> String {
    activity_arn(region, "")
}

/// The ARN of version `version` of the state machine `state_machine_arn` names.
///
/// A version is reached through a QUALIFIED state machine ARN: the state machine's
/// own ARN with the version number appended
/// (`arn:<partition>:states:<region>:<account>:stateMachine:<name>:<version>`),
/// which is the form `CreateStateMachine` reports in `stateMachineVersionArn` and
/// [`parse_state_machine_arn`] reads back as a `qualifier`.
pub fn state_machine_version_arn(state_machine_arn: &str, version: u32) -> String {
    format!("{state_machine_arn}:{version}")
}

/// The ARN of the alias `alias_name` of the state machine `state_machine_arn` names.
///
/// From `StateMachineAliasListItem`'s own documentation: "The alias ARN is a
/// combination of state machine ARN and the alias name separated by a colon (:).
/// For example, `stateMachineARN:PROD`." So an alias is reached through a QUALIFIED
/// state machine ARN, exactly as a version is — the qualifier is the alias name
/// rather than the version number.
pub fn state_machine_alias_arn(state_machine_arn: &str, alias_name: &str) -> String {
    format!("{state_machine_arn}:{alias_name}")
}

/// The `(stateMachineArn, aliasName)` `arn` names, read as a state machine ALIAS ARN
/// of this service in `endpoint_region`, or the message the modelled `InvalidArn`
/// carries.
///
/// An alias ARN is a state machine ARN qualified with the alias name, so text that
/// carries no qualifier, a map-run label, or a qualifier outside the alias name rule
/// ([`is_valid_alias_name`]) does not name an alias at all — a defect in the ARN,
/// which is what `InvalidArn` reports, rather than an alias that is absent. A
/// qualifier of nothing but digits is a VERSION ([`qualifier_version`]) and the alias
/// name rule refuses it for exactly that reason, so a version ARN never resolves to
/// an alias.
pub fn read_state_machine_alias_arn(
    arn: &str,
    endpoint_region: &str,
) -> Result<(String, String), String> {
    let parsed = read_state_machine_arn(arn, endpoint_region)?;
    let not_an_alias =
        || invalid_arn_message(format!("Invalid ARN {STATE_MACHINE_RESOURCE} alias: {arn}"));
    let Some(qualifier) = parsed.qualifier.as_deref() else {
        return Err(not_an_alias());
    };
    if parsed.label().is_some() || !is_valid_alias_name(qualifier) {
        return Err(not_an_alias());
    }
    // The state machine is the ARN without the qualifier, which is its last
    // colon-delimited field — and the name rule above admits no colon into it.
    let Some((state_machine_arn, _)) = arn.rsplit_once(':') else {
        return Err(not_an_alias());
    };
    Ok((state_machine_arn.to_string(), qualifier.to_string()))
}

/// The `(stateMachineArn, version)` `arn` names, read as a state machine VERSION ARN
/// of this service in `endpoint_region`, or the message the modelled `InvalidArn`
/// carries.
///
/// A version ARN is a state machine ARN qualified with the version number, so text
/// that carries no qualifier, a map-run label, or a qualifier that is not a version
/// number ([`qualifier_version`]) does not name a version at all — a defect in the
/// ARN, which is what `InvalidArn` reports, rather than a version that is absent.
/// This is [`read_state_machine_alias_arn`] for the other qualified form, and the two
/// are exclusive: a qualifier of nothing but digits is a version and never an alias.
///
/// An ARN carrying NO qualifier is reported through
/// [`missing_version_qualifier_message`], which states that one condition rather than
/// the generic one: see that function for why.
pub fn read_state_machine_version_arn(
    arn: &str,
    endpoint_region: &str,
) -> Result<(String, u32), String> {
    let parsed = read_state_machine_arn(arn, endpoint_region)?;
    let not_a_version =
        || invalid_arn_message(format!("Invalid ARN {STATE_MACHINE_RESOURCE} version: {arn}"));
    let Some(qualifier) = parsed.qualifier.as_deref() else {
        return Err(missing_version_qualifier_message(arn));
    };
    if parsed.label().is_some() {
        return Err(not_a_version());
    }
    let Some(version) = qualifier_version(qualifier) else {
        return Err(not_a_version());
    };
    // The state machine is the ARN without the qualifier, which is its last
    // colon-delimited field — and a version number carries no colon.
    let Some((state_machine_arn, _)) = arn.rsplit_once(':') else {
        return Err(not_a_version());
    };
    Ok((state_machine_arn.to_string(), version))
}

/// The version number a state machine ARN's qualifier names, or `None` when the
/// qualifier is an alias name rather than a version.
///
/// A qualifier is "either a version or an alias": a version qualifier is the
/// decimal version number, so a qualifier that is not all digits names an alias.
pub fn qualifier_version(qualifier: &str) -> Option<u32> {
    if qualifier.is_empty() || !qualifier.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    qualifier.parse().ok()
}

/// Whether the revision `requested` names is the one a state machine currently
/// holds in `current`.
///
/// A caller pins the revision it wants published: "only publish the state machine
/// version if the current state machine's revision ID matches the specified ID", and
/// a state machine that has never been updated carries no revision ID, which the
/// caller names with [`INITIAL_REVISION_ID`].
pub fn revision_matches(current: Option<&str>, requested: &str) -> bool {
    match current {
        Some(current) => current == requested,
        None => requested == INITIAL_REVISION_ID,
    }
}

/// The ARN of one execution of a state machine.
pub fn execution_arn(region: &str, state_machine_name: &str, execution_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{EXECUTION_RESOURCE}:{state_machine_name}:{execution_name}"),
    )
}

/// The ARN of one Map Run a Distributed Map state of `state_machine_name` started.
///
/// A Map Run ARN is a LongArn naming the `mapRun` resource, whose fields are the
/// state machine's name and the Map state's label joined by `/`, and a unique id
/// (`mapRun:<stateMachineName>/<label>:<id>`) — the form
/// [`read_map_run_arn`] reads back. The label is what tells two Map Runs of one
/// execution apart and the id is what tells two executions' apart.
pub fn map_run_arn(region: &str, state_machine_name: &str, label: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{MAP_RUN_RESOURCE}:{state_machine_name}/{label}:{id}"),
    )
}

/// The ARN of one SYNCHRONOUS Express execution of a state machine.
///
/// See [`EXPRESS_EXECUTION_RESOURCE`] for the form and for what the trailing
/// `execution_id` field is for.
pub fn express_execution_arn(
    region: &str,
    state_machine_name: &str,
    execution_name: &str,
    execution_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{EXPRESS_EXECUTION_RESOURCE}:{state_machine_name}:{execution_name}:{execution_id}"
        ),
    )
}

/// The region field of `arn`, or the emulator's default region when `arn` carries
/// none.
///
/// A resource ARN is minted in the region of the resource it belongs to, not in
/// whatever region the current request was signed for, so an execution ARN is built
/// from its state machine's ARN through this.
pub fn arn_region(arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .filter(|region| !region.is_empty())
        .unwrap_or(DEFAULT_REGION)
        .to_string()
}

/// The message the modelled `InvalidArn` carries for `diagnosis`, the ARN component
/// that failed validation.
///
/// Step Functions wraps every ARN diagnosis in this envelope — `Invalid Arn:
/// '<diagnosis>'` — and the diagnosis is built from the request: `Invalid ARN
/// prefix: <value>` for text that does not begin with `arn:`, `Expected the ARN
/// <arn> to be within region (<region>).` for an ARN of another region. Every
/// refusal in this module goes through here, so one envelope serves every operation.
pub fn invalid_arn_message(diagnosis: String) -> String {
    format!("Invalid Arn: '{diagnosis}'")
}

/// What the modelled `InvalidArn` says about an ARN an operation needs a VERSION
/// qualifier on and which carries none.
///
/// RECORDED text: where the ARN is well formed but names the state machine itself
/// rather than one of its versions, the service does not state the generic ARN
/// diagnosis ([`invalid_arn_message`]) — it names the one thing the ARN is missing,
/// in a sentence of its own, followed by the ARN the request sent. The condition is
/// about the QUALIFIER rather than about the ARN's shape, which is why it is worded
/// here and not through the envelope every other ARN diagnosis is wrapped in.
pub fn missing_version_qualifier_message(arn: &str) -> String {
    format!("Arn missing version qualifier: {arn}")
}

/// What the modelled `InvalidArn` says about an ARN whose RESOURCE TYPE is not one
/// the operation reading it accepts.
///
/// RECORDED text (`sfn.express-sync-failure-returns-a-result-not-an-error-with-billing-details`
/// records `Invalid Arn: 'Resource type not valid in this context: express'` for a
/// synchronous Express execution ARN handed to `GetExecutionHistory`): the service
/// states that the type the ARN names is not valid HERE — it does not name the type
/// the operation wanted — and the one value in the sentence is that type, read off
/// the ARN the request carried. ONE construction, so every reader of an ARN in this
/// service words that refusal the same way, whichever resource type it accepts.
pub fn resource_type_not_valid_message(kind: &str) -> String {
    invalid_arn_message(format!("Resource type not valid in this context: {kind}"))
}

/// The resource portion of `arn`, which must be the ARN of a resource of THIS
/// service in `endpoint_region`, or the message the modelled `InvalidArn` carries.
///
/// Step Functions validates an ARN BEFORE it looks the resource up, so a
/// syntactically wrong ARN, an ARN of another service and an ARN of another region
/// are each the client ARN error rather than a resource that could not be found. An
/// ARN is six colon-delimited fields, the last of which carries the colons of the
/// resource itself; the checks run in the order the fields are written, so the
/// diagnosis names the FIRST component that failed.
///
/// The account field is deliberately not checked: no operation here is scoped by
/// account, and the emulator serves one account.
fn states_arn_resource<'a>(arn: &'a str, endpoint_region: &str) -> Result<&'a str, String> {
    let Some(fields) = arn.strip_prefix("arn:") else {
        return Err(invalid_arn_message(format!("Invalid ARN prefix: {arn}")));
    };
    // A field the ARN stops short of reads as empty, so the checks below judge the
    // fields in the order they are written and the resource's own presence last.
    let mut fields = fields.splitn(5, ':');
    let partition = fields.next().unwrap_or_default();
    let service = fields.next().unwrap_or_default();
    let region = fields.next().unwrap_or_default();
    let _account = fields.next().unwrap_or_default();
    let resource = fields.next();
    if partition.is_empty() {
        return Err(invalid_arn_message(format!("Invalid ARN partition: {arn}")));
    }
    if service != ARN_SERVICE {
        return Err(invalid_arn_message(format!(
            "Invalid ARN service: {service}, expected {ARN_SERVICE}"
        )));
    }
    // A regional endpoint serves only its own region's resources: an ARN naming
    // another region is a defect in the REQUEST, not a resource that is absent here.
    if region != endpoint_region {
        return Err(invalid_arn_message(format!(
            "Expected the ARN {arn} to be within region ({endpoint_region})."
        )));
    }
    resource.ok_or_else(|| {
        invalid_arn_message(format!(
            "Invalid ARN: expected 6 colon-delimited fields in {arn}"
        ))
    })
}

/// The state machine `name[/label][:qualifier]` the remaining resource fields name,
/// shared by the readers that accept a state machine ARN.
fn state_machine_resource<'a>(
    arn: &str,
    parts: &mut impl Iterator<Item = &'a str>,
) -> Result<StateMachineArn, String> {
    let name = parts.next().unwrap_or_default();
    if name.is_empty() {
        return Err(invalid_arn_message(format!(
            "Invalid ARN {STATE_MACHINE_RESOURCE} name: {arn}"
        )));
    }
    Ok(StateMachineArn {
        name: name.to_string(),
        qualifier: parts.next().map(str::to_string),
    })
}

/// The state machine `arn` names, read as an ARN of this service in
/// `endpoint_region`, or the message the modelled `InvalidArn` carries.
pub fn read_state_machine_arn(arn: &str, endpoint_region: &str) -> Result<StateMachineArn, String> {
    let resource = states_arn_resource(arn, endpoint_region)?;
    let mut parts = resource.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    if kind != STATE_MACHINE_RESOURCE {
        return Err(resource_type_not_valid_message(kind));
    }
    state_machine_resource(arn, &mut parts)
}

/// The `(stateMachineName, executionName)` `arn` names, read as an execution ARN of
/// this service in `endpoint_region`, or the message the modelled `InvalidArn`
/// carries.
pub fn read_execution_arn(arn: &str, endpoint_region: &str) -> Result<(String, String), String> {
    let resource = states_arn_resource(arn, endpoint_region)?;
    let mut parts = resource.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    if kind != EXECUTION_RESOURCE {
        return Err(resource_type_not_valid_message(kind));
    }
    let state_machine = parts.next().unwrap_or_default();
    let execution = parts.next().unwrap_or_default();
    if state_machine.is_empty() || execution.is_empty() {
        return Err(invalid_arn_message(format!(
            "Invalid ARN {EXECUTION_RESOURCE} resource: {resource}"
        )));
    }
    Ok((state_machine.to_string(), execution.to_string()))
}

/// Reads `arn` as a Map Run ARN of this service in `endpoint_region`, answering the
/// message the modelled `InvalidArn` carries when it is not one.
///
/// A Map Run ARN is `mapRun:<stateMachineName>/<label>:<id>` ([`map_run_arn`]), so
/// only its resource type and a non-empty remainder are judged here: the ARN names
/// one record in the store, which the caller looks up by the whole ARN.
pub fn read_map_run_arn(arn: &str, endpoint_region: &str) -> Result<(), String> {
    let resource = states_arn_resource(arn, endpoint_region)?;
    let mut parts = resource.splitn(2, ':');
    let kind = parts.next().unwrap_or_default();
    if kind != MAP_RUN_RESOURCE {
        return Err(resource_type_not_valid_message(kind));
    }
    if parts.next().is_none_or(str::is_empty) {
        return Err(invalid_arn_message(format!(
            "Invalid ARN {MAP_RUN_RESOURCE} resource: {resource}"
        )));
    }
    Ok(())
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not an AWS region token is refused here and the default used
/// instead: a generated ARN can never carry text a client cannot parse back.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The HTTP header an X-Ray trace header is passed in.
///
/// From `traceHeader`'s own documentation: "For X-Ray traces, all Amazon Web Services
/// services use the X-Amzn-Trace-Id header from the HTTP request."
pub const TRACE_HEADER_HEADER: &str = "x-amzn-trace-id";

/// The most characters a reported `traceHeader` may carry: the modelled `TraceHeader`
/// shape's own bound (len 0..256, pattern `^\p{ASCII}*$`).
pub const MAX_TRACE_HEADER_CHARS: usize = 256;

/// The trace header an execution carries, given the one its request body supplied.
///
/// From `traceHeader`'s own documentation, on `StartExecution` and
/// `StartSyncExecution` alike: "Using the header is the preferred mechanism to
/// identify a trace. StartExecution and StartSyncExecution API operations can also
/// use traceHeader from the body of the request payload. If both sources are
/// provided, Step Functions will use the header value (preferred) over the value in
/// the request body."
///
/// A request header is caller text of any length and any bytes, while the modelled
/// `TraceHeader` the member is reported through is 0..256 ASCII characters — so a
/// header outside what that shape models is not reported through it and the body
/// value stands.
pub fn effective_trace_header(
    parts: &::http::request::Parts,
    from_body: Option<String>,
) -> Option<String> {
    parts
        .headers
        .get(TRACE_HEADER_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.is_ascii() && value.chars().count() <= MAX_TRACE_HEADER_CHARS)
        .map(str::to_string)
        .or(from_body)
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Characters a state machine or execution name may not contain.
///
/// From the `name` member's own documentation on `CreateStateMachine` and
/// `StartExecution`: "A name must not contain: white space; brackets `< > { } [ ]`;
/// wildcard characters `? *`; special characters `" # % \ ^ | ~ ` $ & , ; : /`;
/// control characters (U+0000-001F, U+007F-009F, U+FFFE-FFFF)".
const FORBIDDEN_NAME_CHARS: &[char] = &[
    '<', '>', '{', '}', '[', ']', '?', '*', '"', '#', '%', '\\', '^', '|', '~', '`', '$', '&', ',',
    ';', ':', '/',
];

/// Whether `name` is an acceptable state machine or execution name.
///
/// The length bounds (1..80) are modelled, so the centralized walk has already
/// refused anything outside them; only the character rule lives here. A refusal
/// is reported through the modelled `InvalidName`, whose message states the
/// condition and the rejected name rather than which character failed — see
/// [`resource_error_message`] with [`INVALID_NAME`].
pub fn is_valid_name(name: &str) -> bool {
    !name.chars().any(|character| {
        character.is_whitespace()
            || FORBIDDEN_NAME_CHARS.contains(&character)
            || matches!(u32::from(character), 0x0000..=0x001F | 0x007F..=0x009F | 0xFFFE | 0xFFFF)
    })
}

/// Whether `name` is an acceptable state machine ALIAS name.
///
/// The whole modelled `CharacterRestrictedName` rule, and the one implementation of
/// it: the length bound (1..80) and the pattern
/// `^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$` — both from
/// `smithy_query(query="operation",operation="CreateStateMachineAlias")` — so ASCII
/// letters, digits, `_`, `-` and `.`, with at least one character that is not a
/// digit. That last clause is what keeps an alias name from reading as a version
/// number, which the `name` member states as "to avoid conflict with version ARNs,
/// don't use an integer in the name of the alias".
///
/// The length is judged here as well as by the centralized walk, because an alias
/// name also arrives as the QUALIFIER of an ARN
/// ([`read_state_machine_alias_arn`]), where no modelled constraint reaches it.
///
/// A refusal is reported as a MODELLED-CONSTRAINT violation, because that is what it
/// is: the rule is the member's own `@pattern`, which the boundary could not enforce
/// (its lookahead is outside the regex dialect the centralized walk compiles), so the
/// refusal states the standard validation envelope the boundary would have stated —
/// see [`pattern_violation_message`] with [`ALIAS_NAME_PATTERN`] — rather than a
/// resource error of this service's own.
pub fn is_valid_alias_name(name: &str) -> bool {
    (MIN_ALIAS_NAME_CHARS..=MAX_ALIAS_NAME_CHARS).contains(&name.chars().count())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && name
            .bytes()
            .any(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'-' | b'.'))
}

/// The modelled `@pattern` of `CharacterRestrictedName`, the shape a state machine
/// alias `name` targets, spelled exactly as the model declares it.
///
/// Source: the model itself — `smithy_query(query="operation",
/// operation="CreateStateMachineAlias")` reports `name` as `{"target":
/// "CharacterRestrictedName", "length": {"min": 1, "max": 80}, "pattern":
/// "^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$"}`. The text is what the refusal QUOTES as
/// the constraint the value failed, so it has to be the model's own spelling; the
/// check itself is [`is_valid_alias_name`], which reads the same rule without a regex
/// engine. A test reads the pattern off the model and fails if the two drift apart.
pub const ALIAS_NAME_PATTERN: &str = r"^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$";

/// The standard validation envelope for `value`, sent at the input member `member`,
/// failing that member's modelled `@pattern`.
///
/// A modelled constraint the centralized walk cannot enforce is still a modelled
/// constraint, so the refusal is worded by the SAME body every other constraint
/// violation of this service is worded by (`crate::service`'s `validation` module,
/// which reaches the tree's shared wording): one clause naming the rejected value,
/// the member it sat at and the pattern it had to satisfy, inside the
/// `N validation error(s) detected` envelope. Nothing about the sentence is written
/// here, so a handler-side check cannot drift from the boundary's wording.
pub fn pattern_violation_message(member: &str, pattern: &str, value: &str) -> String {
    crate::service::validation::one_violation_message(::smithy_server_runtime::Violation {
        path: member.to_string(),
        kind: ::smithy_server_runtime::ViolationKind::Pattern(pattern.to_string()),
        value: Some(value.to_string()),
    })
}

/// Why `role_arn` is not the ARN of an IAM role, or `None` when it is — the
/// modelled `InvalidArn` ("The provided Amazon Resource Name (ARN) is not valid"),
/// whose message is the "Invalid Arn: '<diagnosis>'" envelope the service builds
/// from the request.
///
/// `roleArn` is the only ARN `CreateStateMachine` takes, and the operation models
/// `InvalidArn`, so the ARN is checked before anything is stored: it is "the Amazon
/// Resource Name (ARN) of the IAM role to use for this state machine", so it has to
/// be an ARN, and an IAM `role/` ARN at that. Its length (1..256) is modelled.
pub fn invalid_role_arn_reason(role_arn: &str) -> Option<String> {
    if !role_arn.starts_with("arn:") {
        return Some(format!("Invalid Arn: 'Invalid ARN prefix: {role_arn}'"));
    }
    let fields: Vec<&str> = role_arn.splitn(6, ':').collect();
    let malformed = fields.len() < 6
        || fields[1].is_empty()
        || fields[2].is_empty()
        || fields[5].is_empty();
    if malformed {
        return Some(format!("Invalid Arn: 'Invalid ARN: {role_arn}'"));
    }
    // IAM is a global service, so an IAM ARN carries no region, and a role is the
    // `role/<path><name>` resource.
    let is_role = fields[2] == "iam"
        && fields[5]
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty());
    if !is_role {
        return Some(format!(
            "Invalid Arn: 'Expected the ARN {role_arn} to be an IAM role ARN.'"
        ));
    }
    None
}
///
/// From the operation's own documentation: "CreateStateMachine's idempotency check
/// is based on the state machine name, definition, type, LoggingConfiguration,
/// TracingConfiguration, and EncryptionConfiguration[.] The check is also based on
/// the publish and versionDescription parameters. If a following request has a
/// different roleArn or tags, Step Functions will ignore these differences" — so
/// `roleArn` and `tags` are deliberately absent here, and the name is the store key
/// rather than part of the fingerprint.
///
/// The three configuration structures are compared as the EFFECTIVE configuration
/// each one denotes, not as the literal members the request carried: a request that
/// omits a configuration asks for the same thing as a request that spells its
/// defaults out, and the service stores one state machine either way. So a repeat
/// that sends `{"level": "OFF", "includeExecutionData": false}` where the first
/// request sent no `loggingConfiguration` is the SAME request, and a repeat built by
/// reading a `DescribeStateMachine` response back is idempotent.
pub fn configuration_fingerprint(
    definition: &str,
    sm_type: &str,
    logging_configuration: Option<&LoggingConfiguration>,
    tracing_configuration: Option<&TracingConfiguration>,
    encryption_configuration: Option<&EncryptionConfiguration>,
    publish: bool,
    version_description: Option<&str>,
) -> String {
    let logging = effective_logging_configuration(logging_configuration);
    let tracing = effective_tracing_configuration(tracing_configuration);
    let encryption = effective_encryption_configuration(encryption_configuration);
    format!(
        "{definition}\u{1f}{sm_type}\u{1f}{logging}\u{1f}\
         {tracing}\u{1f}{encryption}\u{1f}\
         {publish}\u{1f}{version_description:?}"
    )
}

/// The logging configuration a request carrying `configuration` effectively asks
/// for, rendered deterministically.
///
/// The defaults live in ONE place: this renders
/// [`reported_logging_configuration`], the same effective structure a read
/// reports, so the fingerprint and the response can never disagree about what a
/// request configured.
pub fn effective_logging_configuration(configuration: Option<&LoggingConfiguration>) -> String {
    let reported = reported_logging_configuration(configuration);
    let level = reported.level.as_deref().unwrap_or(LOG_LEVEL_OFF);
    let include_execution_data = reported.include_execution_data;
    let destinations: Vec<&str> = reported
        .destinations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|destination| destination.cloud_watch_logs_log_group.as_ref())
        .filter_map(|group| group.log_group_arn.as_deref())
        .collect();
    format!("level={level},includeExecutionData={include_execution_data},destinations={destinations:?}")
}

/// The tracing configuration a request carrying `configuration` effectively asks
/// for, rendered from [`reported_tracing_configuration`] for the same reason.
pub fn effective_tracing_configuration(configuration: Option<&TracingConfiguration>) -> String {
    let enabled = reported_tracing_configuration(configuration).enabled;
    format!("enabled={enabled}")
}

/// The encryption configuration a request carrying `configuration` effectively asks
/// for, rendered from [`reported_encryption_configuration`] for the same reason.
///
/// Under [`ENCRYPTION_TYPE_AWS_OWNED_KEY`] the KMS members mean nothing
/// (`kmsDataKeyReusePeriodSeconds`: "Only applies to customer managed keys") and
/// [`invalid_encryption_reason`] refuses them, so they are not part of what it
/// denotes.
pub fn effective_encryption_configuration(configuration: Option<&EncryptionConfiguration>) -> String {
    let reported = reported_encryption_configuration(configuration);
    if reported.r#type == ENCRYPTION_TYPE_AWS_OWNED_KEY {
        return format!("type={ENCRYPTION_TYPE_AWS_OWNED_KEY}");
    }
    format!(
        "type={},kmsKeyId={:?},kmsDataKeyReusePeriodSeconds={:?}",
        reported.r#type,
        reported.kms_key_id.as_deref(),
        reported.kms_data_key_reuse_period_seconds
    )
}

// ---------------------------------------------------------------------------
// Encryption configuration
// ---------------------------------------------------------------------------

/// Why `configuration` is not an encryption configuration this service accepts, or
/// `None` when it is — the modelled `InvalidEncryptionConfiguration`, which is
/// "received when encryptionConfiguration is specified but various conditions exist
/// which make the configuration invalid. For example, if type is set to
/// CUSTOMER_MANAGED_KMS_KEY, but kmsKeyId is null, or kmsDataKeyReusePeriodSeconds
/// is not between 60 and 900".
///
/// The conditions, from the shape's own members:
///
/// * `type` is required and is one of [`ENCRYPTION_TYPE_VALUES`];
/// * under `CUSTOMER_MANAGED_KMS_KEY`, `kmsKeyId` must be there and must name a KMS
///   key: "an alias, alias ARN, key ID, or key ARN of a symmetric encryption KMS
///   key";
/// * `kmsDataKeyReusePeriodSeconds`, when given, is within the modelled 60..900;
/// * under `AWS_OWNED_KEY` neither KMS member applies, so a request that sets one
///   is asking for something the configuration cannot do.
///
/// Every operation that accepts an `encryptionConfiguration` reads this, so one
/// rule covers them all.
pub fn invalid_encryption_reason(configuration: &EncryptionConfiguration) -> Option<String> {
    if !ENCRYPTION_TYPE_VALUES.contains(&configuration.r#type.as_str()) {
        return Some(format!(
            "Invalid Encryption Configuration: type must be one of [{}].",
            ENCRYPTION_TYPE_VALUES.join(", ")
        ));
    }
    if let Some(period) = configuration.kms_data_key_reuse_period_seconds {
        if !(MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS..=MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS)
            .contains(&period)
        {
            return Some(format!(
                "Invalid Encryption Configuration: kmsDataKeyReusePeriodSeconds must be between \
                 {MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS} and \
                 {MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS}."
            ));
        }
    }
    if configuration.r#type == ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY {
        if !configuration
            .kms_key_id
            .as_deref()
            .is_some_and(names_kms_key)
        {
            return Some("Invalid Encryption Configuration: Must set a valid kmsKeyId.".to_string());
        }
        return None;
    }
    if configuration.kms_key_id.is_some() {
        return Some(format!(
            "Invalid Encryption Configuration: Custom kmsKeyId is not allowed when selecting \
             {ENCRYPTION_TYPE_AWS_OWNED_KEY}."
        ));
    }
    if configuration.kms_data_key_reuse_period_seconds.is_some() {
        return Some(format!(
            "Invalid Encryption Configuration: kmsDataKeyReusePeriodSeconds is not allowed when \
             selecting {ENCRYPTION_TYPE_AWS_OWNED_KEY}."
        ));
    }
    None
}

/// Whether `value` names a KMS key the way `kmsKeyId` documents: "an alias, alias
/// ARN, key ID, or key ARN".
fn names_kms_key(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if let Some(resource) = kms_arn_resource(value) {
        return match resource.split_once('/') {
            Some(("key", key_id)) => is_kms_key_id(key_id),
            Some(("alias", alias)) => !alias.is_empty(),
            _ => false,
        };
    }
    if let Some(alias) = value.strip_prefix("alias/") {
        return !alias.is_empty();
    }
    is_kms_key_id(value)
}

/// The resource portion of a KMS ARN, or `None` when `value` is not one.
fn kms_arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let scheme = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    (scheme == "arn" && !partition.is_empty() && service == "kms").then_some(resource)
}

/// Whether `value` is a KMS key id: a UUID, or the `mrk-` form a multi-Region key
/// carries.
fn is_kms_key_id(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix("mrk-") {
        return hex.len() == 32 && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    groups
        .iter()
        .zip([8, 4, 4, 4, 12])
        .all(|(group, width)| {
            group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

// ---------------------------------------------------------------------------
// Logging configuration
// ---------------------------------------------------------------------------

/// The most log destinations one logging configuration may carry, from the
/// `destinations` member's own documentation: "Limited to size 1."
pub const MAX_LOG_DESTINATIONS: usize = 1;

/// What the modelled `InvalidLoggingConfiguration` says about a `logGroupArn` that is
/// not the ARN of a CloudWatch Logs log group as the member documents one ("The ARN
/// must end with `:*`").
///
/// RECORDED text, and ENTIRELY FIXED: the service states the RULE the ARN broke —
/// the qualifier a log group ARN is given with — and carries no value of the request,
/// so neither the submitted ARN, the log group nor the state machine appears in it.
/// The rule is what the caller has to act on, and it is the same sentence whichever
/// ARN was sent.
pub const LOG_GROUP_ARN_NEEDS_QUALIFIER: &str =
    "Invalid Logging Configuration: Log Group ARN must be provided with '*' qualifier.";

/// Why `configuration` is not a logging configuration this service accepts, or
/// `None` when it is — the modelled `InvalidLoggingConfiguration`.
///
/// The conditions come from the shape's own members:
///
/// * `destinations` is "limited to size 1" and is "required, if your log level is
///   not set to OFF", so a level that logs needs exactly one destination and no
///   configuration may carry two;
/// * a destination describes a CloudWatch log group, so one that names no log group
///   describes nothing;
/// * `logGroupArn` is the ARN of a CloudWatch Logs log group and "must end with
///   `:*`" ([`LOG_GROUP_ARN_NEEDS_QUALIFIER`]).
///
/// `level` is a modelled enum, so the centralized walk has already refused a value
/// outside it. Every operation that accepts a `loggingConfiguration` reads this, so
/// one rule covers them all.
pub fn invalid_logging_reason(configuration: &LoggingConfiguration) -> Option<String> {
    let level = configuration.level.as_deref().unwrap_or(LOG_LEVEL_OFF);
    let destinations = configuration.destinations.as_deref().unwrap_or_default();
    if destinations.len() > MAX_LOG_DESTINATIONS
        || (level != LOG_LEVEL_OFF && destinations.len() != MAX_LOG_DESTINATIONS)
    {
        return Some(
            "Invalid Logging Configuration: Must specify exactly one Log Destination.".to_string(),
        );
    }
    for destination in destinations {
        let Some(log_group_arn) = destination
            .cloud_watch_logs_log_group
            .as_ref()
            .and_then(|group| group.log_group_arn.as_deref())
        else {
            return Some(
                "Invalid Logging Configuration: Must specify a CloudWatch Logs log group."
                    .to_string(),
            );
        };
        if !names_log_group(log_group_arn) {
            return Some(LOG_GROUP_ARN_NEEDS_QUALIFIER.to_string());
        }
    }
    None
}

/// Whether `value` is the ARN of a CloudWatch Logs log group, which `logGroupArn`
/// documents as ending with `:*`.
fn names_log_group(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    let (Some(scheme), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    scheme == "arn"
        && !partition.is_empty()
        && service == "logs"
        && resource.starts_with("log-group:")
        && resource.ends_with(":*")
}

// ---------------------------------------------------------------------------
// What a read reports
//
// A describe reports a state machine's EFFECTIVE configuration: a configuration
// structure the create or the update did not set is still part of what the
// resource is, so it is reported at the default its own model declares rather
// than omitted. Omitting it tells a client nothing — it cannot tell "the default"
// from "this emulator does not keep that" — while the default is exactly what the
// service applies to the resource.
// ---------------------------------------------------------------------------

/// The `loggingConfiguration` a read REPORTS for a state machine.
///
/// `LoggingConfiguration` is `{level: LogLevel, includeExecutionData: boolean
/// (default false), destinations: LogDestinationList}`. An absent `level` is
/// [`LOG_LEVEL_OFF`] and an absent `includeExecutionData` is the member's own
/// modelled default, which the generated `bool` already carries.
/// [`effective_logging_configuration`] renders the same effective value for the
/// create-idempotency fingerprint; this one is the shape a response carries.
pub fn reported_logging_configuration(
    stored: Option<&LoggingConfiguration>,
) -> LoggingConfiguration {
    let mut effective = stored.cloned().unwrap_or_default();
    if effective.level.is_none() {
        effective.level = Some(LOG_LEVEL_OFF.to_string());
    }
    effective
}

/// The `tracingConfiguration` a read REPORTS for a state machine.
///
/// `TracingConfiguration` is `{enabled: boolean (default false)}` — "Selects
/// whether or not the state machine's X-Ray tracing is enabled. Default is false" —
/// so the default structure IS the effective one when nothing configured tracing.
pub fn reported_tracing_configuration(
    stored: Option<&TracingConfiguration>,
) -> TracingConfiguration {
    stored.cloned().unwrap_or_default()
}

/// The `encryptionConfiguration` a read REPORTS for a state machine.
///
/// `EncryptionConfiguration.type` is REQUIRED (`AWS_OWNED_KEY` |
/// `CUSTOMER_MANAGED_KMS_KEY`), and only the customer-managed type carries a
/// `kmsKeyId` and a data-key reuse period ([`invalid_encryption_reason`] refuses
/// them under the other), so a state machine created without an encryption
/// configuration is encrypted under [`ENCRYPTION_TYPE_AWS_OWNED_KEY`] with neither.
pub fn reported_encryption_configuration(
    stored: Option<&EncryptionConfiguration>,
) -> EncryptionConfiguration {
    let mut effective = stored.cloned().unwrap_or_default();
    if effective.r#type.is_empty() {
        effective.r#type = ENCRYPTION_TYPE_AWS_OWNED_KEY.to_string();
    }
    effective
}

/// What a request's `includedData` asks a read to report.
///
/// `IncludedData` is `ALL_DATA | METADATA_ONLY`: an operation that reports a state
/// machine `definition` reports it in full for the former and as
/// [`METADATA_ONLY_DEFINITION`] for the latter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludedData {
    /// Report everything, which is also what an absent `includedData` asks for:
    /// every reader that passed none before the member existed still sees the whole
    /// definition.
    AllData,
    /// Report the metadata and none of the definition.
    MetadataOnly,
}

impl IncludedData {
    /// What `value` names, or `None` when it names nothing `IncludedData` declares.
    pub fn from_wire(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some(INCLUDED_DATA_ALL_DATA) => Some(Self::AllData),
            Some(INCLUDED_DATA_METADATA_ONLY) => Some(Self::MetadataOnly),
            Some(_) => None,
        }
    }

    /// Whether this read reports the metadata only.
    pub fn is_metadata_only(self) -> bool {
        matches!(self, Self::MetadataOnly)
    }
}

/// The `definition` a read reports, given its `includedData`.
///
/// From the `definition` output member: "If called with includedData =
/// METADATA_ONLY, the returned definition will be {}." Everything DERIVED from the
/// definition — `variableReferences` — is withheld for the same read, because the
/// definition is what it is read from.
pub fn reported_definition(definition: &str, included_data: IncludedData) -> String {
    if included_data.is_metadata_only() {
        METADATA_ONLY_DEFINITION.to_string()
    } else {
        definition.to_string()
    }
}

/// The `variableReferences` a read reports for `definition`, given its
/// `includedData`.
///
/// Two rules the member documents, applied in ONE place so every read that reports
/// a state machine's configuration — the state machine itself, a published version
/// of it, or the configuration an execution ran — reports them the same way:
///
/// * "states that do not use variable references will not be shown in the
///   response", so a definition whose states reference none is no member at all
///   rather than an empty map;
/// * the member is read OUT OF the definition, so a metadata-only read — which
///   reports no definition ([`reported_definition`]) — reports nothing derived from
///   one either.
pub fn reported_variable_references(
    definition: &str,
    included_data: IncludedData,
) -> Option<HashMap<String, Vec<String>>> {
    if included_data.is_metadata_only() {
        return None;
    }
    let references = asl::variable_references(definition);
    (!references.is_empty()).then(|| references.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// How many items one page of a list operation carries.
///
/// `maxResults` is modelled `0..1000`, so the walk has already refused anything
/// outside that; `0` means the default, per the member's own documentation.
pub fn page_size(max_results: i32) -> usize {
    if max_results <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        usize::try_from(max_results).unwrap_or(DEFAULT_PAGE_SIZE)
    }
}

/// The offset a page token names, or `None` when the token is not one this service
/// issued — which the modelled `InvalidToken` reports.
///
/// A token is the decimal offset of the next item in the operation's own total
/// order, which is what makes it a token "for each page" that keeps meaning across
/// calls.
pub fn page_offset(next_token: Option<&str>) -> Option<usize> {
    match next_token {
        None => Some(0),
        Some(token) => token.parse::<usize>().ok(),
    }
}

/// The token that names the next page, or `None` when `items` were not truncated.
pub fn next_page_token(offset: usize, page: usize, total: usize) -> Option<String> {
    let next = offset.saturating_add(page);
    (next < total).then(|| next.to_string())
}

/// What a HISTORY page token names: the position in this execution's OWN event order
/// — ascending, zero-based — at which the next page begins.
///
/// "The value of nextToken is a unique pagination token for each page. Make the call
/// again using the returned token to retrieve the next page. Keep all other
/// arguments unchanged": the token is a cursor into the HISTORY rather than a count
/// of pages, so the position a read continues from is the event it stopped at,
/// whichever end of the history that read walks from. That is also what binds a
/// token to the read that issued it: a backwards read continues from a position no
/// page of a forwards read ever begins at, so each order's tokens are the other's
/// `InvalidToken`.
///
/// The mapping is its own inverse, so one function turns a position in the READ's
/// order into the cursor the token carries and back.
fn history_cursor(position: usize, total: usize, reverse_order: bool) -> usize {
    if reverse_order {
        total.saturating_sub(1).saturating_sub(position)
    } else {
        position
    }
}

/// The offset into this read's own order that the page token `next_token` names, or
/// `None` when it is not a token this read handed out — which the modelled
/// `InvalidToken` reports.
///
/// A token this read issued names one of ITS page boundaries, inside this
/// execution's history, never the first page (which is asked for by sending no
/// token at all), and no further than this read has got — `handed_out` is how far
/// each read of this history has handed cursors out
/// ([`Execution::issued_page_cursors`]). So text that is no position, a position no
/// page of this read begins at, and a cursor minted for another execution, the other
/// sort order or another page size all resolve to nothing.
pub fn history_page_offset(
    next_token: Option<&str>,
    read: HistoryRead,
    total: usize,
    handed_out: &[HistoryPageCursor],
) -> Option<usize> {
    let Some(token) = next_token else {
        return Some(0);
    };
    let cursor = token.parse::<usize>().ok()?;
    if cursor >= total {
        return None;
    }
    let offset = history_cursor(cursor, total, read.reverse_order);
    let boundary = read.page > 0 && offset.is_multiple_of(read.page);
    if offset == 0 || offset >= total || !boundary {
        return None;
    }
    let reached = handed_out.iter().find(|remembered| remembered.of(read))?;
    (offset <= reached.offset).then_some(offset)
}

/// The cursor that names the next page of a history read, or `None` when the page
/// was not truncated — the one [`history_page_offset`] resolves back, and the one
/// the execution remembers handing out.
pub fn next_history_page_cursor(
    offset: usize,
    page: usize,
    total: usize,
    read: HistoryRead,
) -> Option<HistoryPageCursor> {
    let next = offset.saturating_add(page);
    (next < total).then(|| read.at(next))
}

/// The `nextToken` text a cursor into a history of `total` events is carried as: the
/// position in the EXECUTION's own event order ([`history_cursor`]).
pub fn history_page_token(cursor: HistoryPageCursor, total: usize) -> String {
    history_cursor(cursor.offset, total, cursor.reverse_order).to_string()
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Now.
pub fn now() -> SystemTime {
    SystemTime::now()
}

/// `time` as the modelled `Timestamp` the wire carries.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis(time))
}

/// `time` in whole milliseconds since the Unix epoch.
pub fn epoch_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// `offset` after `time`, without the panic `SystemTime`'s `Add` carries.
///
/// The offsets this service adds to an instant are accumulated from the `Wait` states
/// of a caller's definition, which the model bounds only in BYTES, so an offset no
/// instant can represent holds at the furthest instant this clock carries instead of
/// panicking: the execution then simply never comes due, which is what an unreachable
/// instant means.
pub fn after(time: SystemTime, offset: Duration) -> SystemTime {
    time.checked_add(offset).unwrap_or_else(|| {
        // The furthest instant reachable by halving the offset until it fits: a few
        // steps at most, and it needs no platform-specific maximum.
        let mut offset = offset;
        let mut furthest = time;
        while !offset.is_zero() {
            match time.checked_add(offset) {
                Some(instant) => {
                    furthest = instant;
                    break;
                }
                None => offset /= 2,
            }
        }
        furthest
    })
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as u64)
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `high` and `low` rendered as a version-4 UUID.
///
/// RFC 4122 §4.4: the four most significant bits of the 7th byte carry the version
/// (`0b0100`) and the two most significant bits of the 9th carry the variant
/// (`0b10`); the remaining 122 bits are the drawn value. Rendered 8-4-4-4-12 in
/// lowercase hex, which is the form the service's own docs show.
fn uuid_v4(high: u64, low: u64) -> String {
    let time_low = (high >> 32) as u32;
    let time_mid = (high >> 16) as u16;
    let time_hi_and_version = ((high as u16) & 0x0fff) | 0x4000;
    let clock_seq = (((low >> 48) as u16) & 0x3fff) | 0x8000;
    let node = low & 0x0000_ffff_ffff_ffff;
    format!("{time_low:08x}-{time_mid:04x}-{time_hi_and_version:04x}-{clock_seq:04x}-{node:012x}")
}

// ---------------------------------------------------------------------------
// Running an execution
// ---------------------------------------------------------------------------

/// One attempt at running an execution: what is interpreted, over what input, and
/// where the attempt's history continues from.
///
/// The members travel together — every caller of [`plan_run`] has all of them and
/// none of them means anything without the rest — so they are one argument.
pub struct RunAttempt<'a> {
    pub definition: &'a str,
    pub input: &'a str,
    /// When this attempt started, which every offset the walk reports is measured
    /// from.
    pub start: SystemTime,
    /// What the definition reads through the context object (`$$`): the execution's
    /// and the state machine's own identity, which the caller's request already
    /// fixed.
    pub context: &'a asl::Context,
    /// The type of the event the attempt opens its history with —
    /// [`EVENT_EXECUTION_STARTED`] for a `StartExecution`,
    /// [`EVENT_EXECUTION_REDRIVEN`] for a `RedriveExecution`.
    pub opening: &'a str,
    /// The last id the execution's history already holds, so a redrive's events
    /// CONTINUE the existing numbering instead of restarting it.
    pub previous_event_id: i64,
}

/// Interpret `attempt`'s definition against its input and schedule what the clock
/// owes.
///
/// The run happens HERE, in the call that starts it: [`asl::run`] walks the states
/// and what it reached is reported as a [`Planned`] — a terminal outcome parked in a
/// [`PendingOutcome`] due at `start + EXECUTION_SETTLE_DELAY + <every Wait the walk
/// passed>`, or the activity task the walk SUSPENDED on, which the execution waits
/// at until a worker answers it.
///
/// `tasks` is what the `Task` states of the definition make their service
/// integration calls through ([`integration::interpret`]).
pub fn plan_run(
    attempt: &RunAttempt<'_>,
    tasks: &dyn integration::Tasks,
) -> Planned {
    plan_run_settling_after(attempt, EXECUTION_SETTLE_DELAY, tasks)
}

/// Plan the attempt a `RedriveExecution` begins.
///
/// "When you redrive an execution, it continues the failed execution from the
/// unsuccessful step and uses the same input": when the execution recorded the step
/// its last attempt stopped at ([`asl::RedriveStep`]), the walk begins THERE, with the
/// raw input that step was entered with, so the steps it already got past are not run
/// again; with no such step recorded, the whole definition is walked again from its
/// `StartAt`, which for the deterministic states this emulator runs reaches the same
/// values those steps produced.
///
/// Either way the attempt opens its history with [`EVENT_EXECUTION_REDRIVEN`] —
/// "redriven executions append their event history to the existing event history" —
/// and its outcome is owed to the clock exactly as a start's is.
///
/// `tasks` is the seam [`plan_run`] takes: the step the redrive continues from, and
/// every state after it, makes its service-integration call exactly as the first
/// attempt's did.
pub fn plan_redrive(
    attempt: &RunAttempt<'_>,
    from: Option<&asl::RedriveStep>,
    tasks: &dyn integration::Tasks,
) -> Planned {
    let Some(from) = from else {
        return plan_run(attempt, tasks);
    };
    let RunAttempt {
        definition,
        input,
        start,
        context,
        opening,
        previous_event_id,
    } = *attempt;
    let run = asl::redrive(definition, input, start, context, from, tasks);
    plan_from_run(run, start, EXECUTION_SETTLE_DELAY, Some(opening), previous_event_id)
}

/// What one planned run of a definition left for the store to record.
///
/// At most one of `pending`, `waiting` and `parked` is set: a walk either reached a
/// terminal outcome the clock owes, stopped at an activity task the execution is
/// waiting for, or parked at the job a run-a-job integration started.
#[derive(Debug, Clone)]
pub struct Planned {
    /// The events the attempt has ALREADY produced: the opening event, and — for a
    /// run that suspended — the events of the states it walked up to the task it is
    /// waiting at, which have happened and are therefore history rather than
    /// something the clock still owes.
    pub opening: Vec<HistoryEvent>,
    /// The terminal outcome the walk computed, owed to the clock.
    pub pending: Option<PendingOutcome>,
    /// The activity task the walk suspended on.
    pub waiting: Option<PlannedActivityTask>,
    /// The job of a run-a-job integration the walk parked at.
    pub parked: Option<PlannedJob>,
    /// The Map Run every Distributed Map state the walk entered started, with the
    /// offsets the walk reported turned into instants.
    pub map_runs: Vec<PlannedMapRun>,
    /// The unsuccessful step this attempt stopped at, which a later
    /// `RedriveExecution` continues the execution from ([`asl::Run::stopped_at`]).
    pub stopped_at: Option<asl::RedriveStep>,
}

/// One Map Run a planned run started, ready for the store to record.
///
/// This is [`asl::StartedMapRun`] with the run's offsets turned into instants: the
/// Map Run started when its `Map` state was entered and reaches the status the walk
/// computed when the clock makes it due, which is the same delay the execution's own
/// outcome is owed after.
#[derive(Debug, Clone)]
pub struct PlannedMapRun {
    pub label: String,
    pub status: String,
    pub start_date: SystemTime,
    pub due_at: SystemTime,
    pub max_concurrency: i32,
    pub tolerated_failure_count: i64,
    pub tolerated_failure_percentage: f32,
    pub total_items: i64,
    /// Each item the `Map` state iterated, with the instant its iteration finished:
    /// what the stored record counts its items from.
    pub items: Vec<MapRunItem>,
    /// The child execution each of those items ran as, in the order they started:
    /// what `ListExecutions` reports for this Map Run once the store holds them.
    pub children: Vec<PlannedChildExecution>,
}

/// One child workflow execution a planned run's Distributed Map state started, ready
/// for the store to record as an execution of its own.
///
/// This is [`asl::StartedChildExecution`] with the run's offsets turned into
/// instants: the child started when its iteration did and reaches the terminal status
/// its iteration produced when the clock makes it due — the same delay the Map Run
/// around it and the execution around that are owed after, so no child reports a
/// terminal status before the Map Run that started it could.
#[derive(Debug, Clone)]
pub struct PlannedChildExecution {
    pub name: String,
    pub arn: String,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub status: String,
    pub start_date: SystemTime,
    pub due_at: SystemTime,
    pub item_count: i32,
}

impl PlannedChildExecution {
    /// The bytes the execution record this will become retains.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<Execution>()
            + self.name.len()
            + self.arn.len()
            + self.input.len()
            + self.output.as_ref().map_or(0, String::len)
            + self.error.as_ref().map_or(0, String::len)
            + self.cause.as_ref().map_or(0, String::len)
            // `status` is held twice while the terminal one is still owed to the
            // clock, as the Map Run's own is.
            + 2 * self.status.len()
            // The state machine ARN and the Map Run ARN the record carries, each
            // inside the modelled maximum.
            + 2 * MAX_ARN_BYTES
            // The two history events a child's own life appends: the one it opens
            // with and the terminal one it ends on.
            + 2 * (std::mem::size_of::<HistoryEvent>() + EVENT_EXECUTION_FAILED.len())
    }
}

impl PlannedMapRun {
    /// The bytes the record this will become retains, so an execution is admitted
    /// against the whole of what its start adds to the store.
    ///
    /// The child executions this Map Run started are records of their own, so what
    /// they retain is counted here too: the start is admitted against everything it
    /// writes.
    pub fn measure(&self) -> usize {
        std::mem::size_of::<MapRun>()
            + self.label.len()
            // `status` is held twice while the terminal one is still owed to the clock.
            + 2 * self.status.len()
            + self.items.len().saturating_mul(std::mem::size_of::<MapRunItem>())
            // The three ARNs the stored record carries. Each is built from a state
            // machine name, an execution name and a state label, all of which the
            // model bounds well inside the `Arn` maximum.
            + 3 * MAX_ARN_BYTES
            + self
                .children
                .iter()
                .map(PlannedChildExecution::measure)
                .sum::<usize>()
    }
}

/// The bytes a run of planned Map Runs retains together.
pub fn planned_map_run_bytes(planned: &[PlannedMapRun]) -> usize {
    planned.iter().map(PlannedMapRun::measure).sum()
}

/// As [`plan_run`], with the delay after which the outcome settles stated by the
/// caller rather than taken as [`EXECUTION_SETTLE_DELAY`].
///
/// An ASYNCHRONOUS start (`StartExecution`, `RedriveExecution`) answers while the
/// execution is still `RUNNING`, so the terminal outcome it computed is owed to the
/// clock and settles one [`EXECUTION_SETTLE_DELAY`] later. A SYNCHRONOUS start
/// (`StartSyncExecution`) answers the terminal outcome in the call itself — "the
/// status code in the API response doesn't reflect function errors", so the response
/// already carries `status`, `stopDate` and the output — so it owes the clock
/// nothing and passes `Duration::ZERO`.
pub fn plan_run_settling_after(
    attempt: &RunAttempt<'_>,
    settle: Duration,
    tasks: &dyn integration::Tasks,
) -> Planned {
    let RunAttempt {
        definition,
        input,
        start,
        context,
        opening,
        previous_event_id,
    } = *attempt;
    let run = asl::run(definition, input, start, context, tasks);
    plan_from_run(run, start, settle, Some(opening), previous_event_id)
}

/// Plan the CONTINUATION of an execution whose suspended activity task a worker has
/// answered.
///
/// The definition is walked on FROM the state that scheduled the task, with the raw
/// input that state was entered with and `reply` as its result (see [`asl::resume`]),
/// so what this reports is what the execution does from that reply onward: the events
/// it has produced since, and the terminal outcome it reached, owed to the clock one
/// [`EXECUTION_SETTLE_DELAY`] after the reply just as a start's outcome is owed after
/// the start.
///
/// `start` is the execution's own start, which the context object (`$$`) reports and a
/// `Wait` state's `Timestamp` is measured against; `now` is the instant the reply
/// arrived, which the continuation's own events are stamped from.
///
/// `tasks` is what the `Task` states AFTER the reply make their service integration
/// calls through, exactly as the start's own states did.
///
/// `None` when the definition no longer declares the state the task was scheduled at,
/// so there is no continuation to record.
#[allow(clippy::too_many_arguments)]
pub fn plan_reply(
    definition: &str,
    input: &str,
    start: SystemTime,
    now: SystemTime,
    context: &asl::Context,
    reply: &asl::ActivityReply,
    previous_event_id: i64,
    tasks: &dyn integration::Tasks,
) -> Option<Planned> {
    // What the execution has already spent, which a definition-level `TimeoutSeconds`
    // counts: a task that stayed outstanding was part of this execution's run.
    let elapsed = now.duration_since(start).unwrap_or_default();
    let run = asl::resume(definition, input, start, context, reply, elapsed, tasks)?;
    let mut planned = plan_from_run(run, now, EXECUTION_SETTLE_DELAY, None, previous_event_id);
    // The event that records the task SUCCEEDING carries what the worker answered:
    // "the JSON output of the task", which is the reply this walk continued from.
    let answered = planned
        .opening
        .iter_mut()
        .chain(
            planned
                .pending
                .iter_mut()
                .flat_map(|pending| pending.events.iter_mut()),
        )
        .find(|event| event.event_type == EVENT_ACTIVITY_SUCCEEDED);
    if let Some(event) = answered {
        event.details = EventDetails {
            output: Some(reply.result.to_string()),
            ..EventDetails::default()
        };
    }
    Some(planned)
}

/// Plan the CONTINUATION of an execution whose parked job has ENDED.
///
/// The definition is walked on FROM the state that started the job, with the raw
/// input that state was entered with and what the job ended as its outcome (see
/// [`asl::resume_job`]), so what this reports is what the execution does from that
/// outcome onward: the events it has produced since, and the terminal outcome it
/// reached, owed to the clock one [`EXECUTION_SETTLE_DELAY`] after the job ended just
/// as a worker's reply's outcome is owed after the reply.
///
/// `start` is the execution's own start, which the context object (`$$`) reports and a
/// `Wait` state's `Timestamp` is measured against; `now` is the instant the job was
/// found finished, which the continuation's own events are stamped from.
///
/// `None` when the definition no longer declares the state that started the job, so
/// there is no continuation to record.
#[allow(clippy::too_many_arguments)]
pub fn plan_job_reply(
    definition: &str,
    input: &str,
    start: SystemTime,
    now: SystemTime,
    context: &asl::Context,
    reply: &asl::JobReply,
    previous_event_id: i64,
    tasks: &dyn integration::Tasks,
) -> Option<Planned> {
    // What the execution has already spent, which a definition-level `TimeoutSeconds`
    // counts: a job that stayed unfinished was part of this execution's run.
    let elapsed = now.duration_since(start).unwrap_or_default();
    let run = asl::resume_job(definition, input, start, context, reply, elapsed, tasks)?;
    Some(plan_from_run(
        run,
        now,
        EXECUTION_SETTLE_DELAY,
        None,
        previous_event_id,
    ))
}

/// What a worker's reply that reports its activity task FAILED leaves for the store
/// to record, once no `Retry` of the scheduling state answers for the error.
///
/// There is no walk to interpret: the state has no retrier left for the error
/// ([`ComAmazonawsSfnAwsstepfunctionsState::retry_failed_activity_task`] is what
/// answers one that has) and declares no `Catch` this emulator interprets (see
/// [`asl`]), so the execution waiting at the task fails, carrying the reply's own
/// `error` and `cause`. That outcome is owed to the clock one
/// [`EXECUTION_SETTLE_DELAY`] after the reply, exactly as the outcome a SUCCESSFUL
/// reply continues to is ([`plan_reply`]), so one rule covers both replies: the
/// execution is `RUNNING` until the instant the reply made its outcome due.
///
/// `state_name` is the state that scheduled the task, as the stored task names it,
/// and `previous_event_id` the last event the execution's history holds, so the two
/// events this appends continue that numbering.
pub fn plan_task_failure(
    state_name: &str,
    now: SystemTime,
    error: Option<String>,
    cause: Option<String>,
    previous_event_id: i64,
) -> Planned {
    let due_at = after(now, EXECUTION_SETTLE_DELAY);
    // The task failed when the worker said so; the execution ends when its outcome
    // comes due — the same two instants [`plan_from_run`] stamps a walked state and a
    // terminal event with.
    let failed = HistoryEvent {
        id: previous_event_id + 1,
        previous_event_id,
        event_type: EVENT_ACTIVITY_FAILED.to_string(),
        timestamp: now,
        state_name: Some(state_name.to_string()),
        details: EventDetails::of_failure(error.clone(), cause.clone()),
    };
    let ended = HistoryEvent {
        id: failed.id + 1,
        previous_event_id: failed.id,
        event_type: EVENT_EXECUTION_FAILED.to_string(),
        timestamp: due_at,
        state_name: None,
        details: EventDetails::of_failure(error.clone(), cause.clone()),
    };
    Planned {
        opening: vec![failed],
        pending: Some(PendingOutcome {
            due_at,
            status: EXECUTION_STATUS_FAILED.to_string(),
            output: None,
            error,
            cause,
            events: vec![ended],
        }),
        waiting: None,
        parked: None,
        map_runs: Vec::new(),
        // The attempt stopped at the state that scheduled the task, which the caller
        // records from the task itself.
        stopped_at: None,
    }
}

/// What one walk left for the store to record: its events numbered after
/// `previous_event_id`, its outcome owed `settle` after `start`, and the activity task
/// or Map Runs it produced.
///
/// `opening` is the event the attempt OPENS with, for a walk that is an attempt of its
/// own (`ExecutionStarted`, `ExecutionRedriven`); a walk that CONTINUES an attempt — a
/// worker's reply to the task it was suspended at — opens with none, because the
/// attempt it continues was already opened.
fn plan_from_run(
    mut run: asl::Run,
    start: SystemTime,
    settle: Duration,
    opening: Option<&str>,
    previous_event_id: i64,
) -> Planned {
    let stopped_at = run.stopped_at.take();
    let opened: Vec<HistoryEvent> = opening
        .map(|event_type| HistoryEvent {
            id: previous_event_id + 1,
            previous_event_id,
            event_type: event_type.to_string(),
            timestamp: start,
            state_name: None,
            details: EventDetails::default(),
        })
        .into_iter()
        .collect();
    let first_id = previous_event_id + 1 + i64::try_from(opened.len()).unwrap_or(0);
    // What the FIRST event of this walk names as its predecessor: the event the walk
    // opened with, or — for a walk that continues an attempt — the event the history
    // already ended on. An `ExecutionStarted` is no predecessor, so the event after it
    // reports the sentinel ([`follows_event`]).
    let first_previous = match opening {
        Some(event_type) => follows_event(first_id - 1, event_type),
        None => first_id - 1,
    };
    // The offsets below are accumulated from the `Wait` states of a CALLER'S
    // definition, so every instant is computed with checked arithmetic: an
    // offset no instant can represent is held at the furthest instant this clock
    // carries, rather than reached through the panicking `Add`.
    let due_at = after(start, settle.saturating_add(run.elapsed));
    // What this attempt ENDED with, which is what the event it ends on reports: a
    // later attempt of the same execution computes its own outcome, and the event
    // this one wrote stays in the history.
    let outcome = EventDetails::of_outcome(
        run.output.clone(),
        run.error.clone(),
        run.cause.clone(),
    );
    let events: Vec<HistoryEvent> = run
        .steps
        .into_iter()
        .zip(first_id..)
        .map(|(step, id)| HistoryEvent {
            id,
            previous_event_id: if id == first_id {
                first_previous
            } else {
                id - 1
            },
            event_type: step.event_type.to_string(),
            // Each state was entered and left at its own offset into the run; the
            // terminal event is stamped at the instant the outcome came due.
            timestamp: if step.terminal {
                due_at
            } else {
                after(start, step.after)
            },
            state_name: step.state_name,
            details: if step.terminal {
                outcome.clone()
            } else {
                step.details
            },
        })
        .collect();
    // Every Map Run the walk started, with its offsets turned into instants against
    // the execution's own start: it started when its `Map` state was entered, and
    // reaches the status the walk computed one `settle` after its items finished —
    // the same delay the execution's own outcome is owed after, so a Map Run never
    // reports a terminal status before the execution around it could.
    let map_runs: Vec<PlannedMapRun> = run
        .map_runs
        .into_iter()
        .map(|map_run| PlannedMapRun {
            label: map_run.label,
            status: map_run.status.to_string(),
            start_date: after(start, map_run.started_at),
            due_at: after(start, settle.saturating_add(map_run.stopped_at)),
            max_concurrency: map_run.max_concurrency,
            tolerated_failure_count: map_run.tolerated_failure_count,
            tolerated_failure_percentage: map_run.tolerated_failure_percentage,
            total_items: map_run.total_items,
            // Each ITEM is owed the same way the Map Run around it is: its iteration
            // finished at its own offset into the walk, and is counted one `settle`
            // after that instant — so the last item to finish comes due exactly when
            // the Map Run's own status does, and no item is counted in the call that
            // started the execution.
            items: map_run
                .items
                .into_iter()
                .map(|item| MapRunItem {
                    due_at: after(start, settle.saturating_add(item.finished_at)),
                    failed: item.failed,
                })
                .collect(),
            // And each CHILD execution the same way: it started when its own
            // iteration did, and the terminal status that iteration produced is owed
            // one `settle` after the instant it finished — so a caller reading a
            // child straight after the start sees it RUNNING, exactly as the Map Run
            // and the execution around it are.
            children: map_run
                .children
                .into_iter()
                .map(|child| PlannedChildExecution {
                    name: child.name,
                    arn: child.arn,
                    input: child.input,
                    output: child.output,
                    error: child.error,
                    cause: child.cause,
                    status: child.status.to_string(),
                    start_date: after(start, child.started_at),
                    due_at: after(start, settle.saturating_add(child.finished_at)),
                    item_count: child.item_count,
                })
                .collect(),
        })
        .collect();
    // A walk that SUSPENDED computed no outcome: what it produced is the task it is
    // waiting at, available to a worker once the clock reaches the offset the walk
    // got to — every `Wait` before the state is still owed, and nothing slept for
    // them. The events of the states it walked are history, so they are recorded now
    // rather than parked.
    if let Some(waiting) = run.waiting {
        let mut history = opened;
        history.extend(events);
        let scheduled = PlannedActivityTask {
            activity_arn: waiting.activity_arn,
            state_name: waiting.state_name,
            input: waiting.input,
            state_input: waiting.raw_input.to_string(),
            available_at: after(start, waiting.at),
            timeout_seconds: waiting.timeout_seconds,
            heartbeat_seconds: waiting.heartbeat_seconds,
        };
        // The event that RECORDS the scheduling carries what was scheduled: the walk
        // computed the activity, the input and the bounds, so the event the history
        // keeps reports them rather than leaving its detail member unset.
        if let Some(event) = history
            .iter_mut()
            .rev()
            .find(|event| event.event_type == EVENT_ACTIVITY_SCHEDULED)
        {
            event.details = EventDetails {
                resource: Some(scheduled.activity_arn.clone()),
                input: Some(scheduled.input.clone()),
                timeout_seconds: scheduled.timeout_seconds,
                heartbeat_seconds: scheduled.heartbeat_seconds,
                ..EventDetails::default()
            };
        }
        return Planned {
            opening: history,
            pending: None,
            waiting: Some(scheduled),
            parked: None,
            map_runs,
            stopped_at,
        };
    }
    // A walk that PARKED at a job computed no outcome either: the job was started at
    // the offset the walk got to, and what happens next is that job finishing. The
    // events of the states it walked — including the one recording the submission —
    // are history, so they are recorded now rather than parked.
    if let Some(parked) = run.parked {
        let mut history = opened;
        history.extend(events);
        return Planned {
            opening: history,
            pending: None,
            waiting: None,
            parked: Some(PlannedJob {
                state_name: parked.state_name,
                state_input: parked.raw_input.to_string(),
                job: parked.job,
                api: parked.api,
                parsed_output: parked.parsed_output,
                job_id: parked.job_id,
                naming: parked.naming,
                parked_at: after(start, parked.at),
                timeout_seconds: parked.timeout_seconds,
            }),
            map_runs,
            stopped_at,
        };
    }
    Planned {
        opening: opened,
        pending: Some(PendingOutcome {
            due_at,
            status: run.status.to_string(),
            output: run.output,
            error: run.error,
            cause: run.cause,
            events,
        }),
        waiting: None,
        parked: None,
        map_runs,
        stopped_at,
    }
}

// ---------------------------------------------------------------------------
// What a synchronous execution is billed
// ---------------------------------------------------------------------------

/// The increment `billedDurationInMilliseconds` is reported in.
///
/// An AWS-published billing increment, from Step Functions' pricing for Express
/// Workflows: a workflow's duration "is rounded up to the nearest 100 ms". It is
/// therefore also the SMALLEST billed duration: a run that ended inside the first
/// increment is billed one.
pub const BILLED_DURATION_INCREMENT_MILLIS: i64 = 100;

/// The increment `billedMemoryUsedInMB` is reported in.
///
/// The same source: Express Workflow memory consumption is billed in 64 MB
/// increments, the smallest of which is what a workflow whose data fits inside it
/// costs.
pub const BILLED_MEMORY_INCREMENT_MB: i64 = 64;

/// What `StartSyncExecution` reports as the run's billed duration: the span from the
/// execution's start to its stop, rounded UP to the next
/// [`BILLED_DURATION_INCREMENT_MILLIS`], and never less than one increment.
pub fn billed_duration_millis(start: SystemTime, stop: SystemTime) -> i64 {
    let ran = stop.duration_since(start).unwrap_or_default();
    let millis = i64::try_from(ran.as_millis()).unwrap_or(i64::MAX);
    round_up_to(millis, BILLED_DURATION_INCREMENT_MILLIS)
}

/// What `StartSyncExecution` reports as the run's billed memory: the bytes the run
/// actually retained (its input, the outcome it computed and its history), rounded UP
/// to the next [`BILLED_MEMORY_INCREMENT_MB`], and never less than one increment.
///
/// A run interpreted in this process retains at most the modelled payload maxima, so
/// it normally fits inside the first increment and is billed exactly one — the same
/// floor the published increments give a small workflow.
pub fn billed_memory_mb(retained_bytes: usize) -> i64 {
    let megabytes = i64::try_from(retained_bytes.div_ceil(1024 * 1024)).unwrap_or(i64::MAX);
    round_up_to(megabytes, BILLED_MEMORY_INCREMENT_MB)
}

/// `value` rounded up to the next multiple of `increment`, with one increment as the
/// floor, saturating rather than overflowing.
fn round_up_to(value: i64, increment: i64) -> i64 {
    let increments = value.max(1).saturating_add(increment - 1) / increment;
    increments.saturating_mul(increment)
}

pub mod asl;
pub mod encryption;
pub mod integration;
mod json_diagnostic;
pub mod run_a_job;
pub mod task_authorization;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSfnAwsstepfunctionsState>)
}
