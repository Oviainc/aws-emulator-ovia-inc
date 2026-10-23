// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codedeploy-codedeploy-20141006 service.
//!
//! Every record a handler answers from lives here: applications (with their
//! deployment groups), deployments, and the deployment configurations. Handlers
//! reach it through [`with_state`], which is also where the two things that must
//! happen before ANY request is served happen: seeding the `CodeDeployDefault.*`
//! built-in deployment configurations once, and materialising the deployment
//! transitions the clock already owes.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AlarmConfiguration, AutoRollbackConfiguration, BlueGreenDeploymentConfiguration,
    DeploymentStyle, EC2DeploymentOptions, EC2TagFilter, EC2TagSet, ECSService, LoadBalancerInfo,
    MinimumHealthyHosts, OnPremisesTagSet, RevisionLocation, RollbackInfo, Tag, TagFilter,
    TargetInstances, TrafficRoutingConfig, TriggerConfig, ZonalConfig,
};

/// How long after `CreateDeployment` the emulator owes the deployment its start,
/// and then its terminal status.
///
/// CodeDeploy documents no duration for either step, so both spans are CHOSEN,
/// each inside the emulator-wide 250–750 ms window for a clock-owed transition.
/// Neither is zero, so a caller that polls straight after `CreateDeployment`
/// still observes the non-terminal statuses the real service reports.
const DEPLOYMENT_START_MILLIS: i64 = 250;
/// See [`DEPLOYMENT_START_MILLIS`]: 250 ms after the deployment starts.
const DEPLOYMENT_COMPLETION_MILLIS: i64 = 500;

/// The statuses a deployment the emulator created moves through. All three are
/// members of the model's `DeploymentStatus` enum
/// (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
/// InProgress | Baking | Succeeded | Failed | Stopped | Ready).
pub const DEPLOYMENT_STATUS_CREATED: &str = "Created";
/// See [`DEPLOYMENT_STATUS_CREATED`].
pub const DEPLOYMENT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`DEPLOYMENT_STATUS_CREATED`].
pub const DEPLOYMENT_STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`DEPLOYMENT_STATUS_CREATED`]. The terminal status of a deployment whose
/// deployment group resolved no target to deploy to (see
/// [`deployment_target_count`]), and the one a deployment reaches when something
/// that is part of it failed — a lifecycle event hook reporting a failed
/// validation, say.
pub const DEPLOYMENT_STATUS_FAILED: &str = "Failed";
/// See [`DEPLOYMENT_STATUS_CREATED`]: the status a deployment a caller stopped
/// holds.
pub const DEPLOYMENT_STATUS_STOPPED: &str = "Stopped";

/// The two `LifecycleEventStatus` values a validation function may report
/// (`smithy_query(query="shape",shape="LifecycleEventStatus")` carries six:
/// Pending | InProgress | Succeeded | Failed | Skipped | Unknown — and
/// `PutLifecycleEventHookExecutionStatus`'s own documentation says "only
/// Succeeded and Failed can be passed successfully in your API call").
pub const LIFECYCLE_EVENT_STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`LIFECYCLE_EVENT_STATUS_SUCCEEDED`].
pub const LIFECYCLE_EVENT_STATUS_FAILED: &str = "Failed";

/// The `ErrorCode` CodeDeploy reports on a deployment a lifecycle event hook
/// failed (`smithy_query(query="shape",shape="ErrorCode")`).
pub const ERROR_CODE_HOOK_EXECUTION_FAILURE: &str = "HOOK_EXECUTION_FAILURE";

/// Whether `status` is one a deployment never leaves.
///
/// The model's `DeploymentStatus` enum
/// (`smithy_query(query="shape",shape="DeploymentStatus")`) is Created | Queued |
/// InProgress | Baking | Succeeded | Failed | Stopped | Ready; the last three of
/// those are outcomes, and the rest are stages a deployment is still working
/// through. An operation that acts on a deployment in flight — stopping it,
/// continuing it — is what the modelled `DeploymentAlreadyCompletedException`
/// refuses once the deployment holds one of the outcomes.
pub fn deployment_is_terminal(status: &str) -> bool {
    matches!(
        status,
        DEPLOYMENT_STATUS_SUCCEEDED | DEPLOYMENT_STATUS_FAILED | DEPLOYMENT_STATUS_STOPPED
    )
}

/// The `ErrorCode` a deployment that found nothing to deploy to carries
/// (`smithy_query(query="shape",shape="ErrorCode")` lists it; the AWS CodeDeploy
/// User Guide's "Error codes" documents `NO_INSTANCES` as "no instances were
/// found in your deployment group").
pub const ERROR_CODE_NO_INSTANCES: &str = "NO_INSTANCES";

/// The characters a minted deployment-id suffix is drawn from, and how many of
/// them one carries: CodeDeploy's `d-XXXXXXXXX` deployment ids.
const DEPLOYMENT_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
/// See [`DEPLOYMENT_ID_ALPHABET`].
const DEPLOYMENT_ID_SUFFIX_LEN: usize = 9;
/// See [`DEPLOYMENT_ID_ALPHABET`].
const DEPLOYMENT_ID_PREFIX: &str = "d-";

/// What a handler answers when the engine it was constructed with is not the
/// shared in-memory store.
///
/// Unreachable through the router, which mounts every service on the one shared
/// engine. CodeDeploy's model declares no server-fault error, so this is stated
/// as a refusal rather than invented as a 5xx — and NEVER by building a private
/// engine, which would silently drop the write.
pub const STATE_UNAVAILABLE: &str =
    "CodeDeploy state is unavailable: this service was mounted on an engine that does not \
     provide the shared in-memory store";

/// The `DeploymentCreator` value for a deployment a client asked for, as opposed
/// to one an auto-scaling event or a rollback created
/// (`smithy_query(query="shape",shape="DeploymentCreator")`).
pub const DEPLOYMENT_CREATOR_USER: &str = "user";

/// The `DeploymentType` of a blue/green deployment
/// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE | BLUE_GREEN).
pub const DEPLOYMENT_TYPE_BLUE_GREEN: &str = "BLUE_GREEN";

/// The `InstanceAction` that terminates the original environment's instances
/// after a successful blue/green deployment
/// (`smithy_query(query="shape",shape="InstanceAction")`: TERMINATE |
/// KEEP_ALIVE).
pub const INSTANCE_ACTION_TERMINATE: &str = "TERMINATE";

/// The largest `terminationWaitTimeInMinutes` a deployment group may ask for, as
/// the model documents for the member
/// (`smithy_query(query="doc",shape="BlueInstanceTerminationOption",member="terminationWaitTimeInMinutes")`:
/// "The maximum setting is 2880 minutes (2 days)").
pub const MAX_TERMINATION_WAIT_MINUTES: i32 = 2880;

/// The `DeploymentCreator` value for the deployment CodeDeploy itself creates to
/// roll a deployment back
/// (`smithy_query(query="shape",shape="DeploymentCreator")`).
pub const DEPLOYMENT_CREATOR_ROLLBACK: &str = "codeDeployRollback";

/// The compute platform an application gets when `CreateApplication` omits one:
/// CodeDeploy's own default (`smithy_query(query="shape",shape="ComputePlatform")`
/// lists Server | Lambda | ECS).
pub const DEFAULT_COMPUTE_PLATFORM: &str = "Server";

/// The quotas this emulator enforces, with the modelled error each refusal
/// answers.
///
/// AWS publishes the first two in "Quotas in AWS CodeDeploy" (applications per
/// account per Region; deployment groups per application), so they keep their
/// documented values. Enforcing them is also what keeps the store from growing
/// without limit on repeated requests.
pub const MAX_APPLICATIONS: usize = 1_000;
/// See [`MAX_APPLICATIONS`].
pub const MAX_DEPLOYMENT_GROUPS_PER_APPLICATION: usize = 1_000;

/// How many deployments of history the store keeps.
///
/// CodeDeploy publishes no quota for retained deployment history and does not
/// refuse a deployment because its history is full, so this bound is CHOSEN and
/// is applied as RETENTION: creating a deployment past it drops the oldest
/// deployments, oldest first, so a caller cannot grow the store without limit.
pub const MAX_DEPLOYMENT_HISTORY: usize = 500;

/// How many alarms one alarm configuration may carry.
///
/// The model states it on `AlarmConfiguration$alarms`
/// (`smithy_query(query="shape",shape="AlarmConfiguration")`): "A list of alarms
/// configured for the deployment or deployment group. A maximum of 10 alarms can
/// be added." Past it, the request is refused with
/// `AlarmsLimitExceededException`.
pub const MAX_ALARMS_PER_ALARM_CONFIGURATION: usize = 10;

/// The longest alarm name an alarm configuration may carry, from the model's
/// `Alarm$name` (`smithy_query(query="shape",shape="Alarm")`): "Maximum length
/// is 255 characters. Each alarm name can be used only once in a list of
/// alarms."
pub const MAX_ALARM_NAME_LENGTH: usize = 255;

/// Why an alarm configuration cannot be accepted, as
/// [`validate_alarm_configuration`] answers it.
///
/// Two distinct refusals, because CodeDeploy models two: too many alarms is
/// `AlarmsLimitExceededException`, anything else about the configuration's form
/// is `InvalidAlarmConfigException`. Each caller maps these onto its OWN
/// operation's error enum.
#[derive(Debug)]
pub enum AlarmConfigurationRejection {
    /// More than [`MAX_ALARMS_PER_ALARM_CONFIGURATION`] alarms were specified;
    /// carries how many the request asked for.
    TooManyAlarms(usize),
    /// Malformed in one of the ways `InvalidAlarmConfigException` documents; the
    /// string says which, so the caller can fix the request from the message.
    Malformed(String),
}

