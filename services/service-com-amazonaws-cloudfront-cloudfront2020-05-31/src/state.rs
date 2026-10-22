// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudfront-cloudfront2020-05-31 service.
//!
//! CloudFront's distribution surface is CUSTOMER-owned: every distribution a caller can
//! see was created by `CreateDistribution`, so this store starts EMPTY and every record
//! in it was put there by a mutating call. The store is the single source of truth for
//! each distribution's id, ARN, domain name, status, version token (`ETag`), its
//! configuration as submitted, and the invalidations created under it — so a create or
//! update followed by a get, a list or a delete answers from the one record rather than
//! from the request that is being served.
//!
//! Two things in here are owed by the CLOCK rather than by a call:
//!
//! * a distribution CloudFront has just created or reconfigured is `InProgress` until
//!   its configuration has propagated, and is `Deployed` after
//!   ([`PROPAGATION_DELAY`]);
//! * an invalidation is `InProgress` until its batch finishes, and `Completed` after.
//!
//! Both are materialised by [`ComAmazonawsCloudfrontCloudfront20200531State::settle`],
//! which every handler calls before it reads: the transition was owed at the same
//! instant whether or not anybody called, so applying it on a read never changes what a
//! later caller sees.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AllowedMethods, CacheBehavior, CacheBehaviors, CustomErrorResponses, DefaultCacheBehavior,
    Distribution, DistributionConfig, DistributionSummary, ForwardedValues, FunctionAssociations,
    GeoRestriction, Invalidation, InvalidationBatch, LambdaFunctionAssociations,
    NoSuchDistribution, NoSuchInvalidation, Paths, Restrictions, TrustedKeyGroups, TrustedSigners,
};

/// `Distribution$Status` while a configuration is still propagating to the edge
/// locations. The model documents the terminal value — "When the status is `Deployed`,
/// the distribution's information is fully propagated to all CloudFront edge locations"
/// — so a distribution whose configuration has just changed is not yet in it.
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// `Distribution$Status` once the configuration has propagated: "When the status is
/// `Deployed`, the distribution's information is fully propagated to all CloudFront edge
/// locations."
pub const STATUS_DEPLOYED: &str = "Deployed";

/// `Invalidation$Status` once the batch has finished: "When the invalidation batch is
/// finished, the status is `Completed`."
pub const STATUS_COMPLETED: &str = "Completed";

/// How long after the call that starts it a propagation or an invalidation batch is
/// owed.
///
/// CloudFront publishes no deadline for either, so this span is the EMULATOR's choice
/// rather than a claim about the service: it is long enough that a caller acting
/// straight after a create, an update or an invalidation still observes the transient
/// `InProgress` status (and the state rules that go with it), and short enough that a
/// caller that waits observes the terminal one.
pub const PROPAGATION_DELAY: Duration = Duration::from_millis(500);

/// The number of summaries `ListDistributions` serves when a request names no
/// `MaxItems`, and the ceiling it serves at whatever a request asks for.
///
/// CloudFront's `ListDistributions` pages at 100 items by default; `MaxItems` carries no
/// `@range` in the model, so the ceiling is this emulator's own page bound rather than a
/// published limit. ONE constant is both, so the default can never drift past the
/// ceiling. It bounds ONE response — a caller that wants more pages the rest with
/// `Marker` — so no single request can serve the whole store while holding the state
/// lock.
pub const MAX_ITEMS: i32 = 100;

/// The most distributions this emulator holds.
///
/// The store lives for the process's lifetime and grows only on a create, and a
/// distribution can be given no TTL and cannot be evicted: a `GetDistribution` has to
/// keep answering with what its create returned until a `DeleteDistribution` removes it,
/// so dropping a record would be a divergence rather than a cleanup. A maximum entry
/// count, refused on the create path before anything is inserted, is therefore the bound
/// available here — the same construct this workspace's other services use
/// (`MAX_TRUST_ANCHORS`, `MAX_PROFILES`). It sits far above any realistic client
/// workflow, so a caller never meets it by accident, and the refusal is the modelled
/// `TooManyDistributions`: "Processing your request would cause you to exceed the maximum
/// number of distributions allowed."
pub const MAX_DISTRIBUTIONS: usize = 10_000;

