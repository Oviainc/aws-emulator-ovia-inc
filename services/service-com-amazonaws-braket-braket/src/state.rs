// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-braket-braket service.
//!
//! Everything the Braket operations answer with is computed from the values held
//! here: the seeded device inventory (devices are published by AWS, never created
//! through the API) and the quantum-task records `CreateQuantumTask` writes.
//!
//! This module also owns the few helpers more than one operation needs — the
//! request's region, the clock-owed status transitions, request digests and the
//! opaque pagination cursor — because `ops/mod.rs` is generated and cannot carry a
//! shared module.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{Association, ExperimentalCapabilities};

// ---------------------------------------------------------------------------
// Modelled enum values this service stores and emits.
//
// Each one is a member of the matching `*_VALUES` catalog in
// `ops::types::shared_types`, which is generated from the Smithy model; the test
// `modelled_enum_values_are_the_only_ones_the_service_stores` pins that.
// ---------------------------------------------------------------------------

/// `QuantumTaskStatus` — the task is accepted and not queued yet.
pub const STATUS_CREATED: &str = "CREATED";
/// `QuantumTaskStatus` — the task is waiting for its device.
pub const STATUS_QUEUED: &str = "QUEUED";
/// `QuantumTaskStatus` — the task reached a terminal failure.
pub const STATUS_FAILED: &str = "FAILED";
/// `QuantumTaskStatus` — the task completed successfully.
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// `QuantumTaskStatus` / `CancellationStatus` — cancellation is in progress.
pub const STATUS_CANCELLING: &str = "CANCELLING";
/// `QuantumTaskStatus` / `CancellationStatus` — the task was cancelled.
pub const STATUS_CANCELLED: &str = "CANCELLED";

/// `DeviceStatus` values.
pub const DEVICE_ONLINE: &str = "ONLINE";
pub const DEVICE_OFFLINE: &str = "OFFLINE";
pub const DEVICE_RETIRED: &str = "RETIRED";

/// `DeviceType` values.
pub const DEVICE_TYPE_QPU: &str = "QPU";
pub const DEVICE_TYPE_SIMULATOR: &str = "SIMULATOR";

/// `QueueName` values.
pub const QUEUE_QUANTUM_TASKS: &str = "QUANTUM_TASKS_QUEUE";
pub const QUEUE_JOBS: &str = "JOBS_QUEUE";

/// `QueuePriority` values.
pub const PRIORITY_NORMAL: &str = "Normal";
pub const PRIORITY_PRIORITY: &str = "Priority";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `us-east-1` is the AWS default region.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Clock-owed transitions.
//
// A quantum task AWS accepts is returned in the non-terminal `CREATED` status, so
// its later statuses are owed by the clock rather than by any call. The spans are
// the emulator's own choice (AWS's depend on real device queues); they are never
// zero, so a caller acting immediately after `CreateQuantumTask` still observes
// `CREATED` and the state errors that go with it. Nothing sleeps: the due instant
// is stored on the record and materialised by the next call that reads it.
// ---------------------------------------------------------------------------

/// `CREATED` -> `QUEUED`, measured from `CreateQuantumTask`.
pub const QUEUE_SPAN: Duration = Duration::from_millis(600);
/// `QUEUED` -> terminal, measured from `CreateQuantumTask`: 600 ms after the queue
/// transition that starts it.
pub const TERMINAL_SPAN: Duration = Duration::from_millis(1_200);
/// `CANCELLING` -> `CANCELLED`, measured from `CancelQuantumTask`.
pub const CANCELLATION_SPAN: Duration = Duration::from_millis(600);

/// Why a quantum task this emulator accepted ends in `FAILED`.
///
/// The emulator has no quantum processor and runs no quantum simulator, so no
/// program is ever executed and no result is ever written to the output bucket.
/// Reporting `COMPLETED` would claim results that do not exist, so the task ends
/// in the modelled failure status with this reason instead.
pub const NOT_EXECUTED_FAILURE_REASON: &str = "This emulator does not execute quantum programs: \
no device or simulator ran this quantum task, so no result was written to the output S3 location.";

