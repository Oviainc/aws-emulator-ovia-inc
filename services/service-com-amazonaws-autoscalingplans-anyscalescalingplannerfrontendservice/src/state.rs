// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice service.
//!
//! The whole service is one resource — the scaling plan — so this module owns the
//! stored record, the lifecycle clock that moves a plan out of its non-terminal
//! status, and the two value helpers (timestamps and pagination cursors) that more
//! than one handler in `src/ops/` reaches.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ApplicationSource, PredefinedScalingMetricSpecification, ScalingInstruction,
    TargetTrackingConfiguration, LOAD_METRIC_TYPE_VALUES, METRIC_STATISTIC_VALUES,
    PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES, PREDICTIVE_SCALING_MODE_VALUES,
    SCALABLE_DIMENSION_VALUES, SCALING_METRIC_TYPE_VALUES,
    SCALING_POLICY_UPDATE_BEHAVIOR_VALUES, SERVICE_NAMESPACE_VALUES,
};

/// The only scaling-plan version this service models.
///
/// `CreateScalingPlanResponse$ScalingPlanVersion` is documented as "This value is
/// always 1. Currently, you cannot have multiple scaling plan versions."
pub const ONLY_VERSION: i64 = 1;

/// Documented `MaxResults` bounds for both Describe operations: "This value can be
/// between 1 and 50. The default value is 50."
/// (`smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="MaxResults")`).
pub const MAX_RESULTS_MIN: i32 = 1;
/// Upper bound of the documented `MaxResults` range; also its documented default.
pub const MAX_RESULTS_MAX: i32 = 50;

/// How long a scaling plan stays in a non-terminal status before the clock moves it on.
///
/// CHOSEN, not documented: AWS publishes no duration for `CreationInProgress` ->
/// `Active`, so this emulator uses a fixed span inside its 250-750 ms band. A caller
/// acting straight after `CreateScalingPlan` therefore still observes the transient
/// status and the state errors that go with it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most scaling plans this emulator will hold at once, enforced through the
/// modelled `LimitExceededException` ("Your account exceeded a limit. This exception
/// is thrown when a per-account resource limit is exceeded.").
///
/// AWS enforces a per-account quota here but does not publish its value in the model,
/// so the NUMBER is chosen; what is modelled is that the store is bounded, so a caller
/// cannot grow it without limit.
pub const MAX_SCALING_PLANS: usize = 50;

/// `ScalingPlanStatusCode` value of a plan whose configuration is applied.
pub const STATUS_ACTIVE: &str = "Active";
/// `ScalingPlanStatusCode` value of a plan that is still being created.
pub const STATUS_CREATION_IN_PROGRESS: &str = "CreationInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being updated.
pub const STATUS_UPDATE_IN_PROGRESS: &str = "UpdateInProgress";
/// `ScalingPlanStatusCode` value of a plan that is still being deleted.
pub const STATUS_DELETION_IN_PROGRESS: &str = "DeletionInProgress";

/// `ScalingStatusCode` value of a resource whose scaling configuration is applied.
pub const SCALING_STATUS_ACTIVE: &str = "Active";
/// `ScalingStatusCode` value of a resource whose scaling configuration is not applied
/// yet — "the scaling plan is being created or the scaling configuration could not be
/// applied" (`ScalingPlanResource$ScalingStatusCode` doc).
pub const SCALING_STATUS_INACTIVE: &str = "Inactive";
/// `ScalingStatusCode` value of a resource whose configuration is only partly applied —
/// "because the scaling plan is being created or deleted" (same doc).
pub const SCALING_STATUS_PARTIALLY_ACTIVE: &str = "PartiallyActive";

/// What the clock owes a plan that is in a non-terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingOutcome {
    /// The plan becomes `Active`.
    Active,
    /// The plan is removed from state.
    Removed,
}

/// A transition the clock already owes at `due_at`, whether or not anybody calls.
#[derive(Debug, Clone, Copy)]
pub struct PendingTransition {
    /// The instant the transition became (or becomes) due.
    pub due_at: SystemTime,
    /// What the plan looks like once it is applied.
    pub outcome: PendingOutcome,
}

