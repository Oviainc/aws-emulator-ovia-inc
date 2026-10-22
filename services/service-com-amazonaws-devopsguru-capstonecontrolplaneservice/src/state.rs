// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devopsguru-capstonecontrolplaneservice service.
//!
//! Everything a DevOps Guru client can itself create lives here: the analysis
//! coverage it declares (`UpdateResourceCollection`), the notification channels it
//! adds (`AddNotificationChannel`), the service-integration configuration it sets
//! (`UpdateServiceIntegration`), the event-source integration it toggles
//! (`UpdateEventSourcesConfig`) and the cost estimation it starts
//! (`StartCostEstimation`). Insight feedback a client submits (`PutFeedback`) lives
//! here too, in [`ComAmazonawsDevopsguruCapstonecontrolplaneserviceState::insight_feedback`].
//!
//! Insights are produced by DevOps Guru's own analysis, never by a client call, so
//! [`ComAmazonawsDevopsguruCapstonecontrolplaneserviceState::insights`] is the map
//! that analysis would write and the insight READS (`ListInsights` and
//! `DescribeInsight`) report: this emulator runs no analysis, so the map stays empty
//! for an account whose insights nothing produced, and a read over it reports
//! nothing rather than inventing an insight. An `InsightId` the map does not hold
//! names no insight, which is what `DescribeInsight` refuses with
//! `ResourceNotFoundException`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CloudFormationCollection, InsightTimeRange, PredictionTimeRange, ProactiveInsight,
    ProactiveInsightSummary, ReactiveInsight, ReactiveInsightSummary, ResourceCollection,
    ServiceCollection, TagCollection,
};

/// Per-service state for com-amazonaws-devopsguru-capstonecontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDevopsguruCapstonecontrolplaneserviceState {
    /// The resource coverage `UpdateResourceCollection` maintains and
    /// `GetResourceCollection` reports.
    pub resource_collection: ResourceCoverage,
    /// Notification channels keyed by the id `AddNotificationChannel` minted.
    pub notification_channels: BTreeMap<String, NotificationChannelRecord>,
    /// The last `UpdateServiceIntegration` configuration, which
    /// `DescribeServiceIntegration` reports.
    pub service_integration: ServiceIntegrationRecord,
    /// The account's cost estimation, as `StartCostEstimation` created it and
    /// `GetCostEstimation` reports it. `None` until the first start.
    pub cost_estimation: Option<CostEstimationRecord>,
    /// The last `UpdateEventSourcesConfig` configuration, which
    /// `DescribeEventSourcesConfig` reports.
    pub event_sources: EventSourcesRecord,
    /// Insight feedback the client submitted through `PutFeedback`, keyed by the
    /// `InsightFeedback.Id` it named, and reported by `DescribeFeedback`.
    ///
    /// One entry per insight: `PutFeedback` is a PUT, and `DescribeFeedback`
    /// reports "the most recent feedback submitted", so resubmitting feedback for
    /// the same insight replaces what was stored for it rather than adding a
    /// second opinion about it. Independent of [`Self::insights`]: DevOps Guru
    /// accepts feedback about an insight this account has never seen.
    pub insight_feedback: BTreeMap<String, InsightFeedbackRecord>,
    /// How many `PutFeedback` submissions this store has accepted, which stamps
    /// each record's [`InsightFeedbackRecord::submission`] so the one submitted
    /// most recently is identifiable without a wall clock.
    pub feedback_submissions: u64,
    /// The insights DevOps Guru's analysis produced for this account, keyed by
    /// `InsightId`.
    ///
    /// No modelled operation MINTS an insight — DevOps Guru derives one itself from
    /// anomalies in the covered resources — so this map is what an insight generator
    /// writes and what the insight reads walk. A `BTreeMap` because the reads page over
    /// it: the key order is stable, which is what a page cursor needs.
    pub insights: BTreeMap<String, InsightRecord>,
}

/// One insight's feedback, as `PutFeedback` stored it.
#[derive(Debug, Clone, Default)]
pub struct InsightFeedbackRecord {
    /// `InsightFeedback.Id` — the insight the feedback is about.
    pub insight_id: String,
    /// `InsightFeedback.Feedback` (`InsightFeedbackOption`).
    pub feedback: String,
    /// The 1-based ordinal of the `PutFeedback` call that wrote this record.
    /// `DescribeFeedback` with no `InsightId` reports the highest one, which is
    /// the most recent feedback submitted in this account and Region.
    pub submission: u64,
}

