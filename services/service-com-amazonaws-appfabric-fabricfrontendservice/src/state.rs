// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appfabric-fabricfrontendservice service.
//!
//! AppFabric's resources form one chain: an **app bundle** owns its
//! **ingestions**, and an ingestion owns its **ingestion destinations**. The
//! store mirrors that chain, so a child is reachable only through its parent and
//! deleting a parent takes its children with it.
//!
//! Every record carries the fields the modelled response shapes need, and the
//! `to_*` methods below are the ONLY way a handler builds a response: a response
//! is therefore always read back out of what a create call wrote.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    AppBundle, AppBundleSummary, ConflictException, Destination, DestinationConfiguration,
    Ingestion, IngestionDestination, IngestionSummary, ProcessingConfiguration,
    ResourceNotFoundException, ServiceQuotaExceededException, Tag, ValidationException,
    ValidationExceptionField,
};

/// The ARN namespace (and sigv4 signing name) this service's ARNs carry, from
/// the model's `aws.api#service` trait (`arnNamespace: appfabric`).
pub const ARN_SERVICE: &str = "appfabric";

/// The region an ARN is built in when the request carries no SigV4 credential
/// scope to read one from — the region every generated emulator defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page any `List*` operation returns, which is also the upper bound
/// the model puts on `maxResults` (`@range(min: 1, max: 100)`), and the page size
/// used when a caller names none.
pub const MAX_PAGE_SIZE: usize = 100;

/// The emulator's ceiling on stored app bundles.
///
/// A RESOURCE GUARD, not an AWS-published quota: the store lives in the
/// emulator's process, so a caller must not be able to grow it without bound.
/// Past the ceiling `CreateAppBundle` refuses with the `ServiceQuotaExceededException`
/// it models, which is how a real account at its limit answers.
pub const MAX_APP_BUNDLES: usize = 1_000;

/// The same guard for the ingestions one app bundle may hold.
pub const MAX_INGESTIONS_PER_APP_BUNDLE: usize = 1_000;

/// The same guard for the ingestion destinations one ingestion may hold.
pub const MAX_DESTINATIONS_PER_INGESTION: usize = 1_000;

/// The number of tags ONE resource may hold.
///
/// Not a resource guard but the model's own bound: a `TagList` is
/// `@length(min: 0, max: 50)`, and `ListTagsForResource` reports a resource's
/// whole tag set as one `TagList` — a resource holding more could not be
/// reported, so `TagResource` refuses a merge that would take it past this.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The same guard for remembered idempotency tokens. A token is caller-chosen, so
/// this map would otherwise grow for the life of the process with one entry per
/// distinct token; past the ceiling [`ComAmazonawsAppfabricFabricfrontendserviceState::remember_client_token`]
/// evicts the oldest entry, which is the one least likely to still be retried.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// An app bundle, as `CreateAppBundle` stored it.
#[derive(Debug, Clone)]
pub struct AppBundleRecord {
    /// The bundle's bare UUID — the key it is stored under, and one of the two
    /// forms `appBundleIdentifier` accepts.
    pub id: String,
    pub arn: String,
    pub customer_managed_key_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// Creation order, so a listing is stable and a page token can name a
    /// position that survives a deletion.
    pub sequence: u64,
    pub ingestions: HashMap<String, IngestionRecord>,
}

impl AppBundleRecord {
    /// The modelled `AppBundle` for this record.
    pub fn to_app_bundle(&self) -> AppBundle {
        AppBundle {
            arn: self.arn.clone(),
            customer_managed_key_arn: self.customer_managed_key_arn.clone(),
        }
    }

    /// The modelled `AppBundleSummary` for this record.
    pub fn to_summary(&self) -> AppBundleSummary {
        AppBundleSummary {
            arn: self.arn.clone(),
        }
    }

    /// The ingestion `identifier` names within this bundle, by ARN or bare UUID.
    pub fn resolve_ingestion(&self, identifier: &str) -> Option<&IngestionRecord> {
        match ingestion_key(self, identifier) {
            Some(key) => self.ingestions.get(&key),
            None => None,
        }
    }

    /// The ingestion `identifier` names within this bundle, for mutation.
    pub fn resolve_ingestion_mut(&mut self, identifier: &str) -> Option<&mut IngestionRecord> {
        match ingestion_key(self, identifier) {
            Some(key) => self.ingestions.get_mut(&key),
            None => None,
        }
    }

