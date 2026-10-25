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
//! materialises it.
//!
//! A task whose placement still needs capacity ACQUIRED for it is one step earlier in
//! that lifecycle: it is answered in `PROVISIONING` with no start owed at all (see
//! [`awaits_provisioned_capacity`]), and it crosses `PROVISIONING` -> `PENDING` only once
//! a container instance registers into its cluster —
//! [`ComAmazonawsEcsAmazonec2containerservicev20141113State::place_tasks_awaiting_capacity`]
//! records that instant in [`TaskRecord::pending_due_at`], and the same clock pass
//! materialises it.
//!
//! `StopTask` is the same shape in reverse: the task goes to
//! `DEACTIVATING` with `stoppingAt` set (the `Task$stoppingAt` doc: "the time when the
//! task transitions from the `RUNNING` state to `STOPPING`") and reaches `STOPPED`
//! [`TRANSITION_SPAN`] later. Nothing sleeps: the due instant is stored and compared.
//!
//! A service's task set is placed by the CALL that changes what the service wants
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`], driven by
//! `CreateService` and `UpdateService`), and each task it places then crosses `PENDING`
//! -> `RUNNING` on the same clock. A service whose deployment controller is not the ECS
//! one takes its tasks from a TASK SET instead
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_task_set`], driven by
//! `CreateTaskSet`), placed the same way off the same clock. Where the capacity a
//! placement needs does not exist, nothing is placed and the scheduler records WHY on the
//! service's event stream
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::placement_capacity_reason`]);
//! the desired count is the caller's and does not move because the scheduler could not
//! meet it. The scheduler's steady-state work beyond that —
//! replacing a task somebody else stopped, re-placing a daemon's task when a container
//! instance registers later — is not modelled: nothing runs between calls, and a
//! reporting operation may not place a task nobody asked for.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Attachment, Attribute, AutoScalingGroupProvider, AwsVpcConfiguration, CapacityProvider,
    CapacityProviderStrategyItem, Cluster, ClusterConfiguration, ClusterServiceConnectDefaults,
    ClusterSetting, Container, ContainerDefinition, ContainerInstance, DaemonContainerDefinition,
    DaemonTaskDefinition, DaemonTaskDefinitionSummary, DaemonVolume, Deployment,
    DeploymentCircuitBreaker, DeploymentConfiguration, DeploymentController, EphemeralStorage,
    Failure, KeyValuePair, LoadBalancer, ManagedInstancesProvider, NetworkConfiguration,
    PlacementConstraint, PlacementStrategy, PlatformDevice, PortMapping, ProxyConfiguration,
    Resource, RuntimePlatform, Scale, Service, ServiceConnectConfiguration,
    ServiceCurrentRevisionSummary, ServiceDeploymentBrief, ServiceEvent, ServiceRegistry,
    ServiceVolumeConfiguration, Setting, Tag, Task, TaskDefinition,
    TaskDefinitionPlacementConstraint, TaskOverride, TaskSet,
    ThresholdConfiguration, VersionInfo, Volume, VpcLatticeConfiguration,
    CLUSTER_SETTING_NAME_VALUES,
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
/// It is the SERVICE default for the `containerInsights` account setting (see
/// [`ACCOUNT_SETTINGS`]): a cluster that set nothing reports the account setting in
/// effect, which falls back to this value until `PutAccountSetting` or
/// `PutAccountSettingDefault` stores one — see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_effective_settings`].
pub const CLUSTER_SETTING_DEFAULT_VALUE: &str = "disabled";

/// One Amazon ECS ACCOUNT SETTING: the name `SettingName` defines, the value in effect
/// until somebody stores one, who manages it (`Setting$type`), and the values a write
/// of it accepts.
#[derive(Debug, Clone, Copy)]
pub struct AccountSettingSpec {
    /// The setting name, one of `SettingName`
    /// (`smithy_query(query="shape", shape="SettingName")`).
    pub name: &'static str,
    /// The value this setting has for an account that has never written it.
    pub default_value: &'static str,
    /// `Setting$type`: "Indicates whether Amazon Web Services manages the account
    /// setting, or if the user manages it" — one of `SettingType`.
    pub setting_type: &'static str,
    /// The values `PutAccountSetting`/`PutAccountSettingDefault` accept for this name,
    /// from the `value` member's doc.
    pub accepted_values: &'static [&'static str],
}

/// `Setting$type` for a setting the customer writes.
pub const SETTING_TYPE_USER: &str = "user";

/// `Setting$type` for a setting Amazon Web Services manages: "`aws_managed` account
/// settings are read-only, as Amazon Web Services manages such on the customer's
/// behalf. Currently, the `guardDutyActivate` account setting is the only one Amazon
/// Web Services manages."
pub const SETTING_TYPE_AWS_MANAGED: &str = "aws_managed";

/// The values a toggle account setting accepts. `PutAccountSetting$value`: "Accepted
/// values are `enabled`, `disabled`, `enhanced`, `on`, and `off`" — `enhanced` belongs
/// to `containerInsights` alone, which has its own entry below.
const SETTING_TOGGLE_VALUES: &[&str] = &["enabled", "disabled", "on", "off"];

/// The value an opt-in feature setting has until a write turns it on. Every such
/// setting's own doc in `PutAccountSetting$name` describes what changes "when
/// modified"/"if turned on", so the account starts out with the feature off.
const SETTING_OFF: &str = "disabled";

/// The value the ARN-and-resource-id-format settings report. Their shared doc in
/// `PutAccountSetting$name`: "You must turn on this setting to use Amazon ECS features
/// such as resource tagging" — and this emulator DOES tag every resource it creates and
/// emits the long ARN format for all of them, so the setting that behaviour requires is
/// on.
const SETTING_ON: &str = "enabled";

