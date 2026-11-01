// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rolesanywhere-rolesanywhere service.
//!
//! The trust-anchor, profile and certificate-revocation-list lifecycles
//! (`CreateTrustAnchor`, `GetTrustAnchor`, `ListTrustAnchors`, `DeleteTrustAnchor`,
//! `CreateProfile`, `GetProfile`, `ListProfiles`, `DeleteProfile`, `ImportCrl`,
//! `GetCrl`, `ListCrls`) are backed entirely by the maps below, so a write is visible to the next
//! request on the same engine. The record types and their projections live here rather
//! than in a handler because several operations reach each of them.
//!
//! Every one of those resources is taggable, and each keeps its own tag list on its
//! record, so the ARN-keyed tag operations (`TagResource`, `ListTagsForResource`) resolve
//! an ARN against the records themselves rather than against a second tag table that
//! could disagree with them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AttributeMapping, CrlDetail, NotificationSetting, NotificationSettingDetail, ProfileDetail,
    Source, Tag, TrustAnchorDetail,
};

/// The ARN service namespace (`OPERATIONS.md`: "ARN namespace: `rolesanywhere`").
pub const ARN_NAMESPACE: &str = "rolesanywhere";

/// The region an unsigned request is attributed to, when no SigV4 credential scope
/// names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ProfileDetail$durationSeconds` when the create request omits it: "In requests, if
/// this value is not provided, the default value will be 3600."
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `durationSeconds`).
pub const DEFAULT_PROFILE_DURATION_SECONDS: i32 = 3600;

/// The largest page the `@paginated` list operations serve, and the page size a request
/// that omits `pageSize` gets.
///
/// `ListRequest$pageSize` carries no `@range` in the model, so this ceiling is this
/// emulator's own page bound rather than a published limit. It only bounds ONE
/// response; a caller that wants more pages the rest with `nextToken`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most trust anchors, the most profiles, and the most certificate revocation
/// lists this emulator holds.
///
/// Every store lives for the process's lifetime and grows only on a create, and none of
/// these resources can be given a TTL or evicted: a `GetTrustAnchor` / `GetProfile` /
/// `GetCrl` has to keep answering with what its create returned, so dropping a record
/// would be a divergence rather than a cleanup. A maximum entry count, refused on the create path,
/// is therefore the bound available here — the same construct this workspace's other
/// services use (`MAX_CHANNELS`, `MAX_STORED_CASES`). It sits far above any realistic
/// client workflow, so a caller never meets it by accident. None of these operations
/// models a quota error, so the refusal is their modelled `ValidationException`.
pub const MAX_TRUST_ANCHORS: usize = 10_000;
/// See [`MAX_TRUST_ANCHORS`].
pub const MAX_PROFILES: usize = 10_000;
/// See [`MAX_TRUST_ANCHORS`]. A CRL record also holds the imported `crlData`, whose own
/// size the modelled `Blob` `@length(1, 300000)` already caps per request, so this count
/// is what bounds the PRODUCT — the bytes every stored CRL can add up to.
pub const MAX_CRLS: usize = 1_000;

/// The most tags one resource holds at once.
///
/// `TagList` is `@length(0, 200)` both where a request carries tags and where a read
/// reports them (`smithy_query(query="shape", shape="TagList")`), so 200 is also the most
/// `ListTagsForResource` can answer with: a resource holding more could not be listed.
/// A `TagResource` that would carry a resource past it is refused with that operation's
/// modelled `TooManyTagsException` instead of storing tags no read could report.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The `sourceType` values a trust anchor can carry whose certificate data member the
/// model names: `x509CertificateData` is "Included for trust anchors of type
/// `CERTIFICATE_BUNDLE`" and `acmPcaArn` "Included for trust anchors of type
/// `AWS_ACM_PCA`" (`smithy_query(query="shape", shape="SourceData")`).
pub const CERTIFICATE_BUNDLE_SOURCE_TYPE: &str = "CERTIFICATE_BUNDLE";
/// See [`CERTIFICATE_BUNDLE_SOURCE_TYPE`].
pub const ACM_PCA_SOURCE_TYPE: &str = "AWS_ACM_PCA";

/// One stored trust anchor. Every `TrustAnchorDetail` member this service can know
/// about is persisted here; [`Self::to_detail`] is the only place a response is built
/// from it.
#[derive(Debug, Clone)]
pub struct TrustAnchorRecord {
    pub trust_anchor_id: String,
    pub trust_anchor_arn: String,
    pub name: String,
    pub source: Source,
    pub enabled: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub notification_settings: Vec<NotificationSettingDetail>,
    pub tags: Vec<Tag>,
}

