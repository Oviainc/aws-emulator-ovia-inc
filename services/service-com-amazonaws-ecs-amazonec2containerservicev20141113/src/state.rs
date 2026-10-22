// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecs-amazonec2containerservicev20141113 service.
//!
//! The ECS task lifecycle, held in one place: clusters, container instances,
//! task-definition revisions, services and tasks. Every handler computes its response
//! from what is stored here — the cluster counters (`runningTasksCount`,
//! `pendingTasksCount`, `registeredContainerInstancesCount`, `activeServicesCount`) and
//! a service's own (`runningCount`, `pendingCount`) are DERIVED from the task,
//! container-instance and service maps rather than being bumped by whoever happened to
//! call, so they cannot drift from the records that actually exist.
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
//!
//! A service's task set is placed by the CALL that changes what the service wants
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`], driven by
//! `CreateService` and `UpdateService`), and each task it places then crosses `PENDING`
//! -> `RUNNING` on the same clock. The scheduler's steady-state work beyond that —
//! replacing a task somebody else stopped, re-placing a daemon's task when a container
//! instance registers later — is not modelled: nothing runs between calls, and a
//! reporting operation may not place a task nobody asked for.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Attribute, CapacityProviderStrategyItem, Cluster, ClusterConfiguration, ClusterSetting,
    Container, ContainerDefinition, ContainerInstance, Deployment, DeploymentCircuitBreaker,
    DeploymentConfiguration, DeploymentController, EphemeralStorage, Failure, KeyValuePair,
    LoadBalancer, NetworkConfiguration, PlacementConstraint, PlacementStrategy, PlatformDevice,
    ProxyConfiguration, Resource, RuntimePlatform, Service, ServiceConnectConfiguration,
    ServiceCurrentRevisionSummary, ServiceEvent, ServiceRegistry, ServiceVolumeConfiguration, Tag,
    Task, TaskDefinition, TaskDefinitionPlacementConstraint, TaskOverride, ThresholdConfiguration,
    VersionInfo, Volume, VpcLatticeConfiguration, CLUSTER_SETTING_NAME_VALUES,
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

/// The value a cluster setting takes when neither the cluster nor the account set one.
/// `ClusterSetting$value`: "The supported values are `enhanced`, `enabled`, and
/// `disabled`. ... If a cluster value is specified, it will override the
/// `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault" —
/// and CloudWatch Container Insights is OFF until one of those turns it on, so the
/// account default a cluster inherits here is `disabled`.
///
/// TODO: `PutAccountSetting`/`PutAccountSettingDefault` are scaffold stubs (501), so
/// there is no account-settings store to read the inherited value from yet. When those
/// writes land — they are part of the related open issue on unpersisted ECS cluster
/// mutations — [`ClusterRecord::effective_settings`] must read the account default from
/// that store and fall back to this constant, or a cluster will keep reporting
/// `disabled` after an account default turned Container Insights on.
pub const CLUSTER_SETTING_DEFAULT_VALUE: &str = "disabled";

/// How long a task takes to cross one lifecycle boundary (`PENDING` -> `RUNNING`,
/// `DEACTIVATING` -> `STOPPED`). EMULATOR-CHOSEN — AWS publishes no duration for it —
/// and deliberately non-zero so a caller that reads straight back still observes the
/// transient status the real service reports.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The most tasks one `RunTask` call may place. The `RunTask` `count` doc: "You can
/// specify up to 10 tasks for each call."
pub const MAX_RUN_TASK_COUNT: i32 = 10;

/// A list operation's page size when the caller gives no `maxResults`. The
/// `ListTasks$maxResults` and `ListContainerInstances$maxResults` docs both say: "This
/// value can be between 1 and 100. If this parameter isn't used, then
/// <operation> returns up to 100 results".
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// The largest `maxResults` a list operation accepts, from the same doc sentence.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many containers one task definition may describe. The Amazon ECS service quota
/// "Containers per task definition" is 10, and the cap is also what bounds the work and
/// the stored bytes ONE `RegisterTaskDefinition` can ask for — and, through it, the
/// containers every task of that revision mints and every describe renders.
pub const MAX_CONTAINER_DEFINITIONS: usize = 10;

/// How many ids one describe request may name. Amazon ECS accepts at most 100 per
/// `DescribeClusters`/`DescribeTasks`/`DescribeContainerInstances` call, and the cap is
/// also what bounds the work
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

/// `ContainerInstance$status` for a registered instance: the member's doc, "The `ACTIVE`
/// status indicates that the container instance can accept tasks."
pub const CONTAINER_INSTANCE_ACTIVE: &str = "ACTIVE";
/// `ContainerInstance$status` for an instance taken out of service: "The `DRAINING`
/// indicates that new tasks aren't placed on the container instance".
pub const CONTAINER_INSTANCE_DRAINING: &str = "DRAINING";
/// `ContainerInstance$status` for a deregistered instance: "The instance then
/// transitions to an `INACTIVE` status." The status
/// `ListContainerInstances$status`'s doc excludes by default.
pub const CONTAINER_INSTANCE_INACTIVE: &str = "INACTIVE";

/// How many container instances this emulator holds, across every cluster.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`].
pub const MAX_CONTAINER_INSTANCES: usize = 512;

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

/// `Service$status` for a live service — the member's doc: "The valid values are
/// `ACTIVE`, `DRAINING`, or `INACTIVE`."
pub const SERVICE_ACTIVE: &str = "ACTIVE";
/// See [`SERVICE_ACTIVE`]: the status of a service that is being drained away.
pub const SERVICE_DRAINING: &str = "DRAINING";
/// See [`SERVICE_ACTIVE`]: the status of a service that has been deleted.
pub const SERVICE_INACTIVE: &str = "INACTIVE";

/// `Deployment$status` for the service's current deployment — the member's doc:
/// "`PRIMARY`: The most recent deployment of a service."
pub const DEPLOYMENT_PRIMARY: &str = "PRIMARY";

/// `Deployment$rolloutState` while the deployment is still placing tasks — the member's
/// doc: "When a service deployment is started, it begins in an `IN_PROGRESS` state."
pub const ROLLOUT_IN_PROGRESS: &str = "IN_PROGRESS";
/// `Deployment$rolloutState` once the service holds the tasks it wants — the same doc:
/// "When the service reaches a steady state, the deployment transitions to a
/// `COMPLETED` state."
pub const ROLLOUT_COMPLETED: &str = "COMPLETED";

/// `deploymentConfiguration.strategy` a service takes when the request names none. The
/// `CreateService` doc: "When you create a service which uses the ECS deployment
/// controller, you can choose between the following deployment strategies ... ROLLING:
/// ... the Amazon ECS service scheduler replaces the currently running tasks with new
/// tasks" — the rolling update is the ECS controller's own deployment strategy, and the
/// other three (`BLUE_GREEN`, `LINEAR`, `CANARY`) each need configuration the request
/// has to supply.
pub const DEPLOYMENT_STRATEGY_ROLLING: &str = "ROLLING";

/// `bakeTimeInMinutes` of a deployment configuration the request gave none for. The
/// member's doc: "The time period when both blue and green service revisions are running
/// simultaneously after the production traffic has shifted. You must provide this
/// parameter when you use the BLUE_GREEN deployment strategy" — a deployment with no
/// bake time configured bakes for no time at all.
pub const DEFAULT_BAKE_TIME_MINUTES: i32 = 0;

/// `thresholdConfiguration.type` of a deployment circuit breaker the request gave none
/// for. The `ThresholdConfiguration` doc: "By default, the threshold configuration uses
/// a type of BOUNDED_PERCENT with a value of 50."
pub const THRESHOLD_BOUNDED_PERCENT: &str = "BOUNDED_PERCENT";
/// See [`THRESHOLD_BOUNDED_PERCENT`]: the default `value` of that threshold.
pub const THRESHOLD_DEFAULT_VALUE: i32 = 50;
/// `resetOnHealthyTask` of a deployment circuit breaker the request gave none for. The
/// member's doc: "When set to true, a task that reaches a healthy state resets the
/// failure count to 0. ... The default is true."
pub const CIRCUIT_BREAKER_RESETS_ON_HEALTHY_TASK: bool = true;

