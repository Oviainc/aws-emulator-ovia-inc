// SPDX-License-Identifier: CC0-1.0

//! The idle resource analysis `GetIdleRecommendations` reports: the record the analysis
//! holds for an idle resource, and the filters, ordering and vocabulary a reader applies.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

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

/// The `IdleFinding` values (`crate::ops::types::IDLE_FINDING_VALUES`), named so the rules
/// below read as the model's own vocabulary.
pub const IDLE_FINDING_IDLE: &str = "Idle";
/// See [`IDLE_FINDING_IDLE`].
pub const IDLE_FINDING_UNATTACHED: &str = "Unattached";
/// See [`IDLE_FINDING_IDLE`].
pub const IDLE_FINDING_UNUSED: &str = "Unused";

/// One resource Compute Optimizer's analysis found IDLE: the finding it reached, the
/// utilization it observed as the evidence for it, how long it has been observing the
/// resource and what the resource costs its account per month.
///
/// This is the OBSERVATION; every member of the `IdleRecommendation` a reader reports is
/// DERIVED from it ([`Self::to_recommendation`]), so no two reads of one resource can
/// disagree about its finding, the period analysed or what dropping it would save. A record
/// keeps the resource's REGION-FREE identity for the reason [`resource_arn_in`] gives: the
/// ARN a reader reports names the region of the endpoint the request addressed.
#[derive(Debug, Clone)]
pub struct IdleResourceAnalysis {
    /// The account that owns the resource (`IdleRecommendation$accountId`).
    pub account_id: String,
    /// The service namespace of the resource's ARN — its `ec2`, `dynamodb`, … field — which
    /// together with [`Self::arn_resource`] decides the `IdleRecommendationResourceType`
    /// reported for it ([`idle_resource_type_of`]).
    pub arn_service: String,
    /// Everything after the account field of the resource's ARN
    /// (`natgateway/nat-0a1b2c3d4e5f60001`), separators included.
    pub arn_resource: String,
    /// The `IdleFinding` the analysis reached: [`IDLE_FINDING_IDLE`],
    /// [`IDLE_FINDING_UNATTACHED`] or [`IDLE_FINDING_UNUSED`].
    pub finding: String,
    /// The `IdleUtilizationMetric`s the analysis observed — the evidence for the finding, in
    /// the `IdleMetricName` vocabulary the resource's own kind publishes. An idle resource's
    /// utilization does not vary over the period, so the value observed is the value over
    /// any part of it.
    pub utilization_metrics: Vec<IdleUtilizationMetric>,
    /// How many days Compute Optimizer has been observing this resource. A look-back period
    /// longer than this cannot be honoured, so what a reader reports as analysed is the
    /// shorter of the two ([`Self::analysed_days`]).
    pub days_observed: f64,
    /// What the resource costs its account per month, in [`CURRENCY_USD`]: the figure the
    /// savings opportunity is computed from, because the recommendation for a resource
    /// nothing is using is to stop paying for it.
    pub monthly_cost: f64,
    /// The resource's tags, as key/value pairs, in the order its own service reports them.
    pub tags: Vec<(String, String)>,
}

impl IdleResourceAnalysis {
    /// This resource's REGION-FREE identity — the key [`IdleRecommendationStore`] holds the
    /// record under ([`resource_identity`]).
    pub fn identity(&self) -> String {
        resource_identity(&self.arn_service, &self.account_id, &self.arn_resource)
    }

