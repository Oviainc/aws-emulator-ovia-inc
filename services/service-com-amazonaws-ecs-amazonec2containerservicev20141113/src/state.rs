// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecs-amazonec2containerservicev20141113 service.
//!
//! The ECS task lifecycle, held in one place: clusters, task-definition revisions and
//! tasks. Every handler computes its response from what is stored here — the cluster
//! counters (`runningTasksCount`, `pendingTasksCount`) are DERIVED from the task map
//! rather than being bumped by whoever happened to call, so they cannot drift from the
//! tasks that actually exist.
//!
//! Resource ids are minted from monotonic counters held here, so every ARN this
//! service emits is both unique and recorded against the record it names.
//!
//! ## The clock-owed transitions
//!
//! `RunTask` answers with a task in `PENDING` — the status AWS reports for a task that
//! has been accepted but has not started yet (the `Task$createdAt` doc: "the time when
//! the task entered the `PENDING` state"). The move to `RUNNING` is owed by the CLOCK,
//! not by whoever reads next: [`TaskRecord::running_due_at`] records the instant it
//! falls due and [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]
//! materialises it. `StopTask` is the same shape in reverse: the task goes to
//! `DEACTIVATING` with `stoppingAt` set (the `Task$stoppingAt` doc: "the time when the
//! task transitions from the `RUNNING` state to `STOPPING`") and reaches `STOPPED`
//! [`TRANSITION_SPAN`] later. Nothing sleeps: the due instant is stored and compared.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CapacityProviderStrategyItem, Cluster, ClusterConfiguration, ClusterSetting, Container,
    ContainerDefinition, EphemeralStorage, Failure, KeyValuePair, ProxyConfiguration,
    RuntimePlatform, Tag, Task, TaskDefinition, TaskDefinitionPlacementConstraint, TaskOverride,
    Volume,
};

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as the model's
/// `aws.api#service` trait records it (`arnNamespace: "ecs"`).
pub const ARN_NAMESPACE: &str = "ecs";

/// The cluster an operation acts on when the request names none. The `CreateCluster`
/// `clusterName` doc: "If you don't specify a name for your cluster, you create a
/// cluster that's named default"; every other operation's `cluster` doc: "If you do not
/// specify a cluster, the default cluster is assumed."
pub const DEFAULT_CLUSTER_NAME: &str = "default";

/// How long a task takes to cross one lifecycle boundary (`PENDING` -> `RUNNING`,
/// `DEACTIVATING` -> `STOPPED`). EMULATOR-CHOSEN — AWS publishes no duration for it —
/// and deliberately non-zero so a caller that reads straight back still observes the
/// transient status the real service reports.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The most tasks one `RunTask` call may place. The `RunTask` `count` doc: "You can
/// specify up to 10 tasks for each call."
pub const MAX_RUN_TASK_COUNT: i32 = 10;

/// `ListTasks` page size when the caller gives no `maxResults`. The member's doc: "This
/// value can be between 1 and 100. If this parameter isn't used, then ListTasks returns
/// up to 100 results".
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// The largest `maxResults` `ListTasks` accepts, from the same doc sentence.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many containers one task definition may describe. The Amazon ECS service quota
/// "Containers per task definition" is 10, and the cap is also what bounds the work and
/// the stored bytes ONE `RegisterTaskDefinition` can ask for — and, through it, the
/// containers every task of that revision mints and every describe renders.
pub const MAX_CONTAINER_DEFINITIONS: usize = 10;

/// How many ids one describe request may name. Amazon ECS accepts at most 100 per
/// `DescribeClusters`/`DescribeTasks` call, and the cap is also what bounds the work
/// and the response bytes ONE request can ask this emulator for while it holds the
/// store lock: without it a caller could repeat a single id until the request-body cap
/// and multiply it into an arbitrarily large response.
pub const MAX_DESCRIBE_BATCH: usize = 100;

/// The longest a cluster name or a task-definition family may be. The `clusterName` and
/// `family` docs both say "Up to 255 letters (uppercase and lowercase), numbers,
/// underscores, and hyphens are allowed."
pub const MAX_NAME_LENGTH: usize = 255;

/// `stoppedReason` when `StopTask` is called without a `reason` — the wording Amazon ECS
/// records for an operator-initiated stop.
pub const DEFAULT_STOPPED_REASON: &str = "Task stopped by user";

/// How many clusters this emulator holds. EMULATOR-CHOSEN, so a caller cannot grow the
/// in-memory store without limit.
pub const MAX_CLUSTERS: usize = 64;

