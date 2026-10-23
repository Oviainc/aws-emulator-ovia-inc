// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizer-computeoptimizerservice service.
//!
//! Compute Optimizer's writable surface is the account's ENROLLMENT record plus its
//! RECOMMENDATION PREFERENCES; everything else the service returns is analysis output it
//! derives from observed workloads. Both records live here, together with the rules more
//! than one operation applies to them — scope well-formedness, the preference upsert, the
//! scope precedence `GetEffectiveRecommendationPreferences` resolves with, and the opaque
//! page token `GetRecommendationPreferences` round-trips — so no two handlers can drift
//! from each other's view of the same record.
//!
//! The analysis output this service REPORTS lives here too. The FINDINGS CATALOG holds one
//! row per resource Compute Optimizer has analysed for the account (its finding, its reason
//! codes, its performance risk, what it costs and what the recommendation would cost), part
//! of the store's initial value so every findings reader aggregates the same rows without
//! any of them writing to the store. Idle resource recommendations have their own store
//! ([`IdleRecommendationStore`]): no operation of this API creates one — Compute Optimizer's
//! own analysis of an observed workload does — so the store is what `GetIdleRecommendations`
//! filters, orders and pages over, and the rules it applies to a stored recommendation (the
//! filter vocabulary, the sort dimensions, the ARN -> resource-type catalog) are stated
//! once, here.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use ::aws_smithy_types::DateTime;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    AutoScalingGroupConfiguration, CustomizableMetricParameters, EffectivePreferredResource,
    EffectiveRecommendationPreferences, ExternalMetricsPreference, IdleRecommendation,
    IdleRecommendationFilter, InstanceSavingsEstimationMode, PreferredResource,
    PutRecommendationPreferencesInput, RecommendationPreferencesDetail, Scope, UtilizationMetric,
    UtilizationPreference, IDLE_FINDING_VALUES, IDLE_RECOMMENDATION_FILTER_NAME_VALUES,
    IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES,
};

/// The look-back period in force when no preference sets one. From the model's own
/// `PutRecommendationPreferences$lookBackPeriod` documentation: "When this preference
/// isn't specified, we use the default value `DAYS_14`".
pub const DEFAULT_LOOK_BACK_PERIOD: &str = "DAYS_14";

/// The most preference records this store holds. A `PutRecommendationPreferences` key is
/// built from caller-supplied `resourceType` + `scope`, and a `ResourceArn` scope admits
/// any EC2 instance or Auto Scaling group ARN, so without this bound repeated calls could
/// mint unlimited distinct keys and grow one emulator process without limit. A new key
/// past this count is REFUSED ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::can_hold`]);
/// updating a key the store already holds stays allowed, because it allocates no record.
pub const MAX_PREFERENCE_RECORDS: usize = 1_000;

/// The largest page a paginated reader in this service will build, whatever `maxResults`
/// asks for: the ceiling the model itself puts on `maxResults` (`MaxResults` range
/// 0..1000). Applied in the handler, so no number a request carries can size a response or
/// the allocation that builds it, whatever any other layer does or does not refuse.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The page size a paginated reader uses when the request names none (or names the
/// range's lower bound `0`, which the service documents no meaning for). The service
/// documents no default page size, so this is the emulator's own: a request that asks for
/// no particular size gets one bounded page plus a `nextToken` to walk the rest, rather
/// than the whole store serialised into one response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The `ScopeName` enum values (`crate::ops::types::SCOPE_NAME_VALUES`), named so the
/// precedence and validation rules below read as the model's own vocabulary.
pub const SCOPE_ORGANIZATION: &str = "Organization";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_ACCOUNT_ID: &str = "AccountId";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_RESOURCE_ARN: &str = "ResourceArn";

/// The value an `Organization` scope must carry, per the model's `Scope$value` doc.
pub const ORGANIZATION_SCOPE_VALUE: &str = "ALL_ACCOUNTS";

/// The `Status` enum values this service can be left in by a client call.
pub const STATUS_ACTIVE: &str = "Active";
/// See [`STATUS_ACTIVE`].
pub const STATUS_INACTIVE: &str = "Inactive";

/// The `ResourceType` values whose ARNs `Scope`/`GetEffectiveRecommendationPreferences`
/// accept: "Only EC2 instance and Auto Scaling group ARNs are currently supported."
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`RESOURCE_TYPE_EC2_INSTANCE`].
pub const RESOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";

/// The account's Compute Optimizer enrollment: one record per account, as the service
/// models it (`GetEnrollmentStatus` takes no input and returns exactly one).
#[derive(Debug, Clone)]
pub struct Enrollment {
    /// The persisted `Status`. An account that never called `UpdateEnrollmentStatus` is
    /// not opted in, so this starts `Inactive`.
    pub status: String,
    /// Why the account is in that status.
    pub status_reason: Option<String>,
    /// Whether the last enrollment call included the organization's member accounts.
    pub member_accounts_enrolled: bool,
    /// When the enrollment record was last written; `None` until the first write.
    pub last_updated_timestamp: Option<DateTime>,
    /// How many organization member accounts are opted in. A COUNT, not a flag: it is
    /// zero while no member account is opted in, never absent, because
    /// `GetEnrollmentStatus` reports the count whatever it is.
    pub number_of_member_accounts_opted_in: i32,
}

impl Default for Enrollment {
    fn default() -> Self {
        Self {
            status: STATUS_INACTIVE.to_string(),
            status_reason: None,
            member_accounts_enrolled: false,
            last_updated_timestamp: None,
            number_of_member_accounts_opted_in: 0,
        }
    }
}

impl Enrollment {
    /// Whether the account is opted in. The operations that model
    /// `OptInRequiredException` ("The account is not opted in to Compute Optimizer")
    /// answer it whenever this is false.
    pub fn is_opted_in(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// Whether the analysis this enrollment started is due, i.e. whether
    /// [`ANALYSIS_DELAY`] has elapsed since the enrollment record was last written.
    /// An account that never enrolled has no analysis owed at all.
    ///
    /// The due instant is DERIVED from the stored `lastUpdatedTimestamp` and compared
    /// against the clock; nothing waits for it and no reader writes it.
    pub fn analysis_due(&self) -> bool {
        let now = DateTime::from(::std::time::SystemTime::now()).as_secs_f64();
        self.last_updated_timestamp
            .as_ref()
            .is_some_and(|written| now - written.as_secs_f64() >= ANALYSIS_DELAY.as_secs_f64())
    }
}

/// The identity of a stored preference record: a resource type plus the scope it was
/// written at. A scope-less record is the account-level default, and sorts first because
/// `None` orders before `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreferenceKey {
    /// The `ResourceType` the preference applies to.
    pub resource_type: String,
    /// The `ScopeName`, or `None` for the account-level default.
    pub scope_name: Option<String>,
    /// The `Scope` value, or `None` for the account-level default.
    pub scope_value: Option<String>,
}

impl PreferenceKey {
    /// The key `resource_type` + `scope` addresses. An empty `Scope` object (neither
    /// member set) addresses the same record as no scope at all.
    pub fn new(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            scope_name: scope.and_then(|s| s.name.clone()),
            scope_value: scope.and_then(|s| s.value.clone()),
        }
    }
}

/// One stored recommendation preference record: every preference field a
/// `PutRecommendationPreferences` call can set for one (resourceType, scope) key.
#[derive(Debug, Clone, Default)]
pub struct PreferenceRecord {
    /// The scope exactly as written, so a read reports it back unchanged.
    pub scope: Option<Scope>,
    /// The `ResourceType` this record was written for.
    pub resource_type: String,
    /// `EnhancedInfrastructureMetrics`.
    pub enhanced_infrastructure_metrics: Option<String>,
    /// `InferredWorkloadTypes`.
    pub inferred_workload_types: Option<String>,
    /// `ExternalMetricsPreference`.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`.
    pub look_back_period: Option<String>,
    /// `UtilizationPreferences`.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources`.
    pub preferred_resources: Vec<PreferredResource>,
    /// `SavingsEstimationMode`.
    pub savings_estimation_mode: Option<String>,
}