/// `healthCheckGracePeriodSeconds` a CREATE takes when the request names none. The
/// member's doc: "If you do not specify a health check grace period value, the default
/// value of 0 is used."
pub const DEFAULT_HEALTH_CHECK_GRACE_PERIOD_SECONDS: i32 = 0;

/// The resource part of the ECS service-linked role's ARN. The `CreateService$role` doc:
/// "If your account has already created the Amazon ECS service-linked role, that role is
/// used for your service unless you specify a role here. The service-linked role is
/// required if ... the service is configured to use ... an external deployment
/// controller ... in which case you don't specify a role here."
pub const SERVICE_LINKED_ROLE_RESOURCE: &str =
    "role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS";

/// How many of a service's events a response carries. The `Service$events` doc: "The
/// event stream for your service. A maximum of 100 of the latest events are displayed."
pub const MAX_SERVICE_EVENTS: usize = 100;

/// The scheduling strategy a service takes when the request names none. The
/// `CreateService$desiredCount` doc — "This is required if schedulingStrategy is
/// `REPLICA` or isn't specified" — makes `REPLICA` the unspecified case.
pub const SCHEDULING_REPLICA: &str = "REPLICA";
/// See [`SCHEDULING_REPLICA`]: "The daemon scheduling strategy deploys exactly one task
/// on each active container instance".
pub const SCHEDULING_DAEMON: &str = "DAEMON";

/// The deployment controller a service takes when the request names none. The
/// `CreateService$deploymentController` doc: "If no deployment controller is specified,
/// the default value of ECS is used."
pub const DEPLOYMENT_CONTROLLER_ECS: &str = "ECS";

/// `propagateTags` when the request names none. The member's doc: "If no value is
/// specified, the tags aren't propagated. ... The default is NONE."
pub const PROPAGATE_TAGS_NONE: &str = "NONE";
/// See [`PROPAGATE_TAGS_NONE`]: propagate the SERVICE's tags to its tasks.
pub const PROPAGATE_TAGS_SERVICE: &str = "SERVICE";
/// See [`PROPAGATE_TAGS_NONE`]: propagate the task DEFINITION's tags to the task.
pub const PROPAGATE_TAGS_TASK_DEFINITION: &str = "TASK_DEFINITION";

/// `availabilityZoneRebalancing` a CREATE takes when the request names none — the
/// member's doc: "For create service requests, when no value is specified for
/// AvailabilityZoneRebalancing, Amazon ECS defaults the value to ENABLED."
pub const AZ_REBALANCING_ENABLED: &str = "ENABLED";

/// `maximumPercent` of a `REPLICA` service's deployment configuration when the request
/// supplies none. The member's doc: "The default maximumPercent value for a service
/// using the REPLICA service scheduler is 200%."
pub const REPLICA_MAXIMUM_PERCENT: i32 = 200;
/// `minimumHealthyPercent` of a `REPLICA` service's deployment configuration when the
/// request supplies none. The member's doc: "The default value for a replica service
/// for minimumHealthyPercent is 100%."
pub const REPLICA_MINIMUM_HEALTHY_PERCENT: i32 = 100;
/// `maximumPercent` of a `DAEMON` service: a daemon runs exactly one task per container
/// instance, so a deployment cannot exceed the desired count.
pub const DAEMON_MAXIMUM_PERCENT: i32 = 100;
/// `minimumHealthyPercent` of a `DAEMON` service's deployment configuration when the
/// request supplies none. The member's doc: "The default minimumHealthyPercent value
/// for a service using the DAEMON service schedule is 0% for the CLI, the Amazon Web
/// Services SDKs, and the APIs".
pub const DAEMON_MINIMUM_HEALTHY_PERCENT: i32 = 0;

/// `stopCode` for a task the service scheduler stopped (scaling in, or replacing the
/// task set of a superseded deployment), from the `TaskStopCode` enum.
pub const STOP_CODE_SERVICE_SCHEDULER: &str = "ServiceSchedulerInitiated";

/// `resourceManagementType` of a service a client created through `CreateService`. The
/// member's doc: "Identifies whether an ECS Service is an Express Service managed by
/// ECS, or managed by the customer. The valid values are ECS and CUSTOMER."
pub const RESOURCE_MANAGEMENT_CUSTOMER: &str = "CUSTOMER";

/// How many services this emulator holds, across every cluster. EMULATOR-CHOSEN, for
/// the same reason as [`MAX_CLUSTERS`].
pub const MAX_SERVICES: usize = 64;

/// The longest a `clientToken` may be: "Up to 36 ASCII characters in the range of
/// 33-126 (inclusive) are allowed" — the `CreateService$clientToken` doc.
pub const MAX_CLIENT_TOKEN_LENGTH: usize = 36;

/// How many services one `DescribeServices` request may name. The member's doc: "You
/// may specify up to 10 services to describe in a single operation."
pub const MAX_DESCRIBE_SERVICES: usize = 10;

// The message of a refusal whose condition is that a NAMED resource does not exist.
// Amazon ECS answers each of these with ONE fixed sentence that names no resource: the
// same text for every request that meets the condition, whatever cluster, service, task
// or task definition the request named. Every such refusal in this crate therefore
// carries the constant below rather than a sentence built from the request — only a
// refusal ECS derives from the request (a member that is blank, or outside its
// documented constraint) names a member.

/// `ClusterNotFoundException$message` for a `cluster` that names no stored cluster.
pub const CLUSTER_NOT_FOUND_MESSAGE: &str = "Cluster not found.";

/// `ServiceNotFoundException$message` for a `service` that names no stored service.
pub const SERVICE_NOT_FOUND_MESSAGE: &str = "Service not found.";

/// `InvalidParameterException$message` for a `task` that names no stored task.
pub const TASK_NOT_FOUND_MESSAGE: &str = "The referenced task was not found";

/// `ClientException$message` for a `taskDefinition` that names no stored revision — the
/// refusal every operation that resolves a task-definition reference answers with.
pub const TASK_DEFINITION_NOT_FOUND_MESSAGE: &str = "Unable to describe task definition.";

/// `InvalidParameterException$message` for a `containerInstance` reference that names no
/// instance registered in the cluster. No recording in the corpus fixes this sentence,
/// so the wording follows [`TASK_NOT_FOUND_MESSAGE`]; what the rule fixes is that the
/// sentence is the SAME for every request that meets the condition.
pub const CONTAINER_INSTANCE_NOT_FOUND_MESSAGE: &str =
    "The referenced container instance was not found";

/// `ClusterContainsServicesException$message`: the shape's own doc sentence, which names
/// no cluster (`smithy_query` query="shape" shape="ClusterContainsServicesException").
pub const CLUSTER_CONTAINS_SERVICES_MESSAGE: &str =
    "You can't delete a cluster that contains services.";

/// `ClusterContainsTasksException$message`: the shape's own doc sentence, which names no
/// cluster (`smithy_query` query="shape" shape="ClusterContainsTasksException").
pub const CLUSTER_CONTAINS_TASKS_MESSAGE: &str = "You can't delete a cluster that has active tasks.";

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

impl ClusterRecord {
    /// The cluster's EFFECTIVE settings: one entry for every setting name the model
    /// defines (`ClusterSettingName`), carrying the value stored for this cluster when
    /// one was set and the inherited account default ([`CLUSTER_SETTING_DEFAULT_VALUE`])
    /// otherwise. `Cluster$settings` is "the settings for the cluster. This parameter
    /// indicates whether CloudWatch Container Insights is on or off for a cluster", so a
    /// cluster that set nothing still has an answer to that question and reports it.
    pub fn effective_settings(&self) -> Vec<ClusterSetting> {
        CLUSTER_SETTING_NAME_VALUES
            .iter()
            .map(|name| ClusterSetting {
                name: Some((*name).to_string()),
                value: Some(
                    self.settings
                        .iter()
                        .find(|setting| setting.name.as_deref() == Some(*name))
                        .and_then(|setting| setting.value.clone())
                        .unwrap_or_else(|| CLUSTER_SETTING_DEFAULT_VALUE.to_string()),
                ),
            })
            .collect()
    }
}