/// Every account setting this service has, keyed by the `SettingName` enum
/// (`smithy_query(query="shape", shape="SettingName")`). `ListAccountSettings` reports
/// one effective entry per row, and a write is checked against the row's
/// `accepted_values`.
///
/// A default_value is the value AWS documents for an account that never wrote the
/// setting; the citation is on each row.
pub const ACCOUNT_SETTINGS: &[AccountSettingSpec] = &[
    // "When modified, the Amazon Resource Name (ARN) and resource ID format of the
    // resource type ... is affected. ... You must turn on this setting to use Amazon ECS
    // features such as resource tagging."
    AccountSettingSpec {
        name: "serviceLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    AccountSettingSpec {
        name: "taskLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    AccountSettingSpec {
        name: "containerInstanceLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "If `awsvpcTrunking` is turned on, any new container instances that support the
    // feature are launched have the increased ENI limits available to them" — an opt-in,
    // and this emulator applies no increased ENI limit to a registered instance.
    AccountSettingSpec {
        name: "awsvpcTrunking",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "To use Container Insights with enhanced observability, set the
    // `containerInsights` account setting to `enhanced`. To use Container Insights, set
    // the `containerInsights` account setting to `enabled`" — so the three values
    // `ClusterSetting$value` documents, off by default
    // ([`CLUSTER_SETTING_DEFAULT_VALUE`]).
    AccountSettingSpec {
        name: "containerInsights",
        default_value: CLUSTER_SETTING_DEFAULT_VALUE,
        setting_type: SETTING_TYPE_USER,
        accepted_values: &CLUSTER_SETTING_VALUES,
    },
    // Fargate FIPS-140 compliance is an opt-in an account turns on; this emulator serves
    // no FIPS endpoint.
    AccountSettingSpec {
        name: "fargateFIPSMode",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "Amazon ECS is introducing tagging authorization for resource creation. ...
    // Therefore, you must grant explicit permissions to use the `ecs:TagResource`
    // action" — an opt-in, and this emulator runs no extra tag authorization.
    AccountSettingSpec {
        name: "tagResourceAuthorization",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // `PutAccountSetting$value`: "When you specify `fargateTaskRetirementWaitPeriod` for
    // the `name`, the following are the valid values: `0` ... `7` ... `14`". The wait
    // Amazon Web Services applies until an account changes it is 7 calendar days
    // ("Amazon Web Services Fargate task maintenance", the doc this member links).
    AccountSettingSpec {
        name: "fargateTaskRetirementWaitPeriod",
        default_value: "7",
        setting_type: SETTING_TYPE_USER,
        accepted_values: &["0", "7", "14"],
    },
    // "The `guardDutyActivate` parameter is read-only in Amazon ECS and indicates
    // whether Amazon ECS Runtime Monitoring is enabled or disabled by your security
    // administrator ... Amazon GuardDuty controls this account setting on your behalf" —
    // so it is `aws_managed`, and nothing in this emulator turns Runtime Monitoring on.
    AccountSettingSpec {
        name: "guardDutyActivate",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_AWS_MANAGED,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "The `defaultLogDriverMode` setting supports two values: `blocking` and
    // `non-blocking`. ... On June 25, 2025, Amazon ECS changed the default log driver
    // mode from `blocking` to `non-blocking`".
    AccountSettingSpec {
        name: "defaultLogDriverMode",
        default_value: "non-blocking",
        setting_type: SETTING_TYPE_USER,
        accepted_values: &["blocking", "non-blocking"],
    },
    // "Use `fargateEventWindows` to use EC2 Event Windows associated with Fargate tasks
    // to configure time windows for task retirement" — an opt-in, and this emulator
    // models no event window.
    AccountSettingSpec {
        name: "fargateEventWindows",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
];

/// The row [`ACCOUNT_SETTINGS`] holds for `name`, or `None` when the name is not an
/// account setting. A `name` member that reached a handler targets the closed
/// `SettingName` enum, so `None` means this catalog is missing a row the model defines —
/// which the crate's tests pin.
pub fn account_setting_spec(name: &str) -> Option<&'static AccountSettingSpec> {
    ACCOUNT_SETTINGS
        .iter()
        .find(|setting| setting.name == name)
}

/// The largest `maxResults` `ListAccountSettings` accepts, and the page size it uses
/// when the request gives none. That member's doc: "This value can be between 1 and 10.
/// If this parameter isn't used, then ListAccountSettings returns up to 10 results and a
/// nextToken value if applicable."
pub const ACCOUNT_SETTINGS_PAGE_SIZE: usize = 10;

/// How many DISTINCT principals this emulator holds explicit account settings for.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`]: `PutAccountSetting` takes
/// the principal from the request, so without a cap a caller could grow the in-memory
/// store without limit.
pub const MAX_ACCOUNT_SETTING_PRINCIPALS: usize = 64;

/// The longest `principalArn` an account-setting write may name. EMULATOR-CHOSEN, so one
/// accepted write cannot carry an arbitrarily long key into the store: IAM bounds a
/// principal's name at 64 characters and its path at 512, so no ARN Amazon Web Services
/// itself mints comes close.
pub const MAX_PRINCIPAL_ARN_LENGTH: usize = 1024;

/// What [`ComAmazonawsEcsAmazonec2containerservicev20141113State::put_account_setting`]
/// answers when storing a setting for a principal this emulator does not hold one for
/// yet would push it past [`MAX_ACCOUNT_SETTING_PRINCIPALS`]: nothing was written, so
/// the caller refuses the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSettingCapacityExceeded;

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

/// `ListServices`'s page size when the caller gives no `maxResults`. Its own
/// `maxResults` doc differs from the sentence [`DEFAULT_LIST_PAGE_SIZE`] quotes: "This
/// value can be between 1 and 100. If this parameter isn't used, then ListServices
/// returns up to 10 results and a nextToken value if applicable."
pub const DEFAULT_LIST_SERVICES_PAGE_SIZE: usize = 10;

/// How many containers one task definition may describe. The Amazon ECS service quota
/// "Containers per task definition" is 10, and the cap is also what bounds the work and
/// the stored bytes ONE `RegisterTaskDefinition` can ask for — and, through it, the
/// containers every task of that revision mints and every describe renders.
pub const MAX_CONTAINER_DEFINITIONS: usize = 10;

/// How many load-balancer attachments one service may be given. The Amazon ECS service
/// quota "Target groups per service" is 5, and the cap is also what bounds the work ONE
/// `CreateService` or `UpdateService` can ask for: every entry names up to two target
/// groups that are looked up in the Elastic Load Balancing v2 service's store, so
/// without it a caller could repeat an entry up to the request-body cap and set both how
/// much work this request does and how long that other service's lock is held.
pub const MAX_LOAD_BALANCERS: usize = 5;

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
/// The lifecycle status of a task Amazon ECS has accepted but still has to acquire
/// resources for before it can place it — the status that precedes [`STATUS_PENDING`],
/// which a task only reaches once it has been placed. See
/// [`awaits_provisioned_capacity`].
pub const STATUS_PROVISIONING: &str = "PROVISIONING";
/// See [`STATUS_PENDING`].
pub const STATUS_RUNNING: &str = "RUNNING";
/// See [`STATUS_PENDING`].
pub const STATUS_DEACTIVATING: &str = "DEACTIVATING";
/// See [`STATUS_PENDING`].
pub const STATUS_STOPPED: &str = "STOPPED";

/// `Cluster$attachmentsStatus` while the cluster's capacity-provider association is
/// being applied — the member's doc: "UPDATE_IN_PROGRESS: The available capacity
/// providers for the cluster are updating."
pub const ATTACHMENTS_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// `Cluster$attachmentsStatus` once it has been applied — the same doc:
/// "UPDATE_COMPLETE: The capacity providers have successfully updated."
pub const ATTACHMENTS_UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The message of the `UpdateInProgressException` a cluster mutation is refused with
/// while the cluster's attachments are updating. It states the precondition the
/// `Cluster$attachmentsStatus` doc describes (`UPDATE_IN_PROGRESS` -> `UPDATE_COMPLETE`
/// / `UPDATE_FAILED`), which is what the refusal is about; the HTTP status (400) and the
/// error code are what a client matches on.
pub const CLUSTER_BUSY_MESSAGE: &str = "The specified cluster is in a busy state. Cluster \
                                        attachments must be in UPDATE_COMPLETE or \
                                        UPDATE_FAILED state before they can be updated. \
                                        Wait and try again.";

/// How many capacity providers one cluster may hold, and so the longest
/// `capacityProviders` list and `defaultCapacityProviderStrategy` one
/// `PutClusterCapacityProviders` may carry (every strategy entry must name one of the
/// providers the same request associates). The Amazon ECS service quota "Capacity
/// providers per cluster" is 20, and the cap is also what bounds the work ONE request
/// can ask for: the association is checked list-against-list and against the stored
/// association, so an uncapped list would let one caller choose the cost of both
/// factors.
pub const MAX_CLUSTER_CAPACITY_PROVIDERS: usize = 20;

/// `CapacityProvider$status` for a provider that can be used in a cluster — that
/// member's doc: "Only capacity providers in an `ACTIVE` state can be used in a
/// cluster." `CreateCapacityProvider` answers with the provider already usable, so this
/// is the status a create reports and no clock-owed transition leads to it.
pub const CAPACITY_PROVIDER_ACTIVE: &str = "ACTIVE";

/// `CapacityProvider$status` for a deleted provider — that member's doc: "When a capacity
/// provider is successfully deleted, it has an `INACTIVE` status." A tombstone under that
/// status holds neither its name nor its Auto Scaling group against a new provider.
pub const CAPACITY_PROVIDER_INACTIVE: &str = "INACTIVE";

/// `CapacityProvider$type` for a provider that manages EC2 instances itself — that
/// member's doc: "For Amazon ECS Managed Instances, this value is `MANAGED_INSTANCES`".
/// A provider built on a caller's own Auto Scaling group carries no type.
pub const CAPACITY_PROVIDER_TYPE_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The prefixes a capacity-provider name may not start with, from the
/// `CreateCapacityProvider$name` doc: "The name can't be prefixed with \"aws\", \"ecs\",
/// or \"fargate\"." Matched case-insensitively, since the name itself accepts both cases
/// and the reservation is on the word rather than on its spelling.
pub const CAPACITY_PROVIDER_RESERVED_PREFIXES: [&str; 3] = ["aws", "ecs", "fargate"];

/// How many capacity providers one `DescribeCapacityProviders` page carries when the
/// request names no `maxResults`, and the most it accepts: the member's own doc, "This
/// value can be between 1 and 10. If this parameter is not used, then
/// DescribeCapacityProviders returns up to 10 results".
pub const DESCRIBE_CAPACITY_PROVIDERS_PAGE_SIZE: usize = 10;

/// How many capacity providers this emulator holds. EMULATOR-CHOSEN, for the same
/// reason as [`MAX_CLUSTERS`]: the store is in memory and a single caller must not be
/// able to grow it without bound. A create past it is refused with the modelled
/// `LimitExceededException` ("The limit for the resource was exceeded"), which is what a
/// client's quota handling keys on.
pub const MAX_CAPACITY_PROVIDERS: usize = 64;

/// The values `ClusterSetting$value` accepts, from that member's own doc: "The supported
/// values are `enhanced`, `enabled`, and `disabled`." The setting NAME is a closed enum
/// the boundary validator already refuses outside, but the value is a free string the
/// service constrains itself — so every operation that accepts a `ClusterSetting`
/// ([`crate::ops::create_cluster`], [`crate::ops::update_cluster`],
/// [`crate::ops::update_cluster_settings`]) checks it through
/// [`validate_cluster_settings`].
pub const CLUSTER_SETTING_VALUES: [&str; 3] = ["enhanced", "enabled", "disabled"];

/// `ExecuteCommandConfiguration$logging` value that carries its own destination — the
/// member's doc: "OVERRIDE: Specify the logging details as a part of logConfiguration."
pub const EXECUTE_COMMAND_LOGGING_OVERRIDE: &str = "OVERRIDE";

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

/// `TaskDefinition$status` for a revision that has been deregistered — the
/// `DeregisterTaskDefinition` doc: "Upon deregistration, the task definition is marked
/// as `INACTIVE`." One of the `TaskDefinitionStatus` values
/// (`smithy_query(query="shape", shape="TaskDefinitionStatus")`), and the status a
/// revision keeps: "INACTIVE task definitions remain discoverable in your account
/// indefinitely."
pub const TASK_DEFINITION_INACTIVE: &str = "INACTIVE";

/// `TaskDefinition$status` for a revision whose DELETE has been asked for — the
/// `DeleteTaskDefinitions` doc: "When you delete a task definition revision, it is
/// immediately transitions from the `INACTIVE` to `DELETE_IN_PROGRESS`." The third
/// `TaskDefinitionStatus` value
/// (`smithy_query(query="shape", shape="TaskDefinitionStatus")`), and the status a
/// revision keeps while the deletion is outstanding: "A task definition revision will
/// stay in `DELETE_IN_PROGRESS` status until all the associated tasks and services have
/// been terminated", and while it is there "the task definition name is displayed in the
/// console and returned in the API" — so the revision stays stored and readable.
pub const TASK_DEFINITION_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `DaemonTaskDefinition$status` for a registered daemon revision. One of the
/// `DaemonTaskDefinitionStatus` values
/// (`smithy_query(query="shape", shape="DaemonTaskDefinitionStatus")`: `ACTIVE`,
/// `DELETE_IN_PROGRESS`, `DELETED`), and the one `RegisterDaemonTaskDefinition` answers
/// with: a registered revision is usable straight away, so no clock-owed transition
/// leads to it.
pub const DAEMON_TASK_DEFINITION_ACTIVE: &str = "ACTIVE";

/// `DaemonTaskDefinition$status` while the delete a `DeleteDaemonTaskDefinition` call
/// asked for is still running — the status a revision stands in from the instant the
/// delete was requested until it has been carried out, and the second of the
/// `DaemonTaskDefinitionStatus` values.
pub const DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `DaemonTaskDefinition$status` once a requested delete has been carried out — the
/// terminal `DaemonTaskDefinitionStatus` value, reached [`TRANSITION_SPAN`] after the
/// delete was requested and never left again.
pub const DAEMON_TASK_DEFINITION_DELETED: &str = "DELETED";

/// The `DaemonTaskDefinitionStatusFilter` value that narrows a daemon task-definition
/// listing to nothing at all — `ListDaemonTaskDefinitions$status`: "If you set this
/// parameter to `ALL`, all daemon task definitions are listed regardless of status." The
/// other two values of that enum are statuses a stored revision carries, so they are
/// matched against [`DaemonTaskDefinitionRecord::status`] directly.
pub const DAEMON_TASK_DEFINITION_STATUS_FILTER_ALL: &str = "ALL";

/// The one `DaemonTaskDefinitionRevisionFilter` value
/// ([`crate::ops::types::DAEMON_TASK_DEFINITION_REVISION_FILTER_VALUES`]) —
/// `ListDaemonTaskDefinitions$revision`: "Specify `LAST_REGISTERED` to return only the
/// last registered revision for each daemon task definition family."
pub const DAEMON_REVISION_FILTER_LAST_REGISTERED: &str = "LAST_REGISTERED";

/// The namespace mode a daemon revision records when the registration named none, for
/// `pidMode` and for `ipcMode` alike. Both members' docs: "The valid values are `none`
/// and `shared`. The default is `none`. If `none` is specified or no value is provided,
/// the daemon runs with its own PID/IPC namespace, isolated from other tasks" — so a
/// registration that named nothing and one that named this value describe the same
/// daemon, and the stored revision spells it once.
pub const DAEMON_NAMESPACE_MODE_NONE: &str = "none";

/// How many daemon task-definition revisions this emulator holds, across every family.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_TASK_DEFINITIONS`].
pub const MAX_DAEMON_TASK_DEFINITIONS: usize = MAX_TASK_DEFINITIONS;

/// The `SortOrder` value that REVERSES a listing's documented default order — the
/// `ListTaskDefinitions$sort` doc: "Setting this parameter to `DESC` reverses the sort
/// order on family name and revision." One of the two values the model's enum carries
/// ([`crate::ops::types::SORT_ORDER_VALUES`]); the other, `ASC`, is the order a request
/// that names no sort already gets.
pub const SORT_ORDER_DESC: &str = "DESC";

/// `Failure$reason` for an id that names nothing in this account. The ECS "API failure
/// reasons" list: `MISSING` — "the described resource could not be found".
pub const FAILURE_MISSING: &str = "MISSING";

/// The launch type a task takes when the request names neither a `launchType` nor a
/// `capacityProviderStrategy` and the cluster has no default strategy: ECS runs the
/// task on the EC2 capacity registered to the cluster.
pub const DEFAULT_LAUNCH_TYPE: &str = "EC2";

/// `stopCode` for a stop the caller asked for, from the `TaskStopCode` enum.
pub const STOP_CODE_USER_INITIATED: &str = "UserInitiated";

/// The network mode that gives each task its own elastic network interface, from the
/// `NetworkMode` enum. Named here because three derivations key off it: which launch
/// types a revision is compatible with, which container-instance attributes it
/// requires, and how a port mapping's `hostPort` is defaulted.
pub const NETWORK_MODE_AWSVPC: &str = "awsvpc";

/// The network mode that puts the containers on the host's own network namespace, from
/// the `NetworkMode` enum. Like [`NETWORK_MODE_AWSVPC`] it maps a container port
/// statically onto the same host port.
pub const NETWORK_MODE_HOST: &str = "host";

/// The network mode that leaves the containers without external connectivity, from the
/// `NetworkMode` enum. The `networkMode` doc: "If the network mode is set to none, you
/// cannot specify port mappings in your container definitions".
pub const NETWORK_MODE_NONE: &str = "none";

/// The ELBv2 `TargetType` that registers a target by IP address, from that service's
/// `TargetTypeEnum` (`smithy_query(query="shape", shape="TargetTypeEnum",
/// model_path="services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json")`:
/// `instance`, `ip`, `lambda`, `alb`).
pub const TARGET_TYPE_IP: &str = "ip";

/// The ELBv2 `TargetType` that registers a target by EC2 instance id, from the same
/// enum as [`TARGET_TYPE_IP`].
pub const TARGET_TYPE_INSTANCE: &str = "instance";

/// The `protocol` a port mapping takes when the registration names none. The
/// `PortMapping$protocol` doc: "Valid values are tcp and udp (case-sensitive). The
/// default is tcp."
pub const DEFAULT_PORT_PROTOCOL: &str = "tcp";

/// The `hostPort` a port mapping takes under a dynamic mapping strategy. The
/// `PortMapping$hostPort` doc, for the `bridge` network mode: "you can omit the hostPort
/// (or set it to 0) while specifying a containerPort and your container automatically
/// receives a port in the ephemeral port range" — the port is picked when the task is
/// placed, so the registered revision reports the `0` that asks for one.
pub const DYNAMIC_HOST_PORT: i32 = 0;

/// Every launch type a task-definition projection may report, in the order Amazon ECS
/// reports them in `TaskDefinition$compatibilities`. The membership is the `Compatibility`
/// enum (`smithy_query(query="shape", shape="com.amazonaws.ecs#Compatibility")`:
/// `EC2`, `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`); the ORDER is the one recorded ECS
/// responses carry, so a client that reads the list positionally reads the same launch
/// type the real service named. [`TaskDefinitionRecord::compatibilities`] filters it by
/// what the stored revision's parameters support.
pub const COMPATIBILITY_ORDER: [&str; 4] = ["EXTERNAL", "EC2", "MANAGED_INSTANCES", "FARGATE"];

/// The launch types `Cluster$statistics` separates a cluster's counts by, each paired
/// with the segment its four statistic names carry (`running<segment>TasksCount`,
/// `pending<segment>TasksCount`, `active<segment>ServiceCount`,
/// `draining<segment>ServiceCount`).
///
/// The membership is the `LaunchType` enum (`smithy_query(query="shape",
/// shape="LaunchType")`: `EC2`, `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`), so every
/// launch type a task or a service can be placed with has its own counters: the
/// `Cluster$statistics` doc lists the first [`DOCUMENTED_STATISTICS_LAUNCH_TYPES`] of
/// them by name and introduces the list with "They include the following", so the
/// remaining launch types' counters are reported too. [`cluster_statistics`] fixes the
/// order the entries come in.
pub const STATISTICS_LAUNCH_TYPES: [(&str, &str); 4] = [
    ("EC2", "EC2"),
    ("FARGATE", "Fargate"),
    ("EXTERNAL", "External"),
    ("MANAGED_INSTANCES", "ManagedInstances"),
];

/// How many of [`STATISTICS_LAUNCH_TYPES`] the `Cluster$statistics` doc itself names.
/// Those launch types' counters come first, counter by counter, as the doc lists them.
const DOCUMENTED_STATISTICS_LAUNCH_TYPES: usize = 2;

/// The platform versions this service knows, as the `platformVersion` doc's "Fargate
/// platform versions" page lists them: the Linux platform versions `1.4.0`, `1.3.0`,
/// `1.2.0`, `1.1.0` and `1.0.0` (the Windows one is `1.0.0` too), plus the `LATEST`
/// alias the member's own doc names ("If one isn't specified, the LATEST platform
/// version is used").
///
/// A platform version outside this list names no platform, which is what
/// `PlatformUnknownException` ("The specified platform version doesn't exist.") is
/// modelled for.
pub const PLATFORM_VERSIONS: [&str; 6] = ["LATEST", "1.4.0", "1.3.0", "1.2.0", "1.1.0", "1.0.0"];

/// The `platformVersion` alias that asks for whatever the newest platform version is,
/// rather than naming one: "If one isn't specified, the LATEST platform version is used."
pub const PLATFORM_VERSION_LATEST: &str = "LATEST";

/// The Fargate platform version [`PLATFORM_VERSION_LATEST`] resolves to for a LINUX
/// task, as the `platformVersion` doc's "Fargate platform versions" page states: `1.4.0`
/// is the newest Linux platform version.
pub const LATEST_LINUX_PLATFORM_VERSION: &str = "1.4.0";

/// The Fargate platform version [`PLATFORM_VERSION_LATEST`] resolves to for a Windows
/// task: the same page publishes exactly one Windows platform version, `1.0.0`.
pub const LATEST_WINDOWS_PLATFORM_VERSION: &str = "1.0.0";

/// The launch type whose tasks run on Fargate, and so the one the Fargate-only members
/// (`platformFamily`, `platformVersion`) are reported for.
pub const LAUNCH_TYPE_FARGATE: &str = "FARGATE";

/// The capacity providers Amazon ECS manages for every account in every Region, which
/// place tasks on Fargate: the `UpdateService$capacityProviderStrategy` doc — "When you
/// use Fargate, the capacity providers are FARGATE or FARGATE_SPOT". They are not created
/// by `CreateCapacityProvider` (their names are reserved,
/// [`CAPACITY_PROVIDER_RESERVED_PREFIXES`]) and exist whether or not anybody asked, so
/// they are resolved rather than stored ([`managed_capacity_provider`]).
pub const MANAGED_CAPACITY_PROVIDERS: [&str; 2] = ["FARGATE", "FARGATE_SPOT"];

/// The `OSFamily` a revision that declares no `runtimePlatform` runs on. `RuntimePlatform
/// $operatingSystemFamily` is "The operating system", whose `OSFamily` enum
/// (`smithy_query(query="shape", shape="OSFamily")`) spans `LINUX` and the Windows
/// Server families; a task definition that names none runs on Linux.
pub const DEFAULT_OS_FAMILY: &str = "LINUX";

/// The DISPLAY spelling of an `OSFamily` value, which is what `Service$platformFamily`
/// and `Deployment$platformFamily` report ("The operating system that your tasks in the
/// service run on").
///
/// The enum's own name, rendered for display: the `_`-separated words become `-`-separated
/// words, and a word that is all letters is capitalised (`LINUX` -> `Linux`,
/// `WINDOWS_SERVER_2019_FULL` -> `Windows-Server-2019-Full`). A word that is not all
/// letters is a release designation rather than a word and is kept exactly as the enum
/// spells it (`2019`, `20H2`), so every value of the enum — including the ones no request
/// here names — renders deterministically.
pub fn os_family_display(os_family: &str) -> String {
    os_family
        .split('_')
        .map(|word| {
            if !word.chars().all(|character| character.is_ascii_alphabetic()) {
                return word.to_string();
            }
            let mut characters = word.chars();
            match characters.next() {
                None => String::new(),
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &characters.as_str().to_lowercase()
                }
            }
        })
        .collect::<Vec<String>>()
        .join("-")
}

/// The platform version a DEPLOYMENT runs, resolved from the one the service asks for and
/// the operating-system family its tasks run on.
///
/// A service may ask for [`PLATFORM_VERSION_LATEST`], which names no version: the
/// deployment runs the newest platform version published for its operating system
/// ([`LATEST_LINUX_PLATFORM_VERSION`], [`LATEST_WINDOWS_PLATFORM_VERSION`]). A service
/// that named a version runs that one.
pub fn resolved_platform_version(requested: &str, os_family: &str) -> String {
    if requested != PLATFORM_VERSION_LATEST {
        return requested.to_string();
    }
    if os_family == DEFAULT_OS_FAMILY {
        LATEST_LINUX_PLATFORM_VERSION.to_string()
    } else {
        LATEST_WINDOWS_PLATFORM_VERSION.to_string()
    }
}

/// The message a `PlatformUnknownException` carries: the FIXED sentence Amazon ECS
/// answers for this condition, as the recorded refusal carries it (the error shape's
/// own doc words the same condition as "The specified platform version doesn't
/// exist."). It names nothing from the request.
pub const PLATFORM_UNKNOWN_MESSAGE: &str = "The specified platform does not exist.";

/// The launch types whose capacity is REGISTERED to the cluster, so a task placed with
/// one of them needs a container instance of the cluster to run on. The `launchType`
/// doc: "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
/// cluster. ... The EXTERNAL launch type runs your tasks on your on-premises server or
/// virtual machine (VM) capacity registered to your cluster." The other launch types run
/// on capacity Amazon Web Services manages, which the cluster does not hold.
pub const REGISTERED_CAPACITY_LAUNCH_TYPES: [&str; 2] = ["EC2", "EXTERNAL"];

/// The message the `InvalidParameterException` carries when a task would be placed on
/// capacity registered to a cluster that has none. A FIXED sentence naming nothing from
/// the request, as the recorded refusal carries it.
pub const NO_CONTAINER_INSTANCES_MESSAGE: &str =
    "No Container Instances were found in your cluster.";

/// The message the `InvalidParameterException` carries when the placement runs through a
/// CAPACITY PROVIDER whose capacity is not there: the provider the placement named is
/// what the caller has to scale, and a cluster can carry several of them, so the sentence
/// is built from the request and names that one provider.
pub fn empty_capacity_provider_message(provider_name: &str) -> String {
    format!("The specified capacity provider '{provider_name}' contains no container instances.")
}

/// The container-instance attribute an `awsvpc` task needs: its own elastic network
/// interface. One of the Amazon ECS container-agent capabilities (the ECS developer
/// guide's "awsvpc network mode" requirements), reported through
/// `TaskDefinition$requiresAttributes` so a client can check an instance before placing
/// a task on it.
pub const ATTRIBUTE_TASK_ENI: &str = "ecs.capability.task-eni";

/// The minimum Docker remote-API version an `awsvpc` task needs on the container
/// instance, as the agent publishes it (`com.amazonaws.ecs.capability.docker-remote-api.<version>`).
pub const ATTRIBUTE_DOCKER_REMOTE_API_1_18: &str =
    "com.amazonaws.ecs.capability.docker-remote-api.1.18";

/// The attribute a container needs to read a Secrets Manager secret into an environment
/// variable (`Secret$valueFrom` holding a `secretsmanager` ARN).
pub const ATTRIBUTE_SECRETS_ASM_ENVIRONMENT: &str =
    "ecs.capability.secrets.asm.environment-variables";

/// The same, for a Systems Manager Parameter Store parameter.
pub const ATTRIBUTE_SECRETS_SSM_ENVIRONMENT: &str =
    "ecs.capability.secrets.ssm.environment-variables";

/// The attribute a container needs to read a Secrets Manager secret into its log
/// driver's configuration (`LogConfiguration$secretOptions`).
pub const ATTRIBUTE_SECRETS_ASM_LOG_DRIVER: &str = "ecs.capability.secrets.asm.bootstrap.log-driver";

/// The same, for a Systems Manager Parameter Store parameter.
pub const ATTRIBUTE_SECRETS_SSM_LOG_DRIVER: &str = "ecs.capability.secrets.ssm.bootstrap.log-driver";

/// The attribute a container needs to pull an environment variable file out of Amazon
/// S3 (`EnvironmentFile$type` `s3`).
pub const ATTRIBUTE_ENV_FILES_S3: &str = "ecs.capability.env-files.s3";

/// The attribute a container instance needs to run a container with elevated privileges
/// (`ContainerDefinition$privileged`).
pub const ATTRIBUTE_PRIVILEGED_CONTAINER: &str = "com.amazonaws.ecs.capability.privileged-container";

/// The prefix of the attribute a container instance needs to run a named log driver;
/// the driver name (`LogConfiguration$logDriver`) completes it.
pub const ATTRIBUTE_LOGGING_DRIVER_PREFIX: &str = "com.amazonaws.ecs.capability.logging-driver.";

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

/// `Deployment$status` for a deployment a newer one is replacing — the same doc:
/// "`ACTIVE`: A service deployment that still has running tasks, but are in the process of
/// being replaced with a new launch type, platform version, task definition, or Docker
/// image." A deployment with no task left is "completely replaced" (`INACTIVE`), so it is
/// no longer one of the service's deployments at all.
pub const DEPLOYMENT_ACTIVE: &str = "ACTIVE";

/// How many superseded deployments one service keeps at once.
///
/// A deployment leaves that list as soon as its replacement falls due, so a service holds
/// one or two of them in practice. The bound is what keeps a burst of updates inside a
/// single span from growing the record without limit; the NEWEST are kept, since they are
/// the ones still being replaced.
pub const MAX_SUPERSEDED_DEPLOYMENTS: usize = 10;

/// `Deployment$rolloutState` while the deployment is still placing tasks — the member's
/// doc: "When a service deployment is started, it begins in an `IN_PROGRESS` state."
pub const ROLLOUT_IN_PROGRESS: &str = "IN_PROGRESS";
/// `Deployment$rolloutState` once the service holds the tasks it wants — the same doc:
/// "When the service reaches a steady state, the deployment transitions to a
/// `COMPLETED` state."
pub const ROLLOUT_COMPLETED: &str = "COMPLETED";

/// `ServiceDeploymentBrief$status` while the rollout this deployment records is still
/// under way — one of `ServiceDeploymentStatus`
/// (`smithy_query(query="shape", shape="ServiceDeploymentStatus")`), and the
/// service-deployment counterpart of the [`ROLLOUT_IN_PROGRESS`] the SAME rollout reports
/// on the service's own `Deployment`.
pub const SERVICE_DEPLOYMENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a deployment whose service
/// reached the steady state it was rolling out — the counterpart of
/// [`ROLLOUT_COMPLETED`].
pub const SERVICE_DEPLOYMENT_SUCCESSFUL: &str = "SUCCESSFUL";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a deployment that stopped
/// WITHOUT reaching that steady state, because a newer deployment replaced it or the
/// service it was rolling out was deleted. Those two are the only ways a rollout ends
/// early here: nothing else interrupts one, so no deployment is left open forever.
pub const SERVICE_DEPLOYMENT_STOPPED: &str = "STOPPED";

/// `ListServiceDeployments`'s page size when the caller gives no `maxResults`, from that
/// member's own doc: "This value can be between 1 and 100. If this parameter isn't used,
/// then ListServiceDeployments returns up to 20 results and a nextToken value if
/// applicable." The upper bound is the shared [`MAX_LIST_PAGE_SIZE`].
pub const DEFAULT_LIST_SERVICE_DEPLOYMENTS_PAGE_SIZE: usize = 20;

/// How many service deployments this emulator keeps for ONE service. EMULATOR-CHOSEN, for
/// the same reason as [`MAX_SERVICE_EVENTS`]: every `UpdateService` that forces a new
/// deployment appends one, so without a bound a caller could grow the in-memory store
/// without limit. The newest are the ones kept.
pub const MAX_SERVICE_DEPLOYMENTS_PER_SERVICE: usize = 100;

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

/// `TaskSet$status` for a task set that is NOT serving production traffic — the status
/// every task set a client creates starts in. The member's doc: "PRIMARY: The task set
/// is serving production traffic. ACTIVE: The task set isn't serving production traffic",
/// and `UpdateServicePrimaryTaskSet` is what promotes one ("Modifies which task set in a
/// service is the primary task set").
pub const TASK_SET_ACTIVE: &str = "ACTIVE";

/// `TaskSet$status` for the task set that IS serving production traffic.
pub const TASK_SET_PRIMARY: &str = "PRIMARY";

/// `StabilityStatus` for a task set that has reached its steady state. The
/// `TaskSet$stabilityStatus` doc: the task set is in `STEADY_STATE` when "The task
/// `runningCount` is equal to the `computedDesiredCount`" and "The `pendingCount` is 0";
/// "If any of those conditions aren't met, the stability status returns STABILIZING."
pub const STABILITY_STATUS_STEADY_STATE: &str = "STEADY_STATE";

/// See [`STABILITY_STATUS_STEADY_STATE`]: the task set has not reached its steady state.
pub const STABILITY_STATUS_STABILIZING: &str = "STABILIZING";

/// The only unit a scale is measured in, from the `ScaleUnit` enum (`PERCENT`), and so
/// the unit a stored scale carries when the request named none.
pub const SCALE_UNIT_PERCENT: &str = "PERCENT";

/// The lowest `Scale$value` a task set may carry. The member's doc: "The value,
/// specified as a percent total of a service's `desiredCount`, to scale the task set.
/// Accepted values are numbers between 0 and 100."
pub const MIN_SCALE_PERCENT: f64 = 0.0;

/// The highest `Scale$value` a task set may carry — see [`MIN_SCALE_PERCENT`].
pub const MAX_SCALE_PERCENT: f64 = 100.0;

/// How many task sets one service may hold. The `CreateTaskSet` doc points at the quota
/// ("For information about the maximum number of task sets and other quotas, see Amazon
/// ECS service quotas"), where "Task sets per service" is 5.
pub const MAX_TASK_SETS_PER_SERVICE: usize = 5;

/// The message a `TaskSetNotFoundException` carries. A FIXED sentence naming nothing from
/// the request, like this service's other not-found sentences.
pub const TASK_SET_NOT_FOUND_MESSAGE: &str = "The specified task set was not found.";

/// `TaskSet$startedBy` for a task set CodeDeploy created: "If an CodeDeploy deployment
/// created the task set, the `startedBy` parameter is `CODE_DEPLOY`. If an external
/// deployment created the task set, the `startedBy` field isn't used."
pub const TASK_SET_STARTED_BY_CODE_DEPLOY: &str = "CODE_DEPLOY";

/// Why a `Scale` a request carries cannot be stored, or `None` when it can.
///
/// `Scale$value`: "Accepted values are numbers between 0 and 100" — a percentage of the
/// service's desired count, so a value outside that range describes no task set. The
/// bound is reported as the double it is, the way the refused percentage reads.
pub fn scale_violation(scale: &Scale) -> Option<String> {
    if scale.value > MAX_SCALE_PERCENT {
        return Some(format!(
            "TaskSet Scale value for percent can not be greater than {MAX_SCALE_PERCENT:.1}"
        ));
    }
    if scale.value < MIN_SCALE_PERCENT {
        return Some(format!(
            "TaskSet Scale value for percent can not be less than {MIN_SCALE_PERCENT:.1}"
        ));
    }
    None
}

/// The scale a task set is STORED with: the one the request carried, with the only unit
/// the `ScaleUnit` enum has filled in, and `Scale$value`'s own modelled default (`0`) for
/// a request that named no scale at all.
pub fn stored_scale(requested: Option<&Scale>) -> Scale {
    Scale {
        value: requested.map(|scale| scale.value).unwrap_or_default(),
        unit: Some(SCALE_UNIT_PERCENT.to_string()),
    }
}

/// The service event the scheduler writes when it wants to place a task and cannot.
///
/// The Amazon ECS service event message for a placement that finds no instance to run
/// on, whose `reason` sentence says WHICH capacity was missing — the same sentence
/// `RunTask` refuses with, since both describe the one condition.
pub fn unable_to_place_message(service_name: &str, reason: &str) -> String {
    format!(
        "(service {service_name}) was unable to place a task because no container instance \
         met all of its requirements. Reason: {reason} For more information, see the \
         Troubleshooting section of the Amazon ECS Developer Guide."
    )
}

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

/// The deployment controller CodeDeploy drives, from the `DeploymentControllerType` enum.
/// Named here because the service it controls must carry a load balancer: CodeDeploy
/// shifts traffic between two target groups, which only a listener can reach.
pub const DEPLOYMENT_CONTROLLER_CODE_DEPLOY: &str = "CODE_DEPLOY";

/// The deployment controller a third party drives, from the `DeploymentControllerType`
/// enum. The `CreateTaskSet` doc: "This is used when a service uses the EXTERNAL
/// deployment controller type", so a service under this controller takes its tasks from
/// the task sets a client creates rather than from its own scheduler.
pub const DEPLOYMENT_CONTROLLER_EXTERNAL: &str = "EXTERNAL";

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

/// How many task-definition revisions one `DeleteTaskDefinitions` request may name. The
/// `DeleteTaskDefinitionsRequest$taskDefinitions` doc: "You can specify up to 10 task
/// definitions as a comma separated list."
pub const MAX_DELETE_TASK_DEFINITIONS: usize = 10;

/// How many tags ONE resource may carry, whoever wrote them. The `Tag` doc's first
/// restriction: "Maximum number of tags per resource - 50" — the same number the `Tags`
/// list's own length constraint caps a single request at, which is why it bites only
/// where a request ADDS to a set a resource already carries.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

// The message of a refusal whose condition is that a NAMED resource does not exist.
// Amazon ECS answers most of these with ONE fixed sentence that names no resource: the
// same text for every request that meets the condition, whatever cluster, service, task
// or task definition the request named. Every such refusal in this crate therefore
// carries the constant below rather than a sentence built from the request — a refusal
// ECS DERIVES from the request (a member that is blank or outside its documented
// constraint, or a container-instance reference that resolves to nothing — see
// [`container_instance_not_found_message`]) names the member or the value instead, and
// is built by a function rather than being a constant.

/// `ClusterNotFoundException$message` for a `cluster` that names no stored cluster.
pub const CLUSTER_NOT_FOUND_MESSAGE: &str = "Cluster not found.";

/// `ServiceNotFoundException$message` for a `service` that names no stored service.
pub const SERVICE_NOT_FOUND_MESSAGE: &str = "Service not found.";

/// `InvalidParameterException$message` for a `task` that names no stored task.
pub const TASK_NOT_FOUND_MESSAGE: &str = "The referenced task was not found";

/// `ClientException$message` for a `taskDefinition` that names no stored revision — the
/// refusal every operation that resolves a task-definition reference answers with.
pub const TASK_DEFINITION_NOT_FOUND_MESSAGE: &str = "Unable to describe task definition.";

/// `ClientException$message` for a `daemonTaskDefinition` that names no stored daemon
/// revision. The same condition as [`TASK_DEFINITION_NOT_FOUND_MESSAGE`] one resource
/// along, so like its neighbours it is ONE sentence whatever family and revision the
/// request named.
pub const DAEMON_TASK_DEFINITION_NOT_FOUND_MESSAGE: &str =
    "Unable to describe daemon task definition.";

/// `InvalidParameterException$message` for a `taskDefinition` that names a revision this
/// store holds in a status that cannot run new work — the `DeregisterTaskDefinition` doc:
/// "You can't use an `INACTIVE` task definition to run new tasks or create new services,
/// and you can't update an existing service to reference an `INACTIVE` task definition."
///
/// A condition that names no resource, so like its neighbours above it is ONE sentence
/// whatever family and revision the request named.
pub const TASK_DEFINITION_INACTIVE_MESSAGE: &str = "TaskDefinition is inactive";

/// Why a `taskDefinition` reference a call would put to WORK cannot be used — the two
/// conditions `usable_revision` tells apart, each with the sentence it carries.
///
/// They are two DIFFERENT faults, so they are two different modelled errors: a reference
/// that resolves to nothing names an identifier this store knows nothing about, while one
/// that resolves to a revision the store HOLDS but may no longer run names an existing
/// resource in a status this call cannot accept — a parameter whose value is wrong for the
/// call, not an unknown identifier. A caller branches on the error code to decide what to
/// do next (register the family, or point the request at a live revision), which is why
/// the two never collapse onto one code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDefinitionRefusal {
    /// The reference resolves to no stored revision at all: `ClientException`, with
    /// [`TASK_DEFINITION_NOT_FOUND_MESSAGE`].
    NotFound,
    /// The reference resolves to a stored revision whose status is not
    /// [`TASK_DEFINITION_ACTIVE`]: `InvalidParameterException`, with
    /// [`TASK_DEFINITION_INACTIVE_MESSAGE`].
    Inactive,
}

impl TaskDefinitionRefusal {
    /// The sentence this refusal carries, the same one whichever operation resolved the
    /// reference.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotFound => TASK_DEFINITION_NOT_FOUND_MESSAGE,
            Self::Inactive => TASK_DEFINITION_INACTIVE_MESSAGE,
        }
    }
}

/// `ClientException$message` for a `nextToken` a paginated read cannot resume from: a
/// value the operation never minted. Like its neighbours above it is ONE fixed sentence
/// naming the member and no value — the sentence Amazon ECS answers whatever the
/// unusable token was, which is also why the member's own doc calls a token "an opaque
/// identifier that is only used to retrieve the next items in a list and not for other
/// programmatic purposes".
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid nextToken.";

/// `ClientException$message` for a `taskDefinition` that is not a task-definition
/// reference at all — the condition [`parse_task_definition_reference`] answers `None`
/// for. DERIVED from the request: it names the member, the forms the member documents
/// ("The family and revision (family:revision) or full ARN of the task definition") and
/// the value that is none of them, so a caller sees which value was rejected.
///
/// Every operation that resolves a `taskDefinition` reference reports this ONE sentence,
/// so the refusal cannot drift between the operation that reads a revision and the ones
/// that use it.
pub fn task_definition_reference_message(reference: &str) -> String {
    revision_reference_message("taskDefinition", TASK_DEFINITION_RESOURCE_TYPE, reference)
}

/// `ClientException$message` for a `daemonTaskDefinition` that is not a daemon
/// task-definition reference at all — the condition
/// [`parse_daemon_task_definition_reference`] answers `None` for, and the same sentence
/// [`task_definition_reference_message`] states for the plain resource.
pub fn daemon_task_definition_reference_message(reference: &str) -> String {
    revision_reference_message(
        "daemonTaskDefinition",
        DAEMON_TASK_DEFINITION_RESOURCE_TYPE,
        reference,
    )
}

/// `ClientException$message` for a `taskDefinition` that names a whole FAMILY where the
/// call needs the ONE revision it acts on — the `DeregisterTaskDefinition` doc: "You must
/// specify a revision."
pub fn task_definition_revision_required_message(reference: &str) -> String {
    revision_required_message("taskDefinition", TASK_DEFINITION_RESOURCE_TYPE, reference)
}

/// `ClientException$message` for a `daemonTaskDefinition` that names a whole FAMILY where
/// the call needs the ONE revision it acts on — the `DeleteDaemonTaskDefinition$daemonTaskDefinition`
/// doc: "The family and revision (family:revision) or full Amazon Resource Name (ARN) of
/// the daemon task definition to delete", which unlike the reading operations' member
/// does not offer the family on its own.
pub fn daemon_task_definition_revision_required_message(reference: &str) -> String {
    revision_required_message(
        "daemonTaskDefinition",
        DAEMON_TASK_DEFINITION_RESOURCE_TYPE,
        reference,
    )
}

/// The sentence a reference that names a FAMILY is refused with where the call needs one
/// revision: the `member` it was given in, the forms that do name a revision and the
/// value that names none.
///
/// Spelled once for both resources whose references take those forms, so a call that acts
/// on one revision refuses a family the same way whichever resource it works on.
fn revision_required_message(member: &str, resource_type: &str, reference: &str) -> String {
    format!(
        "{member} must name a revision (family:revision or a {resource_type} ARN); \
         got {reference:?}"
    )
}

/// The sentence a reference that names no revision of `resource_type` is refused with:
/// the `member` it was given in, the forms that member documents and the value that is
/// none of them.
///
/// Spelled once for every member of this shape, so the two resources whose references
/// are "a family, a family:revision or a full ARN" refuse a value the same way.
fn revision_reference_message(member: &str, resource_type: &str, reference: &str) -> String {
    format!(
        "{member} must be a family, family:revision or {resource_type} ARN; \
         got {reference:?}"
    )
}

/// The container instance ID a `containerInstance`/`containerInstances` entry names: the
/// last segment of the value when it is an ARN
/// (`arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`), and the value
/// itself when the member carries the id — "The container instance ID or full ARN of the
/// container instance" are the two forms the member documents, and both name the same
/// instance.
pub fn container_instance_id_from_reference(reference: &str) -> &str {
    if reference.starts_with("arn:") {
        reference.rsplit('/').next().unwrap_or(reference)
    } else {
        reference
    }
}

/// `InvalidParameterException$message` for a `containerInstance` reference that names no
/// instance registered in the cluster, BUILT FROM THE REQUEST: Amazon ECS names the
/// container instance the request referred to — "Referenced container instance <id> not
/// found." — so a caller deregistering a batch can tell which entry was rejected. The
/// corpus records this refusal for `DeregisterContainerInstance` with a reference given
/// as a full ARN, and the recorded sentence carries the instance ID alone, which is why
/// the reference is reduced to its id by [`container_instance_id_from_reference`] rather
/// than echoed whole.
///
/// Every refusal in this crate whose condition is "this container-instance reference
/// resolves to no registered instance" goes through this one function, so the sentence
/// and the value it names are the same wherever the condition is met.
pub fn container_instance_not_found_message(reference: &str) -> String {
    format!(
        "Referenced container instance {} not found.",
        container_instance_id_from_reference(reference)
    )
}

/// `ClusterContainsServicesException$message`: the sentence the wire corpus records for
/// this refusal, which names no cluster. It is the RECORDED text rather than the shape's
/// doc paraphrase ("You can't delete a cluster that contains services."), because the
/// recording is what the wire carries.
pub const CLUSTER_CONTAINS_SERVICES_MESSAGE: &str =
    "The Cluster cannot be deleted while Services are active.";

/// `ClusterContainsTasksException$message`: the shape's own doc sentence, which names no
/// cluster (`smithy_query` query="shape" shape="ClusterContainsTasksException"). No
/// recording fixes this one; what the rule fixes is that the sentence is the SAME for
/// every request that meets the condition.
pub const CLUSTER_CONTAINS_TASKS_MESSAGE: &str = "You can't delete a cluster that has active tasks.";

/// `InvalidParameterException$message` when `DeleteService` is asked to delete a service
/// that is still scaled up. The operation's doc: "You can delete a service if you have no
/// running tasks in it and the desired task count is zero" — a condition of the service,
/// so the sentence names no resource either.
pub const SERVICE_SCALED_ABOVE_ZERO_MESSAGE: &str =
    "The service cannot be stopped while it is scaled above 0.";

/// `InvalidParameterException$message` for a launch of an `awsvpc` task definition that
/// carries no `networkConfiguration`. The condition names no resource — it is a property
/// of the pair (network mode, network configuration) — so every refusal of it, from
/// whichever operation launches the task, carries this one sentence.
pub const AWSVPC_NETWORK_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "Network Configuration must be provided when networkMode 'awsvpc' is specified.";

/// `InvalidParameterException$message` for a `CODE_DEPLOY`-controlled service that names
/// no load balancer. The `DeploymentController$type` doc describes the blue/green
/// deployment CodeDeploy drives by shifting traffic between two target groups, which
/// needs a load balancer in front of them.
pub const CODE_DEPLOY_LOAD_BALANCER_REQUIRED_MESSAGE: &str =
    "When specifying the CODE_DEPLOY deployment controller type, the service requires an \
     Application Load Balancer or Network Load Balancer.";

/// One capacity provider, as `CreateCapacityProvider` created it.
///
/// A provider is a named binding onto the capacity that runs tasks: either a caller's
/// own Auto Scaling group (`autoScalingGroupProvider`) or capacity Amazon ECS manages
/// (`managedInstancesProvider`). It is what a `capacityProviderStrategy` entry names,
/// and what `PutClusterCapacityProviders` associates with a cluster.
#[derive(Debug, Clone)]
pub struct CapacityProviderRecord {
    pub name: String,
    pub arn: String,
    /// `CapacityProvider$status`: [`CAPACITY_PROVIDER_ACTIVE`] for a provider that was
    /// created and not deleted.
    pub status: String,
    /// `CapacityProvider$cluster`: the cluster a MANAGED-INSTANCES provider is scoped to
    /// ("Managed instances capacity providers are cluster-scoped"). `None` for an Auto
    /// Scaling group provider, which the account holds rather than one cluster.
    pub cluster: Option<String>,
    pub auto_scaling_group_provider: Option<AutoScalingGroupProvider>,
    pub managed_instances_provider: Option<ManagedInstancesProvider>,
    /// `CapacityProvider$type`: [`CAPACITY_PROVIDER_TYPE_MANAGED_INSTANCES`] for a
    /// managed-instances provider, and nothing for an Auto Scaling group one.
    pub provider_type: Option<String>,
    pub tags: Vec<Tag>,
}

impl CapacityProviderRecord {
    /// The `CapacityProvider` every capacity-provider operation reports, built from what
    /// is stored. `include_tags` is what `DescribeCapacityProviders$include` gates
    /// (`CapacityProviderField` declares exactly `TAGS`); a create reports the tags it
    /// was given, having no `include` member to narrow them with.
    ///
    /// `updateStatus`/`updateStatusReason` stay unset: they report an update or delete in
    /// flight, and nothing is in flight for a stored, created provider.
    pub fn view(&self, include_tags: bool) -> CapacityProvider {
        CapacityProvider {
            capacity_provider_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            cluster: self.cluster.clone(),
            status: Some(self.status.clone()),
            auto_scaling_group_provider: self.auto_scaling_group_provider.clone(),
            managed_instances_provider: self.managed_instances_provider.clone(),
            update_status: None,
            update_status_reason: None,
            tags: include_tags.then(|| self.tags.clone()),
            r#type: self.provider_type.clone(),
        }
    }

    /// The Auto Scaling group this provider scales, as the ARN or the group name its
    /// `autoScalingGroupProvider` named ("The Amazon Resource Name (ARN) that identifies
    /// the Auto Scaling group, or the Auto Scaling group name"). `None` for a provider
    /// built on managed instances, which has no group of the caller's to collide with.
    pub fn auto_scaling_group(&self) -> Option<&str> {
        self.auto_scaling_group_provider
            .as_ref()
            .map(|provider| provider.auto_scaling_group_arn.as_str())
    }

    /// This provider's binding, as [`capacity_provider_binding_fingerprint`] renders it,
    /// so a repeated create can be compared against what is already stored.
    pub fn binding_fingerprint(&self) -> String {
        capacity_provider_binding_fingerprint(
            self.cluster.as_deref(),
            self.auto_scaling_group_provider.as_ref(),
            self.managed_instances_provider.as_ref(),
        )
    }
}

/// A capacity provider's BINDING, rendered as the string two creates of one name are
/// compared on.
///
/// The binding is what the provider IS: the cluster it is scoped to, plus either the Auto
/// Scaling group it scales (with that group's managed-scaling, termination-protection and
/// draining configuration) or the managed-instances configuration Amazon ECS runs for it.
/// That configuration is a deep tree — a launch template, its network, storage and
/// instance requirements — and these shapes carry a derived `Debug` rather than an
/// equality, so rendering it is what stands in for comparing it member by member: the two
/// sides are the same shape in the same member order, so equal renderings are equal
/// bindings. The Auto Scaling group is rendered through [`auto_scaling_group_key`],
/// because naming one group by ARN and by its bare name names the same group.
///
/// Tags are deliberately NOT part of it: they are metadata a create applies and
/// `TagResource`/`UntagResource` change afterwards, not part of the capacity the provider
/// binds onto.
pub fn capacity_provider_binding_fingerprint(
    cluster: Option<&str>,
    auto_scaling_group_provider: Option<&AutoScalingGroupProvider>,
    managed_instances_provider: Option<&ManagedInstancesProvider>,
) -> String {
    let group = auto_scaling_group_provider.map(|provider| {
        let mut normalised = provider.clone();
        normalised.auto_scaling_group_arn =
            auto_scaling_group_key(&provider.auto_scaling_group_arn).to_string();
        normalised
    });
    format!("cluster={cluster:?} group={group:?} managed={managed_instances_provider:?}")
}

/// The `ClientException` message a create earns when the name it asks for is held by a
/// provider bound onto DIFFERENT capacity. A fixed sentence naming nothing from the
/// request, as the recorded refusal carries it: it tells the caller to change the
/// provider that is there rather than to create it again.
pub const CAPACITY_PROVIDER_EXISTS_MESSAGE: &str =
    "The specified capacity provider already exists. To change the configuration of an \
     existing capacity provider, update the capacity provider.";

/// The `ClientException` message a create earns when the Auto Scaling group it asks for
/// is already backing another capacity provider. A fixed sentence naming nothing from the
/// request, as the recorded refusal carries it: it names the MEMBER at fault and what a
/// retry has to carry, not which provider holds the group.
pub const AUTO_SCALING_GROUP_IN_USE_MESSAGE: &str =
    "The specified Auto Scaling group ARN is already being used by another capacity \
     provider. Specify a unique Auto Scaling group ARN and try again.";

/// A cluster, as `CreateCluster` created it and every cluster-mutating operation
/// (`UpdateCluster`, `UpdateClusterSettings`, `PutClusterCapacityProviders`) has left it.
/// A DELETED cluster is still one of these, with `status` [`CLUSTER_INACTIVE`]:
/// `DeleteCluster` tombstones the record rather than dropping it, because "Clusters with
/// an INACTIVE status may remain discoverable in your account for a period of time."
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
    /// The default Service Connect namespace, as `CreateCluster` or `UpdateCluster` set
    /// it. `None` once a request clears it with an empty namespace name.
    pub service_connect_defaults: Option<ClusterServiceConnectDefaults>,
    /// `Cluster$attachmentsStatus`: `None` until a capacity-provider association is
    /// applied to this cluster, then [`ATTACHMENTS_UPDATE_IN_PROGRESS`] until the clock
    /// makes [`ATTACHMENTS_UPDATE_COMPLETE`] due.
    pub attachments_status: Option<String>,
    /// When the attachments update this cluster is in the middle of falls due. Stored,
    /// never slept on: [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]
    /// materialises it.
    pub attachments_update_due_at: Option<SystemTime>,
}