/// Check an alarm configuration against the rules the model and the two alarm
/// errors state, so every operation that accepts one refuses the same requests.
///
/// `InvalidAlarmConfigException` documents the malformed cases: "The alarm name
/// is empty or null or exceeds the limit of 255 characters. Two alarms with the
/// same name have been specified. The alarm configuration is enabled, but the
/// alarm list is empty." A DISABLED configuration with no alarms is accepted —
/// that is how a caller turns alarm monitoring off.
pub fn validate_alarm_configuration(
    configuration: &AlarmConfiguration,
) -> Result<(), AlarmConfigurationRejection> {
    let alarms = configuration.alarms.as_deref().unwrap_or_default();
    if alarms.len() > MAX_ALARMS_PER_ALARM_CONFIGURATION {
        return Err(AlarmConfigurationRejection::TooManyAlarms(alarms.len()));
    }
    if configuration.enabled && alarms.is_empty() {
        return Err(AlarmConfigurationRejection::Malformed(
            "the alarm configuration is enabled but its alarm list is empty".to_string(),
        ));
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for alarm in alarms {
        let name = alarm.name.as_deref().unwrap_or_default();
        if name.is_empty() {
            return Err(AlarmConfigurationRejection::Malformed(
                "an alarm was specified with no name".to_string(),
            ));
        }
        if name.chars().count() > MAX_ALARM_NAME_LENGTH {
            return Err(AlarmConfigurationRejection::Malformed(format!(
                "an alarm name exceeds the maximum length of {MAX_ALARM_NAME_LENGTH} characters"
            )));
        }
        if !seen.insert(name) {
            return Err(AlarmConfigurationRejection::Malformed(format!(
                "the alarm name {name} was specified more than once"
            )));
        }
    }
    Ok(())
}

/// How many tag groups one `ec2TagSet` / `onPremisesTagSet` may hold, the bound
/// `TagSetListLimitExceededException` answers.
///
/// The model states the figure: that error is "The number of tag groups included
/// in the tag set list exceeded the maximum allowed limit of 3"
/// (`smithy_query(query="shape",shape="TagSetListLimitExceededException")`).
pub const MAX_TAG_SET_GROUPS: usize = 3;

/// How many tag filters ONE selector list of a deployment group may hold —
/// `ec2TagFilters`, `onPremisesInstanceTagFilters`, or one group of either tag
/// set.
///
/// CodeDeploy states the figure for a tag type in
/// `InvalidTargetInstancesException` ("The limit of 10 tags for a tag type was
/// exceeded"). Enforcing it is also what bounds the work of resolving a
/// deployment's targets: every filter a group holds is matched against the tags
/// of every registered on-premises instance, so an unbounded filter list would
/// be unbounded work on each `CreateDeployment`.
pub const MAX_TAG_FILTERS_PER_LIST: usize = 10;

/// How many application revisions one application's registry keeps.
///
/// CodeDeploy publishes no quota for retained revisions and does not refuse a
/// registration because the registry is full, so — like
/// [`MAX_DEPLOYMENT_HISTORY`] — this bound is CHOSEN and is applied as
/// RETENTION: registering past it drops the oldest-registered revisions first,
/// so a caller cannot grow the store without limit.
pub const MAX_REVISIONS_PER_APPLICATION: usize = 500;

/// How many on-premises instance names one call may name, the bound
/// `InstanceLimitExceededException` ("The maximum number of allowed on-premises
/// instances in a single call was exceeded") answers.
///
/// CodeDeploy's API reference states a 100-name limit for the sibling batch
/// call over the same resource (`BatchGetOnPremisesInstances`), so the same 100
/// is applied here; for THIS call the number is CHOSEN to agree with that batch
/// limit, and enforcing it keeps one call's work bounded.
pub const MAX_INSTANCE_NAMES_PER_CALL: usize = 100;

/// How many tags one on-premises instance may carry, the bound
/// `TagLimitExceededException` answers.
///
/// CodeDeploy publishes no per-instance tag count this emulator can cite, so
/// this bound is CHOSEN at the 50 tags per resource AWS's own tagging guidance
/// uses; enforcing it is also what keeps a stored instance from growing without
/// limit on repeated `AddTagsToOnPremisesInstances` calls.
pub const MAX_TAGS_PER_ON_PREMISES_INSTANCE: usize = 50;

/// A tag's key and value, or `None` when the tag is not one CodeDeploy accepts.
///
/// The model states the rule for the tags of an on-premises instance: "Keys and
/// values are both required. Keys cannot be null or empty strings. Value-only
/// tags are not allowed."
/// (`smithy_query(query="doc",shape="AddTagsToOnPremisesInstancesInput",member="tags")`),
/// so a tag missing either member, or carrying an empty key, is refused with the
/// operation's `InvalidTagException`. An empty VALUE is accepted: only keys are
/// documented as unable to be empty.
pub fn tag_key_value(tag: &Tag) -> Option<(&str, &str)> {
    let key = tag.key.as_deref()?;
    let value = tag.value.as_deref()?;
    (!key.is_empty()).then_some((key, value))
}

/// How many on-premises instance records the store keeps.
///
/// CodeDeploy publishes no quota for registered on-premises instances, so this
/// bound is CHOSEN, and it is what keeps a loop of registrations from growing
/// the store without limit. It is applied first as RETENTION — a record that is
/// no longer REGISTERED is dropped, oldest first, to make room for a new
/// registration — and only a registry whose every record is still registered
/// refuses a further registration, which is the one case no eviction can serve.
pub const MAX_ON_PREMISES_INSTANCES: usize = 1_000;

/// A registered application, with the deployment groups and the application
/// revisions that belong to it.
#[derive(Debug, Clone)]
pub struct Application {
    pub application_id: String,
    pub application_name: String,
    pub compute_platform: String,
    pub create_time_ms: i64,
    pub linked_to_git_hub: bool,
    pub git_hub_account_name: Option<String>,
    pub tags: Vec<Tag>,
    pub deployment_groups: HashMap<String, DeploymentGroup>,
    /// Creation order of this application's deployment group names, so a listing
    /// of them is deterministic and pages stably.
    pub deployment_group_order: Vec<String>,
    /// The revisions CodeDeploy knows for this application, by
    /// [`revision_key`]. CodeDeploy tracks revisions by REGISTRATION — it never
    /// probes the location — so a revision is here only because
    /// `RegisterApplicationRevision` registered it or a deployment used it.
    pub revisions: HashMap<String, ApplicationRevision>,
    /// Registration order of this application's revision keys, so a listing is
    /// deterministic and pages stably.
    pub revision_order: Vec<String>,
}

/// One revision of an application, as CodeDeploy's `GenericRevisionInfo`
/// reports it.
#[derive(Debug, Clone)]
pub struct ApplicationRevision {
    /// The location exactly as the caller supplied it; nothing about a revision
    /// is inferred into the stored record.
    pub revision: RevisionLocation,
    pub description: Option<String>,
    pub register_time_ms: i64,
    /// When a deployment first used this revision, and when it last did —
    /// `None` for a revision that was registered but never deployed.
    pub first_used_time_ms: Option<i64>,
    /// See [`ApplicationRevision::first_used_time_ms`].
    pub last_used_time_ms: Option<i64>,
}

impl Application {
    /// Register `revision` with this application, as
    /// `RegisterApplicationRevision` does.
    ///
    /// `registerTime` is when the revision first became known to the
    /// application and is not moved by a later registration of the same
    /// revision — a second registration only carries a new `description`, which
    /// is the member it exists to set.
    pub fn register_revision(
        &mut self,
        key: String,
        revision: RevisionLocation,
        description: Option<String>,
        now_ms: i64,
    ) {
        let entry = self.revision_entry(key, revision, now_ms);
        if description.is_some() {
            entry.description = description;
        }
    }

    /// Record that a deployment used `revision`, which is also how CodeDeploy
    /// learns a revision a deployment named but nobody registered.
    pub fn record_revision_use(
        &mut self,
        key: String,
        revision: RevisionLocation,
        now_ms: i64,
    ) {
        let entry = self.revision_entry(key, revision, now_ms);
        entry.first_used_time_ms = entry.first_used_time_ms.or(Some(now_ms));
        entry.last_used_time_ms = Some(now_ms);
    }

    /// This application's entry for `key`, inserted as newly registered when the
    /// registry does not hold it yet, dropping the oldest-registered revisions
    /// past [`MAX_REVISIONS_PER_APPLICATION`].
    fn revision_entry(
        &mut self,
        key: String,
        revision: RevisionLocation,
        now_ms: i64,
    ) -> &mut ApplicationRevision {
        if !self.revisions.contains_key(&key) {
            self.revisions.insert(
                key.clone(),
                ApplicationRevision {
                    revision,
                    description: None,
                    register_time_ms: now_ms,
                    first_used_time_ms: None,
                    last_used_time_ms: None,
                },
            );
            self.revision_order.push(key.clone());
            if let Some(excess) = self
                .revision_order
                .len()
                .checked_sub(MAX_REVISIONS_PER_APPLICATION)
            {
                for oldest in self.revision_order.drain(..excess).collect::<Vec<_>>() {
                    self.revisions.remove(&oldest);
                }
            }
        }
        self.revisions
            .get_mut(&key)
            .expect("the entry was just inserted")
    }
}

/// A deployment group: every member `CreateDeploymentGroup` accepts, so the
/// group can be read back without losing any of them.
#[derive(Debug, Clone)]
pub struct DeploymentGroup {
    pub deployment_group_id: String,
    pub deployment_group_name: String,
    pub application_name: String,
    pub service_role_arn: String,
    pub deployment_config_name: String,
    pub compute_platform: String,
    pub ec2_tag_filters: Vec<EC2TagFilter>,
    pub on_premises_instance_tag_filters: Vec<TagFilter>,
    pub auto_scaling_groups: Vec<String>,
    pub trigger_configurations: Vec<TriggerConfig>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    pub auto_rollback_configuration: Option<AutoRollbackConfiguration>,
    pub outdated_instances_strategy: Option<String>,
    pub deployment_style: Option<DeploymentStyle>,
    pub blue_green_deployment_configuration: Option<BlueGreenDeploymentConfiguration>,
    pub load_balancer_info: Option<LoadBalancerInfo>,
    pub ec2_tag_set: Option<EC2TagSet>,
    pub on_premises_tag_set: Option<OnPremisesTagSet>,
    pub ecs_services: Vec<ECSService>,
    pub ec2_deployment_options: Option<EC2DeploymentOptions>,
    pub termination_hook_enabled: bool,
    pub tags: Vec<Tag>,
    pub create_time_ms: i64,
}

/// A deployment. `revision` is held exactly as the request supplied it: nothing
/// about a revision is inferred into the stored record, so reading a deployment
/// back returns the caller's own revision.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_id: String,
    pub application_name: String,
    pub deployment_group_name: Option<String>,
    /// The EFFECTIVE configuration: the request's, else the group's.
    pub deployment_config_name: String,
    pub revision: Option<RevisionLocation>,
    /// The identity of `revision` ([`revision_key`]), computed ONCE when the
    /// deployment was recorded. Deriving a group's target revision reads this
    /// rather than re-deriving it, so no later request re-hashes an inline
    /// revision's content.
    pub revision_key: Option<String>,
    pub previous_revision: Option<RevisionLocation>,
    pub status: String,
    pub creator: String,
    pub description: Option<String>,
    pub ignore_application_stop_failures: bool,
    pub update_outdated_instances_only: bool,
    pub file_exists_behavior: Option<String>,
    pub deployment_mode: Option<String>,
    pub auto_rollback_configuration: Option<AutoRollbackConfiguration>,
    pub target_instances: Option<TargetInstances>,
    pub override_alarm_configuration: Option<AlarmConfiguration>,
    pub ec2_deployment_options: Option<EC2DeploymentOptions>,
    pub deployment_style: Option<DeploymentStyle>,
    pub compute_platform: String,
    pub external_id: Option<String>,
    /// The rollback this deployment was given, once a caller stopped it asking
    /// for one: the rollback deployment's id when one was created, and the
    /// reason when there was nothing to roll back to.
    pub rollback_info: Option<RollbackInfo>,
    pub create_time_ms: i64,
    pub start_time_ms: Option<i64>,
    pub complete_time_ms: Option<i64>,
    /// The instants the clock owes this deployment its start and its terminal
    /// status; see [`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`].
    pub starts_at_ms: i64,
    /// See [`Deployment::starts_at_ms`].
    pub completes_at_ms: i64,
    /// Why this deployment failed, when it did, as CodeDeploy reports it on
    /// `DeploymentInfo.errorInformation`.
    pub error_information: Option<crate::ops::types::ErrorInformation>,
    /// The results reported for this deployment's lifecycle event hooks, in the
    /// order they were reported.
    pub lifecycle_hook_executions: Vec<LifecycleEventHookExecution>,
    /// How many deployment targets the deployment group resolved for this
    /// deployment, which is what decides its terminal status; see
    /// [`deployment_target_count`] and [`terminal_status`].
    pub target_count: usize,
    /// Whether this deployment terminates the instances of the original (blue)
    /// environment after traffic routing: true only for a blue/green deployment
    /// whose group chose the `TERMINATE` action, which is also exactly when the
    /// model says `instanceTerminationWaitTimeStarted` is reported
    /// (`smithy_query(query="doc",shape="DeploymentInfo",member="instanceTerminationWaitTimeStarted")`:
    /// "'false' if the KEEP_ALIVE option is specified").
    pub terminates_original_instances: bool,
    /// How much of the group's `terminationWaitTimeInMinutes` this deployment
    /// still owes before it terminates those instances and completes.
    ///
    /// Subtracting it from [`Deployment::completes_at_ms`] gives the instant
    /// traffic routing is complete, which is when the wait starts;
    /// `SkipWaitTimeForInstanceTermination` sets it to zero, which is what
    /// "overrides any specified wait time" means.
    pub termination_wait_millis: i64,
    /// Whether the termination wait has started, materialised by the clock; see
    /// [`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`].
    pub instance_termination_wait_time_started: bool,
}

