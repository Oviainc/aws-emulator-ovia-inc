// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudfront-cloudfront2020-05-31 service.
//!
//! CloudFront's distribution surface is CUSTOMER-owned: every distribution a caller can
//! see was created by `CreateDistribution`, so this store starts EMPTY and every record
//! in it was put there by a mutating call. The store is the single source of truth for
//! each distribution's id, ARN, domain name, status, version token (`ETag`), its
//! configuration as submitted, the tag set associated with it, and the invalidations
//! created under it — so a create or update followed by a get, a list, a tag read or a
//! delete answers from the one record rather than from the request that is being served.
//!
//! The same holds for every other resource kind this store carries — a key value store, a
//! response headers policy, an origin request policy, a cache policy: each record is put
//! there by the call that creates it and is what every later read of that resource answers
//! from. None of a response headers policy, an origin request policy or a cache policy has
//! a status member or a propagation deadline, so nothing about one is owed by the clock.
//!
//! Two things in here are owed by the CLOCK rather than by a call:
//!
//! * a distribution CloudFront has just created or reconfigured is `InProgress` until
//!   its configuration has propagated, and is `Deployed` after
//!   ([`PROPAGATION_DELAY`]);
//! * an invalidation is `InProgress` until its batch finishes, and `Completed` after;
//! * a key value store CloudFront has just created is `PROVISIONING` until it has been
//!   provisioned, and `READY` after ([`KVS_PROVISIONING_DELAY`]);
//! * a distribution tenant CloudFront has just created is `InProgress` until its
//!   configuration has propagated, and `Deployed` after ([`PROPAGATION_DELAY`]).
//!
//! Both are materialised by [`ComAmazonawsCloudfrontCloudfront20200531State::settle`],
//! which every handler calls before it reads: the transition was owed at the same
//! instant whether or not anybody called, so applying it on a read never changes what a
//! later caller sees.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

// NOTE: the three error shapes this module's message constructors below build
// (`DistributionAlreadyExists`, `PreconditionFailed`, `InvalidArgument`) are named at
// their use sites through their full `crate::ops::types::` path rather than added to
// this list, which every operation added to this service edits.
use crate::ops::types::{
    ActiveTrustedKeyGroups, ActiveTrustedSigners, AllowedMethods, CacheBehavior, CacheBehaviors,
    CachePolicy, CachePolicyAlreadyExists, CachePolicyConfig,
    CachedMethods, CloudFrontOriginAccessIdentity, CloudFrontOriginAccessIdentityConfig,
    CNAMEAlreadyExists, ContinuousDeploymentPolicy, ContinuousDeploymentPolicyConfig,
    ContinuousDeploymentSingleHeaderConfig, ContinuousDeploymentSingleWeightConfig,
    CustomErrorResponse, CustomErrorResponses, CustomOriginConfig,
    Customizations, DefaultCacheBehavior, Distribution, DistributionConfig, DistributionSummary,
    DistributionTenant, DomainResult, EntityAlreadyExists, EntityNotFound, ForwardedValues,
    FunctionAssociations, GeoRestriction, Invalidation, InvalidationBatch, InvalidationSummary,
    InvalidTagging, KGKeyPairIds, KeyPairIds, KeyValueStore, LambdaFunctionAssociations,
    LoggingConfig, ManagedCertificateRequest, NoSuchCloudFrontOriginAccessIdentity,
    NoSuchCachePolicy, NoSuchDistribution, NoSuchInvalidation, NoSuchOriginRequestPolicy,
    NoSuchResource,
    NoSuchResponseHeadersPolicy, Origin, OriginRequestPolicy, OriginRequestPolicyAlreadyExists,
    OriginRequestPolicyConfig, OriginSslProtocols, Parameter, ParameterDefinition, Paths,
    ResponseHeadersPolicy,
    ResponseHeadersPolicyAlreadyExists, ResponseHeadersPolicyConfig, Restrictions,
    SessionStickinessConfig, Signer, StringSchemaConfig, Tag, Tags, TrafficConfig,
    TrustedKeyGroups, TrustedSigners, ViewerCertificate,
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

/// The message CloudFront answers `DistributionNotDisabled` with, on the wire.
///
/// One CONSTANT per error code rather than a sentence built from the request: this
/// condition is a property of the distribution's state, so nothing about the request
/// belongs in the text.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. It deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("The specified
/// CloudFront distribution is not disabled. You must disable the distribution before you
/// can delete it."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const DISTRIBUTION_NOT_DISABLED_MESSAGE: &str =
    "The distribution you are trying to delete has not been disabled.";

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

/// The most tags ONE CloudFront resource may carry.
///
/// CloudFront's published quota for tagging is 50 tags per distribution, and a tag set
/// is stored whole on the resource, so this bounds one resource's contribution to the
/// store as well. Over it, the refusal is the modelled `InvalidTagging`: "The tagging
/// specified is not valid."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most domains ONE distribution tenant may hold.
///
/// The model states the quota on `UpdateDistributionTenantRequest$Domains`: "You must
/// specify at least one domain. Each distribution tenant can have up to 5 domains"
/// (`smithy_query(query="doc",shape="UpdateDistributionTenantRequest",member="Domains")`).
/// `DomainList` carries no `@length`, so it is enforced here rather than by the modelled
/// constraints — which also bounds what one record stores and what the conflict scan over
/// the store walks. Over it, the refusal is the modelled `InvalidArgument`: "An argument
/// is invalid."
pub const MAX_DOMAINS_PER_TENANT: usize = 5;

/// The most parameter VALUES one distribution tenant may carry.
///
/// Every value a tenant carries must name a parameter the multi-tenant distribution
/// defines, and no name may repeat ([`parameter_refusal`]), so a longer list names
/// definitions no distribution carries. CloudFront publishes no quota for it, so the
/// number is the EMULATOR's bound — the same order as [`MAX_TAGS_PER_RESOURCE`], this
/// service's other per-resource key-value list — on what one request may submit and one
/// record may retain. Over it, the refusal is the modelled `InvalidArgument`.
pub const MAX_PARAMETERS_PER_TENANT: usize = 50;

/// The most locations one geographic restriction may name.
///
/// `GeoRestriction$Items`: "The `Location` element is a two-letter, uppercase country code
/// for a country that you want to include in your `blacklist` or `whitelist`. Include one
/// `Location` element for each country." There are 26 x 26 two-letter codes, so a list
/// naming more than that names no set of countries at all. The model carries no
/// `@length`, so the bound is enforced where a customization is accepted — which also
/// caps what one tenant record retains.
pub const MAX_GEO_RESTRICTION_LOCATIONS: usize = 26 * 26;

/// The most distribution tenants this emulator holds.
///
/// A tenant record is held for the whole life of the emulator (a later read has to answer
/// with what the create returned), so the store's growth is bounded here rather than left
/// unbounded. CloudFront's own published quota for distribution tenants is per account and
/// adjustable, so this is the EMULATOR's bound, not a claim about the service; over it the
/// refusal is the modelled `EntityLimitExceeded`: "The entity limit has been exceeded."
pub const MAX_DISTRIBUTION_TENANTS: usize = 10_000;

/// `DomainResult$Status` — "Whether the domain is active or inactive" — for a domain a
/// distribution tenant holds.
///
/// A domain becomes a tenant's by the call that associates it, which is state this
/// emulator really holds; it performs no DNS resolution and issues no certificate, so
/// there is no pending validation it could report and nothing in it moves a domain out of
/// this value. `DomainStatus`'s other value (`inactive`) would claim a validation that
/// nothing here runs.
pub const DOMAIN_STATUS_ACTIVE: &str = "active";

/// The prefix every distribution tenant id this service mints carries.
///
/// `GetDistributionTenant`'s `Identifier` is "the ARN, ID, or name of the distribution
/// tenant", so the three namespaces have to be told apart by shape: `DistributionTenant`'s
/// `Name` is `^[a-zA-Z0-9][a-zA-Z0-9-.]{1,126}[a-zA-Z0-9]$`, which admits no underscore,
/// and an ARN starts with `arn:` — so an id carrying an underscore can never be read as
/// either. The model states no id pattern, so the prefix is this emulator's choice.
pub const DISTRIBUTION_TENANT_ID_PREFIX: &str = "dt_";

/// The prefix every connection group id this service mints carries, told apart from a
/// tenant id and from a distribution id for the same reason
/// [`DISTRIBUTION_TENANT_ID_PREFIX`] is.
pub const CONNECTION_GROUP_ID_PREFIX: &str = "cg_";

/// `DistributionTenant$Enabled` for a create that named none.
///
/// "Indicates whether the distribution tenant should be enabled when created. If the
/// distribution tenant is disabled, the distribution tenant won't serve traffic" — the
/// member is optional, and a tenant is created to serve the domains the same call
/// associates with it, so leaving it out asks for the serving tenant.
pub const DEFAULT_TENANT_ENABLED: bool = true;

/// The longest a domain name may be: RFC 1035's 255-octet wire limit less the length and
/// root bytes, the bound every DNS name obeys.
pub const MAX_DOMAIN_NAME_LEN: usize = 253;

/// The longest one label of a domain name may be (RFC 1035).
pub const MAX_DOMAIN_LABEL_LEN: usize = 63;

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

/// The most continuous deployment policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_KEY_VALUE_STORES`]: a policy
/// record is what `GetContinuousDeploymentPolicy` answers from, so it cannot be evicted,
/// and the create path is refused at this entry count with the modelled
/// `TooManyContinuousDeploymentPolicies` ("You have reached the maximum number of
/// continuous deployment policies for this Amazon Web Services account") instead of
/// growing further. The VALUE is this emulator's bound rather than a published quota.
pub const MAX_CONTINUOUS_DEPLOYMENT_POLICIES: usize = 20;

/// The largest share of production traffic a continuous deployment policy may send to a
/// staging distribution.
///
/// `ContinuousDeploymentSingleWeightConfig$Weight` is documented as "The percentage of
/// traffic to send to a staging distribution, expressed as a decimal number between 0 and
/// 0.15", so a weight outside that closed interval is no weight this parameter takes.
pub const MAX_TRAFFIC_WEIGHT: f32 = 0.15;

/// The smallest share of production traffic the same member takes, from the same
/// sentence ("between 0 and 0.15").
pub const MIN_TRAFFIC_WEIGHT: f32 = 0.0;

/// The shortest session stickiness TTL a continuous deployment policy may name.
///
/// `SessionStickinessConfig$IdleTTL` and `$MaximumTTL` are each documented as "Allowed
/// values are 300–3600 seconds (5–60 minutes)", so this bound covers both members.
pub const MIN_SESSION_STICKINESS_TTL: i32 = 300;

/// The longest session stickiness TTL either member takes, from the same sentence.
pub const MAX_SESSION_STICKINESS_TTL: i32 = 3_600;

/// The most staging distribution domain names one continuous deployment policy names.
///
/// `StagingDistributionDnsNames` carries no `@length` in the model, so this is the
/// EMULATOR's bound rather than a published quota — the one caller-chosen LENGTH a
/// `ContinuousDeploymentPolicyConfig` carries, bounded on the way in like every other
/// count this service stores ([`MAX_CACHE_BEHAVIORS`], [`MAX_INVALIDATION_PATHS`]): the
/// list is persisted whole and later compared name by name against every stored policy,
/// so the work a create does is bounded by this times
/// [`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`] rather than by a length the request picks. It
/// is small because a continuous deployment policy routes traffic to ONE staging
/// distribution — the member's own documentation is singular ("The CloudFront domain name
/// of the staging distribution. For example: `d111111abcdef8.cloudfront.net`") — while
/// still admitting a handful, which is what keeps this a resource bound rather than a
/// fidelity claim about a count AWS refuses.
pub const MAX_STAGING_DISTRIBUTION_DNS_NAMES: usize = 10;

/// The most origin access identities this emulator holds.
///
/// CloudFront's published quota is 100 origin access identities per account, and the
/// operation models `TooManyCloudFrontOriginAccessIdentities` — "Processing your request
/// would cause you to exceed the maximum number of origin access identities allowed" —
/// for a create that would exceed it. An identity record is what a later read answers
/// from, so it cannot be evicted: the create path is refused at this entry count rather
/// than growing further.
pub const MAX_ORIGIN_ACCESS_IDENTITIES: usize = 100;

/// How many lowercase-hex characters an Amazon S3 canonical user id is written in.
///
/// `CloudFrontOriginAccessIdentity$S3CanonicalUserId` is "The Amazon S3 canonical user ID
/// for the origin access identity, used when giving the origin access identity read
/// permission to an object in Amazon S3", and S3 writes a canonical user id as 64
/// lowercase hex characters — the form a bucket ACL grantee takes, which is what makes
/// the shape part of the contract rather than cosmetic.
pub const S3_CANONICAL_USER_ID_LEN: usize = 64;

/// The most response headers policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_KEY_VALUE_STORES`]: a policy
/// record is what a later read answers from, so it cannot be evicted, and the create path
/// is refused at this entry count with the modelled `TooManyResponseHeadersPolicies`
/// ("You have reached the maximum number of response headers policies for this Amazon Web
/// Services account") instead of growing further. CloudFront's own ceiling is an account
/// quota that can be raised and the model states no number, so this bound is the
/// emulator's rather than a claim about the service.
pub const MAX_RESPONSE_HEADERS_POLICIES: usize = 1_000;

/// The most origin request policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_RESPONSE_HEADERS_POLICIES`]: a
/// policy record is what a later read answers from, so it cannot be evicted, and the
/// create path is refused at this entry count with the modelled
/// `TooManyOriginRequestPolicies` ("You have reached the maximum number of origin request
/// policies for this Amazon Web Services account") instead of growing further.
/// CloudFront's own ceiling is an account quota that can be raised and the model states no
/// number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_ORIGIN_REQUEST_POLICIES: usize = 1_000;

/// The most cache policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_ORIGIN_REQUEST_POLICIES`]: a
/// policy record is what a later read answers from, so it cannot be evicted, and the
/// create path is refused at this entry count with the modelled `TooManyCachePolicies`
/// ("You have reached the maximum number of cache policies for this Amazon Web Services
/// account") instead of growing further. CloudFront's own ceiling is an account quota that
/// can be raised and the model states no number, so this bound is the emulator's rather
/// than a claim about the service.
pub const MAX_CACHE_POLICIES: usize = 1_000;

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

/// The `ConnectionMode` value that makes a distribution MULTI-TENANT.
///
/// `DistributionConfig$ConnectionMode`: "This field specifies whether the connection mode
/// is through a standard distribution (direct) or a multi-tenant distribution with
/// distribution tenants (tenant-only)" — the model's `ConnectionMode` enum admits exactly
/// `direct` and `tenant-only`.
pub const CONNECTION_MODE_TENANT_ONLY: &str = "tenant-only";

/// The `DomainName` a MULTI-TENANT distribution reports.
///
/// A tenant-only distribution serves no traffic of its own: its viewers reach it through
/// the routing endpoint of a CONNECTION GROUP, and the distribution-level members that
/// would describe one (`Aliases`, `IsIPV6Enabled`, `AnycastIpListId`) are documented as
/// unsupported for it. So there is no `d<label>.cloudfront.net` domain to report, and
/// CloudFront answers the required `Distribution$DomainName` with this fixed placeholder
/// instead. Recorded behaviour — the model states no placeholder — but a fixed literal
/// rather than a generated value.
pub const MULTI_TENANT_DOMAIN_NAME: &str = "-";

/// The order the service emits a cache behaviour's HTTP method list in.
///
/// `MethodsList` is an ORDERED XML list of `<Method>` elements, so its element order is
/// part of the wire value — and CloudFront emits the methods of the chosen set in its own
/// fixed order rather than in the order the request listed them. This is the order
/// CloudFront's own responses carry for the full seven-method set; the smaller sets
/// `AllowedMethods$Quantity` admits ("2 (for `GET` and `HEAD` requests), 3 (for `GET`,
/// `HEAD`, and `OPTIONS` requests) and 7") come back as its subsequences — `HEAD, GET`
/// and `HEAD, GET, OPTIONS`, which is also the order [`DEFAULT_ALLOWED_METHODS`] is
/// written in. Recorded behaviour: the model fixes the member names (`Method`'s enum
/// values) but not the order they are emitted in.
pub const EMITTED_METHOD_ORDER: [&str; 7] = [
    "HEAD", "DELETE", "POST", "GET", "OPTIONS", "PUT", "PATCH",
];

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

/// The tag set one resource carries: tag key -> the tag's value, where a tag whose
/// `Value` the request left out carries none.
///
/// A map rather than the submitted list, because a resource carries at most ONE tag per
/// key; a `BTreeMap` is what makes the order a tag read reports deterministic, which
/// CloudFront leaves unspecified.
pub type TagSet = BTreeMap<String, Option<String>>;

/// The tag set `tags` describes, or the REASON a tag set no resource can carry was
/// refused.
///
/// Two rules the model cannot express: a resource carries at most one tag per key, so a
/// request naming one key twice describes no tag set at all; and a resource carries at
/// most [`MAX_TAGS_PER_RESOURCE`] tags. Each tag's key and value length and character
/// constraints ARE modelled (`TagKey`, `TagValue`), so those are already refused before
/// any handler runs.
///
/// ONE rule for every resource kind this service tags. The reason carries no error
/// wording of its own: each caller prefixes it with the wording of whichever refusal its
/// OWN model declares, so the message a client reads names the error it was given.
pub fn tag_set(tags: &Tags) -> Result<TagSet, String> {
    let items = tags.items.as_deref().unwrap_or_default();
    if items.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a CloudFront resource carries at most {MAX_TAGS_PER_RESOURCE} tags but the \
             request names {}",
            items.len()
        ));
    }
    let mut set = TagSet::new();
    for tag in items {
        // One lookup: a key the set already held is a request that does not describe a
        // tag set at all.
        if set.insert(tag.key.clone(), tag.value.clone()).is_some() {
            return Err(format!("the tag key {} is named more than once", tag.key));
        }
    }
    Ok(set)
}

