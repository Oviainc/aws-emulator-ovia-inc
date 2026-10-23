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
//! codes, its performance risk, what it costs and what the recommendation would cost, and —
//! for a resource analysed as an Amazon Aurora or RDS database — the database configuration
//! the analysis observed), part of the store's initial value so every findings reader
//! aggregates the same rows without any of them writing to the store. A row records a
//! resource's REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader
//! reports names the region of the endpoint the request addressed
//! ([`AnalysedResource::arn_in`], [`request_region`]). Idle resource recommendations have their own store
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
    AutoScalingGroupConfiguration, CustomizableMetricParameters, DBStorageConfiguration,
    EBSEffectiveRecommendationPreferences, EBSEstimatedMonthlySavings, EBSSavingsEstimationMode,
    EBSSavingsOpportunityAfterDiscounts, EBSUtilizationMetric, EffectivePreferredResource,
    EffectiveRecommendationPreferences, EstimatedMonthlySavings, ExternalMetricsPreference,
    IdleRecommendation, IdleRecommendationFilter, InstanceSavingsEstimationMode, PreferredResource,
    PutRecommendationPreferencesInput, RDSDBRecommendation, RDSDBRecommendationFilter,
    RDSDBUtilizationMetric, RecommendationPreferencesDetail, SavingsOpportunity, Scope, Tag,
    UtilizationMetric, UtilizationPreference, VolumeConfiguration, VolumeRecommendation,
    VolumeRecommendationOption, IDLE_FINDING_VALUES, IDLE_RECOMMENDATION_FILTER_NAME_VALUES,
    IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES, IDLE_VALUES,
    R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES, R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
    R_D_S_INSTANCE_FINDING_VALUES, R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
    R_D_S_STORAGE_FINDING_VALUES,
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

/// The region an ARN names when the request carries no SigV4 credential scope to read one
/// from: the region the emulator's own shared AWS config names, so an unsigned call and a
/// default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped and Compute Optimizer is a regional service, so the
/// region in a request's SigV4 credential scope is the region whose resources it is asking
/// about — and the one the ARNs in its answer name.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
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
        _ => account_scope_precedence(key, account_id),
    }
}

/// [`scope_precedence`] for a resource no `ResourceArn` scope can name — a `ResourceArn`
/// scope's "value must be the Amazon Resource Name (ARN) of an EC2 instance or an Auto
/// Scaling group" (`Scope$value`), so for any other resource type only the account-level
/// and organization-level records apply, in the same order they do above.
fn account_scope_precedence(key: &PreferenceKey, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
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

// ---------------------------------------------------------------------------
// Amazon EBS volumes: the inventory Compute Optimizer observes, and the volume
// recommendations it derives from it.
// ---------------------------------------------------------------------------

/// The `ResourceType` an EBS volume preference is written for, beside
/// [`RESOURCE_TYPE_EC2_INSTANCE`]. The `RecommendationSourceType` of the same spelling,
/// which the findings catalog records, is [`SOURCE_TYPE_EBS_VOLUME`].
pub const RESOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";

/// The one `EBSFilterName` value the model declares, and so the one filter name a volume
/// recommendation request can carry (`smithy_query query="shape" shape="EBSFilterName"`;
/// the request boundary refuses every other name against that closed enum, including the
/// `tag:key` and `tag-key` names `EBSFilter$name`'s prose mentions).
pub const EBS_FILTER_NAME_FINDING: &str = "Finding";

/// The `EBSMetricName` values, in the order
/// [`ObservedVolume::utilization_metrics`] reports them
/// (`smithy_query query="shape" shape="EBSMetricName"`).
pub const EBS_METRIC_READ_OPS_PER_SECOND: &str = "VolumeReadOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_OPS_PER_SECOND: &str = "VolumeWriteOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_READ_BYTES_PER_SECOND: &str = "VolumeReadBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_BYTES_PER_SECOND: &str = "VolumeWriteBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_IOPS_EXCEEDED: &str = "VolumeIOPSExceeded";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_THROUGHPUT_EXCEEDED: &str = "VolumeThroughputExceeded";

/// The volume type every EBS volume recommendation option this service builds is of:
/// gp3 is the current-generation general purpose SSD type, whose performance is
/// provisioned independently of size, so one option expresses both a performance change
/// and a cost change.
pub const VOLUME_TYPE_GP3: &str = "gp3";

/// gp3's included performance, from the Amazon EBS volume types documentation: a gp3
/// volume includes 3,000 IOPS and 125 MiB/s of throughput at every size, and can be
/// provisioned up to 16,000 IOPS and 1,000 MiB/s, at most 500 IOPS per GiB and at most
/// 0.25 MiB/s of throughput per provisioned IOPS.
pub const GP3_INCLUDED_IOPS: i32 = 3_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_INCLUDED_THROUGHPUT_MIB: i32 = 125;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS: i32 = 16_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB: i32 = 1_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS_PER_GIB: i32 = 500;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB_PER_IOPS: f64 = 0.25;

/// The headroom a recommendation option is sized with: the option has to carry the
/// observed peak with room to spare, or adopting it would trade cost for a performance
/// risk. The service documents no headroom for EBS volumes, so this is the emulator's
/// own, taken from the only headroom the service does document — "`PERCENT_20` for
/// headroom" (`PutRecommendationPreferences$utilizationPreferences`).
pub const RECOMMENDATION_HEADROOM: f64 = 0.20;

/// Bytes in a MiB: EBS reports volume throughput in MiB/s and CloudWatch reports the
/// `VolumeRead/WriteBytesPerSecond` metrics in bytes per second.
pub const BYTES_PER_MIB: f64 = 1_048_576.0;

/// One observation of a volume's demand: the per-second read and write rates measured
/// over one interval of the period Compute Optimizer analyses.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeSample {
    /// How many days before the recommendation's last refresh this sample was taken, so
    /// the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// `VolumeReadOpsPerSecond`.
    pub read_ops_per_second: f64,
    /// `VolumeWriteOpsPerSecond`.
    pub write_ops_per_second: f64,
    /// `VolumeReadBytesPerSecond`.
    pub read_bytes_per_second: f64,
    /// `VolumeWriteBytesPerSecond`.
    pub write_bytes_per_second: f64,
}

