// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-costoptimizationhub-costoptimizationhubservice service.
//!
//! Three stores, all keyed the way Cost Optimization Hub scopes its data:
//!
//! * `enrollments` — one [`Enrollment`] per account id, written by
//!   `UpdateEnrollmentStatus` and read by `ListEnrollmentStatuses`.
//! * `preferences` — one [`Preferences`] per account id, written by
//!   `UpdatePreferences` and read by `GetPreferences` (and by the recommendation
//!   readers, because `savingsEstimationMode` decides whether the figures they
//!   report are before or after discounts).
//! * `recommendations` + `recommendation_order` — the recommendation catalog
//!   `ListRecommendations` pages over and `GetRecommendation` looks ids up in.
//!   The catalog is part of the store's initial value, so both readers see the
//!   same rows and neither has to write anything to serve a read.

use std::collections::HashMap;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AuroraDbClusterStorage, AuroraDbClusterStorageConfiguration, BlockStoragePerformanceConfiguration,
    ComputeConfiguration, ComputeSavingsPlans, ComputeSavingsPlansConfiguration, EbsVolume,
    EbsVolumeConfiguration, Ec2Instance, Ec2InstanceConfiguration, EstimatedDiscounts, Filter,
    InstanceConfiguration, LambdaFunction, LambdaFunctionConfiguration, NatGateway,
    NatGatewayConfiguration, PreferredCommitment, ResourceCostCalculation, ResourceDetails,
    ResourcePricing, SavingsPlansCostCalculation, SavingsPlansPricing, StorageConfiguration, Tag,
};

/// The `SavingsEstimationMode` value that subtracts commitment and discount
/// amounts from the figures a recommendation reports; the other value
/// (`BeforeDiscounts`) reports the undiscounted cost. Both come from the model's
/// `SavingsEstimationMode` enum (`shared_types::SAVINGS_ESTIMATION_MODE_VALUES`).
pub const AFTER_DISCOUNTS: &str = "AfterDiscounts";

/// `EnrollmentStatus` values, from the model's enum of the same name.
pub const STATUS_ACTIVE: &str = "Active";

/// Currency every figure in the catalog is denominated in.
const CURRENCY_CODE: &str = "USD";

/// An account's enrollment in Cost Optimization Hub.
#[derive(Debug, Clone)]
pub struct Enrollment {
    pub account_id: String,
    /// `EnrollmentStatus`: `Active` | `Inactive`.
    pub status: String,
    /// Whether the last `UpdateEnrollmentStatus` asked for the organization's
    /// member accounts to be enrolled as well.
    pub include_member_accounts: bool,
    pub created_timestamp: DateTime,
    pub last_updated_timestamp: DateTime,
}

/// An account's Cost Optimization Hub preferences. Every member is optional
/// because an account that has never called `UpdatePreferences` has expressed no
/// preference at all, and `GetPreferences` reports exactly what was stored.
#[derive(Debug, Clone, Default)]
pub struct Preferences {
    /// `SavingsEstimationMode`: `AfterDiscounts` | `BeforeDiscounts`.
    pub savings_estimation_mode: Option<String>,
    /// `MemberAccountDiscountVisibility`: `All` | `None`.
    pub member_account_discount_visibility: Option<String>,
    pub preferred_commitment: Option<PreferredCommitment>,
}

impl Preferences {
    /// Apply one `UpdatePreferences` request: every member the request supplies
    /// replaces the stored one, and every member it omits keeps its stored value.
    /// The rule applies at each level, so supplying `preferredCommitment.term`
    /// alone leaves the stored `paymentOption` untouched.
    pub fn merge(
        &mut self,
        savings_estimation_mode: Option<String>,
        member_account_discount_visibility: Option<String>,
        preferred_commitment: Option<PreferredCommitment>,
    ) {
        if let Some(mode) = savings_estimation_mode {
            self.savings_estimation_mode = Some(mode);
        }
        if let Some(visibility) = member_account_discount_visibility {
            self.member_account_discount_visibility = Some(visibility);
        }
        if let Some(commitment) = preferred_commitment {
            let mut stored = self.preferred_commitment.take().unwrap_or_default();
            if let Some(term) = commitment.term {
                stored.term = Some(term);
            }
            if let Some(payment_option) = commitment.payment_option {
                stored.payment_option = Some(payment_option);
            }
            self.preferred_commitment = Some(stored);
        }
    }
}