/// The result a validation function reported for one of a deployment's
/// lifecycle event hooks, through `PutLifecycleEventHookExecutionStatus`.
///
/// The execution id is the hook execution's IDENTITY: CodeDeploy hands it to the
/// validation function in the lifecycle event, and the function reports back
/// under it, so a second report under the same id is a second report of the same
/// lifecycle event.
#[derive(Debug, Clone)]
pub struct LifecycleEventHookExecution {
    pub lifecycle_event_hook_execution_id: String,
    /// Which of the deployment's lifecycle hooks this execution reported for, in
    /// the order its compute platform runs them.
    pub lifecycle_event_name: String,
    /// A `LifecycleEventStatus`: see [`LIFECYCLE_EVENT_STATUS_SUCCEEDED`].
    pub status: String,
    pub update_time_ms: i64,
}

impl Deployment {
    /// The instant this deployment's traffic routing is complete, which is when
    /// a termination wait starts and from which it is measured.
    pub fn traffic_routing_complete_at_ms(&self) -> i64 {
        self.completes_at_ms - self.termination_wait_millis
    }

    /// Whether this deployment has reached a terminal status, by the one
    /// definition of terminal this service has ([`deployment_is_terminal`]).
    pub fn is_complete(&self) -> bool {
        deployment_is_terminal(&self.status)
    }

    /// Whether this deployment is a blue/green one, which is what
    /// `SkipWaitTimeForInstanceTermination` and `ContinueDeployment` act on
    /// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE |
    /// BLUE_GREEN).
    pub fn is_blue_green(&self) -> bool {
        self.deployment_style
            .as_ref()
            .and_then(|style| style.deployment_type.as_deref())
            == Some(DEPLOYMENT_TYPE_BLUE_GREEN)
    }
}

/// A deployment configuration — one of the `CodeDeployDefault.*` built-ins, or
/// one a client created.
#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub deployment_config_id: String,
    pub deployment_config_name: String,
    pub minimum_healthy_hosts: Option<MinimumHealthyHosts>,
    pub traffic_routing_config: Option<TrafficRoutingConfig>,
    pub zonal_config: Option<ZonalConfig>,
    pub compute_platform: String,
    pub create_time_ms: i64,
}

/// A registered on-premises instance.
///
/// The record OUTLIVES its registration: CodeDeploy keeps a deregistered
/// instance, with the instant it was deregistered, which is what lets
/// `ListOnPremisesInstances` answer the `Deregistered` registration status and
/// `GetOnPremisesInstance` report a `deregisterTime`.
#[derive(Debug, Clone)]
pub struct OnPremisesInstance {
    pub instance_name: String,
    pub instance_arn: String,
    /// The ONE IAM ARN the registration was made with: an instance carries an
    /// IAM user ARN or an IAM session ARN, never both.
    pub iam_user_arn: Option<String>,
    /// See [`OnPremisesInstance::iam_user_arn`].
    pub iam_session_arn: Option<String>,
    pub register_time_ms: i64,
    /// `None` while the instance is registered; the instant it was deregistered
    /// once it is not.
    pub deregister_time_ms: Option<i64>,
    pub tags: Vec<Tag>,
}

impl OnPremisesInstance {
    /// Whether this instance is currently registered, as the model's
    /// `RegistrationStatus` (`smithy_query(query="shape",shape="RegistrationStatus")`:
    /// Registered | Deregistered) distinguishes the two.
    pub fn is_registered(&self) -> bool {
        self.deregister_time_ms.is_none()
    }

    /// How many tags this instance would carry once `tags` were added: a tag
    /// REPLACES the value of a key the instance already carries rather than
    /// adding a second entry for it, so only genuinely new keys count.
    ///
    /// Set-keyed rather than list-scanned, so the cost is linear in the number
    /// of keys rather than quadratic in it; both sides are bounded anyway
    /// ([`MAX_TAGS_PER_ON_PREMISES_INSTANCE`] stored, and the same bound on a
    /// request's tag list, which a tagging request is refused against BEFORE
    /// this runs).
    pub fn projected_tag_count(&self, tags: &[(&str, &str)]) -> usize {
        let known: HashSet<&str> = self.tags.iter().filter_map(|tag| tag.key.as_deref()).collect();
        let added: HashSet<&str> = tags
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| !known.contains(key))
            .collect();
        self.tags.len() + added.len()
    }

    /// Set `key` to `value`, replacing the value of a key already present so an
    /// instance never carries two entries for one key.
    pub fn set_tag(&mut self, key: &str, value: &str) {
        match self
            .tags
            .iter_mut()
            .find(|tag| tag.key.as_deref() == Some(key))
        {
            Some(existing) => existing.value = Some(value.to_string()),
            None => self.tags.push(Tag {
                key: Some(key.to_string()),
                value: Some(value.to_string()),
            }),
        }
    }
}

/// Per-service state for com-amazonaws-codedeploy-codedeploy-20141006. A handler reaches it through [`with_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodedeployCodedeploy20141006State {
    /// Applications by `applicationName`.
    pub applications: HashMap<String, Application>,
    /// Creation order of application names, so `ListApplications` is
    /// deterministic and pages stably.
    pub application_order: Vec<String>,
    /// Deployments by `deploymentId`.
    pub deployments: HashMap<String, Deployment>,
    /// Creation order of deployment ids, so `ListDeployments` can answer
    /// newest-first deterministically and page stably.
    pub deployment_order: Vec<String>,
    /// The deployments the clock still owes a transition, creation-ordered. Only
    /// these are walked per request, so the cost of materialising what the clock
    /// owes does not grow with the deployment history.
    pending_deployments: Vec<String>,
    /// Deployment configurations by `deploymentConfigName`.
    pub deployment_configs: HashMap<String, DeploymentConfig>,
    /// The names of this account's stored connections to GitHub accounts, in the
    /// order they were linked, so `ListGitHubAccountTokenNames` is deterministic
    /// and pages stably.
    ///
    /// EMPTY on a fresh store, and that is the faithful answer: CodeDeploy's API
    /// models no operation that CREATES a GitHub account connection — the
    /// connection is made by the GitHub OAuth handshake the CodeDeploy console
    /// drives, which this emulator cannot perform — so an account that has run no
    /// console handshake has no stored connection names. The two modelled
    /// operations over this store only READ it (`ListGitHubAccountTokenNames`) and
    /// REMOVE from it (`DeleteGitHubAccountToken`).
    pub git_hub_account_token_names: Vec<String>,
    /// On-premises instances by `instanceName`, registered and deregistered
    /// alike — a deregistered instance keeps its record.
    pub on_premises_instances: HashMap<String, OnPremisesInstance>,
    /// Registration order of on-premises instance names, so
    /// `ListOnPremisesInstances` is deterministic and pages stably.
    pub on_premises_instance_order: Vec<String>,
    /// The name of the instance each IAM ARN currently backs, so the
    /// duplicate-IAM-ARN refusal is a LOOKUP rather than a scan of the whole
    /// registry under the store lock.
    ///
    /// It carries an entry only while that instance is REGISTERED, which
    /// [`ComAmazonawsCodedeployCodedeploy20141006State::register_on_premises_instance`]
    /// maintains; an operation that deregisters an instance drops its entry
    /// through [`ComAmazonawsCodedeployCodedeploy20141006State::release_iam_arns_of`].
    iam_arn_owners: HashMap<String, String>,
    /// Counter behind every minted identifier, so two ids from one process can
    /// never collide.
    pub next_id: u64,
    /// Whether the `CodeDeployDefault.*` built-ins have been seeded.
    seeded: bool,
}