impl ClusterRecord {
    /// Apply the settings of a request to the stored cluster.
    ///
    /// `ClusterSettings` is keyed by `ClusterSetting$name`, so a request carries the
    /// settings it wants to CHANGE: one the request names replaces the stored value of
    /// that name, and one it does not name keeps it. Shared by `UpdateClusterSettings`
    /// ("Modifies the settings to use for a cluster") and `UpdateCluster$settings` ("The
    /// cluster settings for your cluster"), so both write the same way, and keyed per
    /// cluster because the record is.
    pub fn apply_settings(&mut self, settings: &[ClusterSetting]) {
        for setting in settings {
            match self
                .settings
                .iter_mut()
                .find(|stored| stored.name == setting.name)
            {
                Some(stored) => stored.value = setting.value.clone(),
                None => self.settings.push(setting.clone()),
            }
        }
    }

    /// Start the attachments update that associating capacity providers with this
    /// cluster owes, due [`TRANSITION_SPAN`] from `now`.
    ///
    /// `Cluster$attachmentsStatus` reports `UPDATE_IN_PROGRESS` while "the available
    /// capacity providers for the cluster are updating" and `UPDATE_COMPLETE` once "the
    /// capacity providers have successfully updated" — a transition the CLOCK owes, so a
    /// caller that reads straight back still sees the in-progress state.
    pub fn start_attachments_update(&mut self, now: SystemTime) {
        self.attachments_status = Some(ATTACHMENTS_UPDATE_IN_PROGRESS.to_string());
        self.attachments_update_due_at = Some(now + TRANSITION_SPAN);
    }

    /// Whether this cluster's attachments are mid-update, which is what the busy-state
    /// refusal of a cluster mutation is about: "Cluster attachments must be in
    /// UPDATE_COMPLETE or UPDATE_FAILED state before they can be updated."
    pub fn attachments_updating(&self) -> bool {
        self.attachments_status.as_deref() == Some(ATTACHMENTS_UPDATE_IN_PROGRESS)
    }

    /// The resources attached to the cluster. `Cluster$attachments`: "When using a
    /// capacity provider with a cluster, the capacity provider and associated resources
    /// are returned as cluster attachments" — the Auto Scaling group and managed
    /// instance infrastructure a capacity provider provisions. This emulator provisions
    /// no such resource (`CreateCapacityProvider` mints none), so an associated capacity
    /// provider attaches nothing and the list is empty rather than invented.
    pub fn attachments(&self) -> Vec<Attachment> {
        Vec::new()
    }
}

impl ClusterRecord {
    /// The value stored for one setting NAME on this cluster, or `None` when the cluster
    /// never set it — in which case the cluster inherits the account setting of that
    /// name (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_effective_settings`]).
    pub fn setting_value(&self, name: &str) -> Option<String> {
        self.settings
            .iter()
            .find(|setting| setting.name.as_deref() == Some(name))
            .and_then(|setting| setting.value.clone())
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
    /// The principal that registered this revision, resolved when the registration ran
    /// — not when it is read back.
    pub registered_by: String,
    pub registered_at: DateTime,
    pub deregistered_at: Option<DateTime>,
    /// "The Unix timestamp for the time when the task definition delete was requested" —
    /// absent until `DeleteTaskDefinitions` asks for one, and kept as the instant THAT
    /// call ran.
    pub delete_requested_at: Option<DateTime>,
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

/// Whether `value_from` names a Secrets Manager secret rather than a Systems Manager
/// parameter. The `Secret$valueFrom` doc: "the full ARN of the Secrets Manager secret or
/// the full ARN or name of the Systems Manager Parameter Store parameter", so the
/// service that holds it is the ARN's own service segment.
fn is_secrets_manager_reference(value_from: &str) -> bool {
    value_from
        .split(':')
        .nth(2)
        .is_some_and(|service| service == "secretsmanager")
}

/// The container-instance attributes ONE container of a revision requires, appended to
/// `out`. See [`TaskDefinitionRecord::requires_attributes`] for the rule.
fn container_required_attributes(container: &ContainerDefinition, out: &mut Vec<String>) {
    for secret in container.secrets.iter().flatten() {
        out.push(
            if is_secrets_manager_reference(&secret.value_from) {
                ATTRIBUTE_SECRETS_ASM_ENVIRONMENT
            } else {
                ATTRIBUTE_SECRETS_SSM_ENVIRONMENT
            }
            .to_string(),
        );
    }
    for file in container.environment_files.iter().flatten() {
        if file.r#type.eq_ignore_ascii_case("s3") {
            out.push(ATTRIBUTE_ENV_FILES_S3.to_string());
        }
    }
    if container.privileged == Some(true) {
        out.push(ATTRIBUTE_PRIVILEGED_CONTAINER.to_string());
    }
    if let Some(logging) = container.log_configuration.as_ref() {
        out.push(format!(
            "{ATTRIBUTE_LOGGING_DRIVER_PREFIX}{}",
            logging.log_driver
        ));
        for secret in logging.secret_options.iter().flatten() {
            out.push(
                if is_secrets_manager_reference(&secret.value_from) {
                    ATTRIBUTE_SECRETS_ASM_LOG_DRIVER
                } else {
                    ATTRIBUTE_SECRETS_SSM_LOG_DRIVER
                }
                .to_string(),
            );
        }
    }
}

/// The `ContainerDefinition$user` Amazon ECS materialises on a task's LOG ROUTER — the
/// container that carries a `firelensConfiguration`. The FireLens router runs as the root
/// user, which `user` names by its uid, as the recorded registration carries it.
pub const FIRELENS_ROUTER_USER: &str = "0";

/// Whether `network_mode` maps a container port onto the SAME host port — a static
/// mapping.
///
/// `PortMapping$hostPort`: under `awsvpc` and `host` "the hostPort can either be left
/// blank or set to the same value as the containerPort", while `bridge` binds an open
/// host port of its own choosing. Read both where a mapping is rendered and where a
/// registration is checked, so one rule decides both.
pub fn is_static_port_mapping(network_mode: &str) -> bool {
    network_mode == NETWORK_MODE_AWSVPC || network_mode == NETWORK_MODE_HOST
}

/// The `hostPort` and `protocol` Amazon ECS fills in for a port mapping the
/// registration left partly blank, under `network_mode`.
///
/// `PortMapping$protocol`: "The default is tcp." `PortMapping$hostPort`: under `awsvpc`
/// and `host` "the hostPort can either be left blank or set to the same value as the
/// containerPort" (a STATIC mapping), while under `bridge` omitting it "(or set it to 0)"
/// asks for a port out of the ephemeral range (a DYNAMIC mapping), which is picked when
/// the task is placed. A mapping that declares a `containerPortRange` instead carries no
/// single host port — the doc: "leave this field empty" — so nothing is filled in for it.
/// The `none` network mode never reaches here with a mapping: it "cannot specify port
/// mappings", so `RegisterTaskDefinition` refuses one.
fn defaulted_port_mapping(mapping: &PortMapping, network_mode: &str) -> PortMapping {
    let static_mapping = is_static_port_mapping(network_mode);
    let host_port = match (mapping.host_port, mapping.container_port) {
        (Some(declared), _) => Some(declared),
        (None, _) if mapping.container_port_range.is_some() => None,
        (None, Some(container_port)) if static_mapping => Some(container_port),
        (None, Some(_)) => Some(DYNAMIC_HOST_PORT),
        (None, None) => None,
    };
    PortMapping {
        host_port,
        protocol: Some(
            mapping
                .protocol
                .clone()
                .unwrap_or_else(|| DEFAULT_PORT_PROTOCOL.to_string()),
        ),
        ..mapping.clone()
    }
}

/// The container definitions a registration STORES: the caller's own, with the members
/// Amazon ECS fills in at registration time filled in, so every later read of the
/// revision reports the same rendered definition.
///
/// Two members are rendered here: a port mapping's ([`defaulted_port_mapping`]), and the
/// `user` of a LOG ROUTER. A container that carries a `firelensConfiguration` is the
/// task's log router ("The FireLens configuration for the container. This is used to
/// specify and configure a log router for container logs"), which Amazon ECS runs as root
/// — [`FIRELENS_ROUTER_USER`] — so the registered definition carries that user when the
/// caller named none. Nothing else is invented for a member the caller left out.
pub fn defaulted_container_definitions(
    containers: &[ContainerDefinition],
    network_mode: &str,
) -> Vec<ContainerDefinition> {
    containers
        .iter()
        .map(|container| ContainerDefinition {
            port_mappings: container.port_mappings.as_ref().map(|mappings| {
                mappings
                    .iter()
                    .map(|mapping| defaulted_port_mapping(mapping, network_mode))
                    .collect()
            }),
            user: match (&container.user, &container.firelens_configuration) {
                (Some(user), _) => Some(user.clone()),
                (None, Some(_)) => Some(FIRELENS_ROUTER_USER.to_string()),
                (None, None) => None,
            },
            ..container.clone()
        })
        .collect()
}

impl TaskDefinitionRecord {
    /// The launch types this revision was validated against — `TaskDefinition$
    /// compatibilities`: "Amazon ECS validates the task definition parameters with
    /// those supported by the launch type."
    ///
    /// Derived from the STORED revision and reported in [`COMPATIBILITY_ORDER`], by the
    /// rules the model's own docs state: Fargate needs `awsvpc` ("For Amazon ECS tasks
    /// on Fargate, the awsvpc network mode is required") plus task-level `cpu` and
    /// `memory` ("This field is required for Fargate"); ECS Managed Instances place
    /// tasks on AWS-managed capacity that gives each task its own network interface, so
    /// they likewise need `awsvpc`; `awsvpc` is in turn not available to the external
    /// (on-premises) launch type. EC2 supports every network mode, so it is always
    /// present.
    pub fn compatibilities(&self) -> Vec<String> {
        let awsvpc = self.network_mode == NETWORK_MODE_AWSVPC;
        COMPATIBILITY_ORDER
            .iter()
            .filter(|launch_type| match **launch_type {
                "EC2" => true,
                "EXTERNAL" => !awsvpc,
                "MANAGED_INSTANCES" => awsvpc,
                "FARGATE" => awsvpc && self.cpu.is_some() && self.memory.is_some(),
                // Every launch type is classified above by its own documented
                // requirements, so one added to [`COMPATIBILITY_ORDER`] later has to be
                // classified deliberately rather than inheriting a neighbour's rule.
                _ => false,
            })
            .map(|launch_type| (*launch_type).to_string())
            .collect()
    }

    /// The container-instance attributes a task of this revision requires —
    /// `TaskDefinition$requiresAttributes`: "The container instance attributes required
    /// by your task."
    ///
    /// DERIVED from the stored revision: every feature it declares that only a container
    /// instance publishing a capability can serve contributes that capability's
    /// attribute — the `awsvpc` network mode needs [`ATTRIBUTE_TASK_ENI`] (and with it
    /// the Docker remote-API version that carries task networking), a `secrets` or
    /// `secretOptions` reference needs the attribute of the secret store it names, an
    /// S3 `environmentFiles` entry needs [`ATTRIBUTE_ENV_FILES_S3`], a privileged
    /// container needs [`ATTRIBUTE_PRIVILEGED_CONTAINER`] and a named `logDriver` needs
    /// that driver's attribute. The names are the Amazon ECS container-agent
    /// capabilities, which is what an instance registers them as, so
    /// `ListAttributes`/`DescribeContainerInstances` and this list speak the same
    /// vocabulary.
    ///
    /// Reported deduplicated and sorted by name, so a revision reads back the same list
    /// however its containers were ordered. A revision that declares no such feature
    /// requires nothing and the member stays ABSENT (`None`) rather than reporting an
    /// empty list.
    pub fn requires_attributes(&self) -> Option<Vec<Attribute>> {
        let mut names = Vec::new();
        if self.network_mode == NETWORK_MODE_AWSVPC {
            names.push(ATTRIBUTE_DOCKER_REMOTE_API_1_18.to_string());
            names.push(ATTRIBUTE_TASK_ENI.to_string());
        }
        for container in &self.container_definitions {
            container_required_attributes(container, &mut names);
        }
        names.sort();
        names.dedup();
        if names.is_empty() {
            return None;
        }
        Some(
            names
                .into_iter()
                .map(|name| Attribute {
                    name,
                    ..Attribute::default()
                })
                .collect(),
        )
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
            requires_attributes: self.requires_attributes(),
            placement_constraints: Some(self.placement_constraints.clone()),
            compatibilities: Some(self.compatibilities()),
            runtime_platform: self.runtime_platform.clone(),
            // "The task launch types the task definition was validated against" is
            // `compatibilities`, which Amazon ECS DERIVES; `requiresCompatibilities` is
            // what the REGISTRATION asked for, so a registration that asked for nothing
            // surfaces nothing rather than an explicit empty list the caller never sent.
            requires_compatibilities: (!self.requires_compatibilities.is_empty())
                .then(|| self.requires_compatibilities.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            inference_accelerators: None,
            pid_mode: self.pid_mode.clone(),
            ipc_mode: self.ipc_mode.clone(),
            proxy_configuration: self.proxy_configuration.clone(),
            registered_at: Some(self.registered_at),
            deregistered_at: self.deregistered_at,
            delete_requested_at: self.delete_requested_at,
            // "The principal that registered the task definition", as it was resolved
            // when the registration ran.
            registered_by: Some(self.registered_by.clone()),
            ephemeral_storage: self.ephemeral_storage.clone(),
            enable_fault_injection: self.enable_fault_injection,
        }
    }
}

/// A DAEMON task-definition revision, as `RegisterDaemonTaskDefinition` stored it.
///
/// Its own resource beside [`TaskDefinitionRecord`]: "A daemon task definition is a
/// template that describes the containers that form a daemon", registered under its own
/// families, revisions and ARNs, and described by its own operation — so a daemon
/// revision and a plain revision of the same family name are two different records.
#[derive(Debug, Clone)]
pub struct DaemonTaskDefinitionRecord {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub container_definitions: Vec<DaemonContainerDefinition>,
    pub volumes: Vec<DaemonVolume>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    /// The namespace modes in effect for this revision, with the default each member
    /// documents already applied ([`DAEMON_NAMESPACE_MODE_NONE`]), so every read reports
    /// the mode the daemon actually runs in.
    pub pid_mode: String,
    pub ipc_mode: String,
    /// "The metadata that you apply to the daemon task definition": stored with the
    /// revision, which is the only place a later read could get it from.
    pub tags: Vec<Tag>,
    /// The principal that registered this revision, resolved when the registration ran
    /// — not when it is read back.
    pub registered_by: String,
    pub registered_at: DateTime,
    /// "The Unix timestamp for the time when the daemon task definition delete was
    /// requested" — absent until a delete is asked for.
    pub delete_requested_at: Option<DateTime>,
    /// When [`DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS`] ->
    /// [`DAEMON_TASK_DEFINITION_DELETED`] falls due; `None` both for a revision no delete
    /// was asked for and for one whose delete has already been materialised. Stored and
    /// compared, never slept on
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]).
    pub deleted_due_at: Option<SystemTime>,
}

impl DaemonTaskDefinitionRecord {
    /// Record the delete a `DeleteDaemonTaskDefinition` call asked for at `now`: the
    /// revision enters [`DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS`], keeps the instant
    /// the delete was requested, and owes the clock its completion
    /// [`TRANSITION_SPAN`] later — so a caller reading straight after the call still sees
    /// the delete running.
    pub fn request_delete(&mut self, now: SystemTime) {
        self.status = DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS.to_string();
        self.delete_requested_at = Some(DateTime::from(now));
        self.deleted_due_at = Some(now + TRANSITION_SPAN);
    }