// ---------------------------------------------------------------------------
// Bounds on the caller-driven stores
// ---------------------------------------------------------------------------

/// The most quantum tasks one device may have in flight (not yet terminal) at once.
///
/// Amazon Braket publishes a per-device concurrent-quantum-task quota and answers a
/// request past it with `ServiceQuotaExceededException`; this is the emulator's own
/// figure for every device, and it is what bounds how many live records one caller
/// can hold open.
pub const MAX_TASKS_IN_FLIGHT_PER_DEVICE: usize = 100;

/// The most quantum-task records this emulator keeps at once.
///
/// AWS retains a task's metadata for a documented period; this emulator keeps state
/// in memory for the life of the process, so it bounds the store by COUNT instead: a
/// create makes room first, dropping the oldest FINISHED records (see
/// [`ComAmazonawsBraketBraketState::evict_finished_tasks_over_capacity`]), so the store
/// never holds more than this many — which also bounds the per-request sweep that
/// materialises due transitions. It is far above the in-flight quota above, so a task
/// is only ever dropped after it has finished.
pub const MAX_QUANTUM_TASKS: usize = 1_000;

/// The most `clientToken` entries this emulator remembers at once.
///
/// One per retained task: a token whose task has been dropped is of no further use,
/// because the ARN it would replay no longer resolves. With this cap and
/// [`IDEMPOTENCY_TTL`], a caller sending an endless stream of fresh tokens cannot grow
/// the index without limit (see
/// [`ComAmazonawsBraketBraketState::prune_idempotency`]).
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_QUANTUM_TASKS;

/// How long a `clientToken` is remembered for.
///
/// AWS does not document the window Braket honours an idempotency token for, so this
/// span is the emulator's own choice: long enough that a client retrying a create
/// (minutes at most) always gets the first ARN back, short enough that the index does
/// not hold tokens for work that finished a day ago.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A device in the seeded inventory. Devices are published by AWS and are not
/// created, updated or deleted through the Braket API.
#[derive(Debug, Clone)]
pub struct Device {
    pub device_arn: String,
    pub device_name: String,
    pub provider_name: String,
    /// One of [`DEVICE_TYPE_QPU`] / [`DEVICE_TYPE_SIMULATOR`].
    pub device_type: String,
    /// One of [`DEVICE_ONLINE`] / [`DEVICE_OFFLINE`] / [`DEVICE_RETIRED`].
    pub device_status: String,
    /// The device capability document, as the JSON string `deviceCapabilities`
    /// carries (`@mediaType("application/json")`).
    pub device_capabilities: String,
    /// The inclusive shot range this device's capability document publishes, as
    /// `service.shotsRange`. `CreateQuantumTask` refuses a `shots` value outside
    /// it, so the catalog a caller reads and the rule it is held to agree.
    pub shots_range: (i64, i64),
}

/// A quantum task created through `CreateQuantumTask`.
#[derive(Debug, Clone)]
pub struct QuantumTask {
    pub quantum_task_arn: String,
    /// A `QuantumTaskStatus` value.
    pub status: String,
    pub device_arn: String,
    pub device_parameters: String,
    pub action: String,
    pub shots: i64,
    pub output_s3_bucket: String,
    pub output_s3_key_prefix: String,
    pub output_s3_directory: String,
    pub created_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    /// The ARN of the hybrid job that submitted this task. Stays `None` until
    /// hybrid jobs exist: `CreateJob` is still a stub, so no `jobToken` a caller
    /// presents can name one.
    pub job_arn: Option<String>,
    pub job_token: Option<String>,
    pub associations: Vec<Association>,
    pub tags: BTreeMap<String, String>,
    /// The experimental capabilities the task was created with, if any.
    pub experimental_capabilities: Option<ExperimentalCapabilities>,
    /// When the `CREATED` -> `QUEUED` transition falls due.
    pub queued_due: SystemTime,
    /// When the terminal transition falls due.
    pub terminal_due: SystemTime,
    /// When `CANCELLING` -> `CANCELLED` falls due; set by `CancelQuantumTask`.
    pub cancelled_due: Option<SystemTime>,
}