/// A container instance, as `RegisterContainerInstance` left it: the EC2 or external
/// instance that runs the ECS agent and offers the cluster its capacity.
#[derive(Debug, Clone)]
pub struct ContainerInstanceRecord {
    pub arn: String,
    /// The container-instance id: the last ARN segment, and what a caller may name
    /// instead of the ARN.
    pub instance_id: String,
    pub cluster_name: String,
    pub ec2_instance_id: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub agent_connected: bool,
    pub version_info: Option<VersionInfo>,
    /// `totalResources`, as the agent reported it at registration —
    /// `ContainerInstance$registeredResources` is "the amount of each resource that was
    /// available on the container instance when the container agent registered it".
    pub registered_resources: Vec<Resource>,
    pub attributes: Vec<Attribute>,
    /// The GPU and Neuron devices the agent reported. Recorded because registration is
    /// what reports them; `ContainerInstance` models no member for them, so nothing
    /// renders them back.
    pub platform_devices: Vec<PlatformDevice>,
    pub tags: Vec<Tag>,
    pub registered_at: DateTime,
    pub version: i64,
}

impl ContainerInstanceRecord {
    /// The `ContainerInstance` every container-instance operation reports, built from
    /// what is stored.
    ///
    /// `remainingResources` is "the remaining CPU and memory that wasn't already
    /// allocated to tasks": no task in this emulator is placed on a container instance
    /// (a `TaskRecord` records no instance), so nothing is allocated out of the
    /// registered set and the remainder equals it — for the same reason
    /// `runningTasksCount` and `pendingTasksCount` are a true zero rather than an
    /// invented number.
    ///
    /// `healthStatus` stays absent: nothing runs an agent health check here, so there is
    /// no check result to report, and `agentUpdateStatus` likewise ("If an update wasn't
    /// ever requested, this value is NULL").
    pub fn view(&self, fields: ContainerInstanceFields) -> ContainerInstance {
        ContainerInstance {
            container_instance_arn: Some(self.arn.clone()),
            ec2_instance_id: Some(self.ec2_instance_id.clone()),
            capacity_provider_name: None,
            version: self.version,
            version_info: self.version_info.clone(),
            remaining_resources: Some(self.registered_resources.clone()),
            registered_resources: Some(self.registered_resources.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            agent_connected: self.agent_connected,
            running_tasks_count: 0,
            pending_tasks_count: 0,
            agent_update_status: None,
            attributes: Some(self.attributes.clone()),
            registered_at: Some(self.registered_at),
            // Attachments are created for a trunk elastic network interface, which this
            // emulator never provisions, so a registered instance has none.
            attachments: Some(Vec::new()),
            tags: if fields.tags {
                Some(self.tags.clone())
            } else {
                None
            },
            health_status: None,
        }
    }

    /// Whether this instance counts towards the cluster's registered capacity.
    /// `Cluster$registeredContainerInstancesCount`: "This includes container instances
    /// in both ACTIVE and DRAINING status."
    pub fn is_registered(&self) -> bool {
        self.status == CONTAINER_INSTANCE_ACTIVE || self.status == CONTAINER_INSTANCE_DRAINING
    }
}

/// Which optional container-instance fields a response carries, as
/// `DescribeContainerInstances$include` (the `ContainerInstanceField` enum: `TAGS`,
/// `CONTAINER_INSTANCE_HEALTH`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContainerInstanceFields {
    pub tags: bool,
}

impl ContainerInstanceFields {
    /// Every optional field — what the operation that REGISTERED the instance returns,
    /// since the caller just supplied them and `RegisterContainerInstance` has no
    /// `include` member to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ContainerInstanceField` enum.
    ///
    /// `CONTAINER_INSTANCE_HEALTH` selects nothing: no health check runs in this
    /// emulator, so `ContainerInstance$healthStatus` has no result to carry.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            if field == "TAGS" {
                fields.tags = true;
            }
        }
        fields
    }
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

/// The container-definition collection members a task-definition projection always
/// MATERIALISES: a registration that declared none of them reads back with each member
/// present as an EMPTY LIST, never with the key omitted, so a client may index them
/// unconditionally (`definition["environment"]` is `[]`, not a missing key).
///
/// Recorded ECS behavior, and what AWS's own published `DescribeTaskDefinition` example
/// response shows for a container that declares none of them (`portMappings`,
/// `environment`, `mountPoints`, `volumesFrom` each rendered as `[]`); `systemControls`
/// is the same shape. Members with no empty form are NOT invented: a structure such as
/// `logConfiguration` or a scalar such as `user` stays absent when it was not declared,
/// and a declared value is reported exactly as it was stored.
fn container_definition_view(stored: &ContainerDefinition) -> ContainerDefinition {
    ContainerDefinition {
        port_mappings: Some(stored.port_mappings.clone().unwrap_or_default()),
        environment: Some(stored.environment.clone().unwrap_or_default()),
        mount_points: Some(stored.mount_points.clone().unwrap_or_default()),
        volumes_from: Some(stored.volumes_from.clone().unwrap_or_default()),
        system_controls: Some(stored.system_controls.clone().unwrap_or_default()),
        ..stored.clone()
    }
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
    /// stored — never from the request asking for it. The container definitions go
    /// through [`container_definition_view`], so the collections ECS materialises come
    /// back as empty lists rather than as missing keys.
    pub fn view(&self) -> TaskDefinition {
        TaskDefinition {
            task_definition_arn: Some(self.arn.clone()),
            container_definitions: Some(
                self.container_definitions
                    .iter()
                    .map(container_definition_view)
                    .collect(),
            ),
            family: Some(self.family.clone()),
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            network_mode: Some(self.network_mode.clone()),
            revision: self.revision,
            volumes: Some(self.volumes.clone()),
            status: Some(self.status.clone()),
            // `requiresAttributes` reports the container-instance attributes a task
            // needs. They are derived from what the containers of the revision require
            // (a Docker volume driver, a logging driver, an agent version), which this
            // emulator does not infer, so there is no attribute set to report and it
            // stays absent.
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
    /// The service whose scheduler placed this task, if any. `None` for a standalone
    /// `RunTask`/`StartTask` task, so a service's counts are read off the tasks its own
    /// scheduler placed rather than off whatever shares its `group`.
    pub service_name: Option<String>,
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

/// An ECS service, as `CreateService` created it and `UpdateService` left it.
///
/// The record holds the service's CONFIGURATION only. Everything a response counts —
/// `runningCount`, `pendingCount`, the deployment's `rolloutState` — is derived from the
/// tasks its scheduler actually placed (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_view`]), so a
/// reported count can never drift from the task records that exist.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub arn: String,
    pub name: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub status: String,
    /// `Service$desiredCount`: "The desired number of instantiations of the task
    /// definition to keep running on the service."
    pub desired_count: i32,
    pub task_definition_arn: Option<String>,
    /// The family and revision the ARN above names, kept so the scheduler can read the
    /// revision back when it places a task.
    pub task_definition: Option<(String, i32)>,
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    pub role_arn: Option<String>,
    pub load_balancers: Vec<LoadBalancer>,
    pub service_registries: Vec<ServiceRegistry>,
    pub deployment_configuration: DeploymentConfiguration,
    pub deployment_controller: DeploymentController,
    pub placement_constraints: Vec<PlacementConstraint>,
    pub placement_strategy: Vec<PlacementStrategy>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub volume_configurations: Vec<ServiceVolumeConfiguration>,
    pub vpc_lattice_configurations: Vec<VpcLatticeConfiguration>,
    pub health_check_grace_period_seconds: Option<i32>,
    pub scheduling_strategy: String,
    pub propagate_tags: String,
    pub enable_ecs_managed_tags: bool,
    pub enable_execute_command: bool,
    pub availability_zone_rebalancing: String,
    pub tags: Vec<Tag>,
    /// `Service$createdBy`: "The principal that created the service" — the principal of
    /// the request that created it, resolved by [`caller_principal`].
    pub created_by: String,
    /// `Service$events`: "The event stream for your service", OLDEST first. The
    /// scheduler's own narrative: an entry is appended only when a transition the clock
    /// owed actually happened (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]), never
    /// because somebody read the service.
    pub events: Vec<ServiceEventRecord>,
    /// The `clientToken` the create carried, so repeating that create is idempotent
    /// ("An identifier that you provide to ensure the idempotency of the request").
    pub client_token: Option<String>,
    pub created_at: DateTime,
    /// The id of the service's current (`PRIMARY`) deployment. Every task the scheduler
    /// places for it carries this id as its `startedBy`, which is how a superseded
    /// deployment's tasks are told apart from the current one's.
    pub deployment_id: String,
    pub deployment_created_at: DateTime,
    pub deployment_updated_at: DateTime,
    /// The ARN of the service revision the current deployment rolls out, minted with
    /// that deployment. `ServiceCurrentRevisionSummary$arn`: "The ARN of the current
    /// service revision."
    pub service_revision_arn: String,
    /// When the current deployment's rollout is owed its steady state, or `None` once it
    /// has reached one. `Deployment$rolloutState`: "When a service deployment is started,
    /// it begins in an IN_PROGRESS state. When the service reaches a steady state, the
    /// deployment transitions to a COMPLETED state" — so a rollout is never COMPLETED in
    /// the same instant the call that started it answers; the CLOCK settles it.
    pub deployment_steady_due_at: Option<SystemTime>,
}

/// One entry of a service's event stream (`ServiceEvent`), as stored.
#[derive(Debug, Clone)]
pub struct ServiceEventRecord {
    pub id: String,
    pub created_at: DateTime,
    pub message: String,
}

impl ServiceEventRecord {
    /// The modelled shape a response carries for this event.
    pub fn view(&self) -> ServiceEvent {
        ServiceEvent {
            id: Some(self.id.clone()),
            created_at: Some(self.created_at),
            message: Some(self.message.clone()),
        }
    }
}

impl ServiceRecord {
    /// The deployment controller the service uses, defaulting to `ECS`.
    pub fn controller(&self) -> &str {
        self.deployment_controller.r#type.as_str()
    }

    /// Whether the ECS service scheduler places this service's tasks itself.
    ///
    /// Only the `ECS` deployment controller does: the `Service$taskSets` doc describes
    /// the other two as holding "a set of Amazon ECS tasks in either an CodeDeploy or an
    /// `EXTERNAL` deployment", whose tasks come from `CreateTaskSet` rather than from
    /// the service.
    pub fn places_tasks(&self) -> bool {
        self.controller() == DEPLOYMENT_CONTROLLER_ECS
    }

    /// Whether this service counts towards the cluster's `activeServicesCount`.
    pub fn is_active(&self) -> bool {
        self.status == SERVICE_ACTIVE
    }
}

/// Which optional service fields a response carries, as `DescribeServices$include`
/// (the `ServiceField` enum: `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceFields {
    pub tags: bool,
}

impl ServiceFields {
    /// Every optional field — what the operation that CREATED or MUTATED the service
    /// returns, since the caller just supplied them and `CreateService`/`UpdateService`
    /// have no `include` member to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ServiceField` enum.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            if field == "TAGS" {
                fields.tags = true;
            }
        }
        fields
    }
}

