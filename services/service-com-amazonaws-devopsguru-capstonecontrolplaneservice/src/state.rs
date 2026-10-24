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
    AccountInsightHealth, CloudFormationCollection, CloudFormationHealth, InsightHealth,
    InsightTimeRange, ListInsightsStatusFilter, MonitoredResourceIdentifier, PredictionTimeRange,
    ProactiveInsight, ProactiveInsightSummary, ProactiveOrganizationInsightSummary, ReactiveInsight,
    ReactiveInsightSummary, ReactiveOrganizationInsightSummary, ResourceCollection,
    ServiceCollection, ServiceHealth, ServiceInsightHealth, TagCollection, TagHealth,
    ValidationException, ValidationExceptionField, INSIGHT_TYPE_VALUES,
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
    /// The account that administers DevOps Guru for the organization this store
    /// records, if any.
    ///
    /// An organization-wide read (`DescribeOrganizationResourceCollectionHealth` and
    /// the service's other `*Organization*` operations) reports the member accounts
    /// of an organization, which is a view only that organization's management
    /// account or its registered DevOps Guru delegated administrator holds. No
    /// operation of this service registers one — that registration is made through
    /// Organizations, which this emulator does not emulate — so the field holds
    /// `None` and the one account this emulator serves
    /// ([`::engine::emulator_account_id`]) administers no organization. Read it
    /// through [`Self::administers_organization`], which therefore answers `false`
    /// for every caller a client can produce: an organization-wide read states the
    /// rule once and refuses with its modelled `AccessDeniedException`.
    pub organization_admin_account_id: Option<String>,
}

impl ComAmazonawsDevopsguruCapstonecontrolplaneserviceState {
    /// Whether `account_id` administers the organization this store records — the
    /// caller an organization-wide read is available to. False whenever no
    /// administrator is recorded, which is the only state a client of this emulator
    /// can produce: an organization-only view is then available to nobody.
    pub fn administers_organization(&self, account_id: &str) -> bool {
        self.organization_admin_account_id.as_deref() == Some(account_id)
    }

    /// One `CloudFormationHealth` per covered stack name, in the store's stable
    /// order, each tallying the insights whose own `ResourceCollection` names that
    /// stack.
    ///
    /// The health of the `AWS_CLOUD_FORMATION` collection, as both
    /// `DescribeResourceCollectionHealth` and
    /// `DescribeOrganizationResourceCollectionHealth` report it: one store, one
    /// member set, so the account-scoped and the organization-scoped read cannot
    /// disagree about a stack's health.
    pub fn cloud_formation_healths(&self) -> Vec<CloudFormationHealth> {
        self.resource_collection
            .cloudformation_stack_names
            .iter()
            .map(|stack_name| CloudFormationHealth {
                stack_name: Some(stack_name.clone()),
                insight: Some(
                    InsightTally::over(self.insights.values(), |insight| {
                        insight.resource_collection.as_ref().is_some_and(|about| {
                            about.cloudformation_stack_names.contains(stack_name)
                        })
                    })
                    .insight_health(),
                ),
                analyzed_resource_count: Some(MEMBER_ANALYZED_RESOURCE_COUNT),
            })
            .collect()
    }

    /// One `TagHealth` per covered (`AppBoundaryKey`, `TagValue`) pair — `TagHealth`
    /// names both, so a pair is a member in its own right — each tallying the
    /// insights whose own tag collection names that pair. A key is covered exactly
    /// while one of its values is, so no key yields none.
    pub fn tag_healths(&self) -> Vec<TagHealth> {
        self.resource_collection
            .tag_collections
            .iter()
            .flat_map(|(app_boundary_key, tag_values)| {
                tag_values.iter().map(move |tag_value| TagHealth {
                    app_boundary_key: Some(app_boundary_key.clone()),
                    tag_value: Some(tag_value.clone()),
                    insight: Some(
                        InsightTally::over(self.insights.values(), |insight| {
                            insight.resource_collection.as_ref().is_some_and(|about| {
                                about
                                    .tag_collections
                                    .get(app_boundary_key)
                                    .is_some_and(|values| values.contains(tag_value))
                            })
                        })
                        .insight_health(),
                    ),
                    analyzed_resource_count: Some(MEMBER_ANALYZED_RESOURCE_COUNT),
                })
            })
            .collect()
    }

