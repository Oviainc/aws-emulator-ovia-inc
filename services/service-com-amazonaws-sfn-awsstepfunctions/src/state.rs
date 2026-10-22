// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sfn-awsstepfunctions service.
//!
//! Two resources live here, and every operation answers from them:
//!
//! * a **state machine**, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:stateMachine:<name>`). That ARN is a total
//!   function of the region and the name, so the ARN-keyed map IS the name index
//!   `CreateStateMachine`'s idempotency check needs — and a name is therefore scoped
//!   per region and account, the way the service scopes it, with no second copy of
//!   the record to keep in step;
//! * an **execution**, keyed by its own ARN
//!   (`arn:aws:states:<region>:<account>:execution:<stateMachineName>:<executionName>`),
//!   with `executions_by_sm` listing one state machine's executions in start order so
//!   `ListExecutions` pages without walking the whole store.
//!
//! ## What the clock owes
//!
//! Two transitions here are owed to the clock rather than applied in the call that
//! starts them, because that is what the service reports:
//!
//! * an execution starts `RUNNING` and reaches its terminal status later. The run is
//!   interpreted eagerly by [`asl::run`] at `StartExecution` — and again at
//!   `RedriveExecution`, which re-runs the same definition over the same input — and
//!   parked in [`PendingOutcome`], owed at
//!   `<the attempt's start> + EXECUTION_SETTLE_DELAY + <every Wait>`.
//! * `DeleteStateMachine` is documented as asynchronous: it "sets the state machine's
//!   status to `DELETING` and begins the deletion process", so the record moves to
//!   `DELETING` now and is removed once [`DELETE_SETTLE_DELAY`] has passed.
//!
//! Nothing sleeps: the due instant is stored and [`ComAmazonawsSfnAwsstepfunctionsState::settle`]
//! materialises whatever the clock has already made due. That is the only write a
//! reporting operation makes.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{EncryptionConfiguration, LoggingConfiguration, Tag, TracingConfiguration};

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

/// The resource type an activity ARN carries.
///
/// A Step Functions tagging call takes "the ARN for the Step Functions state
/// machine or activity", so this is the second resource type those calls accept —
/// even though no activity can exist in this emulator yet (`CreateActivity` still
/// answers 501), which is why an activity ARN is a resource that cannot be found
/// rather than an ARN that cannot be read.
pub const ACTIVITY_RESOURCE: &str = "activity";

/// Returned when the shared engine does not carry this service's store, which can
/// only happen on an engine this crate was not mounted on. The model declares no
/// server-fault error for any operation in this slice, so the refusal goes out
/// through the one non-modelled channel the crate has.
pub const STORE_UNAVAILABLE: &str = "the Step Functions store is unavailable on this engine";

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

/// An execution that has not reached a terminal status yet.
pub const EXECUTION_STATUS_RUNNING: &str = "RUNNING";

/// An execution that ran to a `Succeed` state or to `"End": true`.
pub const EXECUTION_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// An execution that ran to a `Fail` state, or that could not be run.
pub const EXECUTION_STATUS_FAILED: &str = "FAILED";

/// An execution `StopExecution` stopped.
pub const EXECUTION_STATUS_ABORTED: &str = "ABORTED";

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

/// The most tags one state machine may carry, refused with the modelled
/// `TooManyTags`. The AWS-wide per-resource tag limit.
pub const MAX_TAGS: usize = 50;

/// The most history events one execution keeps.
///
/// This is the interpreter's own ceiling, not a separate guess: a transition emits
/// at most two events, [`asl::MAX_STATE_TRANSITIONS`] bounds the transitions, and
/// `ExecutionStarted` plus one terminal event bracket them. Each event carries a
/// state name, which [`asl::MAX_STATE_NAME_CHARS`] bounds, so the history's BYTES
/// are bounded too.
pub const MAX_HISTORY_EVENTS: usize = 2 * asl::MAX_STATE_TRANSITIONS + 2;

/// What [`Execution::measure`] reserves for a stop that has not happened yet.
///
/// `StopExecution` writes the caller's `error` and `cause` and one event onto a
/// running execution. Reserving their maxima (the modelled `SensitiveError` and
/// `SensitiveCause` lengths, plus one event) while the execution is still running
/// means a re-measurement after the stop can only shrink the store's total, so
/// admission is the ONLY place the store grows and [`MAX_STORE_BYTES`] bounds what
/// is retained at every instant.
pub const STOP_RESERVE_BYTES: usize = asl::MAX_ERROR_BYTES
    + asl::MAX_CAUSE_BYTES
    + std::mem::size_of::<HistoryEvent>()
    + "ExecutionAborted".len();