/// What [`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`]
/// answers when the placement it was asked for would push this emulator past
/// [`MAX_TASKS`]: nothing was changed, so the caller refuses the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskCapacityExceeded;

/// What one pass over the task map counts for ONE service.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceTally {
    pub running: i32,
    pub pending: i32,
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
    /// `containerInstanceArn` -> the registered container instance.
    pub container_instances: HashMap<String, ContainerInstanceRecord>,
    /// `(clusterName, serviceName)` -> the service. Service names are unique within a
    /// cluster ("Service names must be unique within a cluster, but you can have
    /// similarly named services in multiple clusters"), which is exactly this key.
    pub services: HashMap<(String, String), ServiceRecord>,
    /// Monotonic source of task ids, so every task ARN is distinct.
    next_task_seq: u64,
    /// Monotonic source of container ids, so every container ARN is distinct.
    next_container_seq: u64,
    /// Monotonic source of container-instance ids, so every container-instance ARN and
    /// every minted instance id is distinct.
    next_container_instance_seq: u64,
    /// Monotonic source of service-deployment ids, so every deployment of every service
    /// is distinct.
    next_deployment_seq: u64,
    /// Monotonic source of service-revision ids, so every revision of every service is
    /// distinct.
    next_service_revision_seq: u64,
    /// Monotonic source of service-event ids, so every event of every service is
    /// distinct.
    next_service_event_seq: u64,
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
        self.settle_services(now);
    }

    /// Materialise the SERVICE-level transitions the clock owes, as of `now`: the steady
    /// state a started rollout settles into, and the `INACTIVE` a drained service
    /// reaches once the tasks it was still cleaning up have stopped.
    ///
    /// Both were owed at this instant whether or not anybody called, so a reporting
    /// operation may run them — and each happens at most once, because the instant it
    /// settles is cleared with it.
    fn settle_services(&mut self, now: SystemTime) {
        let keys: Vec<(String, String)> = self.services.keys().cloned().collect();
        for key in keys {
            if !self.services.contains_key(&key) {
                continue;
            }
            // `Deployment$rolloutState`: the rollout completes "when the service reaches
            // a steady state" — which needs BOTH the instant it falls due and the task
            // set the service wants.
            let tally = self.service_tally(&key.0, &key.1);
            let service = &self.services[&key];
            let settled = service.deployment_steady_due_at.filter(|due| {
                *due <= now
                    && service.places_tasks()
                    && tally.running == service.desired_count
                    && tally.pending == 0
            });
            // `DeleteService`: "After all tasks have transitioned to either STOPPING or
            // STOPPED status, the service status moves from DRAINING to INACTIVE."
            let drained = service.status == SERVICE_DRAINING
                && !self.service_has_active_tasks(&key.0, &key.1);

            let events = match settled {
                // The two events the scheduler writes when a rollout reaches its steady
                // state, in the order it writes them. The wording follows the service
                // event messages Amazon ECS documents ("Amazon ECS service event
                // messages" in the Developer Guide).
                Some(due) => {
                    let name = service.name.clone();
                    let deployment_id = service.deployment_id.clone();
                    vec![
                        self.mint_service_event(
                            due,
                            format!(
                                "(service {name}) (deployment {deployment_id}) deployment \
                                 completed."
                            ),
                        ),
                        self.mint_service_event(
                            due,
                            format!("(service {name}) has reached a steady state."),
                        ),
                    ]
                }
                None => Vec::new(),
            };

            let Some(service) = self.services.get_mut(&key) else {
                continue;
            };
            if settled.is_some() {
                service.deployment_steady_due_at = None;
                service.events.extend(events);
                // "A maximum of 100 of the latest events are displayed", so only the
                // latest are kept at all.
                if service.events.len() > MAX_SERVICE_EVENTS {
                    let first_kept = service.events.len() - MAX_SERVICE_EVENTS;
                    service.events.drain(..first_kept);
                }
            }
            if drained {
                service.status = SERVICE_INACTIVE.to_string();
            }
        }
    }

    /// Mint one service event at `at`, with the next event id (32 lowercase hex digits,
    /// the shape every id this emulator mints takes). `ServiceEvent$id` is "The ID string
    /// of the event", with no documented format.
    fn mint_service_event(&mut self, at: SystemTime, message: String) -> ServiceEventRecord {
        self.next_service_event_seq += 1;
        ServiceEventRecord {
            id: format!("{:032x}", self.next_service_event_seq),
            created_at: DateTime::from(at),
            message,
        }
    }

    /// Whether the service still holds a task that has not reached `STOPPED`.
    pub fn service_has_active_tasks(&self, cluster_name: &str, service_name: &str) -> bool {
        self.tasks.values().any(|task| {
            task.cluster_name == cluster_name
                && task.service_name.as_deref() == Some(service_name)
                && task.is_active()
        })
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

    /// Mint the next container-instance id (32 lowercase hex digits, the shape of an ECS
    /// container-instance id).
    pub fn next_container_instance_id(&mut self) -> String {
        self.next_container_instance_seq += 1;
        format!("{:032x}", self.next_container_instance_seq)
    }

    /// Mint the managed-instance id an instance registered WITHOUT an EC2 instance
    /// identity document reports. `ContainerInstance$ec2InstanceId`: "For external
    /// instances, this value is the Amazon Web Services Systems Manager managed instance
    /// ID", which Systems Manager writes as `mi-` followed by 17 hex digits.
    pub fn next_managed_instance_id(&mut self) -> String {
        self.next_container_instance_seq += 1;
        format!("mi-{:017x}", self.next_container_instance_seq)
    }

    /// Mint the id of a service's next deployment. `Deployment$id` is an `ecs-svc/`
    /// identifier; the counter is what makes every deployment of every service distinct.
    pub fn next_deployment_id(&mut self) -> String {
        self.next_deployment_seq += 1;
        format!("ecs-svc/{:019}", self.next_deployment_seq)
    }

    /// Mint the ARN of the service revision a deployment rolls out. A service revision is
    /// "a version of your service that includes the task definition and configuration"
    /// and is created with the deployment that rolls it out, so the counter is what makes
    /// every revision of every service distinct.
    pub fn next_service_revision_arn(
        &mut self,
        region: &str,
        cluster_name: &str,
        service_name: &str,
    ) -> String {
        self.next_service_revision_seq += 1;
        service_revision_arn(
            region,
            cluster_name,
            service_name,
            &format!("{:019}", self.next_service_revision_seq),
        )
    }

    /// Mint the container records of ONE task of `definition`, each with its own ARN and
    /// id. Shared by every path that places a task — `RunTask` and the service
    /// scheduler — so a task's containers are derived from the stored revision the same
    /// way whoever placed it.
    pub fn mint_containers(
        &mut self,
        region: &str,
        cluster_name: &str,
        task_id: &str,
        task_arn: &str,
        definition: &TaskDefinitionRecord,
        last_status: &str,
    ) -> Vec<ContainerRecord> {
        definition
            .container_definitions
            .iter()
            .map(|container| {
                let container_id = self.next_container_id();
                ContainerRecord {
                    container_arn: container_arn(region, cluster_name, task_id, &container_id),
                    task_arn: task_arn.to_string(),
                    name: container.name.clone(),
                    image: container.image.clone(),
                    last_status: last_status.to_string(),
                    // `Container$cpu`: "The value is 0 if no value was specified in the
                    // container definition" — which is what the modelled non-optional
                    // `cpu` already holds.
                    cpu: Some(container.cpu.to_string()),
                    memory: container.memory.map(|memory| memory.to_string()),
                    memory_reservation: container
                        .memory_reservation
                        .map(|memory| memory.to_string()),
                }
            })
            .collect()
    }

    /// How many container instances `cluster_name` holds in a registered status.
    pub fn registered_container_instances(&self, cluster_name: &str) -> usize {
        self.container_instances
            .values()
            .filter(|instance| instance.cluster_name == cluster_name && instance.is_registered())
            .count()
    }

    /// The latest `ACTIVE` revision registered for `family`, if any.
    pub fn latest_active_revision(&self, family: &str) -> Option<&TaskDefinitionRecord> {
        self.task_definitions
            .values()
            .filter(|record| record.family == family && record.status == TASK_DEFINITION_ACTIVE)
            .max_by_key(|record| record.revision)
    }

    /// What the task map counts for ONE service: the tasks its scheduler placed and
    /// still wants, split by lifecycle status. `Service$runningCount` is "the number of
    /// tasks in the cluster that are in the RUNNING state" and `pendingCount` the same
    /// for `PENDING`; a task that has been asked to stop is no longer one of them.
    pub fn service_tally(&self, cluster_name: &str, service_name: &str) -> ServiceTally {
        let mut tally = ServiceTally::default();
        for task in self.tasks.values().filter(|task| {
            task.cluster_name == cluster_name
                && task.service_name.as_deref() == Some(service_name)
                && task.desired_status == STATUS_RUNNING
        }) {
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
            } else if task.last_status == STATUS_PENDING {
                tally.pending += 1;
            }
        }
        tally
    }

    /// The `Service` every service-returning operation reports, built from what is
    /// stored.
    ///
    /// `runningCount` and `pendingCount` are COUNTED off the task map, so they always
    /// agree with the tasks the scheduler actually placed, and the deployment's
    /// `rolloutState` follows from the steady state the CLOCK settled rather than being
    /// asserted.
    ///
    /// Which members a service carries follows its DEPLOYMENT CONTROLLER, not the
    /// operation that reports it: only the ECS controller's own scheduler owns a
    /// deployment and a service revision (`Deployment$rolloutState` is "only returned for
    /// services that use the rolling update (ECS) deployment type"), while the other
    /// controllers' tasks come from a task set (`CreateService$service`: "if the service
    /// uses the CODE_DEPLOY deployment controller, the deploymentController, taskSets and
    /// deployments parameters will be returned, however the deployments parameter will be
    /// an empty list"). `deploymentController` itself is reported for EVERY service: the
    /// recorded responses of the real service carry it for an ECS-controller service too,
    /// which is what the same doc's "will not be returned" sentence no longer matches.
    pub fn service_view(&self, service: &ServiceRecord, fields: ServiceFields) -> Service {
        let tally = self.service_tally(&service.cluster_name, &service.name);
        let scheduler_owned = service.places_tasks();
        let settled = service.deployment_steady_due_at.is_none();
        // "A service will return either a capacityProviderStrategy or launchType
        // parameter, but not both, depending where one was specified when it was
        // created."
        let (launch_type, capacity_provider_strategy) = match &service.launch_type {
            Some(launch_type) => (Some(launch_type.clone()), None),
            None => (None, Some(service.capacity_provider_strategy.clone())),
        };
        let deployment = Deployment {
            id: Some(service.deployment_id.clone()),
            status: Some(DEPLOYMENT_PRIMARY.to_string()),
            task_definition: service.task_definition_arn.clone(),
            desired_count: service.desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            // Nothing executes a container here, so no task of a deployment ever fails
            // to start: the count is a true zero rather than an invented number.
            failed_tasks: 0,
            created_at: Some(service.deployment_created_at),
            updated_at: Some(service.deployment_updated_at),
            capacity_provider_strategy: capacity_provider_strategy.clone(),
            launch_type: launch_type.clone(),
            platform_version: service.platform_version.clone(),
            platform_family: None,
            network_configuration: service.network_configuration.clone(),
            // "When a service deployment is started, it begins in an IN_PROGRESS state.
            // When the service reaches a steady state, the deployment transitions to a
            // COMPLETED state" — the steady state is the clock-owed transition the
            // record holds the due instant of, so a rollout the scheduler has not
            // settled yet is IN_PROGRESS whatever the counts already say.
            rollout_state: Some(
                if settled {
                    ROLLOUT_COMPLETED
                } else {
                    ROLLOUT_IN_PROGRESS
                }
                .to_string(),
            ),
            rollout_state_reason: Some(if settled {
                format!("ECS deployment {} completed.", service.deployment_id)
            } else {
                format!("ECS deployment {} in progress.", service.deployment_id)
            }),
            service_connect_configuration: service.service_connect_configuration.clone(),
            // A Service Connect resource is registered in Cloud Map, which this
            // emulator does not provision, so a deployment has none.
            service_connect_resources: Some(Vec::new()),
            volume_configurations: Some(service.volume_configurations.clone()),
            fargate_ephemeral_storage: None,
            vpc_lattice_configurations: Some(service.vpc_lattice_configurations.clone()),
        };
        Service {
            service_arn: Some(service.arn.clone()),
            service_name: Some(service.name.clone()),
            cluster_arn: Some(service.cluster_arn.clone()),
            load_balancers: Some(service.load_balancers.clone()),
            service_registries: Some(service.service_registries.clone()),
            status: Some(service.status.clone()),
            desired_count: service.desired_count,
            running_count: tally.running,
            pending_count: tally.pending,
            launch_type,
            capacity_provider_strategy,
            platform_version: service.platform_version.clone(),
            // `platformFamily` is "the operating system that your tasks in the service
            // run on ... specified only for tasks using the Fargate launch type", which
            // nothing here resolves, so it stays absent.
            platform_family: None,
            task_definition: service.task_definition_arn.clone(),
            deployment_configuration: Some(service.deployment_configuration.clone()),
            task_sets: if scheduler_owned {
                None
            } else {
                Some(Vec::new())
            },
            deployments: Some(if scheduler_owned {
                vec![deployment]
            } else {
                Vec::new()
            }),
            role_arn: service.role_arn.clone(),
            // "The event stream for your service. A maximum of 100 of the latest events
            // are displayed" — newest first, which is the order the service's own event
            // stream reads in.
            events: Some(
                service
                    .events
                    .iter()
                    .rev()
                    .take(MAX_SERVICE_EVENTS)
                    .map(ServiceEventRecord::view)
                    .collect(),
            ),
            created_at: Some(service.created_at),
            // The service DEPLOYMENT resource (`ListServiceDeployments` /
            // `DescribeServiceDeployments`) is not part of this slice, so there is no
            // deployment ARN to name; the service REVISION the current deployment rolls
            // out is minted with it.
            current_service_deployment: None,
            current_service_revisions: if scheduler_owned {
                Some(vec![ServiceCurrentRevisionSummary {
                    arn: Some(service.service_revision_arn.clone()),
                    requested_task_count: service.desired_count,
                    running_task_count: tally.running,
                    pending_task_count: tally.pending,
                }])
            } else {
                None
            },
            placement_constraints: Some(service.placement_constraints.clone()),
            placement_strategy: Some(service.placement_strategy.clone()),
            network_configuration: service.network_configuration.clone(),
            health_check_grace_period_seconds: service.health_check_grace_period_seconds,
            scheduling_strategy: Some(service.scheduling_strategy.clone()),
            deployment_controller: Some(service.deployment_controller.clone()),
            tags: if fields.tags {
                Some(service.tags.clone())
            } else {
                None
            },
            // "The principal that created the service": the principal of the request
            // that created it, as stored.
            created_by: Some(service.created_by.clone()),
            enable_ecs_managed_tags: service.enable_ecs_managed_tags,
            propagate_tags: Some(service.propagate_tags.clone()),
            enable_execute_command: service.enable_execute_command,
            availability_zone_rebalancing: Some(service.availability_zone_rebalancing.clone()),
            resource_management_type: Some(RESOURCE_MANAGEMENT_CUSTOMER.to_string()),
        }
    }

    /// Whether `cluster_name` still holds a service that has not gone `INACTIVE`.
    /// `DeleteCluster`'s `ClusterContainsServicesException`: "You can't delete a cluster
    /// that contains services."
    pub fn cluster_has_active_services(&self, cluster_name: &str) -> bool {
        self.services
            .values()
            .any(|service| service.cluster_name == cluster_name && service.status != SERVICE_INACTIVE)
    }

    /// Drain a DELETED service: it keeps nothing running and keeps no task set.
    ///
    /// `DeleteService`: "When you delete a service, if there are still running tasks that
    /// require cleanup, the service status moves from ACTIVE to DRAINING ... After all
    /// tasks have transitioned to either STOPPING or STOPPED status, the service status
    /// moves from DRAINING to INACTIVE." The record is KEPT either way: "Services in the
    /// DRAINING or INACTIVE status can still be viewed with the DescribeServices API
    /// operation."
    pub fn drain_service(&mut self, key: &(String, String), now: SystemTime) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let mut task_arns: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == service.cluster_name
                    && task.service_name.as_deref() == Some(service.name.as_str())
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        task_arns.sort();
        let name = service.name.clone();
        for arn in &task_arns {
            self.stop_service_task(arn, now, &format!("Service {name} was deleted"));
        }
        let still_cleaning_up = self.service_has_active_tasks(&key.0, &key.1);
        let Some(service) = self.services.get_mut(key) else {
            return;
        };
        service.desired_count = 0;
        service.status = if still_cleaning_up {
            SERVICE_DRAINING
        } else {
            SERVICE_INACTIVE
        }
        .to_string();
        // A deleted service has no rollout left to settle.
        service.deployment_steady_due_at = None;
    }

    /// Place or stop tasks so the service holds exactly the `desiredCount` tasks it
    /// wants, from `definition`.
    ///
    /// This is the ECS service scheduler's job — the `CreateService` doc: "Runs and
    /// maintains your desired number of tasks from a specified task definition" — and it
    /// runs on the call that CHANGES what the service wants, never on a read. A task of
    /// a superseded deployment is no longer wanted, so a new deployment replaces the
    /// task set it inherited.
    ///
    /// `Err` means the placement would push this emulator past [`MAX_TASKS`]; nothing is
    /// changed in that case.
    pub fn scale_service(
        &mut self,
        key: &(String, String),
        definition: &TaskDefinitionRecord,
        region: &str,
        now: SystemTime,
    ) -> Result<(), TaskCapacityExceeded> {
        let Some(service) = self.services.get(key).cloned() else {
            return Ok(());
        };
        if !service.places_tasks() {
            return Ok(());
        }

        // Task ids are minted from a fixed-width monotonic counter and the ARN ends with
        // the id, so sorting the ARNs orders the task set oldest first.
        let mut wanted: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == service.cluster_name
                    && task.service_name.as_deref() == Some(service.name.as_str())
                    && task.desired_status == STATUS_RUNNING
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        wanted.sort();

        let (superseded, live): (Vec<String>, Vec<String>) = wanted.into_iter().partition(|arn| {
            self.tasks
                .get(arn)
                .and_then(|task| task.started_by.clone())
                .as_deref()
                != Some(service.deployment_id.as_str())
        });
        let desired = usize::try_from(service.desired_count).unwrap_or(0);
        let to_place = desired.saturating_sub(live.len());
        // Nothing is changed when the placement would not fit: the cap is checked before
        // the first task is touched.
        if self.tasks.len() + to_place > MAX_TASKS {
            return Err(TaskCapacityExceeded);
        }
        for arn in &superseded {
            self.stop_service_task(
                arn,
                now,
                &format!("Task stopped by deployment {}", service.deployment_id),
            );
        }
        if live.len() > desired {
            // Scaling in stops the newest tasks of the deployment first: nothing here
            // runs on a container instance or in an Availability Zone, so there is no
            // placement balance to break the tie, and newest-first is deterministic.
            for arn in live[desired..].iter().rev() {
                self.stop_service_task(
                    arn,
                    now,
                    &format!(
                        "Scaling activity initiated by deployment {}",
                        service.deployment_id
                    ),
                );
            }
            return Ok(());
        }

        for _ in 0..to_place {
            self.place_service_task(&service, definition, region, now);
        }
        Ok(())
    }

    /// Place ONE task of `service`, `PENDING` with its start owed by the clock
    /// [`TRANSITION_SPAN`] from now, exactly as `RunTask` leaves a task it placed.
    fn place_service_task(
        &mut self,
        service: &ServiceRecord,
        definition: &TaskDefinitionRecord,
        region: &str,
        now: SystemTime,
    ) {
        let task_id = self.next_task_id();
        let task_arn = task_arn(region, &service.cluster_name, &task_id);
        let containers = self.mint_containers(
            region,
            &service.cluster_name,
            &task_id,
            &task_arn,
            definition,
            STATUS_PENDING,
        );
        // "Determines whether to propagate the tags from the task definition or the
        // service to the task. If no value is specified, the tags aren't propagated."
        let tags = match service.propagate_tags.as_str() {
            PROPAGATE_TAGS_SERVICE => service.tags.clone(),
            PROPAGATE_TAGS_TASK_DEFINITION => definition.tags.clone(),
            _ => Vec::new(),
        };
        let (launch_type, capacity_provider_name) = match &service.launch_type {
            Some(launch_type) => (Some(launch_type.clone()), None),
            None => (
                None,
                service
                    .capacity_provider_strategy
                    .first()
                    .map(|item| item.capacity_provider.clone()),
            ),
        };
        let record = TaskRecord {
            task_arn: task_arn.clone(),
            task_id,
            cluster_name: service.cluster_name.clone(),
            cluster_arn: service.cluster_arn.clone(),
            task_definition_arn: definition.arn.clone(),
            family: definition.family.clone(),
            last_status: STATUS_PENDING.to_string(),
            desired_status: STATUS_RUNNING.to_string(),
            containers,
            cpu: definition.cpu.clone(),
            memory: definition.memory.clone(),
            overrides: None,
            launch_type,
            capacity_provider_name,
            platform_version: service.platform_version.clone(),
            // `Task$startedBy`: for a task the service scheduler placed, the id of the
            // deployment that placed it.
            started_by: Some(service.deployment_id.clone()),
            // `Task$group`: "The name of the task group associated with the task", which
            // for a service's task is `service:<service name>`.
            group: format!("service:{}", service.name),
            service_name: Some(service.name.clone()),
            tags,
            enable_execute_command: service.enable_execute_command,
            created_at: DateTime::from(now),
            started_at: None,
            stopping_at: None,
            stopped_at: None,
            execution_stopped_at: None,
            stopped_reason: None,
            stop_code: None,
            version: 1,
            running_due_at: Some(now + TRANSITION_SPAN),
            stopped_due_at: None,
        };
        self.tasks.insert(task_arn, record);
    }

    /// Ask one of a service's tasks to stop, with `STOPPED` owed by the clock
    /// [`TRANSITION_SPAN`] from now — the same two-step stop `StopTask` records.
    fn stop_service_task(&mut self, task_arn: &str, now: SystemTime, reason: &str) {
        let Some(task) = self.tasks.get_mut(task_arn) else {
            return;
        };
        if task.desired_status == STATUS_STOPPED {
            return;
        }
        task.desired_status = STATUS_STOPPED.to_string();
        task.last_status = STATUS_DEACTIVATING.to_string();
        task.stopping_at = Some(DateTime::from(now));
        task.stopped_reason = Some(reason.to_string());
        task.stop_code = Some(STOP_CODE_SERVICE_SCHEDULER.to_string());
        task.stopped_due_at = Some(now + TRANSITION_SPAN);
        // A task asked to stop never reaches RUNNING: the start it was still waiting for
        // is cancelled, not merely overtaken.
        task.running_due_at = None;
        task.version += 1;
        for container in &mut task.containers {
            container.last_status = STATUS_DEACTIVATING.to_string();
        }
    }

    /// Count every number a described cluster reports in ONE pass over the task map and
    /// one over the service map, so describing a cluster costs one scan each rather than
    /// one scan per reported number.
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
        for service in self
            .services
            .values()
            .filter(|service| service.cluster_name == cluster_name)
        {
            let launch_type = service.launch_type.as_deref();
            if service.is_active() {
                tally.active_services += 1;
                match launch_type {
                    Some("EC2") => tally.active_ec2_services += 1,
                    Some("FARGATE") => tally.active_fargate_services += 1,
                    _ => {}
                }
            } else if service.status == SERVICE_DRAINING {
                match launch_type {
                    Some("EC2") => tally.draining_ec2_services += 1,
                    Some("FARGATE") => tally.draining_fargate_services += 1,
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
    /// The counters are COMPUTED from the task, container-instance and service maps, so
    /// they always agree with the records that exist: `runningTasksCount` is "the number
    /// of tasks in the cluster that are in the RUNNING state", `pendingTasksCount` the
    /// same for `PENDING`, and `activeServicesCount` the services the cluster holds in
    /// an `ACTIVE` status.
    ///
    /// `registeredContainerInstancesCount` is counted off the container-instance map the
    /// same way: "the number of container instances registered into the cluster. This
    /// includes container instances in both ACTIVE and DRAINING status."
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
            registered_container_instances_count: i32::try_from(
                self.registered_container_instances(&cluster.name),
            )
            .unwrap_or(i32::MAX),
            running_tasks_count: i32::try_from(tally.running).unwrap_or(i32::MAX),
            pending_tasks_count: i32::try_from(tally.pending).unwrap_or(i32::MAX),
            active_services_count: i32::try_from(tally.active_services).unwrap_or(i32::MAX),
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
                Some(cluster.effective_settings())
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

    /// The `Cluster` an operation that CREATES or DELETES a cluster reports.
    ///
    /// Such an operation carries no `include` member, and the halves of a `Cluster`
    /// answer that differently.
    ///
    /// What the cluster is CONFIGURED to be is always materialised: `settings` (the
    /// effective value of every setting name the model defines, see
    /// [`ClusterRecord::effective_settings`] — `Cluster$settings` "indicates whether
    /// CloudWatch Container Insights is on or off for a cluster", which is true of a
    /// cluster whoever reads it) and `configuration` ("the execute command and managed
    /// storage configuration for the cluster"), so a client can read back the cluster it
    /// just changed.
    ///
    /// `statistics` — "additional information about your clusters that are separated by
    /// launch type" — is COMPUTED only for a read that asks for it
    /// (`DescribeClusters$include` `STATISTICS`), so here it is an EMPTY list: emitting
    /// launch-type metrics nobody asked for hands metric-name-keyed client code values
    /// the service never reported.
    ///
    /// `tags` is materialised only where the ANSWERING OPERATION'S INPUT SHAPE mentions
    /// tags — the `tags` member a create is handed (`report_tags`), or the `TAGS` entry of
    /// a read's `include`. An operation whose input mentions tags nowhere, a delete,
    /// reports an empty list rather than the resource's tag set, so "no tags on this
    /// cluster" and "tags not requested" stay the same answer they are on AWS.
    pub fn cluster_mutation_view(&self, cluster: &ClusterRecord, report_tags: bool) -> Cluster {
        let mut view = self.cluster_view(
            cluster,
            ClusterFields {
                attachments: true,
                configurations: true,
                settings: true,
                statistics: false,
                tags: report_tags,
            },
        );
        view.statistics = Some(Vec::new());
        view.tags = Some(view.tags.unwrap_or_default());
        view
    }

    /// How many clusters the account holds — the top-level `clusterCount` a
    /// cluster-mutating response carries, read after the call has been applied. See
    /// [`ClusterCountEnvelope`] for why it is not a modelled member.
    pub fn cluster_count(&self) -> i32 {
        i32::try_from(self.clusters.len()).unwrap_or(i32::MAX)
    }
}

/// What one pass over the task map and one over the service map count for a cluster:
/// the lifecycle counts the `Cluster` members report, and the same numbers split by
/// launch type that `Cluster$statistics` reports.
#[derive(Debug, Clone, Copy, Default)]
struct TaskTally {
    running: usize,
    pending: usize,
    active_services: usize,
    active_ec2_services: usize,
    active_fargate_services: usize,
    draining_ec2_services: usize,
    draining_fargate_services: usize,
    running_ec2: usize,
    running_fargate: usize,
    pending_ec2: usize,
    pending_fargate: usize,
}

/// `Cluster$statistics`: "the task and service count ... separated by launch type".
/// Read off the single pass in [`ComAmazonawsEcsAmazonec2containerservicev20141113State::tally`].
fn cluster_statistics(tally: &TaskTally) -> Vec<KeyValuePair> {
    vec![
        pair("runningEC2TasksCount", tally.running_ec2.to_string()),
        pair("runningFargateTasksCount", tally.running_fargate.to_string()),
        pair("pendingEC2TasksCount", tally.pending_ec2.to_string()),
        pair("pendingFargateTasksCount", tally.pending_fargate.to_string()),
        pair(
            "activeEC2ServiceCount",
            tally.active_ec2_services.to_string(),
        ),
        pair(
            "activeFargateServiceCount",
            tally.active_fargate_services.to_string(),
        ),
        pair(
            "drainingEC2ServiceCount",
            tally.draining_ec2_services.to_string(),
        ),
        pair(
            "drainingFargateServiceCount",
            tally.draining_fargate_services.to_string(),
        ),
    ]
}

/// A cluster response plus the top-level `clusterCount` Amazon ECS answers a
/// cluster-mutating call with.
///
/// `clusterCount` is RECORDED behaviour, not modelled behaviour: the Smithy model's
/// `CreateClusterResponse`/`DeleteClusterResponse` declare the single member `cluster`
/// (`smithy_query(query="operation", operation="CreateCluster,DeleteCluster")`), while
/// every recorded 200 from either operation carries `clusterCount` beside `cluster`,
/// counting the clusters the account holds once the call has been applied (1 after the
/// first create, 0 after the last delete). Because the member is not in the model it has
/// no generated field to fill, so this envelope writes it after the modelled payload's
/// own members, with the member schema the operation declares — the modelled shapes and
/// the model file stay exactly as shipped, and an SDK that knows only the modelled shape
/// ignores the extra key.
#[derive(Debug)]
pub struct ClusterCountEnvelope<T> {
    /// The modelled response — the only part an SDK-generated client reads.
    pub payload: T,
    /// How many clusters the account holds after this call.
    pub cluster_count: i32,
    /// The member schema `clusterCount` is written under, declared by the operation
    /// that answers (each response shape names its own member).
    pub member: &'static ::aws_smithy_schema::Schema,
}

impl<T: ::aws_smithy_schema::serde::SerializableStruct> ::aws_smithy_schema::serde::SerializableStruct
    for ClusterCountEnvelope<T>
{
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.payload.serialize_members(serializer)?;
        serializer.write_integer(self.member, self.cluster_count)
    }
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

/// The ARN of a container instance:
/// `arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`.
pub fn container_instance_arn(region: &str, cluster_name: &str, instance_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("container-instance/{cluster_name}/{instance_id}"),
    )
}

/// The container-instance ARN a `containerInstance`/`containerInstances` entry refers
/// to: the value as given when it is already an ARN, otherwise the ARN an instance of
/// that id in `cluster_name` would carry. Every such member's doc: "The container
/// instance ID or full ARN of the container instance".
pub fn container_instance_arn_from_reference(
    region: &str,
    cluster_name: &str,
    reference: &str,
) -> String {
    if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        container_instance_arn(region, cluster_name, reference)
    }
}