    /// This bundle's ingestions in creation order.
    pub fn ingestions_in_order(&self) -> Vec<&IngestionRecord> {
        let mut ingestions: Vec<&IngestionRecord> = self.ingestions.values().collect();
        ingestions.sort_by_key(|ingestion| ingestion.sequence);
        ingestions
    }
}

/// An ingestion, as `CreateIngestion` stored it.
#[derive(Debug, Clone)]
pub struct IngestionRecord {
    pub id: String,
    pub arn: String,
    pub app_bundle_arn: String,
    pub app: String,
    pub tenant_id: String,
    pub ingestion_type: String,
    pub state: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
    pub sequence: u64,
    /// This ingestion's destinations, keyed by bare UUID:
    /// `CreateIngestionDestination` writes them and
    /// `UpdateIngestionDestination` rewrites their configuration in place.
    pub destinations: HashMap<String, IngestionDestinationRecord>,
}

impl IngestionRecord {
    /// The modelled `Ingestion` for this record.
    pub fn to_ingestion(&self) -> Ingestion {
        Ingestion {
            arn: self.arn.clone(),
            app_bundle_arn: self.app_bundle_arn.clone(),
            app: self.app.clone(),
            tenant_id: self.tenant_id.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            state: self.state.clone(),
            ingestion_type: self.ingestion_type.clone(),
        }
    }

    /// The modelled `IngestionSummary` for this record.
    pub fn to_summary(&self) -> IngestionSummary {
        IngestionSummary {
            arn: self.arn.clone(),
            app: self.app.clone(),
            tenant_id: self.tenant_id.clone(),
            state: self.state.clone(),
        }
    }

    /// The ingestion destination `identifier` names within this ingestion, by ARN
    /// or bare UUID.
    pub fn resolve_destination(&self, identifier: &str) -> Option<&IngestionDestinationRecord> {
        match destination_key(self, identifier) {
            Some(key) => self.destinations.get(&key),
            None => None,
        }
    }

    /// The ingestion destination `identifier` names within this ingestion, for
    /// mutation.
    pub fn resolve_destination_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut IngestionDestinationRecord> {
        match destination_key(self, identifier) {
            Some(key) => self.destinations.get_mut(&key),
            None => None,
        }
    }

    /// This ingestion's destinations in creation order.
    pub fn destinations_in_order(&self) -> Vec<&IngestionDestinationRecord> {
        let mut destinations: Vec<&IngestionDestinationRecord> =
            self.destinations.values().collect();
        destinations.sort_by_key(|destination| destination.sequence);
        destinations
    }
}

/// An ingestion destination, as `CreateIngestionDestination` stored it and
/// `UpdateIngestionDestination` last wrote it.
#[derive(Debug, Clone)]
pub struct IngestionDestinationRecord {
    pub id: String,
    pub arn: String,
    pub ingestion_arn: String,
    pub processing_configuration: ProcessingConfiguration,
    pub destination_configuration: DestinationConfiguration,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
    pub sequence: u64,
}

impl IngestionDestinationRecord {
    /// The modelled `IngestionDestination` for this record.
    pub fn to_ingestion_destination(&self) -> IngestionDestination {
        IngestionDestination {
            arn: self.arn.clone(),
            ingestion_arn: self.ingestion_arn.clone(),
            processing_configuration: self.processing_configuration.clone(),
            destination_configuration: self.destination_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// What one spent idempotency token created, so replaying the token replays the
/// resource instead of creating a second one.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The ARN of the resource the first call created.
    pub arn: String,
    /// A canonical rendering of that call's parameters — see
    /// [`request_fingerprint`].
    pub fingerprint: String,
    /// When the token was spent, so the oldest can be evicted first once
    /// [`MAX_CLIENT_TOKENS`] is reached.
    pub sequence: u64,
}

/// Per-service state for com-amazonaws-appfabric-fabricfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppfabricFabricfrontendserviceState {
    /// App bundles, keyed by bare UUID.
    pub app_bundles: HashMap<String, AppBundleRecord>,
    /// Spent idempotency tokens, keyed by [`client_token_key`].
    pub client_tokens: HashMap<String, ClientTokenRecord>,
    /// The source of every record's `sequence`: monotonic across the whole
    /// store, so creation order is recoverable without an insertion-ordered map.
    next_sequence: u64,
}

impl ComAmazonawsAppfabricFabricfrontendserviceState {
    /// The next creation sequence. Never reused, so a page token that names a
    /// sequence keeps naming the same position for the life of the store.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The app bundle `identifier` names, by ARN or bare UUID.
    pub fn resolve_bundle(&self, identifier: &str) -> Option<&AppBundleRecord> {
        self.bundle_key(identifier)
            .and_then(|key| self.app_bundles.get(&key))
    }