    /// This resource's ARN as the endpoint in `region` reports it ([`resource_arn_in`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(&self.arn_service, region, &self.account_id, &self.arn_resource)
    }

    /// `IdleRecommendation$resourceId` — "the unique identifier for the resource": the last
    /// segment of its ARN's resource part, which is the id or name the resource's own service
    /// gives it (`natgateway/nat-0a1b…` -> `nat-0a1b…`).
    pub fn resource_id(&self) -> String {
        self.arn_resource
            .split(['/', ':'])
            .next_back()
            .unwrap_or_default()
            .to_string()
    }

    /// The `IdleRecommendationResourceType` this resource is, or `None` for a resource kind
    /// idle recommendations do not cover ([`idle_resource_type_of`]).
    pub fn resource_type(&self) -> Option<&'static str> {
        idle_resource_type_of(&self.arn_service, resource_kind_of(&self.arn_resource))
    }

    /// How many days of utilization a recommendation for this resource reports as analysed
    /// under `look_back_period`: the period in force, capped by how long the resource has
    /// been observed — the same rule every other observation of this service follows.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// What adopting this recommendation saves per month, and that saving as a percentage of
    /// what the resource costs.
    ///
    /// `IdleSavingsOpportunity$savingsOpportunityPercentage` is "the estimated monthly
    /// savings possible as a percentage of monthly cost by adopting Compute Optimizer's idle
    /// resource recommendations", and the recommendation for a resource the analysis found
    /// idle is to stop paying for it: the whole of what it costs is saved, whatever kind of
    /// resource it is. A resource costing nothing saves nothing.
    pub fn savings(&self) -> (f64, f64) {
        let cost = round_hundredths(self.monthly_cost.max(0.0));
        (percentage_of(cost, cost), cost)
    }

    /// `findingDescription` — "a summary of the findings for the resource": what the stored
    /// finding says about this kind of resource over the days analysed. Derived from the
    /// record, so the sentence cannot disagree with the members reported beside it.
    pub fn finding_description(&self, look_back_days: f64) -> String {
        let resource_type = self.resource_type().unwrap_or("resource");
        let days = round_hundredths(look_back_days);
        match self.finding.as_str() {
            IDLE_FINDING_UNATTACHED => format!(
                "The {resource_type} was not attached to any resource over the last {days} days."
            ),
            IDLE_FINDING_UNUSED => {
                format!("The {resource_type} was not used over the last {days} days.")
            }
            // `Idle`, and anything else the vocabulary holds: the analysis saw next to no use.
            _ => format!(
                "The {resource_type} showed little to no utilization over the last {days} days."
            ),
        }
    }

    /// The modelled `IdleRecommendation` this analysis yields at the endpoint in `region`
    /// under `look_back_period`, as generated at `last_refresh_timestamp`.
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> IdleRecommendation {
        let look_back_days = self.analysed_days(look_back_period);
        let (percentage, value) = self.savings();
        let estimated = || {
            Some(IdleEstimatedMonthlySavings {
                currency: Some(CURRENCY_USD.to_string()),
                value,
            })
        };
        IdleRecommendation {
            resource_arn: Some(self.arn_in(region)),
            resource_id: Some(self.resource_id()),
            resource_type: self.resource_type().map(ToString::to_string),
            account_id: Some(self.account_id.clone()),
            finding: Some(self.finding.clone()),
            finding_description: Some(self.finding_description(look_back_days)),
            savings_opportunity: Some(IdleSavingsOpportunity {
                savings_opportunity_percentage: percentage,
                estimated_monthly_savings: estimated(),
            }),
            // This emulator prices every resource from public pricing and records no
            // negotiated discount ([`SAVINGS_ESTIMATION_SOURCE`]), so the after-discount
            // figures are the before-discount ones — the same stance the volume, function and
            // group readers of this service take.
            savings_opportunity_after_discounts: Some(IdleSavingsOpportunityAfterDiscounts {
                savings_opportunity_percentage: percentage,
                estimated_monthly_savings: estimated(),
            }),
            utilization_metrics: (!self.utilization_metrics.is_empty())
                .then(|| self.utilization_metrics.clone()),
            look_back_period_in_days: look_back_days,
            last_refresh_timestamp,
            tags: modelled_tags(&self.tags),
        }
    }
}

/// The idle resource analysis this service holds, keyed by the REGION-FREE identity of the
/// resource it is about, in the order the analysis recorded it.
///
/// Compute Optimizer's own analysis of an observed workload is what produces an idle resource
/// recommendation, so NO operation of this API creates one: the analysis is part of the
/// store's initial value ([`seed_idle_resource_analyses`]) and `GetIdleRecommendations`
/// reports what is held here, never anything a request carried. `insert` is the only writer,
/// so the record order a page walks and the index a lookup uses cannot drift apart.
#[derive(Debug, Default)]
pub struct IdleRecommendationStore {
    /// Resource identities in record order, so a page walks a stable sequence.
    pub(super) order: Vec<String>,
    pub(super) by_identity: HashMap<String, IdleResourceAnalysis>,
}