/// The `Tags` projection a tag read returns for a stored tag set.
///
/// "A complex type that contains zero or more `Tag` elements": a resource with no tags
/// reports the empty list rather than nothing at all, so a caller reads the same shape
/// whether or not the resource is tagged. ONE projection for every resource kind this
/// service tags.
pub fn tags_projection(tags: &TagSet) -> Tags {
    Tags {
        items: Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        ),
    }
}

/// Add `added` to the tag set `into`, or report the REASON the resulting tag set is one
/// no resource can carry.
///
/// The ADD rule for every resource kind this service tags: a tagging call associates the
/// tags it names with the resource, so a key the resource already carries takes the value
/// the call names and every other key the resource carries is left exactly as it was —
/// adding tags is not replacing the tag set. The resulting set is still bounded by
/// [`MAX_TAGS_PER_RESOURCE`], counted over the UNION of the keys (a call that only
/// changes the values of keys the resource already carries adds nothing to the count).
///
/// All-or-nothing: the bound is decided before anything is written, so a refused add
/// leaves `into` exactly as it was. The reason carries no error wording of its own, for
/// the same reason [`tag_set`]'s does not: each caller prefixes it with the wording of
/// whichever refusal its own model declares.
pub fn merge_tags(into: &mut TagSet, added: TagSet) -> Result<(), String> {
    // The keys `added` contributes are the ones the resource does not carry yet, so the
    // union is counted without building a second set of every key.
    let resulting = into.len() + added.keys().filter(|key| !into.contains_key(*key)).count();
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a CloudFront resource carries at most {MAX_TAGS_PER_RESOURCE} tags but the \
             request would leave it carrying {resulting}"
        ));
    }
    into.extend(added);
    Ok(())
}

/// The modelled `InvalidTagging` for a tag set no resource can carry, wording `reason`
/// as that error documents itself — "The tagging specified is not valid."
pub fn invalid_tagging(reason: String) -> InvalidTagging {
    InvalidTagging {
        message: Some(format!("The tagging specified is not valid: {reason}")),
    }
}