/// The most invalidations this emulator holds for ONE distribution.
///
/// Same shape as [`MAX_DISTRIBUTIONS`]: an invalidation record is what `GetInvalidation`
/// answers from, so it is bounded by a maximum entry count refused before the insert
/// rather than evicted. The refusal is the modelled `TooManyInvalidationsInProgress`:
/// "You have exceeded the maximum number of allowable InProgress invalidation batch
/// requests, or invalidation objects."
pub const MAX_INVALIDATIONS_PER_DISTRIBUTION: usize = 1_000;

/// The most paths ONE invalidation batch may name.
///
/// CloudFront's published quota for an invalidation request is 3,000 files, and the
/// batch is stored whole, so this bounds a single request's contribution to the store as
/// well. Over it, the refusal is the modelled `BatchTooLarge` (HTTP 413): "Invalidation
/// batch specified is too large."
pub const MAX_INVALIDATION_PATHS: usize = 3_000;

/// The `PriceClass` reported for a distribution whose stored configuration names none:
/// the class that covers every CloudFront edge location, which is what
/// "`PriceClass_All`, CloudFront responds to requests for your objects from all
/// CloudFront edge locations" describes.
pub const DEFAULT_PRICE_CLASS: &str = "PriceClass_All";

/// The `HttpVersion` reported for a distribution whose stored configuration names none.
/// `DistributionConfig$HttpVersion`: "The default value for new web distributions is
/// `http2`."
pub const DEFAULT_HTTP_VERSION: &str = "http2";

/// `GeoRestriction$RestrictionType`'s value for a distribution that restricts nothing,
/// reported for a stored configuration that carries no `Restrictions`.
pub const GEO_RESTRICTION_NONE: &str = "none";

/// The 501 message for the one case [`service_state`] answers `None`: this service is
/// mounted on an engine that holds no per-service state, so there is no distribution
/// store to serve the request from. Unreachable through the router, which builds one
/// shared `engine::InMemoryEngine`; it is answered honestly rather than by fabricating a
/// response or by constructing a private store whose writes would be dropped.
pub const STORE_UNAVAILABLE: &str =
    "the CloudFront distribution store is unavailable: this service is mounted on an engine \
     that holds no per-service state";

/// The error a handler answers when [`service_state`] has no store for it: HTTP 501
/// carrying [`STORE_UNAVAILABLE`], in the operation's own error channel.
///
/// Generic over the operation's modelled error enum, so every handler reports this one
/// condition the same way without any of them having to name a modelled error that would
/// misdescribe it (CloudFront models no server fault for these operations).
pub fn store_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: STORE_UNAVAILABLE.to_string(),
    })
}

/// How many characters follow the leading letter of a generated CloudFront token: a
/// distribution id, an invalidation id and a version token (`ETag`) are all a letter
/// followed by 13 uppercase alphanumerics (`E`+13 for a distribution and for the version
/// token, `I`+13 for an invalidation), the shape CloudFront's own values take.
///
/// 13 is also what makes the rendering injective on `u64` (36^13 > 2^64), so every
/// character of a minted token carries mint state rather than padding.
const ID_BODY_LEN: usize = 13;

/// The alphabet CloudFront draws a distribution id, an invalidation id and a version
/// token from: decimal digits and uppercase ASCII letters.
const TOKEN_ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// One invalidation batch submitted against one distribution.
#[derive(Debug, Clone)]
pub struct StoredInvalidation {
    /// `Invalidation$Id` — "The identifier for the invalidation request."
    pub id: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredInvalidation::completed_due`], then
    /// [`STATUS_COMPLETED`].
    pub status: String,
    /// "The date and time the invalidation request was first made."
    pub create_time: SystemTime,
    /// The instant the batch is owed to be `Completed`. Stored rather than slept for.
    pub completed_due: SystemTime,
    /// The batch exactly as submitted — "The current invalidation information for the
    /// batch request."
    pub batch: InvalidationBatch,
}

impl StoredInvalidation {
    /// The `Invalidation` projection every invalidation operation returns, read from the
    /// one stored record.
    pub fn to_invalidation(&self) -> Invalidation {
        Invalidation {
            id: self.id.clone(),
            status: self.status.clone(),
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            invalidation_batch: self.batch.clone(),
        }
    }
}

/// One CloudFront distribution, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredDistribution {
    /// `Distribution$Id`, allocated at creation and never reused.
    pub id: String,
    /// `Distribution$ARN`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// `Distribution$DomainName`, allocated at creation and stable for the
    /// distribution's whole life.
    pub domain_name: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredDistribution::deployed_due`], then
    /// [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this distribution's current configuration is owed to be `Deployed`.
    pub deployed_due: SystemTime,
    /// `Distribution$LastModifiedTime`: when the configuration last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check.
    pub etag: String,
    /// The configuration as submitted, stored whole so a later read returns what was
    /// created or updated rather than a re-derivation of it.
    pub config: DistributionConfig,
    /// Every invalidation created under this distribution, keyed by invalidation id.
    pub invalidations: BTreeMap<String, StoredInvalidation>,
}