    /// The `DaemonTaskDefinition` every daemon task-definition operation returns, built
    /// from what is stored — never from the request asking for it.
    pub fn view(&self) -> DaemonTaskDefinition {
        DaemonTaskDefinition {
            daemon_task_definition_arn: Some(self.arn.clone()),
            family: Some(self.family.clone()),
            revision: self.revision,
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            container_definitions: Some(self.container_definitions.clone()),
            // A registration that declared no volumes reads back with an EMPTY LIST, the
            // same shape a plain revision's `volumes` takes, so a client may index it
            // unconditionally.
            volumes: Some(self.volumes.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            status: Some(self.status.clone()),
            registered_at: Some(self.registered_at),
            delete_requested_at: self.delete_requested_at,
            registered_by: Some(self.registered_by.clone()),
            pid_mode: Some(self.pid_mode.clone()),
            ipc_mode: Some(self.ipc_mode.clone()),
        }
    }

    /// The `DaemonTaskDefinitionSummary` a listing reports for this revision — "A summary
    /// of a daemon task definition", built from the same stored values as [`Self::view`]
    /// so a listing and a describe of one revision can never disagree.
    pub fn summary(&self) -> DaemonTaskDefinitionSummary {
        DaemonTaskDefinitionSummary {
            arn: Some(self.arn.clone()),
            registered_at: Some(self.registered_at),
            registered_by: Some(self.registered_by.clone()),
            delete_requested_at: self.delete_requested_at,
            status: Some(self.status.clone()),
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
    /// When `PROVISIONING` -> `PENDING` falls due: set when the capacity this task was
    /// parked waiting for registers into its cluster, and `None` both for a task that was
    /// placed straight away and for one still waiting for capacity to appear.
    pub pending_due_at: Option<SystemTime>,
    /// When `PENDING` -> `RUNNING` falls due; `None` once it has been materialised, and
    /// `None` for a task that has not been placed yet.
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
    /// The IAM role the CREATE named, resolved to an ARN, and `None` when it named none
    /// — in which case the service-linked role is reported for exactly the services that
    /// need one (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_role_arn`]).
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
    /// The deployments this one REPLACED and whose tasks are still being replaced,
    /// newest first. `Deployment$status`: "ACTIVE: A service deployment that still has
    /// running tasks, but are in the process of being replaced with a new launch type,
    /// platform version, task definition, or Docker image" — so an entry lives here for
    /// the span that replacement takes, and the clock pass that finds that span elapsed
    /// drops it ("INACTIVE: A deployment that has been completely replaced").
    pub superseded_deployments: Vec<SupersededDeployment>,
}

/// A deployment a later one replaced, kept beside the service while the task set it
/// placed is still being replaced.
///
/// It holds the configuration that deployment rolled out, which is NOT the service's
/// current configuration any more — that is the point of reporting it: a client watching a
/// rolling update reads the old deployment's task definition, launch type and network
/// configuration beside the new one's.
#[derive(Debug, Clone)]
pub struct SupersededDeployment {
    /// The `Deployment$id` it was reported under, and the `startedBy` of every task it
    /// placed — which is how its remaining tasks are counted.
    pub id: String,
    pub task_definition_arn: Option<String>,
    pub desired_count: i32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub volume_configurations: Vec<ServiceVolumeConfiguration>,
    pub vpc_lattice_configurations: Vec<VpcLatticeConfiguration>,
    /// Whether this rollout had already reached its steady state when it was superseded,
    /// so the `rolloutState` it is reported with is the one it actually got to rather
    /// than one asserted now.
    pub rollout_completed: bool,
    /// When the task set this deployment placed has been replaced, and so when the
    /// deployment stops being one of the service's: the clock-owed span of the stop the
    /// scheduler asked its tasks for ([`TRANSITION_SPAN`]), stored rather than slept on.
    pub replaced_due_at: SystemTime,
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

    /// Who manages this service (`Service$resourceManagementType`). Every service this
    /// slice holds came from `CreateService`, so it is the customer's
    /// ([`RESOURCE_MANAGEMENT_CUSTOMER`]); an ECS-managed Express service comes from
    /// `CreateExpressGatewayService`, which is not part of this slice. Read by both the
    /// projection and the `ListServices` `resourceManagementType` filter, so the value
    /// reported and the value filtered on cannot drift.
    pub fn resource_management_type(&self) -> &'static str {
        RESOURCE_MANAGEMENT_CUSTOMER
    }

    /// Whether this service's tasks run on Fargate — the launch type the request named,
    /// or the Fargate capacity providers a strategy names ("When you use Fargate, the
    /// capacity providers are FARGATE or FARGATE_SPOT").
    ///
    /// This is what gates the members AWS documents as Fargate-only: `platformFamily`
    /// ("A platform family is specified only for tasks using the Fargate launch type")
    /// and `platformVersion` ("A platform version is only specified for tasks using the
    /// Fargate launch type").
    pub fn runs_on_fargate(&self) -> bool {
        self.launch_type.as_deref() == Some(LAUNCH_TYPE_FARGATE)
            || self.capacity_provider_strategy.iter().any(|item| {
                MANAGED_CAPACITY_PROVIDERS.contains(&item.capacity_provider.as_str())
            })
    }

    /// Move the deployment this service currently reports as `PRIMARY` onto
    /// [`Self::superseded_deployments`], as the deployment a new one replaces.
    ///
    /// Called by the operation that starts a new deployment, BEFORE the record is moved
    /// onto the new deployment's id and configuration, so the entry holds what that
    /// rollout was actually rolling out.
    pub fn supersede_deployment(&mut self, now: SystemTime) {
        self.superseded_deployments.insert(
            0,
            SupersededDeployment {
                id: self.deployment_id.clone(),
                task_definition_arn: self.task_definition_arn.clone(),
                desired_count: self.desired_count,
                created_at: self.deployment_created_at,
                updated_at: self.deployment_updated_at,
                launch_type: self.launch_type.clone(),
                capacity_provider_strategy: self.capacity_provider_strategy.clone(),
                platform_version: self.platform_version.clone(),
                network_configuration: self.network_configuration.clone(),
                service_connect_configuration: self.service_connect_configuration.clone(),
                volume_configurations: self.volume_configurations.clone(),
                vpc_lattice_configurations: self.vpc_lattice_configurations.clone(),
                rollout_completed: self.deployment_steady_due_at.is_none(),
                replaced_due_at: now + TRANSITION_SPAN,
            },
        );
        self.superseded_deployments
            .truncate(MAX_SUPERSEDED_DEPLOYMENTS);
    }
}

/// One SERVICE DEPLOYMENT: the rollout of ONE service revision, as the call that started
/// it wrote it and the clock (or a later deployment) left it.
///
/// `ListServiceDeployments`: "A service deployment happens when you release a software
/// update for the service. You route traffic from the running service revisions to the new
/// service revison" — so a record is written by the call that STARTS a rollout
/// (`CreateService`, and an `UpdateService` that forces a new deployment or moves the
/// service onto another task definition), and never by a read. Scaling a service places
/// or stops tasks without changing the revision it runs, so it starts no deployment.
///
/// The record is the service's own `PRIMARY` deployment seen as a resource of its own:
/// [`Self::deployment_id`] is the `Deployment$id` the service reports, so the status this
/// record carries and the `rolloutState` the service's `Deployment` carries are two views
/// of one rollout and settle on the same instant (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::settle_services`]).
#[derive(Debug, Clone)]
pub struct ServiceDeploymentRecord {
    /// `arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<id>`.
    pub arn: String,
    /// The `Deployment$id` (`ecs-svc/...`) of the service deployment this records.
    pub deployment_id: String,
    pub service_arn: String,
    pub service_name: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    /// When the call that started the rollout created it. The rollout begins with that
    /// same call — the ECS scheduler starts replacing tasks as soon as it is given the
    /// new revision — so it is also the `startedAt` the brief reports.
    pub created_at: DateTime,
    /// `ServiceDeploymentBrief$finishedAt`: "The time that the service deployment
    /// completed", and so `None` for exactly the deployments still under way.
    pub finished_at: Option<DateTime>,
    /// `ServiceDeploymentBrief$targetServiceRevisionArn`: "The ARN of the service revision
    /// being deplyed" — the revision minted with this deployment.
    pub target_service_revision_arn: String,
    /// One of `ServiceDeploymentStatus`; see [`SERVICE_DEPLOYMENT_IN_PROGRESS`].
    pub status: String,
    /// `ServiceDeploymentBrief$statusReason`: "Information about why the service deployment
    /// is in the current status."
    pub status_reason: String,
}

impl ServiceDeploymentRecord {
    /// Whether this rollout has ended — which is exactly when it carries a `finishedAt`,
    /// so a reported status and a reported finish time can never disagree.
    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// The key a listing orders deployments by: the `createdAt` the brief reports, with
    /// the ARN breaking a tie between two deployments created in the same instant (its
    /// last segment is minted from a monotonic fixed-width counter, so the later
    /// deployment always sorts after the earlier one).
    pub fn order_key(&self) -> ((i64, u32), &str) {
        (timestamp_key(self.created_at), self.arn.as_str())
    }

    /// The `ServiceDeploymentBrief` `ListServiceDeployments` reports for this deployment,
    /// built from what is stored.
    pub fn brief(&self) -> ServiceDeploymentBrief {
        ServiceDeploymentBrief {
            service_deployment_arn: Some(self.arn.clone()),
            service_arn: Some(self.service_arn.clone()),
            cluster_arn: Some(self.cluster_arn.clone()),
            started_at: Some(self.created_at),
            created_at: Some(self.created_at),
            finished_at: self.finished_at,
            target_service_revision_arn: Some(self.target_service_revision_arn.clone()),
            status: Some(self.status.clone()),
            status_reason: Some(self.status_reason.clone()),
        }
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

/// Which optional task-set fields a response carries, as `DescribeTaskSets$include`
/// (the `TaskSetField` enum: `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskSetFields {
    pub tags: bool,
}

impl TaskSetFields {
    /// Every optional field — what the operation that CREATED the task set returns,
    /// since the caller just supplied them and `CreateTaskSet` has no `include` member
    /// to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `TaskSetField` enum.
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

/// A service's `include` selection carries over to the task sets reported INSIDE it:
/// `DescribeServices$include` and `DescribeTaskSets$include` name the same `TAGS` field,
/// so a request that asked for the service's tags asked for its task sets' too.
impl From<ServiceFields> for TaskSetFields {
    fn from(fields: ServiceFields) -> Self {
        Self { tags: fields.tags }
    }
}

/// A set of tasks of ONE service, as `CreateTaskSet` created it.
///
/// The record holds the task set's CONFIGURATION only. `computedDesiredCount` is derived
/// from the service's `desiredCount` and this scale, and `runningCount`/`pendingCount`
/// are counted off the tasks this task set actually placed (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::task_set_view`]), so a
/// reported count can never drift from the records that exist.
#[derive(Debug, Clone)]
pub struct TaskSetRecord {
    /// `TaskSet$id`, an `ecs-svc/` identifier like a deployment's.
    pub id: String,
    pub arn: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    pub service_name: String,
    pub service_arn: String,
    pub external_id: Option<String>,
    pub status: String,
    pub task_definition_arn: String,
    /// The family and revision the ARN above names, kept so the scheduler can read the
    /// revision back when it places a task — as [`ServiceRecord::task_definition`] is.
    pub task_definition: (String, i32),
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    pub platform_version: Option<String>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub load_balancers: Vec<LoadBalancer>,
    pub service_registries: Vec<ServiceRegistry>,
    /// `TaskSet$scale`: "A floating-point percentage of your desired number of tasks to
    /// place and keep running in the task set."
    pub scale: Scale,
    pub tags: Vec<Tag>,
    /// The `clientToken` the create carried, so repeating that create is idempotent
    /// ("An identifier that you provide to ensure the idempotency of the request").
    pub client_token: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl TaskSetRecord {
    /// The key this record is stored under: the cluster, the service and the task set's
    /// own id — the three things every task-set operation addresses one by.
    pub fn key(&self) -> (String, String, String) {
        (
            self.cluster_name.clone(),
            self.service_name.clone(),
            self.id.clone(),
        )
    }

    /// Whether `reference` names this task set. The `taskSets` doc: "The ID or full
    /// Amazon Resource Name (ARN) of task sets to describe."
    pub fn is_named_by(&self, reference: &str) -> bool {
        let reference = reference.trim();
        reference == self.id || reference == self.arn
    }
}

/// Where ONE placement takes its capacity from, and whose task it is.
///
/// A service's own scheduler and a task set place tasks the same way and differ only in
/// this: the launch type or capacity provider the placement runs under, the platform
/// version it asks for, and the `startedBy` the placed task carries.
#[derive(Debug, Clone)]
pub struct Placement {
    pub launch_type: Option<String>,
    pub capacity_provider_name: Option<String>,
    pub platform_version: Option<String>,
    /// `Task$startedBy` of every task this placement places: the deployment id for a
    /// service's own scheduler, the task-set id for a task set's.
    pub started_by: String,
}

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
    /// `capacityProviderName` -> the capacity provider. Provider names are unique within
    /// the account ("The name of the capacity provider" identifies it, and a
    /// `capacityProviderStrategy` entry names one by that name alone), which is exactly
    /// this key. Distinct from `ClusterRecord::capacity_providers`, which only records
    /// WHICH of these a cluster is associated with.
    pub capacity_providers: HashMap<String, CapacityProviderRecord>,
    /// `(family, revision)` -> the registered revision.
    pub task_definitions: HashMap<(String, i32), TaskDefinitionRecord>,
    /// `family` -> the revision the next `RegisterTaskDefinition` for it will take.
    /// Kept beside `task_definitions` so a revision number is never reused, even after
    /// a revision is deregistered (`TaskDefinition$revision`: "the revision value
    /// always increases by one. This is even if you deregistered previous revisions").
    pub next_revision: HashMap<String, i32>,
    /// `(family, revision)` -> the registered DAEMON task-definition revision. Daemon
    /// task definitions are their own resource (see [`DaemonTaskDefinitionRecord`]), so
    /// they are held beside `task_definitions` rather than in it: the same family name
    /// can carry both, and a reference of one kind never resolves to the other's record.
    pub daemon_task_definitions: HashMap<(String, i32), DaemonTaskDefinitionRecord>,
    /// `family` -> the revision the next `RegisterDaemonTaskDefinition` for it will
    /// take. Kept beside `daemon_task_definitions` for the reason `next_revision` is
    /// kept beside `task_definitions`: a revision number is never reused, even after the
    /// revision that held it is deleted.
    pub next_daemon_revision: HashMap<String, i32>,
    /// `taskArn` -> the task.
    pub tasks: HashMap<String, TaskRecord>,
    /// `containerInstanceArn` -> the registered container instance.
    pub container_instances: HashMap<String, ContainerInstanceRecord>,
    /// `(clusterName, serviceName)` -> the service. Service names are unique within a
    /// cluster ("Service names must be unique within a cluster, but you can have
    /// similarly named services in multiple clusters"), which is exactly this key.
    pub services: HashMap<(String, String), ServiceRecord>,
    /// `(clusterName, serviceName, taskSetId)` -> the task set. A task set belongs to one
    /// service of one cluster — the three members every task-set operation addresses one
    /// by — and `CreateTaskSet` mints an id that is unique across them all.
    pub task_sets: HashMap<(String, String, String), TaskSetRecord>,
    /// `serviceDeploymentArn` -> the service deployment. A service's rollouts are its
    /// HISTORY, so a record outlives the rollout it names (`ListServiceDeployments` lists
    /// finished deployments as well as the one under way) and the key is the ARN the
    /// listing reports.
    pub service_deployments: HashMap<String, ServiceDeploymentRecord>,
    /// `(principalArn, settingName)` -> the account setting value stored for that ONE
    /// principal by `PutAccountSetting`. An entry here shadows the account-wide default
    /// for that principal alone (`ListAccountSettings$effectiveSettings`: "the account
    /// settings for the root user or the default setting for the `principalArn`").
    pub principal_account_settings: HashMap<(String, String), String>,
    /// `settingName` -> the ACCOUNT-WIDE value, as `PutAccountSettingDefault`
    /// ("Modifies an account setting for all users on an account for whom no individual
    /// account setting has been specified") or a `PutAccountSetting` on the root user
    /// stored it. A name with no entry takes its [`ACCOUNT_SETTINGS`] default.
    pub account_setting_defaults: HashMap<String, String>,
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
    /// Monotonic source of service-deployment ids, so every service deployment of every
    /// service is distinct.
    next_service_deployment_seq: u64,
    /// Monotonic source of service-revision ids, so every revision of every service is
    /// distinct.
    next_service_revision_seq: u64,
    /// Monotonic source of service-event ids, so every event of every service is
    /// distinct.
    next_service_event_seq: u64,
    /// Monotonic source of task-set ids, so every task set of every service is distinct.
    next_task_set_seq: u64,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Materialise every lifecycle transition the clock has already made due, as of
    /// `now`. Safe to call from a reporting operation: it only applies changes that
    /// were owed at this instant whether or not anybody called.
    pub fn advance_clock(&mut self, now: SystemTime) {
        // The capacity-provider association a cluster mutation started: once the update
        // is due the cluster's attachments have "successfully updated", whether or not
        // anybody called to look.
        for cluster in self.clusters.values_mut() {
            if let Some(due) = cluster.attachments_update_due_at {
                if due <= now {
                    cluster.attachments_update_due_at = None;
                    cluster.attachments_status = Some(ATTACHMENTS_UPDATE_COMPLETE.to_string());
                }
            }
        }
        for task in self.tasks.values_mut() {
            // The placement a parked task was owed from the instant capacity registered
            // into its cluster: it has been placed, so it is `PENDING` and its start is
            // owed from there like any other placed task's.
            if let Some(due) = task.pending_due_at {
                if due <= now {
                    task.pending_due_at = None;
                    task.last_status = STATUS_PENDING.to_string();
                    for container in &mut task.containers {
                        container.last_status = STATUS_PENDING.to_string();
                    }
                    task.running_due_at = Some(due + TRANSITION_SPAN);
                    task.version += 1;
                }
            }
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
        // The delete a `DeleteDaemonTaskDefinition` call asked for: once it is due the
        // revision has been deleted, whether or not anybody called to look. Terminal, so
        // the due instant is cleared with it and the status never moves again.
        for record in self.daemon_task_definitions.values_mut() {
            if let Some(due) = record.deleted_due_at {
                if due <= now {
                    record.deleted_due_at = None;
                    record.status = DAEMON_TASK_DEFINITION_DELETED.to_string();
                }
            }
        }
        self.settle_services(now);
    }

    /// Materialise the SERVICE-level transitions the clock owes, as of `now`: the steady
    /// state a started rollout settles into — which is also where the SERVICE DEPLOYMENT
    /// that rollout is recorded as becomes [`SERVICE_DEPLOYMENT_SUCCESSFUL`] — and the
    /// `INACTIVE` a drained service reaches once the tasks it was still cleaning up have
    /// stopped.
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
            // The SAME rollout, as the service-deployment resource: it finishes with the
            // steady state the service reaches, not a moment before or after.
            let settled_deployment = settled.and_then(|_| {
                self.current_service_deployment(service)
                    .map(|deployment| deployment.arn.clone())
            });
            // A superseded deployment is reported while it is "in the process of being
            // replaced"; once that replacement is due it "has been completely replaced"
            // and is no longer one of the service's deployments. The instant it falls due
            // is stored with the deployment, so this pass applies what the clock already
            // owed whether or not anybody called.
            let replaced: Vec<String> = service
                .superseded_deployments
                .iter()
                .filter(|superseded| superseded.replaced_due_at <= now)
                .map(|superseded| superseded.id.clone())
                .collect();

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
            if !replaced.is_empty() {
                service
                    .superseded_deployments
                    .retain(|superseded| !replaced.contains(&superseded.id));
            }
            // The rollout's own record ends where the rollout did. Guarded on an OPEN
            // deployment, so a rollout the clock already finished is not re-finished when
            // a later scale-out puts the service back through a steady state of the same
            // deployment.
            if let (Some(due), Some(arn)) = (settled, settled_deployment) {
                if let Some(deployment) = self.service_deployments.get_mut(&arn) {
                    if !deployment.is_finished() {
                        deployment.status = SERVICE_DEPLOYMENT_SUCCESSFUL.to_string();
                        deployment.status_reason =
                            rollout_state_reason(&deployment.deployment_id, true);
                        deployment.finished_at = Some(DateTime::from(due));
                    }
                }
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

    /// Mint the id of a service's next deployment. `Deployment$id` is an `ecs-svc/`
    /// identifier; the counter is what makes every deployment of every service distinct.
    pub fn next_deployment_id(&mut self) -> String {
        self.next_deployment_seq += 1;
        format!("ecs-svc/{:019}", self.next_deployment_seq)
    }

    /// Mint the id of the next task set. `TaskSet$id` is "The ID of the task set", an
    /// `ecs-svc/` identifier like a deployment's; the counter is what makes every task
    /// set of every service distinct.
    pub fn next_task_set_id(&mut self) -> String {
        self.next_task_set_seq += 1;
        format!("ecs-svc/{:019}", self.next_task_set_seq)
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

    /// Mint the ARN of the SERVICE DEPLOYMENT that records one rollout. The counter is
    /// what makes every deployment of every service distinct, and it is fixed-width, so
    /// sorting two of these ARNs orders the deployments they name oldest first.
    pub fn next_service_deployment_arn(
        &mut self,
        region: &str,
        cluster_name: &str,
        service_name: &str,
    ) -> String {
        self.next_service_deployment_seq += 1;
        service_deployment_arn(
            region,
            cluster_name,
            service_name,
            &format!("{:019}", self.next_service_deployment_seq),
        )
    }

    /// Record the SERVICE DEPLOYMENT of the rollout the CALLER just started for `key`,
    /// and answer its ARN.
    ///
    /// Called by `CreateService` and by an `UpdateService` that starts a new deployment,
    /// AFTER the service record carries the deployment id and the service revision that
    /// rollout belongs to — those two are what the record is built from, so the
    /// deployment a listing reports and the one the service reports cannot drift.
    ///
    /// Only the ECS scheduler's own rollout is a service deployment: the other deployment
    /// controllers' tasks come from a task set ([`ServiceRecord::places_tasks`]), so there
    /// is no rollout of a service revision to record and this answers `None`.
    pub fn start_service_deployment(
        &mut self,
        key: &(String, String),
        region: &str,
        at: DateTime,
    ) -> Option<String> {
        let service = self.services.get(key)?;
        if !service.places_tasks() {
            return None;
        }
        let deployment_id = service.deployment_id.clone();
        let service_arn = service.arn.clone();
        let service_name = service.name.clone();
        let cluster_name = service.cluster_name.clone();
        let cluster_arn = service.cluster_arn.clone();
        let target_service_revision_arn = service.service_revision_arn.clone();
        // A rollout this one replaces never reaches the steady state it was rolling out:
        // its tasks are the ones `scale_service` stops as superseded, so it ends here
        // rather than staying open for a steady state that can no longer arrive.
        self.finish_open_deployments(
            &service_arn,
            at,
            SERVICE_DEPLOYMENT_STOPPED,
            &format!("ECS deployment {deployment_id} replaced this deployment."),
        );
        let arn = self.next_service_deployment_arn(region, &cluster_name, &service_name);
        self.service_deployments.insert(
            arn.clone(),
            ServiceDeploymentRecord {
                arn: arn.clone(),
                status_reason: rollout_state_reason(&deployment_id, false),
                deployment_id,
                service_arn: service_arn.clone(),
                service_name,
                cluster_name,
                cluster_arn,
                created_at: at,
                finished_at: None,
                target_service_revision_arn,
                status: SERVICE_DEPLOYMENT_IN_PROGRESS.to_string(),
            },
        );
        self.prune_service_deployments(&service_arn);
        Some(arn)
    }

    /// End every rollout of `service_arn` that has not finished yet, at `at`.
    ///
    /// The two things that end one early are a deployment that REPLACES it and the DELETE
    /// of the service it was rolling out; both are calls, so neither is a transition the
    /// clock owes.
    fn finish_open_deployments(
        &mut self,
        service_arn: &str,
        at: DateTime,
        status: &str,
        reason: &str,
    ) {
        for deployment in self.service_deployments.values_mut() {
            if deployment.service_arn == service_arn && !deployment.is_finished() {
                deployment.status = status.to_string();
                deployment.status_reason = reason.to_string();
                deployment.finished_at = Some(at);
            }
        }
    }

    /// Drop the oldest of `service_arn`'s deployments down to
    /// [`MAX_SERVICE_DEPLOYMENTS_PER_SERVICE`], the way a service's event stream keeps
    /// only its latest entries.
    fn prune_service_deployments(&mut self, service_arn: &str) {
        let mut arns: Vec<String> = self
            .service_deployments
            .values()
            .filter(|deployment| deployment.service_arn == service_arn)
            .map(|deployment| deployment.arn.clone())
            .collect();
        if arns.len() <= MAX_SERVICE_DEPLOYMENTS_PER_SERVICE {
            return;
        }
        // The ARN ends with a fixed-width monotonic id, so sorting orders them oldest
        // first and the ones to drop are the leading entries.
        arns.sort();
        let first_kept = arns.len() - MAX_SERVICE_DEPLOYMENTS_PER_SERVICE;
        for arn in &arns[..first_kept] {
            self.service_deployments.remove(arn);
        }
    }

    /// The stored SERVICE DEPLOYMENT of `service`'s current (`PRIMARY`) deployment, if
    /// this service has one — `Service$currentServiceDeployment` names it, and the
    /// rollout it records is the one the service's `Deployment` reports.
    pub fn current_service_deployment(
        &self,
        service: &ServiceRecord,
    ) -> Option<&ServiceDeploymentRecord> {
        self.service_deployments.values().find(|deployment| {
            deployment.service_arn == service.arn
                && deployment.deployment_id == service.deployment_id
        })
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

    /// Record the placement every task PARKED in `cluster_name` is now owed, because the
    /// capacity its capacity provider had to acquire has just registered into the cluster.
    ///
    /// The transition falls due [`TRANSITION_SPAN`] from `now` and
    /// [`Self::advance_clock`] materialises it, so nothing here moves a status that is not
    /// owed yet. A task the caller has already asked to stop is not placed: its placement
    /// was cancelled, not merely overtaken.
    pub fn place_tasks_awaiting_capacity(&mut self, cluster_name: &str, now: SystemTime) {
        for task in self.tasks.values_mut() {
            if task.cluster_name == cluster_name
                && task.last_status == STATUS_PROVISIONING
                && task.desired_status == STATUS_RUNNING
                && task.pending_due_at.is_none()
            {
                task.pending_due_at = Some(now + TRANSITION_SPAN);
            }
        }
    }

    /// How many container instances `cluster_name` holds in a registered status.
    pub fn registered_container_instances(&self, cluster_name: &str) -> usize {
        self.container_instances
            .values()
            .filter(|instance| instance.cluster_name == cluster_name && instance.is_registered())
            .count()
    }

    /// The stored key of the container instance a `containerInstance` reference names,
    /// if this store holds it in `cluster_name`.
    ///
    /// Every such member's doc: "The container instance ID or full ARN of the container
    /// instance" — so a reference is either form — and the `cluster` member scopes the
    /// request: "If you do not specify a cluster, the default cluster is assumed." An
    /// instance registered into another cluster is not one this request names.
    pub fn resolve_container_instance(
        &self,
        region: &str,
        cluster_name: &str,
        reference: &str,
    ) -> Option<&ContainerInstanceRecord> {
        let arn = container_instance_arn_from_reference(region, cluster_name, reference);
        self.container_instances
            .get(&arn)
            .filter(|instance| instance.cluster_name == cluster_name)
    }

    /// The latest `ACTIVE` revision registered for `family`, if any.
    pub fn latest_active_revision(&self, family: &str) -> Option<&TaskDefinitionRecord> {
        self.task_definitions
            .values()
            .filter(|record| record.family == family && record.status == TASK_DEFINITION_ACTIVE)
            .max_by_key(|record| record.revision)
    }

    /// The latest `ACTIVE` daemon revision registered for `family`, if any. The
    /// `DescribeDaemonTaskDefinition` doc: "you can simply specify the family to find the
    /// latest ACTIVE revision in that family".
    pub fn latest_active_daemon_revision(
        &self,
        family: &str,
    ) -> Option<&DaemonTaskDefinitionRecord> {
        self.daemon_task_definitions
            .values()
            .filter(|record| {
                record.family == family && record.status == DAEMON_TASK_DEFINITION_ACTIVE
            })
            .max_by_key(|record| record.revision)
    }

    /// The revision a `taskDefinition` reference names for a call that would put it to
    /// WORK — run a task of it, or point a service at it — or the `ClientException`
    /// message that refuses the reference.
    ///
    /// Two conditions, one per [`TaskDefinitionRefusal`]. A reference that resolves to no
    /// stored revision is [`TaskDefinitionRefusal::NotFound`]; one that resolves to a
    /// revision whose status is not [`TASK_DEFINITION_ACTIVE`] is
    /// [`TaskDefinitionRefusal::Inactive`], because "you can't use an `INACTIVE` task
    /// definition to run new tasks or create new services, and you can't update an
    /// existing service to reference an `INACTIVE` task definition" — a deregistered
    /// revision stays readable (`DescribeTaskDefinition` keeps describing it) but is no
    /// longer usable. A reference that names no revision takes "the latest `ACTIVE`
    /// revision", so it can only ever resolve to a usable one.
    ///
    /// A service ALREADY pointed at a revision is untouched by this: it keeps the key it
    /// stored ("Existing services that reference an `INACTIVE` task definition ... can
    /// still scale up or down by modifying the service's desired count"), which is why
    /// this answers only a reference the REQUEST carries.
    pub fn usable_revision(
        &self,
        family: &str,
        revision: Option<i32>,
    ) -> Result<&TaskDefinitionRecord, TaskDefinitionRefusal> {
        match revision {
            None => self
                .latest_active_revision(family)
                .ok_or(TaskDefinitionRefusal::NotFound),
            Some(revision) => match self.task_definitions.get(&(family.to_string(), revision)) {
                None => Err(TaskDefinitionRefusal::NotFound),
                Some(record) if record.status != TASK_DEFINITION_ACTIVE => {
                    Err(TaskDefinitionRefusal::Inactive)
                }
                Some(record) => Ok(record),
            },
        }
    }

    /// What the task map counts for ONE service: the tasks its scheduler placed and
    /// still wants, split by lifecycle status. `Service$runningCount` is "the number of
    /// tasks in the cluster that are in the RUNNING state" and `pendingCount` the same
    /// for `PENDING`; a task that has been asked to stop is no longer one of them.
    pub fn service_tally(&self, cluster_name: &str, service_name: &str) -> ServiceTally {
        self.tally_wanted(|task| {
            task.cluster_name == cluster_name && task.service_name.as_deref() == Some(service_name)
        })
    }

    /// The RUNNING/PENDING split of every task `wanted` selects that is still wanted
    /// RUNNING — defined once, so the service's counts and a task set's cannot disagree
    /// about what each status counts.
    fn tally_wanted(&self, wanted: impl Fn(&TaskRecord) -> bool) -> ServiceTally {
        let mut tally = ServiceTally::default();
        for task in self
            .tasks
            .values()
            .filter(|task| task.desired_status == STATUS_RUNNING && wanted(task))
        {
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
            } else if task.last_status == STATUS_PENDING {
                tally.pending += 1;
            }
        }
        tally
    }

    /// What the task map counts for ONE deployment of a service: the tasks that deployment
    /// placed and that have not stopped yet.
    ///
    /// A service's task carries the id of the deployment that placed it as its `startedBy`
    /// ([`ServiceRecord::deployment_id`]), which is what tells a superseded deployment's
    /// remaining tasks from the current one's. Tasks being replaced are counted here even
    /// though they are on their way out: they are what makes that deployment one that
    /// "still has running tasks".
    fn deployment_tally(&self, cluster_name: &str, deployment_id: &str) -> ServiceTally {
        let mut tally = ServiceTally::default();
        for task in self.tasks.values().filter(|task| {
            task.cluster_name == cluster_name && task.started_by.as_deref() == Some(deployment_id)
        }) {
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
            } else if task.last_status != STATUS_STOPPED {
                tally.pending += 1;
            }
        }
        tally
    }

    /// The operating-system family the tasks of `service` run on, as an `OSFamily` value.
    ///
    /// DERIVED from the revision the service runs: `RuntimePlatform$operatingSystemFamily`
    /// is what a task definition declares its operating system as, and a revision that
    /// declares no `runtimePlatform` runs on [`DEFAULT_OS_FAMILY`] — which is also what a
    /// service that runs no revision of its own (its tasks come from a task set) reports.
    fn service_os_family(&self, service: &ServiceRecord) -> String {
        service
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .and_then(|definition| definition.runtime_platform.as_ref())
            .and_then(|platform| platform.operating_system_family.clone())
            .unwrap_or_else(|| DEFAULT_OS_FAMILY.to_string())
    }

    /// The ARN of the IAM role `Service$roleArn` reports — "The ARN of the IAM role
    /// that's associated with the service" — or `None` for a service that has none.
    ///
    /// The role the create named, when it named one. Otherwise the ECS SERVICE-LINKED
    /// role, for exactly the services `CreateService$role` says need one: "The
    /// service-linked role is required if your task definition uses the `awsvpc` network
    /// mode or if the service is configured to use service discovery, an external
    /// deployment controller, multiple target groups, or Elastic Inference accelerators in
    /// which case you don't specify a role here." A service outside that list that named
    /// no role has no role associated with it, so it reports none.
    fn service_role_arn(&self, service: &ServiceRecord) -> Option<String> {
        if let Some(requested) = service.role_arn.clone() {
            return Some(requested);
        }
        let awsvpc = service
            .task_definition
            .as_ref()
            .and_then(|key| self.task_definitions.get(key))
            .is_some_and(|definition| definition.network_mode == NETWORK_MODE_AWSVPC);
        let requires_service_linked_role = awsvpc
            || !service.places_tasks()
            || !service.service_registries.is_empty()
            || service.load_balancers.len() > 1;
        requires_service_linked_role.then(service_linked_role_arn)
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
        // The Fargate-only members: "A platform family is specified only for tasks using
        // the Fargate launch type", and so is a platform version. A service that does not
        // run on Fargate carries neither, whatever its request happened to say.
        let fargate = service.runs_on_fargate();
        let os_family = self.service_os_family(service);
        // "The operating system that your tasks in the service run on", derived from the
        // revision's `runtimePlatform` and reported in its display spelling.
        let platform_family = fargate.then(|| os_family_display(&os_family));
        // "If one isn't specified, the LATEST platform version is used": the SERVICE
        // reports the version it asks for, which is that alias when it named none, while
        // its DEPLOYMENT reports the version that alias resolves to for its operating
        // system — the version the tasks of the rollout actually run.
        let requested_platform_version = fargate.then(|| {
            service
                .platform_version
                .clone()
                .unwrap_or_else(|| PLATFORM_VERSION_LATEST.to_string())
        });
        let deployment_platform_version = requested_platform_version
            .as_deref()
            .map(|requested| resolved_platform_version(requested, &os_family));
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
            platform_version: deployment_platform_version,
            platform_family: platform_family.clone(),
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
            rollout_state_reason: Some(rollout_state_reason(&service.deployment_id, settled)),
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
            platform_version: requested_platform_version,
            platform_family,
            task_definition: service.task_definition_arn.clone(),
            deployment_configuration: Some(service.deployment_configuration.clone()),
            task_sets: if scheduler_owned {
                None
            } else {
                // "If the service uses the CODE_DEPLOY deployment controller, the
                // deploymentController, taskSets and deployments parameters will be
                // returned": the task sets the service actually holds, which for a
                // controller that is not the ECS scheduler is where its tasks come from.
                Some(
                    self.service_task_sets(&service.cluster_name, &service.name)
                        .into_iter()
                        .map(|task_set| self.task_set_view(task_set, fields.into()))
                        .collect(),
                )
            },
            deployments: Some(if scheduler_owned {
                // The current rollout, then the deployments it is replacing — which are
                // reported for as long as they still hold a task of their own
                // (`Deployment$status`: "ACTIVE: A service deployment that still has
                // running tasks, but are in the process of being replaced").
                let mut deployments = vec![deployment];
                deployments.extend(
                    service
                        .superseded_deployments
                        .iter()
                        .map(|superseded| self.superseded_deployment_view(service, superseded)),
                );
                deployments
            } else {
                Vec::new()
            }),
            role_arn: self.service_role_arn(service),
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
            // "The ARN of the service deployment" the service is on now: the record the
            // call that started this rollout wrote, which is the same deployment the
            // `deployments` list above reports and the one `ListServiceDeployments`
            // answers with.
            current_service_deployment: self
                .current_service_deployment(service)
                .map(|deployment| deployment.arn.clone()),
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
            // `DescribeServices$include`: "Determines whether you want to see the resource
            // tags for the service" — the member is reported only when the request asked
            // for the tags AND the service carries some, as the recorded responses of the
            // real service do: an untagged service omits the key rather than reporting an
            // empty list.
            tags: if fields.tags && !service.tags.is_empty() {
                Some(service.tags.clone())
            } else {
                None
            },
            // "The principal that created the service": the principal of the request
            // that created it, as stored — the same principal
            // `TaskDefinition$registeredBy` names.
            created_by: Some(service.created_by.clone()),
            enable_ecs_managed_tags: service.enable_ecs_managed_tags,
            propagate_tags: Some(service.propagate_tags.clone()),
            enable_execute_command: service.enable_execute_command,
            availability_zone_rebalancing: Some(service.availability_zone_rebalancing.clone()),
            resource_management_type: Some(service.resource_management_type().to_string()),
        }
    }

    /// The `Deployment` a SUPERSEDED deployment is reported as: the configuration it was
    /// rolling out, with the tasks it still holds counted off the task map.
    ///
    /// `Deployment$status` [`DEPLOYMENT_ACTIVE`] — it "still has running tasks, but are in
    /// the process of being replaced". Its `rolloutState` is the one that rollout had
    /// reached when it was superseded, not one asserted here.
    fn superseded_deployment_view(
        &self,
        service: &ServiceRecord,
        superseded: &SupersededDeployment,
    ) -> Deployment {
        let tally = self.deployment_tally(&service.cluster_name, &superseded.id);
        let fargate = superseded.launch_type.as_deref() == Some(LAUNCH_TYPE_FARGATE)
            || superseded.capacity_provider_strategy.iter().any(|item| {
                MANAGED_CAPACITY_PROVIDERS.contains(&item.capacity_provider.as_str())
            });
        let os_family = self.service_os_family(service);
        let platform_version = fargate.then(|| {
            resolved_platform_version(
                superseded
                    .platform_version
                    .as_deref()
                    .unwrap_or(PLATFORM_VERSION_LATEST),
                &os_family,
            )
        });
        Deployment {
            id: Some(superseded.id.clone()),
            status: Some(DEPLOYMENT_ACTIVE.to_string()),
            task_definition: superseded.task_definition_arn.clone(),
            desired_count: superseded.desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            failed_tasks: 0,
            created_at: Some(superseded.created_at),
            updated_at: Some(superseded.updated_at),
            capacity_provider_strategy: superseded
                .launch_type
                .is_none()
                .then(|| superseded.capacity_provider_strategy.clone()),
            launch_type: superseded.launch_type.clone(),
            platform_version,
            platform_family: fargate.then(|| os_family_display(&os_family)),
            network_configuration: superseded.network_configuration.clone(),
            rollout_state: Some(
                if superseded.rollout_completed {
                    ROLLOUT_COMPLETED
                } else {
                    ROLLOUT_IN_PROGRESS
                }
                .to_string(),
            ),
            rollout_state_reason: Some(rollout_state_reason(
                &superseded.id,
                superseded.rollout_completed,
            )),
            service_connect_configuration: superseded.service_connect_configuration.clone(),
            service_connect_resources: Some(Vec::new()),
            volume_configurations: Some(superseded.volume_configurations.clone()),
            fargate_ephemeral_storage: None,
            vpc_lattice_configurations: Some(superseded.vpc_lattice_configurations.clone()),
        }
    }

    /// The capacity provider `name` names, or `None` when nothing holds that name.
    ///
    /// Either one a caller created or one of the providers Amazon ECS manages for every
    /// account ([`managed_capacity_provider`]), so every operation that resolves a
    /// provider by name sees the same set — and `region` is the request's own, which is
    /// the Region the ARN of a managed provider reports.
    pub fn capacity_provider(&self, name: &str, region: &str) -> Option<CapacityProviderRecord> {
        self.capacity_providers
            .get(name)
            .cloned()
            .or_else(|| managed_capacity_provider(name, region))
    }

    /// Every capacity provider this account holds: the ones a caller created, plus the
    /// ones Amazon ECS manages. A created provider that somehow holds a managed name
    /// shadows it, so a name is reported once.
    pub fn all_capacity_providers(&self, region: &str) -> Vec<CapacityProviderRecord> {
        let mut providers: Vec<CapacityProviderRecord> =
            self.capacity_providers.values().cloned().collect();
        providers.extend(
            MANAGED_CAPACITY_PROVIDERS
                .iter()
                .filter(|name| !self.capacity_providers.contains_key(**name))
                .filter_map(|name| managed_capacity_provider(name, region)),
        );
        providers
    }

    /// The cluster an operation may act IN: one that exists and has not been deleted.
    ///
    /// A tombstone (`status` [`CLUSTER_INACTIVE`]) is not one — the `Cluster$status` doc:
    /// "The cluster has been deleted" — so every operation that resolves a `cluster` to
    /// work in refuses it exactly as it refuses a name that never existed. The three
    /// operations that own the tombstone read `clusters` directly instead:
    /// `DescribeClusters` reports it, `DeleteCluster` answers idempotently with it, and
    /// `CreateCluster` creates a new cluster over it.
    pub fn active_cluster(&self, cluster_name: &str) -> Option<&ClusterRecord> {
        self.clusters
            .get(cluster_name)
            .filter(|cluster| cluster.status != CLUSTER_INACTIVE)
    }

    /// Whether [`Self::active_cluster`] names one — the existence check an operation that
    /// only needs to know the cluster is there makes.
    pub fn has_active_cluster(&self, cluster_name: &str) -> bool {
        self.active_cluster(cluster_name).is_some()
    }

    /// Whether `cluster_name` still holds an `ACTIVE` service — one the caller has NOT
    /// deleted yet, and so one of the cluster's `activeServicesCount`.
    ///
    /// `DeleteCluster`'s `ClusterContainsServicesException`: "You can't delete a cluster
    /// that contains services. First, update the service to reduce its desired task count
    /// to 0, and then delete the service" — so a service already being drained away
    /// (`DRAINING`) or deleted (`INACTIVE`) is not what that refusal is about.
    pub fn cluster_has_active_services(&self, cluster_name: &str) -> bool {
        self.services
            .values()
            .any(|service| service.cluster_name == cluster_name && service.is_active())
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
        let service_arn = service.arn.clone();
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
        // And the rollout that was still under way ends here with it, rather than being
        // left open for a steady state the deleted service will never reach. The history
        // itself is KEPT, exactly as the service record is.
        self.finish_open_deployments(
            &service_arn,
            DateTime::from(now),
            SERVICE_DEPLOYMENT_STOPPED,
            &format!("Service {name} was deleted"),
        );
        // A task set belongs to the service it was created in, so a deleted service keeps
        // none: the tasks it placed were stopped above with every other task of the
        // service.
        self.task_sets
            .retain(|(cluster, service, _), _| (cluster, service) != (&key.0, &key.1));
    }

    /// The task sets of ONE service, oldest first.
    ///
    /// Task set ids are minted from a fixed-width monotonic counter, so sorting the ids
    /// orders the task sets by the call that created them.
    pub fn service_task_sets(
        &self,
        cluster_name: &str,
        service_name: &str,
    ) -> Vec<&TaskSetRecord> {
        let mut found: Vec<&TaskSetRecord> = self
            .task_sets
            .values()
            .filter(|task_set| {
                task_set.cluster_name == cluster_name && task_set.service_name == service_name
            })
            .collect();
        found.sort_by(|left, right| left.id.cmp(&right.id));
        found
    }

    /// The task set a `taskSet` reference names in one service of one cluster, if this
    /// store holds it. The member's doc: "The ID or full Amazon Resource Name (ARN) of
    /// task sets", so a reference is either form.
    pub fn resolve_task_set(
        &self,
        cluster_name: &str,
        service_name: &str,
        reference: &str,
    ) -> Option<&TaskSetRecord> {
        self.task_sets.values().find(|task_set| {
            task_set.cluster_name == cluster_name
                && task_set.service_name == service_name
                && task_set.is_named_by(reference)
        })
    }

    /// How many tasks `task_set` wants. `TaskSet$computedDesiredCount`: "This is
    /// calculated by multiplying the service's `desiredCount` by the task set's `scale`
    /// percentage. The result is always rounded up."
    pub fn computed_desired_count(&self, task_set: &TaskSetRecord) -> i32 {
        let Some(service) = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()))
        else {
            return 0;
        };
        let desired = f64::from(service.desired_count.max(0));
        let computed = (desired * task_set.scale.value / MAX_SCALE_PERCENT).ceil();
        if computed <= 0.0 {
            return 0;
        }
        // The product of a count and a percentage cannot leave the `Integer` the member
        // targets, but the cast is still bounded rather than wrapped.
        computed.min(f64::from(i32::MAX)) as i32
    }

    /// What the task map counts for ONE task set: the tasks it placed and still wants,
    /// split by lifecycle status — the same split [`Self::service_tally`] makes, keyed on
    /// the task set that placed the task rather than on the service.
    pub fn task_set_tally(&self, task_set: &TaskSetRecord) -> ServiceTally {
        self.tally_wanted(|task| {
            task.cluster_name == task_set.cluster_name
                && task.started_by.as_deref() == Some(task_set.id.as_str())
        })
    }

    /// The `TaskSet` every task-set-returning operation reports, built from what is
    /// stored.
    ///
    /// `computedDesiredCount` is DERIVED from the service's desired count and this task
    /// set's scale, and `runningCount`/`pendingCount` are COUNTED off the task map, so
    /// the `stabilityStatus` that follows from them is read rather than asserted.
    pub fn task_set_view(&self, task_set: &TaskSetRecord, fields: TaskSetFields) -> TaskSet {
        let tally = self.task_set_tally(task_set);
        let computed_desired_count = self.computed_desired_count(task_set);
        let service = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()));
        // "A platform family is specified only for tasks that use the Fargate launch
        // type", and so is a platform version — the same rule a service's projection
        // follows, read off the capacity THIS task set runs on.
        let fargate = task_set.launch_type.as_deref() == Some(LAUNCH_TYPE_FARGATE)
            || task_set.capacity_provider_strategy.iter().any(|item| {
                MANAGED_CAPACITY_PROVIDERS.contains(&item.capacity_provider.as_str())
            });
        let os_family = self
            .task_definitions
            .get(&task_set.task_definition)
            .and_then(|definition| definition.runtime_platform.as_ref())
            .and_then(|platform| platform.operating_system_family.clone())
            .unwrap_or_else(|| DEFAULT_OS_FAMILY.to_string());
        let requested_platform_version = fargate.then(|| {
            task_set
                .platform_version
                .clone()
                .unwrap_or_else(|| PLATFORM_VERSION_LATEST.to_string())
        });
        TaskSet {
            id: Some(task_set.id.clone()),
            task_set_arn: Some(task_set.arn.clone()),
            service_arn: Some(task_set.service_arn.clone()),
            cluster_arn: Some(task_set.cluster_arn.clone()),
            // "If an CodeDeploy deployment created the task set, the `startedBy`
            // parameter is `CODE_DEPLOY`. If an external deployment created the task set,
            // the `startedBy` field isn't used."
            started_by: service
                .filter(|service| service.controller() == DEPLOYMENT_CONTROLLER_CODE_DEPLOY)
                .map(|_| TASK_SET_STARTED_BY_CODE_DEPLOY.to_string()),
            external_id: task_set.external_id.clone(),
            status: Some(task_set.status.clone()),
            task_definition: Some(task_set.task_definition_arn.clone()),
            computed_desired_count,
            pending_count: tally.pending,
            running_count: tally.running,
            created_at: Some(task_set.created_at),
            updated_at: Some(task_set.updated_at),
            launch_type: task_set.launch_type.clone(),
            capacity_provider_strategy: Some(task_set.capacity_provider_strategy.clone()),
            platform_version: requested_platform_version
                .as_deref()
                .map(|requested| resolved_platform_version(requested, &os_family)),
            platform_family: fargate.then(|| os_family_display(&os_family)),
            network_configuration: task_set.network_configuration.clone(),
            load_balancers: Some(task_set.load_balancers.clone()),
            service_registries: Some(task_set.service_registries.clone()),
            scale: Some(task_set.scale.clone()),
            // "The task `runningCount` is equal to the `computedDesiredCount`" and "The
            // `pendingCount` is 0"; otherwise the task set is still stabilizing.
            stability_status: Some(
                if tally.running == computed_desired_count && tally.pending == 0 {
                    STABILITY_STATUS_STEADY_STATE
                } else {
                    STABILITY_STATUS_STABILIZING
                }
                .to_string(),
            ),
            // "The Unix timestamp for the time when the task set stability status was
            // retrieved": the stability status is computed from the tasks as they stand
            // when the task set was last written, which is the instant it carries.
            stability_status_at: Some(task_set.updated_at),
            // `DescribeTaskSets$include`: "If `TAGS` is specified, the tags are included
            // in the response. If this field is omitted, tags aren't included" — and an
            // untagged task set omits the member rather than reporting an empty list, as
            // a service's projection does.
            tags: if fields.tags && !task_set.tags.is_empty() {
                Some(task_set.tags.clone())
            } else {
                None
            },
            // A Fargate ephemeral-storage setting is encrypted with a customer key the
            // caller supplies, and nothing here supplies one.
            fargate_ephemeral_storage: None,
        }
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
    /// A placement the cluster cannot satisfy places NOTHING and records WHY on the
    /// service's event stream instead (see [`Self::record_placement_failure`]): the
    /// desired count is what the caller asked for and does not move because the scheduler
    /// could not meet it.
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
        let placement = service_placement(&service);

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

        if to_place > 0 {
            if let Some(reason) = self.placement_capacity_reason(&service, &placement) {
                self.record_placement_failure(key, &reason, now);
                return Ok(());
            }
        }
        for _ in 0..to_place {
            self.place_service_task(&service, definition, &placement, region, now);
        }
        Ok(())
    }

    /// Place or stop tasks so `task_set` holds exactly the `computedDesiredCount` tasks
    /// it wants, from `definition`.
    ///
    /// The same scheduler work [`Self::scale_service`] does, for the task set of a
    /// service whose deployment controller is not the ECS one: the `Service$taskSets` doc
    /// describes those as "a set of Amazon ECS tasks in either an CodeDeploy or an
    /// `EXTERNAL` deployment", and how many tasks the set wants is the
    /// `TaskSet$computedDesiredCount` the service's desired count and this scale come to.
    /// It runs on the call that CHANGES what the task set wants, never on a read.
    pub fn scale_task_set(
        &mut self,
        key: &(String, String, String),
        definition: &TaskDefinitionRecord,
        region: &str,
        now: SystemTime,
    ) -> Result<(), TaskCapacityExceeded> {
        let Some(task_set) = self.task_sets.get(key).cloned() else {
            return Ok(());
        };
        let Some(service) = self
            .services
            .get(&(task_set.cluster_name.clone(), task_set.service_name.clone()))
            .cloned()
        else {
            return Ok(());
        };
        let placement = task_set_placement(&task_set);
        let desired = usize::try_from(self.computed_desired_count(&task_set)).unwrap_or(0);

        let mut live: Vec<String> = self
            .tasks
            .values()
            .filter(|task| {
                task.cluster_name == task_set.cluster_name
                    && task.started_by.as_deref() == Some(task_set.id.as_str())
                    && task.desired_status == STATUS_RUNNING
                    && task.is_active()
            })
            .map(|task| task.task_arn.clone())
            .collect();
        live.sort();

        let to_place = desired.saturating_sub(live.len());
        if self.tasks.len() + to_place > MAX_TASKS {
            return Err(TaskCapacityExceeded);
        }
        if live.len() > desired {
            for arn in live[desired..].iter().rev() {
                self.stop_service_task(
                    arn,
                    now,
                    &format!("Scaling activity initiated by task set {}", task_set.id),
                );
            }
            return Ok(());
        }
        if to_place > 0 {
            if let Some(reason) = self.placement_capacity_reason(&service, &placement) {
                let service_key = (task_set.cluster_name.clone(), task_set.service_name.clone());
                self.record_placement_failure(&service_key, &reason, now);
                return Ok(());
            }
        }
        for _ in 0..to_place {
            self.place_service_task(&service, definition, &placement, region, now);
        }
        Ok(())
    }

    /// Why the scheduler cannot place a task of `service` under `placement` right now, or
    /// `None` when it can.
    ///
    /// The capacity a placement runs on has to EXIST before Amazon ECS has anywhere to
    /// put the task: "The EC2 launch type runs your tasks on Amazon EC2 instances
    /// registered to your cluster" (and `EXTERNAL` on the registered on-premises
    /// capacity), and an Auto Scaling group capacity provider's capacity is the container
    /// instances its group registers — unless the provider scales that group itself, in
    /// which case the capacity is coming and the task is parked instead
    /// ([`awaits_provisioned_capacity`]). The sentence this answers is the one `RunTask`
    /// refuses the same condition with, so a client reads one explanation whichever call
    /// hit it.
    pub fn placement_capacity_reason(
        &self,
        service: &ServiceRecord,
        placement: &Placement,
    ) -> Option<String> {
        if self.registered_container_instances(&service.cluster_name) > 0 {
            return None;
        }
        if needs_registered_capacity(placement.launch_type.as_deref()) {
            return Some(NO_CONTAINER_INSTANCES_MESSAGE.to_string());
        }
        let provider = placement
            .capacity_provider_name
            .as_deref()
            .and_then(|name| self.capacity_providers.get(name))?;
        needs_registered_capacity_now(provider)
            .then(|| empty_capacity_provider_message(&provider.name))
    }

    /// Append the scheduler's own account of a placement it could not make to the
    /// service's event stream.
    ///
    /// `Service$events` is "The event stream for your service", and this is the entry
    /// Amazon ECS writes there when it wants to place a task and finds nowhere to put it
    /// — the documented place a client reads why `runningCount` is below `desiredCount`.
    /// A consecutive repeat of the same sentence is not appended: the event stream holds
    /// "the latest events" of the service, and re-stating the same unchanged condition
    /// would push every other event out of it.
    fn record_placement_failure(
        &mut self,
        key: &(String, String),
        reason: &str,
        now: SystemTime,
    ) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let message = unable_to_place_message(&service.name, reason);
        if service
            .events
            .last()
            .is_some_and(|event| event.message == message)
        {
            return;
        }
        let event = self.mint_service_event(now, message);
        let Some(service) = self.services.get_mut(key) else {
            return;
        };
        service.events.push(event);
        // "A maximum of 100 of the latest events are displayed", so only the latest are
        // kept at all.
        if service.events.len() > MAX_SERVICE_EVENTS {
            let first_kept = service.events.len() - MAX_SERVICE_EVENTS;
            service.events.drain(..first_kept);
        }
    }

