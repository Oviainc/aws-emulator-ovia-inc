// SPDX-License-Identifier: CC0-1.0

//! The Runtime Monitoring coverage of the account's compute resources: the ONE set of
//! facts every coverage read of this service is computed from.
//!
//! A coverage row is not a resource GuardDuty owns: the resources are the account's,
//! held by the services that own them, so each one is asked for its own — EC2 for the
//! account's instances, Amazon EKS for its clusters and for the GuardDuty security
//! agent add-on installed on each — through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]). Every fact a row carries
//! comes from that inventory or from the detector's own Runtime Monitoring
//! configuration; a resource type whose owner this emulator cannot ask contributes no
//! rows rather than invented ones.
//!
//! This module holds the rows, the criteria that narrow them and the ordering that
//! makes a page repeatable, so the operations that report coverage — the list and the
//! statistics — cannot disagree about which resources are covered or what one of them
//! is. Each of them builds its own modelled output in its own `src/ops/<op>.rs`.

#[allow(unused_imports)]
use crate::ops::types::*;

/// How many pages of ONE owner's inventory a coverage read walks.
///
/// The resources are the account's and their number is bounded by the owning
/// service's own quotas, not by anything a request carries; the walk is bounded
/// anyway so that one read does an amount of work this service decides rather than
/// an amount an account's size decides.
const MAX_INVENTORY_PAGES: usize = 20;

/// The most resources of ONE owner a coverage read carries rows for, the ceiling
/// [`MAX_INVENTORY_PAGES`] bounds the walk itself by: the rows are built, filtered and
/// ordered in memory before one answer is composed from them, so the work and the
/// memory one read takes are bounded here rather than by how many resources an account
/// has accumulated.
const MAX_COVERED_RESOURCES: usize = 500;

/// The name of the GuardDuty security agent add-on for Amazon EKS: the add-on EKS
/// add-on management installs on a cluster, and the one piece of evidence this
/// emulator holds that the agent is deployed on a resource at all.
const GUARDDUTY_AGENT_ADDON: &str = "aws-guardduty-agent";

/// The `AddonStatus` an installed add-on reports once it is running
/// (`smithy_query(shape="AddonStatus", …)` on the Amazon EKS model); any other status
/// is an add-on that is not (yet) running.
const ADDON_STATUS_ACTIVE: &str = "ACTIVE";

/// The `CoverageStatus` members (`smithy_query(shape="CoverageStatus")`).
pub const HEALTHY: &str = "HEALTHY";
/// See [`HEALTHY`].
pub const UNHEALTHY: &str = "UNHEALTHY";

/// The `ManagementType` members a coverage row uses
/// (`smithy_query(shape="ManagementType")`). "`AUTO_MANAGED` indicates that GuardDuty
/// deploys and manages updates for this resource", "`MANUAL` indicates that you are
/// responsible to deploy, update, and manage the GuardDuty security agent updates for
/// this resource", and "the `DISABLED` status doesn't apply to Amazon EC2 instances and
/// Amazon EKS clusters"
/// (`smithy_query(query="doc",shape="CoverageEc2InstanceDetails",member="ManagementType")`)
/// — which is every resource type these reads carry, so `DISABLED` is never one of
/// them.
const AUTO_MANAGED: &str = "AUTO_MANAGED";
/// See [`AUTO_MANAGED`].
const MANUAL: &str = "MANUAL";

/// The `ResourceType` members a coverage row carries
/// (`smithy_query(shape="ResourceType")`).
pub const RESOURCE_TYPE_EC2: &str = "EC2";
/// See [`RESOURCE_TYPE_EC2`].
pub const RESOURCE_TYPE_EKS: &str = "EKS";

/// The `FeatureAdditionalConfiguration` members that say who deploys the agent on one
/// resource type (`smithy_query(shape="FeatureAdditionalConfiguration")`).
const EC2_AGENT_MANAGEMENT: &str = "EC2_AGENT_MANAGEMENT";
/// See [`EC2_AGENT_MANAGEMENT`].
const EKS_ADDON_MANAGEMENT: &str = "EKS_ADDON_MANAGEMENT";

