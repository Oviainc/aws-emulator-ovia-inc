// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationsignals-applicationsignals service.
//!
//! The service level objective (SLO) is this service's primary resource, so the store is
//! the set of SLOs this emulator holds plus the tags attached to each one by ARN. The
//! derivation helpers below live here, beside the store, because the SLO lifecycle
//! operations (`CreateServiceLevelObjective`, `GetServiceLevelObjective`,
//! `ListServiceLevelObjectives`, `UpdateServiceLevelObjective` and
//! `DeleteServiceLevelObjective`) must all agree on identity, on the ARN they mint and on
//! the server-derived members of a stored record — two copies of that logic would let a
//! Get disagree with the Create that wrote the record.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

use crate::ops::types::{
    DependencyConfig, Goal, Interval, MetricDataQuery, MetricSource,
    MonitoredRequestCountMetricDataQueries, RequestBasedServiceLevelIndicator,
    RequestBasedServiceLevelIndicatorConfig, RequestBasedServiceLevelIndicatorMetric,
    ResourceNotFoundException, RollingInterval, ServiceLevelIndicator,
    ServiceLevelIndicatorConfig, ServiceLevelIndicatorMetric, ServiceLevelObjective,
    ServiceLevelObjectiveSummary,
};

/// This service's SigV4 signing name, which is also its ARN namespace
/// (`OPERATIONS.md`: `sigv4 signing name: application-signals`, `ARN namespace:
/// application-signals`). The modelled `ServiceLevelObjectiveArn` pattern
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`) carries the same
/// namespace.
pub const SIGNING_NAME: &str = "application-signals";

/// The resource-type segment of an SLO ARN, from the modelled `ServiceLevelObjectiveArn`
/// pattern (`:slo/<name>`). Also the `ResourceType` a not-found failure reports.
pub const SLO_RESOURCE_TYPE: &str = "slo";

/// Region for a minted ARN when the request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region (`AGENTS.md`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EvaluationType` for an SLO configured with `SliConfig`
/// (`smithy_query(shape="EvaluationType")` -> `["PeriodBased", "RequestBased"]`).
pub const EVALUATION_TYPE_PERIOD_BASED: &str = "PeriodBased";

/// `EvaluationType` for an SLO configured with `RequestBasedSliConfig`.
pub const EVALUATION_TYPE_REQUEST_BASED: &str = "RequestBased";

/// The largest page `ListServiceLevelObjectives` returns when the caller names no
/// `MaxResults`: the modelled maximum of `ListServiceLevelObjectivesMaxResults`
/// (`range 1..50`), since the member declares no default.
pub const MAX_SLO_PAGE: i32 = 50;

/// The most SLOs this service holds, after which a create is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// The real service enforces a per-account, per-Region quota on service level objectives
/// (CloudWatch Application Signals, "Service level objectives" — a soft quota of 2,000),
/// and the create operation models `ServiceQuotaExceededException` precisely for it. The
/// cap is also what keeps the store bounded: the record count is otherwise chosen by the
/// caller, since nothing else in the create path evicts or expires a record.
pub const MAX_SLOS: usize = 2_000;

/// `Goal.Interval` when omitted. Model doc: "If you omit this parameter, a rolling
/// interval of 7 days is used."
const DEFAULT_ROLLING_INTERVAL_DURATION: i32 = 7;

/// The `DurationUnit` of that default rolling interval
/// (`smithy_query(shape="DurationUnit")` -> `["MINUTE", "HOUR", "DAY", "MONTH"]`).
const DEFAULT_ROLLING_INTERVAL_UNIT: &str = "DAY";

/// `Goal.AttainmentGoal` when omitted. Model doc: "If you omit this parameter, 99 is used
/// to represent 99% as the attainment goal."
const DEFAULT_ATTAINMENT_GOAL: f64 = 99.0;

/// `Goal.WarningThreshold` when omitted. Model doc: "If you omit this parameter, the
/// default of 50.0 is used."
const DEFAULT_WARNING_THRESHOLD: f64 = 50.0;

