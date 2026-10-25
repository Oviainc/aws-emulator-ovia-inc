// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-snowdevicemanagement-snowdevicemanagement service.
//!
//! Snow Device Management manages Amazon Web Services Snow Family devices that an
//! account already ordered with remote management enabled: the API models NO
//! device-creation operation, so a device enters the inventory only through
//! [`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::activate_devices`],
//! never through a request. Having at least one such device in the signed Region is
//! the PRECONDITION the whole device/task surface is served behind: a Region this
//! account has activated none in has no device for a call to act on or be authorized
//! against, so every operation the model declares `AccessDeniedException` on refuses
//! there — [`no_devices_in_region`], checked through
//! [`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::has_devices_in`]
//! before an id is resolved and before a field is validated. The tagging operations,
//! which the model declares no `AccessDeniedException` on, are served regardless.
//! Everything else in here is minted by a caller: `CreateTask` persists a [`Task`]
//! plus one [`Execution`] per target, `CancelTask` moves the ones no device has
//! started yet ([`Execution::is_cancellable`]) to `CANCELED`, and the reporting
//! operations compute their answers from these records. A device performs ONE
//! execution at a time, so the executions of one device are a QUEUE the clock
//! dispatches from one by one
//! ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::settle`]), not a
//! set that advances independently.
//!
//! Two bounds keep the task records finite, and they bound different things: a Region
//! may hold [`IN_FLIGHT_TASK_QUOTA`] tasks at once, which is what `CreateTask` refuses
//! past, and it keeps [`TASK_HISTORY_LIMIT`] records, past which the oldest terminal
//! ones are forgotten
//! ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::trim_task_history`]) —
//! a quota is about what is held now, and nothing here deletes a record on request.
//!
//! The inventory and the tasks are REGIONAL, the way the service is: Snow Device
//! Management reports "all devices on your Amazon Web Services account that have
//! Amazon Web Services Snow Device Management enabled in the Amazon Web Services
//! Region where the command is run", so a record carries the region it belongs to
//! and is invisible in every other one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccessDeniedException, Capacity, CpuOptions, DeviceSummary, EbsInstanceBlockDevice,
    ExecutionSummary, Instance, InstanceBlockDeviceMapping, InstanceState, InstanceSummary,
    PhysicalNetworkInterface, ResourceNotFoundException, ResourceSummary, SecurityGroupIdentifier,
    SoftwareInformation, TaskSummary,
};

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the model's `@aws.api#service` trait gives it
/// (also its sigv4 signing name).
pub const ARN_NAMESPACE: &str = "snow-device-management";

/// The largest page `ListDevices` / `ListTasks` / `ListExecutions` serve, and the
/// page size they use when the request names no `maxResults`: all three model it
/// `@range(min: 1, max: 100)`.
pub const PAGE_LIMIT: i32 = 100;

/// How many tasks one Region may have IN FLIGHT before `CreateTask` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// A service quota bounds what an account currently HOLDS, so this is counted over
/// the tasks still `IN_PROGRESS` in the Region the request is signed for
/// ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::tasks_in_flight_in`]),
/// never over how many tasks were ever created: a task the clock carried to
/// `COMPLETED`, or one `CancelTask` moved to `CANCELED`, is terminal and holds
/// nothing, so the next create succeeds. The value is the emulator's OWN bound, not
/// a claim about any account's AWS quota (Service Quotas is not modelled here), and
/// it is set far above any workflow an emulator serves.
pub const IN_FLIGHT_TASK_QUOTA: usize = 1_000;

/// How many task records one Region keeps, terminal ones included.
///
/// The in-flight quota above bounds what a Region holds at once; this bounds the
/// HISTORY behind it, which nothing else would: a task record is only ever added by
/// a request, and this API models no operation that deletes one. Past this many
/// records in a Region, the oldest TERMINAL ones make room for the new task
/// ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::trim_task_history`]),
/// so the store cannot grow for as long as the emulator runs. It is the emulator's
/// own retention bound, set far above the history any workflow it serves reads back,
/// and it refuses nothing.
pub const TASK_HISTORY_LIMIT: usize = 10_000;

