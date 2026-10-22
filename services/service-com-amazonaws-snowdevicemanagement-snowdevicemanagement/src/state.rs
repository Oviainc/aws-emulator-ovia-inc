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
    AccessDeniedException, Capacity, CpuOptions, DeviceSummary, EbsInstanceBlockDevice,
    ExecutionSummary, Instance, InstanceBlockDeviceMapping, InstanceState, InstanceSummary,
    PhysicalNetworkInterface, ResourceNotFoundException, SecurityGroupIdentifier,
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

/// What the device gate ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::gated_device`])
/// is refused with.
///
/// Recorded behaviour of the real service: a request addressed to a managed
/// device the account does not have activated in the request's region is refused
/// with `AccessDeniedException` (the model gives the shape and its `@httpError
/// 403`; it gives no text), and the refusal names device/region availability
/// rather than an IAM principal — it never says whether that id exists, which is
/// why one message answers every such request.
pub const NO_DEVICES_MESSAGE: &str = "No devices found in this region.";

/// The root device volume of every seeded Amazon EC2 instance.
///
/// `Instance$rootDeviceName` is documented as "The device name of the root device
/// volume (for example, `/dev/sda1`)", which is also the root device a Linux AMI
/// on a Snow device boots from.
const ROOT_DEVICE_NAME: &str = "/dev/sda1";

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
    /// The Amazon EC2 instances running on this device, keyed by `instanceId`.
    ///
    /// Snow Device Management models no instance-creation operation either — an
    /// instance is launched through the device's own EC2-compatible endpoint —
    /// so, like the device itself, the instances are part of the fixture and
    /// live inside the device record: an instance cannot outlive the device it
    /// runs on, and `DescribeDeviceEc2Instances` reads the cloud-side cache of
    /// exactly this set. A `BTreeMap` gives that cache a stable total order.
    pub ec2_instances: BTreeMap<String, Ec2Instance>,
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

/// One Amazon EC2 instance running on a managed device.
///
/// Holds the members the modelled `Instance` reports, as the cloud-side device
/// cache holds them; `DescribeDeviceEc2Instances` projects them with
/// [`Ec2Instance::to_summary`].
#[derive(Debug, Clone)]
pub struct Ec2Instance {
    pub instance_id: String,
    /// "The ID of the AMI used to launch the instance."
    pub image_id: String,
    /// "The Amazon Machine Image (AMI) launch index, which you can use to find
    /// this instance in the launch group."
    pub ami_launch_index: i32,
    /// "The instance type."
    pub instance_type: String,
    /// One of `InstanceStateName`'s values.
    pub state_name: String,
    /// `InstanceState$code` for [`Ec2Instance::state_name`]: the low byte the
    /// model documents for that name.
    pub state_code: i32,
    /// "The private IPv4 address assigned to the instance."
    pub private_ip_address: String,
    /// "The number of cores that the CPU can use."
    pub core_count: i32,
    /// "The number of threads per core in the CPU."
    pub threads_per_core: i32,
    /// "The device name of the root device volume."
    pub root_device_name: String,
    /// The Amazon EBS volume mapped at [`Ec2Instance::root_device_name`].
    pub root_volume_id: String,
    pub security_group_id: String,
    pub security_group_name: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Ec2Instance {
    /// The `InstanceSummary` projection `DescribeDeviceEc2Instances` returns.
    pub fn to_summary(&self) -> InstanceSummary {
        InstanceSummary {
            instance: Some(Instance {
                instance_id: Some(self.instance_id.clone()),
                image_id: Some(self.image_id.clone()),
                ami_launch_index: Some(self.ami_launch_index),
                instance_type: Some(self.instance_type.clone()),
                state: Some(InstanceState {
                    code: Some(self.state_code),
                    name: Some(self.state_name.clone()),
                }),
                private_ip_address: Some(self.private_ip_address.clone()),
                // A seeded instance is reachable on the device's internal
                // network only: a Snow instance has a public address when the
                // operator attaches a direct network interface to it, and this
                // API models no way to ask for one.
                public_ip_address: None,
                created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
                updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
                block_device_mappings: Some(vec![InstanceBlockDeviceMapping {
                    device_name: Some(self.root_device_name.clone()),
                    ebs: Some(EbsInstanceBlockDevice {
                        volume_id: Some(self.root_volume_id.clone()),
                        attach_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
                        // The root volume of an instance launched from an AMI is
                        // deleted with the instance.
                        delete_on_termination: Some(true),
                        // `AttachmentStatus`: the volume is attached while the
                        // instance exists to hold it, and a terminated instance
                        // no longer does.
                        status: Some(
                            if self.state_name == "TERMINATED" {
                                "DETACHED"
                            } else {
                                "ATTACHED"
                            }
                            .to_string(),
                        ),
                    }),
                }]),
                security_groups: Some(vec![SecurityGroupIdentifier {
                    group_id: Some(self.security_group_id.clone()),
                    group_name: Some(self.security_group_name.clone()),
                }]),
                cpu_options: Some(CpuOptions {
                    core_count: Some(self.core_count),
                    threads_per_core: Some(self.threads_per_core),
                }),
                root_device_name: Some(self.root_device_name.clone()),
            }),
            // "When the instance summary was last updated": the cloud-side cache
            // holds what the device last pushed about this instance.
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
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
                ec2_instances: seeded_instances(index, now),
                managed_device_id: managed_device_id.clone(),
            };
            self.devices.insert(managed_device_id, device);
        }
    }

