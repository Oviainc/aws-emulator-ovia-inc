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
use std::time::SystemTime;

use crate::ops::types::{
    Alarm, PredictiveScalingPolicyConfiguration, ScalableTarget, ScalingPolicy, ScheduledAction,
    StepScalingPolicyConfiguration, SuspendedState, TargetTrackingScalingPolicyConfiguration,
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
    /// No writer until the scheduled-action slice: `PutScheduledAction` is still a 501,
    /// so today only the cascade touches this map.
    pub scheduled_actions: BTreeMap<ScheduledActionKey, ScheduledAction>,
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
    let digest = opaque_id(
        &[
            "scalingPolicy",
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
        ]
        .join("\u{1f}"),
        32,
    );
    let id = format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32],
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