impl PreferenceRecord {
    /// A record for a key that has nothing written to it yet.
    pub fn for_key(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            scope: scope.cloned(),
            resource_type: resource_type.to_string(),
            ..Self::default()
        }
    }

    /// Apply a `PutRecommendationPreferences` request: "Creates a new recommendation
    /// preference or updates an existing recommendation preference" — so a supplied
    /// field replaces the stored one and a field the request omits is left alone.
    pub fn apply(&mut self, input: &PutRecommendationPreferencesInput) {
        if input.enhanced_infrastructure_metrics.is_some() {
            self.enhanced_infrastructure_metrics = input.enhanced_infrastructure_metrics.clone();
        }
        if input.inferred_workload_types.is_some() {
            self.inferred_workload_types = input.inferred_workload_types.clone();
        }
        if input.external_metrics_preference.is_some() {
            self.external_metrics_preference = input.external_metrics_preference.clone();
        }
        if input.look_back_period.is_some() {
            self.look_back_period = input.look_back_period.clone();
        }
        if let Some(preferences) = &input.utilization_preferences {
            self.utilization_preferences = preferences.clone();
        }
        if let Some(resources) = &input.preferred_resources {
            self.preferred_resources = resources.clone();
        }
        if input.savings_estimation_mode.is_some() {
            self.savings_estimation_mode = input.savings_estimation_mode.clone();
        }
    }

    /// Clear the one preference `name` (a `RecommendationPreferenceName`) names.
    /// `false` when the name addresses no field on this record, which
    /// [`is_recommendation_preference_name`] refuses before anything is written.
    pub fn clear_preference(&mut self, name: &str) -> bool {
        match name {
            "EnhancedInfrastructureMetrics" => self.enhanced_infrastructure_metrics = None,
            "InferredWorkloadTypes" => self.inferred_workload_types = None,
            "ExternalMetricsPreference" => self.external_metrics_preference = None,
            "LookBackPeriodPreference" => self.look_back_period = None,
            "PreferredResources" => self.preferred_resources.clear(),
            "UtilizationPreferences" => self.utilization_preferences.clear(),
            _ => return false,
        }
        true
    }

    /// Whether any preference field is still set. A record with none left is dropped, so
    /// a later read neither reports it nor inherits from it.
    pub fn holds_any_preference(&self) -> bool {
        self.enhanced_infrastructure_metrics.is_some()
            || self.inferred_workload_types.is_some()
            || self.external_metrics_preference.is_some()
            || self.look_back_period.is_some()
            || self.savings_estimation_mode.is_some()
            || !self.utilization_preferences.is_empty()
            || !self.preferred_resources.is_empty()
    }

    /// This record as `GetRecommendationPreferences` reports it.
    pub fn to_detail(&self) -> RecommendationPreferencesDetail {
        RecommendationPreferencesDetail {
            scope: self.scope.clone(),
            resource_type: Some(self.resource_type.clone()),
            enhanced_infrastructure_metrics: self.enhanced_infrastructure_metrics.clone(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: self.look_back_period.clone(),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: (!self.preferred_resources.is_empty()).then(|| {
                self.preferred_resources
                    .iter()
                    .map(effective_preferred_resource)
                    .collect()
            }),
            savings_estimation_mode: self.savings_estimation_mode.clone(),
        }
    }
}

/// Whether `name` is a `RecommendationPreferenceName`, i.e. one of the preference fields
/// [`PreferenceRecord::clear_preference`] can clear. Checked before any deletion, so a
/// request naming something else cannot have cleared the names that came before it.
pub fn is_recommendation_preference_name(name: &str) -> bool {
    PreferenceRecord::default().clear_preference(name)
}

/// A `PreferredResource` as the service reports it back: the stored include and exclude
/// lists plus `effectiveIncludeList`, "the expanded version of your preferred resource's
/// include list" — the preference "resolves any wildcard expressions and returns the
/// effective list of candidate resource type values".
///
/// A value that is no wildcard expands to itself, and so does the catch-all
/// [`INCLUDE_EVERY_RESOURCE_TYPE`], which already names every candidate. Expanding a
/// NARROWER wildcard (`m5*`) needs the EC2 instance-type catalog, which this emulator does
/// not hold, so the effective list is omitted rather than invented for one of those.
pub fn effective_preferred_resource(resource: &PreferredResource) -> EffectivePreferredResource {
    let include_list = resource.include_list.clone();
    let effective_include_list = include_list
        .as_ref()
        .filter(|values| {
            values
                .iter()
                .all(|value| value == INCLUDE_EVERY_RESOURCE_TYPE || !value.contains('*'))
        })
        .cloned();
    EffectivePreferredResource {
        name: resource.name.clone(),
        include_list,
        effective_include_list,
        exclude_list: resource.exclude_list.clone(),
    }
}

/// The only `PreferredResourceName` the service supports: "Compute Optimizer only supports
/// the customization of `Ec2InstanceTypes`" (`PreferredResource$name`).
pub const PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES: &str = "Ec2InstanceTypes";

/// The wildcard expression that includes every candidate resource type, i.e. the state
/// `PreferredResource$excludeList` describes when no preference narrows the candidates: "if
/// this isn't specified, all supported resources are included by default".
pub const INCLUDE_EVERY_RESOURCE_TYPE: &str = "*";

/// The `CustomizableMetricName` values, from the model's enum of the same name.
pub const METRIC_CPU_UTILIZATION: &str = "CpuUtilization";
/// See [`METRIC_CPU_UTILIZATION`].
pub const METRIC_MEMORY_UTILIZATION: &str = "MemoryUtilization";

/// The utilization preferences in force when no preference sets them, from the model's own
/// `PutRecommendationPreferences$utilizationPreferences` documentation: "When this
/// preference isn't specified, we use the following default values. CPU utilization:
/// `P99_5` for threshold, `PERCENT_20` for headroom. Memory utilization: `PERCENT_20` for
/// headroom." The threshold setting "isn't available for memory utilization", so the memory
/// entry carries headroom only.
pub fn default_utilization_preferences() -> Vec<UtilizationPreference> {
    vec![
        UtilizationPreference {
            metric_name: Some(METRIC_CPU_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: Some("P99_5".to_string()),
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
        UtilizationPreference {
            metric_name: Some(METRIC_MEMORY_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: None,
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
    ]
}

/// The preferred resources in force when no preference sets them: the one list name the
/// service supports, including every candidate resource type and excluding none, because
/// `PreferredResource$excludeList` documents that "if this isn't specified, all supported
/// resources are included by default". Projected through
/// [`effective_preferred_resource`], so the reported `effectiveIncludeList` is the
/// expansion of that include list and not a second statement of it.
pub fn default_preferred_resources() -> Vec<EffectivePreferredResource> {
    vec![effective_preferred_resource(&PreferredResource {
        name: Some(PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES.to_string()),
        include_list: Some(vec![INCLUDE_EVERY_RESOURCE_TYPE.to_string()]),
        exclude_list: Some(Vec::new()),
    })]
}

/// An ARN split into the fields the ARN grammar defines:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`. The resource part keeps
/// its own `:` and `/` separators, because every service words it its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedArn<'a> {
    /// The partition field (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The service namespace the resource belongs to (`ec2`, `rds`, …).
    pub service: &'a str,
    /// The region field, empty for a global resource.
    pub region: &'a str,
    /// The account that owns the resource.
    pub account_id: &'a str,
    /// Everything after the account field, separators included.
    pub resource: &'a str,
}

impl<'a> ParsedArn<'a> {
    /// The leading segment of the resource part — the resource KIND (`instance`,
    /// `volume`, `autoScalingGroup`, …), which is what names the resource's type.
    pub fn resource_kind(&self) -> &'a str {
        self.resource.split(['/', ':']).next().unwrap_or_default()
    }
}

/// Split `arn` into its six ARN fields, or `None` when it is not an ARN at all. The one
/// place this service decides what an ARN looks like, so every operation that takes one
/// accepts and refuses the same strings.
pub fn parse_arn(arn: &str) -> Option<ParsedArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || partition.is_empty() || service.is_empty() || resource.is_empty() {
        return None;
    }
    Some(ParsedArn {
        partition,
        service,
        region,
        account_id,
        resource,
    })
}

/// Whether `value` is "a 12-digit Amazon Web Services account ID" — the account-id shape
/// the service documents on `Scope$value`, and the one every operation of this service
/// that takes an account id checks against.
pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// An EC2 instance or Auto Scaling group ARN, split into the parts the preference rules
/// read: the account that owns the resource and the `ResourceType` it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedResourceArn<'a> {
    /// The ARN's account-id field, which an `AccountId`-scoped preference must match.
    pub account_id: &'a str,
    /// The `ResourceType` the ARN names.
    pub resource_type: &'static str,
}

/// Parse an ARN the preference API accepts, or `None` for anything else: "Only EC2
/// instance and Auto Scaling group ARNs are currently supported" (`Scope$value`,
/// `GetEffectiveRecommendationPreferences$resourceArn`).
pub fn parse_resource_arn(arn: &str) -> Option<ParsedResourceArn<'_>> {
    let parsed = parse_arn(arn)?;
    let resource_type = match (parsed.service, parsed.resource_kind()) {
        ("ec2", "instance") => RESOURCE_TYPE_EC2_INSTANCE,
        ("autoscaling", "autoScalingGroup") => RESOURCE_TYPE_AUTO_SCALING_GROUP,
        _ => return None,
    };
    Some(ParsedResourceArn {
        account_id: parsed.account_id,
        resource_type,
    })
}