/// The monthly price of one resource, split the way `ResourcePricing` models it:
/// the cost before any discount, plus the discount amounts that apply to it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pricing {
    pub cost_before_discounts: f64,
    pub savings_plans_discount: f64,
    pub reserved_instances_discount: f64,
    pub other_discount: f64,
}

impl Pricing {
    /// A price with no discount applied to it.
    const fn list(cost_before_discounts: f64) -> Self {
        Self {
            cost_before_discounts,
            savings_plans_discount: 0.0,
            reserved_instances_discount: 0.0,
            other_discount: 0.0,
        }
    }

    /// A price with a savings-plan discount applied to it.
    const fn discounted(cost_before_discounts: f64, savings_plans_discount: f64) -> Self {
        Self {
            cost_before_discounts,
            savings_plans_discount,
            reserved_instances_discount: 0.0,
            other_discount: 0.0,
        }
    }

    pub fn discounts_total(&self) -> f64 {
        self.savings_plans_discount + self.reserved_instances_discount + self.other_discount
    }

    pub fn cost_after_discounts(&self) -> f64 {
        round_cents(self.cost_before_discounts - self.discounts_total())
    }

    /// The cost to report under a stored `savingsEstimationMode`: after
    /// discounts when the account set `AfterDiscounts`, otherwise the
    /// undiscounted cost (which is what `BeforeDiscounts` names, and what an
    /// account that has expressed no preference is shown).
    pub fn cost_for_mode(&self, mode: Option<&str>) -> f64 {
        if mode == Some(AFTER_DISCOUNTS) {
            self.cost_after_discounts()
        } else {
            round_cents(self.cost_before_discounts)
        }
    }
}

/// The configuration half of a recommendation's `ResourceDetails`: one variant
/// per resource type in the catalog, carrying the members that resource type's
/// configuration shape declares.
#[derive(Debug, Clone)]
pub enum ResourceConfig {
    Ec2Instance {
        instance_type: String,
    },
    EbsVolume {
        volume_type: String,
        size_in_gb: f64,
        iops: f64,
        throughput: f64,
    },
    LambdaFunction {
        memory_size_in_mb: i32,
        architecture: String,
    },
    ComputeSavingsPlans {
        term: String,
        payment_option: String,
        hourly_commitment: String,
    },
    NatGateway {
        active_connection_count: i64,
    },
    AuroraDbClusterStorage {
        storage_type: String,
    },
}

/// One recommendation in the catalog. `ListRecommendations` renders the list
/// view of it and `GetRecommendation` the detail view, so both answers come from
/// one row and cannot disagree.
#[derive(Debug, Clone)]
pub struct StoredRecommendation {
    pub recommendation_id: String,
    pub account_id: String,
    pub region: String,
    pub resource_id: String,
    pub resource_arn: String,
    /// `ResourceType` values, from the model's enum of the same name.
    pub current_resource_type: String,
    pub recommended_resource_type: String,
    pub current_resource_summary: String,
    pub recommended_resource_summary: String,
    pub current_resource_config: ResourceConfig,
    pub recommended_resource_config: ResourceConfig,
    /// `ActionType` value.
    pub action_type: String,
    /// `ImplementationEffort` value.
    pub implementation_effort: String,
    pub restart_needed: bool,
    pub rollback_possible: bool,
    pub current_pricing: Pricing,
    pub recommended_pricing: Pricing,
    pub recommendation_lookback_period_in_days: i32,
    pub cost_calculation_lookback_period_in_days: i32,
    pub currency_code: String,
    /// `Source` value: `ComputeOptimizer` | `CostExplorer`.
    pub source: String,
    pub last_refresh_timestamp: DateTime,
    pub tags: Vec<(String, String)>,
}