    /// The app bundle `identifier` names, for mutation.
    pub fn resolve_bundle_mut(&mut self, identifier: &str) -> Option<&mut AppBundleRecord> {
        match self.bundle_key(identifier) {
            Some(key) => self.app_bundles.get_mut(&key),
            None => None,
        }
    }

    /// The store key of the app bundle `identifier` names.
    ///
    /// The model's `Identifier` shape accepts either form
    /// (`^arn:.+$|^[a-f0-9]{8}-…$`), and both name the same resource: an ARN
    /// matches the record that owns it, anything else is the bare UUID key.
    pub fn bundle_key(&self, identifier: &str) -> Option<String> {
        if is_arn(identifier) {
            self.app_bundles
                .values()
                .find(|bundle| bundle.arn == identifier)
                .map(|bundle| bundle.id.clone())
        } else {
            self.app_bundles
                .contains_key(identifier)
                .then(|| identifier.to_string())
        }
    }

    /// Remember what one spent idempotency token created.
    ///
    /// Bounded: at [`MAX_CLIENT_TOKENS`] the oldest entry is evicted to make room,
    /// so the map cannot grow with the number of distinct tokens a caller sends.
    /// An evicted token simply stops replaying — it behaves like a token never
    /// seen, which is the safe direction for a bound to fail in.
    pub fn remember_client_token(&mut self, key: String, arn: String, fingerprint: String) {
        if self.client_tokens.len() >= MAX_CLIENT_TOKENS && !self.client_tokens.contains_key(&key) {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, spent)| spent.sequence)
                .map(|(token, _)| token.clone());
            if let Some(oldest) = oldest {
                self.client_tokens.remove(&oldest);
            }
        }
        let sequence = self.next_sequence();
        self.client_tokens.insert(
            key,
            ClientTokenRecord {
                arn,
                fingerprint,
                sequence,
            },
        );
    }

    /// App bundles in creation order.
    pub fn bundles_in_order(&self) -> Vec<&AppBundleRecord> {
        let mut bundles: Vec<&AppBundleRecord> = self.app_bundles.values().collect();
        bundles.sort_by_key(|bundle| bundle.sequence);
        bundles
    }

    /// Where the resource an `Arn` names lives in the store, or `None` when no
    /// record carries that ARN.
    ///
    /// The resource-tagging operations take an ARN rather than a typed
    /// identifier, so this is the one place that decides which record a
    /// `resourceArn` denotes: every record this service persists carries its own
    /// ARN, and the walk is over the ownership chain (bundle → ingestion →
    /// ingestion destination), so a resource becomes taggable exactly when the
    /// operation that mints it starts persisting it.
    pub fn locate_tagged(&self, arn: &str) -> Option<TagTarget> {
        for bundle in self.app_bundles.values() {
            if bundle.arn == arn {
                return Some(TagTarget::AppBundle {
                    bundle: bundle.id.clone(),
                });
            }
            for ingestion in bundle.ingestions.values() {
                if ingestion.arn == arn {
                    return Some(TagTarget::Ingestion {
                        bundle: bundle.id.clone(),
                        ingestion: ingestion.id.clone(),
                    });
                }
                for destination in ingestion.destinations.values() {
                    if destination.arn == arn {
                        return Some(TagTarget::IngestionDestination {
                            bundle: bundle.id.clone(),
                            ingestion: ingestion.id.clone(),
                            destination: destination.id.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    /// The tags a located resource holds, as the holding record stores them.
    pub fn tags_of(&self, target: &TagTarget) -> Option<&Vec<Tag>> {
        match target {
            TagTarget::AppBundle { bundle } => {
                self.app_bundles.get(bundle).map(|bundle| &bundle.tags)
            }
            TagTarget::Ingestion { bundle, ingestion } => self
                .app_bundles
                .get(bundle)?
                .ingestions
                .get(ingestion)
                .map(|ingestion| &ingestion.tags),
            TagTarget::IngestionDestination {
                bundle,
                ingestion,
                destination,
            } => self
                .app_bundles
                .get(bundle)?
                .ingestions
                .get(ingestion)?
                .destinations
                .get(destination)
                .map(|destination| &destination.tags),
        }
    }

    /// The same tag set, for mutation by `TagResource` / `UntagResource`.
    pub fn tags_of_mut(&mut self, target: &TagTarget) -> Option<&mut Vec<Tag>> {
        match target {
            TagTarget::AppBundle { bundle } => self
                .app_bundles
                .get_mut(bundle)
                .map(|bundle| &mut bundle.tags),
            TagTarget::Ingestion { bundle, ingestion } => self
                .app_bundles
                .get_mut(bundle)?
                .ingestions
                .get_mut(ingestion)
                .map(|ingestion| &mut ingestion.tags),
            TagTarget::IngestionDestination {
                bundle,
                ingestion,
                destination,
            } => self
                .app_bundles
                .get_mut(bundle)?
                .ingestions
                .get_mut(ingestion)?
                .destinations
                .get_mut(destination)
                .map(|destination| &mut destination.tags),
        }
    }
}

/// The resource a tagging call's `resourceArn` names: the chain of store keys
/// leading to the record that holds its tags.
///
/// Keys rather than a reference, so one immutable walk
/// ([`ComAmazonawsAppfabricFabricfrontendserviceState::locate_tagged`]) serves
/// both the reporting and the mutating tagging operations.
#[derive(Debug, Clone)]
pub enum TagTarget {
    AppBundle {
        bundle: String,
    },
    Ingestion {
        bundle: String,
        ingestion: String,
    },
    IngestionDestination {
        bundle: String,
        ingestion: String,
        destination: String,
    },
}

impl TagTarget {
    /// The resource's kind, as a modelled error and an audit record name it.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::AppBundle { .. } => "AppBundle",
            Self::Ingestion { .. } => "Ingestion",
            Self::IngestionDestination { .. } => "IngestionDestination",
        }
    }

    /// The resource's own bare id — the last key in the chain.
    pub fn id(&self) -> &str {
        match self {
            Self::AppBundle { bundle } => bundle,
            Self::Ingestion { ingestion, .. } => ingestion,
            Self::IngestionDestination { destination, .. } => destination,
        }
    }
}

/// Merge `incoming` into `tags` the way `TagResource` does: a tag list is a map
/// of key-value pairs, so a key already present takes the new value in place
/// (keeping the position it was first assigned at) and a new key is appended.
///
/// Returns the merged set rather than mutating, so a caller can refuse a merge
/// that would take the resource past [`MAX_TAGS_PER_RESOURCE`] without having
/// changed anything.
pub fn merge_tags(tags: &[Tag], incoming: &[Tag]) -> Vec<Tag> {
    let mut merged = tags.to_vec();
    for tag in incoming {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => merged.push(tag.clone()),
        }
    }
    merged
}

/// The store key of the ingestion `identifier` names inside `bundle`, by ARN or
/// bare UUID — the same two forms [`ComAmazonawsAppfabricFabricfrontendserviceState::bundle_key`] accepts.
pub fn ingestion_key(bundle: &AppBundleRecord, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        bundle
            .ingestions
            .values()
            .find(|ingestion| ingestion.arn == identifier)
            .map(|ingestion| ingestion.id.clone())
    } else {
        bundle
            .ingestions
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}

/// The store key of the ingestion destination `identifier` names inside
/// `ingestion`, by ARN or bare UUID — the same two forms every other
/// `Identifier` in this service accepts.
pub fn destination_key(ingestion: &IngestionRecord, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        ingestion
            .destinations
            .values()
            .find(|destination| destination.arn == identifier)
            .map(|destination| destination.id.clone())
    } else {
        ingestion
            .destinations
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}

/// Whether an `Identifier` is in its ARN form rather than its bare-UUID form.
/// The model's pattern allows exactly these two (`^arn:.+$` or a UUID).
fn is_arn(identifier: &str) -> bool {
    identifier.starts_with("arn:")
}

/// A fresh resource id: a version-4 UUID in the lowercase hyphenated form the
/// model's `UUID`/`Identifier` patterns require (RFC 9562 §5.4 — version `4` in
/// the high nibble of octet 6, variant `0b10` in the top bits of octet 8).
///
/// Its bits come from SHA-256 over three independent sources, so an id is both
/// unguessable and unique within the process:
///
/// * the OS entropy std seeds a fresh `RandomState` with on every call, whose
///   hash of the counter below is therefore unpredictable;
/// * a monotonic counter, which makes two ids minted in the same nanosecond on
///   different threads differ by construction;
/// * the wall clock in nanoseconds, so ids do not repeat across processes.
///
/// This takes NO new dependency: `sha2` is already in this crate's dependency
/// closure, and `Cargo.lock` is the one file every concurrent fix in this
/// repository touches, so leaving it untouched is what lets this slice land.
pub fn new_uuid() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: writing to a String never fails.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// The region to build this request's ARNs in: the one the caller signed for,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's ARN for `resource`, e.g. `appbundle/<uuid>`.
pub fn appfabric_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, resource)
}

/// The KMS key ARN a `customerManagedKeyIdentifier` denotes.
///
/// The member takes a key ARN *or* a bare key id, while the response member is
/// an ARN (`customerManagedKeyArn`), so a bare id is expanded into the key ARN
/// it names and an ARN is kept as given.
pub fn customer_managed_key_arn(identifier: &str, region: &str) -> String {
    if is_arn(identifier) {
        identifier.to_string()
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{identifier}"))
    }
}

/// The [`ComAmazonawsAppfabricFabricfrontendserviceState::client_tokens`] key for one operation's token.
///
/// Keyed by operation as well as token, because an idempotency token is scoped
/// to the operation it was passed to: the same token on `CreateAppBundle` and on
/// `CreateIngestion` is two independent requests.
pub fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// A canonical rendering of a create request's parameters, so a replayed
/// idempotency token carrying DIFFERENT parameters can be told from a true
/// retry (which replays the first call's resource).
///
/// Every component is LENGTH-PREFIXED rather than joined by a separator, so the
/// rendering is injective: no caller-chosen tag or tenant value can contain the
/// delimiter and make two different requests render alike (which would replay
/// the wrong resource, or refuse a true retry).
///
/// Tags are sorted, because a tag list is a set of key/value pairs: the same
/// tags in another order are the same request.
pub fn request_fingerprint(fields: &[(&str, &str)], tags: Option<&[Tag]>) -> String {
    let mut rendered = String::new();
    for (name, value) in fields {
        push_component(&mut rendered, name);
        push_component(&mut rendered, value);
    }
    let mut pairs: Vec<(&str, &str)> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    pairs.sort_unstable();
    // The tag count is part of the rendering too, so a field value cannot be
    // confused with the start of the tag section.
    push_component(&mut rendered, &pairs.len().to_string());
    for (key, value) in pairs {
        push_component(&mut rendered, key);
        push_component(&mut rendered, value);
    }
    rendered
}

/// Append one length-prefixed component of a [`request_fingerprint`].
fn push_component(rendered: &mut String, value: &str) {
    rendered.push_str(&value.len().to_string());
    rendered.push(':');
    rendered.push_str(value);
}

/// A canonical rendering of a `ProcessingConfiguration`, so it can go into a
/// [`request_fingerprint`] as one component: the union member that was set, then
/// that member's own values.
///
/// Every component is length-prefixed for the same reason the fingerprint's are:
/// no two different configurations may render alike.
pub fn render_processing_configuration(configuration: &ProcessingConfiguration) -> String {
    let mut rendered = String::new();
    match configuration {
        ProcessingConfiguration::auditLog(audit_log) => {
            push_component(&mut rendered, "auditLog");
            push_component(&mut rendered, &audit_log.schema);
            push_component(&mut rendered, &audit_log.format);
        }
    }
    rendered
}

/// The same canonical rendering for a `DestinationConfiguration`.
pub fn render_destination_configuration(configuration: &DestinationConfiguration) -> String {
    let mut rendered = String::new();
    match configuration {
        DestinationConfiguration::auditLog(audit_log) => {
            push_component(&mut rendered, "auditLog");
            match &audit_log.destination {
                Destination::s3Bucket(bucket) => {
                    push_component(&mut rendered, "s3Bucket");
                    push_component(&mut rendered, &bucket.bucket_name);
                    push_component(&mut rendered, bucket.prefix.as_deref().unwrap_or_default());
                }
                Destination::firehoseStream(stream) => {
                    push_component(&mut rendered, "firehoseStream");
                    push_component(&mut rendered, &stream.stream_name);
                }
            }
        }
    }
    rendered
}

/// One page of a listing, and the token that asks for the page after it.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page, so a caller that follows tokens stops.
    pub next_token: Option<String>,
}