/// Check a `Scope` against the rules the model's `Scope` shape documents, returning the
/// reason it is not valid. Shared by every operation that takes a scope so one cannot
/// accept a scope another refuses.
///
/// * `name` and `value` are set together — a scope is a name/value pair.
/// * `Organization` — "The `value` must be `ALL_ACCOUNTS`".
/// * `AccountId` — "The `value` must be a 12-digit Amazon Web Services account ID".
/// * `ResourceArn` — "The `value` must be the Amazon Resource Name (ARN) of an EC2
///   instance or an Auto Scaling group".
pub fn scope_rejection(scope: Option<&Scope>) -> Option<String> {
    let scope = scope?;
    match (scope.name.as_deref(), scope.value.as_deref()) {
        (None, None) => None,
        (Some(name), None) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope value must be \
             supplied with the scope name {name}."
        )),
        (None, Some(value)) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope name must be \
             supplied with the scope value {value}."
        )),
        (Some(SCOPE_ORGANIZATION), Some(value)) if value != ORGANIZATION_SCOPE_VALUE => {
            Some(format!(
                "The value supplied for parameter scope is not valid: the value of an \
                 {SCOPE_ORGANIZATION} scope must be {ORGANIZATION_SCOPE_VALUE}, not {value}."
            ))
        }
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if !is_account_id(value) => Some(format!(
            "The value supplied for parameter scope is not valid: the value of an \
             {SCOPE_ACCOUNT_ID} scope must be a 12-digit Amazon Web Services account ID, \
             not {value}."
        )),
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if parse_resource_arn(value).is_none() => {
            Some(format!(
                "The value supplied for parameter scope is not valid: the value of a \
                 {SCOPE_RESOURCE_ARN} scope must be the Amazon Resource Name (ARN) of an EC2 \
                 instance or an Auto Scaling group, not {value}."
            ))
        }
        (Some(_), Some(_)) => None,
    }
}

/// How specifically a stored key applies to one resource, smallest first: a resource-level
/// preference beats an account-level one, which beats the account default, which beats the
/// organization-level one. `None` when the key does not apply to this resource at all.
///
/// This is the precedence `GetEffectiveRecommendationPreferences` resolves with —
/// "Considers all applicable preferences that you might have set at the resource, account,
/// and organization level".
fn scope_precedence(key: &PreferenceKey, arn: &str, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if value == arn => Some(0),
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if value == account_id => Some(1),
        (None, _) => Some(2),
        (Some(SCOPE_ORGANIZATION), _) => Some(3),
        _ => None,
    }
}

/// The opaque `nextToken` `GetRecommendationPreferences` hands out: the key the next page
/// starts at, hex-encoded so a client cannot read or construct one.
pub fn encode_page_token(key: &PreferenceKey) -> String {
    let encoded =
        ::serde_json::json!([key.resource_type, key.scope_name, key.scope_value]).to_string();
    ::hex::encode(encoded)
}

/// The key a `nextToken` this service issued stands for, or `None` for a token it did not.
pub fn decode_page_token(token: &str) -> Option<PreferenceKey> {
    let bytes = ::hex::decode(token).ok()?;
    let (resource_type, scope_name, scope_value) =
        ::serde_json::from_slice::<(String, Option<String>, Option<String>)>(&bytes).ok()?;
    Some(PreferenceKey {
        resource_type,
        scope_name,
        scope_value,
    })
}

/// The opaque `nextToken` a recommendation reader hands out: the ARN of the resource the
/// next page starts at, hex-encoded so a client can neither read nor construct one.
pub fn encode_resource_page_token(resource_arn: &str) -> String {
    ::hex::encode(::serde_json::json!({ "startAt": resource_arn }).to_string())
}

/// The resource ARN a `nextToken` this service issued stands for, or `None` for a token
/// it did not — including a token issued for a different page sequence, whose encoded
/// shape is not this one.
pub fn decode_resource_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("startAt")?
        .as_str()
        .map(::std::string::ToString::to_string)
}

// ===========================================================================
// The Auto Scaling group analysis the recommendation readers report
// ===========================================================================

/// How long after an account opts in the analysis of its resources is due.
///
/// Compute Optimizer derives recommendations from the utilization metrics it observes
/// AFTER an account enrolls — "after you opt in, it can take up to 24 hours for
/// Compute Optimizer to deliver your recommendations" — so an account that has just
/// enrolled has none yet. The emulator owes that transition on an emulator-scale span
/// rather than a real day (chosen, not documented), so a caller that reads straight
/// after opting in still observes the pre-analysis state. The instant is DERIVED from
/// the enrollment record's `lastUpdatedTimestamp` and never waited for.
pub const ANALYSIS_DELAY: Duration = Duration::from_millis(500);

/// Hours in a month, the factor that turns an hourly on-demand price into the monthly
/// figures Compute Optimizer reports (`estimatedMonthlySavings`). 730 = 365 × 24 / 12,
/// the convention Amazon Web Services' own pricing pages use.
pub const HOURS_PER_MONTH: f64 = 730.0;

/// The `MetricName` values an Auto Scaling group recommendation reports. "The `Cpu` and
/// `Memory` metrics are the only projected utilization metrics returned"
/// (`AutoScalingGroupRecommendationOption$projectedUtilizationMetrics`), and both are
/// percentages ("Units: Percent", `UtilizationMetric$name`).
pub const METRIC_CPU: &str = "Cpu";
/// See [`METRIC_CPU`].
pub const METRIC_MEMORY: &str = "Memory";

/// The one `MetricStatistic` this API reports: "The Compute Optimizer API, Command Line
/// Interface (CLI), and SDKs return utilization metrics using only the `Maximum`
/// statistic" (`UtilizationMetric$statistic`).
pub const METRIC_STATISTIC_MAXIMUM: &str = "Maximum";

/// The `AsgType` values, from the model's enum of the same name: "whether the EC2 Auto
/// Scaling group has a single instance type or a mixed instance type configuration".
pub const ASG_TYPE_SINGLE_INSTANCE_TYPE: &str = "SingleInstanceType";
/// See [`ASG_TYPE_SINGLE_INSTANCE_TYPE`].
pub const ASG_TYPE_MIXED_INSTANCE_TYPES: &str = "MixedInstanceTypes";

/// The `CpuVendorArchitecture` values (`shared_types::CPU_VENDOR_ARCHITECTURE_VALUES`).
/// `CURRENT` is a candidate of the same CPU vendor and architecture as the group runs
/// today; `AWS_ARM64` is a Graviton candidate — "when you specify `AWS_ARM64` with a
/// `GetAutoScalingGroupRecommendations` request, Compute Optimizer returns
/// recommendations that consist of Graviton instance types only".
pub const CPU_ARCHITECTURE_CURRENT: &str = "CURRENT";
/// See [`CPU_ARCHITECTURE_CURRENT`].
pub const CPU_ARCHITECTURE_AWS_ARM64: &str = "AWS_ARM64";

/// The `SavingsEstimationMode` preference value that activates reporting savings net of
/// Savings Plans and Reserved Instances discounts: "Specify the `AfterDiscounts` status
/// to activate the preference, or specify `BeforeDiscounts` to deactivate" it.
pub const SAVINGS_ESTIMATION_AFTER_DISCOUNTS: &str = "AfterDiscounts";

/// The `InstanceSavingsEstimationModeSource` the emulator's figures come from: every
/// money figure a recommendation reports is computed from the public on-demand prices
/// stored with the analysis ([`GroupConfiguration::hourly_price`]), not from an
/// account's own Cost Explorer or Cost Optimization Hub data.
pub const SAVINGS_ESTIMATION_SOURCE: &str = "PublicPricing";

/// The `InferredWorkloadTypesPreference` value that leaves the inference ON: "The
/// inferred workload type feature is active by default. To deactivate it, create a
/// recommendation preference" (`PutRecommendationPreferences$inferredWorkloadTypes`).
pub const INFERRED_WORKLOAD_TYPES_ACTIVE: &str = "Active";

/// One configuration of an Auto Scaling group: the configuration it runs today, or one
/// the analysis found as a candidate for it. Both sides are the same shape because the
/// service reports them through the same `AutoScalingGroupConfiguration`, and because
/// every derived figure (cost, saving, projected utilization, migration effort) is a
/// comparison BETWEEN two of them.
#[derive(Debug, Clone)]
pub struct GroupConfiguration {
    /// The instance type(s) of the configuration: one for a `SingleInstanceType` group,
    /// several for a `MixedInstanceTypes` one.
    pub instance_types: Vec<String>,
    /// The `AllocationStrategy`, which "is only available for EC2 Auto Scaling groups
    /// with mixed instance types", so a single-instance-type configuration has none.
    pub allocation_strategy: Option<String>,
    /// `desiredCapacity`: the number of instances the configuration runs.
    pub desired_capacity: i32,
    /// `minSize`.
    pub min_size: i32,
    /// `maxSize`.
    pub max_size: i32,
    /// vCPUs of ONE instance of this configuration.
    pub vcpus: f64,
    /// Memory, in GiB, of ONE instance of this configuration.
    pub memory_gib: f64,
    /// The public on-demand price of ONE instance-hour, in [`CURRENCY_CODE`].
    pub hourly_price: f64,
    /// The configuration's `CpuVendorArchitecture`: [`CPU_ARCHITECTURE_CURRENT`] or
    /// [`CPU_ARCHITECTURE_AWS_ARM64`].
    pub architecture: String,
    /// `PerformanceRisk` (0-4) of running the analysed workload on this configuration:
    /// "the likelihood of the recommended instance type not meeting the resource needs
    /// of your workload", 0 meaning it "is predicted to always provide enough hardware
    /// capability".
    pub performance_risk: f64,
}