/// One of the built-in deployment configurations CodeDeploy ships.
#[derive(Debug, Clone, Copy)]
struct BuiltinDeploymentConfig {
    name: &'static str,
    compute_platform: &'static str,
    /// `minimumHealthyHosts` as (type, value), for the EC2/on-premises built-ins.
    minimum_healthy_hosts: Option<(&'static str, i32)>,
    /// `trafficRoutingConfig.type`, for the Lambda and ECS built-ins.
    traffic_routing: Option<&'static str>,
}

/// The built-in deployment configurations CodeDeploy ships, with the
/// `minimumHealthyHosts` / `trafficRoutingConfig` the AWS CodeDeploy User Guide
/// documents for each ("Deployment configurations on an EC2/on-premises compute
/// platform" and "Deployment configurations on an AWS Lambda or Amazon ECS
/// compute platform").
const BUILTIN_DEPLOYMENT_CONFIGS: &[BuiltinDeploymentConfig] = &[
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.OneAtATime",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("FLEET_PERCENT", 99)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.HalfAtATime",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("FLEET_PERCENT", 50)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.AllAtOnce",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("HOST_COUNT", 0)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaAllAtOnce",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some("AllAtOnce"),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSAllAtOnce",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some("AllAtOnce"),
    },
];

impl ComAmazonawsCodedeployCodedeploy20141006State {
    /// Seed the built-in deployment configurations, once per store.
    fn ensure_seeded(&mut self, now_ms: i64) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        for builtin in BUILTIN_DEPLOYMENT_CONFIGS {
            let id = self.mint_uuid("deployment-config");
            self.deployment_configs.insert(
                builtin.name.to_string(),
                DeploymentConfig {
                    deployment_config_id: id,
                    deployment_config_name: builtin.name.to_string(),
                    minimum_healthy_hosts: builtin.minimum_healthy_hosts.map(|(kind, value)| {
                        MinimumHealthyHosts {
                            r#type: Some(kind.to_string()),
                            value,
                        }
                    }),
                    traffic_routing_config: builtin.traffic_routing.map(|kind| {
                        TrafficRoutingConfig {
                            r#type: Some(kind.to_string()),
                            ..Default::default()
                        }
                    }),
                    zonal_config: None,
                    compute_platform: builtin.compute_platform.to_string(),
                    create_time_ms: now_ms,
                },
            );
        }
    }

    /// Materialise every deployment transition that was already due at `now_ms`:
    /// `Created` -> `InProgress` once it was owed a start, then `InProgress` ->
    /// its terminal status ([`terminal_status`]) once it was owed completion.
    ///
    /// This is the one write a reporting operation performs: each transition was
    /// owed by the clock at this instant whether or not anybody called, so
    /// applying it here cannot show a caller anything a later caller would not
    /// also see.
    ///
    /// Only the deployments still awaiting a transition are walked, and each
    /// drops out of that set once it is terminal, so the per-request cost is
    /// bounded by how many deployments are in flight rather than by how many
    /// have ever been created.
    fn advance_clock(&mut self, now_ms: i64) {
        let deployments = &mut self.deployments;
        self.pending_deployments.retain(|id| {
            let Some(deployment) = deployments.get_mut(id) else {
                // Dropped from the history; there is nothing left to settle.
                return false;
            };
            // A deployment that already reached an outcome — it completed, or a
            // caller stopped it — is owed nothing further, so the clock never
            // moves it again.
            if deployment.complete_time_ms.is_some() {
                return false;
            }
            if deployment.start_time_ms.is_none() && deployment.starts_at_ms <= now_ms {
                deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
                deployment.start_time_ms = Some(deployment.starts_at_ms);
            }
            // A blue/green deployment that terminates the original instances
            // reports its termination wait as started once traffic routing is
            // complete, which the clock owes at the same instant whoever asks.
            if deployment.terminates_original_instances
                && deployment.traffic_routing_complete_at_ms() <= now_ms
            {
                deployment.instance_termination_wait_time_started = true;
            }
            if deployment.start_time_ms.is_some()
                && deployment.complete_time_ms.is_none()
                && deployment.completes_at_ms <= now_ms
            {
                deployment.status = terminal_status(deployment.target_count).to_string();
                deployment.complete_time_ms = Some(deployment.completes_at_ms);
            }
            deployment.complete_time_ms.is_none()
        });
    }

    /// Record `deployment` as this store's newest, dropping the oldest history
    /// past [`MAX_DEPLOYMENT_HISTORY`] so the store stays bounded however many
    /// deployments a caller creates.
    pub fn record_deployment(&mut self, deployment: Deployment) {
        let id = deployment.deployment_id.clone();
        self.deployments.insert(id.clone(), deployment);
        self.deployment_order.push(id.clone());
        self.pending_deployments.push(id);
        let Some(excess) = self
            .deployment_order
            .len()
            .checked_sub(MAX_DEPLOYMENT_HISTORY)
        else {
            return;
        };
        let dropped: Vec<String> = self.deployment_order.drain(..excess).collect();
        for oldest in dropped {
            self.deployments.remove(&oldest);
            self.pending_deployments.retain(|pending| pending != &oldest);
        }
    }

    /// Settle `deployment_id` in the terminal `status` at `now_ms`, with the
    /// error information CodeDeploy reports for it, and answer whether the
    /// deployment was there to settle.
    ///
    /// A deployment the clock had not yet started also gets the start it was
    /// owed, and drops out of the set the clock still owes a transition, so
    /// nothing can later overwrite the terminal status written here.
    pub fn settle_deployment(
        &mut self,
        deployment_id: &str,
        status: &str,
        error_information: Option<crate::ops::types::ErrorInformation>,
        now_ms: i64,
    ) -> bool {
        let Some(deployment) = self.deployments.get_mut(deployment_id) else {
            return false;
        };
        deployment.status = status.to_string();
        deployment.start_time_ms = Some(deployment.start_time_ms.unwrap_or(now_ms));
        deployment.complete_time_ms = Some(now_ms);
        deployment.error_information = error_information;
        self.pending_deployments
            .retain(|pending| pending != deployment_id);
        true
    }

    /// The instance `iam_arn` currently backs, if any.
    ///
    /// A LOOKUP, not a scan: the per-registration work does not grow with how
    /// many instances the store holds, so one caller's registrations cannot
    /// lengthen every other caller's time under the store lock.
    pub fn instance_registered_with(&self, iam_arn: &str) -> Option<&OnPremisesInstance> {
        self.iam_arn_owners
            .get(iam_arn)
            .and_then(|name| self.on_premises_instances.get(name))
            .filter(|instance| instance.is_registered())
    }

    /// Record `instance` as this store's newest registration, keeping the
    /// registry bounded by [`MAX_ON_PREMISES_INSTANCES`].
    ///
    /// `false` means the registry is full of instances that are all still
    /// REGISTERED, so nothing can be evicted to make room and the caller must
    /// refuse the request rather than grow the store. Re-registering a name the
    /// store already holds replaces that record and needs no room at all.
    pub fn register_on_premises_instance(&mut self, instance: OnPremisesInstance) -> bool {
        let name = instance.instance_name.clone();
        let replacing = self.on_premises_instances.contains_key(&name);
        if !replacing {
            self.evict_deregistered_instances(MAX_ON_PREMISES_INSTANCES - 1);
            if self.on_premises_instances.len() >= MAX_ON_PREMISES_INSTANCES {
                return false;
            }
            self.on_premises_instance_order.push(name.clone());
        }
        // The IAM ARN this name used to hold is free again; the new one is now
        // taken. Both are single-key operations on the index.
        self.release_iam_arns_of(&name);
        for arn in [&instance.iam_user_arn, &instance.iam_session_arn]
            .into_iter()
            .flatten()
        {
            self.iam_arn_owners.insert(arn.clone(), name.clone());
        }
        self.on_premises_instances.insert(name, instance);
        true
    }

    /// Free the IAM ARNs the instance named `name` holds, so they back no
    /// registered instance any more. Called when its registration ends — by a
    /// re-registration here, and by an operation that deregisters it.
    pub fn release_iam_arns_of(&mut self, name: &str) {
        let Some(instance) = self.on_premises_instances.get(name) else {
            return;
        };
        let held: Vec<String> = [&instance.iam_user_arn, &instance.iam_session_arn]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        for arn in held {
            if self.iam_arn_owners.get(&arn).is_some_and(|owner| owner == name) {
                self.iam_arn_owners.remove(&arn);
            }
        }
    }

    /// Drop on-premises instance records that are no longer REGISTERED, oldest
    /// first, until the registry holds at most `target` of them.
    ///
    /// Retention, not a reporting-time mutation: it only ever drops records a
    /// caller already deregistered, and it runs on the registering call that
    /// needs the room.
    fn evict_deregistered_instances(&mut self, target: usize) {
        if self.on_premises_instances.len() <= target {
            return;
        }
        let mut excess = self.on_premises_instances.len() - target;
        let instances = &mut self.on_premises_instances;
        let owners = &mut self.iam_arn_owners;
        self.on_premises_instance_order.retain(|name| {
            let Some(instance) = instances.get(name) else {
                // Not in the registry at all: the order list is stale here.
                return false;
            };
            if excess == 0 || instance.is_registered() {
                return true;
            }
            for arn in [&instance.iam_user_arn, &instance.iam_session_arn]
                .into_iter()
                .flatten()
            {
                owners.remove(arn);
            }
            instances.remove(name);
            excess -= 1;
            false
        });
    }

    /// Mint an identifier shaped like the UUID CodeDeploy answers with for an
    /// application, a deployment group or a deployment configuration.
    pub fn mint_uuid(&mut self, kind: &str) -> String {
        mint_uuid_from(&mut self.next_id, kind)
    }

    /// Record a connection to a GitHub account under `token_name`, answering
    /// whether it was added (a name the store already holds is left where it is,
    /// so the linking order stays stable and no name is stored twice).
    pub fn record_git_hub_account_token(&mut self, token_name: &str) -> bool {
        if self
            .git_hub_account_token_names
            .iter()
            .any(|stored| stored == token_name)
        {
            return false;
        }
        self.git_hub_account_token_names
            .push(token_name.to_string());
        true
    }

    /// Remove the connection to a GitHub account named `token_name`, answering
    /// whether the store held one.
    pub fn remove_git_hub_account_token(&mut self, token_name: &str) -> bool {
        let before = self.git_hub_account_token_names.len();
        self.git_hub_account_token_names
            .retain(|stored| stored != token_name);
        self.git_hub_account_token_names.len() != before
    }

    /// Mint a `d-XXXXXXXXX` deployment id: the `d-` prefix and nine characters
    /// from CodeDeploy's uppercase-alphanumeric deployment-id alphabet.
    pub fn mint_deployment_id(&mut self) -> String {
        let digest = self.digest("deployment");
        let suffix: String = digest
            .iter()
            .take(DEPLOYMENT_ID_SUFFIX_LEN)
            .map(|byte| {
                DEPLOYMENT_ID_ALPHABET[usize::from(*byte) % DEPLOYMENT_ID_ALPHABET.len()] as char
            })
            .collect();
        format!("{DEPLOYMENT_ID_PREFIX}{suffix}")
    }

    /// The bytes every minted id is cut from: this store's monotonic counter
    /// (unique within the process) folded with a per-process seed (so two
    /// processes do not mint the same ids), hashed so an id discloses neither.
    fn digest(&mut self, kind: &str) -> [u8; 32] {
        digest_from(&mut self.next_id, kind)
    }
}