/// Take one page out of `records`, which must be in creation order.
///
/// `maxResults` is an upper limit on the page (the model bounds it at
/// 1..=[`MAX_PAGE_SIZE`], and a caller that names none gets a full page), and
/// the token names the sequence of the last record already returned, so the next
/// page starts after it even when records in between were deleted.
/// `listing` names the listing the token belongs to (see [`encode_page_token`]),
/// so a token one listing issued is refused by another instead of silently
/// skipping records.
pub fn page<R, T>(
    records: &[&R],
    listing: &str,
    sequence_of: impl Fn(&R) -> u64,
    item_of: impl Fn(&R) -> T,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, ValidationException> {
    let after = match next_token {
        Some(token) => Some(decode_page_token(listing, token)?),
        None => None,
    };
    let limit = max_results.map_or(MAX_PAGE_SIZE, |max| {
        usize::try_from(max)
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE)
    });

    let remaining: Vec<&R> = records
        .iter()
        .copied()
        .filter(|record| after.is_none_or(|after| sequence_of(record) > after))
        .collect();
    let items: Vec<T> = remaining.iter().take(limit).map(|r| item_of(r)).collect();
    // A token is issued only when records are left over, and it names the last
    // record THIS page returned — so the next page starts right after it.
    let next_token = (remaining.len() > items.len())
        .then(|| items.len().saturating_sub(1))
        .and_then(|last| remaining.get(last))
        .map(|last| encode_page_token(listing, sequence_of(last)));

    Ok(Page { items, next_token })
}