impl GroupConfiguration {
    /// Whether this configuration mixes instance types, which is what
    /// `AutoScalingGroupConfiguration$type` reports.
    pub fn is_mixed(&self) -> bool {
        self.instance_types.len() > 1
    }

    /// The `AsgType` of this configuration.
    pub fn asg_type(&self) -> &'static str {
        if self.is_mixed() {
            ASG_TYPE_MIXED_INSTANCE_TYPES
        } else {
            ASG_TYPE_SINGLE_INSTANCE_TYPE
        }
    }

    /// The estimated monthly on-demand cost of running this configuration: the price of
    /// one instance-hour × the instances it runs × [`HOURS_PER_MONTH`].
    pub fn monthly_cost(&self) -> f64 {
        round_hundredths(self.hourly_price * f64::from(self.desired_capacity) * HOURS_PER_MONTH)
    }

    /// The vCPUs the whole group runs on this configuration.
    pub fn total_vcpus(&self) -> f64 {
        self.vcpus * f64::from(self.desired_capacity)
    }

    /// The memory, in GiB, the whole group runs on this configuration.
    pub fn total_memory_gib(&self) -> f64 {
        self.memory_gib * f64::from(self.desired_capacity)
    }

    /// Whether `other` is a different configuration of the same group — a different set
    /// of instance types or a different capacity. A candidate that differs from the
    /// group's own configuration IS a recommendation, which is what makes the group
    /// `NotOptimized` ([`AnalyzedAutoScalingGroup::finding`]).
    pub fn differs_from(&self, other: &Self) -> bool {
        self.instance_types != other.instance_types
            || self.desired_capacity != other.desired_capacity
            || self.min_size != other.min_size
            || self.max_size != other.max_size
    }

    /// This configuration as the modelled `AutoScalingGroupConfiguration`.
    ///
    /// `estimatedInstanceHourReductionPercentage` is left unreported: it "is only
    /// available for EC2 Auto Scaling groups with scaling policies", and this emulator
    /// records no scaling policy for a group, so there is no reduction to project.
    pub fn to_wire(&self) -> AutoScalingGroupConfiguration {
        AutoScalingGroupConfiguration {
            desired_capacity: self.desired_capacity,
            min_size: self.min_size,
            max_size: self.max_size,
            instance_type: (!self.is_mixed())
                .then(|| self.instance_types.first().cloned())
                .flatten(),
            allocation_strategy: self.allocation_strategy.clone(),
            estimated_instance_hour_reduction_percentage: None,
            r#type: Some(self.asg_type().to_string()),
            mixed_instance_types: self.is_mixed().then(|| self.instance_types.clone()),
        }
    }
}

/// One Auto Scaling group Compute Optimizer has analysed: the group as it is configured,
/// the utilization it observed, and the candidate configurations the analysis found for
/// it. Every value a recommendation REPORTS is either one of these stored observations
/// or a figure computed from them, so two readers of the same row cannot disagree.
#[derive(Debug, Clone)]
pub struct AnalyzedAutoScalingGroup {
    /// The account that owns the group.
    pub account_id: String,
    /// `autoScalingGroupArn`.
    pub arn: String,
    /// `autoScalingGroupName`.
    pub name: String,
    /// The group's current configuration.
    pub current: GroupConfiguration,
    /// The utilization the analysis observed, as (`MetricName`, value) pairs at the
    /// [`METRIC_STATISTIC_MAXIMUM`] statistic — the only statistic this API reports.
    pub utilization: Vec<(String, f64)>,
    /// `currentPerformanceRisk`: "the risk of the current Auto Scaling group not meeting
    /// the performance needs of its workloads", a `CurrentPerformanceRisk` value.
    pub current_performance_risk: String,
    /// The workload types the analysis inferred for the instances in the group
    /// (`InferredWorkloadType` values), empty where it inferred none.
    pub inferred_workload_types: Vec<String>,
    /// The candidate configurations the analysis found, best first — so the reported
    /// `rank` is a candidate's position in this list ("the top recommendation option is
    /// ranked as 1").
    pub candidates: Vec<GroupConfiguration>,
    /// The Savings Plans / Reserved Instances discount rate that applies to this
    /// group's compute spend, as a fraction of the on-demand price, or `0.0` where none
    /// does. It is what separates `savingsOpportunityAfterDiscounts` from the on-demand
    /// `savingsOpportunity`.
    pub savings_plans_discount_rate: f64,
}

impl AnalyzedAutoScalingGroup {
    /// The group's `Finding`.
    ///
    /// "An Auto Scaling group is considered not optimized when Compute Optimizer
    /// identifies a recommendation that can provide better performance for your
    /// workload", and "optimized when Compute Optimizer determines that the group is
    /// correctly provisioned to run your workload based on the chosen instance type" —
    /// so the group is `NotOptimized` exactly when the analysis found a candidate that
    /// is not the group's own configuration, and `Optimized` when its own configuration
    /// is the best the analysis found.
    pub fn finding(&self) -> &'static str {
        if self
            .candidates
            .iter()
            .any(|candidate| candidate.differs_from(&self.current))
        {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// The observed utilization as the modelled `UtilizationMetrics`.
    pub fn utilization_metrics(&self) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, value)| UtilizationMetric {
                name: Some(name.clone()),
                statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                value: round_hundredths(*value),
            })
            .collect()
    }

    /// The utilization the observed workload is projected to reach on `candidate`.
    ///
    /// The same work runs on whatever capacity the configuration provides, so each
    /// percentage scales with the ratio of the capacity it was observed on to the
    /// capacity it is projected onto — CPU by the group's total vCPUs, memory by the
    /// group's total memory — and is reported as a percentage, so it is capped at 100.
    pub fn projected_utilization_metrics(
        &self,
        candidate: &GroupConfiguration,
    ) -> Vec<UtilizationMetric> {
        self.utilization
            .iter()
            .map(|(name, observed)| {
                let (from, to) = if name == METRIC_MEMORY {
                    (self.current.total_memory_gib(), candidate.total_memory_gib())
                } else {
                    (self.current.total_vcpus(), candidate.total_vcpus())
                };
                let projected = if to <= 0.0 {
                    *observed
                } else {
                    (observed * from / to).min(100.0)
                };
                UtilizationMetric {
                    name: Some(name.clone()),
                    statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
                    value: round_hundredths(projected),
                }
            })
            .collect()
    }

    /// The `MigrationEffort` of moving this group onto `candidate`, by the rule the
    /// model states: "The migration effort is `VeryLow` if both the current and
    /// recommended instance types are of the same CPU architecture", `Low` "if Amazon
    /// EMR is the inferred workload type and an Amazon Web Services Graviton instance
    /// type is recommended", and `Medium` "if a workload type couldn't be inferred but
    /// an Amazon Web Services Graviton instance type is recommended".
    ///
    /// The model names Amazon EMR as the example of an inferred workload and states no
    /// rule for any other, so the emulator generalises its `Low` case to ANY workload type
    /// the analysis inferred: what the two documented Graviton cases differ on is whether
    /// a workload type was inferred at all, and naming one of the nine
    /// `InferredWorkloadType` values would make the effort of a Kafka or a Redis group
    /// depend on a value the rule says nothing about.
    ///
    /// `infers_workload_types` is whether the account leaves the workload-type inference
    /// active: with the inference deactivated no workload type is inferred for the
    /// group, which is the `Medium` case.
    pub fn migration_effort(
        &self,
        candidate: &GroupConfiguration,
        infers_workload_types: bool,
    ) -> &'static str {
        if candidate.architecture != CPU_ARCHITECTURE_AWS_ARM64 {
            "VeryLow"
        } else if !infers_workload_types || self.inferred_workload_types.is_empty() {
            "Medium"
        } else {
            "Low"
        }
    }

    /// The saving `candidate` offers against the group's own configuration, on the
    /// public on-demand prices stored with the analysis: the monthly cost it removes,
    /// and that saving as a percentage of the group's current monthly cost.
    ///
    /// `discount_rate` nets both sides of the comparison down by a Savings Plans /
    /// Reserved Instances discount, which is how `savingsOpportunityAfterDiscounts` is
    /// computed; pass `0.0` for the on-demand `savingsOpportunity`.
    pub fn savings(&self, candidate: &GroupConfiguration, discount_rate: f64) -> (f64, f64) {
        let retained = 1.0 - discount_rate;
        let current = round_hundredths(self.current.monthly_cost() * retained);
        let projected = round_hundredths(candidate.monthly_cost() * retained);
        let saving = round_hundredths((current - projected).max(0.0));
        (saving, percentage_of(saving, current))
    }

    /// This group as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it: the finding and the costs the analysis reached, with the RECOMMENDED
    /// cost being the top-ranked candidate's — "the top recommendation option is ranked as
    /// 1" — or the group's own cost where the analysis found no other candidate.
    ///
    /// The projection is what keeps one group's analysis in ONE place: the summary reader
    /// and `GetAutoScalingGroupRecommendations` both answer from this row's source, so
    /// they cannot disagree about a group's finding, its performance risk or what adopting
    /// its recommendation saves. An Auto Scaling group recommendation reports no finding
    /// reason codes (`AutoScalingGroupRecommendation` declares no such member), so the
    /// row carries none.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_AUTO_SCALING_GROUP.to_string(),
            resource_arn: self.arn.clone(),
            finding: self.finding().to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: self.current_performance_risk.clone(),
            current_monthly_cost: self.current.monthly_cost(),
            recommended_monthly_cost: self
                .candidates
                .first()
                .unwrap_or(&self.current)
                .monthly_cost(),
            inferred_workload_types: self.inferred_workload_types.clone(),
        }
    }
}

