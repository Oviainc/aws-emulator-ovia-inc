// SPDX-License-Identifier: CC0-1.0

//! The IoT JOB-EXECUTION store, shared by the two services that write and read it.
//!
//! A job execution belongs to one (thing, job) pair and is reached by TWO emulated
//! services: the IoT control plane CREATES one per targeted thing when a job is
//! created (`iot`: `CreateJob` targets things), and the IoT Jobs data plane is where
//! the device then polls, starts and updates it (`iot-jobs-data`:
//! `GetPendingJobExecutions`, `StartNextPendingJobExecution`, `DescribeJobExecution`,
//! `UpdateJobExecution`, `StartCommandExecution`). An execution one service writes is
//! the execution the other must see, so both reach ONE store — this type, registered
//! once in the shared engine — instead of each keeping its own copy keyed by its own
//! state struct.
//!
//! Nothing here names a wire shape or a Smithy type: a service crate projects a
//! [`JobExecutionRecord`] onto the shapes ITS model declares (`JobExecution`,
//! `JobExecutionSummary`, `JobExecutionState`, …). What lives here is the record, the
//! lookups every reader needs, the quotas that bound the store and the one transition
//! the CLOCK owes an execution ([`JobExecutionRecord::time_out_if_due`]).
//!
//! Every caller-driven store is BOUNDED: see the quota constants below,
//! [`JobExecutionStore::try_insert_execution`] (which refuses past a quota and evicts
//! a thing's oldest FINISHED executions) and
//! [`JobExecutionStore::remember_client_token`] (oldest token evicted first).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// A newly enqueued execution's status.
pub const STATUS_QUEUED: &str = "QUEUED";
/// The status a device moves an execution to when it starts working on it.
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// The status an execution reaches when a timer armed on it expires before it
/// finished: `TimeoutConfig$inProgressTimeoutInMinutes` — "If the job execution
/// status is not set to another terminal state before the time expires, it will be
/// automatically set to TIMED_OUT" — and the same for the device-set step timer
/// (`UpdateJobExecution$stepTimeoutInMinutes`). It is a `JobExecutionStatus` value of
/// both IoT models, and TERMINAL (it is not one of the [`PENDING_STATUSES`]).
pub const STATUS_TIMED_OUT: &str = "TIMED_OUT";
/// The status a device reports when it REFUSES the work it was handed. It is a
/// terminal `JobExecutionStatus` value of both IoT models, and the one terminal
/// status an execution can only be moved to while it is still `QUEUED` — a device
/// rejects work it has not started ("Only queued jobs can be rejected").
pub const STATUS_REJECTED: &str = "REJECTED";

/// The two statuses an execution is PENDING in — the ones
/// `GetPendingJobExecutions` documents as "not in a terminal status" and
/// `StartNextPendingJobExecution` as the executions it may start.
pub const PENDING_STATUSES: [&str; 2] = [STATUS_QUEUED, STATUS_IN_PROGRESS];

/// How far through its lifecycle a status leaves a job execution: enqueued (0),
/// taken up by its device (1), finished (2). A device update may hold an execution
/// at its stage or carry it FORWARD; it may never carry it back.
fn lifecycle_stage(status: &str) -> u8 {
    match status {
        STATUS_QUEUED => 0,
        STATUS_IN_PROGRESS => 1,
        // Every other modelled `JobExecutionStatus` value ends the execution.
        _ => 2,
    }
}

/// Why a device update of an execution in `current` to `requested` is REFUSED, or
/// `None` while the move is legal.
///
/// A job execution runs forward through enqueued -> taken up -> finished
/// ([`lifecycle_stage`]), so an update may repeat an execution's own stage (a
/// progress report) or carry it forward (including straight from `QUEUED` to a
/// terminal status, which skips no step the service requires), and nothing may carry
/// it BACK: an execution a device already took up can neither return to the queue
/// nor be rejected, and a finished one moves no further. The caller reports the
/// reason as its own modelled state-transition error (HTTP 409); a caller that has
/// its own wording for a finished execution refuses that case before asking.
pub fn transition_refusal(current: &str, requested: &str) -> Option<String> {
    if is_terminal_status(current) {
        return Some(format!(
            "The status of job execution cannot be changed to be {requested}"
        ));
    }
    if requested == STATUS_REJECTED && current != STATUS_QUEUED {
        return Some(format!(
            "Only queued jobs can be rejected. Job was in status {current}"
        ));
    }
    if lifecycle_stage(requested) < lifecycle_stage(current) {
        return Some(format!(
            "The status of job execution cannot be changed to be {requested}"
        ));
    }
    None
}

/// Check a request's `statusDetails` against the bounds a job execution's details
/// carry — at most [`MAX_STATUS_DETAIL_PAIRS`] name/value pairs, each name within
/// [`MAX_STATUS_DETAIL_NAME_LEN`] and each value within
/// [`MAX_STATUS_DETAIL_VALUE_LEN`] characters — and answer `Err` with the reason the
/// caller reports as its modelled client error.
///
/// NOTHING about the stored record enters the check: a supplied map REPLACES the
/// stored one ([`JobExecutionRecord::try_replace_status_details`]), so the map the
/// request carries is the map the record will hold, and repeated updates cannot
/// accumulate details without bound. An absent member changes nothing and is
/// therefore always acceptable.
pub fn check_status_details(details: Option<&HashMap<String, String>>) -> Result<(), String> {
    let Some(details) = details else {
        return Ok(());
    };
    for (name, value) in details {
        let name_len = name.chars().count();
        if name_len > MAX_STATUS_DETAIL_NAME_LEN {
            return Err(format!(
                "a statusDetails name of {name_len} characters is over the {MAX_STATUS_DETAIL_NAME_LEN} character maximum"
            ));
        }
        let value_len = value.chars().count();
        if value_len > MAX_STATUS_DETAIL_VALUE_LEN {
            return Err(format!(
                "the statusDetails value for {name} is {value_len} characters, over the {MAX_STATUS_DETAIL_VALUE_LEN} character maximum"
            ));
        }
    }
    if details.len() > MAX_STATUS_DETAIL_PAIRS {
        return Err(format!(
            "the job execution would hold {} statusDetails name/value pairs, over the maximum of {MAX_STATUS_DETAIL_PAIRS}",
            details.len()
        ));
    }
    Ok(())
}

/// The `versionNumber` a freshly created execution carries: "Job execution versions
/// are incremented each time they are updated by a device"
/// (`JobExecution$versionNumber`), and a created execution has had no device update.
pub const FIRST_VERSION_NUMBER: i64 = 1;
/// The `executionNumber` of a job's FIRST execution on a device
/// ("a number that identifies a particular job execution on a particular device").
pub const FIRST_EXECUTION_NUMBER: i64 = 1;

/// True while `status` leaves the execution pending.
pub fn is_pending_status(status: &str) -> bool {
    PENDING_STATUSES.contains(&status)
}

/// True for a status that ENDS an execution: every status that is not one of the two
/// [`PENDING_STATUSES`]. The writing service validates a status against its model's
/// `JobExecutionStatus` enum before it reaches this store, so "not pending" and "one
/// of the modelled terminal values" are the same set here — and a status the model
/// adds is classified without editing a second list.
pub fn is_terminal_status(status: &str) -> bool {
    !is_pending_status(status)
}

