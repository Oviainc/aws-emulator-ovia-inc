// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmanager-networkmanager service.
//!
//! The global-network / site / link lifecycle is the core resource tree of Network
//! Manager: a global network owns sites, a site owns links (and, in a later slice,
//! devices). Everything
//! the implemented operations answer is computed from the records below — no handler
//! echoes its input back without storing it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Bandwidth, GlobalNetwork, Link, Location, ResourceNotFoundException, Site, Tag,
    ValidationException, ValidationExceptionField,
};

/// Members of `GlobalNetworkState` / `SiteState` / `LinkState` this slice uses. All three
/// enums carry the same four values (`smithy_query shape=GlobalNetworkState,SiteState,LinkState`:
/// AVAILABLE, DELETING, PENDING, UPDATING), mirrored in
/// `ops::types::GLOBAL_NETWORK_STATE_VALUES` / `SITE_STATE_VALUES` / `LINK_STATE_VALUES`.
pub const STATE_PENDING: &str = "PENDING";
/// See [`STATE_PENDING`].
pub const STATE_AVAILABLE: &str = "AVAILABLE";
/// See [`STATE_PENDING`].
pub const STATE_DELETING: &str = "DELETING";

/// The ARN namespace the model declares for this service (`arnNamespace: networkmanager`).
const ARN_NAMESPACE: &str = "networkmanager";

/// The resource kinds this slice mints: both the id prefix (`global-network-<hex>`,
/// `site-<hex>`, `link-<hex>`) and the ARN resource type (`global-network/<id>`,
/// `site/<id>`, `link/<id>`).
pub const GLOBAL_NETWORK_KIND: &str = "global-network";
/// See [`GLOBAL_NETWORK_KIND`].
pub const SITE_KIND: &str = "site";
/// See [`GLOBAL_NETWORK_KIND`].
pub const LINK_KIND: &str = "link";

/// Network Manager is a global (partition-scoped) service, so its ARNs carry an empty
/// region segment: `arn:aws:networkmanager::<account>:global-network/<id>`.
const ARN_REGION: &str = "";

/// The `ServiceCode` every modelled `ServiceQuotaExceededException` this service raises
/// carries: the model's own `arnNamespace`/signing name for Network Manager.
pub const SERVICE_CODE: &str = ARN_NAMESPACE;

/// The `LimitCode` of the two quotas this slice enforces. The model types `LimitCode` as
/// a free-form string and enumerates no values (`smithy_query
/// shape=ServiceQuotaExceededException`), so each code names its own quota.
pub const LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT: &str = "global-networks-per-account";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_SITES_PER_GLOBAL_NETWORK: &str = "sites-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_LINKS_PER_SITE: &str = "links-per-site";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`]; the quota [`MAX_TAGS_PER_RESOURCE`] bounds.
pub const LIMIT_TAGS_PER_RESOURCE: &str = "tags-per-resource";

/// The message every `ResourceNotFoundException` this service raises carries.
///
/// The model declares the member required but states no text. Two independent recordings
/// of different resources through different operations — `UpdateGlobalNetwork` on a
/// deleted global-network id and `TagResource` on the ARN of a deleted site — both answer
/// the identical string, so the message is a fixed, resource-INDEPENDENT constant: the
/// identifying detail travels in the shape's required `ResourceId` and `ResourceType`,
/// never in the prose. Every not-found refusal of this service is built by
/// [`resource_not_found`], so no handler can reintroduce per-request text.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The modelled `ResourceNotFoundException` (HTTP 404) for the resource `kind` and `id`
/// name: the constant [`RESOURCE_NOT_FOUND_MESSAGE`] plus the identifying detail in
/// `ResourceId` / `ResourceType`.
///
/// `kind` is one of the ARN resource types this service mints ([`GLOBAL_NETWORK_KIND`],
/// [`SITE_KIND`], [`LINK_KIND`]), which is also the `ResourceType` AWS reports.
pub fn resource_not_found(kind: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: id.to_string(),
        resource_type: kind.to_string(),
        context: None,
    }
}