impl StoredRecommendation {
    /// The current resource's monthly cost under `mode`.
    pub fn monthly_cost(&self, mode: Option<&str>) -> f64 {
        self.current_pricing.cost_for_mode(mode)
    }

    /// What adopting the recommendation saves per month under `mode`: the
    /// difference between the current and the recommended resource's cost.
    pub fn monthly_savings(&self, mode: Option<&str>) -> f64 {
        round_cents(
            self.current_pricing.cost_for_mode(mode) - self.recommended_pricing.cost_for_mode(mode),
        )
    }

    /// The monthly saving as a percentage of the current monthly cost.
    pub fn savings_percentage(&self, mode: Option<&str>) -> f64 {
        percentage_of(self.monthly_savings(mode), self.monthly_cost(mode))
    }

    /// The saving over the cost-calculation lookback period: the monthly saving
    /// prorated across that period's length.
    pub fn savings_over_cost_calculation_lookback_period(&self, mode: Option<&str>) -> f64 {
        round_cents(
            self.monthly_savings(mode) * f64::from(self.cost_calculation_lookback_period_in_days)
                / DAYS_PER_MONTH,
        )
    }

    /// The recommendation's tags as the modelled `TagList`, or `None` when it
    /// carries none — the list's `@length` is `1..100`, so an empty list is not
    /// a value this member can take.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some(key.clone()),
                    value: Some(value.clone()),
                })
                .collect(),
        )
    }

    pub fn current_resource_details(&self) -> ResourceDetails {
        resource_details(&self.current_resource_config, &self.current_pricing)
    }

    pub fn recommended_resource_details(&self) -> ResourceDetails {
        resource_details(&self.recommended_resource_config, &self.recommended_pricing)
    }

    /// Whether this recommendation satisfies `filter`: every member the filter
    /// sets must match, and a member that lists values matches when the
    /// recommendation's value is one of them.
    pub fn matches_filter(&self, filter: &Filter) -> bool {
        if filter
            .restart_needed
            .is_some_and(|wanted| wanted != self.restart_needed)
        {
            return false;
        }
        if filter
            .rollback_possible
            .is_some_and(|wanted| wanted != self.rollback_possible)
        {
            return false;
        }
        if !matches_any(&filter.recommendation_ids, &self.recommendation_id)
            || !matches_any(&filter.account_ids, &self.account_id)
            || !matches_any(&filter.regions, &self.region)
            || !matches_any(&filter.resource_ids, &self.resource_id)
            || !matches_any(&filter.resource_arns, &self.resource_arn)
            || !matches_any(&filter.action_types, &self.action_type)
            || !matches_any(&filter.implementation_efforts, &self.implementation_effort)
        {
            return false;
        }
        // `resourceTypes` filters on the resource type the recommendation is
        // about, which is the type the resource has today.
        if !matches_any(&filter.resource_types, &self.current_resource_type) {
            return false;
        }
        if filter
            .tags
            .as_ref()
            .is_some_and(|tags| !tags.iter().any(|wanted| self.has_tag(wanted)))
        {
            return false;
        }
        true
    }

    /// Whether the recommendation carries `wanted`: the key must match, and the
    /// value too when the filter states one.
    fn has_tag(&self, wanted: &Tag) -> bool {
        self.tags.iter().any(|(key, value)| {
            wanted.key.as_deref().is_none_or(|k| k == key)
                && wanted.value.as_deref().is_none_or(|v| v == value)
        })
    }

    /// The value of one `OrderBy` dimension, as a sort key.
    pub fn dimension_key(&self, dimension: Dimension) -> SortKey {
        match dimension {
            Dimension::RecommendationId => SortKey::Text(self.recommendation_id.clone()),
            Dimension::AccountId => SortKey::Text(self.account_id.clone()),
            Dimension::Region => SortKey::Text(self.region.clone()),
            Dimension::ResourceId => SortKey::Text(self.resource_id.clone()),
            Dimension::ResourceArn => SortKey::Text(self.resource_arn.clone()),
            Dimension::CurrentResourceType => SortKey::Text(self.current_resource_type.clone()),
            Dimension::RecommendedResourceType => {
                SortKey::Text(self.recommended_resource_type.clone())
            }
            Dimension::ActionType => SortKey::Text(self.action_type.clone()),
            Dimension::ImplementationEffort => SortKey::Text(self.implementation_effort.clone()),
            Dimension::Source => SortKey::Text(self.source.clone()),
            Dimension::CurrencyCode => SortKey::Text(self.currency_code.clone()),
            Dimension::RestartNeeded => SortKey::Number(f64::from(u8::from(self.restart_needed))),
            Dimension::RollbackPossible => {
                SortKey::Number(f64::from(u8::from(self.rollback_possible)))
            }
            Dimension::RecommendationLookbackPeriodInDays => {
                SortKey::Number(f64::from(self.recommendation_lookback_period_in_days))
            }
            Dimension::LastRefreshTimestamp => {
                SortKey::Number(self.last_refresh_timestamp.as_secs_f64())
            }
        }
    }
}

