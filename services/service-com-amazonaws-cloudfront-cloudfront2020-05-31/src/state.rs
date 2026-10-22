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
//! * an invalidation is `InProgress` until its batch finishes, and `Completed` after;
//! * a key value store CloudFront has just created is `PROVISIONING` until it has been
//!   provisioned, and `READY` after ([`KVS_PROVISIONING_DELAY`]).
//!
//! Both are materialised by [`ComAmazonawsCloudfrontCloudfront20200531State::settle`],
//! which every handler calls before it reads: the transition was owed at the same
//! instant whether or not anybody called, so applying it on a read never changes what a
//! later caller sees.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ActiveTrustedKeyGroups, ActiveTrustedSigners, AllowedMethods, CacheBehavior, CacheBehaviors,
    CachedMethods, CustomErrorResponse, CustomErrorResponses, CustomOriginConfig,
    DefaultCacheBehavior, Distribution, DistributionConfig, DistributionSummary,
    EntityAlreadyExists, EntityNotFound, ForwardedValues, FunctionAssociations, GeoRestriction,
    Invalidation, InvalidationBatch, KGKeyPairIds, KeyPairIds, KeyValueStore,
    LambdaFunctionAssociations, LoggingConfig, NoSuchDistribution, NoSuchInvalidation, Origin,
    OriginSslProtocols, Paths, Restrictions, Signer, Tags, TrustedKeyGroups, TrustedSigners,
    ViewerCertificate,
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

/// The most ordered cache behaviours ONE distribution's configuration may state.
///
/// CloudFront's published quota is 25 cache behaviours per distribution (the Amazon
/// CloudFront Developer Guide's "Quotas", general quotas on distributions). It is also
/// what bounds the per-request work a stored configuration costs: every `Distribution`
/// payload walks the default cache behaviour and each ordered one, so a configuration
/// cannot be stored with an unbounded number of them. Over it, the refusal is the modelled
/// `TooManyCacheBehaviors`: "You cannot create more cache behaviors for the distribution."
pub const MAX_CACHE_BEHAVIORS: usize = 25;

/// The most trusted signers ONE distribution's configuration may state, counted across
/// its cache behaviours.
///
/// CloudFront's published quota is 5 trusted signers (Amazon Web Services accounts) for a
/// distribution, and the modelled refusal is per distribution too: "Your request contains
/// more trusted signers than are allowed per distribution."
pub const MAX_TRUSTED_SIGNERS: usize = 5;

/// The most trusted key groups ONE distribution's configuration may state, counted across
/// its cache behaviours.
///
/// CloudFront's published quota is 4 key groups associated with a single distribution, and
/// the modelled refusal names the same scope: "The number of key groups referenced by this
/// distribution is more than the maximum allowed."
pub const MAX_TRUSTED_KEY_GROUPS: usize = 4;

/// The most paths ONE invalidation batch may name.
///
/// CloudFront's published quota for an invalidation request is 3,000 files, and the
/// batch is stored whole, so this bounds a single request's contribution to the store as
/// well. Over it, the refusal is the modelled `BatchTooLarge` (HTTP 413): "Invalidation
/// batch specified is too large."
pub const MAX_INVALIDATION_PATHS: usize = 3_000;

/// `KeyValueStore$Status` while CloudFront is still provisioning a key value store.
///
/// A store CloudFront has just accepted is not yet usable by a CloudFront function, so
/// the create answers this non-terminal status and the clock — not a later call — owes
/// the store [`KVS_STATUS_READY`].
pub const KVS_STATUS_PROVISIONING: &str = "PROVISIONING";

/// `KeyValueStore$Status` once provisioning has finished: the status a store has to be
/// in before its data plane (`cloudfront-keyvaluestore`, which serves a store in exactly
/// this terminal status) can serve a key.
pub const KVS_STATUS_READY: &str = "READY";

