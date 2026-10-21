// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice service.
//!
//! One job execution record per (thing, job) pair, plus the client-token index
//! `StartCommandExecution` needs for idempotency. Every operation computes its
//! response from these records; none of them keeps a second copy.
//!
//! Both stores a request can grow are bounded: see the quota constants below,
//! `try_insert_execution` (which refuses past the quota and evicts a thing's
//! oldest FINISHED executions) and `remember_client_token` (oldest token
//! evicted first).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    JobExecution, JobExecutionState, JobExecutionSummary, JOB_EXECUTION_STATUS_VALUES,
};

/// A newly enqueued execution's status.
pub const STATUS_QUEUED: &str = "QUEUED";
/// The status a device moves an execution to when it starts working on it.
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// True for a modeled `JobExecutionStatus` that ENDS an execution: every value
/// of the model's enum (`JOB_EXECUTION_STATUS_VALUES`) except the two
/// `GetPendingJobExecutions` documents as "not in a terminal status". Derived
/// from the generated catalog, so a status added to the model is classified
/// without editing a second list.
pub fn is_terminal_status(status: &str) -> bool {
    JOB_EXECUTION_STATUS_VALUES.contains(&status)
        && !matches!(status, STATUS_QUEUED | STATUS_IN_PROGRESS)
}

/// Quota on how many job executions one thing may have pending at once; a
/// `StartCommandExecution` past it is refused with the modeled
/// `ServiceQuotaExceededException` rather than growing the store without bound.
pub const MAX_PENDING_EXECUTIONS_PER_THING: usize = 1_000;
/// How many FINISHED executions one thing keeps: the newest ones stay readable
/// through `DescribeJobExecution`, older ones are evicted as new executions are
/// enqueued, so completed work does not accumulate for the process's lifetime.
pub const MAX_TERMINAL_EXECUTIONS_PER_THING: usize = 100;
/// Quota on how many things may hold job executions at once.
pub const MAX_THINGS_WITH_EXECUTIONS: usize = 10_000;
/// How many client tokens the idempotency index remembers; the oldest is evicted
/// first, so a replay of a recent token is still answered from the index.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;
/// Ceiling on the executions the whole store holds, so the per-thing quotas
/// cannot multiply out without a bound on the total.
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
/// The model documents a maximum for the VALUE only; the same ceiling is applied
/// to the NAME so a record cannot be grown through long keys either.
pub const MAX_STATUS_DETAIL_NAME_LEN: usize = 1_024;

/// `stepTimeoutInMinutes`'s documented lower bound: "Valid values for this
/// parameter range from 1 to 10080 (1 minute to 7 days)."
pub const MIN_STEP_TIMEOUT_MINUTES: i64 = 1;
/// `stepTimeoutInMinutes`'s documented upper bound (7 days).
pub const MAX_STEP_TIMEOUT_MINUTES: i64 = 10_080;
/// `UpdateJobExecution$stepTimeoutInMinutes`: "A value of -1 is also valid and
/// will cancel the current step timer".
pub const CANCEL_STEP_TIMER_MINUTES: i64 = -1;

/// `$next` as it appears in `DescribeJobExecution`'s `jobId` label — the model's
/// own pattern for that member is `^[a-zA-Z0-9_-]+|^\$next$`.
pub const NEXT_JOB_ID: &str = "$next";

/// Wall-clock seconds since the epoch: the unit every timestamp member of
/// `JobExecution` / `JobExecutionSummary` is documented in ("The time, in seconds
/// since the epoch, …").
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One job execution of one job on one thing, as the service persists it.
#[derive(Debug, Clone)]
pub struct JobExecutionRecord {
    /// The job id, which for a command execution is the minted `executionId`.
    pub job_id: String,
    /// The thing the execution runs on, parsed out of the target ARN.
    pub thing_name: String,
    /// One of `JOB_EXECUTION_STATUS_VALUES`.
    pub status: String,
    /// Device-reported name/value pairs. Ordered, so a response is deterministic.
    pub status_details: BTreeMap<String, String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub last_updated_at: i64,
    /// Starts at 1 and is incremented on every device update, as
    /// `JobExecution$versionNumber` documents.
    pub version_number: i64,
    pub execution_number: i64,
    /// The step timer's deadline, in epoch seconds, when one is armed.
    pub step_timeout_expires_at: Option<i64>,
    /// The command execution timer's deadline, in epoch seconds, when one is armed.
    pub execution_timeout_expires_at: Option<i64>,
    /// The job document, built when the execution was created.
    pub job_document: String,
}