impl StoredDistribution {
    /// Apply every transition the clock owes this distribution at `now`.
    ///
    /// Idempotent and monotonic: a status that is already terminal stays put, so calling
    /// this on a read cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != STATUS_DEPLOYED && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
        for invalidation in self.invalidations.values_mut() {
            if invalidation.status != STATUS_COMPLETED && now >= invalidation.completed_due {
                invalidation.status = STATUS_COMPLETED.to_string();
            }
        }
    }

    /// "The number of invalidation batches currently in progress" — counted from the
    /// stored invalidations, so it falls as their batches finish.
    pub fn in_progress_invalidation_batches(&self) -> i32 {
        let in_progress = self
            .invalidations
            .values()
            .filter(|invalidation| invalidation.status != STATUS_COMPLETED)
            .count();
        i32::try_from(in_progress).unwrap_or(i32::MAX)
    }

    /// The `Distribution` projection `CreateDistribution`, `GetDistribution` and
    /// `UpdateDistribution` return.
    pub fn to_distribution(&self) -> Distribution {
        Distribution {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            in_progress_invalidation_batches: self.in_progress_invalidation_batches(),
            domain_name: self.domain_name.clone(),
            // "ActiveTrustedSigners/ActiveTrustedKeyGroups: the key pairs that
            // CloudFront can use to verify the signatures of signed URLs and cookies."
            // Nothing in this slice activates a signer or a key group, so there is no
            // active set to report.
            active_trusted_signers: None,
            active_trusted_key_groups: None,
            distribution_config: self.config.clone(),
            // An ICP recordal applies only to a distribution with alternate domain
            // names in the China partition, which this emulator does not serve.
            alias_icp_recordals: None,
        }
    }

    /// The `DistributionSummary` projection `ListDistributions` returns.
    ///
    /// Every member it shares with [`StoredDistribution::to_distribution`] takes the
    /// same value: both read the one stored record. Where the summary requires a member
    /// the stored configuration leaves out, the value reported is the one the absent
    /// configuration member means (see [`DEFAULT_PRICE_CLASS`],
    /// [`DEFAULT_HTTP_VERSION`], [`GEO_RESTRICTION_NONE`]).
    pub fn to_summary(&self) -> DistributionSummary {
        let config = &self.config;
        DistributionSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            // "ETag: the current version of the distribution" — the same token the last
            // mutation handed back.
            e_tag: Some(self.etag.clone()),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            domain_name: self.domain_name.clone(),
            aliases: config.aliases.clone().unwrap_or_default(),
            origins: config.origins.clone(),
            origin_groups: config.origin_groups.clone(),
            default_cache_behavior: config.default_cache_behavior.clone(),
            // An absent wrapper means no cache behaviours and no custom error
            // responses at all, which is the empty wrapper.
            cache_behaviors: config.cache_behaviors.clone().unwrap_or(CacheBehaviors {
                quantity: 0,
                items: None,
            }),
            custom_error_responses: config.custom_error_responses.clone().unwrap_or(
                CustomErrorResponses {
                    quantity: 0,
                    items: None,
                },
            ),
            comment: config.comment.clone(),
            price_class: config
                .price_class
                .clone()
                .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
            enabled: config.enabled,
            // A configuration that names no certificate configures nothing about the
            // viewer certificate, so there is nothing to report under it.
            viewer_certificate: config.viewer_certificate.clone().unwrap_or_default(),
            restrictions: config
                .restrictions
                .clone()
                .unwrap_or_else(|| Restrictions {
                    geo_restriction: GeoRestriction {
                        restriction_type: GEO_RESTRICTION_NONE.to_string(),
                        quantity: 0,
                        items: None,
                    },
                }),
            web_acl_id: config.web_acl_id.clone().unwrap_or_default(),
            http_version: config
                .http_version
                .clone()
                .unwrap_or_else(|| DEFAULT_HTTP_VERSION.to_string()),
            is_ipv6_enabled: config.is_ipv6_enabled.unwrap_or_default(),
            alias_icp_recordals: None,
            staging: config.staging.unwrap_or_default(),
            connection_mode: config.connection_mode.clone(),
            anycast_ip_list_id: config.anycast_ip_list_id.clone(),
            viewer_mtls_config: config.viewer_mtls_config.clone(),
            connection_function_association: config.connection_function_association.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-cloudfront-cloudfront2020-05-31. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudfrontCloudfront20200531State {
    /// Every distribution this emulator holds, keyed by distribution id. A `BTreeMap` is
    /// what gives `ListDistributions` the stable total order its `Marker` indexes into.
    pub distributions: BTreeMap<String, StoredDistribution>,
    /// `DistributionConfig$CallerReference` -> distribution id, so a caller reference
    /// already associated with a distribution is detected without scanning the store.
    pub caller_references: HashMap<String, String>,
    /// The counter every generated id, domain name and version token is minted from, so
    /// no two mints collide and each mutation yields a FRESH `ETag`.
    pub next_token_seq: u64,
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// Apply every transition the clock owes at `now`, across the whole store.
    pub fn settle(&mut self, now: SystemTime) {
        for distribution in self.distributions.values_mut() {
            distribution.settle(now);
        }
    }

    /// The distribution `id` names, or `None` when the store holds none.
    pub fn distribution(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions.get(id)
    }

    /// A fresh value from the mint: a scrambled counter rendered in the id alphabet.
    ///
    /// The scramble is a bijection on `u64` and 36^13 exceeds 2^64, so distinct counter
    /// values always render as distinct tokens — the mint never repeats itself.
    fn mint(&mut self, len: usize) -> String {
        self.next_token_seq = self.next_token_seq.wrapping_add(1);
        render_token(scramble(self.next_token_seq), len)
    }

    /// Allocate a distribution id no stored distribution holds: `E` + 13 uppercase
    /// alphanumerics, the shape CloudFront distribution ids take.
    pub fn allocate_distribution_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.distributions.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate the domain name a distribution is served on: a generated label under
    /// `cloudfront.net`, in the lowercase DNS labels take.
    pub fn allocate_domain_name(&mut self) -> String {
        let label = self.mint(ID_BODY_LEN).to_lowercase();
        format!("d{label}.cloudfront.net")
    }

    /// Allocate an invalidation id no invalidation of the named distribution holds.
    pub fn allocate_invalidation_id(&mut self, distribution_id: &str) -> String {
        loop {
            let candidate = format!("I{}", self.mint(ID_BODY_LEN));
            let taken = self
                .distributions
                .get(distribution_id)
                .is_some_and(|distribution| distribution.invalidations.contains_key(&candidate));
            if !taken {
                break candidate;
            }
        }
    }

    /// Mint the version token a mutation hands back. Every call yields a fresh one, so
    /// an `If-Match` carrying the version from before a mutation no longer matches.
    ///
    /// Same shape as a distribution id (`E` + 13), which is the shape CloudFront's own
    /// `ETag` values take; it is drawn from the same counter, so no token this service
    /// mints is ever minted twice.
    pub fn next_etag(&mut self) -> String {
        format!("E{}", self.mint(ID_BODY_LEN))
    }

    /// Replace the configuration of the distribution `id` names, and report the record
    /// as it now stands.
    ///
    /// The stored configuration becomes the one submitted, `LastModifiedTime` becomes
    /// `now`, the version token is freshly minted, and the distribution goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline — the new configuration has
    /// to reach the edge locations before it is deployed again.
    ///
    /// `None` when the store holds no such distribution, checked BEFORE the mint so a
    /// version token is never spent on a distribution that is not there.
    pub fn apply_config(
        &mut self,
        id: &str,
        config: DistributionConfig,
        now: SystemTime,
    ) -> Option<&StoredDistribution> {
        if !self.distributions.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.distributions.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        stored.status = STATUS_IN_PROGRESS.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        Some(stored)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudfrontCloudfront20200531State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudfrontCloudfront20200531State>)
}

/// The number of summaries one `ListDistributions` response serves: the `MaxItems` the
/// request named, or [`MAX_ITEMS`] when it named none, bounded above by [`MAX_ITEMS`].
///
/// `None` when the request named a size that is no page at all (zero or negative), which
/// the caller answers with the modelled `InvalidArgument`.
pub fn page_size(max_items: Option<i32>) -> Option<usize> {
    let requested = max_items.unwrap_or(MAX_ITEMS);
    if requested < 1 {
        return None;
    }
    usize::try_from(requested.min(MAX_ITEMS)).ok()
}

/// The ARN of a distribution. CloudFront is a GLOBAL service (`arnNamespace`
/// `cloudfront`, no regional endpoint), so the ARN's region segment is empty:
/// `arn:aws:cloudfront::<account>:distribution/<Id>`.
pub fn distribution_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("distribution/{id}"))
}

