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
//! The analysis output this service REPORTS lives here too, in its own store
//! ([`IdleRecommendationStore`]): no operation of this API creates an idle resource
//! recommendation — Compute Optimizer's own analysis of an observed workload does — so the
//! store is what `GetIdleRecommendations` filters, orders and pages over, and the rules it
//! applies to a stored recommendation (the filter vocabulary, the sort dimensions, the ARN
//! -> resource-type catalog) are stated once, here.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    CustomizableMetricParameters, EffectivePreferredResource, ExternalMetricsPreference,
    IdleRecommendation, IdleRecommendationFilter, PreferredResource,
    PutRecommendationPreferencesInput, RecommendationPreferencesDetail, Scope, UtilizationPreference,
    IDLE_FINDING_VALUES, IDLE_RECOMMENDATION_FILTER_NAME_VALUES,
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

/// The largest page `GetRecommendationPreferences` will build, whatever `maxResults` asks
/// for: the ceiling the model itself puts on `maxResults` (`MaxResults` range 0..1000).
/// Applied in the handler, so no number a request carries can size a response or the
/// allocation that builds it, whatever any other layer does or does not refuse.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The page size `GetRecommendationPreferences` uses when the request names none (or names
/// the range's lower bound `0`, which the service documents no meaning for). The service
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
#[derive(Debug, Default)]
pub struct ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The account's enrollment record, written by `UpdateEnrollmentStatus` and reported
    /// by `GetEnrollmentStatus`.
    pub enrollment: Enrollment,
    /// Every stored recommendation preference, ordered by key so a page boundary is
    /// stable across calls.
    pub preferences: BTreeMap<PreferenceKey, PreferenceRecord>,
    /// The idle resource recommendations this service holds, which
    /// `GetIdleRecommendations` reports. See [`IdleRecommendationStore`]: no operation of
    /// this API writes one.
    pub idle_recommendations: IdleRecommendationStore,
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