/// The opaque `nextToken` naming the position after `sequence` WITHIN one
/// listing ([`APP_BUNDLE_LISTING`], or [`ingestion_listing`] for one bundle's
/// ingestions).
///
/// The listing is part of the token because a position is only meaningful inside
/// the listing that issued it: a token carried across to another listing — or to
/// another bundle's ingestions — would otherwise be read as a position there and
/// silently skip records. Decoding checks it, so such a token is refused.
///
/// The rendering is a version prefix, the listing and hex: every character is
/// URL-unreserved, so the token survives a round trip through a query string
/// whatever the client's encoder does with it (base64's `+` and `/` do not), and
/// it is well inside the 1..=2048 the model allows.
pub fn encode_page_token(listing: &str, sequence: u64) -> String {
    format!("v1-{listing}-{sequence:016x}")
}

/// The position a `nextToken` names in `listing`, or a `ValidationException`
/// naming the member when the token is not one this listing issued.
pub fn decode_page_token(listing: &str, token: &str) -> Result<u64, ValidationException> {
    token
        .strip_prefix(&format!("v1-{listing}-"))
        .and_then(|sequence| u64::from_str_radix(sequence, 16).ok())
        .ok_or_else(|| {
            field_validation_failed("nextToken", "nextToken is not a valid pagination token")
        })
}