/// How long after the call that creates it a key value store is owed
/// [`KVS_STATUS_READY`].
///
/// CloudFront publishes no provisioning deadline, so — like [`PROPAGATION_DELAY`] — this
/// span is the EMULATOR's choice rather than a claim about the service: long enough that
/// a caller describing the store straight after the create still observes
/// [`KVS_STATUS_PROVISIONING`], short enough that a caller that waits observes the
/// terminal one. The instant is stored on the record; nothing ever sleeps for it.
pub const KVS_PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The most key value stores this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_DISTRIBUTIONS`]: a store record
/// is what `DescribeKeyValueStore` answers from, so it cannot be evicted, and the create
/// path is refused at this entry count with the modelled `EntityLimitExceeded` ("The
/// entity limit has been exceeded") instead of growing further. It matches the data
/// plane's own ceiling on distinct stores, so a store this service accepts is one that
/// service can also hold.
pub const MAX_KEY_VALUE_STORES: usize = 1_000;

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

/// `IsIPV6Enabled` for a configuration that names none: a new distribution answers IPv6
/// DNS requests with an IPv6 address ("Enable IPv6" is on for a new distribution — the
/// Amazon CloudFront Developer Guide's distribution settings), which is what `true`
/// means for `DistributionConfig$IsIPV6Enabled`.
pub const DEFAULT_IS_IPV6_ENABLED: bool = true;

/// `Staging` for a configuration that names none: a distribution a caller creates
/// directly is the primary distribution, not the staging distribution of a continuous
/// deployment policy.
pub const DEFAULT_STAGING: bool = false;

/// `Origin$ConnectionAttempts` for an origin that names none. `Origin$ConnectionAttempts`:
/// "The minimum number is 1, the maximum is 3, and the default (if you don't specify
/// otherwise) is 3."
pub const DEFAULT_CONNECTION_ATTEMPTS: i32 = 3;

/// `Origin$ConnectionTimeout` for an origin that names none, in seconds.
/// `Origin$ConnectionTimeout`: "The minimum timeout is 1 second, the maximum is 10
/// seconds, and the default (if you don't specify otherwise) is 10 seconds."
pub const DEFAULT_CONNECTION_TIMEOUT: i32 = 10;

/// `CustomOriginConfig$OriginReadTimeout` for a custom origin that names none, in
/// seconds: "The minimum timeout is 1 second, the maximum is 120 seconds, and the default
/// (if you don't specify otherwise) is 30 seconds."
pub const DEFAULT_ORIGIN_READ_TIMEOUT: i32 = 30;

/// `CustomOriginConfig$OriginKeepaliveTimeout` for a custom origin that names none, in
/// seconds: "The minimum timeout is 1 second, the maximum is 120 seconds, and the default
/// (if you don't specify otherwise) is 5 seconds."
pub const DEFAULT_ORIGIN_KEEPALIVE_TIMEOUT: i32 = 5;

/// `CustomOriginConfig$OriginSslProtocols` for a custom origin that names none: the
/// protocols CloudFront uses to connect to an origin over HTTPS when the configuration
/// chooses none. `OriginSslProtocols$Items` is an `SslProtocol` list, whose values the
/// member documents as "`SSLv3`, `TLSv1`, `TLSv1.1`, and `TLSv1.2`"; the three TLS ones
/// are what a configuration that names none gets, in protocol order.
pub const DEFAULT_ORIGIN_SSL_PROTOCOLS: [&str; 3] = ["TLSv1", "TLSv1.1", "TLSv1.2"];

/// `AllowedMethods$Items` (and `CachedMethods$Items`) for a cache behaviour that names
/// none: the read-only methods, which are the ones CloudFront forwards to the origin and
/// caches the response of unless a configuration asks for more.
pub const DEFAULT_ALLOWED_METHODS: [&str; 2] = ["HEAD", "GET"];

/// `SmoothStreaming` for a cache behaviour that names none: Microsoft Smooth Streaming is
/// something a configuration opts into.
pub const DEFAULT_SMOOTH_STREAMING: bool = false;

/// `Compress` for a cache behaviour that names none: automatic compression is something a
/// configuration opts into.
pub const DEFAULT_COMPRESS: bool = false;

/// `MinTTL` for a cache behaviour that caches on `ForwardedValues` and names no minimum,
/// in seconds: CloudFront forwards the next request to the origin as soon as the object is
/// stale, which is what a zero minimum means.
pub const DEFAULT_MIN_TTL: i64 = 0;

/// `DefaultTTL` for a cache behaviour that caches on `ForwardedValues` and names none, in
/// seconds: the one day the Amazon CloudFront Developer Guide documents as the default
/// TTL applied when the origin adds no `Cache-Control` or `Expires` header.
pub const DEFAULT_DEFAULT_TTL: i64 = 86_400;

/// `MaxTTL` for a cache behaviour that caches on `ForwardedValues` and names none, in
/// seconds: the one year the Amazon CloudFront Developer Guide documents as the default
/// maximum TTL.
pub const DEFAULT_MAX_TTL: i64 = 31_536_000;

/// `CustomErrorResponse$ErrorCachingMinTTL` for a custom error response that names none,
/// in seconds: the five minutes the Amazon CloudFront Developer Guide documents as how
/// long CloudFront caches an error response from the origin by default
/// ("Customizing error responses").
pub const DEFAULT_ERROR_CACHING_MIN_TTL: i64 = 300;

/// `ViewerCertificate$SSLSupportMethod` reported for a distribution served on CloudFront's
/// own certificate: such a distribution "accepts HTTPS connections from all viewers
/// including those that don't support SNI", which is what `vip` names.
pub const SSL_SUPPORT_METHOD_VIP: &str = "vip";

/// `ViewerCertificate$MinimumProtocolVersion` reported for a distribution served on
/// CloudFront's own certificate: "If the distribution uses the CloudFront domain name such
/// as `d111111abcdef8.cloudfront.net` (you set `CloudFrontDefaultCertificate` to `true`),
/// CloudFront automatically sets the security policy to `TLSv1` regardless of the value
/// that you set here."
pub const MINIMUM_PROTOCOL_VERSION_DEFAULT_CERTIFICATE: &str = "TLSv1";

/// `ViewerCertificate$CertificateSource` for a distribution served on CloudFront's own
/// certificate, from the `CertificateSource` enum.
pub const CERTIFICATE_SOURCE_CLOUDFRONT: &str = "cloudfront";

/// `ViewerCertificate$CertificateSource` for a distribution served on an ACM certificate.
pub const CERTIFICATE_SOURCE_ACM: &str = "acm";

/// `ViewerCertificate$CertificateSource` for a distribution served on an IAM server
/// certificate.
pub const CERTIFICATE_SOURCE_IAM: &str = "iam";

/// The single endpoint CloudFront's control API is served on.
///
/// CloudFront is a GLOBAL service with one API endpoint, so the fully qualified URI of a
/// resource it just created is that resource's path under this host — it is not derived
/// from the `Host` header of the request that created it.
pub const SERVICE_ENDPOINT: &str = "cloudfront.amazonaws.com";

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
    /// The configuration as submitted, with every member the service itself materialises
    /// resolved into it ([`resolve_distribution_config`]), stored whole so a later read
    /// returns the configuration the distribution actually runs under rather than a
    /// re-derivation of it.
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
            active_trusted_signers: Some(self.active_trusted_signers()),
            active_trusted_key_groups: Some(self.active_trusted_key_groups()),
            distribution_config: self.config.clone(),
            // An ICP recordal applies only to a distribution with alternate domain
            // names in the China partition, which this emulator does not serve.
            alias_icp_recordals: None,
        }
    }

    /// Every cache behaviour of the stored configuration, as the view that names the list
    /// wrappers they share: the default cache behaviour first, then the ordered ones.
    ///
    /// Bounded by [`MAX_CACHE_BEHAVIORS`], [`MAX_TRUSTED_SIGNERS`] and
    /// [`MAX_TRUSTED_KEY_GROUPS`], which [`config_limit_exceeded`] refuses a configuration
    /// over BEFORE it is stored — so the walks below cost a bounded amount of work on
    /// every read, whatever a request asked to store.
    fn behaviors(&self) -> Vec<BehaviorLists<'_>> {
        behavior_views(&self.config)
    }

    /// "A list of Amazon Web Services accounts and the active CloudFront key pairs in each
    /// account that CloudFront can use to verify the signatures of signed URLs and signed
    /// cookies."
    ///
    /// Read from the stored configuration, which is what says who the trusted signers are:
    /// `Enabled` is "true if any of the Amazon Web Services accounts in the list are
    /// configured as trusted signers", `Quantity` is "the number of Amazon Web Services
    /// accounts in the list", and the list itself is every account the distribution's cache
    /// behaviours trust, each named once in the order they first appear.
    ///
    /// A signer's `KeyPairIds` are the ACTIVE CloudFront key pairs of that account. No call
    /// in this emulator creates a CloudFront key pair (`CreatePublicKey` and the key-group
    /// operations still answer 501), so no account has one and the reported set is empty.
    pub fn active_trusted_signers(&self) -> ActiveTrustedSigners {
        let behaviors = self.behaviors();
        let accounts = first_seen(
            behaviors
                .iter()
                .filter_map(|behavior| behavior.trusted_signers)
                .flat_map(|signers| signers.items.as_deref().unwrap_or_default()),
        );
        ActiveTrustedSigners {
            enabled: behaviors
                .iter()
                .any(|behavior| behavior.trusted_signers.is_some_and(|signers| signers.enabled)),
            quantity: i32::try_from(accounts.len()).unwrap_or(i32::MAX),
            items: (!accounts.is_empty()).then(|| {
                accounts
                    .into_iter()
                    .map(|account| Signer {
                        aws_account_number: Some(account),
                        key_pair_ids: Some(KeyPairIds {
                            quantity: 0,
                            items: None,
                        }),
                    })
                    .collect()
            }),
        }
    }

    /// "A list of key groups, and the public keys in each key group, that CloudFront can
    /// use to verify the signatures of signed URLs and signed cookies."
    ///
    /// Read from the stored configuration the same way [`StoredDistribution::active_trusted_signers`]
    /// is: `Quantity` is "the number of key groups in the list" and the list is every key
    /// group the distribution's cache behaviours trust, each named once. A key group's
    /// public keys are empty for the same reason a signer's key pairs are — this emulator
    /// mints no public key — so `Enabled` reports whether the configuration trusts any key
    /// group at all.
    pub fn active_trusted_key_groups(&self) -> ActiveTrustedKeyGroups {
        let behaviors = self.behaviors();
        let groups = first_seen(
            behaviors
                .iter()
                .filter_map(|behavior| behavior.trusted_key_groups)
                .flat_map(|groups| groups.items.as_deref().unwrap_or_default()),
        );
        ActiveTrustedKeyGroups {
            enabled: behaviors
                .iter()
                .any(|behavior| behavior.trusted_key_groups.is_some_and(|groups| groups.enabled)),
            quantity: i32::try_from(groups.len()).unwrap_or(i32::MAX),
            items: (!groups.is_empty()).then(|| {
                groups
                    .into_iter()
                    .map(|group| KGKeyPairIds {
                        key_group_id: Some(group),
                        key_pair_ids: Some(KeyPairIds {
                            quantity: 0,
                            items: None,
                        }),
                    })
                    .collect()
            }),
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

/// One CloudFront key value store, as this emulator persists it.
///
/// The record is the single source of truth for the store's id, ARN, comment, status and
/// `LastModifiedTime`, so the create, a later describe and a later mutation all answer
/// from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredKeyValueStore {
    /// `KeyValueStore$Name` — "The name of the key value store." Unique per account, so
    /// it is also the key this store is held under.
    pub name: String,
    /// `KeyValueStore$Id` — "The unique Id for the key value store": the UUID minted at
    /// creation, which is also the resource part of [`StoredKeyValueStore::arn`].
    pub id: String,
    /// `KeyValueStore$ARN`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// `KeyValueStore$Comment`, as submitted. The member is required on the wire, so a
    /// create that named none reports the empty comment.
    pub comment: String,
    /// [`KVS_STATUS_PROVISIONING`] until [`StoredKeyValueStore::ready_due`], then
    /// [`KVS_STATUS_READY`].
    pub status: String,
    /// The instant this store is owed [`KVS_STATUS_READY`]. Stored rather than slept for.
    pub ready_due: SystemTime,
    /// `KeyValueStore$LastModifiedTime` — "The last-modified time of the key value
    /// store": when the resource last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this store.
    pub etag: String,
    /// The tags submitted with the create, held verbatim so a tagging operation reads
    /// what was stored rather than re-deriving it.
    pub tags: Option<Tags>,
}

impl StoredKeyValueStore {
    /// Apply the provisioning transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic: it only ever moves the non-terminal status to the
    /// terminal one, and only once the deadline has passed, so a read that calls it
    /// cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == KVS_STATUS_PROVISIONING && now >= self.ready_due {
            self.status = KVS_STATUS_READY.to_string();
        }
    }

    /// The `KeyValueStore` projection every key value store operation returns, read from
    /// the one stored record.
    pub fn to_key_value_store(&self) -> KeyValueStore {
        KeyValueStore {
            name: self.name.clone(),
            id: self.id.clone(),
            comment: self.comment.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
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
    /// Every key value store this emulator holds, keyed by its name — "In your account,
    /// the key value store names must be unique", which is what makes the name the key
    /// and a repeated name an `EntityAlreadyExists`. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through.
    pub key_value_stores: BTreeMap<String, StoredKeyValueStore>,
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// Apply every transition the clock owes at `now`, across the whole store.
    pub fn settle(&mut self, now: SystemTime) {
        for distribution in self.distributions.values_mut() {
            distribution.settle(now);
        }
        for key_value_store in self.key_value_stores.values_mut() {
            key_value_store.settle(now);
        }
    }

    /// The key value store `name` names, or `None` when the store holds none.
    pub fn key_value_store(&self, name: &str) -> Option<&StoredKeyValueStore> {
        self.key_value_stores.get(name)
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
        render_token(self.mint_u64(), len)
    }

    /// One fresh scrambled counter value — the single draw every mint is rendered from.
    fn mint_u64(&mut self) -> u64 {
        self.next_token_seq = self.next_token_seq.wrapping_add(1);
        scramble(self.next_token_seq)
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

    /// Allocate a key value store id no stored key value store holds.
    ///
    /// `KeyValueStore$Id` is "The unique Id for the key value store", and the identifier
    /// CloudFront mints for one is a UUID — the id that forms the resource part of the
    /// store's ARN, `arn:aws:cloudfront::<account>:key-value-store/<uuid>`, which is how
    /// the key value store DATA plane addresses the store. So this renders two draws of
    /// the mint (128 bits) as the 8-4-4-4-12 lowercase hex a UUID takes; distinct draws
    /// render distinctly, so the mint never repeats an id.
    pub fn allocate_key_value_store_id(&mut self) -> String {
        loop {
            let high = self.mint_u64();
            let low = self.mint_u64();
            let hex = format!("{high:016x}{low:016x}");
            let candidate = format!(
                "{}-{}-{}-{}-{}",
                &hex[..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            );
            if !self
                .key_value_stores
                .values()
                .any(|store| store.id == candidate)
            {
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
    /// The stored configuration becomes the one submitted with the service's own members
    /// resolved into it ([`resolve_distribution_config`]), `LastModifiedTime` becomes
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
        stored.config = resolve_distribution_config(config);
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

/// The ARN of a key value store. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:key-value-store/<Id>`. This is the
/// identifier the key value store DATA plane (`cloudfront-keyvaluestore`) addresses the
/// store by, so the resource part is the store's minted UUID id.
pub fn key_value_store_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("key-value-store/{id}"))
}

