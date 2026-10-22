// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationautoscaling-anyscalefrontendservice service.
//!
//! A scalable target's identity in Application Auto Scaling is the triple
//! (`ServiceNamespace`, `ResourceId`, `ScalableDimension`) — the model documents that
//! "scalable targets are uniquely identified by the combination of resource ID, scalable
//! dimension, and namespace" — so [`TargetKey`] is that triple and both maps below are
//! keyed by it. A scaling policy adds its `PolicyName` ([`PolicyKey`]).
//!
//! Both maps are `BTreeMap`s, so `Describe*` enumerates in a stable order and an opaque
//! `NextToken` only has to carry the last key a page emitted.
//!
//! Besides the state itself this module holds the helpers more than one operation needs:
//! ARN minting, the page/token codec and the shared validation messages. They live here
//! rather than in one operation's file so the six handlers of the target/policy lifecycle
//! cannot drift from each other.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Alarm, NotScaledReason, PredictiveScalingPolicyConfiguration, ScalableTarget, ScalingActivity,
    ScalingPolicy, ScheduledAction, StepScalingPolicyConfiguration, SuspendedState,
    TargetTrackingScalingPolicyConfiguration,
};

/// The natural AWS identity of a scalable target, and the prefix of a scaling policy's
/// and a scheduled action's identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetKey {
    pub service_namespace: String,
    pub resource_id: String,
    pub scalable_dimension: String,
}

/// A scaling policy's identity: its target plus its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyKey {
    pub target: TargetKey,
    pub policy_name: String,
}

/// A scheduled action's identity: its target plus its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduledActionKey {
    pub target: TargetKey,
    pub scheduled_action_name: String,
}

/// What `RegisterScalableTarget` persists, and what `DescribeScalableTargets`
/// reconstructs a `ScalableTarget` from.
///
/// `MinCapacity`/`MaxCapacity`/`RoleARN` are stored unconditionally rather than as
/// options because the modelled `ScalableTarget` requires all three: the first
/// registration must supply the two capacities (the model's own documentation: "This
/// property is required when registering a new scalable target") and `RoleARN` falls back
/// to the service-linked role the service uses when the caller names none.
#[derive(Debug, Clone)]
pub struct ScalableTargetRecord {
    pub arn: String,
    pub min_capacity: i32,
    pub max_capacity: i32,
    /// The capacity this service believes the scalable target currently sits at.
    ///
    /// Application Auto Scaling learns a resource's capacity from the target service
    /// (ECS, DynamoDB, …) and moves it on the target's behalf; this emulator does not
    /// drive those data planes yet, so it tracks the capacity of the scalable target
    /// itself. A first registration adopts the window's floor — the registered minimum
    /// is the smallest capacity "inside of this range", and adopting it records no
    /// activity because nothing was moved — and every later movement is an action this
    /// service performed, so it is recorded as a scaling activity.
    pub current_capacity: i32,
    pub role_arn: String,
    pub creation_time: SystemTime,
    pub suspended_state: Option<SuspendedState>,
    pub tags: BTreeMap<String, String>,
}

/// What `PutScheduledAction` persists: the action as `DescribeScheduledActions` reports
/// it, plus the bookkeeping the clock needs.
///
/// `last_applied` is the instant the action last moved the target's capacity bounds. It
/// is what makes a recurring schedule idempotent: the occurrence that fires is the first
/// one AFTER this instant, so materialising the same occurrence twice is impossible.
#[derive(Debug, Clone)]
pub struct ScheduledActionRecord {
    pub action: ScheduledAction,
    pub last_applied: Option<SystemTime>,
}

/// What `PutScalingPolicy` persists, and what `DescribeScalingPolicies` reconstructs a
/// `ScalingPolicy` from.
#[derive(Debug, Clone)]
pub struct ScalingPolicyRecord {
    pub policy_arn: String,
    pub policy_type: String,
    pub step_config: Option<StepScalingPolicyConfiguration>,
    pub target_tracking_config: Option<TargetTrackingScalingPolicyConfiguration>,
    pub predictive_config: Option<PredictiveScalingPolicyConfiguration>,
    pub alarms: Vec<Alarm>,
    pub creation_time: SystemTime,
}

/// One entry of the scaling-activity log: a capacity movement this service performed,
/// or declined to perform, on a scalable target.
///
/// The status is NOT stored as a single current value. A movement is non-terminal when
/// it starts and terminal once the clock reaches [`ScalingActivityRecord::fulfilled_at`],
/// so the record keeps the terminal status it is owed and a reader derives the status it
/// reports from the instant it reads at. Nothing has to tick for an activity to complete.
#[derive(Debug, Clone)]
pub struct ScalingActivityRecord {
    /// The activity's unique identifier, as `ScalingActivity$ActivityId`.
    pub activity_id: String,
    /// Where this activity sits in the order the log recorded them, so activities that
    /// share a `StartTime` still have one newest-first order.
    pub sequence: u64,
    /// The scalable target the activity belongs to; its three members are what
    /// `DescribeScalingActivities` filters on.
    pub target: TargetKey,
    /// "A simple description of what action the scaling activity intends to accomplish."
    pub description: String,
    /// "A simple description of what caused the scaling activity to happen."
    pub cause: String,
    pub start_time: SystemTime,
    /// When the activity reaches [`ScalingActivityRecord::terminal_status_code`], or
    /// `None` for one that was already terminal when it was recorded (an activity that
    /// was not scaled never started).
    pub fulfilled_at: Option<SystemTime>,
    /// The status the activity ends in, one of `SCALING_ACTIVITY_STATUS_CODE_VALUES`.
    pub terminal_status_code: String,
    /// The message that goes with that terminal status.
    pub terminal_status_message: Option<String>,
    pub details: Option<String>,
    /// Why the activity was not scaled; empty for one that was. A non-empty list is
    /// what makes this a "not scaled activity", which a `Describe` hides unless the
    /// caller asks for it.
    ///
    /// Only [`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::record_capacity_change`]
    /// writes records today and a movement it records WAS scaled, so no stored activity
    /// carries reasons yet: the writer that will is the one that evaluates a scaling
    /// policy or fires a scheduled action and declines to move capacity
    /// (`AlreadyAtMaxCapacity` and the other codes the model's `NotScaledReason$Code`
    /// lists). Same for `details`.
    pub not_scaled_reasons: Vec<NotScaledReason>,
}

/// Per-service state for com-amazonaws-applicationautoscaling-anyscalefrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState {
    /// Registered scalable targets, ordered by their natural identity.
    pub targets: BTreeMap<TargetKey, ScalableTargetRecord>,
    /// Scaling policies, ordered by (target, policy name).
    pub policies: BTreeMap<PolicyKey, ScalingPolicyRecord>,
    /// `ScalableTargetARN` -> the target it names, so an ARN-addressed operation
    /// (`TagResource` and friends) can resolve a `ResourceARN` back to a target.
    pub arn_index: BTreeMap<String, TargetKey>,
    /// Scheduled actions, ordered by (target, action name). `DeregisterScalableTarget`
    /// cascades into this map, so a deregistered target leaves none behind.
    ///
    /// The record holds the modelled `ScheduledAction` as-is — every member of it is
    /// either part of the key or a value `PutScheduledAction` persisted verbatim, so
    /// `DescribeScheduledActions` has nothing to rebuild — plus the one thing the clock
    /// needs: when an occurrence of it was last applied.
    pub scheduled_actions: BTreeMap<ScheduledActionKey, ScheduledActionRecord>,
    /// The scaling-activity log, oldest first — every capacity movement this service
    /// performed, within the window it reports over. `DescribeScalingActivities` reads
    /// it newest first.
    pub activities: Vec<ScalingActivityRecord>,
    /// How many activities have ever been recorded. It only grows, so it both orders the
    /// log and makes each minted `ActivityId` unique.
    pub activities_recorded: u64,
    /// The targets an update is IN FLIGHT for right now, which is what makes a concurrent
    /// update of one target observable: `ConcurrentUpdateException` is documented as what
    /// AWS answers "if you request an update to an Application Auto Scaling resource that
    /// already has a pending update", so an update has to be pending for the span it
    /// takes, and a second request for the same target inside that span is refused
    /// instead of silently overwriting the first one's read-modify-write.
    ///
    /// Entered and left by [`Self::begin_target_update`] / [`Self::end_target_update`];
    /// a span, never a stored status, so nothing has to be cleaned up by a later caller.
    pub updating: BTreeSet<TargetKey>,
}

impl ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState {
    /// Record the scaling activity for a capacity movement this service has just made on
    /// `target`, and answer its `ActivityId`.
    ///
    /// The activity is non-terminal for [`ACTIVITY_FULFILMENT`] and `Successful`
    /// afterwards: this service moved the capacity, so the movement is fulfilled.
    pub fn record_capacity_change(
        &mut self,
        target: &TargetKey,
        capacity: i32,
        cause: impl Into<String>,
        at: SystemTime,
    ) -> String {
        let noun = capacity_noun(&target.scalable_dimension);
        let sequence = self.activities_recorded;
        self.activities_recorded += 1;
        let activity_id = scaling_activity_id(sequence, target);
        self.activities.push(ScalingActivityRecord {
            activity_id: activity_id.clone(),
            sequence,
            target: target.clone(),
            description: format!("Setting {noun} to {capacity}."),
            cause: cause.into(),
            start_time: at,
            fulfilled_at: Some(at + ACTIVITY_FULFILMENT),
            terminal_status_code: ACTIVITY_SUCCESSFUL.to_string(),
            terminal_status_message: Some(format!(
                "Successfully set {noun} to {capacity}. Change successfully fulfilled by {}.",
                target.service_namespace
            )),
            details: None,
            not_scaled_reasons: Vec::new(),
        });
        // The emulator's own ceiling on the log: the oldest entries go first, exactly as
        // ageing out of the reported window would have dropped them.
        let overflow = self.activities.len().saturating_sub(MAX_SCALING_ACTIVITIES);
        if overflow > 0 {
            self.activities.drain(..overflow);
        }
        activity_id
    }

    /// Mark an update of `target` as in flight, answering `false` when another update of
    /// the SAME target already is — the caller then refuses its request with the modelled
    /// `ConcurrentUpdateException`. Updates of DIFFERENT targets never collide.
    pub fn begin_target_update(&mut self, target: &TargetKey) -> bool {
        self.updating.insert(target.clone())
    }

    /// Release the in-flight marker [`Self::begin_target_update`] took.
    pub fn end_target_update(&mut self, target: &TargetKey) {
        self.updating.remove(target);
    }

