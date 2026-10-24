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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    ConflictException, DependencyConfig, Goal, GroupingAttributeDefinition, GroupingConfiguration,
    Interval, MetricDataQuery, MetricSource, MonitoredRequestCountMetricDataQueries,
    RequestBasedServiceLevelIndicator, RequestBasedServiceLevelIndicatorConfig,
    RequestBasedServiceLevelIndicatorMetric, ResourceNotFoundException, RollingInterval,
    ServiceLevelIndicator, ServiceLevelIndicatorConfig, ServiceLevelIndicatorMetric,
    ServiceLevelObjective, ServiceLevelObjectiveSummary,
};
// The shapes the time-ranged topology reads project, on their own `use` so this set and the
// SLO lifecycle's above are edited on separate lines.
use crate::ops::types::{
    MetricReference, Service, ServiceDependency, ServiceDependent, ServiceOperation, ServiceSummary,
};

/// This service's SigV4 signing name, which is also its ARN namespace
/// (`OPERATIONS.md`: `sigv4 signing name: application-signals`, `ARN namespace:
/// application-signals`). The modelled `ServiceLevelObjectiveArn` pattern
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`) carries the same
/// namespace.
pub const SIGNING_NAME: &str = "application-signals";

/// The resource-type segment of an SLO ARN, from the modelled `ServiceLevelObjectiveArn`
/// pattern (`:slo/<name>`), which spells that segment in lower case.
pub const SLO_ARN_RESOURCE: &str = "slo";

/// The `ResourceType` an SLO failure reports, and the type token its message names: the
/// resource's TYPE NAME, which is capitalised — not the lower-case segment an ARN spells
/// (`SLO_ARN_RESOURCE`). The two are different strings for one resource kind, and a client
/// comparing `ResourceType` is comparing against the type name.
pub const SLO_RESOURCE_TYPE: &str = "Slo";

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

/// The largest page `ListAuditFindings` returns when the caller names no `MaxResults`.
/// Model doc for the member: "The maximum number of audit findings to return in one
/// operation. If you omit this parameter, the default of 10 is used."
pub const DEFAULT_AUDIT_FINDING_PAGE: i32 = 10;

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

/// The [`NEXT_TOKEN_TAG`] of each time-ranged topology read. One tag per operation, so a
/// token one of them issued is not read as a cursor by another: the offsets they count are
/// into different projections, and honouring a foreign one would silently skip entries.
pub const SERVICE_OPERATIONS_TOKEN_TAG: &str = "service-operations/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependencies`.
pub const SERVICE_DEPENDENCIES_TOKEN_TAG: &str = "service-dependencies/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependents`.
pub const SERVICE_DEPENDENTS_TOKEN_TAG: &str = "service-dependents/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListAuditFindings`.
pub const AUDIT_FINDINGS_TOKEN_TAG: &str = "audit-findings/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServices`.
pub const SERVICES_TOKEN_TAG: &str = "services/1/";

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
    /// This account's grouping configuration, or `None` while none is configured.
    ///
    /// At most ONE configuration exists: `PutGroupingConfiguration` "creates or updates the
    /// grouping configuration for this account", so a second put REPLACES the first rather
    /// than adding a record, and `DeleteGroupingConfiguration` "deletes the grouping
    /// configuration for this account", which returns the store to `None`. The single slot
    /// is also why this is an `Option` rather than a map: like `slos`, this store is one
    /// account's and one Region's.
    pub grouping_configuration: Option<GroupingConfiguration>,
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

    /// Replace this account's grouping configuration with `definitions`, stamping
    /// `updated_at` as the moment it was last written, and hand back the record that was
    /// stored. The stored record — never the request — is what the writer answers with and
    /// what a later read reports, so the two can never disagree.
    pub fn put_grouping_configuration(
        &mut self,
        definitions: Vec<GroupingAttributeDefinition>,
        updated_at: ::aws_smithy_types::DateTime,
    ) -> GroupingConfiguration {
        let configuration = GroupingConfiguration {
            grouping_attribute_definitions: definitions,
            updated_at,
        };
        self.grouping_configuration = Some(configuration.clone());
        configuration
    }

    /// Remove this account's grouping configuration, answering whether one was configured.
    ///
    /// Idempotent: clearing an account that has no configuration leaves the store `None`
    /// and is not an error, which is what the operation's `@idempotent` trait and its empty
    /// modelled error set (no not-found error) require.
    pub fn clear_grouping_configuration(&mut self) -> bool {
        self.grouping_configuration.take().is_some()
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
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{SLO_ARN_RESOURCE}/{name}"))
}