/// The `CoverageFilterCriterionKey` members (`smithy_query(shape="CoverageFilterCriterionKey")`),
/// each naming the coverage attribute a criterion compares.
const KEY_ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`KEY_ACCOUNT_ID`].
const KEY_RESOURCE_TYPE: &str = "RESOURCE_TYPE";
/// See [`KEY_ACCOUNT_ID`].
const KEY_COVERAGE_STATUS: &str = "COVERAGE_STATUS";
/// See [`KEY_ACCOUNT_ID`].
const KEY_ADDON_VERSION: &str = "ADDON_VERSION";
/// See [`KEY_ACCOUNT_ID`].
const KEY_CLUSTER_NAME: &str = "CLUSTER_NAME";
/// See [`KEY_ACCOUNT_ID`].
const KEY_ECS_CLUSTER_NAME: &str = "ECS_CLUSTER_NAME";
/// See [`KEY_ACCOUNT_ID`].
const KEY_EKS_CLUSTER_NAME: &str = "EKS_CLUSTER_NAME";
/// See [`KEY_ACCOUNT_ID`].
const KEY_MANAGEMENT_TYPE: &str = "MANAGEMENT_TYPE";
/// See [`KEY_ACCOUNT_ID`].
const KEY_AGENT_VERSION: &str = "AGENT_VERSION";
/// See [`KEY_ACCOUNT_ID`].
const KEY_INSTANCE_ID: &str = "INSTANCE_ID";
/// See [`KEY_ACCOUNT_ID`].
const KEY_CLUSTER_ARN: &str = "CLUSTER_ARN";
/// The `CoverageSortKey` member no criterion key carries
/// (`smithy_query(shape="CoverageSortKey")`): a row is ordered by the reason it is
/// unhealthy, not filtered on it.
const KEY_ISSUE: &str = "ISSUE";
/// See [`KEY_ISSUE`]: the sort key that orders by an instant rather than by text.
const KEY_UPDATED_AT: &str = "UPDATED_AT";

/// The `OrderBy` member that reverses the order (`smithy_query(shape="OrderBy")`).
const ORDER_DESCENDING: &str = "DESC";

/// The input members a refusal names, spelled as the wire spells them
/// (`smithy_query(operation="ListCoverage")`).
const CRITERION_KEY_MEMBER: &str = "filterCriteria.filterCriterion.criterionKey";
/// See [`CRITERION_KEY_MEMBER`].
const SORT_ATTRIBUTE_MEMBER: &str = "sortCriteria.attributeName";
/// See [`CRITERION_KEY_MEMBER`].
const SORT_ORDER_MEMBER: &str = "sortCriteria.orderBy";

// Named, not taken by position out of the generated value lists: these assertions fail
// the BUILD if the model stops carrying a value this file switches on, where an index
// would instead have silently matched some other member.
const _: () = {
    let statuses = [HEALTHY, UNHEALTHY];
    let mut index = 0;
    while index < statuses.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::COVERAGE_STATUS_VALUES,
            statuses[index]
        ));
        index += 1;
    }
    let management = [AUTO_MANAGED, MANUAL];
    let mut index = 0;
    while index < management.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::MANAGEMENT_TYPE_VALUES,
            management[index]
        ));
        index += 1;
    }
    let resource_types = [RESOURCE_TYPE_EC2, RESOURCE_TYPE_EKS];
    let mut index = 0;
    while index < resource_types.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::RESOURCE_TYPE_VALUES,
            resource_types[index]
        ));
        index += 1;
    }
    let configurations = [EC2_AGENT_MANAGEMENT, EKS_ADDON_MANAGEMENT];
    let mut index = 0;
    while index < configurations.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::FEATURE_ADDITIONAL_CONFIGURATION_VALUES,
            configurations[index]
        ));
        index += 1;
    }
    let criterion_keys = [
        KEY_ACCOUNT_ID,
        KEY_RESOURCE_TYPE,
        KEY_COVERAGE_STATUS,
        KEY_ADDON_VERSION,
        KEY_CLUSTER_NAME,
        KEY_ECS_CLUSTER_NAME,
        KEY_EKS_CLUSTER_NAME,
        KEY_MANAGEMENT_TYPE,
        KEY_AGENT_VERSION,
        KEY_INSTANCE_ID,
        KEY_CLUSTER_ARN,
    ];
    let mut index = 0;
    while index < criterion_keys.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::COVERAGE_FILTER_CRITERION_KEY_VALUES,
            criterion_keys[index]
        ));
        index += 1;
    }
    let sort_keys = [KEY_ISSUE, KEY_UPDATED_AT];
    let mut index = 0;
    while index < sort_keys.len() {
        assert!(super::enum_carries(
            crate::ops::types::shared_types::COVERAGE_SORT_KEY_VALUES,
            sort_keys[index]
        ));
        index += 1;
    }
    assert!(super::enum_carries(
        crate::ops::types::shared_types::ORDER_BY_VALUES,
        ORDER_DESCENDING
    ));
};

