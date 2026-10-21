// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-snowdevicemanagement-snowdevicemanagement service.
//!
//! Snow Device Management manages Amazon Web Services Snow Family devices that an
//! account already ordered: the API models NO device-creation operation, so the
//! device inventory is part of the emulator's own fixture and is seeded once, on
//! the first call that touches the store ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::ensure_seeded`]).
//! Everything else in here is minted by a caller: `CreateTask` persists a [`Task`]
//! plus one [`Execution`] per target, `CancelTask` moves them to `CANCELED`, and
//! the reporting operations compute their answers from these records.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Capacity, DeviceSummary, ExecutionSummary, PhysicalNetworkInterface, ResourceNotFoundException,
    SoftwareInformation,
};

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the model's `@aws.api#service` trait gives it
/// (also its sigv4 signing name).
pub const ARN_NAMESPACE: &str = "snow-device-management";

/// The largest page `ListDevices` / `ListExecutions` serve, and the page size they
/// use when the request names no `maxResults`: both model it
/// `@range(min: 1, max: 100)`.
pub const PAGE_LIMIT: i32 = 100;

/// How many tasks the store holds before `CreateTask` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on the store, not a claim about any account's
/// AWS quota (Service Quotas is not modelled here). It is set far above any
/// workflow an emulator serves, and it is what stops the state growing without
/// end.
pub const TASK_QUOTA: usize = 1_000;

/// How long an execution stays `QUEUED` before the clock owes it `IN_PROGRESS`.
///
/// AWS dispatches a command to a Snow device over however long the device takes
/// to check in; the emulator CHOOSES this span. It is deliberately not zero, so a
/// caller that reads straight after `CreateTask` still observes the `QUEUED`
/// execution. The due instant is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const EXECUTION_START_SPAN: Duration = Duration::from_millis(400);

/// How long an execution stays `IN_PROGRESS` before the clock owes it
/// `SUCCEEDED` and applies the command to the target device. Chosen for the same
/// reason as [`EXECUTION_START_SPAN`].
pub const EXECUTION_FINISH_SPAN: Duration = Duration::from_millis(400);

/// The `resourceType` segment a not-found message names for a managed device.
pub const DEVICE_RESOURCE_TYPE: &str = "managed-device";

/// The `resourceType` segment a not-found message names for a task.
pub const TASK_RESOURCE_TYPE: &str = "task";

/// The task a `CreateTask` asks the targeted devices to perform.
///
/// The modelled `Command` union carries two empty structures (`unlock`,
/// `reboot`), so a two-variant enum holds everything it says while staying
/// comparable and copyable — which is what lets a settled execution decide what
/// to apply to its device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCommand {
    /// `Command$unlock` — "Unlocks the device."
    Unlock,
    /// `Command$reboot` — "Reboots the device."
    Reboot,
}

/// One Amazon Web Services Snow Family device this account manages.
#[derive(Debug, Clone)]
pub struct Device {
    pub managed_device_id: String,
    pub managed_device_arn: String,
    /// "The type of Amazon Web Services Snow Family device."
    pub device_type: String,
    /// "The ID of the job used to order the device."
    pub associated_with_job: String,
    /// One of `UnlockState`'s values: `LOCKED`, `UNLOCKED` or `UNLOCKING`.
    pub device_state: String,
    pub physical_network_interfaces: Vec<PhysicalNetworkInterface>,
    pub device_capacities: Vec<Capacity>,
    pub software: SoftwareInformation,
    pub tags: BTreeMap<String, String>,
    pub last_reached_out_at: SystemTime,
    pub last_updated_at: SystemTime,
}

impl Device {
    fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The `DeviceSummary` projection `ListDevices` returns.
    pub fn to_summary(&self) -> DeviceSummary {
        DeviceSummary {
            managed_device_id: Some(self.managed_device_id.clone()),
            managed_device_arn: Some(self.managed_device_arn.clone()),
            associated_with_job: Some(self.associated_with_job.clone()),
            tags: Some(self.tag_map()),
        }
    }

    /// The tags `DescribeDevice` reports.
    pub fn described_tags(&self) -> HashMap<String, String> {
        self.tag_map()
    }
}