/// The ARN of the SLO an `Id` denotes, for either form the modelled
/// `ServiceLevelObjectiveId` pattern admits: an `Id` already in ARN form IS that ARN, and a
/// bare name is rendered into the ARN it would carry in `region` (this account's, since the
/// store is one account's — see [`slo_name_of`]).
///
/// This is how an error identifies an SLO that is not stored: there is no record to read an
/// ARN off, so the ARN is derived from the identity the request named.
pub fn slo_arn_of(region: &str, id: &str) -> String {
    match id.split_once(&format!(":{SLO_ARN_RESOURCE}/")) {
        Some((prefix, _)) if prefix.starts_with("arn:") => id.to_string(),
        _ => slo_arn(region, id),
    }
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
///
/// The failure identifies the SLO by its ARN — the full resource identifier, which is what
/// `ResourceId`'s doc ("Can't find the resource id") names and what a client parses to
/// resolve the resource — and reports the capitalised type name in `ResourceType`. Both are
/// `@required` members a client branches on, so a bare name there would hand it an
/// un-namespaced id and a type token that no equality test against the type name matches.
/// The message states the same two facts, so a caller reading only the prose sees the ARN
/// too.
pub fn slo_not_found(region: &str, id: &str) -> ResourceNotFoundException {
    let arn = slo_arn_of(region, id);
    ResourceNotFoundException {
        message: format!("{SLO_RESOURCE_TYPE} with arn '{arn}' does not exist."),
        resource_type: SLO_RESOURCE_TYPE.to_string(),
        resource_id: arn,
    }
}

/// The modelled `ConflictException` for a create that names an SLO already stored.
///
/// `ConflictException` carries only `Message`, so the message is the only place the
/// conflicting resource can be identified — and it identifies it the same way the not-found
/// failure does, by the ARN of the resource that already exists (read off the stored
/// record, since there IS one), not by the bare name the request happened to send.
pub fn slo_already_exists(arn: &str) -> ConflictException {
    ConflictException {
        message: format!("{SLO_RESOURCE_TYPE} with arn '{arn}' already exists."),
    }
}

/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated.
///
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
///
/// The clause names the member path and the requirement and NOTHING of the value that was
/// rejected: `ValidationException` carries only `message`, so this text is the whole
/// machine-readable detail a client gets, and the rejected value is caller input that would
/// otherwise be copied into every client log recording the failure.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// [`constraint_clause`] for a member that was ABSENT, whose requirement is that it be
/// present.
///
/// Worded through the same builder as every other clause, with NO rendering of the absent
/// value: the recorded refusal of a call that omits a `@required` `@httpQuery` member reads
/// `1 validation error detected: Value at 'endTime' failed to satisfy constraint: Member
/// must not be null` — the clause states the member and the requirement, and says nothing
/// where a value would go. Reporting the absence as a `null` VALUE would describe a value
/// the request never carried.
pub fn absent_member_clause(member_path: &str) -> String {
    constraint_clause(member_path, "Member must not be null")
}

/// The `ValidationException` message over one or more clauses: the count of violated
/// constraints, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with: the requirement
/// stated is the expression a value must satisfy, not the anchoring the model uses to make
/// the match total. A `$` escaped by a backslash is a literal dollar sign inside the
/// expression, not the end anchor, so it stays.
///
/// Only the expression's OUTER anchors are stripped: a `@pattern` that anchors each branch
/// of an alternation separately (`^a$|^b$`, as `ServiceLevelObjectiveId` does) keeps the
/// inner ones, because they are part of the expression's own structure rather than a wrapper
/// around it — dropping them would change which strings the reported expression describes.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
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

/// The `MetricType` the indicator of a stored SLO reports, from the config that built it.
///
/// Both indicator shapes document the member identically
/// (`smithy_query(shape="ServiceLevelIndicatorMetric")`, member `MetricType`): "If the SLO
/// monitors either the `LATENCY` or `AVAILABILITY` metric that Application Signals
/// collects, this field displays which of those metrics is used." The member therefore
/// reports the standard metric Application Signals SELECTED for this SLO, not whatever the
/// request happened to carry, so it is reported only when the config actually made that
/// selection:
///
/// * the config must name the Application Signals entity whose standard metrics are
///   collected — its `KeyAttributes`, which the member's config-side doc requires for an
///   SLO "related to a metric collected by Application Signals"; and
/// * the config must not specify a metric of its OWN, since then that metric, not a
///   standard one, is what the SLO monitors: CloudWatch metric or metric-math queries
///   (`MetricDataQueries`, or a request-based config's request-count queries), a
///   `MetricName` which the model defines as "the name of the CloudWatch metric to use for
///   the SLO, when using a custom metric rather than Application Signals standard metrics",
///   or a `MetricSource`, "the metric source for SLOs on resources other than Application
///   Signals services".
///
/// Otherwise the member stays ABSENT: reporting it would tell a client the SLO tracks an
/// Application Signals standard metric when the SLO's metric is the one the request spelled
/// out, and would send that classification back on a Get-into-Update round trip.
fn reported_metric_type(
    requested: Option<&String>,
    key_attributes: Option<&HashMap<String, String>>,
    metric_name: Option<&str>,
    metric_source: Option<&MetricSource>,
    config_names_its_own_metric: bool,
) -> Option<String> {
    let selects_standard_metric = key_attributes.is_some()
        && metric_name.is_none()
        && metric_source.is_none()
        && !config_names_its_own_metric;
    requested.filter(|_| selects_standard_metric).cloned()
}

/// The `ServiceLevelIndicator` a period-based `SliConfig` describes.
///
/// Every member of the indicator is the corresponding member of the config, with the
/// config's own modelled defaults applied to `MetricThreshold` and `ComparisonOperator`,
/// except `MetricType`, which is reported only when the config selected an Application
/// Signals standard metric (see [`reported_metric_type`]).
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
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .metric_data_queries
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty()),
            ),
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
/// stored exactly as supplied, and `MetricType` follows the same standard-metric rule as
/// the period-based indicator (see [`reported_metric_type`]).
/// `MonitoredRequestCountMetric` is `@required` on the
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
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .total_request_count_metric
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty())
                    || metric.monitored_request_count_metric.is_some(),
            ),
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
    encode_cursor(NEXT_TOKEN_TAG, last_name)
}