/// One insight, as the insight reads report it.
///
/// Holds every member `ProactiveInsightSummary` and `ReactiveInsightSummary` carry,
/// plus the two the DETAIL shapes `ProactiveInsight` and `ReactiveInsight` add;
/// [`InsightRecord::insight_type`] is what decides which of the two a read renders.
#[derive(Debug, Clone, Default)]
pub struct InsightRecord {
    /// `InsightId` — the map key, repeated here so a page of records is self-contained.
    pub id: String,
    /// `InsightName`.
    pub name: String,
    /// `InsightType`: `PROACTIVE` or `REACTIVE` — see `INSIGHT_TYPE_VALUES`.
    pub insight_type: String,
    /// `InsightSeverity` — see `INSIGHT_SEVERITY_VALUES`.
    pub severity: String,
    /// `InsightStatus`: `ONGOING` while the behaviour continues, `CLOSED` once it
    /// ended — see `INSIGHT_STATUS_VALUES`.
    pub status: String,
    /// `InsightTimeRange.StartTime`, in milliseconds since the epoch. Required by the
    /// modelled shape, so every record carries one.
    pub start_time_millis: i64,
    /// `InsightTimeRange.EndTime`, in milliseconds since the epoch: the instant the
    /// behaviour ended, which only a `CLOSED` insight has.
    pub end_time_millis: Option<i64>,
    /// `PredictionTimeRange.StartTime`, in milliseconds since the epoch. Only a
    /// proactive insight predicts a window, so this is absent on a reactive one.
    pub prediction_start_time_millis: Option<i64>,
    /// `PredictionTimeRange.EndTime`, in milliseconds since the epoch.
    pub prediction_end_time_millis: Option<i64>,
    /// `ResourceCollection` — the resources this insight is about. The same
    /// stack-name / tag-collection shape as the account's coverage, so it reuses
    /// [`ResourceCoverage`]; absent when the insight names no resources.
    pub resource_collection: Option<ResourceCoverage>,
    /// `ServiceCollection.ServiceNames` (`SERVICE_NAME_VALUES`), empty when the
    /// insight names no service.
    pub service_names: Vec<String>,
    /// `AssociatedResourceArns`, empty when the insight names no resource ARN.
    pub associated_resource_arns: Vec<String>,
    /// `SsmOpsItemId`, present only when OpsCenter opened an OpsItem for this
    /// insight. Carried by the DETAIL shapes (`DescribeInsight`) only — the summary
    /// shapes model no such member.
    pub ssm_ops_item_id: Option<String>,
    /// `Description` — what the insight says. Carried by the DETAIL shapes only.
    pub description: Option<String>,
}

impl InsightRecord {
    /// `InsightType`'s `PROACTIVE` member, as the model spells it.
    pub const PROACTIVE: &'static str = "PROACTIVE";

    /// Whether this record is reported as a PROACTIVE insight — the response member
    /// a detail read renders it on, and the summary shape a listing renders it as.
    pub fn is_proactive(&self) -> bool {
        self.insight_type == Self::PROACTIVE
    }

    /// This record as the modelled `ProactiveInsight`, the DETAIL shape
    /// `DescribeInsight` reports. Built from the SAME record as
    /// [`InsightRecord::to_proactive_summary`], so a listed insight and the one a
    /// caller then describes cannot disagree.
    pub fn to_proactive_insight(&self) -> ProactiveInsight {
        ProactiveInsight {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            prediction_time_range: self.prediction_time_range(),
            resource_collection: self.wire_resource_collection(),
            ssm_ops_item_id: self.ssm_ops_item_id.clone(),
            description: self.description.clone(),
        }
    }

    /// This record as the modelled `ReactiveInsight`, which carries no
    /// `PredictionTimeRange`: only a proactive insight predicts a window.
    pub fn to_reactive_insight(&self) -> ReactiveInsight {
        ReactiveInsight {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            resource_collection: self.wire_resource_collection(),
            ssm_ops_item_id: self.ssm_ops_item_id.clone(),
            description: self.description.clone(),
        }
    }