/// How many task-definition revisions this emulator holds, across every family.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`].
pub const MAX_TASK_DEFINITIONS: usize = 512;

/// How many tasks this emulator holds, across every cluster. EMULATOR-CHOSEN, for the
/// same reason as [`MAX_CLUSTERS`].
pub const MAX_TASKS: usize = 512;

/// The statuses a task's `lastStatus` takes in this emulator. `PENDING` and `RUNNING`
/// and `STOPPED` are the ECS task-lifecycle statuses; `DEACTIVATING` is the first
/// lifecycle status a stopping task takes (`Task$stoppingAt`: "the time when the task
/// transitions from the RUNNING state to STOPPING").
pub const STATUS_PENDING: &str = "PENDING";
/// See [`STATUS_PENDING`].
pub const STATUS_RUNNING: &str = "RUNNING";
/// See [`STATUS_PENDING`].
pub const STATUS_DEACTIVATING: &str = "DEACTIVATING";
/// See [`STATUS_PENDING`].
pub const STATUS_STOPPED: &str = "STOPPED";

/// `Cluster$status` for a live cluster.
pub const CLUSTER_ACTIVE: &str = "ACTIVE";
/// `Cluster$status` for a deleted cluster: "The cluster has been deleted."
pub const CLUSTER_INACTIVE: &str = "INACTIVE";

/// `TaskDefinition$status` for a registered revision.
pub const TASK_DEFINITION_ACTIVE: &str = "ACTIVE";

/// `Failure$reason` for an id that names nothing in this account. The ECS "API failure
/// reasons" list: `MISSING` — "the described resource could not be found".
pub const FAILURE_MISSING: &str = "MISSING";

/// The launch type a task takes when the request names neither a `launchType` nor a
/// `capacityProviderStrategy` and the cluster has no default strategy: ECS runs the
/// task on the EC2 capacity registered to the cluster.
pub const DEFAULT_LAUNCH_TYPE: &str = "EC2";

/// `stopCode` for a stop the caller asked for, from the `TaskStopCode` enum.
pub const STOP_CODE_USER_INITIATED: &str = "UserInitiated";

/// `healthStatus` this emulator reports. The `Container$healthStatus` doc: "If health
/// checks aren't configured for this container in its task definition, then it reports
/// the health status as UNKNOWN." Nothing executes a container here, so no container
/// ever reports a health result and the status stays `UNKNOWN`.
pub const HEALTH_UNKNOWN: &str = "UNKNOWN";

/// A cluster, as `CreateCluster` left it.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub tags: Vec<Tag>,
    pub settings: Vec<ClusterSetting>,
    pub configuration: Option<ClusterConfiguration>,
    pub capacity_providers: Vec<String>,
    pub default_capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
}

/// One container of a task, derived from the container definition the task's
/// task-definition revision stored.
#[derive(Debug, Clone)]
pub struct ContainerRecord {
    pub container_arn: String,
    pub task_arn: String,
    pub name: Option<String>,
    pub image: Option<String>,
    pub last_status: String,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub memory_reservation: Option<String>,
}

impl ContainerRecord {
    /// The `Container` every task-returning operation reports, built from what is
    /// stored.
    pub fn view(&self) -> Container {
        Container {
            container_arn: Some(self.container_arn.clone()),
            task_arn: Some(self.task_arn.clone()),
            name: self.name.clone(),
            image: self.image.clone(),
            last_status: Some(self.last_status.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            memory_reservation: self.memory_reservation.clone(),
            health_status: Some(HEALTH_UNKNOWN.to_string()),
            ..Container::default()
        }
    }
}

/// A task-definition revision, as `RegisterTaskDefinition` stored it.
#[derive(Debug, Clone)]
pub struct TaskDefinitionRecord {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub container_definitions: Vec<ContainerDefinition>,
    pub network_mode: String,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub volumes: Vec<Volume>,
    pub placement_constraints: Vec<TaskDefinitionPlacementConstraint>,
    pub requires_compatibilities: Vec<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub proxy_configuration: Option<ProxyConfiguration>,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub runtime_platform: Option<RuntimePlatform>,
    pub enable_fault_injection: Option<bool>,
    pub tags: Vec<Tag>,
    pub registered_at: DateTime,
    pub deregistered_at: Option<DateTime>,
}

impl TaskDefinitionRecord {
    /// The launch types this revision was validated against — `TaskDefinition$
    /// compatibilities`: "Amazon ECS validates the task definition parameters with
    /// those supported by the launch type."
    ///
    /// Derived from the STORED revision, by the two rules the model's own docs state:
    /// Fargate needs `awsvpc` ("For Amazon ECS tasks on Fargate, the awsvpc network
    /// mode is required") plus task-level `cpu` and `memory` ("This field is required
    /// for Fargate"), and `awsvpc` is not available to the external (on-premises)
    /// launch type. EC2 supports every network mode, so it is always present.
    pub fn compatibilities(&self) -> Vec<String> {
        let awsvpc = self.network_mode == "awsvpc";
        let mut out = vec![DEFAULT_LAUNCH_TYPE.to_string()];
        if awsvpc && self.cpu.is_some() && self.memory.is_some() {
            out.push("FARGATE".to_string());
        }
        if !awsvpc {
            out.push("EXTERNAL".to_string());
        }
        out
    }

