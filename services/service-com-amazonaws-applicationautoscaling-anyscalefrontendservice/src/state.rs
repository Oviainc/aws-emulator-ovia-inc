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

use std::collections::BTreeMap;
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
    /// The modelled `ScheduledAction` is stored as-is rather than behind a record of its
    /// own: every member of it is either part of the key or a value `PutScheduledAction`
    /// persisted verbatim, so `DescribeScheduledActions` has nothing to rebuild.
    pub scheduled_actions: BTreeMap<ScheduledActionKey, ScheduledAction>,
    /// The scaling-activity log, oldest first — every capacity movement this service
    /// performed, within the window it reports over. `DescribeScalingActivities` reads
    /// it newest first.
    pub activities: Vec<ScalingActivityRecord>,
    /// How many activities have ever been recorded. It only grows, so it both orders the
    /// log and makes each minted `ActivityId` unique.
    pub activities_recorded: u64,
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
/// The opaque id is derived from the target's identity, so a target that is registered,
/// deregistered and registered again under the same identity is never handed two ARNs
/// that are both "current", and the value is reproducible rather than per-run.
pub fn scalable_target_arn(region: &str, key: &TargetKey) -> String {
    let id = opaque_id(
        &[
            "scalable-target",
            &key.service_namespace,
            &key.resource_id,
            &key.scalable_dimension,
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
pub fn service_linked_role_arn(service_namespace: &str) -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/{service_namespace}.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling"
        ),
    )
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

/// The message a refusal of a `ResourceARN` that names no registered scalable target
/// carries. AWS documents no wording for it, so this names the member and the value the
/// way this service's other refusals do.
pub fn unknown_resource_arn_message(resource_arn: &str) -> String {
    format!(
        "Value '{resource_arn}' at 'resourceARN' failed to satisfy constraint: \
         Member must identify a registered scalable target"
    )
}