/// The deployment configuration a service stores: what the request supplied, with the
/// documented defaults materialised for the members a response always carries.
///
/// `maximumPercent`: "The default maximumPercent value for a service using the REPLICA
/// service scheduler is 200%" — a daemon runs exactly one task per container instance,
/// so its deployment cannot exceed the desired count. `minimumHealthyPercent`: "The
/// default value for a replica service for minimumHealthyPercent is 100%. The default
/// minimumHealthyPercent value for a service using the DAEMON service schedule is 0% for
/// the CLI, the Amazon Web Services SDKs, and the APIs".
///
/// The two members that belong to the ROLLING update the ECS controller performs —
/// `strategy` and `deploymentCircuitBreaker` ("The deployment circuit breaker can only be
/// used for services using the rolling update (ECS) deployment type") — are defaulted
/// only for a service whose scheduler owns that rollout; a service driven by an external
/// deployment controller carries neither. `bakeTimeInMinutes` belongs to the
/// configuration itself and is defaulted for every service.
pub fn deployment_configuration(
    requested: Option<DeploymentConfiguration>,
    daemon: bool,
    scheduler_owned: bool,
) -> DeploymentConfiguration {
    let mut configuration = requested.unwrap_or_default();
    configuration.maximum_percent = Some(configuration.maximum_percent.unwrap_or(if daemon {
        DAEMON_MAXIMUM_PERCENT
    } else {
        REPLICA_MAXIMUM_PERCENT
    }));
    configuration.minimum_healthy_percent =
        Some(
            configuration
                .minimum_healthy_percent
                .unwrap_or(if daemon {
                    DAEMON_MINIMUM_HEALTHY_PERCENT
                } else {
                    REPLICA_MINIMUM_HEALTHY_PERCENT
                }),
        );
    configuration.bake_time_in_minutes = Some(
        configuration
            .bake_time_in_minutes
            .unwrap_or(DEFAULT_BAKE_TIME_MINUTES),
    );
    if scheduler_owned {
        configuration.strategy = Some(
            configuration
                .strategy
                .unwrap_or_else(|| DEPLOYMENT_STRATEGY_ROLLING.to_string()),
        );
        configuration.deployment_circuit_breaker = Some(
            configuration
                .deployment_circuit_breaker
                .unwrap_or_else(default_deployment_circuit_breaker),
        );
    }
    configuration
}