    /// The `TaskDefinition` every task-definition operation returns, built from what is
    /// stored — never from the request asking for it.
    pub fn view(&self) -> TaskDefinition {
        TaskDefinition {
            task_definition_arn: Some(self.arn.clone()),
            container_definitions: Some(self.container_definitions.clone()),
            family: Some(self.family.clone()),
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            network_mode: Some(self.network_mode.clone()),
            revision: self.revision,
            volumes: Some(self.volumes.clone()),
            status: Some(self.status.clone()),
            // `requiresAttributes` reports the container-instance attributes a task
            // needs. Container-instance registration is not implemented in this slice,
            // so there is no attribute set to report and it stays absent.
            requires_attributes: None,
            placement_constraints: Some(self.placement_constraints.clone()),
            compatibilities: Some(self.compatibilities()),
            runtime_platform: self.runtime_platform.clone(),
            requires_compatibilities: Some(self.requires_compatibilities.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            inference_accelerators: None,
            pid_mode: self.pid_mode.clone(),
            ipc_mode: self.ipc_mode.clone(),
            proxy_configuration: self.proxy_configuration.clone(),
            registered_at: Some(self.registered_at),
            deregistered_at: self.deregistered_at,
            delete_requested_at: None,
            registered_by: None,
            ephemeral_storage: self.ephemeral_storage.clone(),
            enable_fault_injection: self.enable_fault_injection,
        }
    }
}

/// A task, as `RunTask` created it and `StopTask`/the clock left it.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_arn: String,
    /// The task id: the last ARN segment, and what a caller may name instead of the ARN.
    pub task_id: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub task_definition_arn: String,
    pub family: String,
    pub last_status: String,
    pub desired_status: String,
    pub containers: Vec<ContainerRecord>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub overrides: Option<TaskOverride>,
    pub launch_type: Option<String>,
    pub capacity_provider_name: Option<String>,
    pub platform_version: Option<String>,
    pub started_by: Option<String>,
    pub group: String,
    pub tags: Vec<Tag>,
    pub enable_execute_command: bool,
    pub created_at: DateTime,
    pub started_at: Option<DateTime>,
    pub stopping_at: Option<DateTime>,
    pub stopped_at: Option<DateTime>,
    pub execution_stopped_at: Option<DateTime>,
    pub stopped_reason: Option<String>,
    pub stop_code: Option<String>,
    pub version: i64,
    /// When `PENDING` -> `RUNNING` falls due; `None` once it has been materialised.
    pub running_due_at: Option<SystemTime>,
    /// When `DEACTIVATING` -> `STOPPED` falls due; `None` once materialised.
    pub stopped_due_at: Option<SystemTime>,
}

impl TaskRecord {
    /// The `Task` every task-returning operation reports, built from what is stored.
    ///
    /// `include_tags` follows `DescribeTasks$include`: "Specifies whether you want to
    /// see the resource tags for the task. If TAGS is specified, the tags are included
    /// in the response. If this field is omitted, tags aren't included."
    pub fn view(&self, include_tags: bool) -> Task {
        Task {
            cluster_arn: Some(self.cluster_arn.clone()),
            containers: Some(self.containers.iter().map(ContainerRecord::view).collect()),
            cpu: self.cpu.clone(),
            created_at: Some(self.created_at),
            desired_status: Some(self.desired_status.clone()),
            enable_execute_command: self.enable_execute_command,
            execution_stopped_at: self.execution_stopped_at,
            group: Some(self.group.clone()),
            health_status: Some(HEALTH_UNKNOWN.to_string()),
            last_status: Some(self.last_status.clone()),
            launch_type: self.launch_type.clone(),
            capacity_provider_name: self.capacity_provider_name.clone(),
            platform_version: self.platform_version.clone(),
            memory: self.memory.clone(),
            overrides: self.overrides.clone(),
            started_at: self.started_at,
            started_by: self.started_by.clone(),
            stop_code: self.stop_code.clone(),
            stopped_at: self.stopped_at,
            stopped_reason: self.stopped_reason.clone(),
            stopping_at: self.stopping_at,
            tags: if include_tags {
                Some(self.tags.clone())
            } else {
                None
            },
            task_arn: Some(self.task_arn.clone()),
            task_definition_arn: Some(self.task_definition_arn.clone()),
            version: self.version,
            ..Task::default()
        }
    }