/// The path of a distribution resource under this service's API version.
pub fn distribution_path(id: &str) -> String {
    format!("/2020-05-31/distribution/{id}")
}

/// The path of one invalidation of one distribution.
pub fn invalidation_path(distribution_id: &str, invalidation_id: &str) -> String {
    format!("/2020-05-31/distribution/{distribution_id}/invalidation/{invalidation_id}")
}

/// The fully qualified URI a `Location` header reports for `path`, built from the host
/// the request was addressed to so it names the endpoint the caller is already talking
/// to. Falls back to this service's own endpoint when the request carries no host.
pub fn location_uri(parts: &::http::request::Parts, path: &str) -> String {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| is_host_shaped(host))
        .unwrap_or("cloudfront.amazonaws.com");
    format!("https://{host}{path}")
}

/// Whether `value` has the shape of a host a URI can be built from.
///
/// The value is interpolated into a URI, so an arbitrary header value must not be able
/// to shape one: only a name of host characters, with at most one numeric `:port`
/// suffix, is accepted.
fn is_host_shaped(value: &str) -> bool {
    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (value, None),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    let port_ok = match port {
        None => true,
        Some(port) => {
            !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit())
        }
    };
    host_ok && port_ok
}

/// Whether `value` has the shape of a distribution id this service mints (`E` followed
/// by [`ID_BODY_LEN`] uppercase alphanumerics).
///
/// `ListDistributions`'s `Marker` is a distribution id, and the model constrains it no
/// further than `string`, so this is what tells a resume point the service issued from
/// a value a client made up.
pub fn is_distribution_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of a version token (`ETag`) this service mints.
///
/// `If-Match` carries one, and the model constrains it no further than `string`: a value
/// outside this shape was never a distribution version, which is a different failure
/// from a version that is simply no longer current.
///
/// DELIBERATELY the same predicate as [`is_distribution_id`]: CloudFront draws a
/// distribution id and a version token from one shape (`E` + 13), so the two namespaces
/// are indistinguishable by shape alone and nothing here can tell them apart either.
/// What separates them is the comparison that follows — a token is the CURRENT version
/// only if it equals the one stored.
pub fn is_version_token(value: &str) -> bool {
    is_distribution_id(value)
}