/// `ServiceLevelIndicatorConfig.MetricThreshold`'s modelled `@default` of `0`.
const DEFAULT_SLI_METRIC_THRESHOLD: f64 = 0.0;

/// `ServiceLevelIndicatorConfig.ComparisonOperator`'s modelled `@default` of `"LessThan"`.
const DEFAULT_SLI_COMPARISON_OPERATOR: &str = "LessThan";

/// The marker every `NextToken` this service issues carries, so a well-formed token this
/// service did not issue is refused instead of being read as a cursor. The `/` cannot occur
/// in an SLO name (the modelled `ServiceLevelObjectiveName` pattern admits only
/// `[-._0-9A-Za-z ]`), so the prefix can never be mistaken for part of the cursor.
const NEXT_TOKEN_TAG: &str = "slos/1/";

/// Per-service state for com-amazonaws-applicationsignals-applicationsignals. A handler reaches it through [`service_state`].
///
/// `slos` is keyed by SLO NAME: a name is unique within an account and region (the
/// `ConflictException` a repeated create answers is exactly that uniqueness), and the
/// ordered map gives `ListServiceLevelObjectives` a deterministic order to page over.
///
/// The key is the name ALONE, not `(region, name)`, because this store is one account's and
/// one Region's: a process holds a single engine state, every ARN it mints carries
/// `engine::emulator_account_id()`, and `ListServiceLevelObjectives` reports nothing for any
/// other account. The region in a minted ARN is therefore a rendering of the caller's SigV4
/// scope rather than part of an SLO's identity, which is also why [`slo_name_of`] resolves
/// an ARN by its name segment and ignores the rest.
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Every SLO created in this emulator, keyed by `ServiceLevelObjective::name`. Bounded
    /// at [`MAX_SLOS`] entries by the create path, which is the only writer.
    pub slos: BTreeMap<String, ServiceLevelObjective>,
    /// Tags per resource ARN, the key the service's tagging operations address a resource
    /// by (`TagResource`/`ListTagsForResource` take a `ResourceArn`). One entry per
    /// accepted create, so [`MAX_SLOS`] bounds this map too.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The stored SLO an `Id` names, accepting either form the modelled
    /// `ServiceLevelObjectiveId` pattern allows: a bare name or a full `slo/<name>` ARN.
    pub fn resolve(&self, id: &str) -> Option<&ServiceLevelObjective> {
        self.slos.get(slo_name_of(id))
    }

    /// [`Self::resolve`] for a mutating caller.
    pub fn resolve_mut(&mut self, id: &str) -> Option<&mut ServiceLevelObjective> {
        self.slos.get_mut(slo_name_of(id))
    }

    /// Remove the SLO an `Id` names, along with its tags, and hand back the removed
    /// record. `None` when no SLO is stored under that name.
    pub fn remove(&mut self, id: &str) -> Option<ServiceLevelObjective> {
        let removed = self.slos.remove(slo_name_of(id));
        if let Some(slo) = removed.as_ref() {
            self.tags.remove(&slo.arn);
        }
        removed
    }

    /// Whether another SLO would put this service over its [`MAX_SLOS`] quota.
    pub fn at_slo_quota(&self) -> bool {
        self.slos.len() >= MAX_SLOS
    }

    /// One page of the SLOs that pass `matches`, in name order, starting strictly after
    /// `cursor`, together with the cursor for the next page (`None` once the page is the
    /// last one).
    ///
    /// At most `page_size + 1` records are COLLECTED, so the memory the page holds and the
    /// response it produces are bounded by the page rather than by the number of stored
    /// records. The range scan itself still steps over any record the filter rejects, so a
    /// selector that matches few SLOs walks the stored set once.
    pub fn page<F>(
        &self,
        cursor: Option<&str>,
        page_size: usize,
        matches: F,
    ) -> (Vec<ServiceLevelObjectiveSummary>, Option<String>)
    where
        F: Fn(&ServiceLevelObjective) -> bool,
    {
        let lower = match cursor {
            Some(cursor) => Bound::Excluded(cursor),
            None => Bound::Unbounded,
        };
        let mut window: Vec<&ServiceLevelObjective> = self
            .slos
            .range::<str, _>((lower, Bound::Unbounded))
            .map(|(_, slo)| slo)
            .filter(|slo| matches(slo))
            .take(page_size.saturating_add(1))
            .collect();

        let next_cursor = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|slo| encode_next_token(&slo.name))
        } else {
            None
        };
        (window.into_iter().map(summary_of).collect(), next_cursor)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationsignalsApplicationsignalsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationsignalsApplicationsignalsState>)
}