    /// One `ServiceHealth` per Amazon Web Services service DevOps Guru ANALYSED.
    ///
    /// Membership is discovered by that analysis, not declared by the client
    /// (`ServiceName` is a closed enum and `UpdateResourceCollection` cannot name
    /// one), and this store's record of an analysed service is the
    /// `ServiceCollection` its insights name. So the services reported are exactly
    /// those, sorted for a stable page order, and an account whose analysis produced
    /// no insight reports an empty list rather than an invented service.
    ///
    /// `ServiceInsightHealth` models only the two open counts, so the MTTR the tally
    /// also computes is dropped here. The coverage attributes no entry to an Amazon
    /// Web Services service, so how many monitored resources belong to one is not
    /// something this store records: the optional `AnalyzedResourceCount` is omitted
    /// rather than guessed.
    pub fn service_healths(&self) -> Vec<ServiceHealth> {
        let analysed: BTreeSet<&String> = self
            .insights
            .values()
            .flat_map(|insight| insight.service_names.iter())
            .collect();
        analysed
            .into_iter()
            .map(|service_name| ServiceHealth {
                service_name: Some(service_name.clone()),
                insight: Some(
                    InsightTally::over(self.insights.values(), |insight| {
                        insight.service_names.contains(service_name)
                    })
                    .service_insight_health(),
                ),
                analyzed_resource_count: None,
            })
            .collect()
    }

    /// The account-level tally: every insight this store holds, whatever resource
    /// collection it is about, which is what `AccountInsightHealth` counts — "the
    /// number of open proactive insights in your Amazon Web Services account".
    pub fn account_insight_tally(&self) -> InsightTally {
        InsightTally::over(self.insights.values(), |_| true)
    }
}

/// `AnalyzedResourceCount` for ONE member of a resource collection: how many
/// resources DevOps Guru is monitoring within that member.
///
/// The analysed population is the coverage itself — see
/// [`ResourceCoverage::monitored_resource_count`] — so each coverage entry IS one
/// monitored resource, one per covered stack name and one per (app-boundary key, tag
/// value) pair. A member of a health report is exactly one such entry, so it accounts
/// for exactly one analysed resource, and the members of a collection type therefore
/// sum to the account-level `AnalyzedResourceCount` that `DescribeAccountHealth`
/// reports over the same store — the reads of one store agree instead of contradicting
/// each other.
pub const MEMBER_ANALYZED_RESOURCE_COUNT: i64 = 1;

/// `InsightType`'s `REACTIVE` member, as the model spells it — see
/// `INSIGHT_TYPE_VALUES` in `ops/types/shared_types.rs`. Its `PROACTIVE` member is
/// [`InsightRecord::PROACTIVE`].
pub const INSIGHT_TYPE_REACTIVE: &str = "REACTIVE";

/// `InsightStatus` as the model spells "the behaviour has not ended" — see
/// `INSIGHT_STATUS_VALUES`.
pub const INSIGHT_STATUS_ONGOING: &str = "ONGOING";

/// `InsightStatus` as the model spells "the behaviour ended" — the other of
/// `INSIGHT_STATUS_VALUES`.
pub const INSIGHT_STATUS_CLOSED: &str = "CLOSED";

/// The insight tally over a SET of stored insights: how many of them are open, by
/// `InsightType`, and the Mean Time to Recover over the closed ones.
///
/// `InsightHealth`, `ServiceInsightHealth` and `AccountInsightHealth` are three
/// projections of this one tally — the first adds the MTTR, the other two carry only
/// the open counts — so every health read of this store computes its numbers the same
/// way and two reads cannot contradict each other.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsightTally {
    /// The `ONGOING` insights of type `PROACTIVE` in the set.
    pub open_proactive: i32,
    /// The `ONGOING` insights of type `REACTIVE` in the set.
    pub open_reactive: i32,
    /// The mean, over the `CLOSED` insights of the set that record an end instant, of
    /// how long each took to recover: its `InsightTimeRange` end minus its start.
    pub mean_time_to_recover_millis: i64,
}