    /// Drop the activities that have aged out of the window this service reports over.
    ///
    /// This is owed by the CLOCK, not by the read that calls it: an activity older than
    /// [`ACTIVITY_RETENTION`] was already outside the window at that same instant,
    /// whether or not anybody asked.
    pub fn expire_old_activities(&mut self, now: SystemTime) {
        // Before the window is wider than the clock there is nothing to expire.
        let Some(cutoff) = now.checked_sub(ACTIVITY_RETENTION) else {
            return;
        };
        self.activities
            .retain(|activity| activity.start_time >= cutoff);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>)
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------

/// How many scalable targets this store holds before a NEW registration is refused with
/// the modelled `LimitExceededException`.
///
/// This is the EMULATOR's own bound, not a claim about any AWS account quota: a
/// long-lived emulator serves unauthenticated callers, and `ResourceId` is a free-form
/// string, so without a ceiling N requests leave N resident records for ever. It is set
/// far above any realistic workflow, so nothing a test or a client does legitimately
/// reaches it. Bounding `targets` also bounds `arn_index` (one entry per target) and the
/// per-request vector a `Describe` builds.
pub const MAX_SCALABLE_TARGETS: usize = 10_000;

/// The same bound for scaling policies, whose records additionally carry the caller's
/// whole policy configuration. See [`MAX_SCALABLE_TARGETS`].
pub const MAX_SCALING_POLICIES: usize = 20_000;

/// The same bound for scheduled actions, which hang off a target the same way a policy
/// does and are keyed by a free-form name. See [`MAX_SCALABLE_TARGETS`].
pub const MAX_SCHEDULED_ACTIONS: usize = 20_000;

/// How many scaling activities the log keeps before the oldest are dropped.
///
/// The same emulator-own bound as [`MAX_SCALABLE_TARGETS`], for the same reason: the log
/// grows with every capacity movement and is otherwise only trimmed by
/// [`ACTIVITY_RETENTION`], which a long-lived emulator may never reach.
pub const MAX_SCALING_ACTIVITIES: usize = 10_000;

/// The window `DescribeScalingActivities` reports over: the operation's own
/// documentation, "provides descriptive information about the scaling activities in the
/// specified namespace from the previous six weeks".
pub const ACTIVITY_RETENTION: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// How long after the call that starts it a scaling activity reaches its terminal
/// status.
///
/// CHOSEN, not documented: on AWS the fulfilment time is the target service's own. The
/// span is inside this project's 250-750 ms band for a transition the clock owes, so a
/// caller that reads the activity log straight after the call that moved capacity still
/// observes the non-terminal activity, and one that reads a moment later sees it
/// fulfilled.
pub const ACTIVITY_FULFILMENT: Duration = Duration::from_millis(600);

/// The non-terminal status a capacity movement carries until it is fulfilled, from the
/// model's `ScalingActivityStatusCode` enum (`SCALING_ACTIVITY_STATUS_CODE_VALUES`).
pub const ACTIVITY_IN_PROGRESS: &str = "InProgress";
/// The status a fulfilled capacity movement ends in. See [`ACTIVITY_IN_PROGRESS`].
pub const ACTIVITY_SUCCESSFUL: &str = "Successful";

// ---------------------------------------------------------------------------
// Request-derived values
// ---------------------------------------------------------------------------

/// The region every ARN this service mints is scoped to: the one in the caller's SigV4
/// credential scope, or `us-east-1` when the request carries none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// A scalable target's ARN: `arn:aws:application-autoscaling:<region>:<account>:scalable-target/<id>`.
///
/// The opaque id is derived from the target's identity AND from the instant it was
/// registered at, because a scalable target is a RESOURCE and not just a name:
/// deregistering it retires that resource, and registering the same identity again mints
/// a new one, which carries a new ARN and none of the retired target's tags or policies.
/// Deriving the id from the registration instant is what gives the new resource its own
/// identity while keeping the ARN stable for every read of the target that holds it.
pub fn scalable_target_arn(region: &str, key: &TargetKey, registered_at: SystemTime) -> String {
    let registered_nanos = registered_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = opaque_id(
        &[
            "scalable-target",
            &key.service_namespace,
            &key.resource_id,
            &key.scalable_dimension,
            &registered_nanos.to_string(),
        ]
        .join("\u{1f}"),
        36,
    );
    ::emulator_server::emulator_arn(
        "application-autoscaling",
        region,
        &format!("scalable-target/{id}"),
    )
}

/// A scaling policy's ARN, in the shape Application Auto Scaling documents:
/// `arn:aws:autoscaling:<region>:<account>:scalingPolicy:<id>:resource/<namespace>/<resource id>:policyName/<name>`.
pub fn scaling_policy_arn(region: &str, key: &PolicyKey) -> String {
    let id = uuid_shaped_id(
        &[
            "scalingPolicy",
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
        ]
        .join("\u{1f}"),
    );
    ::emulator_server::emulator_arn(
        "autoscaling",
        region,
        &format!(
            "scalingPolicy:{id}:resource/{}/{}:policyName/{}",
            key.target.service_namespace, key.target.resource_id, key.policy_name
        ),
    )
}

/// A scheduled action's ARN, in the shape Application Auto Scaling documents:
/// `arn:aws:autoscaling:<region>:<account>:scheduledAction:<id>:resource/<namespace>/<resource id>:scheduledActionName/<name>`.
///
/// Derived from the action's identity for the same reason the policy ARN is: re-putting
/// the same scheduled action must not hand the caller a second ARN.
pub fn scheduled_action_arn(region: &str, key: &ScheduledActionKey) -> String {
    let id = uuid_shaped_id(
        &[
            "scheduledAction",
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.scheduled_action_name,
        ]
        .join("\u{1f}"),
    );
    ::emulator_server::emulator_arn(
        "autoscaling",
        region,
        &format!(
            "scheduledAction:{id}:resource/{}/{}:scheduledActionName/{}",
            key.target.service_namespace, key.target.resource_id, key.scheduled_action_name
        ),
    )
}

/// A scaling activity's `ActivityId`.
///
/// Derived from the log sequence the activity was recorded at and the target it moved,
/// so it is unique (the sequence only grows), stable across reads, and reproducible
/// rather than per-run.
pub fn scaling_activity_id(sequence: u64, key: &TargetKey) -> String {
    uuid_shaped_id(
        &[
            "scalingActivity",
            &sequence.to_string(),
            &key.service_namespace,
            &key.resource_id,
            &key.scalable_dimension,
        ]
        .join("\u{1f}"),
    )
}

/// The 8-4-4-4-12 identifier that sits inside an `autoscaling` sub-resource ARN,
/// derived from `seed` so it is reproducible rather than per-run.
fn uuid_shaped_id(seed: &str) -> String {
    let digest = opaque_id(seed, 32);
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32],
    )
}

/// The service-linked role Application Auto Scaling uses for a namespace when the caller
/// names no `RoleARN`: the model documents that "if the service supports service-linked
/// roles, Application Auto Scaling uses a service-linked role, which it creates if it
/// does not yet exist".
///
/// The role is one PER NAMESPACE, not one per account: it is assumed through that
/// namespace's own service principal and named for the resource type it scales, so the
/// name carries that type as a suffix. See [`service_linked_role_name`].
pub fn service_linked_role_arn(service_namespace: &str) -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/{service_namespace}.application-autoscaling.amazonaws.com/{}",
            service_linked_role_name(service_namespace)
        ),
    )
}

/// The base name every Application Auto Scaling service-linked role starts with.
const SERVICE_LINKED_ROLE_BASE: &str = "AWSServiceRoleForApplicationAutoScaling";

/// The resource-type suffix each namespace's service-linked role name carries.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto
/// Scaling User Guide, which publishes one role per supported service, each named
/// `AWSServiceRoleForApplicationAutoScaling_<suffix>`. The rows are the model's own
/// `ServiceNamespace` value set (`SERVICE_NAMESPACE_VALUES`), so every modelled namespace
/// has one; a namespace outside the enum — which every handler refuses before reaching
/// here — falls back to the unsuffixed base name rather than to a guessed suffix.
const SERVICE_LINKED_ROLE_SUFFIXES: &[(&str, &str)] = &[
    ("appstream", "AppStreamFleet"),
    ("cassandra", "CassandraTable"),
    ("comprehend", "ComprehendEndpoint"),
    ("custom-resource", "CustomResource"),
    ("dynamodb", "DynamoDBTable"),
    ("ec2", "EC2SpotFleetRequest"),
    ("ecs", "ECSService"),
    ("elasticache", "ElastiCacheRG"),
    ("elasticmapreduce", "EMRInstanceGroup"),
    ("kafka", "KafkaCluster"),
    ("lambda", "LambdaConcurrency"),
    ("neptune", "NeptuneCluster"),
    ("rds", "RDSCluster"),
    ("sagemaker", "SageMakerEndpoint"),
    ("workspaces", "WorkSpacesPool"),
];

/// The name of the service-linked role a namespace's scalable targets are scaled through.
pub fn service_linked_role_name(service_namespace: &str) -> String {
    match SERVICE_LINKED_ROLE_SUFFIXES
        .iter()
        .find(|(namespace, _)| *namespace == service_namespace)
    {
        Some((_, suffix)) => format!("{SERVICE_LINKED_ROLE_BASE}_{suffix}"),
        None => SERVICE_LINKED_ROLE_BASE.to_string(),
    }
}

/// The smallest capacity a scalable dimension can be registered with.
///
/// A scalable dimension's capacity is the owning service's own provisioning unit, so its
/// floor is that service's documented minimum rather than zero for everything: the Amazon
/// DynamoDB Developer Guide ("Managing settings on DynamoDB provisioned capacity tables":
/// the smallest provisionable read or write capacity is one unit) and the Amazon Keyspaces
/// Developer Guide (the same for a provisioned-throughput table) both put that floor at
/// one unit, so auto scaling cannot be asked to take such a resource below it. A dimension
/// whose owning service documents no floor — a desired count, a replica count, a node
/// count — keeps zero, which is what lets a target scale all the way in.
const MIN_CAPACITY_FLOORS: &[(&str, i32)] = &[
    ("cassandra:table:ReadCapacityUnits", 1),
    ("cassandra:table:WriteCapacityUnits", 1),
    ("dynamodb:index:ReadCapacityUnits", 1),
    ("dynamodb:index:WriteCapacityUnits", 1),
    ("dynamodb:table:ReadCapacityUnits", 1),
    ("dynamodb:table:WriteCapacityUnits", 1),
];

/// The smallest capacity `scalable_dimension` accepts. See [`MIN_CAPACITY_FLOORS`].
pub fn min_capacity_floor(scalable_dimension: &str) -> i32 {
    MIN_CAPACITY_FLOORS
        .iter()
        .find(|(dimension, _)| *dimension == scalable_dimension)
        .map_or(0, |(_, floor)| *floor)
}

/// `len` lowercase hex characters derived from `seed`, clamped to the 64 characters a
/// SHA-256 digest renders to so a caller asking for more gets the whole digest rather
/// than a panic.
fn opaque_id(seed: &str, len: usize) -> String {
    use sha2::Digest;
    let digest = hex::encode(sha2::Sha256::digest(seed.as_bytes()));
    debug_assert!(
        len <= digest.len(),
        "an opaque id cannot be longer than the {} characters of a SHA-256 digest",
        digest.len()
    );
    digest[..len.min(digest.len())].to_string()
}

// ---------------------------------------------------------------------------
// Output shapes, rebuilt from the records
// ---------------------------------------------------------------------------

/// A `SystemTime` as the modelled timestamp type.
pub fn timestamp_of(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The `ScalableTarget` a `Describe` returns for a stored record.
pub fn scalable_target_of(key: &TargetKey, record: &ScalableTargetRecord) -> ScalableTarget {
    ScalableTarget {
        service_namespace: key.service_namespace.clone(),
        resource_id: key.resource_id.clone(),
        scalable_dimension: key.scalable_dimension.clone(),
        min_capacity: record.min_capacity,
        max_capacity: record.max_capacity,
        // Only a predictive-scaling policy produces a predicted capacity, and this
        // emulator forecasts none, so the member stays absent rather than invented.
        predicted_capacity: None,
        role_arn: record.role_arn.clone(),
        creation_time: timestamp_of(record.creation_time),
        suspended_state: record.suspended_state.clone(),
        scalable_target_arn: Some(record.arn.clone()),
    }
}

/// The `ScalingPolicy` a `Describe` returns for a stored record.
pub fn scaling_policy_of(key: &PolicyKey, record: &ScalingPolicyRecord) -> ScalingPolicy {
    ScalingPolicy {
        policy_arn: record.policy_arn.clone(),
        policy_name: key.policy_name.clone(),
        service_namespace: key.target.service_namespace.clone(),
        resource_id: key.target.resource_id.clone(),
        scalable_dimension: key.target.scalable_dimension.clone(),
        policy_type: record.policy_type.clone(),
        step_scaling_policy_configuration: record.step_config.clone(),
        target_tracking_scaling_policy_configuration: record.target_tracking_config.clone(),
        predictive_scaling_policy_configuration: record.predictive_config.clone(),
        alarms: Some(record.alarms.clone()),
        creation_time: timestamp_of(record.creation_time),
    }
}

/// The capacity a scalable dimension names, in the words a scaling activity's text uses.
///
/// A dimension "consists of the service namespace, resource type, and scaling property"
/// (the model's own documentation for the member), so the property is its last segment,
/// and splitting that at its word boundaries gives the noun the model's own
/// `DescribeScalingActivities` example carries: `ecs:service:DesiredCount` yields
/// "desired count", as in "Setting desired count to 1.".
pub fn capacity_noun(scalable_dimension: &str) -> String {
    let property = scalable_dimension
        .rsplit(':')
        .next()
        .unwrap_or(scalable_dimension);
    let mut words: Vec<String> = Vec::new();
    for character in property.chars() {
        match words.last_mut() {
            Some(word) if !character.is_uppercase() => word.push(character),
            _ => words.push(character.to_lowercase().to_string()),
        }
    }
    words.join(" ")
}

/// The `ScalingActivity` a `Describe` returns for a stored record, as of `now`.
///
/// An activity whose fulfilment instant has not arrived yet is reported in its
/// non-terminal status with no `EndTime`; from that instant on it reports the terminal
/// status it was recorded with, and the `EndTime` is that instant — so the same record
/// answers the same thing to every reader at the same clock reading.
pub fn scaling_activity_of(record: &ScalingActivityRecord, now: SystemTime) -> ScalingActivity {
    let pending = record.fulfilled_at.filter(|due| *due > now);
    ScalingActivity {
        activity_id: record.activity_id.clone(),
        service_namespace: record.target.service_namespace.clone(),
        resource_id: record.target.resource_id.clone(),
        scalable_dimension: record.target.scalable_dimension.clone(),
        description: record.description.clone(),
        cause: record.cause.clone(),
        start_time: timestamp_of(record.start_time),
        end_time: match pending {
            Some(_) => None,
            None => record.fulfilled_at.map(timestamp_of),
        },
        status_code: match pending {
            Some(_) => ACTIVITY_IN_PROGRESS.to_string(),
            None => record.terminal_status_code.clone(),
        },
        status_message: match pending {
            Some(_) => None,
            None => record.terminal_status_message.clone(),
        },
        details: record.details.clone(),
        // The member is "only available when IncludeNotScaledActivities is true" AND
        // the activity is one that was not scaled; an activity that scaled carries none.
        not_scaled_reasons: (!record.not_scaled_reasons.is_empty())
            .then(|| record.not_scaled_reasons.clone()),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `MaxResults`' documented bounds and default: "This value can be between 1 and 50. The
/// default value is 50."
pub const MIN_MAX_RESULTS: i32 = 1;
/// See [`MIN_MAX_RESULTS`].
pub const MAX_MAX_RESULTS: i32 = 50;
/// See [`MIN_MAX_RESULTS`].
pub const DEFAULT_MAX_RESULTS: i32 = 50;

/// The page size a `Describe` serves, or the `ValidationException` message for a
/// `MaxResults` outside its documented range.
pub fn page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
    if !(MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: \
             Member must be between {MIN_MAX_RESULTS} and {MAX_MAX_RESULTS}"
        ));
    }
    Ok(requested as usize)
}