/// `part` as a percentage of `whole`, to two decimal places; zero when `whole` is zero,
/// because no percentage of nothing is defined.
pub fn percentage_of(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    round_hundredths(part / whole * 100.0)
}

/// Round a derived figure to two decimal places, so a money amount lands on whole cents
/// and a percentage does not report binary-float noise.
pub fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The number of days a `LookBackPeriodPreference` names
/// (`LOOK_BACK_PERIOD_PREFERENCE_VALUES` are `DAYS_14` | `DAYS_32` | `DAYS_93`), which is
/// what `lookBackPeriodInDays` reports: "the number of days for which utilization metrics
/// were analyzed". Falls back to the service default's length for a value outside that
/// vocabulary, which no stored preference can be.
pub fn look_back_period_in_days(preference: &str) -> f64 {
    preference
        .strip_prefix("DAYS_")
        .and_then(|days| days.parse::<f64>().ok())
        .unwrap_or(14.0)
}

/// The preferences in force for one resource: what the account stored for it, with the
/// service's documented default for every preference it never set.
///
/// This is the same resolution `GetEffectiveRecommendationPreferences` reports, applied
/// to the resource a recommendation is about, so the preferences a recommendation says it
/// was generated under are the preferences a read of that resource reports.
#[derive(Debug, Clone)]
pub struct EffectivePreferences {
    /// `EnhancedInfrastructureMetrics`, reported only where a record sets it.
    pub enhanced_infrastructure_metrics: Option<String>,
    /// `InferredWorkloadTypesPreference`, which defaults to
    /// [`INFERRED_WORKLOAD_TYPES_ACTIVE`] because "the inferred workload type feature is
    /// active by default".
    pub inferred_workload_types: String,
    /// `ExternalMetricsPreference`, reported only where a record sets it.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`, which defaults to [`DEFAULT_LOOK_BACK_PERIOD`].
    pub look_back_period: String,
    /// `UtilizationPreferences`: the stored preference, or the service's documented
    /// default for a resource type that HAS one — "you can only set CPU and memory
    /// utilization preferences for the Amazon EC2 instance resource type", so the list is
    /// empty for every other resource type.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources`, which default to [`default_preferred_resources`].
    pub preferred_resources: Vec<EffectivePreferredResource>,
    /// `SavingsEstimationMode`, reported only where a record sets it: the preference is
    /// what "activates" savings net of discounts.
    pub savings_estimation_mode: Option<String>,
}

impl EffectivePreferences {
    /// Whether savings are reported net of Savings Plans and Reserved Instances
    /// discounts, i.e. whether the account activated the preference.
    pub fn after_discounts(&self) -> bool {
        self.savings_estimation_mode.as_deref() == Some(SAVINGS_ESTIMATION_AFTER_DISCOUNTS)
    }

    /// Whether Compute Optimizer infers workload types for the resource.
    pub fn infers_workload_types(&self) -> bool {
        self.inferred_workload_types == INFERRED_WORKLOAD_TYPES_ACTIVE
    }

    /// These preferences as the modelled `EffectiveRecommendationPreferences` a
    /// recommendation carries, under the CPU vendor architectures the request asked for.
    ///
    /// `utilizationPreferences` is reported only where the resource type has one at all
    /// (see the field's doc), so an Auto Scaling group's recommendation carries none.
    /// `savingsEstimationMode` names where the money figures came from
    /// ([`SAVINGS_ESTIMATION_SOURCE`]).
    pub fn to_wire(
        &self,
        cpu_vendor_architectures: Vec<String>,
    ) -> EffectiveRecommendationPreferences {
        EffectiveRecommendationPreferences {
            cpu_vendor_architectures: Some(cpu_vendor_architectures),
            enhanced_infrastructure_metrics: self.enhanced_infrastructure_metrics.clone(),
            inferred_workload_types: Some(self.inferred_workload_types.clone()),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: Some(self.look_back_period.clone()),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: Some(self.preferred_resources.clone()),
            savings_estimation_mode: Some(InstanceSavingsEstimationMode {
                source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
            }),
        }
    }
}

/// The Auto Scaling group analysis this emulator holds, built once with the store.
///
/// The emulator observes no real workload, so WHICH groups are analysed is a fixed set
/// rather than something derived from traffic; every figure a recommendation reports is
/// nevertheless computed from the observations stored here (cost, saving, percentage,
/// projected utilization, finding, rank and migration effort all follow from these rows),
/// and a reader of a row never writes to it.
///
/// The rows span the shapes a client must handle: a single-instance-type group that is
/// over-provisioned (two candidates, one of them Graviton), a group that is correctly
/// provisioned (its own configuration is the only candidate, so no saving), and a
/// mixed-instance-type group with a Savings Plans discount against its spend.
fn seed_auto_scaling_group_analyses() -> Vec<AnalyzedAutoScalingGroup> {
    let account_id = ::engine::emulator_account_id().to_string();
    let region = "us-east-1";
    let arn = |group_id: &str, name: &str| {
        ::engine::emulator_arn(
            "autoscaling",
            region,
            &format!("autoScalingGroup:{group_id}:autoScalingGroupName/{name}"),
        )
    };

    vec![
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            arn: arn("6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e51", "web-tier-asg"),
            name: "web-tier-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["m5.2xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 4,
                min_size: 2,
                max_size: 8,
                vcpus: 8.0,
                memory_gib: 32.0,
                hourly_price: 0.384,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 12.4),
                (METRIC_MEMORY.to_string(), 28.6),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["Nginx".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m5.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.192,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec!["m6g.xlarge".to_string()],
                    allocation_strategy: None,
                    desired_capacity: 4,
                    min_size: 2,
                    max_size: 8,
                    vcpus: 4.0,
                    memory_gib: 16.0,
                    hourly_price: 0.154,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id: account_id.clone(),
            arn: arn("6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e52", "batch-worker-asg"),
            name: "batch-worker-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 86.5),
                (METRIC_MEMORY.to_string(), 71.2),
            ],
            current_performance_risk: PERFORMANCE_RISK_LOW.to_string(),
            inferred_workload_types: Vec::new(),
            // The group's own configuration is the only candidate the analysis found,
            // which is what makes it `Optimized`: there is no saving to report.
            candidates: vec![GroupConfiguration {
                instance_types: vec!["c5.4xlarge".to_string()],
                allocation_strategy: None,
                desired_capacity: 6,
                min_size: 6,
                max_size: 12,
                vcpus: 16.0,
                memory_gib: 32.0,
                hourly_price: 0.68,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 1.0,
            }],
            savings_plans_discount_rate: 0.0,
        },
        AnalyzedAutoScalingGroup {
            account_id,
            arn: arn("6f3a7a2c-0d6c-4a3e-9c2b-0a1b2c3d4e53", "mixed-fleet-asg"),
            name: "mixed-fleet-asg".to_string(),
            current: GroupConfiguration {
                instance_types: vec![
                    "m5.large".to_string(),
                    "m5a.large".to_string(),
                    "m6i.large".to_string(),
                ],
                allocation_strategy: Some("LowestPrice".to_string()),
                desired_capacity: 10,
                min_size: 4,
                max_size: 20,
                vcpus: 2.0,
                memory_gib: 8.0,
                hourly_price: 0.096,
                architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                performance_risk: 0.0,
            },
            utilization: vec![
                (METRIC_CPU.to_string(), 9.1),
                (METRIC_MEMORY.to_string(), 22.3),
            ],
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            inferred_workload_types: vec!["AmazonEmr".to_string()],
            candidates: vec![
                GroupConfiguration {
                    instance_types: vec!["m6g.large".to_string(), "m6gd.large".to_string()],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.077,
                    architecture: CPU_ARCHITECTURE_AWS_ARM64.to_string(),
                    performance_risk: 1.0,
                },
                GroupConfiguration {
                    instance_types: vec![
                        "m5.large".to_string(),
                        "m5a.large".to_string(),
                        "m6i.large".to_string(),
                    ],
                    allocation_strategy: Some("LowestPrice".to_string()),
                    desired_capacity: 6,
                    min_size: 4,
                    max_size: 20,
                    vcpus: 2.0,
                    memory_gib: 8.0,
                    hourly_price: 0.096,
                    architecture: CPU_ARCHITECTURE_CURRENT.to_string(),
                    performance_risk: 1.0,
                },
            ],
            savings_plans_discount_rate: 0.28,
        },
    ]
}