    /// This record as the modelled `ProactiveInsightSummary`.
    pub fn to_proactive_summary(&self) -> ProactiveInsightSummary {
        ProactiveInsightSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            prediction_time_range: self.prediction_time_range(),
            resource_collection: self.wire_resource_collection(),
            service_collection: self.wire_service_collection(),
            associated_resource_arns: self.wire_associated_resource_arns(),
        }
    }

    /// This record as the modelled `ReactiveInsightSummary`, which carries no
    /// `PredictionTimeRange`: only a proactive insight predicts a window.
    pub fn to_reactive_summary(&self) -> ReactiveInsightSummary {
        ReactiveInsightSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            resource_collection: self.wire_resource_collection(),
            service_collection: self.wire_service_collection(),
            associated_resource_arns: self.wire_associated_resource_arns(),
        }
    }

    fn insight_time_range(&self) -> InsightTimeRange {
        InsightTimeRange {
            start_time: timestamp(self.start_time_millis),
            end_time: self.end_time_millis.map(timestamp),
        }
    }

    fn prediction_time_range(&self) -> Option<PredictionTimeRange> {
        self.prediction_start_time_millis
            .map(|start| PredictionTimeRange {
                start_time: timestamp(start),
                end_time: self.prediction_end_time_millis.map(timestamp),
            })
    }

    fn wire_resource_collection(&self) -> Option<ResourceCollection> {
        self.resource_collection
            .as_ref()
            .map(|coverage| ResourceCollection {
                cloud_formation: (!coverage.cloudformation_stack_names.is_empty()).then(|| {
                    CloudFormationCollection {
                        stack_names: Some(
                            coverage.cloudformation_stack_names.iter().cloned().collect(),
                        ),
                    }
                }),
                tags: (!coverage.tag_collections.is_empty()).then(|| {
                    coverage
                        .tag_collections
                        .iter()
                        .map(|(app_boundary_key, tag_values)| TagCollection {
                            app_boundary_key: app_boundary_key.clone(),
                            tag_values: tag_values.iter().cloned().collect(),
                        })
                        .collect()
                }),
            })
    }

    fn wire_service_collection(&self) -> Option<ServiceCollection> {
        (!self.service_names.is_empty()).then(|| ServiceCollection {
            service_names: Some(self.service_names.clone()),
        })
    }

    fn wire_associated_resource_arns(&self) -> Option<Vec<String>> {
        (!self.associated_resource_arns.is_empty())
            .then(|| self.associated_resource_arns.clone())
    }
}

/// The account an insight read resolves against: the `AccountId` the request
/// carries, or this emulator's own account when it carries none.
pub fn requested_account_id(account_id: Option<&str>) -> String {
    match account_id {
        Some(account_id) => account_id.to_string(),
        None => ::engine::emulator_account_id().to_string(),
    }
}

/// Whether the caller may read `account_id`'s insights.
///
/// DevOps Guru serves another account's insights only through an organization
/// relationship — the `AccountId` member is "The ID of the member account in the
/// organization". A generated emulator serves ONE account
/// ([`::engine::emulator_account_id`]) and is in no organization, so no other
/// account is accessible to it and a request naming one is an authorization
/// refusal (`AccessDeniedException`), not a lookup that happens to miss.
pub fn account_is_accessible(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// A stored instant as the modelled `Timestamp`.
pub fn timestamp(millis_since_epoch: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis_since_epoch)
}

/// A submitted `Timestamp` as the milliseconds this store keeps, or `None` when the
/// instant is too far from the epoch to be one.
pub fn millis_of(at: &::aws_smithy_types::DateTime) -> Option<i64> {
    at.to_millis().ok()
}

/// The set of resources DevOps Guru analyses for this account and region.
///
/// Both halves are SETS, not lists: `UpdateResourceCollection` is an
/// add/remove of coverage, so adding the same stack name or tag value twice
/// leaves the coverage unchanged and `GetResourceCollection` reports each one
/// once, in a stable order.
#[derive(Debug, Clone, Default)]
pub struct ResourceCoverage {
    /// `UpdateCloudFormationCollectionFilter.StackNames`, reported as
    /// `ResourceCollectionFilter.CloudFormation.StackNames`.
    pub cloudformation_stack_names: BTreeSet<String>,
    /// `UpdateTagCollectionFilter.{AppBoundaryKey,TagValues}`, reported as
    /// `ResourceCollectionFilter.Tags`: one entry per app-boundary key, each
    /// holding that key's tag values. The account's analysis boundary is a single
    /// tag key, so this holds at most [`APP_BOUNDARY_KEYS_PER_ACCOUNT`] entry.
    pub tag_collections: BTreeMap<String, BTreeSet<String>>,
}

/// How many app-boundary keys one account's coverage holds: ONE.
///
/// DevOps Guru's own invariant, not an emulator guard (so it does not live with the
/// store bounds below): `TagCollection`'s `AppBoundaryKey` documentation says "All
/// Amazon Web Services resources in your account and Region tagged with this key make up
/// your DevOps Guru application and analysis boundary", so an account has a single
/// analysis boundary and therefore a single such key. `UpdateResourceCollection` adds
/// tag VALUES under the configured key, and refuses an `ADD` that would leave the
/// account covering a second, different key.
pub const APP_BOUNDARY_KEYS_PER_ACCOUNT: usize = 1;