/// [`ComAmazonawsCodedeployCodedeploy20141006State::mint_uuid`] over the id
/// counter alone, so a handler holding a record of this store borrowed can still
/// mint — which is what lets it mint only AFTER every refusal has been answered.
pub fn mint_uuid_from(counter: &mut u64, kind: &str) -> String {
    let hex = hex::encode(digest_from(counter, kind));
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// See [`ComAmazonawsCodedeployCodedeploy20141006State::digest`].
fn digest_from(counter: &mut u64, kind: &str) -> [u8; 32] {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    hasher.finalize().into()
}

/// A seed drawn once per process, so ids minted by two emulator processes from
/// the same counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The instant the clock owes a deployment created at `create_time_ms` its start.
pub fn deployment_start_due(create_time_ms: i64) -> i64 {
    create_time_ms + DEPLOYMENT_START_MILLIS
}

/// The instant the clock owes a deployment created at `create_time_ms` its
/// terminal status.
pub fn deployment_completion_due(create_time_ms: i64) -> i64 {
    create_time_ms + DEPLOYMENT_COMPLETION_MILLIS
}

/// What a deployment of `group` owes the original (blue) environment: whether it
/// terminates its instances at all, and how long it waits before doing so.
///
/// Only a blue/green deployment group has an original environment to terminate
/// (`DeploymentStyle.deploymentType`), and only its `TERMINATE` action
/// terminates it; the wait is the group's own
/// `terminationWaitTimeInMinutes` — a duration the caller asked for, so it keeps
/// its value — held to the range the model documents for that member
/// ([`MAX_TERMINATION_WAIT_MINUTES`]).
pub fn termination_wait(group: &DeploymentGroup) -> (bool, i64) {
    let blue_green = group
        .deployment_style
        .as_ref()
        .and_then(|style| style.deployment_type.as_deref())
        == Some(DEPLOYMENT_TYPE_BLUE_GREEN);
    let option = group
        .blue_green_deployment_configuration
        .as_ref()
        .and_then(|config| config.terminate_blue_instances_on_deployment_success.as_ref());
    let terminates = blue_green
        && option
            .and_then(|option| option.action.as_deref())
            .is_some_and(|action| action == INSTANCE_ACTION_TERMINATE);
    if !terminates {
        return (false, 0);
    }
    let minutes = option
        .map(|option| option.termination_wait_time_in_minutes)
        .unwrap_or_default()
        .clamp(0, MAX_TERMINATION_WAIT_MINUTES);
    (true, i64::from(minutes) * 60_000)
}

/// Epoch milliseconds as the modelled `timestamp` type.
pub fn timestamp(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
}

/// A modelled `timestamp` as epoch milliseconds, for comparing a request's time
/// range against a stored record. `None` when the value cannot be expressed in
/// milliseconds at all — answer that as a refusal rather than as a filter that
/// silently matches nothing.
pub fn timestamp_millis(value: &::aws_smithy_types::DateTime) -> Option<i64> {
    value.to_millis().ok()
}

/// The deployment configuration CodeDeploy applies when neither the request nor
/// the deployment group names one: the default built-in for the compute platform.
pub fn default_deployment_config_for(compute_platform: &str) -> &'static str {
    match compute_platform {
        "Lambda" => "CodeDeployDefault.LambdaAllAtOnce",
        "ECS" => "CodeDeployDefault.ECSAllAtOnce",
        _ => "CodeDeployDefault.OneAtATime",
    }
}

/// Whether `id` is shaped like a deployment id at all.
///
/// CodeDeploy mints deployment ids as `d-` plus nine characters of its
/// uppercase-alphanumeric alphabet (see
/// [`ComAmazonawsCodedeployCodedeploy20141006State::mint_deployment_id`]), and
/// the model puts no `@pattern` on `DeploymentId`
/// (`smithy_query(query="shape",shape="DeploymentId")`), so the format a request
/// must satisfy is the one the service itself issues. Anything else is not an id
/// of anything and is what `InvalidDeploymentIdException` describes: an
/// operation taking a `deploymentId` gates the FORMAT before it looks the
/// deployment up, so "that is not an id" and "no deployment has that id" stay
/// two different answers to a client that branches on them.
pub fn deployment_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(DEPLOYMENT_ID_PREFIX) else {
        return false;
    };
    suffix.len() == DEPLOYMENT_ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|byte| DEPLOYMENT_ID_ALPHABET.contains(&byte))
}

/// How much of a caller-supplied value a refusal message quotes back.
///
/// A refusal that names the value the caller sent is what lets them fix it, but
/// `DeploymentId` carries no modelled length, so the value is bounded here
/// rather than echoed whole.
const ECHOED_VALUE_CHARS: usize = 64;

/// A caller-supplied value as a refusal message quotes it: itself when short,
/// else its first [`ECHOED_VALUE_CHARS`] characters and an ellipsis.
pub fn echoed(value: &str) -> String {
    let mut shown: String = value.chars().take(ECHOED_VALUE_CHARS).collect();
    if shown.chars().count() < value.chars().count() {
        shown.push('…');
    }
    shown
}

/// The refusal message every operation answers `InvalidDeploymentIdException`
/// with, so the operations that gate the same format cannot word it differently.
/// It quotes the id the caller sent, bounded by [`echoed`].
pub fn malformed_deployment_id_message(deployment_id: &str) -> String {
    format!(
        "Specified DeploymentId is not in the valid format: {}",
        echoed(deployment_id)
    )
}

/// How many deployment targets `group` selects, as this emulator can resolve
/// them.
///
/// CodeDeploy resolves a deployment's targets from its deployment group, and a
/// deployment that resolves none runs to `Failed` rather than reporting success
/// for a revision that reached nothing (the AWS CodeDeploy User Guide's "Error
/// codes" gives that failure the [`ERROR_CODE_NO_INSTANCES`] code). What this
/// emulator can resolve, per compute platform
/// (`smithy_query(query="shape",shape="ComputePlatform")`):
///
/// * `ECS`: the group's `ecsServices` — one target per cluster/service pair.
/// * `Lambda`: a Lambda deployment has exactly one target, the function the
///   revision's AppSpec names — whatever the revision holds, there is one of
///   them — so a Lambda group never resolves none.
/// * `Server` (EC2/on-premises): the instances the group selects. The instances
///   this emulator HOLDS are the on-premises ones registered with it, so they
///   are resolved from that registry through the group's
///   `onPremisesInstanceTagFilters` and `onPremisesTagSet` (see
///   [`group_selects_instance`]). An `ec2TagFilter`, an `ec2TagSet` or an Auto
///   Scaling group names EC2 inventory, which CodeDeploy's state does not keep
///   and which this emulator reads from no other service, so those selectors
///   resolve nothing.
pub fn deployment_target_count(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    group: &DeploymentGroup,
) -> usize {
    match group.compute_platform.as_str() {
        "ECS" => group.ecs_services.len(),
        // Lambda: one target per deployment; see the note above.
        "Lambda" => 1,
        // Server (EC2/on-premises), which is what `computePlatform` holds here
        // when it is neither of the two above — the member is the modelled enum
        // Server | Lambda | ECS. A value outside that enum could not reach the
        // store, and would resolve nothing either: a platform this emulator
        // cannot target has no targets to report.
        _ => state
            .on_premises_instance_order
            .iter()
            .filter_map(|name| state.on_premises_instances.get(name))
            // A deregistered instance keeps its record but is no longer part of
            // the fleet, so a deployment cannot target it.
            .filter(|instance| instance.is_registered())
            .filter(|instance| group_selects_instance(group, &instance.tags))
            .count(),
    }
}

/// Whether a deployment group selects an on-premises instance carrying `tags`.
///
/// The two on-premises selectors a group can carry work as the AWS CodeDeploy
/// User Guide's "Tagging instances for deployments" describes: a
/// `onPremisesInstanceTagFilters` list includes an instance that matches ANY of
/// its filters, while an `onPremisesTagSet` includes one that matches at least
/// one filter in EVERY one of its groups. A group carrying neither selects no
/// on-premises instance at all.
fn group_selects_instance(group: &DeploymentGroup, tags: &[Tag]) -> bool {
    let by_filters = group
        .on_premises_instance_tag_filters
        .iter()
        .any(|filter| tag_filter_matches(filter, tags));
    let by_tag_set = group
        .on_premises_tag_set
        .as_ref()
        .and_then(|set| set.on_premises_tag_set_list.as_ref())
        .is_some_and(|groups| {
            !groups.is_empty()
                && groups.iter().all(|filters| {
                    filters
                        .iter()
                        .any(|filter| tag_filter_matches(filter, tags))
                })
        });
    by_filters || by_tag_set
}