/// One task a caller created, with the record every task operation reads.
#[derive(Debug, Clone)]
pub struct Task {
    pub task_id: String,
    pub task_arn: String,
    pub targets: Vec<String>,
    pub command: TaskCommand,
    pub description: Option<String>,
    /// One of `TaskState`'s values: `IN_PROGRESS`, `COMPLETED` or `CANCELED`.
    pub state: String,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub completed_at: Option<SystemTime>,
}

impl Task {
    /// The tags `DescribeTask` reports.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// One task execution: the work a single target device does for one task.
#[derive(Debug, Clone)]
pub struct Execution {
    pub task_id: String,
    pub execution_id: String,
    pub managed_device_id: String,
    /// One of `ExecutionState`'s values.
    pub state: String,
    pub started_at: Option<SystemTime>,
    pub last_updated_at: SystemTime,
    /// The instant the clock owes this execution its next state, while it has
    /// one. `None` once the execution is terminal.
    pub due: Option<SystemTime>,
}

impl Execution {
    /// True once no further state change is owed: the execution finished, was
    /// cancelled, or failed.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "SUCCEEDED" | "FAILED" | "CANCELED" | "REJECTED" | "TIMED_OUT"
        )
    }

    /// The `ExecutionSummary` projection `ListExecutions` returns.
    pub fn to_summary(&self) -> ExecutionSummary {
        ExecutionSummary {
            task_id: Some(self.task_id.clone()),
            execution_id: Some(self.execution_id.clone()),
            managed_device_id: Some(self.managed_device_id.clone()),
            state: Some(self.state.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-snowdevicemanagement-snowdevicemanagement. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSnowdevicemanagementSnowdevicemanagementState {
    /// The managed-device inventory, keyed by `managedDeviceId`. A `BTreeMap` is
    /// what gives `ListDevices` the stable total order its pagination token
    /// indexes into.
    pub devices: BTreeMap<String, Device>,
    /// Every task, keyed by `taskId`.
    pub tasks: BTreeMap<String, Task>,
    /// Every execution, keyed by `(taskId, managedDeviceId)` — the pair
    /// `DescribeExecution` addresses one by, and an order `ListExecutions`
    /// paginates over.
    pub executions: BTreeMap<(String, String), Execution>,
    /// A `CreateTask` `clientToken` to the task that token created, so a repeated
    /// create reports the first task instead of minting a second.
    pub idempotency: HashMap<String, String>,
    /// The counter task ids are minted from.
    pub task_id_seq: u64,
    /// The counter execution ids are minted from.
    pub execution_id_seq: u64,
    /// Whether the device inventory has been seeded yet.
    seeded: bool,
}

/// How many devices the fixture inventory holds.
const SEEDED_DEVICE_COUNT: u64 = 3;

impl ComAmazonawsSnowdevicemanagementSnowdevicemanagementState {
    /// Seed the managed-device inventory, once per process.
    ///
    /// Snow Device Management has no device-creation operation — a device becomes
    /// manageable because the account ordered one — so the emulator ships an
    /// inventory instead. It is seeded on the first call that touches the store
    /// rather than at construction because the device ARNs name the region the
    /// request was signed for, and the store is built before any request exists.
    /// Ids, ARNs and job ids are derived from the device's index, so they are
    /// stable for the life of the process.
    pub fn ensure_seeded(&mut self, region: &str, now: SystemTime) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        for index in 0..SEEDED_DEVICE_COUNT {
            let managed_device_id = seeded_device_id(index);
            // Two of the three devices were ordered by the same job, so a
            // `jobId` filter has both a multi-device and a single-device answer
            // to compute.
            let associated_with_job = seeded_job_id(index / 2);
            let device = Device {
                managed_device_arn: device_arn(&managed_device_id, region),
                device_type: SEEDED_DEVICE_TYPES[usize::try_from(index)
                    .expect("the seeded device count fits in usize")
                    % SEEDED_DEVICE_TYPES.len()]
                .to_string(),
                // Every seeded device is `LOCKED`: unlocking one is what an
                // `unlock` task does, and a device AWS hands back already
                // unlocked would make that task unobservable.
                device_state: "LOCKED".to_string(),
                physical_network_interfaces: vec![PhysicalNetworkInterface {
                    physical_network_interface_id: Some(format!("s.ni-{index:016x}")),
                    // `PhysicalConnectorType`'s values, cycled so the inventory
                    // is not uniform.
                    physical_connector_type: Some(
                        SEEDED_CONNECTOR_TYPES[usize::try_from(index)
                            .expect("the seeded device count fits in usize")
                            % SEEDED_CONNECTOR_TYPES.len()]
                        .to_string(),
                    ),
                    // `IpAddressAssignment`: a seeded device holds a fixed
                    // address, so it is `STATIC`.
                    ip_address_assignment: Some("STATIC".to_string()),
                    ip_address: Some(format!("10.0.0.{}", index + 10)),
                    netmask: Some("255.255.255.0".to_string()),
                    default_gateway: Some("10.0.0.1".to_string()),
                    mac_address: Some(format!("02:00:00:00:00:{:02x}", index + 1)),
                }],
                device_capacities: vec![
                    Capacity {
                        name: Some("HDD Storage".to_string()),
                        unit: Some("Byte".to_string()),
                        total: Some(8_000_000_000_000),
                        used: Some(0),
                        available: Some(8_000_000_000_000),
                    },
                    Capacity {
                        name: Some("Memory".to_string()),
                        unit: Some("Byte".to_string()),
                        total: Some(103_079_215_104),
                        used: Some(0),
                        available: Some(103_079_215_104),
                    },
                ],
                software: SoftwareInformation {
                    installed_version: Some("1.0.0".to_string()),
                    // Nothing is being installed on a seeded device, so there is
                    // no version in flight and no install in progress.
                    installing_version: None,
                    install_state: Some("NA".to_string()),
                },
                tags: BTreeMap::new(),
                last_reached_out_at: now,
                last_updated_at: now,
                associated_with_job,
                managed_device_id: managed_device_id.clone(),
            };
            self.devices.insert(managed_device_id, device);
        }
    }

    /// Mint an unused task id. `TaskId` is bounded 1..64 and otherwise
    /// unconstrained; `st-` plus 16 hex digits is the shape AWS task ids carry.
    pub fn mint_task_id(&mut self) -> String {
        loop {
            self.task_id_seq = self.task_id_seq.wrapping_add(1);
            let id = format!("st-{:016x}", self.task_id_seq);
            if !self.tasks.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an execution id. `ExecutionId` is bounded 1..64 and otherwise
    /// unconstrained, so this is an opaque 32-hex-digit value.
    pub fn mint_execution_id(&mut self) -> String {
        self.execution_id_seq = self.execution_id_seq.wrapping_add(1);
        format!("{:032x}", self.execution_id_seq)
    }

    /// Materialise every state change the clock already owes.
    ///
    /// An execution the span has elapsed on moves `QUEUED` -> `IN_PROGRESS` ->
    /// `SUCCEEDED`; the second of those applies the task's command to the target
    /// device. A task whose every execution is terminal becomes `COMPLETED`.
    /// Nothing here depends on who called: the same changes were owed at this
    /// instant even if nobody had.
    pub fn settle(&mut self, now: SystemTime) {
        let tasks = &self.tasks;
        // What the finished executions ask of their devices, applied after the
        // loop so the executions and the devices are never borrowed at once.
        let mut effects: Vec<(String, TaskCommand)> = Vec::new();
        for execution in self.executions.values_mut() {
            while let Some(due) = execution.due {
                if now < due {
                    break;
                }
                match execution.state.as_str() {
                    "QUEUED" => {
                        execution.state = "IN_PROGRESS".to_string();
                        execution.started_at = Some(due);
                        execution.last_updated_at = due;
                        execution.due = Some(due + EXECUTION_FINISH_SPAN);
                    }
                    "IN_PROGRESS" => {
                        execution.state = "SUCCEEDED".to_string();
                        execution.last_updated_at = due;
                        execution.due = None;
                        if let Some(task) = tasks.get(&execution.task_id) {
                            effects.push((execution.managed_device_id.clone(), task.command));
                        }
                    }
                    // A terminal execution carries no due instant, so this is
                    // unreachable; clearing it keeps the invariant true rather
                    // than looping forever on a record that broke it.
                    _ => execution.due = None,
                }
            }
        }

        for (managed_device_id, command) in effects {
            let Some(device) = self.devices.get_mut(&managed_device_id) else {
                continue;
            };
            // The device answered the command, so it checked in.
            device.last_reached_out_at = now;
            match command {
                // "Unlocks the device." — `UnlockState` is the member that
                // reports whether a device is unlocked.
                TaskCommand::Unlock => {
                    if device.device_state != "UNLOCKED" {
                        device.device_state = "UNLOCKED".to_string();
                        device.last_updated_at = now;
                    }
                }
                // "Reboots the device." A reboot leaves no member of the device
                // record different afterwards; only the check-in above records
                // that it happened.
                TaskCommand::Reboot => {}
            }
        }

        // A task is finished once every execution it created is.
        let task_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.state == "IN_PROGRESS")
            .map(|(id, _)| id.clone())
            .collect();
        for task_id in task_ids {
            let mut finished_at: Option<SystemTime> = None;
            let mut all_terminal = true;
            for execution in self
                .executions
                .values()
                .filter(|execution| execution.task_id == task_id)
            {
                if execution.is_terminal() {
                    finished_at = Some(match finished_at {
                        Some(previous) => previous.max(execution.last_updated_at),
                        None => execution.last_updated_at,
                    });
                } else {
                    all_terminal = false;
                    break;
                }
            }
            let Some(completed_at) = finished_at.filter(|_| all_terminal) else {
                continue;
            };
            let Some(task) = self.tasks.get_mut(&task_id) else {
                continue;
            };
            task.state = "COMPLETED".to_string();
            task.completed_at = Some(completed_at);
            task.last_updated_at = completed_at;
        }
    }
}

/// The ids of the seeded inventory, in `ListDevices` order.
///
/// `ManagedDeviceId` is bounded 1..64 and otherwise unconstrained; `smd-` plus
/// 16 hex digits is the shape AWS managed-device ids carry.
pub fn seeded_device_id(index: u64) -> String {
    format!("smd-{index:016x}")
}

/// The job ids the seeded inventory is associated with. `JobId` is bounded 1..64;
/// `JID` plus a UUID is the shape Snow Family job ids carry.
pub fn seeded_job_id(index: u64) -> String {
    format!("JID{index:08x}-0000-4000-8000-{index:012x}")
}

/// The Snow Family device types the seeded inventory cycles through.
const SEEDED_DEVICE_TYPES: &[&str] = &["SNC1_HDD", "SNC1_SSD", "EDGE_C"];

/// `PhysicalConnectorType`'s values, as the model lists them.
const SEEDED_CONNECTOR_TYPES: &[&str] = &["RJ45", "SFP_PLUS", "QSFP"];

/// The ARN of a managed device: `…:managed-device/<managedDeviceId>`.
pub fn device_arn(managed_device_id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DEVICE_RESOURCE_TYPE}/{managed_device_id}"),
    )
}

/// The ARN of a task: `…:task/<taskId>`.
pub fn task_arn(task_id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_RESOURCE_TYPE}/{task_id}"),
    )
}