/// The field separator inside a `NextToken`'s payload. A unit separator cannot appear in
/// a namespace, a resource id, a scalable dimension or a policy name, so splitting on it
/// recovers exactly the fields that were encoded.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque `NextToken` carrying the key a page ended on.
pub fn encode_next_token(fields: &[&str]) -> String {
    hex::encode(fields.join(&TOKEN_SEPARATOR.to_string()))
}

/// The fields a `NextToken` carries, or `None` when the token is not one this service
/// issued — the caller answers that with `InvalidNextTokenException`.
pub fn decode_next_token(token: &str, field_count: usize) -> Option<Vec<String>> {
    let bytes = hex::decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let fields: Vec<String> = text
        .split(TOKEN_SEPARATOR)
        .map(ToString::to_string)
        .collect();
    (fields.len() == field_count).then_some(fields)
}

/// Cut `items` (already filtered and in key order) down to one page.
///
/// Returns the page and, only when items remain beyond it, the `NextToken` that resumes
/// after the last item emitted — so the final page carries no token and a client's loop
/// terminates.
pub fn take_page<T>(
    mut items: Vec<T>,
    page_size: usize,
    token_of: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    if items.len() <= page_size {
        return (items, None);
    }
    items.truncate(page_size);
    let token = items.last().map(&token_of);
    (items, token)
}

// ---------------------------------------------------------------------------
// The service's own refusal texts
// ---------------------------------------------------------------------------
//
// A refusal this SERVICE reaches (rather than one the protocol's boundary validator
// reaches from a modelled constraint) carries the service's own sentence: a statement of
// the rule that was broken, naming the request's own values where the rule is about them.
// It is never the boundary validator's "Value 'X' at 'member' failed to satisfy
// constraint: Member must …" template, which belongs to the modelled constraints the
// framework enforces, and it never names a value the caller did not supply (the request's
// region, say). The texts below are the ones several handlers share; a text only one
// handler reaches lives beside it.

/// The message every refusal of an inverted capacity window carries.
///
/// A capacity window is refused for ONE reason — "scaling policies will not scale capacity
/// to values that are outside of the minimum and maximum range", which is not a range when
/// the bounds are inverted — so every operation that takes a window answers it with the
/// same sentence: `RegisterScalableTarget`'s `MinCapacity`/`MaxCapacity` pair and
/// `PutScheduledAction`'s `ScalableTargetAction` alike. Also the text the sibling Auto
/// Scaling Plans service answers a `ScalingInstruction` with, for the same rule.
pub const CAPACITY_RANGE_INVERTED_MESSAGE: &str =
    "Maximum capacity cannot be less than minimum capacity";

/// The message a request addressed to an ARN that names no registered scalable target
/// carries.
///
/// The resource a tag operation addresses is the scalable target its `ResourceARN` names,
/// and either that target is registered or it is not: the refusal states exactly that, and
/// the ARN asked about is reported in the modelled `ResourceName` member rather than spliced
/// into the sentence.
pub const SCALABLE_TARGET_NOT_FOUND_MESSAGE: &str = "Scalable Target not found";

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// `Ok` when `value` is one of `allowed`, else the `ValidationException` message naming
/// the member, the offending value and the permitted set — the shape AWS's own
/// constraint refusals take.
pub fn check_enum(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    ))
}

/// `Ok` when a `Describe`'s dimension filter is accompanied by the resource it narrows.
///
/// The model documents the rule on the member itself: "If you specify a scalable
/// dimension, you must also specify a resource ID." A dimension names a capacity
/// property of ONE resource, so filtering on it alone is a filter with no subject.
pub fn check_dimension_requires_resource_id(
    resource_id: Option<&str>,
    scalable_dimension: Option<&str>,
) -> Result<(), String> {
    match (resource_id, scalable_dimension) {
        (None, Some(dimension)) => Err(format!(
            "Value '{dimension}' at 'scalableDimension' failed to satisfy constraint: \
             A resource ID must be specified when a scalable dimension is specified"
        )),
        _ => Ok(()),
    }
}

/// `Ok` when `scalable_dimension` belongs to `service_namespace`.
///
/// A scalable dimension is not independent of the namespace it is named with: the
/// model's own documentation for the member says "this string consists of the service
/// namespace, resource type, and scaling property", so every value in the
/// `ScalableDimension` enum is `<service namespace>:<resource type>:<scaling property>`
/// and the pair is checkable from the dimension itself rather than from a hand-written
/// table. The two halves being individually legal is not enough — a dimension belongs to
/// exactly one namespace.
pub fn check_dimension_namespace(
    service_namespace: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    if scalable_dimension.split(':').next() == Some(service_namespace) {
        return Ok(());
    }
    Err(format!(
        "Value '{scalable_dimension}' at 'scalableDimension' failed to satisfy constraint: \
         Member must be a scalable dimension of the '{service_namespace}' service namespace"
    ))
}

/// The resource type a `ScalableDimension` names: its middle segment.
pub fn dimension_resource_type(scalable_dimension: &str) -> &str {
    scalable_dimension.split(':').nth(1).unwrap_or_default()
}

/// The scaling property a `ScalableDimension` names: its last segment.
pub fn dimension_scaling_property(scalable_dimension: &str) -> &str {
    scalable_dimension.split(':').nth(2).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The ResourceId a ScalableDimension requires
// ---------------------------------------------------------------------------

/// The `ResourceId` form each (service namespace, resource type) pair documents.
///
/// `ResourceId` and `ScalableDimension` are not independent members: the dimension "consists
/// of the service namespace, resource type, and scaling property" and the resource id
/// "consists of the resource type and unique identifier", so the resource type the dimension
/// names fixes the shape the resource id has to take. Every row is the example the model's own
/// `ResourceId` documentation gives for that resource type
/// (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#RegisterScalableTargetRequest",member="ResourceId")`),
/// rewritten as a template: `{...}` is one non-empty segment carrying neither `:` nor `/`,
/// and `{...*}` is the whole non-empty remainder.
///
/// `custom-resource` has no row on purpose — the same documentation says "Custom resources are
/// not supported with a resource type […] The unique identifier is defined by the service
/// provider", so there is no shape to check.
const RESOURCE_ID_FORMS: &[(&str, &str, &str)] = &[
    ("appstream", "fleet", "fleet/{fleet name}"),
    ("cassandra", "table", "keyspace/{keyspace}/table/{table}"),
    (
        "comprehend",
        "document-classifier-endpoint",
        "arn:{partition}:comprehend:{region}:{account}:document-classifier-endpoint/{name*}",
    ),
    (
        "comprehend",
        "entity-recognizer-endpoint",
        "arn:{partition}:comprehend:{region}:{account}:entity-recognizer-endpoint/{name*}",
    ),
    ("dynamodb", "table", "table/{table name}"),
    ("dynamodb", "index", "table/{table name}/index/{index name}"),
    ("ec2", "spot-fleet-request", "spot-fleet-request/{request id}"),
    ("ecs", "service", "service/{cluster name}/{service name}"),
    ("elasticache", "cache-cluster", "cache-cluster/{cluster name}"),
    (
        "elasticache",
        "replication-group",
        "replication-group/{replication group name}",
    ),
    (
        "elasticmapreduce",
        "instancegroup",
        "instancegroup/{cluster id}/{instance group id}",
    ),
    (
        "kafka",
        "broker-storage",
        "arn:{partition}:kafka:{region}:{account}:cluster/{name*}",
    ),
    (
        "lambda",
        "function",
        "function:{function name}:{version or alias}",
    ),
    ("neptune", "cluster", "cluster:{cluster name}"),
    ("rds", "cluster", "cluster:{cluster name}"),
    (
        "sagemaker",
        "inference-component",
        "inference-component/{inference component name}",
    ),
    (
        "sagemaker",
        "variant",
        "endpoint/{endpoint name}/variant/{variant name}",
    ),
    ("workspaces", "workspacespool", "workspacespool/{pool id}"),
];

/// `Ok` when `resource_id` has the form the resource type in `scalable_dimension` documents,
/// else the `ValidationException` message naming the resource id, the dimension it was paired
/// with and the form that pairing requires.
pub fn check_resource_id_form(
    service_namespace: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    let resource_type = dimension_resource_type(scalable_dimension);
    let Some((_, _, template)) = RESOURCE_ID_FORMS
        .iter()
        .find(|(namespace, candidate, _)| *namespace == service_namespace && *candidate == resource_type)
    else {
        return Ok(());
    };
    if matches_resource_id_form(template, resource_id) {
        return Ok(());
    }
    Err(format!(
        "ResourceId not valid for ScalableDimension {scalable_dimension}: {resource_id} must \
         name a resource of type {resource_type} in the {service_namespace} service namespace, \
         of the form {template}"
    ))
}

/// Whether `value` matches one [`RESOURCE_ID_FORMS`] template.
fn matches_resource_id_form(template: &str, value: &str) -> bool {
    /// The characters that separate a resource id's segments, so a `{...}` placeholder
    /// stands for exactly one segment rather than swallowing the rest of the id.
    const SEPARATORS: [char; 2] = [':', '/'];

    let mut template = template;
    let mut value = value;
    while !template.is_empty() {
        if let Some(after_brace) = template.strip_prefix('{') {
            let Some(end) = after_brace.find('}') else {
                return false;
            };
            let whole_remainder = after_brace[..end].ends_with('*');
            template = &after_brace[end + 1..];
            if whole_remainder {
                // A `{...*}` placeholder is only ever the last thing in a template.
                return template.is_empty() && !value.is_empty();
            }
            let taken = match template.chars().next() {
                Some(delimiter) => match value.find(delimiter) {
                    Some(at) => at,
                    None => return false,
                },
                None => value.len(),
            };
            if taken == 0 || value[..taken].contains(SEPARATORS) {
                return false;
            }
            value = &value[taken..];
        } else {
            let literal_len = template.find('{').unwrap_or(template.len());
            let Some(rest) = value.strip_prefix(&template[..literal_len]) else {
                return false;
            };
            value = rest;
            template = &template[literal_len..];
        }
    }
    value.is_empty()
}

// ---------------------------------------------------------------------------
// The resource a scalable target names, resolved in the service that owns it
// ---------------------------------------------------------------------------
//
// LOCK SCOPE. The helpers below, and the alarm apply/retire helpers after them, take
// ANOTHER service's store (DynamoDB's, CloudWatch's). Every one of them is called with
// THIS service's store NOT held: a handler resolves the resource before it locks, and the
// alarm work is split into plan (under this lock) / apply (no lock) / record (under this
// lock again). So no request holds two of these stores at once, and there is no lock order
// to get wrong.

/// The provisioned capacity of the resource a scalable target names, resolved in the
/// service that OWNS that resource.
///
/// One arm per namespace whose resource store this emulator holds — the same "resolve the
/// resource" rule the `ResourceId` form rows express, in one place, so adding the next
/// namespace is adding an arm here rather than a branch at each call site:
///
/// * `Ok(Some(capacity))` — the resource resolved, and this is its capacity for the
///   property the dimension names;
/// * `Ok(None)` — this emulator holds no store for that namespace, so there is nothing to
///   resolve and the registration is answered on its own terms;
/// * `Err(message)` — the resource cannot carry this target, as a `ValidationException`
///   message naming the member and why.
pub fn resolve_target_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    target: &TargetKey,
) -> Result<Option<i64>, String> {
    match target.service_namespace.as_str() {
        "dynamodb" => resolve_dynamodb_capacity(
            engine,
            region,
            &target.resource_id,
            &target.scalable_dimension,
        ),
        _ => Ok(None),
    }
}