impl VolumeSample {
    /// The I/O operations per second this sample demanded, read plus write — what a
    /// volume's IOPS capacity has to carry.
    pub fn iops(&self) -> f64 {
        self.read_ops_per_second + self.write_ops_per_second
    }

    /// The throughput this sample demanded in MiB/s, read plus write — what a volume's
    /// throughput capacity has to carry.
    pub fn throughput_mib_per_second(&self) -> f64 {
        (self.read_bytes_per_second + self.write_bytes_per_second) / BYTES_PER_MIB
    }
}

/// One Amazon EBS volume Compute Optimizer observes: its configuration, its tags, and
/// the metric samples a recommendation for it is computed from. This is the OBSERVATION;
/// everything a `VolumeRecommendation` reports is derived from it, so no two readers of
/// the same volume can disagree.
#[derive(Debug, Clone)]
pub struct ObservedVolume {
    /// The volume's EC2 volume id, which is its key in the inventory. The ARN a reader
    /// reports is built from it and the region the request was signed for
    /// ([`ObservedVolume::volume_arn`]), because Compute Optimizer is a REGIONAL service:
    /// the volumes it reports are the ones in the Region the caller addressed.
    pub volume_id: String,
    /// The account that owns the volume.
    pub account_id: String,
    /// The volume's configuration as EC2 reports it: type, size, and the baseline and
    /// burst IOPS and throughput that type and size deliver.
    pub configuration: VolumeConfiguration,
    /// The volume's tags, as key/value pairs, in the order EC2 reports them.
    pub tags: Vec<(String, String)>,
    /// How many days Compute Optimizer has been observing this volume. A look-back
    /// preference longer than this cannot be honoured, so the analysed period is the
    /// shorter of the two.
    pub days_observed: f64,
    /// The metric samples observed, most recent first.
    pub samples: Vec<VolumeSample>,
}

impl ObservedVolume {
    /// This volume's ARN in `region`: `arn:aws:ec2:<region>:<account>:volume/<id>`.
    pub fn volume_arn(&self, region: &str) -> String {
        ::engine::emulator_arn("ec2", region, &format!("volume/{}", self.volume_id))
    }

    /// The samples the look-back period in force analyses: those taken within
    /// `look_back_days` of the last refresh.
    fn analysed_samples(&self, look_back_days: f64) -> impl Iterator<Item = &VolumeSample> {
        self.samples
            .iter()
            .filter(move |sample| sample.days_ago <= look_back_days)
    }

    /// The peak of one sample measurement over the analysed period, or `0.0` when the
    /// period holds no sample.
    fn peak(&self, look_back_days: f64, of: fn(&VolumeSample) -> f64) -> f64 {
        self.analysed_samples(look_back_days)
            .map(of)
            .fold(0.0_f64, f64::max)
    }

    /// How many analysed samples `exceeded` answers true for.
    fn samples_exceeding(
        &self,
        look_back_days: f64,
        exceeded: impl Fn(&VolumeSample) -> bool,
    ) -> f64 {
        self.analysed_samples(look_back_days)
            .filter(|sample| exceeded(sample))
            .count() as f64
    }