/// Why a coverage request was refused, and which member carried the fault.
///
/// One type for every operation that takes a `CoverageFilterCriteria` or a
/// `CoverageSortCriteria`, so all of them answer an unevaluable criterion with the same
/// words under the same member name. A criterion key, sort key or order this service's
/// model does not carry is REFUSED rather than ignored: silently ignoring one would
/// answer with resources the caller asked to exclude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The member carries a value the model does not.
    InvalidValue(&'static str),
    /// The request left out a member the criterion cannot be evaluated without.
    Missing(&'static str),
}

impl Rejection {
    /// The refusal message, naming the request member at fault.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidValue(member) => super::invalid_parameter_message(member),
            Self::Missing(member) => super::missing_parameter_message(member),
        }
    }
}

/// What the detector's Runtime Monitoring configuration says about coverage: which
/// resource types are covered at all, and who deploys the agent on each.
///
/// "Make sure the accounts have Runtime Monitoring enabled and GuardDuty agent running
/// on their resources" (`smithy_query(query="doc",shape="ListCoverage")`), so a
/// detector whose Runtime Monitoring is disabled covers nothing and reports no rows.
/// `RUNTIME_MONITORING` covers every compute resource type, while the older
/// `EKS_RUNTIME_MONITORING` covers only the Amazon EKS ones — "`RUNTIME_MONITORING`
/// already includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="CreateDetector")`).
#[derive(Debug)]
pub struct CoverageScope {
    covers_ec2: bool,
    covers_eks: bool,
    /// The `ManagementType` an EC2 row reports.
    ec2_management: &'static str,
    /// The `ManagementType` an EKS row reports.
    eks_management: &'static str,
}

impl CoverageScope {
    /// The coverage one detector's configuration provides.
    pub fn of(detector: &super::Detector) -> Self {
        let runtime_monitoring =
            detector.feature_status(super::FEATURE_RUNTIME_MONITORING) == super::ENABLED;
        let eks_runtime_monitoring =
            detector.feature_status(super::FEATURE_EKS_RUNTIME_MONITORING) == super::ENABLED;
        // Whichever feature provides the coverage is the one whose additional
        // configuration says who deploys the agent.
        let eks_feature = if runtime_monitoring {
            super::FEATURE_RUNTIME_MONITORING
        } else {
            super::FEATURE_EKS_RUNTIME_MONITORING
        };
        Self {
            covers_ec2: runtime_monitoring,
            covers_eks: runtime_monitoring || eks_runtime_monitoring,
            ec2_management: management_type(configuration_enabled(
                detector,
                super::FEATURE_RUNTIME_MONITORING,
                EC2_AGENT_MANAGEMENT,
            )),
            eks_management: management_type(configuration_enabled(
                detector,
                eks_feature,
                EKS_ADDON_MANAGEMENT,
            )),
        }
    }
}

/// Is one feature's additional configuration enabled on this detector?
fn configuration_enabled(detector: &super::Detector, feature: &str, configuration: &str) -> bool {
    detector
        .features
        .iter()
        .find(|record| record.name == feature)
        .and_then(|record| {
            record
                .additional_configuration
                .iter()
                .find(|config| config.name == configuration)
        })
        .is_some_and(|config| config.status == super::ENABLED)
}

/// Who deploys and updates the agent, from whether GuardDuty's own agent management is
/// enabled for that resource type. See [`AUTO_MANAGED`] for the two members' meaning.
fn management_type(auto_managed: bool) -> &'static str {
    if auto_managed {
        AUTO_MANAGED
    } else {
        MANUAL
    }
}