/// The path of a key value store resource under this service's API version. The
/// resource is addressed by NAME, as `DescribeKeyValueStore`'s `{Name}` label is.
pub fn key_value_store_path(name: &str) -> String {
    format!("/2020-05-31/key-value-store/{name}")
}

/// The fully qualified URI a `Location` header reports for `path`.
///
/// CloudFront's control API is GLOBAL and served on one endpoint
/// ([`SERVICE_ENDPOINT`]), so the URI of a resource it just created is that resource's
/// path under that endpoint — the same URI for every caller, rather than one derived
/// from the `Host` header the request happened to carry.
pub fn location_uri(path: &str) -> String {
    format!("https://{SERVICE_ENDPOINT}{path}")
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

/// The values of `values`, each kept once, at the position it first appears.
///
/// Membership goes through a set rather than a scan of what has been kept, so the cost is
/// linear in the number of entries (times the log of the distinct ones) however many
/// entries a configuration states.
fn first_seen<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen: BTreeSet<&'a str> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if seen.insert(value.as_str()) {
            out.push(value.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Resolving a submitted configuration into the one the distribution runs under
// ---------------------------------------------------------------------------

/// Resolve every member of a submitted `DistributionConfig` that the SERVICE supplies.
///
/// A `DistributionConfig` member a request leaves out is not unset on the distribution: it
/// takes the value the service applies, and the distribution then runs under — and reports
/// — the fully resolved configuration. So the resolution happens once, when the
/// configuration is STORED (by `CreateDistribution` and by
/// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_config`]), and every later read,
/// list and read-modify-write round trip answers from the resolved record rather than from
/// the request that created it.
///
/// Three kinds of member are resolved, and nothing else:
///
/// * a LIST WRAPPER (`Aliases`, `CustomHeaders`, `OriginGroups`, `CacheBehaviors`,
///   `CustomErrorResponses`, `TrustedSigners`, `TrustedKeyGroups`,
///   `LambdaFunctionAssociations`, `FunctionAssociations`) — an absent wrapper means the
///   list is empty, which is the wrapper stating `Quantity` 0;
/// * a member with a DOCUMENTED default (the origin timeouts and attempts, the TTLs,
///   `PriceClass`, `HttpVersion`, `Logging`'s four `@default` members, the geo restriction
///   type, the allowed methods, the origin SSL protocols) — the documented value;
/// * a member whose absence has exactly one meaning (`DefaultRootObject`, `OriginPath`,
///   `OriginAccessControlId`, `WebACLId`, `FieldLevelEncryptionId` and
///   `ContinuousDeploymentPolicyId` name nothing — the empty string; `OriginShield`,
///   `GrpcConfig`, `SmoothStreaming`, `Compress` and `Staging` are off; `ViewerCertificate`
///   is CloudFront's own certificate).
///
/// A member that is a CHOICE the configuration has not made (a cache policy id, a
/// real-time log configuration, an anycast ip list, a tenant or connection-mode setting)
/// is left absent: the service applies nothing to it.
pub fn resolve_distribution_config(mut config: DistributionConfig) -> DistributionConfig {
    config.aliases = Some(config.aliases.unwrap_or_default());
    config.default_root_object = Some(config.default_root_object.unwrap_or_default());

    let origins = std::mem::take(&mut config.origins.items);
    config.origins.items = origins.into_iter().map(resolve_origin).collect();
    config.origin_groups = Some(config.origin_groups.unwrap_or_default());

    config.default_cache_behavior =
        resolve_default_cache_behavior(std::mem::take(&mut config.default_cache_behavior));
    let mut behaviors = config.cache_behaviors.take().unwrap_or_default();
    if let Some(items) = behaviors.items.take() {
        behaviors.items = Some(items.into_iter().map(resolve_cache_behavior).collect());
    }
    config.cache_behaviors = Some(behaviors);

    let mut responses = config.custom_error_responses.take().unwrap_or_default();
    if let Some(items) = responses.items.take() {
        responses.items = Some(
            items
                .into_iter()
                .map(resolve_custom_error_response)
                .collect(),
        );
    }
    config.custom_error_responses = Some(responses);

    config.logging = Some(resolve_logging(config.logging.take()));
    config.price_class = Some(
        config
            .price_class
            .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
    );
    config.viewer_certificate = Some(resolve_viewer_certificate(config.viewer_certificate.take()));
    config.restrictions = Some(config.restrictions.unwrap_or_else(|| Restrictions {
        geo_restriction: GeoRestriction {
            restriction_type: GEO_RESTRICTION_NONE.to_string(),
            quantity: 0,
            items: None,
        },
    }));
    config.web_acl_id = Some(config.web_acl_id.unwrap_or_default());
    config.http_version = Some(
        config
            .http_version
            .unwrap_or_else(|| DEFAULT_HTTP_VERSION.to_string()),
    );
    config.is_ipv6_enabled = Some(config.is_ipv6_enabled.unwrap_or(DEFAULT_IS_IPV6_ENABLED));
    config.continuous_deployment_policy_id =
        Some(config.continuous_deployment_policy_id.unwrap_or_default());
    config.staging = Some(config.staging.unwrap_or(DEFAULT_STAGING));
    config
}

/// Resolve the members the service supplies on ONE origin.
fn resolve_origin(mut origin: Origin) -> Origin {
    origin.origin_path = Some(origin.origin_path.unwrap_or_default());
    origin.custom_headers = Some(origin.custom_headers.unwrap_or_default());
    origin.connection_attempts = Some(
        origin
            .connection_attempts
            .unwrap_or(DEFAULT_CONNECTION_ATTEMPTS),
    );
    origin.connection_timeout = Some(
        origin
            .connection_timeout
            .unwrap_or(DEFAULT_CONNECTION_TIMEOUT),
    );
    // "CloudFront Origin Shield" is off unless the configuration turns it on, and an
    // origin shield that is off names no region.
    origin.origin_shield = Some(origin.origin_shield.unwrap_or_default());
    origin.origin_access_control_id = Some(origin.origin_access_control_id.unwrap_or_default());
    origin.custom_origin_config = origin
        .custom_origin_config
        .map(resolve_custom_origin_config);
    origin
}

/// Resolve the members the service supplies on a custom origin's configuration.
fn resolve_custom_origin_config(mut custom: CustomOriginConfig) -> CustomOriginConfig {
    custom.origin_ssl_protocols = Some(custom.origin_ssl_protocols.unwrap_or_else(|| {
        OriginSslProtocols {
            quantity: i32::try_from(DEFAULT_ORIGIN_SSL_PROTOCOLS.len()).unwrap_or(i32::MAX),
            items: DEFAULT_ORIGIN_SSL_PROTOCOLS
                .iter()
                .map(|protocol| (*protocol).to_string())
                .collect(),
        }
    }));
    custom.origin_read_timeout = Some(
        custom
            .origin_read_timeout
            .unwrap_or(DEFAULT_ORIGIN_READ_TIMEOUT),
    );
    custom.origin_keepalive_timeout = Some(
        custom
            .origin_keepalive_timeout
            .unwrap_or(DEFAULT_ORIGIN_KEEPALIVE_TIMEOUT),
    );
    custom
}

/// Resolve the members the service supplies on a cache behaviour.
///
/// `DefaultCacheBehavior` and `CacheBehavior` are two Smithy structures declaring the SAME
/// members, so ONE macro resolves both — two copies of this walk would be two things to
/// keep in step.
macro_rules! resolve_behavior_members {
    ($behavior:expr) => {{
        let mut behavior = $behavior;
        // An absent trusted-signer or trusted-key-group wrapper is a behaviour that
        // requires no signed URL or cookie: not enabled, and nobody in the list.
        behavior.trusted_signers = Some(behavior.trusted_signers.unwrap_or_default());
        behavior.trusted_key_groups = Some(behavior.trusted_key_groups.unwrap_or_default());
        behavior.allowed_methods = Some(resolve_allowed_methods(behavior.allowed_methods));
        behavior.smooth_streaming = Some(behavior.smooth_streaming.unwrap_or(DEFAULT_SMOOTH_STREAMING));
        behavior.compress = Some(behavior.compress.unwrap_or(DEFAULT_COMPRESS));
        behavior.lambda_function_associations =
            Some(behavior.lambda_function_associations.unwrap_or_default());
        behavior.function_associations = Some(behavior.function_associations.unwrap_or_default());
        behavior.field_level_encryption_id =
            Some(behavior.field_level_encryption_id.unwrap_or_default());
        // "The gRPC configuration for your cache behavior" is off unless it is asked for.
        behavior.grpc_config = Some(behavior.grpc_config.unwrap_or_default());
        // The legacy cache-key members travel as ONE group: a behaviour that states its
        // cache key with `ForwardedValues` also gets the three TTLs resolved, while one
        // that attaches a cache policy states none of them and gets none.
        if let Some(forwarded) = behavior.forwarded_values {
            behavior.forwarded_values = Some(resolve_forwarded_values(forwarded));
            behavior.min_ttl = Some(behavior.min_ttl.unwrap_or(DEFAULT_MIN_TTL));
            behavior.default_ttl = Some(behavior.default_ttl.unwrap_or(DEFAULT_DEFAULT_TTL));
            behavior.max_ttl = Some(behavior.max_ttl.unwrap_or(DEFAULT_MAX_TTL));
        }
        behavior
    }};
}

/// Resolve the members the service supplies on the default cache behaviour.
fn resolve_default_cache_behavior(behavior: DefaultCacheBehavior) -> DefaultCacheBehavior {
    resolve_behavior_members!(behavior)
}

/// Resolve the members the service supplies on one ordered cache behaviour.
fn resolve_cache_behavior(behavior: CacheBehavior) -> CacheBehavior {
    resolve_behavior_members!(behavior)
}

/// The methods a cache behaviour allows and caches: the ones it names, or
/// [`DEFAULT_ALLOWED_METHODS`] when it names none. A behaviour that names the allowed
/// methods but not the cached ones caches the same default set.
fn resolve_allowed_methods(allowed: Option<AllowedMethods>) -> AllowedMethods {
    let mut allowed = allowed.unwrap_or_else(|| AllowedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
        cached_methods: None,
    });
    allowed.cached_methods = Some(allowed.cached_methods.unwrap_or_else(|| CachedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
    }));
    allowed
}

