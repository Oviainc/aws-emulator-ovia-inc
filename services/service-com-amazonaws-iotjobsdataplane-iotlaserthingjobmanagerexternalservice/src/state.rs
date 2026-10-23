// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice service.
//!
//! The job executions this data plane serves are NOT this crate's own store: an
//! execution is created when an IoT job targets a thing (the `iot` control plane's
//! `CreateJob`) or when a command is started here, and the device then polls, starts
//! and updates it through the five operations in `src/ops/`. Both services therefore
//! reach ONE store — [`service_common::iot_job_executions::JobExecutionStore`],
//! registered once in the shared engine under that shared type — instead of each
//! keeping its own copy keyed by its own state struct.
//!
//! What stays here is this crate's projection of a stored record onto the shapes
//! THIS model declares ([`to_job_execution`], [`to_summary`], [`to_execution_state`])
//! and the minting of a command execution's id ([`mint_execution_id`]). The store's
//! own rules — the lookups, the status classification and the quotas — live in the
//! shared module and are re-exported below, so a handler keeps writing
//! `crate::state::…`.

use crate::ops::types::{JobExecution, JobExecutionState, JobExecutionSummary};

pub use service_common::iot_job_executions::{
    check_status_details, is_pending_status, is_terminal_status, now_epoch_seconds,
    transition_refusal, JobExecutionRecord, JobExecutionStore, StartedCommand,
    CANCEL_STEP_TIMER_MINUTES, FIRST_EXECUTION_NUMBER, FIRST_VERSION_NUMBER,
    MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING, MAX_PENDING_EXECUTIONS_PER_THING,
    MAX_REMEMBERED_CLIENT_TOKENS, MAX_STATUS_DETAIL_NAME_LEN, MAX_STATUS_DETAIL_PAIRS,
    MAX_STATUS_DETAIL_VALUE_LEN, MAX_STEP_TIMEOUT_MINUTES, MAX_STORED_DOCUMENT_BYTES,
    MAX_STORED_EXECUTIONS, MAX_TERMINAL_EXECUTIONS_PER_THING, MAX_THINGS_WITH_EXECUTIONS,
    MIN_STEP_TIMEOUT_MINUTES, NEXT_JOB_ID, PENDING_STATUSES, STATUS_IN_PROGRESS, STATUS_QUEUED,
    STATUS_REJECTED,
};

/// The shared index of which IoT COMMANDS exist, which `StartCommandExecution`
/// validates its `commandArn` against before enqueuing anything.
///
/// Re-exported here for the same reason the job-execution store above is: commands
/// are owned by the IoT control plane (`iot:CreateCommand`, still a scaffold), the
/// engine keys state by TYPE, and a store two service crates share has to be
/// declared in a crate they both depend on. See `service_common::iot_commands`.
pub use ::service_common::iot_commands;

/// The command index in the shared engine: call it with a handler's
/// `&*self.engine`, exactly as [`service_state`] is called for this service's own
/// store.
///
/// The accessor lives here rather than in the shared crate so that crate needs no
/// dependency on the engine; the TYPE is shared, so this service and the IoT
/// control plane that writes it reach ONE index. Same contract as
/// [`service_state`]: answer `None` with a modelled server error, never with a
/// fresh engine, or the handler validates against a store nobody else can see.
pub fn command_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<iot_commands::IotCommandRegistry>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<iot_commands::IotCommandRegistry>)
}

/// This service's state, under the name the generated `src/lib.rs` re-exports: the
/// shared job-execution store itself.
///
/// It is an ALIAS and not a struct of its own on purpose — the engine keys state by
/// TYPE, so a second struct wrapping the same records would hand this service and the
/// IoT control plane two stores and each would see only its own writes.
pub type ComAmazonawsIotjobsdataplaneIotlaserthingjobmanagerexternalserviceState =
    JobExecutionStore;

/// The fixed message AWS answers a terminal execution's read with, as
/// `TerminalStateException` (HTTP 410) carries it: the service states the condition
/// it refused on rather than naming the request. It carries nothing from the request,
/// so it is one string and not a template.
pub const TERMINAL_STATE_MESSAGE: &str =
    "Job Execution has reached terminal state. It is neither IN_PROGRESS nor QUEUED";