/// Every value a `ResourceType` member of this service's network-resource operations
/// accepts, in lexicographic order so a paginated listing of them is stable.
///
/// The set is the enumeration in the member's own documentation (`smithy_query
/// query="doc" shape="GetNetworkResourceCountsRequest" member="ResourceType"`): three
/// Direct Connect types (`dxcon`, `dx-gateway`, `dx-vif`), eight Network Manager types
/// (`attachment`, `connect-peer`, `connection`, `core-network`, `device`, `link`,
/// `peering`, `site`) and six Amazon VPC types (`customer-gateway`, `transit-gateway`,
/// `transit-gateway-attachment`, `transit-gateway-connect-peer`,
/// `transit-gateway-route-table`, `vpn-connection`). The modelled `ConstrainedString`
/// carries only a length and a permissive pattern, so a value outside this set is refused
/// here rather than by the schema.
pub const NETWORK_RESOURCE_TYPES: [&str; 17] = [
    "attachment",
    "connect-peer",
    "connection",
    "core-network",
    "customer-gateway",
    "device",
    "dx-gateway",
    "dx-vif",
    "dxcon",
    "link",
    "peering",
    "site",
    "transit-gateway",
    "transit-gateway-attachment",
    "transit-gateway-connect-peer",
    "transit-gateway-route-table",
    "vpn-connection",
];

/// Whether `resource_type` is one of the values [`NETWORK_RESOURCE_TYPES`] lists.
pub fn is_network_resource_type(resource_type: &str) -> bool {
    NETWORK_RESOURCE_TYPES.contains(&resource_type)
}

/// The tag-key prefix AWS reserves for its own tags.
///
/// "Do not use the `aws:` prefix in your tag names or values because it is reserved for
/// AWS use. You can't edit or delete tag names or values with this prefix"
/// (AWS tagging documentation), so a request that asks to SET or REMOVE such a key is
/// refused and nothing is stored. The comparison is case-insensitive, as AWS's own
/// reserved-prefix check is.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The documented maximum length of a tag key, in characters: "Constraints: Maximum
/// length of 128 characters" on `Tag$Key`
/// (`smithy_query query="doc" shape="Tag" member="Key"`) — a constraint the modelled
/// `length 0..10000000` does not express, so it is checked here.
pub const MAX_TAG_KEY_CHARS: usize = 128;
/// The documented maximum length of a tag value, in characters: "Constraints: Maximum
/// length of 256 characters" on `Tag$Value`
/// (`smithy_query query="doc" shape="Tag" member="Value"`).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// How many tags one resource may carry: AWS's published tagging limit of 50 tags per
/// resource, which applies to every taggable resource. Asking for more is refused with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores
/// nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many global networks one account may hold here, and how many sites one global
/// network may hold.
///
/// Network Manager publishes an account-level quota for both, adjustable per account, and
/// the model carries neither number, so these two are CHOSEN: their purpose is that a
/// caller cannot grow the emulator's store without bound. Reaching one is answered with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores nothing,
/// exactly as exceeding a real quota does.
pub const MAX_GLOBAL_NETWORKS: usize = 100;
/// See [`MAX_GLOBAL_NETWORKS`].
pub const MAX_SITES_PER_GLOBAL_NETWORK: usize = 100;
/// How many links one site may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_LINKS_PER_SITE: usize = 100;

/// How long after the creating call a `PENDING` resource settles to `AVAILABLE`.
///
/// AWS documents no span for this transition, so the value is CHOSEN; the emulator
/// contract fixes a chosen settle span inside 250–750 ms and forbids zero, so a caller
/// that reads straight after the create still observes the transient `PENDING` state.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SETTLE_DELAY: Duration = Duration::from_millis(500);