/// The SLO name an `Id` denotes. The modelled `ServiceLevelObjectiveId` pattern is an
/// alternation of the bare `ServiceLevelObjectiveName` and the full
/// `ServiceLevelObjectiveArn`, so both forms name one record and every operation that
/// takes an `Id` must resolve them identically.
pub fn slo_name_of(id: &str) -> &str {
    match id.split_once(":slo/") {
        Some((prefix, name)) if prefix.starts_with("arn:") => name,
        _ => id,
    }
}

/// The ARN of the SLO called `name` in `region`, in the shape the modelled
/// `ServiceLevelObjectiveArn` pattern requires.
pub fn slo_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{SLO_RESOURCE_TYPE}/{name}"))
}

/// The region an ARN minted for this request names: the one in the caller's SigV4
/// credential scope, else the emulator's default. An empty region in the scope falls back
/// too, so a minted ARN never carries an empty region segment.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled `ResourceNotFoundException` for an `Id` that names no stored SLO.
pub fn slo_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: SLO_RESOURCE_TYPE.to_string(),
        resource_id: id.to_string(),
        message: format!("Service level objective {id} does not exist"),
    }
}

/// The default `Goal.Interval`: a rolling interval of 7 days.
fn default_interval() -> Interval {
    Interval::RollingInterval(RollingInterval {
        duration_unit: DEFAULT_ROLLING_INTERVAL_UNIT.to_string(),
        duration: DEFAULT_ROLLING_INTERVAL_DURATION,
    })
}

/// The `Goal` stored for a request, with each member the model documents a default for
/// filled in when the request omitted it.
pub fn goal_with_defaults(requested: Option<&Goal>) -> Goal {
    Goal {
        interval: Some(
            requested
                .and_then(|goal| goal.interval.clone())
                .unwrap_or_else(default_interval),
        ),
        attainment_goal: Some(
            requested
                .and_then(|goal| goal.attainment_goal)
                .unwrap_or(DEFAULT_ATTAINMENT_GOAL),
        ),
        warning_threshold: Some(
            requested
                .and_then(|goal| goal.warning_threshold)
                .unwrap_or(DEFAULT_WARNING_THRESHOLD),
        ),
    }
}