/// One stored scaling plan: exactly the members the `ScalingPlan` output shape
/// requires, so `DescribeScalingPlans` is a pure projection of this record.
#[derive(Debug, Clone)]
pub struct ScalingPlanRecord {
    /// `ScalingPlanName` — the primary key.
    pub name: String,
    /// `ScalingPlanVersion`; always [`ONLY_VERSION`].
    pub version: i64,
    /// `ApplicationSource`, stored as the caller supplied it.
    pub application_source: ApplicationSource,
    /// `ScalingInstructions`, stored as the caller supplied them.
    pub scaling_instructions: Vec<ScalingInstruction>,
    /// `StatusCode` — a `ScalingPlanStatusCode` value.
    pub status_code: String,
    /// `StatusMessage`.
    pub status_message: Option<String>,
    /// `StatusStartTime` — when the plan entered `status_code`.
    pub status_start_time: SystemTime,
    /// `CreationTime`.
    pub creation_time: SystemTime,
    /// The clock-owed transition out of a non-terminal `status_code`, if any.
    pub pending: Option<PendingTransition>,
}

impl ScalingPlanRecord {
    /// A freshly created plan: `CreationInProgress`, with the move to `Active` already
    /// owed at `now + SETTLE_AFTER`.
    pub fn creating(
        name: String,
        application_source: ApplicationSource,
        scaling_instructions: Vec<ScalingInstruction>,
        now: SystemTime,
    ) -> Self {
        Self {
            name,
            version: ONLY_VERSION,
            application_source,
            scaling_instructions,
            status_code: STATUS_CREATION_IN_PROGRESS.to_string(),
            status_message: None,
            status_start_time: now,
            creation_time: now,
            pending: Some(PendingTransition {
                due_at: now + SETTLE_AFTER,
                outcome: PendingOutcome::Active,
            }),
        }
    }

    /// Enter a non-terminal `status`, owing `outcome` at `now + SETTLE_AFTER`.
    pub fn begin(&mut self, status: &str, outcome: PendingOutcome, now: SystemTime) {
        self.status_code = status.to_string();
        self.status_start_time = now;
        self.pending = Some(PendingTransition {
            due_at: now + SETTLE_AFTER,
            outcome,
        });
    }

    /// True while the plan is being created, updated or deleted — the state in which
    /// `UpdateScalingPlan` and `DeleteScalingPlan` answer `ConcurrentUpdateException`
    /// ("You cannot update a scaling plan if it is in the process of being created,
    /// updated, or deleted").
    pub fn is_in_progress(&self) -> bool {
        self.pending.is_some()
    }

    /// The `ScalingStatusCode` every resource of this plan reports, derived from the
    /// plan's own status: applied only once the plan itself is applied.
    pub fn scaling_status_code(&self) -> &'static str {
        match self.status_code.as_str() {
            STATUS_ACTIVE => SCALING_STATUS_ACTIVE,
            STATUS_CREATION_IN_PROGRESS => SCALING_STATUS_INACTIVE,
            _ => SCALING_STATUS_PARTIALLY_ACTIVE,
        }
    }
}

/// Per-service state for com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Every stored plan, keyed by `ScalingPlanName`. A `BTreeMap` so both Describe
    /// operations page over a stable, deterministic ordering.
    pub scaling_plans: BTreeMap<String, ScalingPlanRecord>,
}

impl ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState {
    /// Apply every transition the clock already owes at `now`.
    ///
    /// This is the ONLY write a reporting operation makes: the work was due at `now`
    /// whether or not anybody called, so running it here cannot make a read change
    /// what a later caller sees.
    pub fn settle_due(&mut self, now: SystemTime) {
        let due: Vec<(String, PendingOutcome, SystemTime)> = self
            .scaling_plans
            .values()
            .filter_map(|plan| {
                let pending = plan.pending?;
                (pending.due_at <= now)
                    .then(|| (plan.name.clone(), pending.outcome, pending.due_at))
            })
            .collect();
        for (name, outcome, due_at) in due {
            match outcome {
                PendingOutcome::Removed => {
                    self.scaling_plans.remove(&name);
                }
                PendingOutcome::Active => {
                    if let Some(plan) = self.scaling_plans.get_mut(&name) {
                        plan.status_code = STATUS_ACTIVE.to_string();
                        // The plan entered `Active` when the transition came due, not
                        // when somebody happened to look.
                        plan.status_start_time = due_at;
                        plan.pending = None;
                    }
                }
            }
        }
    }