/// The currency every monetary figure in the findings catalog is denominated in, one of
/// the model's `Currency` values (`crate::ops::types::CURRENCY_VALUES`).
pub const CURRENCY_USD: &str = "USD";

/// The `CurrentPerformanceRisk` values (`crate::ops::types::CURRENT_PERFORMANCE_RISK_VALUES`),
/// named so the catalog and the rating counts read as the model's own vocabulary.
pub const PERFORMANCE_RISK_VERY_LOW: &str = "VeryLow";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_LOW: &str = "Low";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_MEDIUM: &str = "Medium";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_HIGH: &str = "High";

/// The `Finding` values (`crate::ops::types::FINDING_VALUES`).
pub const FINDING_UNDERPROVISIONED: &str = "Underprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OVERPROVISIONED: &str = "Overprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OPTIMIZED: &str = "Optimized";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_NOT_OPTIMIZED: &str = "NotOptimized";

/// The `RecommendationSourceType` values the findings catalog holds resources of
/// (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`).
pub const SOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LAMBDA_FUNCTION: &str = "LambdaFunction";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_ECS_SERVICE: &str = "EcsService";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_RDS_DB_INSTANCE: &str = "RdsDBInstance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LICENSE: &str = "License";

/// The `Finding` values a summary of `resource_type` reports a count for, from the
/// `GetRecommendationSummaries` documentation: "It returns the number of: Amazon EC2
/// instances in an account that are Underprovisioned, Overprovisioned, or Optimized.
/// EC2 Auto Scaling groups in an account that are NotOptimized, or Optimized. Amazon EBS
/// volumes in an account that are NotOptimized, or Optimized. Lambda functions in an
/// account that are NotOptimized, or Optimized. Amazon ECS services in an account that
/// are Underprovisioned, Overprovisioned, or Optimized. Commercial software licenses in
/// an account that are InsufficientMetrics, NotOptimized or Optimized. Amazon Aurora and
/// Amazon RDS databases in an account that are Underprovisioned, Overprovisioned,
/// Optimized, or NotOptimized."
///
/// Every value here is one of the model's `Finding` enum, because that is what
/// `Summary$name` targets: the one documented finding outside it — a license's
/// `InsufficientMetrics`, which belongs to `LicenseFinding` — is therefore not a count a
/// `Summary` can carry, so a `License` summary reports the two findings `Finding` holds.
///
/// An empty slice for a resource type whose findings this emulator does not summarise, so
/// a reader reports no summary for it rather than an invented vocabulary.
pub fn findings_reported_for(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        SOURCE_TYPE_EC2_INSTANCE | SOURCE_TYPE_ECS_SERVICE => &[
            FINDING_UNDERPROVISIONED,
            FINDING_OVERPROVISIONED,
            FINDING_OPTIMIZED,
        ],
        SOURCE_TYPE_AUTO_SCALING_GROUP
        | SOURCE_TYPE_EBS_VOLUME
        | SOURCE_TYPE_LAMBDA_FUNCTION
        | SOURCE_TYPE_LICENSE => &[FINDING_NOT_OPTIMIZED, FINDING_OPTIMIZED],
        SOURCE_TYPE_RDS_DB_INSTANCE => &[
            FINDING_UNDERPROVISIONED,
            FINDING_OVERPROVISIONED,
            FINDING_OPTIMIZED,
            FINDING_NOT_OPTIMIZED,
        ],
        _ => &[],
    }
}

/// One resource Compute Optimizer has analysed for an account: the optimization finding
/// it carries, what it costs today, what the recommended configuration would cost, and
/// the risk its current configuration runs. This is the row the findings readers report
/// and `GetRecommendationSummaries` aggregates, so every figure a summary carries is
/// computed from these and not stated twice.
#[derive(Debug, Clone)]
pub struct AnalysedResource {
    /// The account that owns the resource.
    pub account_id: String,
    /// The `RecommendationSourceType` the resource is analysed as.
    pub resource_type: String,
    /// The resource's ARN.
    pub resource_arn: String,
    /// The `Finding` the analysis reached for it.
    pub finding: String,
    /// The `FindingReasonCode`s behind that finding; empty where the finding has none.
    pub finding_reason_codes: Vec<String>,
    /// The `CurrentPerformanceRisk` the current configuration runs.
    pub current_performance_risk: String,
    /// What the resource costs per month in its current configuration.
    pub current_monthly_cost: f64,
    /// What the recommended configuration would cost per month. Equal to
    /// [`Self::current_monthly_cost`] for a resource whose configuration the analysis
    /// would not change, and higher for one it would grow.
    pub recommended_monthly_cost: f64,
    /// The `InferredWorkloadType`s running on the resource; empty where none was inferred.
    pub inferred_workload_types: Vec<String>,
}