/// A stored global network. `state` is not a field: it is a function of the clock
/// (see [`GlobalNetworkRecord::state_at`]), so reading one never writes.
#[derive(Debug, Clone)]
pub struct GlobalNetworkRecord {
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl GlobalNetworkRecord {
    /// This network's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `GlobalNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> GlobalNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `GlobalNetwork` for this record, forcing `state` — used by
    /// `DeleteGlobalNetwork`, which reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> GlobalNetwork {
        GlobalNetwork {
            global_network_id: Some(self.id.clone()),
            global_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored site, back-referencing the global network that owns it.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl SiteRecord {
    /// This site's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Site` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Site {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Site` for this record, forcing `state` — used by `DeleteSite`.
    pub fn to_shape_with_state(&self, state: &str) -> Site {
        Site {
            site_id: Some(self.id.clone()),
            site_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            description: self.description.clone(),
            location: self.location.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored link, back-referencing the site (and through it the global network) it
/// belongs to. Like the records above, `state` is a function of the clock.
#[derive(Debug, Clone)]
pub struct LinkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub site_id: String,
    pub description: Option<String>,
    /// The link's `Type`, a free-form string the model does not enumerate.
    pub link_type: Option<String>,
    /// The upload/download speeds in Mbps, as the request supplied them.
    pub bandwidth: Bandwidth,
    pub provider: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl LinkRecord {
    /// This link's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Link` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Link {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Link` for this record, forcing `state` — used by `DeleteLink`, which
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Link {
        Link {
            link_id: Some(self.id.clone()),
            link_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            site_id: Some(self.site_id.clone()),
            description: self.description.clone(),
            r#type: self.link_type.clone(),
            bandwidth: Some(self.bandwidth.clone()),
            provider: self.provider.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-networkmanager-networkmanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Global networks by `GlobalNetworkId`. A `BTreeMap`, so listing order is stable
    /// and a pagination token can name a key offset into it.
    pub global_networks: BTreeMap<String, GlobalNetworkRecord>,
    /// Sites by `SiteId`, each carrying its owning `GlobalNetworkId`.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Links by `LinkId`, each carrying the `SiteId` and `GlobalNetworkId` it belongs to.
    pub links: BTreeMap<String, LinkRecord>,
    /// Monotonic source for generated resource-id suffixes, so ids are unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

impl ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Mint the next id for `prefix`, e.g. `global-network-00000000000000001`.
    ///
    /// Network Manager ids are `<kind>-<hex>` (the model's own patterns for the kinds
    /// that constrain them read `^core-network-([0-9a-f]{8,17})$`), so the suffix is 17
    /// lowercase hex digits of the monotonic counter.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!("{prefix}-{:017x}", self.id_counter)
    }

    /// The ARN of a resource of this service: `arn:aws:networkmanager::<account>:<kind>/<id>`.
    pub fn resource_arn(kind: &str, id: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("{kind}/{id}"))
    }

    /// How many sites `global_network_id` owns, for the per-global-network quota.
    pub fn site_count_of(&self, global_network_id: &str) -> usize {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the sites owned by `global_network_id`, in `SiteId` order.
    pub fn site_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .map(|site| site.id.clone())
            .collect()
    }

    /// How many links the global network `global_network_id` holds, across all its sites.
    pub fn link_count_of_network(&self, global_network_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.global_network_id == global_network_id)
            .count()
    }

    /// How many resources of `resource_type` the global network `global_network_id`
    /// holds, for the network-resource inventory.
    ///
    /// `resource_type` must be one of [`NETWORK_RESOURCE_TYPES`] (the caller refuses
    /// anything else). Only the kinds this service stores can be non-zero; a kind whose
    /// creating operation is still a scaffold stub is held nowhere, so the count reported
    /// for it is the zero the store really holds, not a placeholder.
    pub fn network_resource_count(&self, global_network_id: &str, resource_type: &str) -> i32 {
        let held = match resource_type {
            SITE_KIND => self.site_count_of(global_network_id),
            LINK_KIND => self.link_count_of_network(global_network_id),
            _ => 0,
        };
        i32::try_from(held).unwrap_or(i32::MAX)
    }

    /// How many links `site_id` holds, for the per-site quota.
    pub fn link_count_of_site(&self, site_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .count()
    }

    /// The ids of the links attached to `site_id`, in `LinkId` order.
    pub fn link_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .map(|link| link.id.clone())
            .collect()
    }

    /// The stored tag list of the resource `arn` names, or `None` when this service holds
    /// no resource under that ARN.
    ///
    /// Tags live on the resource record itself, so every reader of a resource
    /// (`DescribeGlobalNetworks`, `GetSites`, `GetLinks`, `ListTagsForResource`) answers
    /// from the one stored list.
    pub fn tags_of(&self, arn: &str) -> Option<&Vec<Tag>> {
        let parts = parse_resource_arn(arn)?;
        match parts.kind {
            GLOBAL_NETWORK_KIND => self
                .global_networks
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            SITE_KIND => self
                .sites
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            LINK_KIND => self
                .links
                .get(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &record.tags),
            _ => None,
        }
    }

    /// The stored tag list of the resource `arn` names, for writing. See [`Self::tags_of`].
    pub fn tags_of_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        let parts = parse_resource_arn(arn)?;
        match parts.kind {
            GLOBAL_NETWORK_KIND => self
                .global_networks
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            SITE_KIND => self
                .sites
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            LINK_KIND => self
                .links
                .get_mut(parts.id)
                .filter(|record| record.arn == arn)
                .map(|record| &mut record.tags),
            _ => None,
        }
    }
}

/// The resource an ARN of this service names: the ARN resource type (which is also the
/// resource-id prefix this service mints, see [`GLOBAL_NETWORK_KIND`]) and the id.
#[derive(Debug, Clone, Copy)]
pub struct ArnParts<'a> {
    /// The ARN resource type, e.g. `global-network`.
    pub kind: &'a str,
    /// The resource id inside that type.
    pub id: &'a str,
}

/// Split a Network Manager resource ARN into its kind and id.
///
/// Every ARN this service mints reads `arn:<partition>:networkmanager::<account>:<kind>/<id>`
/// (see [`ComAmazonawsNetworkmanagerNetworkmanagerState::resource_arn`]). Answers `None`
/// when the string is not an ARN of this service in that `<kind>/<id>` form at all — the
/// caller refuses that as a `ValidationException` on the ARN member, which is a different
/// answer from a well-formed ARN naming a resource that does not exist.
///
/// Only the form is judged here, not which account, partition or region the ARN carries:
/// an ARN of this service that names another account's resource, or carries a region this
/// global service does not use, parses and then matches no stored record, so the caller
/// answers it as NOT FOUND rather than as a malformed request.
pub fn parse_resource_arn(arn: &str) -> Option<ArnParts<'_>> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // Partition and account are always present in an ARN; the region segment of this
    // global service is empty, so it is the only one allowed to be.
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    let _region = fields.next()?;
    if fields.next()?.is_empty() {
        return None;
    }
    let (kind, id) = fields.next()?.split_once('/')?;
    if kind.is_empty() || id.is_empty() {
        return None;
    }
    Some(ArnParts { kind, id })
}

/// Why a requested tag set or key list was refused: the input member to blame and the
/// reason, which the caller reports as a `ValidationException` (HTTP 400).
#[derive(Debug, Clone)]
pub struct TagRejection {
    /// The input member the refusal names, e.g. `Tags`.
    pub member: &'static str,
    /// Why that member is not acceptable.
    pub message: String,
}

impl TagRejection {
    fn new(member: &'static str, message: String) -> Self {
        Self { member, message }
    }