    /// The plan stored under `name` whose version is `version`.
    pub fn plan(&self, name: &str, version: i64) -> Option<&ScalingPlanRecord> {
        self.scaling_plans
            .get(name)
            .filter(|plan| plan.version == version)
    }

    /// Mutable access to the plan stored under `name` whose version is `version`.
    pub fn plan_mut(&mut self, name: &str, version: i64) -> Option<&mut ScalingPlanRecord> {
        self.scaling_plans
            .get_mut(name)
            .filter(|plan| plan.version == version)
    }
}

/// A modelled `TimestampType` from a stored wall-clock instant.
///
/// Clamped at the epoch rather than panicking: a host clock before 1970 is not a
/// client error.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The identity of an `ApplicationSource`, used to enforce "You can create one
/// scaling plan per application source" (`ApplicationSource` member doc).
///
/// `None` when the source names neither a stack nor a tag filter, so it identifies no
/// application at all. A source that names BOTH is refused before this is reached —
/// see [`validate_application_source`].
pub fn application_source_key(source: &ApplicationSource) -> Option<String> {
    if let Some(arn) = source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        return Some(format!("stack:{arn}"));
    }
    let filters = source.tag_filters.as_ref()?;
    if filters.is_empty() {
        return None;
    }
    let mut rendered: Vec<String> = filters
        .iter()
        .map(|filter| {
            let key = filter.key.clone().unwrap_or_default();
            let mut values = filter.values.clone().unwrap_or_default();
            values.sort();
            format!("{key}={}", values.join(","))
        })
        .collect();
    rendered.sort();
    Some(format!("tags:{}", rendered.join(";")))
}

/// Whether a tag-filter list identifies anything: a filter list that is absent or
/// empty selects nothing.
fn names_tag_filters(source: &ApplicationSource) -> bool {
    source
        .tag_filters
        .as_ref()
        .is_some_and(|filters| !filters.is_empty())
}

/// Whether a `CloudFormationStackARN` is present and non-empty.
fn names_stack(source: &ApplicationSource) -> bool {
    source
        .cloud_formation_stack_arn
        .as_deref()
        .is_some_and(|arn| !arn.is_empty())
}

/// Whether `arn` is the Amazon Resource Name of a CloudFormation stack.
///
/// `ApplicationSource$CloudFormationStackARN` is documented as "The Amazon Resource
/// Name (ARN) of a AWS CloudFormation stack", and an ARN is
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — so a stack ARN names the
/// `cloudformation` service and a `stack/…` resource.
fn is_cloud_formation_stack_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("cloudformation"),
        Some(region),
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
        && !region.is_empty()
        && !account.is_empty()
        && resource
            .strip_prefix("stack/")
            .is_some_and(|name| !name.is_empty())
}

/// Validate an `ApplicationSource` a caller wrote, returning its identity key.
///
/// `ApplicationSource` is documented as "A CloudFormation stack or a set of tags" —
/// the two are ALTERNATIVES, so naming neither identifies no application and naming
/// both is contradictory, and a stack is named by its ARN. `Err` carries the refusal
/// message naming the member at fault.
pub fn validate_application_source(source: &ApplicationSource) -> Result<String, String> {
    if let Some(arn) = source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        if !is_cloud_formation_stack_arn(arn) {
            return Err(format!("Invalid CloudFormation stack ARN: {arn}"));
        }
    }
    match (names_stack(source), names_tag_filters(source)) {
        (true, true) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters, not both"
                .to_string(),
        ),
        (false, false) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters"
                .to_string(),
        ),
        _ => application_source_key(source).ok_or_else(|| {
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters".to_string()
        }),
    }
}

// ---------------------------------------------------------------------------
// ScalingInstruction semantics
//
// The model constrains each member's TYPE, but a scaling instruction also has to hold
// together across its members, and `CreateScalingPlan` / `UpdateScalingPlan` both
// model `ValidationException` ("An exception was thrown for a validation issue. Review
// the parameters provided.") for input that does not. Every constant and relation
// below comes from the model, from its documentation, or from the refusal the service
// itself answers with — never from the values of one particular request.
// ---------------------------------------------------------------------------

/// Documented bound on `ScalingInstruction$TargetTrackingConfigurations`: "The target
/// tracking configurations (up to 10)."
pub const MAX_TARGET_TRACKING_CONFIGURATIONS: usize = 10;