impl IdleRecommendationStore {
    /// Record `analysis` under the identity of the resource it is about. Re-recording the
    /// same resource replaces the stored analysis in place, keeping its position in the
    /// order — a refreshed analysis of one resource does not reorder the rest.
    pub fn insert(&mut self, analysis: IdleResourceAnalysis) {
        let identity = analysis.identity();
        if !self.by_identity.contains_key(&identity) {
            self.order.push(identity.clone());
        }
        self.by_identity.insert(identity, analysis);
    }

    /// The analysis held for the resource `resource_arn` names, or `None` when the service
    /// holds none — which is what puts that ARN in a response's `errors` list. The lookup is
    /// region-free, so an ARN naming the resource at any regional endpoint finds it.
    pub fn get(&self, resource_arn: &str) -> Option<&IdleResourceAnalysis> {
        self.by_identity.get(&identity_of_arn(resource_arn)?)
    }

    /// Every recorded analysis, in record order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &IdleResourceAnalysis> {
        self.order
            .iter()
            .filter_map(|identity| self.by_identity.get(identity))
    }

    /// How many resources the analysis found idle.
    pub fn len(&self) -> usize {
        self.by_identity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_identity.is_empty()
    }
}

impl FromIterator<IdleResourceAnalysis> for IdleRecommendationStore {
    /// The store holding `analyses`, in the order they are given — how the seeded analysis
    /// becomes the store's initial value.
    fn from_iter<T: IntoIterator<Item = IdleResourceAnalysis>>(analyses: T) -> Self {
        let mut store = Self::default();
        for analysis in analyses {
            store.insert(analysis);
        }
        store
    }
}

