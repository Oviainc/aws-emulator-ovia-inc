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

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Attachment, Attribute, CapacityProviderStrategyItem, Cluster, ClusterConfiguration,
    ClusterServiceConnectDefaults, ClusterSetting, Container, ContainerDefinition,
    ContainerInstance, Deployment, DeploymentCircuitBreaker, DeploymentConfiguration,
    DeploymentController, EphemeralStorage, Failure, KeyValuePair, LoadBalancer,
    NetworkConfiguration, PlacementConstraint, PlacementStrategy, PlatformDevice, PortMapping,
    ProxyConfiguration, Resource, RuntimePlatform, Service, ServiceConnectConfiguration,
    ServiceCurrentRevisionSummary, ServiceEvent, ServiceRegistry, ServiceVolumeConfiguration,
    Setting, Tag, Task, TaskDefinition, TaskDefinitionPlacementConstraint, TaskOverride,
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
    let static_mapping = network_mode == NETWORK_MODE_AWSVPC || network_mode == NETWORK_MODE_HOST;
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
/// Only the port-mapping defaults ([`defaulted_port_mapping`]) are rendered here;
/// nothing else is invented for a member the caller left out.
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
            // "The principal that registered the task definition", as it was resolved
            // when the registration ran.
            registered_by: Some(self.registered_by.clone()),
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
            // that created it, as stored — the same principal
            // `TaskDefinition$registeredBy` names.
            created_by: Some(service.created_by.clone()),
            enable_ecs_managed_tags: service.enable_ecs_managed_tags,
            propagate_tags: Some(service.propagate_tags.clone()),
            enable_execute_command: service.enable_execute_command,
            availability_zone_rebalancing: Some(service.availability_zone_rebalancing.clone()),
            resource_management_type: Some(RESOURCE_MANAGEMENT_CUSTOMER.to_string()),
        }
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