    /// "The number of days for which utilization metrics were analyzed for the volume"
    /// (`VolumeRecommendation$lookBackPeriodInDays`): the preferred look-back period
    /// ([`look_back_period_in_days`]), or however long this volume has been observed when
    /// that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// The metrics `VolumeRecommendation$utilizationMetrics` reports: every
    /// `EBSMetricName` the model declares, each the peak of the analysed samples — and,
    /// for the two "exceeded" metrics, the number of analysed samples whose demand went
    /// past the capacity the volume's configuration delivers.
    pub fn utilization_metrics(&self, look_back_days: f64) -> Vec<EBSUtilizationMetric> {
        let iops_capacity = configured_iops_capacity(&self.configuration);
        let throughput_capacity = configured_throughput_capacity(&self.configuration);
        let metric = |name: &str, value: f64| EBSUtilizationMetric {
            name: Some(name.to_string()),
            statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
            value,
        };
        vec![
            metric(
                EBS_METRIC_READ_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_ops_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_OPS_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_ops_per_second),
            ),
            metric(
                EBS_METRIC_READ_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.read_bytes_per_second),
            ),
            metric(
                EBS_METRIC_WRITE_BYTES_PER_SECOND,
                self.peak(look_back_days, |sample| sample.write_bytes_per_second),
            ),
            metric(
                EBS_METRIC_IOPS_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| sample.iops() > iops_capacity),
            ),
            metric(
                EBS_METRIC_THROUGHPUT_EXCEEDED,
                self.samples_exceeding(look_back_days, |sample| {
                    sample.throughput_mib_per_second() > throughput_capacity
                }),
            ),
        ]
    }

    /// The configuration this service recommends for the volume: gp3 at the same size,
    /// with IOPS and throughput provisioned to carry the analysed peak plus
    /// [`RECOMMENDATION_HEADROOM`], inside gp3's documented limits. A volume whose peak
    /// already fits gp3's included performance gets gp3's included performance.
    pub fn recommended_configuration(&self, look_back_days: f64) -> VolumeConfiguration {
        let size = self.configuration.volume_size.max(0);
        let headroom = 1.0 + RECOMMENDATION_HEADROOM;

        let peak_iops = self.peak(look_back_days, VolumeSample::iops);
        let iops_ceiling = GP3_MAX_IOPS
            .min(size.saturating_mul(GP3_MAX_IOPS_PER_GIB))
            .max(GP3_INCLUDED_IOPS);
        let iops = ((peak_iops * headroom).ceil() as i32).clamp(GP3_INCLUDED_IOPS, iops_ceiling);

        let peak_throughput = self.peak(look_back_days, VolumeSample::throughput_mib_per_second);
        let throughput_ceiling = GP3_MAX_THROUGHPUT_MIB
            .min((f64::from(iops) * GP3_MAX_THROUGHPUT_MIB_PER_IOPS) as i32)
            .max(GP3_INCLUDED_THROUGHPUT_MIB);
        let throughput = ((peak_throughput * headroom).ceil() as i32)
            .clamp(GP3_INCLUDED_THROUGHPUT_MIB, throughput_ceiling);

        VolumeConfiguration {
            volume_type: Some(VOLUME_TYPE_GP3.to_string()),
            volume_size: size,
            volume_baseline_iops: iops,
            // gp3 performance is provisioned rather than bursted — a gp3 volume uses no
            // burst credits — so the option delivers its baseline and nothing above it.
            volume_burst_iops: 0,
            volume_baseline_throughput: throughput,
            volume_burst_throughput: 0,
            // A recommendation changes a volume's type and provisioned performance; the
            // volume stays attached where it was, so whether it is a root volume is a
            // property of the volume, not of the option.
            root_volume: self.configuration.root_volume,
        }
    }

    /// How close the analysed peak came to what `configuration` sustains, as a
    /// `PerformanceRisk` on the model's 0..4 scale: 0 while the peak stays under a
    /// quarter of the sustained capacity, 4 once it reaches 125% of it. "The higher the
    /// risk, the more likely the current EBS volume doesn't have sufficient capacity"
    /// (`VolumeRecommendation$currentPerformanceRisk`), so the score rises with that
    /// ratio and is measured against the BASELINE: burst capacity is not sustained.
    pub fn performance_risk(
        &self,
        configuration: &VolumeConfiguration,
        look_back_days: f64,
    ) -> f64 {
        let baseline_iops = f64::from(configuration.volume_baseline_iops.max(0));
        let baseline_throughput = f64::from(configuration.volume_baseline_throughput.max(0));
        let iops_ratio = demand_ratio(self.peak(look_back_days, VolumeSample::iops), baseline_iops);
        let throughput_ratio = demand_ratio(
            self.peak(look_back_days, VolumeSample::throughput_mib_per_second),
            baseline_throughput,
        );
        let ratio = iops_ratio.max(throughput_ratio);
        ((ratio - 0.25) / 0.25).clamp(0.0, 4.0)
    }

    /// The volume's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some(key.clone()),
                    value: Some(value.clone()),
                })
                .collect()
        })
    }

    /// The volume recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every figure is computed here from the stored configuration and samples: the
    /// metrics are the analysed peaks, the option is the configuration those peaks ask
    /// for, each cost comes from [`volume_monthly_cost`], and the finding is
    /// `NotOptimized` exactly when the option would either carry a peak the current
    /// configuration does not sustain or cost less than it — "a volume is considered not
    /// optimized when Compute Optimizer identifies a recommendation that can provide
    /// better performance for your workload", and `Optimized` otherwise, where "Compute
    /// Optimizer might [still] recommend a new generation volume type"
    /// (`VolumeRecommendation$finding`).
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> VolumeRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let current = self.configuration.clone();
        let recommended = self.recommended_configuration(look_back_days);
        let current_risk = self.performance_risk(&current, look_back_days);
        let option_risk = self.performance_risk(&recommended, look_back_days);
        let savings = self.savings(look_back_days);

        VolumeRecommendation {
            volume_arn: Some(self.volume_arn(region)),
            account_id: Some(self.account_id.clone()),
            current_configuration: Some(current),
            finding: Some(self.finding(look_back_days).to_string()),
            utilization_metrics: Some(self.utilization_metrics(look_back_days)),
            look_back_period_in_days: look_back_days,
            volume_recommendation_options: Some(vec![VolumeRecommendationOption {
                configuration: Some(recommended),
                performance_risk: option_risk,
                // One option, so it is the first-ranked one. "Rank 1 is considered the
                // best" (`VolumeRecommendationOption$rank`).
                rank: 1,
                savings_opportunity: savings.map(|(percentage, value)| SavingsOpportunity {
                    savings_opportunity_percentage: percentage,
                    estimated_monthly_savings: Some(EstimatedMonthlySavings {
                        currency: Some(CURRENCY_USD.to_string()),
                        value,
                    }),
                }),
                // This emulator prices a volume from the public price table in
                // [`volume_monthly_cost`] and records no negotiated discount for storage,
                // so the after-discount figures are the before-discount ones — which is
                // what the reported [`SAVINGS_ESTIMATION_SOURCE`] says.
                savings_opportunity_after_discounts: savings.map(|(percentage, value)| {
                    EBSSavingsOpportunityAfterDiscounts {
                        savings_opportunity_percentage: percentage,
                        estimated_monthly_savings: Some(EBSEstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value,
                        }),
                    }
                }),
            }]),
            last_refresh_timestamp,
            current_performance_risk: Some(performance_risk_band(current_risk).to_string()),
            effective_recommendation_preferences: Some(EBSEffectiveRecommendationPreferences {
                savings_estimation_mode: Some(EBSSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
                look_back_period: Some(look_back_period.to_string()),
            }),
            tags: self.tag_list(),
        }
    }

    /// The volume's `EBSFinding` over the analysed period: `NotOptimized` exactly when the
    /// recommended configuration would either carry a peak the current one does not
    /// sustain or cost less than it, `Optimized` otherwise. `EBSFinding`'s two values are
    /// [`FINDING_NOT_OPTIMIZED`] and [`FINDING_OPTIMIZED`].
    pub fn finding(&self, look_back_days: f64) -> &'static str {
        let recommended = self.recommended_configuration(look_back_days);
        let safer = self.performance_risk(&recommended, look_back_days)
            < self.performance_risk(&self.configuration, look_back_days);
        let cheaper = match (
            volume_monthly_cost(&self.configuration),
            volume_monthly_cost(&recommended),
        ) {
            (Some(current), Some(option)) => option < current,
            _ => false,
        };
        if safer || cheaper {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// What adopting the recommended configuration saves per month and that saving as a
    /// percentage of what the volume costs today, or `None` for a volume type this
    /// emulator holds no price for ([`volume_monthly_cost`]) — whose recommendation then
    /// carries no savings figure rather than an invented one.
    pub fn savings(&self, look_back_days: f64) -> Option<(f64, f64)> {
        let current = volume_monthly_cost(&self.configuration)?;
        let option = volume_monthly_cost(&self.recommended_configuration(look_back_days))?;
        let saved = round_hundredths((current - option).max(0.0));
        Some((percentage_of(saved, current), saved))
    }

    /// This volume as a row of the findings catalog, i.e. as `GetRecommendationSummaries`
    /// aggregates it, under the look-back period in force.
    ///
    /// The projection is what keeps one volume's analysis in ONE place: the summary reader
    /// and `GetEBSVolumeRecommendations` both answer from this observation, so they cannot
    /// disagree about a volume's finding, its performance risk or what adopting its
    /// recommendation saves. A volume recommendation reports no finding reason codes
    /// (`VolumeRecommendation` declares no such member) and infers no workload type, so
    /// the row carries neither.
    pub fn to_analysed_resource(&self, look_back_period: &str) -> AnalysedResource {
        let look_back_days = self.analysed_days(look_back_period);
        let current_monthly_cost = volume_monthly_cost(&self.configuration).unwrap_or_default();
        let recommended_monthly_cost =
            volume_monthly_cost(&self.recommended_configuration(look_back_days))
                .unwrap_or(current_monthly_cost);
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_EBS_VOLUME.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a volume is its EC2 volume id.
            arn_service: "ec2".to_string(),
            arn_resource: format!("volume/{}", self.volume_id),
            finding: self.finding(look_back_days).to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: performance_risk_band(
                self.performance_risk(&self.configuration, look_back_days),
            )
            .to_string(),
            current_monthly_cost,
            recommended_monthly_cost,
            inferred_workload_types: Vec::new(),
            // A volume is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
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

/// How much of `capacity` a peak demand of `demand` takes. A configuration that
/// sustains nothing is saturated by any demand at all, and idle under none.
fn demand_ratio(demand: f64, capacity: f64) -> f64 {
    if capacity > 0.0 {
        demand / capacity
    } else if demand > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// The `CurrentPerformanceRisk` band a `PerformanceRisk` score falls in: the model's
/// four bands over the score's 0..4 range, lowest risk first, so the reported band and
/// the option's numeric risk always say the same thing.
pub fn performance_risk_band(score: f64) -> &'static str {
    const BANDS: [&str; 4] = [
        PERFORMANCE_RISK_VERY_LOW,
        PERFORMANCE_RISK_LOW,
        PERFORMANCE_RISK_MEDIUM,
        PERFORMANCE_RISK_HIGH,
    ];
    let band = (score.max(0.0) as usize).min(BANDS.len() - 1);
    BANDS[band]
}

/// The IOPS a configuration can deliver, burst included.
pub fn configured_iops_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_iops
            .max(configuration.volume_burst_iops)
            .max(0),
    )
}