/// The idle resource analysis this emulator holds, built once when the store is created.
///
/// Compute Optimizer "generates recommendations for idle resources that meet a specific set of
/// requirements" from the utilization it observes, so WHICH resources are idle is analysis
/// output and not something a request writes: like the EBS volume observation, the Auto
/// Scaling group analysis and the findings catalog, it is part of the store's initial value,
/// and every figure a reader reports is computed from what is recorded here.
///
/// The set holds the resource kinds Compute Optimizer reports ONLY as idle resources — a NAT
/// gateway, a DynamoDB table, an ElastiCache cluster, a MemoryDB cluster, a DocumentDB
/// cluster, a WorkSpace and a SageMaker endpoint — plus an UNATTACHED Amazon EBS volume,
/// which is not part of the volume inventory [`observed_volumes`] holds either, because a
/// volume attached to nothing carries no workload to rightsize. Every `IdleFinding` value
/// occurs, the costs all differ so an ordering is observable, the observed periods differ so a
/// look-back preference is observable, and each row's metrics are the ones its own resource
/// kind publishes.
///
/// A row records the resource's REGION-FREE identity ([`resource_arn_in`]): Compute Optimizer
/// is regional, so the region a reported ARN names is the region of the endpoint the request
/// addressed.
pub(super) fn seed_idle_resource_analyses() -> Vec<IdleResourceAnalysis> {
    let account_id = ::engine::emulator_account_id().to_string();
    // Every metric this analysis records is the highest value observed over the period, which
    // is the one statistic this API reports ([`METRIC_STATISTIC_MAXIMUM`]).
    let metric = |name: &str, value: f64| IdleUtilizationMetric {
        name: Some(name.to_string()),
        statistic: Some(METRIC_STATISTIC_MAXIMUM.to_string()),
        value,
        // The analysis recorded no dimension breakdown for these metrics.
        dimensions: None,
    };
    let row = |arn_service: &str,
               arn_resource: &str,
               finding: &str,
               utilization_metrics: Vec<IdleUtilizationMetric>,
               days_observed: f64,
               monthly_cost: f64,
               tags: &[(&str, &str)]| IdleResourceAnalysis {
        account_id: account_id.clone(),
        arn_service: arn_service.to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        utilization_metrics,
        days_observed,
        monthly_cost: round_hundredths(monthly_cost),
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    };

    vec![
        // A NAT gateway nothing routes through: no connection and no packet from either
        // side. Priced at the published $0.045 per gateway-hour.
        row(
            "ec2",
            "natgateway/nat-0a1b2c3d4e5f60001",
            IDLE_FINDING_UNUSED,
            vec![
                metric("ActiveConnectionCount", 0.0),
                metric("PacketsInFromSource", 0.0),
                metric("PacketsInFromDestination", 0.0),
            ],
            60.0,
            0.045 * HOURS_PER_MONTH,
            &[("Name", "legacy-egress")],
        ),
        // A 200 GiB gp2 volume attached to nothing: no read and no write operation reaches
        // it. Priced by [`volume_monthly_cost`], the same table the volume reader prices
        // from, so one volume type has one price in this service.
        row(
            "ec2",
            "volume/vol-0a1b2c3d4e5f60009",
            IDLE_FINDING_UNATTACHED,
            vec![
                metric("VolumeReadOpsPerSecond", 0.0),
                metric("VolumeWriteOpsPerSecond", 0.0),
            ],
            93.0,
            volume_monthly_cost(&VolumeConfiguration {
                volume_type: Some("gp2".to_string()),
                volume_size: 200,
                volume_baseline_iops: 600,
                volume_burst_iops: 3_000,
                volume_baseline_throughput: 128,
                volume_burst_throughput: 128,
                root_volume: Some(false),
            })
            .unwrap_or_default(),
            &[("Owner", "TeamA")],
        ),
        // A provisioned DynamoDB table nothing reads or writes. Priced at its provisioned
        // capacity: 20 read and 20 write capacity units at the published $0.00013 and
        // $0.00065 per unit-hour.
        row(
            "dynamodb",
            "table/legacy-sessions",
            IDLE_FINDING_IDLE,
            vec![
                metric("ConsumedReadCapacityUnits", 0.0),
                metric("ConsumedWriteCapacityUnits", 0.0),
                metric("ConsumedChangeDataCaptureUnits", 0.0),
            ],
            14.0,
            20.0 * (0.000_13 + 0.000_65) * HOURS_PER_MONTH,
            &[("Owner", "TeamB")],
        ),
        // An ElastiCache cluster serving nothing: no command, no connection and a processor
        // that is all but idle. Priced at the published $0.154 per cache.m6g.large-hour.
        row(
            "elasticache",
            "cluster:sessions-cache",
            IDLE_FINDING_IDLE,
            vec![
                metric("EngineCPUUtilization", 0.8),
                metric("CurrConnections", 0.0),
                metric("GetTypeCmds", 0.0),
                metric("SetTypeCmds", 0.0),
            ],
            30.0,
            0.154 * HOURS_PER_MONTH,
            &[],
        ),
        // A MemoryDB cluster nothing has queried: no keyspace hit and no keyspace miss.
        // Priced at the published $0.068 per db.t4g.small-hour.
        row(
            "memorydb",
            "cluster/leaderboard-archive",
            IDLE_FINDING_UNUSED,
            vec![
                metric("KeyspaceHits", 0.0),
                metric("KeyspaceMisses", 0.0),
                metric("NewConnections", 0.0),
            ],
            9.0,
            0.068 * HOURS_PER_MONTH,
            &[("Owner", "TeamB"), ("Environment", "Archive")],
        ),
        // A DocumentDB cluster no client connects to. Priced at the published $0.078 per
        // db.t3.medium-hour.
        row(
            "rds",
            "cluster:docdb-reporting",
            IDLE_FINDING_IDLE,
            vec![
                metric("DatabaseConnections", 0.0),
                metric("CPU", 1.1),
                metric("Memory", 12.0),
            ],
            45.0,
            0.078 * HOURS_PER_MONTH,
            &[],
        ),
        // A WorkSpace nobody logs in to, billed monthly: the published $35.00 per month of a
        // Value bundle on monthly billing.
        row(
            "workspaces",
            "workspace/ws-0a1b2c3d4e5f6",
            IDLE_FINDING_UNUSED,
            vec![metric("UserConnected", 0.0), metric("IsIdle", 1.0)],
            93.0,
            35.00,
            &[("Owner", "TeamC")],
        ),
        // A SageMaker endpoint nothing invokes. Priced at the published $0.115 per
        // ml.m5.large-hour.
        row(
            "sagemaker",
            "endpoint/scoring-canary",
            IDLE_FINDING_IDLE,
            vec![metric("Invocations", 0.0), metric("CPU", 0.6)],
            21.0,
            0.115 * HOURS_PER_MONTH,
            &[("Owner", "TeamC")],
        ),
    ]
}