    /// Place ONE task of `service`, `PENDING` with its start owed by the clock
    /// [`TRANSITION_SPAN`] from now, exactly as `RunTask` leaves a task it placed — or
    /// parked in [`STATUS_PROVISIONING`] when the strategy's provider still has to acquire
    /// the capacity to place it on ([`awaits_provisioned_capacity`]).
    fn place_service_task(
        &mut self,
        service: &ServiceRecord,
        definition: &TaskDefinitionRecord,
        placement: &Placement,
        region: &str,
        now: SystemTime,
    ) {
        let awaiting_capacity = awaits_provisioned_capacity(
            placement
                .capacity_provider_name
                .as_deref()
                .and_then(|name| self.capacity_providers.get(name)),
            self.registered_container_instances(&service.cluster_name),
        );
        let task_id = self.next_task_id();
        let task_arn = task_arn(region, &service.cluster_name, &task_id);
        let containers = self.mint_containers(
            region,
            &service.cluster_name,
            &task_id,
            &task_arn,
            definition,
            initial_task_status(awaiting_capacity),
        );
        // "Determines whether to propagate the tags from the task definition or the
        // service to the task. If no value is specified, the tags aren't propagated."
        let tags = match service.propagate_tags.as_str() {
            PROPAGATE_TAGS_SERVICE => service.tags.clone(),
            PROPAGATE_TAGS_TASK_DEFINITION => definition.tags.clone(),
            _ => Vec::new(),
        };
        let record = TaskRecord {
            task_arn: task_arn.clone(),
            task_id,
            cluster_name: service.cluster_name.clone(),
            cluster_arn: service.cluster_arn.clone(),
            task_definition_arn: definition.arn.clone(),
            family: definition.family.clone(),
            last_status: initial_task_status(awaiting_capacity).to_string(),
            desired_status: STATUS_RUNNING.to_string(),
            containers,
            cpu: definition.cpu.clone(),
            memory: definition.memory.clone(),
            overrides: None,
            launch_type: placement.launch_type.clone(),
            capacity_provider_name: placement.capacity_provider_name.clone(),
            platform_version: placement.platform_version.clone(),
            // `Task$startedBy`: for a task the service scheduler placed, the id of the
            // deployment that placed it; for a task set's, the id of that task set.
            started_by: Some(placement.started_by.clone()),
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
            // A task that is not placed yet has no start owed: the clock owes it only
            // once the capacity it waits for exists.
            pending_due_at: None,
            running_due_at: if awaiting_capacity {
                None
            } else {
                Some(now + TRANSITION_SPAN)
            },
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
        // A task asked to stop never reaches RUNNING: the start it was still waiting for,
        // and the placement it was parked for, are cancelled rather than merely overtaken.
        task.running_due_at = None;
        task.pending_due_at = None;
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
            let launch_type = statistics_launch_type_index(task.launch_type.as_deref());
            if task.last_status == STATUS_RUNNING {
                tally.running += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].running_tasks += 1;
                }
            } else if task.last_status == STATUS_PENDING {
                tally.pending += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].pending_tasks += 1;
                }
            }
        }
        for service in self
            .services
            .values()
            .filter(|service| service.cluster_name == cluster_name)
        {
            let launch_type = statistics_launch_type_index(service.launch_type.as_deref());
            if service.is_active() {
                tally.active_services += 1;
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].active_services += 1;
                }
            } else if service.status == SERVICE_DRAINING {
                if let Some(index) = launch_type {
                    tally.by_launch_type[index].draining_services += 1;
                }
            }
        }
        tally
    }

    /// Whether `cluster_name` still holds a task the caller has NOT released: one that is
    /// neither stopped nor stopping, i.e. whose `desiredStatus` is still `RUNNING`.
    ///
    /// `DeleteCluster`'s `ClusterContainsTasksException`: "You can't delete a cluster that
    /// has active tasks." A task that has been asked to stop is on its way out and is
    /// already excluded from everything else that counts a cluster's tasks
    /// (`Service$runningCount`, `ListTasks`' default filter), so it does not hold the
    /// cluster open either.
    pub fn cluster_has_active_tasks(&self, cluster_name: &str) -> bool {
        self.tasks.values().any(|task| {
            task.cluster_name == cluster_name
                && task.desired_status == STATUS_RUNNING
                && task.is_active()
        })
    }

    /// The capacity providers that tasks of `cluster_name` which have not stopped were
    /// placed with, collected in ONE pass over the task map.
    ///
    /// `PutClusterCapacityProviders`: "You can only disassociate an existing capacity
    /// provider from a cluster if it's not being used by any existing tasks" — this is
    /// that use, gathered once so a caller's provider list cannot multiply the scan.
    pub fn capacity_providers_in_use(&self, cluster_name: &str) -> HashSet<String> {
        self.tasks
            .values()
            .filter(|task| task.cluster_name == cluster_name && task.is_active())
            .filter_map(|task| task.capacity_provider_name.clone())
            .collect()
    }

    /// The value stored for `name` for ONE principal, or `None` when that principal has
    /// no setting of that name.
    ///
    /// The ROOT user's own settings ARE the account-wide defaults — `PutAccountSetting`
    /// `principalArn`: "If you specify the root user, it modifies the account setting
    /// for all users ... unless an individual account setting was specified" — so a
    /// lookup for the root principal reads the defaults map.
    pub fn explicit_account_setting(&self, principal_arn: &str, name: &str) -> Option<String> {
        if principal_arn == root_principal_arn() {
            return self.account_setting_defaults.get(name).cloned();
        }
        self.principal_account_settings
            .get(&(principal_arn.to_string(), name.to_string()))
            .cloned()
    }

    /// The ACCOUNT-WIDE value of `name`: what a write of the default (or of the root
    /// user's setting) stored, falling back to the [`ACCOUNT_SETTINGS`] default for a
    /// name the account has never written.
    pub fn default_account_setting(&self, name: &str) -> String {
        self.account_setting_defaults
            .get(name)
            .cloned()
            .unwrap_or_else(|| {
                account_setting_spec(name)
                    // Unreachable for a `name` off the closed `SettingName` enum; a name
                    // outside the catalog is reported off rather than invented.
                    .map_or(SETTING_OFF, |setting| setting.default_value)
                    .to_string()
            })
    }

    /// The EFFECTIVE account setting of `name` for `principal_arn`, and the principal it
    /// comes from: the principal's own value when it has one, otherwise the account-wide
    /// default, which belongs to the root user.
    ///
    /// `ListAccountSettings$effectiveSettings`: "If `true`, the account settings for the
    /// root user or the default setting for the `principalArn` are returned."
    pub fn effective_account_setting(&self, principal_arn: &str, name: &str) -> ResolvedSetting {
        match self.explicit_account_setting(principal_arn, name) {
            Some(value) => ResolvedSetting {
                value,
                principal_arn: principal_arn.to_string(),
            },
            None => ResolvedSetting {
                value: self.default_account_setting(name),
                principal_arn: root_principal_arn(),
            },
        }
    }

    /// Store the account setting `name` for ONE principal, as `PutAccountSetting` does,
    /// and answer the setting it reports.
    ///
    /// A write against the ROOT user is a write of the account-wide default, because the
    /// root user's settings are those defaults (see [`Self::explicit_account_setting`]).
    ///
    /// REFUSED with [`AccountSettingCapacityExceeded`], storing nothing, when the
    /// principal is one this emulator holds no setting for yet and
    /// [`MAX_ACCOUNT_SETTING_PRINCIPALS`] are already stored: the principal comes from
    /// the request, so the number of them a caller can leave behind is bounded here the
    /// way every other caller-driven map in this store is.
    pub fn put_account_setting(
        &mut self,
        principal_arn: &str,
        name: &str,
        value: &str,
    ) -> Result<Setting, AccountSettingCapacityExceeded> {
        if principal_arn == root_principal_arn() {
            return Ok(self.put_account_setting_default(name, value));
        }
        let principals: HashSet<&str> = self
            .principal_account_settings
            .keys()
            .map(|(principal, _)| principal.as_str())
            .collect();
        if !principals.contains(principal_arn)
            && principals.len() >= MAX_ACCOUNT_SETTING_PRINCIPALS
        {
            return Err(AccountSettingCapacityExceeded);
        }
        self.principal_account_settings.insert(
            (principal_arn.to_string(), name.to_string()),
            value.to_string(),
        );
        Ok(setting_view(name, value, principal_arn))
    }

    /// Store the ACCOUNT-WIDE default of `name`, as `PutAccountSettingDefault` does, and
    /// answer the setting it reports: one that belongs to the root user, since the
    /// default applies "for all users on an account for whom no individual account
    /// setting has been specified".
    pub fn put_account_setting_default(&mut self, name: &str, value: &str) -> Setting {
        self.account_setting_defaults
            .insert(name.to_string(), value.to_string());
        setting_view(name, value, &root_principal_arn())
    }

    /// Clear the account setting `name` for ONE principal, as `DeleteAccountSetting`
    /// does ("Disables an account setting for a specified user, role, or the root user
    /// for an account"), and answer the value that was stored, or `None` when that
    /// principal had none of that name.
    ///
    /// Clearing a principal's entry is what makes the account-wide default apply to it
    /// again ([`Self::effective_account_setting`]); a clear against the ROOT user clears
    /// the account-wide default itself, for the same reason a write against it sets one
    /// (see [`Self::explicit_account_setting`]), so the name falls back to its
    /// [`ACCOUNT_SETTINGS`] default.
    pub fn delete_account_setting(&mut self, principal_arn: &str, name: &str) -> Option<String> {
        if principal_arn == root_principal_arn() {
            return self.account_setting_defaults.remove(name);
        }
        self.principal_account_settings
            .remove(&(principal_arn.to_string(), name.to_string()))
    }

    /// The cluster's EFFECTIVE settings: one entry for every setting name the model
    /// defines (`ClusterSettingName`), carrying the value stored for this cluster when
    /// one was set and the account setting of that name otherwise.
    ///
    /// `ClusterSetting$value`: "If a cluster value is specified, it will override the
    /// `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault"
    /// — so what a cluster that set nothing reports is exactly the account setting in
    /// effect for the caller this emulator authenticates every request as
    /// ([`caller_identity_arn`]), which falls back to the account-wide default and then
    /// to the setting's own default. `Cluster$settings` "indicates whether CloudWatch
    /// Container Insights is on or off for a cluster", so a cluster that set nothing
    /// still has an answer to that question and reports it.
    pub fn cluster_effective_settings(&self, cluster: &ClusterRecord) -> Vec<ClusterSetting> {
        let principal_arn = caller_identity_arn();
        CLUSTER_SETTING_NAME_VALUES
            .iter()
            .map(|name| ClusterSetting {
                name: Some((*name).to_string()),
                value: Some(cluster.setting_value(name).unwrap_or_else(|| {
                    self.effective_account_setting(&principal_arn, name).value
                })),
            })
            .collect()
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
            // `settings`, `tags` and `statistics` are LISTS a described cluster always
            // carries: `include` decides whether they are materialised, not whether the
            // member is there, so a cluster read without `include` reports them empty
            // rather than absent and a client cannot mistake "not asked for" for "none
            // stored".
            settings: Some(if fields.settings {
                self.cluster_effective_settings(cluster)
            } else {
                Vec::new()
            }),
            tags: Some(if fields.tags {
                cluster.tags.clone()
            } else {
                Vec::new()
            }),
            statistics: Some(if fields.statistics {
                cluster_statistics(&tally)
            } else {
                Vec::new()
            }),
            // `Cluster$attachments` and `Cluster$attachmentsStatus` are what
            // `DescribeClusters$include` `ATTACHMENTS` asks for, so they are reported
            // only when it does. The list itself stays empty: attachments are the
            // resources a capacity provider provisions, and this emulator provisions
            // none (see [`ClusterRecord::attachments`]).
            attachments: if fields.attachments {
                Some(cluster.attachments())
            } else {
                None
            },
            attachments_status: if fields.attachments {
                cluster.attachments_status.clone()
            } else {
                None
            },
            service_connect_defaults: cluster.service_connect_defaults.clone(),
        }
    }

    /// The `Cluster` an operation that CREATES, UPDATES or DELETES a cluster reports.
    ///
    /// Such an operation carries no `include` member, and the halves of a `Cluster`
    /// answer that differently.
    ///
    /// What the cluster is CONFIGURED to be is always materialised: `settings` (the
    /// effective value of every setting name the model defines, see
    /// [`Self::cluster_effective_settings`] — `Cluster$settings` "indicates whether
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

    /// How many clusters the account holds under `cluster_name` — the top-level
    /// `clusterCount` a cluster-mutating response carries, read AFTER the call has been
    /// applied. See [`ClusterCountEnvelope`] for why it is not a modelled member.
    ///
    /// A cluster NAME identifies one cluster ("you create a cluster that's named
    /// default" / "the short name ... of the cluster"), and a DELETED cluster is not one
    /// the account holds: the record stays as a tombstone so a reader can still describe
    /// it ("The cluster has been deleted"), but the account no longer holds that cluster,
    /// so it is not counted. A create therefore answers `1` and a delete `0`, whatever
    /// else the account holds beside the cluster the call named — which is what every
    /// recorded `clusterCount` carries, including the ones recorded while a second,
    /// untouched cluster was live.
    pub fn cluster_count_for(&self, cluster_name: &str) -> i32 {
        i32::from(
            self.clusters
                .get(cluster_name)
                .is_some_and(|cluster| cluster.status != CLUSTER_INACTIVE),
        )
    }

    /// The tag set stored on the resource `resource_arn` names, or `None` when this store
    /// holds no resource of that kind with that ARN.
    ///
    /// The resource's OWN `tags` — the list its create was given and its describe reports
    /// — is the one tag set this service holds, so whoever writes it (this service's own
    /// create, or a cross-service tagging call reaching into these records) and whoever
    /// reads it see the same list, with no second copy to drift from it.
    ///
    /// Each kind is keyed by something other than its ARN, so the scan is over the ONE
    /// map that kind lives in rather than over every map.
    pub fn resource_tags(&self, kind: TaggedResourceKind, resource_arn: &str) -> Option<&[Tag]> {
        match kind {
            TaggedResourceKind::CapacityProvider => self
                .capacity_providers
                .values()
                .find(|provider| provider.arn == resource_arn)
                .map(|provider| provider.tags.as_slice()),
            TaggedResourceKind::Cluster => self
                .clusters
                .values()
                .find(|cluster| cluster.arn == resource_arn)
                .map(|cluster| cluster.tags.as_slice()),
            TaggedResourceKind::ContainerInstance => self
                .container_instances
                .values()
                .find(|instance| instance.arn == resource_arn)
                .map(|instance| instance.tags.as_slice()),
            TaggedResourceKind::Service => self
                .services
                .values()
                .find(|service| service.arn == resource_arn)
                .map(|service| service.tags.as_slice()),
            TaggedResourceKind::Task => self
                .tasks
                .values()
                .find(|task| task.task_arn == resource_arn)
                .map(|task| task.tags.as_slice()),
            TaggedResourceKind::TaskDefinition => self
                .task_definitions
                .values()
                .find(|definition| definition.arn == resource_arn)
                .map(|definition| definition.tags.as_slice()),
        }
    }

    /// The tag set stored on the resource `resource_arn` names, for a caller that is
    /// about to CHANGE it — the same resolution as [`Self::resource_tags`], so a write
    /// and a read of one resource's tags reach the same list.
    pub fn resource_tags_mut(
        &mut self,
        kind: TaggedResourceKind,
        resource_arn: &str,
    ) -> Option<&mut Vec<Tag>> {
        match kind {
            TaggedResourceKind::CapacityProvider => self
                .capacity_providers
                .values_mut()
                .find(|provider| provider.arn == resource_arn)
                .map(|provider| &mut provider.tags),
            TaggedResourceKind::Cluster => self
                .clusters
                .values_mut()
                .find(|cluster| cluster.arn == resource_arn)
                .map(|cluster| &mut cluster.tags),
            TaggedResourceKind::ContainerInstance => self
                .container_instances
                .values_mut()
                .find(|instance| instance.arn == resource_arn)
                .map(|instance| &mut instance.tags),
            TaggedResourceKind::Service => self
                .services
                .values_mut()
                .find(|service| service.arn == resource_arn)
                .map(|service| &mut service.tags),
            TaggedResourceKind::Task => self
                .tasks
                .values_mut()
                .find(|task| task.task_arn == resource_arn)
                .map(|task| &mut task.tags),
            TaggedResourceKind::TaskDefinition => self
                .task_definitions
                .values_mut()
                .find(|definition| definition.arn == resource_arn)
                .map(|definition| &mut definition.tags),
        }
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
    /// One entry per [`STATISTICS_LAUNCH_TYPES`] entry, in that order.
    by_launch_type: [LaunchTypeTally; STATISTICS_LAUNCH_TYPES.len()],
}