/// Quota on how many job executions one thing may have pending at once; a request
/// past it is refused with the caller's modelled quota error rather than growing the
/// store without bound.
pub const MAX_PENDING_EXECUTIONS_PER_THING: usize = 1_000;
/// How many COMMAND executions one device may have outstanding (non-terminal) at
/// once: AWS IoT Device Management publishes a non-adjustable command-execution
/// concurrency limit of 100 concurrent command executions per target device, so a
/// start request for a device that already holds that many is REFUSED with the
/// starting operation's modelled admission error instead of being enqueued.
///
/// It is the service's own published quota and is deliberately independent of
/// [`MAX_PENDING_EXECUTIONS_PER_THING`], which is this emulator's memory bound on
/// the job executions a thing holds whatever created them: a job execution is not a
/// command execution and consumes none of the device's command-execution
/// concurrency.
pub const MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING: usize = 100;
/// How many FINISHED executions one thing keeps: the newest ones stay resolvable
/// (so a reader can still tell "finished" from "never existed"), older ones are
/// evicted as new executions are enqueued, so completed work does not accumulate for
/// the process's lifetime.
pub const MAX_TERMINAL_EXECUTIONS_PER_THING: usize = 100;
/// Quota on how many things may hold job executions at once.
pub const MAX_THINGS_WITH_EXECUTIONS: usize = 10_000;
/// How many client tokens the idempotency index remembers; the oldest is evicted
/// first, so a replay of a recent token is still answered from the index.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;
/// Ceiling on the executions the whole store holds, so the per-thing quotas cannot
/// multiply out without a bound on the total.
pub const MAX_STORED_EXECUTIONS: usize = 50_000;
/// Ceiling on the job-document bytes the whole store holds, so a store full of
/// maximum-size documents is bounded in BYTES and not only in record count.
pub const MAX_STORED_DOCUMENT_BYTES: usize = 64 * 1024 * 1024;

/// How many name/value pairs a job execution's `statusDetails` may hold: the IoT
/// Jobs quota for a job execution's status details.
pub const MAX_STATUS_DETAIL_PAIRS: usize = 10;
/// "The maximum length of the value in the name/value pair is 1,024 characters"
/// — every member that carries `DetailsMap`.
pub const MAX_STATUS_DETAIL_VALUE_LEN: usize = 1_024;
/// The model documents a maximum for the VALUE only; the same ceiling is applied to
/// the NAME so a record cannot be grown through long keys either.
pub const MAX_STATUS_DETAIL_NAME_LEN: usize = 1_024;

/// `stepTimeoutInMinutes`'s documented lower bound: "Valid values for this parameter
/// range from 1 to 10080 (1 minute to 7 days)."
pub const MIN_STEP_TIMEOUT_MINUTES: i64 = 1;
/// `stepTimeoutInMinutes`'s documented upper bound (7 days).
pub const MAX_STEP_TIMEOUT_MINUTES: i64 = 10_080;
/// `UpdateJobExecution$stepTimeoutInMinutes`: "A value of -1 is also valid and will
/// cancel the current step timer".
pub const CANCEL_STEP_TIMER_MINUTES: i64 = -1;

/// `$next` as it appears in `DescribeJobExecution`'s `jobId` label — the model's own
/// pattern for that member is `^[a-zA-Z0-9_-]+|^\$next$`.
pub const NEXT_JOB_ID: &str = "$next";

/// `ThingName`'s modelled maximum length (len 1..128) — the member every operation
/// of both planes names a device by.
pub const MAX_THING_NAME_LEN: usize = 128;