impl ResourceCoverage {
    /// How many resources this coverage puts under DevOps Guru analysis.
    ///
    /// The analysed population is the coverage itself: there is no real CloudFormation
    /// stack or tagged resource for this emulator to expand a filter against, so each
    /// coverage entry IS one monitored resource — one per covered stack name, and one
    /// per `(app-boundary key, tag value)` pair. Both halves are sets, so a repeated
    /// `ADD` does not raise the count and a `REMOVE` lowers it.
    ///
    /// `DescribeAccountHealth` reports this as `AnalyzedResourceCount`, and bills one
    /// resource-analysis hour per entry as `ResourceHours`.
    pub fn monitored_resource_count(&self) -> usize {
        self.cloudformation_stack_names.len()
            + self
                .tag_collections
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
    }
}

/// One notification channel, as `AddNotificationChannel` stored it.
#[derive(Debug, Clone, Default)]
pub struct NotificationChannelRecord {
    /// The id this service minted for the channel (`NotificationChannelId`).
    pub id: String,
    /// `Config.Sns.TopicArn` — the identity of the channel: a topic may back at
    /// most one channel.
    pub sns_topic_arn: String,
    /// `Config.Filters`, kept absent when the request carried none so a read
    /// reports the config that was submitted rather than a filled-in default.
    pub filters: Option<NotificationFiltersRecord>,
}

/// `Config.Filters` as submitted: `NotificationFilterConfig`'s two optional lists.
#[derive(Debug, Clone, Default)]
pub struct NotificationFiltersRecord {
    /// `Filters.Severities` (`InsightSeverity` values).
    pub severities: Option<Vec<String>>,
    /// `Filters.MessageTypes` (`NotificationMessageType` values).
    pub message_types: Option<Vec<String>>,
}

/// The persisted `ServiceIntegrationConfig`.
#[derive(Debug, Clone)]
pub struct ServiceIntegrationRecord {
    /// `ServiceIntegration.OpsCenter.OptInStatus` (`OptInStatus`).
    pub ops_center_opt_in_status: String,
    /// `ServiceIntegration.LogsAnomalyDetection.OptInStatus` (`OptInStatus`).
    pub logs_anomaly_detection_opt_in_status: String,
    /// `ServiceIntegration.KMSServerSideEncryption`.
    pub kms_server_side_encryption: KmsServerSideEncryptionRecord,
}

/// The persisted `EventSourcesConfig`.
///
/// One record per account and region, of fixed shape: `EventSourcesConfig` models a
/// single integration (`AmazonCodeGuruProfiler`), so this store grows by nothing when
/// it is written.
#[derive(Debug, Clone)]
pub struct EventSourcesRecord {
    /// `EventSources.AmazonCodeGuruProfiler.Status` (`EventSourceOptInStatus`).
    pub code_guru_profiler_status: String,
}

/// Before any `UpdateEventSourcesConfig`, DevOps Guru consumes nothing from CodeGuru
/// Profiler: the integration is opted out until a client enables it.
impl Default for EventSourcesRecord {
    fn default() -> Self {
        Self {
            code_guru_profiler_status: OPT_IN_STATUS_DISABLED.to_string(),
        }
    }
}

/// The persisted `KMSServerSideEncryptionIntegration`.
#[derive(Debug, Clone)]
pub struct KmsServerSideEncryptionRecord {
    /// `OptInStatus` (`OptInStatus`).
    pub opt_in_status: String,
    /// `KMSKeyId`, present only for a customer-managed key.
    pub kms_key_id: Option<String>,
    /// `Type` (`ServerSideEncryptionType`).
    pub encryption_type: Option<String>,
}

/// One cost estimation, as `StartCostEstimation` created it.
///
/// There is at most ONE per account and Region: `StartCostEstimation` returns no id
/// and `GetCostEstimation` takes none, so neither operation can name a second
/// estimation. A new start therefore replaces the previous estimate rather than
/// adding to the store, which is also why this record is not a growth site.
#[derive(Debug, Clone)]
pub struct CostEstimationRecord {
    /// `ClientToken` as submitted (`@idempotencyToken`), so a retry of the same
    /// request is recognised as the same estimation rather than starting another.
    pub client_token: Option<String>,
    /// `ResourceCollection.CloudFormation.StackNames` as submitted, left `None` when
    /// the request carried no CloudFormation filter, so a read reports the collection
    /// that was submitted rather than a filled-in default.
    pub stack_names: Option<Vec<String>>,
    /// `ResourceCollection.Tags` as submitted: one `(AppBoundaryKey, TagValues)` entry
    /// per filter, in request order.
    pub tags: Option<Vec<(String, Vec<String>)>>,
    /// When the estimation was started (`TimeRange.StartTime`).
    pub started_at: SystemTime,
    /// When the analysis is due to be finished: the estimation reports `ONGOING`
    /// before this instant and `COMPLETED` from it on, with `TimeRange.EndTime` at
    /// it. Stored rather than waited for, so no read ever blocks.
    pub completes_at: SystemTime,
    /// The per-resource-type costs computed when the estimation started.
    pub costs: Vec<ResourceCostRecord>,
}