/// Whether `value` is `prefix` followed by [`ID_BODY_LEN`] characters of the id
/// alphabet.
fn is_prefixed_token(value: &str, prefix: char) -> bool {
    match value.strip_prefix(prefix) {
        Some(body) => body.len() == ID_BODY_LEN && body.bytes().all(is_token_byte),
        None => false,
    }
}

/// A byte of the id alphabet: a decimal digit or an uppercase ASCII letter.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_digit() || b.is_ascii_uppercase()
}

/// What an `If-Match` header says about a stored version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfMatch {
    /// The header carries the distribution's current version: the mutation may proceed.
    Current,
    /// The header is absent or carries something that is not a version token at all —
    /// `InvalidIfMatchVersion`.
    Invalid,
    /// The header carries a version token, but not the current one —
    /// `PreconditionFailed`.
    Stale,
}

/// Compare an `If-Match` header against a distribution's current version.
///
/// `InvalidIfMatchVersion` is documented as "the `If-Match` version is missing or not
/// valid", so an absent header and a value that is not a version token answer the same
/// way; a well-formed token that is not the current one is a failed precondition
/// instead. The value may be quoted, as HTTP entity tags normally are, or bare, as
/// CloudFront's own `ETag` header reports it.
pub fn check_if_match(current: &str, if_match: Option<&str>) -> IfMatch {
    let Some(raw) = if_match else {
        return IfMatch::Invalid;
    };
    let value = raw.trim().trim_start_matches("W/").trim_matches('"');
    if !is_version_token(value) {
        return IfMatch::Invalid;
    }
    if value == current {
        IfMatch::Current
    } else {
        IfMatch::Stale
    }
}