/// The `IdleRecommendationResourceType` the `<service>:<resource-kind>` ARN fields name, or
/// `None` for a resource kind idle recommendations do not cover.
///
/// The catalog is the model's own `IdleRecommendationResourceType` value set
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`), each value paired with
/// the `<service>:<resource-kind>` ARN fields AWS gives that resource. A DocumentDB cluster
/// carries the `rds` namespace, and the value set declares no other cluster of that
/// namespace — no Aurora or RDS DB cluster value — so `rds:cluster` maps to
/// `DocumentDBCluster`, the one cluster type idle recommendations cover.
pub fn idle_resource_type_of(service: &str, resource_kind: &str) -> Option<&'static str> {
    let resource_type = match (service, resource_kind) {
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

/// The `RecommendationSourceType` a resource of `idle_resource_type` is SUMMARISED under,
/// or `None` for a type outside the idle vocabulary.
///
/// A recommendation summary names its group with a `RecommendationSourceType`
/// (`RecommendationSummary$recommendationResourceType`) while an idle record names its
/// resource with an `IdleRecommendationResourceType`
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`), and the two enums spell
/// several of the same resources differently (`EC2Instance` against `Ec2Instance`,
/// `EBSVolume` against `EbsVolume`, `ECSService` against `EcsService`, `RDSDBInstance`
/// against `RdsDBInstance`); the rest spell them identically. Stated once here, so the idle
/// analysis of one resource is summarised under the same group the rightsizing analysis of
/// that resource is.
pub fn source_type_of_idle_resource_type(idle_resource_type: &str) -> Option<&'static str> {
    let source_type = match idle_resource_type {
        "EC2Instance" => SOURCE_TYPE_EC2_INSTANCE,
        "EBSVolume" => SOURCE_TYPE_EBS_VOLUME,
        "ECSService" => SOURCE_TYPE_ECS_SERVICE,
        "RDSDBInstance" => SOURCE_TYPE_RDS_DB_INSTANCE,
        "AutoScalingGroup" => SOURCE_TYPE_AUTO_SCALING_GROUP,
        "NatGateway" => SOURCE_TYPE_NAT_GATEWAY,
        "DynamoDBTable" => SOURCE_TYPE_DYNAMODB_TABLE,
        "ElastiCacheCluster" => SOURCE_TYPE_ELASTICACHE_CLUSTER,
        "MemoryDBCluster" => SOURCE_TYPE_MEMORYDB_CLUSTER,
        "DocumentDBCluster" => SOURCE_TYPE_DOCUMENTDB_CLUSTER,
        "WorkSpaces" => SOURCE_TYPE_WORKSPACES,
        "SageMakerEndpoint" => SOURCE_TYPE_SAGEMAKER_ENDPOINT,
        _ => return None,
    };
    debug_assert!(
        RECOMMENDATION_SOURCE_TYPE_VALUES.contains(&source_type),
        "a summary group may only name a modelled RecommendationSourceType value"
    );
    Some(source_type)
}

/// Whether this service's idle analysis covers resources of `source_type`, i.e. whether a
/// summary of that group has an `IdleFinding` vocabulary to count at all.
///
/// The idle vocabulary is `IdleRecommendationResourceType`
/// (`crate::ops::types::IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES`) and a summary names its
/// group with a `RecommendationSourceType`, so membership is decided by mapping the idle
/// vocabulary through [`source_type_of_idle_resource_type`] rather than by a second list
/// that could drift from it — and so a `RecommendationSourceType` outside the idle
/// vocabulary (a Lambda function, a license, a storage group) is summarised with no idle
/// counts at all.
pub fn reports_idle_recommendations(source_type: &str) -> bool {
    IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES
        .iter()
        .filter_map(|idle_resource_type| source_type_of_idle_resource_type(idle_resource_type))
        .any(|covered| covered == source_type)
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