/// What a cluster's tasks and services count for ONE launch type: the four
/// `Cluster$statistics` counters that launch type contributes.
#[derive(Debug, Clone, Copy, Default)]
struct LaunchTypeTally {
    running_tasks: usize,
    pending_tasks: usize,
    active_services: usize,
    draining_services: usize,
}

/// Where `launch_type` sits in [`STATISTICS_LAUNCH_TYPES`], or `None` when the task or
/// service was placed through a capacity provider rather than a launch type — there is
/// no launch-type counter for it to contribute to.
fn statistics_launch_type_index(launch_type: Option<&str>) -> Option<usize> {
    let launch_type = launch_type?;
    STATISTICS_LAUNCH_TYPES
        .iter()
        .position(|(name, _)| *name == launch_type)
}

/// `Cluster$statistics`: "the task and service count ... separated by launch type".
///
/// One entry per counter per launch type ([`STATISTICS_LAUNCH_TYPES`]), read off the
/// single pass in [`ComAmazonawsEcsAmazonec2containerservicev20141113State::tally`]. The
/// ORDER is the one recorded ECS responses carry, so a client that reads the list
/// positionally reads the same metric the real service named: the launch types the
/// `Cluster$statistics` doc lists come first, counter by counter in the order it lists
/// them, and the launch types it does not name follow, each one's task counters before
/// any of their service counters.
fn cluster_statistics(tally: &TaskTally) -> Vec<KeyValuePair> {
    let segment = |index: usize| STATISTICS_LAUNCH_TYPES[index].1;
    let running_tasks = |index: usize| {
        pair(
            &format!("running{}TasksCount", segment(index)),
            tally.by_launch_type[index].running_tasks.to_string(),
        )
    };
    let pending_tasks = |index: usize| {
        pair(
            &format!("pending{}TasksCount", segment(index)),
            tally.by_launch_type[index].pending_tasks.to_string(),
        )
    };
    let active_services = |index: usize| {
        pair(
            &format!("active{}ServiceCount", segment(index)),
            tally.by_launch_type[index].active_services.to_string(),
        )
    };
    let draining_services = |index: usize| {
        pair(
            &format!("draining{}ServiceCount", segment(index)),
            tally.by_launch_type[index].draining_services.to_string(),
        )
    };

    let documented = 0..DOCUMENTED_STATISTICS_LAUNCH_TYPES;
    let rest = DOCUMENTED_STATISTICS_LAUNCH_TYPES..STATISTICS_LAUNCH_TYPES.len();
    let mut statistics = Vec::with_capacity(4 * STATISTICS_LAUNCH_TYPES.len());
    for index in documented.clone() {
        statistics.push(running_tasks(index));
    }
    for index in documented.clone() {
        statistics.push(pending_tasks(index));
    }
    for index in documented.clone() {
        statistics.push(active_services(index));
    }
    for index in documented {
        statistics.push(draining_services(index));
    }
    for index in rest.clone() {
        statistics.push(running_tasks(index));
        statistics.push(pending_tasks(index));
    }
    for index in rest {
        statistics.push(active_services(index));
        statistics.push(draining_services(index));
    }
    statistics
}

/// A cluster response plus the top-level `clusterCount` Amazon ECS answers a
/// cluster-mutating call with.
///
/// `clusterCount` is RECORDED behaviour, not modelled behaviour: the Smithy model's
/// `CreateClusterResponse`/`DeleteClusterResponse` declare the single member `cluster`
/// (`smithy_query(query="operation", operation="CreateCluster,DeleteCluster")`), while
/// every recorded 200 from either operation carries `clusterCount` beside `cluster`,
/// counting the clusters the account holds under the name the call named once the call
/// has been applied (1 from a create, 0 from a delete — see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_count_for`]).
/// Because the member is not in the model it has
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