    /// The managed device `managed_device_id` names, or the device gate's
    /// refusal — [`NO_DEVICES_MESSAGE`] as an `AccessDeniedException`.
    ///
    /// Snow Device Management manages devices the account already ordered and
    /// activated, so a request addressed to a device outside that inventory is
    /// refused rather than resolved: the caller learns neither whether the id
    /// exists nor anything about it. Every operation that takes a
    /// `managedDeviceId` declares `AccessDeniedException` (`@httpError 403`),
    /// which is what this refusal is reported as.
    pub fn gated_device(&self, managed_device_id: &str) -> Result<&Device, AccessDeniedException> {
        self.devices
            .get(managed_device_id)
            .ok_or_else(|| AccessDeniedException {
                message: NO_DEVICES_MESSAGE.to_string(),
            })
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

/// How many Amazon EC2 instances the fixture places on each seeded device, by
/// device index.
///
/// The counts differ so an answer computed from the device's own cache is
/// distinguishable from one computed from the whole inventory, and the last
/// device runs none: a device with no instances answers an empty list, not an
/// error.
const SEEDED_INSTANCE_COUNTS: &[u64] = &[4, 2, 0];

/// `InstanceStateName`'s values, each with the `InstanceState$code` the model
/// documents for it ("the valid values for the instance state code" — `0`:
/// pending, `16`: running, `32`: shutting-down, `48`: terminated, `64`:
/// stopping, `80`: stopped). The fixture cycles through the whole set, so every
/// published name/code pair appears in some instance.
const SEEDED_INSTANCE_STATES: &[(&str, i32)] = &[
    ("PENDING", 0),
    ("RUNNING", 16),
    ("SHUTTING_DOWN", 32),
    ("TERMINATED", 48),
    ("STOPPING", 64),
    ("STOPPED", 80),
];

/// The Snow Family compute instance types the fixture cycles through, each with
/// the number of vCPUs it has, as the Snowball Edge developer guide's
/// instance-type table lists them (`sbe-c` is the Compute Optimized family).
const SEEDED_INSTANCE_TYPES: &[(&str, i32)] = &[
    ("sbe-c.large", 2),
    ("sbe-c.xlarge", 4),
    ("sbe-c.2xlarge", 8),
    ("sbe-c.4xlarge", 16),
];

/// The id of the seeded instance at `index` in the whole fixture.
///
/// A Snow device's local Amazon EC2 resources carry the EC2 id shape behind an
/// `s.` prefix (the same prefix the seeded physical network interfaces use):
/// `s.i-` plus 17 hex digits for an instance.
pub fn seeded_instance_id(index: u64) -> String {
    format!("s.i-{index:017x}")
}

/// The ids of the instances the fixture places on the device at `device_index`,
/// in the order the device cache holds them (which is their id order, so a
/// caller's request order cannot be what an answer's order comes from).
pub fn seeded_instance_ids(device_index: u64) -> Vec<String> {
    let offset = seeded_instance_offset(device_index);
    (0..seeded_instance_count(device_index))
        .map(|slot| seeded_instance_id(offset + slot))
        .collect()
}

/// How many instances the fixture places on the device at `device_index`.
fn seeded_instance_count(device_index: u64) -> u64 {
    usize::try_from(device_index)
        .ok()
        .and_then(|index| SEEDED_INSTANCE_COUNTS.get(index))
        .copied()
        .unwrap_or(0)
}

/// How many instances the fixture placed on the devices before `device_index`,
/// which is where that device's instance ids start.
fn seeded_instance_offset(device_index: u64) -> u64 {
    (0..device_index).map(seeded_instance_count).sum()
}

/// The Amazon EC2 instances the fixture places on the device at `device_index`.
fn seeded_instances(device_index: u64, now: SystemTime) -> BTreeMap<String, Ec2Instance> {
    let offset = seeded_instance_offset(device_index);
    let mut instances = BTreeMap::new();
    for slot in 0..seeded_instance_count(device_index) {
        let index = usize::try_from(offset + slot).expect("the fixture instance count fits usize");
        let launch_index = i32::try_from(slot).expect("the per-device instance count fits in i32");
        let (state_name, state_code) = SEEDED_INSTANCE_STATES[index % SEEDED_INSTANCE_STATES.len()];
        let (instance_type, vcpus) = SEEDED_INSTANCE_TYPES[index % SEEDED_INSTANCE_TYPES.len()];
        let instance_id = seeded_instance_id(offset + slot);
        instances.insert(
            instance_id.clone(),
            Ec2Instance {
                instance_id,
                // Every instance on one device was launched from the AMI that
                // device holds.
                image_id: format!("s.ami-{device_index:017x}"),
                ami_launch_index: launch_index,
                instance_type: instance_type.to_string(),
                state_name: state_name.to_string(),
                state_code,
                // The device itself answers on 10.0.0.<index+10>; the instances
                // it runs sit on their own subnet behind it.
                private_ip_address: format!("10.0.{}.{}", device_index + 1, slot + 10),
                // A Snow compute instance's vCPUs are cores with a single
                // thread each.
                core_count: vcpus,
                threads_per_core: 1,
                root_device_name: ROOT_DEVICE_NAME.to_string(),
                root_volume_id: format!("s.vol-{:017x}", offset + slot),
                // One security group per device, as a device's default VPC
                // gives it.
                security_group_id: format!("s.sg-{device_index:017x}"),
                security_group_name: "default".to_string(),
                created_at: now,
                updated_at: now,
            },
        );
    }
    instances
}

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

/// The largest a `ManagedDeviceId` or a `TaskId` may be: both are modelled
/// `@length(min: 1, max: 64)`, so an id outside that bound names no resource this
/// service can hold, whatever an ARN carrying it looks like.
pub const RESOURCE_ID_LIMIT: usize = 64;

/// How the tagging operations word a `resourceArn` they cannot parse.
///
/// This is the wording the behavioural corpus recorded for this refusal class, and it
/// is ONE message for the whole class — every unparseable value gets it, whichever ARN
/// field is wrong — so it states the class and is never fitted to a particular request.
/// The values a caller sent are not echoed back into it.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// How the tagging operations word a `resourceArn` that parses but addresses no device
/// and no task.
///
/// Recorded wording, and ONE message for the whole class — it names neither the
/// resource type nor the id, so it is the same whichever resource was addressed and
/// discloses nothing beyond "not found". The device and task operations keep their own
/// modelled `ResourceNotFoundException` wording ([`not_found`]), which is a different
/// shape on a different set of operations.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// Which kind of resource a tagging operation's `resourceArn` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// `…:managed-device/<managedDeviceId>`
    Device,
    /// `…:task/<taskId>`
    Task,
}

impl TaggedResourceKind {
    /// The `resourceType` segment this kind's ARN carries, which is also what a
    /// not-found message names it by.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Device => DEVICE_RESOURCE_TYPE,
            Self::Task => TASK_RESOURCE_TYPE,
        }
    }
}

