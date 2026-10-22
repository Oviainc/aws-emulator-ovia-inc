// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devopsguru-capstonecontrolplaneservice service.
//!
//! Everything a DevOps Guru client can itself create lives here: the analysis
//! coverage it declares (`UpdateResourceCollection`), the notification channels it
//! adds (`AddNotificationChannel`), and the service-integration configuration it
//! sets (`UpdateServiceIntegration`).
//!
//! Insights are produced by DevOps Guru's own analysis, never by a client call, so
//! [`ComAmazonawsDevopsguruCapstonecontrolplaneserviceState::insights`] is the map
//! that analysis would write and the insight READS (`ListInsights` today) report:
//! this emulator runs no analysis, so the map stays empty for an account whose
//! insights nothing produced, and a read over it reports nothing rather than
//! inventing an insight.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    CloudFormationCollection, InsightTimeRange, PredictionTimeRange, ProactiveInsightSummary,
    ReactiveInsightSummary, ResourceCollection, ServiceCollection, TagCollection,
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
    /// The insights DevOps Guru's analysis produced for this account, keyed by
    /// `InsightId`.
    ///
    /// No modelled operation MINTS an insight — DevOps Guru derives one itself from
    /// anomalies in the covered resources — so this map is what an insight generator
    /// writes and what the insight reads walk. A `BTreeMap` because the reads page over
    /// it: the key order is stable, which is what a page cursor needs.
    pub insights: BTreeMap<String, InsightRecord>,
}

/// One insight, as the insight reads report it.
///
/// Holds every member `ProactiveInsightSummary` and `ReactiveInsightSummary` carry;
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
}

impl InsightRecord {
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
    /// holding that key's tag values.
    pub tag_collections: BTreeMap<String, BTreeSet<String>>,
}

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

/// `OptInStatus` as the model spells "off" — see `OPT_IN_STATUS_VALUES` in
/// `ops/types/shared_types.rs`.
pub const OPT_IN_STATUS_DISABLED: &str = "DISABLED";

/// The `ResourceType` a channel-shaped `ConflictException` or
/// `ResourceNotFoundException` reports. `ResourceIdType` is an unconstrained string,
/// so this value names the kind of resource the reported `ResourceId` belongs to.
pub const NOTIFICATION_CHANNEL_RESOURCE_TYPE: &str = "NOTIFICATION_CHANNEL";

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
// 1..2048. Nothing caller-sized reaches the store unchecked.
//
// The product is therefore finite: at most MAX_COVERED_STACK_NAMES stack names, at
// most MAX_COVERED_TAG_KEYS app-boundary keys each holding at most
// MAX_TAG_VALUES_PER_KEY values, at most MAX_NOTIFICATION_CHANNELS channels, and one
// service-integration record of fixed shape.
//
// `insights` carries no cap because no request can grow it: insights come from DevOps
// Guru's analysis, which this emulator does not run, so nothing a caller sends adds an
// entry. An operation that ever mints one brings its own bound.
// ---------------------------------------------------------------------------

/// DevOps Guru's own documented ceiling, from `UpdateResourceCollection`'s and
/// `CloudFormationCollectionFilter`'s model documentation: "You can specify up to 500
/// Amazon Web Services CloudFormation stacks."
pub const MAX_COVERED_STACK_NAMES: usize = 500;

/// Emulator resource guard, NOT a published AWS quota: the most app-boundary keys this
/// store holds. `UpdateTagCollectionFilters` declares no list length, so without this
/// one request could add arbitrarily many keys. Set well above any real use — DevOps
/// Guru's own guidance is a single app-boundary key per account.
pub const MAX_COVERED_TAG_KEYS: usize = 100;

/// Emulator resource guard, NOT a published AWS quota: the most tag values this store
/// holds for one app-boundary key. See [`MAX_COVERED_TAG_KEYS`].
pub const MAX_TAG_VALUES_PER_KEY: usize = 500;

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
    let end = (offset + page_size).min(items.len());
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