/// The DynamoDB table, and global secondary index, a `dynamodb` `ResourceId` names.
pub fn split_dynamodb_resource_id(resource_id: &str) -> Option<(&str, Option<&str>)> {
    let rest = resource_id.strip_prefix("table/")?;
    match rest.split_once("/index/") {
        Some((table, index)) => {
            (!table.is_empty() && !index.is_empty()).then_some((table, Some(index)))
        }
        None => (!rest.is_empty()).then_some((rest, None)),
    }
}

/// The provisioned capacity the DynamoDB store holds for what a `dynamodb` scalable target
/// names, or the `ValidationException` message for a resource that cannot carry that target.
///
/// A scalable target is a capacity dimension OF A RESOURCE, so registering one has to resolve
/// that resource in the service that owns it: a table that does not exist has no capacity to
/// scale, and neither has one in `PAY_PER_REQUEST` (on-demand) billing mode, whose throughput
/// "ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST" — there is
/// no provisioned value for a policy to move. The same holds one level down for a global
/// secondary index, which must exist on the table the resource id names.
///
/// This is the one namespace whose backing service this emulator can resolve. For a namespace
/// whose owning service holds no resource store here there is nothing to resolve, so the
/// registration is accepted on its own terms; extending the check is a matter of adding the
/// resolver, not of changing the rule.
pub fn resolve_dynamodb_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<Option<i64>, String> {
    use ::service_com_amazonaws_dynamodb_dynamodb_20120810 as dynamodb;

    let malformed = || {
        format!(
            "ResourceId not valid for the dynamodb service namespace: {resource_id} must name a \
             DynamoDB table or global secondary index"
        )
    };
    let (table_name, index_name) = split_dynamodb_resource_id(resource_id).ok_or_else(malformed)?;
    let Some(store) = dynamodb::state::service_state(engine) else {
        // Not the shared in-memory engine, so there is no store to resolve against: the
        // resource is UNRESOLVABLE rather than refused (a reason about this process, not
        // about the request) and rather than resolved to a capacity of zero.
        return Ok(None);
    };
    let store = store.lock();
    let now = SystemTime::now();
    // A DynamoDB table name is unique per REGION, which is why the store is keyed by it:
    // the target resolves against the region its own request is signed for.
    let table = store
        .table(region, table_name)
        .ok()
        .filter(|table| !matches!(table.status(now), dynamodb::state::TableStatus::Deleting))
        .ok_or_else(|| format!("DynamoDB table does not exist: {resource_id}"))?;
    if table.provisioned_throughput.is_none() {
        return Err(format!(
            "DynamoDB table {resource_id} is in {} billing mode and has no provisioned \
             throughput to scale",
            table.billing_mode
        ));
    }
    let reads = dimension_scaling_property(scalable_dimension) == "ReadCapacityUnits";
    match index_name {
        None => Ok(Some(table.provisioned_throughput.as_ref().map_or(
            0,
            |throughput| {
                if reads {
                    throughput.read_capacity_units
                } else {
                    throughput.write_capacity_units
                }
            },
        ))),
        Some(index_name) => {
            let index = table
                .global_secondary_indexes
                .iter()
                .find(|index| index.name == index_name)
                .ok_or_else(|| format!("DynamoDB index does not exist: {resource_id}"))?;
            Ok(Some(index.provisioned_throughput.as_ref().map_or(
                0,
                |throughput| {
                    if reads {
                        throughput.read_capacity_units
                    } else {
                        throughput.write_capacity_units
                    }
                },
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// The CloudWatch alarms a target-tracking policy owns
// ---------------------------------------------------------------------------

/// Which of a target-tracking policy's alarms this is.
///
/// A target-tracking policy is driven by alarms, not by the policy document: one alarm per
/// direction on the metric being tracked, and — for a resource whose tracked metric counts
/// capacity consumed rather than a percentage — one more per direction on the resource's
/// provisioned capacity, because the utilisation thresholds are only valid for the capacity
/// they were computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmKind {
    /// Scale out: the tracked metric is above the target.
    AlarmHigh,
    /// Scale in: the tracked metric is below the target.
    AlarmLow,
    /// Scale out: the resource's provisioned capacity moved up.
    ProvisionedCapacityHigh,
    /// Scale in: the resource's provisioned capacity moved down.
    ProvisionedCapacityLow,
}

impl AlarmKind {
    /// Every kind, so a sync pass can retire the alarms a policy no longer wants.
    pub const ALL: [Self; 4] = [
        Self::AlarmHigh,
        Self::AlarmLow,
        Self::ProvisionedCapacityHigh,
        Self::ProvisionedCapacityLow,
    ];

    /// The name segment this kind contributes to its alarm's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlarmHigh => "AlarmHigh",
            Self::AlarmLow => "AlarmLow",
            Self::ProvisionedCapacityHigh => "ProvisionedCapacityHigh",
            Self::ProvisionedCapacityLow => "ProvisionedCapacityLow",
        }
    }

    /// Whether this alarm drives scale-out (rather than scale-in).
    fn scales_out(self) -> bool {
        matches!(self, Self::AlarmHigh | Self::ProvisionedCapacityHigh)
    }

    /// Whether this alarm watches the resource's provisioned capacity rather than the
    /// tracked metric.
    fn watches_capacity(self) -> bool {
        matches!(
            self,
            Self::ProvisionedCapacityHigh | Self::ProvisionedCapacityLow
        )
    }
}

/// One alarm a target-tracking policy wants to exist in CloudWatch.
#[derive(Debug, Clone)]
pub struct AlarmSpec {
    pub kind: AlarmKind,
    pub name: String,
    pub metric_namespace: String,
    pub metric_name: String,
    pub statistic: String,
    pub dimensions: Vec<(String, String)>,
    pub period: i32,
    pub evaluation_periods: i32,
    pub threshold: f64,
    pub comparison_operator: String,
}

/// What CloudWatch metric a `PredefinedMetricType` tracks.
///
/// Hand-written from AWS's published "Predefined metrics for target tracking scaling
/// policies" table (Application Auto Scaling User Guide), one row per value of the model's
/// `MetricType` enum (`METRIC_TYPE_VALUES`): the namespace and metric name the alarms watch,
/// the statistic they aggregate with, and whether the metric counts units CONSUMED over the
/// alarm's period — the one case where the target utilisation is not itself the threshold.
struct PredefinedMetric {
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
    /// `true` when the metric is a count of capacity units consumed over the period, so a
    /// target expressed as a percentage of provisioned capacity becomes
    /// `target% * period * provisioned units`; `false` when the metric is already the
    /// quantity the target value is expressed in.
    consumed_units: bool,
    /// The alarm period, in seconds. 60 for every metric but the high-resolution ones,
    /// which the model documents as using "20-second CloudWatch metrics […] evaluate
    /// metrics every 20 seconds".
    period: i32,
    /// The `ScalableDimension` values this metric is published FOR.
    ///
    /// A predefined metric is a metric of ONE service's resources, so it only describes
    /// that service's dimensions: the published table above names the service (and, where
    /// it splits them, the capacity property) each metric belongs to, and the model itself
    /// says of the one metric that spans two services that "ALBRequestCountPerTarget -
    /// This metric type applies only to Spot Fleets and ECS services". Where the guide
    /// publishes a metric for a service without splitting it by capacity property, every
    /// dimension of that service is listed rather than a finer split being invented.
    dimensions: &'static [&'static str],
    /// The window a `TargetValue` must lie in, where the owning service publishes one.
    ///
    /// `None` where it publishes none: the target is then expressed in whatever the metric
    /// is measured in (a request count per target, a connection count) and nothing bounds
    /// it. `Some` for the provisioned-throughput utilisation metrics, whose target is a
    /// percentage of provisioned capacity inside a published window.
    target_range: Option<(f64, f64)>,
}

/// DynamoDB's provisioned READ capacity: a table's, and a global secondary index's.
const DYNAMODB_READ: &[&str] = &[
    "dynamodb:table:ReadCapacityUnits",
    "dynamodb:index:ReadCapacityUnits",
];
/// DynamoDB's provisioned WRITE capacity.
const DYNAMODB_WRITE: &[&str] = &[
    "dynamodb:table:WriteCapacityUnits",
    "dynamodb:index:WriteCapacityUnits",
];
/// Amazon Keyspaces' provisioned read capacity.
const CASSANDRA_READ: &[&str] = &["cassandra:table:ReadCapacityUnits"];
/// Amazon Keyspaces' provisioned write capacity.
const CASSANDRA_WRITE: &[&str] = &["cassandra:table:WriteCapacityUnits"];
/// An ECS service's task count.
const ECS_SERVICE: &[&str] = &["ecs:service:DesiredCount"];
/// A Spot Fleet request's target capacity.
const SPOT_FLEET: &[&str] = &["ec2:spot-fleet-request:TargetCapacity"];
/// "ALBRequestCountPerTarget - This metric type applies only to Spot Fleets and ECS
/// services" (`PredefinedMetricSpecification$PredefinedMetricType`).
const ALB_REQUEST_COUNT: &[&str] = &[
    "ecs:service:DesiredCount",
    "ec2:spot-fleet-request:TargetCapacity",
];
/// An Aurora read-replica count.
const RDS_CLUSTER: &[&str] = &["rds:cluster:ReadReplicaCount"];
/// A Neptune read-replica count.
const NEPTUNE_CLUSTER: &[&str] = &["neptune:cluster:ReadReplicaCount"];
/// An AppStream 2.0 fleet's capacity.
const APPSTREAM_FLEET: &[&str] = &["appstream:fleet:DesiredCapacity"];
/// Comprehend's two endpoint kinds, which publish the same inference metric.
const COMPREHEND_ENDPOINTS: &[&str] = &[
    "comprehend:document-classifier-endpoint:DesiredInferenceUnits",
    "comprehend:entity-recognizer-endpoint:DesiredInferenceUnits",
];
/// A Lambda function's provisioned concurrency.
const LAMBDA_CONCURRENCY: &[&str] = &["lambda:function:ProvisionedConcurrency"];
/// An MSK cluster's broker storage.
const KAFKA_STORAGE: &[&str] = &["kafka:broker-storage:VolumeSize"];
/// Every ElastiCache dimension: the guide publishes the engine-CPU and memory metrics for
/// the service without splitting them by capacity property.
const ELASTICACHE: &[&str] = &[
    "elasticache:cache-cluster:Nodes",
    "elasticache:replication-group:NodeGroups",
    "elasticache:replication-group:Replicas",
];
/// A SageMaker endpoint variant's instance count.
const SAGEMAKER_VARIANT_INSTANCES: &[&str] = &["sagemaker:variant:DesiredInstanceCount"];
/// A SageMaker endpoint variant's provisioned concurrency.
const SAGEMAKER_VARIANT_CONCURRENCY: &[&str] = &["sagemaker:variant:DesiredProvisionedConcurrency"];
/// A SageMaker inference component's copy count.
const SAGEMAKER_INFERENCE_COMPONENT: &[&str] = &["sagemaker:inference-component:DesiredCopyCount"];
/// A WorkSpaces pool's user sessions.
const WORKSPACES_POOL: &[&str] = &["workspaces:workspacespool:DesiredUserSessions"];

/// The window a provisioned-throughput utilisation target must lie in.
///
/// Both services whose scalable dimension IS a provisioned read/write throughput publish
/// the same one: the Amazon DynamoDB Developer Guide ("Managing throughput capacity
/// automatically with DynamoDB auto scaling": a target utilisation between 20 and 90
/// percent) and the Amazon Keyspaces Developer Guide for a provisioned-throughput table.
const PROVISIONED_UTILIZATION_RANGE: (f64, f64) = (20.0, 90.0);

const PREDEFINED_METRICS: &[PredefinedMetric] = &[
    metric("DynamoDBReadCapacityUtilization", "AWS/DynamoDB", "ConsumedReadCapacityUnits", "Sum", true, 60, DYNAMODB_READ, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("DynamoDBWriteCapacityUtilization", "AWS/DynamoDB", "ConsumedWriteCapacityUnits", "Sum", true, 60, DYNAMODB_WRITE, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("CassandraReadCapacityUtilization", "AWS/Cassandra", "ConsumedReadCapacityUnits", "Sum", true, 60, CASSANDRA_READ, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("CassandraWriteCapacityUtilization", "AWS/Cassandra", "ConsumedWriteCapacityUnits", "Sum", true, 60, CASSANDRA_WRITE, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("ALBRequestCountPerTarget", "AWS/ApplicationELB", "RequestCountPerTarget", "Sum", false, 60, ALB_REQUEST_COUNT, None),
    metric("ECSServiceAverageCPUUtilization", "AWS/ECS", "CPUUtilization", "Average", false, 60, ECS_SERVICE, None),
    metric("ECSServiceAverageMemoryUtilization", "AWS/ECS", "MemoryUtilization", "Average", false, 60, ECS_SERVICE, None),
    metric("ECSServiceAverageCPUUtilizationHighResolution", "AWS/ECS", "CPUUtilization", "Average", false, 20, ECS_SERVICE, None),
    metric("ECSServiceAverageMemoryUtilizationHighResolution", "AWS/ECS", "MemoryUtilization", "Average", false, 20, ECS_SERVICE, None),
    metric("EC2SpotFleetRequestAverageCPUUtilization", "AWS/EC2Spot", "CPUUtilization", "Average", false, 60, SPOT_FLEET, None),
    metric("EC2SpotFleetRequestAverageNetworkIn", "AWS/EC2Spot", "NetworkIn", "Average", false, 60, SPOT_FLEET, None),
    metric("EC2SpotFleetRequestAverageNetworkOut", "AWS/EC2Spot", "NetworkOut", "Average", false, 60, SPOT_FLEET, None),
    metric("RDSReaderAverageCPUUtilization", "AWS/RDS", "CPUUtilization", "Average", false, 60, RDS_CLUSTER, None),
    metric("RDSReaderAverageDatabaseConnections", "AWS/RDS", "DatabaseConnections", "Average", false, 60, RDS_CLUSTER, None),
    metric("NeptuneReaderAverageCPUUtilization", "AWS/Neptune", "CPUUtilization", "Average", false, 60, NEPTUNE_CLUSTER, None),
    metric("AppStreamAverageCapacityUtilization", "AWS/AppStream", "CapacityUtilization", "Average", false, 60, APPSTREAM_FLEET, None),
    metric("ComprehendInferenceUtilization", "AWS/Comprehend", "InferenceUtilization", "Average", false, 60, COMPREHEND_ENDPOINTS, None),
    metric("LambdaProvisionedConcurrencyUtilization", "AWS/Lambda", "ProvisionedConcurrencyUtilization", "Average", false, 60, LAMBDA_CONCURRENCY, None),
    metric("KafkaBrokerStorageUtilization", "AWS/Kafka", "KafkaDataLogsDiskUsed", "Average", false, 60, KAFKA_STORAGE, None),
    metric("ElastiCacheEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCachePrimaryEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheReplicaEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseMemoryUsagePercentage", "AWS/ElastiCache", "DatabaseMemoryUsagePercentage", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseMemoryUsageCountedForEvictPercentage", "AWS/ElastiCache", "DatabaseMemoryUsageCountedForEvictPercentage", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseCapacityUsageCountedForEvictPercentage", "AWS/ElastiCache", "DatabaseCapacityUsageCountedForEvictPercentage", "Average", false, 60, ELASTICACHE, None),
    metric("SageMakerVariantInvocationsPerInstance", "AWS/SageMaker", "InvocationsPerInstance", "Sum", false, 60, SAGEMAKER_VARIANT_INSTANCES, None),
    metric("SageMakerVariantProvisionedConcurrencyUtilization", "AWS/SageMaker", "ProvisionedConcurrencyUtilization", "Average", false, 60, SAGEMAKER_VARIANT_CONCURRENCY, None),
    metric("SageMakerVariantConcurrentRequestsPerModelHighResolution", "AWS/SageMaker", "ConcurrentRequestsPerModel", "Average", false, 20, SAGEMAKER_VARIANT_INSTANCES, None),
    metric("SageMakerInferenceComponentInvocationsPerCopy", "AWS/SageMaker", "InvocationsPerCopy", "Sum", false, 60, SAGEMAKER_INFERENCE_COMPONENT, None),
    metric("SageMakerInferenceComponentConcurrentRequestsPerCopyHighResolution", "AWS/SageMaker", "ConcurrentRequestsPerCopy", "Average", false, 20, SAGEMAKER_INFERENCE_COMPONENT, None),
    metric("WorkSpacesAverageUserSessionsCapacityUtilization", "AWS/WorkSpaces", "UserSessionsCapacityUtilization", "Average", false, 60, WORKSPACES_POOL, None),
];

/// One [`PREDEFINED_METRICS`] row, so the table above reads as a table.
#[allow(clippy::too_many_arguments)]
const fn metric(
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
    consumed_units: bool,
    period: i32,
    dimensions: &'static [&'static str],
    target_range: Option<(f64, f64)>,
) -> PredefinedMetric {
    PredefinedMetric {
        metric_type,
        namespace,
        metric_name,
        statistic,
        consumed_units,
        period,
        dimensions,
        target_range,
    }
}

/// The metric a `PredefinedMetricType` tracks, when it is one this emulator knows.
fn predefined_metric(metric_type: &str) -> Option<&'static PredefinedMetric> {
    PREDEFINED_METRICS
        .iter()
        .find(|metric| metric.metric_type == metric_type)
}

/// The `ScalableDimension` values `metric_type` is published for, when it is a metric this
/// service scales on. See [`PredefinedMetric::dimensions`].
pub fn predefined_metric_dimensions(metric_type: &str) -> Option<&'static [&'static str]> {
    predefined_metric(metric_type).map(|metric| metric.dimensions)
}

/// The window `metric_type`'s target value must lie in, where its owning service publishes
/// one. See [`PredefinedMetric::target_range`].
pub fn predefined_metric_target_range(metric_type: &str) -> Option<(f64, f64)> {
    predefined_metric(metric_type).and_then(|metric| metric.target_range)
}

/// Whether a target-tracking policy on `scalable_dimension` may track a CUSTOMIZED metric.
///
/// A customized metric is a series of the caller's own choosing, which presupposes that the
/// resource's capacity can be driven from an arbitrary signal. The two services whose
/// scalable dimension IS a provisioned read/write throughput — DynamoDB and Amazon
/// Keyspaces — scale it from their own consumed-capacity utilisation and from nothing else,
/// and refuse a customized specification (recorded AWS behaviour: a customized metric on a
/// DynamoDB dimension is answered 400 `ValidationException`).
pub fn customized_metric_supported(scalable_dimension: &str) -> bool {
    !matches!(
        scalable_dimension.split(':').next(),
        Some("dynamodb" | "cassandra")
    )
}

/// The METRIC a target-tracking configuration tracks, as a comparable key.
///
/// Two configurations have the same signature when they track the same series: the same
/// predefined metric type (with the same resource label, which is part of WHICH series a
/// per-target metric is), or the same customized namespace, metric name, statistic, unit
/// and dimension set. The target value is deliberately not part of it — two policies
/// steering one metric towards two values are exactly the conflict the uniqueness rule
/// refuses.
pub fn metric_signature(config: &TargetTrackingScalingPolicyConfiguration) -> String {
    if let Some(predefined) = &config.predefined_metric_specification {
        return format!(
            "predefined\u{1f}{}\u{1f}{}",
            predefined.predefined_metric_type,
            predefined.resource_label.as_deref().unwrap_or_default()
        );
    }
    match &config.customized_metric_specification {
        Some(customized) => {
            let mut dimensions: Vec<String> = customized
                .dimensions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|dimension| format!("{}={}", dimension.name, dimension.value))
                .collect();
            dimensions.sort();
            format!(
                "customized\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                customized.namespace.as_deref().unwrap_or_default(),
                customized.metric_name.as_deref().unwrap_or_default(),
                customized.statistic.as_deref().unwrap_or_default(),
                customized.unit.as_deref().unwrap_or_default(),
                dimensions.join(",")
            )
        }
        // A configuration with neither specification is refused before this is reached; it
        // tracks no series at all.
        None => "none".to_string(),
    }
}

/// How long an alarm's metric has to stay past its threshold before the alarm fires, as
/// a number of periods.
///
/// Chosen by this emulator, not a documented AWS value: a scale-out alarm reacts in two
/// periods and a scale-in alarm is deliberately slower, the asymmetry target tracking is
/// built on ("scaling in is more gradual than scaling out").
const SCALE_OUT_EVALUATION_PERIODS: i32 = 2;
const SCALE_IN_EVALUATION_PERIODS: i32 = 15;

/// The period of the two alarms that watch the resource's provisioned capacity, and how
/// many of them must breach. Also this emulator's choice; the capacity of a resource moves
/// far less often than its utilisation, so these alarms are coarser.
const CAPACITY_ALARM_PERIOD: i32 = 300;
const CAPACITY_ALARM_EVALUATION_PERIODS: i32 = 3;

/// The fraction of the target utilisation the scale-in alarm sits at.
///
/// Target tracking does not scale in the moment utilisation dips below the target — it waits
/// for a margin below it, or every small dip would remove capacity that the next data point
/// asks back. The margin is 20% of the target.
const SCALE_IN_TARGET_FRACTION: f64 = 0.8;

/// The prefix every alarm a target-tracking policy mints carries, which is what lets a
/// client find them with `DescribeAlarms`' `AlarmNamePrefix`.
const ALARM_NAME_PREFIX: &str = "TargetTracking";

/// CloudWatch's `AlarmName` length bound ("len 1..255" on `PutMetricAlarm$AlarmName`), which
/// a resource id of up to 1600 characters would otherwise blow through.
const MAX_ALARM_NAME_LEN: usize = 255;

/// The name of one of a policy's alarms: the `TargetTracking` prefix, the resource the policy
/// scales, which of the policy's alarms this is, and an opaque id that makes the name unique
/// to this policy.
pub fn policy_alarm_name(key: &PolicyKey, kind: AlarmKind) -> String {
    let id = opaque_id(
        &[
            "alarm",
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
            kind.as_str(),
        ]
        .join("\u{1f}"),
        32,
    );
    let kind = kind.as_str();
    let suffix = format!("-{kind}-{id}");
    let room = MAX_ALARM_NAME_LEN
        .saturating_sub(ALARM_NAME_PREFIX.len() + 1 + suffix.len())
        .min(key.target.resource_id.len());
    let resource = &key.target.resource_id[..floor_char_boundary(&key.target.resource_id, room)];
    // A resource id long enough to leave no room at all is left out rather than rendered
    // as an empty segment between two separators.
    if resource.is_empty() {
        return format!("{ALARM_NAME_PREFIX}{suffix}");
    }
    format!("{ALARM_NAME_PREFIX}-{resource}{suffix}")
}

/// The largest index at or below `at` that `text` can be split on.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The CloudWatch dimensions that identify the resource a scalable target names.
///
/// One row per resource type this emulator can name, each from that service's own
/// CloudWatch metric dimensions: a DynamoDB table metric is dimensioned by `TableName` (and a
/// global secondary index additionally by `GlobalSecondaryIndexName`), an ECS service metric by
/// `ClusterName` and `ServiceName`, and so on. A resource type with no row contributes no
/// dimensions rather than a guessed one.
fn metric_dimensions(
    service_namespace: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Vec<(String, String)> {
    let named = |name: &str, value: &str| vec![(name.to_string(), value.to_string())];
    let segments: Vec<&str> = resource_id.split(['/', ':']).collect();
    match (service_namespace, dimension_resource_type(scalable_dimension)) {
        ("dynamodb", "table") => match split_dynamodb_resource_id(resource_id) {
            Some((table, _)) => named("TableName", table),
            None => Vec::new(),
        },
        ("dynamodb", "index") => match split_dynamodb_resource_id(resource_id) {
            Some((table, Some(index))) => vec![
                ("TableName".to_string(), table.to_string()),
                ("GlobalSecondaryIndexName".to_string(), index.to_string()),
            ],
            _ => Vec::new(),
        },
        ("ecs", "service") => match segments.as_slice() {
            ["service", cluster, service] => vec![
                ("ClusterName".to_string(), (*cluster).to_string()),
                ("ServiceName".to_string(), (*service).to_string()),
            ],
            _ => Vec::new(),
        },
        ("cassandra", "table") => match segments.as_slice() {
            ["keyspace", keyspace, "table", table] => vec![
                ("Keyspace".to_string(), (*keyspace).to_string()),
                ("TableName".to_string(), (*table).to_string()),
            ],
            _ => Vec::new(),
        },
        ("ec2", "spot-fleet-request") => segments
            .get(1)
            .map(|id| named("FleetRequestId", id))
            .unwrap_or_default(),
        ("elasticmapreduce", "instancegroup") => match segments.as_slice() {
            ["instancegroup", cluster, group] => vec![
                ("JobFlowId".to_string(), (*cluster).to_string()),
                ("InstanceGroupId".to_string(), (*group).to_string()),
            ],
            _ => Vec::new(),
        },
        ("appstream", "fleet") => segments
            .get(1)
            .map(|name| named("Fleet", name))
            .unwrap_or_default(),
        ("lambda", "function") => match segments.as_slice() {
            ["function", name, qualifier] => vec![
                ("FunctionName".to_string(), (*name).to_string()),
                (
                    "Resource".to_string(),
                    format!("{name}:{qualifier}"),
                ),
            ],
            _ => Vec::new(),
        },
        ("rds" | "neptune", "cluster") => segments
            .get(1)
            .map(|name| named("DBClusterIdentifier", name))
            .unwrap_or_default(),
        ("sagemaker", "variant") => match segments.as_slice() {
            ["endpoint", endpoint, "variant", variant] => vec![
                ("EndpointName".to_string(), (*endpoint).to_string()),
                ("VariantName".to_string(), (*variant).to_string()),
            ],
            _ => Vec::new(),
        },
        ("sagemaker", "inference-component") => segments
            .get(1)
            .map(|name| named("InferenceComponentName", name))
            .unwrap_or_default(),
        ("elasticache", "cache-cluster") => segments
            .get(1)
            .map(|name| named("CacheClusterId", name))
            .unwrap_or_default(),
        ("elasticache", "replication-group") => segments
            .get(1)
            .map(|name| named("ReplicationGroupId", name))
            .unwrap_or_default(),
        ("workspaces", "workspacespool") => segments
            .get(1)
            .map(|id| named("WorkspacesPoolId", id))
            .unwrap_or_default(),
        // An MSK target is named by its cluster ARN (`…:cluster/<name>/<uuid>`), and the
        // service's own metrics are dimensioned by that cluster's NAME.
        ("kafka", "broker-storage") => resource_id
            .split_once(":cluster/")
            .and_then(|(_, rest)| rest.split('/').next())
            .filter(|name| !name.is_empty())
            .map(|name| named("Cluster Name", name))
            .unwrap_or_default(),
        // A Comprehend endpoint is named by its own ARN, which is also the dimension its
        // metrics carry.
        ("comprehend", "document-classifier-endpoint" | "entity-recognizer-endpoint") => {
            named("EndpointArn", resource_id)
        }
        _ => Vec::new(),
    }
}

/// The alarms a target-tracking policy wants to exist right now.
///
/// Empty for any other policy type: only target tracking is alarm-driven.
///
/// `provisioned_capacity` is the resource's current provisioned capacity, which the
/// utilisation thresholds of a consumed-units metric are recomputed from on every pass — the
/// threshold is a function of the target, the alarm period and that capacity, never a copy of
/// the target value.
///
/// Which alarms are wanted is what `DisableScaleIn` and the target's `SuspendedState` decide:
/// scale-in is "disabled and the target tracking scaling policy won't remove capacity" for the
/// first, and for the second "while a suspension is in effect, all scale-in/scale-out
/// activities that are triggered by a scaling policy are suspended" — a suspended direction has
/// no alarm to trigger it, and resuming it brings the alarm back.
pub fn target_tracking_alarms(
    key: &PolicyKey,
    record: &ScalingPolicyRecord,
    suspended_state: Option<&SuspendedState>,
    provisioned_capacity: i64,
) -> Vec<AlarmSpec> {
    if record.policy_type != "TargetTrackingScaling" {
        return Vec::new();
    }
    let Some(config) = record.target_tracking_config.as_ref() else {
        return Vec::new();
    };

    let predefined = config
        .predefined_metric_specification
        .as_ref()
        .and_then(|specification| predefined_metric(&specification.predefined_metric_type));
    let customized = config.customized_metric_specification.as_ref();

    // The metric the two utilisation alarms watch: the predefined metric's own series, or
    // the one the caller described. A customised specification that carries only a metric
    // math query names no single series, so there is no static-threshold alarm to mint.
    let (metric_namespace, metric_name, statistic, dimensions, period, consumed_units) =
        match (predefined, customized) {
            (Some(metric), _) => (
                metric.namespace.to_string(),
                metric.metric_name.to_string(),
                metric.statistic.to_string(),
                metric_dimensions(
                    &key.target.service_namespace,
                    &key.target.resource_id,
                    &key.target.scalable_dimension,
                ),
                metric.period,
                metric.consumed_units,
            ),
            (None, Some(specification)) => {
                let (Some(namespace), Some(metric_name)) = (
                    specification.namespace.clone(),
                    specification.metric_name.clone(),
                ) else {
                    return Vec::new();
                };
                (
                    namespace,
                    metric_name,
                    specification
                        .statistic
                        .clone()
                        .unwrap_or_else(|| "Average".to_string()),
                    specification
                        .dimensions
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|dimension| (dimension.name, dimension.value))
                        .collect(),
                    60,
                    false,
                )
            }
            (None, None) => return Vec::new(),
        };

    let suspended_in = suspended_state
        .and_then(|state| state.dynamic_scaling_in_suspended)
        .unwrap_or(false);
    let suspended_out = suspended_state
        .and_then(|state| state.dynamic_scaling_out_suspended)
        .unwrap_or(false);
    let scale_in_disabled = config.disable_scale_in.unwrap_or(false) || suspended_in;

    // The utilisation threshold. For a metric that counts units consumed over the period,
    // the target is a percentage of the provisioned capacity, so the comparable value is
    // `target% * period * provisioned units`; for a metric that already is the quantity the
    // target names, the target IS the threshold.
    let utilisation_threshold = |fraction: f64| {
        if consumed_units {
            config.target_value / 100.0 * f64::from(period) * provisioned_capacity as f64 * fraction
        } else {
            config.target_value * fraction
        }
    };

    let mut alarms = Vec::new();
    for kind in AlarmKind::ALL {
        let withheld = if kind.scales_out() {
            suspended_out
        } else {
            scale_in_disabled
        };
        if withheld {
            continue;
        }
        // The provisioned-capacity pair exists only where the thresholds depend on the
        // capacity: it is what tells the policy its utilisation thresholds are stale.
        if kind.watches_capacity() && !consumed_units {
            continue;
        }
        let (metric_namespace, metric_name, statistic, period, threshold, comparison_operator) =
            if kind.watches_capacity() {
                (
                    metric_namespace.clone(),
                    provisioned_capacity_metric_name(&key.target.scalable_dimension),
                    "Average".to_string(),
                    CAPACITY_ALARM_PERIOD,
                    provisioned_capacity as f64,
                    if kind.scales_out() {
                        "GreaterThanThreshold"
                    } else {
                        "LessThanThreshold"
                    }
                    .to_string(),
                )
            } else {
                (
                    metric_namespace.clone(),
                    metric_name.clone(),
                    statistic.clone(),
                    period,
                    utilisation_threshold(if kind.scales_out() {
                        1.0
                    } else {
                        SCALE_IN_TARGET_FRACTION
                    }),
                    if kind.scales_out() {
                        "GreaterThanThreshold"
                    } else {
                        "LessThanThreshold"
                    }
                    .to_string(),
                )
            };
        alarms.push(AlarmSpec {
            kind,
            name: policy_alarm_name(key, kind),
            metric_namespace,
            metric_name,
            statistic,
            dimensions: dimensions.clone(),
            period,
            evaluation_periods: if kind.watches_capacity() {
                CAPACITY_ALARM_EVALUATION_PERIODS
            } else if kind.scales_out() {
                SCALE_OUT_EVALUATION_PERIODS
            } else {
                SCALE_IN_EVALUATION_PERIODS
            },
            threshold,
            comparison_operator,
        });
    }
    alarms
}

/// The metric that reports the provisioned capacity the utilisation thresholds were computed
/// from, for a dimension whose tracked metric counts consumed units.
fn provisioned_capacity_metric_name(scalable_dimension: &str) -> String {
    if dimension_scaling_property(scalable_dimension) == "ReadCapacityUnits" {
        "ProvisionedReadCapacityUnits".to_string()
    } else {
        "ProvisionedWriteCapacityUnits".to_string()
    }
}

/// The alarm work one target's policies need: what each policy's alarm set should be, and
/// under which ARN its alarms are wired.
///
/// A plan is computed under THIS service's lock and applied with NO store lock held, which
/// is what keeps a cross-service write out of this service's critical section.
#[derive(Debug, Default)]
pub struct AlarmPlan {
    /// The region the alarms' own ARNs are scoped to.
    pub region: String,
    /// One entry per policy of the target: its key, its ARN, and the alarms it wants.
    pub policies: Vec<(PolicyKey, String, Vec<AlarmSpec>)>,
}

/// STEP 1 of the alarm sync, under this service's lock ONLY: what every policy of `target`
/// wants its alarm set to be.
///
/// `resolved_capacity` is the resource's own provisioned capacity, read from the service
/// that owns it BEFORE this lock was taken. `None` falls back to the target's
/// `MinCapacity`, which Application Auto Scaling keeps a scalable resource at or above, in
/// two cases: a resource that cannot be read any more (deleted under the target), and a
/// namespace whose store this emulator cannot resolve at all — so a Keyspaces
/// (`cassandra`) target's consumed-units threshold stays pinned to `MinCapacity` until
/// that store has a resolver, which is a row in [`resolve_target_resource`].
pub fn plan_target_alarms(
    state: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    region: &str,
    target: &TargetKey,
    resolved_capacity: Option<i64>,
) -> AlarmPlan {
    let suspended_state = state
        .targets
        .get(target)
        .and_then(|record| record.suspended_state.clone());
    let provisioned_capacity = resolved_capacity.unwrap_or_else(|| {
        state
            .targets
            .get(target)
            .map_or(0, |record| i64::from(record.min_capacity))
    });
    let policies = state
        .policies
        .iter()
        .filter(|(key, _)| &key.target == target)
        .map(|(key, record)| {
            (
                key.clone(),
                record.policy_arn.clone(),
                target_tracking_alarms(
                    key,
                    record,
                    suspended_state.as_ref(),
                    provisioned_capacity,
                ),
            )
        })
        .collect();
    AlarmPlan {
        region: region.to_string(),
        policies,
    }
}

/// STEP 2, with NO store lock held: make CloudWatch hold exactly the alarms `plan` wants,
/// and answer what it now holds per policy.
pub fn apply_target_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    plan: &AlarmPlan,
) -> Vec<(PolicyKey, Vec<Alarm>)> {
    plan.policies
        .iter()
        .map(|(key, policy_arn, wanted)| {
            (
                key.clone(),
                apply_alarms(engine, &plan.region, key, policy_arn, wanted),
            )
        })
        .collect()
}

/// STEP 3, under this service's lock again: record on each policy the alarms CloudWatch now
/// holds for it, so `DescribeScalingPolicies` and `PutScalingPolicy` report them.
///
/// Answers the policies that are GONE — deleted while the lock was released — whose alarms
/// CloudWatch now holds for nothing. The caller retires those once it has dropped the lock
/// again, so the window the split opens cannot leave an orphaned alarm behind.
#[must_use]
pub fn record_minted_alarms(
    state: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    minted: Vec<(PolicyKey, Vec<Alarm>)>,
) -> Vec<PolicyKey> {
    let mut orphaned = Vec::new();
    for (key, alarms) in minted {
        match state.policies.get_mut(&key) {
            Some(record) => record.alarms = alarms,
            None if !alarms.is_empty() => orphaned.push(key),
            None => {}
        }
    }
    orphaned
}

/// Retire every alarm one policy owns, for a policy that is going away.
///
/// Call it with NO store lock held. Only a TARGET-TRACKING policy's alarms are its own:
/// "deleting a step scaling policy deletes the underlying alarm action, but does not delete
/// the CloudWatch alarm", so a step policy's alarms are the CALLER's and survive it — which
/// is why the caller passes the deleted policy's type.
pub fn retire_policy_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    key: &PolicyKey,
    policy_type: &str,
) {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

    if policy_type != "TargetTrackingScaling" {
        return;
    }
    let Some(store) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = store.lock();
    for kind in AlarmKind::ALL {
        store.alarms.remove(&policy_alarm_name(key, kind));
    }
}