/// The cursor a `NextToken` carries, or `None` when the token is not one this service
/// issued — which covers a token that is not hex, is not UTF-8, or is well-formed but
/// carries no [`NEXT_TOKEN_TAG`], so a forged token is refused rather than honoured as a
/// page boundary.
pub fn decode_next_token(token: &str) -> Option<String> {
    decode_cursor(NEXT_TOKEN_TAG, token)
}

/// A `NextToken` carrying `cursor` under `tag`: hex, so the caller reads it as the opaque
/// value the model documents rather than as a store key it could edit.
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}

/// The cursor `token` carries under `tag`, or `None` when it is not a token this service
/// issued under that tag.
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(tag).map(ToString::to_string)
}

/// One page of a derived projection, plus the token for the page after it.
///
/// The projections the time-ranged topology reads page over are DERIVED (they are
/// recomputed from the stored SLOs on every call) rather than stored, so a cursor into them
/// is an offset into their deterministic order, not a key of a stored record. Each read
/// passes its own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, so a token is only ever
/// resolved against the projection that issued it; `Err` carries the refusal message for a
/// token that is not one this service issued under that tag.
pub fn page_of<T>(
    items: Vec<T>,
    tag: &str,
    token: Option<&str>,
    page_size: usize,
) -> Result<(Vec<T>, Option<String>), String> {
    // A page of nothing could not advance its own cursor, so a caller following `NextToken`
    // would loop for ever. Every caller's `MaxResults` is modelled with a minimum of 1, so
    // this only ever clamps a bound no request can state.
    let page_size = page_size.max(1);
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .ok_or_else(|| format!("NextToken `{token}` is not a token this service issued"))?,
    };
    let mut page: Vec<T> = items.into_iter().skip(offset).collect();
    let next_token = if page.len() > page_size {
        page.truncate(page_size);
        Some(encode_cursor(tag, &offset.saturating_add(page_size).to_string()))
    } else {
        None
    };
    Ok((page, next_token))
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