/// The values `DescribeClusters$include` accepts: the model's `ClusterField` enum, which
/// [`ClusterFields::from_include`] reads. Also what identifies a refused `include` value
/// as a CLUSTER read's, in `crate::service`.
pub const CLUSTER_FIELDS: [&str; 5] = [
    "ATTACHMENTS",
    "CONFIGURATIONS",
    "SETTINGS",
    "STATISTICS",
    "TAGS",
];

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

/// A `Failure` for a revision a delete cannot act on because it has not been
/// deregistered: the `DeleteTaskDefinitions` doc's own requirement, "You must deregister
/// a task definition revision before you delete it", which is also why the operation
/// reports it per entry rather than refusing the whole batch — one entry of a request may
/// be deletable while another is not.
pub fn not_inactive_failure(arn: &str, status: &str) -> Failure {
    Failure {
        arn: Some(arn.to_string()),
        reason: Some(TASK_DEFINITION_NOT_INACTIVE_REASON.to_string()),
        // "The details of the failure": the status the revision is actually in, which is
        // what tells the caller whether to deregister it or to wait for a delete already
        // under way.
        detail: Some(format!("The task definition is in {status} status.")),
    }
}

/// `Failure$reason` for a revision a delete cannot act on because it is not `INACTIVE`.
pub const TASK_DEFINITION_NOT_INACTIVE_REASON: &str =
    "Task definition must be in INACTIVE status to be deleted.";

/// A `Failure` for an id that names nothing: `reason: "MISSING"`.
pub fn missing_failure(arn: &str) -> Failure {
    Failure {
        arn: Some(arn.to_string()),
        reason: Some(FAILURE_MISSING.to_string()),
        detail: None,
    }
}

/// The `MISSING` `Failure` a batch read reports for an identifier that resolves to no
/// resource of `resource_type`, naming the identifier AS THE CALLER SENT IT: the value
/// itself when it is already an ARN, otherwise that id alone under the resource type.
///
/// The cluster segment of a task or container-instance ARN
/// ([`task_arn`], [`container_instance_arn`]) identifies the cluster the resource is
/// REGISTERED IN, which only a stored resource has; the id of one that does not exist
/// identifies it account-wide by itself, so no cluster is inserted into the ARN the
/// failure echoes. That is what lets a caller match each failure entry back to the
/// identifier it sent.
pub fn unresolved_failure(region: &str, resource_type: &str, reference: &str) -> Failure {
    let arn = if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{resource_type}/{reference}"))
    };
    missing_failure(&arn)
}

/// Check a requested `launch_type` against the launch types `definition` supports,
/// answering the message an `InvalidParameterException` carries when it is not one of
/// them.
///
/// `TaskDefinition$compatibilities`: "Amazon ECS validates the task definition
/// parameters with those supported by the launch type" — a revision whose parameters do
/// not support a launch type was never validated against it, so it cannot be placed
/// with it, whichever operation does the placing. Shared by every operation that takes a
/// `launchType`, so a launch type `RunTask` refuses is refused by `CreateService` too.
pub fn validate_launch_type(
    definition: &TaskDefinitionRecord,
    launch_type: &str,
) -> Result<(), String> {
    if definition
        .compatibilities()
        .iter()
        .any(|supported| supported == launch_type)
    {
        return Ok(());
    }
    Err(format!(
        "Task definition does not support launch_type {launch_type}."
    ))
}

/// The platform version a request asks for, if it names one at all: a blank member
/// names no platform version, so it reads the same as an absent one ("If one isn't
/// specified, the LATEST platform version is used").
pub fn requested_platform_version(platform_version: Option<&str>) -> Option<&str> {
    platform_version
        .map(str::trim)
        .filter(|version| !version.is_empty())
}

/// Whether a request's `platformVersion` names a platform version this service knows
/// ([`PLATFORM_VERSIONS`]). A request that names one it does not is answered with
/// `PlatformUnknownException` — "The specified platform version doesn't exist." —
/// whichever operation carries it.
pub fn is_unknown_platform_version(platform_version: Option<&str>) -> bool {
    requested_platform_version(platform_version)
        .is_some_and(|version| !PLATFORM_VERSIONS.contains(&version))
}

/// Whether a task placed with `launch_type` needs a container instance registered to
/// its cluster ([`REGISTERED_CAPACITY_LAUNCH_TYPES`]). A task placed through a capacity
/// provider carries no launch type, and the provider's own capacity is what places it,
/// so it does not.
pub fn needs_registered_capacity(launch_type: Option<&str>) -> bool {
    launch_type.is_some_and(|launch_type| REGISTERED_CAPACITY_LAUNCH_TYPES.contains(&launch_type))
}

/// Whether a task placed through `provider` needs a container instance registered to its
/// cluster ALREADY — which is the other half of [`needs_registered_capacity`], for the
/// placement that carries a capacity provider instead of a launch type.
///
/// An Auto Scaling group provider's capacity is the container instances its group
/// registers into the cluster, so a cluster with none has nowhere to place the task —
/// unless the provider scales that group itself. `ManagedScaling`'s doc: "When managed
/// scaling is turned on, Amazon ECS manages the scale-in and scale-out actions of the
/// Auto Scaling group", so such a request is one Amazon ECS satisfies by adding capacity;
/// "If managed scaling is off, the user must manage the scaling of the Auto Scaling
/// group", so then nothing is coming. A managed-instances provider manages its own
/// instances, so it never needs capacity to be there first.
pub fn needs_registered_capacity_now(provider: &CapacityProviderRecord) -> bool {
    let Some(group) = provider.auto_scaling_group_provider.as_ref() else {
        return false;
    };
    group
        .managed_scaling
        .as_ref()
        .and_then(|scaling| scaling.status.as_deref())
        != Some(MANAGED_SCALING_ENABLED)
}

/// Whether a task routed through `provider` still has to have capacity ACQUIRED for it
/// before Amazon ECS can place it, given the `registered_instances` its cluster holds.
///
/// It is the accepted half of [`needs_registered_capacity_now`]: an Auto Scaling group
/// provider whose managed scaling Amazon ECS runs ("Amazon ECS manages the scale-in and
/// scale-out actions of the Auto Scaling group") turns a placement into one ECS can
/// satisfy by adding capacity, and until that capacity is registered as a container
/// instance there is nothing to place the task ON. Such a task is accepted but not placed,
/// which is what [`STATUS_PROVISIONING`] reports; a cluster that already holds registered
/// capacity places the task now, and a provider that names no Auto Scaling group of this
/// account (a predefined Fargate provider, a managed-instances provider) acquires no
/// container instance for this cluster, so neither parks a task.
pub fn awaits_provisioned_capacity(
    provider: Option<&CapacityProviderRecord>,
    registered_instances: usize,
) -> bool {
    registered_instances == 0
        && provider.is_some_and(|provider| {
            provider.auto_scaling_group_provider.is_some()
                && !needs_registered_capacity_now(provider)
        })
}

/// The `lastStatus` a task is placed with: [`STATUS_PROVISIONING`] while its capacity is
/// still being acquired, else [`STATUS_PENDING`]. Shared by every path that places a task,
/// so a task starts in the same lifecycle state whoever placed it.
pub fn initial_task_status(awaiting_capacity: bool) -> &'static str {
    if awaiting_capacity {
        STATUS_PROVISIONING
    } else {
        STATUS_PENDING
    }
}

/// Where a task the SERVICE's own scheduler places takes its capacity from. "A service
/// will return either a `capacityProviderStrategy` or `launchType` parameter, but not
/// both", so exactly one of the two decides the placement.
pub fn service_placement(service: &ServiceRecord) -> Placement {
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
    Placement {
        launch_type,
        capacity_provider_name,
        platform_version: service.platform_version.clone(),
        started_by: service.deployment_id.clone(),
    }
}

/// Where a task a TASK SET places takes its capacity from. The task set carries its own
/// launch type or capacity provider strategy ("If a `launchType` is specified, the
/// `capacityProviderStrategy` parameter must be omitted"), so its tasks are placed on
/// that rather than on the service's.
pub fn task_set_placement(task_set: &TaskSetRecord) -> Placement {
    Placement {
        launch_type: task_set.launch_type.clone(),
        capacity_provider_name: task_set
            .capacity_provider_strategy
            .first()
            .map(|item| item.capacity_provider.clone()),
        platform_version: task_set.platform_version.clone(),
        started_by: task_set.id.clone(),
    }
}

/// `ManagedScaling$status` when Amazon ECS scales the Auto Scaling group itself, from the
/// `ManagedScalingStatus` enum (`ENABLED` / `DISABLED`). The member is optional and the
/// doc states the default is off ("the default is off"), so an absent or `DISABLED`
/// status is managed scaling OFF.
pub const MANAGED_SCALING_ENABLED: &str = "ENABLED";

/// Check the `ClusterSettings` of a request against [`CLUSTER_SETTING_VALUES`],
/// answering the message an `InvalidParameterException` carries when one is outside it.
///
/// Shared by every operation that accepts cluster settings, so the same value is
/// accepted or refused whichever of them a client calls.
pub fn validate_cluster_settings(settings: &[ClusterSetting]) -> Result<(), String> {
    for setting in settings {
        let value = setting.value.as_deref().unwrap_or_default();
        if !CLUSTER_SETTING_VALUES.contains(&value) {
            return Err(format!(
                "settings value must be one of {}; got {value:?}",
                CLUSTER_SETTING_VALUES.join(", ")
            ));
        }
    }
    Ok(())
}

/// Check a `ClusterConfiguration` a request carries, answering the message an
/// `InvalidParameterException` carries when it is not usable.
///
/// `ExecuteCommandConfiguration$logConfiguration`: "When `logging=OVERRIDE` is
/// specified, a `logConfiguration` must be provided." Shared by every operation that
/// accepts a cluster configuration.
pub fn validate_cluster_configuration(configuration: &ClusterConfiguration) -> Result<(), String> {
    if let Some(execute_command) = &configuration.execute_command_configuration {
        if execute_command.logging.as_deref() == Some(EXECUTE_COMMAND_LOGGING_OVERRIDE)
            && execute_command.log_configuration.is_none()
        {
            return Err(format!(
                "configuration.executeCommandConfiguration.logConfiguration is required \
                 when logging is {EXECUTE_COMMAND_LOGGING_OVERRIDE}"
            ));
        }
    }
    Ok(())
}

/// The stored default Service Connect namespace a request's
/// `ClusterServiceConnectDefaultsRequest` asks for.
///
/// The member's doc: "If you update the cluster with an empty string `\"\"` for the
/// namespace name, the cluster configuration for Service Connect is removed" — so an
/// empty namespace CLEARS the default rather than storing a nameless one.
pub fn service_connect_defaults(namespace: &str) -> Option<ClusterServiceConnectDefaults> {
    if namespace.is_empty() {
        None
    } else {
        Some(ClusterServiceConnectDefaults {
            namespace: Some(namespace.to_string()),
        })
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

/// The ARN of the identity every caller of this emulator authenticates as, and so the
/// principal a "who did this" member names (`TaskDefinition$registeredBy`: "The
/// principal that registered the task definition").
///
/// Every request this emulator serves is signed with the emulator's own credentials, so
/// the caller is the one IAM user it publishes — the very principal its
/// `sts:GetCallerIdentity` answers, built the same way, so the principal a response
/// names is one the caller can look up.
///
/// Distinct from [`caller_principal`], which spells a per-request name out of the
/// request's own access key id: a member AWS fills from the AUTHENTICATED IAM identity
/// has to name an identity that identity service knows, and a name derived from an
/// access key id is not one.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The ARN of the account's ROOT user — the principal whose account settings are the
/// account-wide defaults (`PutAccountSettingDefault`: "Modifies an account setting for
/// all users on an account for whom no individual account setting has been specified").
pub fn root_principal_arn() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// An account setting value together with the principal it came from.
#[derive(Debug, Clone)]
pub struct ResolvedSetting {
    pub value: String,
    pub principal_arn: String,
}

/// The `Setting` a response reports for one name, value and principal.
///
/// `Setting$type` comes from the catalog rather than the request: it "indicates whether
/// Amazon Web Services manages the account setting, or if the user manages it", which is
/// a property of the NAME ([`ACCOUNT_SETTINGS`]).
pub fn setting_view(name: &str, value: &str, principal_arn: &str) -> Setting {
    Setting {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
        principal_arn: Some(principal_arn.to_string()),
        r#type: Some(
            account_setting_spec(name)
                .map_or(SETTING_TYPE_USER, |setting| setting.setting_type)
                .to_string(),
        ),
    }
}

/// Check the `value` a write of the account setting `name` carries against the values
/// that name accepts ([`ACCOUNT_SETTINGS`]), answering the message an
/// `InvalidParameterException` carries when it is outside them.
///
/// Shared by `PutAccountSetting` and `PutAccountSettingDefault`, so the same value is
/// accepted or refused whichever of them a client calls.
pub fn validate_account_setting_value(name: &str, value: &str) -> Result<(), String> {
    let Some(spec) = account_setting_spec(name) else {
        // Unreachable from the wire: `name` targets the closed `SettingName` enum, which
        // the request walk has already checked.
        return Err(format!("name must be an Amazon ECS account setting; got {name:?}"));
    };
    if !spec.accepted_values.contains(&value) {
        return Err(format!(
            "value for {name} must be one of {}; got {value:?}",
            spec.accepted_values.join(", ")
        ));
    }
    Ok(())
}

/// Whether `principal_arn` is a principal an account setting may be written for, and a
/// key this store may hold.
///
/// `PutAccountSetting$principalArn`: "The ARN of the principal, which can be a user,
/// role, or the root user" — an IAM ARN, which is the six colon-separated fields
/// `arn:<partition>:iam::<account>:<resource>` ([`::engine::emulator_arn`] builds every
/// one this emulator emits the same way, with the empty region a global service carries).
/// A value that is not one names no principal a setting could belong to, and is refused
/// before it can become a key — as is one longer than [`MAX_PRINCIPAL_ARN_LENGTH`].
pub fn is_principal_arn(principal_arn: &str) -> bool {
    if principal_arn.len() > MAX_PRINCIPAL_ARN_LENGTH {
        return false;
    }
    // The resource is the LAST field, so it keeps any colon of its own.
    let fields: Vec<&str> = principal_arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == "iam"
        && fields[3].is_empty()
        && !fields[4].is_empty()
        && !fields[5].is_empty()
}

/// The message an `InvalidParameterException` carries when a request's `principalArn`
/// is not a principal an account setting may belong to ([`is_principal_arn`]): the
/// member that is wrong and the value it was given.
///
/// Shared by every account-setting operation that takes a `principalArn`, so the same
/// value is refused with the same sentence whichever of them a client calls.
pub fn invalid_principal_arn_message(principal_arn: &str) -> String {
    format!("Invalid PrincipalArn: {principal_arn}")
}

/// The ARN resource type a task-definition revision is named under, as the resource
/// segment of [`task_definition_arn`] and the one
/// [`parse_task_definition_reference`] reads a reference's family out of.
pub const TASK_DEFINITION_RESOURCE_TYPE: &str = "task-definition";

/// The ARN resource type a DAEMON task-definition revision is named under — its own
/// resource, so a daemon revision's ARN is never a plain revision's and neither
/// reference form resolves to the other's store.
pub const DAEMON_TASK_DEFINITION_RESOURCE_TYPE: &str = "daemon-task-definition";

/// The ARN of a task-definition revision:
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
pub fn task_definition_arn(region: &str, family: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_DEFINITION_RESOURCE_TYPE}/{family}:{revision}"),
    )
}

/// The ARN of a daemon task-definition revision:
/// `arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:<revision>`.
pub fn daemon_task_definition_arn(region: &str, family: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DAEMON_TASK_DEFINITION_RESOURCE_TYPE}/{family}:{revision}"),
    )
}

/// The ARN resource type a container instance is named under, as the resource segment of
/// [`container_instance_arn`].
pub const CONTAINER_INSTANCE_RESOURCE_TYPE: &str = "container-instance";

/// The ARN of a container instance:
/// `arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`.
pub fn container_instance_arn(region: &str, cluster_name: &str, instance_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CONTAINER_INSTANCE_RESOURCE_TYPE}/{cluster_name}/{instance_id}"),
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