/// The throughput in MiB/s a configuration can deliver, burst included.
pub fn configured_throughput_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_throughput
            .max(configuration.volume_burst_throughput)
            .max(0),
    )
}

/// What one month of a volume with this configuration costs in USD, or `None` for a
/// volume type this emulator holds no price for — whose recommendation then carries no
/// savings figure rather than an invented one.
///
/// Amazon EBS public pricing in the `us-east-1` Region, as the EBS pricing page lists
/// it: `gp3` $0.08 per GiB-month plus $0.005 per provisioned IOPS-month above the 3,000
/// included and $0.040 per provisioned MiB/s-month above the 125 included; `gp2` $0.10
/// per GiB-month; `io1` $0.125 per GiB-month plus $0.065 per provisioned IOPS-month;
/// `io2` $0.125 per GiB-month plus provisioned IOPS tiered at $0.065 for the first
/// 32,000, $0.046 up to 64,000 and $0.032 above; `st1` $0.045, `sc1` $0.015 and
/// `standard` $0.05 per GiB-month. Every cost and saving this service reports is
/// computed from this one table, so the figures of a recommendation agree with each
/// other.
pub fn volume_monthly_cost(configuration: &VolumeConfiguration) -> Option<f64> {
    let volume_type = configuration.volume_type.as_deref()?;
    let size = f64::from(configuration.volume_size.max(0));
    let provisioned_iops = configuration.volume_baseline_iops.max(0);
    let provisioned_throughput = configuration.volume_baseline_throughput.max(0);
    let cost = match volume_type {
        "gp3" => {
            0.08 * size
                + 0.005 * f64::from((provisioned_iops - GP3_INCLUDED_IOPS).max(0))
                + 0.040 * f64::from((provisioned_throughput - GP3_INCLUDED_THROUGHPUT_MIB).max(0))
        }
        "gp2" => 0.10 * size,
        "io1" => 0.125 * size + 0.065 * f64::from(provisioned_iops),
        "io2" => 0.125 * size + io2_provisioned_iops_cost(provisioned_iops),
        "st1" => 0.045 * size,
        "sc1" => 0.015 * size,
        "standard" => 0.05 * size,
        _ => return None,
    };
    Some(round_hundredths(cost))
}

/// The tiered provisioned-IOPS charge of an `io2` volume, per [`volume_monthly_cost`].
fn io2_provisioned_iops_cost(provisioned_iops: i32) -> f64 {
    let first_tier = provisioned_iops.min(32_000);
    let second_tier = (provisioned_iops - 32_000).clamp(0, 32_000);
    let third_tier = (provisioned_iops - 64_000).max(0);
    0.065 * f64::from(first_tier) + 0.046 * f64::from(second_tier) + 0.032 * f64::from(third_tier)
}

/// An Amazon EBS volume ARN — `arn:<partition>:ec2:<region>:<account>:volume/<id>` —
/// split into the parts the readers need, or `None` for anything that is not one.
/// `GetEBSVolumeRecommendations$volumeArns` takes "the Amazon Resource Name (ARN) of the
/// volumes for which to return recommendations", so anything else is not a volume ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedVolumeArn<'a> {
    /// The ARN's account-id field: the account that owns the volume.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the volume is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The EC2 volume id, which is the inventory's key.
    pub volume_id: &'a str,
}