/// One `ServiceResourceCost`: the monthly cost of analysing the resources of one
/// type that the estimation's collection covers.
#[derive(Debug, Clone)]
pub struct ResourceCostRecord {
    /// `ServiceResourceCost.Type`.
    pub resource_type: String,
    /// `ServiceResourceCost.State` (`CostEstimationServiceResourceState`).
    pub state: String,
    /// `ServiceResourceCost.Count` — how many resources of this type are analysed.
    pub count: i32,
    /// `ServiceResourceCost.UnitCost` — the price per resource hour.
    pub unit_cost: f64,
    /// `ServiceResourceCost.Cost` — the monthly cost for those resources.
    pub cost: f64,
}

impl CostEstimationRecord {
    /// The `CostEstimationStatus` to report at `now`: the estimation is `ONGOING`
    /// until its completion instant and `COMPLETED` from then on, so a reader
    /// derives the status from the clock instead of a read having to move it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.completes_at {
            COST_ESTIMATION_STATUS_ONGOING
        } else {
            COST_ESTIMATION_STATUS_COMPLETED
        }
    }

    /// `TimeRange.EndTime`: the completion instant once it has passed, and nothing
    /// while the estimation is still running — an unfinished estimation has no end.
    pub fn end_time(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.completes_at).then_some(self.completes_at)
    }

    /// Whether `client_token` is this estimation's idempotency token. An absent or
    /// empty token identifies nothing, so it never matches.
    pub fn has_token(&self, client_token: Option<&str>) -> bool {
        match (self.client_token.as_deref(), client_token) {
            (Some(stored), Some(offered)) => !stored.is_empty() && stored == offered,
            _ => false,
        }
    }

    /// Whether this estimation was started for exactly the collection
    /// `(stack_names, tags)` names — what tells a retry of the same request from a
    /// different request sent under the same idempotency token.
    pub fn covers_same_collection(
        &self,
        stack_names: Option<&Vec<String>>,
        tags: Option<&Vec<(String, Vec<String>)>>,
    ) -> bool {
        self.stack_names.as_ref() == stack_names && self.tags.as_ref() == tags
    }
}

/// `CostEstimationStatus` as the model spells "still running" — see
/// `COST_ESTIMATION_STATUS_VALUES` in `ops/types/shared_types.rs`.
pub const COST_ESTIMATION_STATUS_ONGOING: &str = "ONGOING";

/// `CostEstimationStatus` as the model spells "finished".
pub const COST_ESTIMATION_STATUS_COMPLETED: &str = "COMPLETED";

/// `CostEstimationServiceResourceState` for a resource the client declared for
/// analysis — see `COST_ESTIMATION_SERVICE_RESOURCE_STATE_VALUES` in
/// `ops/types/shared_types.rs`.
///
/// A resource is `ACTIVE` when it produces metrics, events or logs, and only active
/// resource hours are charged. A resource inside the account's analysis coverage is
/// exactly what the estimate is being made for, and this emulator has no metric
/// stream that could show one quiet, so a covered resource is counted active.
pub const COST_ESTIMATION_RESOURCE_ACTIVE: &str = "ACTIVE";

/// The `ServiceResourceCost.Type` for the CloudFormation half of a cost estimation's
/// collection.
///
/// This emulator's resource inventory is the coverage the client declared through
/// `UpdateResourceCollection` — it performs none of AWS's own per-service resource
/// discovery — so what it can count for a stack-defined boundary is the covered
/// stacks the request names.
pub const COST_ESTIMATION_STACK_TYPE: &str = "AWS::CloudFormation::Stack";

/// The `ServiceResourceCost.Type` for the tag half of a cost estimation's collection:
/// one covered app-boundary tag value. See [`COST_ESTIMATION_STACK_TYPE`] for why the
/// unit counted is a coverage entry.
pub const COST_ESTIMATION_TAG_TYPE: &str = "AWS::Tag";

/// The `ResourceType` a cost-estimation-shaped `ConflictException` reports.
/// `ResourceIdType` is an unconstrained string, so this value names the kind of
/// resource the reported `ResourceId` belongs to.
pub const COST_ESTIMATION_RESOURCE_TYPE: &str = "COST_ESTIMATION";

/// Emulator-enforced ceiling on `ResourceCollection.Tags`, from the documented AWS
/// rule rather than from the model: `TagCostEstimationResourceCollectionFilter`'s
/// documentation states "The maximum number of tags you can specify for a cost
/// estimate is one", and `TagCostEstimationResourceCollectionFilters` carries no list
/// length, so `StartCostEstimation` is what refuses a longer list.
pub const MAX_COST_ESTIMATION_TAG_FILTERS: usize = 1;