/// Page size used when `maxResults` is absent or `0`.
///
/// From `ListExecutions`' and `ListStateMachines`' own `maxResults` documentation:
/// "The default is 100 and the maximum allowed page size is 1000. A value of 0 uses
/// the default."
pub const DEFAULT_PAGE_SIZE: usize = 100;

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
    /// The members `CreateStateMachine`'s idempotency check reads, rendered once at
    /// create time: see [`configuration_fingerprint`].
    pub fingerprint: String,
    /// When `DeleteStateMachine`'s removal becomes due; `None` while `ACTIVE`.
    pub delete_due_at: Option<SystemTime>,
    /// The bytes the store counts for this record against [`MAX_STORE_BYTES`].
    ///
    /// Set by [`ComAmazonawsSfnAwsstepfunctionsState::insert_state_machine`] from
    /// [`StateMachine::measure`], and moved by the one operation that changes what a
    /// stored state machine holds:
    /// [`ComAmazonawsSfnAwsstepfunctionsState::merge_state_machine_tags`] re-counts
    /// the tag set it wrote. A constructor may leave it `0`.
    pub retained_bytes: usize,
}

impl StateMachine {
    /// Whether `DeleteStateMachine` has already begun deleting this machine.
    pub fn is_deleting(&self) -> bool {
        self.delete_due_at.is_some()
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
            + self.fingerprint.len()
            + self.revision_id.as_ref().map_or(0, String::len)
            + self.description.as_ref().map_or(0, String::len)
            + tags_bytes(&self.tags)
    }
}

/// The bytes a tag set holds, so a tag write can move the store's total by what it
/// changed instead of re-measuring a record whose definition is up to 1 MiB.
fn tags_bytes(tags: &[Tag]) -> usize {
    tags.iter()
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
}