/// [`DEFAULT_ALLOWED_METHODS`] as a `MethodsList`.
fn default_methods() -> Vec<String> {
    DEFAULT_ALLOWED_METHODS
        .iter()
        .map(|method| (*method).to_string())
        .collect()
}

/// Resolve the list wrappers inside a legacy `ForwardedValues`: an absent `Headers` or
/// `QueryStringCacheKeys` forwards none and keys on none, which is the empty wrapper.
fn resolve_forwarded_values(mut forwarded: ForwardedValues) -> ForwardedValues {
    forwarded.headers = Some(forwarded.headers.unwrap_or_default());
    forwarded.query_string_cache_keys = Some(forwarded.query_string_cache_keys.unwrap_or_default());
    forwarded
}

/// Resolve the members the service supplies on one custom error response.
fn resolve_custom_error_response(mut response: CustomErrorResponse) -> CustomErrorResponse {
    response.error_caching_min_ttl = Some(
        response
            .error_caching_min_ttl
            .unwrap_or(DEFAULT_ERROR_CACHING_MIN_TTL),
    );
    response
}

/// Resolve the access-logging configuration. Every member of `LoggingConfig` carries a
/// Smithy `@default` — `Enabled` and `IncludeCookies` `false`, `Bucket` and `Prefix` the
/// empty string — which is also what a configuration that names no `Logging` at all gets:
/// "If you don't want to enable logging when you create a distribution ... specify `false`
/// for `Enabled`, and specify empty `Bucket` and `Prefix` elements."
fn resolve_logging(logging: Option<LoggingConfig>) -> LoggingConfig {
    let mut logging = logging.unwrap_or_default();
    logging.enabled = Some(logging.enabled.unwrap_or(false));
    logging.include_cookies = Some(logging.include_cookies.unwrap_or(false));
    logging.bucket = Some(logging.bucket.unwrap_or_default());
    logging.prefix = Some(logging.prefix.unwrap_or_default());
    logging
}