/// The smallest `MinCapacity` a scaling instruction may ask for.
///
/// `ResourceCapacity` carries no `@range` in the model, but the service refuses a
/// minimum capacity below one unit ("Minimum capacity cannot be less than 1"): a
/// scaling instruction has to leave the resource at least one unit of capacity.
pub const MIN_RESOURCE_CAPACITY: i32 = 1;

/// Lower bound of `ScalingInstruction$PredictiveScalingMaxCapacityBuffer`: "The range
/// is 1-100."
pub const MAX_CAPACITY_BUFFER_MIN: i32 = 1;
/// Upper bound of the same documented range.
pub const MAX_CAPACITY_BUFFER_MAX: i32 = 100;

/// The `PredictiveScalingMaxCapacityBehavior` value that makes
/// `PredictiveScalingMaxCapacityBuffer` required: "Required if the
/// PredictiveScalingMaxCapacityBehavior is set to SetMaxCapacityAboveForecastCapacity,
/// and cannot be used otherwise."
pub const BEHAVIOR_ABOVE_FORECAST_CAPACITY: &str = "SetMaxCapacityAboveForecastCapacity";

/// The only `ScalingMetricType` a `PredefinedScalingMetricSpecification$ResourceLabel`
/// may accompany: "You can't specify a resource label unless the metric type is
/// ALBRequestCountPerTarget".
pub const ALB_SCALING_METRIC_TYPE: &str = "ALBRequestCountPerTarget";

/// The same rule for `PredefinedLoadMetricSpecification$ResourceLabel`: "You can't
/// specify a resource label unless the metric type is ALBTargetGroupRequestCount".
pub const ALB_LOAD_METRIC_TYPE: &str = "ALBTargetGroupRequestCount";

/// `CustomizedLoadMetricSpecification$Statistic`: "The only valid value is Sum."
pub const LOAD_METRIC_STATISTIC: &str = "Sum";

/// The documented bound on `TargetTrackingConfiguration$TargetValue`: "Although this
/// property accepts numbers of type Double, it won't accept values that are either too
/// small or too large." The service states the two bounds itself, in the refusal it
/// answers with: "For target tracking scaling, target value must be between
/// '8.51592E-109' and '1.174271E108'".
pub const TARGET_VALUE_MIN: f64 = 8.515_92e-109;
/// Upper bound of that same range.
pub const TARGET_VALUE_MAX: f64 = 1.174_271e108;

/// The two bounds as the service renders them in its refusal text.
const TARGET_VALUE_MIN_TEXT: &str = "8.51592E-109";
const TARGET_VALUE_MAX_TEXT: &str = "1.174271E108";