/// The [`encode_page_token`] listing name of `ListAppBundles`.
pub const APP_BUNDLE_LISTING: &str = "appbundles";

/// The [`encode_page_token`] listing name of `ListIngestions` for ONE bundle:
/// the ingestions of two bundles are two listings, so a token from one is not a
/// position in the other.
pub fn ingestion_listing(bundle_id: &str) -> String {
    format!("ingestions-{bundle_id}")
}

/// A modelled `ValidationException` refusing one member by name.
pub fn field_validation_failed(member: &str, message: &str) -> ValidationException {
    ValidationException {
        message: message.to_string(),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
    }
}

/// A modelled `ResourceNotFoundException` for an app bundle the store does not hold.
pub fn app_bundle_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("App bundle {identifier} was not found."),
        resource_id: identifier.to_string(),
        resource_type: "AppBundle".to_string(),
    }
}

/// A modelled `ResourceNotFoundException` for an ingestion the store does not hold.
pub fn ingestion_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Ingestion {identifier} was not found."),
        resource_id: identifier.to_string(),
        resource_type: "Ingestion".to_string(),
    }
}

/// A modelled `ResourceNotFoundException` for an ingestion destination the store
/// does not hold — the REPORTING path's refusal (`GetIngestionDestination`,
/// `ListIngestionDestinations`), which AppFabric answers with HTTP 404.
pub fn ingestion_destination_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified ingestion destination does not exist".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "IngestionDestination".to_string(),
    }
}