/// Resolve the distribution's viewer certificate.
///
/// A configuration that names no certificate of its own is served on CloudFront's own
/// certificate: "If the distribution doesn't use `Aliases` ... set
/// `CloudFrontDefaultCertificate` to `true` and leave all other fields empty." Under that
/// certificate the security policy is fixed — "CloudFront automatically sets the security
/// policy to `TLSv1` regardless of the value that you set here" — and the distribution
/// accepts HTTPS from every viewer, SNI or not ([`SSL_SUPPORT_METHOD_VIP`]). The
/// deprecated `Certificate` and `CertificateSource` members report which certificate is in
/// use and where it is kept, so they follow from the certificate the configuration chose.
fn resolve_viewer_certificate(certificate: Option<ViewerCertificate>) -> ViewerCertificate {
    let mut resolved = certificate.unwrap_or_default();
    let acm = resolved.acm_certificate_arn.clone();
    let iam = resolved.iam_certificate_id.clone();
    if acm.is_none() && iam.is_none() {
        resolved.cloud_front_default_certificate =
            Some(resolved.cloud_front_default_certificate.unwrap_or(true));
    }
    if resolved.cloud_front_default_certificate == Some(true) {
        resolved.ssl_support_method = Some(
            resolved
                .ssl_support_method
                .unwrap_or_else(|| SSL_SUPPORT_METHOD_VIP.to_string()),
        );
        resolved.minimum_protocol_version =
            Some(MINIMUM_PROTOCOL_VERSION_DEFAULT_CERTIFICATE.to_string());
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_CLOUDFRONT.to_string());
    } else if let Some(arn) = acm {
        resolved.certificate = Some(arn);
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_ACM.to_string());
    } else if let Some(id) = iam {
        resolved.certificate = Some(id);
        resolved.certificate_source = Some(CERTIFICATE_SOURCE_IAM.to_string());
    }
    resolved
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

