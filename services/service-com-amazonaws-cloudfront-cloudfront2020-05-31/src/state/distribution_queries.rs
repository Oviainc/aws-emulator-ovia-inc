// SPDX-License-Identifier: CC0-1.0

//! The reads over the DISTRIBUTION COLLECTION.
//!
//! What a create is refused with, how a listing walks the distributions in the one order
//! every distribution listing serves them in, and which stored configurations REFERENCE a
//! given policy, key group or web ACL — the question a delete of that policy, and a listing
//! filtered by it, both ask.
//!
//! The records themselves live in the parent module, and the store's own methods in
//! [`super::store`]; nothing here mutates anything.

use super::*;

/// Why a create was refused, for the calling operation to report as its own modelled
/// error.
///
/// Both of this service's distribution creates run the same checks, so each one reports
/// these same refusals; a variant carries the message its modelled error takes.
#[derive(Debug, Clone)]
pub enum CreateRefusal {
    /// `InconsistentQuantities` — "The value of Quantity and the size of Items don't
    /// match."
    InconsistentQuantities(String),
    /// A count in the submitted configuration over this service's maximum, which names
    /// the modelled error it is refused with (see [`ConfigLimit`]).
    ConfigLimit(ConfigLimit),
    /// `DistributionAlreadyExists` — "The caller reference you attempted to create the
    /// distribution with is associated with another distribution."
    DistributionAlreadyExists(String),
    /// `TooManyDistributions` — "Processing your request would cause you to exceed the
    /// maximum number of distributions allowed."
    TooManyDistributions(String),
    /// `InvalidViewerCertificate` — the configuration's alternate domain names and its
    /// viewer certificate disagree (see [`alias_certificate_mismatch`]).
    InvalidViewerCertificate(String),
    /// `InvalidOriginAccessControl` — an origin of the configuration names an origin
    /// access control this account does not hold (see
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::unknown_origin_access_control`]).
    InvalidOriginAccessControl(String),
    /// `IllegalOriginAccessConfiguration` — an origin of the configuration names both an
    /// origin access control and an origin access identity (see
    /// [`origin_with_both_access_mechanisms`]).
    IllegalOriginAccessConfiguration(String),
    /// `InvalidLambdaFunctionAssociation` — a cache behaviour of the configuration carries
    /// an edge association naming no published function version (see
    /// [`lambda_association_without_version`]).
    InvalidLambdaFunctionAssociation(String),
    /// An argument of the submitted configuration CloudFront cannot read: a cache
    /// behaviour's function association names an event type no CloudFront function runs on
    /// (see [`function_association_event_refusal`]).
    InvalidArgument(String),
}

/// What a create hands back: the new record's id and version token, and the
/// `Distribution` projection — every one of them read from the record just persisted.
#[derive(Debug, Clone)]
pub struct CreatedDistribution {
    /// `Distribution$Id`, and the path segment the `Location` header names.
    pub id: String,
    /// The version token this create minted, reported in the `ETag` header.
    pub etag: String,
    /// The `Distribution` the response carries.
    pub distribution: Distribution,
}

/// Whether a configuration describes a MULTI-TENANT distribution.
///
/// Two members say so, and either one is enough: `ConnectionMode` `tenant-only` selects
/// "a multi-tenant distribution with distribution tenants", and `TenantConfig` is the
/// member that "only supports multi-tenant distributions. You can't specify this field
/// for standard distributions." A configuration naming neither is a standard
/// (`direct`) distribution.
///
/// A configuration that names `TenantConfig` AND `direct` contradicts itself, and this
/// reads it as multi-tenant rather than refusing it: the exclusivity the prose states is
/// a validation this slice does not implement — no modelled refusal is wired for ANY of
/// the members documented as unsupported per connection mode (`Aliases`,
/// `IsIPV6Enabled`, `AnycastIpListId`, …), and adding one for this single pair would
/// refuse a request the rest of the surface accepts. Until that validation is
/// implemented as a whole, the tenancy-only member is the one taken to describe the
/// distribution's tenancy, so the domain reported stays consistent with the tenancy the
/// same configuration states.
pub fn is_multi_tenant(config: &DistributionConfig) -> bool {
    config.connection_mode.as_deref() == Some(CONNECTION_MODE_TENANT_ONLY)
        || config.tenant_config.is_some()
}

/// The `ConnectionMode` a stored configuration describes: [`CONNECTION_MODE_TENANT_ONLY`]
/// for a multi-tenant distribution, [`CONNECTION_MODE_DIRECT`] for a standard one.
///
/// Decided through [`is_multi_tenant`], so the connection mode a distribution is LISTED
/// under is the same tenancy every other projection of the one record reports — its
/// `DomainName` above all — and a configuration that named no `ConnectionMode` is listed
/// as the standard distribution it is served as.
pub fn connection_mode_of(config: &DistributionConfig) -> &'static str {
    if is_multi_tenant(config) {
        CONNECTION_MODE_TENANT_ONLY
    } else {
        CONNECTION_MODE_DIRECT
    }
}

/// The `ConnectionMode` a SUBMITTED configuration declares.
///
/// `ConnectionMode` "specifies whether the connection mode is through a standard
/// distribution (direct) or a multi-tenant distribution with distribution tenants
/// (tenant-only)", so a configuration that states the member declares exactly the mode it
/// names; one that states none declares the mode its other members describe
/// ([`connection_mode_of`]) — [`CONNECTION_MODE_TENANT_ONLY`] for a configuration carrying
/// the multi-tenant-only `TenantConfig`, and the standard [`CONNECTION_MODE_DIRECT`]
/// otherwise.
///
/// This is the value an UPDATE is measured against the stored one by: what a configuration
/// declares is what the distribution it describes would be served as, so two configurations
/// declaring different modes describe different KINDS of distribution. The member itself
/// takes precedence over the tenancy derived from the rest, so a request that states it is
/// read as having asked for exactly that mode.
pub fn connection_mode_declared(config: &DistributionConfig) -> &'static str {
    match config.connection_mode.as_deref() {
        Some(mode) if mode == CONNECTION_MODE_TENANT_ONLY => CONNECTION_MODE_TENANT_ONLY,
        Some(_) => CONNECTION_MODE_DIRECT,
        None => connection_mode_of(config),
    }
}

/// ONE page of the stored distributions `matches` selects, as the `DistributionList`
/// every CloudFront distribution listing answers with.
///
/// The page contract is the same for the unfiltered listing and for each listing that
/// serves a SUBSET of the store, so it lives here once rather than in each handler:
///
/// * `marker` is the distribution id the previous page ended at, so this page starts
///   after it — the store is ordered by id, so the resume point is a range bound rather
///   than a scan, and walking the pages enumerates every selected distribution exactly
///   once, in id order;
/// * `page_size` is how many summaries this response serves (see [`page_size`]), and
///   `max_items` is reported as "the value you provided for the MaxItems request
///   parameter";
/// * `NextMarker` is present exactly when selected distributions remain, carrying the id
///   this page ended at, and `IsTruncated` says the same thing;
/// * `Quantity` is the length of `Items`, and a page that lists nothing carries no
///   `Items` element at all rather than an empty one.
///
/// Each summary is [`StoredDistribution::to_summary`] of the one stored record, so a
/// filtered listing and the unfiltered one can never disagree about a distribution.
pub fn distribution_page<F>(
    distributions: &BTreeMap<String, StoredDistribution>,
    marker: Option<&str>,
    max_items: i32,
    page_size: usize,
    mut matches: F,
) -> crate::ops::types::DistributionList
where
    F: FnMut(&StoredDistribution) -> bool,
{
    let (items, is_truncated, last_id) = distribution_page_walk(
        distributions,
        marker,
        page_size,
        &mut matches,
        StoredDistribution::to_summary,
    );

    crate::ops::types::DistributionList {
        marker: marker.unwrap_or_default().to_string(),
        next_marker: if is_truncated { last_id } else { None },
        max_items,
        is_truncated,
        quantity: i32::try_from(items.len()).unwrap_or(i32::MAX),
        items: if items.is_empty() { None } else { Some(items) },
    }
}

/// ONE page of the IDS of the stored distributions `matches` selects, as the
/// `DistributionIdList` every CloudFront "distributions associated with this resource"
/// listing answers with.
///
/// The SAME page contract as [`distribution_page`] — the marker is the distribution id the
/// previous page ended at, `NextMarker` and `IsTruncated` arrive exactly when selected
/// distributions remain, and `Quantity` states the length of `Items` — over the same walk,
/// so an id listing and a summary listing can never disagree about which distributions a
/// filter selects or about the order they are served in. Only the projection differs:
/// `DistributionIdList$Items` "contains the distribution IDs in the list".
///
/// Where a `DistributionList` drops its `Items` element on an empty page, an id list keeps
/// it: the recorded service answers a filter nothing matches with the element present and
/// empty, so a caller reads the ids of every page the same way rather than having to tell an
/// empty page from a missing member.
pub fn distribution_id_page<F>(
    distributions: &BTreeMap<String, StoredDistribution>,
    marker: Option<&str>,
    max_items: i32,
    page_size: usize,
    mut matches: F,
) -> crate::ops::types::DistributionIdList
where
    F: FnMut(&StoredDistribution) -> bool,
{
    let (items, is_truncated, last_id) = distribution_page_walk(
        distributions,
        marker,
        page_size,
        &mut matches,
        |stored| stored.id.clone(),
    );

    crate::ops::types::DistributionIdList {
        marker: marker.unwrap_or_default().to_string(),
        next_marker: if is_truncated { last_id } else { None },
        max_items,
        is_truncated,
        quantity: i32::try_from(items.len()).unwrap_or(i32::MAX),
        items: Some(items),
    }
}

/// The one page walk behind every CloudFront distribution listing: what `project` reads
/// from each selected record, whether selected distributions remain after the page, and the
/// distribution id this page ended at.
///
/// The store is ordered by id, so `marker` is a range bound rather than a scan and walking
/// the pages enumerates every selected distribution exactly once, in id order.
pub(super) fn distribution_page_walk<'a, T, F, M>(
    distributions: &'a BTreeMap<String, StoredDistribution>,
    marker: Option<&str>,
    page_size: usize,
    matches: &mut F,
    mut project: M,
) -> (Vec<T>, bool, Option<String>)
where
    F: FnMut(&StoredDistribution) -> bool,
    M: FnMut(&'a StoredDistribution) -> T,
{
    let start = match marker {
        Some(marker) => std::ops::Bound::Excluded(marker),
        None => std::ops::Bound::Unbounded,
    };
    let mut items: Vec<T> = Vec::new();
    let mut is_truncated = false;
    let mut last: Option<&'a StoredDistribution> = None;
    for (_, stored) in distributions.range::<str, _>((start, std::ops::Bound::Unbounded)) {
        if !matches(stored) {
            continue;
        }
        if items.len() >= page_size {
            is_truncated = true;
            break;
        }
        items.push(project(stored));
        last = Some(stored);
    }
    // The id is taken once, from the record the page ended at, rather than cloned per item.
    (
        items,
        is_truncated,
        last.map(|stored| stored.id.clone()),
    )
}

/// Whether `config` associates any of its cache behaviours with the cache policy
/// `cache_policy_id` names.
///
/// A cache policy is attached to a distribution by a cache behaviour's `CachePolicyId`, and
/// a distribution's behaviours are its DEFAULT cache behaviour together with its ordered
/// ones — both declare the member, so both are walked here and a distribution is associated
/// with the policy as soon as ONE of them names it.
pub fn references_cache_policy(config: &DistributionConfig, cache_policy_id: &str) -> bool {
    behavior_attachments(
        config,
        |behavior| behavior.cache_policy_id.as_deref(),
        |behavior| behavior.cache_policy_id.as_deref(),
    )
    .any(|id| id == cache_policy_id)
}

/// Whether `config` associates any of its cache behaviours with the origin request policy
/// `origin_request_policy_id` names.
///
/// An origin request policy is attached to a distribution by a cache behaviour's
/// `OriginRequestPolicyId`, and a distribution's behaviours are its DEFAULT cache behaviour
/// together with its ordered ones — both declare the member, so both are walked here and a
/// distribution is associated with the policy as soon as ONE of them names it. The SAME
/// rule [`references_cache_policy`] states for the other policy a cache behaviour attaches,
/// over the same walk.
pub fn references_origin_request_policy(
    config: &DistributionConfig,
    origin_request_policy_id: &str,
) -> bool {
    behavior_attachments(
        config,
        |behavior| behavior.origin_request_policy_id.as_deref(),
        |behavior| behavior.origin_request_policy_id.as_deref(),
    )
    .any(|id| id == origin_request_policy_id)
}

/// Whether `config` associates any of its cache behaviours with the response headers policy
/// `response_headers_policy_id` names.
///
/// A response headers policy is attached to a distribution by a cache behaviour's
/// `ResponseHeadersPolicyId`, and a distribution's behaviours are its DEFAULT cache behaviour
/// together with its ordered ones — both declare the member, so both are walked here and a
/// distribution is associated with the policy as soon as ONE of them names it. The SAME rule
/// [`references_cache_policy`] states for the other policies a cache behaviour attaches, over
/// the same walk.
pub fn references_response_headers_policy(
    config: &DistributionConfig,
    response_headers_policy_id: &str,
) -> bool {
    behavior_attachments(
        config,
        |behavior| behavior.response_headers_policy_id.as_deref(),
        |behavior| behavior.response_headers_policy_id.as_deref(),
    )
    .any(|id| id == response_headers_policy_id)
}

/// Whether `config` references the field-level encryption configuration
/// `field_level_encryption_id` names in any of its cache behaviours.
///
/// A field-level encryption configuration is used by a distribution through a cache
/// behaviour's `FieldLevelEncryptionId`, and a distribution's behaviours are its DEFAULT
/// cache behaviour together with its ordered ones — both declare the member, so both are
/// walked here and a distribution uses the configuration as soon as ONE of them names it.
/// The SAME rule [`references_cache_policy`] states for the policies a cache behaviour
/// attaches, over the same walk.
pub fn references_field_level_encryption_config(
    config: &DistributionConfig,
    field_level_encryption_id: &str,
) -> bool {
    behavior_attachments(
        config,
        |behavior| behavior.field_level_encryption_id.as_deref(),
        |behavior| behavior.field_level_encryption_id.as_deref(),
    )
    .any(|id| id == field_level_encryption_id)
}

/// Whether `config` references the field-level encryption profile `profile_id` names in any
/// of its mappings.
///
/// A field-level encryption configuration names the profile a request's matching fields are
/// encrypted under through a MAPPING: a query argument-profile mapping
/// (`QueryArgProfile$ProfileId`, required on that mapping) and a content type-profile mapping
/// (`ContentTypeProfile$ProfileId`, optional, so a mapping that names none selects no profile
/// at all rather than an unknown one). Both sections are optional, and each carries its
/// mappings in a wrapper that may be left out — so a configuration references the profile as
/// soon as ONE mapping of EITHER section names it, and a configuration carrying no mapping
/// references nothing. The SAME rule [`references_field_level_encryption_config`] states for
/// the configuration a distribution's cache behaviour names, over that resource's own
/// attachment members.
pub fn references_field_level_encryption_profile(
    config: &FieldLevelEncryptionConfig,
    profile_id: &str,
) -> bool {
    let query_arg = config
        .query_arg_profile_config
        .as_ref()
        .and_then(|section| section.query_arg_profiles.as_ref())
        .and_then(|profiles| profiles.items.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|mapping| mapping.profile_id.as_str());
    let content_type = config
        .content_type_profile_config
        .as_ref()
        .and_then(|section| section.content_type_profiles.as_ref())
        .and_then(|profiles| profiles.items.as_deref())
        .unwrap_or_default()
        .iter()
        .filter_map(|mapping| mapping.profile_id.as_deref());
    query_arg.chain(content_type).any(|id| id == profile_id)
}

/// Whether `config` references the key group `key_group_id` names in any of its cache
/// behaviours.
///
/// A key group is referenced by a cache behaviour through its `TrustedKeyGroups` list —
/// "a list of key groups that CloudFront can use to validate signed URLs or signed
/// cookies" — rather than through a single-valued member, so the LIST of every behaviour
/// is walked and a distribution references the group as soon as ONE entry names it. The
/// behaviours walked are the same ones every other attachment predicate walks (the default
/// cache behaviour together with the ordered ones), and the reference is what the stored
/// list holds: the `Enabled` flag beside it says whether signed access is switched on, not
/// whether the group is named, which is also how
/// [`StoredDistribution::active_trusted_key_groups`] reports the trusted set.
pub fn references_key_group(config: &DistributionConfig, key_group_id: &str) -> bool {
    behavior_views(config)
        .iter()
        .filter_map(|behavior| behavior.trusted_key_groups)
        .flat_map(|groups| groups.items.as_deref().unwrap_or_default())
        .any(|id| id == key_group_id)
}

/// The `WebACLId` a listing names to select the distributions that are associated with NO
/// web ACL: "if you specify "null" for the ID, the request returns a list of the
/// distributions that aren't associated with a web ACL".
pub const WEB_ACL_ID_NONE: &str = "null";

/// Whether `config` is associated with the web ACL `web_acl_id` names.
///
/// A web ACL is attached to a distribution by the single `DistributionConfig$WebACLId`
/// member, whose EMPTY value is what "no web ACL" is — that is what a disassociation
/// leaves behind — so the association is an exact comparison against the one stored
/// string, and the sentinel [`WEB_ACL_ID_NONE`] selects the distributions that carry no
/// association at all.
///
/// A web ACL lives in WAF, which this store cannot resolve, so an identifier no
/// distribution carries selects nothing rather than being refused: the value is only ever
/// compared, never validated.
pub fn associated_with_web_acl(config: &DistributionConfig, web_acl_id: &str) -> bool {
    let stored = config.web_acl_id.as_deref().unwrap_or_default();
    if web_acl_id == WEB_ACL_ID_NONE {
        stored.is_empty()
    } else {
        stored == web_acl_id
    }
}

/// What every cache behaviour of `config` attaches through one of its members — the default
/// behaviour first, then each ordered one — skipping the behaviours that attach nothing.
///
/// A distribution's behaviours are its default cache behaviour and its ordered ones, which
/// are DIFFERENT shapes carrying the same attachment members, so each is read by its own
/// accessor and the walk over them is stated once here.
pub(crate) fn behavior_attachments<'a, D, O>(
    config: &'a DistributionConfig,
    from_default: D,
    from_ordered: O,
) -> impl Iterator<Item = &'a str> + 'a
where
    D: FnOnce(&'a DefaultCacheBehavior) -> Option<&'a str>,
    O: FnMut(&'a CacheBehavior) -> Option<&'a str> + 'a,
{
    std::iter::once(from_default(&config.default_cache_behavior))
        .chain(
            config
                .cache_behaviors
                .as_ref()
                .and_then(|behaviors| behaviors.items.as_deref())
                .unwrap_or_default()
                .iter()
                .map(from_ordered),
        )
        .flatten()
}

/// `methods` in [`EMITTED_METHOD_ORDER`], as a permutation of what was submitted: each
/// method the order names appears as many times as it was submitted, and a value the
/// order does not name keeps its submitted position relative to the other such values,
/// after them. The length is therefore unchanged, so the `Quantity` beside the list
/// still states it.
///
/// The trailing group is unreachable for a request: `MethodsList`'s member targets the
/// `Method` enum, so the framework's deserializer refuses a value outside it before a
/// handler runs, and the order below names every value the enum has. It is kept so that
/// this is a pure reordering — a value this function cannot place is never dropped.
pub(super) fn in_emitted_order(methods: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(methods.len());
    for known in EMITTED_METHOD_ORDER {
        out.extend(
            methods
                .iter()
                .filter(|method| method.as_str() == known)
                .cloned(),
        );
    }
    out.extend(
        methods
            .iter()
            .filter(|method| !EMITTED_METHOD_ORDER.contains(&method.as_str()))
            .cloned(),
    );
    out
}