/// The `ServiceLevelIndicator` a period-based `SliConfig` describes.
///
/// Every member of the indicator is the corresponding member of the config, with the
/// config's own modelled defaults applied to `MetricThreshold` and `ComparisonOperator`.
/// `SliMetric.MetricDataQueries` is `@required` on the indicator but optional on the
/// config: when the request supplies no queries it stays EMPTY rather than being filled
/// with a synthesised CloudWatch query — the real service builds that query from its own
/// standard-metric catalogue (namespace, dimension set and query id), none of which this
/// emulator can compute, and inventing one would report a metric nothing selected.
pub fn sli_from_config(config: &ServiceLevelIndicatorConfig) -> ServiceLevelIndicator {
    let metric = &config.sli_metric_config;
    ServiceLevelIndicator {
        sli_metric: ServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            metric_data_queries: metric.metric_data_queries.clone().unwrap_or_default(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config
            .metric_threshold
            .unwrap_or(DEFAULT_SLI_METRIC_THRESHOLD),
        comparison_operator: config
            .comparison_operator
            .clone()
            .unwrap_or_else(|| DEFAULT_SLI_COMPARISON_OPERATOR.to_string()),
    }
}

/// The `RequestBasedServiceLevelIndicator` a `RequestBasedSliConfig` describes.
///
/// `MetricThreshold`/`ComparisonOperator` carry no default on this config (the model
/// declares them only "required if this SLO is tracking the Latency metric"), so they are
/// stored exactly as supplied. `MonitoredRequestCountMetric` is `@required` on the
/// indicator but optional on the config; as with the period-based queries it is kept at an
/// empty good-count list rather than being filled with a synthesised query.
pub fn request_based_sli_from_config(
    config: &RequestBasedServiceLevelIndicatorConfig,
) -> RequestBasedServiceLevelIndicator {
    let metric = &config.request_based_sli_metric_config;
    RequestBasedServiceLevelIndicator {
        request_based_sli_metric: RequestBasedServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            total_request_count_metric: metric
                .total_request_count_metric
                .clone()
                .unwrap_or_default(),
            monitored_request_count_metric: metric
                .monitored_request_count_metric
                .clone()
                .unwrap_or(MonitoredRequestCountMetricDataQueries::GoodCountMetric(
                    Vec::new(),
                )),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config.metric_threshold,
        comparison_operator: config.comparison_operator.clone(),
    }
}

/// The `MetricSourceType` an SLI describes.
///
/// `smithy_query(shape="MetricSourceType")` lists exactly six values — `AppMonitor`,
/// `Canary`, `CloudWatchMetric`, `Service`, `ServiceDependency`, `ServiceOperation` — and
/// the member's doc calls them "the SLI metric source type for this SLO". They name the
/// six kinds of source an SLI config can describe, so the value follows from which source
/// members the request supplied, most specific first: a `MetricSource` names a resource
/// outside Application Signals and carries its own `Type` key attribute; otherwise a
/// dependency, then an operation, then a service narrows the Application Signals entity;
/// and an SLI that names no entity at all is reading plain CloudWatch metrics.
///
/// A `MetricSource` whose `Type` names neither of the two specific resource kinds the enum
/// has values for falls back to `CloudWatchMetric` rather than to `None`: the modelled
/// `MetricSource` doc defines the member as "the metric source for SLOs on resources other
/// than Application Signals services", so such an SLO IS reading that resource's CloudWatch
/// metrics, and `CloudWatchMetric` is the enum's only value for a non-Application-Signals
/// source. Reporting `None` would drop a source kind the request did state, and reporting
/// `Service` would claim an Application Signals entity the request did not name.
fn metric_source_type(
    key_attributes: Option<&HashMap<String, String>>,
    operation_name: Option<&str>,
    dependency_config: Option<&DependencyConfig>,
    metric_source: Option<&MetricSource>,
    metric_data_queries: &[MetricDataQuery],
) -> Option<String> {
    if let Some(source) = metric_source {
        // A key-attribute `Type` is a CloudFormation-style name such as
        // `AWS::RUM::AppMonitor`; its last segment is the enum value.
        let kind = source
            .metric_source_key_attributes
            .get("Type")
            .and_then(|value| value.rsplit("::").next())
            .unwrap_or_default();
        return Some(
            match kind {
                "AppMonitor" => "AppMonitor",
                "Canary" => "Canary",
                _ => "CloudWatchMetric",
            }
            .to_string(),
        );
    }
    if dependency_config.is_some() {
        return Some("ServiceDependency".to_string());
    }
    if operation_name.is_some() {
        return Some("ServiceOperation".to_string());
    }
    if key_attributes.is_some() {
        return Some("Service".to_string());
    }
    if !metric_data_queries.is_empty() {
        return Some("CloudWatchMetric".to_string());
    }
    None
}

/// The `MetricSourceType` of a stored SLO, from whichever indicator it carries.
pub fn derive_metric_source_type(
    sli: Option<&ServiceLevelIndicator>,
    request_based_sli: Option<&RequestBasedServiceLevelIndicator>,
) -> Option<String> {
    if let Some(sli) = sli {
        let metric = &sli.sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            &metric.metric_data_queries,
        );
    }
    if let Some(request_based_sli) = request_based_sli {
        let metric = &request_based_sli.request_based_sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            &metric.total_request_count_metric,
        );
    }
    None
}