/// How many days one month of cost covers, for prorating a monthly figure over a
/// lookback period measured in days.
const DAYS_PER_MONTH: f64 = 30.0;

/// A dimension `ListRecommendations`' `orderBy` can sort on. Each one is a
/// `Recommendation` member that carries an ordered value, named as the model
/// names it (`smithy_query query="shape" shape="Recommendation"`); the two
/// mode-dependent money members are not here because the value they carry
/// depends on the caller's stored `savingsEstimationMode`, so
/// [`Dimension::parse`] resolves them in the handler instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    RecommendationId,
    AccountId,
    Region,
    ResourceId,
    ResourceArn,
    CurrentResourceType,
    RecommendedResourceType,
    ActionType,
    ImplementationEffort,
    Source,
    CurrencyCode,
    RestartNeeded,
    RollbackPossible,
    RecommendationLookbackPeriodInDays,
    LastRefreshTimestamp,
}

/// A sort key: a dimension's value in a form two recommendations can be ordered
/// by.
#[derive(Debug, Clone, PartialEq)]
pub enum SortKey {
    Text(String),
    Number(f64),
}

impl SortKey {
    /// Order two keys of the same dimension. Keys of different kinds never meet:
    /// one dimension always yields one kind.
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => a.total_cmp(b),
            (Self::Text(_), Self::Number(_)) => std::cmp::Ordering::Less,
            (Self::Number(_), Self::Text(_)) => std::cmp::Ordering::Greater,
        }
    }
}

/// The money dimensions whose value depends on the stored
/// `savingsEstimationMode`, resolved against a recommendation by the handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyDimension {
    EstimatedMonthlyCost,
    EstimatedMonthlySavings,
    EstimatedSavingsPercentage,
}

/// Either kind of `orderBy` dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDimension {
    Plain(Dimension),
    Money(MoneyDimension),
}

impl Dimension {
    /// Resolve an `orderBy.dimension` value, or `None` when it names no
    /// `Recommendation` member. Compared case-insensitively, so `accountId` and
    /// `AccountId` name the same member.
    pub fn parse(dimension: &str) -> Option<OrderDimension> {
        let key = dimension.trim().to_ascii_lowercase();
        let plain = match key.as_str() {
            "recommendationid" => Self::RecommendationId,
            "accountid" => Self::AccountId,
            "region" => Self::Region,
            "resourceid" => Self::ResourceId,
            "resourcearn" => Self::ResourceArn,
            "currentresourcetype" => Self::CurrentResourceType,
            "recommendedresourcetype" => Self::RecommendedResourceType,
            "actiontype" => Self::ActionType,
            "implementationeffort" => Self::ImplementationEffort,
            "source" => Self::Source,
            "currencycode" => Self::CurrencyCode,
            "restartneeded" => Self::RestartNeeded,
            "rollbackpossible" => Self::RollbackPossible,
            "recommendationlookbackperiodindays" => Self::RecommendationLookbackPeriodInDays,
            "lastrefreshtimestamp" => Self::LastRefreshTimestamp,
            "estimatedmonthlycost" => {
                return Some(OrderDimension::Money(MoneyDimension::EstimatedMonthlyCost))
            }
            "estimatedmonthlysavings" => {
                return Some(OrderDimension::Money(
                    MoneyDimension::EstimatedMonthlySavings,
                ))
            }
            "estimatedsavingspercentage" => {
                return Some(OrderDimension::Money(
                    MoneyDimension::EstimatedSavingsPercentage,
                ))
            }
            _ => return None,
        };
        Some(OrderDimension::Plain(plain))
    }
}