/// DevOps Guru's published price to analyse one active resource for one hour, in USD:
/// "$0.0028 per resource hour" (<https://aws.amazon.com/devops-guru/pricing/>), the
/// rate `GetCostEstimation` reports as `ServiceResourceCost.UnitCost`.
pub const RESOURCE_HOUR_UNIT_COST: f64 = 0.0028;

/// The hours a monthly estimate is priced over: AWS bills a 730-hour month
/// (365 days ÷ 12 months × 24 hours), and the estimate is the cost to analyse the
/// resources for a month.
pub const HOURS_PER_MONTH: f64 = 730.0;

/// A money amount at the precision of the per-resource-hour unit price (four decimal
/// places), so a cost reports a currency value rather than a binary-float tail.
pub fn round_cost(amount: f64) -> f64 {
    (amount * 10_000.0).round() / 10_000.0
}

/// The monthly cost of analysing `count` active resources at
/// [`RESOURCE_HOUR_UNIT_COST`] over a [`HOURS_PER_MONTH`] month.
pub fn monthly_cost(count: i32) -> f64 {
    round_cost(f64::from(count) * RESOURCE_HOUR_UNIT_COST * HOURS_PER_MONTH)
}

/// How long after `StartCostEstimation` the estimate is due to be finished.
///
/// DevOps Guru creates the estimate asynchronously — `StartCostEstimation` "starts the
/// creation" of it and `GetCostEstimation` reports `ONGOING` "if it's still in
/// progress" — and AWS publishes no duration for that work, so this span is the
/// emulator's own choice: never zero, so a caller reading straight after the start
/// still sees `ONGOING`, and short enough that an ordinary poll sees `COMPLETED`.
pub const COST_ESTIMATION_DURATION: Duration = Duration::from_millis(300);

/// `OptInStatus` as the model spells "off" — see `OPT_IN_STATUS_VALUES` in
/// `ops/types/shared_types.rs`. `EventSourceOptInStatus` declares the same two
/// values (`DISABLED` | `ENABLED`), so the event-source record spells "off" with
/// this same constant.
pub const OPT_IN_STATUS_DISABLED: &str = "DISABLED";

/// `OptInStatus` as the model spells "on" — the other of `OPT_IN_STATUS_VALUES`.
pub const OPT_IN_STATUS_ENABLED: &str = "ENABLED";

/// The `ServerSideEncryptionType` whose key the CUSTOMER supplies — see
/// `SERVER_SIDE_ENCRYPTION_TYPE_VALUES` in `ops/types/shared_types.rs`. The other
/// value, `AWS_OWNED_KMS_KEY`, names a key DevOps Guru owns, so it carries no
/// caller-supplied `KMSKeyId`.
pub const SERVER_SIDE_ENCRYPTION_TYPE_CUSTOMER_MANAGED_KEY: &str = "CUSTOMER_MANAGED_KEY";

/// The `ValidationExceptionReason` a refusal reports when the request is inconsistent
/// with what the account already holds — see `VALIDATION_EXCEPTION_REASON_VALUES` in
/// `ops/types/shared_types.rs`.
pub const VALIDATION_REASON_PARAMETER_INCONSISTENT_WITH_SERVICE_STATE: &str =
    "PARAMETER_INCONSISTENT_WITH_SERVICE_STATE";

/// The `ValidationExceptionReason` a refusal reports when the request's members cannot
/// hold together — one requires another the request did not supply, say. Another of
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const VALIDATION_REASON_INVALID_PARAMETER_COMBINATION: &str =
    "INVALID_PARAMETER_COMBINATION";

/// The `ResourceType` an insight-shaped `ResourceNotFoundException` reports — the
/// kind of resource the refused `ResourceId` would have named. `ResourceIdType` is an
/// unconstrained string, so this value is what says which kind that is.
pub const INSIGHT_RESOURCE_TYPE: &str = "INSIGHT";

/// Before any `UpdateServiceIntegration`, every integration is opted out: a
/// client that never enabled OpsCenter, log anomaly detection or customer-managed
/// encryption has none of them.
impl Default for ServiceIntegrationRecord {
    fn default() -> Self {
        Self {
            ops_center_opt_in_status: OPT_IN_STATUS_DISABLED.to_string(),
            logs_anomaly_detection_opt_in_status: OPT_IN_STATUS_DISABLED.to_string(),
            kms_server_side_encryption: KmsServerSideEncryptionRecord::default(),
        }
    }
}