/// The region this request was signed for — what a minted ARN names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ResourceNotFoundException` payload for an id that names no resource of
/// `resource_type`.
pub fn not_found(resource_type: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{resource_type} {id} not found"),
    }
}

/// Encode a pagination resume key as a `NextToken`.
///
/// `NextToken` is modelled `@pattern("[a-zA-Z0-9+/=]*")`, which a raw id does not
/// satisfy (`smd-…` and `st-…` both carry a `-`), so the key is hex-encoded: the
/// `hex` crate's alphabet is `[0-9a-f]`, a subset of the modelled one, and the
/// encoding is reversible, which is what lets a page resume exactly where the
/// previous one stopped.
pub fn encode_token(resume_key: &str) -> String {
    ::hex::encode(resume_key)
}

/// Decode a `NextToken` back into the resume key [`encode_token`] built it from,
/// or `None` when it is not a token this service issued.
pub fn decode_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// The page size a list operation serves.
///
/// `maxResults` is modelled `@range(min: 1, max: 100)` and the boundary validator
/// refused anything outside it before a handler ran, so the only case left is an
/// absent value: serve the largest page the model allows.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT);
    debug_assert!(
        (1..=PAGE_LIMIT).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).expect("a page size in 1..=100 fits in usize")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSnowdevicemanagementSnowdevicemanagementState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSnowdevicemanagementSnowdevicemanagementState>)
}