impl QuantumTask {
    /// Whether the task is in a status it can never leave.
    pub fn is_terminal(&self) -> bool {
        self.status == STATUS_COMPLETED
            || self.status == STATUS_FAILED
            || self.status == STATUS_CANCELLED
    }

    /// Whether the task still occupies its device's queue.
    pub fn is_queued(&self) -> bool {
        self.status == STATUS_CREATED || self.status == STATUS_QUEUED
    }

    /// The queue priority a task runs at: tasks submitted by a hybrid job run on
    /// the priority queue, every other task on the normal one.
    pub fn queue_priority(&self) -> &'static str {
        if self.job_token.is_some() {
            PRIORITY_PRIORITY
        } else {
            PRIORITY_NORMAL
        }
    }
}

/// An `@idempotencyToken` this service has seen, and what it minted.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The digest of the request the token was first used with, so a token reused for
    /// a different request is refused rather than answered with the first ARN.
    pub fingerprint: String,
    /// The ARN that first use minted.
    pub quantum_task_arn: String,
    /// When the token was first seen — what [`IDEMPOTENCY_TTL`] is measured from.
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-braket-braket. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBraketBraketState {
    /// The seeded device inventory, keyed by device ARN and ordered by it.
    pub devices: BTreeMap<String, Device>,
    /// Every quantum task created in this emulator, keyed by its ARN.
    pub quantum_tasks: BTreeMap<String, QuantumTask>,
    /// `clientToken` -> what that token minted, so replaying an `@idempotencyToken`
    /// returns the first ARN instead of creating a second task. Bounded: see
    /// [`ComAmazonawsBraketBraketState::prune_idempotency`].
    pub idempotency: BTreeMap<String, IdempotencyRecord>,
}

impl Default for ComAmazonawsBraketBraketState {
    fn default() -> Self {
        Self {
            devices: seeded_devices(),
            quantum_tasks: BTreeMap::new(),
            idempotency: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsBraketBraketState {
    /// Materialise every status transition the clock has already made due.
    ///
    /// This is the one write a reporting operation may make: each transition was
    /// owed at `now` whether or not anybody called, so applying it changes no
    /// answer a caller could otherwise have seen.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        for task in self.quantum_tasks.values_mut() {
            apply_due_transitions_to(task, now);
        }
    }

    /// The tasks still queued on `device_arn`, oldest first — the queue a
    /// position is counted in.
    pub fn queued_tasks_on(&self, device_arn: &str) -> Vec<&QuantumTask> {
        let mut queued: Vec<&QuantumTask> = self
            .quantum_tasks
            .values()
            .filter(|t| t.device_arn == device_arn && t.is_queued())
            .collect();
        queued.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.quantum_task_arn.cmp(&b.quantum_task_arn))
        });
        queued
    }

    /// How many tasks on `device_arn` have not reached a terminal status — the
    /// number [`MAX_TASKS_IN_FLIGHT_PER_DEVICE`] bounds.
    ///
    /// Call [`apply_due_transitions`](Self::apply_due_transitions) first, so a task
    /// the clock has already finished is not counted as in flight.
    pub fn tasks_in_flight_on(&self, device_arn: &str) -> usize {
        self.quantum_tasks
            .values()
            .filter(|t| t.device_arn == device_arn && !t.is_terminal())
            .count()
    }

    /// Make room for one more task record: drop the oldest FINISHED ones, with the
    /// idempotency tokens that name them, until the store holds at most
    /// `MAX_QUANTUM_TASKS - 1` — so after the insert that follows, the store holds at
    /// most [`MAX_QUANTUM_TASKS`] records however many tasks are created, and the
    /// per-request sweep over it stays bounded with it.
    ///
    /// Only terminal records are dropped, and oldest first, so a task still in flight
    /// is never forgotten and the most recent history is the history that is kept.
    pub fn evict_finished_tasks_over_capacity(&mut self) {
        if self.quantum_tasks.len() < MAX_QUANTUM_TASKS {
            return;
        }
        let mut finished: Vec<(SystemTime, String)> = self
            .quantum_tasks
            .values()
            .filter(|task| task.is_terminal())
            .map(|task| (task.ended_at.unwrap_or(task.created_at), task.quantum_task_arn.clone()))
            .collect();
        finished.sort();
        for (_, arn) in finished {
            if self.quantum_tasks.len() < MAX_QUANTUM_TASKS {
                break;
            }
            self.quantum_tasks.remove(&arn);
            self.idempotency
                .retain(|_, record| record.quantum_task_arn != arn);
        }
    }

    /// Make room for one more `clientToken`: drop the entries that can no longer serve
    /// a replay, and then the oldest of what is left, until the index holds at most
    /// `MAX_IDEMPOTENCY_TOKENS - 1` — so after the insert that follows it holds at most
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// An entry is of no further use once the task it names has been evicted, or once
    /// it is older than [`IDEMPOTENCY_TTL`] — so an endless stream of fresh tokens
    /// cannot grow this index without limit even though every create stores one.
    pub fn prune_idempotency(&mut self, now: SystemTime) {
        let tasks = &self.quantum_tasks;
        self.idempotency.retain(|_, record| {
            if !tasks.contains_key(&record.quantum_task_arn) {
                return false;
            }
            match now.duration_since(record.recorded_at) {
                Ok(age) => age < IDEMPOTENCY_TTL,
                // Recorded in the future (the clock moved back): keep it, its TTL has
                // not run out.
                Err(_) => true,
            }
        });
        if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
            return;
        }
        let mut by_age: Vec<(SystemTime, String)> = self
            .idempotency
            .iter()
            .map(|(token, record)| (record.recorded_at, token.clone()))
            .collect();
        by_age.sort();
        for (_, token) in by_age {
            if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
                break;
            }
            self.idempotency.remove(&token);
        }
    }
}

