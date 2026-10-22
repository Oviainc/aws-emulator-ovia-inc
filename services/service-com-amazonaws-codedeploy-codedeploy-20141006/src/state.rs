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
/// See [`DEPLOYMENT_STATUS_CREATED`]: the status a deployment a caller stopped
/// holds.
pub const DEPLOYMENT_STATUS_STOPPED: &str = "Stopped";

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
        DEPLOYMENT_STATUS_SUCCEEDED | "Failed" | DEPLOYMENT_STATUS_STOPPED
    )
}

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

/// A registered application, with the deployment groups that belong to it.
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
    /// `Succeeded` once it was owed completion.
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
            if deployment.start_time_ms.is_some()
                && deployment.complete_time_ms.is_none()
                && deployment.completes_at_ms <= now_ms
            {
                deployment.status = DEPLOYMENT_STATUS_SUCCEEDED.to_string();
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
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let digest = self.digest("deployment");
        let suffix: String = digest
            .iter()
            .take(9)
            .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
            .collect();
        format!("d-{suffix}")
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
        error_information: None,
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
        instance_termination_wait_time_started: false,
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
        ec2_tag_filters: non_empty(&group.ec2_tag_filters),
        on_premises_instance_tag_filters: non_empty(&group.on_premises_instance_tag_filters),
        auto_scaling_groups: non_empty(
            &group
                .auto_scaling_groups
                .iter()
                .map(|name| crate::ops::types::AutoScalingGroup {
                    name: Some(name.clone()),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        ),
        service_role_arn: Some(group.service_role_arn.clone()),
        target_revision: last_attempted
            .as_ref()
            .and_then(|deployment| deployment.revision.clone()),
        trigger_configurations: non_empty(&group.trigger_configurations),
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
        ecs_services: non_empty(&group.ecs_services),
        ec2_deployment_options: group.ec2_deployment_options.clone(),
        termination_hook_enabled: group.termination_hook_enabled,
    }
}

/// A stored list as an optional modelled list: absent when nothing was stored,
/// so a group created without one does not report an empty list for it.
fn non_empty<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
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