/// The deployment circuit breaker a service that configured none carries: off, with the
/// documented failure threshold.
///
/// `DeploymentCircuitBreaker$enable` is what "determines whether to use the deployment
/// circuit breaker logic for the service", so a service that configured none has it
/// turned off and rolls nothing back. `resetOnHealthyTask`: "The default is true".
/// `ThresholdConfiguration`: "By default, the threshold configuration uses a type of
/// BOUNDED_PERCENT with a value of 50."
fn default_deployment_circuit_breaker() -> DeploymentCircuitBreaker {
    DeploymentCircuitBreaker {
        enable: false,
        rollback: false,
        reset_on_healthy_task: Some(CIRCUIT_BREAKER_RESETS_ON_HEALTHY_TASK),
        threshold_configuration: Some(ThresholdConfiguration {
            r#type: THRESHOLD_BOUNDED_PERCENT.to_string(),
            value: THRESHOLD_DEFAULT_VALUE,
        }),
    }
}

/// The ARN of the IAM role a service reports in `Service$roleArn`, from the `role` the
/// request named and the deployment controller the service uses.
///
/// `Service$roleArn` is "The ARN of the IAM role that's associated with the service",
/// and `CreateService$role` names the role that gets associated: the one the request
/// named — "The name or full Amazon Resource Name (ARN) of the IAM role" — or, when it
/// named none, the ECS service-linked role, which "is required if ... the service is
/// configured to use ... an external deployment controller ... in which case you don't
/// specify a role here". A service whose own scheduler runs the rollout and that named
/// no role has no role associated with it, so it reports none.
pub fn service_role_arn(requested: Option<&str>, scheduler_owned: bool) -> Option<String> {
    match requested {
        // "If your specified role has a path other than /, then you must either specify
        // the full role ARN (this is recommended) or prefix the role name with the path."
        Some(reference) if reference.starts_with("arn:") => Some(reference.to_string()),
        Some(name) => Some(::engine::emulator_arn(
            "iam",
            "",
            &format!("role/{}", name.trim_start_matches('/')),
        )),
        None if !scheduler_owned => Some(service_linked_role_arn()),
        None => None,
    }
}