/// The ARN of the IAM role the CREATE named, resolved from the `role` reference it
/// carried, and `None` when it named none.
///
/// `CreateService$role`: "The name or full Amazon Resource Name (ARN) of the IAM role".
/// Which services report the ECS SERVICE-LINKED role instead is decided by what the
/// service IS, not by this request alone, so it is resolved where the service is reported
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_role_arn`]).
pub fn requested_role_arn(requested: Option<&str>) -> Option<String> {
    match requested {
        // "If your specified role has a path other than /, then you must either specify
        // the full role ARN (this is recommended) or prefix the role name with the path."
        Some(reference) if reference.starts_with("arn:") => Some(reference.to_string()),
        Some(name) => Some(::engine::emulator_arn(
            "iam",
            "",
            &format!("role/{}", name.trim_start_matches('/')),
        )),
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

/// The ARN of a service deployment:
/// `arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<id>`.
pub fn service_deployment_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    deployment_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("service-deployment/{cluster_name}/{service_name}/{deployment_id}"),
    )
}

/// Why a rollout is in the state it is in, for the deployment that rolls it out.
///
/// `Deployment$rolloutStateReason` is "A description of the rollout state of a
/// deployment"; the service deployment that records the SAME rollout reports it as its own
/// `statusReason` ("Information about why the service deployment is in the current
/// status"), so both read it from here and cannot drift.
pub fn rollout_state_reason(deployment_id: &str, settled: bool) -> String {
    if settled {
        format!("ECS deployment {deployment_id} completed.")
    } else {
        format!("ECS deployment {deployment_id} in progress.")
    }
}

/// The task-set ID a `taskSet` reference names: the value as given when it is an id,
/// otherwise the id a task-set ARN ends with
/// (`task-set/<cluster>/<service>/<id>`, where the id itself carries an `ecs-svc/`
/// prefix). The member's doc: "The ID or full Amazon Resource Name (ARN) of task sets",
/// so a reference is either form and an ARN is never wrapped in another one.
pub fn task_set_id_from_reference(reference: &str) -> String {
    let reference = reference.trim();
    match reference.rsplit_once(":task-set/") {
        Some((_, tail)) => tail.splitn(3, '/').nth(2).unwrap_or(tail).to_string(),
        None => reference.to_string(),
    }
}

/// Why a `clientToken` a request carries cannot be stored, or `None` when it can.
///
/// Every such member's doc: "It must be unique and is case sensitive. Up to 36 ASCII
/// characters in the range of 33-126 (inclusive) are allowed." Shared by every operation
/// that takes one, so a token one of them refuses is refused by the others too.
pub fn client_token_violation(token: &str) -> Option<String> {
    let legal = token.len() <= MAX_CLIENT_TOKEN_LENGTH
        && !token.is_empty()
        && token.bytes().all(|byte| (33..=126).contains(&byte));
    if legal {
        return None;
    }
    Some(format!(
        "clientToken must be up to {MAX_CLIENT_TOKEN_LENGTH} ASCII characters in the \
         range 33-126; got {token:?}"
    ))
}

/// The ARN of a service: `arn:aws:ecs:<region>:<account>:service/<cluster>/<name>`.
pub fn service_arn(region: &str, cluster_name: &str, service_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("service/{cluster_name}/{service_name}"),
    )
}

/// The ARN of a task set:
/// `arn:aws:ecs:<region>:<account>:task-set/<cluster>/<service>/<task set id>`. The id a
/// task set carries already has its own `ecs-svc/` prefix, so the ARN ends with it.
pub fn task_set_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    task_set_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("task-set/{cluster_name}/{service_name}/{task_set_id}"),
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

/// The ARN resource type a task is named under, as the resource segment of [`task_arn`].
pub const TASK_RESOURCE_TYPE: &str = "task";

/// The ARN of a task: `arn:aws:ecs:<region>:<account>:task/<cluster>/<id>`.
pub fn task_arn(region: &str, cluster_name: &str, task_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_RESOURCE_TYPE}/{cluster_name}/{task_id}"),
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

/// The ARN of a capacity provider: `arn:aws:ecs:<region>:<account>:capacity-provider/<name>`.
pub fn capacity_provider_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("capacity-provider/{name}"))
}

/// The record of one of the capacity providers Amazon ECS MANAGES
/// ([`MANAGED_CAPACITY_PROVIDERS`]), or `None` for a name that is not one of them.
///
/// Every account holds these in every Region: nothing creates them and nothing can take
/// their names, so they are resolved from the name and the request's own Region rather
/// than stored — which is also what keeps the ARN they report in the Region the caller
/// asked in. They bind onto Fargate capacity, which is neither a caller's Auto Scaling
/// group nor the managed instances Amazon ECS runs for a cluster, so they carry neither
/// provider configuration and no `type`; `CapacityProvider$status` is
/// [`CAPACITY_PROVIDER_ACTIVE`] — they are there and usable, and nothing can delete them.
pub fn managed_capacity_provider(name: &str, region: &str) -> Option<CapacityProviderRecord> {
    if !MANAGED_CAPACITY_PROVIDERS.contains(&name) {
        return None;
    }
    Some(CapacityProviderRecord {
        name: name.to_string(),
        arn: capacity_provider_arn(region, name),
        status: CAPACITY_PROVIDER_ACTIVE.to_string(),
        cluster: None,
        auto_scaling_group_provider: None,
        managed_instances_provider: None,
        provider_type: None,
        tags: Vec::new(),
    })
}

/// The message the refusal of a capacity-provider `name` carries. A fixed sentence, as
/// the recorded refusal carries it, restating the whole of the name rule
/// `CreateCapacityProvider$name` documents — the length, the character set and the
/// prefixes Amazon ECS keeps for its own providers — whichever half of it the value broke.
pub const CAPACITY_PROVIDER_NAME_INVALID_MESSAGE: &str =
    "The specified capacity provider name is invalid. Up to 255 characters are allowed, \
     including letters (upper and lowercase), numbers, underscores, and hyphens. The name \
     cannot be prefixed with \"aws\", \"ecs\", or \"fargate\". Specify a valid name and \
     try again.";

/// Whether a capacity-provider `name` is one the caller may take, or the refusal it earns.
///
/// The `CreateCapacityProvider$name` doc: "Up to 255 characters are allowed. They include
/// letters (both upper and lowercase letters), numbers, underscores (_), and hyphens (-).
/// The name can't be prefixed with \"aws\", \"ecs\", or \"fargate\"." The character rule is
/// [`is_valid_name`]'s, shared with every other ECS name; the reserved prefixes are this
/// member's own, and are what keeps a caller from shadowing `FARGATE`/`FARGATE_SPOT`.
///
/// The name is the provider's resource IDENTIFIER (the model's `resourceIdentifier` on
/// this member), so a value outside that one rule is refused as the `ClientException`
/// modelled for "specifying an identifier that isn't valid", carrying the one sentence
/// that states the rule.
pub fn validate_capacity_provider_name(name: &str) -> Result<(), &'static str> {
    let lowered = name.to_ascii_lowercase();
    let reserved = CAPACITY_PROVIDER_RESERVED_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix));
    if !is_valid_name(name) || reserved {
        return Err(CAPACITY_PROVIDER_NAME_INVALID_MESSAGE);
    }
    Ok(())
}

/// One kind of Amazon ECS resource that carries its OWN tag set, and so can be named by
/// a tagging operation's `resourceArn`.
///
/// The kinds are the ones those members document — `ListTagsForResource$resourceArn`:
/// "the supported resources are Amazon ECS tasks, services, task definitions, clusters,
/// and container instances", and `TagResource$resourceArn`, which adds the capacity
/// provider — and each one's ARN form is the one the builder named on its variant emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// [`capacity_provider_arn`].
    CapacityProvider,
    /// [`cluster_arn`].
    Cluster,
    /// [`container_instance_arn`].
    ContainerInstance,
    /// [`service_arn`].
    Service,
    /// [`task_arn`].
    Task,
    /// [`task_definition_arn`].
    TaskDefinition,
}

/// The ARN resource-type segment each taggable kind carries, as that kind's ARN builder
/// in this module spells it. The segments are the catalog [`TaggedResourceKind::from_arn`]
/// resolves an ARN against, and the list a refusal names.
pub const TAGGED_RESOURCE_TYPES: [(&str, TaggedResourceKind); 6] = [
    ("capacity-provider", TaggedResourceKind::CapacityProvider),
    ("cluster", TaggedResourceKind::Cluster),
    ("container-instance", TaggedResourceKind::ContainerInstance),
    ("service", TaggedResourceKind::Service),
    ("task", TaggedResourceKind::Task),
    (
        TASK_DEFINITION_RESOURCE_TYPE,
        TaggedResourceKind::TaskDefinition,
    ),
];

/// Which MODELLED error a "this `resourceArn` names no such resource" refusal takes.
///
/// Spelled as a kind rather than as one operation's error enum, so every operation that
/// resolves a `resourceArn` refuses the same ARN with the same error code and the same
/// sentence, whichever of them the client called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceRefusal {
    /// `ClusterNotFoundException` — the refusal ECS models for a cluster that is not
    /// there, and the one every operation in this crate answers for that condition.
    ClusterNotFound,
    /// `ClientException`.
    Client,
    /// `InvalidParameterException`.
    InvalidParameter,
}

impl TaggedResourceKind {
    /// The kind `resource_arn` names, or `None` when it is not the ARN of a taggable
    /// Amazon ECS resource at all — a value in another service's namespace, a malformed
    /// ARN, or an ECS resource that carries no tag set of its own.
    ///
    /// The KIND is read from the resource type alone. The partition, Region and account
    /// are part of the resource's IDENTITY, not of its kind, and the lookups above
    /// compare a whole ARN against the whole ARN the record stores (which carries the
    /// serving Region and account) — so an ECS ARN of another partition, Region or
    /// account names a resource of a known kind that this store does not hold, and earns
    /// that kind's own not-found refusal rather than being called a malformed parameter.
    pub fn from_arn(resource_arn: &str) -> Option<Self> {
        // `arn:<partition>:ecs:<region>:<account>:<resource>`. The resource is the LAST
        // field, so it keeps any colon of its own (a task definition's revision).
        let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
        if fields.len() != 6 || fields[0] != "arn" || fields[2] != ARN_NAMESPACE {
            return None;
        }
        let resource_type = fields[5].split('/').next().unwrap_or_default();
        TAGGED_RESOURCE_TYPES
            .iter()
            .find(|(segment, _)| *segment == resource_type)
            .map(|(_, kind)| *kind)
    }

    /// The refusal a `resourceArn` of this kind earns when the store holds no such
    /// resource: the modelled error it takes and the sentence that error carries.
    ///
    /// Each sentence is the one this crate already answers wherever that kind fails to
    /// resolve, so "this ARN names no cluster" reads the same from a tagging operation as
    /// from a describe.
    pub fn not_found_refusal(self, resource_arn: &str) -> (ResourceRefusal, String) {
        match self {
            Self::Cluster => (
                ResourceRefusal::ClusterNotFound,
                CLUSTER_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::TaskDefinition => (
                ResourceRefusal::Client,
                TASK_DEFINITION_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::Service => (
                ResourceRefusal::InvalidParameter,
                SERVICE_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::Task => (
                ResourceRefusal::InvalidParameter,
                TASK_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::ContainerInstance => (
                ResourceRefusal::InvalidParameter,
                container_instance_not_found_message(resource_arn),
            ),
            Self::CapacityProvider => (
                ResourceRefusal::InvalidParameter,
                resource_not_found_message(resource_arn),
            ),
        }
    }
}

/// The `message` a refusal carries for a `resourceArn` that names no resource of a kind
/// this crate holds no sentence of its own for: the `ResourceNotFoundException` doc
/// sentence ("The specified resource wasn't found.") with the ARN the request named, so
/// a caller can tell which ARN was rejected.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("The specified resource wasn't found: {resource_arn}")
}

/// `InvalidParameterException$message` for a `resourceArn` that is not the ARN of a
/// taggable Amazon ECS resource ([`TaggedResourceKind::from_arn`] answers `None`).
///
/// DERIVED from the request, like the other reference refusals in this module: it names
/// the member, the resource types the member accepts ([`TAGGED_RESOURCE_TYPES`]) and the
/// value that is none of them.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    let kinds: Vec<&str> = TAGGED_RESOURCE_TYPES
        .iter()
        .map(|(segment, _)| *segment)
        .collect();
    format!(
        "resourceArn must be an Amazon ECS resource ARN naming one of {}; got \
         {resource_arn:?}",
        kinds.join(", ")
    )
}

/// The prefix Amazon Web Services keeps for its OWN tags, so no caller-supplied tag may
/// carry it: the `Tag` doc — "Do not use `aws:`, `AWS:`, or any upper or lowercase
/// combination of such as a prefix for either keys or values as it is reserved for Amazon
/// Web Services use. You cannot edit or delete tag keys or values with this prefix."
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The refusal a request's `tags` list earns when one of its entries would write into the
/// reserved namespace above, or `None` when every entry is the caller's own to write.
///
/// The restriction is the service's, not the model's: `TagKey`'s pattern admits ':', so a
/// reserved key is a well-formed value the SERVICE refuses — which is why every operation
/// that takes a `tags` list checks it here, and reports it as the bad parameter it is.
/// "any upper or lowercase combination of such", so the prefix compares without regard to
/// case, and it is checked on "either keys or values".
///
/// The message names the MEMBER of the entry that failed and the prefix it may not carry,
/// so a caller is pointed at the one thing to change; like the other fixed refusals in
/// this module it carries none of the request's values, since the condition is about the
/// prefix and not about what follows it.
pub fn reserved_tag_prefix_violation(tags: &[Tag]) -> Option<String> {
    tags.iter().find_map(|tag| {
        [("key", tag.key.as_deref()), ("value", tag.value.as_deref())]
            .into_iter()
            .find_map(|(member, value)| {
                starts_with_reserved_tag_prefix(value.unwrap_or_default()).then(|| {
                    format!("Tag {member} must not start with {RESERVED_TAG_PREFIX}")
                })
            })
    })
}

/// Whether `value` carries [`RESERVED_TAG_PREFIX`], in any combination of cases.
fn starts_with_reserved_tag_prefix(value: &str) -> bool {
    value
        .get(..RESERVED_TAG_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_PREFIX))
}

/// Associate `requested` with the tag set `stored`, the way a tagging write does:
/// "Associates the specified tags to a resource with the specified resourceArn. If
/// existing tags on a resource aren't specified in the request parameters, they aren't
/// changed."
///
/// So a key the request names takes the value the request gives it ("each tag key must be
/// unique, and each tag key can have only one value"), a key the resource does not carry
/// yet is added, and a key the request does not name keeps the value it had. Keys compare
/// EXACTLY — "Tag keys and values are case-sensitive" — and the request's entries are
/// applied in order, so the resource ends up with one entry per key whatever the request
/// listed. The value is stored as the request gave it, so an empty value stays an empty
/// value rather than reading as a key with none.
///
/// `Err` carries the message of the resource limit the merged set would break ("Maximum
/// number of tags per resource - 50", [`MAX_TAGS_PER_RESOURCE`]); `stored` is left
/// untouched in that case, so a refused write changes nothing.
///
/// The limit is what bounds the WORK one write may ask for: how many tags the write would
/// leave the resource carrying is ONE entry per distinct key across the two lists, counted
/// in one pass before anything is copied, so a write over the limit is refused before the
/// merge is done; and the merge itself is keyed through an index over the stored keys, so
/// applying the request costs one lookup per entry rather than a walk over the set it is
/// growing.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) -> Result<(), String> {
    let merged_count = stored
        .iter()
        .chain(requested)
        .map(|tag| tag.key.as_deref())
        .collect::<HashSet<Option<&str>>>()
        .len();
    if merged_count > MAX_TAGS_PER_RESOURCE {
        return Err(tags_per_resource_limit_message(merged_count));
    }
    let mut merged = stored.clone();
    let mut positions: HashMap<Option<String>, usize> = HashMap::with_capacity(merged.len());
    for (position, tag) in merged.iter().enumerate() {
        positions.entry(tag.key.clone()).or_insert(position);
    }
    for tag in requested {
        match positions.get(&tag.key) {
            Some(&position) => merged[position].value = tag.value.clone(),
            None => {
                positions.insert(tag.key.clone(), merged.len());
                merged.push(tag.clone());
            }
        }
    }
    *stored = merged;
    Ok(())
}

/// Delete the tags `tag_keys` names from the tag set `stored`: "Deletes specified tags
/// from a resource", where the keys are "the keys of the tags to be removed".
///
/// A key the resource does not carry removes nothing, so asking twice leaves the same tag
/// set as asking once, and a key the request does not name is left where it is.
pub fn remove_tags(stored: &mut Vec<Tag>, tag_keys: &[String]) {
    stored.retain(|tag| {
        !tag_keys
            .iter()
            .any(|removed| tag.key.as_deref() == Some(removed.as_str()))
    });
}

/// `LimitExceededException$message` for a tagging write whose merged tag set would carry
/// more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// DERIVED from the request: it names the limit and how many tags the resource would be
/// left carrying, so a caller can tell how far over it went.
pub fn tags_per_resource_limit_message(merged_count: usize) -> String {
    format!(
        "The limit of {MAX_TAGS_PER_RESOURCE} tags per resource was exceeded: the request \
         would leave the resource with {merged_count} tags"
    )
}

/// `AwsVpcConfiguration$assignPublicIp` as Amazon ECS materialises it when a service
/// leaves it out — the member's own doc: "When you use `create-service` or
/// `update-service`, the default is `DISABLED`." One of the `AssignPublicIp` enum's two
/// values (`smithy_query(query="shape", shape="AssignPublicIp")`).
pub const ASSIGN_PUBLIC_IP_DISABLED: &str = "DISABLED";

/// The network configuration a service STORES: the one the request carried, with the
/// members Amazon ECS fills in filled in, so every later read reports the same rendered
/// configuration rather than only what the caller sent.
///
/// `assignPublicIp` takes the value its own doc gives it — "When you use `create-service`
/// or `update-service`, the default is `DISABLED`" — and `securityGroups`, which a
/// request may leave out, is MATERIALISED as an empty list, as the recorded read-back of
/// a configuration that named none carries it: the group the task actually gets is then
/// "the default security group for the VPC", whose id belongs to the VPC rather than to
/// this configuration, so none is invented in its place. `subnets` is required, so there
/// is nothing to fill in for it.
pub fn defaulted_network_configuration(
    requested: Option<&NetworkConfiguration>,
) -> Option<NetworkConfiguration> {
    let configuration = requested?;
    let awsvpc_configuration = configuration
        .awsvpc_configuration
        .as_ref()
        .map(|awsvpc| AwsVpcConfiguration {
            assign_public_ip: Some(
                awsvpc
                    .assign_public_ip
                    .clone()
                    .unwrap_or_else(|| ASSIGN_PUBLIC_IP_DISABLED.to_string()),
            ),
            security_groups: Some(awsvpc.security_groups.clone().unwrap_or_default()),
            ..awsvpc.clone()
        });
    Some(NetworkConfiguration {
        awsvpc_configuration,
    })
}

/// The refusal a launch earns when its `networkConfiguration` does not agree with the
/// network mode of the task definition it runs, or `None` when the two agree.
///
/// The rule, which the model cannot express because `networkConfiguration` is optional on
/// every launch: a task definition in the `awsvpc` network mode gets its own elastic
/// network interface, so the launch has to say which subnets and security groups that
/// interface takes — and a task definition in any other mode has no interface of its own
/// to configure, so a network configuration is not a thing that launch can carry.
///
/// Shared by every operation that launches a task from a stored revision, so `RunTask`
/// and `CreateService` cannot answer differently about the same pair.
pub fn network_configuration_violation(
    network_mode: &str,
    network_configuration: Option<&NetworkConfiguration>,
) -> Option<String> {
    // `NetworkConfiguration` carries exactly one member, `awsvpcConfiguration`, so a
    // configuration without it configures no interface and is not one.
    let configured = network_configuration
        .is_some_and(|configuration| configuration.awsvpc_configuration.is_some());
    match (network_mode == NETWORK_MODE_AWSVPC, configured) {
        (true, false) => Some(AWSVPC_NETWORK_CONFIGURATION_REQUIRED_MESSAGE.to_string()),
        (false, true) => Some(format!(
            "Network Configuration must not be provided when networkMode \
             '{network_mode}' is specified."
        )),
        _ => None,
    }
}

/// The refusal an exec-enabled launch earns when the task definition it runs declares no
/// `taskRoleArn`, or `None` when it declares one (or when the launch does not enable ECS
/// Exec at all).
///
/// The rule: ECS Exec runs through the SSM agent inside the task, which signs its calls
/// with the task's own IAM role — so a launch that turns `enableExecuteCommand` on
/// requires the revision to carry a task role. `subject` names what the refusal is about
/// ("The service couldn't be created", "The task couldn't be launched"), since the
/// sentence AWS words is the one the calling operation's own answer carries. Its
/// remediation clause names WHERE the role belongs — in the task definition, which is the
/// resource the caller has to change, rather than on this request.
pub fn execute_command_violation(
    enabled: bool,
    definition: Option<&TaskDefinitionRecord>,
    subject: &str,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let has_role = definition.is_some_and(|found| {
        found
            .task_role_arn
            .as_deref()
            .is_some_and(|arn| !arn.trim().is_empty())
    });
    (!has_role).then(|| {
        format!(
            "{subject} because a valid taskRoleArn is not being used. Specify a valid \
             task role in your task definition and try again."
        )
    })
}

/// What [`execute_command_violation`] names in a `CreateService` refusal.
pub const EXECUTE_COMMAND_SERVICE_SUBJECT: &str = "The service couldn't be created";

/// What [`execute_command_violation`] names in a `RunTask` refusal: the launch of the
/// task itself, which is what ECS Exec attaches to.
pub const EXECUTE_COMMAND_TASK_SUBJECT: &str = "The task couldn't be launched";

/// What [`execute_command_violation`] names in an `UpdateService` refusal.
pub const EXECUTE_COMMAND_SERVICE_UPDATE_SUBJECT: &str = "The service couldn't be updated";

/// The refusal a `loadBalancers` list earns when one of its bindings names a container,
/// or a container port, the task definition it is bound to does not declare — or `None`
/// when every binding resolves.
///
/// The rule: a load-balancer binding attaches ONE container port of the task to a target
/// group, so the container it names must be one of the revision's `containerDefinitions`
/// and the port must be one that container declares in its `portMappings` (as a single
/// `containerPort`, or inside a `containerPortRange`). The refusal names the container and
/// the port at fault, because it is derived from the request.
pub fn load_balancer_binding_violation(
    definition: Option<&TaskDefinitionRecord>,
    load_balancers: &[LoadBalancer],
) -> Option<String> {
    let definition = definition?;
    for binding in load_balancers {
        let Some(container_name) = binding.container_name.as_deref() else {
            return Some(
                "A load balancer configuration must specify the containerName it \
                 attaches."
                    .to_string(),
            );
        };
        let Some(container) = definition
            .container_definitions
            .iter()
            .find(|container| container.name.as_deref() == Some(container_name))
        else {
            return Some(format!(
                "The container {container_name} does not exist in the task definition."
            ));
        };
        let Some(container_port) = binding.container_port else {
            return Some(format!(
                "The container {container_name} load balancer configuration must specify \
                 a containerPort."
            ));
        };
        let declared = container
            .port_mappings
            .iter()
            .flatten()
            .any(|mapping| port_mapping_covers(mapping, container_port));
        if !declared {
            return Some(format!(
                "The container {container_name} did not have a container port \
                 {container_port} defined."
            ));
        }
    }
    None
}

/// Whether one `portMapping` declares `port` as a container port: either as its own
/// `containerPort`, or inside the `containerPortRange` it declares instead ("The port
/// number range on the container that's bound to the dynamically mapped host port
/// range", written `<start>-<end>`).
fn port_mapping_covers(mapping: &PortMapping, port: i32) -> bool {
    if mapping.container_port == Some(port) {
        return true;
    }
    mapping
        .container_port_range
        .as_deref()
        .and_then(|range| range.split_once('-'))
        .and_then(|(start, end)| {
            Some((start.trim().parse::<i32>().ok()?, end.trim().parse::<i32>().ok()?))
        })
        .is_some_and(|(start, end)| (start..=end).contains(&port))
}

/// The target groups of `arns` that NO load balancer forwards to, read from the Elastic
/// Load Balancing v2 service's OWN store through the one shared engine — the same
/// one-directional, guard-free read [`target_group_target_types`] makes, and under the
/// same LOCK ORDER: it takes the ELBv2 store's lock, holds no other while it does, and
/// hands back owned values, so it is called with NO guard of this service's store held.
///
/// An ARN that names no stored target group is NOT reported here: whether a group exists
/// at all is [`load_balancer_refusal`]'s answer, and reporting it twice would give one
/// condition two sentences.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own store
/// being unreachable.
pub fn unattached_target_groups<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arns: &[String],
) -> Option<Vec<String>> {
    let state =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        arns.iter()
            .filter(|arn| {
                store
                    .target_groups
                    .get(*arn)
                    .is_some_and(|group| group.load_balancer_arns.is_empty())
            })
            .cloned()
            .collect(),
    )
}

/// The refusal a `loadBalancers` list earns when one of the target groups it names has no
/// load balancer forwarding to it — or `None` when every named group is attached to one.
///
/// The rule: a service registers its tasks as TARGETS of the group, and only a listener
/// on a load balancer can send traffic to a group — so a group no load balancer forwards
/// to can never reach the tasks, and ECS refuses the binding at create time. The
/// association is LIVE: `unattached_target_groups` reads it at request time out of the
/// records the ELBv2 service holds (`TargetGroupRecord::load_balancer_arns`, which its own
/// `DescribeTargetGroups` reports as `LoadBalancerArns` and its listeners maintain), so
/// the same group is refused before a listener forwards to it and accepted after.
///
/// `arns` fixes the order the bindings are reported in, so a request naming two
/// unattached groups is always refused on the same one.
pub fn orphan_target_group_refusal(arns: &[String], unattached: &[String]) -> Option<String> {
    arns.iter()
        .find(|arn| unattached.contains(*arn))
        .map(|arn| {
            format!(
                "The target group with targetGroupArn {arn} does not have an associated \
                 load balancer."
            )
        })
}

/// Whether an `include` list asks for the resource's TAGS.
///
/// Every ECS `include` member targets a closed `…Field` enum that declares `TAGS`
/// (`CapacityProviderField`, `ClusterField`, `ContainerInstanceField`, `ServiceField`,
/// `TaskField`), and a field outside the enum cannot reach a handler — the boundary
/// validator refuses it — so the question is only whether `TAGS` is among them.
pub fn include_requests_tags(include: Option<&Vec<String>>) -> bool {
    include
        .into_iter()
        .flatten()
        .any(|field| field == INCLUDE_FIELD_TAGS)
}

/// The `include` field every ECS describe gates its tag set on.
pub const INCLUDE_FIELD_TAGS: &str = "TAGS";

/// The capacity-provider NAME a `capacityProviders` entry refers to: the value as given
/// when it is a short name, and the name out of a capacity-provider ARN
/// (`arn:aws:ecs:<region>:<account>:capacity-provider/<name>`) when it is one — the
/// member's doc, "The short name or full Amazon Resource Name (ARN) of one or more
/// capacity providers".
pub fn capacity_provider_name_from_reference(reference: &str) -> String {
    match reference.rsplit_once(":capacity-provider/") {
        Some((_, name)) => name.to_string(),
        None => reference.to_string(),
    }
}

/// The Auto Scaling group an `autoScalingGroupArn` member names, as the key two
/// capacity providers are compared on: the group NAME, which is the last segment of an
/// Auto Scaling group ARN
/// (`arn:aws:autoscaling:<region>:<account>:autoScalingGroup:<id>:autoScalingGroupName/<name>`)
/// and the value itself when the member carries the name ("The Amazon Resource Name
/// (ARN) that identifies the Auto Scaling group, or the Auto Scaling group name").
///
/// Naming one group by ARN and by name is naming the SAME group, so the reuse rule
/// cannot be slipped by spelling it the other way.
pub fn auto_scaling_group_key(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

/// The `family` and optional `revision` a `taskDefinition` request member names.
///
/// The member's doc: "The family and revision (family:revision) or full ARN of the task
/// definition to run. If a revision isn't specified, the latest ACTIVE revision is
/// used." `None` when the value is not a task-definition reference at all.
pub fn parse_task_definition_reference(reference: &str) -> Option<(String, Option<i32>)> {
    parse_revision_reference(reference, TASK_DEFINITION_RESOURCE_TYPE)
}

/// The `family` and optional `revision` a `daemonTaskDefinition` request member names.
///
/// The member's doc: "The family for the latest ACTIVE revision, family and revision
/// (family:revision) for a specific revision in the family, or full Amazon Resource Name
/// (ARN) of the daemon task definition" — the same three forms
/// [`parse_task_definition_reference`] reads, under this resource's own ARN type.
pub fn parse_daemon_task_definition_reference(reference: &str) -> Option<(String, Option<i32>)> {
    parse_revision_reference(reference, DAEMON_TASK_DEFINITION_RESOURCE_TYPE)
}

/// The `family` and optional `revision` a reference to a revision of `resource_type`
/// names: a family, a `family:revision`, or the full ARN of either.
///
/// One function for both resources whose references take those three forms, so a value
/// is read the same way whichever member carried it.
fn parse_revision_reference(
    reference: &str,
    resource_type: &str,
) -> Option<(String, Option<i32>)> {
    let tail = match reference.strip_prefix("arn:") {
        Some(_) => reference.rsplit_once(&format!(":{resource_type}/"))?.1,
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

/// A wire timestamp as a key that ORDERS and COMPARES: whole seconds and the nanoseconds
/// within them. [`DateTime`] is not itself ordered, and comparing two of them through
/// floating-point seconds would make two instants a few nanoseconds apart compare equal.
pub fn timestamp_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The `TargetType` a target group must carry to receive the tasks of a revision whose
/// `networkMode` is `network_mode`.
///
/// `LoadBalancer$targetGroupArn`: "If your service's task definition uses the `awsvpc`
/// network mode, you must choose `ip` as the target type, not `instance`. Do this when
/// creating your target groups because tasks that use the `awsvpc` network mode are
/// associated with an elastic network interface, not an Amazon EC2 instance." Every
/// other mode (`bridge`, [`NETWORK_MODE_HOST`], [`NETWORK_MODE_NONE`]) leaves the task
/// on the container instance's network interface, so the load balancer reaches it at
/// that instance — the `instance` target type.
pub fn required_target_type(network_mode: &str) -> &'static str {
    if network_mode == NETWORK_MODE_AWSVPC {
        TARGET_TYPE_IP
    } else {
        TARGET_TYPE_INSTANCE
    }
}

/// Every ELBv2 target group a `loadBalancers` list names, in request order: each entry's
/// `targetGroupArn` plus the `alternateTargetGroupArn` of its `advancedConfiguration`,
/// which a blue/green deployment registers the SAME tasks into and which is therefore
/// bound by the same rule.
///
/// The list is caller-supplied, so the duplicate check is a hash-set lookup rather than a
/// scan — the walk is linear in the list, and the list itself is bounded by
/// [`MAX_LOAD_BALANCERS`] before a handler calls this.
pub fn requested_target_group_arns(load_balancers: &[LoadBalancer]) -> Vec<String> {
    let mut arns = Vec::new();
    let mut seen = HashSet::new();
    for entry in load_balancers {
        for arn in [
            entry.target_group_arn.as_deref(),
            entry
                .advanced_configuration
                .as_ref()
                .and_then(|advanced| advanced.alternate_target_group_arn.as_deref()),
        ]
        .into_iter()
        .flatten()
        {
            if seen.insert(arn.to_string()) {
                arns.push(arn.to_string());
            }
        }
    }
    arns
}

/// The `TargetType` of each ELBv2 target group in `arns`, read from the Elastic Load
/// Balancing v2 service's OWN store through the one shared engine — so a target group
/// created over that API is seen here with no propagation step and no second copy of it
/// kept in this service's state. An ARN that names no stored target group is absent from
/// the answer.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own
/// store being unreachable.
///
/// LOCK ORDER: this takes the ELBv2 store's lock and holds no other while it does, and
/// it hands back owned values rather than a guard — so it is called with NO guard of
/// this service's store held. Keeping the cross-service read one-directional and
/// guard-free is what makes a lock cycle between the two stores unreachable. The map it
/// allocates is bounded by the request's own `loadBalancers` list.
pub fn target_group_target_types<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arns: &[String],
) -> Option<HashMap<String, String>> {
    let state =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        arns.iter()
            .filter_map(|arn| {
                store
                    .target_groups
                    .get(arn)
                    .map(|group| (arn.clone(), group.target_type.clone()))
            })
            .collect(),
    )
}

/// The refusal a `loadBalancers` list a request NAMES earns, or `None` when every target
/// group it names exists and can carry the service's tasks.
///
/// `target_types` is what [`target_group_target_types`] read for
/// [`requested_target_group_arns`] of the same list: an ARN it does not hold names no
/// target group at all, and one it does is checked by [`target_group_type_refusal`]. A
/// service with no resolved task definition (a deployment controller that takes its tasks
/// from a task set) has no network mode to check against, so only the existence of the
/// target group is.
pub fn load_balancer_refusal(
    load_balancers: &[LoadBalancer],
    network_mode: Option<&str>,
    target_types: &HashMap<String, String>,
) -> Option<String> {
    let arns = requested_target_group_arns(load_balancers);
    for arn in &arns {
        if !target_types.contains_key(arn) {
            return Some(format!(
                "The target group with targetGroupArn {arn} does not exist"
            ));
        }
    }
    network_mode.and_then(|mode| target_group_type_refusal(&arns, mode, target_types))
}

/// The compatibility half of [`load_balancer_refusal`]: the refusal the target groups in
/// `arns` earn against a revision whose `networkMode` is `network_mode`, over those the
/// ELBv2 store still holds. `None` when every one of them carries the target type
/// [`required_target_type`] names for that mode.
pub fn target_group_type_refusal(
    arns: &[String],
    network_mode: &str,
    target_types: &HashMap<String, String>,
) -> Option<String> {
    let required = required_target_type(network_mode);
    for arn in arns {
        let Some(target_type) = target_types.get(arn) else {
            continue;
        };
        if target_type != required {
            // The form Amazon ECS words this refusal in, composed from the request and the
            // resolved record: the target group the entry named, the target type that group
            // has, the network mode the revision declares, and the target type that mode
            // registers its tasks as — the actionable half, since the caller has to create
            // or pick another group to get past it.
            return Some(format!(
                "The provided target group {arn} has target type {target_type}, which is \
                 incompatible with the {network_mode} network mode specified in the task \
                 definition. Use a target group whose target type is {required}."
            ));
        }
    }
    None
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