impl AnalysedResource {
    /// The monthly saving of adopting the recommendation for this resource: what it costs
    /// today less what the recommended configuration costs, and NOTHING where the
    /// recommendation does not cost less — an underprovisioned resource is recommended a
    /// larger configuration, which saves no money.
    pub fn monthly_savings(&self) -> f64 {
        (self.current_monthly_cost - self.recommended_monthly_cost).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Idle resource recommendations
//
// Compute Optimizer "generates recommendations for idle resources that meet a
// specific set of requirements" from the utilization it OBSERVES, so no request to
// this API produces one: `GetIdleRecommendations` only reports what the analysis
// recorded. Everything below is therefore about reading that record — the filter
// vocabulary, the sort dimensions, the ARN -> resource-type catalog and the page
// cursor — and nothing below writes a recommendation.
// ---------------------------------------------------------------------------

/// The largest page `GetIdleRecommendations` will build, whatever `maxResults` asks for,
/// and the size it uses when a request names none: the ceiling the model itself puts on
/// `maxResults` (`IdleMaxResults` range 0..100). Applied in the handler, so no number a
/// request carries can size a response or the allocation that builds it.
pub const IDLE_MAX_PAGE_SIZE: usize = 100;

/// The `IdleRecommendationFilterName` values
/// (`crate::ops::types::IDLE_RECOMMENDATION_FILTER_NAME_VALUES`), named so the filter rule
/// below reads as the model's own vocabulary.
pub const IDLE_FILTER_FINDING: &str = "Finding";
/// See [`IDLE_FILTER_FINDING`].
pub const IDLE_FILTER_RESOURCE_TYPE: &str = "ResourceType";

/// The `Dimension` values `OrderBy$dimension` sorts on (`crate::ops::types::DIMENSION_VALUES`).
pub const DIMENSION_SAVINGS_VALUE: &str = "SavingsValue";
/// See [`DIMENSION_SAVINGS_VALUE`].
pub const DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT: &str = "SavingsValueAfterDiscount";

/// The `Order` value that sorts descending (`crate::ops::types::ORDER_VALUES` is
/// `Asc | Desc`); see [`idle_sorts_descending`].
pub const ORDER_DESC: &str = "Desc";

/// The `IdleRecommendationError$code` this service reports for the one condition it can
/// report: the request named a resource the service holds no idle recommendation for, which
/// is exactly what `IdleRecommendationError` describes ("Returns of list of resources that
/// doesn't have idle recommendations"). The model documents `code` only as "The error code"
/// and names no vocabulary for it, so this is the emulator's own stable code; the
/// requirement the entry carries is its `identifier`.
pub const IDLE_NO_RECOMMENDATION_CODE: &str = "ResourceNotFound";

/// The idle resource recommendations this service holds, keyed by the resource ARN that
/// identifies one, in the order they were recorded.
///
/// Compute Optimizer's own analysis of an observed workload is what produces an idle
/// resource recommendation, so NO operation of this API creates one: the store holds the
/// modelled `IdleRecommendation` as recorded, and `GetIdleRecommendations` reports entries
/// straight out of it, never from the request. `insert` is the only writer, so the record
/// order a page walks and the by-ARN index a lookup uses cannot drift apart.
#[derive(Debug, Default)]
pub struct IdleRecommendationStore {
    /// Resource ARNs in record order, so a page walks a stable sequence.
    order: Vec<String>,
    by_arn: HashMap<String, IdleRecommendation>,
}

impl IdleRecommendationStore {
    /// Record `recommendation` under `resource_arn`. Re-recording the same ARN replaces the
    /// stored recommendation in place, keeping its position in the order — a refreshed
    /// analysis of one resource does not reorder the rest.
    pub fn insert(&mut self, resource_arn: String, recommendation: IdleRecommendation) {
        if !self.by_arn.contains_key(&resource_arn) {
            self.order.push(resource_arn.clone());
        }
        self.by_arn.insert(resource_arn, recommendation);
    }

    /// The recommendation held for `resource_arn`, or `None` when the service holds none —
    /// which is what puts that ARN in a response's `errors` list.
    pub fn get(&self, resource_arn: &str) -> Option<&IdleRecommendation> {
        self.by_arn.get(resource_arn)
    }

    /// Every recorded recommendation, in record order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &IdleRecommendation> {
        self.order.iter().filter_map(|arn| self.by_arn.get(arn))
    }

    /// How many recommendations are recorded.
    pub fn len(&self) -> usize {
        self.by_arn.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_arn.is_empty()
    }
}

/// The `IdleRecommendationResourceType` an ARN names, or `None` when the ARN names no
/// resource kind idle recommendations cover.
///
/// The catalog is the model's own `IdleRecommendationResourceType` value set
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`), each value paired with
/// the `<service>:<resource-kind>` ARN fields AWS gives that resource. A DocumentDB cluster
/// carries the `rds` namespace, and the value set declares no other cluster of that
/// namespace — no Aurora or RDS DB cluster value — so `rds:cluster` maps to
/// `DocumentDBCluster`, the one cluster type idle recommendations cover.
pub fn idle_resource_type_of_arn(arn: &str) -> Option<&'static str> {
    let parsed = parse_arn(arn)?;
    let resource_type = match (parsed.service, parsed.resource_kind()) {
        ("ec2", "instance") => "EC2Instance",
        ("ec2", "volume") => "EBSVolume",
        ("ec2", "natgateway") => "NatGateway",
        ("autoscaling", "autoScalingGroup") => "AutoScalingGroup",
        ("ecs", "service") => "ECSService",
        ("rds", "db") => "RDSDBInstance",
        ("rds", "cluster") => "DocumentDBCluster",
        ("dynamodb", "table") => "DynamoDBTable",
        ("elasticache", "cluster") => "ElastiCacheCluster",
        ("memorydb", "cluster") => "MemoryDBCluster",
        ("workspaces", "workspace") => "WorkSpaces",
        ("sagemaker", "endpoint") => "SageMakerEndpoint",
        _ => return None,
    };
    debug_assert!(
        IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES.contains(&resource_type),
        "the catalog may only name modelled IdleRecommendationResourceType values"
    );
    Some(resource_type)
}

/// Check an `IdleRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is an `IdleRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: `Finding` selects on `IdleFinding` and
/// `ResourceType` on `IdleRecommendationResourceType`. A name or a value outside those sets
/// is refused rather than silently matching nothing.
pub fn idle_filter_rejection(filters: &[IdleRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                IDLE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            IDLE_FILTER_FINDING => IDLE_FINDING_VALUES,
            IDLE_FILTER_RESOURCE_TYPE => IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    IDLE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `recommendation` satisfy every `GetIdleRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. A member
/// the stored recommendation does not carry matches nothing. An unrecognised name never
/// reaches here — [`idle_filter_rejection`] refuses the request first.
pub fn idle_recommendation_matches_filters(
    recommendation: &IdleRecommendation,
    filters: &[IdleRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(IDLE_FILTER_FINDING) => recommendation.finding.as_deref(),
            Some(IDLE_FILTER_RESOURCE_TYPE) => recommendation.resource_type.as_deref(),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

/// The figure `OrderBy$dimension` sorts a recommendation on: the estimated monthly savings
/// of the dimension's savings opportunity — `SavingsValue` reads the opportunity before
/// discounts, `SavingsValueAfterDiscount` the one after them. A recommendation that carries
/// no estimate for that dimension sorts as the modelled default of the member it lacks
/// (`IdleEstimatedMonthlySavings$value` defaults to `0`).
pub fn idle_savings_value(recommendation: &IdleRecommendation, dimension: &str) -> f64 {
    // The two dimensions the model declares, each reading its own savings opportunity. A
    // dimension outside them is refused by the modelled `@enum` on `OrderBy$dimension` at
    // the request boundary; the before-discount figure is the conservative reading of one
    // this model does not yet declare.
    let after_discounts = dimension == DIMENSION_SAVINGS_VALUE_AFTER_DISCOUNT;
    let estimate = if after_discounts {
        recommendation
            .savings_opportunity_after_discounts
            .as_ref()
            .and_then(|opportunity| opportunity.estimated_monthly_savings.as_ref())
    } else {
        recommendation
            .savings_opportunity
            .as_ref()
            .and_then(|opportunity| opportunity.estimated_monthly_savings.as_ref())
    };
    estimate.map_or(0.0, |savings| savings.value)
}

/// Whether an `OrderBy$order` sorts descending. `Desc` does; `Asc`, an absent order and a
/// value the modelled `@enum` on `OrderBy$order` would refuse at the request boundary all
/// sort ascending, because the model documents no default direction and ascending is this
/// emulator's documented choice.
pub fn idle_sorts_descending(order: Option<&str>) -> bool {
    order == Some(ORDER_DESC)
}

/// Where a page of a paginated read resumed from: how far into the selected sequence the
/// previous page stopped, and a fingerprint of the selection it was taken with, so resuming
/// with a different selection is refused instead of answered with a page of another result
/// set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdlePageCursor {
    /// How many selected recommendations the previous pages already reported.
    pub offset: usize,
    /// The fingerprint of the request members that select and order the result set.
    pub selection_fingerprint: String,
}

/// A stable digest of `value`'s debug rendering, used as the selection fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its members in
/// declaration order, so the same selection always digests to the same string and a
/// different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    ::hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
}

/// The opaque `nextToken` `GetIdleRecommendations` hands out: the cursor itself,
/// hex-encoded, so a page token needs no server-side entry and a read never writes.
pub fn encode_idle_page_token(cursor: &IdlePageCursor) -> String {
    ::hex::encode(format!("{}:{}", cursor.offset, cursor.selection_fingerprint))
}

/// The cursor a `nextToken` this service issued stands for, or `None` for a token it did
/// not issue. What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would issue for this request.
pub fn decode_idle_page_token(token: &str) -> Option<IdlePageCursor> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (offset, selection_fingerprint) = decoded.split_once(':')?;
    Some(IdlePageCursor {
        offset: offset.parse().ok()?,
        selection_fingerprint: selection_fingerprint.to_string(),
    })
}

/// Per-service state for com-amazonaws-computeoptimizer-computeoptimizerservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The account's enrollment record, written by `UpdateEnrollmentStatus` and reported
    /// by `GetEnrollmentStatus`.
    pub enrollment: Enrollment,
    /// Every stored recommendation preference, ordered by key so a page boundary is
    /// stable across calls.
    pub preferences: BTreeMap<PreferenceKey, PreferenceRecord>,
    /// Every Auto Scaling group Compute Optimizer has analysed, in analysis order, so a
    /// page boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetAutoScalingGroupRecommendations` REPORTS
    /// this analysis and must not mutate the store (see
    /// [`seed_auto_scaling_group_analyses`]). It is the SINGLE source for a group: the Auto
    /// Scaling group rows of [`Self::analysed_resources`] are projected from it.
    pub auto_scaling_group_analyses: Vec<AnalyzedAutoScalingGroup>,
    /// The resources Compute Optimizer has analysed, in catalog order. Part of the
    /// store's initial value rather than something a reader writes on first use, so every
    /// findings reader serves the same rows and none of them mutates the store. The Auto
    /// Scaling group rows are PROJECTED from [`Self::auto_scaling_group_analyses`]
    /// ([`seed_analysed_resources`]), so the two readers of a group cannot disagree about
    /// its finding, its risk or what it costs.
    pub analysed_resources: Vec<AnalysedResource>,
    /// The idle resource recommendations this service holds, which
    /// `GetIdleRecommendations` reports. See [`IdleRecommendationStore`]: no operation of
    /// this API writes one.
    pub idle_recommendations: IdleRecommendationStore,
}

impl Default for ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The store's initial value: the account is not opted in, nobody has written a
    /// preference, the Auto Scaling group analysis and the findings catalog this emulator
    /// analysed — which a reader only sees once the account is enrolled — and no idle
    /// resource recommendation (only an analysis records one).
    fn default() -> Self {
        Self {
            enrollment: Enrollment::default(),
            preferences: BTreeMap::new(),
            auto_scaling_group_analyses: seed_auto_scaling_group_analyses(),
            analysed_resources: seed_analysed_resources(),
            idle_recommendations: IdleRecommendationStore::default(),
        }
    }
}