/// A count in a submitted configuration that exceeds what this service stores, and the
/// message that says so. Each variant names the modelled error the handler answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLimit {
    /// More ordered cache behaviours than [`MAX_CACHE_BEHAVIORS`] —
    /// `TooManyCacheBehaviors`.
    CacheBehaviors(String),
    /// More trusted signers than [`MAX_TRUSTED_SIGNERS`] — `TooManyTrustedSigners`.
    TrustedSigners(String),
    /// More trusted key groups than [`MAX_TRUSTED_KEY_GROUPS`] —
    /// `TooManyKeyGroupsAssociatedToDistribution`.
    TrustedKeyGroups(String),
}

/// The first count in `config` that exceeds this service's maximum, or `None` when every
/// one of them is within it.
///
/// A stored configuration is walked on every `Distribution` payload — the default cache
/// behaviour and each ordered one, and the trusted signers and key groups they name — so a
/// configuration's counts bound the work every later read of that distribution does. They
/// are therefore checked on the way IN, before anything is stored, against the published
/// quotas ([`MAX_CACHE_BEHAVIORS`], [`MAX_TRUSTED_SIGNERS`], [`MAX_TRUSTED_KEY_GROUPS`]),
/// and refused with the error CloudFront models for each.
pub fn config_limit_exceeded(config: &DistributionConfig) -> Option<ConfigLimit> {
    let behaviors = config
        .cache_behaviors
        .as_ref()
        .map_or(0, |behaviors| items_len(&behaviors.items));
    if behaviors > MAX_CACHE_BEHAVIORS {
        return Some(ConfigLimit::CacheBehaviors(format!(
            "This distribution states {behaviors} cache behaviors; at most \
             {MAX_CACHE_BEHAVIORS} are allowed per distribution"
        )));
    }

    let lists = behavior_views(config);
    let signers: usize = lists
        .iter()
        .filter_map(|behavior| behavior.trusted_signers)
        .map(|signers| items_len(&signers.items))
        .sum();
    if signers > MAX_TRUSTED_SIGNERS {
        return Some(ConfigLimit::TrustedSigners(format!(
            "This distribution states {signers} trusted signers; at most \
             {MAX_TRUSTED_SIGNERS} are allowed per distribution"
        )));
    }

    let key_groups: usize = lists
        .iter()
        .filter_map(|behavior| behavior.trusted_key_groups)
        .map(|groups| items_len(&groups.items))
        .sum();
    if key_groups > MAX_TRUSTED_KEY_GROUPS {
        return Some(ConfigLimit::TrustedKeyGroups(format!(
            "This distribution references {key_groups} key groups; at most \
             {MAX_TRUSTED_KEY_GROUPS} are allowed per distribution"
        )));
    }

    None
}