/// Whether `name` satisfies the model's `ThingName`: 1..128 characters matching
/// `^[a-zA-Z0-9:_-]+$`.
///
/// ONE rule for the whole IoT family: a name that fails it can never be addressed by
/// a `/things/{thingName}/...` request, so an execution stored under it would exist
/// and be unreachable. Smithy `length` counts CHARACTERS, so this does too.
pub fn is_modeled_thing_name(name: &str) -> bool {
    let length = name.chars().count();
    (1..=MAX_THING_NAME_LEN).contains(&length)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// Seconds in one minute, for the timer members documented in MINUTES.
const SECONDS_PER_MINUTE: i64 = 60;

/// Wall-clock seconds since the epoch: the unit every timestamp member of
/// `JobExecution` / `JobExecutionSummary` is documented in ("The time, in seconds
/// since the epoch, …").
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One job execution of one job on one thing, as the services persist it.
#[derive(Debug, Clone)]
pub struct JobExecutionRecord {
    /// The job id: the `jobId` the job was created with, or, for a command
    /// execution, the minted `executionId`.
    pub job_id: String,
    /// The thing the execution runs on.
    pub thing_name: String,
    /// One of the model's `JobExecutionStatus` values.
    pub status: String,
    /// Device-reported name/value pairs. Ordered, so a response is deterministic.
    pub status_details: BTreeMap<String, String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub last_updated_at: i64,
    /// Starts at [`FIRST_VERSION_NUMBER`] and is incremented on every device update,
    /// as `JobExecution$versionNumber` documents.
    pub version_number: i64,
    pub execution_number: i64,
    /// The step timer's deadline, in epoch seconds, when one is armed.
    pub step_timeout_expires_at: Option<i64>,
    /// The in-progress / command-execution timer's deadline, in epoch seconds, when
    /// one is armed.
    pub execution_timeout_expires_at: Option<i64>,
    /// The job's `timeoutConfig.inProgressTimeoutInMinutes`, when the job that
    /// created this execution set one. It is kept rather than applied at once
    /// because the member documents "The timer is started when the job execution
    /// status is set to IN_PROGRESS", which is a later call.
    pub in_progress_timeout_minutes: Option<i64>,
    /// The job document the device runs, when this emulator holds it. `None` for a
    /// job whose document lives outside the emulator (`CreateJob$documentSource`
    /// names an S3 object), which this emulator does not fetch and therefore does
    /// not report.
    pub job_document: Option<String>,
    /// The ARN of the COMMAND this execution runs, for an execution a command start
    /// created (`iot-jobs-data`: `StartCommandExecution`); `None` for one a JOB
    /// created (`iot`: `CreateJob`).
    ///
    /// It is what makes an execution countable against the target device's
    /// published command-execution concurrency limit
    /// ([`MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING`], counted by
    /// [`JobExecutionStore::concurrent_command_executions`]), which a job execution
    /// does not consume.
    pub command_arn: Option<String>,
}

impl JobExecutionRecord {
    /// A newly created execution of `job_id` on `thing_name`: `QUEUED`, never
    /// started, at [`FIRST_VERSION_NUMBER`] and [`FIRST_EXECUTION_NUMBER`]. A caller
    /// that knows better overwrites the public fields (`execution_number` for a
    /// re-targeted job, the timers for a job that configures one).
    pub fn queued(
        job_id: String,
        thing_name: String,
        job_document: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            job_id,
            thing_name,
            status: STATUS_QUEUED.to_string(),
            status_details: BTreeMap::new(),
            queued_at: now,
            started_at: None,
            last_updated_at: now,
            version_number: FIRST_VERSION_NUMBER,
            execution_number: FIRST_EXECUTION_NUMBER,
            step_timeout_expires_at: None,
            execution_timeout_expires_at: None,
            in_progress_timeout_minutes: None,
            job_document,
            command_arn: None,
        }
    }

    /// True when this execution runs a COMMAND, i.e. it was created by a command
    /// start and therefore occupies one of its device's command-execution
    /// concurrency slots while it is pending.
    pub fn is_command_execution(&self) -> bool {
        self.command_arn.is_some()
    }

    /// True when the execution has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        is_terminal_status(&self.status)
    }

    /// True while the execution is pending: `QUEUED` or `IN_PROGRESS`, the two
    /// statuses `StartNextPendingJobExecution` documents as pending.
    pub fn is_pending(&self) -> bool {
        !self.is_terminal()
    }

    /// Seconds left before the soonest armed timer would move this execution to
    /// `TIMED_OUT`, floored at zero; `None` when no timer is armed or the execution
    /// is already terminal.
    pub fn approximate_seconds_before_timed_out(&self, now: i64) -> Option<i64> {
        let deadline = self.timeout_deadline()?;
        Some(deadline.saturating_sub(now).max(0))
    }

    /// The instant, in epoch seconds, at which this execution becomes `TIMED_OUT`:
    /// the SOONEST armed timer's deadline, either the step timer the device set or
    /// the job's in-progress timer. `None` when no timer is armed, or when the
    /// execution has already reached a status it never leaves (a finished execution
    /// has no deadline left to reach).
    pub fn timeout_deadline(&self) -> Option<i64> {
        if self.is_terminal() {
            return None;
        }
        match (self.step_timeout_expires_at, self.execution_timeout_expires_at) {
            (Some(step), Some(exec)) => Some(step.min(exec)),
            (Some(step), None) => Some(step),
            (None, Some(exec)) => Some(exec),
            (None, None) => None,
        }
    }

    /// Materialise the `TIMED_OUT` transition the CLOCK has already made due, and
    /// answer whether this call is what moved the execution.
    ///
    /// A timer AWS arms moves the execution by itself — "If the job execution status
    /// is not set to another terminal state before the time expires, it will be
    /// automatically set to TIMED_OUT" — so the move was owed at
    /// [`Self::timeout_deadline`] whether or not anybody called; recording it when a
    /// handler next touches the record is what makes a later reader see the status the
    /// deadline already decided. The transition is stamped at the DEADLINE and not at
    /// `now`, so the record says when the execution timed out rather than when it was
    /// next read, and `versionNumber` is left alone: "Job execution versions are
    /// incremented each time they are updated by a device"
    /// (`JobExecution$versionNumber`), and the service's own timer is not a device
    /// update.
    pub fn time_out_if_due(&mut self, now: i64) -> bool {
        match self.timeout_deadline() {
            Some(deadline) if deadline <= now => {
                self.status = STATUS_TIMED_OUT.to_string();
                self.last_updated_at = deadline;
                self.step_timeout_expires_at = None;
                self.execution_timeout_expires_at = None;
                true
            }
            _ => false,
        }
    }

    /// The `statusDetails` in the `HashMap` form every modelled `DetailsMap` member
    /// carries.
    pub fn status_details_map(&self) -> HashMap<String, String> {
        self.status_details
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Apply a request's `statusDetails`: a map the request CARRIES replaces the
    /// stored one wholesale (so a name the device no longer sends is dropped), while
    /// an ABSENT member leaves the stored map exactly as it was ("If not specified,
    /// the statusDetails are unchanged").
    ///
    /// The map is checked against the record's bounds first
    /// ([`check_status_details`]) and `Err` names the reason for the caller to report
    /// as its modelled client error; nothing is written on a refusal, so a map the
    /// record cannot hold leaves the execution untouched.
    pub fn try_replace_status_details(
        &mut self,
        details: Option<&HashMap<String, String>>,
    ) -> Result<(), String> {
        check_status_details(details)?;
        if let Some(details) = details {
            self.status_details = details
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
        }
        Ok(())
    }

    /// Why a device update of this execution to `requested` is refused, or `None`
    /// while the move is legal — [`transition_refusal`] against the stored status.
    pub fn transition_refusal(&self, requested: &str) -> Option<String> {
        transition_refusal(&self.status, requested)
    }

    /// Apply a request's `stepTimeoutInMinutes`: absent leaves the current step timer
    /// alone, [`CANCEL_STEP_TIMER_MINUTES`] cancels it, and any other value arms it
    /// from `now`.
    pub fn arm_step_timer(&mut self, minutes: Option<i64>, now: i64) {
        match minutes {
            None => {}
            Some(CANCEL_STEP_TIMER_MINUTES) => self.step_timeout_expires_at = None,
            Some(minutes) => {
                self.step_timeout_expires_at =
                    Some(now.saturating_add(minutes.saturating_mul(SECONDS_PER_MINUTE)));
            }
        }
    }

    /// Move the execution to `IN_PROGRESS` at `now`, recording the start and arming
    /// the job's in-progress timer on the FIRST start.
    ///
    /// `TimeoutConfig$inProgressTimeoutInMinutes`: "The timer is started when the job
    /// execution status is set to IN_PROGRESS" — so an execution resumed while
    /// already in progress keeps both its original `startedAt` and the deadline that
    /// first start armed.
    pub fn start(&mut self, now: i64) {
        self.status = STATUS_IN_PROGRESS.to_string();
        if self.started_at.is_none() {
            self.started_at = Some(now);
            if let Some(minutes) = self.in_progress_timeout_minutes {
                self.execution_timeout_expires_at =
                    Some(now.saturating_add(minutes.saturating_mul(SECONDS_PER_MINUTE)));
            }
        }
    }
}

/// What one `clientToken` of `StartCommandExecution` committed, so a replay of the
/// same token answers with the same execution and a replay with different parameters
/// can be refused as a conflict.
#[derive(Debug, Clone)]
pub struct StartedCommand {
    pub execution_id: String,
    /// Digest of the request members the token covers.
    pub request_fingerprint: String,
}

/// The job executions of every thing, with the client-token index the command-
/// execution surface needs for idempotency.
///
/// Every caller-driven store is BOUNDED: `things` holds at most
/// [`MAX_THINGS_WITH_EXECUTIONS`] things, each with at most
/// [`MAX_PENDING_EXECUTIONS_PER_THING`] pending and
/// [`MAX_TERMINAL_EXECUTIONS_PER_THING`] finished executions (oldest finished evicted
/// first), with store-wide ceilings of [`MAX_STORED_EXECUTIONS`] records and
/// [`MAX_STORED_DOCUMENT_BYTES`] job-document bytes so the per-thing quotas cannot
/// multiply out; each record's `statusDetails` is bounded by
/// [`MAX_STATUS_DETAIL_PAIRS`] / [`MAX_STATUS_DETAIL_NAME_LEN`] /
/// [`MAX_STATUS_DETAIL_VALUE_LEN`]; and the client-token index holds at most
/// [`MAX_REMEMBERED_CLIENT_TOKENS`] entries (oldest evicted first).
///
/// Separately from those memory bounds, the store reports the SERVICE's published
/// per-device command-execution concurrency limit
/// ([`Self::try_admit_command_execution`]), which a command start enforces before it
/// mints anything.
#[derive(Debug, Default)]
pub struct JobExecutionStore {
    /// thing name -> that thing's job executions, in the order they were enqueued.
    pub things: BTreeMap<String, Vec<JobExecutionRecord>>,
    /// `StartCommandExecution` client token -> what that token committed.
    pub idempotency: BTreeMap<String, StartedCommand>,
    /// The remembered client tokens in insertion order, so the index evicts the
    /// oldest token first once it is full.
    pub idempotency_order: VecDeque<String>,
    /// How many executions `things` holds in total, for the store-wide ceiling.
    pub stored_executions: usize,
    /// How many job-document bytes `things` holds in total, for the store-wide byte
    /// ceiling.
    pub stored_document_bytes: usize,
    /// Monotonic counter feeding minted execution ids.
    next_execution_seq: u64,
}