/// Create or update every alarm in `wanted` in the CloudWatch store, delete the ones this
/// policy no longer wants, and answer the modelled `Alarm` list for the policy record.
fn apply_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key: &PolicyKey,
    policy_arn: &str,
    wanted: &[AlarmSpec],
) -> Vec<Alarm> {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
    use cloudwatch::ops::types::{Dimension, MetricAlarm};

    let Some(store) = cloudwatch::state::service_state(engine) else {
        return Vec::new();
    };
    let mut store = store.lock();
    let now = cloudwatch::state::now_secs();

    for kind in AlarmKind::ALL {
        if wanted.iter().all(|spec| spec.kind != kind) {
            store.alarms.remove(&policy_alarm_name(key, kind));
        }
    }

    let mut minted = Vec::new();
    for spec in wanted {
        let arn = cloudwatch::state::alarm_arn(region, &spec.name);
        // An alarm that already exists keeps the state an evaluation gave it, and the
        // tags a caller put on it; only its configuration is rewritten, exactly as
        // PutMetricAlarm does.
        let existing = store.alarms.get(&spec.name).map(|stored| {
            (
                stored.alarm.state_value.clone(),
                stored.alarm.state_reason.clone(),
                stored.alarm.state_updated_timestamp,
                stored.alarm.state_transitioned_timestamp,
                stored.tags.clone(),
            )
        });
        let (state_value, state_reason, state_updated, state_transitioned, tags) =
            existing.unwrap_or_else(|| {
                (
                    Some("INSUFFICIENT_DATA".to_string()),
                    Some("Unchecked: initial alarm creation".to_string()),
                    Some(cloudwatch::state::to_timestamp(now)),
                    Some(cloudwatch::state::to_timestamp(now)),
                    Vec::new(),
                )
            });
        let alarm = MetricAlarm {
            alarm_name: Some(spec.name.clone()),
            alarm_arn: Some(arn.clone()),
            alarm_description: Some(format!(
                "DO NOT EDIT OR DELETE. For TargetTrackingScaling policy {} of scalable target \
                 {} {} {}.",
                key.policy_name,
                key.target.service_namespace,
                key.target.resource_id,
                key.target.scalable_dimension
            )),
            alarm_configuration_updated_timestamp: Some(cloudwatch::state::to_timestamp(now)),
            actions_enabled: Some(true),
            // The alarm's action IS the scaling policy: that is the wiring that makes a
            // target-tracking policy alarm-driven.
            alarm_actions: Some(vec![policy_arn.to_string()]),
            state_value,
            state_reason,
            state_updated_timestamp: state_updated,
            state_transitioned_timestamp: state_transitioned,
            metric_name: Some(spec.metric_name.clone()),
            namespace: Some(spec.metric_namespace.clone()),
            statistic: Some(spec.statistic.clone()),
            dimensions: Some(
                spec.dimensions
                    .iter()
                    .map(|(name, value)| Dimension {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            ),
            period: Some(spec.period),
            evaluation_periods: Some(spec.evaluation_periods),
            threshold: Some(spec.threshold),
            comparison_operator: Some(spec.comparison_operator.clone()),
            ..MetricAlarm::default()
        };
        store.alarms.insert(
            spec.name.clone(),
            cloudwatch::state::StoredAlarm { alarm, tags },
        );
        minted.push(Alarm {
            alarm_name: spec.name.clone(),
            alarm_arn: arn,
        });
    }
    minted
}

// ---------------------------------------------------------------------------
// Scheduled actions
// ---------------------------------------------------------------------------

/// A scheduled action's parsed `Schedule`.
///
/// "The following formats are supported: At expressions - at(yyyy-mm-ddThh:mm:ss), Rate
/// expressions - rate(value unit), Cron expressions - cron(fields)" — three forms, so three
/// variants, and anything else is not a schedule at all.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// One occurrence, at this LOCAL date and time: "at and cron expressions use Universal
    /// Coordinated Time (UTC) by default" — by default, so the wall-clock reading the
    /// expression carries is resolved in the action's own time zone, not stored as an
    /// instant.
    At(::chrono::NaiveDateTime),
    /// An occurrence every `period`.
    Rate(::std::time::Duration),
    /// An occurrence at every instant the cron expression names. Boxed because a parsed
    /// cron expression is an order of magnitude larger than the other two forms.
    Cron(Box<::cron::Schedule>),
}

/// The units a `rate(value unit)` expression accepts, and what each is worth in seconds:
/// "For rate expressions, value is a positive integer and unit is minute | minutes | hour
/// | hours | day | days."
///
/// The one place the unit set lives — [`parse_schedule`] reads membership and the
/// multiplier from it, and the suites pin the set through it.
pub const RATE_UNITS: &[(&str, u64)] = &[
    ("minute", 60),
    ("minutes", 60),
    ("hour", 3_600),
    ("hours", 3_600),
    ("day", 86_400),
    ("days", 86_400),
];

/// How many whitespace-separated fields a `cron(…)` schedule carries: "The cron format
/// consists of six fields separated by white spaces: [Minutes] [Hours] [Day_of_Month]
/// [Month] [Day_of_Week] [Year]."
pub const CRON_FIELDS: usize = 6;

/// The largest instant this service computes with: 9999-12-31T23:59:59Z, the end of the
/// range a modelled timestamp can be rendered in.
///
/// Every request-derived instant and interval is bounded by it, so no caller-chosen value
/// can reach an arithmetic operation that is not representable.
pub const MAX_TIMESTAMP_SECS: u64 = 253_402_300_799;

/// The longest interval a `rate(value unit)` expression may name.
///
/// The EMULATOR's own bound, not a documented AWS one: `value` is an arbitrary integer and
/// an interval has to be addable to an instant, so the magnitude is refused here rather
/// than carried into instant arithmetic that cannot represent it. A hundred years is far
/// past any interval a scheduled action is useful at.
pub const MAX_RATE_SECS: u64 = 100 * 365 * 86_400;

/// The detail a `Schedule` that is not one of the three documented forms is refused with.
///
/// The three forms are the model's own ("At expressions - at(yyyy-mm-ddThh:mm:ss), Rate
/// expressions - rate(value unit), Cron expressions - cron(fields)"), and the service states
/// them back as the syntax a schedule expression has to have: a caller whose expression is
/// not a schedule at all is told the grammar, not which member failed which constraint.
pub const SCHEDULE_SYNTAX_DETAIL: &str = "Schedule expressions must have the following syntax: \
     rate(<number>\\s?(minutes?|hours?|days?)), cron(<cron_expression>) or \
     at(yyyy-MM-dd'T'HH:mm:ss)";

/// The six cron fields, in the order the service documents them, as one phrase \u2014 what a
/// refusal names when the expression does not carry that many.
pub const CRON_FIELD_LAYOUT: &str = "[Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]";

/// The name of each cron field, in order: "The cron format consists of six fields separated
/// by white spaces: [Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]."
pub const CRON_FIELD_NAMES: [&str; CRON_FIELDS] = [
    "Minutes",
    "Hours",
    "Day_of_Month",
    "Month",
    "Day_of_Week",
    "Year",
];

/// Wrap one parse detail as the `ValidationException` message a schedule expression is
/// refused with: the service's own sentence for "this is not a schedule", with the detail of
/// what stopped the parse.
fn refuse_schedule(detail: &str) -> String {
    format!("Invalid schedule expression. Details: {detail}.")
}

/// Parse a `Schedule`, or the `ValidationException` message for one that is not a schedule.
pub fn parse_schedule(schedule: &str) -> Result<Schedule, String> {
    let body = |keyword: &str| {
        schedule
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
            // Where the body starts, in characters, so a parse detail can report the
            // position of the token it stopped on within the whole expression.
            .map(|body| (body, keyword.chars().count() + 1))
    };

    if let Some((instant, _)) = body("at") {
        let parsed = ::chrono::NaiveDateTime::parse_from_str(instant.trim(), "%Y-%m-%dT%H:%M:%S")
            .map_err(|_| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        return Ok(Schedule::At(parsed));
    }
    if let Some((rate, _)) = body("rate") {
        let mut parts = rate.split_whitespace();
        let (Some(value), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(refuse_schedule(SCHEDULE_SYNTAX_DETAIL));
        };
        let value: u64 = value
            .parse()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        let seconds = RATE_UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, seconds)| *seconds)
            .ok_or_else(|| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        // The magnitude is refused here, where the caller is told why, rather than carried
        // into instant arithmetic that cannot represent it. The EMULATOR's own bound, so the
        // detail says what the bound is.
        let total = value
            .checked_mul(seconds)
            .filter(|total| *total <= MAX_RATE_SECS)
            .ok_or_else(|| {
                refuse_schedule(&format!(
                    "the schedule interval must not exceed {MAX_RATE_SECS} seconds"
                ))
            })?;
        return Ok(Schedule::Rate(::std::time::Duration::from_secs(total)));
    }
    if let Some((fields, offset)) = body("cron") {
        return Ok(Schedule::Cron(Box::new(
            parse_cron(fields, offset).map_err(|detail| refuse_schedule(&detail))?,
        )));
    }
    Err(refuse_schedule(SCHEDULE_SYNTAX_DETAIL))
}

/// The time zone an `at` or `cron` expression is read in: "specifies the time zone used when
/// setting a scheduled action by using an at or cron expression. If a time zone is not
/// provided, UTC is used by default. Valid values are the canonical names of the IANA time
/// zones", so the name is resolved against the IANA zone database itself rather than against
/// a hand-written list, and an unknown name is the `ValidationException` message.
pub fn schedule_timezone(timezone: Option<&str>) -> Result<::chrono_tz::Tz, String> {
    let Some(timezone) = timezone else {
        return Ok(::chrono_tz::UTC);
    };
    timezone.parse::<::chrono_tz::Tz>().map_err(|_| {
        format!(
            "Value '{timezone}' at 'timezone' failed to satisfy constraint: Member must be the \
             canonical name of an IANA time zone, such as Etc/GMT+9 or Pacific/Tahiti"
        )
    })
}

/// Parse the six-field cron expression the service documents into a [`cron::Schedule`].
///
/// "The cron format consists of six fields separated by white spaces: [Minutes] [Hours]
/// [Day_of_Month] [Month] [Day_of_Week] [Year]" — a minute-resolution expression, which the
/// `cron` crate reads in its seven-field form (seconds first), so a literal `0` seconds field
/// is prepended. A `?` ("no specific value") in the day-of-month or day-of-week field is the
/// same statement as `*` in the grammar the crate parses; `?` in any OTHER field states
/// nothing, so it is a token that stops the parse.
///
/// `offset` is where `fields` starts within the whole `Schedule` expression, in characters,
/// so a refusal reports the position of the token it stopped on as the caller wrote it.
///
/// A refusal names that POSITION and that TOKEN: each field is parsed in its own place (the
/// other five held at `*`, which every field accepts), so the field that stopped the parse is
/// the one reported rather than the whole expression.
fn parse_cron(fields: &str, offset: usize) -> Result<::cron::Schedule, String> {
    let body = fields;
    let fields = whitespace_fields(body);
    for (index, (at, field)) in fields.iter().enumerate().take(CRON_FIELDS) {
        if !cron_field_parses(index, field) {
            return Err(format!(
                "parse error at position {}: unexpected token '{field}' in the {} field",
                offset + at + 1,
                CRON_FIELD_NAMES[index]
            ));
        }
    }
    if let Some((at, field)) = fields.get(CRON_FIELDS) {
        return Err(format!(
            "parse error at position {}: unexpected token '{field}' after the {} field, the \
             cron format consists of {CRON_FIELDS} fields ({CRON_FIELD_LAYOUT})",
            offset + at + 1,
            CRON_FIELD_NAMES[CRON_FIELDS - 1]
        ));
    }
    if fields.len() < CRON_FIELDS {
        return Err(format!(
            "parse error at position {}: unexpected end of expression, the {} field is missing, \
             the cron format consists of {CRON_FIELDS} fields ({CRON_FIELD_LAYOUT})",
            offset + body.chars().count() + 1,
            CRON_FIELD_NAMES[fields.len()]
        ));
    }
    let normalised: Vec<String> = ::std::iter::once("0".to_string())
        .chain(
            fields
                .iter()
                .enumerate()
                .map(|(index, (_, field))| normalised_cron_field(index, field).to_string()),
        )
        .collect();
    // Every field parses in its own place, so what is left is a combination the grammar does
    // not allow; the expression itself is the token that stopped the parse.
    normalised.join(" ").parse::<::cron::Schedule>().map_err(|error| {
        format!(
            "parse error at position {}: unexpected token '{body}', not a cron expression \
             ({error})",
            offset + 1
        )
    })
}

/// The whitespace-separated fields of `text`, each with the CHARACTER position it starts at \u2014
/// the position a parse refusal reports, which `split_whitespace` alone does not carry.
fn whitespace_fields(text: &str) -> Vec<(usize, &str)> {
    let mut fields = Vec::new();
    let mut start: Option<(usize, usize)> = None;
    for (position, (at, character)) in text.char_indices().enumerate() {
        match (character.is_whitespace(), start) {
            (false, None) => start = Some((position, at)),
            (true, Some((position, from))) => {
                fields.push((position, &text[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some((position, from)) = start {
        fields.push((position, &text[from..]));
    }
    fields
}

/// What the `cron` crate's grammar reads a field as: `?` ("no specific value") is `*` in the
/// day-of-month and day-of-week fields, and means nothing anywhere else.
fn normalised_cron_field(index: usize, field: &str) -> &str {
    /// The indexes of the day-of-month and day-of-week fields in [`CRON_FIELD_NAMES`].
    const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

    if field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
        "*"
    } else {
        field
    }
}

/// Whether `field` is a legal cron field in the place `index` names, judged by parsing it
/// THERE with every other field held at `*`.
fn cron_field_parses(index: usize, field: &str) -> bool {
    // The crate's seven-field form: seconds, then the six the service documents.
    let mut expression = ["0", "*", "*", "*", "*", "*", "*"];
    expression[index + 1] = normalised_cron_field(index, field);
    expression
        .join(" ")
        .parse::<::cron::Schedule>()
        .is_ok()
}

/// Apply every scheduled action whose occurrence the clock has already made due.
///
/// "At the scheduled time, if the current capacity is below the minimum capacity, Application
/// Auto Scaling scales out to the minimum capacity. If the current capacity is above the
/// maximum capacity, Application Auto Scaling scales in to the maximum capacity" — the action
/// moves the scalable target's own bounds, and it was due at its scheduled instant whether or
/// not anybody called, which is why a reporting operation may materialise it.
///
/// The occurrence that fires is the first one after the action was last applied (or after it
/// was created), bounded by `StartTime` and `EndTime` when it carries them: "when you specify
/// start and end times with a recurring schedule using a cron expression or rates, they form
/// the boundaries for when the recurring action starts and stops".
///
/// Every step is fallible on purpose: an action whose schedule, time zone or instants cannot
/// be computed with is skipped, so no stored record can make this pass fail.
pub fn apply_due_scheduled_actions(
    state: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    now: SystemTime,
) {
    let keys: Vec<ScheduledActionKey> = state.scheduled_actions.keys().cloned().collect();
    for key in keys {
        let Some(record) = state.scheduled_actions.get(&key) else {
            continue;
        };
        let Ok(schedule) = parse_schedule(&record.action.schedule) else {
            continue;
        };
        let Ok(timezone) = schedule_timezone(record.action.timezone.as_deref()) else {
            continue;
        };
        let since = record
            .last_applied
            .unwrap_or_else(|| system_time_of(&record.action.creation_time));
        let start = record.action.start_time.as_ref().map(system_time_of);
        let end = record.action.end_time.as_ref().map(system_time_of);
        let mut window_start = match start {
            Some(start) if start > since => start,
            _ => since,
        };
        let action = record.action.scalable_target_action.clone();
        // EVERY occurrence the clock has already made due is materialised in this one
        // pass, not one per request: a recurring action whose target was not read for
        // several periods must report the bounds the last due occurrence set, not the
        // first. The walk is bounded by [`MAX_DUE_OCCURRENCES_PER_PASS`], so a fine-grained
        // schedule left alone for a long time cannot make one request walk for ever; what
        // is left is picked up by the next pass.
        let mut applied = None;
        for _ in 0..MAX_DUE_OCCURRENCES_PER_PASS {
            let Some(occurrence) = next_occurrence(&schedule, timezone, window_start) else {
                break;
            };
            if occurrence > now || end.is_some_and(|end| occurrence > end) {
                break;
            }
            applied = Some(occurrence);
            window_start = occurrence;
        }
        let Some(occurrence) = applied else {
            continue;
        };
        let mut movement = None;
        if let Some(target) = state.targets.get_mut(&key.target) {
            if let Some(action) = action {
                if let Some(min) = action.min_capacity {
                    target.min_capacity = min;
                }
                if let Some(max) = action.max_capacity {
                    target.max_capacity = max;
                }
                // The bounds are a range: an action that moves one past the other is
                // applied as the point the two meet at rather than as an inverted window.
                target.max_capacity = target.max_capacity.max(target.min_capacity);
                // "At the scheduled time, if the current capacity is below the minimum
                // capacity, Application Auto Scaling scales out to the minimum capacity.
                // If the current capacity is above the maximum capacity, Application Auto
                // Scaling scales in to the maximum capacity" — the same movement
                // registering a narrower window makes, so it is the same kind of scaling
                // activity, with the action that caused it named.
                let moved = target
                    .current_capacity
                    .clamp(target.min_capacity, target.max_capacity);
                if moved != target.current_capacity {
                    movement = Some((
                        moved,
                        if moved > target.current_capacity {
                            format!(
                                "scheduled action {} set the minimum capacity to {}",
                                key.scheduled_action_name, target.min_capacity
                            )
                        } else {
                            format!(
                                "scheduled action {} set the maximum capacity to {}",
                                key.scheduled_action_name, target.max_capacity
                            )
                        },
                    ));
                    target.current_capacity = moved;
                }
            }
        }
        // The activity is recorded at the instant the occurrence was DUE, not at the
        // instant somebody happened to read it: the movement was owed then.
        if let Some((moved, cause)) = movement {
            state.record_capacity_change(&key.target, moved, cause, occurrence);
        }
        if let Some(record) = state.scheduled_actions.get_mut(&key) {
            record.last_applied = Some(occurrence);
        }
    }
}

/// How many occurrences of ONE action a single pass materialises.
///
/// Every occurrence of an action sets the same capacity bounds, so walking them changes
/// nothing after the first — the walk exists only to leave `last_applied` at the last due
/// instant. The bound keeps the work per request a constant: a `rate(1 minute)` action
/// whose target is read a year later catches up over several passes instead of walking
/// half a million instants in one.
const MAX_DUE_OCCURRENCES_PER_PASS: usize = 1_000;

/// The first instant `schedule` fires at strictly after `after`, read in `timezone`.
///
/// `None` when the schedule names no further occurrence, or none that is representable:
/// every arithmetic step here is checked, because the schedule and the instant it is
/// computed from both come from a request.
fn next_occurrence(
    schedule: &Schedule,
    timezone: ::chrono_tz::Tz,
    after: SystemTime,
) -> Option<SystemTime> {
    use ::chrono::TimeZone;

    let local_after = timezone.from_utc_datetime(&datetime_of(after)?.naive_utc());
    match schedule {
        // A local wall-clock reading is one instant per zone — and, in the hour a zone
        // skips, none at all, which is a schedule with no occurrence rather than a guess.
        Schedule::At(at) => {
            let at = instant_of(timezone.from_local_datetime(at).earliest()?)?;
            (at > after).then_some(at)
        }
        Schedule::Rate(period) => after.checked_add(*period),
        Schedule::Cron(cron) => instant_of(cron.after(&local_after).next()?),
    }
}

/// `at` as a UTC date and time, or `None` when it is past the range one can represent.
fn datetime_of(at: SystemTime) -> Option<::chrono::DateTime<::chrono::Utc>> {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .min(MAX_TIMESTAMP_SECS);
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).ok()?, 0)
}

/// A zoned date and time as a `SystemTime`, or `None` when it is outside the representable
/// range.
fn instant_of<Tz: ::chrono::TimeZone>(at: ::chrono::DateTime<Tz>) -> Option<SystemTime> {
    let seconds = u64::try_from(at.timestamp()).ok()?;
    (seconds <= MAX_TIMESTAMP_SECS)
        .then(|| SystemTime::UNIX_EPOCH.checked_add(::std::time::Duration::from_secs(seconds)))
        .flatten()
}

/// A modelled timestamp as a `SystemTime`, clamped to the range this service computes in.
///
/// A timestamp arrives from a request, so neither end of it is trusted: a value below the
/// epoch reads as the epoch and one past [`MAX_TIMESTAMP_SECS`] (or not a finite number at
/// all) as that bound, rather than reaching a conversion that cannot represent it.
pub fn system_time_of(stamp: &::aws_smithy_types::DateTime) -> SystemTime {
    let seconds = stamp.as_secs_f64();
    if !seconds.is_finite() || seconds <= 0.0 {
        return SystemTime::UNIX_EPOCH;
    }
    let seconds = if seconds >= MAX_TIMESTAMP_SECS as f64 {
        MAX_TIMESTAMP_SECS
    } else {
        seconds as u64
    };
    SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(seconds)
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// How many tags one scalable target holds.
///
/// `TagResource`'s own documentation points at "Tagging support for Application Auto
/// Scaling" and at "User-defined tag restrictions" for "the rules that apply to tag keys
/// and tag values"; those restrictions cap a resource at 50 tags. The model puts no
/// `@length` on `TagMap`, so this bound is enforced by the handlers rather than at the
/// boundary.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix Amazon Web Services reserves for its own tags: a caller may
/// neither set nor remove a key in it. `TagKey` carries no `@pattern`, so the reserved
/// namespace is enforced by the handlers.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The prefix is matched case-insensitively, which is deliberately a SUPERSET of the
/// documented rule rather than a modelled constraint: tag keys themselves are
/// case-sensitive ("both case-sensitive strings"), and AWS documents the reserved prefix
/// in lower case only, so `AWS:`-prefixed keys are refused here on the conservative
/// reading that a key cannot leave the reserved namespace by changing the case of the
/// prefix alone.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Ok` when no key in `keys` is in the reserved namespace, else the
/// `ValidationException` message naming `member` and the offending key.
///
/// The keys are judged BEFORE anything is written, so a batch holding one reserved key
/// is refused whole and no key from it lands. The lowest offending key is reported, so a
/// request whose keys arrive in any order is refused with the same message.
pub fn check_tag_keys_unreserved<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let reserved = keys.into_iter().filter(|key| is_reserved_tag_key(key)).min();
    match reserved {
        None => Ok(()),
        Some(key) => Err(format!(
            "Value '{key}' at '{member}' failed to satisfy constraint: \
             Member must not begin with the reserved prefix '{RESERVED_TAG_KEY_PREFIX}'"
        )),
    }
}

/// `Ok` when `member`'s entry count is within [`MAX_TAGS_PER_RESOURCE`], else the
/// `ValidationException` message a request too large for ANY resource is refused with —
/// input validation, independent of which resource the request addresses.
pub fn check_tag_count(member: &str, count: usize) -> Result<(), String> {
    if count <= MAX_TAGS_PER_RESOURCE {
        return Ok(());
    }
    Err(format!(
        "Value at '{member}' failed to satisfy constraint: Member must have at most \
         {MAX_TAGS_PER_RESOURCE} entries, and {count} were supplied"
    ))
}

/// `TagKey`'s own `@length(min: 1, max: 128)`, and `TagValue`'s `@length(min: 0, max:
/// 256)` — the shapes `TagMap`'s key and value target.
///
/// A map's KEYS are not reached by the boundary validator (it judges a map's values, so
/// an over-long tag VALUE never arrives here, while an over-long tag KEY does), so these
/// two bounds are checked by the handlers. Lengths are in Unicode code points, as Smithy
/// `@length` counts a string.
pub const MIN_TAG_KEY_CHARS: usize = 1;
/// See [`MIN_TAG_KEY_CHARS`].
pub const MAX_TAG_KEY_CHARS: usize = 128;
/// See [`MIN_TAG_KEY_CHARS`].
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// `Ok` when `key` is within `TagKey`'s modelled length, else the `ValidationException`
/// message naming `member`, the key and the bounds.
pub fn check_tag_key(member: &str, key: &str) -> Result<(), String> {
    let chars = key.chars().count();
    if (MIN_TAG_KEY_CHARS..=MAX_TAG_KEY_CHARS).contains(&chars) {
        return Ok(());
    }
    Err(format!(
        "Value '{key}' at '{member}' failed to satisfy constraint: Member must have \
         length between {MIN_TAG_KEY_CHARS} and {MAX_TAG_KEY_CHARS}, inclusive"
    ))
}

/// `Ok` when `value` is within `TagValue`'s modelled length, else the
/// `ValidationException` message naming the key it belongs to and the bound.
pub fn check_tag_value(member: &str, key: &str, value: &str) -> Result<(), String> {
    let chars = value.chars().count();
    if chars <= MAX_TAG_VALUE_CHARS {
        return Ok(());
    }
    Err(format!(
        "Value at '{member}.{key}' failed to satisfy constraint: Member must have length \
         less than or equal to {MAX_TAG_VALUE_CHARS}"
    ))
}