/// The modelled `NoSuchResource` for an ARN no resource this emulator holds carries.
pub fn no_such_resource(arn: &str) -> NoSuchResource {
    NoSuchResource {
        message: Some(format!(
            "The specified CloudFront resource does not exist: {arn}"
        )),
    }
}

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

    /// The `InvalidationSummary` projection a list page carries — "a summary of an
    /// invalidation request" — read from the SAME stored record
    /// [`StoredInvalidation::to_invalidation`] reports, so a summary and a read of the
    /// same invalidation can never disagree about its status or its creation time.
    pub fn to_summary(&self) -> InvalidationSummary {
        InvalidationSummary {
            id: self.id.clone(),
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            status: self.status.clone(),
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
    /// The tags associated with this distribution. A distribution created without a tag
    /// set carries none, which is the empty set rather than an absent one.
    pub tags: TagSet,
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

    /// This distribution's invalidations in the order a list serves them.
    ///
    /// CloudFront documents the order on `ListInvalidations`'s `Marker`: "the results are
    /// returned in decreasing order from most recent to oldest, [so] the most recent
    /// results are on the first page". Two invalidations stamped with the SAME instant are
    /// ordered by id, which makes this a stable TOTAL order rather than merely a sorted
    /// one — and that is what lets a `Marker` naming one of them resume at exactly one
    /// position.
    ///
    /// Reads [`StoredInvalidation::status`] nowhere, so the caller decides whether to
    /// settle the clock first; every list handler does, because the summaries it builds
    /// from these records report that status.
    pub fn invalidations_most_recent_first(&self) -> Vec<&StoredInvalidation> {
        let mut ordered: Vec<&StoredInvalidation> = self.invalidations.values().collect();
        ordered.sort_by(|left, right| {
            right
                .create_time
                .cmp(&left.create_time)
                .then_with(|| left.id.cmp(&right.id))
        });
        ordered
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

    /// Whether a delete may remove this distribution.
    ///
    /// CloudFront's teardown is TWO steps, and the second one is a wait: you disable the
    /// distribution, and then — "wait until the value of the `Status` column for your
    /// distribution changes from `InProgress` to `Deployed`" — you delete it. So a
    /// distribution is deletable only once its stored configuration has it disabled AND
    /// that configuration has finished propagating; a delete issued while either is
    /// untrue is refused with `DistributionNotDisabled` and the record stays readable.
    ///
    /// Reads [`StoredDistribution::status`], so the caller settles the clock first.
    pub fn deletable(&self) -> bool {
        !self.config.enabled && self.status == STATUS_DEPLOYED
    }

    /// The `DomainName` this distribution reports.
    ///
    /// A standard distribution reports the edge domain allocated to it at creation; a
    /// multi-tenant one has no domain of its own and reports
    /// [`MULTI_TENANT_DOMAIN_NAME`]. Read from the STORED configuration, so every
    /// projection of one record \u2014 `Distribution` and `DistributionSummary` alike \u2014 reports
    /// the same value, and a configuration change is reflected by all of them at once.
    pub fn reported_domain_name(&self) -> String {
        if is_multi_tenant(&self.config) {
            MULTI_TENANT_DOMAIN_NAME.to_string()
        } else {
            self.domain_name.clone()
        }
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
            domain_name: self.reported_domain_name(),
            active_trusted_signers: Some(self.active_trusted_signers()),
            active_trusted_key_groups: Some(self.active_trusted_key_groups()),
            distribution_config: self.config.clone(),
            // An ICP recordal applies only to a distribution with alternate domain
            // names in the China partition, which this emulator does not serve.
            alias_icp_recordals: None,
        }
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
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
            domain_name: self.reported_domain_name(),
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
    /// The tags submitted with the create, held so a tagging operation reads what was
    /// stored rather than re-deriving it. The SAME representation a distribution's tags
    /// take, so one tagging rule covers both resource kinds.
    pub tags: TagSet,
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

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
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

/// One CloudFront continuous deployment policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, its configuration as
/// submitted, its `LastModifiedTime` and its version token, so the create, a later read
/// and a later listing all answer from it rather than from the request being served.
///
/// Nothing here is owed by the clock: `ContinuousDeploymentPolicy` carries no status
/// member, so a policy CloudFront has accepted is complete the moment it is stored.
#[derive(Debug, Clone)]
pub struct StoredContinuousDeploymentPolicy {
    /// `ContinuousDeploymentPolicy$Id`, minted at creation. Also the key this policy is
    /// held under, and the path segment `GetContinuousDeploymentPolicy` addresses it by.
    pub id: String,
    /// `ContinuousDeploymentPolicy$LastModifiedTime`: when the policy last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this policy.
    pub etag: String,
    /// `ContinuousDeploymentPolicyConfig` as submitted, stored whole: a later read
    /// answers from this record, never from the request that is being served.
    pub config: ContinuousDeploymentPolicyConfig,
}

impl StoredContinuousDeploymentPolicy {
    /// The `ContinuousDeploymentPolicy` projection every continuous deployment policy
    /// operation returns, read from the one stored record.
    pub fn to_policy(&self) -> ContinuousDeploymentPolicy {
        ContinuousDeploymentPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            continuous_deployment_policy_config: self.config.clone(),
        }
    }

    /// The staging distribution domain names this policy routes traffic to — the names
    /// that make a staging distribution one a policy already covers.
    pub fn staging_distribution_dns_names(&self) -> &[String] {
        staging_distribution_dns_names(&self.config)
    }
}

/// The staging distribution domain names a configuration names, in the order it states
/// them. An absent `Items` names none, whatever `Quantity` says.
pub fn staging_distribution_dns_names(config: &ContinuousDeploymentPolicyConfig) -> &[String] {
    config
        .staging_distribution_dns_names
        .items
        .as_deref()
        .unwrap_or_default()
}

/// One CloudFront distribution tenant, as this emulator persists it.
///
/// A tenant is "a distribution tenant" of ONE multi-tenant distribution: it holds the
/// domains its viewers reach, the parameter values that fill in the distribution's
/// `TenantConfig`, and the customizations that override the distribution's own settings.
/// The record is the single source of truth for every one of them, so the create, a later
/// read and a later mutation all answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredDistributionTenant {
    /// `DistributionTenant$Id`, allocated at creation and never reused.
    pub id: String,
    /// `DistributionTenant$Arn`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `DistributionTenant$Name` \u2014 "unique within your Amazon Web Services account. This
    /// name can't be updated after you create the distribution tenant."
    pub name: String,
    /// `DistributionTenant$DistributionId` \u2014 the multi-tenant distribution this tenant
    /// was created under.
    pub distribution_id: String,
    /// The domains associated with this tenant, in the order the create listed them. Each
    /// is reported as a `DomainResult` in [`DOMAIN_STATUS_ACTIVE`].
    pub domains: Vec<String>,
    /// `DistributionTenant$ConnectionGroupId` \u2014 the connection group the tenant's viewers
    /// are routed through: the one the create named, or the account's default one.
    pub connection_group_id: String,
    /// `DistributionTenant$Customizations`, exactly as submitted: these are "specific
    /// values that you can override or disable from the multi-tenant distribution", so the
    /// service supplies nothing of its own here.
    pub customizations: Option<Customizations>,
    /// `DistributionTenant$Parameters`, exactly as submitted \u2014 the values that fill in the
    /// distribution's `TenantConfig$ParameterDefinitions`.
    pub parameters: Vec<Parameter>,
    /// The `ManagedCertificateRequest` the create carried, held so the certificate surface
    /// (`GetManagedCertificateDetails`, still a scaffold) reads what was asked for. No
    /// certificate is issued and no validation state is reported for it: this emulator
    /// runs no ACM.
    pub managed_certificate_request: Option<ManagedCertificateRequest>,
    /// The tags submitted with the create, in the one representation every tagged
    /// resource in this store uses.
    pub tags: TagSet,
    /// `DistributionTenant$Enabled` \u2014 whether the tenant serves traffic.
    pub enabled: bool,
    /// [`STATUS_IN_PROGRESS`] until [`StoredDistributionTenant::deployed_due`], then
    /// [`STATUS_DEPLOYED`]: a tenant's configuration reaches the edge locations the same
    /// way its distribution's does.
    pub status: String,
    /// The instant this tenant's current configuration is owed to be deployed. Stored
    /// rather than slept for.
    pub deployed_due: SystemTime,
    /// `DistributionTenant$CreatedTime`.
    pub created_time: SystemTime,
    /// `DistributionTenant$LastModifiedTime`.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this tenant.
    pub etag: String,
}

impl StoredDistributionTenant {
    /// Apply the propagation transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, for the same reason
    /// [`StoredDistribution::settle`] is: it only ever moves the non-terminal status to the
    /// terminal one, and only once the deadline has passed, so a read that calls it cannot
    /// change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_IN_PROGRESS && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// Whether this tenant holds `domain`, compared as DNS names are.
    pub fn holds_domain(&self, domain: &str) -> bool {
        self.domains.iter().any(|held| same_domain(held, domain))
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `DistributionTenant` projection every distribution tenant operation returns,
    /// read from the one stored record.
    pub fn to_distribution_tenant(&self) -> DistributionTenant {
        DistributionTenant {
            id: Some(self.id.clone()),
            distribution_id: Some(self.distribution_id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            domains: Some(
                self.domains
                    .iter()
                    .map(|domain| DomainResult {
                        domain: domain.clone(),
                        status: Some(DOMAIN_STATUS_ACTIVE.to_string()),
                    })
                    .collect(),
            ),
            tags: Some(self.to_tags()),
            customizations: self.customizations.clone(),
            // A tenant that was created with no parameter values carries none, rather
            // than an empty list the request never described.
            parameters: (!self.parameters.is_empty()).then(|| self.parameters.clone()),
            connection_group_id: Some(self.connection_group_id.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(
                self.last_modified_time,
            )),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
        }
    }
}

/// One CloudFront origin access identity, as this emulator persists it.
///
/// The record is the single source of truth for the identity's id, its S3 canonical user
/// id, its version token and its configuration as submitted, so the create, a later
/// `GetCloudFrontOriginAccessIdentity` and a later `GetCloudFrontOriginAccessIdentityConfig`
/// all answer from it rather than from the request being served.
///
/// An identity carries NO status: CloudFront answers a create with the identity ready to
/// be named in a bucket policy, so there is no transition the clock owes one and nothing
/// here for [`ComAmazonawsCloudfrontCloudfront20200531State::settle`] to materialise.
#[derive(Debug, Clone)]
pub struct StoredOriginAccessIdentity {
    /// `CloudFrontOriginAccessIdentity$Id` — "The ID for the origin access identity":
    /// the token minted at creation, which is also the key this record is held under and
    /// the path segment its `Location` names.
    pub id: String,
    /// `CloudFrontOriginAccessIdentity$S3CanonicalUserId` — the canonical user id a
    /// bucket policy or ACL names to grant this identity read access, minted with the
    /// identity and fixed for its whole life.
    pub s3_canonical_user_id: String,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this identity.
    pub etag: String,
    /// The configuration as submitted. Both of its members are required on the wire, so
    /// there is nothing for the service to resolve into it: a later read answers with
    /// what was stored.
    pub config: CloudFrontOriginAccessIdentityConfig,
}

impl StoredOriginAccessIdentity {
    /// The `CloudFrontOriginAccessIdentity` projection every identity operation returns,
    /// read from the one stored record.
    pub fn to_identity(&self) -> CloudFrontOriginAccessIdentity {
        CloudFrontOriginAccessIdentity {
            id: self.id.clone(),
            s3_canonical_user_id: self.s3_canonical_user_id.clone(),
            cloud_front_origin_access_identity_config: Some(self.config.clone()),
        }
    }
}

/// One CloudFront response headers policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it
/// was created with, its `LastModifiedTime` and its version token, so the create and
/// every later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `ResponseHeadersPolicy` is `Id`,
/// `LastModifiedTime` and `ResponseHeadersPolicyConfig` only — a policy carries no status
/// and no propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredResponseHeadersPolicy {
    /// `ResponseHeadersPolicy$Id` — the identifier minted when the policy was created,
    /// and the key the policy is held under.
    pub id: String,
    /// The configuration as submitted — "Contains metadata about the response headers
    /// policy, and a set of configurations that specify the HTTP headers." Stored whole,
    /// so a read reports what was stored rather than re-deriving anything from a request.
    pub config: ResponseHeadersPolicyConfig,
    /// `ResponseHeadersPolicy$LastModifiedTime` — "The date and time when the response
    /// headers policy was last modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredResponseHeadersPolicy {
    /// The `ResponseHeadersPolicy` projection every response headers policy operation
    /// returns, read from the one stored record.
    pub fn to_response_headers_policy(&self) -> ResponseHeadersPolicy {
        ResponseHeadersPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            response_headers_policy_config: self.config.clone(),
        }
    }
}

/// One CloudFront origin request policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it was
/// created with, its `LastModifiedTime` and its version token, so the create and every
/// later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `OriginRequestPolicy` is `Id`,
/// `LastModifiedTime` and `OriginRequestPolicyConfig` only — a policy carries no status and
/// no propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredOriginRequestPolicy {
    /// `OriginRequestPolicy$Id` — the identifier minted when the policy was created, and
    /// the key the policy is held under.
    pub id: String,
    /// The configuration as submitted — "An origin request policy configuration." Stored
    /// whole, so a read reports what was stored rather than re-deriving anything from a
    /// request, down to the members the create left out.
    pub config: OriginRequestPolicyConfig,
    /// `OriginRequestPolicy$LastModifiedTime` — "The date and time when the origin request
    /// policy was last modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredOriginRequestPolicy {
    /// The `OriginRequestPolicy` projection every origin request policy operation returns,
    /// read from the one stored record.
    pub fn to_origin_request_policy(&self) -> OriginRequestPolicy {
        OriginRequestPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            origin_request_policy_config: self.config.clone(),
        }
    }
}