    /// Whether this task still occupies its cluster: anything that has not reached
    /// `STOPPED` yet.
    pub fn is_active(&self) -> bool {
        self.last_status != STATUS_STOPPED
    }
}

/// Per-service state for com-amazonaws-ecs-amazonec2containerservicev20141113. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// `clusterName` -> the cluster.
    pub clusters: HashMap<String, ClusterRecord>,
    /// `(family, revision)` -> the registered revision.
    pub task_definitions: HashMap<(String, i32), TaskDefinitionRecord>,
    /// `family` -> the revision the next `RegisterTaskDefinition` for it will take.
    /// Kept beside `task_definitions` so a revision number is never reused, even after
    /// a revision is deregistered (`TaskDefinition$revision`: "the revision value
    /// always increases by one. This is even if you deregistered previous revisions").
    pub next_revision: HashMap<String, i32>,
    /// `taskArn` -> the task.
    pub tasks: HashMap<String, TaskRecord>,
    /// Monotonic source of task ids, so every task ARN is distinct.
    next_task_seq: u64,
    /// Monotonic source of container ids, so every container ARN is distinct.
    next_container_seq: u64,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Materialise every lifecycle transition the clock has already made due, as of
    /// `now`. Safe to call from a reporting operation: it only applies changes that
    /// were owed at this instant whether or not anybody called.
    pub fn advance_clock(&mut self, now: SystemTime) {
        for task in self.tasks.values_mut() {
            if let Some(due) = task.running_due_at {
                if due <= now {
                    task.running_due_at = None;
                    task.last_status = STATUS_RUNNING.to_string();
                    task.started_at = Some(DateTime::from(due));
                    for container in &mut task.containers {
                        container.last_status = STATUS_RUNNING.to_string();
                    }
                    task.version += 1;
                }
            }
            if let Some(due) = task.stopped_due_at {
                if due <= now {
                    task.stopped_due_at = None;
                    // A task that was still PENDING when it was stopped never ran, so
                    // it gets no startedAt — only the stop timestamps.
                    task.running_due_at = None;
                    task.last_status = STATUS_STOPPED.to_string();
                    task.stopped_at = Some(DateTime::from(due));
                    task.execution_stopped_at = Some(DateTime::from(due));
                    for container in &mut task.containers {
                        container.last_status = STATUS_STOPPED.to_string();
                    }
                    task.version += 1;
                }
            }
        }
    }

    /// Mint the next task id (32 lowercase hex digits, the shape of an ECS task id).
    pub fn next_task_id(&mut self) -> String {
        self.next_task_seq += 1;
        format!("{:032x}", self.next_task_seq)
    }

    /// Mint the next container id (32 lowercase hex digits, the shape of an ECS
    /// container id).
    pub fn next_container_id(&mut self) -> String {
        self.next_container_seq += 1;
        format!("{:032x}", self.next_container_seq)
    }

    /// The latest `ACTIVE` revision registered for `family`, if any.
    pub fn latest_active_revision(&self, family: &str) -> Option<&TaskDefinitionRecord> {
        self.task_definitions
            .values()
            .filter(|record| record.family == family && record.status == TASK_DEFINITION_ACTIVE)
            .max_by_key(|record| record.revision)
    }