/// Whether two invalidation batches ask for the same thing, which is what decides
/// whether a repeated `CallerReference` is the same request again.
pub fn same_batch(left: &InvalidationBatch, right: &InvalidationBatch) -> bool {
    same_paths(&left.paths, &right.paths)
}

/// Whether two `Paths` name the same objects, in the same order.
fn same_paths(left: &Paths, right: &Paths) -> bool {
    left.quantity == right.quantity
        && left.items.as_deref().unwrap_or_default() == right.items.as_deref().unwrap_or_default()
}

/// How many entries an optional list carries. An absent list carries none, so its
/// wrapper's `Quantity` must be zero.
fn items_len<T>(items: &Option<Vec<T>>) -> usize {
    items.as_ref().map_or(0, Vec::len)
}

/// The `InconsistentQuantities` message for one wrapper whose `Quantity` disagrees with
/// the length of its `Items`, or `None` when the two agree.
fn quantity_disagreement(label: &str, quantity: i32, len: usize) -> Option<String> {
    if i64::from(quantity) == i64::try_from(len).unwrap_or(i64::MAX) {
        return None;
    }
    Some(format!(
        "The value of Quantity and the size of Items don't match: {label} states Quantity \
         {quantity} but carries {len} items"
    ))
}

/// The list wrappers one cache behaviour carries.
///
/// `DefaultCacheBehavior` and `CacheBehavior` declare the same ones, so both are
/// checked through this one view rather than through two copies of the same walk.
struct BehaviorLists<'a> {
    trusted_signers: Option<&'a TrustedSigners>,
    trusted_key_groups: Option<&'a TrustedKeyGroups>,
    allowed_methods: Option<&'a AllowedMethods>,
    lambda_function_associations: Option<&'a LambdaFunctionAssociations>,
    function_associations: Option<&'a FunctionAssociations>,
    forwarded_values: Option<&'a ForwardedValues>,
}

impl<'a> From<&'a DefaultCacheBehavior> for BehaviorLists<'a> {
    fn from(behavior: &'a DefaultCacheBehavior) -> Self {
        Self {
            trusted_signers: behavior.trusted_signers.as_ref(),
            trusted_key_groups: behavior.trusted_key_groups.as_ref(),
            allowed_methods: behavior.allowed_methods.as_ref(),
            lambda_function_associations: behavior.lambda_function_associations.as_ref(),
            function_associations: behavior.function_associations.as_ref(),
            forwarded_values: behavior.forwarded_values.as_ref(),
        }
    }
}

impl<'a> From<&'a CacheBehavior> for BehaviorLists<'a> {
    fn from(behavior: &'a CacheBehavior) -> Self {
        Self {
            trusted_signers: behavior.trusted_signers.as_ref(),
            trusted_key_groups: behavior.trusted_key_groups.as_ref(),
            allowed_methods: behavior.allowed_methods.as_ref(),
            lambda_function_associations: behavior.lambda_function_associations.as_ref(),
            function_associations: behavior.function_associations.as_ref(),
            forwarded_values: behavior.forwarded_values.as_ref(),
        }
    }
}