/// The resource a well-formed `resourceArn` addresses: its kind and its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedResource {
    pub kind: TaggedResourceKind,
    pub id: String,
}

/// Why a `resourceArn` addresses no resource of this service at all.
///
/// Distinct from "there is no such resource": the ARN never named one, so the
/// answer cannot be a 404 about an id this service could hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArnRefusal {
    /// The value is not an ARN of this service naming a device or a task. The
    /// tagging operations answer this with HTTP 400 `BadRequestException`.
    Malformed(String),
    /// A well-formed ARN whose account is not the one this emulator serves.
    /// Answered with the modelled `AccessDeniedException` (HTTP 403), which says
    /// nothing about whether that account holds such a resource.
    ForeignAccount(String),
}

/// Resolve a tagging operation's `resourceArn` to the device or task it names.
///
/// The ARN has to be this service's own:
/// `arn:<partition>:snow-device-management:<region>:<account>:managed-device/<id>`
/// or `…:task/<id>`, with a 12-digit account and an id inside the modelled
/// `@length(min: 1, max: 64)`. Anything else never named a resource of this
/// service — [`ArnRefusal::Malformed`]. A well-formed ARN for another account is
/// [`ArnRefusal::ForeignAccount`]; one for this account resolves to the id a
/// caller's record is keyed by, whatever region the ARN names, because the
/// inventory and the task store are per-account.
pub fn parse_tagged_resource_arn(resource_arn: &str) -> Result<TaggedResource, ArnRefusal> {
    let malformed = || ArnRefusal::Malformed(INVALID_RESOURCE_ARN_MESSAGE.to_string());

    // `splitn` keeps the resource portion whole, so a resource id carrying a `:`
    // is not mistaken for a seventh ARN field.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return Err(malformed());
    };
    if *prefix != "arn"
        || partition.is_empty()
        || !partition
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || *service != ARN_NAMESPACE
        || region.is_empty()
    {
        return Err(malformed());
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed());
    }

    let Some((resource_type, id)) = resource.split_once('/') else {
        return Err(malformed());
    };
    let kind = match resource_type {
        DEVICE_RESOURCE_TYPE => TaggedResourceKind::Device,
        TASK_RESOURCE_TYPE => TaggedResourceKind::Task,
        _ => return Err(malformed()),
    };
    if !(1..=RESOURCE_ID_LIMIT).contains(&id.len()) {
        return Err(malformed());
    }

    if *account != ::engine::emulator_account_id() {
        return Err(ArnRefusal::ForeignAccount(format!(
            "you don't have sufficient access to perform this action on {resource_arn}"
        )));
    }

    Ok(TaggedResource {
        kind,
        id: id.to_string(),
    })
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