/// Whether any of `tags` satisfies one on-premises tag filter.
///
/// `TagFilterType` (`smithy_query(query="shape",shape="TagFilterType")`:
/// KEY_ONLY | VALUE_ONLY | KEY_AND_VALUE) says WHICH of the filter's members has
/// to match: `KEY_AND_VALUE` needs BOTH, so a filter asking for it while naming
/// only one of them describes no tag and selects nothing. A filter that states no
/// type can only mean the members it does name, and one that names neither
/// member selects nothing either.
fn tag_filter_matches(filter: &TagFilter, tags: &[Tag]) -> bool {
    let key_matches = |tag: &Tag| filter.key.is_some() && tag.key == filter.key;
    let value_matches = |tag: &Tag| filter.value.is_some() && tag.value == filter.value;
    match filter.r#type.as_deref() {
        Some("KEY_ONLY") => tags.iter().any(key_matches),
        Some("VALUE_ONLY") => tags.iter().any(value_matches),
        Some("KEY_AND_VALUE") => {
            filter.key.is_some()
                && filter.value.is_some()
                && tags.iter().any(|tag| key_matches(tag) && value_matches(tag))
        }
        // A filter that states no type: the members it does name.
        _ => {
            (filter.key.is_some() || filter.value.is_some())
                && tags.iter().any(|tag| {
                    (filter.key.is_none() || key_matches(tag))
                        && (filter.value.is_none() || value_matches(tag))
                })
        }
    }
}

/// The terminal status a deployment with `target_count` targets reaches:
/// `Succeeded` when its revision had somewhere to go, `Failed` when the
/// deployment group resolved nothing to deploy to.
fn terminal_status(target_count: usize) -> &'static str {
    if target_count == 0 {
        DEPLOYMENT_STATUS_FAILED
    } else {
        DEPLOYMENT_STATUS_SUCCEEDED
    }
}

/// Whether `config` asks for automatic rollback without naming an event that
/// would trigger one, which is one of the cases
/// `InvalidAutoRollbackConfigException` describes ("automatic rollback is
/// enabled, but an invalid triggering event type or no event types were
/// listed").
pub fn auto_rollback_is_enabled_without_events(config: &AutoRollbackConfiguration) -> bool {
    config.enabled && config.events.as_ref().is_none_or(Vec::is_empty)
}

/// `config` as it is STORED: `None` when it carries no information.
///
/// A disabled automatic rollback listing no events says nothing a caller did not
/// already get by omitting the member, so it is normalised away at the store
/// rather than kept and echoed — a round-tripped deployment-group or deployment
/// document then holds exactly the members that carry information, and absent
/// and information-free requests read back the same. A configuration that lists
/// events is kept whether or not it is enabled: the events are information.
pub fn normalized_auto_rollback_configuration(
    config: Option<AutoRollbackConfiguration>,
) -> Option<AutoRollbackConfiguration> {
    config.filter(|config| config.enabled || config.events.as_ref().is_some_and(|e| !e.is_empty()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodedeployCodedeploy20141006State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodedeployCodedeploy20141006State>)
}

/// This service's ARN namespace
/// (`smithy_query(query="service_info")`: `arnNamespace` `codedeploy`).
const ARN_SERVICE: &str = "codedeploy";

/// The region an emitted ARN carries when the request carries no SigV4
/// credential scope to take one from (an anonymous or host-routed request).
const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of an on-premises instance:
/// `arn:aws:codedeploy:<region>:<account>:instance/<instanceName>`, the form the
/// CodeDeploy User Guide shows for a registered on-premises instance
/// ("arn:aws:codedeploy:us-west-2:111122223333:instance/AssetTag12010298EX").
/// The account segment comes from the engine, never from a literal.
pub fn instance_arn(region: &str, instance_name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("instance/{instance_name}"))
}

/// Why CodeDeploy will not accept a name a caller sent for an on-premises
/// instance. Each operation maps it to the modelled error IT declares, so the
/// two refusals stay one rule shared by every on-premises operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRefusal {
    /// The caller sent no name at all (absent, empty, or only whitespace).
    Missing,
    /// The caller sent a name outside the form CodeDeploy accepts.
    Malformed,
}

/// The longest resource name CodeDeploy accepts, as the model states it for
/// every name member it constrains (`smithy_query(query="shape",shape="ApplicationName")`:
/// `@length(min: 1, max: 100)`).
pub const MAX_NAME_LENGTH: usize = 100;

/// The characters a CodeDeploy resource name is made of, as the model states it
/// for every name member it constrains
/// (`smithy_query(query="shape",shape="ApplicationName")`: pattern
/// `^[A-Za-z0-9+=,.@_-]*$`).
fn is_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || "+=,.@_-".contains(character)
}

/// Check `name` against the form CodeDeploy accepts for an on-premises instance
/// name: at least one character, at most [`MAX_NAME_LENGTH`], all of them from
/// the service's resource-name character set.
///
/// `InstanceName` carries no `@length` or `@pattern` of its own in the model, so
/// the boundary validator lets every string through and this is where the form
/// is decided.
pub fn check_instance_name(name: &str) -> Result<(), NameRefusal> {
    if name.trim().is_empty() {
        return Err(NameRefusal::Missing);
    }
    if name.chars().count() > MAX_NAME_LENGTH || !name.chars().all(is_name_character) {
        return Err(NameRefusal::Malformed);
    }
    Ok(())
}

/// Whether `candidate` is an ARN of `service` naming a `resource_type` resource.
///
/// Six colon-separated fields — `arn:<partition>:<service>:<region>:<account>:<resource>`
/// — is the ARN form AWS documents, with the resource part opening on the
/// resource type. IAM and STS ARNs are global, so the region field is allowed to
/// be empty; the partition, the account and the resource identifier are not.
fn is_arn_of(candidate: &str, service: &str, resource_type: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(found_service),
        Some(_region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
        && found_service == service
        && !account.is_empty()
        && resource
            .strip_prefix(resource_type)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|identifier| !identifier.is_empty())
}

/// Whether `candidate` is an IAM user ARN — `arn:aws:iam::<account>:user/<name>`,
/// the form `RegisterOnPremisesInstance$iamUserArn` documents ("The ARN of the
/// user to associate with the on-premises instance").
pub fn is_iam_user_arn(candidate: &str) -> bool {
    is_arn_of(candidate, "iam", "user")
}

/// Whether `candidate` is an IAM session ARN —
/// `arn:aws:sts::<account>:assumed-role/<role>/<session>`, the form an STS
/// assumed-role session carries, which is what
/// `RegisterOnPremisesInstance$iamSessionArn` associates with the instance.
pub fn is_iam_session_arn(candidate: &str) -> bool {
    is_arn_of(candidate, "sts", "assumed-role")
}

/// A stored on-premises instance as the modelled `InstanceInfo`.
pub fn instance_info(instance: &OnPremisesInstance) -> crate::ops::types::InstanceInfo {
    crate::ops::types::InstanceInfo {
        instance_name: Some(instance.instance_name.clone()),
        iam_session_arn: instance.iam_session_arn.clone(),
        iam_user_arn: instance.iam_user_arn.clone(),
        instance_arn: Some(instance.instance_arn.clone()),
        register_time: Some(timestamp(instance.register_time_ms)),
        deregister_time: instance.deregister_time_ms.map(timestamp),
        tags: non_empty(&instance.tags),
    }
}

/// How many items one page of a listing carries.
///
/// CodeDeploy's list operations are `@paginated` on `nextToken` alone — neither
/// the model nor the API reference states a page size — so this one is CHOSEN.
pub const PAGE_SIZE: usize = 100;

/// Cut `items` into the page `token` asks for, with the token for the next page.
///
/// `None` means the token is not one this service issued, or names an item this
/// listing no longer carries: answer it with the operation's
/// `InvalidNextTokenException`. The token names the LAST item already returned,
/// not a position, so a second call continues after that item however many items
/// have been created in the meantime — an offset into a newest-first listing
/// would shift under every new record and repeat or skip one.
pub fn paginate(items: &[String], token: Option<&str>) -> Option<(Vec<String>, Option<String>)> {
    let start = match token {
        Some(token) => {
            let last_returned = decode_page_token(token)?;
            items.iter().position(|item| *item == last_returned)? + 1
        }
        None => 0,
    };
    if start > items.len() {
        return None;
    }
    let end = (start + PAGE_SIZE).min(items.len());
    let next = (end < items.len())
        .then(|| items.get(end - 1).map(|last| encode_page_token(last)))
        .flatten();
    Some((items[start..end].to_vec(), next))
}

/// Render the last item of a page as the opaque continuation token a client
/// echoes back.
///
/// Tagged and hex-rendered so it cannot be mistaken for — or hand-built from —
/// an item identifier.
fn encode_page_token(last_returned: &str) -> String {
    format!("p{}", hex::encode(last_returned))
}

/// Read back a token [`encode_page_token`] produced; `None` for anything else.
fn decode_page_token(token: &str) -> Option<String> {
    let encoded = token.strip_prefix('p')?;
    String::from_utf8(hex::decode(encoded).ok()?).ok()
}

/// A stored application as the modelled `ApplicationInfo`.
pub fn application_info(application: &Application) -> crate::ops::types::ApplicationInfo {
    crate::ops::types::ApplicationInfo {
        application_id: Some(application.application_id.clone()),
        application_name: Some(application.application_name.clone()),
        create_time: Some(timestamp(application.create_time_ms)),
        linked_to_git_hub: application.linked_to_git_hub,
        git_hub_account_name: application.git_hub_account_name.clone(),
        compute_platform: Some(application.compute_platform.clone()),
    }
}

/// A stored deployment configuration as the modelled `DeploymentConfigInfo`.
pub fn deployment_config_info(
    config: &DeploymentConfig,
) -> crate::ops::types::DeploymentConfigInfo {
    crate::ops::types::DeploymentConfigInfo {
        deployment_config_id: Some(config.deployment_config_id.clone()),
        deployment_config_name: Some(config.deployment_config_name.clone()),
        minimum_healthy_hosts: config.minimum_healthy_hosts.clone(),
        create_time: Some(timestamp(config.create_time_ms)),
        compute_platform: Some(config.compute_platform.clone()),
        traffic_routing_config: config.traffic_routing_config.clone(),
        zonal_config: config.zonal_config.clone(),
    }
}