impl TrustAnchorRecord {
    /// The `TrustAnchorDetail` every trust-anchor operation returns.
    pub fn to_detail(&self) -> TrustAnchorDetail {
        TrustAnchorDetail {
            trust_anchor_id: Some(self.trust_anchor_id.clone()),
            trust_anchor_arn: Some(self.trust_anchor_arn.clone()),
            name: Some(self.name.clone()),
            source: Some(self.source.clone()),
            enabled: Some(self.enabled),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            notification_settings: Some(self.notification_settings.clone()),
        }
    }
}

/// One stored certificate revocation list. Every `CrlDetail` member this service can
/// know about is persisted here; [`Self::to_detail`] is the only place a response is
/// built from it.
#[derive(Debug, Clone)]
pub struct CrlRecord {
    pub crl_id: String,
    pub crl_arn: String,
    pub name: String,
    pub enabled: bool,
    /// The PEM bytes the import (or a later update) carried, stored verbatim: `crlData`
    /// is "The state of the certificate revocation list (CRL) after a read or write
    /// operation" (`smithy_query(query="shape", shape="CrlDetail")`), so a read returns
    /// exactly the bytes that were written.
    pub crl_data: Vec<u8>,
    pub trust_anchor_arn: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Tags the import carried. `CrlDetail` models no tag member, so they are reachable
    /// only through `ListTagsForResource`.
    pub tags: Vec<Tag>,
}