impl BehaviorLists<'_> {
    /// Record every wrapper this behaviour carries, under `label`.
    fn collect(&self, label: &str, counts: &mut Vec<(String, i32, usize)>) {
        if let Some(signers) = self.trusted_signers {
            counts.push((
                format!("{label}.TrustedSigners"),
                signers.quantity,
                items_len(&signers.items),
            ));
        }
        if let Some(groups) = self.trusted_key_groups {
            counts.push((
                format!("{label}.TrustedKeyGroups"),
                groups.quantity,
                items_len(&groups.items),
            ));
        }
        if let Some(methods) = self.allowed_methods {
            counts.push((
                format!("{label}.AllowedMethods"),
                methods.quantity,
                methods.items.len(),
            ));
            if let Some(cached) = &methods.cached_methods {
                counts.push((
                    format!("{label}.AllowedMethods.CachedMethods"),
                    cached.quantity,
                    cached.items.len(),
                ));
            }
        }
        if let Some(lambdas) = self.lambda_function_associations {
            counts.push((
                format!("{label}.LambdaFunctionAssociations"),
                lambdas.quantity,
                items_len(&lambdas.items),
            ));
        }
        if let Some(functions) = self.function_associations {
            counts.push((
                format!("{label}.FunctionAssociations"),
                functions.quantity,
                items_len(&functions.items),
            ));
        }
        if let Some(forwarded) = self.forwarded_values {
            if let Some(names) = &forwarded.cookies.whitelisted_names {
                counts.push((
                    format!("{label}.ForwardedValues.Cookies.WhitelistedNames"),
                    names.quantity,
                    items_len(&names.items),
                ));
            }
            if let Some(headers) = &forwarded.headers {
                counts.push((
                    format!("{label}.ForwardedValues.Headers"),
                    headers.quantity,
                    items_len(&headers.items),
                ));
            }
            if let Some(keys) = &forwarded.query_string_cache_keys {
                counts.push((
                    format!("{label}.ForwardedValues.QueryStringCacheKeys"),
                    keys.quantity,
                    items_len(&keys.items),
                ));
            }
        }
    }
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in `config`
/// whose `Quantity` disagrees with the length of its `Items`, or `None` when every
/// wrapper agrees.
///
/// Every CloudFront list travels in a wrapper that states its own length (`Quantity`)
/// beside the list itself (`Items`), and the service refuses a request where the two
/// disagree: "The value of Quantity and the size of Items don't match." The rule holds
/// for EVERY wrapper, not for a chosen few, so this walks all of the ones a
/// `DistributionConfig` reaches.
pub fn config_quantity_mismatch(config: &DistributionConfig) -> Option<String> {
    let mut counts: Vec<(String, i32, usize)> = Vec::new();

    if let Some(aliases) = &config.aliases {
        counts.push((
            "Aliases".to_string(),
            aliases.quantity,
            items_len(&aliases.items),
        ));
    }
    counts.push((
        "Origins".to_string(),
        config.origins.quantity,
        config.origins.items.len(),
    ));
    for origin in &config.origins.items {
        let label = format!("Origins.Items[{}]", origin.id);
        if let Some(headers) = &origin.custom_headers {
            counts.push((
                format!("{label}.CustomHeaders"),
                headers.quantity,
                items_len(&headers.items),
            ));
        }
        if let Some(protocols) = origin
            .custom_origin_config
            .as_ref()
            .and_then(|custom| custom.origin_ssl_protocols.as_ref())
        {
            counts.push((
                format!("{label}.CustomOriginConfig.OriginSslProtocols"),
                protocols.quantity,
                protocols.items.len(),
            ));
        }
    }
    if let Some(groups) = &config.origin_groups {
        counts.push((
            "OriginGroups".to_string(),
            groups.quantity,
            items_len(&groups.items),
        ));
        for group in groups.items.as_deref().unwrap_or_default() {
            let label = format!("OriginGroups.Items[{}]", group.id);
            let codes = &group.failover_criteria.status_codes;
            counts.push((
                format!("{label}.FailoverCriteria.StatusCodes"),
                codes.quantity,
                codes.items.len(),
            ));
            counts.push((
                format!("{label}.Members"),
                group.members.quantity,
                group.members.items.len(),
            ));
        }
    }
    BehaviorLists::from(&config.default_cache_behavior).collect("DefaultCacheBehavior", &mut counts);
    if let Some(behaviors) = &config.cache_behaviors {
        counts.push((
            "CacheBehaviors".to_string(),
            behaviors.quantity,
            items_len(&behaviors.items),
        ));
        for behavior in behaviors.items.as_deref().unwrap_or_default() {
            let label = format!("CacheBehaviors.Items[{}]", behavior.path_pattern);
            BehaviorLists::from(behavior).collect(&label, &mut counts);
        }
    }
    if let Some(responses) = &config.custom_error_responses {
        counts.push((
            "CustomErrorResponses".to_string(),
            responses.quantity,
            items_len(&responses.items),
        ));
    }
    if let Some(restrictions) = &config.restrictions {
        let geo = &restrictions.geo_restriction;
        counts.push((
            "Restrictions.GeoRestriction".to_string(),
            geo.quantity,
            items_len(&geo.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InconsistentQuantities` message for an invalidation batch whose
/// `Paths.Quantity` disagrees with the number of paths it carries, or `None`.
pub fn batch_quantity_mismatch(batch: &InvalidationBatch) -> Option<String> {
    quantity_disagreement(
        "Paths",
        batch.paths.quantity,
        items_len(&batch.paths.items),
    )
}

/// How many paths an invalidation batch names, which [`MAX_INVALIDATION_PATHS`] bounds.
pub fn batch_path_count(batch: &InvalidationBatch) -> usize {
    items_len(&batch.paths.items)
}

/// The modelled `NoSuchDistribution` for a distribution id the store does not hold.
pub fn no_such_distribution(id: &str) -> NoSuchDistribution {
    NoSuchDistribution {
        message: Some(format!("The specified distribution does not exist: {id}")),
    }
}

/// The modelled `NoSuchInvalidation` for an invalidation id a distribution does not
/// hold.
pub fn no_such_invalidation(id: &str) -> NoSuchInvalidation {
    NoSuchInvalidation {
        message: Some(format!("The specified invalidation does not exist: {id}")),
    }
}

/// Mix a counter so consecutive values render as unrelated tokens.
///
/// This is SplitMix64's finaliser applied to the counter times the 64-bit golden ratio.
/// Every step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the
/// whole function is one: distinct counters never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render `value` as `len` characters of the id alphabet, least significant first.
///
/// For `len >= 13` the rendering is injective on `u64` (36^13 > 2^64), which is what
/// makes a minted id or version token unique.
fn render_token(value: u64, len: usize) -> String {
    let mut rest = value;
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let digit = (rest % 36) as usize;
        rest /= 36;
        out.push(char::from(TOKEN_ALPHABET[digit]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mint never repeats a token: distinct counter values render distinctly, which
    /// is what makes two creates two distributions and every mutation a fresh version.
    #[test]
    fn minted_tokens_are_distinct() {
        let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1_000 {
            let id = state.allocate_distribution_id();
            assert!(
                is_distribution_id(&id),
                "case allocate_distribution_id: expected an `E`+13 id, got {id:?}"
            );
            assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated a token");
            let etag = state.next_etag();
            assert!(
                is_version_token(&etag),
                "case next_etag: expected an `E`+13 version token, got {etag:?}"
            );
            assert!(seen.insert(etag.clone()), "case {etag:?}: the mint repeated a token");
        }
    }

    /// An `If-Match` is current, stale or not a version at all — and nothing else.
    #[test]
    fn if_match_classifies_every_header_form() {
        let current = "E2QWRUHAPOMQZL";
        let cases: &[(Option<&str>, IfMatch)] = &[
            (None, IfMatch::Invalid),
            (Some(""), IfMatch::Invalid),
            (Some("not-a-version"), IfMatch::Invalid),
            (Some("E2QWRUHAPOMQZ"), IfMatch::Invalid),
            (Some(current), IfMatch::Current),
            (Some("\"E2QWRUHAPOMQZL\""), IfMatch::Current),
            (Some("E3QWRUHAPOMQZL"), IfMatch::Stale),
        ];
        for (header, expected) in cases {
            let actual = check_if_match(current, *header);
            assert_eq!(
                actual, *expected,
                "case If-Match={header:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