/// A stored deployment as the modelled `DeploymentInfo`.
pub fn deployment_info(deployment: &Deployment) -> crate::ops::types::DeploymentInfo {
    crate::ops::types::DeploymentInfo {
        application_name: Some(deployment.application_name.clone()),
        deployment_group_name: deployment.deployment_group_name.clone(),
        deployment_config_name: Some(deployment.deployment_config_name.clone()),
        deployment_id: Some(deployment.deployment_id.clone()),
        previous_revision: deployment.previous_revision.clone(),
        revision: deployment.revision.clone(),
        status: Some(deployment.status.clone()),
        error_information: deployment_error_information(deployment),
        create_time: Some(timestamp(deployment.create_time_ms)),
        start_time: deployment.start_time_ms.map(timestamp),
        complete_time: deployment.complete_time_ms.map(timestamp),
        deployment_overview: None,
        description: deployment.description.clone(),
        creator: Some(deployment.creator.clone()),
        ignore_application_stop_failures: deployment.ignore_application_stop_failures,
        auto_rollback_configuration: deployment.auto_rollback_configuration.clone(),
        update_outdated_instances_only: deployment.update_outdated_instances_only,
        rollback_info: deployment.rollback_info.clone(),
        deployment_style: deployment.deployment_style.clone(),
        target_instances: deployment.target_instances.clone(),
        instance_termination_wait_time_started: deployment
            .instance_termination_wait_time_started,
        blue_green_deployment_configuration: None,
        load_balancer_info: None,
        additional_deployment_status_info: None,
        file_exists_behavior: deployment.file_exists_behavior.clone(),
        deployment_mode: deployment.deployment_mode.clone(),
        deployment_status_messages: None,
        compute_platform: Some(deployment.compute_platform.clone()),
        external_id: deployment.external_id.clone(),
        related_deployments: None,
        override_alarm_configuration: deployment.override_alarm_configuration.clone(),
        ec2_deployment_options: deployment.ec2_deployment_options.clone(),
    }
}

/// Why a failed deployment failed, as the modelled `ErrorInformation`.
///
/// A failure the service RECORDED on the deployment — a lifecycle event hook
/// reporting a failed validation, say — is reported as it was recorded; anything
/// else is DERIVED from the deployment's own record (the terminal status the
/// clock gave it and the targets its group resolved) rather than stored a second
/// time where it could disagree with the status beside it.
fn deployment_error_information(
    deployment: &Deployment,
) -> Option<crate::ops::types::ErrorInformation> {
    if let Some(recorded) = deployment.error_information.clone() {
        return Some(recorded);
    }
    (deployment.status == DEPLOYMENT_STATUS_FAILED && deployment.target_count == 0).then(|| {
        crate::ops::types::ErrorInformation {
            code: Some(ERROR_CODE_NO_INSTANCES.to_string()),
            message: Some(
                "No instances found for the deployment group of this deployment".to_string(),
            ),
        }
    })
}

/// A stored deployment group as the modelled `DeploymentGroupInfo`.
///
/// `lastAttemptedDeployment` and `lastSuccessfulDeployment` are DERIVED from the
/// deployment index — the newest deployment of this group, and the newest one
/// that succeeded — rather than kept as a second copy that could disagree with
/// it. `targetRevision` is the revision of that last attempt, which is what
/// CodeDeploy reports as the group's target.
pub fn deployment_group_info(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    group: &DeploymentGroup,
) -> crate::ops::types::DeploymentGroupInfo {
    let of_group = |deployment: &&Deployment| {
        deployment.application_name == group.application_name
            && deployment.deployment_group_name.as_deref()
                == Some(group.deployment_group_name.as_str())
    };
    let newest = |predicate: &dyn Fn(&&Deployment) -> bool| {
        state
            .deployment_order
            .iter()
            .rev()
            .filter_map(|id| state.deployments.get(id))
            .find(|deployment| of_group(deployment) && predicate(deployment))
            .cloned()
    };
    let last_attempted = newest(&|_| true);
    let last_successful = newest(&|deployment| deployment.status == DEPLOYMENT_STATUS_SUCCEEDED);
    let as_last_deployment = |deployment: &Deployment| crate::ops::types::LastDeploymentInfo {
        deployment_id: Some(deployment.deployment_id.clone()),
        status: Some(deployment.status.clone()),
        end_time: deployment.complete_time_ms.map(timestamp),
        create_time: Some(timestamp(deployment.create_time_ms)),
    };

    crate::ops::types::DeploymentGroupInfo {
        application_name: Some(group.application_name.clone()),
        deployment_group_id: Some(group.deployment_group_id.clone()),
        deployment_group_name: Some(group.deployment_group_name.clone()),
        deployment_config_name: Some(group.deployment_config_name.clone()),
        // The group's instance-selection and trigger lists are reported as the
        // lists the group HOLDS, empty included: a request that sent one empty
        // and a request that omitted it gave the group the same thing, so they
        // read back the same way and a client can index the member instead of
        // handling absent and empty separately.
        ec2_tag_filters: Some(group.ec2_tag_filters.clone()),
        on_premises_instance_tag_filters: Some(group.on_premises_instance_tag_filters.clone()),
        auto_scaling_groups: Some(
            group
                .auto_scaling_groups
                .iter()
                .map(|name| crate::ops::types::AutoScalingGroup {
                    name: Some(name.clone()),
                    ..Default::default()
                })
                .collect(),
        ),
        service_role_arn: Some(group.service_role_arn.clone()),
        target_revision: last_attempted
            .as_ref()
            .and_then(|deployment| deployment.revision.clone()),
        trigger_configurations: Some(group.trigger_configurations.clone()),
        alarm_configuration: group.alarm_configuration.clone(),
        auto_rollback_configuration: group.auto_rollback_configuration.clone(),
        deployment_style: group.deployment_style.clone(),
        outdated_instances_strategy: group.outdated_instances_strategy.clone(),
        blue_green_deployment_configuration: group.blue_green_deployment_configuration.clone(),
        load_balancer_info: group.load_balancer_info.clone(),
        last_successful_deployment: last_successful.as_ref().map(as_last_deployment),
        last_attempted_deployment: last_attempted.as_ref().map(as_last_deployment),
        ec2_tag_set: group.ec2_tag_set.clone(),
        on_premises_tag_set: group.on_premises_tag_set.clone(),
        compute_platform: Some(group.compute_platform.clone()),
        // `ecsServices` is reported only when the group has any: the model scopes
        // the member to one compute platform ("This applies only to deployment
        // groups that use the Amazon ECS compute platform"), so an empty list is
        // not something every group holds.
        ecs_services: non_empty(&group.ecs_services),
        ec2_deployment_options: group.ec2_deployment_options.clone(),
        termination_hook_enabled: group.termination_hook_enabled,
    }
}

/// A stored list as an optional modelled list: absent when nothing was stored,
/// for a member a resource does not always carry.
fn non_empty<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// A stored revision as the modelled `GenericRevisionInfo`, with the deployment
/// groups this revision is the current target revision of.
///
/// `deploymentGroups` is DERIVED from the deployment index — the groups whose
/// last attempted deployment deployed this revision — rather than kept as a
/// second copy that could disagree with it. `targets` is the one
/// [`target_revision_keys`] map the caller built for this request, so projecting
/// a page of revisions walks the index ONCE however many revisions it carries.
pub fn generic_revision_info(
    application: &Application,
    targets: &HashMap<String, String>,
    key: &str,
) -> crate::ops::types::GenericRevisionInfo {
    let revision = application.revisions.get(key);
    crate::ops::types::GenericRevisionInfo {
        description: revision.and_then(|revision| revision.description.clone()),
        deployment_groups: non_empty(&deployment_groups_targeting(application, targets, key)),
        first_used_time: revision
            .and_then(|revision| revision.first_used_time_ms)
            .map(timestamp),
        last_used_time: revision
            .and_then(|revision| revision.last_used_time_ms)
            .map(timestamp),
        register_time: revision.map(|revision| timestamp(revision.register_time_ms)),
    }
}

/// The names of `application`'s deployment groups whose target revision — the
/// revision of their last attempted deployment, from the `targets` map — is the
/// revision `key` names, in the application's own group order so the answer is
/// deterministic.
pub fn deployment_groups_targeting(
    application: &Application,
    targets: &HashMap<String, String>,
    key: &str,
) -> Vec<String> {
    application
        .deployment_group_order
        .iter()
        .filter(|group| targets.get(group.as_str()).map(String::as_str) == Some(key))
        .cloned()
        .collect()
}

/// The target revision of each of `application`'s deployment groups that has
/// one: the [`revision_key`] of its last attempted deployment, by group name.
///
/// A group's target revision is the revision of its last attempted deployment,
/// which is what `DeploymentGroupInfo.targetRevision` reports; it is DERIVED
/// from the deployment index rather than kept as a second copy that could
/// disagree with it.
///
/// ONE reverse pass over the deployment index answers every group at once — the
/// newest deployment of a group is the first one the pass meets — and it reads
/// each deployment's STORED revision key, so nothing here re-hashes a revision's
/// content. Build it once per request and hand it to the projection.
pub fn target_revision_keys(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    application: &Application,
) -> HashMap<String, String> {
    let mut targets: HashMap<String, String> = HashMap::new();
    for deployment in state
        .deployment_order
        .iter()
        .rev()
        .filter_map(|id| state.deployments.get(id))
    {
        if deployment.application_name != application.application_name {
            continue;
        }
        let (Some(group), Some(key)) = (
            deployment.deployment_group_name.clone(),
            deployment.revision_key.clone(),
        ) else {
            continue;
        };
        targets.entry(group).or_insert(key);
    }
    targets
}

