// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devopsguru-capstonecontrolplaneservice service.
//!
//! Everything a DevOps Guru client can itself create lives here: the analysis
//! coverage it declares (`UpdateResourceCollection`), the notification channels it
//! adds (`AddNotificationChannel`), and the service-integration configuration it
//! sets (`UpdateServiceIntegration`). Insights and anomalies are produced by AWS
//! analysis rather than by the client, so they are not part of this store.

use std::collections::{BTreeMap, BTreeSet};

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
}

/// The set of resources DevOps Guru analyses for this account and region.
///
/// Both halves are SETS, not lists: `UpdateResourceCollection` is an
/// add/remove of coverage, so adding the same stack name or tag value twice
/// leaves the coverage unchanged and `GetResourceCollection` reports each one
/// once, in a stable order.
#[derive(Debug, Default)]
pub struct ResourceCoverage {
    /// `UpdateCloudFormationCollectionFilter.StackNames`, reported as
    /// `ResourceCollectionFilter.CloudFormation.StackNames`.
    pub cloudformation_stack_names: BTreeSet<String>,
    /// `UpdateTagCollectionFilter.{AppBoundaryKey,TagValues}`, reported as
    /// `ResourceCollectionFilter.Tags`: one entry per app-boundary key, each
    /// holding that key's tag values.
    pub tag_collections: BTreeMap<String, BTreeSet<String>>,
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
    let offset = u32::from_str_radix(offset, 16).ok()? as usize;
    offset.is_multiple_of(PAGE_SIZE).then_some(offset)
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
    if offset > items.len() {
        return None;
    }
    let end = (offset + PAGE_SIZE).min(items.len());
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