impl JobExecutionRecord {
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
    /// `TIMED_OUT`, floored at zero; `None` when no timer is armed or the
    /// execution is already terminal.
    pub fn approximate_seconds_before_timed_out(&self, now: i64) -> Option<i64> {
        if self.is_terminal() {
            return None;
        }
        let deadline = match (self.step_timeout_expires_at, self.execution_timeout_expires_at) {
            (Some(step), Some(exec)) => Some(step.min(exec)),
            (Some(step), None) => Some(step),
            (None, Some(exec)) => Some(exec),
            (None, None) => None,
        }?;
        Some(deadline.saturating_sub(now).max(0))
    }

    /// This record as the modeled `JobExecution`. `include_job_document` carries
    /// the caller's `includeJobDocument` decision, which differs per operation.
    pub fn to_job_execution(&self, include_job_document: bool, now: i64) -> JobExecution {
        JobExecution {
            job_id: Some(self.job_id.clone()),
            thing_name: Some(self.thing_name.clone()),
            status: Some(self.status.clone()),
            status_details: Some(self.status_details_map()),
            queued_at: self.queued_at,
            started_at: self.started_at,
            last_updated_at: self.last_updated_at,
            approximate_seconds_before_timed_out: self.approximate_seconds_before_timed_out(now),
            version_number: self.version_number,
            execution_number: Some(self.execution_number),
            job_document: include_job_document.then(|| self.job_document.clone()),
        }
    }

    /// This record as the modeled `JobExecutionSummary` the pending lists carry.
    pub fn to_summary(&self) -> JobExecutionSummary {
        JobExecutionSummary {
            job_id: Some(self.job_id.clone()),
            queued_at: self.queued_at,
            started_at: self.started_at,
            last_updated_at: self.last_updated_at,
            version_number: self.version_number,
            execution_number: Some(self.execution_number),
        }
    }

    /// This record as the modeled `JobExecutionState`.
    pub fn to_execution_state(&self) -> JobExecutionState {
        JobExecutionState {
            status: Some(self.status.clone()),
            status_details: Some(self.status_details_map()),
            version_number: self.version_number,
        }
    }

    /// Apply a request's `statusDetails` WITHIN the bounds a job execution's
    /// details carry: every supplied pair is set and an absent map leaves the
    /// details unchanged ("If not specified, the statusDetails are unchanged"),
    /// but a map that would push the record past
    /// [`MAX_STATUS_DETAIL_PAIRS`], [`MAX_STATUS_DETAIL_NAME_LEN`] or
    /// [`MAX_STATUS_DETAIL_VALUE_LEN`] is refused with the reason, for the caller
    /// to report as the modeled client error. Nothing is written on a refusal:
    /// the whole map is checked before the first insert, so repeated updates
    /// cannot accumulate details without bound.
    pub fn try_merge_status_details(
        &mut self,
        details: Option<&HashMap<String, String>>,
    ) -> Result<(), String> {
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
        let resulting_pairs = self
            .status_details
            .keys()
            .chain(details.keys())
            .collect::<BTreeSet<&String>>()
            .len();
        if resulting_pairs > MAX_STATUS_DETAIL_PAIRS {
            return Err(format!(
                "the job execution would hold {resulting_pairs} statusDetails name/value pairs, over the maximum of {MAX_STATUS_DETAIL_PAIRS}"
            ));
        }
        for (name, value) in details {
            self.status_details.insert(name.clone(), value.clone());
        }
        Ok(())
    }

    /// Apply a request's `stepTimeoutInMinutes`: absent leaves the current step
    /// timer alone, `-1` cancels it, and any other value arms it from `now`.
    pub fn arm_step_timer(&mut self, minutes: Option<i64>, now: i64) {
        match minutes {
            None => {}
            Some(CANCEL_STEP_TIMER_MINUTES) => self.step_timeout_expires_at = None,
            Some(minutes) => {
                self.step_timeout_expires_at =
                    Some(now.saturating_add(minutes.saturating_mul(60)));
            }
        }
    }