    /// The modelled `ValidationException` this rejection reports. Every caller lifts the
    /// same exception into its own operation error enum, so the refusal reads the same
    /// whichever operation the tags arrived on.
    pub fn to_exception(&self) -> ValidationException {
        ValidationException {
            message: self.message.clone(),
            reason: Some("FieldValidationFailed".to_string()),
            fields: Some(vec![ValidationExceptionField {
                name: self.member.to_string(),
                message: self.message.clone(),
            }]),
        }
    }
}

/// Why the `Tags` a request asks to store were refused.
#[derive(Debug, Clone)]
pub enum TagProblem {
    /// A tag breaks a rule every tag obeys; the caller answers [`TagRejection::to_exception`].
    Invalid(TagRejection),
    /// The request carries more distinct keys than [`MAX_TAGS_PER_RESOURCE`] (this many),
    /// so the caller answers its modelled `ServiceQuotaExceededException`.
    TooMany(usize),
}

/// The checks every operation that STORES a `Tags` member runs before storing anything:
/// the per-tag rules of [`validate_tags`], then the per-resource tag quota.
///
/// Shared by the creates that carry tags, so one rule change reaches all of them; the
/// caller only lifts the problem into its own error enum.
pub fn check_requested_tags(tags: Option<&Vec<Tag>>) -> Result<(), TagProblem> {
    let tags = tags.map_or(&[][..], Vec::as_slice);
    validate_tags(tags).map_err(TagProblem::Invalid)?;
    let count = tag_count_after_merge(&[], tags);
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(TagProblem::TooMany(count));
    }
    Ok(())
}

/// The message a refusal past the per-resource tag quota carries, shared by every
/// operation that stores tags: `resulting` is how many tags the resource would carry if
/// the request were accepted.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags, but the request would leave {resulting}"
    )
}

/// Check a tag set a request asks to APPLY (`TagResource`, and the `Tags` of a create).
///
/// Refuses an empty or over-long key, an over-long value, and any key carrying the
/// AWS-reserved prefix. A key repeated inside one request is NOT an error: tag keys are
/// unique per resource, so the last value for a key wins (see [`merge_tags`]).
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagRejection> {
    const MEMBER: &str = "Tags";
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        check_tag_key(MEMBER, key)?;
        let value = tag.value.as_deref().unwrap_or_default();
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            return Err(TagRejection::new(
                MEMBER,
                format!(
                    "the value of tag key {key:?} must be at most {MAX_TAG_VALUE_CHARS} characters, but is {length}"
                ),
            ));
        }
    }
    Ok(())
}