/// One coverage row, as this service computes it before a request's criteria narrow it:
/// the resource, the account and detector it belongs to, what its agent coverage is, and
/// the instant the facts behind that coverage last changed.
#[derive(Debug)]
pub struct CoverageRow {
    resource_id: String,
    resource_type: &'static str,
    account_id: String,
    detector_id: String,
    coverage_status: &'static str,
    /// "The reason why a coverage status was `UNHEALTHY`"
    /// (`smithy_query(query="doc",shape="CoverageResource",member="Issue")`), absent on
    /// a covered resource.
    issue: Option<String>,
    /// "The timestamp at which the coverage details for the resource were last updated"
    /// (`smithy_query(query="doc",shape="CoverageResource",member="UpdatedAt")`): the
    /// instant the resource's own coverage facts last changed, in epoch milliseconds,
    /// and absent when the owning service reports none.
    updated_at_millis: Option<i64>,
    management_type: &'static str,
    /// `CoverageEc2InstanceDetails.InstanceId`, on an EC2 row.
    instance_id: Option<String>,
    /// `CoverageEc2InstanceDetails.InstanceType`, on an EC2 row.
    instance_type: Option<String>,
    /// `CoverageEksClusterDetails.ClusterName`, on an EKS row.
    cluster_name: Option<String>,
    /// `AddonDetails.AddonVersion` of the agent add-on, when the cluster carries it.
    addon_version: Option<String>,
    /// `AddonDetails.AddonStatus` of the agent add-on, when the cluster carries it.
    addon_status: Option<String>,
}

impl CoverageRow {
    /// This row as the modelled `CoverageResource`.
    ///
    /// Each resource type fills the `ResourceDetails` member of its own type and leaves
    /// the others absent (`smithy_query(shape="CoverageResourceDetails")`). A member
    /// this emulator holds no fact for is left ABSENT rather than filled with a
    /// stand-in: `CoverageEksClusterDetails.CoveredNodes` / `CompatibleNodes` count the
    /// cluster's nodes, which no inventory here enumerates, and
    /// `CoverageEc2InstanceDetails.ClusterArn` names the ECS cluster an instance belongs
    /// to, which the service that owns the ECS clusters does not answer for yet
    /// (`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_clusters.rs`
    /// is still a scaffold).
    pub fn reported(&self) -> CoverageResource {
        let details = match self.resource_type {
            RESOURCE_TYPE_EC2 => CoverageResourceDetails {
                resource_type: Some(RESOURCE_TYPE_EC2.to_string()),
                ec2_instance_details: Some(CoverageEc2InstanceDetails {
                    instance_id: self.instance_id.clone(),
                    instance_type: self.instance_type.clone(),
                    cluster_arn: None,
                    // No deployment of the agent ON an instance is recorded anywhere in
                    // this emulator, so there is no version to report.
                    agent_details: None,
                    management_type: Some(self.management_type.to_string()),
                }),
                eks_cluster_details: None,
                ecs_cluster_details: None,
            },
            _ => CoverageResourceDetails {
                resource_type: Some(RESOURCE_TYPE_EKS.to_string()),
                eks_cluster_details: Some(CoverageEksClusterDetails {
                    cluster_name: self.cluster_name.clone(),
                    covered_nodes: None,
                    compatible_nodes: None,
                    addon_details: self.addon_version.as_ref().map(|version| AddonDetails {
                        addon_version: Some(version.clone()),
                        addon_status: self.addon_status.clone(),
                    }),
                    management_type: Some(self.management_type.to_string()),
                }),
                ec2_instance_details: None,
                ecs_cluster_details: None,
            },
        };
        CoverageResource {
            resource_id: Some(self.resource_id.clone()),
            detector_id: Some(self.detector_id.clone()),
            account_id: Some(self.account_id.clone()),
            resource_details: Some(details),
            coverage_status: Some(self.coverage_status.to_string()),
            issue: self.issue.clone(),
            updated_at: self
                .updated_at_millis
                .map(::aws_smithy_types::DateTime::from_millis),
        }
    }