/// The entity members a stored SLO's indicator identifies: key attributes, operation name,
/// dependency, metric source and composite-SLI configuration. `ListServiceLevelObjectives`
/// both projects and filters on them, so they are read out of the record in ONE place.
pub struct SloEntity {
    pub key_attributes: Option<HashMap<String, String>>,
    pub operation_name: Option<String>,
    pub dependency_config: Option<DependencyConfig>,
    pub metric_source: Option<MetricSource>,
    pub composite_sli_config: Option<crate::ops::types::CompositeSliConfig>,
}

/// Read the entity an SLO's indicator identifies.
pub fn entity_of(slo: &ServiceLevelObjective) -> SloEntity {
    if let Some(sli) = slo.sli.as_ref() {
        let metric = &sli.sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    SloEntity {
        key_attributes: None,
        operation_name: None,
        dependency_config: None,
        metric_source: None,
        composite_sli_config: None,
    }
}

/// The `ServiceLevelObjectiveSummary` projection of a stored SLO, as
/// `ListServiceLevelObjectives` returns it.
pub fn summary_of(slo: &ServiceLevelObjective) -> ServiceLevelObjectiveSummary {
    let entity = entity_of(slo);
    ServiceLevelObjectiveSummary {
        arn: slo.arn.clone(),
        name: slo.name.clone(),
        key_attributes: entity.key_attributes,
        operation_name: entity.operation_name,
        dependency_config: entity.dependency_config,
        created_time: Some(slo.created_time),
        evaluation_type: slo.evaluation_type.clone(),
        metric_source_type: slo.metric_source_type.clone(),
        metric_source: entity.metric_source,
        composite_sli_config: entity.composite_sli_config,
    }
}

/// The message a handler reports when [`service_state`] answers `None`.
///
/// `None` means this service was mounted on an engine type it does not know — an assembly
/// mistake no request can fix, and one a correctly built router never produces. None of the
/// SLO operations models a server-fault error, so each reports it as the modelled
/// `ThrottlingException`: of the errors they do declare, it is the only one that does not
/// mislead the caller about its own request.
pub const STATE_UNAVAILABLE: &str = "Application Signals state is unavailable";

/// `NextToken` is opaque to the caller: the hex encoding of [`NEXT_TOKEN_TAG`] followed by
/// the last SLO name the previous page returned, which the next call resolves against the
/// ordered store as an exclusive cursor.
pub fn encode_next_token(last_name: &str) -> String {
    ::hex::encode(format!("{NEXT_TOKEN_TAG}{last_name}"))
}

/// The cursor a `NextToken` carries, or `None` when the token is not one this service
/// issued — which covers a token that is not hex, is not UTF-8, or is well-formed but
/// carries no [`NEXT_TOKEN_TAG`], so a forged token is refused rather than honoured as a
/// page boundary.
pub fn decode_next_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(NEXT_TOKEN_TAG)
        .map(ToString::to_string)
}

/// Whether `candidate` holds every entry of `filter` with the same value.
///
/// Attribute filters on this API name the entity to match, and an entity is identified by
/// as few as one of its up to four `Attributes` entries (`length 1..4`), so a filter
/// matches when it is a subset of the stored attributes — exact equality is the case where
/// the caller names them all.
pub fn attributes_contain(
    candidate: Option<&HashMap<String, String>>,
    filter: &HashMap<String, String>,
) -> bool {
    let Some(candidate) = candidate else {
        return filter.is_empty();
    };
    filter
        .iter()
        .all(|(key, value)| candidate.get(key) == Some(value))
}

/// Whether two `DependencyConfig`s name the same dependency. The shape carries no
/// `PartialEq`, so the comparison is written out over its two members.
pub fn same_dependency(left: Option<&DependencyConfig>, right: &DependencyConfig) -> bool {
    left.is_some_and(|left| {
        left.dependency_operation_name == right.dependency_operation_name
            && left.dependency_key_attributes == right.dependency_key_attributes
    })
}