/// A modelled `ValidationException` refusing a MUTATION of an ingestion
/// destination because the ingestion it is nested under does not exist.
///
/// AppFabric checks the ingestion-destination chain on its mutation path as
/// INPUT validation rather than as a lookup: a `*IngestionDestination` write
/// naming an ingestion that does not exist is answered HTTP 400
/// `ValidationException`, while the reporting path answers HTTP 404
/// (see [`ingestion_not_found`]).
pub fn ingestion_does_not_exist() -> ValidationException {
    field_validation_failed(
        "ingestionIdentifier",
        "The specified ingestion does not exist",
    )
}

/// The same refusal for the ingestion destination the mutation names.
pub fn ingestion_destination_does_not_exist() -> ValidationException {
    field_validation_failed(
        "ingestionDestinationIdentifier",
        "The specified ingestion destination does not exist",
    )
}

/// A modelled `ResourceNotFoundException` for a tagging call whose `resourceArn`
/// names no resource this store holds.
///
/// The ARN is the only identifier the caller gave, so it is also the
/// `resourceId`; the `resourceType` is read out of the ARN itself (see
/// [`arn_resource_type`]) because an ARN naming nothing has no record to read a
/// kind from.
pub fn tagged_resource_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Resource {arn} was not found."),
        resource_id: arn.to_string(),
        resource_type: arn_resource_type(arn).to_string(),
    }
}

/// The kind of resource an ARN names, from the LAST type segment of its
/// resource part: `…:appbundle/<id>/ingestion/<id>` names an ingestion.
///
/// The segment names are the ones this service's own ARNs are built from (see
/// `CreateAppBundle` and `CreateIngestion`); any other ARN — another service's,
/// or a malformed one — is reported as the generic `Resource`.
pub fn arn_resource_type(arn: &str) -> &'static str {
    let mut kind = "Resource";
    for segment in arn.split('/') {
        // The first segment carries the ARN head, whose last `:`-separated
        // piece is the resource type (`…:000000000000:appbundle`).
        match segment.split(':').next_back().unwrap_or(segment) {
            "appbundle" => kind = "AppBundle",
            "ingestion" => kind = "Ingestion",
            "ingestiondestination" => kind = "IngestionDestination",
            "appauthorization" => kind = "AppAuthorization",
            _ => {}
        }
    }
    kind
}

/// A modelled `ServiceQuotaExceededException` for a create call that would take
/// the store past one of the ceilings above.
pub fn quota_exceeded(
    resource_id: &str,
    resource_type: &str,
    quota_code: &str,
    limit: usize,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "The limit of {limit} {resource_type} resources for {resource_id} has been reached."
        ),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
        service_code: ARN_SERVICE.to_string(),
        quota_code: quota_code.to_string(),
    }
}

/// A modelled `ConflictException` for an idempotency token replayed with
/// different parameters.
///
/// AWS documents this case as an `IdempotentParameterMismatch` failure, a shape
/// this service's model does not declare; `ConflictException` ("the request has
/// created a conflict, check the request parameters and try again") is the
/// modelled error that states it.
pub fn idempotent_parameter_mismatch(
    client_token: &str,
    resource_type: &str,
) -> ConflictException {
    ConflictException {
        message: format!(
            "The clientToken {client_token} was already used with different request parameters."
        ),
        resource_id: client_token.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// Record one state-changing AppFabric operation: what the caller asked for, who
/// asked (the access key id the request presented), which server-minted resource
/// changed and how the call ended.
///
/// The one place this service writes an audit record, so every create and delete
/// records the same fields. Only wire metadata and server-minted identifiers go
/// in: never a bundle's tags, its customer-managed key or any credential
/// material.
pub fn audit_resource_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    resource_type: &'static str,
    resource_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        resource_type,
        resource_id,
        "appfabric resource changed"
    );
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppfabricFabricfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppfabricFabricfrontendserviceState>)
}