/// The ARN of the ECS service-linked role, `AWSServiceRoleForECS`. An IAM ARN carries no
/// region, which is what the empty region renders.
pub fn service_linked_role_arn() -> String {
    ::engine::emulator_arn("iam", "", SERVICE_LINKED_ROLE_RESOURCE)
}

/// The principal that made this request, for `Service$createdBy`.
///
/// Derived from the request's own SigV4 access key id, so the member reports WHO called
/// rather than a fabricated identity. That id is caller-controlled text, so it is first
/// reduced to the characters an access key id can hold.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let raw = ::engine::extract_akid(&ctx.parts().headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// The ARN of a service revision:
/// `arn:aws:ecs:<region>:<account>:service-revision/<cluster>/<service>/<id>`.
pub fn service_revision_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    revision_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("service-revision/{cluster_name}/{service_name}/{revision_id}"),
    )
}

/// The ARN of a service: `arn:aws:ecs:<region>:<account>:service/<cluster>/<name>`.
pub fn service_arn(region: &str, cluster_name: &str, service_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("service/{cluster_name}/{service_name}"),
    )
}

/// The CLUSTER and the service NAME a `service`/`services` entry refers to.
///
/// Every such member accepts "the short name or full Amazon Resource Name (ARN)" of the
/// service, and a service ARN carries the cluster that hosts it
/// (`service/<cluster>/<name>`), so an ARN names the cluster as well as the service. A
/// short name, or the old single-segment ARN form, names no cluster.
pub fn cluster_and_service_from_reference(reference: &str) -> (Option<String>, String) {
    match reference.rsplit_once(":service/") {
        Some((_, tail)) => match tail.split_once('/') {
            Some((cluster, name)) => (Some(cluster.to_string()), name.to_string()),
            None => (None, tail.to_string()),
        },
        None => (None, reference.trim().to_string()),
    }
}

/// The service NAME a `service`/`services` entry refers to: the value as given when it
/// is a short name, otherwise the last segment of a service ARN.
pub fn service_name_from_reference(reference: &str) -> String {
    cluster_and_service_from_reference(reference).1
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