/// Every cache behaviour of `config` as the view that names the list wrappers they share:
/// the default cache behaviour first, then the ordered ones.
fn behavior_views(config: &DistributionConfig) -> Vec<BehaviorLists<'_>> {
    let mut out = vec![BehaviorLists::from(&config.default_cache_behavior)];
    if let Some(behaviors) = &config.cache_behaviors {
        for behavior in behaviors.items.as_deref().unwrap_or_default() {
            out.push(BehaviorLists::from(behavior));
        }
    }
    out
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

/// The modelled `EntityNotFound` (HTTP 404) for a key value store name the store does
/// not hold — "The entity was not found."
pub fn key_value_store_not_found(name: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(format!(
            "The specified key value store does not exist: {name}"
        )),
    }
}

/// The modelled `EntityAlreadyExists` (HTTP 409) for a key value store name that is
/// already taken — "The entity already exists. You must provide a unique entity."
pub fn key_value_store_already_exists(name: &str) -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(format!(
            "A key value store with the name {name} already exists in this account"
        )),
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

    /// A minted key value store id is a UUID and never repeats: the id is the resource
    /// part of the store's ARN, which is how the data plane addresses one store.
    #[test]
    fn minted_key_value_store_ids_are_distinct_uuids() {
        let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1_000 {
            let id = state.allocate_key_value_store_id();
            let widths: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                widths,
                vec![8, 4, 4, 4, 12],
                "case allocate_key_value_store_id: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                id.bytes()
                    .all(|b| b == b'-' || (b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
                "case allocate_key_value_store_id: expected lowercase hex, got {id:?}"
            );
            assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated an id");
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