/// Check a key list a request asks to REMOVE (`UntagResource`). A key the resource does
/// not carry is not an error — removal is idempotent — but a key AWS reserves cannot be
/// removed any more than it can be set.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), TagRejection> {
    const MEMBER: &str = "TagKeys";
    for key in keys {
        check_tag_key(MEMBER, key)?;
    }
    Ok(())
}

/// The rules every tag key obeys, whichever member carries it.
fn check_tag_key(member: &'static str, key: &str) -> Result<(), TagRejection> {
    if key.is_empty() {
        return Err(TagRejection::new(
            member,
            "a tag key must not be empty".to_string(),
        ));
    }
    let length = key.chars().count();
    if length > MAX_TAG_KEY_CHARS {
        return Err(TagRejection::new(
            member,
            format!("a tag key must be at most {MAX_TAG_KEY_CHARS} characters, but {length} were given"),
        ));
    }
    if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Err(TagRejection::new(
            member,
            format!(
                "tag key {key:?} uses the {RESERVED_TAG_PREFIX} prefix, which is reserved for AWS"
            ),
        ));
    }
    Ok(())
}

/// How many tags the resource would carry once `requested` is merged into `existing`,
/// for the [`MAX_TAGS_PER_RESOURCE`] quota: a key already held is replaced, not added.
pub fn tag_count_after_merge(existing: &[Tag], requested: &[Tag]) -> usize {
    let mut keys: BTreeSet<&str> = existing.iter().filter_map(|tag| tag.key.as_deref()).collect();
    keys.extend(requested.iter().filter_map(|tag| tag.key.as_deref()));
    keys.len()
}

/// Merge `requested` into the resource's stored tags.
///
/// Tag keys are unique per resource, so a key already held keeps its position and takes
/// the requested value and a new key is appended — which is what makes a repeated
/// `TagResource` overwrite rather than duplicate.
pub fn merge_tags(existing: &mut Vec<Tag>, requested: Vec<Tag>) {
    for tag in requested {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
            None => existing.push(tag),
        }
    }
}

/// Remove every tag whose key `keys` names. A key the resource does not carry is ignored,
/// so removing twice answers the same as removing once.
pub fn remove_tags(existing: &mut Vec<Tag>, keys: &[String]) {
    existing.retain(|held| {
        held.key
            .as_deref()
            .is_none_or(|key| !keys.iter().any(|wanted| wanted == key))
    });
}

/// `SystemTime` as a Smithy timestamp (epoch seconds, sub-second part kept).
fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// One page of a paginated listing.
pub struct Page<T> {
    /// The items on this page, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` when this is the last page.
    pub next_token: Option<String>,
}

/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";

/// Encode the continuation token that resumes AFTER `last_key`.
///
/// The token is opaque to the caller: hex of a prefixed key offset into the relevant
/// `BTreeMap`, which is why the listing map is ordered. Hex, so the token survives the
/// query string a `NextToken` travels back in (`@httpQuery("nextToken")`) unescaped,
/// whatever the key holds.
fn encode_page_token(last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}

/// Decode a continuation token minted by [`encode_page_token`], or `None` when the token
/// is not one this service produced — the caller answers that with a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// Take one page of `entries` (which must be sorted by key) starting after `after`.
///
/// `max_results` is the caller's page size; absent means "every remaining item". A
/// `next_token` is emitted only when items remain after this page, so a client that
/// follows tokens sees every item exactly once.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let start = match after {
        Some(key) => entries
            .iter()
            .position(|(candidate, _)| candidate.as_str() > key)
            .unwrap_or(entries.len()),
        None => 0,
    };
    let limit = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(usize::MAX)
        // A page is never empty while items remain: `MaxResults` is modelled `1..500`, so
        // 0 cannot arrive, and clamping keeps the token below from skipping an item.
        .max(1);
    let end = start.saturating_add(limit).min(entries.len());
    let next_token = if end < entries.len() {
        entries
            .get(end.saturating_sub(1))
            .map(|(key, _)| encode_page_token(key))
    } else {
        None
    };
    let items = entries
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, item)| item)
        .collect();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmanagerNetworkmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmanagerNetworkmanagerState>)
}