    fn status_details_map(&self) -> HashMap<String, String> {
        self.status_details
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// What one `clientToken` of `StartCommandExecution` committed, so a replay of
/// the same token answers with the same execution and a replay with different
/// parameters can be refused as a conflict.
#[derive(Debug, Clone)]
pub struct StartedCommand {
    pub execution_id: String,
    /// Digest of the request members the token covers.
    pub request_fingerprint: String,
}

/// Per-service state for com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice. A handler reaches it through [`service_state`].
///
/// Every caller-driven store is BOUNDED: `things` holds at most
/// [`MAX_THINGS_WITH_EXECUTIONS`] things, each with at most
/// [`MAX_PENDING_EXECUTIONS_PER_THING`] pending and
/// [`MAX_TERMINAL_EXECUTIONS_PER_THING`] finished executions (oldest finished
/// evicted first), with store-wide ceilings of [`MAX_STORED_EXECUTIONS`] records
/// and [`MAX_STORED_DOCUMENT_BYTES`] job-document bytes so the per-thing quotas
/// cannot multiply out; each record's `statusDetails` is bounded by
/// [`MAX_STATUS_DETAIL_PAIRS`] / [`MAX_STATUS_DETAIL_NAME_LEN`] /
/// [`MAX_STATUS_DETAIL_VALUE_LEN`]; and the client-token index holds at most
/// [`MAX_REMEMBERED_CLIENT_TOKENS`] entries (oldest evicted first).
#[derive(Debug, Default)]
pub struct ComAmazonawsIotjobsdataplaneIotlaserthingjobmanagerexternalserviceState {
    /// thing name -> that thing's job executions, in the order they were enqueued.
    pub things: BTreeMap<String, Vec<JobExecutionRecord>>,
    /// `StartCommandExecution` client token -> what that token committed.
    pub idempotency: BTreeMap<String, StartedCommand>,
    /// The remembered client tokens in insertion order, so the index evicts the
    /// oldest token first once it is full.
    pub idempotency_order: VecDeque<String>,
    /// How many executions `things` holds in total, for the store-wide ceiling.
    pub stored_executions: usize,
    /// How many job-document bytes `things` holds in total, for the store-wide
    /// byte ceiling.
    pub stored_document_bytes: usize,
    /// Monotonic counter feeding minted execution ids.
    pub next_execution_seq: u64,
}

impl ComAmazonawsIotjobsdataplaneIotlaserthingjobmanagerexternalserviceState {
    /// The thing's executions, oldest-enqueued first; empty for a thing that has
    /// none (every operation reports an unknown thing as having no executions
    /// rather than inventing one).
    pub fn executions_for(&self, thing_name: &str) -> &[JobExecutionRecord] {
        self.things
            .get(thing_name)
            .map_or(&[][..], Vec::as_slice)
    }

    /// The thing's pending executions with `status`, oldest-queued first.
    pub fn pending_with_status(&self, thing_name: &str, status: &str) -> Vec<&JobExecutionRecord> {
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
    /// (`IN_PROGRESS`) execution before any `QUEUED` one, oldest-queued first
    /// within each group, enqueue order breaking a tie.
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

    /// Index of the execution `job_id` names on `thing_name`. With
    /// `execution_number` it is that exact execution; without one it is the
    /// latest (highest `executionNumber`), which is what
    /// `DescribeJobExecution$executionNumber` documents as the default.
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

    /// Index of the execution a `jobId` label resolves to, resolving `$next` to
    /// the thing's next pending execution.
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

    /// The record a `jobId` label resolves to, with `$next` resolving to the
    /// thing's next pending execution. ONE fallible lookup, so a caller has a
    /// single not-found path.
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
    pub fn record_mut(&mut self, thing_name: &str, index: usize) -> Option<&mut JobExecutionRecord> {
        self.things.get_mut(thing_name)?.get_mut(index)
    }

    /// Mint an opaque execution id: a digest of this store's monotonic counter
    /// and the current clock, rendered as 32 hex characters so it satisfies
    /// `CommandExecutionId` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`). Derived from
    /// nothing in the request, so two identical requests get different ids.
    pub fn mint_execution_id(&mut self) -> String {
        use sha2::{Digest, Sha256};

        self.next_execution_seq += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(self.next_execution_seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        hex::encode(&hasher.finalize()[..16])
    }

    /// Enqueue `record` for its thing, within the store's quotas: `Err` names the
    /// quota the request would exceed, for the caller to report as the modeled
    /// `ServiceQuotaExceededException`. Enqueueing also evicts the thing's
    /// oldest FINISHED executions beyond [`MAX_TERMINAL_EXECUTIONS_PER_THING`],
    /// so a long-lived store does not keep every completed execution forever.
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
                            evicted_bytes += held.job_document.len();
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
        let document_bytes = record.job_document.len();
        if self.stored_document_bytes.saturating_add(document_bytes) > MAX_STORED_DOCUMENT_BYTES {
            return Err(format!(
                "storing this job document would exceed the maximum of {MAX_STORED_DOCUMENT_BYTES} stored job-document bytes"
            ));
        }

        self.things.entry(thing_name).or_default().push(record);
        self.stored_executions = self.stored_executions.saturating_add(1);
        self.stored_document_bytes = self.stored_document_bytes.saturating_add(document_bytes);
        Ok(())
    }

    /// Remember what a `clientToken` committed, evicting the oldest remembered
    /// token once the index is full.
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

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotjobsdataplaneIotlaserthingjobmanagerexternalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotjobsdataplaneIotlaserthingjobmanagerexternalserviceState>)
}