    /// The `CoverageStatus` this row reports.
    pub fn coverage_status(&self) -> &'static str {
        self.coverage_status
    }

    /// The `ResourceType` this row reports.
    pub fn resource_type(&self) -> &'static str {
        self.resource_type
    }

    /// The coverage attribute one `CoverageFilterCriterionKey` or `CoverageSortKey`
    /// names, or `None` when this row carries no such attribute — which is what a row
    /// of another resource type carries for a key that names one type's detail.
    fn attribute(&self, key: &str) -> Option<String> {
        match key {
            KEY_ACCOUNT_ID => Some(self.account_id.clone()),
            KEY_RESOURCE_TYPE => Some(self.resource_type.to_string()),
            KEY_COVERAGE_STATUS => Some(self.coverage_status.to_string()),
            KEY_MANAGEMENT_TYPE => Some(self.management_type.to_string()),
            KEY_ADDON_VERSION => self.addon_version.clone(),
            // `CLUSTER_NAME` is the cluster of whichever kind the row carries, and the
            // type-specific keys only of their own kind.
            KEY_CLUSTER_NAME => self.cluster_name.clone(),
            KEY_EKS_CLUSTER_NAME => (self.resource_type == RESOURCE_TYPE_EKS)
                .then(|| self.cluster_name.clone())
                .flatten(),
            KEY_INSTANCE_ID => self.instance_id.clone(),
            KEY_ISSUE => self.issue.clone(),
            // No ECS cluster inventory is reachable, so no row is in an ECS cluster:
            // `ECS_CLUSTER_NAME` and `CLUSTER_ARN` resolve on no row, which is exactly
            // how a key naming a detail the row does not carry behaves.
            KEY_ECS_CLUSTER_NAME | KEY_CLUSTER_ARN => None,
            // No agent deployment on an EC2 instance is recorded, so no row reports an
            // agent version.
            KEY_AGENT_VERSION => None,
            _ => None,
        }
    }

    /// The key this row is ordered by under one `CoverageSortKey`.
    ///
    /// `UPDATED_AT` orders by the INSTANT, every other key by the attribute's text; a
    /// row the key resolves on no attribute for orders as the neutral key rather than
    /// being dropped from the answer.
    fn ordering_key(&self, key: &str) -> (i64, String) {
        if key == KEY_UPDATED_AT {
            (self.updated_at_millis.unwrap_or_default(), String::new())
        } else {
            (0, self.attribute(key).unwrap_or_default())
        }
    }
}

/// One `CoverageFilterCriterion`, resolved to the attribute it compares and the values
/// it admits and excludes.
#[derive(Debug)]
pub struct Criterion {
    key: String,
    /// "Represents an equal condition that is applied to a single field while
    /// retrieving the coverage details"
    /// (`smithy_query(query="doc",shape="CoverageFilterCondition",member="Equals")`).
    equals: Vec<String>,
    /// The `NotEquals` counterpart of [`Self::equals`].
    not_equals: Vec<String>,
}

impl Criterion {
    /// Does `row` satisfy this criterion?
    ///
    /// `Equals` admits a row whose attribute is one of the values it lists, so a row
    /// that carries no such attribute cannot satisfy it; `NotEquals` excludes a row
    /// whose attribute is one of its values, so a row carrying no such attribute is not
    /// excluded. A condition that lists no value at all restricts nothing.
    fn matches(&self, row: &CoverageRow) -> bool {
        let value = row.attribute(&self.key);
        let admitted = self.equals.is_empty()
            || value
                .as_deref()
                .is_some_and(|value| self.equals.iter().any(|wanted| wanted == value));
        let not_excluded = value
            .as_deref()
            .is_none_or(|value| !self.not_equals.iter().any(|unwanted| unwanted == value));
        admitted && not_excluded
    }
}

/// The criteria a request asks the coverage facts to be narrowed by.
///
/// A `criterionKey` the model does not carry is REFUSED rather than ignored: ignoring
/// it would report resources the caller asked to exclude.
pub fn requested_criteria(
    criteria: Option<&CoverageFilterCriteria>,
) -> Result<Vec<Criterion>, Rejection> {
    let mut requested = Vec::new();
    for criterion in criteria
        .into_iter()
        .flat_map(|criteria| criteria.filter_criterion.iter().flatten())
    {
        let key = criterion
            .criterion_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .ok_or(Rejection::Missing(CRITERION_KEY_MEMBER))?;
        if !super::enum_carries(
            crate::ops::types::shared_types::COVERAGE_FILTER_CRITERION_KEY_VALUES,
            key,
        ) {
            return Err(Rejection::InvalidValue(CRITERION_KEY_MEMBER));
        }
        let condition = criterion.filter_condition.as_ref();
        requested.push(Criterion {
            key: key.to_string(),
            equals: condition
                .and_then(|condition| condition.equals.clone())
                .unwrap_or_default(),
            not_equals: condition
                .and_then(|condition| condition.not_equals.clone())
                .unwrap_or_default(),
        });
    }
    Ok(requested)
}

/// Drop the rows no criterion admits: every criterion must hold.
pub fn retain_matching(rows: &mut Vec<CoverageRow>, criteria: &[Criterion]) {
    rows.retain(|row| criteria.iter().all(|criterion| criterion.matches(row)));
}