    /// Count every number a described cluster reports in ONE pass over the task map,
    /// so describing a cluster costs one scan rather than one scan per reported number.
    fn tally(&self, cluster_name: &str) -> TaskTally {
        let mut tally = TaskTally::default();
        for task in self
            .tasks
            .values()
            .filter(|task| task.cluster_name == cluster_name)
        {
            let launch_type = task.launch_type.as_deref();
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
                match launch_type {
                    Some("EC2") => tally.running_ec2 += 1,
                    Some("FARGATE") => tally.running_fargate += 1,
                    _ => {}
                }
            } else if task.last_status == STATUS_PENDING {
                tally.pending += 1;
                match launch_type {
                    Some("EC2") => tally.pending_ec2 += 1,
                    Some("FARGATE") => tally.pending_fargate += 1,
                    _ => {}
                }
            }
        }
        tally
    }

    /// Whether `cluster_name` still holds a task that has not stopped.
    pub fn cluster_has_active_tasks(&self, cluster_name: &str) -> bool {
        self.tasks
            .values()
            .any(|task| task.cluster_name == cluster_name && task.is_active())
    }

    /// The `Cluster` a cluster-returning operation reports.
    ///
    /// The counters are COMPUTED from the task map, so they always agree with the tasks
    /// that exist: `runningTasksCount` is "the number of tasks in the cluster that are
    /// in the RUNNING state" and `pendingTasksCount` the same for `PENDING`. Container
    /// instances and services are not part of this slice — nothing registers one — so
    /// their counts are a true zero rather than an invented number.
    ///
    /// `fields` follows `DescribeClusters$include`: "Determines whether to include
    /// additional information about the clusters in the response. If this field is
    /// omitted, this information isn't included."
    pub fn cluster_view(&self, cluster: &ClusterRecord, fields: ClusterFields) -> Cluster {
        let tally = self.tally(&cluster.name);
        Cluster {
            cluster_arn: Some(cluster.arn.clone()),
            cluster_name: Some(cluster.name.clone()),
            status: Some(cluster.status.clone()),
            registered_container_instances_count: 0,
            running_tasks_count: i32::try_from(tally.running).unwrap_or(i32::MAX),
            pending_tasks_count: i32::try_from(tally.pending).unwrap_or(i32::MAX),
            active_services_count: 0,
            capacity_providers: Some(cluster.capacity_providers.clone()),
            default_capacity_provider_strategy: Some(
                cluster.default_capacity_provider_strategy.clone(),
            ),
            configuration: if fields.configurations {
                cluster.configuration.clone()
            } else {
                None
            },
            settings: if fields.settings {
                Some(cluster.settings.clone())
            } else {
                None
            },
            tags: if fields.tags {
                Some(cluster.tags.clone())
            } else {
                None
            },
            statistics: if fields.statistics {
                Some(cluster_statistics(&tally))
            } else {
                None
            },
            // Capacity-provider attachments are created by
            // PutClusterCapacityProviders, which is not part of this slice, so a
            // cluster here has none to report.
            attachments: if fields.attachments {
                Some(Vec::new())
            } else {
                None
            },
            attachments_status: None,
            service_connect_defaults: None,
        }
    }

}

/// What one pass over the task map counts for a cluster: the lifecycle counts the
/// `Cluster` members report, and the same numbers split by launch type that
/// `Cluster$statistics` reports.
#[derive(Debug, Clone, Copy, Default)]
struct TaskTally {
    running: usize,
    pending: usize,
    running_ec2: usize,
    running_fargate: usize,
    pending_ec2: usize,
    pending_fargate: usize,
}

/// `Cluster$statistics`: "the task and service count ... separated by launch type".
/// Read off the single pass in [`ComAmazonawsEcsAmazonec2containerservicev20141113State::tally`];
/// the service counts are zero because services are not part of this slice.
fn cluster_statistics(tally: &TaskTally) -> Vec<KeyValuePair> {
    vec![
        pair("runningEC2TasksCount", tally.running_ec2.to_string()),
        pair("runningFargateTasksCount", tally.running_fargate.to_string()),
        pair("pendingEC2TasksCount", tally.pending_ec2.to_string()),
        pair("pendingFargateTasksCount", tally.pending_fargate.to_string()),
        pair("activeEC2ServiceCount", "0".to_string()),
        pair("activeFargateServiceCount", "0".to_string()),
        pair("drainingEC2ServiceCount", "0".to_string()),
        pair("drainingFargateServiceCount", "0".to_string()),
    ]
}

/// One `KeyValuePair` of `Cluster$statistics`.
fn pair(name: &str, value: String) -> KeyValuePair {
    KeyValuePair {
        name: Some(name.to_string()),
        value: Some(value),
    }
}