impl JobExecutionStore {
    /// The thing's executions, oldest-enqueued first; empty for a thing that has none
    /// (every operation reports an unknown thing as having no executions rather than
    /// inventing one).
    pub fn executions_for(&self, thing_name: &str) -> &[JobExecutionRecord] {
        self.things.get(thing_name).map_or(&[][..], Vec::as_slice)
    }

    /// The thing's executions with `status`, oldest-queued first.
    pub fn with_status(&self, thing_name: &str, status: &str) -> Vec<&JobExecutionRecord> {
        let mut found: Vec<&JobExecutionRecord> = self
            .executions_for(thing_name)
            .iter()
            .filter(|record| record.status == status)
            .collect();
        // Stable, so two executions enqueued in the same second keep enqueue order.
        found.sort_by_key(|record| record.queued_at);
        found
    }

    /// Index of the thing's next pending execution: an already-started
    /// (`IN_PROGRESS`) execution before any `QUEUED` one, oldest-queued first within
    /// each group, enqueue order breaking a tie.
    pub fn next_pending_index(&self, thing_name: &str) -> Option<usize> {
        self.executions_for(thing_name)
            .iter()
            .enumerate()
            .filter(|(_, record)| record.is_pending())
            .min_by_key(|(position, record)| {
                let group = u8::from(record.status != STATUS_IN_PROGRESS);
                (group, record.queued_at, *position)
            })
            .map(|(position, _)| position)
    }

    /// Index of the execution `job_id` names on `thing_name`. With `execution_number`
    /// it is that exact execution; without one it is the latest (highest
    /// `executionNumber`), which is what `DescribeJobExecution$executionNumber`
    /// documents as the default.
    pub fn execution_index(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: Option<i64>,
    ) -> Option<usize> {
        self.executions_for(thing_name)
            .iter()
            .enumerate()
            .filter(|(_, record)| record.job_id == job_id)
            .filter(|(_, record)| execution_number.is_none_or(|n| record.execution_number == n))
            .max_by_key(|(position, record)| (record.execution_number, *position))
            .map(|(position, _)| position)
    }

    /// Index of the execution a `jobId` label resolves to, resolving `$next` to the
    /// thing's next pending execution.
    pub fn resolve_index(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: Option<i64>,
    ) -> Option<usize> {
        if job_id == NEXT_JOB_ID {
            self.next_pending_index(thing_name)
        } else {
            self.execution_index(thing_name, job_id, execution_number)
        }
    }

    /// The record a `jobId` label resolves to, with `$next` resolving to the thing's
    /// next pending execution. ONE fallible lookup, so a caller has a single
    /// not-found path.
    pub fn resolve_record(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: Option<i64>,
    ) -> Option<&JobExecutionRecord> {
        let index = self.resolve_index(thing_name, job_id, execution_number)?;
        self.record(thing_name, index)
    }

    /// The record at `index` of `thing_name`'s executions.
    pub fn record(&self, thing_name: &str, index: usize) -> Option<&JobExecutionRecord> {
        self.things.get(thing_name)?.get(index)
    }

    /// The record at `index` of `thing_name`'s executions, for update.
    pub fn record_mut(
        &mut self,
        thing_name: &str,
        index: usize,
    ) -> Option<&mut JobExecutionRecord> {
        self.things.get_mut(thing_name)?.get_mut(index)
    }

    /// The `executionNumber` the next execution of `job_id` on `thing_name` gets: one
    /// past the highest the thing already holds for that job, so re-targeting a thing
    /// with the same job numbers the executions in order, and a job's first execution
    /// on a device is [`FIRST_EXECUTION_NUMBER`].
    pub fn next_execution_number(&self, thing_name: &str, job_id: &str) -> i64 {
        self.executions_for(thing_name)
            .iter()
            .filter(|record| record.job_id == job_id)
            .map(|record| record.execution_number)
            .max()
            .map_or(FIRST_EXECUTION_NUMBER, |highest| {
                highest.saturating_add(1)
            })
    }