impl Default for KmsServerSideEncryptionRecord {
    fn default() -> Self {
        Self {
            opt_in_status: OPT_IN_STATUS_DISABLED.to_string(),
            kms_key_id: None,
            encryption_type: None,
        }
    }
}

/// How many items one page of a paginated read carries.
///
/// `GetResourceCollection` and `ListNotificationChannels` model no `MaxResults`,
/// so the page size is the service's own; this is the emulator's choice, not a
/// number AWS publishes. It bounds response amplification; the write-side caps
/// below are what bound the store itself.
///
/// It is also the default page size of a read that DOES model `MaxResults`
/// (`ListInsights`), where it coincides with that member's modelled maximum of 100.
pub const PAGE_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every entry in this store is put there by a caller and is reclaimed only by that
// caller's explicit `REMOVE` / `RemoveNotificationChannel` — a generated emulator
// keeps its state for the life of the process, and neither a TTL nor an eviction
// policy is available to it, because `GetResourceCollection` and
// `ListNotificationChannels` must still answer what the caller wrote. So every
// growth site carries a maximum entry count, checked BEFORE the write so a refusal
// leaves the store exactly as it was.
//
// Per-entry SIZE is already bounded by the model, which the boundary validator
// enforces before a handler runs: `StackName` len 1..128, `AppBoundaryKey`
// len 1..128, `TagValue` len 0..256, `TopicArn` len 36..1024, `KMSKeyId` len
// 1..2048, `InsightId` len 1..100. Nothing caller-sized reaches the store unchecked.
//
// The product is therefore finite: at most MAX_COVERED_STACK_NAMES stack names, the
// account's one app-boundary key ([`APP_BOUNDARY_KEYS_PER_ACCOUNT`]) holding at most
// MAX_TAG_VALUES_PER_KEY values, at most MAX_NOTIFICATION_CHANNELS channels, at most
// MAX_FEEDBACK_INSIGHTS feedback records, one service-integration record of fixed
// shape, and one cost estimation.
//
// The cost estimation is not a growth site either: `StartCostEstimation` replaces the
// single record in place, and the collection it stores is bounded by the model
// (`CostEstimationStackNames` and `CostEstimationTagValues` are both fixed at 1 item)
// together with the one-tag maximum `TagCostEstimationResourceCollectionFilter`
// documents, which `StartCostEstimation` enforces before it stores anything.
//
// `insights` carries no cap because no request can grow it: insights come from DevOps
// Guru's analysis, which this emulator does not run, so nothing a caller sends adds an
// entry. An operation that ever mints one brings its own bound.
// ---------------------------------------------------------------------------

/// DevOps Guru's own documented ceiling, from `UpdateResourceCollection`'s and
/// `CloudFormationCollectionFilter`'s model documentation: "You can specify up to 500
/// Amazon Web Services CloudFormation stacks."
pub const MAX_COVERED_STACK_NAMES: usize = 500;

/// Emulator resource guard, NOT a published AWS quota: the most tag values this store
/// holds for the account's app-boundary key. `UpdateTagCollectionFilters` declares no
/// list length, so without this one request could add arbitrarily many values.
pub const MAX_TAG_VALUES_PER_KEY: usize = 500;

/// Emulator resource guard, NOT a published AWS quota: the most insights this store
/// holds feedback for. DevOps Guru models no operation that withdraws feedback, so a
/// record is never reclaimed; without this one caller could grow the store without
/// bound by submitting feedback for fresh insight ids. Resubmitting feedback for an
/// insight already in the store replaces its record and so is never refused. Set well
/// above any ordinary workflow: a human rates a handful of insights.
pub const MAX_FEEDBACK_INSIGHTS: usize = 1_000;

/// Emulator resource guard, NOT a published AWS quota: the most notification channels
/// this store holds. `AddNotificationChannel` refuses past it with the
/// `ServiceQuotaExceededException` (HTTP 402) the model declares on exactly that
/// operation and on no read. AWS's own notification-channel quota is far smaller and
/// adjustable, so no single value is the contract; this is set well above any
/// ordinary workflow.
pub const MAX_NOTIFICATION_CHANNELS: usize = 200;

/// The 24 hex characters that bind a page token to the collection it walks.
///
/// A token names an offset into ONE collection, so it must not be replayable
/// against another: `GetResourceCollection`'s AWS_TAGS cursor is not a
/// `ListNotificationChannels` cursor. The digest of the scope string is what
/// makes a token from another collection unresolvable instead of silently
/// seeking into the wrong list.
fn scope_digest(scope: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(scope.as_bytes());
    hex::encode(digest)[..24].to_string()
}