/// What each `ScalableDimension` scales, and what can be tracked for it.
///
/// Columns: the scalable dimension; where the resource id of a resource it applies to
/// starts, taken from the worked examples in `ScalingInstruction$ResourceId`'s own
/// documentation (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`)
/// — `autoScalingGroup/my-asg`, `service/default/sample-webapp`,
/// `spot-fleet-request/sfr-73fbd2ce-…`, `table/my-table`,
/// `table/my-table/index/my-table-index`, `cluster:my-db-cluster`; and the
/// `ScalingMetricType`s that measure that resource, each named for what it measures
/// (`ASG…` an Auto Scaling group, `ECSService…` an ECS service, `DynamoDBRead…` /
/// `DynamoDBWrite…` the matching capacity of a table or index, `RDSReader…` an Aurora
/// replica, `EC2SpotFleetRequest…` a Spot Fleet request), with
/// `ALBRequestCountPerTarget` documented as applying "only to Auto Scaling groups, Spot
/// Fleet requests, and ECS services"
/// (`smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="PredefinedScalingMetricType")`).
const SCALABLE_DIMENSIONS: &[(&str, &str, &[&str])] = &[
    (
        "autoscaling:autoScalingGroup:DesiredCapacity",
        "autoScalingGroup/",
        &[
            "ASGAverageCPUUtilization",
            "ASGAverageNetworkIn",
            "ASGAverageNetworkOut",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "ecs:service:DesiredCount",
        "service/",
        &[
            "ECSServiceAverageCPUUtilization",
            "ECSServiceAverageMemoryUtilization",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "ec2:spot-fleet-request:TargetCapacity",
        "spot-fleet-request/",
        &[
            "EC2SpotFleetRequestAverageCPUUtilization",
            "EC2SpotFleetRequestAverageNetworkIn",
            "EC2SpotFleetRequestAverageNetworkOut",
            ALB_SCALING_METRIC_TYPE,
        ],
    ),
    (
        "rds:cluster:ReadReplicaCount",
        "cluster:",
        &[
            "RDSReaderAverageCPUUtilization",
            "RDSReaderAverageDatabaseConnections",
        ],
    ),
    (
        "dynamodb:table:ReadCapacityUnits",
        "table/",
        &["DynamoDBReadCapacityUtilization"],
    ),
    (
        "dynamodb:table:WriteCapacityUnits",
        "table/",
        &["DynamoDBWriteCapacityUtilization"],
    ),
    (
        "dynamodb:index:ReadCapacityUnits",
        "table/",
        &["DynamoDBReadCapacityUtilization"],
    ),
    (
        "dynamodb:index:WriteCapacityUnits",
        "table/",
        &["DynamoDBWriteCapacityUtilization"],
    ),
];

/// The row of [`SCALABLE_DIMENSIONS`] for `dimension`, if the model lists it.
fn scalable_dimension(dimension: &str) -> Option<(&'static str, &'static [&'static str])> {
    SCALABLE_DIMENSIONS
        .iter()
        .find(|(name, _, _)| *name == dimension)
        .map(|(_, prefix, metrics)| (*prefix, *metrics))
}

/// How the service refuses a `(ServiceNamespace, ResourceId, ScalableDimension)` triple
/// that does not describe one scalable resource: "Unsupported service namespace,
/// resource type or scalable dimension".
///
/// `pub` because every operation that names a scalable resource — the two write paths
/// through [`validate_scaling_instructions`] and the forecast read in
/// `crate::ops::get_scaling_plan_resource_forecast_data` — refuses such a triple with
/// this one sentence.
pub const UNSUPPORTED_RESOURCE: &str =
    "Unsupported service namespace, resource type or scalable dimension";

/// The segment a DynamoDB global secondary index's resource id carries, from the same
/// doc: "the resource type is `index` and the unique identifier is the resource ID.
/// Example: `table/my-table/index/my-table-index`".
const INDEX_SEGMENT: &str = "/index/";

/// Validate the `ScalingInstructions` a caller wrote.
///
/// `CreateScalingPlan` and `UpdateScalingPlan` both take this list and both model
/// `ValidationException`, so both refuse the same input the same way, before touching
/// state. `Err` carries the message naming the offending member and the constraint it
/// broke.
pub fn validate_scaling_instructions(instructions: &[ScalingInstruction]) -> Result<(), String> {
    if instructions.is_empty() {
        return Err(
            "ScalingInstructions must specify at least one scaling instruction".to_string(),
        );
    }
    // "Each scaling instruction applies to one resource" (`ScalingInstruction` doc), so
    // two instructions for one scalable resource contradict each other.
    let mut covered: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    for (index, instruction) in instructions.iter().enumerate() {
        validate_scaling_instruction(index, instruction)?;
        let resource = (
            instruction.service_namespace.as_str(),
            instruction.resource_id.as_str(),
            instruction.scalable_dimension.as_str(),
        );
        if !covered.insert(resource) {
            return Err(format!(
                "ScalingInstructions[{index}]: a scaling plan can specify only one scaling \
                 instruction per scalable resource, and '{}' ({}) is already covered",
                instruction.resource_id, instruction.scalable_dimension
            ));
        }
    }
    Ok(())
}

/// Every relation one `ScalingInstruction` has to satisfy on its own.
fn validate_scaling_instruction(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let at = |member: &str| format!("ScalingInstructions[{index}].{member}");

    one_of(
        &at("ServiceNamespace"),
        &instruction.service_namespace,
        SERVICE_NAMESPACE_VALUES,
    )?;
    one_of(
        &at("ScalableDimension"),
        &instruction.scalable_dimension,
        SCALABLE_DIMENSION_VALUES,
    )?;
    // Every `ScalableDimension` value is prefixed with the namespace of the service
    // that owns it, so a dimension from another service names no scalable resource.
    let dimension_namespace = instruction
        .scalable_dimension
        .split(':')
        .next()
        .unwrap_or_default();
    if dimension_namespace != instruction.service_namespace {
        return Err(UNSUPPORTED_RESOURCE.to_string());
    }
    validate_resource_id(&instruction.resource_id, &instruction.scalable_dimension)?;

    // "The minimum capacity of the resource" / "The maximum capacity of the resource":
    // a capacity is a count of units, and the maximum bounds the minimum. The refusals
    // name the members in prose, the way the service words them.
    if instruction.min_capacity < MIN_RESOURCE_CAPACITY {
        return Err(format!(
            "Minimum capacity cannot be less than {MIN_RESOURCE_CAPACITY}"
        ));
    }
    if instruction.max_capacity < instruction.min_capacity {
        return Err("Maximum capacity cannot be less than minimum capacity".to_string());
    }

    // "The amount of time, in seconds, to buffer the run time of scheduled scaling
    // actions": the model already bounds this at 0, which is checked again here so the
    // same refusal holds however the value reaches the handler.
    if instruction
        .scheduled_action_buffer_time
        .is_some_and(|seconds| seconds < 0)
    {
        return Err(format!(
            "{}: a time in seconds cannot be negative",
            at("ScheduledActionBufferTime")
        ));
    }
    if let Some(behavior) = instruction.scaling_policy_update_behavior.as_deref() {
        one_of(
            &at("ScalingPolicyUpdateBehavior"),
            behavior,
            SCALING_POLICY_UPDATE_BEHAVIOR_VALUES,
        )?;
    }

    validate_target_tracking_configurations(index, instruction)?;
    validate_predictive_scaling(index, instruction)
}

/// A resource id has to identify a resource of the kind its scalable dimension scales.
///
/// `pub` so the forecast read applies the same rule to its own `ResourceId` as the two
/// write paths apply to a scaling instruction's.
pub fn validate_resource_id(resource_id: &str, dimension: &str) -> Result<(), String> {
    // A dimension the model does not list is refused by its own enum check, not here.
    let Some((prefix, _)) = scalable_dimension(dimension) else {
        return Ok(());
    };
    let resource_type = dimension.split(':').nth(1).unwrap_or_default();
    let identifies = resource_id.starts_with(prefix)
        && match resource_type {
            // A global secondary index is addressed through its table, a table is not.
            "index" => resource_id.contains(INDEX_SEGMENT),
            "table" => !resource_id.contains(INDEX_SEGMENT),
            _ => true,
        };
    if !identifies {
        return Err(UNSUPPORTED_RESOURCE.to_string());
    }
    Ok(())
}

/// The target tracking configurations of one instruction: "(up to 10). Each of these
/// structures must specify a unique scaling metric and a target value for the metric."
fn validate_target_tracking_configurations(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let member = format!("ScalingInstructions[{index}].TargetTrackingConfigurations");
    let configurations = &instruction.target_tracking_configurations;
    if configurations.is_empty() {
        return Err(format!(
            "{member}: at least one target tracking configuration is required"
        ));
    }
    if configurations.len() > MAX_TARGET_TRACKING_CONFIGURATIONS {
        return Err(format!(
            "{member}: Value of length {} failed to satisfy constraint: Member must have \
             length less than or equal to {MAX_TARGET_TRACKING_CONFIGURATIONS}",
            configurations.len()
        ));
    }
    let mut metrics: BTreeSet<String> = BTreeSet::new();
    for (position, configuration) in configurations.iter().enumerate() {
        let at = format!("{member}[{position}]");
        let metric = validate_target_tracking_configuration(
            &at,
            &instruction.scalable_dimension,
            configuration,
        )?;
        if !metrics.insert(metric.clone()) {
            return Err(format!(
                "{at}: each target tracking configuration must specify a unique scaling \
                 metric, and '{metric}' is already specified"
            ));
        }
    }
    Ok(())
}

/// One target tracking configuration, answering the identity of the metric it tracks so
/// the caller can enforce uniqueness across the instruction.
fn validate_target_tracking_configuration(
    at: &str,
    dimension: &str,
    configuration: &TargetTrackingConfiguration,
) -> Result<String, String> {
    let target_value = configuration.target_value;
    if !(TARGET_VALUE_MIN..=TARGET_VALUE_MAX).contains(&target_value) {
        return Err(format!(
            "For target tracking scaling, target value must be between \
             '{TARGET_VALUE_MIN_TEXT}' and '{TARGET_VALUE_MAX_TEXT}', but was \
             '{target_value:?}'."
        ));
    }
    // "The amount of time, in seconds, …" — none of the three is a negative span.
    for (name, seconds) in [
        ("ScaleOutCooldown", configuration.scale_out_cooldown),
        ("ScaleInCooldown", configuration.scale_in_cooldown),
        (
            "EstimatedInstanceWarmup",
            configuration.estimated_instance_warmup,
        ),
    ] {
        if seconds.is_some_and(|value| value < 0) {
            return Err(format!("{at}.{name}: a time in seconds cannot be negative"));
        }
    }

    // "You can specify either a predefined metric or a customized metric": one of the
    // two, never neither and never both.
    match (
        &configuration.predefined_scaling_metric_specification,
        &configuration.customized_scaling_metric_specification,
    ) {
        (Some(_), Some(_)) => Err(format!(
            "{at}: specify either PredefinedScalingMetricSpecification or \
             CustomizedScalingMetricSpecification, not both"
        )),
        (None, None) => Err(format!(
            "{at}: specify either PredefinedScalingMetricSpecification or \
             CustomizedScalingMetricSpecification"
        )),
        (Some(predefined), None) => validate_predefined_scaling_metric(at, dimension, predefined),
        (None, Some(customized)) => {
            one_of(
                &format!("{at}.CustomizedScalingMetricSpecification.Statistic"),
                &customized.statistic,
                METRIC_STATISTIC_VALUES,
            )?;
            let mut dimensions: Vec<String> = customized
                .dimensions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|dimension| format!("{}={}", dimension.name, dimension.value))
                .collect();
            dimensions.sort();
            Ok(format!(
                "{}/{}/{}[{}]",
                customized.namespace,
                customized.metric_name,
                customized.statistic,
                dimensions.join(",")
            ))
        }
    }
}

/// A predefined scaling metric: a modelled metric type that measures the resource the
/// instruction scales, and a resource label only where one is allowed.
fn validate_predefined_scaling_metric(
    at: &str,
    dimension: &str,
    specification: &PredefinedScalingMetricSpecification,
) -> Result<String, String> {
    let member = format!("{at}.PredefinedScalingMetricSpecification");
    let metric = specification.predefined_scaling_metric_type.as_str();
    one_of(
        &format!("{member}.PredefinedScalingMetricType"),
        metric,
        SCALING_METRIC_TYPE_VALUES,
    )?;
    // A predefined metric measures one kind of resource, so only the metrics of the
    // resource this instruction scales can be tracked for it.
    if let Some((_, supported)) = scalable_dimension(dimension) {
        if !supported.contains(&metric) {
            return Err(format!(
                "Scalable dimension {dimension} only supports the following predefined \
                 metric types: {}",
                supported.join(", ")
            ));
        }
    }
    if specification.resource_label.is_some() && metric != ALB_SCALING_METRIC_TYPE {
        return Err(format!(
            "{member}.ResourceLabel: a resource label can only be specified when the metric \
             type is {ALB_SCALING_METRIC_TYPE}"
        ));
    }
    Ok(metric.to_string())
}

/// The predictive-scaling members of one instruction, each of which is documented as
/// valid only when a load metric specification configures predictive scaling.
fn validate_predictive_scaling(
    index: usize,
    instruction: &ScalingInstruction,
) -> Result<(), String> {
    let at = |member: &str| format!("ScalingInstructions[{index}].{member}");
    let predefined = instruction.predefined_load_metric_specification.as_ref();
    let customized = instruction.customized_load_metric_specification.as_ref();

    // "This parameter or a CustomizedLoadMetricSpecification is required when
    // configuring predictive scaling, and cannot be used otherwise."
    if predefined.is_some() && customized.is_some() {
        return Err(format!(
            "{}: specify either PredefinedLoadMetricSpecification or \
             CustomizedLoadMetricSpecification, not both",
            at("PredefinedLoadMetricSpecification")
        ));
    }
    if let Some(specification) = predefined {
        let member = at("PredefinedLoadMetricSpecification");
        one_of(
            &format!("{member}.PredefinedLoadMetricType"),
            &specification.predefined_load_metric_type,
            LOAD_METRIC_TYPE_VALUES,
        )?;
        if specification.resource_label.is_some()
            && specification.predefined_load_metric_type != ALB_LOAD_METRIC_TYPE
        {
            return Err(format!(
                "{member}.ResourceLabel: a resource label can only be specified when the \
                 metric type is {ALB_LOAD_METRIC_TYPE}"
            ));
        }
    }
    if let Some(specification) = customized {
        // "The statistic of the metric. The only valid value is Sum."
        if specification.statistic != LOAD_METRIC_STATISTIC {
            return Err(format!(
                "{}.Statistic: the only valid value is {LOAD_METRIC_STATISTIC}",
                at("CustomizedLoadMetricSpecification")
            ));
        }
    }

    let predictive = predefined.is_some() || customized.is_some();
    if let Some(mode) = instruction.predictive_scaling_mode.as_deref() {
        one_of(&at("PredictiveScalingMode"), mode, PREDICTIVE_SCALING_MODE_VALUES)?;
    }
    let behavior = instruction
        .predictive_scaling_max_capacity_behavior
        .as_deref();
    if let Some(behavior) = behavior {
        one_of(
            &at("PredictiveScalingMaxCapacityBehavior"),
            behavior,
            PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES,
        )?;
        // "Only valid when configuring predictive scaling."
        if !predictive {
            return Err(format!(
                "{}: only valid when configuring predictive scaling, which requires a load \
                 metric specification",
                at("PredictiveScalingMaxCapacityBehavior")
            ));
        }
    }

    // "Required if the PredictiveScalingMaxCapacityBehavior is set to
    // SetMaxCapacityAboveForecastCapacity, and cannot be used otherwise. The range is
    // 1-100."
    let above_forecast = behavior == Some(BEHAVIOR_ABOVE_FORECAST_CAPACITY);
    match instruction.predictive_scaling_max_capacity_buffer {
        Some(_) if !above_forecast => Err(format!(
            "{}: can only be specified when PredictiveScalingMaxCapacityBehavior is \
             {BEHAVIOR_ABOVE_FORECAST_CAPACITY}",
            at("PredictiveScalingMaxCapacityBuffer")
        )),
        Some(buffer)
            if !(MAX_CAPACITY_BUFFER_MIN..=MAX_CAPACITY_BUFFER_MAX).contains(&buffer) =>
        {
            Err(format!(
                "{}: Value '{buffer}' failed to satisfy constraint: Member must be between \
                 {MAX_CAPACITY_BUFFER_MIN} and {MAX_CAPACITY_BUFFER_MAX}",
                at("PredictiveScalingMaxCapacityBuffer")
            ))
        }
        None if above_forecast => Err(format!(
            "{}: required when PredictiveScalingMaxCapacityBehavior is \
             {BEHAVIOR_ABOVE_FORECAST_CAPACITY}",
            at("PredictiveScalingMaxCapacityBuffer")
        )),
        _ => Ok(()),
    }
}

/// Refuse a value the model's own enum does not list, naming the member and the values
/// it accepts.
///
/// `pub` so every handler refuses an off-enum value with the same sentence.
pub fn one_of(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member}: Value '{value}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{}]",
        allowed.join(", ")
    ))
}