    /// The next value of the store's monotonic sequence, for a service that mints
    /// opaque execution ids from it.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_execution_seq = self.next_execution_seq.saturating_add(1);
        self.next_execution_seq
    }

    /// Enqueue `record` for its thing, within the store's quotas: `Err` names the
    /// quota the request would exceed, for the caller to report as ITS modelled quota
    /// error. Enqueueing also evicts the thing's oldest FINISHED executions beyond
    /// [`MAX_TERMINAL_EXECUTIONS_PER_THING`], so a long-lived store does not keep
    /// every completed execution forever.
    pub fn try_insert_execution(&mut self, record: JobExecutionRecord) -> Result<(), String> {
        let thing_name = record.thing_name.clone();
        // Every quota is checked WITHOUT touching the map, so a refused request
        // leaves no entry behind and consumes no thing slot.
        let held = self.things.get(&thing_name);
        if held.is_none() && self.things.len() >= MAX_THINGS_WITH_EXECUTIONS {
            return Err(format!(
                "the maximum of {MAX_THINGS_WITH_EXECUTIONS} things with job executions is already reached"
            ));
        }
        if held.is_some_and(|executions| {
            executions.iter().filter(|h| h.is_pending()).count()
                >= MAX_PENDING_EXECUTIONS_PER_THING
        }) {
            return Err(format!(
                "the thing already has the maximum of {MAX_PENDING_EXECUTIONS_PER_THING} pending job executions"
            ));
        }

        // Evict this thing's oldest finished executions first, so the bytes and
        // records they held are released before the store-wide ceilings below.
        let (evicted_records, evicted_bytes) = match self.things.get_mut(&thing_name) {
            Some(executions) => {
                let mut excess = executions
                    .iter()
                    .filter(|held| held.is_terminal())
                    .count()
                    .saturating_sub(MAX_TERMINAL_EXECUTIONS_PER_THING);
                let mut evicted_records = 0usize;
                let mut evicted_bytes = 0usize;
                if excess > 0 {
                    // `retain` visits in enqueue order, so the OLDEST finished
                    // executions are the ones dropped.
                    executions.retain(|held| {
                        if held.is_terminal() && excess > 0 {
                            excess -= 1;
                            evicted_records += 1;
                            evicted_bytes += document_bytes(held);
                            false
                        } else {
                            true
                        }
                    });
                }
                (evicted_records, evicted_bytes)
            }
            None => (0, 0),
        };
        self.stored_executions = self.stored_executions.saturating_sub(evicted_records);
        self.stored_document_bytes = self.stored_document_bytes.saturating_sub(evicted_bytes);

        if self.stored_executions >= MAX_STORED_EXECUTIONS {
            return Err(format!(
                "the store already holds the maximum of {MAX_STORED_EXECUTIONS} job executions"
            ));
        }
        let bytes = document_bytes(&record);
        if self.stored_document_bytes.saturating_add(bytes) > MAX_STORED_DOCUMENT_BYTES {
            return Err(format!(
                "storing this job document would exceed the maximum of {MAX_STORED_DOCUMENT_BYTES} stored job-document bytes"
            ));
        }

        self.things.entry(thing_name).or_default().push(record);
        self.stored_executions = self.stored_executions.saturating_add(1);
        self.stored_document_bytes = self.stored_document_bytes.saturating_add(bytes);
        Ok(())
    }

    /// Enqueue every record of ONE job, all-or-nothing: the first refusal rolls the
    /// records already inserted back out, so a job that does not fit inside the
    /// quotas leaves no half-targeted job behind. The eviction of a thing's oldest
    /// FINISHED executions is a bound and is not rolled back — an evicted execution
    /// is gone whether or not the request that displaced it completed.
    ///
    /// `records` is consumed LAZILY, one at a time: a caller's iterator may build a
    /// record per target (each carrying a copy of the job document), and only the
    /// record being inserted is materialised, so the store's own ceilings
    /// ([`MAX_STORED_EXECUTIONS`], [`MAX_STORED_DOCUMENT_BYTES`]) bound the memory
    /// one request can claim instead of being checked after it was already claimed.
    pub fn try_insert_executions<I>(&mut self, records: I) -> Result<(), String>
    where
        I: IntoIterator<Item = JobExecutionRecord>,
    {
        let mut inserted: Vec<(String, String, i64)> = Vec::new();
        for record in records {
            let rollback_key = (
                record.thing_name.clone(),
                record.job_id.clone(),
                record.execution_number,
            );
            match self.try_insert_execution(record) {
                Ok(()) => inserted.push(rollback_key),
                Err(reason) => {
                    for (thing_name, job_id, execution_number) in inserted {
                        self.remove_execution(&thing_name, &job_id, execution_number);
                    }
                    return Err(reason);
                }
            }
        }
        Ok(())
    }

    /// Drop one execution, keeping the store's record and byte counts in step with
    /// what it holds. `true` when a record was removed.
    pub fn remove_execution(
        &mut self,
        thing_name: &str,
        job_id: &str,
        execution_number: i64,
    ) -> bool {
        let Some(executions) = self.things.get_mut(thing_name) else {
            return false;
        };
        let Some(position) = executions.iter().position(|held| {
            held.job_id == job_id && held.execution_number == execution_number
        }) else {
            return false;
        };
        let removed = executions.remove(position);
        if executions.is_empty() {
            self.things.remove(thing_name);
        }
        self.stored_executions = self.stored_executions.saturating_sub(1);
        self.stored_document_bytes = self
            .stored_document_bytes
            .saturating_sub(document_bytes(&removed));
        true
    }

    /// Drop every execution of `job_id`, on every thing, keeping the store's record
    /// and byte counts in step with what it holds. Answers how many were dropped.
    ///
    /// Deleting a job deletes "its related job executions", so the control plane
    /// needs exactly this: the executions of one job wherever they are queued.
    pub fn remove_job_executions(&mut self, job_id: &str) -> usize {
        let mut removed = 0usize;
        let mut released = 0usize;
        for executions in self.things.values_mut() {
            executions.retain(|held| {
                if held.job_id == job_id {
                    removed += 1;
                    released += document_bytes(held);
                    false
                } else {
                    true
                }
            });
        }
        self.things
            .retain(|_, executions| !executions.is_empty());
        self.stored_executions = self.stored_executions.saturating_sub(removed);
        self.stored_document_bytes = self.stored_document_bytes.saturating_sub(released);
        removed
    }

    /// How many executions of `job_id` are still pending, on any thing — a job with
    /// one of these is a job still running on some device.
    pub fn pending_executions_of_job(&self, job_id: &str) -> usize {
        self.things
            .values()
            .flatten()
            .filter(|held| held.job_id == job_id && held.is_pending())
            .count()
    }

    /// How many COMMAND executions `thing_name` currently holds in a non-terminal
    /// status — the device's command-execution concurrency, as the published
    /// per-device limit counts it ([`MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING`]).
    ///
    /// Only command executions count: an execution a JOB created is not a command
    /// execution. Only non-terminal ones count: an execution that has finished,
    /// failed, been rejected or timed out has released its slot.
    pub fn concurrent_command_executions(&self, thing_name: &str) -> usize {
        self.executions_for(thing_name)
            .iter()
            .filter(|held| held.is_command_execution() && held.is_pending())
            .count()
    }

    /// The admission decision for ONE more command execution on `thing_name`:
    /// `Ok(())` while the device is under the published per-device concurrency
    /// limit, and `Err` naming the limit it is already at, for the caller to report
    /// as ITS modelled admission error. Nothing is enqueued either way — this is the
    /// check a start makes BEFORE it mints a record.
    ///
    /// Timers that have already come due are materialised first
    /// ([`Self::time_out_due_executions`]): an execution whose timeout has passed is
    /// `TIMED_OUT`, hence terminal, hence no longer holding a slot — the move was
    /// owed at its own deadline whether or not anybody called, so the device's
    /// concurrency is read at the current instant rather than at the last call's.
    pub fn try_admit_command_execution(
        &mut self,
        thing_name: &str,
        now: i64,
    ) -> Result<(), String> {
        self.time_out_due_executions(thing_name, now);
        let outstanding = self.concurrent_command_executions(thing_name);
        if outstanding >= MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING {
            return Err(format!(
                "the device already has {outstanding} command executions that have not reached a terminal status, the maximum of {MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING} concurrent command executions per device"
            ));
        }
        Ok(())
    }

    /// Materialise the `TIMED_OUT` transition every one of `thing_name`'s executions
    /// has already come due for, and answer how many moved. See
    /// [`JobExecutionRecord::time_out_if_due`] for why a reporting handler may write
    /// this: the move was owed at each execution's own deadline, so a handler calls
    /// this before it reads, and the whole of the thing's queue is materialised at one
    /// instant rather than only the one record a caller asked about.
    pub fn time_out_due_executions(&mut self, thing_name: &str, now: i64) -> usize {
        let Some(executions) = self.things.get_mut(thing_name) else {
            return 0;
        };
        let mut moved = 0usize;
        for record in executions.iter_mut() {
            if record.time_out_if_due(now) {
                moved += 1;
            }
        }
        moved
    }

    /// Remember what a `clientToken` committed, evicting the oldest remembered token
    /// once the index is full.
    pub fn remember_client_token(
        &mut self,
        token: String,
        execution_id: String,
        request_fingerprint: String,
    ) {
        while self.idempotency_order.len() >= MAX_REMEMBERED_CLIENT_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency.remove(&oldest);
                }
                None => break,
            }
        }
        if self
            .idempotency
            .insert(
                token.clone(),
                StartedCommand {
                    execution_id,
                    request_fingerprint,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(token);
        }
    }
}

/// The job-document bytes one record holds; a record whose document is not in this
/// emulator holds none.
fn document_bytes(record: &JobExecutionRecord) -> usize {
    record.job_document.as_ref().map_or(0, String::len)
}