/// One CloudFront cache policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it was
/// created with, its `LastModifiedTime` and its version token, so the create and every
/// later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `CachePolicy` is `Id`,
/// `LastModifiedTime` and `CachePolicyConfig` only — a policy carries no status and no
/// propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredCachePolicy {
    /// `CachePolicy$Id` — the identifier minted when the policy was created, and the key
    /// the policy is held under.
    pub id: String,
    /// The configuration as submitted — "A cache policy configuration." Stored whole, so
    /// a read reports what was stored rather than re-deriving anything from a request,
    /// down to the members the create left out.
    pub config: CachePolicyConfig,
    /// `CachePolicy$LastModifiedTime` — "The date and time when the cache policy was last
    /// modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredCachePolicy {
    /// The `CachePolicy` projection every cache policy operation returns, read from the
    /// one stored record.
    pub fn to_cache_policy(&self) -> CachePolicy {
        CachePolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            cache_policy_config: self.config.clone(),
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
    /// Every continuous deployment policy this emulator holds, keyed by its id — the
    /// identifier `GetContinuousDeploymentPolicy` addresses one by. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for
    /// `ListContinuousDeploymentPolicies`'s `Marker` to index into.
    pub continuous_deployment_policies: BTreeMap<String, StoredContinuousDeploymentPolicy>,
    /// Every distribution tenant this emulator holds, keyed by tenant id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order a listing pages
    /// through.
    pub distribution_tenants: BTreeMap<String, StoredDistributionTenant>,
    /// The id of the account's DEFAULT connection group, once something has needed one.
    ///
    /// "When you first create a distribution tenant and you don't specify a connection
    /// group, CloudFront will automatically create a default connection group for you" —
    /// so it is minted ONCE, by the first create that needs it, and every later tenant
    /// that names no connection group is routed through that same one.
    pub default_connection_group_id: Option<String>,
    /// Every origin access identity this emulator holds, keyed by its id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_access_identities: BTreeMap<String, StoredOriginAccessIdentity>,
    /// Every response headers policy this emulator holds, keyed by its minted id — the
    /// identifier every response headers policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub response_headers_policies: BTreeMap<String, StoredResponseHeadersPolicy>,
    /// Every origin request policy this emulator holds, keyed by its minted id — the
    /// identifier every origin request policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_request_policies: BTreeMap<String, StoredOriginRequestPolicy>,
    /// Every cache policy this emulator holds, keyed by its minted id — the identifier
    /// every cache policy operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through.
    pub cache_policies: BTreeMap<String, StoredCachePolicy>,
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
        for tenant in self.distribution_tenants.values_mut() {
            tenant.settle(now);
        }
    }

    /// The key value store `name` names, or `None` when the store holds none.
    pub fn key_value_store(&self, name: &str) -> Option<&StoredKeyValueStore> {
        self.key_value_stores.get(name)
    }

    /// The origin access identity `id` names, or `None` when the store holds none.
    pub fn origin_access_identity(&self, id: &str) -> Option<&StoredOriginAccessIdentity> {
        self.origin_access_identities.get(id)
    }

    /// The origin access identity created under `caller_reference`, or `None` when no
    /// identity was.
    ///
    /// A scan over the identities rather than a second index beside them: the stored
    /// records are the single source of truth for which references are taken, so a scan
    /// cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_ORIGIN_ACCESS_IDENTITIES`].
    pub fn origin_access_identity_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredOriginAccessIdentity> {
        self.origin_access_identities
            .values()
            .find(|identity| identity.config.caller_reference == caller_reference)
    }

    /// The distribution `id` names, or `None` when the store holds none.
    pub fn distribution(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions.get(id)
    }

    /// The continuous deployment policy `id` names, or `None` when the store holds none.
    pub fn continuous_deployment_policy(
        &self,
        id: &str,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        self.continuous_deployment_policies.get(id)
    }

    /// The policy that already routes traffic to one of `dns_names`, or `None` when no
    /// stored policy names any of them.
    ///
    /// `StagingDistributionInUse` is documented as "A continuous deployment policy for
    /// this staging distribution already exists", so a staging distribution takes part in
    /// at most ONE policy and this is what decides whether a create may claim it.
    ///
    /// The request's names are indexed into a set ONCE, so the scan costs one set lookup
    /// per stored name rather than a walk of the request's list per stored name: the work
    /// is linear in the two lengths, each of which is bounded
    /// ([`MAX_STAGING_DISTRIBUTION_DNS_NAMES`] per policy,
    /// [`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`] policies).
    pub fn policy_using_staging_distribution(
        &self,
        dns_names: &[String],
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        let claimed: BTreeSet<&str> = dns_names.iter().map(String::as_str).collect();
        self.continuous_deployment_policies.values().find(|policy| {
            policy
                .staging_distribution_dns_names()
                .iter()
                .any(|held| claimed.contains(held.as_str()))
        })
    }

    /// The policy whose stored configuration is `config`, or `None` when none is.
    ///
    /// `ContinuousDeploymentPolicyAlreadyExists` is documented as "A continuous deployment
    /// policy with this configuration already exists", so this is what separates a create
    /// that repeats a policy already in the store from one that claims a staging
    /// distribution another policy holds.
    pub fn policy_with_config(
        &self,
        config: &ContinuousDeploymentPolicyConfig,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        self.continuous_deployment_policies
            .values()
            .find(|policy| same_policy_config(&policy.config, config))
    }

    /// The distribution tenant `identifier` names, or `None` when the store holds none.
    ///
    /// A distribution tenant operation names its tenant by "the ARN, ID, or name of the
    /// distribution tenant", so all three resolve to the one record: the id is the store's
    /// key, and the ARN and the name are compared against the STORED values rather than
    /// re-derived out of the request. The three namespaces cannot overlap — see
    /// [`DISTRIBUTION_TENANT_ID_PREFIX`] — so at most one of them matches.
    pub fn distribution_tenant(&self, identifier: &str) -> Option<&StoredDistributionTenant> {
        if let Some(tenant) = self.distribution_tenants.get(identifier) {
            return Some(tenant);
        }
        self.distribution_tenants
            .values()
            .find(|tenant| tenant.arn == identifier || tenant.name == identifier)
    }

    /// Whether `domain` is already associated with a CloudFront resource this store holds:
    /// a domain of a distribution tenant other than `except_tenant_id`, or an alias of a
    /// distribution.
    ///
    /// "The CNAME specified is already defined for CloudFront": one domain name routes to
    /// at most one CloudFront resource, so the check spans every resource kind that can
    /// claim one. `except_tenant_id` is the tenant a mutation is rewriting, whose own
    /// domains are no conflict with itself.
    pub fn domain_in_use(&self, domain: &str, except_tenant_id: Option<&str>) -> bool {
        let held_by_tenant = self.distribution_tenants.values().any(|tenant| {
            Some(tenant.id.as_str()) != except_tenant_id && tenant.holds_domain(domain)
        });
        held_by_tenant
            || self.distributions.values().any(|distribution| {
                distribution
                    .config
                    .aliases
                    .as_ref()
                    .and_then(|aliases| aliases.items.as_deref())
                    .unwrap_or_default()
                    .iter()
                    .any(|alias| same_domain(alias, domain))
            })
    }

    /// Whether `id` names a connection group this store holds.
    ///
    /// The only connection group this emulator holds is the account's default one
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::default_connection_group`]);
    /// `CreateConnectionGroup` is still a scaffold, so no other one can have been created.
    /// When it lands it extends this lookup with its own store rather than replacing the
    /// rule: an id no connection group carries names no entity.
    pub fn holds_connection_group(&self, id: &str) -> bool {
        self.default_connection_group_id.as_deref() == Some(id)
    }

    /// The response headers policy `id` names, or `None` when the store holds none.
    pub fn response_headers_policy(&self, id: &str) -> Option<&StoredResponseHeadersPolicy> {
        self.response_headers_policies.get(id)
    }

    /// The response headers policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `ResponseHeadersPolicyConfig$Name`: "The name must be unique for response headers
    /// policies in this Amazon Web Services account" — so at most one policy answers,
    /// and this is what makes a repeated name a conflict rather than a second policy.
    /// The comparison is against the stored configuration, never against a request.
    pub fn response_headers_policy_named(
        &self,
        name: &str,
    ) -> Option<&StoredResponseHeadersPolicy> {
        self.response_headers_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The origin request policy `id` names, or `None` when the store holds none.
    pub fn origin_request_policy(&self, id: &str) -> Option<&StoredOriginRequestPolicy> {
        self.origin_request_policies.get(id)
    }

    /// The cache policy `id` names, or `None` when the store holds none.
    pub fn cache_policy(&self, id: &str) -> Option<&StoredCachePolicy> {
        self.cache_policies.get(id)
    }

    /// The cache policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `CachePolicyConfig$Name` is "A unique name to identify the cache policy", and the
    /// create models `CachePolicyAlreadyExists` for a name another policy already holds —
    /// so at most one policy answers, and this is what makes a repeated name a conflict
    /// rather than a second policy. The comparison is against the stored configuration,
    /// never against a request.
    pub fn cache_policy_named(&self, name: &str) -> Option<&StoredCachePolicy> {
        self.cache_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The origin request policy whose STORED configuration carries `name`, or `None`.
    ///
    /// `OriginRequestPolicyConfig$Name` is "A unique name to identify the origin request
    /// policy", and the create models `OriginRequestPolicyAlreadyExists` for a name
    /// another policy already holds — so at most one policy answers, and this is what
    /// makes a repeated name a conflict rather than a second policy. The comparison is
    /// against the stored configuration, never against a request.
    pub fn origin_request_policy_named(
        &self,
        name: &str,
    ) -> Option<&StoredOriginRequestPolicy> {
        self.origin_request_policies
            .values()
            .find(|policy| policy.config.name == name)
    }

    /// The tag set of the resource `arn` names, across every resource kind this store
    /// holds, or `None` when it holds no such resource.
    ///
    /// A tagging call names its resource by ARN rather than by id or name, and the ARN a
    /// resource carries is the one minted when it was created, so the lookup compares
    /// against the STORED value rather than re-deriving an identifier out of the
    /// request. An ARN naming a kind of CloudFront resource this emulator does not hold
    /// matches nothing, which is exactly what makes it a resource that does not exist.
    pub fn tags_for_resource(&self, arn: &str) -> Option<&TagSet> {
        if let Some(distribution) = self
            .distributions
            .values()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&distribution.tags);
        }
        if let Some(store) = self.key_value_stores.values().find(|store| store.arn == arn) {
            return Some(&store.tags);
        }
        self.distribution_tenants
            .values()
            .find(|tenant| tenant.arn == arn)
            .map(|tenant| &tenant.tags)
    }

    /// The tag set of the resource `arn` names, MUTABLY, so a tagging call writes the
    /// very set a tag read reports rather than a second copy of it.
    ///
    /// Resolves the ARN exactly as [`ComAmazonawsCloudfrontCloudfront20200531State::tags_for_resource`]
    /// does — across every resource kind this store holds, comparing against the STORED
    /// ARN — so a tagging call and a tag read can never disagree about which resource an
    /// ARN names. `None` is an ARN this emulator holds no resource for.
    pub fn tags_for_resource_mut(&mut self, arn: &str) -> Option<&mut TagSet> {
        if let Some(distribution) = self
            .distributions
            .values_mut()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&mut distribution.tags);
        }
        self.key_value_stores
            .values_mut()
            .find(|store| store.arn == arn)
            .map(|store| &mut store.tags)
    }

    /// Persist a new distribution carrying `config` and `tags`, and report the record
    /// that was stored — or, when the caller reference already names a distribution
    /// carrying this very configuration, report THAT record and store nothing.
    ///
    /// The ONE create path. `CreateDistribution` and `CreateDistributionWithTags` create
    /// the same resource under the same rules and differ only in whether the request
    /// carries a tag set, so the configuration check, the caller-reference rule, the
    /// store bound, the mint and the insert all live here and cannot drift apart.
    ///
    /// Every refusal is decided BEFORE anything is claimed, so a refused create leaves
    /// the store exactly as it was: no id, domain name, version token or caller
    /// reference is spent on it.
    pub fn create_distribution(
        &mut self,
        config: DistributionConfig,
        tags: TagSet,
        now: SystemTime,
    ) -> Result<CreatedDistribution, CreateRefusal> {
        // "The value of Quantity and the size of Items don't match." A malformed
        // configuration is refused before any id or caller reference is claimed for it.
        if let Some(message) = config_quantity_mismatch(&config) {
            return Err(CreateRefusal::InconsistentQuantities(message));
        }

        // A configuration is walked on every payload that reports it, so the counts that
        // drive that walk are bounded by CloudFront's published quotas on the way IN,
        // each refused with the error the model names for it.
        if let Some(limit) = config_limit_exceeded(&config) {
            return Err(CreateRefusal::ConfigLimit(limit));
        }

        // A caller reference identifies ONE distribution for its whole life, and what it
        // takes part in is this: repeating a create under a reference the store already
        // holds is either the SAME request again or a second distribution under a taken
        // reference. What separates the two is the content of the configuration, which is
        // what the recorded service does on both branches: a repeat whose configuration is
        // the one that reference already names is answered with that distribution, its
        // version and its location (creating nothing), and a repeat that changes the
        // configuration is refused — "The caller reference you attempted to create the
        // distribution with is associated with another distribution."
        // [`ComAmazonawsCloudfrontCloudfront20200531State::create_invalidation`]'s caller
        // reference follows the same rule for an invalidation batch.
        let caller_reference = config.caller_reference.clone();
        // The configuration as the service will run it: the members submitted plus every
        // member the service itself supplies. Resolved BEFORE the caller-reference
        // question so the comparison below is one between like and like — the stored
        // record is resolved too — and a repeat that states a default explicitly and one
        // that leaves it out are the same request, as they describe the same distribution.
        let config = resolve_distribution_config(config);
        // The reference index and the store are written together — a delete releases both
        // — so an entry that names no stored distribution cannot arise; were one to, the
        // reference it indexes is associated with nothing and this create claims it.
        if let Some(existing_id) = self.caller_references.get(&caller_reference).cloned() {
            if let Some(existing) = self.distribution(&existing_id) {
                if same_config(&existing.config, &config) {
                    // The same request again: the record the first create left behind, at
                    // its current version. Nothing is minted and nothing is stored — so a
                    // tag set this repeat carries changes nothing either, the distribution
                    // keeping the tags it was created with.
                    return Ok(CreatedDistribution {
                        id: existing.id.clone(),
                        etag: existing.etag.clone(),
                        distribution: existing.to_distribution(),
                    });
                }
                return Err(CreateRefusal::DistributionAlreadyExists(
                    distribution_already_exists_message(&existing_id),
                ));
            }
        }

        // "Processing your request would cause you to exceed the maximum number of
        // distributions allowed." The store grows only here and a distribution cannot be
        // evicted (a later Get has to answer with what this create returned), so the
        // bound is an entry count refused BEFORE anything is claimed.
        if self.distributions.len() >= MAX_DISTRIBUTIONS {
            return Err(CreateRefusal::TooManyDistributions(format!(
                "This emulator holds at most {MAX_DISTRIBUTIONS} distributions; delete one \
                 before creating another"
            )));
        }

        let id = self.allocate_distribution_id();
        let domain_name = self.allocate_domain_name();
        let etag = self.next_etag();
        let stored = StoredDistribution {
            arn: distribution_arn(&id),
            id: id.clone(),
            domain_name,
            // A distribution CloudFront has just accepted is not yet propagated to the
            // edge locations, so it reports the transient status until the clock owes it
            // the terminal one.
            status: STATUS_IN_PROGRESS.to_string(),
            deployed_due: now + PROPAGATION_DELAY,
            last_modified_time: now,
            etag: etag.clone(),
            // Stored whole, with every member the service itself supplies resolved into
            // it: a later read answers from this record — the configuration the
            // distribution actually runs under — never from the request that was served.
            config,
            tags,
            invalidations: BTreeMap::new(),
        };
        let created = CreatedDistribution {
            id: id.clone(),
            etag,
            distribution: stored.to_distribution(),
        };
        self.caller_references.insert(caller_reference, id.clone());
        self.distributions.insert(id, stored);
        Ok(created)
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
            let candidate = self.mint_uuid();
            if !self
                .key_value_stores
                .values()
                .any(|store| store.id == candidate)
            {
                break candidate;
            }
        }
    }

    /// Allocate a continuous deployment policy id no stored policy holds.
    ///
    /// The same UUID mint [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`]
    /// serves every resource CloudFront identifies by one: the policy-shaped resources of
    /// its control plane (cache policies, origin request policies, continuous deployment
    /// policies) carry a UUID rather than the `E`+13 token a distribution carries.
    pub fn allocate_continuous_deployment_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self
                .continuous_deployment_policies
                .contains_key(&candidate)
            {
                break candidate;
            }
        }
    }

    /// Allocate a distribution tenant id no stored tenant holds:
    /// [`DISTRIBUTION_TENANT_ID_PREFIX`] followed by a minted token.
    pub fn allocate_distribution_tenant_id(&mut self) -> String {
        loop {
            let candidate = format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.distribution_tenants.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate an origin access identity id no stored identity holds.
    ///
    /// `CloudFrontOriginAccessIdentity$Id` is "The ID for the origin access identity, for
    /// example, `E74FTE3AJFJ256A`": the same shape a distribution id and a version token
    /// take (`E` + [`ID_BODY_LEN`] uppercase alphanumerics), drawn from the same mint, so
    /// no two identities are minted the same id.
    pub fn allocate_origin_access_identity_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.origin_access_identities.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a response headers policy id no stored policy holds.
    ///
    /// `ResponseHeadersPolicy$Id` is the identifier every response headers policy
    /// operation addresses one by, and CloudFront mints a UUID for one (the same shape
    /// its cache and origin request policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws
    /// render distinctly, so the mint never repeats an id.
    pub fn allocate_response_headers_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.response_headers_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate an origin request policy id no stored policy holds.
    ///
    /// `OriginRequestPolicy$Id` is the identifier every origin request policy operation
    /// addresses one by, and CloudFront mints a UUID for one (the same shape its cache and
    /// response headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_origin_request_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.origin_request_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a cache policy id no stored policy holds.
    ///
    /// `CachePolicy$Id` is the identifier every cache policy operation addresses one by,
    /// and CloudFront mints a UUID for one (the same shape its origin request and response
    /// headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_cache_policy_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.cache_policies.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// The id of the account's default connection group, minting it on the first call that
    /// needs one — which is what "CloudFront will automatically create a default connection
    /// group for you" means. Every later call reads that one id back.
    pub fn default_connection_group(&mut self) -> String {
        if let Some(id) = &self.default_connection_group_id {
            return id.clone();
        }
        let id = format!("{CONNECTION_GROUP_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
        self.default_connection_group_id = Some(id.clone());
        id
    }

    /// Allocate the Amazon S3 canonical user id of an origin access identity, in the
    /// [`S3_CANONICAL_USER_ID_LEN`] lowercase hex characters S3 writes one in.
    ///
    /// Drawn from the same mint as every other generated value here (four draws, 256
    /// bits, rendered as hex), and checked against the stored identities so no two
    /// identities share the canonical user id a bucket policy names them by.
    pub fn allocate_s3_canonical_user_id(&mut self) -> String {
        loop {
            let mut candidate = String::with_capacity(S3_CANONICAL_USER_ID_LEN);
            while candidate.len() < S3_CANONICAL_USER_ID_LEN {
                candidate.push_str(&format!("{:016x}", self.mint_u64()));
            }
            if !self
                .origin_access_identities
                .values()
                .any(|identity| identity.s3_canonical_user_id == candidate)
            {
                break candidate;
            }
        }
    }

    /// One fresh UUID from the mint: two draws (128 bits) rendered as the 8-4-4-4-12
    /// lowercase hex a UUID takes. ONE rendering for every resource CloudFront identifies
    /// by UUID, so no two of them mint ids in different shapes.
    fn mint_uuid(&mut self) -> String {
        let high = self.mint_u64();
        let low = self.mint_u64();
        let hex = format!("{high:016x}{low:016x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Allocate the domain name a distribution is served on: a generated label under
    /// `cloudfront.net`, in the lowercase DNS labels take.
    ///
    /// What a distribution REPORTS is [`StoredDistribution::reported_domain_name`]: a
    /// multi-tenant distribution has no domain of its own, so it reports the placeholder
    /// instead of the label allocated here.
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

    /// Replace the comment of the key value store `name` names, and report the record as
    /// it now stands.
    ///
    /// `KeyValueStore$Comment` is the only member `UpdateKeyValueStore` submits, so the
    /// stored comment becomes the one submitted, `LastModifiedTime` becomes `now` — "when
    /// the resource last changed" — and the version token is freshly minted, so the token
    /// the caller sent no longer satisfies an `If-Match` against this store.
    ///
    /// The store's identity (name, id, ARN) and its PROVISIONING progress are untouched:
    /// changing a comment neither re-provisions the store's data plane nor renames it, so
    /// [`StoredKeyValueStore::status`] and [`StoredKeyValueStore::ready_due`] stay as the
    /// create left them and the clock still owes the same transition at the same instant.
    ///
    /// `None` when the store holds no such key value store, checked BEFORE the mint so a
    /// version token is never spent on a store that is not there.
    pub fn apply_key_value_store_comment(
        &mut self,
        name: &str,
        comment: String,
        now: SystemTime,
    ) -> Option<&StoredKeyValueStore> {
        if !self.key_value_stores.contains_key(name) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.key_value_stores.get_mut(name)?;
        stored.comment = comment;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }
}

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
fn in_emitted_order(methods: &[String]) -> Vec<String> {
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

/// The path of a continuous deployment policy resource under this service's API version.
/// The resource is addressed by id, as `GetContinuousDeploymentPolicy`'s `{Id}` label is.
pub fn continuous_deployment_policy_path(id: &str) -> String {
    format!("/2020-05-31/continuous-deployment-policy/{id}")
}

/// The ARN of a distribution tenant. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:distribution-tenant/<Id>`, the
/// resource type this service's own `/distribution-tenant` routes name.
pub fn distribution_tenant_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("distribution-tenant/{id}"))
}

/// Whether two domain names are the same name. DNS names are case-insensitive, so the
/// comparison is too — a domain claimed in one case is claimed in every case.
pub fn same_domain(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Why `domain` is no domain name a CloudFront resource can be reached at, or `None` when
/// it is one.
///
/// `DomainItem$Domain` is modelled as a plain `string`, so the shape of a DNS name is
/// this check rather than a modelled constraint: a name is a dot-separated sequence of at
/// least two non-empty labels, each at most [`MAX_DOMAIN_LABEL_LEN`] characters and
/// carrying no whitespace, and the whole name is at most [`MAX_DOMAIN_NAME_LEN`]
/// characters (RFC 1035). The character set of a label is deliberately NOT narrowed to
/// ASCII: an internationalised domain name is a domain name.
///
/// ONE rule for every operation that associates a domain with a resource, so the create
/// and a later association cannot drift apart. The reason carries no error wording of its
/// own: the caller prefixes it with the wording of whichever refusal its own model
/// declares.
pub fn domain_name_refusal(domain: &str) -> Option<String> {
    if domain.is_empty() {
        return Some("a domain name cannot be empty".to_string());
    }
    if domain.chars().count() > MAX_DOMAIN_NAME_LEN {
        return Some(format!(
            "the domain {domain} is longer than the {MAX_DOMAIN_NAME_LEN} characters a domain \
             name may carry"
        ));
    }
    // The root label a fully qualified name ends in carries nothing, so a trailing dot is
    // not a second, empty label.
    let labels: Vec<&str> = domain.trim_end_matches('.').split('.').collect();
    if labels.len() < 2 {
        return Some(format!(
            "the domain {domain} is a single label, not a domain name"
        ));
    }
    for label in labels {
        if label.is_empty() {
            return Some(format!("the domain {domain} carries an empty label"));
        }
        if label.chars().count() > MAX_DOMAIN_LABEL_LEN {
            return Some(format!(
                "the domain {domain} carries a label longer than the {MAX_DOMAIN_LABEL_LEN} \
                 characters a label may carry"
            ));
        }
        if label.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Some(format!(
                "the domain {domain} carries whitespace, which no domain name does"
            ));
        }
    }
    None
}

/// Why `customizations` is no set of customizations a distribution tenant can carry, or
/// `None` when it is one.
///
/// A tenant's customizations are "specific values that you can override or disable from
/// the multi-tenant distribution", and the only list among them is the geographic
/// restriction's locations, bounded by [`MAX_GEO_RESTRICTION_LOCATIONS`] — so no caller
/// chooses how much work one customization costs, nor how large the record that retains it
/// is. Every other member is a single value the model already constrains.
///
/// ONE rule for every operation that accepts a tenant's customizations. The reason carries
/// no error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn customizations_refusal(customizations: &Customizations) -> Option<String> {
    let locations = customizations
        .geo_restrictions
        .as_ref()
        .and_then(|geo| geo.locations.as_deref())
        .unwrap_or_default();
    if locations.len() > MAX_GEO_RESTRICTION_LOCATIONS {
        return Some(format!(
            "a geographic restriction names at most {MAX_GEO_RESTRICTION_LOCATIONS} locations \
             but the request names {}",
            locations.len()
        ));
    }
    None
}

/// Why `parameters` does not satisfy the `TenantConfig$ParameterDefinitions` of the
/// multi-tenant distribution a tenant is created or updated under, or `None` when it
/// does.
///
/// "A parameter is specified as a key-value pair. A valid parameter value must exist for
/// any parameter that is marked as required in the multi-tenant distribution" — so three
/// things make a parameter list invalid: a value for a parameter the distribution defines
/// nothing for, one parameter named twice (which describes no single value), and a
/// definition marked `Required` for which neither the request nor the definition's own
/// `DefaultValue` supplies a VALID value — a value being valid means a non-empty one, so
/// an empty string satisfies a required parameter no more than an absent one does.
///
/// A definition carrying no `StringSchema` states no `Required` flag at all
/// (`ParameterDefinitionSchema`'s only member is optional), so there is nothing to require
/// of the request for it.
///
/// ONE rule for every operation that supplies a tenant's parameter values. The reason
/// carries no error wording of its own, for the same reason [`domain_name_refusal`]'s
/// does not.
pub fn parameter_refusal(
    definitions: &[ParameterDefinition],
    parameters: &[Parameter],
) -> Option<String> {
    // The definitions are INDEXED once, so the check costs one logarithmic lookup per
    // submitted value rather than a scan of the definitions per value: its cost is the
    // SUM of the two lengths, never their product.
    let defined: BTreeMap<&str, Option<&StringSchemaConfig>> = definitions
        .iter()
        .map(|definition| {
            (
                definition.name.as_str(),
                definition.definition.string_schema.as_ref(),
            )
        })
        .collect();
    let mut submitted: BTreeMap<&str, &str> = BTreeMap::new();
    for parameter in parameters {
        if submitted
            .insert(parameter.name.as_str(), parameter.value.as_str())
            .is_some()
        {
            return Some(format!(
                "the parameter {} is named more than once",
                parameter.name
            ));
        }
        if !defined.contains_key(parameter.name.as_str()) {
            return Some(format!(
                "the distribution defines no parameter named {}",
                parameter.name
            ));
        }
    }
    for (name, schema) in &defined {
        let Some(schema) = schema else {
            continue;
        };
        let supplied = submitted.get(name).is_some_and(|value| !value.is_empty())
            || schema
                .default_value
                .as_deref()
                .is_some_and(|value| !value.is_empty());
        if schema.required && !supplied {
            return Some(format!(
                "the parameter {name} is required by the distribution and the request supplies \
                 no value for it"
            ));
        }
    }
    None
}

/// The path of one origin access identity under this service's API version — the route
/// `GetCloudFrontOriginAccessIdentity` serves (`/origin-access-identity/cloudfront/{Id}`),
/// which is therefore the resource the create's `Location` names.
pub fn origin_access_identity_path(id: &str) -> String {
    format!("/2020-05-31/origin-access-identity/cloudfront/{id}")
}

/// The path of a response headers policy resource under this service's API version. The
/// resource is addressed by ID, as `GetResponseHeadersPolicy`'s `{Id}` label is.
pub fn response_headers_policy_path(id: &str) -> String {
    format!("/2020-05-31/response-headers-policy/{id}")
}

/// The path of an origin request policy resource under this service's API version. The
/// resource is addressed by ID, as `GetOriginRequestPolicy`'s `{Id}` label is.
pub fn origin_request_policy_path(id: &str) -> String {
    format!("/2020-05-31/origin-request-policy/{id}")
}

/// The path of a cache policy resource under this service's API version. The resource is
/// addressed by ID, as `GetCachePolicy`'s `{Id}` label is.
pub fn cache_policy_path(id: &str) -> String {
    format!("/2020-05-31/cache-policy/{id}")
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

/// Whether `value` has the shape every id this service MINTS AS A UUID takes
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`]): the 8-4-4-4-12
/// lowercase hex of a UUID.
///
/// Nothing here is specific to one resource kind — a key value store, a response headers
/// policy and a continuous deployment policy all carry an id of this shape — so one
/// predicate serves every listing whose `Marker` is such an id.
/// `ListContinuousDeploymentPolicies`'s `Marker` is a policy id and the model constrains
/// it no further than `string`, so this is what tells a resume point the service issued
/// from a value a client made up.
pub fn is_minted_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
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

/// Whether two distribution configurations ask for the same thing, which is what decides
/// whether a repeated `CallerReference` is the same create again or a second
/// distribution under a reference that is already taken.
///
/// Compared over the WHOLE configuration rather than a chosen few members: the question is
/// about "the content of the config", so every member a caller can submit takes part, down
/// to the members a request left out. Comparing the DESERIALIZED configurations also makes
/// the comparison insensitive to the request document's whitespace and member order, which
/// is what "identical … (ignoring white space)" asks for.
///
/// The comparison goes through the shapes' derived `Debug` rendering, which spells out
/// every member in declaration order with its value, because the alternative — deriving
/// `PartialEq` on the ~45 shapes a `DistributionConfig` reaches — means editing
/// `src/ops/types/shared_types.rs`, and that file carries the model's
/// `ReferrerPolicyList` value `unsafe-url`, which the merge gate's deterministic scanners
/// read as `unsafe` Rust and refuse at HIGH. So the comparison lives here instead, and
/// what it rests on — that the rendering keeps every member, including the `@sensitive`
/// `Comment` — is pinned by `same_config_separates_configurations_that_differ` below: a
/// rendering that hid or truncated a member would make two configurations differing in it
/// compare equal, and that test fails the moment it does.
pub fn same_config(left: &DistributionConfig, right: &DistributionConfig) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// Whether two continuous deployment policy configurations ask for the same thing, which
/// is what decides whether a create repeats a policy the store already holds.
///
/// Compared MEMBER BY MEMBER over the whole configuration rather than through a rendering
/// of it: `ContinuousDeploymentPolicyConfig` reaches few enough shapes to spell the
/// comparison out, so a create costs no allocation per stored policy (unlike
/// [`same_config`], which renders a `DistributionConfig`'s ~45 shapes because deriving
/// `PartialEq` on them is not available here). Every member a caller can submit takes
/// part, down to the members a request left out.
pub fn same_policy_config(
    left: &ContinuousDeploymentPolicyConfig,
    right: &ContinuousDeploymentPolicyConfig,
) -> bool {
    left.enabled == right.enabled
        && left.staging_distribution_dns_names.quantity
            == right.staging_distribution_dns_names.quantity
        && staging_distribution_dns_names(left) == staging_distribution_dns_names(right)
        && same_traffic_config(left.traffic_config.as_ref(), right.traffic_config.as_ref())
}

/// Whether two optional traffic configurations ask for the same routing. An absent one
/// matches only an absent one: a configuration that names no `TrafficConfig` is not the
/// configuration that names one.
fn same_traffic_config(left: Option<&TrafficConfig>, right: Option<&TrafficConfig>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.r#type == right.r#type
                && same_single_weight_config(
                    left.single_weight_config.as_ref(),
                    right.single_weight_config.as_ref(),
                )
                && same_single_header_config(
                    left.single_header_config.as_ref(),
                    right.single_header_config.as_ref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional single-weight configurations name the same share of traffic.
///
/// The weights are compared by their BITS: two configurations are the same request again
/// only if the weight submitted is the same value, and the bit pattern is what says so
/// without an approximate comparison between floats.
fn same_single_weight_config(
    left: Option<&ContinuousDeploymentSingleWeightConfig>,
    right: Option<&ContinuousDeploymentSingleWeightConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.weight.to_bits() == right.weight.to_bits()
                && same_session_stickiness_config(
                    left.session_stickiness_config.as_ref(),
                    right.session_stickiness_config.as_ref(),
                )
        }
        _ => false,
    }
}

/// Whether two optional session stickiness configurations name the same session window.
fn same_session_stickiness_config(
    left: Option<&SessionStickinessConfig>,
    right: Option<&SessionStickinessConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.idle_ttl == right.idle_ttl && left.maximum_ttl == right.maximum_ttl
        }
        _ => false,
    }
}

/// Whether two optional single-header configurations select the same requests.
fn same_single_header_config(
    left: Option<&ContinuousDeploymentSingleHeaderConfig>,
    right: Option<&ContinuousDeploymentSingleHeaderConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.header == right.header && left.value == right.value,
        _ => false,
    }
}

/// Whether two origin access identity configurations ask for the same thing, which is
/// what decides whether a repeated `CallerReference` is the same create again or a second
/// identity under a reference that is already taken.
///
/// The same comparison [`same_config`] makes, for the same reasons: every member a caller
/// can submit takes part, and comparing the DESERIALIZED configurations is what makes it
/// insensitive to the request document's whitespace and member order — "the content of the
/// `CloudFrontOriginAccessIdentityConfig` is identical to the original request (ignoring
/// white space)".
pub fn same_origin_access_identity_config(
    left: &CloudFrontOriginAccessIdentityConfig,
    right: &CloudFrontOriginAccessIdentityConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
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
///
/// Either list is reported in [`EMITTED_METHOD_ORDER`] rather than in the order the
/// request listed it: `MethodsList` is an ordered XML list, and the service emits the
/// methods of the set selected in its own fixed order. The default set is written in that
/// order already, so the reordering is what a SUBMITTED list goes through.
fn resolve_allowed_methods(allowed: Option<AllowedMethods>) -> AllowedMethods {
    let mut allowed = allowed.unwrap_or_else(|| AllowedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
        cached_methods: None,
    });
    allowed.items = in_emitted_order(&allowed.items);
    let mut cached = allowed.cached_methods.unwrap_or_else(|| CachedMethods {
        quantity: i32::try_from(DEFAULT_ALLOWED_METHODS.len()).unwrap_or(i32::MAX),
        items: default_methods(),
    });
    cached.items = in_emitted_order(&cached.items);
    allowed.cached_methods = Some(cached);
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
///
/// The sentence is CloudFront's own for this refusal, built from the request: it names
/// the wrapper, the `Quantity` the request stated and the number of `Items` it actually
/// carried, in that order — "The specified quantity of <wrapper> (<stated>) does not
/// match the actual quantity supplied (<carried>)." The model's doc text for the shape
/// ("The value of Quantity and the size of Items don't match.") describes the condition
/// rather than the message the service sends.
fn quantity_disagreement(label: &str, quantity: i32, len: usize) -> Option<String> {
    if i64::from(quantity) == i64::try_from(len).unwrap_or(i64::MAX) {
        return None;
    }
    Some(format!(
        "The specified quantity of {label} ({quantity}) does not match the actual quantity \
         supplied ({len})."
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

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// response headers policy configuration whose `Quantity` disagrees with the length of
/// its `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`] applies to a distribution configuration:
/// every CloudFront list travels in a wrapper stating its own length beside the list
/// itself, and the service refuses a request where the two disagree. This walks all of
/// the wrappers a `ResponseHeadersPolicyConfig` reaches — the three CORS lists that are
/// required when `CorsConfig` is named, the optional exposed-headers list, and the
/// custom-header and remove-header lists.
pub fn response_headers_policy_quantity_mismatch(
    config: &ResponseHeadersPolicyConfig,
) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(cors) = &config.cors_config {
        let origins = &cors.access_control_allow_origins;
        counts.push((
            "CorsConfig.AccessControlAllowOrigins",
            origins.quantity,
            origins.items.len(),
        ));
        let headers = &cors.access_control_allow_headers;
        counts.push((
            "CorsConfig.AccessControlAllowHeaders",
            headers.quantity,
            headers.items.len(),
        ));
        let methods = &cors.access_control_allow_methods;
        counts.push((
            "CorsConfig.AccessControlAllowMethods",
            methods.quantity,
            methods.items.len(),
        ));
        if let Some(exposed) = &cors.access_control_expose_headers {
            counts.push((
                "CorsConfig.AccessControlExposeHeaders",
                exposed.quantity,
                items_len(&exposed.items),
            ));
        }
    }
    if let Some(custom) = &config.custom_headers_config {
        counts.push((
            "CustomHeadersConfig",
            custom.quantity,
            items_len(&custom.items),
        ));
    }
    if let Some(remove) = &config.remove_headers_config {
        counts.push((
            "RemoveHeadersConfig",
            remove.quantity,
            items_len(&remove.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in an origin
/// request policy configuration whose `Quantity` disagrees with the length of its `Items`,
/// or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`] and
/// [`response_headers_policy_quantity_mismatch`] apply: every CloudFront list travels in a
/// wrapper stating its own length beside the list itself, and the service refuses a
/// request where the two disagree. An `OriginRequestPolicyConfig` reaches exactly three of
/// them — the optional `Headers`, `Cookies` and `QueryStrings` lists its three behaviour
/// configurations carry.
pub fn origin_request_policy_quantity_mismatch(
    config: &OriginRequestPolicyConfig,
) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(headers) = &config.headers_config.headers {
        counts.push((
            "HeadersConfig.Headers",
            headers.quantity,
            items_len(&headers.items),
        ));
    }
    if let Some(cookies) = &config.cookies_config.cookies {
        counts.push((
            "CookiesConfig.Cookies",
            cookies.quantity,
            items_len(&cookies.items),
        ));
    }
    if let Some(query_strings) = &config.query_strings_config.query_strings {
        counts.push((
            "QueryStringsConfig.QueryStrings",
            query_strings.quantity,
            items_len(&query_strings.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in a cache
/// policy configuration whose `Quantity` disagrees with the length of its `Items`, or
/// `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`],
/// [`response_headers_policy_quantity_mismatch`] and
/// [`origin_request_policy_quantity_mismatch`] apply: every CloudFront list travels in a
/// wrapper stating its own length beside the list itself, and the service refuses a
/// request where the two disagree. A `CachePolicyConfig` reaches exactly three of them —
/// the optional `Headers`, `Cookies` and `QueryStrings` lists the three behaviour
/// configurations under `ParametersInCacheKeyAndForwardedToOrigin` carry — and reaches
/// none at all when the configuration names no cache key parameters.
pub fn cache_policy_quantity_mismatch(config: &CachePolicyConfig) -> Option<String> {
    let parameters = config
        .parameters_in_cache_key_and_forwarded_to_origin
        .as_ref()?;
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(headers) = &parameters.headers_config.headers {
        counts.push((
            "HeadersConfig.Headers",
            headers.quantity,
            items_len(&headers.items),
        ));
    }
    if let Some(cookies) = &parameters.cookies_config.cookies {
        counts.push((
            "CookiesConfig.Cookies",
            cookies.quantity,
            items_len(&cookies.items),
        ));
    }
    if let Some(query_strings) = &parameters.query_strings_config.query_strings {
        counts.push((
            "QueryStringsConfig.QueryStrings",
            query_strings.quantity,
            items_len(&query_strings.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// Why a cache policy configuration is unusable, as the sentence the modelled
/// `InvalidArgument` carries — or `None` when every member is within what the model and
/// the documentation allow.
///
/// Two rules, each one the documentation states and each sentence BUILT FROM THE REQUEST:
/// it names the member and the constraint it broke, which is the one thing the caller
/// cannot work out for itself.
///
/// * The three TTLs are ORDERED: each is "The {minimum,default,maximum} amount of time, in
///   seconds, that objects stay in the CloudFront cache", so the minimum cannot exceed the
///   default or the maximum and the default cannot exceed the maximum. Only the members
///   the request SUPPLIES are compared: a member the request leaves out carries
///   CloudFront's own documented default for it (86400 or `MinTTL`, whichever is larger,
///   for `DefaultTTL`; 31536000 or `DefaultTTL`, whichever is larger, for `MaxTTL`), and
///   refusing a configuration whose only disorder comes from that default would refuse
///   what the service accepts.
/// * A TTL is an amount of time in seconds, so a negative one names no duration. The model
///   puts no `@range` on `long`, so nothing refuses it before a handler runs.
///
/// `CachePolicyConfig$Comment`'s documented bound is checked beside these, through the one
/// [`comment_too_long`] sentence every comment bound in this service is refused with.
pub fn cache_policy_config_invalid(config: &CachePolicyConfig) -> Option<String> {
    if let Some(comment) = &config.comment {
        let length = comment.chars().count();
        if length > MAX_CACHE_POLICY_COMMENT {
            return comment_too_long(length).message;
        }
    }

    for (label, value) in [
        ("MinTTL", Some(config.min_ttl)),
        ("DefaultTTL", config.default_ttl),
        ("MaxTTL", config.max_ttl),
    ] {
        if let Some(value) = value {
            if value < 0 {
                return Some(format!(
                    "An argument is invalid: {label} is an amount of time in seconds and cannot \
                     be negative, but the request submits {value}"
                ));
            }
        }
    }

    // The pairs the ordering rule constrains, each compared only where the request
    // supplies both members of it.
    let pairs: [(&str, Option<i64>, &str, Option<i64>); 3] = [
        ("MinTTL", Some(config.min_ttl), "DefaultTTL", config.default_ttl),
        ("MinTTL", Some(config.min_ttl), "MaxTTL", config.max_ttl),
        ("DefaultTTL", config.default_ttl, "MaxTTL", config.max_ttl),
    ];
    for (lower_label, lower, upper_label, upper) in pairs {
        let (Some(lower), Some(upper)) = (lower, upper) else {
            continue;
        };
        if lower > upper {
            return Some(format!(
                "An argument is invalid: {upper_label} must be greater than or equal to \
                 {lower_label}, but the request submits {lower_label} {lower} and {upper_label} \
                 {upper}"
            ));
        }
    }

    None
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

/// `TrafficConfig$Type` selecting a share of traffic, from
/// [`crate::ops::types::CONTINUOUS_DEPLOYMENT_POLICY_TYPE_VALUES`] — the values the model
/// says the member accepts.
pub const TRAFFIC_TYPE_SINGLE_WEIGHT: &str = "SingleWeight";

/// `TrafficConfig$Type` selecting requests by header, from the same enum.
pub const TRAFFIC_TYPE_SINGLE_HEADER: &str = "SingleHeader";

/// The prefix `ContinuousDeploymentSingleHeaderConfig$Header` must carry: "The request
/// header name that you want CloudFront to send to your staging distribution. The header
/// must contain the prefix `aws-cf-cd-`."
pub const SINGLE_HEADER_PREFIX: &str = "aws-cf-cd-";

/// The modelled `InconsistentQuantities` message for a continuous deployment policy
/// configuration whose `StagingDistributionDnsNames.Quantity` disagrees with the number of
/// domain names it carries, or `None`.
///
/// The one wrapper a `ContinuousDeploymentPolicyConfig` reaches, checked under the same
/// rule every other CloudFront list wrapper is ([`quantity_disagreement`]).
pub fn policy_config_quantity_mismatch(
    config: &ContinuousDeploymentPolicyConfig,
) -> Option<String> {
    let names = &config.staging_distribution_dns_names;
    quantity_disagreement(
        "StagingDistributionDnsNames",
        names.quantity,
        items_len(&names.items),
    )
}

/// Why a continuous deployment policy configuration is unusable, as the sentence the
/// modelled `InvalidArgument` carries — or `None` when every member is within what the
/// model and the documentation allow.
///
/// Each sentence is BUILT FROM THE REQUEST: it names the member and the constraint it
/// broke, which is the one thing the caller cannot work out for itself. The constraints
/// are the documented ones:
///
/// * `TrafficConfig$Type` "The type of traffic configuration" selects WHICH of the two
///   traffic configurations applies, so the one it names has to be there — and the value
///   has to be one the enum admits;
/// * `ContinuousDeploymentSingleHeaderConfig$Header` "must contain the prefix
///   `aws-cf-cd-`";
/// * `ContinuousDeploymentSingleWeightConfig$Weight` is "expressed as a decimal number
///   between 0 and 0.15";
/// * `SessionStickinessConfig$IdleTTL` and `$MaximumTTL` each have "Allowed values …
///   300–3600 seconds", and `IdleTTL` "must be less than or equal to `MaximumTTL`";
/// * `StagingDistributionDnsNames` carries at most
///   [`MAX_STAGING_DISTRIBUTION_DNS_NAMES`] domain names — the one caller-chosen LENGTH
///   in this configuration, bounded on the way IN like every other count this service
///   stores ([`MAX_CACHE_BEHAVIORS`], [`MAX_INVALIDATION_PATHS`]).
pub fn policy_config_invalid_argument(
    config: &ContinuousDeploymentPolicyConfig,
) -> Option<String> {
    // The LENGTH of the submitted domain name list is caller-chosen, and the record is
    // stored and later scanned name by name, so the count is refused FIRST — before any
    // other member is walked, before the store is locked and before anything is minted.
    // `Quantity` is taken into the count as well, defensively: it is the SAME bound, not a
    // second rule, and `policy_config_quantity_mismatch` has already refused a `Quantity`
    // that disagrees with `Items` by the time a handler reaches here, so the two spellings
    // cannot disagree — this one simply does not depend on that order.
    let names = &config.staging_distribution_dns_names;
    let stated = i64::from(names.quantity).max(i64::try_from(items_len(&names.items)).unwrap_or(0));
    if stated > i64::try_from(MAX_STAGING_DISTRIBUTION_DNS_NAMES).unwrap_or(i64::MAX) {
        return Some(format!(
            "StagingDistributionDnsNames names {stated} staging distribution domain names; at \
             most {MAX_STAGING_DISTRIBUTION_DNS_NAMES} are allowed."
        ));
    }
    // `None` from this function means "nothing to refuse", so each absent OPTIONAL member
    // below ends the walk rather than failing it: a configuration that names no traffic
    // configuration, or no session stickiness, has no constraint left to break. The `?` is
    // what clippy's `question_mark` lint requires here, so it reads as that early end.
    let traffic = config.traffic_config.as_ref()?;
    let kind = traffic.r#type.as_str();
    if kind == TRAFFIC_TYPE_SINGLE_WEIGHT {
        let Some(weight_config) = traffic.single_weight_config.as_ref() else {
            return Some(format!(
                "TrafficConfig.Type {TRAFFIC_TYPE_SINGLE_WEIGHT} requires \
                 TrafficConfig.SingleWeightConfig."
            ));
        };
        let weight = weight_config.weight;
        if !(MIN_TRAFFIC_WEIGHT..=MAX_TRAFFIC_WEIGHT).contains(&weight) {
            return Some(format!(
                "TrafficConfig.SingleWeightConfig.Weight ({weight}) must be a decimal number \
                 between {MIN_TRAFFIC_WEIGHT} and {MAX_TRAFFIC_WEIGHT}."
            ));
        }
        // No session stickiness named: nothing left to refuse (see above).
        let stickiness = weight_config.session_stickiness_config.as_ref()?;
        for (member, ttl) in [
            ("IdleTTL", stickiness.idle_ttl),
            ("MaximumTTL", stickiness.maximum_ttl),
        ] {
            if !(MIN_SESSION_STICKINESS_TTL..=MAX_SESSION_STICKINESS_TTL).contains(&ttl) {
                return Some(format!(
                    "TrafficConfig.SingleWeightConfig.SessionStickinessConfig.{member} ({ttl}) \
                     must be between {MIN_SESSION_STICKINESS_TTL} and \
                     {MAX_SESSION_STICKINESS_TTL} seconds."
                ));
            }
        }
        if stickiness.idle_ttl > stickiness.maximum_ttl {
            return Some(format!(
                "TrafficConfig.SingleWeightConfig.SessionStickinessConfig.IdleTTL ({}) must be \
                 less than or equal to MaximumTTL ({}).",
                stickiness.idle_ttl, stickiness.maximum_ttl
            ));
        }
        return None;
    }
    if kind == TRAFFIC_TYPE_SINGLE_HEADER {
        let Some(header_config) = traffic.single_header_config.as_ref() else {
            return Some(format!(
                "TrafficConfig.Type {TRAFFIC_TYPE_SINGLE_HEADER} requires \
                 TrafficConfig.SingleHeaderConfig."
            ));
        };
        if !header_config.header.starts_with(SINGLE_HEADER_PREFIX) {
            return Some(format!(
                "TrafficConfig.SingleHeaderConfig.Header ({}) must contain the prefix \
                 {SINGLE_HEADER_PREFIX}.",
                header_config.header
            ));
        }
        return None;
    }
    // A value outside `ContinuousDeploymentPolicyType` is already refused at the request
    // boundary by the model's own enum constraint, so this is the fallback for a `Type`
    // that reached here some other way — never a second spelling of that refusal.
    Some(format!(
        "TrafficConfig.Type ({kind}) must be one of {}.",
        crate::ops::types::CONTINUOUS_DEPLOYMENT_POLICY_TYPE_VALUES.join(", ")
    ))
}

/// `NoSuchContinuousDeploymentPolicy`'s message: the sentence the model states on the
/// shape itself, "The continuous deployment policy doesn't exist."
pub const NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE: &str =
    "The continuous deployment policy doesn't exist.";

/// The modelled `NoSuchContinuousDeploymentPolicy` for a policy id the store does not
/// hold. Fixed service text, for the same reason [`no_such_distribution`] is.
pub fn no_such_continuous_deployment_policy() -> crate::ops::types::NoSuchContinuousDeploymentPolicy
{
    crate::ops::types::NoSuchContinuousDeploymentPolicy {
        message: Some(NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE.to_string()),
    }
}

/// The modelled `StagingDistributionInUse` (HTTP 409) for a create whose staging
/// distribution another policy already routes traffic to — "A continuous deployment
/// policy for this staging distribution already exists" — naming that policy, the one
/// piece of detail the caller cannot work out for itself.
pub fn staging_distribution_in_use(policy_id: &str) -> crate::ops::types::StagingDistributionInUse {
    crate::ops::types::StagingDistributionInUse {
        message: Some(format!(
            "A continuous deployment policy for this staging distribution already exists: \
             {policy_id}"
        )),
    }
}

/// The modelled `ContinuousDeploymentPolicyAlreadyExists` (HTTP 409) for a create
/// repeating a configuration the store already holds — "A continuous deployment policy
/// with this configuration already exists" — naming the policy that holds it.
pub fn continuous_deployment_policy_already_exists(
    policy_id: &str,
) -> crate::ops::types::ContinuousDeploymentPolicyAlreadyExists {
    crate::ops::types::ContinuousDeploymentPolicyAlreadyExists {
        message: Some(format!(
            "A continuous deployment policy with this configuration already exists: {policy_id}"
        )),
    }
}

/// The modelled `TooManyContinuousDeploymentPolicies` (HTTP 400) for a create at this
/// emulator's bound ([`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`]).
///
/// The sentence states the BOUND and nothing else: the status and the code are the
/// contract, the text is free, and it advises no remedy — deleting a policy is
/// `DeleteContinuousDeploymentPolicy`, still a scaffold in this emulator.
pub fn too_many_continuous_deployment_policies()
-> crate::ops::types::TooManyContinuousDeploymentPolicies {
    crate::ops::types::TooManyContinuousDeploymentPolicies {
        message: Some(format!(
            "This emulator holds at most {MAX_CONTINUOUS_DEPLOYMENT_POLICIES} continuous \
             deployment policies for one account"
        )),
    }
}

/// `NoSuchDistribution`'s message: the sentence the model states on the shape itself,
/// "The specified distribution does not exist."
pub const NO_SUCH_DISTRIBUTION_MESSAGE: &str = "The specified distribution does not exist.";

/// `NoSuchInvalidation`'s message: the sentence the model states on the shape itself,
/// "The specified invalidation does not exist."
pub const NO_SUCH_INVALIDATION_MESSAGE: &str = "The specified invalidation does not exist.";

/// `PreconditionFailed`'s message, as CloudFront sends it: "The request failed because it
/// didn't meet the preconditions in one or more request-header fields."
///
/// The model's doc text for the shape states the condition ("The precondition in one or
/// more of the request fields evaluated to `false`."); this is the sentence the service
/// puts on the wire. Either way it is fixed service text that names no request value: a
/// caller learns WHICH precondition failed from the `If-Match` it sent, not from a
/// version token echoed back to it.
pub const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

/// The sentence `DistributionAlreadyExists` carries, up to the distribution the caller
/// reference is already associated with, which CloudFront names at the end of it.
const DISTRIBUTION_ALREADY_EXISTS_PREFIX: &str =
    "The caller reference that you are using to create a distribution is associated with \
     another distribution. Already exists: ";

/// `InvalidArgument`'s message for a `MaxItems` a page size cannot be taken from.
///
/// One sentence for every unusable value — a value that is no integer at all, zero, or a
/// negative one — because it states the CONSTRAINT on the parameter rather than the
/// value that broke it: "MaxItems must be a non-zero positive integer."
pub const MAX_ITEMS_INVALID_MESSAGE: &str = "MaxItems must be a non-zero positive integer.";

/// The modelled `NoSuchDistribution` for a distribution id the store does not hold.
///
/// The id is NOT in the message: CloudFront's sentence for this refusal is fixed service
/// text, and the caller already knows which distribution it asked for.
pub fn no_such_distribution() -> NoSuchDistribution {
    NoSuchDistribution {
        message: Some(NO_SUCH_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchInvalidation` for an invalidation id a distribution does not
/// hold. Fixed service text, for the same reason [`no_such_distribution`] is.
pub fn no_such_invalidation() -> NoSuchInvalidation {
    NoSuchInvalidation {
        message: Some(NO_SUCH_INVALIDATION_MESSAGE.to_string()),
    }
}

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version
/// token but not the current one.
pub fn precondition_failed() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_MESSAGE.to_string()),
    }
}

/// The sentence `DistributionAlreadyExists` carries: CloudFront's own wording, up to the
/// distribution the caller reference is already associated with, which it names at the
/// end — the one piece of request-derived detail in it, and the one a caller cannot work
/// out for itself.
///
/// A message rather than the shape, because the create path this service shares between
/// `CreateDistribution` and `CreateDistributionWithTags` reports its refusals as
/// [`CreateRefusal`], which each operation then lifts into its own modelled error.
pub fn distribution_already_exists_message(existing_id: &str) -> String {
    format!("{DISTRIBUTION_ALREADY_EXISTS_PREFIX}{existing_id}")
}

/// The modelled `InvalidArgument` for a `MaxItems` value no page size can be taken from.
pub fn invalid_max_items() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MAX_ITEMS_INVALID_MESSAGE.to_string()),
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

/// The modelled `EntityNotFound` (HTTP 404) for a distribution id no stored distribution
/// carries, for the operations whose model names `EntityNotFound` rather than
/// `NoSuchDistribution` for that condition.
pub fn distribution_not_found(id: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(format!("The specified distribution does not exist: {id}")),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an ARN, an id or a name —
/// no stored distribution tenant answers to.
pub fn distribution_tenant_not_found(identifier: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(format!(
            "The specified distribution tenant does not exist: {identifier}"
        )),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for a connection group id no connection group
/// this store holds carries.
pub fn connection_group_not_found(id: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(format!("The specified connection group does not exist: {id}")),
    }
}

/// The modelled `EntityAlreadyExists` (HTTP 409) for a distribution tenant name that is
/// already taken: the name is "unique within your Amazon Web Services account".
pub fn distribution_tenant_already_exists(name: &str) -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(format!(
            "A distribution tenant with the name {name} already exists in this account"
        )),
    }
}

/// The modelled `CNAMEAlreadyExists` (HTTP 409) for a domain another CloudFront resource
/// already claims.
///
/// The sentence is CloudFront's own for this code, with the domain that conflicts named at
/// the end of it so a caller that submitted several learns which one.
pub fn cname_already_exists(domain: &str) -> CNAMEAlreadyExists {
    CNAMEAlreadyExists {
        message: Some(format!(
            "One or more of the CNAMEs you provided are already associated with a different \
             resource: {domain}"
        )),
    }
}

/// `NoSuchCloudFrontOriginAccessIdentity`'s message: the sentence the model states on the
/// shape itself, "The specified origin access identity does not exist."
pub const NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE: &str =
    "The specified origin access identity does not exist.";

/// The modelled `NoSuchCloudFrontOriginAccessIdentity` (HTTP 404) for an identity id the
/// store does not hold.
///
/// The id is NOT in the message, for the same reason it is not in
/// [`no_such_distribution`]'s: the sentence is fixed service text, and the caller already
/// knows which identity it asked for.
pub fn no_such_origin_access_identity() -> NoSuchCloudFrontOriginAccessIdentity {
    NoSuchCloudFrontOriginAccessIdentity {
        message: Some(NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE.to_string()),
    }
}

/// The sentence `CloudFrontOriginAccessIdentityAlreadyExists` carries, up to the identity
/// the caller reference is already associated with, which is named at the end — the one
/// piece of request-derived detail in it, and the one a caller cannot work out for
/// itself. The same shape [`distribution_already_exists_message`] takes, for the same
/// rule applied to the other resource.
pub fn origin_access_identity_already_exists_message(existing_id: &str) -> String {
    format!(
        "The caller reference that you are using to create an origin access identity is \
         associated with another identity. Already exists: {existing_id}"
    )
}

/// The longest comment an origin access identity carries.
///
/// `CloudFrontOriginAccessIdentityConfig$Comment`: "A comment to describe the origin
/// access identity. The comment cannot be longer than 128 characters." The member carries
/// no `@length` in the model, so the bound is not refused before a handler runs and the
/// create path checks it itself.
pub const MAX_ORIGIN_ACCESS_IDENTITY_COMMENT: usize = 128;

/// The modelled `InvalidArgument` for an origin access identity comment past
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`].
///
/// The message states the CONSTRAINT the request broke and the length it submitted, which
/// is the rule the refusal is built from rather than text echoed out of the request.
pub fn comment_too_long(length: usize) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: the comment cannot be longer than \
             {MAX_ORIGIN_ACCESS_IDENTITY_COMMENT} characters but the request submits {length}"
        )),
    }
}

/// The modelled `NoSuchResponseHeadersPolicy` (HTTP 404) for a response headers policy
/// id the store does not hold.
///
/// The sentence is the one the model states on the shape itself — "The response headers
/// policy does not exist." — a property of the store rather than of the request, so
/// nothing from the request belongs in the text.
pub fn no_such_response_headers_policy() -> NoSuchResponseHeadersPolicy {
    NoSuchResponseHeadersPolicy {
        message: Some(NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchResponseHeadersPolicy`'s message, as the model documents the shape.
pub const NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE: &str =
    "The response headers policy does not exist.";

/// The modelled `ResponseHeadersPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken — "A response headers policy with this name already exists. You must
/// provide a unique name. To modify an existing response headers policy, use
/// UpdateResponseHeadersPolicy."
pub fn response_headers_policy_already_exists(name: &str) -> ResponseHeadersPolicyAlreadyExists {
    ResponseHeadersPolicyAlreadyExists {
        message: Some(format!(
            "A response headers policy with the name {name} already exists in this account. You \
             must provide a unique name. To modify an existing response headers policy, use \
             UpdateResponseHeadersPolicy."
        )),
    }
}

/// The modelled `NoSuchOriginRequestPolicy` (HTTP 404) for an origin request policy id the
/// store does not hold.
///
/// The sentence is the one the model states on the shape itself — "The origin request
/// policy does not exist." — a property of the store rather than of the request, so
/// nothing from the request belongs in the text.
pub fn no_such_origin_request_policy() -> NoSuchOriginRequestPolicy {
    NoSuchOriginRequestPolicy {
        message: Some(NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchOriginRequestPolicy`'s message, as the model documents the shape.
pub const NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE: &str = "The origin request policy does not exist.";

/// The modelled `OriginRequestPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken — "An origin request policy with this name already exists. You must
/// provide a unique name. To modify an existing origin request policy, use
/// UpdateOriginRequestPolicy."
pub fn origin_request_policy_already_exists(name: &str) -> OriginRequestPolicyAlreadyExists {
    OriginRequestPolicyAlreadyExists {
        message: Some(format!(
            "An origin request policy with the name {name} already exists in this account. You \
             must provide a unique name. To modify an existing origin request policy, use \
             UpdateOriginRequestPolicy."
        )),
    }
}

/// The longest comment a cache policy carries.
///
/// `CachePolicyConfig$Comment`: "A comment to describe the cache policy. The comment
/// cannot be longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_CACHE_POLICY_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchCachePolicy` (HTTP 404) for a cache policy id the store does not
/// hold.
///
/// The sentence is the one the model states on the shape itself — "The cache policy does
/// not exist." — a property of the store rather than of the request, so nothing from the
/// request belongs in the text.
pub fn no_such_cache_policy() -> NoSuchCachePolicy {
    NoSuchCachePolicy {
        message: Some(NO_SUCH_CACHE_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchCachePolicy`'s message, as the model documents the shape.
pub const NO_SUCH_CACHE_POLICY_MESSAGE: &str = "The cache policy does not exist.";

/// The modelled `CachePolicyAlreadyExists` (HTTP 409) for a policy name that is already
/// taken — "A cache policy with this name already exists. You must provide a unique name.
/// To modify an existing cache policy, use UpdateCachePolicy."
pub fn cache_policy_already_exists(name: &str) -> CachePolicyAlreadyExists {
    CachePolicyAlreadyExists {
        message: Some(format!(
            "A cache policy with the name {name} already exists in this account. You must \
             provide a unique name. To modify an existing cache policy, use UpdateCachePolicy."
        )),
    }
}

/// Mint the request id ONE response names the request it answers with — the `RequestId`
/// element of this service's `<ErrorResponse>` envelope (`crate::service::envelope`).
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes and the same form
/// [`ComAmazonawsCloudfrontCloudfront20200531State::allocate_key_value_store_id`] renders
/// an id in. The VALUE is per-response and is not a requirement: nothing can predict it
/// and no caller may depend on a particular one. What it must do is name ONE response, so
/// it is drawn from a per-process counter run through [`scramble`] — the bijection every
/// token this service mints is drawn through, so two responses never carry the same id —
/// mixed with the instant the response was answered, so two runs of this emulator do not
/// mint the same sequence. It is an identifier for correlation, never a secret: nothing
/// authenticates with it.
///
/// A free function rather than a mint on the store: a failure the request boundary
/// refused never reaches a handler, so the envelope that answers it has no state to reach.
pub fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
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
    use crate::ops::types::{Aliases, Origin, Origins};

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

    /// A minted request id has the shape an AWS request id takes and names ONE response:
    /// the envelope's whole requirement is that a failure can be correlated, so two
    /// responses must never carry the same id.
    #[test]
    fn minted_request_ids_are_distinct_and_shaped() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..10_000 {
            let id = new_request_id();
            let widths: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                widths,
                vec![8, 4, 4, 4, 12],
                "case new_request_id: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                id.bytes()
                    .all(|b| b == b'-' || (b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
                "case new_request_id: expected lowercase hex, got {id:?}"
            );
            assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated a request id");
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

    /// A configuration a client could submit, as the baseline the cases below differ
    /// from one member at a time.
    fn a_config() -> DistributionConfig {
        DistributionConfig {
            caller_reference: "ref-baseline".to_string(),
            comment: "the baseline".to_string(),
            enabled: true,
            origins: Origins {
                quantity: 1,
                items: vec![Origin {
                    id: "primary".to_string(),
                    domain_name: "origin.example.com".to_string(),
                    ..Origin::default()
                }],
            },
            default_cache_behavior: DefaultCacheBehavior {
                target_origin_id: "primary".to_string(),
                viewer_protocol_policy: "allow-all".to_string(),
                ..DefaultCacheBehavior::default()
            },
            ..DistributionConfig::default()
        }
    }

    /// [`same_config`] separates exactly the configurations that differ, over every kind
    /// of member a `DistributionConfig` carries: a required scalar, the `@sensitive`
    /// `Comment`, a boolean, an optional scalar the baseline leaves out, a nested
    /// structure, and a list wrapper's `Quantity` and `Items`.
    ///
    /// This is what the create path's idempotency rests on: a comparison that could not
    /// see one of these members would answer a second, DIFFERENT configuration with the
    /// first distribution instead of the modelled conflict.
    #[test]
    fn same_config_separates_configurations_that_differ() {
        let baseline = a_config();
        assert!(
            same_config(&baseline, &a_config()),
            "case identical: expected two equal configurations to compare the same"
        );

        let mut differing: Vec<(&str, DistributionConfig)> = Vec::new();
        differing.push((
            "caller-reference",
            DistributionConfig {
                caller_reference: "ref-other".to_string(),
                ..a_config()
            },
        ));
        differing.push((
            "sensitive-comment",
            DistributionConfig {
                comment: "a different comment".to_string(),
                ..a_config()
            },
        ));
        differing.push((
            "empty-comment",
            DistributionConfig {
                comment: String::new(),
                ..a_config()
            },
        ));
        differing.push((
            "enabled",
            DistributionConfig {
                enabled: false,
                ..a_config()
            },
        ));
        differing.push((
            "optional-scalar-named",
            DistributionConfig {
                price_class: Some(DEFAULT_PRICE_CLASS.to_string()),
                ..a_config()
            },
        ));
        differing.push((
            "nested-structure",
            DistributionConfig {
                default_cache_behavior: DefaultCacheBehavior {
                    viewer_protocol_policy: "https-only".to_string(),
                    ..a_config().default_cache_behavior
                },
                ..a_config()
            },
        ));
        differing.push((
            "list-wrapper-items",
            DistributionConfig {
                origins: Origins {
                    quantity: 1,
                    items: vec![Origin {
                        id: "primary".to_string(),
                        domain_name: "other.example.com".to_string(),
                        ..Origin::default()
                    }],
                },
                ..a_config()
            },
        ));
        differing.push((
            "list-wrapper-quantity",
            DistributionConfig {
                origins: Origins {
                    quantity: 2,
                    ..a_config().origins
                },
                ..a_config()
            },
        ));
        differing.push((
            "absent-wrapper-becomes-empty",
            DistributionConfig {
                aliases: Some(Aliases {
                    quantity: 0,
                    items: None,
                }),
                ..a_config()
            },
        ));

        for (case, other) in &differing {
            assert!(
                !same_config(&baseline, other),
                "case {case}: expected a configuration differing in {case} to compare \
                 DIFFERENT, got the same; baseline {baseline:?} vs {other:?}"
            );
            assert!(
                same_config(other, other),
                "case {case}: expected a configuration to compare the same as itself"
            );
        }
    }
}