impl Execution {
    /// Whether this execution has reached a terminal status.
    pub fn is_terminal(&self) -> bool {
        self.status != EXECUTION_STATUS_RUNNING
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
            + redrive_tokens_bytes(&self.redrive_tokens)
            + history_bytes(&self.history)
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
        if self.history.len() >= MAX_HISTORY_EVENTS {
            return;
        }
        let previous_event_id = self.history.last().map_or(0, |event| event.id);
        self.history.push(HistoryEvent {
            id: previous_event_id + 1,
            previous_event_id,
            event_type: event_type.to_string(),
            timestamp,
            state_name: None,
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
        pending: PendingOutcome,
    ) {
        self.status = EXECUTION_STATUS_RUNNING.to_string();
        self.stop_date = None;
        self.output = None;
        self.error = None;
        self.cause = None;
        self.redrive_count = self.redrive_count.saturating_add(1);
        self.redrive_date = Some(now);
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
        for event in opening {
            if self.history.len() >= MAX_HISTORY_EVENTS {
                break;
            }
            self.history.push(event);
        }
        self.pending = Some(pending);
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
    /// Every execution, keyed by its ARN.
    pub executions: BTreeMap<String, Execution>,
    /// One state machine's execution ARNs in start order.
    pub executions_by_sm: HashMap<String, Vec<String>>,
    /// The execution ARNs that still carry a [`PendingOutcome`], so settling is
    /// proportional to what is actually owed rather than to the whole store.
    pending_executions: Vec<String>,
    /// The state machine ARNs that are `DELETING`, for the same reason.
    deleting_state_machines: Vec<String>,
    /// The bytes every stored record holds together, so the bound on the store is a
    /// bound on the PRODUCT of the record counts and the caller-sized text each
    /// record carries, not on each factor alone. See [`MAX_STORE_BYTES`].
    retained_bytes: usize,
    /// Mixed into each minted execution name so two mints in the same nanosecond
    /// differ. See [`ComAmazonawsSfnAwsstepfunctionsState::fresh_execution_name`].
    name_counter: u64,
}

impl ComAmazonawsSfnAwsstepfunctionsState {
    /// Materialise every transition the clock has already made due.
    ///
    /// This is the ONE write a reporting operation performs, and it is owed whether
    /// or not anybody called: an execution whose outcome came due is reported
    /// terminal, and a state machine whose deletion came due is gone. Executions
    /// settle before deletions, so an outcome owed before the machine disappeared is
    /// still recorded.
    pub fn settle(&mut self, now: SystemTime) {
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
        self.name_counter = self.name_counter.wrapping_add(1);
        let mut seed = now_nanos() ^ self.name_counter.rotate_left(29);
        let high = splitmix64(&mut seed);
        let low = splitmix64(&mut seed);
        uuid_v4(high, low)
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

    /// Merge `incoming` into the tags of the state machine `arn` names.
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
    pub fn merge_state_machine_tags(
        &mut self,
        arn: &str,
        incoming: &[Tag],
    ) -> Result<(), TagWriteRejection> {
        let Some(machine) = self.state_machines.get(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        if incoming.len() > MAX_TAGS {
            return Err(TagWriteRejection::TooManyTags(incoming.len()));
        }

        let mut merged = machine.tags.clone();
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
        let before = machine.retained_bytes;
        let after = before
            .saturating_sub(tags_bytes(&machine.tags))
            .saturating_add(tags_bytes(&merged));
        if after > before && !self.has_room_for(after - before) {
            return Err(TagWriteRejection::NoRoom);
        }

        let Some(machine) = self.state_machines.get_mut(arn) else {
            return Err(TagWriteRejection::NotFound);
        };
        machine.tags = merged;
        machine.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
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

    /// Remove a state machine and its executions.
    ///
    /// The executions go with it, so no execution ARN is left naming a state machine
    /// that no longer exists.
    pub fn remove_state_machine(&mut self, arn: &str) -> Option<StateMachine> {
        let machine = self.state_machines.remove(arn)?;
        self.retained_bytes = self.retained_bytes.saturating_sub(machine.retained_bytes);
        for execution_arn in self.executions_by_sm.remove(arn).unwrap_or_default() {
            if let Some(execution) = self.executions.remove(&execution_arn) {
                self.retained_bytes = self.retained_bytes.saturating_sub(execution.retained_bytes);
            }
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

    /// Stop the execution `arn` names, and report the `stopDate` it carries.
    ///
    /// The outcome the interpreter had computed is DISCARDED along with its place in
    /// the settle queue: the execution was stopped before the clock made that outcome
    /// due, so it never reached it. An execution that has already reached a terminal
    /// status is not moved — its own `stopDate` comes back, so two callers that stop
    /// the same execution see the same instant. `None` when nothing matched.
    pub fn stop_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        error: Option<String>,
        cause: Option<String>,
    ) -> Option<SystemTime> {
        let execution = self.executions.get_mut(arn)?;
        if let Some(stop_date) = execution.stop_date {
            return Some(stop_date);
        }
        let before = execution.retained_bytes;
        execution.pending = None;
        execution.status = EXECUTION_STATUS_ABORTED.to_string();
        execution.stop_date = Some(now);
        execution.error = error;
        execution.cause = cause;
        execution.push_event("ExecutionAborted", now);
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
        Some(now)
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
    pub fn redrive_execution(
        &mut self,
        arn: &str,
        now: SystemTime,
        client_token: Option<&str>,
        opening: Vec<HistoryEvent>,
        pending: PendingOutcome,
    ) -> Result<SystemTime, RedriveRejection> {
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
        let after = candidate.measure();
        if after > before && !self.has_room_for(after - before) {
            return Err(RedriveRejection::NoRoom);
        }
        candidate.retained_bytes = after;
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(after)
            .saturating_sub(before);
        // The re-run's outcome is owed to the clock, so this execution joins the
        // settle queue again; it left the queue when its first attempt settled.
        self.pending_executions.retain(|pending| pending != arn);
        self.pending_executions.push(arn.to_string());
        self.executions.insert(arn.to_string(), candidate);
        Ok(now)
    }

    /// How many executions this state machine already holds.
    pub fn execution_count(&self, state_machine_arn: &str) -> usize {
        self.executions_by_sm
            .get(state_machine_arn)
            .map_or(0, Vec::len)
    }

    /// One state machine's executions, most recent first.
    ///
    /// From `ListExecutions`' own documentation: "Results are sorted by time, with
    /// the most recent execution first. Running executions are sorted by their
    /// `startDate` [...] and other executions are sorted by their `stopDate`." The
    /// sort key is therefore an execution's `stopDate` once it has one and its
    /// `startDate` until then, descending; the ARN breaks ties so the order is total
    /// and a page token means the same thing on the next call.
    pub fn executions_newest_first(&self, state_machine_arn: &str) -> Vec<&Execution> {
        let mut executions: Vec<&Execution> = self
            .executions_by_sm
            .get(state_machine_arn)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|arn| self.executions.get(arn))
            .collect();
        executions.sort_by(|left, right| {
            let left_key = left.stop_date.unwrap_or(left.start_date);
            let right_key = right.stop_date.unwrap_or(right.start_date);
            right_key
                .cmp(&left_key)
                .then_with(|| right.start_date.cmp(&left.start_date))
                .then_with(|| left.arn.cmp(&right.arn))
        });
        executions
    }
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

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

/// The taggable resource `arn` names, or `None` when `arn` is not the ARN of one.
pub fn parse_taggable_arn(arn: &str) -> Option<TaggableResource> {
    if let Some(state_machine) = parse_state_machine_arn(arn) {
        return Some(TaggableResource::StateMachine(state_machine));
    }
    let resource = states_arn_resource(arn)?;
    let mut parts = resource.splitn(3, ':');
    if parts.next()? != ACTIVITY_RESOURCE {
        return None;
    }
    let name = parts.next().filter(|name| !name.is_empty())?;
    // An activity ARN carries no qualifier: only versions and aliases of a state
    // machine do.
    if parts.next().is_some() {
        return None;
    }
    Some(TaggableResource::Activity(name.to_string()))
}

/// What the modelled `InvalidArn` says about an ARN that is not a taggable Step
/// Functions ARN, so every operation reading [`parse_taggable_arn`] words its
/// refusal the same way.
pub fn untaggable_arn_message(resource_arn: &str) -> String {
    format!("'{resource_arn}' is not the ARN of a Step Functions state machine or activity")
}

/// What the modelled `ResourceNotFound` says about a taggable ARN the store holds no
/// resource under, for the same reason.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("could not find the resource {resource_arn}")
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

/// Why a tag write was not applied; see
/// [`ComAmazonawsSfnAwsstepfunctionsState::merge_state_machine_tags`].
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

/// The parts of a state machine ARN this service reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMachineArn {
    pub name: String,
    /// The `:<version>` or `:<alias>` suffix a qualified ARN carries, if any.
    pub qualifier: Option<String>,
}

/// The ARN of the state machine `name` in `region`.
pub fn state_machine_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{STATE_MACHINE_RESOURCE}:{name}"),
    )
}

/// The prefix every state machine ARN of `region` begins with: the ARN of the empty
/// name, which no state machine can take (`name` is modelled 1..80).
pub fn state_machine_arn_prefix(region: &str) -> String {
    state_machine_arn(region, "")
}

/// The ARN of one execution of a state machine.
pub fn execution_arn(region: &str, state_machine_name: &str, execution_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{EXECUTION_RESOURCE}:{state_machine_name}:{execution_name}"),
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

/// The resource portion of an ARN of this service, or `None` when `arn` is not one.
///
/// An ARN is six colon-delimited fields, the last of which carries the colons of the
/// resource itself; anything that is not `arn:<partition>:states:...` with a
/// non-empty partition is refused, which is what the modelled `InvalidArn` reports.
fn states_arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let scheme = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    let resource = fields.next()?;
    (scheme == "arn" && !partition.is_empty() && service == ARN_SERVICE).then_some(resource)
}

/// The state machine an ARN names, or `None` when `arn` is not a state machine ARN
/// of this service.
pub fn parse_state_machine_arn(arn: &str) -> Option<StateMachineArn> {
    let resource = states_arn_resource(arn)?;
    let mut parts = resource.splitn(3, ':');
    if parts.next()? != STATE_MACHINE_RESOURCE {
        return None;
    }
    let name = parts.next()?;
    if name.is_empty() {
        return None;
    }
    Some(StateMachineArn {
        name: name.to_string(),
        qualifier: parts.next().map(str::to_string),
    })
}

/// The `(stateMachineName, executionName)` an execution ARN names, or `None` when
/// `arn` is not an execution ARN of this service.
pub fn parse_execution_arn(arn: &str) -> Option<(String, String)> {
    let resource = states_arn_resource(arn)?;
    let mut parts = resource.splitn(3, ':');
    if parts.next()? != EXECUTION_RESOURCE {
        return None;
    }
    let state_machine = parts.next()?;
    let execution = parts.next()?;
    (!state_machine.is_empty() && !execution.is_empty())
        .then(|| (state_machine.to_string(), execution.to_string()))
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

/// Why `name` is not an acceptable state machine or execution name, or `None` when
/// it is. The length bounds (1..80) are modelled, so the centralized walk has
/// already refused anything outside them; only the character rule lives here.
pub fn invalid_name_reason(name: &str) -> Option<String> {
    for (position, character) in name.chars().enumerate() {
        let why = if character.is_whitespace() {
            "white space"
        } else if FORBIDDEN_NAME_CHARS.contains(&character) {
            "a bracket, wildcard or special character"
        } else if matches!(u32::from(character), 0x0000..=0x001F | 0x007F..=0x009F | 0xFFFE | 0xFFFF)
        {
            "a control character"
        } else {
            continue;
        };
        return Some(format!(
            "name must not contain {why}: found U+{:04X} at character {} of '{name}'",
            u32::from(character),
            position + 1
        ));
    }
    None
}

/// The members `CreateStateMachine`'s idempotency check compares, rendered as one
/// string.
///
/// From the operation's own documentation: "CreateStateMachine's idempotency check
/// is based on the state machine name, definition, type, LoggingConfiguration,
/// TracingConfiguration, and EncryptionConfiguration[.] The check is also based on
/// the publish and versionDescription parameters. If a following request has a
/// different roleArn or tags, Step Functions will ignore these differences" — so
/// `roleArn` and `tags` are deliberately absent here, and the name is the store key
/// rather than part of the fingerprint.
///
/// `Debug` renders the three configuration structures: each is a plain record of
/// `Option<String>`, `bool`, `i32` and `Vec` members, so the rendering is a total,
/// deterministic function of their contents.
pub fn configuration_fingerprint(
    definition: &str,
    sm_type: &str,
    logging_configuration: Option<&LoggingConfiguration>,
    tracing_configuration: Option<&TracingConfiguration>,
    encryption_configuration: Option<&EncryptionConfiguration>,
    publish: bool,
    version_description: Option<&str>,
) -> String {
    format!(
        "{definition}\u{1f}{sm_type}\u{1f}{logging_configuration:?}\u{1f}\
         {tracing_configuration:?}\u{1f}{encryption_configuration:?}\u{1f}\
         {publish}\u{1f}{version_description:?}"
    )
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

/// Interpret `definition` against `input` and schedule what the clock owes.
///
/// The run happens HERE, in the call that starts it: [`asl::run`] walks the states
/// and the result is parked in a [`PendingOutcome`] due at
/// `start + EXECUTION_SETTLE_DELAY + <every Wait the walk passed>`. Returns the event
/// the attempt opens its history with, and that outcome.
///
/// `opening` is that event's type — [`EVENT_EXECUTION_STARTED`] for a
/// `StartExecution`, [`EVENT_EXECUTION_REDRIVEN`] for a `RedriveExecution` — and
/// `previous_event_id` the last id the execution's history already holds, so a
/// redrive's events CONTINUE the existing numbering instead of restarting it.
pub fn plan_run(
    definition: &str,
    input: &str,
    start: SystemTime,
    opening: &str,
    previous_event_id: i64,
) -> (Vec<HistoryEvent>, PendingOutcome) {
    let run = asl::run(definition, input, start);
    let started = HistoryEvent {
        id: previous_event_id + 1,
        previous_event_id,
        event_type: opening.to_string(),
        timestamp: start,
        state_name: None,
    };
    let due_at = start + EXECUTION_SETTLE_DELAY + run.elapsed;
    let events: Vec<HistoryEvent> = run
        .steps
        .into_iter()
        .zip(started.id + 1..)
        .map(|(step, id)| HistoryEvent {
            id,
            previous_event_id: id - 1,
            event_type: step.event_type.to_string(),
            // Each state was entered and left at its own offset into the run; the
            // terminal event is stamped at the instant the outcome came due.
            timestamp: if step.terminal {
                due_at
            } else {
                start + step.after
            },
            state_name: step.state_name,
        })
        .collect();
    (
        vec![started],
        PendingOutcome {
            due_at,
            status: run.status.to_string(),
            output: run.output,
            error: run.error,
            cause: run.cause,
            events,
        },
    )
}

pub mod asl {
    //! The Amazon States Language subset this emulator EXECUTES.
    //!
    //! ASL is Step Functions' own definition language; no crates.io crate parses or
    //! runs it, so the walk below is written here, over `serde_json` — the
    //! maintained parser for the JSON the language is written in.
    //!
    //! Only states this emulator can run without reaching outside the process are
    //! executed: `Pass`, `Succeed`, `Fail` and `Wait`. Everything else is NOT run
    //! and NOT faked — neither a `Task`'s integration nor the JSONPath members
    //! (`InputPath`, `OutputPath`, `Parameters`, `ResultPath`) are interpreted.
    //! An execution that reaches one ends in the service's own failure state:
    //! `FAILED`, with the ASL-documented [`RUNTIME_ERROR`] error name and a cause
    //! that says exactly what was not run.
    //!
    //! ## The document the walk carries
    //!
    //! A state's input and output is a JSON DOCUMENT, and a document no state
    //! TRANSFORMED is carried on in its own BYTES rather than re-encoded from a
    //! parsed copy: the caller's `input` text reaches `output` byte for byte when the
    //! walk only passed it through. JSON is parsed to check that the document IS
    //! JSON, and a `Pass` state's `Result` is a value of the parsed definition, so
    //! that one state puts out that value's own encoding.

    use std::time::{Duration, SystemTime};

    use serde_json::{Map, Value};

    /// The ASL predefined error name for a failure the interpreter itself raises
    /// ("States.Runtime — an execution failed due to some exception that could not
    /// be processed").
    pub const RUNTIME_ERROR: &str = "States.Runtime";

    /// The ASL predefined error name for a payload that outgrew what a state may
    /// carry ("States.DataLimitExceeded").
    pub const DATA_LIMIT_ERROR: &str = "States.DataLimitExceeded";

    /// The most state transitions one execution makes before the interpreter stops
    /// it.
    ///
    /// Emulator step budget, NOT an AWS quota: it is what bounds a definition whose
    /// `Next` chain forms a cycle. An execution that exceeds it ends `FAILED` with
    /// [`RUNTIME_ERROR`].
    pub const MAX_STATE_TRANSITIONS: usize = 1_000;

    /// The most characters a state name may carry.
    ///
    /// From the Amazon States Language: a state's name "must be unique within the
    /// scope of the whole state machine and has a maximum length of 80 characters".
    /// A longer one is refused at create time, which is also what keeps an
    /// execution's history bounded in BYTES: every history event carries the name of
    /// the state it happened in.
    pub const MAX_STATE_NAME_CHARS: usize = 80;

    /// The most bytes a state's output may carry: the maximum the modelled
    /// `SensitiveData` shape that `input`/`output` target accepts (0..262144).
    ///
    /// A run whose value grows past it ends `FAILED` with ASL's own
    /// [`DATA_LIMIT_ERROR`], rather than storing a payload no response could carry.
    pub const MAX_PAYLOAD_BYTES: usize = 262_144;

    /// The most bytes a `Fail` state's `Error` and `Cause` may carry: the maxima of
    /// the modelled `SensitiveError` (0..256) and `SensitiveCause` (0..32768) shapes
    /// `DescribeExecution` reports them through. A longer one is refused at create
    /// time.
    pub const MAX_ERROR_BYTES: usize = 256;

    /// See [`MAX_ERROR_BYTES`].
    pub const MAX_CAUSE_BYTES: usize = 32_768;

    /// The longest a `Wait` state waits: Step Functions' published maximum `Wait`
    /// duration of one year, in seconds. A definition asking for more is out of
    /// range, and ends the execution with [`RUNTIME_ERROR`] rather than being
    /// silently shortened to this.
    pub const MAX_WAIT_SECONDS: u64 = 31_622_400;

    /// The state types this emulator executes.
    pub const EXECUTED_STATE_TYPES: &[&str] = &["Fail", "Pass", "Succeed", "Wait"];

    /// State types that end an execution, so they must NOT carry `Next`.
    const TERMINAL_STATE_TYPES: &[&str] = &["Fail", "Succeed"];

    /// State types that continue an execution, so each must carry `Next` or
    /// `"End": true`. `Choice` is absent because it branches through its own
    /// `Choices`/`Default` members instead.
    const CONTINUING_STATE_TYPES: &[&str] = &["Map", "Parallel", "Pass", "Task", "Wait"];

    /// The members each executed state type may carry. A state that carries any
    /// other member is not executed: the member changes what the state does, and
    /// running the state while ignoring it would report an outcome the definition
    /// does not describe.
    fn executable_members(state_type: &str) -> &'static [&'static str] {
        match state_type {
            "Pass" => &["Type", "Comment", "Result", "Next", "End"],
            "Wait" => &["Type", "Comment", "Seconds", "Timestamp", "Next", "End"],
            "Succeed" => &["Type", "Comment"],
            "Fail" => &["Type", "Comment", "Error", "Cause"],
            _ => &[],
        }
    }

    /// The first member of `state` this emulator does not execute, if any.
    fn unexecutable_member(state_type: &str, state: &Map<String, Value>) -> Option<String> {
        let allowed = executable_members(state_type);
        state
            .keys()
            .find(|member| !allowed.contains(&member.as_str()))
            .cloned()
    }

    /// One history event the walk produced.
    #[derive(Debug, Clone)]
    pub struct Step {
        pub event_type: &'static str,
        pub state_name: Option<String>,
        /// How far into the run this event happened.
        pub after: Duration,
        /// Whether this is the event the execution ends on.
        pub terminal: bool,
    }

    /// What one walk of a definition produced.
    #[derive(Debug, Clone)]
    pub struct Run {
        /// `SUCCEEDED` or `FAILED`.
        pub status: &'static str,
        /// The document the run ended with, in the bytes it was written with.
        pub output: Option<String>,
        pub error: Option<String>,
        pub cause: Option<String>,
        pub steps: Vec<Step>,
        /// The total of every `Wait` the walk passed.
        pub elapsed: Duration,
    }

    /// Why `definition` is not a definition this service accepts, or `Ok` when it
    /// is.
    ///
    /// These are the Amazon States Language's own structural rules, so the refusal
    /// is `CreateStateMachine`'s modelled `InvalidDefinition`: the definition is a
    /// JSON object carrying `StartAt` and a non-empty `States` map, `StartAt` names
    /// a declared state, every state is an object with a string `Type`, every `Next`
    /// names a declared state, a terminal state carries no `Next`, and a continuing
    /// state carries `Next` or `"End": true`.
    ///
    /// A definition may use states this emulator does not EXECUTE and still be
    /// valid, because it is valid ASL: what cannot be run surfaces at execution
    /// time, as a `FAILED` execution, not as a rejected create.
    pub fn validate(definition: &str) -> Result<(), String> {
        let document: Value = serde_json::from_str(definition)
            .map_err(|error| format!("the definition is not valid JSON: {error}"))?;
        let document = document
            .as_object()
            .ok_or_else(|| "the definition must be a JSON object".to_string())?;
        let start_at = document
            .get("StartAt")
            .and_then(Value::as_str)
            .ok_or_else(|| "the definition must carry a string 'StartAt' member".to_string())?;
        let states = document
            .get("States")
            .and_then(Value::as_object)
            .ok_or_else(|| "the definition must carry an object 'States' member".to_string())?;
        if states.is_empty() {
            return Err("'States' must declare at least one state".to_string());
        }
        if !states.contains_key(start_at) {
            return Err(format!(
                "'StartAt' names '{start_at}', which 'States' does not declare"
            ));
        }
        for (name, state) in states {
            // A state name is a caller-chosen JSON object key and lands in every
            // history event of that state, so its LENGTH is bounded here, not only
            // the number of events: this is what keeps a history's bytes bounded.
            if name.chars().count() > MAX_STATE_NAME_CHARS {
                return Err(format!(
                    "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                     definition declares one of {}",
                    name.chars().count()
                ));
            }
            let state = state
                .as_object()
                .ok_or_else(|| format!("state '{name}' must be a JSON object"))?;
            let state_type = state
                .get("Type")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("state '{name}' must carry a string 'Type' member"))?;
            // A `Fail` state's `Error` and `Cause` are reported verbatim by
            // `DescribeExecution`, through shapes with their own maxima, so a
            // definition carrying a longer one could never be reported.
            for (member, cap) in [("Error", MAX_ERROR_BYTES), ("Cause", MAX_CAUSE_BYTES)] {
                if let Some(value) = state.get(member).and_then(Value::as_str) {
                    if value.len() > cap {
                        return Err(format!(
                            "state '{name}' carries a '{member}' of {} bytes, more than the {cap} \
                             this service reports",
                            value.len()
                        ));
                    }
                }
            }
            match state.get("Next") {
                Some(next) => {
                    let next = next
                        .as_str()
                        .ok_or_else(|| format!("state '{name}' has a non-string 'Next' member"))?;
                    if !states.contains_key(next) {
                        return Err(format!(
                            "state '{name}' transitions to '{next}', which 'States' does not declare"
                        ));
                    }
                    if TERMINAL_STATE_TYPES.contains(&state_type) {
                        return Err(format!(
                            "state '{name}' is a '{state_type}' state, which ends an execution and must not carry 'Next'"
                        ));
                    }
                }
                None => {
                    if CONTINUING_STATE_TYPES.contains(&state_type)
                        && state.get("End").and_then(Value::as_bool) != Some(true)
                    {
                        return Err(format!(
                            "state '{name}' is a '{state_type}' state, so it must carry either 'Next' or \"End\": true"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Walk `definition` from its `StartAt` state with `input` as the first state's
    /// input, and report what happened.
    pub fn run(definition: &str, input: &str, start: SystemTime) -> Run {
        let mut steps: Vec<Step> = Vec::new();
        let mut elapsed = Duration::ZERO;

        let document: Value = match serde_json::from_str(definition) {
            Ok(document) => document,
            Err(error) => {
                return runtime_failure(
                    steps,
                    elapsed,
                    format!("the state machine definition is not valid JSON: {error}"),
                );
            }
        };
        let (Some(start_at), Some(states)) = (
            document.get("StartAt").and_then(Value::as_str),
            document.get("States").and_then(Value::as_object),
        ) else {
            return runtime_failure(
                steps,
                elapsed,
                "the state machine definition carries no 'StartAt' and 'States' pair".to_string(),
            );
        };
        // The execution input is the FIRST state's input document. It is parsed only
        // to check that it is JSON: what the walk carries on is the caller's own
        // bytes.
        if let Err(error) = serde_json::from_str::<Value>(input) {
            return runtime_failure(
                steps,
                elapsed,
                format!("the execution input is not valid JSON: {error}"),
            );
        }
        let mut current: String = input.to_string();

        let mut name = start_at.to_string();
        for _ in 0..MAX_STATE_TRANSITIONS {
            let Some(state) = states.get(&name).and_then(Value::as_object) else {
                return runtime_failure(
                    steps,
                    elapsed,
                    format!("state '{name}' is not declared by the state machine"),
                );
            };
            let state_type = state.get("Type").and_then(Value::as_str).unwrap_or_default();
            if !EXECUTED_STATE_TYPES.contains(&state_type) {
                return runtime_failure(
                    steps,
                    elapsed,
                    format!(
                        "state '{name}' is of type '{state_type}', which this emulator does not \
                         execute, so nothing was run for it"
                    ),
                );
            }
            if let Some(member) = unexecutable_member(state_type, state) {
                return runtime_failure(
                    steps,
                    elapsed,
                    format!(
                        "state '{name}' carries the member '{member}', which this emulator does \
                         not interpret, so nothing was run for it"
                    ),
                );
            }
            match state_type {
                "Pass" => {
                    steps.push(step("PassStateEntered", &name, elapsed));
                    // A `Pass` state's output is its `Result` when it declares one,
                    // and its input unchanged otherwise. A `Result` is a value of the
                    // PARSED definition, so what the state puts out is that value's
                    // own encoding; the pass-through case carries the input
                    // document's bytes on, because the state did not transform it.
                    if let Some(result) = state.get("Result") {
                        current = result.to_string();
                    }
                    steps.push(step("PassStateExited", &name, elapsed));
                }
                "Wait" => {
                    steps.push(step("WaitStateEntered", &name, elapsed));
                    match wait_duration(state, start, elapsed) {
                        Ok(duration) => elapsed += duration,
                        Err(why) => {
                            return runtime_failure(steps, elapsed, format!("state '{name}': {why}"));
                        }
                    }
                    steps.push(step("WaitStateExited", &name, elapsed));
                }
                "Succeed" => {
                    steps.push(step("SucceedStateEntered", &name, elapsed));
                    steps.push(step("SucceedStateExited", &name, elapsed));
                    return success(steps, elapsed, &current);
                }
                // A `Fail` state ends the execution with the error and cause it
                // declares, both of which ASL leaves optional.
                _ => {
                    steps.push(step("FailStateEntered", &name, elapsed));
                    return failure(
                        steps,
                        elapsed,
                        state.get("Error").and_then(Value::as_str).map(str::to_string),
                        state.get("Cause").and_then(Value::as_str).map(str::to_string),
                    );
                }
            }
            if state.get("End").and_then(Value::as_bool) == Some(true) {
                return success(steps, elapsed, &current);
            }
            match state.get("Next").and_then(Value::as_str) {
                Some(next) => name = next.to_string(),
                None => {
                    return runtime_failure(
                        steps,
                        elapsed,
                        format!("state '{name}' carries neither 'Next' nor \"End\": true"),
                    );
                }
            }
        }
        runtime_failure(
            steps,
            elapsed,
            format!(
                "the execution made more than the {MAX_STATE_TRANSITIONS} state transitions this \
                 emulator runs, so it was stopped"
            ),
        )
    }

    /// How long a `Wait` state waits.
    ///
    /// `Seconds` is the caller's own figure and keeps its value; `Timestamp` is an
    /// instant, so the wait is however much of it is still ahead of the run.
    fn wait_duration(
        state: &Map<String, Value>,
        start: SystemTime,
        elapsed: Duration,
    ) -> Result<Duration, String> {
        if let Some(seconds) = state.get("Seconds") {
            let seconds = seconds
                .as_u64()
                .ok_or_else(|| "'Seconds' must be a non-negative whole number".to_string())?;
            if seconds > MAX_WAIT_SECONDS {
                return Err(format!(
                    "'Seconds' is {seconds}, more than the {MAX_WAIT_SECONDS} seconds a 'Wait' \
                     state may wait for"
                ));
            }
            return Ok(Duration::from_secs(seconds));
        }
        if let Some(timestamp) = state.get("Timestamp") {
            let text = timestamp
                .as_str()
                .ok_or_else(|| "'Timestamp' must be an ISO 8601 string".to_string())?;
            let due = ::aws_smithy_types::DateTime::from_str(
                text,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .map_err(|error| format!("'Timestamp' is not an ISO 8601 instant: {error}"))?;
            let due_millis = due
                .to_millis()
                .map_err(|error| format!("'Timestamp' is out of range: {error}"))?;
            let reached = super::epoch_millis(start)
                .saturating_add(i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
            // A `Timestamp` already behind the run is no wait at all; one further
            // ahead than a `Wait` may wait for is out of range, like an over-maximum
            // `Seconds`, and is NOT silently shortened to the maximum.
            let remaining = u64::try_from(due_millis.saturating_sub(reached)).unwrap_or(0);
            if remaining > MAX_WAIT_SECONDS.saturating_mul(1_000) {
                return Err(format!(
                    "'Timestamp' is {text}, further ahead than the {MAX_WAIT_SECONDS} seconds a \
                     'Wait' state may wait for"
                ));
            }
            return Ok(Duration::from_millis(remaining));
        }
        Err("a 'Wait' state must carry 'Seconds' or 'Timestamp'".to_string())
    }

    fn step(event_type: &'static str, state_name: &str, after: Duration) -> Step {
        Step {
            event_type,
            state_name: Some(state_name.to_string()),
            after,
            terminal: false,
        }
    }

    fn success(mut steps: Vec<Step>, elapsed: Duration, document: &str) -> Run {
        // The document the walk carried, in the bytes it was written with: the
        // execution's reported `output` is what the caller or the definition wrote,
        // not a re-encoding of it.
        let output = document.to_string();
        // A state's output is bounded by what the service can report it through, so
        // a run whose value outgrew that ends with ASL's own data-limit error rather
        // than storing a payload no response could carry.
        if output.len() > MAX_PAYLOAD_BYTES {
            return failure(
                steps,
                elapsed,
                Some(DATA_LIMIT_ERROR.to_string()),
                Some(format!(
                    "the execution output is {} bytes, more than the {MAX_PAYLOAD_BYTES} a state's \
                     output may carry",
                    output.len()
                )),
            );
        }
        steps.push(Step {
            event_type: "ExecutionSucceeded",
            state_name: None,
            after: elapsed,
            terminal: true,
        });
        Run {
            status: super::EXECUTION_STATUS_SUCCEEDED,
            output: Some(output),
            error: None,
            cause: None,
            steps,
            elapsed,
        }
    }

    fn failure(
        mut steps: Vec<Step>,
        elapsed: Duration,
        error: Option<String>,
        cause: Option<String>,
    ) -> Run {
        steps.push(Step {
            event_type: "ExecutionFailed",
            state_name: None,
            after: elapsed,
            terminal: true,
        });
        Run {
            status: super::EXECUTION_STATUS_FAILED,
            output: None,
            error,
            cause,
            steps,
            elapsed,
        }
    }

    /// A failure the interpreter itself raised, carrying ASL's own
    /// [`RUNTIME_ERROR`] name and a cause that says what was not run.
    fn runtime_failure(steps: Vec<Step>, elapsed: Duration, cause: String) -> Run {
        failure(steps, elapsed, Some(RUNTIME_ERROR.to_string()), Some(cause))
    }
}

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