/// Parse an Amazon EBS volume ARN, or `None` for anything that is not one. See
/// [`ParsedVolumeArn`].
pub fn parse_volume_arn(arn: &str) -> Option<ParsedVolumeArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, _partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || *service != "ec2" {
        return None;
    }
    let (resource_kind, volume_id) = resource.split_once('/')?;
    if resource_kind != "volume" || volume_id.is_empty() {
        return None;
    }
    Some(ParsedVolumeArn {
        account_id,
        region,
        volume_id,
    })
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
        // The catalog records a resource's REGION-FREE identity
        // ([`AnalysedResource::arn_in`]), so the group's own ARN is split back into the
        // fields a reader rebuilds it from.
        let parsed = parse_arn(&self.arn);
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_AUTO_SCALING_GROUP.to_string(),
            arn_service: parsed
                .map(|arn| arn.service.to_string())
                .unwrap_or_default(),
            arn_resource: parsed
                .map(|arn| arn.resource.to_string())
                .unwrap_or_default(),
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
            // A group is not a database, so there is no database configuration to record.
            rds_database: None,
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
    /// The service namespace the resource belongs to — the `ec2`, `rds`, … field of its ARN.
    pub arn_service: String,
    /// Everything after the account field of the resource's ARN (`db:orders-primary`),
    /// separators included: the resource's identity WITHOUT the region, which
    /// [`Self::arn_in`] builds the ARN for one regional endpoint from.
    pub arn_resource: String,
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
    /// What the analysis recorded about the resource AS A DATABASE, for a row analysed as an
    /// Amazon Aurora or RDS database; `None` for every other resource type, and for a
    /// database whose configuration the analysis did not record. See [`RdsDatabaseDetail`].
    pub rds_database: Option<RdsDatabaseDetail>,
}