impl InsightTally {
    /// The tally over the insights `covers` keeps.
    ///
    /// No modelled operation mints an insight — DevOps Guru derives one from anomalies
    /// in the covered resources — so the set walked is this store's [`insights`] map,
    /// what that analysis writes and every insight read reports. An emulator over
    /// which no analysis has run therefore tallies an empty set and answers zeros,
    /// with no recovery time accrued to average.
    ///
    /// [`insights`]: ComAmazonawsDevopsguruCapstonecontrolplaneserviceState::insights
    pub fn over<'a>(
        insights: impl Iterator<Item = &'a InsightRecord>,
        covers: impl Fn(&InsightRecord) -> bool,
    ) -> Self {
        let mut open_proactive = 0i32;
        let mut open_reactive = 0i32;
        let mut recovered = 0i64;
        let mut total_recovery_millis = 0i64;
        for insight in insights.filter(|insight| covers(insight)) {
            match (insight.insight_type.as_str(), insight.status.as_str()) {
                (InsightRecord::PROACTIVE, INSIGHT_STATUS_ONGOING) => {
                    open_proactive = open_proactive.saturating_add(1);
                }
                (INSIGHT_TYPE_REACTIVE, INSIGHT_STATUS_ONGOING) => {
                    open_reactive = open_reactive.saturating_add(1);
                }
                // A closed insight is no longer open; what it contributes is the time
                // it took to recover, which only an insight whose behaviour has an end
                // instant carries.
                (_, INSIGHT_STATUS_CLOSED) => {
                    if let Some(end) = insight.end_time_millis {
                        // Saturating throughout, so a stored instant pair far enough
                        // apart to overflow cannot panic a read.
                        total_recovery_millis = total_recovery_millis
                            .saturating_add(end.saturating_sub(insight.start_time_millis).max(0));
                        recovered = recovered.saturating_add(1);
                    }
                }
                _ => {}
            }
        }
        Self {
            open_proactive,
            open_reactive,
            // The mean over the set's closed insights; zero while none has closed,
            // because no recovery time has been accrued.
            mean_time_to_recover_millis: if recovered == 0 {
                0
            } else {
                total_recovery_millis / recovered
            },
        }
    }

    /// This tally as the modelled `InsightHealth`, which carries the MTTR.
    pub fn insight_health(&self) -> InsightHealth {
        InsightHealth {
            open_proactive_insights: self.open_proactive,
            open_reactive_insights: self.open_reactive,
            mean_time_to_recover_in_milliseconds: Some(self.mean_time_to_recover_millis),
        }
    }

    /// This tally as the modelled `ServiceInsightHealth`, which models only the two
    /// open counts.
    pub fn service_insight_health(&self) -> ServiceInsightHealth {
        ServiceInsightHealth {
            open_proactive_insights: self.open_proactive,
            open_reactive_insights: self.open_reactive,
        }
    }

    /// This tally as the modelled `AccountInsightHealth`, which models only the two
    /// open counts.
    pub fn account_insight_health(&self) -> AccountInsightHealth {
        AccountInsightHealth {
            open_proactive_insights: self.open_proactive,
            open_reactive_insights: self.open_reactive,
        }
    }
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

    /// Whether this record is reported as a REACTIVE insight — the OTHER of
    /// `INSIGHT_TYPE_VALUES`'s two values, which [`Self::is_proactive`] tests for.
    ///
    /// A listing partitions its page with both predicates, so every insight whose
    /// `insight_type` the model admits lands on exactly one of the response's two lists.
    /// A value outside the enum is not one this store holds — `InsightType` is modelled
    /// as an enum, and only DevOps Guru's own analysis writes the map — and it is
    /// reported as neither type rather than as REACTIVE by default.
    pub fn is_reactive(&self) -> bool {
        self.insight_type == INSIGHT_TYPE_REACTIVE
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

    /// This record as the modelled `ProactiveOrganizationInsightSummary`: the same
    /// insight [`Self::to_proactive_summary`] renders for the account-scoped listing,
    /// stamped with the member account that holds it and the organizational unit that
    /// account belongs to — the two members an organization-wide listing adds. Built
    /// from the SAME record, so the account-scoped and the organization-scoped listing
    /// of one store cannot disagree about an insight.
    ///
    /// The organization summary shapes model no `AssociatedResourceArns` and no
    /// `SsmOpsItemId`, so neither is rendered here.
    pub fn to_proactive_organization_summary(
        &self,
        account_id: &str,
        organizational_unit_id: Option<&str>,
    ) -> ProactiveOrganizationInsightSummary {
        ProactiveOrganizationInsightSummary {
            id: Some(self.id.clone()),
            account_id: Some(account_id.to_string()),
            organizational_unit_id: organizational_unit_id.map(ToString::to_string),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            prediction_time_range: self.prediction_time_range(),
            resource_collection: self.wire_resource_collection(),
            service_collection: self.wire_service_collection(),
        }
    }

    /// This record as the modelled `ReactiveOrganizationInsightSummary`, which carries
    /// no `PredictionTimeRange`: only a proactive insight predicts a window.
    pub fn to_reactive_organization_summary(
        &self,
        account_id: &str,
        organizational_unit_id: Option<&str>,
    ) -> ReactiveOrganizationInsightSummary {
        ReactiveOrganizationInsightSummary {
            id: Some(self.id.clone()),
            account_id: Some(account_id.to_string()),
            organizational_unit_id: organizational_unit_id.map(ToString::to_string),
            name: Some(self.name.clone()),
            severity: Some(self.severity.clone()),
            status: Some(self.status.clone()),
            insight_time_range: Some(self.insight_time_range()),
            resource_collection: self.wire_resource_collection(),
            service_collection: self.wire_service_collection(),
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

// ---------------------------------------------------------------------------
// What an insight listing selects
//
// `ListInsights` and `ListOrganizationInsights` take the SAME
// `ListInsightsStatusFilter` and select over the same `insights` map — one
// account-scoped, one organization-wide — so the selector, the time ranges it
// compares against and the refusal of a malformed filter live here once. Two
// listings of one store then cannot disagree about which insights a filter names.
// ---------------------------------------------------------------------------

/// Why a submitted insight filter is refused, stated WITHOUT naming an operation's
/// error enum: the shared selector cannot construct one, so it reports the refusal and
/// each handler lifts it into its own modelled `ValidationException` through
/// [`FilterRejection::validation_exception`].
#[derive(Debug, Clone)]
pub struct FilterRejection {
    /// What is wrong with the request, as the exception's `Message`.
    pub message: String,
    /// One of `VALIDATION_EXCEPTION_REASON_VALUES`.
    pub reason: &'static str,
    /// The request member at fault, as `ValidationExceptionField.Name`.
    pub field: String,
    /// What that member must satisfy, as `ValidationExceptionField.Message`.
    pub field_message: String,
}

impl FilterRejection {
    /// The refusal of a `NextToken` that names no page of `collection`.
    ///
    /// Every paginated read of this store answers a stale or foreign cursor the same
    /// way: a token is bound to the selection it was issued for, so one issued for
    /// another selection, by another operation, or into a collection that has since
    /// shrunk names no page at all. `scoped_by` names the request members the cursor is
    /// bound to.
    pub fn stale_cursor(operation: &str, collection: &str, scoped_by: &str) -> Self {
        Self {
            message: format!("NextToken does not name a page of {collection}"),
            reason: "FIELD_VALIDATION_FAILED",
            field: "NextToken".to_string(),
            field_message: format!(
                "must be a token returned by an earlier {operation} for the same {scoped_by}"
            ),
        }
    }

    /// This refusal as the modelled `ValidationException` every insight listing
    /// declares (HTTP 400).
    pub fn validation_exception(&self) -> ValidationException {
        ValidationException {
            message: self.message.clone(),
            reason: Some(self.reason.to_string()),
            fields: Some(vec![ValidationExceptionField {
                name: self.field.clone(),
                message: self.field_message.clone(),
            }]),
        }
    }
}

/// A closed interval of instants, in milliseconds since the epoch. An absent bound is
/// unbounded on that side, which is what `StartTimeRange` and `EndTimeRange` model:
/// both of their members are optional.
#[derive(Debug, Clone, Copy)]
pub struct MillisRange {
    from: Option<i64>,
    to: Option<i64>,
}

impl MillisRange {
    /// Whether `at` lies in this interval.
    pub fn holds(&self, at: i64) -> bool {
        self.from.is_none_or(|from| at >= from) && self.to.is_none_or(|to| at <= to)
    }

    /// The interval as a page-scope fragment, so a cursor into one window does not
    /// resolve against another.
    fn page_scope(&self) -> String {
        let bound = |at: Option<i64>| at.map_or_else(|| "-".to_string(), |at| at.to_string());
        format!("{}/{}", bound(self.from), bound(self.to))
    }
}

/// The ONE status filter a request names, resolved to what it selects.
///
/// `ListInsightsStatusFilter` carries three optional members and `StatusFilter`'s
/// documentation says "You can specify one status filter", so exactly one of them is a
/// well-formed request. The model cannot express that, so [`InsightStatusSelector::of`]
/// enforces it.
#[derive(Debug, Clone, Copy)]
pub enum InsightStatusSelector {
    /// `Ongoing`: the insights of this type whose behaviour has not ended.
    Ongoing { insight_type: &'static str },
    /// `Closed`: the insights of this type whose behaviour ended inside the range.
    Closed {
        insight_type: &'static str,
        ended: MillisRange,
    },
    /// `Any`: the insights of this type, whatever their status, whose behaviour started
    /// inside the range.
    Any {
        insight_type: &'static str,
        started: MillisRange,
    },
}

impl InsightStatusSelector {
    /// The selector `filter` names, or the [`FilterRejection`] that refuses it.
    pub fn of(filter: &ListInsightsStatusFilter) -> Result<Self, FilterRejection> {
        let named: Vec<&str> = [
            filter.ongoing.is_some().then_some("Ongoing"),
            filter.closed.is_some().then_some("Closed"),
            filter.any.is_some().then_some("Any"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if named.len() != 1 {
            let named = if named.is_empty() {
                "none".to_string()
            } else {
                named.join(" + ")
            };
            return Err(FilterRejection {
                message: format!(
                    "StatusFilter must specify exactly one of Ongoing, Closed or Any, got {named}"
                ),
                reason: "INVALID_PARAMETER_COMBINATION",
                field: "StatusFilter".to_string(),
                field_message: "exactly one of Ongoing, Closed or Any must be specified"
                    .to_string(),
            });
        }

        if let Some(ongoing) = filter.ongoing.as_ref() {
            return Ok(Self::Ongoing {
                insight_type: insight_type_of(&ongoing.r#type, "StatusFilter.Ongoing.Type")?,
            });
        }
        if let Some(closed) = filter.closed.as_ref() {
            return Ok(Self::Closed {
                insight_type: insight_type_of(&closed.r#type, "StatusFilter.Closed.Type")?,
                ended: millis_range(
                    closed.end_time_range.from_time.as_ref(),
                    closed.end_time_range.to_time.as_ref(),
                    "StatusFilter.Closed.EndTimeRange",
                )?,
            });
        }
        let any = filter
            .any
            .as_ref()
            .expect("exactly one selector is named, and it is neither Ongoing nor Closed");
        Ok(Self::Any {
            insight_type: insight_type_of(&any.r#type, "StatusFilter.Any.Type")?,
            started: millis_range(
                any.start_time_range.from_time.as_ref(),
                any.start_time_range.to_time.as_ref(),
                "StatusFilter.Any.StartTimeRange",
            )?,
        })
    }

    /// Whether this selector selects `insight`.
    pub fn matches(&self, insight: &InsightRecord) -> bool {
        match self {
            Self::Ongoing { insight_type } => {
                insight.insight_type == *insight_type && insight.status == INSIGHT_STATUS_ONGOING
            }
            Self::Closed {
                insight_type,
                ended,
            } => {
                insight.insight_type == *insight_type
                    && insight.status == INSIGHT_STATUS_CLOSED
                    // A closed insight's behaviour ended at `EndTime`; one that records
                    // no end instant lies in no end-time range.
                    && insight.end_time_millis.is_some_and(|at| ended.holds(at))
            }
            Self::Any {
                insight_type,
                started,
            } => insight.insight_type == *insight_type && started.holds(insight.start_time_millis),
        }
    }

    /// The scope a page token over this selection is bound to: a cursor into one
    /// selection must not seek into another's. A caller prefixes its operation, so a
    /// cursor of the account-scoped listing does not resolve against the
    /// organization-wide one either.
    pub fn page_scope(&self) -> String {
        match self {
            Self::Ongoing { insight_type } => {
                format!("{INSIGHT_STATUS_ONGOING}/{insight_type}")
            }
            Self::Closed {
                insight_type,
                ended,
            } => format!(
                "{INSIGHT_STATUS_CLOSED}/{insight_type}/ended/{}",
                ended.page_scope()
            ),
            Self::Any {
                insight_type,
                started,
            } => format!("ANY/{insight_type}/started/{}", started.page_scope()),
        }
    }
}

/// `value` as one of `InsightType`'s values, or the rejection naming `member`.
///
/// The model declares the member as an enum, so the boundary validator refuses anything
/// else before a handler runs; this keeps such a value a 400 rather than a silently
/// empty answer if it ever reaches here.
fn insight_type_of(value: &str, member: &str) -> Result<&'static str, FilterRejection> {
    match value {
        InsightRecord::PROACTIVE => Ok(InsightRecord::PROACTIVE),
        INSIGHT_TYPE_REACTIVE => Ok(INSIGHT_TYPE_REACTIVE),
        other => {
            let allowed = INSIGHT_TYPE_VALUES.join(" | ");
            Err(FilterRejection {
                message: format!("{member} must be one of {allowed}, got '{other}'"),
                reason: "FIELD_VALIDATION_FAILED",
                field: member.to_string(),
                field_message: format!("must be one of {allowed}"),
            })
        }
    }
}

/// A submitted time range in the milliseconds the store keeps, or the rejection naming
/// `member` when a bound is not a representable instant.
///
/// A range whose `FromTime` is after its `ToTime` is NOT refused: it holds no instant,
/// so it selects nothing, which is what an empty interval means. The model states no
/// ordering rule for either range, so refusing it would be this emulator's invention.
fn millis_range(
    from: Option<&::aws_smithy_types::DateTime>,
    to: Option<&::aws_smithy_types::DateTime>,
    member: &str,
) -> Result<MillisRange, FilterRejection> {
    let bound = |at: Option<&::aws_smithy_types::DateTime>, side: &str| match at {
        None => Ok(None),
        Some(at) => millis_of(at).map(Some).ok_or_else(|| FilterRejection {
            message: format!("{member}.{side} is not an instant this service can represent"),
            reason: "FIELD_VALIDATION_FAILED",
            field: format!("{member}.{side}"),
            field_message: "must be an instant representable in milliseconds since the epoch"
                .to_string(),
        }),
    };
    Ok(MillisRange {
        from: bound(from, "FromTime")?,
        to: bound(to, "ToTime")?,
    })
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

/// One day, in milliseconds.
///
/// The granularity of a DevOps Guru time range whose bounds are DAYS rather than
/// instants: `DescribeAccountOverview`'s and `DescribeOrganizationOverview`'s `FromTime`
/// and `ToTime` are both documented "The start time granularity is at the day level. The
/// floor of the start time is used."
pub const MILLIS_PER_DAY: i64 = 86_400_000;

/// `millis_since_epoch` floored to the start of the UTC day that holds it — the "floor"
/// a day-granular time range applies to each of its bounds.
///
/// Euclidean division, so an instant before the epoch floors to the start of ITS day
/// instead of towards the epoch. The multiplication SATURATES: a bound comes straight
/// from a request, and the floor of an instant within a day of `i64::MIN` lies below
/// `i64::MIN`, so the product is clamped to the earliest instant this store can keep
/// rather than overflowing.
pub fn day_floor_millis(millis_since_epoch: i64) -> i64 {
    millis_since_epoch
        .div_euclid(MILLIS_PER_DAY)
        .saturating_mul(MILLIS_PER_DAY)
}

/// A range of whole UTC DAYS, in the milliseconds this store keeps: every instant from
/// the start of the first day up to, but not including, the start of the day AFTER the
/// last.
///
/// What a DevOps Guru overview's `FromTime` / `ToTime` pair means. Both members are
/// documented "The start time granularity is at the day level. The floor of the start
/// time is used", so each names a DAY rather than an instant and the range spans those
/// days in full. The model's "occurred after this day" / "occurred before this day" is
/// read as naming the first and the last day OF the range, which is the only reading
/// under which a one-day request — `FromTime` and `ToTime` on the same day, or the
/// `ToTime` default of "the current day" — holds any instant at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayRange {
    /// The start of the first day — inclusive.
    pub from: i64,
    /// The start of the day after the last — exclusive, so the last day counts in full.
    pub until: i64,
}

impl DayRange {
    /// The range whose first day holds `from_millis` and whose last day holds
    /// `to_millis`, or — when the request named no end — the current day.
    ///
    /// A `from_millis` after `to_millis` yields a range that holds no instant, so it
    /// selects nothing, which is what an empty interval means. The model states no
    /// ordering rule for the two members, so refusing such a request would be this
    /// emulator's invention.
    pub fn of_days(from_millis: i64, to_millis: Option<i64>) -> Self {
        let last_day = day_floor_millis(to_millis.unwrap_or_else(now_millis));
        Self {
            from: day_floor_millis(from_millis),
            until: last_day.saturating_add(MILLIS_PER_DAY),
        }
    }

    /// Whether the instant `at` lies in this range.
    pub fn holds(&self, at: i64) -> bool {
        at >= self.from && at < self.until
    }
}

/// The wall clock in the milliseconds this store keeps.
///
/// What "the current day" means for a read whose `ToTime` is absent.
pub fn now_millis() -> i64 {
    let now = ::std::time::SystemTime::now();
    match now.duration_since(::std::time::UNIX_EPOCH) {
        Ok(since_epoch) => i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => {
            i64::try_from(before_epoch.duration().as_millis()).map_or(i64::MIN, |millis| -millis)
        }
    }
}

/// Which member accounts of the organization an organization-wide read leaves in scope.
///
/// Every `*Organization*` operation of this service carries the same two optional
/// filters: `AccountIds` is "The ID of the Amazon Web Services account" and
/// `OrganizationalUnitIds` "The ID of the organizational unit". Both NARROW the
/// organization-wide answer to the member accounts they name, so an account is in scope
/// exactly while every filter the request carries selects it. A filter that names
/// nothing restricts nothing, which is what an absent filter means, so an empty list and
/// an absent member behave alike (`AccountIdList` and `OrganizationalUnitIdList` are both
/// modelled len 0..5, so an empty list is a well-formed request).
#[derive(Debug, Clone, Copy)]
pub struct OrganizationAccountScope<'a> {
    /// The `AccountIds` the request names, or `None` when it names none.
    pub account_ids: Option<&'a Vec<String>>,
    /// The `OrganizationalUnitIds` the request names, or `None` when it names none.
    pub organizational_unit_ids: Option<&'a Vec<String>>,
}

impl<'a> OrganizationAccountScope<'a> {
    /// The scope a request's two filter members name.
    pub fn of(
        account_ids: &'a Option<Vec<String>>,
        organizational_unit_ids: &'a Option<Vec<String>>,
    ) -> Self {
        Self {
            account_ids: named(account_ids),
            organizational_unit_ids: named(organizational_unit_ids),
        }
    }

    /// Whether this scope holds the member account `account_id`, whose organizational
    /// unit membership is `organizational_unit_ids`.
    ///
    /// An `OrganizationalUnitIds` filter selects the member accounts of the units it
    /// names, so an account in none of them — which is every account of a store that
    /// records no organizational unit — is out of scope rather than reported as if the
    /// filter had not been sent.
    pub fn holds(&self, account_id: &str, organizational_unit_ids: &[String]) -> bool {
        self.account_ids
            .is_none_or(|wanted| wanted.iter().any(|id| id == account_id))
            && self.organizational_unit_ids.is_none_or(|wanted| {
                wanted
                    .iter()
                    .any(|unit| organizational_unit_ids.iter().any(|held| held == unit))
            })
    }

    /// Whether this scope holds the one member account this emulator serves.
    ///
    /// The store holds the coverage, the insights and the configuration of a SINGLE
    /// account and records no organizational-unit membership for it, so the
    /// organization's membership is that one account in no unit: an
    /// `OrganizationalUnitIds` filter therefore leaves no account in scope. Stated here
    /// once, so every organization-wide read of this store scopes its answer the same
    /// way and a store that one day records unit membership has one place to change.
    pub fn holds_served_member(&self, account_id: &str) -> bool {
        self.holds(account_id, &[])
    }

    /// This scope as a fragment of a page token's scope string, so a cursor issued under
    /// one filter does not resolve under another.
    ///
    /// The join is unambiguous because of the two modelled patterns: `AwsAccountId` is
    /// `^\d{12}$` and `OrganizationalUnitId` `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`, so no
    /// id a well-formed request can carry holds the `,` or `/` this fragment separates
    /// with, and two different filter sets cannot render the same string.
    pub fn page_fragment(&self) -> String {
        format!(
            "accounts/{}/units/{}",
            self.account_ids
                .map_or_else(|| "-".to_string(), |ids| ids.join(",")),
            self.organizational_unit_ids
                .map_or_else(|| "-".to_string(), |ids| ids.join(",")),
        )
    }
}

/// The ids a filter member names, or `None` when it names none: an absent list and an
/// empty one both restrict nothing.
fn named(list: &Option<Vec<String>>) -> Option<&Vec<String>> {
    match list {
        Some(ids) if !ids.is_empty() => Some(ids),
        _ => None,
    }
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
    /// resource-analysis hour per entry as `ResourceHours`. The entries themselves are
    /// [`Self::monitored_resources`], so this count is exactly that list's length.
    pub fn monitored_resource_count(&self) -> usize {
        self.cloudformation_stack_names.len()
            + self
                .tag_collections
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
    }

    /// The resources this coverage puts under DevOps Guru analysis, one
    /// `MonitoredResourceIdentifier` each, as `ListMonitoredResources` reports them.
    ///
    /// The same population [`Self::monitored_resource_count`] counts and
    /// `DescribeAccountHealth` reports as `AnalyzedResourceCount` — one entry per
    /// covered stack name, then one per `(app-boundary key, tag value)` pair — so the
    /// list a client walks and the count it is told cannot disagree. Both halves are
    /// sets, so the order is the store's stable one, which is what a page cursor needs.
    ///
    /// Each entry carries:
    ///
    /// * `ResourceCollection` — the one coverage entry this resource IS, so the
    ///   identity of every reported resource is exact even when its name is not a
    ///   well-formed [`monitored_resource_name`].
    /// * `Type` — [`MONITORED_STACK_RESOURCE_TYPE`] or [`MONITORED_TAG_RESOURCE_TYPE`],
    ///   the same value a cost estimation over the same entry reports.
    /// * `ResourcePermission` — [`RESOURCE_PERMISSION_FULL`]: nothing in this store
    ///   records DevOps Guru being denied access to a covered entry.
    /// * no `LastUpdated`. `UpdateResourceCollection` stores coverage as SET
    ///   MEMBERSHIP, and this same shape also records what an insight is ABOUT, where
    ///   a per-entry instant means nothing — so the store holds no instant at which an
    ///   entry was last updated and a read omits the optional member rather than
    ///   inventing one, exactly as `ServiceHealth.AnalyzedResourceCount` is
    ///   omitted in [`service_healths`].
    ///
    /// [`service_healths`]: ComAmazonawsDevopsguruCapstonecontrolplaneserviceState::service_healths
    pub fn monitored_resources(&self) -> Vec<MonitoredResourceIdentifier> {
        let stacks = self
            .cloudformation_stack_names
            .iter()
            .map(|stack_name| MonitoredResourceIdentifier {
                monitored_resource_name: monitored_resource_name(stack_name),
                r#type: Some(MONITORED_STACK_RESOURCE_TYPE.to_string()),
                resource_permission: Some(RESOURCE_PERMISSION_FULL.to_string()),
                last_updated: None,
                resource_collection: Some(ResourceCollection {
                    cloud_formation: Some(CloudFormationCollection {
                        stack_names: Some(vec![stack_name.clone()]),
                    }),
                    tags: None,
                }),
            });
        let tagged = self
            .tag_collections
            .iter()
            .flat_map(|(app_boundary_key, tag_values)| {
                tag_values
                    .iter()
                    .map(move |tag_value| MonitoredResourceIdentifier {
                        monitored_resource_name: monitored_resource_name(tag_value),
                        r#type: Some(MONITORED_TAG_RESOURCE_TYPE.to_string()),
                        resource_permission: Some(RESOURCE_PERMISSION_FULL.to_string()),
                        last_updated: None,
                        resource_collection: Some(ResourceCollection {
                            cloud_formation: None,
                            tags: Some(vec![TagCollection {
                                app_boundary_key: app_boundary_key.clone(),
                                tag_values: vec![tag_value.clone()],
                            }]),
                        }),
                    })
            });
        stacks.chain(tagged).collect()
    }
}

/// The `MonitoredResourceIdentifier.Type` of the monitored resource one covered
/// CloudFormation stack is.
///
/// The SAME value a cost estimation reports for that coverage entry
/// ([`COST_ESTIMATION_STACK_TYPE`]), so two reads of one store cannot disagree about
/// what a covered stack is.
pub const MONITORED_STACK_RESOURCE_TYPE: &str = COST_ESTIMATION_STACK_TYPE;

/// The `MonitoredResourceIdentifier.Type` of the monitored resource one covered
/// `(AppBoundaryKey, TagValue)` pair is — [`COST_ESTIMATION_TAG_TYPE`], for the reason
/// [`MONITORED_STACK_RESOURCE_TYPE`] gives.
pub const MONITORED_TAG_RESOURCE_TYPE: &str = COST_ESTIMATION_TAG_TYPE;

/// `ResourcePermission` as the model spells "DevOps Guru may access this resource" —
/// see `RESOURCE_PERMISSION_VALUES` in `ops/types/shared_types.rs`, whose other value,
/// `MISSING_PERMISSION`, says the opposite.
///
/// A coverage entry is a boundary the client itself declared and that
/// `UpdateResourceCollection` accepted, and no operation of this service records a
/// permission DevOps Guru lacks, so every monitored resource in this store is
/// accessible to it.
pub const RESOURCE_PERMISSION_FULL: &str = "FULL_PERMISSION";

/// The length a `MonitoredResourceName` may not exceed, from
/// `smithy_query(query="shape",shape="MonitoredResourceName")`: length 1..512.
const MAX_MONITORED_RESOURCE_NAME: usize = 512;

/// `candidate` as a `MonitoredResourceName`, or `None` when it is not one.
///
/// `MonitoredResourceName` is length 1..512 matching `^[\.\-_\/#A-Za-z0-9]+$`
/// (`smithy_query(query="shape",shape="MonitoredResourceName")`), while the value a
/// coverage entry is named by is a `StackName` or a `TagValue`, whose own patterns
/// admit characters that one does not — a `*` wildcard in a stack name; a space, `=`,
/// `:`, `+` or `@` in a tag value, or the empty tag value. Such an entry is reported
/// with its identity in `ResourceCollection` and NO `MonitoredResourceName`, rather
/// than with a name the model refuses.
pub fn monitored_resource_name(candidate: &str) -> Option<String> {
    let length = candidate.chars().count();
    let well_formed = (1..=MAX_MONITORED_RESOURCE_NAME).contains(&length)
        && candidate.chars().all(|c| {
            matches!(c, '.' | '-' | '_' | '/' | '#') || c.is_ascii_alphanumeric()
        });
    well_formed.then(|| candidate.to_string())
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

/// `CostEstimationServiceResourceState` for a resource a cost estimation's collection
/// names — see `COST_ESTIMATION_SERVICE_RESOURCE_STATE_VALUES` in
/// `ops/types/shared_types.rs`.
///
/// A resource is `ACTIVE` when it produces metrics, events or logs, and only active
/// resource hours are charged. The resources a submitted collection names are exactly
/// what the estimate is being made for, and this emulator has no metric stream that
/// could show one quiet, so a named resource is counted active.
pub const COST_ESTIMATION_RESOURCE_ACTIVE: &str = "ACTIVE";

/// The `ServiceResourceCost.Type` for the CloudFormation half of a cost estimation's
/// collection.
///
/// A cost estimation prices the collection the request submits, not the account's
/// analysis coverage (that is the whole point of `StartCostEstimation` taking its own
/// filter), and this emulator performs none of AWS's own per-service resource
/// discovery — so what it can count for a stack-defined boundary is the stacks the
/// request names.
pub const COST_ESTIMATION_STACK_TYPE: &str = "AWS::CloudFormation::Stack";

/// The `ServiceResourceCost.Type` for the tag half of a cost estimation's collection:
/// one app-boundary tag value the request names. See [`COST_ESTIMATION_STACK_TYPE`]
/// for why the unit counted is a submitted boundary entry.
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