/// Whether `value` is in `allowed`, treating an absent list as "not filtered".
fn matches_any(allowed: &Option<Vec<String>>, value: &str) -> bool {
    allowed
        .as_ref()
        .is_none_or(|values| values.iter().any(|v| v == value))
}

/// Round a money amount to whole cents, so a figure derived by subtraction does
/// not report binary-float noise.
pub fn round_cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

/// `part` as a percentage of `whole`, to two decimal places; zero when `whole`
/// is zero, because no percentage of nothing is defined.
pub fn percentage_of(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    round_cents(part / whole * 100.0)
}

/// Build the modelled `ResourceDetails` union for one resource configuration and
/// its price.
fn resource_details(config: &ResourceConfig, pricing: &Pricing) -> ResourceDetails {
    match config {
        ResourceConfig::Ec2Instance { instance_type } => {
            ResourceDetails::ec2Instance(Ec2Instance {
                configuration: Some(Ec2InstanceConfiguration {
                    instance: Some(InstanceConfiguration {
                        r#type: Some(instance_type.clone()),
                    }),
                }),
                cost_calculation: Some(resource_cost_calculation(pricing)),
            })
        }
        ResourceConfig::EbsVolume {
            volume_type,
            size_in_gb,
            iops,
            throughput,
        } => ResourceDetails::ebsVolume(EbsVolume {
            configuration: Some(EbsVolumeConfiguration {
                storage: Some(StorageConfiguration {
                    r#type: Some(volume_type.clone()),
                    size_in_gb: Some(*size_in_gb),
                }),
                performance: Some(BlockStoragePerformanceConfiguration {
                    iops: Some(*iops),
                    throughput: Some(*throughput),
                }),
                attachment_state: Some("attached".to_string()),
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::LambdaFunction {
            memory_size_in_mb,
            architecture,
        } => ResourceDetails::lambdaFunction(LambdaFunction {
            configuration: Some(LambdaFunctionConfiguration {
                compute: Some(ComputeConfiguration {
                    v_cpu: None,
                    memory_size_in_mb: Some(*memory_size_in_mb),
                    architecture: Some(architecture.clone()),
                    platform: None,
                }),
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::ComputeSavingsPlans {
            term,
            payment_option,
            hourly_commitment,
        } => ResourceDetails::computeSavingsPlans(ComputeSavingsPlans {
            configuration: Some(ComputeSavingsPlansConfiguration {
                account_scope: Some("Payer".to_string()),
                term: Some(term.clone()),
                payment_option: Some(payment_option.clone()),
                hourly_commitment: Some(hourly_commitment.clone()),
            }),
            cost_calculation: Some(SavingsPlansCostCalculation {
                pricing: Some(SavingsPlansPricing {
                    monthly_savings_plans_eligible_cost: Some(round_cents(
                        pricing.cost_before_discounts,
                    )),
                    estimated_monthly_commitment: Some(pricing.cost_after_discounts()),
                    savings_percentage: Some(percentage_of(
                        pricing.discounts_total(),
                        pricing.cost_before_discounts,
                    )),
                    estimated_on_demand_cost: Some(round_cents(pricing.cost_before_discounts)),
                }),
            }),
        }),
        ResourceConfig::NatGateway {
            active_connection_count,
        } => ResourceDetails::natGateway(NatGateway {
            configuration: Some(NatGatewayConfiguration {
                active_connection_count: Some(*active_connection_count),
                packets_in_from_source: None,
                packets_in_from_destination: None,
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::AuroraDbClusterStorage { storage_type } => {
            ResourceDetails::auroraDbClusterStorage(AuroraDbClusterStorage {
                configuration: Some(AuroraDbClusterStorageConfiguration {
                    storage_type: Some(storage_type.clone()),
                }),
                cost_calculation: Some(resource_cost_calculation(pricing)),
            })
        }
    }
}

/// The `ResourceCostCalculation` for a price: the modelled split of the cost
/// before discounts, the discounts, and the cost after them.
fn resource_cost_calculation(pricing: &Pricing) -> ResourceCostCalculation {
    ResourceCostCalculation {
        usages: None,
        pricing: Some(ResourcePricing {
            estimated_cost_before_discounts: Some(round_cents(pricing.cost_before_discounts)),
            estimated_net_unused_amortized_commitments: None,
            estimated_discounts: Some(EstimatedDiscounts {
                savings_plans_discount: Some(round_cents(pricing.savings_plans_discount)),
                reserved_instances_discount: Some(round_cents(pricing.reserved_instances_discount)),
                other_discount: Some(round_cents(pricing.other_discount)),
            }),
            estimated_cost_after_discounts: Some(pricing.cost_after_discounts()),
        }),
    }
}

/// The account whose Cost Optimization Hub data a request addresses.
///
/// Every generated emulator presents the one canonical account id
/// ([`engine::emulator_account_id`]); it is never derived from the caller's
/// access key.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// Version marker inside every pagination token this service issues, so a token
/// from an older encoding is refused rather than misread.
const PAGE_TOKEN_VERSION: &str = "cohv1";

/// Encode the offset a page ends at into an opaque `nextToken`.
///
/// `kind` names the list the token belongs to, so a token issued by one
/// paginated operation is not accepted by another.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{kind}:{offset}"))
}

/// The offset a `nextToken` resumes at, or `None` when the token was not issued
/// by [`encode_page_token`] for this `kind`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}

/// One page of a list: the rows to return and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: &'a [T],
    pub next_token: Option<String>,
}

/// Cut the page `next_token` asks for out of `all`.
///
/// `max_results` caps the page; absent, the whole remainder is returned. The
/// token is issued only when rows remain after this page, so the last page
/// carries none.
pub fn paginate<'a, T>(
    kind: &str,
    all: &'a [T],
    offset: usize,
    max_results: Option<i32>,
) -> Page<'a, T> {
    let start = offset.min(all.len());
    let remaining = &all[start..];
    let limit = max_results.map_or(remaining.len(), |m| usize::try_from(m).unwrap_or(0));
    let items = &remaining[..limit.min(remaining.len())];
    let end = start + items.len();
    let next_token = if items.is_empty() || end >= all.len() {
        None
    } else {
        Some(encode_page_token(kind, end))
    };
    Page { items, next_token }
}

/// Per-service state for com-amazonaws-costoptimizationhub-costoptimizationhubservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// Enrollment per account id, written by `UpdateEnrollmentStatus`.
    pub enrollments: HashMap<String, Enrollment>,
    /// Preferences per account id, written by `UpdatePreferences`.
    pub preferences: HashMap<String, Preferences>,
    /// The recommendation catalog, by recommendation id.
    pub recommendations: HashMap<String, StoredRecommendation>,
    /// The catalog's order, so listing and paging it are stable across calls.
    pub recommendation_order: Vec<String>,
}

impl Default for ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// The store's initial value: no enrollment and no preference (nobody has
    /// called yet), and the recommendation catalog this emulator reports.
    ///
    /// The catalog is part of the initial value rather than something a reader
    /// writes on first use, so `ListRecommendations` and `GetRecommendation`
    /// serve the same rows without either of them mutating the store.
    fn default() -> Self {
        let mut state = Self {
            enrollments: HashMap::new(),
            preferences: HashMap::new(),
            recommendations: HashMap::new(),
            recommendation_order: Vec::new(),
        };
        for recommendation in seed_recommendations() {
            state
                .recommendation_order
                .push(recommendation.recommendation_id.clone());
            state
                .recommendations
                .insert(recommendation.recommendation_id.clone(), recommendation);
        }
        state
    }
}

/// The recommendation catalog, generated once when the store is created.
///
/// The emulator observes no real usage, so the catalog is a fixed set of
/// resources rather than something derived from traffic; every figure a
/// recommendation reports is nevertheless computed from the prices stored here
/// (so cost, saving and percentage agree with each other and with the
/// `ResourceDetails` cost calculation), and both recommendation readers read
/// these rows.
fn seed_recommendations() -> Vec<StoredRecommendation> {
    let account_id = caller_account_id().to_string();
    let now = DateTime::from(std::time::SystemTime::now());
    let east = "us-east-1";
    let west = "us-west-2";

    vec![
        // Two recommendations for ONE instance: rightsizing it, or moving it to
        // Graviton. `includeAllRecommendations` decides whether a caller sees
        // both or only the better one.
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0001".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "i-0a1b2c3d4e5f60001".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60001"),
            current_resource_type: "Ec2Instance".to_string(),
            recommended_resource_type: "Ec2Instance".to_string(),
            current_resource_summary: "m5.2xlarge".to_string(),
            recommended_resource_summary: "m5.xlarge".to_string(),
            current_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.2xlarge".to_string(),
            },
            recommended_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.xlarge".to_string(),
            },
            action_type: "Rightsize".to_string(),
            implementation_effort: "VeryLow".to_string(),
            restart_needed: true,
            rollback_possible: true,
            current_pricing: Pricing::discounted(280.32, 28.03),
            recommended_pricing: Pricing::discounted(140.16, 14.02),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("team".to_string(), "payments".to_string()),
            ],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0002".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "i-0a1b2c3d4e5f60001".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60001"),
            current_resource_type: "Ec2Instance".to_string(),
            recommended_resource_type: "Ec2Instance".to_string(),
            current_resource_summary: "m5.2xlarge".to_string(),
            recommended_resource_summary: "m6g.2xlarge".to_string(),
            current_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.2xlarge".to_string(),
            },
            recommended_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m6g.2xlarge".to_string(),
            },
            action_type: "MigrateToGraviton".to_string(),
            implementation_effort: "Medium".to_string(),
            restart_needed: true,
            rollback_possible: true,
            current_pricing: Pricing::discounted(280.32, 28.03),
            recommended_pricing: Pricing::discounted(224.26, 22.43),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("team".to_string(), "payments".to_string()),
            ],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0003".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "vol-00112233445566003".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "volume/vol-00112233445566003"),
            current_resource_type: "EbsVolume".to_string(),
            recommended_resource_type: "EbsVolume".to_string(),
            current_resource_summary: "gp2 500 GiB".to_string(),
            recommended_resource_summary: "gp3 500 GiB".to_string(),
            current_resource_config: ResourceConfig::EbsVolume {
                volume_type: "gp2".to_string(),
                size_in_gb: 500.0,
                iops: 1500.0,
                throughput: 128.0,
            },
            recommended_resource_config: ResourceConfig::EbsVolume {
                volume_type: "gp3".to_string(),
                size_in_gb: 500.0,
                iops: 3000.0,
                throughput: 125.0,
            },
            action_type: "Upgrade".to_string(),
            implementation_effort: "Low".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(50.0),
            recommended_pricing: Pricing::list(40.0),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "production".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0004".to_string(),
            account_id: account_id.clone(),
            region: west.to_string(),
            resource_id: "invoice-renderer".to_string(),
            resource_arn: ::engine::emulator_arn("lambda", west, "function:invoice-renderer"),
            current_resource_type: "LambdaFunction".to_string(),
            recommended_resource_type: "LambdaFunction".to_string(),
            current_resource_summary: "3008 MB, x86_64".to_string(),
            recommended_resource_summary: "1024 MB, x86_64".to_string(),
            current_resource_config: ResourceConfig::LambdaFunction {
                memory_size_in_mb: 3008,
                architecture: "x86_64".to_string(),
            },
            recommended_resource_config: ResourceConfig::LambdaFunction {
                memory_size_in_mb: 1024,
                architecture: "x86_64".to_string(),
            },
            action_type: "Rightsize".to_string(),
            implementation_effort: "Low".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(72.4),
            recommended_pricing: Pricing::list(24.8),
            recommendation_lookback_period_in_days: 30,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "staging".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0005".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "compute-savings-plan-recommendation".to_string(),
            resource_arn: ::engine::emulator_arn(
                "savingsplans",
                "",
                "savingsplan/compute-savings-plan-recommendation",
            ),
            current_resource_type: "ComputeSavingsPlans".to_string(),
            recommended_resource_type: "ComputeSavingsPlans".to_string(),
            current_resource_summary: "No commitment".to_string(),
            recommended_resource_summary: "ThreeYears, AllUpfront, 1.20 USD/hour".to_string(),
            current_resource_config: ResourceConfig::ComputeSavingsPlans {
                term: "ThreeYears".to_string(),
                payment_option: "AllUpfront".to_string(),
                hourly_commitment: "0.00".to_string(),
            },
            recommended_resource_config: ResourceConfig::ComputeSavingsPlans {
                term: "ThreeYears".to_string(),
                payment_option: "AllUpfront".to_string(),
                hourly_commitment: "1.20".to_string(),
            },
            action_type: "PurchaseSavingsPlans".to_string(),
            implementation_effort: "VeryLow".to_string(),
            restart_needed: false,
            rollback_possible: false,
            current_pricing: Pricing::list(1_314.0),
            recommended_pricing: Pricing::discounted(1_314.0, 438.0),
            recommendation_lookback_period_in_days: 60,
            cost_calculation_lookback_period_in_days: 60,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: Vec::new(),
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0006".to_string(),
            account_id: account_id.clone(),
            region: west.to_string(),
            resource_id: "nat-0f1e2d3c4b5a60006".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", west, "natgateway/nat-0f1e2d3c4b5a60006"),
            current_resource_type: "NatGateway".to_string(),
            recommended_resource_type: "NatGateway".to_string(),
            current_resource_summary: "Idle NAT gateway".to_string(),
            recommended_resource_summary: "Delete".to_string(),
            current_resource_config: ResourceConfig::NatGateway {
                active_connection_count: 0,
            },
            recommended_resource_config: ResourceConfig::NatGateway {
                active_connection_count: 0,
            },
            action_type: "Delete".to_string(),
            implementation_effort: "High".to_string(),
            restart_needed: true,
            rollback_possible: false,
            current_pricing: Pricing::list(32.85),
            recommended_pricing: Pricing::list(0.0),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 14,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "sandbox".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0007".to_string(),
            account_id,
            region: west.to_string(),
            resource_id: "ledger-aurora-cluster".to_string(),
            resource_arn: ::engine::emulator_arn("rds", west, "cluster:ledger-aurora-cluster"),
            current_resource_type: "AuroraDbClusterStorage".to_string(),
            recommended_resource_type: "AuroraDbClusterStorage".to_string(),
            current_resource_summary: "aurora".to_string(),
            recommended_resource_summary: "aurora-iopt1".to_string(),
            current_resource_config: ResourceConfig::AuroraDbClusterStorage {
                storage_type: "aurora".to_string(),
            },
            recommended_resource_config: ResourceConfig::AuroraDbClusterStorage {
                storage_type: "aurora-iopt1".to_string(),
            },
            action_type: "Upgrade".to_string(),
            implementation_effort: "VeryHigh".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(910.5),
            recommended_pricing: Pricing::list(728.4),
            recommendation_lookback_period_in_days: 30,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("team".to_string(), "ledger".to_string())],
        },
    ]
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState>)
}