/// Apply the transitions `now` has made due to one task.
fn apply_due_transitions_to(task: &mut QuantumTask, now: SystemTime) {
    if task.status == STATUS_CANCELLING {
        if let Some(due) = task.cancelled_due {
            if now >= due {
                task.status = STATUS_CANCELLED.to_string();
                task.ended_at = Some(due);
            }
        }
        return;
    }
    if !task.is_queued() {
        return;
    }
    if now >= task.terminal_due {
        // Nothing executed the program, so the task cannot complete; it ends in
        // the modelled failure status, carrying the reason why.
        task.status = STATUS_FAILED.to_string();
        task.ended_at = Some(task.terminal_due);
        task.failure_reason = Some(NOT_EXECUTED_FAILURE_REASON.to_string());
    } else if now >= task.queued_due {
        task.status = STATUS_QUEUED.to_string();
    }
}

// ---------------------------------------------------------------------------
// The seeded device inventory
// ---------------------------------------------------------------------------

/// One row of the seeded inventory.
struct DeviceSeed {
    arn: &'static str,
    name: &'static str,
    provider: &'static str,
    device_type: &'static str,
    device_status: &'static str,
    qubit_count: u32,
    shots_range: (i64, i64),
    action_types: &'static [&'static str],
}

/// The device inventory this emulator publishes.
///
/// The ARNs, names and providers follow Amazon Braket's published supported-devices
/// list (simulator ARNs carry no region or account; a QPU's ARN carries the region
/// that hosts it). A real device's status tracks hardware availability windows,
/// which the emulator does not model, so each row's status is a FIXED seed chosen so
/// every modelled `DeviceStatus` is reachable — `ONLINE` devices accept tasks, and
/// the `OFFLINE` / `RETIRED` rows are what `CreateQuantumTask` refuses with
/// `DeviceOfflineException` / `DeviceRetiredException`.
const DEVICE_SEEDS: &[DeviceSeed] = &[
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
        name: "SV1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 34,
        shots_range: (0, 100_000),
        action_types: &[
            "braket.ir.openqasm.program",
            "braket.ir.jaqcd.program",
        ],
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/dm1",
        name: "DM1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 17,
        shots_range: (0, 100_000),
        action_types: &[
            "braket.ir.openqasm.program",
            "braket.ir.jaqcd.program",
        ],
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/tn1",
        name: "TN1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 50,
        shots_range: (1, 1_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/ionq/Aria-1",
        name: "Aria 1",
        provider: "IonQ",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 25,
        shots_range: (1, 1_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/ionq/Forte-1",
        name: "Forte 1",
        provider: "IonQ",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_OFFLINE,
        qubit_count: 36,
        shots_range: (1, 1_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:eu-north-1::device/qpu/iqm/Garnet",
        name: "Garnet",
        provider: "IQM",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 20,
        shots_range: (1, 10_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-west-1::device/qpu/rigetti/Ankaa-3",
        name: "Ankaa-3",
        provider: "Rigetti",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 84,
        shots_range: (1, 10_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/quera/Aquila",
        name: "Aquila",
        provider: "QuEra",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 256,
        shots_range: (1, 1_000),
        action_types: &["braket.ir.ahs.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:eu-west-2::device/qpu/oqc/Lucy",
        name: "Lucy",
        provider: "Oxford Quantum Circuits",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 8,
        shots_range: (1, 10_000),
        action_types: &["braket.ir.openqasm.program"],
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-west-1::device/qpu/rigetti/Aspen-M-3",
        name: "Aspen-M-3",
        provider: "Rigetti",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 79,
        shots_range: (1, 10_000),
        action_types: &["braket.ir.openqasm.program"],
    },
];

/// Build the seeded inventory, keyed and ordered by device ARN.
fn seeded_devices() -> BTreeMap<String, Device> {
    DEVICE_SEEDS
        .iter()
        .map(|seed| {
            let device = Device {
                device_arn: seed.arn.to_string(),
                device_name: seed.name.to_string(),
                provider_name: seed.provider.to_string(),
                device_type: seed.device_type.to_string(),
                device_status: seed.device_status.to_string(),
                device_capabilities: capabilities_document(seed),
                shots_range: seed.shots_range,
            };
            (device.device_arn.clone(), device)
        })
        .collect()
}

/// The `deviceCapabilities` JSON document for one seeded device.
///
/// Shaped like a Braket device capability document (a schema header, the service
/// section with the shot range and execution windows, the accepted action types and
/// the paradigm's qubit count) and built with `serde_json`, so what a caller parses
/// is always well-formed JSON. The values are this inventory's own: the emulator has
/// no hardware to report calibration data for.
fn capabilities_document(seed: &DeviceSeed) -> String {
    let actions: serde_json::Map<String, serde_json::Value> = seed
        .action_types
        .iter()
        .map(|action_type| {
            (
                (*action_type).to_string(),
                serde_json::json!({
                    "actionType": action_type,
                    "version": ["1"],
                }),
            )
        })
        .collect();
    serde_json::json!({
        "braketSchemaHeader": {
            "name": "braket.device_schema.device_capabilities",
            "version": "1",
        },
        "service": {
            "braketSchemaHeader": {
                "name": "braket.device_schema.device_service_properties",
                "version": "1",
            },
            "executionWindows": [{
                "executionDay": "Everyday",
                "windowStartHour": "00:00:00",
                "windowEndHour": "23:59:59",
            }],
            "shotsRange": [seed.shots_range.0, seed.shots_range.1],
            "deviceDocumentation": {
                "summary": format!("{} device {}, emulated.", seed.provider, seed.name),
            },
        },
        "action": serde_json::Value::Object(actions),
        "paradigm": {
            "qubitCount": seed.qubit_count,
            "nativeGateSet": ["rx", "rz", "cz"],
        },
        "deviceParameters": {},
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBraketBraketState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBraketBraketState>)
}

/// The region every ARN this request mints carries: the one in the request's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SystemTime` as the modelled `Timestamp` the wire carries.
///
/// The conversion is deterministic — one instant always renders to the same
/// `Timestamp` — so a value read twice is the same value. Its resolution is that of
/// an `f64` of seconds since the epoch, which is sub-microsecond at present dates and
/// finer than the `date-time` form the protocol writes.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A modelled `Timestamp` as whole nanoseconds since the epoch, so two `Timestamp`s
/// are compared on one integer scale rather than field by field.
pub fn date_time_as_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// A stable hex digest of `parts`, used to tell one request from another (an
/// idempotency replay from a reuse of the same token, a pagination token from one
/// issued for a different filter set).
///
/// Each part is length-prefixed, so no two different field lists can render to the
/// same input.
pub fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

/// Mint the identifier of a new quantum task: a version-4 UUID, as the resource part
/// of a Braket quantum-task ARN carries.
///
/// Built here rather than with the `uuid` crate for one reason: adding a dependency
/// adds this crate's `Cargo.lock` entry to the change, and that one line is what the
/// serialized merge into the shared mainline kept refusing to squash. The bytes come
/// from a SHA-256 over values unique to this call — a per-process counter and the wall
/// clock — so no two tasks share an id and the id is not derived from the request; the
/// version and variant nibbles are then set, so the result is a well-formed v4 UUID
/// (RFC 9562 section 5.4: 122 random bits, version 0b0100, variant 0b10).
pub fn new_task_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(b"braket-quantum-task");
    hasher.update(seq.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&bytes[0..4]),
        hex::encode(&bytes[4..6]),
        hex::encode(&bytes[6..8]),
        hex::encode(&bytes[8..10]),
        hex::encode(&bytes[10..16]),
    )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The largest page either search operation returns, and the page size it uses when
/// the caller names none. It is the `maxResults` maximum the model declares
/// (`@range(min: 1, max: 100)`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The most filters one search request may carry.
///
/// The model's own constraint on both filter lists
/// (`SearchDevicesRequest$filters` / `SearchQuantumTasksRequest$filters`,
/// `@length(min: 0, max: 10)`), so the boundary validator refuses a request past it
/// before a handler runs. Each handler re-checks it so the work it does is bounded by
/// the code a reader can see, not only by the schema: a search performs at most
/// `MAX_FILTERS_PER_REQUEST * MAX_VALUES_PER_FILTER * MAX_QUANTUM_TASKS` comparisons.
pub const MAX_FILTERS_PER_REQUEST: usize = 10;

/// The most values one filter may carry: the model's own constraint on both filters'
/// `values` member (`String256List`, `@length(min: 1, max: 10)`).
pub const MAX_VALUES_PER_FILTER: usize = 10;

/// Where a page of results starts, and which filter set it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// How many matches precede this page.
    pub offset: usize,
    /// The digest of the filters the token was issued for, so a token replayed
    /// against a different filter set is refused instead of silently skipping.
    pub fingerprint: String,
}

impl PageCursor {
    /// Render the cursor as the opaque `nextToken` a caller echoes back. The
    /// encoding carries the cursor itself, so issuing a token writes no state —
    /// which is what lets the `@readonly` search operations hand one out.
    pub fn encode(&self) -> String {
        hex::encode(format!("v1:{}:{}", self.offset, self.fingerprint))
    }

    /// Read back a token this service issued. `None` for anything else.
    pub fn decode(token: &str) -> Option<Self> {
        let decoded = hex::decode(token).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let mut fields = text.split(':');
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some("v1"), Some(offset), Some(fingerprint), None) => Some(Self {
                offset: offset.parse().ok()?,
                fingerprint: fingerprint.to_string(),
            }),
            _ => None,
        }
    }
}

/// Take the page of `items` the cursor and page size select, and the token that
/// returns the next one.
///
/// `None` for the token on the last page: the paginated contract ends a traversal
/// by omitting `nextToken`.
pub fn paginate<T: Clone>(
    items: &[T],
    offset: usize,
    page_size: usize,
    fingerprint: &str,
) -> (Vec<T>, Option<String>) {
    let start = offset.min(items.len());
    let end = start.saturating_add(page_size).min(items.len());
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then(|| {
        PageCursor {
            offset: end,
            fingerprint: fingerprint.to_string(),
        }
        .encode()
    });
    (page, next)
}