// =============================================================================
// The time range every time-ranged read takes
// =============================================================================

/// The refusal a `StartTime`/`EndTime` pair earns, or `None` when the range is one this
/// service serves.
///
/// Every operation that takes the pair documents it as a period to report over
/// (`smithy_query(query="operation", operation="GetService")`: both members `@required`),
/// and a period runs forwards: the real service refuses an inverted one with
/// `ValidationException` and states the rule with both offending instants in it. The
/// instants are rendered the way the service states them — the `Date.toString` form, in
/// UTC — so the message names the times the request carried rather than a re-encoding the
/// caller never sent.
///
/// The comparison is STRICT, because that is the rule the message states: a window whose
/// start equals its end spans no time at all, so it is refused with the same wording as an
/// inverted one rather than reporting over an empty period.
pub fn time_range_refusal(start_time: &DateTime, end_time: &DateTime) -> Option<String> {
    if start_time.as_secs_f64() < end_time.as_secs_f64() {
        return None;
    }
    Some(format!(
        "Invalid start time {} and end time {} provided: start time must be before end time",
        stated_instant(start_time),
        stated_instant(end_time),
    ))
}

/// One instant as this service states it in a refusal: `EEE MMM dd HH:mm:ss UTC yyyy`
/// (`Sun Sep 06 02:00:00 UTC 2026`), the `java.util.Date` rendering of a UTC instant.
///
/// The calendar conversion is the LIBRARY's: `DateTime::fmt(Format::DateTime)` renders the
/// instant as RFC-3339 (`2026-09-06T02:00:00Z`) and this only reorders those already-correct
/// fields and names the month, so no leap-year or era arithmetic is re-derived here. The
/// weekday is not part of that rendering, and it needs no calendar either: day zero of the
/// epoch — 1970-01-01 — was a Thursday, so it is the day count modulo 7.
///
/// An instant the library cannot render at all is stated as its epoch-second value, so the
/// message still names the instant the request carried.
fn stated_instant(at: &DateTime) -> String {
    /// Indexed by the epoch day count modulo 7, which is 0 on Thursday 1970-01-01.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    /// The month abbreviations `Date.toString` uses, in month order.
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let Ok(rendered) = at.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
        return at.as_secs_f64().to_string();
    };
    // `<date>T<time>Z`, where the time may carry a fractional part the `Date` form omits.
    let Some((date, time)) = rendered.split_once('T') else {
        return rendered;
    };
    let mut fields = date.split('-');
    let (Some(year), Some(month), Some(day)) = (fields.next(), fields.next(), fields.next())
    else {
        return rendered;
    };
    let hour_minute_second = time.trim_end_matches('Z');
    let hour_minute_second = hour_minute_second
        .split_once('.')
        .map_or(hour_minute_second, |(whole, _)| whole);
    let Some(month) = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTHS.get(month.wrapping_sub(1)))
    else {
        return rendered;
    };
    let weekday = WEEKDAYS[at.secs().div_euclid(86_400).rem_euclid(7) as usize];
    format!("{weekday} {month} {day} {hour_minute_second} UTC {year}")
}

// =============================================================================
// The service topology the stored SLOs are evidence of
// =============================================================================

/// Whether a stored SLO is evidence of the entity it monitors over the requested window.
///
/// An SLO monitors its entity from the moment it is created until it is deleted, so the
/// period it is evidence for is `[CreatedTime, now]`, and the two periods overlap exactly
/// when the window does not END before the SLO existed and does not BEGIN after now. So a
/// read over a past window reports an entity the account has only just started monitoring
/// as absent, and a window entirely in the future observes nothing at all.
fn monitored_in_window(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
) -> bool {
    slo.created_time.as_secs_f64() <= window_end.as_secs_f64() && window_has_begun(window_start)
}

/// Whether a window has begun: nothing has been observed in one that starts after now,
/// however far ahead it ends.
fn window_has_begun(window_start: &DateTime) -> bool {
    window_start.as_secs_f64() <= DateTime::from(::std::time::SystemTime::now()).as_secs_f64()
}

/// A dedup/order key for one `Attributes` map: its entries in key order. Used only to
/// order and deduplicate derived topology entries, never put on the wire.
fn attribute_key(attributes: &HashMap<String, String>) -> String {
    attributes
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(key, value)| format!("{key}\u{1f}{value}"))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