/// The findings catalog, built once when the store is created.
///
/// This emulator observes no real workloads, so WHICH resources it holds is a fixed set
/// rather than something derived from traffic; every figure a summary reports is
/// nevertheless computed from the costs, findings, reason codes and risks stored here, so
/// the counts, the savings and the percentage always agree with each other and with the
/// rows a per-resource findings reader would report.
///
/// The catalog spans the finding, reason-code, performance-risk and inferred-workload
/// vocabularies of several resource types so an aggregate over it is not a single row
/// restated: it holds a resource whose recommendation saves money, one the analysis would
/// GROW (so it saves none), and one it leaves alone.
fn seed_analysed_resources() -> Vec<AnalysedResource> {
    let account = ::engine::emulator_account_id();
    let east = "us-east-1";
    let west = "us-west-2";
    let row = |resource_type: &str,
               resource_arn: String,
               finding: &str,
               finding_reason_codes: &[&str],
               current_performance_risk: &str,
               current_monthly_cost: f64,
               recommended_monthly_cost: f64,
               inferred_workload_types: &[&str]| AnalysedResource {
        account_id: account.to_string(),
        resource_type: resource_type.to_string(),
        resource_arn,
        finding: finding.to_string(),
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: current_performance_risk.to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: inferred_workload_types
            .iter()
            .map(|workload| (*workload).to_string())
            .collect(),
    };

    vec![
        // An over-provisioned instance: the recommendation is smaller, so it saves.
        row(
            SOURCE_TYPE_EC2_INSTANCE,
            ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60001"),
            FINDING_OVERPROVISIONED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            280.32,
            140.16,
            &["PostgreSql"],
        ),
        // An under-provisioned instance: the recommendation is LARGER, so it saves
        // nothing even though its configuration changes.
        row(
            SOURCE_TYPE_EC2_INSTANCE,
            ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60002"),
            FINDING_UNDERPROVISIONED,
            &[METRIC_REASON_MEMORY_UNDERPROVISIONED],
            PERFORMANCE_RISK_HIGH,
            70.08,
            140.16,
            &["Redis"],
        ),
        // An optimized instance: nothing to change, nothing to save.
        row(
            SOURCE_TYPE_EC2_INSTANCE,
            ::engine::emulator_arn("ec2", west, "instance/i-0a1b2c3d4e5f60003"),
            FINDING_OPTIMIZED,
            &[],
            PERFORMANCE_RISK_VERY_LOW,
            140.16,
            140.16,
            &[],
        ),
        row(
            SOURCE_TYPE_EBS_VOLUME,
            ::engine::emulator_arn("ec2", east, "volume/vol-00112233445566001"),
            FINDING_NOT_OPTIMIZED,
            &[],
            PERFORMANCE_RISK_VERY_LOW,
            50.0,
            40.0,
            &[],
        ),
        // A volume the analysis leaves alone: optimized, so there is nothing to save on
        // it, and the EbsVolume summary counts a resource at each of its two findings.
        row(
            SOURCE_TYPE_EBS_VOLUME,
            ::engine::emulator_arn("ec2", east, "volume/vol-00112233445566002"),
            FINDING_OPTIMIZED,
            &[],
            PERFORMANCE_RISK_VERY_LOW,
            25.0,
            25.0,
            &[],
        ),
        row(
            SOURCE_TYPE_LAMBDA_FUNCTION,
            ::engine::emulator_arn("lambda", west, "function:invoice-renderer"),
            FINDING_NOT_OPTIMIZED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            18.0,
            12.0,
            &[],
        ),
        row(
            SOURCE_TYPE_ECS_SERVICE,
            ::engine::emulator_arn("ecs", east, "service/payments/checkout"),
            FINDING_OVERPROVISIONED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            120.0,
            80.0,
            &[],
        ),
        row(
            SOURCE_TYPE_RDS_DB_INSTANCE,
            ::engine::emulator_arn("rds", east, "db:orders-primary"),
            FINDING_OVERPROVISIONED,
            &[],
            PERFORMANCE_RISK_MEDIUM,
            300.0,
            225.0,
            &[],
        ),
    ]
    .into_iter()
    // The Auto Scaling group rows are the SAME analysis
    // `GetAutoScalingGroupRecommendations` reports, projected onto the findings row
    // ([`AnalyzedAutoScalingGroup::to_analysed_resource`]) rather than stated a second
    // time here, so a summary of the account's Auto Scaling groups counts exactly the
    // groups that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        seed_auto_scaling_group_analyses()
            .iter()
            .map(AnalyzedAutoScalingGroup::to_analysed_resource),
    )
    .collect()
}

/// The `FindingReasonCode` values (`crate::ops::types::FINDING_REASON_CODE_VALUES`) — the
/// only two `ReasonCodeSummary$name` can carry.
pub const METRIC_REASON_MEMORY_OVERPROVISIONED: &str = "MemoryOverprovisioned";
/// See [`METRIC_REASON_MEMORY_OVERPROVISIONED`].
pub const METRIC_REASON_MEMORY_UNDERPROVISIONED: &str = "MemoryUnderprovisioned";

/// The opaque `nextToken` `GetRecommendationSummaries` hands out: the
/// `RecommendationSourceType` the next page starts at, hex-encoded so a client can
/// neither read nor construct one. A token from the other paginated reader decodes to
/// nothing here (and this one to nothing there), so neither can resume the other's walk.
pub fn encode_summary_page_token(resource_type: &str) -> String {
    ::hex::encode(::serde_json::json!({ "summariesFrom": resource_type }).to_string())
}

/// The resource type a `nextToken` this operation issued stands for, or `None` for a
/// token it did not.
pub fn decode_summary_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("summariesFrom")?
        .as_str()
        .map(ToString::to_string)
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Whether a `PutRecommendationPreferences` for `key` may be stored: an update to a
    /// key already held always may, a NEW key only while the store is below
    /// [`MAX_PREFERENCE_RECORDS`]. Callers refuse with a modelled error rather than
    /// growing past the bound.
    pub fn can_hold(&self, key: &PreferenceKey) -> bool {
        self.preferences.len() < MAX_PREFERENCE_RECORDS || self.preferences.contains_key(key)
    }

    /// The stored records that apply to `resource_arn`, MOST SPECIFIC FIRST, so a caller
    /// resolving an effective value takes the first record that sets the field it wants.
    pub fn records_applying_to(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == arn.resource_type)
            .filter_map(|(key, record)| {
                scope_precedence(key, resource_arn, arn.account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The preferences in force for one resource: the value from the most specific scope
    /// that sets each field, and the service's documented default for every field no
    /// record sets — "considers all applicable preferences that you might have set at the
    /// resource, account, and organization level", each field independently, so a
    /// resource-level record that sets one preference still inherits the rest.
    ///
    /// The ONE resolution in this service: `GetEffectiveRecommendationPreferences` reports
    /// it and every recommendation reader states the preferences it was generated under
    /// from it, so the two cannot disagree.
    pub fn effective_preferences(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> EffectivePreferences {
        let applicable = self.records_applying_to(resource_arn, arn);
        let effective = |field: fn(&PreferenceRecord) -> bool| {
            applicable.iter().copied().find(|record| field(record))
        };
        EffectivePreferences {
            enhanced_infrastructure_metrics: effective(|record| {
                record.enhanced_infrastructure_metrics.is_some()
            })
            .and_then(|record| record.enhanced_infrastructure_metrics.clone()),
            inferred_workload_types: effective(|record| record.inferred_workload_types.is_some())
                .and_then(|record| record.inferred_workload_types.clone())
                .unwrap_or_else(|| INFERRED_WORKLOAD_TYPES_ACTIVE.to_string()),
            external_metrics_preference: effective(|record| {
                record.external_metrics_preference.is_some()
            })
            .and_then(|record| record.external_metrics_preference.clone()),
            look_back_period: effective(|record| record.look_back_period.is_some())
                .and_then(|record| record.look_back_period.clone())
                .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string()),
            // "You can only set CPU and memory utilization preferences for the Amazon EC2
            // instance resource type", so for any other resource type there is no default
            // to report and the list is empty.
            utilization_preferences: effective(|record| !record.utilization_preferences.is_empty())
                .map(|record| record.utilization_preferences.clone())
                .unwrap_or_else(|| {
                    if arn.resource_type == RESOURCE_TYPE_EC2_INSTANCE {
                        default_utilization_preferences()
                    } else {
                        Vec::new()
                    }
                }),
            preferred_resources: effective(|record| !record.preferred_resources.is_empty())
                .map(|record| {
                    record
                        .preferred_resources
                        .iter()
                        .map(effective_preferred_resource)
                        .collect()
                })
                .unwrap_or_else(default_preferred_resources),
            savings_estimation_mode: effective(|record| record.savings_estimation_mode.is_some())
                .and_then(|record| record.savings_estimation_mode.clone()),
        }
    }

    /// The Auto Scaling groups whose analysis a caller can read, in analysis order.
    ///
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only
    /// once the analysis that enrollment starts is due ([`ANALYSIS_DELAY`]), so an
    /// account that never opted in, one that opted out, and one that has just opted in
    /// all read the empty set rather than an analysis nothing has produced yet.
    pub fn visible_auto_scaling_group_analyses(&self) -> &[AnalyzedAutoScalingGroup] {
        if !self.enrollment.is_opted_in() || !self.enrollment.analysis_due() {
            return &[];
        }
        &self.auto_scaling_group_analyses
    }

    /// The analysed resources `account_id` owns, grouped by `RecommendationSourceType` and
    /// keyed by it, so a reader walks the groups in one stable order whatever the catalog's
    /// own order is.
    ///
    /// This emulator presents exactly one account ([`engine::emulator_account_id`]), so a
    /// request that asks for any other account's findings — which only an organization's
    /// management account may do — selects nothing rather than someone else's resources.
    pub fn analysed_resources_by_type(
        &self,
        account_id: &str,
    ) -> BTreeMap<&str, Vec<&AnalysedResource>> {
        let mut grouped: BTreeMap<&str, Vec<&AnalysedResource>> = BTreeMap::new();
        for resource in &self.analysed_resources {
            if resource.account_id != account_id {
                continue;
            }
            grouped
                .entry(resource.resource_type.as_str())
                .or_default()
                .push(resource);
        }
        grouped
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>)
}