/// Which optional cluster fields a response carries, as `DescribeClusters$include`
/// (the `ClusterField` enum: `ATTACHMENTS`, `CONFIGURATIONS`, `SETTINGS`, `STATISTICS`,
/// `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClusterFields {
    pub attachments: bool,
    pub configurations: bool,
    pub settings: bool,
    pub statistics: bool,
    pub tags: bool,
}

impl ClusterFields {
    /// Every optional field — what the operation that CREATED or MUTATED the cluster
    /// returns, since the caller just supplied them and `CreateCluster` has no
    /// `include` member to narrow them with.
    pub fn all() -> Self {
        Self {
            attachments: true,
            configurations: true,
            settings: true,
            statistics: true,
            tags: true,
        }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ClusterField` enum.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            match field.as_str() {
                "ATTACHMENTS" => fields.attachments = true,
                "CONFIGURATIONS" => fields.configurations = true,
                "SETTINGS" => fields.settings = true,
                "STATISTICS" => fields.statistics = true,
                "TAGS" => fields.tags = true,
                _ => {}
            }
        }
        fields
    }
}

/// A `Failure` for an id that names nothing: `reason: "MISSING"`.
pub fn missing_failure(arn: &str) -> Failure {
    Failure {
        arn: Some(arn.to_string()),
        reason: Some(FAILURE_MISSING.to_string()),
        detail: None,
    }
}

/// The cluster NAME a `cluster` request member refers to: the short name as given, or
/// the last segment of a cluster ARN. Every `cluster` member's doc: "The short name or
/// full Amazon Resource Name (ARN) of the cluster".
pub fn cluster_name_from_reference(reference: Option<&str>) -> String {
    let reference = reference
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CLUSTER_NAME);
    match reference.rsplit_once(":cluster/") {
        Some((_, name)) => name.to_string(),
        None => reference.to_string(),
    }
}

/// The task ARN a `task`/`tasks` entry refers to: the value as given when it is already
/// an ARN, otherwise the ARN a task of that id in `cluster_name` would carry. Every
/// such member's doc: "the task ID or full ARN entry of the task".
pub fn task_arn_from_reference(region: &str, cluster_name: &str, reference: &str) -> String {
    if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        task_arn(region, cluster_name, reference)
    }
}

/// The ARN of a cluster: `arn:aws:ecs:<region>:<account>:cluster/<name>`.
pub fn cluster_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster/{name}"))
}

/// The ARN of a task-definition revision:
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
pub fn task_definition_arn(region: &str, family: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("task-definition/{family}:{revision}"),
    )
}

/// The ARN of a task: `arn:aws:ecs:<region>:<account>:task/<cluster>/<id>`.
pub fn task_arn(region: &str, cluster_name: &str, task_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("task/{cluster_name}/{task_id}"),
    )
}

/// The ARN of a container:
/// `arn:aws:ecs:<region>:<account>:container/<cluster>/<task id>/<container id>`.
pub fn container_arn(
    region: &str,
    cluster_name: &str,
    task_id: &str,
    container_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("container/{cluster_name}/{task_id}/{container_id}"),
    )
}

/// Whether `name` is a legal cluster name or task-definition family: "Up to 255 letters
/// (uppercase and lowercase), numbers, underscores, and hyphens are allowed" — the
/// `CreateCluster$clusterName` and `RegisterTaskDefinition$family` docs.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LENGTH
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The `family` and optional `revision` a `taskDefinition` request member names.
///
/// The member's doc: "The family and revision (family:revision) or full ARN of the task
/// definition to run. If a revision isn't specified, the latest ACTIVE revision is
/// used." `None` when the value is not a task-definition reference at all.
pub fn parse_task_definition_reference(reference: &str) -> Option<(String, Option<i32>)> {
    let tail = match reference.strip_prefix("arn:") {
        Some(_) => reference.rsplit_once(":task-definition/")?.1,
        None => reference,
    };
    let (family, revision) = match tail.rsplit_once(':') {
        Some((family, revision)) => (family, Some(revision)),
        None => (tail, None),
    };
    if !is_valid_name(family) {
        return None;
    }
    match revision {
        None => Some((family.to_string(), None)),
        // `family:*` names every revision, which the doc calls out for IAM resources;
        // for a run it behaves as "no revision specified", i.e. the latest ACTIVE one.
        Some("*") => Some((family.to_string(), None)),
        Some(revision) => revision
            .parse::<i32>()
            .ok()
            .filter(|revision| *revision > 0)
            .map(|revision| (family.to_string(), Some(revision))),
    }
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcsAmazonec2containerservicev20141113State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcsAmazonec2containerservicev20141113State>)
}