/// The attribute a request asks the coverage details to be ordered by, and whether the
/// order is reversed.
///
/// A request naming no attribute leaves the rows in this service's own total order (see
/// [`sort_rows`]); an `attributeName` or `orderBy` the model does not carry is refused,
/// for the same reason an unknown criterion key is.
pub fn requested_sort(
    criteria: Option<&CoverageSortCriteria>,
) -> Result<(Option<String>, bool), Rejection> {
    let attribute = criteria
        .and_then(|criteria| criteria.attribute_name.as_deref())
        .filter(|attribute| !attribute.is_empty());
    if let Some(attribute) = attribute {
        if !super::enum_carries(
            crate::ops::types::shared_types::COVERAGE_SORT_KEY_VALUES,
            attribute,
        ) {
            return Err(Rejection::InvalidValue(SORT_ATTRIBUTE_MEMBER));
        }
    }
    let order = criteria
        .and_then(|criteria| criteria.order_by.as_deref())
        .filter(|order| !order.is_empty());
    if let Some(order) = order {
        if !super::enum_carries(crate::ops::types::shared_types::ORDER_BY_VALUES, order) {
            return Err(Rejection::InvalidValue(SORT_ORDER_MEMBER));
        }
    }
    Ok((
        attribute.map(str::to_string),
        order == Some(ORDER_DESCENDING),
    ))
}

/// Order the rows: by the requested attribute when the request named one, and then —
/// always — by resource type and resource id, so the order is TOTAL.
///
/// A total order is what makes a pagination token mean the same thing on the call that
/// issues it and the call that hands it back: two rows that tie on the requested
/// attribute would otherwise be free to swap between pages.
pub fn sort_rows(rows: &mut [CoverageRow], key: Option<&str>, descending: bool) {
    rows.sort_by(|left, right| {
        let primary = match key {
            Some(key) => left.ordering_key(key).cmp(&right.ordering_key(key)),
            None => ::std::cmp::Ordering::Equal,
        };
        let primary = if descending { primary.reverse() } else { primary };
        primary.then_with(|| {
            (left.resource_type, left.resource_id.as_str())
                .cmp(&(right.resource_type, right.resource_id.as_str()))
        })
    });
}

/// Every resource of the account the detector's configuration covers, with its coverage
/// facts: the inventory both coverage reads are computed from.
///
/// A resource type the scope does not cover is not asked for at all, and an owner that
/// does not answer contributes no rows (see [`ec2_coverage`] and [`eks_coverage`]).
pub async fn covered_resources(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    detector_id: &str,
    account_id: &str,
    scope: &CoverageScope,
) -> Vec<CoverageRow> {
    let mut rows: Vec<CoverageRow> = Vec::new();
    if scope.covers_ec2 {
        rows.extend(ec2_coverage(ctx, region, detector_id, account_id, scope).await);
    }
    if scope.covers_eks {
        rows.extend(eks_coverage(ctx, region, detector_id, account_id, scope).await);
    }
    rows
}

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and this
/// service names no other service's crate. `region` is the request's own signing
/// region, so the resources asked for are the ones of the Region the detector serves.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:expr, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url($endpoint)
            // The transport is in-process: there is nothing to retry, time out or
            // watch for a stalled stream, and each of those would need a sleep
            // implementation this client is not given. The same goes for caching the
            // identity: the credentials are handed over directly.
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

/// One instant of an SDK timestamp in epoch milliseconds, the form this service holds
/// every instant in.
fn millis_of(instant: &::aws_smithy_types::DateTime) -> i64 {
    instant
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(instant.subsec_nanos() / 1_000_000))
}