/// The EARLIEST an execution leaves the queue: how long it stays `QUEUED` before
/// the clock owes it `IN_PROGRESS` on a device that is not performing another one.
///
/// AWS dispatches a command to a Snow device over however long the device takes
/// to check in; the emulator CHOOSES this span. It is deliberately not zero, so a
/// caller that reads straight after `CreateTask` still observes the `QUEUED`
/// execution. The due instant is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps. It is a floor and not the
/// dispatch instant itself: "Only one task is executed on a device at a time", so
/// an execution whose device is already performing one waits for that one to end
/// ([`ComAmazonawsSnowdevicemanagementSnowdevicemanagementState::settle`]).
pub const EXECUTION_START_SPAN: Duration = Duration::from_millis(400);

/// How long an execution stays `IN_PROGRESS` before the clock owes it
/// `SUCCEEDED` and applies the command to the target device. Chosen for the same
/// reason as [`EXECUTION_START_SPAN`].
pub const EXECUTION_FINISH_SPAN: Duration = Duration::from_millis(400);

/// The `resourceType` segment a not-found message names for a managed device.
pub const DEVICE_RESOURCE_TYPE: &str = "managed-device";

/// The `ResourceSummary$resourceType` of an Amazon EC2 instance running on a
/// device, which is the only resource type a device reports: "Returns a list of
/// the Amazon Web Services resources available for a device. Currently, Amazon
/// EC2 instances are the only supported resource type." The value is the
/// AWS-wide type identifier of an EC2 instance, the same string the `type`
/// filter of that operation takes.
pub const EC2_INSTANCE_RESOURCE_TYPE: &str = "AWS::EC2::Instance";

/// The `resourceType` segment a not-found message names for a task.
pub const TASK_RESOURCE_TYPE: &str = "task";

/// The `resourceType` segment a not-found message names for an execution. An
/// execution is addressed by the `(taskId, managedDeviceId)` pair rather than by an
/// id of its own, so that pair is what the message states.
pub const EXECUTION_RESOURCE_TYPE: &str = "execution";

/// The tag-key prefix AWS reserves for its own use.
///
/// "Do not use the `aws:` prefix in your tag names or values because it is reserved for
/// Amazon Web Services use. You can't edit or delete tag names or values with this
/// prefix." (AWS tagging documentation — the rule is service-independent, which is why
/// the model carries no pattern for it.) [`tag_key_violation`] enforces it on the tag
/// KEY, which is the half of the rule the recorded service refuses a request for, and
/// "you can't edit or delete" covers a removal as much as a write: every operation that
/// NAMES a tag key goes through that one check.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// How many tags one resource may carry: the AWS-wide tagging quota ("Maximum number of
/// tags per resource: 50").
pub const MAX_TAGS: usize = 50;