/// The message of a `ResourceNotFoundException` (HTTP 404) for a `jobId`/`thingName`
/// pair that has no execution: both are `@required` `@httpLabel` members, so the
/// service builds the message from the request it refused.
pub fn not_found_message(job_id: &str, thing_name: &str) -> String {
    format!("Job {job_id} not found for {thing_name}")
}

/// The message of an `InvalidStateTransitionException` (HTTP 409) for an update of an
/// execution that has already finished: built from the request's `jobId` and
/// `thingName` and the status the execution finished in.
pub fn already_finished_message(job_id: &str, thing_name: &str, status: &str) -> String {
    format!("Job {job_id} for {thing_name} has already finished with status {status}")
}

/// The message of the `InvalidRequestException` (HTTP 400) an `expectedVersion` that
/// does not match the stored version is refused with: it names BOTH versions — the
/// one the request asked for and the one the service holds — so a device can re-read
/// and retry without a separate `DescribeJobExecution`.
pub fn version_mismatch_message(expected: i64, stored: i64) -> String {
    format!("Expected version {expected} but found version {stored}")
}

/// A stored record as the modeled `JobExecution`. `include_job_document` carries the
/// caller's `includeJobDocument` decision, which differs per operation; a job whose
/// document this emulator does not hold reports none whatever the caller asked for.
///
/// Two optional members are reported ONLY when there is something to report, which is
/// the wire form the recorded service answers these device-facing operations with:
/// `thingName` is not echoed at all (every one of these operations names the thing in
/// its own URI), and `statusDetails` is left out while the execution carries no
/// name/value pair rather than being sent as an empty map. Both members are optional
/// in the model (`smithy_query(query="shape", shape="JobExecution")`: neither is
/// `@required`).
///
/// The evidence for leaving `thingName` out is the recorded corpus
/// `simparity-wire-aws-v1`: the `$next` step
/// `iotjobsdata-job-execution-queue-poll-and-start#task#5` and the read
/// `#task#7` diverged on `$.execution.thingName` while this projection echoed it,
/// and both match with it absent. If a later recording of some operation DOES carry
/// the member, this projection takes a parameter rather than a constant.
pub fn to_job_execution(
    record: &JobExecutionRecord,
    include_job_document: bool,
    now: i64,
) -> JobExecution {
    JobExecution {
        job_id: Some(record.job_id.clone()),
        thing_name: None,
        status: Some(record.status.clone()),
        status_details: reported_status_details(record),
        queued_at: record.queued_at,
        started_at: record.started_at,
        last_updated_at: record.last_updated_at,
        approximate_seconds_before_timed_out: record.approximate_seconds_before_timed_out(now),
        version_number: record.version_number,
        execution_number: Some(record.execution_number),
        job_document: include_job_document
            .then(|| record.job_document.clone())
            .flatten(),
    }
}

/// The `statusDetails` a response carries: the record's pairs, or NOTHING while it
/// holds none — an execution with no details reports no map, not an empty one.
fn reported_status_details(
    record: &JobExecutionRecord,
) -> Option<std::collections::HashMap<String, String>> {
    (!record.status_details.is_empty()).then(|| record.status_details_map())
}

/// A stored record as the modeled `JobExecutionSummary` the pending lists carry.
pub fn to_summary(record: &JobExecutionRecord) -> JobExecutionSummary {
    JobExecutionSummary {
        job_id: Some(record.job_id.clone()),
        queued_at: record.queued_at,
        started_at: record.started_at,
        last_updated_at: record.last_updated_at,
        version_number: record.version_number,
        execution_number: Some(record.execution_number),
    }
}

/// A stored record as the modeled `JobExecutionState`. `statusDetails` follows the
/// same rule as in [`to_job_execution`]: no pairs, no map.
pub fn to_execution_state(record: &JobExecutionRecord) -> JobExecutionState {
    JobExecutionState {
        status: Some(record.status.clone()),
        status_details: reported_status_details(record),
        version_number: record.version_number,
    }
}

/// Mint an opaque execution id: a digest of the store's monotonic counter and the
/// current clock, rendered as 32 hex characters so it satisfies
/// `CommandExecutionId` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`). Derived from nothing
/// in the request, so two identical requests get different ids.
pub fn mint_execution_id(store: &mut JobExecutionStore) -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    let sequence = store.next_sequence();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hex::encode(&hasher.finalize()[..16])
}

/// The shared job-execution store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<JobExecutionStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<JobExecutionStore>)
}