/// A page cursor rendered as the model's `UuidNextToken`: 36 characters matching
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
///
/// The first group is the offset; the remaining 24 hex characters are
/// [`scope_digest`]. The token is therefore self-describing — resolving it needs
/// no server-side cursor table, so a read never has to write to serve a page.
pub fn encode_page_token(scope: &str, offset: usize) -> Option<String> {
    let offset = u32::try_from(offset).ok()?;
    let digest = scope_digest(scope);
    Some(format!(
        "{:08x}-{}-{}-{}-{}",
        offset,
        &digest[0..4],
        &digest[4..8],
        &digest[8..12],
        &digest[12..24]
    ))
}

/// The offset a token names, or `None` when this service did not issue it for `scope`.
///
/// The strict inverse of [`encode_page_token`]: the token must have the modelled
/// `8-4-4-4-12` lowercase-hex shape, its 24 digest characters must be `scope`'s, and
/// its offset must name a page boundary. Every token this service issues satisfies all
/// three, so anything else — a different collection's cursor, an offset edited off a
/// boundary, an uppercase or `+`-prefixed rewrite — resolves to no page rather than to
/// an arbitrary seek (the digest is not a secret, so the offset group is the part a
/// caller can plausibly change).
pub fn decode_page_token(scope: &str, token: &str) -> Option<usize> {
    let offset = decode_token_offset(scope, token)?;
    offset.is_multiple_of(PAGE_SIZE).then_some(offset)
}

/// The offset `token` names for `scope`, checking only what the token itself must
/// satisfy: the modelled `8-4-4-4-12` lowercase-hex shape and `scope`'s digest.
///
/// [`decode_page_token`] adds the page-boundary check, which it can only do because its
/// page size is this service's own [`PAGE_SIZE`]. An operation that models `MaxResults`
/// pages at the size the CALLER asked for, and AWS honours a `MaxResults` that changes
/// between pages of one walk, so there the offset need not be a multiple of the current
/// page size — see [`page_sized`].
fn decode_token_offset(scope: &str, token: &str) -> Option<usize> {
    let groups: Vec<&str> = token.split('-').collect();
    let [offset, a, b, c, d] = groups[..] else {
        return None;
    };
    let widths_ok = [offset.len(), a.len(), b.len(), c.len(), d.len()] == [8, 4, 4, 4, 12];
    let lowercase_hex = token.chars().all(|c| matches!(c, '-' | '0'..='9' | 'a'..='f'));
    if !widths_ok || !lowercase_hex {
        return None;
    }
    if format!("{a}{b}{c}{d}") != scope_digest(scope) {
        return None;
    }
    Some(u32::from_str_radix(offset, 16).ok()? as usize)
}

/// One page of `items`, plus the token for the next page when more remain.
///
/// `None` means the supplied token does not resolve against `scope` — the caller
/// answers that with the operation's `ValidationException`, because a token this
/// service never issued for this collection names no page of it.
///
/// An offset past the end is `None` for the same reason, which also covers a token
/// this service DID issue for a collection that has since shrunk: the page it named no
/// longer exists, and reporting an empty page instead would tell the caller the walk
/// had finished when entries it never reached were removed. The `ValidationException`
/// says the cursor is stale, so the caller restarts the walk and sees the collection
/// as it is now.
pub fn page<T: Clone>(
    items: &[T],
    scope: &str,
    token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let offset = match token {
        Some(token) => decode_page_token(scope, token)?,
        None => 0,
    };
    page_at(items, scope, offset, PAGE_SIZE)
}

/// [`page`], at a page size the CALLER chose through a modelled `MaxResults`.
///
/// Resolves the token on its shape and its digest alone: the caller may ask for a
/// different `MaxResults` on each call of one walk, so an offset this service issued at
/// an earlier page size is not a multiple of the current one and must still resolve.
/// `None` has the same two meanings as in [`page`] — a token not issued for `scope`, or
/// an offset past the end of a collection that has since shrunk — which the caller
/// answers with its `ValidationException`.
pub fn page_sized<T: Clone>(
    items: &[T],
    scope: &str,
    token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    let offset = match token {
        Some(token) => decode_token_offset(scope, token)?,
        None => 0,
    };
    page_at(items, scope, offset, page_size.max(1))
}

/// The `page_size` items at `offset`, plus the token for the page after them.
fn page_at<T: Clone>(
    items: &[T],
    scope: &str,
    offset: usize,
    page_size: usize,
) -> Option<(Vec<T>, Option<String>)> {
    if offset > items.len() {
        return None;
    }
    // Saturating, so a page size a caller pushed past the modelled maximum cannot
    // wrap the end of the slice below its start.
    let end = offset.saturating_add(page_size).min(items.len());
    let next = if end < items.len() {
        encode_page_token(scope, end)
    } else {
        None
    };
    Some((items[offset..end].to_vec(), next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>)
}