/// The longest tag key AWS accepts ("Maximum key length: 128 Unicode characters in
/// UTF-8").
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value AWS accepts ("Maximum value length: 256 Unicode characters in
/// UTF-8").
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The message `TagResource` refuses a `resourceArn` it cannot parse with.
///
/// One sentence for the whole refusal class, echoing no part of the request. It is the
/// wording the behavioural corpus recorded for THIS operation; `UntagResource`'s
/// recorded wording for the same class carries a `: bad syntax` suffix
/// ([`INVALID_RESOURCE_ARN_MESSAGE`]), so the two are kept apart rather than one being
/// fitted to the other. What a client branches on is the 400 and the
/// `BadRequestException` wire code (see [`crate::scaffolded::Scaffolded::bad_request`]),
/// not this prose.
pub const TAG_RESOURCE_INVALID_ARN_MESSAGE: &str = "Invalid input resource arn";

/// The root device volume of every Amazon EC2 instance the fixture places on a
/// device.
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
    /// The Region this device has Snow Device Management enabled in. A request
    /// signed for any other Region does not see it.
    pub region: String,
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
    /// This device's tags, as the modelled `TagMap` carries them.
    pub fn tag_map(&self) -> HashMap<String, String> {
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
                // A fixture instance is reachable on the device's internal
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

    /// The `ResourceSummary` projection `ListDeviceResources` returns: "a
    /// structure defining the resource's type, Amazon Resource Name (ARN), and
    /// ID".
    pub fn to_resource_summary(&self, region: &str) -> ResourceSummary {
        ResourceSummary {
            resource_type: EC2_INSTANCE_RESOURCE_TYPE.to_string(),
            arn: Some(instance_arn(&self.instance_id, region)),
            id: Some(self.instance_id.clone()),
        }
    }
}

/// One task a caller created, with the record every task operation reads.
#[derive(Debug, Clone)]
pub struct Task {
    pub task_id: String,
    pub task_arn: String,
    /// The Region the task was created in — the one its targets are managed in.
    pub region: String,
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
    /// True while this task still holds work: `TaskState` is `IN_PROGRESS`, the one
    /// non-terminal value of the three (`COMPLETED` and `CANCELED` are both final,
    /// and a task in either holds nothing a quota counts).
    pub fn is_in_flight(&self) -> bool {
        self.state == "IN_PROGRESS"
    }

    /// The tags `DescribeTask` reports.
    pub fn tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The `TaskSummary` projection `ListTasks` returns.
    pub fn to_summary(&self) -> TaskSummary {
        TaskSummary {
            task_id: self.task_id.clone(),
            task_arn: Some(self.task_arn.clone()),
            state: Some(self.state.clone()),
            tags: Some(self.tag_map()),
        }
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

    /// True while this execution can still be stopped: the device has not been
    /// given the command yet, so it is still waiting in the queue.
    ///
    /// `CancelTask` "can cancel a task only if it's still in a `QUEUED` state.
    /// Tasks that are already running can't be cancelled." `TaskState` carries
    /// no `QUEUED` value, so the state that sentence names is this record's
    /// `ExecutionState`: once the device has started the command the cancel has
    /// arrived too late and the execution runs to its own outcome.
    pub fn is_cancellable(&self) -> bool {
        self.state == "QUEUED"
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
    /// Every execution, keyed by `(region, taskId, managedDeviceId)`: the Region
    /// the task was created in, then the pair `DescribeExecution` addresses one
    /// by. The Region leads the key so a record is read out of the same partition
    /// the request resolved its task in — reach them through [`Self::execution_in`]
    /// / [`Self::executions_in`] rather than by rebuilding the key — and the
    /// `managedDeviceId` trails it, which is the order `ListExecutions` paginates
    /// over within one task.
    pub executions: BTreeMap<(String, String, String), Execution>,
    /// A `CreateTask` `clientToken` to the task that token created, keyed by the
    /// `(region, clientToken)` pair the token was used in, so a repeated create
    /// reports the first task instead of minting a second — and a token reused in
    /// another Region, where that task does not exist, names nothing.
    pub idempotency: HashMap<(String, String), String>,
    /// The counter task ids are minted from.
    pub task_id_seq: u64,
    /// The counter execution ids are minted from.
    pub execution_id_seq: u64,
    /// The counter managed-device ids are minted from.
    device_id_seq: u64,
}

impl ComAmazonawsSnowdevicemanagementSnowdevicemanagementState {
    /// The execution the `(taskId, managedDeviceId)` pair addresses in `region`, or
    /// `None` when that Region holds no such record.
    pub fn execution_in(
        &self,
        region: &str,
        task_id: &str,
        managed_device_id: &str,
    ) -> Option<&Execution> {
        self.executions.get(&(
            region.to_string(),
            task_id.to_string(),
            managed_device_id.to_string(),
        ))
    }

    /// Every execution of the task `task_id` names in `region`, with the
    /// `managedDeviceId` each belongs to, in that id's order.
    pub fn executions_in<'a>(
        &'a self,
        region: &'a str,
        task_id: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Execution)> + 'a {
        self.executions
            .iter()
            .filter(move |((execution_region, execution_task, _), _)| {
                execution_region.as_str() == region && execution_task.as_str() == task_id
            })
            .map(|((_, _, managed_device_id), execution)| (managed_device_id.as_str(), execution))
    }

    /// Every execution of the task `task_id` names in `region`, to change.
    pub fn executions_in_mut<'a>(
        &'a mut self,
        region: &'a str,
        task_id: &'a str,
    ) -> impl Iterator<Item = &'a mut Execution> + 'a {
        self.executions
            .iter_mut()
            .filter(move |((execution_region, execution_task, _), _)| {
                execution_region.as_str() == region && execution_task.as_str() == task_id
            })
            .map(|(_, execution)| execution)
    }

    /// Whether this account has Snow Device Management activated on at least one
    /// device in `region`.
    ///
    /// This is the account/region precondition the device and task surface is served
    /// behind: Snow Device Management acts on devices, and a Region holding none has
    /// nothing for a call to act on or be authorized against, so every operation the
    /// model declares `AccessDeniedException` on refuses with
    /// [`no_devices_in_region`] there — ahead of resolving any id and ahead of
    /// validating any field, so a caller learns nothing about what a Region it has no
    /// device in holds. The tagging operations, which the model declares no
    /// `AccessDeniedException` on, do not consult this.
    pub fn has_devices_in(&self, region: &str) -> bool {
        self.devices.values().any(|device| device.region == region)
    }

    /// The device `managed_device_id` names in `region`, or `None` when this
    /// account manages no such device there.
    pub fn device_in(&self, region: &str, managed_device_id: &str) -> Option<&Device> {
        self.devices
            .get(managed_device_id)
            .filter(|device| device.region == region)
    }

    /// How many tasks are still in flight in `region`: the consumption
    /// [`IN_FLIGHT_TASK_QUOTA`] bounds.
    ///
    /// Counted over the non-terminal records of ONE Region, the way every other
    /// record in this state is scoped, because that is what the Region the request
    /// is signed for actually holds — a terminal task, and a task of another
    /// Region, consume none of it.
    pub fn tasks_in_flight_in(&self, region: &str) -> usize {
        self.tasks
            .values()
            .filter(|task| task.region == region && task.is_in_flight())
            .count()
    }

    /// Drop the oldest terminal tasks of `region` until it keeps no more than
    /// `keep` records, answering how many were dropped.
    ///
    /// The store's retention bound, applied where the store GROWS. Only terminal
    /// records are candidates and the oldest go first, so a task still in flight is
    /// never dropped and the recent history a caller reads back outlives the
    /// forgotten one; a Region holding nothing but in-flight tasks loses nothing.
    /// The executions of a dropped task go with it, as do the `clientToken` entries
    /// that named it — a token whose task is gone names nothing, and a create
    /// carrying it mints a new task.
    pub fn trim_task_history(&mut self, region: &str, keep: usize) -> usize {
        let mut oldest_first: Vec<(SystemTime, String)> = self
            .tasks
            .values()
            .filter(|task| task.region == region)
            .map(|task| (task.created_at, task.task_id.clone()))
            .collect();
        let excess = oldest_first.len().saturating_sub(keep);
        if excess == 0 {
            return 0;
        }
        // The id breaks a tie, so two tasks created in the same instant still have
        // one order and the choice is not left to the map's iteration.
        oldest_first.sort();

        let mut dropped: HashSet<String> = HashSet::new();
        for (_, task_id) in oldest_first {
            if dropped.len() == excess {
                break;
            }
            if self
                .tasks
                .get(&task_id)
                .is_some_and(|task| task.is_in_flight())
            {
                continue;
            }
            dropped.insert(task_id);
        }
        if dropped.is_empty() {
            return 0;
        }

        for task_id in &dropped {
            self.tasks.remove(task_id);
        }
        self.executions.retain(|(execution_region, execution_task, _), _| {
            execution_region.as_str() != region || !dropped.contains(execution_task)
        });
        self.idempotency
            .retain(|_, task_id| !dropped.contains(task_id));
        dropped.len()
    }

    /// The task `task_id` names in `region`, or `None` when no such task was
    /// created there.
    pub fn task_in(&self, region: &str, task_id: &str) -> Option<&Task> {
        self.tasks
            .get(task_id)
            .filter(|task| task.region == region)
    }

    /// The task `task_id` names in `region`, to change: the mutable counterpart of
    /// [`Self::task_in`], so a write lands in the partition the read resolved it
    /// in.
    pub fn task_in_mut(&mut self, region: &str, task_id: &str) -> Option<&mut Task> {
        self.tasks
            .get_mut(task_id)
            .filter(|task| task.region == region)
    }

    /// Activate Snow Device Management on `count` devices in `region`, answering
    /// the ids they were given.
    ///
    /// This is the ONLY way a device enters the inventory, and it is deliberately
    /// not reachable from a request: Snow Device Management models no
    /// device-creation operation, because a device becomes manageable by being
    /// ORDERED — a Snow Family job created with remote management enabled — which
    /// is another service's API. Until something activates a device here, this
    /// account's inventory in `region` is empty, and the device/task surface is
    /// refused there ([`Self::has_devices_in`]).
    ///
    /// Ids, ARNs and job ids are the store's own: an id is minted from the
    /// monotonic device counter (so it is unique across Regions), and two
    /// consecutive devices share one ordering job, the way one job ships more than
    /// one device.
    pub fn activate_devices(&mut self, region: &str, count: u64, now: SystemTime) -> Vec<String> {
        let mut activated = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        for _ in 0..count {
            let index = self.device_id_seq;
            self.device_id_seq += 1;
            let managed_device_id = managed_device_id(index);
            let device = Device {
                managed_device_arn: device_arn(&managed_device_id, region),
                region: region.to_string(),
                device_type: DEVICE_TYPES[usize::try_from(index % DEVICE_TYPES_LEN)
                    .expect("an index modulo the type count fits in usize")]
                .to_string(),
                // A device Snow Device Management has just been activated on is
                // `LOCKED`: unlocking one is what an `unlock` task does, and a
                // device that arrived unlocked would make that task unobservable.
                device_state: "LOCKED".to_string(),
                physical_network_interfaces: vec![PhysicalNetworkInterface {
                    physical_network_interface_id: Some(format!("s.ni-{index:016x}")),
                    // `PhysicalConnectorType`'s values, cycled so the inventory
                    // is not uniform.
                    physical_connector_type: Some(
                        CONNECTOR_TYPES[usize::try_from(index % CONNECTOR_TYPES_LEN)
                            .expect("an index modulo the connector count fits in usize")]
                        .to_string(),
                    ),
                    // `IpAddressAssignment`: an activated device holds a fixed
                    // address, so it is `STATIC`.
                    ip_address_assignment: Some("STATIC".to_string()),
                    ip_address: Some(format!("10.0.0.{}", index % 240 + 10)),
                    netmask: Some("255.255.255.0".to_string()),
                    default_gateway: Some("10.0.0.1".to_string()),
                    mac_address: Some(format!("02:00:00:00:00:{:02x}", index % 256)),
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
                    // Nothing is being installed on a device that just arrived, so
                    // there is no version in flight and no install in progress.
                    installing_version: None,
                    install_state: Some("NA".to_string()),
                },
                tags: BTreeMap::new(),
                last_reached_out_at: now,
                last_updated_at: now,
                associated_with_job: ordering_job_id(index / 2),
                ec2_instances: fixture_instances(index, now),
                managed_device_id: managed_device_id.clone(),
            };
            self.devices.insert(managed_device_id.clone(), device);
            activated.push(managed_device_id);
        }
        activated
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

    /// The tags held on the device or task a tagging operation's `resourceArn`
    /// resolved to ([`parse_tagged_resource_arn`]), or `None` when this account holds
    /// no such record.
    ///
    /// The lookup is by the id the ARN carries — the key each record is stored under —
    /// so a tag read and `UntagResource` address exactly the same record.
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<HashMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Device => self.devices.get(&resource.id).map(Device::tag_map),
            TaggedResourceKind::Task => self.tasks.get(&resource.id).map(Task::tag_map),
        }
    }

    /// The tags held on the device or task a tagging operation's `resourceArn`
    /// resolved to, to CHANGE: the mutable counterpart of [`Self::tags_of`], so a
    /// tag write lands on exactly the record a tag read reports.
    ///
    /// This is the one place the kind/id → stored-tags resolution lives for the
    /// write paths: `TagResource` and `UntagResource` both go through it, so the
    /// two cannot resolve a `resourceArn` differently from each other or from the
    /// read path. `None` means this account holds no such record, which is the
    /// tagging APIs' not-found class ([`TAGGING_NOT_FOUND_MESSAGE`]).
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut BTreeMap<String, String>> {
        match resource.kind {
            TaggedResourceKind::Device => self
                .devices
                .get_mut(&resource.id)
                .map(|device| &mut device.tags),
            TaggedResourceKind::Task => self.tasks.get_mut(&resource.id).map(|task| &mut task.tags),
        }
    }

    /// Materialise every state change the clock already owes.
    ///
    /// An execution the span has elapsed on moves `QUEUED` -> `IN_PROGRESS` ->
    /// `SUCCEEDED`; the second of those applies the task's command to the target
    /// device. One device performs ONE execution at a time, so the executions of
    /// one device are a QUEUE rather than a set that advances independently: one
    /// whose device is still performing another stays `QUEUED` however long its own
    /// dispatch span has been elapsed, and leaves the queue once that one is
    /// terminal. The queue's order is the order the executions were created in,
    /// which is the order this map is keyed in — the task ids a Region's records
    /// carry are minted from a monotonic counter at a fixed width, so iterating
    /// `(region, taskId, managedDeviceId)` reaches each device's executions oldest
    /// first. A task whose every execution is terminal becomes `COMPLETED`. Nothing
    /// here depends on who called: the same changes were owed at this instant even
    /// if nobody had.
    pub fn settle(&mut self, now: SystemTime) {
        let tasks = &self.tasks;
        // What the finished executions ask of their devices, applied after the
        // loop so the executions and the devices are never borrowed at once.
        let mut effects: Vec<(String, TaskCommand)> = Vec::new();
        // Per device, the instant it stops being occupied by the execution it is
        // already performing. A device with no entry is performing nothing.
        let mut occupied_until: HashMap<&str, SystemTime> = HashMap::new();
        for ((_, _, managed_device_id), execution) in self.executions.iter_mut() {
            let occupied = occupied_until.get(managed_device_id.as_str()).copied();
            if execution.due.is_none() {
                // A terminal execution owes nothing. One the device actually
                // performed occupied it until the instant it ended; one that was
                // stopped before it was ever dispatched never occupied it at all.
                if execution.started_at.is_some() {
                    let ended = match occupied {
                        Some(busy) => busy.max(execution.last_updated_at),
                        None => execution.last_updated_at,
                    };
                    occupied_until.insert(managed_device_id.as_str(), ended);
                }
                continue;
            }
            let mut occupied = occupied;
            while let Some(due) = execution.due {
                match execution.state.as_str() {
                    "QUEUED" => {
                        // The command leaves the queue once its dispatch span has
                        // passed AND the device has finished what it was already
                        // performing — whichever of the two is later.
                        let starts = occupied.map_or(due, |busy| due.max(busy));
                        if now < starts {
                            // Still queued, and nothing else reaches this device
                            // before the run it waits for would end. Nothing is
                            // written: the instant is recomputed from the same
                            // records on the next call.
                            occupied = Some(starts + EXECUTION_FINISH_SPAN);
                            break;
                        }
                        execution.state = "IN_PROGRESS".to_string();
                        execution.started_at = Some(starts);
                        execution.last_updated_at = starts;
                        execution.due = Some(starts + EXECUTION_FINISH_SPAN);
                    }
                    "IN_PROGRESS" => {
                        if now < due {
                            // The device is performing this one until it ends.
                            occupied = Some(due);
                            break;
                        }
                        execution.state = "SUCCEEDED".to_string();
                        execution.last_updated_at = due;
                        execution.due = None;
                        occupied = Some(due);
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
            if let Some(occupied) = occupied {
                occupied_until.insert(managed_device_id.as_str(), occupied);
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
            .filter(|(_, task)| task.is_in_flight())
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

/// The id form a managed device carries.
///
/// `ManagedDeviceId` is bounded 1..64 and otherwise unconstrained; `smd-` plus
/// 16 hex digits is the shape AWS managed-device ids carry. The hex rendering of
/// the store's counter keeps the ids in one stable total order, which is the order
/// `ListDevices` pages over.
pub fn managed_device_id(index: u64) -> String {
    format!("smd-{index:016x}")
}

/// The id form of the Snow Family job a device was ordered by. `JobId` is bounded
/// 1..64; `JID` plus a UUID is the shape Snow Family job ids carry.
pub fn ordering_job_id(index: u64) -> String {
    format!("JID{index:08x}-0000-4000-8000-{index:012x}")
}

/// The Snow Family device types an activated inventory cycles through.
const DEVICE_TYPES: &[&str] = &["SNC1_HDD", "SNC1_SSD", "EDGE_C"];

/// `DEVICE_TYPES`'s length, as the `u64` the index arithmetic needs.
const DEVICE_TYPES_LEN: u64 = DEVICE_TYPES.len() as u64;

/// `PhysicalConnectorType`'s values, as the model lists them.
const CONNECTOR_TYPES: &[&str] = &["RJ45", "SFP_PLUS", "QSFP"];

/// `CONNECTOR_TYPES`'s length, as the `u64` the index arithmetic needs.
const CONNECTOR_TYPES_LEN: u64 = CONNECTOR_TYPES.len() as u64;

/// How many Amazon EC2 instances the fixture places on each activated device, by
/// device index.
///
/// The counts differ so an answer computed from the device's own cache is
/// distinguishable from one computed from the whole inventory, and the third
/// device runs none: a device with no instances answers an empty list, not an
/// error.
const FIXTURE_INSTANCE_COUNTS: &[u64] = &[4, 2, 0];

/// `InstanceStateName`'s values, each with the `InstanceState$code` the model
/// documents for it ("the valid values for the instance state code" — `0`:
/// pending, `16`: running, `32`: shutting-down, `48`: terminated, `64`:
/// stopping, `80`: stopped). The fixture cycles through the whole set, so every
/// published name/code pair appears in some instance.
const FIXTURE_INSTANCE_STATES: &[(&str, i32)] = &[
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
const FIXTURE_INSTANCE_TYPES: &[(&str, i32)] = &[
    ("sbe-c.large", 2),
    ("sbe-c.xlarge", 4),
    ("sbe-c.2xlarge", 8),
    ("sbe-c.4xlarge", 16),
];

/// The id of the fixture instance at `index` in the whole inventory.
///
/// A Snow device's local Amazon EC2 resources carry the EC2 id shape behind an
/// `s.` prefix (the same prefix the activated physical network interfaces use):
/// `s.i-` plus 17 hex digits for an instance.
pub fn instance_id(index: u64) -> String {
    format!("s.i-{index:017x}")
}

/// The ids of the instances the fixture places on the device at `device_index`,
/// in the order the device cache holds them (which is their id order, so a
/// caller's request order cannot be what an answer's order comes from).
pub fn device_instance_ids(device_index: u64) -> Vec<String> {
    let offset = instance_offset(device_index);
    (0..instance_count(device_index))
        .map(|slot| instance_id(offset + slot))
        .collect()
}

/// How many instances the fixture places on the device at `device_index`.
fn instance_count(device_index: u64) -> u64 {
    usize::try_from(device_index)
        .ok()
        .and_then(|index| FIXTURE_INSTANCE_COUNTS.get(index))
        .copied()
        .unwrap_or(0)
}

/// How many instances the fixture placed on the devices before `device_index`,
/// which is where that device's instance ids start.
fn instance_offset(device_index: u64) -> u64 {
    (0..device_index).map(instance_count).sum()
}

/// The Amazon EC2 instances the fixture places on the device at `device_index`.
fn fixture_instances(device_index: u64, now: SystemTime) -> BTreeMap<String, Ec2Instance> {
    let offset = instance_offset(device_index);
    let mut instances = BTreeMap::new();
    for slot in 0..instance_count(device_index) {
        let index = usize::try_from(offset + slot).expect("the fixture instance count fits usize");
        let launch_index = i32::try_from(slot).expect("the per-device instance count fits in i32");
        let (state_name, state_code) =
            FIXTURE_INSTANCE_STATES[index % FIXTURE_INSTANCE_STATES.len()];
        let (instance_type, vcpus) = FIXTURE_INSTANCE_TYPES[index % FIXTURE_INSTANCE_TYPES.len()];
        let instance_id = instance_id(offset + slot);
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

/// The ARN of an Amazon EC2 instance: the EC2 service's own
/// `…:instance/<instanceId>` form, since the resource an instance summary names
/// is an EC2 instance and not a Snow Device Management resource.
pub fn instance_arn(instance_id: &str, region: &str) -> String {
    ::engine::emulator_arn("ec2", region, &format!("instance/{instance_id}"))
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

/// The input member a tagging WRITE reports its tag-key violations under
/// (`TagResource$tags`, `CreateTask$tags`).
pub const TAGS_INPUT_MEMBER: &str = "tags";

/// The input member a tag REMOVAL reports its tag-key violations under
/// (`UntagResource$tagKeys`).
pub const TAG_KEYS_INPUT_MEMBER: &str = "tagKeys";

/// Why one tag KEY is not a key this service accepts, or `None` when it is.
///
/// The rules are the AWS-wide ones on a tag key ([`RESERVED_TAG_PREFIX`],
/// [`MAX_TAG_KEY_LENGTH`], and a key must not be empty); they hold wherever a request
/// NAMES a tag key, whether it is writing that tag or removing it, so every operation
/// that takes a key — [`tag_violation`] for a `TagMap`, `UntagResource` for its
/// `tagKeys` — refuses the same keys here. `field` is the input member the message
/// reports the key under, since that is the only part of the sentence that differs
/// between those operations.
pub fn tag_key_violation(field: &str, key: &str) -> Option<String> {
    if key.is_empty() {
        return Some(format!(
            "Value '' at '{field}' failed to satisfy constraint: A tag key must not be empty"
        ));
    }
    if key.to_ascii_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Some(format!(
            "Value '{key}' at '{field}' failed to satisfy constraint: Tag keys must not \
             begin with the reserved prefix '{RESERVED_TAG_PREFIX}'"
        ));
    }
    if key.chars().count() > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Value '{key}' at '{field}' failed to satisfy constraint: Tag keys must have \
             length less than or equal to {MAX_TAG_KEY_LENGTH}"
        ));
    }
    None
}

/// Why a tag map cannot be applied to a resource that already carries `existing`, or
/// `None` when every tag in it is acceptable.
///
/// The rules are the AWS-wide tagging ones ([`RESERVED_TAG_PREFIX`],
/// [`MAX_TAG_KEY_LENGTH`], [`MAX_TAG_VALUE_LENGTH`], [`MAX_TAGS`]); the model constrains
/// `TagMap` with none of them, so they are enforced here — the key half through
/// [`tag_key_violation`], which the removal path enforces too. The keys are walked in
/// sorted order, so one request always gets one answer.
pub fn tag_violation(
    tags: &HashMap<String, String>,
    existing: &BTreeMap<String, String>,
) -> Option<String> {
    let sorted: BTreeMap<&str, &str> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    for (key, value) in &sorted {
        if let Some(violation) = tag_key_violation(TAGS_INPUT_MEMBER, key) {
            return Some(violation);
        }
        if value.chars().count() > MAX_TAG_VALUE_LENGTH {
            return Some(format!(
                "Value '{value}' at 'tags' failed to satisfy constraint: Tag values must \
                 have length less than or equal to {MAX_TAG_VALUE_LENGTH}"
            ));
        }
    }
    // What the resource would end up carrying: its own keys the request does not
    // replace, plus every key the request names.
    let merged = existing
        .keys()
        .filter(|key| !tags.contains_key(key.as_str()))
        .count()
        + tags.len();
    if merged > MAX_TAGS {
        return Some(format!(
            "Value '{merged} tags' at 'tags' failed to satisfy constraint: A resource must \
             not carry more than {MAX_TAGS} tags"
        ));
    }
    None
}

/// The `ValidationException` message a request whose tags break [`tag_violation`] or
/// whose tag key breaks [`tag_key_violation`] is refused with.
///
/// AWS's validation refusals count the failed constraints and then state each one, so a
/// single unacceptable tag reads `1 validation error detected: <the violation>`. Every
/// operation that names a tag — a whole `TagMap` or just its keys — builds its message
/// here, so the same unacceptable tag is answered the same way whichever one carried it.
pub fn tag_validation_message(violation: &str) -> String {
    format!("1 validation error detected: {violation}")
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

/// How this service words the account/region device precondition it refuses a
/// device or task call with.
///
/// ONE sentence for the whole class: it states that the signed Region holds no
/// Snow-Device-Management-activated device, echoes nothing a caller sent, and names
/// no principal and no permission — the refusal is about device availability in the
/// Region, not about the caller's authorization, which is why it is kept apart from
/// [`access_denied_message`].
pub const NO_DEVICES_IN_REGION_MESSAGE: &str = "No devices found in this region.";

/// The `AccessDeniedException` payload every gated operation refuses a Region this
/// account has activated no device in with.
///
/// Built once here so all ten of them answer the identical code and message,
/// whatever the request named: the refusal is decided before an id is resolved and
/// before a field is validated, so nothing about the request can reach it.
pub fn no_devices_in_region() -> AccessDeniedException {
    AccessDeniedException {
        message: NO_DEVICES_IN_REGION_MESSAGE.to_string(),
    }
}

/// The IAM user name of the identity this emulator's long-term credentials speak
/// for.
///
/// The emulator publishes exactly ONE long-term identity, and STS is what defines
/// it: `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every
/// request whose access key id names no session (`AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`).
const AMBIENT_USER_NAME: &str = "emulator";

/// The principal that made this request, as an ARN.
///
/// Server-derived from the emulator's own identity contract, never from the
/// request: it is the ARN of the identity a request signed with this emulator's
/// credentials speaks for, the one [`AMBIENT_USER_NAME`] names. An access key id is
/// a credential id, not a user name, so it is never rendered into an IAM ARN.
pub fn request_principal() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"))
}

/// How `ListTagsForResource` words its refusal of an ARN outside the account this
/// emulator serves.
///
/// It names the CALLER's own principal — the identity whose authorization was
/// evaluated — and names neither the ARN it was asked about, nor its account, nor any
/// resource: the refusal is decided before anything is looked up, so a caller must not
/// be able to learn from it whether a resource exists in an account that is not its
/// own. This is the wording the behavioural corpus recorded for the tag READ
/// (`snow-device-management.cross-account-tag-arn-notfound-no-leak`); the mutating
/// tagging operations carry their own recorded wording, which is why this is not one
/// shared constant.
pub fn access_denied_message() -> String {
    format!(
        "User: {} is not authorized to access this resource",
        request_principal()
    )
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
/// caller's record is keyed by, whatever region the ARN names, because a device id
/// and a task id are unique across the account's regions.
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