#[cfg(test)]
mod tests {
    //! The store's own rules: classification, ordering, numbering and the bounds.

    use super::*;

    fn enqueue(store: &mut JobExecutionStore, thing: &str, job: &str, now: i64) {
        let mut record = JobExecutionRecord::queued(
            job.to_string(),
            thing.to_string(),
            Some("{}".to_string()),
            now,
        );
        record.execution_number = store.next_execution_number(thing, job);
        store
            .try_insert_execution(record)
            .expect("the store accepts an execution below every quota");
    }

    /// RULE: a device update may hold an execution at its lifecycle stage or carry
    /// it FORWARD (queued -> taken up -> finished), never back; and `REJECTED` is
    /// reachable only from `QUEUED`.
    ///
    /// The rows span every pair of a pending status with a modelled status, plus the
    /// moves out of a finished execution.
    #[test]
    fn a_status_update_may_never_carry_an_execution_backward() {
        // (case, stored status, requested status, legal)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("queued stays queued", STATUS_QUEUED, STATUS_QUEUED, true),
            ("queued is taken up", STATUS_QUEUED, STATUS_IN_PROGRESS, true),
            ("queued succeeds directly", STATUS_QUEUED, "SUCCEEDED", true),
            ("queued fails directly", STATUS_QUEUED, "FAILED", true),
            ("queued is rejected", STATUS_QUEUED, STATUS_REJECTED, true),
            ("queued is canceled", STATUS_QUEUED, "CANCELED", true),
            ("queued is removed", STATUS_QUEUED, "REMOVED", true),
            ("queued times out", STATUS_QUEUED, STATUS_TIMED_OUT, true),
            (
                "in progress reports progress",
                STATUS_IN_PROGRESS,
                STATUS_IN_PROGRESS,
                true,
            ),
            ("in progress succeeds", STATUS_IN_PROGRESS, "SUCCEEDED", true),
            ("in progress fails", STATUS_IN_PROGRESS, "FAILED", true),
            (
                "in progress times out",
                STATUS_IN_PROGRESS,
                STATUS_TIMED_OUT,
                true,
            ),
            (
                "in progress is rewound to the queue",
                STATUS_IN_PROGRESS,
                STATUS_QUEUED,
                false,
            ),
            (
                "in progress is rejected",
                STATUS_IN_PROGRESS,
                STATUS_REJECTED,
                false,
            ),
            ("a finished execution restarts", "SUCCEEDED", STATUS_IN_PROGRESS, false),
            ("a finished execution is requeued", "FAILED", STATUS_QUEUED, false),
            ("a finished execution finishes again", "SUCCEEDED", "FAILED", false),
        ];