/// The coverage of the account's EC2 instances, asked of the service that owns them.
///
/// An instance is covered by the GuardDuty security agent running ON it, and nothing in
/// this emulator deploys software onto an instance or receives a report from one — so
/// no instance is reported `HEALTHY`: each row says, as its issue, that the agent is
/// not installed. What the detector's configuration does decide is the `ManagementType`
/// the row reports, which is who WOULD deploy that agent.
///
/// An instance EC2 has already terminated is no longer a resource of the account
/// ("recently terminated instances might appear in the returned results" —
/// `smithy_query(query="doc",shape="DescribeInstances")` on the EC2 model), so it is
/// left out rather than reported as uncovered.
///
/// An answer that is not EC2 reporting instances — no EC2 to ask, or a failure — yields
/// NO rows and a warning, the way this emulator reports nothing it did not observe.
async fn ec2_coverage(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    detector_id: &str,
    account_id: &str,
    scope: &CoverageScope,
) -> Vec<CoverageRow> {
    let client = callback_client!(
        aws_sdk_ec2,
        ctx,
        format!("https://ec2.{region}.amazonaws.com"),
        region
    );
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_INVENTORY_PAGES {
        if rows.len() >= MAX_COVERED_RESOURCES {
            break;
        }
        let answer = client
            .describe_instances()
            .set_next_token(token.clone())
            .send()
            .await;
        let output = match answer {
            Ok(output) => output,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the account's EC2 instances could not be resolved for the coverage report"
                );
                break;
            }
        };
        for instance in output
            .reservations()
            .iter()
            .flat_map(::aws_sdk_ec2::types::Reservation::instances)
            .take(MAX_COVERED_RESOURCES.saturating_sub(rows.len()))
        {
            let Some(instance_id) = instance.instance_id() else {
                continue;
            };
            let state = instance
                .state()
                .and_then(::aws_sdk_ec2::types::InstanceState::name)
                .map(::aws_sdk_ec2::types::InstanceStateName::as_str)
                .unwrap_or_default();
            if state == TERMINATED_INSTANCE_STATE || state == SHUTTING_DOWN_INSTANCE_STATE {
                continue;
            }
            rows.push(CoverageRow {
                resource_id: instance_id.to_string(),
                resource_type: RESOURCE_TYPE_EC2,
                account_id: account_id.to_string(),
                detector_id: detector_id.to_string(),
                coverage_status: UNHEALTHY,
                issue: Some(format!(
                    "The GuardDuty security agent is not installed on {instance_id}."
                )),
                // The instance's own instant: the facts this row reports about it last
                // changed when EC2 launched it.
                updated_at_millis: instance.launch_time().map(millis_of),
                management_type: scope.ec2_management,
                instance_id: Some(instance_id.to_string()),
                instance_type: instance
                    .instance_type()
                    .map(|instance_type| instance_type.as_str().to_string()),
                cluster_name: None,
                addon_version: None,
                addon_status: None,
            });
        }
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    rows
}

/// The `InstanceStateName` members that name an instance the account no longer holds
/// (`smithy_query(shape="InstanceStateName")` on the EC2 model).
const TERMINATED_INSTANCE_STATE: &str = "terminated";
/// See [`TERMINATED_INSTANCE_STATE`].
const SHUTTING_DOWN_INSTANCE_STATE: &str = "shutting-down";

/// The coverage of the account's Amazon EKS clusters, asked of the service that owns
/// them.
///
/// A cluster is covered when the GuardDuty security agent add-on
/// ([`GUARDDUTY_AGENT_ADDON`]) is installed on it and running: that add-on IS the agent
/// deployment for an EKS cluster, and it is a resource Amazon EKS holds, so the
/// coverage status is read off it — `HEALTHY` while the add-on is
/// [`ADDON_STATUS_ACTIVE`], `UNHEALTHY` with the reason otherwise. The version the row
/// reports is the add-on's own.
///
/// As in [`ec2_coverage`], an answer that is not Amazon EKS reporting its clusters
/// yields no rows.
async fn eks_coverage(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    detector_id: &str,
    account_id: &str,
    scope: &CoverageScope,
) -> Vec<CoverageRow> {
    let client = callback_client!(
        aws_sdk_eks,
        ctx,
        format!("https://eks.{region}.amazonaws.com"),
        region
    );
    let mut cluster_names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_INVENTORY_PAGES {
        if cluster_names.len() >= MAX_COVERED_RESOURCES {
            break;
        }
        let answer = client
            .list_clusters()
            .set_next_token(token.clone())
            .send()
            .await;
        let output = match answer {
            Ok(output) => output,
            Err(error) => {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the account's Amazon EKS clusters could not be resolved for the coverage report"
                );
                break;
            }
        };
        cluster_names.extend(
            output
                .clusters()
                .iter()
                .take(MAX_COVERED_RESOURCES.saturating_sub(cluster_names.len()))
                .cloned(),
        );
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }

    let mut rows = Vec::new();
    for cluster_name in cluster_names {
        let addon = agent_addon(&client, &cluster_name).await;
        let addon_status = match &addon {
            AgentAddon::Installed { status, .. } => status.clone(),
            AgentAddon::Absent | AgentAddon::Unknown => None,
        };
        // Three ANSWERS, not two: an add-on Amazon EKS reports, an add-on it reports the
        // cluster does not carry, and a question it did not answer. Only the second is
        // evidence the agent is not installed, so only it says so.
        let (coverage_status, issue) = match (&addon, addon_status.as_deref()) {
            (AgentAddon::Installed { .. }, Some(ADDON_STATUS_ACTIVE)) => (HEALTHY, None),
            (AgentAddon::Installed { .. }, Some(status)) => (
                UNHEALTHY,
                Some(format!(
                    "The GuardDuty security agent add-on {GUARDDUTY_AGENT_ADDON} is {status} on cluster {cluster_name}."
                )),
            ),
            (AgentAddon::Absent, _) => (
                UNHEALTHY,
                Some(format!(
                    "The GuardDuty security agent add-on {GUARDDUTY_AGENT_ADDON} is not installed on cluster {cluster_name}."
                )),
            ),
            (AgentAddon::Installed { .. } | AgentAddon::Unknown, _) => (
                UNHEALTHY,
                Some(format!(
                    "The state of the GuardDuty security agent add-on {GUARDDUTY_AGENT_ADDON} on cluster {cluster_name} could not be determined."
                )),
            ),
        };
        // The instant the coverage facts last changed: the add-on's, when the cluster
        // carries one, and otherwise the cluster's own creation instant — there is
        // nothing else about its coverage that has changed since.
        let updated_at_millis = match &addon {
            AgentAddon::Installed {
                modified_at_millis: Some(modified_at),
                ..
            } => Some(*modified_at),
            _ => cluster_created_millis(&client, &cluster_name).await,
        };
        let addon_version = match &addon {
            AgentAddon::Installed { version, .. } => version.clone(),
            AgentAddon::Absent | AgentAddon::Unknown => None,
        };
        rows.push(CoverageRow {
            resource_id: cluster_name.clone(),
            resource_type: RESOURCE_TYPE_EKS,
            account_id: account_id.to_string(),
            detector_id: detector_id.to_string(),
            coverage_status,
            issue,
            updated_at_millis,
            management_type: scope.eks_management,
            instance_id: None,
            instance_type: None,
            cluster_name: Some(cluster_name),
            addon_version,
            addon_status,
        });
    }
    rows
}