impl AnalysedResource {
    /// This resource's ARN as the endpoint in `region` reports it.
    ///
    /// Compute Optimizer is a REGIONAL service: each regional endpoint analyses and reports
    /// the resources of its own region, so a resource the catalog holds is a resource of the
    /// region whose endpoint a request addressed, and the ARN reported for it names that
    /// region. The catalog therefore records the resource's region-free identity and every
    /// reader builds the ARN for the region its request was signed for
    /// ([`request_region`]).
    pub fn arn_in(&self, region: &str) -> String {
        if self.account_id == ::engine::emulator_account_id() {
            ::engine::emulator_arn(&self.arn_service, region, &self.arn_resource)
        } else {
            // `emulator_arn` always names the emulator's own account; a row recorded for
            // another account keeps that account in its ARN.
            format!(
                "arn:aws:{}:{region}:{}:{}",
                self.arn_service, self.account_id, self.arn_resource
            )
        }
    }

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

/// The error `code` a findings reader reports for the one condition it can report: the
/// request named a resource this service holds no recommendation for, which is exactly what
/// `IdleRecommendationError` ("Returns of list of resources that doesn't have idle
/// recommendations") and `GetRecommendationError` ("Describes an error experienced when
/// getting recommendations") describe. The model documents `code` only as "The error code"
/// and names no vocabulary for it, so this is the emulator's own stable code, stated once so
/// every reader reports the same one; the requirement the entry carries is its `identifier`.
pub const NO_RECOMMENDATION_CODE: &str = "ResourceNotFound";

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
pub struct PageCursor {
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

/// The opaque `nextToken` an offset-paged reader hands out: the cursor itself, hex-encoded,
/// so a page token needs no server-side entry and a read never writes. Shared by every
/// reader of this service that walks a selected sequence by offset, each of which binds its
/// own operation name into the selection fingerprint, so one reader's token resumes no
/// other's walk.
pub fn encode_offset_page_token(cursor: &PageCursor) -> String {
    ::hex::encode(format!("{}:{}", cursor.offset, cursor.selection_fingerprint))
}

/// The cursor a `nextToken` this service issued stands for, or `None` for a token it did
/// not issue. What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would issue for this request.
pub fn decode_offset_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (offset, selection_fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        selection_fingerprint: selection_fingerprint.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Amazon Aurora and RDS database recommendations
//
// `GetRDSDatabaseRecommendations` "Returns Amazon Aurora and RDS database
// recommendations", which Compute Optimizer "generates ... for Amazon Aurora and RDS
// databases that meet a specific set of requirements" from the utilization it OBSERVES.
// So, like every other findings reader of this service, it REPORTS the rows the findings
// catalog holds — here the ones analysed as `RdsDBInstance`, the source type the model's
// `RDSDBRecommendation` describes — and writes nothing. The rules below are the ones that
// turn a stored row into the modelled recommendation and select over it: the filter
// vocabulary, which finding member a stored `Finding` is reported on, and how a look-back
// preference reads as a number of days.
// ---------------------------------------------------------------------------

/// The `RDSDBRecommendationFilterName` values
/// (`crate::ops::types::R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES`), named so the filter
/// rules below read as the model's own vocabulary.
pub const RDS_FILTER_INSTANCE_FINDING: &str = "InstanceFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_INSTANCE_FINDING_REASON_CODE: &str = "InstanceFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING: &str = "StorageFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING_REASON_CODE: &str = "StorageFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_IDLE: &str = "Idle";

/// The `Idle` enum values (`crate::ops::types::IDLE_VALUES`), which `RDSDBRecommendation$idle`
/// carries: whether the service also holds an IDLE recommendation for the database.
pub const IDLE_TRUE: &str = "True";
/// See [`IDLE_TRUE`].
pub const IDLE_FALSE: &str = "False";

/// What the findings catalog records about a resource it analysed as an Amazon Aurora or RDS
/// DATABASE, beyond the finding, risk and costs every analysed resource carries: the
/// database's configuration, the configuration the analysis would move it to, and the
/// utilization it observed.
///
/// `None` on a row the analysis recorded no database configuration for, and every field is
/// optional in its own right, so a reader reports exactly what was observed and omits the
/// members nothing was recorded for rather than inventing a value for them.
#[derive(Debug, Clone, Default)]
pub struct RdsDatabaseDetail {
    /// `RDSDBRecommendation$engine` — "The database engine of the DB instance".
    pub engine: Option<String>,
    /// `RDSDBRecommendation$engineVersion`.
    pub engine_version: Option<String>,
    /// `RDSDBRecommendation$promotionTier`, set only for an Aurora cluster member.
    pub promotion_tier: Option<i32>,
    /// `RDSDBRecommendation$dbClusterIdentifier`, set only for a cluster member.
    pub db_cluster_identifier: Option<String>,
    /// `RDSDBRecommendation$currentDBInstanceClass`.
    pub current_db_instance_class: Option<String>,
    /// The DB instance class the analysis would move the database to, which is what an
    /// `RDSDBInstanceRecommendationOption` names. Absent where the analysis would keep the
    /// current class.
    pub recommended_db_instance_class: Option<String>,
    /// `RDSDBRecommendation$currentStorageConfiguration`.
    pub current_storage_configuration: Option<DBStorageConfiguration>,
    /// The storage configuration the analysis would move the database to, which is what an
    /// `RDSDBStorageRecommendationOption` names.
    pub recommended_storage_configuration: Option<DBStorageConfiguration>,
    /// The `RDSDBUtilizationMetric`s the analysis observed over its look-back period.
    pub utilization_metrics: Vec<RDSDBUtilizationMetric>,
    /// When the recommendation was last generated (`lastRefreshTimestamp`).
    pub last_refresh_timestamp: Option<DateTime>,
    /// The tags of the database.
    pub tags: Vec<Tag>,
}

/// Which `RDSDBRecommendation` findings a stored `Finding` is reported as: the instance
/// finding and the storage finding, in that order.
///
/// Compute Optimizer analyses a database's INSTANCE and its STORAGE, and reports a finding
/// for each, in the vocabulary the model declares for that member: `RDSInstanceFinding` is
/// `Optimized | Overprovisioned | Underprovisioned`, and `RDSStorageFinding` declares those
/// three plus `NotOptimized`. The catalog records ONE finding per analysed resource, so that
/// finding is reported on the side whose enum declares it — the instance side when both do,
/// since `NotOptimized` is the one finding only storage can carry — and the other side
/// reports `Optimized`, the value both vocabularies use for a configuration the analysis
/// would not change, because the analysis recorded nothing against it. Every `Finding` the
/// catalog can hold is declared by at least one of the two; a value outside both is reported
/// on neither member.
pub fn rds_finding_members(finding: &str) -> (Option<&str>, Option<&str>) {
    // `FINDING_OPTIMIZED` is the `Finding` value `RDSInstanceFinding` and `RDSStorageFinding`
    // both declare, so one constant names it for all three.
    match (
        R_D_S_INSTANCE_FINDING_VALUES.contains(&finding),
        R_D_S_STORAGE_FINDING_VALUES.contains(&finding),
    ) {
        (true, _) => (Some(finding), Some(FINDING_OPTIMIZED)),
        (false, true) => (Some(FINDING_OPTIMIZED), Some(finding)),
        (false, false) => (None, None),
    }
}

/// The stored reason codes `vocabulary` declares, in stored order.
///
/// A resource's reason codes are reported on the member whose enum declares them —
/// `instanceFindingReasonCodes` targets `RDSInstanceFindingReasonCode` and
/// `storageFindingReasonCodes` targets `RDSStorageFindingReasonCode` — so a stored code is
/// reported on the member that can carry it and on no other.
pub fn reason_codes_in(vocabulary: &[&str], codes: &[String]) -> Vec<String> {
    codes
        .iter()
        .filter(|code| vocabulary.contains(&code.as_str()))
        .cloned()
        .collect()
}

/// Check an `RDSDBRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is an `RDSDBRecommendationFilterName` and the values a filter carries are the value
/// set of the member that name selects on. A name or a value outside those sets is refused
/// rather than silently matching nothing — the same rule [`idle_filter_rejection`] applies to
/// the other findings reader's filters.
pub fn rds_filter_rejection(filters: &[RDSDBRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = rds_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
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

/// The value set an `RDSDBRecommendationFilterName` selects on, or `None` for a name the
/// model does not declare.
fn rds_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        RDS_FILTER_INSTANCE_FINDING => Some(R_D_S_INSTANCE_FINDING_VALUES),
        RDS_FILTER_INSTANCE_FINDING_REASON_CODE => {
            Some(R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES)
        }
        RDS_FILTER_STORAGE_FINDING => Some(R_D_S_STORAGE_FINDING_VALUES),
        RDS_FILTER_STORAGE_FINDING_REASON_CODE => Some(R_D_S_STORAGE_FINDING_REASON_CODE_VALUES),
        RDS_FILTER_IDLE => Some(IDLE_VALUES),
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetRDSDatabaseRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for a member that is a LIST of reason codes, when any stored code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`rds_filter_rejection`] refuses the request first.
pub fn rds_recommendation_matches_filters(
    recommendation: &RDSDBRecommendation,
    filters: &[RDSDBRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let matches_one = |member: Option<&str>| {
            member.is_some_and(|member| values.iter().any(|value| value == member))
        };
        let matches_any = |members: Option<&Vec<String>>| {
            members.is_some_and(|members| {
                members
                    .iter()
                    .any(|member| values.iter().any(|value| value == member))
            })
        };
        match filter.name.as_deref() {
            Some(RDS_FILTER_INSTANCE_FINDING) => matches_one(recommendation.instance_finding.as_deref()),
            Some(RDS_FILTER_STORAGE_FINDING) => matches_one(recommendation.storage_finding.as_deref()),
            Some(RDS_FILTER_IDLE) => matches_one(recommendation.idle.as_deref()),
            Some(RDS_FILTER_INSTANCE_FINDING_REASON_CODE) => {
                matches_any(recommendation.instance_finding_reason_codes.as_ref())
            }
            Some(RDS_FILTER_STORAGE_FINDING_REASON_CODE) => {
                matches_any(recommendation.storage_finding_reason_codes.as_ref())
            }
            _ => false,
        }
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
    /// The Amazon EBS volumes Compute Optimizer observes, by volume id, so a page
    /// boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetEBSVolumeRecommendations` REPORTS this
    /// observation and must not mutate the store (see [`observed_volumes`]). It is the
    /// SINGLE source for a volume: the EBS volume rows of [`Self::analysed_resources`]
    /// are projected from it.
    pub volumes: BTreeMap<String, ObservedVolume>,
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
    /// Scaling group rows are PROJECTED from [`Self::auto_scaling_group_analyses`] and the
    /// EBS volume rows from [`Self::volumes`] ([`seed_analysed_resources`]), so the two
    /// readers of one resource cannot disagree about its finding, its risk or what it
    /// costs.
    pub analysed_resources: Vec<AnalysedResource>,
    /// The idle resource recommendations this service holds, which
    /// `GetIdleRecommendations` reports. See [`IdleRecommendationStore`]: no operation of
    /// this API writes one.
    pub idle_recommendations: IdleRecommendationStore,
}

impl Default for ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The store's initial value: the account is not opted in, nobody has written a
    /// preference, the EBS volume observation, the Auto Scaling group analysis and the
    /// findings catalog this emulator analysed — which a reader only sees once the account
    /// is enrolled — and no idle resource recommendation (only an analysis records one).
    fn default() -> Self {
        Self {
            enrollment: Enrollment::default(),
            preferences: BTreeMap::new(),
            volumes: observed_volumes()
                .into_iter()
                .map(|volume| (volume.volume_id.clone(), volume))
                .collect(),
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
///
/// A row records a resource's REGION-FREE identity (its service namespace and resource
/// part): Compute Optimizer is regional, so the region a reported ARN names is the region
/// of the endpoint the request addressed ([`AnalysedResource::arn_in`]).
fn seed_analysed_resources() -> Vec<AnalysedResource> {
    let account = ::engine::emulator_account_id();
    let row = |resource_type: &str,
               arn_service: &str,
               arn_resource: &str,
               finding: &str,
               finding_reason_codes: &[&str],
               current_performance_risk: &str,
               current_monthly_cost: f64,
               recommended_monthly_cost: f64,
               inferred_workload_types: &[&str]| AnalysedResource {
        account_id: account.to_string(),
        resource_type: resource_type.to_string(),
        arn_service: arn_service.to_string(),
        arn_resource: arn_resource.to_string(),
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
        // Set below for the rows analysed as a database; every other resource type has no
        // database configuration to record.
        rds_database: None,
    };

    vec![
        // An over-provisioned instance: the recommendation is smaller, so it saves.
        row(
            SOURCE_TYPE_EC2_INSTANCE,
            "ec2",
            "instance/i-0a1b2c3d4e5f60001",
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
            "ec2",
            "instance/i-0a1b2c3d4e5f60002",
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
            "ec2",
            "instance/i-0a1b2c3d4e5f60003",
            FINDING_OPTIMIZED,
            &[],
            PERFORMANCE_RISK_VERY_LOW,
            140.16,
            140.16,
            &[],
        ),
        row(
            SOURCE_TYPE_LAMBDA_FUNCTION,
            "lambda",
            "function:invoice-renderer",
            FINDING_NOT_OPTIMIZED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            18.0,
            12.0,
            &[],
        ),
        row(
            SOURCE_TYPE_ECS_SERVICE,
            "ecs",
            "service/payments/checkout",
            FINDING_OVERPROVISIONED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            120.0,
            80.0,
            &[],
        ),
        // An over-provisioned Aurora/RDS database: the analysis recommends a smaller DB
        // instance class, so it saves. The database's own configuration and the utilization
        // observed on it are recorded too, which is what `GetRDSDatabaseRecommendations`
        // reports beside the finding, the risk and the costs every analysed row carries.
        AnalysedResource {
            rds_database: Some(RdsDatabaseDetail {
                // `Engine`/`EngineVersion` are free-form strings in the model; these are the
                // engine name and version an RDS PostgreSQL instance reports.
                engine: Some("postgres".to_string()),
                engine_version: Some("15.4".to_string()),
                // Not a cluster member, so no promotion tier and no cluster identifier.
                promotion_tier: None,
                db_cluster_identifier: None,
                current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
                recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
                current_storage_configuration: Some(DBStorageConfiguration {
                    storage_type: Some("gp3".to_string()),
                    allocated_storage: 500,
                    iops: Some(3_000),
                    max_allocated_storage: Some(1_000),
                    storage_throughput: Some(125),
                }),
                // The analysis would keep the storage as it is: its finding is about the
                // instance, so there is no storage configuration to move to.
                recommended_storage_configuration: None,
                // `RDSDBMetricName` / `RDSDBMetricStatistic` values
                // (`crate::ops::types::R_D_S_D_B_METRIC_NAME_VALUES`).
                utilization_metrics: vec![
                    RDSDBUtilizationMetric {
                        name: Some("CPU".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 17.5,
                    },
                    RDSDBUtilizationMetric {
                        name: Some("Memory".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 24.0,
                    },
                    RDSDBUtilizationMetric {
                        name: Some("DatabaseConnections".to_string()),
                        statistic: Some("Maximum".to_string()),
                        value: 12.0,
                    },
                ],
                last_refresh_timestamp: None,
                tags: Vec::new(),
            }),
            ..row(
                SOURCE_TYPE_RDS_DB_INSTANCE,
                "rds",
                "db:orders-primary",
                FINDING_OVERPROVISIONED,
                &[],
                PERFORMANCE_RISK_MEDIUM,
                300.0,
                225.0,
                &[],
            )
        },
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
    // The Amazon EBS volume rows are the SAME observation
    // `GetEBSVolumeRecommendations` reports, projected the same way
    // ([`ObservedVolume::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, so a summary of the account's volumes counts exactly the
    // volumes that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        observed_volumes()
            .iter()
            .map(|volume| volume.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
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
        self.records_applying_to_resource(resource_arn, arn.resource_type, arn.account_id)
    }

    /// The stored records that apply to one resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — the same precedence [`records_applying_to`]
    /// resolves with, for a resource whose type and account a caller already knows (a
    /// findings reader reporting the preferences in force for a row it holds, say) rather
    /// than one it parses out of an ARN.
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_applying_to_resource(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                scope_precedence(key, resource_arn, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The stored records that apply to a resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — [`records_applying_to`] for a resource type no
    /// `ResourceArn` scope can name (see [`account_scope_precedence`]).
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_for_account(
        &self,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                account_scope_precedence(key, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The look-back period in force for a resource of `resource_type` owned by
    /// `account_id`: the value the most specific record that sets it carries, or
    /// [`DEFAULT_LOOK_BACK_PERIOD`] when none does.
    pub fn effective_look_back_period(&self, resource_type: &str, account_id: &str) -> String {
        self.records_for_account(resource_type, account_id)
            .into_iter()
            .find_map(|record| record.look_back_period.clone())
            .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string())
    }

    /// The Amazon EBS volumes whose observation a caller can read, by volume id.
    ///
    /// The same gate [`visible_auto_scaling_group_analyses`] applies: Compute Optimizer
    /// reports recommendations for an ENROLLED account only, and only once the analysis
    /// that enrollment starts is due ([`ANALYSIS_DELAY`]), so an account that never opted
    /// in, one that opted out, and one that has just opted in all read the empty set
    /// rather than an analysis nothing has produced yet.
    ///
    /// [`visible_auto_scaling_group_analyses`]: Self::visible_auto_scaling_group_analyses
    pub fn visible_volumes(&self) -> impl Iterator<Item = &ObservedVolume> {
        let visible = self.enrollment.is_opted_in() && self.enrollment.analysis_due();
        self.volumes.values().filter(move |_| visible)
    }
}

/// The Amazon EBS volume inventory this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so the volumes and their metric samples are a
/// fixed observation rather than something derived from traffic. Everything a
/// recommendation reports is nevertheless computed from what is stored here — the
/// analysed peaks come from these samples, the recommended configuration from those
/// peaks, and every cost from [`volume_monthly_cost`] — so a reader cannot report a
/// figure the observation does not support.
///
/// The set spans the cases a volume recommendation distinguishes: a previous-generation
/// volume whose demand fits its capacity (cheaper on gp3), a gp3 volume whose demand
/// outruns its provisioned IOPS (needs more performance), a heavily over-provisioned
/// Provisioned IOPS volume whose peak demand arrived outside the default look-back
/// period, and a gp3 volume already provisioned for its workload.
fn observed_volumes() -> Vec<ObservedVolume> {
    let account_id = ::engine::emulator_account_id().to_string();

    vec![
        // gp2, 500 GiB: 3 IOPS per GiB of baseline (1,500) with a 3,000 IOPS burst and
        // 250 MiB/s of throughput. Its peak fits that comfortably, so the gp3 option
        // carries the same workload for less money.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60001".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("gp2".to_string()),
                volume_size: 500,
                volume_baseline_iops: 1_500,
                volume_burst_iops: 3_000,
                volume_baseline_throughput: 250,
                volume_burst_throughput: 250,
                root_volume: Some(true),
            },
            tags: vec![("Owner".to_string(), "TeamA".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 300.0,
                    write_ops_per_second: 400.0,
                    read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 25.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 3.0,
                    read_ops_per_second: 450.0,
                    write_ops_per_second: 450.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 40.0,
                    read_ops_per_second: 200.0,
                    write_ops_per_second: 150.0,
                    read_bytes_per_second: 10.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 8.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 100 GiB at its included performance, driven past it: the option has to
        // provision more IOPS and more throughput, which costs MORE than the volume
        // costs today — a recommendation is not always a saving.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60002".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 100,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![("Owner".to_string(), "TeamB".to_string())],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 0.0,
                    read_ops_per_second: 2_400.0,
                    write_ops_per_second: 2_400.0,
                    read_bytes_per_second: 60.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 10.0,
                    read_ops_per_second: 1_000.0,
                    write_ops_per_second: 900.0,
                    read_bytes_per_second: 40.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 30.0 * BYTES_PER_MIB,
                },
            ],
        },
        // io1, 200 GiB with 8,000 provisioned IOPS: far more than the workload asks for,
        // and its heaviest day falls OUTSIDE the default look-back period, so a longer
        // look-back preference changes the option this volume gets.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60003".to_string(),
            account_id: account_id.clone(),
            configuration: VolumeConfiguration {
                volume_type: Some("io1".to_string()),
                volume_size: 200,
                volume_baseline_iops: 8_000,
                volume_burst_iops: 0,
                volume_baseline_throughput: 500,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: vec![
                ("Owner".to_string(), "TeamB".to_string()),
                ("Environment".to_string(), "production".to_string()),
            ],
            days_observed: 93.0,
            samples: vec![
                VolumeSample {
                    days_ago: 1.0,
                    read_ops_per_second: 600.0,
                    write_ops_per_second: 600.0,
                    read_bytes_per_second: 30.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 20.0 * BYTES_PER_MIB,
                },
                VolumeSample {
                    days_ago: 50.0,
                    read_ops_per_second: 1_500.0,
                    write_ops_per_second: 1_500.0,
                    read_bytes_per_second: 100.0 * BYTES_PER_MIB,
                    write_bytes_per_second: 60.0 * BYTES_PER_MIB,
                },
            ],
        },
        // gp3, 50 GiB, already provisioned for its workload — and observed for only a
        // week, so the period analysed is that week however long a look-back period the
        // account prefers.
        ObservedVolume {
            volume_id: "vol-0a1b2c3d4e5f60004".to_string(),
            account_id,
            configuration: VolumeConfiguration {
                volume_type: Some(VOLUME_TYPE_GP3.to_string()),
                volume_size: 50,
                volume_baseline_iops: GP3_INCLUDED_IOPS,
                volume_burst_iops: 0,
                volume_baseline_throughput: GP3_INCLUDED_THROUGHPUT_MIB,
                volume_burst_throughput: 0,
                root_volume: Some(false),
            },
            tags: Vec::new(),
            days_observed: 7.0,
            samples: vec![VolumeSample {
                days_ago: 0.0,
                read_ops_per_second: 700.0,
                write_ops_per_second: 800.0,
                read_bytes_per_second: 20.0 * BYTES_PER_MIB,
                write_bytes_per_second: 20.0 * BYTES_PER_MIB,
            }],
        },
    ]
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
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
        self.effective_preferences_for(resource_arn, arn.resource_type, arn.account_id)
    }

    /// The preferences in force for one resource of `resource_type` owned by `account_id` —
    /// the same resolution [`effective_preferences`] performs, for a resource whose type and
    /// account a caller already knows (a findings reader reporting the preferences a stored
    /// row's recommendation was generated under, say) rather than one it parses out of an
    /// ARN.
    ///
    /// [`effective_preferences`]: Self::effective_preferences
    pub fn effective_preferences_for(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> EffectivePreferences {
        let applicable = self.records_applying_to_resource(resource_arn, resource_type, account_id);
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
                    if resource_type == RESOURCE_TYPE_EC2_INSTANCE {
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