/// The `MetricReference`s this emulator can report for a discovered entity: NONE.
///
/// A `MetricReference` identifies a CloudWatch metric by three `@required` members —
/// `Namespace`, `MetricName` and `MetricType` — and Application Signals fills them from its
/// own standard-metric catalogue: which metric of which namespace it collects for an
/// entity, under which metric type and dimension set. This emulator cannot compute that
/// catalogue, and no stored record states it either: an SLO's
/// `ServiceLevelIndicatorMetric.MetricType` reports a standard metric only for an SLO that
/// names NO metric of its own ([`reported_metric_type`]), which is precisely the SLO that
/// names no namespace or metric name to report beside it. So the list stays EMPTY rather
/// than reporting a metric nothing selected — the same choice [`sli_from_config`] makes for
/// `SliMetric.MetricDataQueries`.
///
/// Single-homed here so the four topology reads cannot differ on it, and so this reasoning
/// is stated once.
fn no_metric_references() -> Vec<MetricReference> {
    Vec::new()
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Every stored SLO that monitors the entity `key_attributes` identifies, over a window
    /// ending at `window_end`.
    ///
    /// The requested attributes identify the entity, and an entity is identified by as few
    /// as one of its up to four `Attributes` entries, so an SLO matches when its own entity
    /// attributes hold every entry the request named ([`attributes_contain`] — the same
    /// match `ListServiceLevelObjectives` already applies to the same member).
    fn monitoring<'a>(
        &'a self,
        key_attributes: &'a HashMap<String, String>,
        window_start: &'a DateTime,
        window_end: &'a DateTime,
    ) -> impl Iterator<Item = &'a ServiceLevelObjective> + 'a {
        self.slos.values().filter(move |slo| {
            monitored_in_window(slo, window_start, window_end)
                && attributes_contain(entity_of(slo).key_attributes.as_ref(), key_attributes)
        })
    }

    /// The entity `key_attributes` identifies, as `GetService` reports it over the window.
    ///
    /// The reported key attributes are the fullest set this emulator holds for the entity —
    /// the request's own when it holds none, since the attributes ARE the entity's identity
    /// and the caller named it. So an entity no stored SLO monitors is reported by its
    /// identity alone rather than refused: Application Signals identifies an entity by its
    /// key attributes rather than by a stored resource id, and this read reports what it
    /// observed for that identity.
    ///
    /// `MetricReferences` is empty for the reason stated on [`no_metric_references`].
    /// `AttributeMaps`, `ServiceGroups` and `LogGroupReferences` stay absent: they are
    /// platform, grouping and log-group facts the real service learns from the telemetry an
    /// instrumented application sends it, which this emulator never receives.
    pub fn observed_service(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Service {
        let mut reported = key_attributes.clone();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            if let Some(observed) = entity_of(slo).key_attributes {
                if observed.len() > reported.len() {
                    reported = observed;
                }
            }
        }
        Service {
            key_attributes: reported,
            attribute_maps: None,
            service_groups: None,
            metric_references: no_metric_references(),
            log_group_references: None,
        }
    }

    /// Every service entity this emulator has observed over the window, as `ListServices`
    /// reports it: one summary per distinct discovered service, in a deterministic order.
    ///
    /// The stored SLOs are this emulator's only evidence of a discovered service — it
    /// receives no instrumentation telemetry — so the discovered set is the set of entities
    /// the stored SLOs monitor, each over the window it is evidence for
    /// ([`monitored_in_window`], the same window rule the other topology reads apply). A
    /// dependency an SLO names is NOT in that set: the edge is evidence that the monitored
    /// entity calls it, not that Application Signals discovered and instrumented the callee,
    /// and `ListServiceDependencies` is where the edge is reported.
    ///
    /// An identity that is a STRICT SUBSET of another observed identity is not reported
    /// separately: as few as one of an entity's up to four `Attributes` entries identifies
    /// it ([`attributes_contain`]), so the subset names the same entity as the fuller set,
    /// and `GetService` answers for it with exactly that fuller set
    /// ([`Self::observed_service`]). Reporting both would list one service twice and hand
    /// the caller a summary whose `KeyAttributes` no `GetService` echoes back.
    ///
    /// `MetricReferences` is empty for the reason stated on [`no_metric_references`], and
    /// `AttributeMaps`/`ServiceGroups` stay absent, exactly as [`Self::observed_service`]
    /// leaves them: they are platform, grouping and telemetry facts the real service learns
    /// from an instrumented application, which this emulator never receives.
    pub fn observed_services(
        &self,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceSummary> {
        let mut by_identity: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_start, window_end))
        {
            let Some(attributes) = entity_of(slo).key_attributes else {
                continue;
            };
            // `ServiceSummary.KeyAttributes` is `@required` with `length 1..4`: an SLO whose
            // indicator names no entity at all is evidence of no service.
            if attributes.is_empty() {
                continue;
            }
            by_identity.insert(attribute_key(&attributes), attributes);
        }

        let identities: Vec<HashMap<String, String>> = by_identity.into_values().collect();
        identities
            .iter()
            .filter(|candidate| {
                !identities.iter().any(|other| {
                    other.len() > candidate.len() && attributes_contain(Some(other), candidate)
                })
            })
            .map(|key_attributes| ServiceSummary {
                key_attributes: key_attributes.clone(),
                attribute_maps: None,
                service_groups: None,
                metric_references: no_metric_references(),
            })
            .collect()
    }

    /// The operations of the entity `key_attributes` identifies, in name order: one entry
    /// per distinct operation the stored SLOs name on it.
    pub fn observed_operations(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceOperation> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            if let Some(name) = entity_of(slo).operation_name {
                names.insert(name);
            }
        }
        names
            .into_iter()
            .map(|name| ServiceOperation {
                name,
                metric_references: no_metric_references(),
            })
            .collect()
    }

    /// The dependencies of the entity `key_attributes` identifies: one entry per distinct
    /// edge the stored SLOs record from one of its operations to an operation of another
    /// entity.
    ///
    /// An SLO that names a dependency but no operation of its own records no edge: the
    /// modelled `ServiceDependency` states the edge as "the operation in this service that
    /// calls the dependency" (`OperationName`, `@required`), so without it there is no edge
    /// to report.
    pub fn observed_dependencies(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceDependency> {
        let mut by_edge: BTreeMap<String, ServiceDependency> = BTreeMap::new();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            let entity = entity_of(slo);
            let (Some(operation_name), Some(dependency)) =
                (entity.operation_name, entity.dependency_config)
            else {
                continue;
            };
            by_edge
                .entry(format!(
                    "{operation_name}\u{1d}{}\u{1d}{}",
                    attribute_key(&dependency.dependency_key_attributes),
                    dependency.dependency_operation_name,
                ))
                .or_insert_with(|| ServiceDependency {
                    operation_name,
                    dependency_key_attributes: dependency.dependency_key_attributes,
                    dependency_operation_name: dependency.dependency_operation_name,
                    metric_references: no_metric_references(),
                });
        }
        by_edge.into_values().collect()
    }

    /// The dependents of the entity `key_attributes` identifies: the inverse of
    /// [`Self::observed_dependencies`], one entry per distinct edge the stored SLOs record
    /// from another entity INTO one of this entity's operations.
    ///
    /// `OperationName` is the operation of the named entity that was invoked (the
    /// dependency operation the edge points at), and `DependentOperationName` the operation
    /// of the dependent that invoked it — which an SLO that names no operation of its own
    /// leaves absent, as the model allows.
    pub fn observed_dependents(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ServiceDependent> {
        let mut by_edge: BTreeMap<String, ServiceDependent> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_start, window_end))
        {
            let entity = entity_of(slo);
            let Some(dependency) = entity.dependency_config else {
                continue;
            };
            if !attributes_contain(Some(&dependency.dependency_key_attributes), key_attributes) {
                continue;
            }
            let Some(dependent_key_attributes) = entity.key_attributes else {
                continue;
            };
            by_edge
                .entry(format!(
                    "{}\u{1d}{}\u{1d}{}",
                    attribute_key(&dependent_key_attributes),
                    entity.operation_name.clone().unwrap_or_default(),
                    dependency.dependency_operation_name,
                ))
                .or_insert_with(|| ServiceDependent {
                    operation_name: Some(dependency.dependency_operation_name),
                    dependent_key_attributes,
                    dependent_operation_name: entity.operation_name,
                    metric_references: no_metric_references(),
                });
        }
        by_edge.into_values().collect()
    }
}