/// What Amazon EKS answered about the GuardDuty security agent add-on on one cluster.
///
/// The three answers are kept apart because they are three different facts, and only
/// one of them is evidence about the agent: an add-on reported with its version and
/// status, a cluster Amazon EKS says carries no such add-on, and a question it did not
/// answer at all.
#[derive(Debug)]
enum AgentAddon {
    Installed {
        version: Option<String>,
        status: Option<String>,
        modified_at_millis: Option<i64>,
    },
    Absent,
    Unknown,
}

/// Ask Amazon EKS for the agent add-on of one cluster.
///
/// `ResourceNotFoundException` is Amazon EKS reporting that the cluster carries no such
/// add-on, which is [`AgentAddon::Absent`]; any other failure is this emulator not
/// getting an answer ([`AgentAddon::Unknown`]) and is worth a warning, because a
/// coverage report that cannot ask tells a caller less than it looks like it does.
async fn agent_addon(client: &::aws_sdk_eks::Client, cluster_name: &str) -> AgentAddon {
    let answer = client
        .describe_addon()
        .cluster_name(cluster_name)
        .addon_name(GUARDDUTY_AGENT_ADDON)
        .send()
        .await;
    match answer {
        Ok(output) => match output.addon() {
            Some(addon) => AgentAddon::Installed {
                version: addon.addon_version().map(str::to_string),
                status: addon.status().map(|status| status.as_str().to_string()),
                modified_at_millis: addon.modified_at().map(millis_of),
            },
            // A 200 carrying no add-on says nothing about the deployment either way.
            None => AgentAddon::Unknown,
        },
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_eks::operation::describe_addon::DescribeAddonError::is_resource_not_found_exception,
            ) =>
        {
            AgentAddon::Absent
        }
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the GuardDuty security agent add-on of an Amazon EKS cluster could not be resolved for the coverage report"
            );
            AgentAddon::Unknown
        }
    }
}

/// The instant Amazon EKS created one cluster, or `None` when it reports none or could
/// not be asked — in which case the row leaves `UpdatedAt` absent rather than inventing
/// an instant.
async fn cluster_created_millis(
    client: &::aws_sdk_eks::Client,
    cluster_name: &str,
) -> Option<i64> {
    match client.describe_cluster().name(cluster_name).send().await {
        Ok(output) => output
            .cluster()
            .and_then(::aws_sdk_eks::types::Cluster::created_at)
            .map(millis_of),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Amazon EKS cluster could not be resolved for the coverage report"
            );
            None
        }
    }
}