/// The identity of an application revision: which artifact it names, not how it
/// is packaged.
///
/// `None` for a `RevisionLocation` that names no location at all — the model
/// holds one location per `revisionType`
/// (`smithy_query(query="shape",shape="RevisionLocation")`: s3Location,
/// gitHubLocation, string, appSpecContent) and `revisionType` itself only says
/// which of them to read — which is what `InvalidRevisionException` describes.
///
/// `s3Location.bundleType` is NOT part of the identity: it says how to unpack
/// the object, not which object it is. The inline `string` and `appSpecContent`
/// revisions are identified by the digest of their content, so two requests
/// carrying the same AppSpec name the same revision however they were sent.
pub fn revision_key(revision: &RevisionLocation) -> Option<String> {
    if let Some(s3) = &revision.s3_location {
        let field = |value: &Option<String>| value.clone().unwrap_or_default();
        return Some(format!(
            "S3|{}|{}|{}|{}",
            field(&s3.bucket),
            field(&s3.key),
            field(&s3.version),
            field(&s3.e_tag)
        ));
    }
    if let Some(github) = &revision.git_hub_location {
        let field = |value: &Option<String>| value.clone().unwrap_or_default();
        return Some(format!(
            "GitHub|{}|{}",
            field(&github.repository),
            field(&github.commit_id)
        ));
    }
    if let Some(raw) = &revision.string {
        return Some(format!(
            "String|{}",
            content_digest(raw.content.as_deref().unwrap_or_default())
        ));
    }
    if let Some(app_spec) = &revision.app_spec_content {
        return Some(format!(
            "AppSpecContent|{}",
            content_digest(app_spec.content.as_deref().unwrap_or_default())
        ));
    }
    None
}

/// The hex SHA-256 of an inline revision's content, which is what identifies it.
fn content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Run `body` against this service's store, with the built-ins seeded and every
/// clock-owed deployment completion already applied.
///
/// EVERY handler goes through here, so no handler can observe an unseeded store
/// or a deployment that should already have completed. `None` means the engine
/// is not the shared in-memory one — answer it with a modelled server error.
pub fn with_state<Eng: Send + Sync + 'static, R>(
    engine: &Eng,
    body: impl FnOnce(&mut ComAmazonawsCodedeployCodedeploy20141006State) -> R,
) -> Option<R> {
    let state = service_state(engine)?;
    let mut guard = state.lock();
    let now = now_ms();
    guard.ensure_seeded(now);
    guard.advance_clock(now);
    Some(body(&mut guard))
}

#[cfg(test)]
mod tests {
    //! What [`deployment_target_count`] resolves, over a store built in place.
    //!
    //! These tables span the selector forms exhaustively and cheaply — every
    //! `TagFilterType`, a filter naming only one member, an empty tag set, a
    //! deregistered instance — where building the same row over the wire costs a
    //! registration, a tagging call and a deployment each. The rule they pin is
    //! also asserted end-to-end, through the operations a client calls, in
    //! `tests/all/codedeploy_deployment_outcome.rs`.

    use super::*;

    /// A deployment group of `platform` carrying no selector at all; each case
    /// below sets the one member it is about.
    fn group(platform: &str) -> DeploymentGroup {
        DeploymentGroup {
            deployment_group_id: "dgid".to_string(),
            deployment_group_name: "dg".to_string(),
            application_name: "app".to_string(),
            service_role_arn: "arn:aws:iam::123456789012:role/CodeDeployRole".to_string(),
            deployment_config_name: "CodeDeployDefault.OneAtATime".to_string(),
            compute_platform: platform.to_string(),
            ec2_tag_filters: Vec::new(),
            on_premises_instance_tag_filters: Vec::new(),
            auto_scaling_groups: Vec::new(),
            trigger_configurations: Vec::new(),
            alarm_configuration: None,
            auto_rollback_configuration: None,
            outdated_instances_strategy: None,
            deployment_style: None,
            blue_green_deployment_configuration: None,
            load_balancer_info: None,
            ec2_tag_set: None,
            on_premises_tag_set: None,
            ecs_services: Vec::new(),
            ec2_deployment_options: None,
            termination_hook_enabled: false,
            tags: Vec::new(),
            create_time_ms: 0,
        }
    }

    fn tag(key: &str, value: &str) -> Tag {
        Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    /// One on-premises tag filter: `kind` is a `TagFilterType` value, or `None`
    /// for a filter that states no type.
    fn filter(key: Option<&str>, value: Option<&str>, kind: Option<&str>) -> TagFilter {
        TagFilter {
            key: key.map(str::to_string),
            value: value.map(str::to_string),
            r#type: kind.map(str::to_string),
        }
    }

    /// Put one on-premises instance in the registry.
    fn register(
        state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
        name: &str,
        tags: Vec<Tag>,
        deregistered: bool,
    ) {
        state.register_on_premises_instance(OnPremisesInstance {
            instance_name: name.to_string(),
            instance_arn: instance_arn("us-east-1", name),
            iam_user_arn: Some(format!("arn:aws:iam::123456789012:user/{name}")),
            iam_session_arn: None,
            register_time_ms: 1,
            deregister_time_ms: deregistered.then_some(2),
            tags,
        });
    }

    /// RULE: an EC2/on-premises deployment group targets the REGISTERED
    /// on-premises instances its `onPremisesInstanceTagFilters` (any filter) or
    /// its `onPremisesTagSet` (one filter from every group) select.
    #[test]
    fn a_server_group_targets_the_on_premises_instances_it_selects() {
        // Rows differ along the selector the group carries, the `TagFilterType`
        // it uses, and whether the instance's tags satisfy it.
        struct Case {
            case: &'static str,
            filters: Vec<TagFilter>,
            tag_set: Option<Vec<Vec<TagFilter>>>,
            expected: usize,
        }
        let cases = vec![
            Case {
                case: "no selector at all",
                filters: Vec::new(),
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE matching both instances",
                filters: vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "KEY_AND_VALUE matching neither",
                filters: vec![filter(Some("Env"), Some("staging"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE naming only the key",
                filters: vec![filter(Some("Env"), None, Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE naming only the value",
                filters: vec![filter(None, Some("prod"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_ONLY, which ignores the value",
                filters: vec![filter(Some("Rack"), Some("ignored"), Some("KEY_ONLY"))],
                tag_set: None,
                expected: 1,
            },
            Case {
                case: "VALUE_ONLY, which ignores the key",
                filters: vec![filter(Some("ignored"), Some("seven"), Some("VALUE_ONLY"))],
                tag_set: None,
                expected: 1,
            },
            Case {
                case: "no type, so the members the filter names",
                filters: vec![filter(Some("Env"), Some("prod"), None)],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "a filter naming nothing",
                filters: vec![filter(None, None, None)],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "two filters, either of which includes an instance",
                filters: vec![
                    filter(Some("Rack"), Some("seven"), Some("KEY_AND_VALUE")),
                    filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE")),
                ],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "a tag set whose every group matches one instance",
                filters: Vec::new(),
                tag_set: Some(vec![
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    vec![filter(Some("Rack"), Some("seven"), Some("KEY_AND_VALUE"))],
                ]),
                expected: 1,
            },
            Case {
                case: "a tag set with a group nothing matches",
                filters: Vec::new(),
                tag_set: Some(vec![
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    vec![filter(Some("Rack"), Some("nine"), Some("KEY_AND_VALUE"))],
                ]),
                expected: 0,
            },
            Case {
                case: "an empty tag set",
                filters: Vec::new(),
                tag_set: Some(Vec::new()),
                expected: 0,
            },
        ];

        for case in cases {
            let mut state = ComAmazonawsCodedeployCodedeploy20141006State::default();
            register(
                &mut state,
                "rack-seven",
                vec![tag("Env", "prod"), tag("Rack", "seven")],
                false,
            );
            register(&mut state, "plain", vec![tag("Env", "prod")], false);
            // Deregistered, and tagged exactly like the first: it is a record,
            // not a target.
            register(
                &mut state,
                "retired",
                vec![tag("Env", "prod"), tag("Rack", "seven")],
                true,
            );

            let mut group = group("Server");
            group.on_premises_instance_tag_filters = case.filters;
            group.on_premises_tag_set = case.tag_set.map(|list| OnPremisesTagSet {
                on_premises_tag_set_list: Some(list),
            });
            let got = deployment_target_count(&state, &group);
            assert_eq!(
                got, case.expected,
                "case {:?}: expected {} target(s), got {got}",
                case.case, case.expected
            );
        }
    }

    /// RULE: an EC2 selector resolves nothing here — CodeDeploy's state keeps no
    /// EC2 inventory — while an ECS group's targets are its `ecsServices` and a
    /// Lambda group has exactly one target.
    #[test]
    fn the_other_selectors_and_platforms_resolve_their_own_targets() {
        let mut state = ComAmazonawsCodedeployCodedeploy20141006State::default();
        register(&mut state, "rack-seven", vec![tag("Env", "prod")], false);

        let ec2_filter = EC2TagFilter {
            key: Some("Env".to_string()),
            value: Some("prod".to_string()),
            r#type: Some("KEY_AND_VALUE".to_string()),
        };
        let mut by_ec2_filter = group("Server");
        by_ec2_filter.ec2_tag_filters = vec![ec2_filter.clone()];
        let mut by_ec2_tag_set = group("Server");
        by_ec2_tag_set.ec2_tag_set = Some(EC2TagSet {
            ec2_tag_set_list: Some(vec![vec![ec2_filter]]),
        });
        let mut by_auto_scaling = group("Server");
        by_auto_scaling.auto_scaling_groups = vec!["asg-web".to_string()];
        let mut one_service = group("ECS");
        one_service.ecs_services = vec![ECSService {
            cluster_name: Some("cluster".to_string()),
            service_name: Some("web".to_string()),
        }];
        let mut two_services = group("ECS");
        two_services.ecs_services = vec![
            ECSService {
                cluster_name: Some("cluster".to_string()),
                service_name: Some("web".to_string()),
            },
            ECSService {
                cluster_name: Some("cluster".to_string()),
                service_name: Some("api".to_string()),
            },
        ];

        let cases: &[(&str, DeploymentGroup, usize)] = &[
            ("an ec2 tag filter", by_ec2_filter, 0),
            ("an ec2 tag set", by_ec2_tag_set, 0),
            ("an auto scaling group", by_auto_scaling, 0),
            ("an ecs group with no service", group("ECS"), 0),
            ("an ecs group with one service", one_service, 1),
            ("an ecs group with two services", two_services, 2),
            ("a lambda group", group("Lambda"), 1),
        ];

        for (case, group, expected) in cases {
            let got = deployment_target_count(&state, group);
            assert_eq!(
                got, *expected,
                "case {case:?}: expected {expected} target(s), got {got}"
            );
            assert_eq!(
                terminal_status(got),
                if *expected == 0 {
                    DEPLOYMENT_STATUS_FAILED
                } else {
                    DEPLOYMENT_STATUS_SUCCEEDED
                },
                "case {case:?}: expected the terminal status its targets imply, got {got} target(s)"
            );
        }
    }
}