impl CrlRecord {
    /// The `CrlDetail` every CRL operation returns.
    pub fn to_detail(&self) -> CrlDetail {
        CrlDetail {
            crl_id: Some(self.crl_id.clone()),
            crl_arn: Some(self.crl_arn.clone()),
            name: Some(self.name.clone()),
            enabled: Some(self.enabled),
            crl_data: Some(self.crl_data.clone()),
            trust_anchor_arn: Some(self.trust_anchor_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// One stored profile. Every `ProfileDetail` member this service can know about is
/// persisted here; [`Self::to_detail`] is the only place a response is built from it.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub profile_id: String,
    pub profile_arn: String,
    pub name: String,
    pub role_arns: Vec<String>,
    pub managed_policy_arns: Vec<String>,
    pub session_policy: Option<String>,
    pub duration_seconds: i32,
    pub enabled: bool,
    pub require_instance_properties: bool,
    pub accept_role_session_name: bool,
    pub created_by: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Certificate-field mappings applied to the authenticating end-entity certificate.
    /// `PutAttributeMapping` / `DeleteAttributeMapping` are still stubs (each tracked by
    /// its own gap issue), so nothing can add one yet and the list stays as created —
    /// empty — rather than being canned.
    pub attribute_mappings: Vec<AttributeMapping>,
    pub tags: Vec<Tag>,
}

impl ProfileRecord {
    /// The `ProfileDetail` every profile operation returns.
    pub fn to_detail(&self) -> ProfileDetail {
        ProfileDetail {
            profile_id: Some(self.profile_id.clone()),
            profile_arn: Some(self.profile_arn.clone()),
            name: Some(self.name.clone()),
            require_instance_properties: Some(self.require_instance_properties),
            enabled: Some(self.enabled),
            created_by: Some(self.created_by.clone()),
            session_policy: self.session_policy.clone(),
            role_arns: Some(self.role_arns.clone()),
            managed_policy_arns: Some(self.managed_policy_arns.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            duration_seconds: Some(self.duration_seconds),
            accept_role_session_name: Some(self.accept_role_session_name),
            attribute_mappings: Some(self.attribute_mappings.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-rolesanywhere-rolesanywhere. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` keyed by the resource's `Uuid`, so iteration order is a
/// stable total order and the `@paginated` list operations can resume from an opaque
/// cursor without keeping a server-side token table.
#[derive(Debug, Default)]
pub struct ComAmazonawsRolesanywhereRolesanywhereState {
    /// Every stored trust anchor, keyed by its `trustAnchorId`.
    pub trust_anchors: BTreeMap<String, TrustAnchorRecord>,
    /// Every stored profile, keyed by its `profileId`.
    pub profiles: BTreeMap<String, ProfileRecord>,
    /// Every stored certificate revocation list, keyed by its `crlId`.
    pub crls: BTreeMap<String, CrlRecord>,
    /// Monotonic input to [`Self::mint_uuid`]; never returned to a caller.
    id_counter: u64,
}

impl ComAmazonawsRolesanywhereRolesanywhereState {
    /// A fresh resource id in the modelled `Uuid` form
    /// (`@length(36, 36)` + `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`),
    /// unused by any stored trust anchor, profile or certificate revocation list.
    ///
    /// Derived from a per-store counter and the wall clock — never from the request —
    /// so two identical requests still get distinct ids. Every character is lowercase
    /// hex, which satisfies both character classes in the pattern.
    pub fn mint_uuid(&mut self) -> String {
        loop {
            self.id_counter = self.id_counter.wrapping_add(1);
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0u128, |d| d.as_nanos());
            let mut hasher = Sha256::new();
            hasher.update(self.id_counter.to_le_bytes());
            hasher.update(nanos.to_le_bytes());
            let hex = hex::encode(&hasher.finalize()[..16]);
            let id = format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            );
            if !self.trust_anchors.contains_key(&id)
                && !self.profiles.contains_key(&id)
                && !self.crls.contains_key(&id)
            {
                return id;
            }
        }
    }

    /// The tags attached to the resource this ARN names, or `None` when no stored trust
    /// anchor, profile or certificate revocation list carries that ARN.
    ///
    /// The ARN is matched against the one every record was minted with, so an ARN is
    /// resolvable exactly as long as the resource it names exists.
    pub fn tags(&self, resource_arn: &str) -> Option<&Vec<Tag>> {
        self.trust_anchors
            .values()
            .find(|record| record.trust_anchor_arn == resource_arn)
            .map(|record| &record.tags)
            .or_else(|| {
                self.profiles
                    .values()
                    .find(|record| record.profile_arn == resource_arn)
                    .map(|record| &record.tags)
            })
            .or_else(|| {
                self.crls
                    .values()
                    .find(|record| record.crl_arn == resource_arn)
                    .map(|record| &record.tags)
            })
    }

    /// The tags attached to the resource this ARN names, for writing. See [`Self::tags`].
    pub fn tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(record) = self
            .trust_anchors
            .values_mut()
            .find(|record| record.trust_anchor_arn == resource_arn)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .profiles
            .values_mut()
            .find(|record| record.profile_arn == resource_arn)
        {
            return Some(&mut record.tags);
        }
        self.crls
            .values_mut()
            .find(|record| record.crl_arn == resource_arn)
            .map(|record| &mut record.tags)
    }
}

/// Merge the tags a request carried into the tags a resource already holds, keyed by tag
/// key: a key the resource already carries takes the submitted value, a key it does not
/// carry is added, and a key the request does not name keeps the value it had. So one
/// tagging call never drops another's tags, and re-tagging a key replaces its value in
/// place rather than attaching the key twice.
pub fn merge_tags(held: &mut Vec<Tag>, incoming: Vec<Tag>) {
    for tag in incoming {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// How many tags the resource would hold after [`merge_tags`]: the number of distinct
/// keys across what it holds and what the request names, because a merge replaces a
/// value in place rather than adding a second entry for the same key.
pub fn merged_tag_count(held: &[Tag], incoming: &[Tag]) -> usize {
    held.iter()
        .chain(incoming)
        .map(|tag| tag.key.as_str())
        .collect::<BTreeSet<&str>>()
        .len()
}

/// One page of a `BTreeMap`-backed collection, plus the cursor to resume from.
///
/// `after` is start-EXCLUSIVE: the page begins just past the key the previous page
/// ended on, so across pages every resource is enumerated exactly once — no duplicates
/// and no skips, even when a resource is created or deleted between two pages. A
/// `nextToken` comes back only when entries remain beyond this page.
/// The work is O(`page_size`), not O(store): the map is walked through
/// [`BTreeMap::range`] from the cursor and at most ONE entry past the page is ever
/// looked at — that entry's existence is what says another page remains.
pub fn page<'a, T>(
    items: &'a BTreeMap<String, T>,
    after: Option<&str>,
    page_size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let lower = match after {
        Some(cursor) => ::std::ops::Bound::Excluded(cursor.to_string()),
        None => ::std::ops::Bound::Unbounded,
    };
    let window = items
        .range((lower, ::std::ops::Bound::Unbounded))
        .take(page_size.saturating_add(1));

    let mut entries: Vec<&'a T> = Vec::new();
    let mut last_key: Option<&String> = None;
    let mut more = false;
    for (index, (key, value)) in window.enumerate() {
        if index == page_size {
            more = true;
            break;
        }
        last_key = Some(key);
        entries.push(value);
    }

    let next_token = last_key
        .filter(|_| more)
        .map(|key| encode_next_token(key));
    (entries, next_token)
}

/// The page size a list request asks for: the modelled `pageSize`, or [`MAX_PAGE_SIZE`]
/// when it is absent, bounded above by [`MAX_PAGE_SIZE`] so one response is always a
/// bounded page whatever reaches the handler.
///
/// `None` when the request named a non-positive page, which is no page at all: the
/// caller is told so rather than being handed a one-entry page it did not ask for.
pub fn page_size(requested: Option<i32>) -> Option<usize> {
    let requested = requested.unwrap_or(MAX_PAGE_SIZE);
    if requested < 1 {
        return None;
    }
    usize::try_from(requested.min(MAX_PAGE_SIZE)).ok()
}

/// The wording a list operation refuses a non-positive `pageSize` with.
pub fn non_positive_page_size_message() -> String {
    validation_message(
        "pageSize",
        "Member must have value greater than or equal to 1",
    )
}

/// Encode a list cursor as an opaque `nextToken`.
///
/// Hex of the last key this page returned: it satisfies the modelled `nextToken`
/// `@length(1, 10000)`, is opaque to the caller, and carries the whole cursor — so
/// paging needs no server-side token table, which matters because a `@readonly` list
/// operation must not write to state.
pub fn encode_next_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a `nextToken` points just past, or `None` when the token is not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The `message` a `ValidationException` carries for one member that failed one
/// constraint, worded the way the boundary validator words its own refusals. Shared so
/// every operation words a refusal identically.
pub fn validation_message(member: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: {constraint}"
    )
}

/// The `message` a `ResourceNotFoundException` carries when the resource a request names
/// by ARN is not stored: the fixed wording this service words an ARN-keyed miss with,
/// whose only variable part is the ARN the request carried.
pub fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// The ARN of the certificate revocation list this id names, in the Region the request
/// was signed for: the ARN its import minted, so a refusal naming a CRL that is not
/// stored words the SAME ARN the read of a stored one answers with.
pub fn crl_arn(region: &str, crl_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("crl/{crl_id}"))
}

/// The trust-anchor id a `TrustAnchorArn` names, or the empty string when the ARN carries
/// no id.
///
/// `TrustAnchorArn` is `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$")`
/// (`smithy_query(query="operation", operation="ImportCrl")`), so the resource — everything
/// past the fifth colon of `arn:<partition>:rolesanywhere:<region>:<account>:<resource>` —
/// starts with `trust-anchor` and names the id after a separator. Parsing is total: a
/// resource part that is not `trust-anchor` plus a separator plus an id yields the empty
/// string, which no trust anchor can be stored under.
pub fn trust_anchor_id_in_arn(trust_anchor_arn: &str) -> &str {
    let resource = trust_anchor_arn.splitn(6, ':').nth(5).unwrap_or_default();
    resource
        .strip_prefix("trust-anchor/")
        .or_else(|| resource.strip_prefix("trust-anchor:"))
        .unwrap_or_default()
}

/// The `message` a refusal carries when a request references a trust anchor that is not
/// stored: the fixed wording, whose only variable part is the id taken from the ARN the
/// request carried.
pub fn trust_anchor_not_found_message(trust_anchor_id: &str) -> String {
    format!("Unable to find Trust Anchor with ID {trust_anchor_id}.")
}

/// The region this request was signed for, from its SigV4 credential scope; an
/// unsigned request is attributed to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Project the `NotificationSetting`s a create request carried into the
/// `NotificationSettingDetail`s a read returns, stamping `configuredBy` with the
/// account that configured them — "for customized notifications settings, it is the
/// respective account ID" (`NotificationSettingDetail$configuredBy` doc).
pub fn notification_details(settings: Vec<NotificationSetting>) -> Vec<NotificationSettingDetail> {
    settings
        .into_iter()
        .map(|setting| NotificationSettingDetail {
            enabled: setting.enabled,
            event: setting.event,
            threshold: setting.threshold,
            channel: setting.channel,
            configured_by: Some(::engine::emulator_account_id().to_string()),
        })
        .collect()
}

/// The failure a handler answers when this service's state cannot be reached — i.e.
/// the emulator was built on an engine type this service cannot use, so the operation
/// cannot be served at all. Unreachable in the assembled emulator, which builds exactly
/// one `InMemoryEngine`; never answer it by constructing a private engine, whose writes
/// no later request can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator was built on an engine this service cannot reach its state \
                  through, so no IAM Roles Anywhere operation can be served"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRolesanywhereRolesanywhereState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRolesanywhereRolesanywhereState>)
}