        for (case, current, requested, legal) in rows {
            let refusal = transition_refusal(current, requested);
            assert_eq!(
                refusal.is_none(),
                *legal,
                "case {case:?}: expected legal == {legal} for {current} -> {requested}, got {refusal:?}"
            );
        }
    }

    /// RULE: a supplied `statusDetails` REPLACES the stored map (a name no longer
    /// sent is dropped), an absent one leaves it unchanged, and a map past the
    /// bounds is refused with the record left untouched.
    #[test]
    fn supplied_status_details_replace_the_stored_map_and_an_absent_one_keeps_it() {
        let mut record =
            JobExecutionRecord::queued("job".to_string(), "thing".to_string(), None, 1_000);

        /// One row: the case, the pairs the request carries (`None` when the member
        /// is omitted) and the pairs the record then holds, in key order.
        type Row = (&'static str, Option<&'static [(&'static str, &'static str)]>, &'static [(&'static str, &'static str)]);

        let rows: &[Row] = &[
            (
                "the first write",
                Some(&[("stage", "download"), ("attempt", "1")]),
                &[("attempt", "1"), ("stage", "download")],
            ),
            (
                "a write that no longer sends every name",
                Some(&[("stage", "verify")]),
                &[("stage", "verify")],
            ),
            ("a write that omits the member", None, &[("stage", "verify")]),
            ("a write that empties the map", Some(&[]), &[]),
        ];

        for (case, written, expected) in rows {
            let details: Option<HashMap<String, String>> = written.map(|pairs| {
                pairs
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect()
            });
            record
                .try_replace_status_details(details.as_ref())
                .unwrap_or_else(|reason| {
                    panic!("case {case:?}: expected the map accepted: {reason}")
                });
            let held: Vec<(&str, &str)> = record
                .status_details
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            assert_eq!(
                held, *expected,
                "case {case:?}: expected the record to hold {expected:?}, got {held:?}"
            );
        }

        // One pair past the bound is refused, and the record keeps what it had.
        let over_bound: HashMap<String, String> = (0..=MAX_STATUS_DETAIL_PAIRS)
            .map(|index| (format!("key-{index}"), "v".to_string()))
            .collect();
        let refusal = record.try_replace_status_details(Some(&over_bound));
        assert!(
            refusal.is_err(),
            "case one pair too many: expected a refusal, got {refusal:?}"
        );
        assert!(
            record.status_details.is_empty(),
            "case one pair too many: the refused write must change nothing, got {:?}",
            record.status_details
        );
    }

    /// A created execution is pending at version 1, execution number 1, and the two
    /// pending statuses are the only ones that are not terminal.
    #[test]
    fn a_created_execution_is_queued_and_only_pending_statuses_are_non_terminal() {
        let record = JobExecutionRecord::queued(
            "job".to_string(),
            "thing".to_string(),
            None,
            1_700_000_000,
        );
        assert!(
            record.is_pending() && !record.is_terminal(),
            "case created: expected a pending execution, got status {:?}",
            record.status
        );
        assert_eq!(
            (record.version_number, record.execution_number),
            (FIRST_VERSION_NUMBER, FIRST_EXECUTION_NUMBER),
            "case created: expected the first version and execution number, got {record:?}"
        );

        // (case, status, pending)
        let rows: [(&str, &str, bool); 6] = [
            ("queued", STATUS_QUEUED, true),
            ("in progress", STATUS_IN_PROGRESS, true),
            ("succeeded", "SUCCEEDED", false),
            ("failed", "FAILED", false),
            ("timed out", "TIMED_OUT", false),
            ("rejected", "REJECTED", false),
        ];
        for (case, status, pending) in rows {
            assert_eq!(
                is_pending_status(status),
                pending,
                "case {case:?}: expected is_pending_status == {pending}, got {}",
                is_pending_status(status)
            );
            assert_eq!(
                is_terminal_status(status),
                !pending,
                "case {case:?}: expected is_terminal_status == {}, got {}",
                !pending,
                is_terminal_status(status)
            );
        }
    }

    /// A thing name is accepted exactly on the model's `ThingName` constraint
    /// (len 1..128, `^[a-zA-Z0-9:_-]+$`) — the one rule both planes parse a target
    /// ARN with, since a name outside it can never be addressed.
    #[test]
    fn only_a_modeled_thing_name_is_accepted() {
        let at_maximum = "a".repeat(MAX_THING_NAME_LEN);
        let past_maximum = "a".repeat(MAX_THING_NAME_LEN + 1);
        // (case, name, accepted) — each published bound and one past it, plus the
        // punctuation the pattern allows and some it does not.
        let rows: [(&str, &str, bool); 10] = [
            ("one character, the lower bound", "a", true),
            ("empty, one under the lower bound", "", false),
            ("at the upper bound", at_maximum.as_str(), true),
            ("one past the upper bound", past_maximum.as_str(), false),
            ("digits and the allowed punctuation", "Laser:probe_1-2", true),
            ("a slash", "laser/probe", false),
            ("a space", "laser probe", false),
            ("a dot", "laser.probe", false),
            ("a non-ascii letter", "läser", false),
            ("a plus", "laser+probe", false),
        ];
        for (case, name, accepted) in rows {
            assert_eq!(
                is_modeled_thing_name(name),
                accepted,
                "case {case:?}: expected is_modeled_thing_name == {accepted}, got {}",
                is_modeled_thing_name(name)
            );
        }
    }

    /// `executionNumber` counts a job's executions ON ONE DEVICE: the first is 1, a
    /// re-target of the same job is 2, and another job on the same thing starts at 1
    /// again.
    #[test]
    fn execution_numbers_count_per_thing_and_job() {
        let mut store = JobExecutionStore::default();
        // (case, thing, job, expected execution number)
        let rows: [(&str, &str, &str, i64); 5] = [
            ("first job on thing a", "thing-a", "job-1", 1),
            ("same job again on thing a", "thing-a", "job-1", 2),
            ("another job on thing a", "thing-a", "job-2", 1),
            ("same job on thing b", "thing-b", "job-1", 1),
            ("third time on thing a", "thing-a", "job-1", 3),
        ];
        for (case, thing, job, expected) in rows {
            let actual = store.next_execution_number(thing, job);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected executionNumber {expected}, got {actual}"
            );
            enqueue(&mut store, thing, job, 1_700_000_000);
        }
    }

    /// `$next` serves an already-started execution before any queued one, and queued
    /// ones oldest-first.
    #[test]
    fn the_next_pending_execution_is_in_progress_first_then_oldest_queued() {
        let mut store = JobExecutionStore::default();
        enqueue(&mut store, "thing", "job-old", 100);
        enqueue(&mut store, "thing", "job-new", 200);
        let first = store
            .resolve_record("thing", NEXT_JOB_ID, None)
            .expect("a thing with two queued executions has a next one");
        assert_eq!(
            first.job_id, "job-old",
            "case all queued: expected the oldest queued execution, got {:?}",
            first.job_id
        );

        let index = store
            .execution_index("thing", "job-new", None)
            .expect("the newer execution is in the store");
        store
            .record_mut("thing", index)
            .expect("the index came from this store")
            .start(300);
        let started = store
            .resolve_record("thing", NEXT_JOB_ID, None)
            .expect("a started execution is still pending");
        assert_eq!(
            started.job_id, "job-new",
            "case one started: expected the in-progress execution first, got {:?}",
            started.job_id
        );
    }

    /// The in-progress timer is armed by the FIRST start and survives a resume.
    #[test]
    fn the_in_progress_timer_is_armed_once_at_the_first_start() {
        let mut record =
            JobExecutionRecord::queued("job".to_string(), "thing".to_string(), None, 1_000);
        record.in_progress_timeout_minutes = Some(2);
        record.start(1_000);
        assert_eq!(
            record.execution_timeout_expires_at,
            Some(1_000 + 2 * SECONDS_PER_MINUTE),
            "case first start: expected the timer armed from the start, got {record:?}"
        );
        record.start(5_000);
        assert_eq!(
            (record.started_at, record.execution_timeout_expires_at),
            (Some(1_000), Some(1_000 + 2 * SECONDS_PER_MINUTE)),
            "case resumed: expected the first start's instant and deadline, got {record:?}"
        );
    }

    /// A thing keeps at most [`MAX_TERMINAL_EXECUTIONS_PER_THING`] finished
    /// executions, oldest evicted first, and the byte count follows the records.
    #[test]
    fn finished_executions_are_evicted_oldest_first() {
        let mut store = JobExecutionStore::default();
        for index in 0..=MAX_TERMINAL_EXECUTIONS_PER_THING {
            let job = format!("job-{index}");
            enqueue(&mut store, "thing", &job, 100 + index as i64);
            let at = store
                .execution_index("thing", &job, None)
                .expect("the execution just enqueued");
            store
                .record_mut("thing", at)
                .expect("the index came from this store")
                .status = "SUCCEEDED".to_string();
        }
        assert!(
            store.execution_index("thing", "job-0", None).is_some(),
            "case at the bound: the oldest finished execution is still held, got {} records",
            store.executions_for("thing").len()
        );

        enqueue(&mut store, "thing", "one-more", 9_999);
        assert!(
            store.execution_index("thing", "job-0", None).is_none(),
            "case past the bound: expected the oldest finished execution evicted, got it still held"
        );
        assert_eq!(
            store.stored_executions,
            store.executions_for("thing").len(),
            "case past the bound: the record count must follow the store, got {}",
            store.stored_executions
        );
        assert_eq!(
            store.stored_document_bytes,
            store
                .executions_for("thing")
                .iter()
                .map(document_bytes)
                .sum::<usize>(),
            "case past the bound: the byte count must follow the store, got {}",
            store.stored_document_bytes
        );
    }

    /// Enqueue one COMMAND execution for `thing`, the way a command start does:
    /// admitted against the device's concurrency limit, then stored carrying the
    /// `commandArn` that makes it a command execution.
    fn enqueue_command(store: &mut JobExecutionStore, thing: &str, id: &str, now: i64) {
        store
            .try_admit_command_execution(thing, now)
            .unwrap_or_else(|limit| panic!("admitting {id}: {limit}"));
        let mut record = JobExecutionRecord::queued(
            id.to_string(),
            thing.to_string(),
            Some("{}".to_string()),
            now,
        );
        record.command_arn = Some("arn:aws:iot:us-east-1:111122223333:command/reboot".to_string());
        store
            .try_insert_execution(record)
            .expect("the store accepts an execution below every quota");
    }

    /// A device's command-execution concurrency counts exactly its NON-TERMINAL
    /// COMMAND executions: a job execution occupies no slot, the device is admitted
    /// up to [`MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING`] and refused past it,
    /// another device is unaffected, and a slot is released both by a finished
    /// execution and by one the clock has timed out.
    #[test]
    fn a_device_admits_command_executions_up_to_the_published_concurrency_limit() {
        const NOW: i64 = 1_700_000_000;
        let mut store = JobExecutionStore::default();

        // A JOB execution is not a command execution.
        enqueue(&mut store, "thing", "job-1", NOW);
        assert_eq!(
            store.concurrent_command_executions("thing"),
            0,
            "case \"a job execution\": expected no command-execution slot consumed, got {}",
            store.concurrent_command_executions("thing")
        );

        for index in 0..MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING {
            enqueue_command(&mut store, "thing", &format!("cmd-{index}"), NOW);
        }
        assert_eq!(
            store.concurrent_command_executions("thing"),
            MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING,
            "case \"at the limit\": expected {MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING} outstanding, got {}",
            store.concurrent_command_executions("thing")
        );
        assert!(
            store.try_admit_command_execution("thing", NOW).is_err(),
            "case \"past the limit\": expected the device refused, got admitted"
        );
        assert!(
            store.try_admit_command_execution("other-thing", NOW).is_ok(),
            "case \"another device\": expected its own empty concurrency, got refused"
        );

        // A finished execution releases its slot.
        let at = store
            .execution_index("thing", "cmd-0", None)
            .expect("the first command execution");
        store
            .record_mut("thing", at)
            .expect("the index came from this store")
            .status = "SUCCEEDED".to_string();
        assert_eq!(
            store.concurrent_command_executions("thing"),
            MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING - 1,
            "case \"one finished\": expected one slot released, got {}",
            store.concurrent_command_executions("thing")
        );

        // Refill the released slot with an execution carrying its own timeout: the
        // device is at the limit again at that instant...
        enqueue_command(&mut store, "thing", "cmd-timing-out", NOW);
        let at = store
            .execution_index("thing", "cmd-timing-out", None)
            .expect("the execution just enqueued");
        store
            .record_mut("thing", at)
            .expect("the index came from this store")
            .execution_timeout_expires_at = Some(NOW + 60);
        assert!(
            store.try_admit_command_execution("thing", NOW).is_err(),
            "case \"at the limit again\": expected the device refused, got admitted"
        );
        // ...and under it once that execution's own deadline has passed, which the
        // clock owed whether or not anybody called.
        assert!(
            store.try_admit_command_execution("thing", NOW + 61).is_ok(),
            "case \"a timed-out execution\": expected its slot released, got refused"
        );
        assert_eq!(
            store
                .resolve_record("thing", "cmd-timing-out", None)
                .map(|record| record.status.clone()),
            Some(STATUS_TIMED_OUT.to_string()),
            "case \"a timed-out execution\": expected it moved to TIMED_OUT, got {:?}",
            store.resolve_record("thing", "cmd-timing-out", None)
        );
    }

    /// One row of the timer table: where each armed deadline sits relative to the
    /// instant the record is touched at.
    struct TimerRow {
        case: &'static str,
        step: Option<i64>,
        execution: Option<i64>,
        now: i64,
        timed_out: bool,
    }

    /// A pending execution times out AT its soonest armed deadline and not before,
    /// whichever timer armed it, and a timer that was re-armed past `now` survives.
    #[test]
    fn a_pending_execution_times_out_exactly_at_its_soonest_deadline() {
        let rows: [TimerRow; 8] = [
            TimerRow { case: "no timer armed", step: None, execution: None, now: 9_999, timed_out: false },
            TimerRow { case: "step timer still running", step: Some(2_000), execution: None, now: 1_999, timed_out: false },
            TimerRow { case: "step timer due this second", step: Some(2_000), execution: None, now: 2_000, timed_out: true },
            TimerRow { case: "step timer long past", step: Some(2_000), execution: None, now: 99_999, timed_out: true },
            TimerRow { case: "in-progress timer due", step: None, execution: Some(3_000), now: 3_000, timed_out: true },
            TimerRow { case: "in-progress timer running", step: None, execution: Some(3_000), now: 2_999, timed_out: false },
            TimerRow { case: "both armed, the step one is sooner", step: Some(2_000), execution: Some(3_000), now: 2_000, timed_out: true },
            TimerRow { case: "both armed, neither due yet", step: Some(2_000), execution: Some(3_000), now: 1_000, timed_out: false },
        ];
        for TimerRow {
            case,
            step,
            execution,
            now,
            timed_out: expected,
        } in rows
        {
            let mut record =
                JobExecutionRecord::queued("job".to_string(), "thing".to_string(), None, 1_000);
            record.step_timeout_expires_at = step;
            record.execution_timeout_expires_at = execution;
            let moved = record.time_out_if_due(now);
            assert_eq!(
                moved, expected,
                "case {case:?}: expected time_out_if_due == {expected}, got {moved} \
                 with {record:?}"
            );
            let soonest = match (step, execution) {
                (Some(step), Some(execution)) => Some(step.min(execution)),
                (Some(step), None) => Some(step),
                (None, Some(execution)) => Some(execution),
                (None, None) => None,
            };
            if expected {
                assert_eq!(
                    (record.status.as_str(), Some(record.last_updated_at)),
                    (STATUS_TIMED_OUT, soonest),
                    "case {case:?}: a timed-out execution is stamped at its deadline, \
                     got {record:?}"
                );
                assert!(
                    record.is_terminal()
                        && record.approximate_seconds_before_timed_out(now).is_none(),
                    "case {case:?}: a timed-out execution is terminal and reports no \
                     remaining time, got {record:?}"
                );
                assert!(
                    !record.time_out_if_due(now + 60),
                    "case {case:?}: timing out is idempotent, got a second move with \
                     {record:?}"
                );
            } else {
                assert_eq!(
                    (record.status.as_str(), record.timeout_deadline()),
                    (STATUS_QUEUED, soonest),
                    "case {case:?}: an execution whose deadline has not passed is \
                     untouched, got {record:?}"
                );
            }
        }
    }

    /// Materialising a thing's due timeouts moves EVERY execution the clock made due
    /// and leaves the rest alone, so the thing's whole queue is read at one instant.
    #[test]
    fn due_timeouts_are_materialized_for_the_whole_thing() {
        let mut store = JobExecutionStore::default();
        // (job, step deadline) — one long past due, one due exactly now, one later,
        // one with no timer at all.
        let rows: [(&str, Option<i64>); 4] = [
            ("job-past", Some(1_500)),
            ("job-now", Some(2_000)),
            ("job-later", Some(2_001)),
            ("job-untimed", None),
        ];
        for (job, deadline) in rows {
            enqueue(&mut store, "thing", job, 1_000);
            let at = store
                .execution_index("thing", job, None)
                .expect("the execution just enqueued");
            store
                .record_mut("thing", at)
                .expect("the index came from this store")
                .step_timeout_expires_at = deadline;
        }
        // A second thing's executions are not touched by reading the first.
        enqueue(&mut store, "other-thing", "job-past", 1_000);
        let at = store
            .execution_index("other-thing", "job-past", None)
            .expect("the other thing's execution");
        store
            .record_mut("other-thing", at)
            .expect("the index came from this store")
            .step_timeout_expires_at = Some(1_500);

        let moved = store.time_out_due_executions("thing", 2_000);
        assert_eq!(
            moved, 2,
            "case two due: expected two executions moved, got {moved} with {:?}",
            store.executions_for("thing")
        );
        // (job, expected status)
        let expected: [(&str, &str); 4] = [
            ("job-past", STATUS_TIMED_OUT),
            ("job-now", STATUS_TIMED_OUT),
            ("job-later", STATUS_QUEUED),
            ("job-untimed", STATUS_QUEUED),
        ];
        for (job, status) in expected {
            let record = store
                .resolve_record("thing", job, None)
                .expect("every enqueued execution is still held");
            assert_eq!(
                record.status, status,
                "case {job:?}: expected status {status}, got {:?}",
                record.status
            );
        }
        let untouched = store
            .resolve_record("other-thing", "job-past", None)
            .expect("the other thing's execution is still held");
        assert_eq!(
            untouched.status, STATUS_QUEUED,
            "case other thing: reading one thing must not move another's executions, \
             got {:?}",
            untouched.status
        );
    }
}