/// AWS's wording for "the name and version in this request name no stored plan",
/// built from the request's own `ScalingPlanName` and `ScalingPlanVersion`.
///
/// Every operation that looks a plan up by name and version refuses with this one
/// text, whatever error shape it models the refusal in (`ObjectNotFoundException`
/// where the operation models it, `ValidationException` where it does not), so a
/// client that discriminates not-found variants by message reads the same sentence
/// on every path.
pub fn plan_not_found_message(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

/// Encode a pagination cursor as an opaque token.
///
/// The `kind` prefix is part of the encoded value, so a token minted by one
/// operation does not decode for another.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{kind}:{value}"))
}

/// Decode a cursor minted by [`encode_cursor`] for the same `kind`.
///
/// `None` for anything this service did not mint — the caller answers
/// `InvalidNextTokenException`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(ToString::to_string)
}

/// The page size to use, honouring the documented 1..=50 range and the documented
/// default of 50.
///
/// `Err` carries the refusal message naming the member and the constraint it broke.
pub fn effective_max_results(requested: Option<i32>) -> Result<usize, String> {
    match requested {
        None => Ok(MAX_RESULTS_MAX as usize),
        Some(value) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&value) => Ok(value as usize),
        Some(value) => Err(format!(
            "1 validation error detected: Value '{value}' at 'maxResults' failed to satisfy \
             constraint: Member must be between {MAX_RESULTS_MIN} and {MAX_RESULTS_MAX}"
        )),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState>)
}
