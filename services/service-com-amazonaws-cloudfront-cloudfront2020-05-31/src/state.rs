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
//! The same holds for every other resource kind this store carries — a key value store, an
//! Anycast static IP list, a
//! response headers policy, an origin request policy, a cache policy, an origin access
//! control, a CloudFront function, a connection function, a public key, a key group, a
//! real-time log
//! configuration: each record is put
//! there by the call that creates it and is what every later read of that resource answers
//! from. A resource policy is carried the same way, except that it is keyed by the ARN of
//! the resource it is attached to rather than by an id of its own: that ARN is the only
//! identifier the resource-policy surface addresses a policy by.
//!
//! None of a response headers policy, an origin request policy, a cache policy, an
//! origin access control, a public key, a key group or a real-time log configuration has a
//! status
//! member or a propagation deadline, so
//! nothing about one is owed by the clock; neither is anything about a function or a
//! connection function, each of which reaches its `LIVE` stage through a publish CALL
//! rather than by waiting.
//!
//! Several things in here are owed by the CLOCK rather than by a call:
//!
//! * a distribution CloudFront has just created or reconfigured is `InProgress` until
//!   its configuration has propagated, and is `Deployed` after
//!   ([`PROPAGATION_DELAY`]);
//! * a streaming (RTMP) distribution is `InProgress` and `Deployed` on the same rule, for
//!   the same reason ([`PROPAGATION_DELAY`]);
//! * an invalidation is `InProgress` until its batch finishes, and `Completed` after;
//! * a key value store CloudFront has just created is `PROVISIONING` until it has been
//!   provisioned, and `READY` after ([`KVS_PROVISIONING_DELAY`]);
//! * a distribution tenant CloudFront has just created is `InProgress` until its
//!   configuration has propagated, and `Deployed` after ([`PROPAGATION_DELAY`]);
//! * a connection group CloudFront has just created is `InProgress` until its routing
//!   endpoint has propagated, and `Deployed` after ([`PROPAGATION_DELAY`]);
//! * an Anycast static IP list CloudFront has just created is `Deploying` until its
//!   allocated addresses have been deployed, and `Deployed` after ([`PROPAGATION_DELAY`]),
//!   and each of its IPAM CIDR configurations is provisioned on the same deadline.
//!
//! Each of them is materialised by
//! [`ComAmazonawsCloudfrontCloudfront20200531State::settle`],
//! which every handler calls before it reads: the transition was owed at the same
//! instant whether or not anybody called, so applying it on a read never changes what a
//! later caller sees.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

// NOTE: the three error shapes this module's message constructors below build
// (`DistributionAlreadyExists`, `PreconditionFailed`, `InvalidArgument`) are named at
// their use sites through their full `crate::ops::types::` path rather than added to
// this list, which every operation added to this service edits.
use crate::ops::types::{
    ActiveTrustedKeyGroups, ActiveTrustedSigners, Aliases, AllowedMethods, AnycastIpList,
    AnycastIpListSummary, CacheBehavior, CacheBehaviors,
    CachePolicy, CachePolicyAlreadyExists, CachePolicyConfig,
    CachedMethods, CloudFrontOriginAccessIdentity, CloudFrontOriginAccessIdentityConfig,
    CloudFrontOriginAccessIdentitySummary,
    CNAMEAlreadyExists, ConnectionGroup, ConnectionGroupSummary, ContinuousDeploymentPolicy,
    ContinuousDeploymentPolicyConfig,
    ContinuousDeploymentSingleHeaderConfig, ContinuousDeploymentSingleWeightConfig,
    CustomErrorResponse, CustomErrorResponses, CustomOriginConfig,
    Customizations, DefaultCacheBehavior, Distribution, DistributionConfig, DistributionSummary,
    DistributionTenant, DistributionTenantSummary, DomainConflict, DomainResult,
    EntityAlreadyExists,
    EntityNotFound, FieldLevelEncryption, FieldLevelEncryptionConfig,
    FieldLevelEncryptionProfile, FieldLevelEncryptionProfileConfig,
    FieldLevelEncryptionProfileSummary, FieldLevelEncryptionSummary,
    ForwardedValues,
    FunctionAssociations, GeoRestriction, Invalidation, InvalidationBatch, InvalidationSummary,
    InvalidTagging, IpamCidrConfig, IpamConfig, KGKeyPairIds, KeyGroup, KeyGroupConfig,
    KeyPairIds, KeyValueStore,
    LambdaFunctionAssociations,
    LoggingConfig, ManagedCertificateDetails, ManagedCertificateRequest, MonitoringSubscription,
    NoSuchCachePolicy,
    NoSuchCloudFrontOriginAccessIdentity, NoSuchDistribution, NoSuchInvalidation,
    NoSuchMonitoringSubscription, NoSuchOriginAccessControl, NoSuchOriginRequestPolicy,
    NoSuchResource,
    NoSuchResponseHeadersPolicy, Origin, OriginAccessControl, OriginAccessControlConfig,
    OriginAccessControlSummary, OriginRequestPolicy, OriginRequestPolicyAlreadyExists,
    OriginRequestPolicyConfig, OriginSslProtocols, Parameter, ParameterDefinition, Paths,
    PublicKey, PublicKeyConfig, PublicKeySummary, ResponseHeadersPolicy,
    ResponseHeadersPolicyAlreadyExists, ResponseHeadersPolicyConfig, Restrictions,
    SessionStickinessConfig, Signer, StreamingDistribution, StreamingDistributionConfig,
    StreamingDistributionSummary, StreamingLoggingConfig, StringSchemaConfig, Tag, Tags,
    TrafficConfig, TrustedKeyGroups, TrustedSigners, ValidationTokenDetail, ViewerCertificate,
    WebAclCustomization,
};

/// The CloudFront FUNCTION slice of this store: the function record, its two stages,
/// their quotas, the ARN and path a function is addressed by, and the modelled errors the
/// function operations answer with. Declared as a SUBMODULE so that surface grows without
/// touching the parts of this file the other resource kinds own; `pub use` keeps every
/// name reachable as `crate::state::<name>`, which is where each handler names it.
pub mod functions;
pub use functions::*;

/// The CONNECTION FUNCTION slice of this store: the connection function record, its two
/// stages, their quotas, the id, ARN and path one is addressed by, and the modelled errors
/// the connection function operations answer with. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod connection_functions;
pub use connection_functions::*;

/// The REAL-TIME LOG CONFIGURATION slice of this store: the configuration record, the ARN
/// one is addressed by, the bound a create is refused at, the rules a submitted
/// configuration satisfies, and the modelled errors the real-time log configuration
/// operations answer with. A SUBMODULE for the same reason [`functions`] is one; `pub use`
/// keeps every name reachable as `crate::state::<name>`.
pub mod realtime_log_configs;
pub use realtime_log_configs::*;

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

/// The most resource policies this emulator holds — one per resource that carries one.
///
/// Same construct and same reasoning as [`MAX_DISTRIBUTIONS`]: a policy record is what
/// `GetResourcePolicy` answers from, so it cannot be evicted, and the bound is a maximum
/// entry count refused on the write path before anything is inserted. Real CloudFront needs
/// no bound of its own here, because a policy exists only for a VPC origin and the quota on
/// those is enforced where one is created, which is why `PutResourcePolicy` models no limit
/// error at all; the refusal over this bound is therefore the modelled `InvalidArgument`,
/// the one general client refusal that operation declares. A write that REPLACES the policy
/// of a resource already in the store grows nothing, so it is never refused by it, and the
/// count sits far above any realistic client workflow.
pub const MAX_RESOURCE_POLICIES: usize = 1_000;

/// The most streaming (RTMP) distributions this emulator holds.
///
/// Same construct and same reasoning as [`MAX_DISTRIBUTIONS`]: a streaming distribution
/// record is what `GetStreamingDistribution` answers from, so it cannot be evicted and
/// the bound is a maximum entry count refused on the create path before anything is
/// minted. Over it, the refusal is the modelled `TooManyStreamingDistributions`:
/// "Processing your request would cause you to exceed the maximum number of streaming
/// distributions allowed."
pub const MAX_STREAMING_DISTRIBUTIONS: usize = 10_000;

/// The most invalidations this emulator holds for ONE distribution, or for ONE
/// distribution tenant.
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

/// The most alternate domain names (CNAMEs) ONE distribution's configuration may state.
///
/// CloudFront's published quota is 100 alternate domain names per distribution (the Amazon
/// CloudFront Developer Guide's "Quotas", general quotas on distributions). Over it, a
/// streaming distribution create is refused with the modelled
/// `TooManyStreamingDistributionCNAMEs`: "Your request contains more CNAMEs than are
/// allowed per distribution."
pub const MAX_DISTRIBUTION_CNAMES: usize = 100;

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

/// The `CustomizationActionType` a distribution tenant's web ACL customization carries when
/// a web ACL of its own is associated with it: "you can specify `override` to specify a
/// separate WAF web ACL for the distribution tenant". The other value the enum admits
/// (`disable`) is the one that takes the tenant out of its distribution's protections
/// without naming a web ACL, which is a customization a caller states rather than one an
/// association produces.
pub const CUSTOMIZATION_ACTION_OVERRIDE: &str = "override";

/// `DnsConfiguration$Status` for a domain that "is correctly configured and points to the
/// correct routing endpoint of the connection group".
pub const DNS_CONFIGURATION_VALID: &str = "valid-configuration";
/// `DnsConfiguration$Status` for a domain where "there is either a missing DNS record or
/// the DNS record exists but it's using an incorrect routing endpoint": the status for a
/// verification that was MADE and disagreed — a DNS record that resolves the domain to
/// some other endpoint.
pub const DNS_CONFIGURATION_INVALID: &str = "invalid-configuration";

/// `DnsConfiguration$Status` for a domain CloudFront "can't validate": the status for a
/// verification that could not be MADE — "CloudFront can't verify the DNS record, or the
/// DNS lookup request failed or timed out" — as opposed to one that was made and
/// disagreed. A name no DNS record this service can read resolves is in exactly that
/// position: recorded behaviour, which answers this rather than
/// [`DNS_CONFIGURATION_INVALID`] for a domain with no DNS record.
pub const DNS_CONFIGURATION_UNKNOWN: &str = "unknown-configuration";

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

/// The name this emulator gives the account's AUTOMATICALLY created connection group.
///
/// `ConnectionGroupSummary$Name` is required, so the group CloudFront creates for you
/// ("CloudFront will automatically create a default connection group for you") carries
/// one too. AWS documents no name for it, so this is the emulator's choice; it is stable
/// across reads, and a create that has already taken this name leaves the automatic one
/// to [`ComAmazonawsCloudfrontCloudfront20200531State::default_connection_group`]'s
/// fallback, which is derived from the group's own minted id.
pub const DEFAULT_CONNECTION_GROUP_NAME: &str = "default";

/// `CreateConnectionGroup$Ipv6Enabled` for a create that named none: "Enable IPv6 for
/// the connection group. The default is `true`."
pub const DEFAULT_CONNECTION_GROUP_IPV6_ENABLED: bool = true;

/// `CreateConnectionGroup$Enabled` for a create that named none.
///
/// "Enable the connection group" — the member is optional, and a connection group is
/// created to route the traffic of the distribution tenants placed in it, so leaving it
/// out asks for the routing group, exactly as [`DEFAULT_TENANT_ENABLED`] does for a
/// tenant.
pub const DEFAULT_CONNECTION_GROUP_ENABLED: bool = true;

/// How many connection groups this emulator holds.
///
/// `CreateConnectionGroup` models `EntityLimitExceeded` ("The entity limit has been
/// exceeded"), so the store is bounded; the bound is this emulator's, chosen like
/// [`MAX_KEY_VALUE_STORES`] rather than read off a published quota, and it is an entry
/// count refused before the insert.
pub const MAX_CONNECTION_GROUPS: usize = 1_000;

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

/// `AnycastIpList$Status` while the list's addresses are still being deployed.
///
/// The member documents its own values — "The status of the Anycast static IP list. Valid
/// values: `Deployed`, `Deploying`, or `Failed`." — so a list CloudFront has just accepted
/// is in this non-terminal one and the clock, not a later call, owes it
/// [`STATUS_DEPLOYED`]. `Failed` is reachable only by an allocation this emulator never
/// performs, so nothing here ever reports it.
pub const ANYCAST_IP_LIST_STATUS_DEPLOYING: &str = "Deploying";

/// What an `AnycastIpList$Id` this service MINTS starts with, as the ids in this crate's
/// `ListConnectionGroups` cases are written (`ipl-08d1a1b2c3d4e5f67`).
///
/// `AnycastIpList$Id` carries no pattern in the model, so this is the form this service
/// writes an id in rather than a constraint it may read an id against: an identifier a
/// caller submits is read with [`is_namespaced_resource_id`], which is why a list id under
/// another namespace label is answered as a list that is ABSENT rather than as an identifier
/// that cannot be read.
pub const ANYCAST_IP_LIST_ID_PREFIX: &str = "ipl-";

/// How many lowercase hex digits follow [`ANYCAST_IP_LIST_ID_PREFIX`] in an
/// `AnycastIpList$Id`, from the same ids: one mint draw (64 bits, 16 digits) zero-padded to
/// this width, so every id this service mints is the width a client reads.
pub const ANYCAST_IP_LIST_ID_HEX_LEN: usize = 17;

/// The `IpCount` values a create may allocate an Anycast static IP list for:
/// `CreateAnycastIpListRequest$IpCount` is "The number of static IP addresses that are
/// allocated to the Anycast static IP list. Valid values: 21 or 3." The member carries no
/// `@range` in the model, so the documented value set is what admits an `IpCount` and
/// anything else is an argument CloudFront cannot allocate for
/// ([`invalid_anycast_ip_count`]).
pub const ANYCAST_IP_COUNTS: [i32; 2] = [3, 21];

/// `IpAddressType` "ipv4" — "`ipv4` only"
/// (`smithy_query(query="shape",shape="IpAddressType")`). Also what a create that names no
/// address type allocates from: the list has to report the family its addresses came from,
/// and IPv4 is the one every Anycast static IP list carries.
pub const IP_ADDRESS_TYPE_IPV4: &str = "ipv4";

/// `IpAddressType` "ipv6" — "`ipv6` only".
pub const IP_ADDRESS_TYPE_IPV6: &str = "ipv6";

/// `IpAddressType` "dualstack" — "`dualstack` - Allocate a list of both IPv4 and IPv6
/// addresses", so a list of this type carries addresses of BOTH families.
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "dualstack";

/// `IpamCidrStatus` while an IPAM CIDR configuration's range is still being provisioned
/// (`smithy_query(query="shape",shape="IpamCidrStatus")`), which is where a configuration
/// a create just submitted starts: the status is the service's, never the caller's.
pub const IPAM_CIDR_STATUS_PROVISIONING: &str = "provisioning";

/// `IpamCidrStatus` once the range has been provisioned — owed by the same clock that owes
/// the list its deployment, since the provisioning is part of that deployment.
pub const IPAM_CIDR_STATUS_PROVISIONED: &str = "provisioned";

/// The IPv4 block an Anycast static IP is allocated from: TEST-NET-2, the
/// `198.51.100.0/24` block reserved for documentation (RFC 5737). The addresses CloudFront
/// allocates are globally routable ones this emulator cannot hand out, so it allocates from
/// a block reserved for exactly this purpose rather than from address space that belongs to
/// somebody.
pub const ANYCAST_IPV4_PREFIX: &str = "198.51.100.";

/// How many addresses [`ANYCAST_IPV4_PREFIX`]'s block can serve: a `/24` holds 256
/// addresses, of which the network and broadcast ones are no host address, so host numbers
/// run 1..=254. It is also the bound on the whole Anycast static IP surface of this
/// emulator — a create that cannot be served an address from the block is refused with the
/// modelled `EntityLimitExceeded` ([`anycast_ip_pool_exhausted`]).
pub const ANYCAST_IP_POOL: u32 = 254;

/// The IPv6 block an Anycast static IP is allocated from: `2001:db8::/32`, reserved for
/// documentation (RFC 3849), for the reason [`ANYCAST_IPV4_PREFIX`]'s block is.
pub const ANYCAST_IPV6_PREFIX: &str = "2001:db8::";

/// `ManagedCertificateStatus` while the domains of a CloudFront managed ACM certificate
/// are still being validated
/// (`smithy_query(query="shape",shape="ManagedCertificateStatus")`).
///
/// A certificate CloudFront has just requested for a distribution tenant has not been
/// validated yet, so the request answers this non-terminal status and the clock — not a
/// later call — owes the certificate [`CERTIFICATE_STATUS_ISSUED`].
pub const CERTIFICATE_STATUS_PENDING_VALIDATION: &str = "pending-validation";

/// `ManagedCertificateStatus` once the certificate has been validated and issued: the
/// terminal status a tenant's viewers are served over it in.
pub const CERTIFICATE_STATUS_ISSUED: &str = "issued";

/// How long after the call that requests it a CloudFront managed ACM certificate is owed
/// [`CERTIFICATE_STATUS_ISSUED`].
///
/// No validation deadline is published, so — like [`PROPAGATION_DELAY`] — this span is the
/// EMULATOR's choice rather than a claim about the service: long enough that a caller
/// reading the certificate straight after the request still observes
/// [`CERTIFICATE_STATUS_PENDING_VALIDATION`], short enough that a caller that waits
/// observes the terminal one. The instant is stored on the record; nothing ever sleeps for
/// it.
pub const CERTIFICATE_VALIDATION_DELAY: Duration = Duration::from_millis(500);

/// The `ValidationTokenHost` that asks CLOUDFRONT to serve the HTTP validation token of a
/// CloudFront managed ACM certificate: "For `cloudfront`, CloudFront will automatically
/// serve the validation token. Choose this mode if you can point the domain's DNS to
/// CloudFront immediately"
/// (`smithy_query(query="shape",shape="ManagedCertificateRequest")`).
///
/// Serving the token AT the domain is only possible once the domain resolves to a
/// CloudFront resource, which is why this mode — and not the other one, which leaves the
/// token to the caller's own infrastructure — makes the create check the domain's DNS
/// before it admits the request
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::unverifiable_certificate_domain`]).
pub const VALIDATION_TOKEN_HOST_CLOUDFRONT: &str = "cloudfront";

/// The well-known path an HTTP domain validation token is served at, between the host and
/// the token's own file name.
pub const VALIDATION_TOKEN_PATH: &str = "/.well-known/pki-validation/";

/// The suffix of the host that serves the content of an HTTP domain validation token, the
/// one a self-hosted validation redirects to.
pub const VALIDATION_HOST_SUFFIX: &str = "acm-validations.aws";

/// The region a CloudFront managed ACM certificate lives in: CloudFront serves viewers
/// over certificates in `us-east-1`, so that is the region segment of one's ARN, and the
/// only region segment any ARN of this global service's resources carries.
pub const CERTIFICATE_REGION: &str = "us-east-1";

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

/// The most origin access controls this emulator holds.
///
/// CloudFront's published quota is 100 origin access controls per account, and
/// `CreateOriginAccessControl` models `TooManyOriginAccessControls` — "The number of
/// origin access controls in your Amazon Web Services account exceeds the maximum
/// allowed" — for a create that would exceed it. A control record is what a later read
/// answers from, so it cannot be evicted: the create path is refused at this entry count
/// rather than growing further.
pub const MAX_ORIGIN_ACCESS_CONTROLS: usize = 100;

/// The longest `OriginAccessControlConfig$Name` this service accepts.
///
/// The member carries no `@length` in the model, but its documentation states the bound:
/// "A name to identify the origin access control. You can specify up to 64 characters."
pub const MAX_ORIGIN_ACCESS_CONTROL_NAME: usize = 64;

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

/// The prefix a public key id carries.
///
/// CloudFront answers a `CreatePublicKey` with an id like `K2K8NC4HVFE3MO` — a `K`
/// followed by [`ID_BODY_LEN`] characters of the id alphabet — so the prefix is what
/// separates the public key namespace from the `E`-prefixed one every other CloudFront
/// entity id is drawn from.
pub const PUBLIC_KEY_ID_PREFIX: char = 'K';

/// The most public keys this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_CACHE_POLICIES`]: a public key
/// record is what a later read answers from, so it cannot be evicted, and the create path
/// is refused at this entry count with the modelled `TooManyPublicKeys` ("The maximum
/// number of public keys for field-level encryption have been created. To create a new
/// public key, delete one of the existing keys.") instead of growing further. CloudFront's
/// own ceiling is an account quota that can be raised and the model states no number, so
/// this bound is the emulator's rather than a claim about the service.
pub const MAX_PUBLIC_KEYS: usize = 1_000;

/// The most key groups this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_PUBLIC_KEYS`]: a key group record
/// is what a later read answers from, so it cannot be evicted, and the create path is
/// refused at this entry count with the modelled `TooManyKeyGroups` ("You have reached the
/// maximum number of key groups for this Amazon Web Services account.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_KEY_GROUPS: usize = 1_000;

/// The most public keys ONE key group may name.
///
/// CloudFront's published quota is 5 public keys in a single key group (the Amazon
/// CloudFront Developer Guide's "Quotas", quotas on signed URLs and signed cookies), and
/// the modelled refusal names the same scope: "The number of public keys in this key group
/// is more than the maximum allowed." The list is stored whole, so this also bounds one
/// group's contribution to the store.
pub const MAX_PUBLIC_KEYS_IN_KEY_GROUP: usize = 5;

/// The prefix a field-level encryption profile id carries.
///
/// CloudFront answers a `CreateFieldLevelEncryptionProfile` with an id like
/// `P280MFCLSYOCVU` — a `P` followed by [`ID_BODY_LEN`] characters of the id alphabet — so
/// the prefix is what separates the profile namespace from the `E`-prefixed one every other
/// CloudFront entity id is drawn from and from the `K`-prefixed public key one.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX: char = 'P';

/// The most field-level encryption profiles this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_PUBLIC_KEYS`]: a profile record is
/// what a later read answers from, so it cannot be evicted, and the create path is refused
/// at this entry count with the modelled `TooManyFieldLevelEncryptionProfiles` ("The maximum
/// number of profiles for field-level encryption have been created.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_PROFILES: usize = 1_000;

/// The prefix a field-level encryption CONFIGURATION id carries.
///
/// CloudFront answers a `CreateFieldLevelEncryptionConfig` with an id in the shape its own
/// answers show — a `C` followed by [`ID_BODY_LEN`] characters of the id alphabet — so the
/// prefix is what separates the configuration namespace from the `P`-prefixed profile one,
/// the `K`-prefixed public key one and the `E`-prefixed distribution one.
pub const FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX: char = 'C';

/// The most field-level encryption configurations this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as
/// [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`]: a configuration record is what a later read
/// answers from, so it cannot be evicted, and the create path is refused at this entry
/// count with the modelled `TooManyFieldLevelEncryptionConfigs` ("The maximum number of
/// configurations for field-level encryption have been created.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS: usize = 1_000;

/// The most query argument-profile mappings ONE field-level encryption configuration may
/// carry.
///
/// CHOSEN by this emulator: the mappings are stored whole inside the configuration record,
/// so this is what bounds one configuration's contribution to the store, and a request past
/// it is refused with the modelled `TooManyFieldLevelEncryptionQueryArgProfiles` ("The
/// maximum number of query arg profiles for field-level encryption have been created.").
/// CloudFront's own ceiling is an account quota that can be raised and the model states no
/// number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES: usize = 100;

/// The most content type-profile mappings ONE field-level encryption configuration may
/// carry.
///
/// CHOSEN by this emulator, for the same reason as
/// [`MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES`], and refused with the modelled
/// `TooManyFieldLevelEncryptionContentTypeProfiles` ("The maximum number of content type
/// profiles for field-level encryption have been created.").
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES: usize = 100;

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

/// The `ConnectionMode` value that makes a distribution a STANDARD one.
///
/// `DistributionConfig$ConnectionMode` is optional and the service applies nothing to it
/// (see [`resolve_distribution_config`]), so a configuration that names none describes a
/// standard distribution: one with an edge domain of its own that serves its own traffic.
/// The model's `ConnectionMode` enum admits exactly `direct` and `tenant-only`.
pub const CONNECTION_MODE_DIRECT: &str = "direct";

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

/// `NoSuchResource`'s message, in the form CloudFront answers that code with: the error
/// covers whichever resource the request named, so the sentence names none of them, and
/// nothing in it is built out of the call.
pub const NO_SUCH_RESOURCE_MESSAGE: &str =
    "The specified resource doesn't exist. Update your request and try again.";

/// The modelled `NoSuchResource` for an ARN no resource this emulator holds carries.
///
/// The wording is the one CloudFront answers for this error class, and it names no part
/// of the request: the ARN that was asked about is not part of the message, so nothing
/// here is built out of the call.
pub fn no_such_resource() -> NoSuchResource {
    NoSuchResource {
        message: Some(NO_SUCH_RESOURCE_MESSAGE.to_string()),
    }
}

/// Whether `path` is one of the paths `pattern` names, with `*` standing for any run of
/// characters and `?` for exactly one.
///
/// ONE matcher for the two places CloudFront matches a path against a pattern: a cache
/// behaviour's `CacheBehavior$PathPattern` ("The pattern that specifies which requests to
/// apply the behavior to", whose wildcards the member documents as `*` and `?`) and an
/// invalidation's `Paths$Items` ("The path of the object to invalidate", which may end in
/// `*`). A pattern that does not begin at the root is matched against the path without its
/// leading `/` as well, because a path pattern may be written with or without one.
pub fn matches_path_pattern(pattern: &str, path: &str) -> bool {
    matches_glob(pattern, path) || matches_glob(pattern, path.trim_start_matches('/'))
}

/// The most character comparisons one wildcard match makes before it reports no match.
///
/// A single-backtrack glob walk is O(pattern x subject), and both come from caller-stored
/// or caller-sent values, so the work one comparison can ask for is capped here. The cap
/// is far above any real path and pattern (a `PathPattern` is at most 255 characters and a
/// request path far less than this bound), so no real match reaches it; one that does is
/// reported as NO match, which for a cache behaviour means the next behaviour (and in the
/// end the default one) decides the request, and for an invalidation means the object is
/// left in the cache rather than removed on the strength of a comparison that was cut
/// short.
const MAX_GLOB_COMPARISONS: usize = 1 << 16;

/// The wildcard match itself, over characters: the standard linear walk with one
/// backtrack point per `*`.
fn matches_glob(pattern: &str, subject: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let subject: Vec<char> = subject.chars().collect();
    let (mut at_pattern, mut at_subject) = (0usize, 0usize);
    // Where the last `*` was, and how much of the subject it has consumed so far.
    let mut star: Option<usize> = None;
    let mut consumed = 0usize;
    let mut comparisons = 0usize;
    while at_subject < subject.len() {
        comparisons += 1;
        if comparisons > MAX_GLOB_COMPARISONS {
            return false;
        }
        let literal = at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == subject[at_subject]);
        if literal {
            at_pattern += 1;
            at_subject += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            consumed = at_subject;
            at_pattern += 1;
        } else if let Some(wildcard) = star {
            // Give the wildcard one more character and try the rest of the pattern again.
            at_pattern = wildcard + 1;
            consumed += 1;
            at_subject = consumed;
        } else {
            return false;
        }
    }
    while at_pattern < pattern.len() && pattern[at_pattern] == '*' {
        at_pattern += 1;
    }
    at_pattern == pattern.len()
}

/// One response a distribution's edge holds in its cache.
///
/// The in-process transport this emulator fetches an origin over carries a status and a
/// body (see `crate::service::edge`), so those are what an entry holds, beside the
/// instants that decide whether it may still be served.
#[derive(Debug, Clone)]
pub struct CachedEdgeResponse {
    /// The status the origin answered.
    pub status: u16,
    /// The bytes the origin answered.
    pub body: Vec<u8>,
    /// The path the viewer request carried, so an invalidation can be matched against
    /// this entry when a batch is created.
    pub path: String,
    /// When the origin fetch that filled this entry happened: the order the cache evicts
    /// in when it is full.
    pub stored_at: SystemTime,
    /// When this entry stops being servable, from the cache behaviour's TTL.
    pub expires_at: SystemTime,
}

/// How many cached responses one distribution's edge holds at once.
///
/// A real edge cache is bounded by the storage of an edge location; this emulator holds
/// its cache in memory beside the distribution, so it is bounded by an entry count and
/// the least recently filled entry is dropped to make room. The bound is this emulator's
/// own, not CloudFront's: eviction is invisible to a caller except as a cache MISS, which
/// is a thing CloudFront answers too.
pub const MAX_EDGE_CACHE_ENTRIES: usize = 256;

/// The most path-pattern comparisons one invalidation makes against the edge cache before
/// it empties the cache instead ([`StoredDistribution::invalidate_edge_cache`]).
///
/// This emulator's own bound on the work one call can ask for: the cache holds at most
/// [`MAX_EDGE_CACHE_ENTRIES`] entries and a batch names at most
/// [`MAX_INVALIDATION_PATHS`] paths, so their product is what this caps.
pub const MAX_INVALIDATION_MATCHES: usize = 4096;

/// The CloudFront resource TYPES the tagging surface accepts, as the resource part of an
/// ARN spells each of them.
///
/// Read off the model: a resource type is taggable exactly where the operation that
/// creates it takes tags — `CreateDistributionWithTags` and
/// `CreateStreamingDistributionWithTags` through their `…ConfigWithTags` payload, and
/// `CreateFunction`, `CreateConnectionFunction`, `CreateKeyValueStore`,
/// `CreateAnycastIpList`, `CreateConnectionGroup` and `CreateDistributionTenant` through
/// their `Tags` member (`smithy_query(query="operation", operation="…")`). Every other
/// kind of CloudFront resource — a cache policy, an origin request policy, a response
/// headers policy, an origin access control, a public key, a real-time log configuration,
/// a continuous deployment policy, an origin access identity — is created by an operation
/// that takes no tags at all, so its ARN names no taggable type.
///
/// Sorted, so the list reads as the SET it is.
pub const TAGGABLE_RESOURCE_TYPES: [&str; 8] = [
    "anycast-ip-list",
    "connection-function",
    "connection-group",
    "distribution",
    "distribution-tenant",
    "function",
    "key-value-store",
    "streaming-distribution",
];

/// The resource TYPE an ARN names — the token its resource part opens with — or `None`
/// where the ARN carries no resource part at all.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and this service's
/// resources spell their resource part `<type>/<id>`, so the type is the token up to the
/// first separator. `ResourceARN`'s pattern is already enforced before any handler runs,
/// so what is read here is the one part of the ARN the pattern leaves open.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let end = resource.find(['/', ':']).unwrap_or(resource.len());
    let kind = &resource[..end];
    (!kind.is_empty()).then_some(kind)
}

/// The modelled `InvalidArgument` (HTTP 400) a tagging call naming a resource type the
/// tagging surface does not accept is refused with, or `None` when the type is one it
/// accepts (so the question becomes whether the store holds that resource).
///
/// What the refusal states is the CONSTRAINT on the parameter — the resource type the
/// submitted ARN names is not one the tagging surface takes — so the one variable part of
/// the sentence is that type, read out of the request's own ARN. It is decided BEFORE any
/// lookup: a type the surface does not accept is a parameter the caller has to fix,
/// whether or not a resource of that type exists, which is what keeps `NoSuchResource` for
/// an accepted type whose resource is absent.
///
/// An ARN carrying no resource part names no type for the sentence to state, so it is left
/// to the existence lookup the accepted types go through.
pub fn untaggable_resource_type(arn: &str) -> Option<crate::ops::types::InvalidArgument> {
    let kind = arn_resource_type(arn)?;
    if TAGGABLE_RESOURCE_TYPES.contains(&kind) {
        return None;
    }
    Some(crate::ops::types::InvalidArgument {
        message: Some(format!("The parameter resource type: {kind} is invalid.")),
    })
}

/// One invalidation batch submitted against one distribution or one distribution tenant.
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

/// Materialise the batch completions the clock owes `invalidations` at `now`.
///
/// Idempotent and monotonic: a batch that has already finished stays finished, and one
/// whose deadline has not passed is left alone, so applying this on a read never changes
/// what a later caller sees. The invalidations of a distribution and those of a
/// distribution tenant are held in the same kind of map and finish under the same rule, so
/// this is one function rather than one per owner.
pub fn settle_invalidations(
    invalidations: &mut BTreeMap<String, StoredInvalidation>,
    now: SystemTime,
) {
    for invalidation in invalidations.values_mut() {
        if invalidation.status != STATUS_COMPLETED && now >= invalidation.completed_due {
            invalidation.status = STATUS_COMPLETED.to_string();
        }
    }
}

/// `invalidations` in the order a list serves them.
///
/// CloudFront documents the order on the `Marker` of every invalidation listing: "the
/// results are returned in decreasing order from most recent to oldest, [so] the most
/// recent results are on the first page". Two invalidations stamped with the SAME instant
/// are ordered by id, which makes this a stable TOTAL order rather than merely a sorted
/// one — and that is what lets a `Marker` naming one of them resume at exactly one
/// position.
///
/// Reads [`StoredInvalidation::status`] nowhere, so the caller decides whether to settle
/// the clock first; every list handler does, because the summaries it builds from these
/// records report that status.
pub fn invalidations_most_recent_first(
    invalidations: &BTreeMap<String, StoredInvalidation>,
) -> Vec<&StoredInvalidation> {
    let mut ordered: Vec<&StoredInvalidation> = invalidations.values().collect();
    ordered.sort_by(|left, right| {
        right
            .create_time
            .cmp(&left.create_time)
            .then_with(|| left.id.cmp(&right.id))
    });
    ordered
}

/// ONE page of `invalidations`, as the `InvalidationList` every CloudFront invalidation
/// listing answers with — or `None` when `marker` names none of them.
///
/// The page contract is the same whichever resource the invalidations hang off, so it
/// lives here once rather than in each handler:
///
/// * the order is the documented one, most recent first
///   ([`invalidations_most_recent_first`]);
/// * "To get the next page of results, set Marker to the value of the NextMarker from the
///   current page's response. This value is the same as the ID of the last invalidation
///   batch on that page" — so the marker is a resume point this service handed out, and
///   this page starts after the invalidation it names. A value naming none of them was
///   never such a resume point: this answers `None`, and the caller refuses it with the
///   modelled `InvalidArgument` naming the resource it asked about, rather than silently
///   serving the first page again;
/// * `page_size` is how many summaries this response serves (see [`page_size`]), and
///   `max_items` is reported as "the value that you provided for the MaxItems request
///   parameter";
/// * `NextMarker` is present exactly when invalidations remain, carrying the id this page
///   ended at, and `IsTruncated` says the same thing;
/// * `Quantity` is the length of `Items`, and a page that lists nothing carries no `Items`
///   element at all rather than an empty one.
///
/// Each summary is [`StoredInvalidation::to_summary`] of the one stored record, so a list
/// page and a read of the same invalidation can never disagree about it.
pub fn invalidation_page(
    invalidations: &BTreeMap<String, StoredInvalidation>,
    marker: Option<&str>,
    max_items: i32,
    page_size: usize,
) -> Option<crate::ops::types::InvalidationList> {
    let ordered = invalidations_most_recent_first(invalidations);
    let start = match marker {
        Some(marker) => {
            ordered
                .iter()
                .position(|invalidation| invalidation.id == marker)?
                + 1
        }
        None => 0,
    };

    let remaining = &ordered[start..];
    let served = remaining.len().min(page_size);
    let items: Vec<InvalidationSummary> = remaining[..served]
        .iter()
        .map(|invalidation| invalidation.to_summary())
        .collect();
    let is_truncated = remaining.len() > served;

    Some(crate::ops::types::InvalidationList {
        marker: marker.unwrap_or_default().to_string(),
        next_marker: if is_truncated {
            items.last().map(|summary| summary.id.clone())
        } else {
            None
        },
        max_items,
        is_truncated,
        quantity: i32::try_from(items.len()).unwrap_or(i32::MAX),
        items: if items.is_empty() { None } else { Some(items) },
    })
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
    /// The additional-CloudWatch-metrics subscription associated with this distribution,
    /// or `None` while it has none.
    ///
    /// A sub-resource of the distribution rather than a member of its configuration —
    /// `CreateMonitoringSubscription`, `GetMonitoringSubscription` and
    /// `DeleteMonitoringSubscription` address it under
    /// `/distributions/{Id}/monitoring-subscription` — so it is carried on this record
    /// and released with it, and changing it leaves the configuration, the version token
    /// and the propagation status of the distribution exactly as they were. A
    /// distribution is created without one: nothing but a create puts one here.
    pub monitoring_subscription: Option<MonitoringSubscription>,
    /// What this distribution's edge holds in its cache, keyed by the cache key the edge
    /// computed for the request ([`crate::service::edge`]).
    ///
    /// A sub-resource of the running distribution rather than a member of its
    /// configuration: nothing reports it, and filling it changes no value any read
    /// answers with.
    pub edge_cache: BTreeMap<String, CachedEdgeResponse>,
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
        settle_invalidations(&mut self.invalidations, now);
    }

    /// This distribution's invalidations in the order a list serves them — the one order
    /// every invalidation listing uses ([`invalidations_most_recent_first`]).
    pub fn invalidations_most_recent_first(&self) -> Vec<&StoredInvalidation> {
        invalidations_most_recent_first(&self.invalidations)
    }

    /// The cached response the edge may serve for `key`, or `None` when it must fetch the
    /// origin.
    ///
    /// An entry is servable while its TTL has not run out. An invalidation does not have
    /// to be consulted here: a batch removes what it names from the cache when it is
    /// CREATED ([`StoredDistribution::invalidate_edge_cache`]), so this is one map lookup
    /// and a comparison however many batches the distribution has had. A READ: it reports
    /// what the cache holds and changes nothing.
    pub fn cached_edge_response(&self, key: &str, now: SystemTime) -> Option<&CachedEdgeResponse> {
        self.edge_cache
            .get(key)
            .filter(|entry| now < entry.expires_at)
    }

    /// Remove from the edge cache every object `paths` names — "you can invalidate files
    /// ... to remove them from CloudFront edge caches".
    ///
    /// Done once, by the call that creates the batch, rather than on every later cache
    /// read: one invalidation is one pass over the at most [`MAX_EDGE_CACHE_ENTRIES`]
    /// entries, and a viewer request pays nothing for the batches that came before it.
    /// A batch whose paths and the cache between them would need more comparisons than
    /// [`MAX_INVALIDATION_MATCHES`] empties the cache instead of matching: removing more
    /// than the batch named can only cost a later request a cache MISS, which is a thing
    /// CloudFront answers too, and it keeps the work one call can ask for bounded.
    pub fn invalidate_edge_cache(&mut self, paths: &[String]) {
        if self.edge_cache.is_empty() || paths.is_empty() {
            return;
        }
        let comparisons = self.edge_cache.len().saturating_mul(paths.len());
        if comparisons > MAX_INVALIDATION_MATCHES {
            self.edge_cache.clear();
            return;
        }
        self.edge_cache.retain(|_, entry| {
            !paths
                .iter()
                .any(|path| matches_path_pattern(path, &entry.path))
        });
    }

    /// Hold `entry` in this distribution's edge cache under `key`.
    ///
    /// Bounded by [`MAX_EDGE_CACHE_ENTRIES`]: once the cache is full the entry filled
    /// longest ago makes room for the new one.
    pub fn store_edge_response(&mut self, key: String, entry: CachedEdgeResponse) {
        while self.edge_cache.len() >= MAX_EDGE_CACHE_ENTRIES {
            let oldest = self
                .edge_cache
                .iter()
                .min_by_key(|(_, held)| held.stored_at)
                .map(|(held_key, _)| held_key.clone());
            match oldest {
                Some(held_key) => {
                    self.edge_cache.remove(&held_key);
                }
                None => break,
            }
        }
        self.edge_cache.insert(key, entry);
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
    /// projection of one record — `Distribution` and `DistributionSummary` alike — reports
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
    ///
    /// `key_groups` are the account's stored key groups, which the active key group set
    /// below reports the public keys of: a distribution's payload names the key groups its
    /// configuration trusts, and the keys in each one are the ones those RECORDS hold.
    pub fn to_distribution(
        &self,
        key_groups: &BTreeMap<String, StoredKeyGroup>,
    ) -> Distribution {
        Distribution {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            in_progress_invalidation_batches: self.in_progress_invalidation_batches(),
            domain_name: self.reported_domain_name(),
            active_trusted_signers: Some(self.active_trusted_signers()),
            active_trusted_key_groups: Some(self.active_trusted_key_groups(key_groups)),
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
    /// A signer's `KeyPairIds` are the ACTIVE CloudFront key pairs of that account — the
    /// account-wide key pairs a root user creates, which this service's API cannot create
    /// (no operation in the model mints one), so no account has one and the reported set is
    /// empty. The public keys a key group holds are a different thing, reported by
    /// [`StoredDistribution::active_trusted_key_groups`] from the stored key groups.
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
    /// group the distribution's cache behaviours trust, each named once, and `Enabled`
    /// reports whether the configuration trusts any key group at all.
    ///
    /// A group's `KeyPairIds` are "the identifiers of the public keys in each key group
    /// that CloudFront can use to verify the signatures", which is what the key group
    /// RECORD `key_groups` holds for that id states — so the set is read from the store
    /// rather than from this distribution's configuration, and a trusted id the account
    /// holds no key group for contributes no key.
    pub fn active_trusted_key_groups(
        &self,
        key_groups: &BTreeMap<String, StoredKeyGroup>,
    ) -> ActiveTrustedKeyGroups {
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
                    .map(|group| {
                        let keys = key_groups
                            .get(&group)
                            .map(|stored| stored.config.items.as_slice())
                            .unwrap_or_default();
                        KGKeyPairIds {
                            key_group_id: Some(group),
                            key_pair_ids: Some(KeyPairIds {
                                quantity: i32::try_from(keys.len()).unwrap_or(i32::MAX),
                                items: (!keys.is_empty()).then(|| keys.to_vec()),
                            }),
                        }
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

/// One CloudFront streaming (RTMP) distribution, as this emulator persists it.
///
/// The record is the single source of truth for the distribution's id, ARN, domain name,
/// status, version token (`ETag`) and configuration, so the create, a later get and a
/// later listing all answer from it rather than from the request that is being served —
/// the same shape [`StoredDistribution`] takes for a web distribution.
#[derive(Debug, Clone)]
pub struct StoredStreamingDistribution {
    /// `StreamingDistribution$Id`, allocated at creation and never reused.
    pub id: String,
    /// `StreamingDistribution$ARN`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `StreamingDistribution$DomainName`, allocated at creation and stable for the
    /// distribution's whole life.
    pub domain_name: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredStreamingDistribution::deployed_due`], then
    /// [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this distribution's current configuration is owed to be `Deployed`.
    pub deployed_due: SystemTime,
    /// `StreamingDistribution$LastModifiedTime`: when the configuration last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this distribution.
    pub etag: String,
    /// The configuration as submitted, with every member the service itself materialises
    /// resolved into it ([`resolve_streaming_distribution_config`]), stored whole so a
    /// later read returns the configuration the distribution actually runs under.
    pub config: StreamingDistributionConfig,
    /// The tags associated with this distribution. A streaming distribution is one of the
    /// resource types the tagging surface accepts ([`TAGGABLE_RESOURCE_TYPES`]), so the
    /// set lives on the record the rest of the distribution is read from — the same place
    /// [`StoredDistribution::tags`] lives.
    pub tags: TagSet,
}

impl StoredStreamingDistribution {
    /// Apply every transition the clock owes this streaming distribution at `now`.
    ///
    /// Idempotent and monotonic, for the same reason [`StoredDistribution::settle`] is: a
    /// status that is already terminal stays put, so applying this on a read cannot change
    /// what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != STATUS_DEPLOYED && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// "A complex type that lists the Amazon Web Services accounts, if any, that you
    /// included in the `TrustedSigners` complex type for this distribution."
    ///
    /// Read from the stored configuration, which is what says who the trusted signers are;
    /// each account is named once, at the position it first appears. A signer's
    /// `KeyPairIds` are the ACTIVE CloudFront key pairs of that account, and no call in
    /// this emulator creates a CloudFront key pair, so the reported set is empty — the
    /// same rule [`StoredDistribution::active_trusted_signers`] follows.
    pub fn active_trusted_signers(&self) -> ActiveTrustedSigners {
        let accounts = first_seen(
            self.config
                .trusted_signers
                .items
                .as_deref()
                .unwrap_or_default()
                .iter(),
        );
        ActiveTrustedSigners {
            enabled: self.config.trusted_signers.enabled,
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

    /// The `StreamingDistribution` projection `CreateStreamingDistribution` and
    /// `GetStreamingDistribution` return.
    pub fn to_streaming_distribution(&self) -> StreamingDistribution {
        StreamingDistribution {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            domain_name: self.domain_name.clone(),
            active_trusted_signers: self.active_trusted_signers(),
            streaming_distribution_config: self.config.clone(),
        }
    }

    /// The `StreamingDistributionSummary` projection `ListStreamingDistributions` returns.
    ///
    /// Every member it shares with
    /// [`StoredStreamingDistribution::to_streaming_distribution`] takes the same value:
    /// both read the one stored record. The configuration is stored resolved
    /// ([`resolve_streaming_distribution_config`]), so the members the summary requires
    /// but the configuration makes optional are present on it.
    pub fn to_summary(&self) -> StreamingDistributionSummary {
        let config = &self.config;
        StreamingDistributionSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            domain_name: self.domain_name.clone(),
            s3_origin: config.s3_origin.clone(),
            aliases: config.aliases.clone().unwrap_or_default(),
            trusted_signers: config.trusted_signers.clone(),
            comment: config.comment.clone(),
            price_class: config
                .price_class
                .clone()
                .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
            enabled: config.enabled,
        }
    }
}

/// Resolve every member of a submitted `StreamingDistributionConfig` that the SERVICE
/// supplies, so the stored record is the configuration the distribution actually runs
/// under rather than the request document that asked for it.
///
/// Three members are optional on the wire and mean something definite when left out, so
/// each is materialised here once, when the configuration is STORED:
///
/// * `Aliases` — "a complex type that contains information about CNAMEs (alternate domain
///   names), if any": naming none is the empty wrapper;
/// * `Logging` — "a complex type that controls whether access logs are written": naming no
///   logging configuration means logging is off, with no bucket and no prefix;
/// * `PriceClass` — the price class the distribution serves from, which is
///   [`DEFAULT_PRICE_CLASS`] where a configuration states none, exactly as it is for a web
///   distribution ([`resolve_distribution_config`]).
///
/// Resolving on the way IN is what makes two creates that describe the same distribution —
/// one stating a default explicitly, one leaving it out — the same request to the
/// caller-reference comparison.
pub fn resolve_streaming_distribution_config(
    mut config: StreamingDistributionConfig,
) -> StreamingDistributionConfig {
    config.aliases = Some(config.aliases.unwrap_or(Aliases {
        quantity: 0,
        items: None,
    }));
    config.logging = Some(config.logging.unwrap_or(StreamingLoggingConfig {
        enabled: false,
        bucket: String::new(),
        prefix: String::new(),
    }));
    config.price_class = Some(
        config
            .price_class
            .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
    );
    config
}

/// Whether two streaming distribution configurations ask for the same thing, which is what
/// decides whether a repeated `CallerReference` is the same create again or a second
/// streaming distribution under a reference that is already taken.
///
/// The same comparison [`same_origin_access_identity_config`] makes, for the same reasons:
/// every member a caller can submit takes part — down to the members a request left out,
/// which are resolved before the comparison — and comparing the DESERIALIZED
/// configurations is what makes it insensitive to the request document's whitespace and
/// member order.
pub fn same_streaming_distribution_config(
    left: &StreamingDistributionConfig,
    right: &StreamingDistributionConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
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

/// One CloudFront Anycast static IP list, as this emulator persists it.
///
/// The record is the single source of truth for the list's id, ARN, name, status, address
/// type, the addresses ALLOCATED to it, its IPAM CIDR configurations, its
/// `LastModifiedTime` and its version token, so the create, a later read and a later
/// mutation all answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredAnycastIpList {
    /// `AnycastIpList$Id`, minted at creation. Also the key this list is held under and
    /// the path segment `GetAnycastIpList` addresses it by.
    pub id: String,
    /// `AnycastIpList$Name` — "The name of the Anycast static IP list", as submitted.
    pub name: String,
    /// `AnycastIpList$Arn`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// [`ANYCAST_IP_LIST_STATUS_DEPLOYING`] until
    /// [`StoredAnycastIpList::deployed_due`], then [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this list is owed [`STATUS_DEPLOYED`]. Stored rather than slept for.
    pub deployed_due: SystemTime,
    /// `AnycastIpList$IpAddressType` — the family the addresses below were allocated
    /// from, [`IP_ADDRESS_TYPE_IPV4`] when the create named none.
    pub ip_address_type: String,
    /// `AnycastIpList$IpCount` — "The number of IP addresses in the Anycast static IP
    /// list", so it is the length of [`StoredAnycastIpList::anycast_ip_hosts`].
    pub ip_count: i32,
    /// The address SLOTS allocated to this list out of the service's pool: allocated ONCE,
    /// by the create, and held for the list's whole life, so no address is ever allocated
    /// to two lists.
    ///
    /// `AnycastIpList$AnycastIps` — "The static IP addresses that are allocated to the
    /// Anycast static IP list" — is these slots rendered in the families the list's
    /// `IpAddressType` admits ([`anycast_ips_of`]), so a list that changes its address type
    /// reports the SAME allocation in the new families rather than taking a second one.
    pub anycast_ip_hosts: Vec<u32>,
    /// The IPAM CIDR configurations this list was created or updated with, each carrying
    /// the status the SERVICE put on it. Empty when none were named, which is what makes
    /// `AnycastIpList$IpamConfig` absent rather than an element describing nothing.
    pub ipam_cidr_configs: Vec<IpamCidrConfig>,
    /// `AnycastIpList$LastModifiedTime` — "The last time the Anycast static IP list was
    /// modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this list.
    pub etag: String,
    /// The tags submitted with the create, in the SAME representation every other tagged
    /// resource of this service uses, so one tagging rule covers them all.
    pub tags: TagSet,
}

impl StoredAnycastIpList {
    /// Apply the deployment transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, for the reason [`StoredKeyValueStore::settle`] is: it only
    /// ever moves a non-terminal status to the terminal one, and only once the deadline has
    /// passed, so a read that calls it cannot change what a later caller sees. An IPAM CIDR
    /// configuration is provisioned as part of that deployment, so its own status settles
    /// on the same deadline.
    pub fn settle(&mut self, now: SystemTime) {
        if now < self.deployed_due {
            return;
        }
        if self.status == ANYCAST_IP_LIST_STATUS_DEPLOYING {
            self.status = STATUS_DEPLOYED.to_string();
        }
        for config in &mut self.ipam_cidr_configs {
            if config.status.as_deref() == Some(IPAM_CIDR_STATUS_PROVISIONING) {
                config.status = Some(IPAM_CIDR_STATUS_PROVISIONED.to_string());
            }
        }
    }

    /// `AnycastIpList$IpamConfig` — "The IPAM configuration for the Anycast static IP
    /// list, that contains the quantity and list of IPAM CIDR configurations."
    ///
    /// `None` for a list created without any: the member is optional, so a list that has no
    /// IPAM configuration reports none rather than a wrapper stating a quantity of zero.
    /// `IpamConfig$Quantity` is "The number of IPAM CIDR configurations in the
    /// `IpamCidrConfigs` list", so it is read off that list rather than stored beside it.
    pub fn to_ipam_config(&self) -> Option<IpamConfig> {
        if self.ipam_cidr_configs.is_empty() {
            return None;
        }
        Some(IpamConfig {
            quantity: i32::try_from(self.ipam_cidr_configs.len()).unwrap_or(i32::MAX),
            ipam_cidr_configs: self.ipam_cidr_configs.clone(),
        })
    }

    /// The `AnycastIpList` projection every Anycast static IP list operation returns, read
    /// from the one stored record.
    pub fn to_anycast_ip_list(&self) -> AnycastIpList {
        AnycastIpList {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            arn: self.arn.clone(),
            ip_address_type: Some(self.ip_address_type.clone()),
            ipam_config: self.to_ipam_config(),
            anycast_ips: anycast_ips_of(&self.anycast_ip_hosts, &self.ip_address_type),
            ip_count: self.ip_count,
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
        }
    }

    /// The `AnycastIpListSummary` projection `ListAnycastIpLists` serves, read from the one
    /// stored record: "an abbreviated version of the `AnycastIpList` structure. Omits the
    /// allocated static IP addresses (`AnycastIpList$AnycastIps`)." It is the only
    /// projection that carries the version token, as `AnycastIpListSummary$ETag` — "the
    /// current version (ETag value) of the Anycast static IP list" — which the full
    /// structure reports in the `ETag` header instead.
    pub fn to_summary(&self) -> AnycastIpListSummary {
        AnycastIpListSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            arn: self.arn.clone(),
            ip_count: self.ip_count,
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            ip_address_type: Some(self.ip_address_type.clone()),
            e_tag: Some(self.etag.clone()),
            ipam_config: self.to_ipam_config(),
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

/// One CloudFront connection group, as this emulator persists it.
///
/// A connection group is what a distribution tenant's viewers are routed through: it
/// holds the routing endpoint (the DNS name) CloudFront assigned to it, whether it is
/// enabled, whether IPv6 is on, and whether it is the account's automatically created
/// DEFAULT group. The record is the single source of truth for every one of them, so the
/// create, a later read and a later listing all answer from it rather than from the
/// request being served.
#[derive(Debug, Clone)]
pub struct StoredConnectionGroup {
    /// `ConnectionGroup$Id`, minted at creation and never reused. Also the key this
    /// record is held under, and the resume point a listing's `Marker` names.
    pub id: String,
    /// `ConnectionGroup$Name` — "Enter a friendly identifier that is unique within your
    /// Amazon Web Services account. This name can't be updated after you create the
    /// connection group."
    pub name: String,
    /// `ConnectionGroup$Arn`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `ConnectionGroup$RoutingEndpoint` — "The routing endpoint (also known as the DNS
    /// name) that is assigned to the connection group, such as
    /// `d111111abcdef8.cloudfront.net`": assigned by the service at creation, so it is
    /// minted here and never derived from the request.
    pub routing_endpoint: String,
    /// `ConnectionGroup$AnycastIpListId`, exactly as submitted — and only ever an id the
    /// create resolved to an Anycast static IP list this service holds. It is also what
    /// `ListConnectionGroups`'s `AssociationFilter` filters on.
    pub anycast_ip_list_id: Option<String>,
    /// `ConnectionGroup$Ipv6Enabled` — whether IPv6 is enabled for the group.
    pub ipv6_enabled: bool,
    /// `ConnectionGroup$Enabled` — whether the connection group is enabled.
    pub enabled: bool,
    /// `ConnectionGroup$IsDefault` — "Whether the connection group is the default
    /// connection group for the distribution tenants." True of the ONE group this
    /// service creates by itself, when a tenant that named none needed one.
    pub is_default: bool,
    /// [`STATUS_IN_PROGRESS`] until [`StoredConnectionGroup::deployed_due`], then
    /// [`STATUS_DEPLOYED`]: a connection group's routing endpoint reaches the edge
    /// locations the same way a distribution's configuration does.
    pub status: String,
    /// The instant this group is owed [`STATUS_DEPLOYED`]. Stored rather than slept for.
    pub deployed_due: SystemTime,
    /// `ConnectionGroup$CreatedTime`.
    pub created_time: SystemTime,
    /// `ConnectionGroup$LastModifiedTime`.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this group.
    pub etag: String,
    /// The tags submitted with the create, in the one representation every tagged
    /// resource in this store uses.
    pub tags: TagSet,
}

impl StoredConnectionGroup {
    /// Apply the propagation transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, for the same reason [`StoredDistribution::settle`] is:
    /// it only ever moves the non-terminal status to the terminal one, and only once the
    /// deadline has passed, so a read that calls it cannot change what a later caller
    /// sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_IN_PROGRESS && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `ConnectionGroup` projection every single-group operation returns, read from
    /// the one stored record.
    pub fn to_connection_group(&self) -> ConnectionGroup {
        ConnectionGroup {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(
                self.last_modified_time,
            )),
            tags: Some(self.to_tags()),
            ipv6_enabled: Some(self.ipv6_enabled),
            routing_endpoint: Some(self.routing_endpoint.clone()),
            anycast_ip_list_id: self.anycast_ip_list_id.clone(),
            status: Some(self.status.clone()),
            enabled: Some(self.enabled),
            is_default: Some(self.is_default),
        }
    }

    /// The `ConnectionGroupSummary` a listing reports, read from the same stored record
    /// the single-group projection is read from — so a listing and a read can never
    /// disagree about one group.
    pub fn to_summary(&self) -> ConnectionGroupSummary {
        ConnectionGroupSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            routing_endpoint: self.routing_endpoint.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            e_tag: self.etag.clone(),
            anycast_ip_list_id: self.anycast_ip_list_id.clone(),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
            is_default: Some(self.is_default),
        }
    }
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
    /// `DistributionTenant$Name` — "unique within your Amazon Web Services account. This
    /// name can't be updated after you create the distribution tenant."
    pub name: String,
    /// `DistributionTenant$DistributionId` — the multi-tenant distribution this tenant
    /// was created under.
    pub distribution_id: String,
    /// The domains associated with this tenant, in the order the create listed them. Each
    /// is reported as a `DomainResult` in [`DOMAIN_STATUS_ACTIVE`].
    pub domains: Vec<String>,
    /// `DistributionTenant$ConnectionGroupId` — the connection group the tenant's viewers
    /// are routed through: the one the create named, or the account's default one.
    pub connection_group_id: String,
    /// `DistributionTenant$Customizations`, exactly as submitted: these are "specific
    /// values that you can override or disable from the multi-tenant distribution", so the
    /// service supplies nothing of its own here.
    pub customizations: Option<Customizations>,
    /// `DistributionTenant$Parameters`, exactly as submitted — the values that fill in the
    /// distribution's `TenantConfig$ParameterDefinitions`.
    pub parameters: Vec<Parameter>,
    /// The CloudFront managed ACM certificate requested for this tenant, or `None` when
    /// the create asked for none. It is MINTED by the call that carried the
    /// `ManagedCertificateRequest` and is the single source of truth every later read of
    /// the certificate surface answers from.
    pub managed_certificate: Option<StoredManagedCertificate>,
    /// The tags submitted with the create, in the one representation every tagged
    /// resource in this store uses.
    pub tags: TagSet,
    /// `DistributionTenant$Enabled` — whether the tenant serves traffic.
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
    /// Every invalidation created under this distribution tenant, keyed by invalidation id.
    ///
    /// A tenant's invalidations are addressed under the TENANT
    /// (`/distribution-tenant/{Id}/invalidation/...`), so they are held on its record
    /// rather than on the multi-tenant distribution's: a create, a read and a list of one
    /// tenant's invalidations all answer from here, and no other tenant's listing reaches
    /// them.
    pub invalidations: BTreeMap<String, StoredInvalidation>,
}

impl StoredDistributionTenant {
    /// Apply every transition the clock owes this tenant at `now`: its own propagation, and
    /// the completion of each invalidation batch created under it.
    ///
    /// Idempotent and monotonic, for the same reason
    /// [`StoredDistribution::settle`] is: it only ever moves the non-terminal status to the
    /// terminal one, and only once the deadline has passed, so a read that calls it cannot
    /// change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_IN_PROGRESS && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
        if let Some(certificate) = &mut self.managed_certificate {
            certificate.settle(now);
        }
        settle_invalidations(&mut self.invalidations, now);
    }

    /// Whether this tenant holds `domain`, compared as DNS names are.
    pub fn holds_domain(&self, domain: &str) -> bool {
        self.domains.iter().any(|held| same_domain(held, domain))
    }

    /// Whether `domain` is still waiting for this tenant's CloudFront managed certificate to
    /// be validated.
    ///
    /// A managed certificate is minted [`CERTIFICATE_STATUS_PENDING_VALIDATION`] over the
    /// domains it covers (one validation token each), and until it is issued CloudFront
    /// cannot serve those domains — so a domain it covers is not yet reachable at the edge.
    /// A tenant that asked for no managed certificate waits for nothing, and neither does a
    /// domain the certificate does not cover.
    pub fn domain_pending_validation(&self, domain: &str) -> bool {
        self.managed_certificate.as_ref().is_some_and(|certificate| {
            certificate.status == CERTIFICATE_STATUS_PENDING_VALIDATION
                && certificate
                    .validation_tokens
                    .iter()
                    .any(|token| same_domain(&token.domain, domain))
        })
    }

    /// Whether a viewer asking for `domain` is routed to this tenant: the tenant holds the
    /// domain AND the domain is not still waiting for a managed certificate
    /// ([`Self::domain_pending_validation`]).
    ///
    /// This is also what makes the tenant's claim on the domain an ENFORCED one
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]): a domain nothing
    /// routes yet is a domain no route collides with.
    pub fn serves_domain(&self, domain: &str) -> bool {
        self.holds_domain(domain) && !self.domain_pending_validation(domain)
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `DomainResult` list every projection of this tenant reports: the domains it
    /// holds, in the order they were associated, each in [`DOMAIN_STATUS_ACTIVE`].
    ///
    /// ONE mapping, so the full document and a listing's summary cannot drift apart about a
    /// tenant's domains.
    fn to_domain_results(&self) -> Vec<DomainResult> {
        self.domains
            .iter()
            .map(|domain| DomainResult {
                domain: domain.clone(),
                status: Some(DOMAIN_STATUS_ACTIVE.to_string()),
            })
            .collect()
    }

    /// The `DistributionTenant` projection every distribution tenant operation returns,
    /// read from the one stored record.
    pub fn to_distribution_tenant(&self) -> DistributionTenant {
        DistributionTenant {
            id: Some(self.id.clone()),
            distribution_id: Some(self.distribution_id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            domains: Some(self.to_domain_results()),
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

    /// The `DistributionTenantSummary` projection every LISTING of distribution tenants
    /// reports, read from the same stored record [`Self::to_distribution_tenant`] reads, so
    /// a listing and a single-tenant read can never disagree about one tenant.
    ///
    /// The summary carries the tenant's current version (`DistributionTenantSummary$ETag`),
    /// which the full projection reports beside the document rather than inside it; every
    /// other member is the same value under the same name. A summary carries no
    /// `Parameters`, so the parameter values a tenant holds are reached through a read of
    /// the tenant itself.
    pub fn to_summary(&self) -> DistributionTenantSummary {
        DistributionTenantSummary {
            id: self.id.clone(),
            distribution_id: self.distribution_id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            domains: self.to_domain_results(),
            connection_group_id: Some(self.connection_group_id.clone()),
            customizations: self.customizations.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            e_tag: self.etag.clone(),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
        }
    }

    /// The ARN of the ACM certificate this tenant's customizations name, or `None` when its
    /// customizations name none.
    ///
    /// `Customizations$Certificate` is "the Certificate Manager (ACM) certificate" the
    /// tenant overrides its distribution's with, so the ARN a tenant is associated with is
    /// the one in that member of its stored record — never one derived from anything else.
    pub fn customization_certificate_arn(&self) -> Option<&str> {
        self.customizations
            .as_ref()?
            .certificate
            .as_ref()
            .map(|certificate| certificate.arn.as_str())
    }

    /// Whether a valid TLS certificate attached to this tenant covers `domain`.
    ///
    /// Two certificates can be attached to a distribution tenant: the ACM certificate its
    /// customizations name — "the Certificate Manager (ACM) certificate" it overrides its
    /// distribution's with — and the CloudFront managed certificate it requested. A managed
    /// certificate is this service's OWN record, so both its status and the names it covers
    /// are read from it ([`StoredManagedCertificate::covers_domain`]): one that is still
    /// waiting for its domains to be validated secures nothing. A certificate named by ARN
    /// is a resource of another service, whose covered names this service holds no record
    /// of, so an attached one is taken at its word for the domain asked about — the same
    /// reading [`names_custom_certificate`] gives a distribution's viewer certificate.
    pub fn certificate_covers_domain(&self, domain: &str) -> bool {
        self.customization_certificate_arn().is_some()
            || self
                .managed_certificate
                .as_ref()
                .is_some_and(|certificate| certificate.covers_domain(domain))
    }

    /// The ARN of the WAF web ACL this tenant's customizations name, or `None` when its
    /// customizations name none — which includes a customization that DISABLES the web ACL
    /// it inherits, since disabling names no ARN.
    pub fn customization_web_acl_arn(&self) -> Option<&str> {
        self.customizations
            .as_ref()?
            .web_acl
            .as_ref()?
            .arn
            .as_deref()
    }
}

/// What an `UpdateDistributionTenant` changes about one stored distribution tenant.
///
/// Every member mirrors the one the update submits, and each is OPTIONAL in the same sense:
/// `None` is "the request left this member out", which leaves the stored value exactly as
/// it was, and `Some` is the value the tenant is updated to. The struct is what carries the
/// resolved change into
/// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_distribution_tenant_update`], so
/// the rule for which members a partial update touches is stated in ONE place rather than
/// once per member at the call site.
#[derive(Debug, Default)]
pub struct DistributionTenantUpdate {
    /// `DistributionId` — "The ID for the multi-tenant distribution."
    pub distribution_id: Option<String>,
    /// The domain names from `Domains` — "The domains to update for the distribution
    /// tenant."
    pub domains: Option<Vec<String>>,
    /// `Customizations` — the tenant's overrides of its distribution's settings.
    pub customizations: Option<Customizations>,
    /// `Parameters` — the values that fill in the distribution's
    /// `TenantConfig$ParameterDefinitions`.
    pub parameters: Option<Vec<Parameter>>,
    /// `ConnectionGroupId` — "The ID of the target connection group."
    pub connection_group_id: Option<String>,
    /// `Enabled` — "whether the distribution tenant should be updated to an enabled state".
    pub enabled: Option<bool>,
    /// `ManagedCertificateRequest` — the CloudFront managed ACM certificate the update asks
    /// for, which MINTS one over the domains the tenant holds after the update.
    pub managed_certificate_request: Option<ManagedCertificateRequest>,
}

/// The customization set a tenant's record retains, or `None` when the set names nothing at
/// all.
///
/// A tenant's customizations are "specific values that you can override or disable from the
/// multi-tenant distribution", so a set in which every one of them is absent describes no
/// customization: it is retained as none rather than as an element describing nothing. ONE
/// rule for every write that stores a tenant's customizations.
fn retained_customizations(customizations: Customizations) -> Option<Customizations> {
    (customizations.web_acl.is_some()
        || customizations.certificate.is_some()
        || customizations.geo_restrictions.is_some())
    .then_some(customizations)
}

/// One CloudFront managed ACM certificate, as this emulator persists it.
///
/// The certificate is MINTED by the call that carries a `ManagedCertificateRequest` for a
/// distribution tenant — nothing a read does brings one into being — and this record is
/// the single source of truth for its ARN, its status, the validation-token host that was
/// asked for and the validation token of each domain it covers, so every later read of
/// the certificate answers from it rather than from the request being served.
///
/// Its status is the one thing here the CLOCK owes: a certificate that has just been
/// requested is [`CERTIFICATE_STATUS_PENDING_VALIDATION`] until its domains have been
/// validated ([`CERTIFICATE_VALIDATION_DELAY`]), and [`CERTIFICATE_STATUS_ISSUED`] after.
#[derive(Debug, Clone)]
pub struct StoredManagedCertificate {
    /// `ManagedCertificateDetails$CertificateArn` — the ARN minted for the certificate at
    /// the moment it was requested, fixed for its whole life.
    pub arn: String,
    /// `ManagedCertificateDetails$CertificateStatus`:
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] until
    /// [`StoredManagedCertificate::issued_due`], then [`CERTIFICATE_STATUS_ISSUED`].
    pub status: String,
    /// The instant this certificate is owed its issuance. Stored rather than slept for.
    pub issued_due: SystemTime,
    /// The request the certificate was minted from, held because what it asked for is
    /// what the certificate reports: its `ValidationTokenHost` is
    /// `ManagedCertificateDetails$ValidationTokenHost`.
    pub request: ManagedCertificateRequest,
    /// One validation token per domain the certificate covers, in the order the request
    /// that minted it listed them — "Contains details about the validation token of the
    /// specified CloudFront managed ACM certificate."
    pub validation_tokens: Vec<StoredValidationToken>,
}

impl StoredManagedCertificate {
    /// Apply the transition the clock owes this certificate at `now`.
    ///
    /// Idempotent and monotonic for the same reason every other `settle` here is: it only
    /// ever moves the non-terminal status to the terminal one, and only once the deadline
    /// has passed, so a read that calls it cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == CERTIFICATE_STATUS_PENDING_VALIDATION && now >= self.issued_due {
            self.status = CERTIFICATE_STATUS_ISSUED.to_string();
        }
    }

    /// Whether this certificate is one a viewer connection can be served over NOW, and
    /// covers `domain`.
    ///
    /// A certificate secures a domain only once it has been ISSUED: while it is
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] the domains it was requested for have not
    /// been validated, so it covers none of them yet. Issued, it covers the names it was
    /// minted over — one validation token each — and every name a wildcard among them
    /// covers ([`certificate_covers`]).
    pub fn covers_domain(&self, domain: &str) -> bool {
        self.status == CERTIFICATE_STATUS_ISSUED
            && certificate_covers(
                self.validation_tokens
                    .iter()
                    .map(|token| token.domain.as_str()),
                domain,
            )
    }

    /// The `ManagedCertificateDetails` projection the certificate surface returns, read
    /// from the one stored record.
    pub fn to_managed_certificate_details(&self) -> ManagedCertificateDetails {
        ManagedCertificateDetails {
            certificate_arn: Some(self.arn.clone()),
            certificate_status: Some(self.status.clone()),
            // "Specify how the HTTP validation token will be served when requesting the
            // CloudFront managed ACM certificate": the mode the request chose is the mode
            // the certificate is validated in, so it is reported back unchanged.
            validation_token_host: Some(self.request.validation_token_host.clone()),
            validation_token_details: Some(
                self.validation_tokens
                    .iter()
                    .map(StoredValidationToken::to_detail)
                    .collect(),
            ),
        }
    }
}

/// The HTTP validation token of ONE domain of a managed certificate.
///
/// A token is minted per domain with the certificate, so the pair of locations a caller
/// serving the token itself has to redirect between is a property of the stored record
/// rather than of the call that reads it.
#[derive(Debug, Clone)]
pub struct StoredValidationToken {
    /// `ValidationTokenDetail$Domain` — the domain this token validates.
    pub domain: String,
    /// The token's own opaque name, the last segment of both locations below.
    pub token: String,
}

impl StoredValidationToken {
    /// The `ValidationTokenDetail` a certificate read reports for this token.
    pub fn to_detail(&self) -> ValidationTokenDetail {
        ValidationTokenDetail {
            domain: self.domain.clone(),
            // "The domain to redirect from" is where a viewer asks for the token — the
            // well-known path under the domain being validated; "The domain to redirect
            // to" is where the token's content is served from.
            redirect_from: Some(validation_token_location(&self.domain, &self.token)),
            redirect_to: Some(validation_token_location(
                &format!("{}.{VALIDATION_HOST_SUFFIX}", self.token),
                &self.token,
            )),
        }
    }
}

/// Where the HTTP validation token `token` is served under `host`: the well-known
/// validation path under the host, ending in the token's own file name.
pub fn validation_token_location(host: &str, token: &str) -> String {
    format!("{host}{VALIDATION_TOKEN_PATH}{token}.txt")
}

/// The ARN of a CloudFront managed ACM certificate. The certificate is an ACM resource in
/// the one region CloudFront serves viewers over certificates from
/// ([`CERTIFICATE_REGION`]): `arn:aws:acm:us-east-1:<account>:certificate/<id>`.
pub fn managed_certificate_arn(id: &str) -> String {
    ::engine::emulator_arn("acm", CERTIFICATE_REGION, &format!("certificate/{id}"))
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

    /// The `CloudFrontOriginAccessIdentitySummary` one `ListCloudFrontOriginAccessIdentities`
    /// page element carries, read from the one stored record.
    ///
    /// Every member of the summary is required on the wire, and each is read from this
    /// record: the minted id, the canonical user id minted with it, and "the comment for
    /// this origin access identity, as originally specified when created" — the comment
    /// of the stored configuration rather than anything re-derived per listing.
    pub fn to_summary(&self) -> CloudFrontOriginAccessIdentitySummary {
        CloudFrontOriginAccessIdentitySummary {
            id: self.id.clone(),
            s3_canonical_user_id: self.s3_canonical_user_id.clone(),
            comment: self.config.comment.clone(),
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

/// One CloudFront origin access control, as this emulator persists it.
///
/// The record is the single source of truth for the control's id, the configuration it
/// was created with and its version token, so the create, a later
/// `GetOriginAccessControl` and every `ListOriginAccessControls` page answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `OriginAccessControl` is `Id` and
/// `OriginAccessControlConfig` only — a control carries no status and no propagation
/// deadline, so a control this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredOriginAccessControl {
    /// `OriginAccessControl$Id` — "The unique identifier of the origin access control":
    /// the token minted at creation, which is also the key this record is held under and
    /// the path segment its `Location` names.
    pub id: String,
    /// The configuration as submitted — stored whole, so a read reports what was stored
    /// rather than re-deriving anything from a request.
    pub config: OriginAccessControlConfig,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this control.
    pub etag: String,
}

impl StoredOriginAccessControl {
    /// The `OriginAccessControl` projection every origin access control operation
    /// returns, read from the one stored record.
    pub fn to_origin_access_control(&self) -> OriginAccessControl {
        OriginAccessControl {
            id: self.id.clone(),
            origin_access_control_config: Some(self.config.clone()),
        }
    }

    /// The `OriginAccessControlSummary` a listing reports for this control.
    ///
    /// `OriginAccessControlSummary$Description` is REQUIRED while
    /// `OriginAccessControlConfig$Description` is optional, so a control created without
    /// a description is summarised with an empty one rather than with no element.
    pub fn to_summary(&self) -> OriginAccessControlSummary {
        OriginAccessControlSummary {
            id: self.id.clone(),
            description: self.config.description.clone().unwrap_or_default(),
            name: self.config.name.clone(),
            signing_protocol: self.config.signing_protocol.clone(),
            signing_behavior: self.config.signing_behavior.clone(),
            origin_access_control_origin_type: self
                .config
                .origin_access_control_origin_type
                .clone(),
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

/// The `CachePolicyType` a policy this ACCOUNT created carries — "custom – Returns only
/// the custom policies created in your Amazon Web Services account". One of the two values
/// the model's `CachePolicyType` enum admits (`CACHE_POLICY_TYPE_VALUES`).
pub const CACHE_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `CachePolicyType` a policy AWS created carries — "managed – Returns only the managed
/// policies created by Amazon Web Services". The other value the model's `CachePolicyType`
/// enum admits.
pub const CACHE_POLICY_TYPE_MANAGED: &str = "managed";

/// One entry of the catalog of AWS-MANAGED cache policies.
///
/// A managed cache policy is created by AWS rather than by this account: every account has
/// the same ones, under the identifier and name AWS publishes for each, which is what
/// `ListCachePolicies`'s filter separates from the account's own policies ("managed –
/// Returns only the managed policies created by Amazon Web Services"). Nothing a caller
/// does creates, changes or removes one, so the catalog is STATIC data read beside the
/// mutable store rather than a slice of it — it consumes none of the account's cache policy
/// allowance and no call can take an entry out of it.
///
/// The entries in [`MANAGED_CACHE_POLICIES`] are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed cache policies"), which lists each
/// policy's name, its identifier and its cache key and TTL settings; the model itself
/// carries no list of them (`smithy_query(query="shape",shape="CachePolicyType")` admits
/// only the two type values), so the catalog is written out here.
#[derive(Debug, Clone)]
pub struct ManagedCachePolicy {
    /// `CachePolicy$Id` — the identifier AWS publishes for this policy, the same in every
    /// account.
    pub id: &'static str,
    /// `CachePolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `CachePolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// `CachePolicyConfig$MinTTL`, in seconds.
    pub min_ttl: i64,
    /// `CachePolicyConfig$DefaultTTL`, in seconds.
    pub default_ttl: i64,
    /// `CachePolicyConfig$MaxTTL`, in seconds.
    pub max_ttl: i64,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingGzip`.
    pub gzip: bool,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli`.
    pub brotli: bool,
    /// A value of `CachePolicyHeaderBehavior`.
    pub header_behavior: &'static str,
    /// The headers the cache key includes, when the behaviour names a list.
    pub headers: &'static [&'static str],
    /// A value of `CachePolicyCookieBehavior`.
    pub cookie_behavior: &'static str,
    /// The cookies the cache key includes, when the behaviour names a list.
    pub cookies: &'static [&'static str],
    /// A value of `CachePolicyQueryStringBehavior`.
    pub query_string_behavior: &'static str,
    /// The query strings the cache key includes, when the behaviour names a list.
    pub query_strings: &'static [&'static str],
}

impl ManagedCachePolicy {
    /// The `CachePolicy` projection a read of this managed policy reports: its published
    /// identity beside the configuration AWS gave it.
    pub fn to_cache_policy(&self) -> CachePolicy {
        self.to_stored().to_cache_policy()
    }

    /// This catalog entry as a cache policy RECORD, so every read of a cache policy answers
    /// a published policy exactly as it answers one this account created — the two are the
    /// same resource kind, differing only in who created them.
    pub fn to_stored(&self) -> StoredCachePolicy {
        StoredCachePolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this managed policy reports, derived from its published
    /// id by the one rule every published policy's version follows
    /// ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> CachePolicyConfig {
        CachePolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            default_ttl: Some(self.default_ttl),
            max_ttl: Some(self.max_ttl),
            min_ttl: self.min_ttl,
            parameters_in_cache_key_and_forwarded_to_origin: Some(
                crate::ops::types::ParametersInCacheKeyAndForwardedToOrigin {
                    enable_accept_encoding_gzip: self.gzip,
                    enable_accept_encoding_brotli: Some(self.brotli),
                    headers_config: crate::ops::types::CachePolicyHeadersConfig {
                        header_behavior: self.header_behavior.to_string(),
                        headers: name_list(self.headers)
                            .map(|(quantity, items)| crate::ops::types::Headers { quantity, items }),
                    },
                    cookies_config: crate::ops::types::CachePolicyCookiesConfig {
                        cookie_behavior: self.cookie_behavior.to_string(),
                        cookies: name_list(self.cookies).map(|(quantity, items)| {
                            crate::ops::types::CookieNames { quantity, items }
                        }),
                    },
                    query_strings_config: crate::ops::types::CachePolicyQueryStringsConfig {
                        query_string_behavior: self.query_string_behavior.to_string(),
                        query_strings: name_list(self.query_strings).map(|(quantity, items)| {
                            crate::ops::types::QueryStringNames { quantity, items }
                        }),
                    },
                },
            ),
        }
    }
}

/// The `(Quantity, Items)` wrapper a name list travels in, or `None` when the behaviour
/// names no list at all — a behaviour of `none`, `all` or `allExcept` nothing is listed
/// under carries no wrapper rather than an empty one.
fn name_list(names: &'static [&'static str]) -> Option<(i32, Option<Vec<String>>)> {
    if names.is_empty() {
        return None;
    }
    Some((
        i32::try_from(names.len()).unwrap_or(i32::MAX),
        Some(names.iter().map(|name| (*name).to_string()).collect()),
    ))
}

/// The instant every policy AWS published reports as its `LastModifiedTime`.
///
/// A published policy belongs to AWS and no call in this account changes one, so the
/// instant is FIXED rather than drawn from the clock: every read of a published catalog, in
/// any process, reports the same one. It is this service's own API version date
/// (`2020-05-31T00:00:00Z`), the earliest instant any policy of this API could have been
/// published at.
pub fn managed_policy_last_modified() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_POLICY_LAST_MODIFIED_UNIX_SECONDS)
}

/// The version token a read of a policy AWS published reports.
///
/// DERIVED from the published id rather than minted: nothing changes a published policy, so
/// its version is the same on every read and in every process, which a drawn token could
/// not be. It takes the shape every version token of this service takes
/// ([`is_version_token`]): `E` followed by [`ID_BODY_LEN`] characters of the id alphabet,
/// here the leading hex digits of the id.
pub fn managed_policy_etag(id: &str) -> String {
    let body: String = id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|digit| digit.to_ascii_uppercase())
        .take(ID_BODY_LEN)
        .collect();
    format!("E{body}")
}

/// `2020-05-31T00:00:00Z` in seconds since the Unix epoch.
const MANAGED_POLICY_LAST_MODIFIED_UNIX_SECONDS: u64 = 1_590_883_200;

/// The AWS-managed cache policies, in the order their ids sort in — the same total order
/// every listing of this service pages through, so the catalog can be merged with the
/// account's own policies without re-sorting it.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it (see
/// [`ManagedCachePolicy`] for the source).
pub const MANAGED_CACHE_POLICIES: &[ManagedCachePolicy] = &[
    ManagedCachePolicy {
        id: "08627262-05a9-4f76-9ded-b50ca2e3a84f",
        name: "Managed-Elemental-MediaPackage",
        comment: "Policy for an AWS Elemental MediaPackage origin",
        min_ttl: 0,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: false,
        header_behavior: "whitelist",
        headers: &["Origin"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "whitelist",
        query_strings: &["aws.manifestfilter", "start", "end", "m"],
    },
    ManagedCachePolicy {
        id: "2e54312d-136d-493c-8eb9-b001f22f67d2",
        name: "Managed-Amplify",
        comment: "Policy for an AWS Amplify web application origin",
        min_ttl: 2,
        default_ttl: 2,
        max_ttl: 600,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &["Authorization", "Host"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
        name: "Managed-CachingDisabled",
        comment: "Policy with caching disabled",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 0,
        gzip: false,
        brotli: false,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "4cc15a8a-d715-48a4-82b8-cc0b614638fe",
        name: "Managed-UseOriginCacheControlHeaders-QueryStrings",
        comment: "Policy for an origin that returns Cache-Control headers and serves \
                  different objects for different query strings",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "658327ea-f89d-4fab-a63d-7e88639e58f6",
        name: "Managed-CachingOptimized",
        comment: "Policy with caching enabled. Supports Gzip and Brotli compression.",
        min_ttl: 1,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "83da9c7e-98b4-4e11-a168-04f0df8e2c65",
        name: "Managed-UseOriginCacheControlHeaders",
        comment: "Policy for an origin that returns Cache-Control headers",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "b2884449-e4de-46a7-ac36-70bc7f1ddd6d",
        name: "Managed-CachingOptimizedForUncompressedObjects",
        comment: "Policy with caching enabled that does not support compression",
        min_ttl: 1,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: false,
        brotli: false,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
];

/// The managed cache policy `id` names, or `None` when the catalog holds none — what tells
/// an id AWS published from one this account minted.
pub fn managed_cache_policy(id: &str) -> Option<&'static ManagedCachePolicy> {
    MANAGED_CACHE_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// The `OriginRequestPolicyType` a policy this ACCOUNT created carries — "custom – Returns
/// only the custom policies created in your Amazon Web Services account". One of the two
/// values the model's `OriginRequestPolicyType` enum admits.
pub const ORIGIN_REQUEST_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `OriginRequestPolicyType` a policy AWS created carries — "managed – Returns only the
/// managed policies created by Amazon Web Services". The other value the same enum admits.
pub const ORIGIN_REQUEST_POLICY_TYPE_MANAGED: &str = "managed";

/// One entry of the catalog of AWS-MANAGED origin request policies.
///
/// The same reference data [`ManagedCachePolicy`] holds for the cache policy family, for
/// the origin request policy family: AWS creates these policies, every account has the same
/// ones under the identifier and name AWS publishes for each, and nothing a caller does
/// here creates, changes or removes one — so the catalog is STATIC data read beside the
/// mutable store rather than a slice of it.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed origin request policies"); the model
/// itself carries no list of them (`smithy_query(query="shape",
/// shape="OriginRequestPolicyType")` admits only the two type values), so the catalog is
/// written out here.
#[derive(Debug, Clone)]
pub struct ManagedOriginRequestPolicy {
    /// `OriginRequestPolicy$Id` — the identifier AWS publishes for this policy, the same in
    /// every account.
    pub id: &'static str,
    /// `OriginRequestPolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `OriginRequestPolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// A value of `OriginRequestPolicyHeaderBehavior`.
    pub header_behavior: &'static str,
    /// The headers the behaviour names, when it names a list.
    pub headers: &'static [&'static str],
    /// A value of `OriginRequestPolicyCookieBehavior`.
    pub cookie_behavior: &'static str,
    /// The cookies the behaviour names, when it names a list.
    pub cookies: &'static [&'static str],
    /// A value of `OriginRequestPolicyQueryStringBehavior`.
    pub query_string_behavior: &'static str,
    /// The query strings the behaviour names, when it names a list.
    pub query_strings: &'static [&'static str],
}

impl ManagedOriginRequestPolicy {
    /// The `OriginRequestPolicy` projection a read of this published policy reports: its
    /// published identity beside the configuration AWS gave it.
    pub fn to_origin_request_policy(&self) -> OriginRequestPolicy {
        self.to_stored().to_origin_request_policy()
    }

    /// This catalog entry as an origin request policy RECORD, so every read of an origin
    /// request policy answers a published policy exactly as it answers one this account
    /// created — the two are the same resource kind, differing only in who created them.
    pub fn to_stored(&self) -> StoredOriginRequestPolicy {
        StoredOriginRequestPolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this published policy reports, by the one rule every
    /// published policy's version follows ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> OriginRequestPolicyConfig {
        OriginRequestPolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            headers_config: crate::ops::types::OriginRequestPolicyHeadersConfig {
                header_behavior: self.header_behavior.to_string(),
                headers: name_list(self.headers)
                    .map(|(quantity, items)| crate::ops::types::Headers { quantity, items }),
            },
            cookies_config: crate::ops::types::OriginRequestPolicyCookiesConfig {
                cookie_behavior: self.cookie_behavior.to_string(),
                cookies: name_list(self.cookies)
                    .map(|(quantity, items)| crate::ops::types::CookieNames { quantity, items }),
            },
            query_strings_config: crate::ops::types::OriginRequestPolicyQueryStringsConfig {
                query_string_behavior: self.query_string_behavior.to_string(),
                query_strings: name_list(self.query_strings).map(|(quantity, items)| {
                    crate::ops::types::QueryStringNames { quantity, items }
                }),
            },
        }
    }
}

/// The AWS-managed origin request policies, in the order their ids sort in — the same total
/// order every listing of this service pages through, so the catalog can be merged with the
/// account's own policies without re-sorting it.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it (see
/// [`ManagedOriginRequestPolicy`] for the source).
pub const MANAGED_ORIGIN_REQUEST_POLICIES: &[ManagedOriginRequestPolicy] = &[
    ManagedOriginRequestPolicy {
        id: "216adef6-5c7f-47e4-b989-5492eafa07d3",
        name: "Managed-AllViewer",
        comment: "Policy to forward all parameters in viewer requests",
        header_behavior: "allViewer",
        headers: &[],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "33f36d7e-f396-46d9-90e0-52428a34d9dc",
        name: "Managed-AllViewerAndCloudFrontHeaders-2022-06",
        comment: "Policy to forward all parameters in viewer requests and all CloudFront \
                  headers added through 2022-06",
        header_behavior: "allViewerAndWhitelistCloudFront",
        headers: &[
            "CloudFront-Forwarded-Proto",
            "CloudFront-Is-Android-Viewer",
            "CloudFront-Is-Desktop-Viewer",
            "CloudFront-Is-IOS-Viewer",
            "CloudFront-Is-Mobile-Viewer",
            "CloudFront-Is-SmartTV-Viewer",
            "CloudFront-Is-Tablet-Viewer",
            "CloudFront-Viewer-ASN",
            "CloudFront-Viewer-Address",
            "CloudFront-Viewer-City",
            "CloudFront-Viewer-Country",
            "CloudFront-Viewer-Country-Name",
            "CloudFront-Viewer-Country-Region",
            "CloudFront-Viewer-Country-Region-Name",
            "CloudFront-Viewer-Http-Version",
            "CloudFront-Viewer-Latitude",
            "CloudFront-Viewer-Longitude",
            "CloudFront-Viewer-Metro-Code",
            "CloudFront-Viewer-Postal-Code",
            "CloudFront-Viewer-TLS",
            "CloudFront-Viewer-Time-Zone",
        ],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "59781a5b-3903-41f3-afcb-af62929ccde1",
        name: "Managed-CORS-CustomOrigin",
        comment: "Policy to forward the Origin header, for CORS requests to a custom origin",
        header_behavior: "whitelist",
        headers: &["Origin"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "775133bc-15f2-49f9-abea-afb2e0bf67d2",
        name: "Managed-Elemental-MediaTailor-PersonalizedManifests",
        comment: "Policy for an AWS Elemental MediaTailor origin serving personalized \
                  manifests",
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
            "User-Agent",
            "X-Forwarded-For",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "88a5eaf4-2fd4-4709-b370-b4c650ea3fcf",
        name: "Managed-CORS-S3Origin",
        comment: "Policy to forward the CORS request headers, for CORS requests to an \
                  Amazon S3 origin",
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "acba4595-bd28-49b8-b9fe-13317c0390fa",
        name: "Managed-UserAgentRefererHeaders",
        comment: "Policy to forward the User-Agent and Referer headers",
        header_behavior: "whitelist",
        headers: &["User-Agent", "Referer"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "b689b0a8-53d0-40ab-baf2-68738e2966ac",
        name: "Managed-AllViewerExceptHostHeader",
        comment: "Policy to forward all parameters in viewer requests except for the Host \
                  header",
        header_behavior: "allExcept",
        headers: &["host"],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
];

/// The managed origin request policy `id` names, or `None` when the catalog holds none —
/// what tells an id AWS published from one this account minted.
pub fn managed_origin_request_policy(id: &str) -> Option<&'static ManagedOriginRequestPolicy> {
    MANAGED_ORIGIN_REQUEST_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// The `ResponseHeadersPolicyType` a policy this ACCOUNT created carries — the custom
/// policies "created in your Amazon Web Services account". One of the two values the
/// model's `ResponseHeadersPolicyType` enum admits.
pub const RESPONSE_HEADERS_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `ResponseHeadersPolicyType` a policy AWS created carries — the managed policies
/// "created by Amazon Web Services". The other value the same enum admits.
pub const RESPONSE_HEADERS_POLICY_TYPE_MANAGED: &str = "managed";

/// The CORS settings a published response headers policy carries, when it carries any.
///
/// Every member here is one of `ResponseHeadersPolicyCorsConfig`'s; a policy that states no
/// CORS behaviour at all carries no `CorsConfig` element rather than an empty one.
#[derive(Debug, Clone)]
pub struct ManagedCorsSettings {
    /// `AccessControlAllowOrigins$Items` — the origins the policy allows.
    pub allow_origins: &'static [&'static str],
    /// `AccessControlAllowHeaders$Items` — the request headers the policy allows.
    pub allow_headers: &'static [&'static str],
    /// `AccessControlAllowMethods$Items` — the methods the policy allows, each a value of
    /// `ResponseHeadersPolicyAccessControlAllowMethodsValues`.
    pub allow_methods: &'static [&'static str],
    /// `AccessControlAllowCredentials`.
    pub allow_credentials: bool,
    /// `AccessControlMaxAgeSec`, in seconds, for a policy that answers preflight requests.
    pub max_age_seconds: Option<i32>,
    /// `OriginOverride` — whether CloudFront overrides a header the origin already set.
    pub origin_override: bool,
}

/// One entry of the catalog of AWS-MANAGED response headers policies.
///
/// The same reference data [`ManagedCachePolicy`] holds for the cache policy family, for
/// the response headers policy family: AWS creates these policies, every account has the
/// same ones under the identifier and name AWS publishes for each, and nothing a caller
/// does here creates, changes or removes one.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed response headers policies").
#[derive(Debug, Clone)]
pub struct ManagedResponseHeadersPolicy {
    /// `ResponseHeadersPolicy$Id` — the identifier AWS publishes for this policy, the same
    /// in every account.
    pub id: &'static str,
    /// `ResponseHeadersPolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `ResponseHeadersPolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// The policy's `CorsConfig`, or `None` for a policy that states no CORS behaviour.
    pub cors: Option<ManagedCorsSettings>,
    /// Whether the policy carries the published security headers
    /// ([`managed_security_headers_config`]). The published policies that add security
    /// headers all add the SAME set, so the set is one constant here rather than repeated
    /// per entry.
    pub security_headers: bool,
}

impl ManagedResponseHeadersPolicy {
    /// The `ResponseHeadersPolicy` projection a read of this published policy reports.
    pub fn to_response_headers_policy(&self) -> ResponseHeadersPolicy {
        self.to_stored().to_response_headers_policy()
    }

    /// This catalog entry as a response headers policy RECORD, so every read of a response
    /// headers policy answers a published policy exactly as it answers one this account
    /// created.
    pub fn to_stored(&self) -> StoredResponseHeadersPolicy {
        StoredResponseHeadersPolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this published policy reports, by the one rule every
    /// published policy's version follows ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> ResponseHeadersPolicyConfig {
        ResponseHeadersPolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            cors_config: self.cors.as_ref().map(|cors| {
                crate::ops::types::ResponseHeadersPolicyCorsConfig {
                    access_control_allow_origins:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowOrigins {
                            quantity: i32::try_from(cors.allow_origins.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_origins
                                .iter()
                                .map(|origin| (*origin).to_string())
                                .collect(),
                        },
                    access_control_allow_headers:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowHeaders {
                            quantity: i32::try_from(cors.allow_headers.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_headers
                                .iter()
                                .map(|header| (*header).to_string())
                                .collect(),
                        },
                    access_control_allow_methods:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowMethods {
                            quantity: i32::try_from(cors.allow_methods.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_methods
                                .iter()
                                .map(|method| (*method).to_string())
                                .collect(),
                        },
                    access_control_allow_credentials: cors.allow_credentials,
                    // A policy that exposes no response header to script carries no
                    // `AccessControlExposeHeaders` element at all, rather than an empty one.
                    access_control_expose_headers: None,
                    access_control_max_age_sec: cors.max_age_seconds,
                    origin_override: cors.origin_override,
                }
            }),
            security_headers_config: if self.security_headers {
                Some(managed_security_headers_config())
            } else {
                None
            },
            // The published policies state no server-timing, custom or removed headers, so
            // those elements are absent rather than empty.
            server_timing_headers_config: None,
            custom_headers_config: None,
            remove_headers_config: None,
        }
    }
}

/// The security headers every published policy that adds them adds: the set Amazon
/// publishes for `Managed-SecurityHeadersPolicy`, which the combined CORS-and-security
/// policies carry unchanged.
pub fn managed_security_headers_config() -> crate::ops::types::ResponseHeadersPolicySecurityHeadersConfig
{
    crate::ops::types::ResponseHeadersPolicySecurityHeadersConfig {
        xss_protection: Some(crate::ops::types::ResponseHeadersPolicyXSSProtection {
            r#override: false,
            protection: true,
            mode_block: Some(true),
            report_uri: None,
        }),
        frame_options: Some(crate::ops::types::ResponseHeadersPolicyFrameOptions {
            r#override: false,
            frame_option: "SAMEORIGIN".to_string(),
        }),
        referrer_policy: Some(crate::ops::types::ResponseHeadersPolicyReferrerPolicy {
            r#override: false,
            referrer_policy: "strict-origin-when-cross-origin".to_string(),
        }),
        // The published policy states no Content-Security-Policy: the directives belong to
        // the application, so AWS leaves that header to a policy the account writes.
        content_security_policy: None,
        content_type_options: Some(crate::ops::types::ResponseHeadersPolicyContentTypeOptions {
            r#override: false,
        }),
        strict_transport_security: Some(
            crate::ops::types::ResponseHeadersPolicyStrictTransportSecurity {
                r#override: false,
                include_subdomains: Some(true),
                preload: Some(false),
                access_control_max_age_sec: MANAGED_STRICT_TRANSPORT_SECURITY_MAX_AGE_SECONDS,
            },
        ),
    }
}

/// `Strict-Transport-Security`'s `max-age` in the published security headers: one year, the
/// value Amazon publishes for `Managed-SecurityHeadersPolicy`.
const MANAGED_STRICT_TRANSPORT_SECURITY_MAX_AGE_SECONDS: i32 = 31_536_000;

/// `Access-Control-Max-Age` in the published policies that answer preflight requests, the
/// value Amazon publishes for them.
const MANAGED_CORS_PREFLIGHT_MAX_AGE_SECONDS: i32 = 600;

/// The CORS settings the published policies that allow a SIMPLE cross-origin request carry:
/// any origin, any request header, and the methods a simple request uses.
const MANAGED_SIMPLE_CORS: ManagedCorsSettings = ManagedCorsSettings {
    allow_origins: &["*"],
    allow_headers: &["*"],
    allow_methods: &["GET", "HEAD", "POST"],
    allow_credentials: false,
    max_age_seconds: None,
    origin_override: true,
};

/// The CORS settings the published policies that also answer PREFLIGHT requests carry: any
/// origin, any request header, every method, and the published `Access-Control-Max-Age`.
const MANAGED_PREFLIGHT_CORS: ManagedCorsSettings = ManagedCorsSettings {
    allow_origins: &["*"],
    allow_headers: &["*"],
    allow_methods: &["ALL"],
    allow_credentials: false,
    max_age_seconds: Some(MANAGED_CORS_PREFLIGHT_MAX_AGE_SECONDS),
    origin_override: true,
};

/// The AWS-managed response headers policies, in the order their ids sort in — the same
/// total order every listing of this service pages through.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it (see
/// [`ManagedResponseHeadersPolicy`] for the source).
pub const MANAGED_RESPONSE_HEADERS_POLICIES: &[ManagedResponseHeadersPolicy] = &[
    ManagedResponseHeadersPolicy {
        id: "5cc3b908-e619-4b99-88e5-2cf7f45965bd",
        name: "Managed-CORS-With-Preflight",
        comment: "Allows CORS requests from any origin, including preflight requests",
        cors: Some(MANAGED_PREFLIGHT_CORS),
        security_headers: false,
    },
    ManagedResponseHeadersPolicy {
        id: "60669652-455b-4ae9-85a4-c4c02393f86c",
        name: "Managed-SimpleCORS",
        comment: "Allows simple CORS requests from any origin",
        cors: Some(MANAGED_SIMPLE_CORS),
        security_headers: false,
    },
    ManagedResponseHeadersPolicy {
        id: "67f7725c-6f97-4210-82d7-5512b31e9d03",
        name: "Managed-SecurityHeadersPolicy",
        comment: "Adds a set of security headers to every response",
        cors: None,
        security_headers: true,
    },
    ManagedResponseHeadersPolicy {
        id: "e61eb345-f6c1-4b97-f0d2-0bdcb2b0b6d3",
        name: "Managed-CORS-and-SecurityHeadersPolicy",
        comment: "Allows simple CORS requests from any origin and adds a set of security \
                  headers to every response",
        cors: Some(MANAGED_SIMPLE_CORS),
        security_headers: true,
    },
    ManagedResponseHeadersPolicy {
        id: "eaab4381-ed33-4a86-88ca-d9558dc6cd63",
        name: "Managed-CORS-with-preflight-and-SecurityHeadersPolicy",
        comment: "Allows CORS requests from any origin, including preflight requests, and \
                  adds a set of security headers to every response",
        cors: Some(MANAGED_PREFLIGHT_CORS),
        security_headers: true,
    },
];

/// The managed response headers policy `id` names, or `None` when the catalog holds none —
/// what tells an id AWS published from one this account minted.
pub fn managed_response_headers_policy(
    id: &str,
) -> Option<&'static ManagedResponseHeadersPolicy> {
    MANAGED_RESPONSE_HEADERS_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// One CloudFront public key, as this emulator persists it.
///
/// The record is the single source of truth for the key's id, the configuration it was
/// uploaded with, the instant it was uploaded and its version token, so the create and
/// every later read — `GetPublicKey` and every `ListPublicKeys` page — answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `PublicKey` is `Id`, `CreatedTime`
/// and `PublicKeyConfig` only — a public key carries no status and no propagation
/// deadline, so a key this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredPublicKey {
    /// `PublicKey$Id` — "The identifier of the public key": the token minted at creation,
    /// which is also the key this record is held under and the path segment its
    /// `Location` names.
    pub id: String,
    /// `PublicKey$CreatedTime` — "The date and time when the public key was uploaded."
    /// Written once, by the create, so every later read reports the same instant.
    pub created_time: SystemTime,
    /// The configuration as submitted — stored whole, so a read reports what was stored
    /// rather than re-deriving anything from a request, down to the members the create
    /// left out.
    pub config: PublicKeyConfig,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this public key.
    pub etag: String,
}

impl StoredPublicKey {
    /// The `PublicKey` projection every public key operation returns, read from the one
    /// stored record.
    pub fn to_public_key(&self) -> PublicKey {
        PublicKey {
            id: self.id.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            public_key_config: self.config.clone(),
        }
    }

    /// The `PublicKeySummary` a listing reports for this key.
    ///
    /// Every member is the stored record's: `PublicKeySummary` repeats the configuration's
    /// `Name`, `EncodedKey` and `Comment` beside the identity the create minted, and
    /// `Comment` is optional on both, so a key uploaded without one is summarised without
    /// one rather than with an empty element invented by the read.
    pub fn to_summary(&self) -> PublicKeySummary {
        PublicKeySummary {
            id: self.id.clone(),
            name: self.config.name.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            encoded_key: self.config.encoded_key.clone(),
            comment: self.config.comment.clone(),
        }
    }
}

/// One CloudFront key group, as this emulator persists it.
///
/// "A key group contains a list of public keys that you can use with CloudFront signed URLs
/// and signed cookies." The record is the single source of truth for the group's id, the
/// configuration it was created with, the instant it was last written and its version
/// token, so the create and every later read — `GetKeyGroup` above all — answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `KeyGroup` is `Id`,
/// `LastModifiedTime` and `KeyGroupConfig` only — a key group carries no status and no
/// propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredKeyGroup {
    /// `KeyGroup$Id` — "The identifier for the key group": the token minted at creation,
    /// which is also the key this record is held under and the path segment its `Location`
    /// names.
    pub id: String,
    /// The configuration as submitted — "A key group configuration." Stored whole, so a
    /// read reports what was stored rather than re-deriving anything from a request, down
    /// to the members the create left out.
    pub config: KeyGroupConfig,
    /// `KeyGroup$LastModifiedTime` — "The date and time when the key group was last
    /// modified." Written by the call that last wrote the record, so every later read
    /// reports the same instant.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this key group.
    pub etag: String,
}

impl StoredKeyGroup {
    /// The `KeyGroup` projection every key group operation returns, read from the one
    /// stored record.
    pub fn to_key_group(&self) -> KeyGroup {
        KeyGroup {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            key_group_config: self.config.clone(),
        }
    }
}

/// One CloudFront field-level encryption profile, as this emulator persists it.
///
/// The record is the single source of truth for the profile's id, the configuration it was
/// created with, the instant it was last written and its version token, so the create and
/// every later read — `GetFieldLevelEncryptionProfile` and
/// `GetFieldLevelEncryptionProfileConfig` — answer from it rather than from the request
/// being served.
///
/// Nothing in here is owed by the CLOCK: the model's `FieldLevelEncryptionProfile` is `Id`,
/// `LastModifiedTime` and `FieldLevelEncryptionProfileConfig` only — a profile carries no
/// status and no propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredFieldLevelEncryptionProfile {
    /// `FieldLevelEncryptionProfile$Id` — "The ID for a field-level encryption profile
    /// configuration": the token minted at creation, which is also the key this record is
    /// held under and the path segment its `Location` names.
    pub id: String,
    /// `FieldLevelEncryptionProfile$LastModifiedTime` — "The last time the field-level
    /// encryption profile was updated", which for a profile nothing has updated yet is the
    /// instant the create stored it.
    pub last_modified_time: SystemTime,
    /// The configuration as submitted — stored whole, so a read reports what was stored
    /// rather than re-deriving anything from a request, down to the members the create
    /// left out.
    pub config: FieldLevelEncryptionProfileConfig,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this profile.
    pub etag: String,
}

impl StoredFieldLevelEncryptionProfile {
    /// The `FieldLevelEncryptionProfile` projection every profile operation returns, read
    /// from the one stored record.
    pub fn to_profile(&self) -> FieldLevelEncryptionProfile {
        FieldLevelEncryptionProfile {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            field_level_encryption_profile_config: self.config.clone(),
        }
    }

    /// The `FieldLevelEncryptionProfileSummary` a listing reports for this profile.
    ///
    /// Every member is the stored record's: the summary repeats the configuration's `Name`,
    /// `EncryptionEntities` and `Comment` beside the identity the create minted and the
    /// instant that record was last written, and `Comment` is optional on both, so a profile
    /// created without one is summarised without one rather than with an empty element
    /// invented by the read.
    pub fn to_summary(&self) -> FieldLevelEncryptionProfileSummary {
        FieldLevelEncryptionProfileSummary {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            name: self.config.name.clone(),
            encryption_entities: self.config.encryption_entities.clone(),
            comment: self.config.comment.clone(),
        }
    }
}

/// One CloudFront field-level encryption CONFIGURATION, as this emulator persists it.
///
/// The record is the single source of truth for the configuration's id, the configuration it
/// was created with, the instant it was last written and its version token, so the create and
/// every later read — `GetFieldLevelEncryption` and `GetFieldLevelEncryptionConfig` — answer
/// from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `FieldLevelEncryption` is `Id`,
/// `LastModifiedTime` and `FieldLevelEncryptionConfig` only — a configuration carries no
/// status and no propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredFieldLevelEncryptionConfig {
    /// `FieldLevelEncryption$Id` — "The configuration ID for a field-level encryption
    /// configuration": the token minted at creation, which is also the key this record is
    /// held under and the path segment its `Location` names.
    pub id: String,
    /// `FieldLevelEncryption$LastModifiedTime` — "The last time the field-level encryption
    /// configuration was changed", which for a configuration nothing has changed yet is the
    /// instant the create stored it.
    pub last_modified_time: SystemTime,
    /// The configuration as submitted — stored whole, so a read reports what was stored
    /// rather than re-deriving anything from a request, down to the members the create
    /// left out.
    pub config: FieldLevelEncryptionConfig,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this configuration.
    pub etag: String,
}

impl StoredFieldLevelEncryptionConfig {
    /// The `FieldLevelEncryption` projection every configuration operation returns, read
    /// from the one stored record.
    pub fn to_field_level_encryption(&self) -> FieldLevelEncryption {
        FieldLevelEncryption {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            field_level_encryption_config: self.config.clone(),
        }
    }

    /// The `FieldLevelEncryptionSummary` a listing reports for this configuration.
    ///
    /// Every member is the stored record's: the summary repeats the stored configuration's
    /// `Comment`, `QueryArgProfileConfig` and `ContentTypeProfileConfig` beside the identity
    /// the create minted, and all three are optional on both, so a configuration stored
    /// without one is summarised without one rather than with an element invented by the
    /// read. `CallerReference` is not a summary member, so it is not reported here.
    pub fn to_summary(&self) -> FieldLevelEncryptionSummary {
        FieldLevelEncryptionSummary {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            comment: self.config.comment.clone(),
            query_arg_profile_config: self.config.query_arg_profile_config.clone(),
            content_type_profile_config: self.config.content_type_profile_config.clone(),
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
    /// Every Anycast static IP list this emulator holds, keyed by its minted id — the
    /// identifier every Anycast static IP list operation addresses one by, and the id a
    /// connection group's `AnycastIpListId` resolves against
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::holds_anycast_ip_list`]). A
    /// `BTreeMap` for the same reason as `distributions`: one stable total order for
    /// `ListAnycastIpLists`'s `Marker` to index into.
    pub anycast_ip_lists: BTreeMap<String, StoredAnycastIpList>,
    /// The host number of the next Anycast static IP this store allocates, within the
    /// blocks [`ANYCAST_IPV4_PREFIX`] and [`ANYCAST_IPV6_PREFIX`] name. One counter for
    /// both families, so no two addresses this store allocates — of either family — are
    /// ever rendered from the same number, and the addresses of a list are stable for its
    /// whole life because nothing ever re-allocates them.
    pub next_anycast_ip: u32,
    /// Every continuous deployment policy this emulator holds, keyed by its id — the
    /// identifier `GetContinuousDeploymentPolicy` addresses one by. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for
    /// `ListContinuousDeploymentPolicies`'s `Marker` to index into.
    pub continuous_deployment_policies: BTreeMap<String, StoredContinuousDeploymentPolicy>,
    /// Every distribution tenant this emulator holds, keyed by tenant id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order a listing pages
    /// through.
    pub distribution_tenants: BTreeMap<String, StoredDistributionTenant>,
    /// Every connection group this emulator holds, keyed by connection group id — the
    /// identifier every connection group operation addresses one by. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for
    /// `ListConnectionGroups`'s `Marker` to index into.
    ///
    /// At most ONE entry carries [`StoredConnectionGroup::is_default`]: "When you first
    /// create a distribution tenant and you don't specify a connection group, CloudFront
    /// will automatically create a default connection group for you" — so that one is
    /// created ONCE, by the first call that needs it, and every later tenant that names no
    /// connection group is routed through it.
    pub connection_groups: BTreeMap<String, StoredConnectionGroup>,
    /// Every origin access identity this emulator holds, keyed by its id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_access_identities: BTreeMap<String, StoredOriginAccessIdentity>,
    /// Every response headers policy this emulator holds, keyed by its minted id — the
    /// identifier every response headers policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub response_headers_policies: BTreeMap<String, StoredResponseHeadersPolicy>,
    /// Every CloudFront function this emulator holds, keyed by its name — "A function
    /// with the same name already exists in this Amazon Web Services account" is what
    /// makes the name the key and a repeated name a `FunctionAlreadyExists`. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through. Everything that reads or writes it lives in [`functions`].
    pub functions: BTreeMap<String, StoredFunction>,
    /// Every origin access control this emulator holds, keyed by its minted id — the
    /// identifier every origin access control operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_access_controls: BTreeMap<String, StoredOriginAccessControl>,
    /// Every origin request policy this emulator holds, keyed by its minted id — the
    /// identifier every origin request policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_request_policies: BTreeMap<String, StoredOriginRequestPolicy>,
    /// Every cache policy this emulator holds, keyed by its minted id — the identifier
    /// every cache policy operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through.
    pub cache_policies: BTreeMap<String, StoredCachePolicy>,
    /// Every connection function this emulator holds, keyed by its minted id — the
    /// identifier every mutating connection function operation addresses one by (its name
    /// and its ARN resolve to the same record). A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through. Everything
    /// that reads or writes it lives in [`connection_functions`].
    pub connection_functions: BTreeMap<String, StoredConnectionFunction>,
    /// Every public key this emulator holds, keyed by its minted id — the identifier every
    /// public key operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for `ListPublicKeys`'s `Marker` to index
    /// into.
    pub public_keys: BTreeMap<String, StoredPublicKey>,
    /// Every key group this emulator holds, keyed by its minted id — the identifier every
    /// key group operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for `ListKeyGroups`'s `Marker` to index
    /// into.
    pub key_groups: BTreeMap<String, StoredKeyGroup>,
    /// Every field-level encryption profile this emulator holds, keyed by its minted id —
    /// the identifier every field-level encryption profile operation addresses one by. A
    /// `BTreeMap` for the same reason as `distributions`: one stable total order for
    /// `ListFieldLevelEncryptionProfiles`'s `Marker` to index into.
    pub field_level_encryption_profiles: BTreeMap<String, StoredFieldLevelEncryptionProfile>,
    /// Every field-level encryption configuration this emulator holds, keyed by its minted
    /// id — the identifier every field-level encryption configuration operation addresses
    /// one by. A `BTreeMap` for the same reason as `distributions`: one stable total order
    /// for `ListFieldLevelEncryptionConfigs`'s `Marker` to index into.
    pub field_level_encryption_configs: BTreeMap<String, StoredFieldLevelEncryptionConfig>,
    /// Every streaming (RTMP) distribution this emulator holds, keyed by its id — the
    /// identifier every streaming distribution operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for
    /// `ListStreamingDistributions`'s `Marker` to index into.
    pub streaming_distributions: BTreeMap<String, StoredStreamingDistribution>,
    /// Every real-time log configuration this emulator holds, keyed by its name — "The
    /// unique name of this real-time log configuration", which is what makes the name the
    /// key and a repeated name a `RealtimeLogConfigAlreadyExists`. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for a listing to page
    /// through. Everything that reads or writes it lives in [`realtime_log_configs`].
    pub realtime_log_configs: BTreeMap<String, StoredRealtimeLogConfig>,
    /// The resource policy of every resource that carries one, keyed by the ARN of the
    /// resource it is attached to — the one identifier the resource-policy surface
    /// addresses a policy by, in all three of its operations, so a policy written under an
    /// ARN is the policy every later read of that ARN answers from. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order.
    ///
    /// The value is the policy document exactly as it was submitted: the surface stores a
    /// document, never a re-derivation of one, so what a read reports is byte for byte what
    /// the last write persisted.
    pub resource_policies: BTreeMap<String, String>,
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
        for anycast_ip_list in self.anycast_ip_lists.values_mut() {
            anycast_ip_list.settle(now);
        }
        for tenant in self.distribution_tenants.values_mut() {
            tenant.settle(now);
        }
        for connection_group in self.connection_groups.values_mut() {
            connection_group.settle(now);
        }
        for streaming_distribution in self.streaming_distributions.values_mut() {
            streaming_distribution.settle(now);
        }
        for anycast_ip_list in self.anycast_ip_lists.values_mut() {
            anycast_ip_list.settle(now);
        }
    }

    /// The key value store `name` names, or `None` when the store holds none.
    pub fn key_value_store(&self, name: &str) -> Option<&StoredKeyValueStore> {
        self.key_value_stores.get(name)
    }

    /// The resource policy attached to the resource `arn` names, or `None` when that
    /// resource carries none.
    pub fn resource_policy(&self, arn: &str) -> Option<&str> {
        self.resource_policies.get(arn).map(String::as_str)
    }

    /// Attach `document` to the resource `arn` names as its resource policy, replacing
    /// whatever policy that resource carried before.
    ///
    /// One policy per resource: the surface's write is a PUT, so a second write under the
    /// same ARN supersedes the first rather than adding a second policy beside it.
    pub fn put_resource_policy(&mut self, arn: &str, document: &str) {
        self.resource_policies
            .insert(arn.to_string(), document.to_string());
    }

    /// Remove the resource policy attached to the resource `arn` names, answering whether
    /// that resource carried one.
    ///
    /// `false` is the whole "there was nothing to remove" answer, which is what lets the
    /// delete tell an accepted ARN carrying no policy apart from one it removed a policy
    /// from; nothing else in the store is touched either way, since a policy is keyed by the
    /// ARN it was attached to alone.
    pub fn delete_resource_policy(&mut self, arn: &str) -> bool {
        self.resource_policies.remove(arn).is_some()
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

    /// The id of the distribution that attaches the origin access identity `id` to one of
    /// its Amazon S3 origins, or `None` when nothing stored names it.
    ///
    /// `CloudFrontOriginAccessIdentityInUse` is documented as "The Origin Access Identity
    /// specified is already in use", and a stored configuration names an identity in
    /// exactly two places: `S3OriginConfig$OriginAccessIdentity` on an origin of a
    /// distribution, and `S3Origin$OriginAccessIdentity` on a streaming distribution. Both
    /// carry the reference form [`origin_access_identity_reference`] builds, and both are
    /// documented EMPTY when the configuration names no identity — so what makes an
    /// identity in use is a stored S3 origin of either family naming it, and the answer is
    /// the distribution that does.
    ///
    /// The scan reads the STORED configurations rather than a second index beside them, for
    /// the same reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_origin_access_control`]
    /// scans: it cannot drift out of step with what a read of those distributions reports,
    /// and both collections are bounded ([`MAX_DISTRIBUTIONS`] and
    /// [`MAX_STREAMING_DISTRIBUTIONS`], each record with its own bounded origin list).
    ///
    /// The holder's ID rather than its record, because the two families are stored as
    /// different types and only the id is common to both.
    pub fn distribution_using_origin_access_identity(&self, id: &str) -> Option<&str> {
        if id.is_empty() {
            return None;
        }
        let reference = origin_access_identity_reference(id);
        self.distributions
            .values()
            .find(|distribution| {
                distribution.config.origins.items.iter().any(|origin| {
                    origin
                        .s3_origin_config
                        .as_ref()
                        .is_some_and(|s3| s3.origin_access_identity.trim() == reference)
                })
            })
            .map(|distribution| distribution.id.as_str())
            .or_else(|| {
                self.streaming_distributions
                    .values()
                    .find(|distribution| {
                        distribution.config.s3_origin.origin_access_identity.trim() == reference
                    })
                    .map(|distribution| distribution.id.as_str())
            })
    }

    /// The streaming distribution `id` names, or `None` when the store holds none.
    pub fn streaming_distribution(&self, id: &str) -> Option<&StoredStreamingDistribution> {
        self.streaming_distributions.get(id)
    }

    /// The streaming distribution created under `caller_reference`, or `None` when no
    /// streaming distribution was.
    ///
    /// A scan over the stored records rather than a second index beside them, for the
    /// reason [`ComAmazonawsCloudfrontCloudfront20200531State::origin_access_identity_by_caller_reference`]
    /// is one: the records are the single source of truth for which references are taken,
    /// so a scan cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_STREAMING_DISTRIBUTIONS`].
    pub fn streaming_distribution_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredStreamingDistribution> {
        self.streaming_distributions
            .values()
            .find(|distribution| distribution.config.caller_reference == caller_reference)
    }

    /// The distribution `id` names, or `None` when the store holds none.
    pub fn distribution(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions.get(id)
    }

    /// The distribution whose edge domain name is `domain_name`, or `None` when the store
    /// holds none.
    ///
    /// A domain name is allocated to one distribution at creation and is stable for its
    /// whole life, so it addresses at most one record. This is how a resource that names a
    /// distribution by its DOMAIN name rather than by its id — a continuous deployment
    /// policy's `StagingDistributionDnsNames` — resolves to the distribution it routes
    /// traffic to.
    pub fn distribution_by_domain_name(&self, domain_name: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| distribution.domain_name == domain_name)
    }

    /// The continuous deployment policy `id` names, or `None` when the store holds none.
    pub fn continuous_deployment_policy(
        &self,
        id: &str,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        self.continuous_deployment_policies.get(id)
    }

    /// The distribution that attaches the continuous deployment policy `id`, or `None` when
    /// no stored distribution does.
    ///
    /// `ContinuousDeploymentPolicyInUse` is documented as "You cannot delete a continuous
    /// deployment policy that is associated with a primary distribution", and a distribution
    /// attaches one through its `ContinuousDeploymentPolicyId` — so what makes a policy in
    /// use is a stored distribution naming it. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for the policies a cache behaviour attaches: the scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports, and the collection is bounded
    /// ([`MAX_DISTRIBUTIONS`]). A distribution that attaches no policy carries the empty
    /// string rather than an id, so an id matches only a distribution that really names it.
    pub fn distribution_using_continuous_deployment_policy(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions.values().find(|distribution| {
            distribution
                .config
                .continuous_deployment_policy_id
                .as_deref()
                .is_some_and(|attached| attached == id)
        })
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

    /// The distribution tenant `domain` routes to, or `None` when no tenant this store
    /// holds serves it.
    ///
    /// "A domain name associated with the target distribution tenant": the association is
    /// the tenant's own `Domains`, and a domain routes to at most one CloudFront resource
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]), so at most one
    /// tenant answers. DNS names are case-insensitive, so the lookup is too
    /// ([`StoredDistributionTenant::serves_domain`]).
    ///
    /// This is the HOST-to-tenant map a viewer's request is resolved through, so it holds a
    /// domain only once CloudFront can serve it: a domain still waiting for the tenant's
    /// CloudFront managed certificate to be validated is not in it yet
    /// ([`StoredDistributionTenant::domain_pending_validation`]), which is why a by-domain
    /// read of it answers "no such entity" while the certificate is pending. It is also the
    /// index a second claim on the domain is refused against
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]): a claim is
    /// enforced from the moment it can be served, not from the moment it is written down.
    ///
    /// Several tenants may therefore hold the same domain while none of their claims is
    /// served yet, and the validation that resolves such a race decides ONE of them: the
    /// EARLIEST claim validated for a domain is the one it routes to, so the map answers the
    /// first-created serving tenant and a later duplicate never takes a domain away from
    /// it. The id breaks a tie, so the answer is a property of the store rather than of the
    /// walk.
    pub fn distribution_tenant_by_domain(&self, domain: &str) -> Option<&StoredDistributionTenant> {
        self.distribution_tenants
            .values()
            .filter(|tenant| tenant.serves_domain(domain))
            .min_by(|left, right| {
                (left.created_time, left.id.as_str()).cmp(&(right.created_time, right.id.as_str()))
            })
    }

    /// Whether `domain` is already SERVED by a CloudFront resource this store holds: a
    /// domain of a distribution tenant other than `except_tenant_id` that the tenant can
    /// already be reached at, or an alias of a distribution.
    ///
    /// "The CNAME specified is already defined for CloudFront": one domain name routes to
    /// at most one CloudFront resource, so the check spans every resource kind that can
    /// claim one. `except_tenant_id` is the tenant a mutation is rewriting, whose own
    /// domains are no conflict with itself.
    ///
    /// A claim counts only once the certificate it is served over has been VALIDATED, which
    /// is exactly when the domain enters the host-to-tenant map
    /// ([`StoredDistributionTenant::serves_domain`]): until then nothing resolves the domain
    /// to the resource, so there is no route for a second claim to collide with and the
    /// claim is not enforced. A tenant whose CloudFront managed certificate is still
    /// pending validation therefore does not take a domain, while a tenant whose
    /// certificate has been issued — and a distribution's alias, which cannot be declared
    /// without a custom viewer certificate that covers it
    /// ([`alias_certificate_mismatch`]) — does.
    pub fn domain_in_use(&self, domain: &str, except_tenant_id: Option<&str>) -> bool {
        let served_by_tenant = self.distribution_tenants.values().any(|tenant| {
            Some(tenant.id.as_str()) != except_tenant_id && tenant.serves_domain(domain)
        });
        served_by_tenant
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

    /// Every domain association this store holds that conflicts with the domain
    /// `requested`, as the `DomainConflict` each is reported as.
    ///
    /// A conflict is an association of a name that cannot be served beside `requested`:
    /// the same name, or a wildcard alternate domain name that covers it — or that it
    /// covers ([`domains_conflict`]). The associations CloudFront's domain surface
    /// addresses are a standard distribution's alternate domain names and a distribution
    /// tenant's domains, which is also the pair `DistributionResourceType` admits, so the
    /// alias of a legacy RTMP distribution is none of them: there is no resource type to
    /// report one under.
    ///
    /// One association is reported ONCE — the conflicts are the SET of (domain, resource)
    /// associations — and they are ordered by the conflicting domain and then by the id of
    /// the resource holding it, so the order is a property of the STORE rather than of the
    /// walk and a resume point into it names the same place on every call.
    pub fn domain_conflicts(&self, requested: &str) -> Vec<DomainConflict> {
        let account_id = ::engine::emulator_account_id();
        let mut conflicts: Vec<DomainConflict> = Vec::new();
        let mut push = |domain: &str, resource_type: &str, resource_id: &str| {
            conflicts.push(DomainConflict {
                domain: domain.to_string(),
                resource_type: resource_type.to_string(),
                resource_id: resource_id.to_string(),
                account_id: account_id.to_string(),
            });
        };
        for distribution in self.distributions.values() {
            let aliases = distribution
                .config
                .aliases
                .as_ref()
                .and_then(|aliases| aliases.items.as_deref())
                .unwrap_or_default();
            for alias in aliases {
                if domains_conflict(alias, requested) {
                    push(
                        alias,
                        DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION,
                        &distribution.id,
                    );
                }
            }
        }
        for tenant in self.distribution_tenants.values() {
            for domain in &tenant.domains {
                if domains_conflict(domain, requested) {
                    push(domain, DISTRIBUTION_RESOURCE_TYPE_TENANT, &tenant.id);
                }
            }
        }
        conflicts.sort_by_key(conflict_order);
        conflicts.dedup_by_key(|conflict| conflict_order(conflict));
        conflicts
    }

    /// The CloudFront endpoint the DNS name `name` resolves to, or `None` when this
    /// service holds no record that resolves it.
    ///
    /// CloudFront answers for the names it MINTS — the `d<label>.cloudfront.net` endpoints
    /// it allocates: a connection group's routing endpoint ("also known as the DNS name
    /// ... such as `d111111abcdef8.cloudfront.net`") and a distribution's own
    /// `DomainName`. Each of those names resolves to itself, and that is the whole of what
    /// this emulator can resolve: a name in a CUSTOMER's zone (the domain of a
    /// distribution tenant, an alias of a distribution) is published by the customer's own
    /// DNS, which this emulator neither runs nor reaches — so it holds no record for one,
    /// exactly as [`DOMAIN_STATUS_ACTIVE`] says of the association it does hold.
    ///
    /// [`MULTI_TENANT_DOMAIN_NAME`] is excluded: it is the fixed placeholder a
    /// multi-tenant distribution reports in place of an edge domain, not a name that
    /// resolves anywhere.
    pub fn resolved_cloudfront_endpoint(&self, name: &str) -> Option<String> {
        if let Some(group) = self.connection_group_by_routing_endpoint(name) {
            return Some(group.routing_endpoint.clone());
        }
        if let Some(distribution) = self.distributions.values().find(|distribution| {
            let reported = distribution.reported_domain_name();
            reported != MULTI_TENANT_DOMAIN_NAME && same_domain(&reported, name)
        }) {
            return Some(distribution.reported_domain_name());
        }
        self.streaming_distributions
            .values()
            .find(|distribution| same_domain(&distribution.domain_name, name))
            .map(|distribution| distribution.domain_name.clone())
    }

    /// The first domain a CloudFront managed ACM certificate `request` would cover whose
    /// ownership this service cannot verify, or `None` when the request asks for no
    /// verification or every covered domain is verified.
    ///
    /// `ManagedCertificateRequest$ValidationTokenHost` decides WHO serves the HTTP
    /// validation token: [`VALIDATION_TOKEN_HOST_CLOUDFRONT`] asks CloudFront to serve it at
    /// the domain itself, which it can only do once that domain resolves to a CloudFront
    /// resource — so the request is admitted only for a domain this service resolves to one,
    /// and the other mode ("you serve the validation token from your existing
    /// infrastructure") leaves the validation to the caller and demands nothing of DNS.
    ///
    /// The DNS view is the one and only one this service has
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::resolved_cloudfront_endpoint`]), so
    /// the names a certificate request is admitted for are exactly the names a verification
    /// of the DNS configuration can read a record for. The covered domains are the ones the
    /// resource claims plus the `PrimaryDomainName` the request names, if any: CloudFront
    /// has to serve a token at every one of them.
    pub fn unverifiable_certificate_domain(
        &self,
        request: &ManagedCertificateRequest,
        domains: &[String],
    ) -> Option<String> {
        if request.validation_token_host != VALIDATION_TOKEN_HOST_CLOUDFRONT {
            return None;
        }
        self.unproven_domain(
            domains.iter().map(String::as_str).chain(
                request
                    .primary_domain_name
                    .as_deref()
                    .filter(|domain| !domain.is_empty()),
            ),
        )
        .map(str::to_string)
    }

    /// Whether `id` names a connection group this store holds — an id no connection group
    /// carries names no entity.
    pub fn holds_connection_group(&self, id: &str) -> bool {
        self.connection_groups.contains_key(id)
    }

    /// Whether any distribution tenant this store holds is routed through the connection
    /// group `id`.
    ///
    /// A connection group is "what a distribution tenant's viewers are routed through", and
    /// the attachment lives on the tenant as its `ConnectionGroupId`, so the question is
    /// answered against the STORED tenants rather than against any request — the way
    /// [`Self::key_value_store_is_associated`] answers the same question for the other
    /// attachment this store holds. It is what decides whether a group may be deleted.
    pub fn connection_group_is_associated(&self, id: &str) -> bool {
        self.distribution_tenants
            .values()
            .any(|tenant| tenant.connection_group_id == id)
    }

    /// The first of `domains` this service has no proof the account controls, or `None`
    /// when every one of them is proven.
    ///
    /// A domain is where a distribution tenant's viewers reach it, so CloudFront binds one
    /// only to a domain the account's control of which it can see established. The one
    /// proof this service can read is its own DNS view
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::resolved_cloudfront_endpoint`]):
    /// a domain that already resolves to a CloudFront resource in this account is a domain
    /// whose DNS the account steers. A name in a customer's zone is published by DNS this
    /// emulator neither runs nor reaches, so nothing here proves it — and an ACM
    /// certificate the caller names by ARN proves nothing either, since a certificate
    /// imported into ACM carries no domain validation at all. The remaining way to bind
    /// such a domain is to ask CloudFront to validate it, which is what a
    /// `ManagedCertificateRequest` on the call does.
    ///
    /// ONE rule for every call that binds a domain to a tenant, and the ONE place the DNS
    /// view is consulted per domain — the certificate-request check above answers from it
    /// too — so an admission and a later association cannot drift apart.
    pub fn unproven_domain<'a, I>(&self, domains: I) -> Option<&'a str>
    where
        I: IntoIterator<Item = &'a str>,
    {
        domains
            .into_iter()
            .find(|domain| self.resolved_cloudfront_endpoint(domain).is_none())
    }

    /// Whether `id` names an Anycast static IP list this store holds.
    ///
    /// Every list in the store was put there by `CreateAnycastIpList`, so an id no stored
    /// list carries is no argument this service can resolve — which is what the
    /// `InvalidArgument` the connection group create and listing answer for one says
    /// ([`invalid_anycast_ip_list`]). The ONE place that resolves an `AnycastIpListId`, so
    /// every operation that takes one agrees on which ids exist.
    pub fn holds_anycast_ip_list(&self, id: &str) -> bool {
        self.anycast_ip_lists.contains_key(id)
    }

    /// The Anycast static IP list `id` names, or `None` when the store holds none.
    pub fn anycast_ip_list(&self, id: &str) -> Option<&StoredAnycastIpList> {
        self.anycast_ip_lists.get(id)
    }

    /// The Anycast static IP list whose STORED name is `name`, or `None` when none is.
    ///
    /// A scan over the stored records rather than a second index beside them, for the
    /// reason [`ComAmazonawsCloudfrontCloudfront20200531State::origin_access_identity_by_caller_reference`]
    /// is one: the records are the single source of truth for which names are taken, and
    /// the collection is bounded by the address pool every list is allocated from
    /// ([`ANYCAST_IP_POOL`]).
    pub fn anycast_ip_list_named(&self, name: &str) -> Option<&StoredAnycastIpList> {
        self.anycast_ip_lists
            .values()
            .find(|list| list.name == name)
    }

    /// The connection group `identifier` names, or `None` when the store holds none.
    ///
    /// `GetConnectionGroup$Identifier` is "The ID, name, or Amazon Resource Name (ARN) of
    /// the connection group", so all three resolve to the one record: the id is the
    /// store's key, and the ARN and the name are compared against the STORED values
    /// rather than re-derived out of the request. The three namespaces cannot overlap —
    /// an id carries [`CONNECTION_GROUP_ID_PREFIX`] and an ARN starts with `arn:` — so at
    /// most one of them matches.
    pub fn connection_group(&self, identifier: &str) -> Option<&StoredConnectionGroup> {
        if let Some(group) = self.connection_groups.get(identifier) {
            return Some(group);
        }
        self.connection_groups
            .values()
            .find(|group| group.arn == identifier || group.name == identifier)
    }

    /// The connection group whose STORED name is `name`, or `None` when none is.
    ///
    /// `CreateConnectionGroup$Name` is "a friendly identifier that is unique within your
    /// Amazon Web Services account", so at most one group answers, and this is what makes
    /// a repeated name a conflict rather than a second group.
    pub fn connection_group_named(&self, name: &str) -> Option<&StoredConnectionGroup> {
        self.connection_groups
            .values()
            .find(|group| group.name == name)
    }

    /// The connection group whose STORED routing endpoint is `endpoint`, or `None` when
    /// none is.
    ///
    /// `ConnectionGroup$RoutingEndpoint` is "the routing endpoint (also known as the DNS
    /// name) that is assigned to the connection group", and this service assigns a fresh
    /// one to each group, so at most one group answers. The comparison is the one every
    /// DNS name in this store is compared by ([`same_domain`]): a name is the same name in
    /// any case.
    pub fn connection_group_by_routing_endpoint(
        &self,
        endpoint: &str,
    ) -> Option<&StoredConnectionGroup> {
        self.connection_groups
            .values()
            .find(|group| same_domain(&group.routing_endpoint, endpoint))
    }

    /// The response headers policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_response_headers_policy`].
    pub fn response_headers_policy(&self, id: &str) -> Option<&StoredResponseHeadersPolicy> {
        self.response_headers_policies.get(id)
    }

    /// The response headers policy `id` names wherever it came from: one this account
    /// created with `CreateResponseHeadersPolicy`, or one AWS published
    /// ([`MANAGED_RESPONSE_HEADERS_POLICIES`]).
    ///
    /// The same rule [`Self::readable_cache_policy`] states for the cache policy family.
    pub fn readable_response_headers_policy(
        &self,
        id: &str,
    ) -> Option<Cow<'_, StoredResponseHeadersPolicy>> {
        match self.response_headers_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => {
                managed_response_headers_policy(id).map(|managed| Cow::Owned(managed.to_stored()))
            }
        }
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

    /// The origin access control `id` names, or `None` when the store holds none.
    pub fn origin_access_control(&self, id: &str) -> Option<&StoredOriginAccessControl> {
        self.origin_access_controls.get(id)
    }

    /// The origin access control whose STORED configuration asks for exactly what
    /// `config` asks for, or `None` when no control does.
    ///
    /// `CreateOriginAccessControl` models `OriginAccessControlAlreadyExists` as "An origin
    /// access control with the specified parameters already exists", so what makes a
    /// create a conflict is the whole set of parameters rather than any single member: a
    /// request that changes any of them describes a DIFFERENT control and creates one.
    ///
    /// A scan over the controls rather than a second index beside them: the stored
    /// records are the single source of truth for which configurations exist, so a scan
    /// cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_ORIGIN_ACCESS_CONTROLS`].
    pub fn origin_access_control_with_config(
        &self,
        config: &OriginAccessControlConfig,
    ) -> Option<&StoredOriginAccessControl> {
        self.origin_access_controls
            .values()
            .find(|control| same_origin_access_control_config(&control.config, config))
    }

    /// The distribution that attaches the origin access control `id` to one of its
    /// origins, or `None` when no stored distribution does.
    ///
    /// `OriginAccessControlInUse` is documented as "Cannot delete the origin access
    /// control because it's in use by one or more distributions", and `Origin$
    /// OriginAccessControlId` is the only place a configuration names a control, so what
    /// makes a control in use is a stored origin naming it. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of
    /// step with what a read of those distributions reports; both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with its own bounded origin list).
    ///
    /// An empty `id` names no control — [`resolve_distribution_config`] stores the empty
    /// string for an origin that named none — so it never matches.
    pub fn distribution_using_origin_access_control(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        if id.is_empty() {
            return None;
        }
        self.distributions.values().find(|distribution| {
            distribution
                .config
                .origins
                .items
                .iter()
                .any(|origin| origin.origin_access_control_id.as_deref() == Some(id))
        })
    }

    /// The origin access control a configuration's origins name that this account does
    /// NOT hold, or `None` when every control they name exists.
    ///
    /// An origin reaches its bucket WITH the credentials of the origin access control the
    /// configuration names, so a configuration can only be stored while every control it
    /// names exists: the reference is resolved against the stored controls on the way IN,
    /// on every path that stores a `DistributionConfig`, and an id the store does not hold
    /// — never created, or created and since deleted — is refused with the modelled
    /// `InvalidOriginAccessControl` before anything is written. The check is the mirror of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_origin_access_control`],
    /// which is what keeps a control a stored origin names from being deleted, so the two
    /// together are why no stored origin can name a control that is gone.
    ///
    /// `Origin$OriginAccessControlId` is the only place a configuration names a control,
    /// and an empty id names none — [`resolve_distribution_config`] stores the empty string
    /// for an origin that named no control — so an absent or empty member is not a
    /// reference and resolves to nothing.
    pub fn unknown_origin_access_control<'config>(
        &self,
        config: &'config DistributionConfig,
    ) -> Option<&'config str> {
        config
            .origins
            .items
            .iter()
            .filter_map(|origin| origin.origin_access_control_id.as_deref())
            .filter(|id| !id.is_empty())
            .find(|id| self.origin_access_control(id).is_none())
    }

    /// The origin request policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_origin_request_policy`].
    pub fn origin_request_policy(&self, id: &str) -> Option<&StoredOriginRequestPolicy> {
        self.origin_request_policies.get(id)
    }

    /// The origin request policy `id` names wherever it came from: one this account created
    /// with `CreateOriginRequestPolicy`, or one AWS published
    /// ([`MANAGED_ORIGIN_REQUEST_POLICIES`]).
    ///
    /// The same rule [`Self::readable_cache_policy`] states for the cache policy family: an
    /// origin request policy id addresses ONE policy whichever of the two created it, so
    /// every read of a policy by id resolves both and an id outside both names no policy at
    /// all. The account's store is consulted first, so a record it holds is always what
    /// answers.
    pub fn readable_origin_request_policy(
        &self,
        id: &str,
    ) -> Option<Cow<'_, StoredOriginRequestPolicy>> {
        match self.origin_request_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => managed_origin_request_policy(id).map(|managed| Cow::Owned(managed.to_stored())),
        }
    }

    /// The distribution that attaches the origin request policy `id` to one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// `OriginRequestPolicyInUse` is documented as "Cannot delete the origin request policy
    /// because it is attached to one or more cache behaviors", and a behaviour attaches a
    /// policy through its `OriginRequestPolicyId` — so what makes a policy in use is a stored
    /// behaviour naming it, which is the rule [`references_origin_request_policy`] states for
    /// the whole configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for the other policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports; both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded behaviour list). A behaviour
    /// that attaches no policy carries no `OriginRequestPolicyId` at all rather than an empty
    /// one, so an id matches only a behaviour that really names it.
    pub fn distribution_using_origin_request_policy(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_origin_request_policy(&distribution.config, id))
    }

    /// The cache policy `id` names, or `None` when the store holds none.
    ///
    /// The ACCOUNT's own policies only — what a mutation addresses, since nothing in this
    /// account changes a policy AWS published. A READ addresses both, through
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::readable_cache_policy`].
    pub fn cache_policy(&self, id: &str) -> Option<&StoredCachePolicy> {
        self.cache_policies.get(id)
    }

    /// The cache policy `id` names wherever it came from: one this account created with
    /// `CreateCachePolicy`, or one AWS published ([`MANAGED_CACHE_POLICIES`]).
    ///
    /// A cache policy id addresses ONE policy whichever of the two created it — "The unique
    /// identifier for the cache policy" — so every read of a policy by id resolves both,
    /// and an id outside both names no policy at all. The account's store is consulted
    /// first, so a record it holds is always what answers.
    pub fn readable_cache_policy(&self, id: &str) -> Option<Cow<'_, StoredCachePolicy>> {
        match self.cache_policies.get(id) {
            Some(stored) => Some(Cow::Borrowed(stored)),
            None => managed_cache_policy(id).map(|managed| Cow::Owned(managed.to_stored())),
        }
    }

    /// The distribution that attaches the cache policy `id` to one of its cache
    /// behaviours, or `None` when no stored distribution does.
    ///
    /// `CachePolicyInUse` is documented as "Cannot delete the cache policy because it is
    /// attached to one or more cache behaviors", and a behaviour attaches a policy through
    /// its `CachePolicyId` — so what makes a policy in use is a stored behaviour naming it,
    /// which is the rule [`references_cache_policy`] states for the whole configuration.
    /// The scan reads the STORED configurations rather than a second index beside them, so
    /// it cannot drift out of step with what a read of those distributions reports; both
    /// collections are bounded ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded
    /// behaviour list). A behaviour that attaches no policy carries no `CachePolicyId` at
    /// all rather than an empty one, so an id matches only a behaviour that really names it.
    pub fn distribution_using_cache_policy(&self, id: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_cache_policy(&distribution.config, id))
    }

    /// The public key `id` names, or `None` when the store holds none.
    pub fn public_key(&self, id: &str) -> Option<&StoredPublicKey> {
        self.public_keys.get(id)
    }

    /// The public key uploaded under `caller_reference`, or `None` when no key was.
    ///
    /// `PublicKeyConfig$CallerReference` is "A string included in the request to help make
    /// sure that the request can't be replayed", so at most one key answers and this is
    /// what tells a replayed create from a second key under a reference already taken.
    ///
    /// A scan over the keys rather than a second index beside them: the stored records are
    /// the single source of truth for which references are taken, so a scan cannot drift
    /// out of step with them, and the collection is bounded by [`MAX_PUBLIC_KEYS`].
    pub fn public_key_by_caller_reference(&self, caller_reference: &str) -> Option<&StoredPublicKey> {
        self.public_keys
            .values()
            .find(|key| key.config.caller_reference == caller_reference)
    }

    /// The key group whose STORED configuration lists the public key `public_key_id` names,
    /// or `None` when no stored group does.
    ///
    /// "A key group contains a list of public keys that you can use with CloudFront signed
    /// URLs and signed cookies", and that list is `KeyGroupConfig$Items` — "A list of the
    /// identifiers of the public keys in the key group" — so what puts a public key in use is
    /// a stored group naming it, whichever position of whichever group's list names it. The
    /// SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for a policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those groups reports, and both collections are bounded
    /// ([`MAX_KEY_GROUPS`] groups, each with a bounded key list).
    pub fn key_group_using_public_key(&self, public_key_id: &str) -> Option<&StoredKeyGroup> {
        self.key_groups
            .values()
            .find(|group| group.config.items.iter().any(|id| id == public_key_id))
    }

    /// The field-level encryption profile whose STORED configuration names the public key
    /// `public_key_id` names in one of its encryption entities, or `None` when no stored
    /// profile does.
    ///
    /// An encryption entity names the key its matching fields are encrypted with through
    /// `EncryptionEntity$PublicKeyId` — "The public key associated with a set of field-level
    /// encryption patterns, to be used when encrypting the fields that match the patterns" —
    /// so what puts a public key in use is a stored profile naming it, which is the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::key_group_using_public_key`] states
    /// for the other way a stored record reaches a public key. The entity list is optional on
    /// the wire, so a profile carrying none names no key at all rather than an unknown one,
    /// and the scan reads the stored configurations rather than a second index beside them.
    pub fn field_level_encryption_profile_using_public_key(
        &self,
        public_key_id: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles.values().find(|stored| {
            stored
                .config
                .encryption_entities
                .items
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|entity| entity.public_key_id == public_key_id)
        })
    }

    /// The key group `id` names, or `None` when the store holds none.
    pub fn key_group(&self, id: &str) -> Option<&StoredKeyGroup> {
        self.key_groups.get(id)
    }

    /// The key group whose STORED configuration carries `name`, or `None`.
    ///
    /// `KeyGroupConfig$Name` is "A name to identify the key group", and the create models
    /// `KeyGroupAlreadyExists` for a name another group already holds — so at most one
    /// group answers, and this is what makes a repeated name a conflict rather than a
    /// second group. The comparison is against the stored configuration, never against a
    /// request, and the collection is bounded by [`MAX_KEY_GROUPS`].
    pub fn key_group_named(&self, name: &str) -> Option<&StoredKeyGroup> {
        self.key_groups
            .values()
            .find(|group| group.config.name == name)
    }

    /// The field-level encryption profile `id` names, or `None` when the store holds none.
    pub fn field_level_encryption_profile(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles.get(id)
    }

    /// The field-level encryption profile created under `caller_reference`, or `None` when
    /// no profile was.
    ///
    /// `FieldLevelEncryptionProfileConfig$CallerReference` is "A unique number that ensures
    /// that the request can't be replayed", so at most one profile answers and this is what
    /// tells a replayed create from a second profile under a reference already taken.
    ///
    /// A scan over the profiles rather than a second index beside them, for the same reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::public_key_by_caller_reference`]
    /// scans: the stored records are the single source of truth for which references are
    /// taken, so a scan cannot drift out of step with them, and the collection is bounded
    /// by [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`].
    pub fn field_level_encryption_profile_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        self.field_level_encryption_profiles
            .values()
            .find(|profile| profile.config.caller_reference == caller_reference)
    }

    /// The field-level encryption configuration `id` names, or `None` when the store holds
    /// none.
    pub fn field_level_encryption_config(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs.get(id)
    }

    /// The field-level encryption configuration created under `caller_reference`, or `None`
    /// when no configuration was.
    ///
    /// `FieldLevelEncryptionConfig$CallerReference` is "A unique number that ensures the
    /// request can't be replayed", so at most one configuration answers and this is what
    /// tells a replayed create from a second configuration under a reference already taken.
    ///
    /// A scan over the configurations rather than a second index beside them, for the same
    /// reason
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::field_level_encryption_profile_by_caller_reference`]
    /// scans: the stored records are the single source of truth for which references are
    /// taken, so a scan cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`].
    pub fn field_level_encryption_config_by_caller_reference(
        &self,
        caller_reference: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs
            .values()
            .find(|config| config.config.caller_reference == caller_reference)
    }

    /// The distribution that uses the field-level encryption configuration `id` names for
    /// one of its cache behaviours, or `None` when no stored distribution does.
    ///
    /// A cache behaviour names the configuration its matching fields are encrypted under
    /// through its `FieldLevelEncryptionId` — "the value of `ID` for the field-level
    /// encryption configuration that you want CloudFront to use for encrypting specific
    /// fields of data for this cache behavior" — so what puts a configuration in use is a
    /// stored behaviour naming it, which is the rule
    /// [`references_field_level_encryption_config`] states for the whole configuration. The
    /// SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_cache_policy`]
    /// states for a policy a cache behaviour attaches. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those distributions reports, and both collections are bounded
    /// ([`MAX_DISTRIBUTIONS`] distributions, each with a bounded behaviour list).
    pub fn distribution_using_field_level_encryption_config(
        &self,
        id: &str,
    ) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_field_level_encryption_config(&distribution.config, id))
    }

    /// The field-level encryption configuration that uses the profile `id` names in one of
    /// its mappings, or `None` when no stored configuration does.
    ///
    /// A configuration names the profile its matching fields are encrypted under through a
    /// mapping's `ProfileId` — `QueryArgProfile$ProfileId` is "ID of profile to use for
    /// field-level encryption query argument-profile mapping" and
    /// `ContentTypeProfile$ProfileId` is "The profile ID for a field-level encryption content
    /// type-profile mapping" — so what puts a profile in use is a stored configuration naming
    /// it, which is the rule [`references_field_level_encryption_profile`] states for the whole
    /// configuration. The SAME rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::distribution_using_field_level_encryption_config`]
    /// states for the configuration a cache behaviour names. The scan reads the STORED
    /// configurations rather than a second index beside them, so it cannot drift out of step
    /// with what a read of those configurations reports, and both mapping lists are bounded
    /// ([`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`] configurations, each with bounded mapping
    /// lists).
    pub fn field_level_encryption_config_using_profile(
        &self,
        id: &str,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        self.field_level_encryption_configs
            .values()
            .find(|stored| references_field_level_encryption_profile(&stored.config, id))
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
        if let Some(store) = self.key_value_store_by_arn(arn) {
            return Some(&store.tags);
        }
        if let Some(group) = self
            .connection_groups
            .values()
            .find(|group| group.arn == arn)
        {
            return Some(&group.tags);
        }
        if let Some(function) = self.function_by_arn(arn) {
            return Some(&function.tags);
        }
        if let Some(function) = self.connection_function_by_arn(arn) {
            return Some(&function.tags);
        }
        if let Some(distribution) = self
            .streaming_distributions
            .values()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&distribution.tags);
        }
        if let Some(list) = self.anycast_ip_lists.values().find(|list| list.arn == arn) {
            return Some(&list.tags);
        }
        self.distribution_tenants
            .values()
            .find(|tenant| tenant.arn == arn)
            .map(|tenant| &tenant.tags)
    }

    /// Whether this store holds the CloudFront resource `arn` names, whatever its type.
    ///
    /// The question an operation that addresses a resource by ARN has to answer before it
    /// can say anything about that resource — so it is resolved the way every other ARN
    /// lookup here resolves one, against the STORED record rather than by re-deriving an
    /// identifier out of the request:
    ///
    /// * a kind that mints an ARN of its own is matched on the ARN it carries, the very
    ///   comparison [`Self::tags_for_resource`] makes, so no two lookups can disagree about
    ///   which resource an ARN names;
    /// * a kind that mints none is matched on the id its own routes address it by, read out
    ///   of the resource part of an ARN of this service ([`cloudfront_arn_resource`]). A
    ///   policy family also answers for the policies AWS publishes, since an id addresses
    ///   one policy whichever of the two created it.
    ///
    /// `false` is "this emulator holds no such resource", whatever the ARN's type: a type
    /// this store holds nothing of at all, another service's ARN, and an ARN carrying no
    /// resource part all answer it.
    pub fn holds_resource(&self, arn: &str) -> bool {
        if self.tags_for_resource(arn).is_some() || self.realtime_log_config_by_arn(arn).is_some() {
            return true;
        }
        let Some((kind, id)) = cloudfront_arn_resource(arn) else {
            return false;
        };
        match kind {
            "cache-policy" => self.readable_cache_policy(id).is_some(),
            "continuous-deployment-policy" => self.continuous_deployment_policies.contains_key(id),
            "field-level-encryption-config" => self.field_level_encryption_configs.contains_key(id),
            "field-level-encryption-profile" => {
                self.field_level_encryption_profiles.contains_key(id)
            }
            "key-group" => self.key_groups.contains_key(id),
            "origin-access-control" => self.origin_access_controls.contains_key(id),
            "origin-access-identity" => self.origin_access_identities.contains_key(id),
            "origin-request-policy" => self.readable_origin_request_policy(id).is_some(),
            "public-key" => self.public_keys.contains_key(id),
            "response-headers-policy" => self.readable_response_headers_policy(id).is_some(),
            _ => false,
        }
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
        if let Some(store) = self
            .key_value_stores
            .values_mut()
            .find(|store| store.arn == arn)
        {
            return Some(&mut store.tags);
        }
        if let Some(group) = self
            .connection_groups
            .values_mut()
            .find(|group| group.arn == arn)
        {
            return Some(&mut group.tags);
        }
        // The FIELD rather than the [`Self::function_by_arn`] lookup the tag READ goes
        // through: a method taking `&mut self` would borrow the whole store, which the
        // arms above already borrow a field of. The comparison is the same one.
        if let Some(function) = self
            .functions
            .values_mut()
            .find(|function| function.arn == arn)
        {
            return Some(&mut function.tags);
        }
        // The connection function arm, resolved against the STORED ARN exactly as the tag
        // read resolves it, and against the field for the same borrow reason as above.
        if let Some(function) = self
            .connection_functions
            .values_mut()
            .find(|function| function.arn == arn)
        {
            return Some(&mut function.tags);
        }
        // The streaming distribution arm, resolved against the STORED ARN exactly as the
        // tag read resolves it, and against the field for the same borrow reason as above.
        if let Some(distribution) = self
            .streaming_distributions
            .values_mut()
            .find(|distribution| distribution.arn == arn)
        {
            return Some(&mut distribution.tags);
        }
        // The Anycast static IP list arm: a list carries the tags its create submitted, so
        // it is one of the resource kinds a tagging call addresses by ARN.
        if let Some(list) = self
            .anycast_ip_lists
            .values_mut()
            .find(|list| list.arn == arn)
        {
            return Some(&mut list.tags);
        }
        // The distribution tenant arm, so a tagging call and a tag read agree about every
        // resource kind this store holds and tags.
        self.distribution_tenants
            .values_mut()
            .find(|tenant| tenant.arn == arn)
            .map(|tenant| &mut tenant.tags)
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

        // An alternate domain name is served only under a certificate that validates it,
        // so a configuration whose aliases and viewer certificate disagree is refused
        // before any distribution exists to carry the alias.
        if let Some(message) = alias_certificate_mismatch(&config) {
            return Err(CreateRefusal::InvalidViewerCertificate(message));
        }

        // A Lambda@Edge association invokes ONE published version of a function, so a
        // configuration whose association names no version is refused before any
        // distribution is created carrying it.
        if let Some(message) = lambda_association_without_version(&config) {
            return Err(CreateRefusal::InvalidLambdaFunctionAssociation(message));
        }

        // An origin reaches a private bucket EITHER with the credentials of an origin
        // access control OR as an origin access identity, never both: a configuration whose
        // origin names both says nothing about which signs it, so it is refused before any
        // distribution is created from it.
        if origin_with_both_access_mechanisms(&config) {
            return Err(CreateRefusal::IllegalOriginAccessConfiguration(
                ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE.to_string(),
            ));
        }

        // An origin signs its requests to the bucket with the credentials of the origin
        // access control it names, so a configuration can only be stored while every
        // control it names exists: a reference to one this account does not hold is
        // refused before any distribution is created from it.
        if self.unknown_origin_access_control(&config).is_some() {
            return Err(CreateRefusal::InvalidOriginAccessControl(
                INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string(),
            ));
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
                        distribution: existing.to_distribution(&self.key_groups),
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
            // Additional CloudWatch metrics are enabled by a call of their own, so a
            // distribution this create minted carries no monitoring subscription until
            // `CreateMonitoringSubscription` puts one on it.
            monitoring_subscription: None,
            // Nothing is cached at the edge until the edge serves a request.
            edge_cache: BTreeMap::new(),
        };
        let created = CreatedDistribution {
            id: id.clone(),
            etag,
            distribution: stored.to_distribution(&self.key_groups),
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

    /// Allocate a streaming distribution id no stored streaming distribution holds.
    ///
    /// `StreamingDistribution$Id` is "The identifier for the RTMP distribution. For
    /// example: `EGTXBD79EXAMPLE`": the same shape a distribution id takes (`E` +
    /// [`ID_BODY_LEN`] uppercase alphanumerics), drawn from the same mint, so no two
    /// streaming distributions are minted the same id.
    pub fn allocate_streaming_distribution_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.streaming_distributions.contains_key(&candidate) {
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

    /// Allocate an Anycast static IP list id no stored list holds.
    ///
    /// `AnycastIpList$Id` is "The ID of the Anycast static IP list", and the model puts no
    /// pattern on it, so the shape is the one CloudFront writes such an id in:
    /// [`ANYCAST_IP_LIST_ID_PREFIX`] followed by [`ANYCAST_IP_LIST_ID_HEX_LEN`] lowercase
    /// hex digits. It is rendered from ONE draw of the mint, zero-padded to that width, so
    /// distinct draws render distinctly and no two lists are minted the same id.
    pub fn allocate_anycast_ip_list_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{ANYCAST_IP_LIST_ID_PREFIX}{:0width$x}",
                self.mint_u64(),
                width = ANYCAST_IP_LIST_ID_HEX_LEN
            );
            if !self.anycast_ip_lists.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate `ip_count` Anycast static IP address SLOTS, or `None` when the pool cannot
    /// serve that many more.
    ///
    /// "The static IP addresses that are allocated to the Anycast static IP list": the
    /// slots are allocated ONCE, here, and the list holds exactly as many of them as its
    /// `IpCount` — "The number of IP addresses in the Anycast static IP list". Which
    /// FAMILY each slot is reported in follows the list's address type and is decided where
    /// the list is read ([`anycast_ips_of`]), so a list that changes its address type keeps
    /// the allocation it already holds.
    ///
    /// Each slot comes off [`ComAmazonawsCloudfrontCloudfront20200531State::next_anycast_ip`],
    /// so no address is allocated to two lists; `None` once that counter has walked the
    /// whole of [`ANYCAST_IP_POOL`], which the caller reports as the modelled
    /// `EntityLimitExceeded`.
    ///
    /// The count a caller asks for never sizes the allocation on its own: the pool check
    /// below runs BEFORE anything is allocated, so the loop that follows runs at most
    /// [`ANYCAST_IP_POOL`] times whatever the request named, and a count that does not even
    /// fit the counter is refused there rather than allocated for.
    pub fn allocate_anycast_ip_hosts(&mut self, ip_count: i32) -> Option<Vec<u32>> {
        let wanted = u32::try_from(ip_count).ok()?;
        if self.next_anycast_ip.checked_add(wanted)? > ANYCAST_IP_POOL {
            return None;
        }
        let hosts = (0..wanted)
            .map(|_| {
                self.next_anycast_ip += 1;
                self.next_anycast_ip
            })
            .collect();
        Some(hosts)
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

    /// Allocate an origin access control id no stored control holds.
    ///
    /// `OriginAccessControl$Id` is "The unique identifier of the origin access control",
    /// and CloudFront mints one in the shape every CloudFront entity id takes (`E` +
    /// [`ID_BODY_LEN`] uppercase alphanumerics, as `--origin-access-control-id
    /// E127EXAMPLE51Z` shows), drawn from the same mint as every other id here so no two
    /// controls are minted the same id.
    pub fn allocate_origin_access_control_id(&mut self) -> String {
        loop {
            let candidate = format!("E{}", self.mint(ID_BODY_LEN));
            if !self.origin_access_controls.contains_key(&candidate) {
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

    /// Allocate a public key id no stored public key holds.
    ///
    /// `PublicKey$Id` is "The identifier of the public key", and CloudFront mints one in
    /// the shape its own answers show (`K2K8NC4HVFE3MO`): a `K` followed by
    /// [`ID_BODY_LEN`] uppercase alphanumerics, the id alphabet every other entity id here
    /// is drawn from — so no two keys are minted the same id and the namespace is
    /// distinguishable from a distribution's `E`-prefixed one.
    pub fn allocate_public_key_id(&mut self) -> String {
        loop {
            let candidate = format!("{PUBLIC_KEY_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.public_keys.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a key group id no stored key group holds.
    ///
    /// `KeyGroup$Id` is "The identifier for the key group", and CloudFront mints a UUID for
    /// one (the shape its own `create-key-group` answers show, the same one its cache,
    /// origin request and response headers policy ids take), so this renders one draw of
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::mint_uuid`] — distinct draws render
    /// distinctly, so the mint never repeats an id.
    pub fn allocate_key_group_id(&mut self) -> String {
        loop {
            let candidate = self.mint_uuid();
            if !self.key_groups.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a field-level encryption profile id no stored profile holds.
    ///
    /// `FieldLevelEncryptionProfile$Id` is "The ID for a field-level encryption profile
    /// configuration", and CloudFront mints one in the shape its own answers show
    /// (`P280MFCLSYOCVU`): a [`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`] followed by
    /// [`ID_BODY_LEN`] uppercase alphanumerics, the id alphabet every other entity id here
    /// is drawn from — so no two profiles are minted the same id and the namespace is
    /// distinguishable from a distribution's `E`-prefixed one.
    pub fn allocate_field_level_encryption_profile_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX}{}",
                self.mint(ID_BODY_LEN)
            );
            if !self.field_level_encryption_profiles.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Allocate a field-level encryption configuration id no stored configuration holds.
    ///
    /// `FieldLevelEncryption$Id` is "The configuration ID for a field-level encryption
    /// configuration", and CloudFront mints one in the shape its own answers show: a
    /// [`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`] followed by [`ID_BODY_LEN`] uppercase
    /// alphanumerics, the id alphabet every other entity id here is drawn from — so no two
    /// configurations are minted the same id and the namespace is distinguishable from the
    /// profile's `P`-prefixed one.
    pub fn allocate_field_level_encryption_config_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX}{}",
                self.mint(ID_BODY_LEN)
            );
            if !self.field_level_encryption_configs.contains_key(&candidate) {
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

    /// Allocate a connection group id no stored connection group holds:
    /// [`CONNECTION_GROUP_ID_PREFIX`] followed by a minted token.
    pub fn allocate_connection_group_id(&mut self) -> String {
        loop {
            let candidate = format!("{CONNECTION_GROUP_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.connection_groups.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// The id of the account's default connection group, creating the group on the first
    /// call that needs one — which is what "CloudFront will automatically create a default
    /// connection group for you" means. Every later call reads that one id back, and the
    /// record it names is what a read and a listing of that group answer from.
    ///
    /// The group this creates is the only one carrying
    /// [`StoredConnectionGroup::is_default`], and it is created exactly as
    /// `CreateConnectionGroup` creates one: a minted id, a minted routing endpoint and a
    /// minted version token, `InProgress` until the clock owes it `Deployed`.
    pub fn default_connection_group(&mut self, now: SystemTime) -> String {
        if let Some(group) = self.connection_groups.values().find(|group| group.is_default) {
            return group.id.clone();
        }
        let id = self.allocate_connection_group_id();
        // The automatic group's name is [`DEFAULT_CONNECTION_GROUP_NAME`] unless a create
        // already took it: a name identifies ONE group in the account, so the fallback is
        // derived from this group's own minted id, which no other group holds.
        let name = match self.connection_group_named(DEFAULT_CONNECTION_GROUP_NAME) {
            None => DEFAULT_CONNECTION_GROUP_NAME.to_string(),
            Some(_) => format!("{DEFAULT_CONNECTION_GROUP_NAME}-{id}"),
        };
        let routing_endpoint = self.allocate_domain_name();
        let etag = self.next_etag();
        let stored = StoredConnectionGroup {
            arn: connection_group_arn(&id),
            id: id.clone(),
            name,
            routing_endpoint,
            // Nothing asked for an Anycast static IP list, so the automatic group names
            // none; every other member takes the same default a create that named none
            // would get.
            anycast_ip_list_id: None,
            ipv6_enabled: DEFAULT_CONNECTION_GROUP_IPV6_ENABLED,
            enabled: DEFAULT_CONNECTION_GROUP_ENABLED,
            is_default: true,
            status: STATUS_IN_PROGRESS.to_string(),
            deployed_due: now + PROPAGATION_DELAY,
            created_time: now,
            last_modified_time: now,
            etag,
            tags: TagSet::new(),
        };
        self.connection_groups.insert(id.clone(), stored);
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

    /// Mint the CloudFront managed ACM certificate a `ManagedCertificateRequest` asks
    /// for, covering `domains`.
    ///
    /// The identity of the certificate — its ARN and the validation token of each domain
    /// — is drawn from the one mint every other generated value here comes from, so no
    /// two certificates and no two tokens are minted the same. The certificate starts
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] and the CLOCK owes it its issuance, so
    /// nothing a later call does decides when it is issued.
    pub fn request_managed_certificate(
        &mut self,
        request: ManagedCertificateRequest,
        domains: &[String],
        now: SystemTime,
    ) -> StoredManagedCertificate {
        let arn = managed_certificate_arn(&self.mint_uuid());
        let validation_tokens = domains
            .iter()
            .map(|domain| StoredValidationToken {
                domain: domain.clone(),
                token: self.mint_validation_token(),
            })
            .collect();
        StoredManagedCertificate {
            arn,
            status: CERTIFICATE_STATUS_PENDING_VALIDATION.to_string(),
            issued_due: now + CERTIFICATE_VALIDATION_DELAY,
            request,
            validation_tokens,
        }
    }

    /// One fresh HTTP validation token name: two draws of the mint (128 bits) rendered as
    /// the lowercase hex an opaque token file name is written in.
    fn mint_validation_token(&mut self) -> String {
        format!("{:016x}{:016x}", self.mint_u64(), self.mint_u64())
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
    ///
    /// A connection group's `RoutingEndpoint` is "also known as the DNS name ... such as
    /// `d111111abcdef8.cloudfront.net`" — the same generated name in the same shape, so it
    /// is drawn from here too rather than from a second rendering of the mint.
    pub fn allocate_domain_name(&mut self) -> String {
        let label = self.mint(ID_BODY_LEN).to_lowercase();
        format!("d{label}.cloudfront.net")
    }

    /// Allocate an invalidation id no invalidation of the named distribution tenant holds.
    pub fn allocate_tenant_invalidation_id(&mut self, tenant_id: &str) -> String {
        loop {
            let candidate = format!("I{}", self.mint(ID_BODY_LEN));
            let taken = self
                .distribution_tenants
                .get(tenant_id)
                .is_some_and(|tenant| tenant.invalidations.contains_key(&candidate));
            if !taken {
                break candidate;
            }
        }
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

    /// Mint the identifier the edge stamps on one request it serves.
    ///
    /// `X-Amz-Cf-Id` "identifies the request that you're interested in" — one opaque
    /// token per request, so it is drawn fresh from the same counter every other
    /// generated value comes from and is never derived from the request. Rendered as 32
    /// lowercase hex characters (two draws, 128 bits): an opaque token, which is all a
    /// client may read it as.
    pub fn mint_edge_request_id(&mut self) -> String {
        format!("{:016x}{:016x}", self.mint_u64(), self.mint_u64())
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

    /// The `Distribution` payload of the record `id` names, beside the version token that
    /// record carries, or `None` when the store holds no such distribution.
    ///
    /// ONE place a `Distribution` payload is built from this store, because the projection
    /// reads TWO parts of it: the distribution record itself and the account's key groups,
    /// whose public keys the active key group set reports
    /// ([`StoredDistribution::active_trusted_key_groups`]). Every operation that answers
    /// with a `Distribution` goes through it, so they cannot report different things for
    /// the same state.
    pub fn distribution_payload(&self, id: &str) -> Option<(Distribution, String)> {
        let stored = self.distributions.get(id)?;
        Some((stored.to_distribution(&self.key_groups), stored.etag.clone()))
    }

    /// Set or clear the WAF web ACL customization of the distribution tenant `id` names,
    /// and report the record as it now stands.
    ///
    /// `Customizations$WebAcl` is "the WAF web ACL" a tenant overrides or disables, and it
    /// is the ONE member a web ACL association of a tenant writes: `Some` stores the
    /// customization, `None` takes it off the record, and a customization set that is then
    /// empty is dropped altogether rather than reported as an element describing nothing.
    /// The tenant's other customizations are carried over untouched, because the
    /// association says nothing about them.
    ///
    /// The write versions the tenant exactly as any other change to its configuration
    /// does — `LastModifiedTime` becomes `now`, a fresh version token is minted so the
    /// token the caller matched on no longer satisfies an `If-Match`, and the tenant goes
    /// back to [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the new
    /// configuration has to reach the edge locations before it is deployed again.
    ///
    /// `None` when the store holds no such tenant, checked BEFORE the mint so a version
    /// token is never spent on a tenant that is not there.
    pub fn apply_tenant_web_acl(
        &mut self,
        id: &str,
        web_acl: Option<WebAclCustomization>,
        now: SystemTime,
    ) -> Option<&StoredDistributionTenant> {
        if !self.distribution_tenants.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let tenant = self.distribution_tenants.get_mut(id)?;
        let mut customizations = tenant.customizations.take().unwrap_or_default();
        customizations.web_acl = web_acl;
        tenant.customizations = retained_customizations(customizations);
        tenant.last_modified_time = now;
        tenant.etag = etag;
        tenant.status = STATUS_IN_PROGRESS.to_string();
        tenant.deployed_due = now + PROPAGATION_DELAY;
        Some(tenant)
    }

    /// Apply an `UpdateDistributionTenant` to the tenant `id` names, and report the record
    /// as it now stands.
    ///
    /// Every member the update submits is optional and each is documented as what the
    /// tenant is updated TO ("the domains to update", "updated to an enabled state"), so a
    /// member the request leaves out is left exactly as it was and one it states REPLACES
    /// what the record held — the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_anycast_ip_list_update`]
    /// follows. A stated customization set that names nothing is retained as none at all,
    /// so the record never carries an element describing nothing.
    ///
    /// A `ManagedCertificateRequest` on the update MINTS a certificate, over the domains
    /// the tenant holds once the update has landed, and replaces whatever certificate the
    /// record carried; an update that asks for none leaves the existing one alone.
    ///
    /// The write versions the tenant exactly as every other change to its configuration
    /// does: `LastModifiedTime` becomes `now`, a fresh version token is minted so the token
    /// the caller matched on no longer satisfies an `If-Match`, and the tenant goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the configuration it
    /// now runs under has to reach the edge locations before it is deployed again. The
    /// tenant's identity — its id, its ARN, its name and its creation time — is untouched:
    /// an update submits none of them, and "[the name] can't be updated after you create the
    /// distribution tenant".
    ///
    /// `None` when the store holds no such tenant, checked BEFORE the mint so a version
    /// token is never spent on a tenant that is not there.
    pub fn apply_distribution_tenant_update(
        &mut self,
        id: &str,
        update: DistributionTenantUpdate,
        now: SystemTime,
    ) -> Option<&StoredDistributionTenant> {
        if !self.distribution_tenants.contains_key(id) {
            return None;
        }
        let DistributionTenantUpdate {
            distribution_id,
            domains,
            customizations,
            parameters,
            connection_group_id,
            enabled,
            managed_certificate_request,
        } = update;
        // The certificate covers the domains the tenant holds AFTER this update, so it is
        // minted from the effective list rather than from the one the record carried.
        let certificate_domains = domains.clone().unwrap_or_else(|| {
            self.distribution_tenants
                .get(id)
                .map(|tenant| tenant.domains.clone())
                .unwrap_or_default()
        });
        let etag = self.next_etag();
        let managed_certificate = managed_certificate_request
            .map(|request| self.request_managed_certificate(request, &certificate_domains, now));
        let tenant = self.distribution_tenants.get_mut(id)?;
        if let Some(distribution_id) = distribution_id {
            tenant.distribution_id = distribution_id;
        }
        if let Some(domains) = domains {
            tenant.domains = domains;
        }
        if let Some(customizations) = customizations {
            tenant.customizations = retained_customizations(customizations);
        }
        if let Some(parameters) = parameters {
            tenant.parameters = parameters;
        }
        if let Some(connection_group_id) = connection_group_id {
            tenant.connection_group_id = connection_group_id;
        }
        if let Some(enabled) = enabled {
            tenant.enabled = enabled;
        }
        if let Some(certificate) = managed_certificate {
            tenant.managed_certificate = Some(certificate);
        }
        tenant.last_modified_time = now;
        tenant.etag = etag;
        tenant.status = STATUS_IN_PROGRESS.to_string();
        tenant.deployed_due = now + PROPAGATION_DELAY;
        Some(tenant)
    }

    /// Replace the configuration of the cache policy `id` names, and report the record as
    /// it now stands.
    ///
    /// `UpdateCachePolicy` replaces the configuration WHOLE — "When you update a cache
    /// policy configuration, all the fields are updated with the values provided in the
    /// request. You cannot update some fields independent of others." — so the stored
    /// configuration becomes the one submitted, down to the members the request left out;
    /// `LastModifiedTime` becomes `now` ("the date and time when the cache policy was last
    /// modified"), and the version token is freshly minted, so the token the caller matched
    /// on no longer satisfies an `If-Match` against this policy. Nothing else about a cache
    /// policy changes: the id is the one it was created under, and a policy carries no
    /// status and no propagation deadline.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_cache_policy_config(
        &mut self,
        id: &str,
        config: CachePolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredCachePolicy> {
        if !self.cache_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.cache_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin request policy `id` names, and report the
    /// record as it now stands.
    ///
    /// `UpdateOriginRequestPolicy` replaces the configuration WHOLE — "When you update an
    /// origin request policy configuration, all the fields are updated with the values
    /// provided in the request. You cannot update some fields independent of others." — so
    /// the stored configuration becomes the one submitted, down to the members the request
    /// left out; `LastModifiedTime` becomes `now` ("the date and time when the origin
    /// request policy was last modified"), and the version token is freshly minted, so the
    /// token the caller matched on no longer satisfies an `If-Match` against this policy.
    /// Nothing else about an origin request policy changes: the id is the one it was created
    /// under, and a policy carries no status and no propagation deadline.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_origin_request_policy_config(
        &mut self,
        id: &str,
        config: OriginRequestPolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredOriginRequestPolicy> {
        if !self.origin_request_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_request_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin access control `id` names, and report the
    /// record as it now stands.
    ///
    /// `UpdateOriginAccessControl` submits a whole `OriginAccessControlConfig` as its
    /// payload, so the stored configuration becomes the one submitted, down to the members
    /// the request left out, and the version token is freshly minted — "the new version of
    /// the origin access control after it has been updated" — so the token the caller
    /// matched on no longer satisfies an `If-Match` against this control. Nothing else
    /// about a control changes: the id is the one it was created under, and a control
    /// carries no status, no last-modified time and no propagation deadline.
    ///
    /// `None` when the store holds no such control, checked BEFORE the mint so a version
    /// token is never spent on a control that is not there.
    pub fn apply_origin_access_control_config(
        &mut self,
        id: &str,
        config: OriginAccessControlConfig,
    ) -> Option<&StoredOriginAccessControl> {
        if !self.origin_access_controls.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_access_controls.get_mut(id)?;
        stored.config = config;
        stored.etag = etag;
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

    /// Apply an `UpdateAnycastIpList` to the list `id` names, and report the record as it
    /// now stands.
    ///
    /// `IpAddressType` ("The IP address type for the Anycast static IP list") and
    /// `IpamCidrConfigs` ("A list of IPAM CIDR configurations … for updating the Anycast
    /// static IP list") are the only members the update submits, and each is optional, so a
    /// member the request leaves out is left exactly as it was. What the update changes:
    ///
    /// * the address type becomes the one submitted, and the list's own address SLOTS are
    ///   re-reported in the families that type admits ([`anycast_ips_of`]) — the allocation
    ///   is the list's for its whole life, so nothing more is taken out of the pool and no
    ///   address can land on two lists;
    /// * the IPAM CIDR configurations become the ones submitted, each carrying the status
    ///   the SERVICE puts on a range it has just been asked for, which the clock then owes
    ///   the provisioned one;
    /// * the list re-enters the transient status, since the addresses and ranges it now
    ///   runs under have to reach the edge locations before they are in effect, and the
    ///   CLOCK owes it the terminal status again;
    /// * `LastModifiedTime` becomes `now` and the version token is freshly minted, so the
    ///   token the caller matched on no longer satisfies an `If-Match` against this list.
    ///
    /// The list's identity (id, ARN, name), its `IpCount` and its tag set are untouched: an
    /// update submits none of them.
    ///
    /// `None` when the store holds no such list, checked BEFORE the mint so a version token
    /// is never spent on a list that is not there.
    pub fn apply_anycast_ip_list_update(
        &mut self,
        id: &str,
        ip_address_type: Option<String>,
        ipam_cidr_configs: Option<Vec<IpamCidrConfig>>,
        now: SystemTime,
    ) -> Option<&StoredAnycastIpList> {
        if !self.anycast_ip_lists.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.anycast_ip_lists.get_mut(id)?;
        if let Some(ip_address_type) = ip_address_type {
            stored.ip_address_type = ip_address_type;
        }
        if let Some(configs) = ipam_cidr_configs {
            stored.ipam_cidr_configs = configs
                .into_iter()
                .map(|config| IpamCidrConfig {
                    status: Some(IPAM_CIDR_STATUS_PROVISIONING.to_string()),
                    ..config
                })
                .collect();
        }
        stored.status = ANYCAST_IP_LIST_STATUS_DEPLOYING.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Apply an update to the connection group `id` names, and report the record as it now
    /// stands.
    ///
    /// Every member `UpdateConnectionGroup` submits beside the group's id is optional and
    /// is documented as what the group is updated TO ("whether the connection group is
    /// enabled", "enable IPv6 for the connection group", "the ID of the Anycast static IP
    /// list"), so a member the request leaves out is left exactly as it was and one it
    /// states REPLACES what the record held — the same rule
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_anycast_ip_list_update`] and
    /// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_distribution_tenant_update`]
    /// follow.
    ///
    /// The write versions the group exactly as every other change to it does:
    /// `LastModifiedTime` becomes `now`, a fresh version token is minted so the token the
    /// caller matched on no longer satisfies an `If-Match`, and the group goes back to
    /// [`STATUS_IN_PROGRESS`] with a new propagation deadline, since the settings it now
    /// runs under have to reach the edge locations before they are in effect. The group's
    /// identity — its id, its ARN, its name, its routing endpoint, its creation time and
    /// whether it is the account's default one — is untouched: an update submits none of
    /// them, and the name "can't be updated after you create the connection group".
    ///
    /// `None` when the store holds no such group, checked BEFORE the mint so a version
    /// token is never spent on a group that is not there.
    pub fn apply_connection_group_update(
        &mut self,
        id: &str,
        anycast_ip_list_id: Option<String>,
        enabled: Option<bool>,
        ipv6_enabled: Option<bool>,
        now: SystemTime,
    ) -> Option<&StoredConnectionGroup> {
        if !self.connection_groups.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.connection_groups.get_mut(id)?;
        if let Some(anycast_ip_list_id) = anycast_ip_list_id {
            stored.anycast_ip_list_id = Some(anycast_ip_list_id);
        }
        if let Some(enabled) = enabled {
            stored.enabled = enabled;
        }
        if let Some(ipv6_enabled) = ipv6_enabled {
            stored.ipv6_enabled = ipv6_enabled;
        }
        stored.status = STATUS_IN_PROGRESS.to_string();
        stored.deployed_due = now + PROPAGATION_DELAY;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the origin access identity `id` names, and report the
    /// record as it now stands.
    ///
    /// The submitted configuration becomes the stored one WHOLE, and the version token is
    /// freshly minted, so the token the caller matched on no longer satisfies an
    /// `If-Match` against this identity. Nothing else about an identity changes: the id it
    /// was created under and the Amazon S3 canonical user id minted with it are fixed for
    /// its whole life, and an identity carries neither a status nor a propagation deadline,
    /// so the clock owes it nothing.
    ///
    /// `None` when the store holds no such identity, checked BEFORE the mint so a version
    /// token is never spent on an identity that is not there.
    pub fn apply_origin_access_identity_config(
        &mut self,
        id: &str,
        config: CloudFrontOriginAccessIdentityConfig,
    ) -> Option<&StoredOriginAccessIdentity> {
        if !self.origin_access_identities.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.origin_access_identities.get_mut(id)?;
        stored.config = config;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the public key `id` names, and report the record as it
    /// now stands.
    ///
    /// `UpdatePublicKey` submits a whole `PublicKeyConfig` as its payload, so the stored
    /// configuration becomes the submitted one and the version token is freshly minted, so
    /// the token the caller matched on no longer satisfies an `If-Match` against this key.
    /// Nothing else about a key changes: the id it was uploaded under and the instant it was
    /// uploaded are fixed for its whole life, and a key carries neither a status nor a
    /// propagation deadline, so the clock owes it nothing. WHICH members an update may move
    /// is the separate rule [`immutable_public_key_change`] states, decided by the caller
    /// before it reaches here.
    ///
    /// `None` when the store holds no such key, checked BEFORE the mint so a version token
    /// is never spent on a key that is not there.
    pub fn apply_public_key_config(
        &mut self,
        id: &str,
        config: PublicKeyConfig,
    ) -> Option<&StoredPublicKey> {
        if !self.public_keys.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.public_keys.get_mut(id)?;
        stored.config = config;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the key group `id` names, and report the record as it
    /// now stands.
    ///
    /// `UpdateKeyGroup` replaces the configuration WHOLE — "When you update a key group, all
    /// the fields are updated with the values provided in the request. You cannot update some
    /// fields independent of others." — so the stored configuration becomes the one
    /// submitted, down to the members the request left out; `LastModifiedTime` becomes `now`
    /// ("the date and time when the key group was last modified"), and the version token is
    /// freshly minted, so the token the caller matched on no longer satisfies an `If-Match`
    /// against this group. Nothing else about a key group changes: the id is the one it was
    /// created under, and a group carries no status and no propagation deadline.
    ///
    /// `None` when the store holds no such key group, checked BEFORE the mint so a version
    /// token is never spent on a group that is not there.
    pub fn apply_key_group_config(
        &mut self,
        id: &str,
        config: KeyGroupConfig,
        now: SystemTime,
    ) -> Option<&StoredKeyGroup> {
        if !self.key_groups.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.key_groups.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the continuous deployment policy `id` names, and
    /// report the record as it now stands.
    ///
    /// The submitted configuration becomes the stored one WHOLE, `LastModifiedTime`
    /// becomes `now` — "the date and time the policy was last modified" — and the version
    /// token is freshly minted, so the token the caller matched on no longer satisfies an
    /// `If-Match` against this policy. Nothing else about a policy changes: the id is the
    /// one it was created under, and a policy carries neither a status nor a propagation
    /// deadline, so the clock owes it nothing.
    ///
    /// `None` when the store holds no such policy, checked BEFORE the mint so a version
    /// token is never spent on a policy that is not there.
    pub fn apply_continuous_deployment_policy_config(
        &mut self,
        id: &str,
        config: ContinuousDeploymentPolicyConfig,
        now: SystemTime,
    ) -> Option<&StoredContinuousDeploymentPolicy> {
        if !self.continuous_deployment_policies.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.continuous_deployment_policies.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the field-level encryption profile `id` names, and
    /// report the record as it now stands.
    ///
    /// `UpdateFieldLevelEncryptionProfile` submits a whole `FieldLevelEncryptionProfileConfig`
    /// as its payload, so the stored configuration becomes the submitted one WHOLE,
    /// `LastModifiedTime` becomes `now` — "the last time the field-level encryption profile
    /// was updated" — and the version token is freshly minted, so the token the caller matched
    /// on no longer satisfies an `If-Match` against this profile. Nothing else about a profile
    /// changes: the id is the one the create minted, and a profile carries neither a status nor
    /// a propagation deadline, so the clock owes it nothing. WHICH members an update may move
    /// is the caller's rule, decided before it reaches here.
    ///
    /// `None` when the store holds no such profile, checked BEFORE the mint so a version
    /// token is never spent on a profile that is not there.
    pub fn apply_field_level_encryption_profile_config(
        &mut self,
        id: &str,
        config: FieldLevelEncryptionProfileConfig,
        now: SystemTime,
    ) -> Option<&StoredFieldLevelEncryptionProfile> {
        if !self.field_level_encryption_profiles.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.field_level_encryption_profiles.get_mut(id)?;
        stored.config = config;
        stored.last_modified_time = now;
        stored.etag = etag;
        Some(stored)
    }

    /// Replace the configuration of the field-level encryption CONFIGURATION `id` names, and
    /// report the record as it now stands.
    ///
    /// The same write [`Self::apply_field_level_encryption_profile_config`] performs for the
    /// profile, on the configuration record: `UpdateFieldLevelEncryptionConfig` submits a whole
    /// `FieldLevelEncryptionConfig` as its payload, so the stored configuration becomes the
    /// submitted one WHOLE, `LastModifiedTime` becomes `now` — "the last time the field-level
    /// encryption configuration was changed" — and the version token is freshly minted, so the
    /// token the caller matched on no longer satisfies an `If-Match` against this
    /// configuration. Nothing else about a configuration changes: the id is the one the create
    /// minted, and a configuration carries neither a status nor a propagation deadline, so the
    /// clock owes it nothing. WHICH members an update may move is the caller's rule, decided
    /// before it reaches here.
    ///
    /// `None` when the store holds no such configuration, checked BEFORE the mint so a version
    /// token is never spent on a configuration that is not there.
    pub fn apply_field_level_encryption_config(
        &mut self,
        id: &str,
        config: FieldLevelEncryptionConfig,
        now: SystemTime,
    ) -> Option<&StoredFieldLevelEncryptionConfig> {
        if !self.field_level_encryption_configs.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.field_level_encryption_configs.get_mut(id)?;
        stored.config = config;
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
fn distribution_page_walk<'a, T, F, M>(
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

/// The ARN of a streaming distribution. CloudFront is a GLOBAL service, so the ARN's
/// region segment is empty, and the resource part names the RTMP resource kind:
/// `arn:aws:cloudfront::<account>:streaming-distribution/<Id>`.
pub fn streaming_distribution_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("streaming-distribution/{id}"))
}

/// The path of a streaming distribution resource under this service's API version — the
/// URI `CreateStreamingDistribution` reports as the `Location` of what it created.
pub fn streaming_distribution_path(id: &str) -> String {
    format!("/2020-05-31/streaming-distribution/{id}")
}

/// The path of one invalidation of one distribution.
pub fn invalidation_path(distribution_id: &str, invalidation_id: &str) -> String {
    format!("/2020-05-31/distribution/{distribution_id}/invalidation/{invalidation_id}")
}

/// The path of one invalidation of one distribution TENANT — the URI the tenant
/// invalidation operations are bound to.
pub fn distribution_tenant_invalidation_path(tenant_id: &str, invalidation_id: &str) -> String {
    format!("/2020-05-31/distribution-tenant/{tenant_id}/invalidation/{invalidation_id}")
}

/// The ARN of a key value store. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:key-value-store/<Id>`. This is the
/// identifier the key value store DATA plane (`cloudfront-keyvaluestore`) addresses the
/// store by, so the resource part is the store's minted UUID id.
pub fn key_value_store_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("key-value-store/{id}"))
}

/// The path of a key value store resource under this service's API version.
///
/// A create's `Location` addresses the new resource by the identifier the SERVICE minted
/// for it, never by the `Name` the caller supplied — and the identifier a key value store
/// is addressed by is its ARN ([`key_value_store_arn`]), whose own last segment is the
/// generated `Id`. That is what the recorded `CreateKeyValueStore` response carries, and
/// it is the identifier the whole key value store surface uses for a store: the control
/// plane reports it as `KeyValueStore$ARN`, and the data plane
/// (`cloudfront-keyvaluestore`) takes it as the `KvsARN` label of every one of its
/// routes. So `identifier` is a store ARN, not a name and not a bare id.
///
/// The ARN rather than the bare `Id` is what the recorded corpus holds this header to:
/// replaying `cloudfront-keyvaluestore-put-get-describe-etag-chain#setup#1` with the
/// bare `Id` as the last segment still reported `$headers.location` diverging, and with
/// the ARN that step left the divergence set. Both forms end in the generated `Id`; the
/// recording is what distinguishes them.
pub fn key_value_store_path(identifier: &str) -> String {
    format!("/2020-05-31/key-value-store/{identifier}")
}

/// The ARN of an Anycast static IP list. CloudFront is a GLOBAL service, so the ARN's
/// region segment is empty: `arn:aws:cloudfront::<account>:anycast-ip-list/<Id>`, the
/// resource type this service's own `/anycast-ip-list` routes name.
pub fn anycast_ip_list_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("anycast-ip-list/{id}"))
}

/// "The static IP addresses that are allocated to the Anycast static IP list": the slots
/// `hosts` holds, rendered in the families `ip_address_type` admits.
///
/// `ipv4` reports IPv4 addresses, `ipv6` reports IPv6 ones, and `dualstack` is "a list of
/// both IPv4 and IPv6 addresses", so it renders alternate slots from the two blocks and
/// holds at least one of each. The ALLOCATION is the list's own
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_anycast_ip_hosts`]) and is
/// untouched here, so a list whose address type changes reports the same slots in the new
/// families rather than taking a second allocation out of the pool.
pub fn anycast_ips_of(hosts: &[u32], ip_address_type: &str) -> Vec<String> {
    hosts
        .iter()
        .enumerate()
        .map(|(offset, host)| {
            let ipv6 = match ip_address_type {
                IP_ADDRESS_TYPE_IPV6 => true,
                IP_ADDRESS_TYPE_DUALSTACK => offset % 2 == 1,
                _ => false,
            };
            if ipv6 {
                format!("{ANYCAST_IPV6_PREFIX}{host:x}")
            } else {
                format!("{ANYCAST_IPV4_PREFIX}{host}")
            }
        })
        .collect()
}

/// Why the domain list a call would bind to a distribution tenant is no list of domains a
/// tenant can hold, or `None` when it is one.
///
/// "You must specify at least one domain", "Each distribution tenant can have up to 5
/// domains", and a domain is where the tenant's viewers reach it — so an empty list, a list
/// over [`MAX_DOMAINS_PER_TENANT`], a member that is no domain name
/// ([`domain_name_refusal`]) and a domain named twice (which describes no single
/// association) are each refused. The count is decided FIRST, so neither the per-domain walk
/// nor anything after it grows with a list the caller chose the length of.
///
/// ONE rule for every call that binds a tenant's domains, and decided from the request
/// alone, so a caller's list is bounded before any store lock is taken. The reason carries
/// no error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn tenant_domains_refusal(domains: &[String]) -> Option<String> {
    if domains.is_empty() {
        return Some("you must specify at least one domain for a distribution tenant".to_string());
    }
    if domains.len() > MAX_DOMAINS_PER_TENANT {
        return Some(format!(
            "a distribution tenant holds at most {MAX_DOMAINS_PER_TENANT} domains but the \
             request names {}",
            domains.len()
        ));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for domain in domains {
        if let Some(reason) = domain_name_refusal(domain) {
            return Some(reason);
        }
        if !seen.insert(domain.to_ascii_lowercase()) {
            return Some(format!("the domain {domain} is named more than once"));
        }
    }
    None
}

/// Why `parameters` is more parameter values than a distribution tenant's record carries, or
/// `None` when it is not.
///
/// The values END UP IN THE STORED RECORD, so the count is bounded
/// ([`MAX_PARAMETERS_PER_TENANT`]) before anything walks them: neither the work a request
/// causes nor the size of the record it stores grows with a list the caller chose the length
/// of. Whether the values SATISFY the distribution's definitions is the separate rule
/// [`parameter_refusal`] states, which needs the stored distribution.
///
/// ONE rule for every call that supplies a tenant's parameter values. The reason carries no
/// error wording of its own, for the same reason [`domain_name_refusal`]'s does not.
pub fn tenant_parameter_count_refusal(parameters: &[Parameter]) -> Option<String> {
    (parameters.len() > MAX_PARAMETERS_PER_TENANT).then(|| {
        format!(
            "a distribution tenant carries at most {MAX_PARAMETERS_PER_TENANT} parameter values \
             but the request names {}",
            parameters.len()
        )
    })
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

/// The ARN of a connection group. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:connection-group/<Id>`, the resource
/// type this service's own `/connection-group` routes name.
pub fn connection_group_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("connection-group/{id}"))
}

/// The resource type a resource policy may be attached to, as the resource part of an ARN
/// spells it.
///
/// CloudFront's resource-policy surface scopes cross-account sharing of ONE kind of
/// resource, a VPC origin, and the model spells that type the same way the service's own
/// routes do (`/2020-05-31/vpc-origin/{Id}`), as every other resource type of this service
/// is spelled in its ARN ([`arn_resource_type`]).
pub const RESOURCE_POLICY_RESOURCE_TYPE: &str = "vpc-origin";

/// The ARN of a VPC origin. CloudFront is a GLOBAL service, so the ARN's region segment is
/// empty: `arn:aws:cloudfront::<account>:vpc-origin/<Id>`, the resource type this service's
/// own `/vpc-origin` routes name.
pub fn vpc_origin_arn(id: &str) -> String {
    ::engine::emulator_arn(
        "cloudfront",
        "",
        &format!("{RESOURCE_POLICY_RESOURCE_TYPE}/{id}"),
    )
}

/// The resource an ARN of THIS service names, as the `(type, id)` pair its resource part
/// spells, or `None` where the value is not an ARN of this service naming a resource.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` and this service's
/// resources spell their resource part `<type>/<id>` ([`arn_resource_type`]), so what is
/// read here is `arn:`, a named partition and this service's own namespace — the region and
/// account segments that follow are the owner's and narrow nothing about WHICH resource the
/// ARN names — and then that pair. One reading of an ARN for every caller that has to
/// resolve one against this store, so no two of them can disagree about which resource an
/// ARN names.
pub fn cloudfront_arn_resource(arn: &str) -> Option<(&str, &str)> {
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" || segments.next()?.is_empty() || segments.next()? != "cloudfront"
    {
        return None;
    }
    let (kind, id) = segments.nth(2)?.split_once('/')?;
    (!kind.is_empty() && !id.is_empty()).then_some((kind, id))
}

/// The resource a `ResourceArn` names for the resource-policy surface — the id of the VPC
/// origin it addresses — or `None` where the ARN names no such resource.
///
/// `ResourceArn` is modelled as a plain `string` with no pattern, so nothing is refused
/// before a handler runs and reading the ARN is this check: it is an ARN of this service
/// ([`cloudfront_arn_resource`]) whose resource part names [`RESOURCE_POLICY_RESOURCE_TYPE`]
/// and carries an id. A string that is no ARN at all, an ARN of another service and an ARN
/// naming another resource type all answer `None` — which is not one refusal but two, told
/// apart by whether this store holds the resource the ARN names
/// ([`resource_policy_arn_refusal`]).
pub fn resource_policy_resource(arn: &str) -> Option<&str> {
    let (kind, id) = cloudfront_arn_resource(arn)?;
    (kind == RESOURCE_POLICY_RESOURCE_TYPE).then_some(id)
}

/// The sentence either resource-policy WRITE — the one that attaches a policy and the one
/// that removes it — refuses a `ResourceArn` naming an existing resource that is not a VPC
/// origin with: the whole text is fixed, so nothing of the request — neither the ARN nor the
/// type it names — reaches it. Recorded behaviour.
pub const INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE: &str =
    "The specified resource type isn't valid. Specify a VPC origin and try again.";

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy WRITE naming an existing
/// resource the surface does not attach policies to is refused with — "An argument is
/// invalid."
///
/// Both writes of the surface word the condition this way: what a caller has to fix is the
/// resource type it named, whether it was attaching a policy or removing one.
///
/// Decided AFTER the ARN is resolved against the store: a type refusal states that the
/// resource the caller named takes no resource policy, which is only true of a resource that
/// is there, so an ARN this service holds no resource for is the absent-resource answer
/// ([`resource_policy_resource_absent`]) instead.
pub fn invalid_resource_policy_resource_type() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE.to_string()),
    }
}

/// The sentence a resource-policy READ refuses a `ResourceArn` naming an existing resource
/// that is not a VPC origin with.
///
/// The CONDITION the three operations of the surface refuse is one
/// ([`resource_policy_arn_refusal`]); the sentence each puts on the wire is the
/// service's own, and the read words it as a demand for a usable ARN rather than naming the
/// resource type the write asks for. Recorded behaviour; the whole text is fixed, so nothing
/// of the request reaches it.
pub const INVALID_RESOURCE_POLICY_ARN_MESSAGE: &str =
    "The ResourceArn parameter is required. Specify a valid ARN and try again.";

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy READ naming an existing
/// resource the surface does not attach policies to is refused with.
pub fn invalid_resource_policy_arn() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESOURCE_POLICY_ARN_MESSAGE.to_string()),
    }
}

/// What every ARN opens with, whatever partition, service or resource follows it.
///
/// A value that does not open with it is no ARN at all, which is the one condition
/// [`MALFORMED_ARN_MESSAGE`] states.
pub const ARN_PREFIX: &str = "arn:";

/// The sentence an operation taking an ARN refuses a value that is no ARN AT ALL with: the
/// whole text is fixed, and it states the FORM the parameter has to take rather than the
/// value it was given. Recorded behaviour.
pub const MALFORMED_ARN_MESSAGE: &str = "ARNs must start with 'arn:'";

/// The modelled `InvalidArgument` (HTTP 400) for a parameter that is no ARN at all —
/// "An argument is invalid."
///
/// A value that does not open with [`ARN_PREFIX`] cannot be READ as an ARN, so the refusal
/// is about its form and nothing of the value reaches the sentence.
pub fn malformed_arn() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MALFORMED_ARN_MESSAGE.to_string()),
    }
}

/// Why the resource-policy surface refuses `arn`, or `None` when it names a resource the
/// surface attaches policies to.
///
/// THREE conditions, in the order the service decides them:
///
/// * [`ResourceArnRefusal::Malformed`] — a value that is no ARN at all cannot be read as
///   one, so it is refused on its FORM;
/// * [`ResourceArnRefusal::AbsentResource`] — an ARN naming no resource this service holds
///   ([`ComAmazonawsCloudfrontCloudfront20200531State::holds_resource`]) is reported ABSENT:
///   there is no resource for a policy to be attached to, read from or removed from, which
///   is a property of the store rather than of the parameter;
/// * [`ResourceArnRefusal::WrongType`] — an ARN naming a resource this service DOES hold,
///   of a type other than [`RESOURCE_POLICY_RESOURCE_TYPE`], is refused on the resource TYPE
///   it names: that resource exists and takes no resource policy.
///
/// The accepted type is not resolved against the store: this emulator holds no VPC origins
/// of its own, so a policy is keyed by the ARN it was attached to and whether that key
/// carries one is the surface's own existence answer ([`resource_policy_not_found`]).
///
/// ONE rule for every operation of the surface, which all take the one `ResourceArn`, so no
/// two of them can disagree about which values they accept. The SENTENCE each puts on the
/// wire is the service's own per operation: see [`malformed_arn`],
/// [`invalid_resource_policy_resource_type`], [`invalid_resource_policy_arn`] and
/// [`resource_policy_resource_absent`].
pub fn resource_policy_arn_refusal(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    arn: &str,
) -> Option<ResourceArnRefusal> {
    if !arn.starts_with(ARN_PREFIX) {
        return Some(ResourceArnRefusal::Malformed);
    }
    if resource_policy_resource(arn).is_some() {
        return None;
    }
    Some(if store.holds_resource(arn) {
        ResourceArnRefusal::WrongType
    } else {
        ResourceArnRefusal::AbsentResource
    })
}

/// Why a `ResourceArn` names no resource the resource-policy surface attaches policies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnRefusal {
    /// The value is no ARN at all: it does not open with [`ARN_PREFIX`].
    Malformed,
    /// The value is an ARN, and this service holds no resource it names.
    AbsentResource,
    /// The value is an ARN of a resource this service holds, of a type other than
    /// [`RESOURCE_POLICY_RESOURCE_TYPE`].
    WrongType,
}

/// The modelled `EntityNotFound` (HTTP 404) a resource-policy call naming a resource this
/// service holds none of is refused with — "The entity was not found."
///
/// The sentence is the one CloudFront answers for the class ([`NO_SUCH_RESOURCE_MESSAGE`]):
/// it states that the resource asked about is not there and names no part of the request, so
/// nothing in it is built out of the call.
pub fn resource_policy_resource_absent() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_RESOURCE_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) a resource-policy write that would grow the
/// store past [`MAX_RESOURCE_POLICIES`] is refused with, beside the other refusals of this
/// surface rather than built at its one call site.
///
/// The BOUND is this emulator's, so the sentence is too: real CloudFront bounds the
/// policies an account can hold through the quota on the resources they are attached to and
/// models no limit error on this operation at all.
pub fn too_many_resource_policies() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "This emulator holds resource policies for at most {MAX_RESOURCE_POLICIES} \
             resources; delete one before attaching another"
        )),
    }
}

/// The sentence a read of a resource policy that was never written answers with: the
/// CONDITION, carrying nothing from the request, as every other not-found sentence of this
/// service states its own condition ([`KEY_VALUE_STORE_NOT_FOUND_MESSAGE`],
/// [`CONNECTION_GROUP_NOT_FOUND_MESSAGE`]).
pub const RESOURCE_POLICY_NOT_FOUND_MESSAGE: &str = "The specified resource policy does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for a resource that carries no resource policy
/// — "The entity was not found."
///
/// Takes no ARN: what the refusal states is that no policy is attached to the resource
/// asked about, which is a property of the store rather than of the value submitted.
pub fn resource_policy_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(RESOURCE_POLICY_NOT_FOUND_MESSAGE.to_string()),
    }
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

/// `DistributionResourceType`'s value for a standard or multi-tenant DISTRIBUTION, as the
/// model's enum spells it (`crate::ops::types::DISTRIBUTION_RESOURCE_TYPE_VALUES`, which
/// `smithy_query(query="shape",shape="DistributionResourceType")` reports the same values
/// for).
pub const DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION: &str = "distribution";

/// `DistributionResourceType`'s value for a DISTRIBUTION TENANT — the other value the
/// model's enum admits beside [`DISTRIBUTION_RESOURCE_TYPE_DISTRIBUTION`].
pub const DISTRIBUTION_RESOURCE_TYPE_TENANT: &str = "distribution-tenant";

/// The key one reported domain conflict is ordered and deduplicated by: the conflicting
/// domain, compared as DNS names are, and then the id of the resource holding it.
///
/// A domain can be associated with more than one resource in a store this service never
/// refused the second association to, so the domain alone is no key; the pair is, because
/// one resource holds one domain once.
fn conflict_order(conflict: &DomainConflict) -> (String, String) {
    (
        conflict.domain.to_lowercase(),
        conflict.resource_id.clone(),
    )
}

/// Whether the alternate domain name `pattern` is a WILDCARD name that covers `name`.
///
/// A CloudFront wildcard alternate domain name replaces the whole leftmost label of a
/// name: "if you provide `*.example.com` as input, the returned list can include ... any
/// alternate domain names covered by that wildcard (for example, `www.example.com`,
/// `test.example.com`, `dev.example.com`, and so on)". So a wildcard covers exactly the
/// names that carry ONE label in place of the `*`, a name that is not a wildcard covers
/// nothing but itself, and a wildcard does not cover another wildcard. The suffix is
/// compared as DNS names are ([`same_domain`]).
pub fn wildcard_covers(pattern: &str, name: &str) -> bool {
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    let Some((label, rest)) = name.split_once('.') else {
        return false;
    };
    !label.is_empty() && label != "*" && same_domain(rest, suffix)
}

/// Whether the domain association `held` conflicts with the domain `requested`.
///
/// "Lists existing domain associations that conflict with the domain that you specify":
/// two names conflict when CloudFront cannot route them separately — they are the same
/// name, or one of them is a wildcard alternate domain name that covers the other, which
/// is why the relation is symmetric. ONE rule for every surface that reports or refuses an
/// overlapping domain.
pub fn domains_conflict(held: &str, requested: &str) -> bool {
    same_domain(held, requested)
        || wildcard_covers(held, requested)
        || wildcard_covers(requested, held)
}

/// Whether a certificate that covers the names in `covered` covers `domain`.
///
/// A certificate covers the names it was issued over, and a wildcard name among them
/// covers every name under it ([`wildcard_covers`]) — the direction is what separates this
/// from [`domains_conflict`]: a certificate for one name does not cover the wildcard that
/// covers that name.
pub fn certificate_covers<'a, I>(covered: I, domain: &str) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    covered
        .into_iter()
        .any(|name| same_domain(name, domain) || wildcard_covers(name, domain))
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

/// The value a distribution's configuration carries to attach the origin access identity
/// `id` to an Amazon S3 origin.
///
/// `S3OriginConfig$OriginAccessIdentity` documents the form exactly — "The format of the
/// value is: `origin-access-identity/cloudfront/ID-of-origin-access-identity`", where the
/// id is "the value that CloudFront returned in the `ID` element when you created the
/// origin access identity" — and `S3Origin$OriginAccessIdentity` names the same identity
/// the same way, so both are built here rather than spelled out at each use site.
pub fn origin_access_identity_reference(id: &str) -> String {
    format!("origin-access-identity/cloudfront/{id}")
}

/// The path of a response headers policy resource under this service's API version. The
/// resource is addressed by ID, as `GetResponseHeadersPolicy`'s `{Id}` label is.
pub fn response_headers_policy_path(id: &str) -> String {
    format!("/2020-05-31/response-headers-policy/{id}")
}

/// The path of an origin access control resource under this service's API version — the
/// route `GetOriginAccessControl` serves (`/origin-access-control/{Id}`), which is
/// therefore the resource the create's `Location` names.
pub fn origin_access_control_path(id: &str) -> String {
    format!("/2020-05-31/origin-access-control/{id}")
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

/// The path of a public key resource under this service's API version. The resource is
/// addressed by ID, as `GetPublicKey`'s `{Id}` label is.
pub fn public_key_path(id: &str) -> String {
    format!("/2020-05-31/public-key/{id}")
}

/// The path of a key group resource under this service's API version — the route
/// `GetKeyGroup` serves (`/key-group/{Id}`), which is therefore the resource the create's
/// `Location` names.
pub fn key_group_path(id: &str) -> String {
    format!("/2020-05-31/key-group/{id}")
}

/// The path of a field-level encryption profile resource under this service's API version.
/// The resource is addressed by ID, as `GetFieldLevelEncryptionProfile`'s `{Id}` label is.
pub fn field_level_encryption_profile_path(id: &str) -> String {
    format!("/2020-05-31/field-level-encryption-profile/{id}")
}

/// The path of a field-level encryption configuration resource under this service's API
/// version — the route `GetFieldLevelEncryption` serves (`/field-level-encryption/{Id}`),
/// which is therefore the resource the create's `Location` names.
pub fn field_level_encryption_config_path(id: &str) -> String {
    format!("/2020-05-31/field-level-encryption/{id}")
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

/// Whether `value` has the shape of a streaming distribution id this service mints.
///
/// `ListStreamingDistributions`'s `Marker` is "the value you can use for the `Marker`
/// request parameter to continue listing your RTMP distributions where they left off",
/// which is a streaming distribution id this service handed out, and the model constrains
/// it no further than `string` — so this is what tells a resume point the service issued
/// from a value a client made up. The same shape [`is_distribution_id`] describes, because
/// CloudFront draws every distribution id from it.
pub fn is_streaming_distribution_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of a connection group id this service mints
/// ([`CONNECTION_GROUP_ID_PREFIX`] followed by [`ID_BODY_LEN`] uppercase alphanumerics).
///
/// `ListConnectionGroups`'s `Marker` is "the marker for the next set of connection groups
/// to retrieve", which is a value this service handed out, and the model constrains it no
/// further than `string` — so this is what tells a resume point the service issued from a
/// value a client made up.
pub fn is_connection_group_id(value: &str) -> bool {
    value
        .strip_prefix(CONNECTION_GROUP_ID_PREFIX)
        .is_some_and(is_token_body)
}

/// Whether `value` has the shape of a distribution tenant id this service MINTS
/// ([`DISTRIBUTION_TENANT_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters of the id
/// alphabet).
///
/// A distribution tenant listing's `Marker` is "the marker for the next set of results",
/// which is a tenant id this service handed out, and the model constrains it no further
/// than `string` — so this is what tells a resume point the service issued from a value a
/// client made up.
///
/// NOT what an operation that ADDRESSES a tenant reads its path identifier with: the width
/// and the alphabet here are this mint's, not CloudFront's, so refusing an identifier on
/// them would answer an argument failure for a tenant that is merely absent. That decision
/// is [`is_namespaced_resource_id`]'s.
pub fn is_distribution_tenant_id(value: &str) -> bool {
    value
        .strip_prefix(DISTRIBUTION_TENANT_ID_PREFIX)
        .is_some_and(is_token_body)
}

/// The longest a namespace label may be in an identifier [`is_namespaced_resource_id`]
/// admits.
///
/// The labels CloudFront writes these ids under are short: two letters for a distribution
/// tenant ([`DISTRIBUTION_TENANT_ID_PREFIX`]) or a connection group
/// ([`CONNECTION_GROUP_ID_PREFIX`]), three for an Anycast static IP list
/// ([`ANYCAST_IP_LIST_ID_PREFIX`]) — so this is the longest label any id of this service's
/// resources carries, and a longer word followed by a separator is not a namespace at all.
const MAX_ID_NAMESPACE_LABEL_LEN: usize = 3;

/// Whether `value` could be an opaque identifier CloudFront issued for a namespaced
/// resource: a lowercase namespace label of at most [`MAX_ID_NAMESPACE_LABEL_LEN`] letters,
/// a separator (`_` or `-`), and a non-empty body of identifier characters (ASCII
/// alphanumerics and those same separators).
///
/// This is ALL a request alone decides about such an identifier, and it is what the
/// operations that address one by a path member (`GetAnycastIpList$Id`,
/// `DeleteDistributionTenant$Id`, …) read it with. Those members are plain `string` in the
/// model — no `@pattern`, no `@length` — so neither the label, nor the width of the body,
/// nor its alphabet is a constraint this service may refuse a value on: the ids it MINTS are
/// one form of a namespace, never the only form CloudFront writes one in. Whether an
/// identifier of this form was ever ISSUED, and to which resource, is a property of the
/// STORE — so a value of this form that nothing answers to is a resource that is ABSENT (the
/// operation's not-found code, status and fixed sentence), while a value of no such form is
/// the identifier that cannot be read at all ([`invalid_id_format`]).
pub fn is_namespaced_resource_id(value: &str) -> bool {
    let Some(separator) = value.find(['_', '-']) else {
        return false;
    };
    let (label, rest) = value.split_at(separator);
    let body = &rest[1..];
    !label.is_empty()
        && label.len() <= MAX_ID_NAMESPACE_LABEL_LEN
        && label.bytes().all(|b| b.is_ascii_lowercase())
        && !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `value` has the shape of a key group id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_key_group_id`] draws a UUID).
///
/// A key group id is a UUID, whose form is fully specified — five hex groups of fixed width
/// — so unlike a namespaced id ([`is_namespaced_resource_id`]) the BODY is what says the
/// value could be one at all, and this predicate is what a read of a key group reads its
/// `Id` with.
pub fn is_key_group_id(value: &str) -> bool {
    is_minted_uuid(value)
}

/// Whether `value` has the shape of a continuous deployment policy id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_continuous_deployment_policy_id`]
/// draws a UUID), for the reason [`is_key_group_id`] exists.
pub fn is_continuous_deployment_policy_id(value: &str) -> bool {
    is_minted_uuid(value)
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

/// Whether `value` has the shape of an origin access control id this service mints.
///
/// `ListOriginAccessControls`'s `Marker` is an origin access control id — "set this
/// field's value to the value of `NextMarker` from the current page's response" — and the
/// model constrains it no further than `string`, so this is what tells a resume point the
/// service issued from a value a client made up. The same shape
/// [`is_distribution_id`] describes, because CloudFront draws every entity id from it.
pub fn is_origin_access_control_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
}

/// Whether `value` has the shape of an origin access identity id this service mints
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_origin_access_identity_id`]).
///
/// `ListCloudFrontOriginAccessIdentities`'s `Marker` is an origin access identity id —
/// "set the `Marker` to the value of the `NextMarker` from the current page's response
/// (which is also the ID of the last identity on that page)" — and the model constrains it
/// no further than `string`, so this is what tells a resume point the service issued from a
/// value a client made up. The same shape [`is_distribution_id`] describes, because
/// CloudFront draws every entity id of this family from it.
pub fn is_origin_access_identity_id(value: &str) -> bool {
    is_prefixed_token(value, 'E')
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

/// Whether `value` has the shape of a public key id this service mints
/// ([`PUBLIC_KEY_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters of the id alphabet).
///
/// `ListPublicKeys`'s `Marker` is a public key id — "set the Marker to the value of the
/// NextMarker from the current page's response (which is also the ID of the last public
/// key on that page)" — and the model constrains it no further than `string`, so this is
/// what tells a resume point the service issued from a value a client made up.
pub fn is_public_key_id(value: &str) -> bool {
    is_prefixed_token(value, PUBLIC_KEY_ID_PREFIX)
}

/// Whether `value` has the shape of a field-level encryption CONFIGURATION id this service
/// mints ([`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters
/// of the id alphabet).
///
/// `ListFieldLevelEncryptionConfigs`'s `Marker` is a configuration id — "set the Marker to
/// the value of the NextMarker from the current page's response (which is also the ID of the
/// last configuration on that page)" — and the model constrains it no further than `string`,
/// so this is what tells a resume point the service issued from a value a client made up.
pub fn is_field_level_encryption_config_id(value: &str) -> bool {
    is_prefixed_token(value, FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX)
}

/// Whether `value` has the shape of a field-level encryption profile id this service mints
/// ([`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`] followed by [`ID_BODY_LEN`] characters of
/// the id alphabet).
///
/// `ListFieldLevelEncryptionProfiles`'s `Marker` is a profile id — "set the Marker to the
/// value of the NextMarker from the current page's response (which is also the ID of the
/// last profile on that page)" — and the model constrains it no further than `string`, so
/// this is what tells a resume point the service issued from a value a client made up.
pub fn is_field_level_encryption_profile_id(value: &str) -> bool {
    is_prefixed_token(value, FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX)
}

/// The longest name a key value store carries, and the length its own name constraint
/// bounds every store name by.
///
/// `KeyValueStoreName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` (from
/// `smithy_query` on `CreateKeyValueStoreRequest$Name`), so this is the model's bound
/// rather than a number this emulator chose.
pub const MAX_KEY_VALUE_STORE_NAME: usize = 64;

/// Whether `value` is a name a key value store of this account can be held under: the
/// `KeyValueStoreName` constraint, `length 1..`[`MAX_KEY_VALUE_STORE_NAME`] over the
/// characters its pattern admits (ASCII letters, digits, `-` and `_`).
///
/// `ListKeyValueStores`'s `Marker` is the name the previous page ended at, and the model
/// constrains that parameter no further than `string`, so this is what tells a resume
/// point this service handed out from a value a client made up.
pub fn is_key_value_store_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_KEY_VALUE_STORE_NAME
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `value` is `prefix` followed by [`ID_BODY_LEN`] characters of the id
/// alphabet.
fn is_prefixed_token(value: &str, prefix: char) -> bool {
    match value.strip_prefix(prefix) {
        Some(body) => is_token_body(body),
        None => false,
    }
}

/// Whether `body` is [`ID_BODY_LEN`] characters of the id alphabet — the body every
/// token this service mints carries, whatever prefix it is minted under.
fn is_token_body(body: &str) -> bool {
    body.len() == ID_BODY_LEN && body.bytes().all(is_token_byte)
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

/// The version token an `If-Match` header carries, or `None` when the header is absent or
/// carries something that is not a version token at all.
///
/// The FORM of the header, decided against nothing but the header itself: a mutation can
/// therefore tell "missing or not valid" ([`invalid_if_match_version`]) from "not the
/// current version" without looking a resource up, which is what lets an operation refuse a
/// header no version could make valid BEFORE it decides whether the resource exists.
///
/// The value may be quoted, as HTTP entity tags normally are, or bare, as CloudFront's own
/// `ETag` header reports it, and may carry the weak-comparison prefix.
pub fn if_match_version(if_match: Option<&str>) -> Option<&str> {
    let value = if_match?.trim().trim_start_matches("W/").trim_matches('"');
    is_version_token(value).then_some(value)
}

/// Compare an `If-Match` header against a distribution's current version.
///
/// `InvalidIfMatchVersion` is documented as "the `If-Match` version is missing or not
/// valid", so an absent header and a value that is not a version token answer the same
/// way ([`if_match_version`]); a well-formed token that is not the current one is a failed
/// precondition instead.
///
/// That split is what every mutation of this service answers with EXCEPT
/// `DeleteContinuousDeploymentPolicy`, whose recorded behaviour refuses a well-formed token
/// that is not the policy's with `InvalidIfMatchVersion` as well — "missing or not valid"
/// read as "not valid FOR THIS RESOURCE". That operation therefore compares the token itself
/// (see its handler) rather than calling this, and the difference is deliberate: do not
/// "correct" either side into the other.
pub fn check_if_match(current: &str, if_match: Option<&str>) -> IfMatch {
    let Some(value) = if_match_version(if_match) else {
        return IfMatch::Invalid;
    };
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

/// Whether two public key configurations ask for the same thing, which is what decides
/// whether a repeated `CallerReference` is the same create again or a second public key
/// under a reference that is already taken.
///
/// The same comparison [`same_origin_access_identity_config`] makes, for the same reasons:
/// every member a caller can submit takes part — down to the members a request left out —
/// and comparing the DESERIALIZED configurations is what makes it insensitive to the
/// request document's whitespace and member order.
pub fn same_public_key_config(left: &PublicKeyConfig, right: &PublicKeyConfig) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// The member of a public key's configuration an update would CHANGE, out of the members
/// that are fixed for the key's life, or `None` when it changes none of them.
///
/// "Update public key information. Note that the only value you can change is the comment."
/// — so `CallerReference`, `Name` and `EncodedKey` are fixed once the key is uploaded, and a
/// request submitting a different value for any one of them asks for a change the key does
/// not take ([`cannot_change_immutable_public_key_fields`]). The comparison is against the
/// STORED configuration, so what decides it is the record rather than the request alone, and
/// a request that resubmits the stored values unchanged beside a new comment is no change to
/// an immutable member at all.
///
/// The member's name is reported for the caller that needs to know WHICH value it may not
/// move; the modelled refusal's own sentence states the condition.
pub fn immutable_public_key_change(
    stored: &PublicKeyConfig,
    submitted: &PublicKeyConfig,
) -> Option<&'static str> {
    [
        (
            "CallerReference",
            &stored.caller_reference,
            &submitted.caller_reference,
        ),
        ("Name", &stored.name, &submitted.name),
        ("EncodedKey", &stored.encoded_key, &submitted.encoded_key),
    ]
    .into_iter()
    .find_map(|(member, held, asked)| (held != asked).then_some(member))
}

/// `CannotChangeImmutablePublicKeyFields`'s message, on the wire: the sentence names the
/// members that are fixed for a key's life — which is what the refusal is about, and what
/// tells a caller that `Comment` is updatable while `Name` and `EncodedKey` are not.
///
/// The WIRE text, which differs from the sentence the MODEL's documentation of the shape uses
/// ("You can't change the value of a public key."); do not "correct" this constant to the
/// documentation sentence. It carries nothing from the request either way.
pub const CANNOT_CHANGE_IMMUTABLE_PUBLIC_KEY_FIELDS_MESSAGE: &str =
    "You cannot modify encoded material and name of a public key once created.";

/// The modelled `CannotChangeImmutablePublicKeyFields` (HTTP 400) for an update that would
/// move a member fixed for the key's life ([`immutable_public_key_change`]).
pub fn cannot_change_immutable_public_key_fields()
-> crate::ops::types::CannotChangeImmutablePublicKeyFields {
    crate::ops::types::CannotChangeImmutablePublicKeyFields {
        message: Some(CANNOT_CHANGE_IMMUTABLE_PUBLIC_KEY_FIELDS_MESSAGE.to_string()),
    }
}

/// Whether two field-level encryption profile configurations ask for the same thing, which
/// is what decides whether a repeated `CallerReference` is the same create again or a second
/// profile under a reference that is already taken.
///
/// The same comparison [`same_public_key_config`] makes, for the same reasons: every member
/// a caller can submit takes part — down to the members a request left out — and comparing
/// the DESERIALIZED configurations is what makes it insensitive to the request document's
/// whitespace and member order.
pub fn same_field_level_encryption_profile_config(
    left: &FieldLevelEncryptionProfileConfig,
    right: &FieldLevelEncryptionProfileConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// Whether two field-level encryption configurations ask for the same thing, which is what
/// decides whether a repeated `CallerReference` is the same create again or a second
/// configuration under a reference that is already taken.
///
/// The same comparison [`same_field_level_encryption_profile_config`] makes, for the same
/// reasons: every member a caller can submit takes part — down to the members a request left
/// out — and comparing the DESERIALIZED configurations is what makes it insensitive to the
/// request document's whitespace and member order.
pub fn same_field_level_encryption_config(
    left: &FieldLevelEncryptionConfig,
    right: &FieldLevelEncryptionConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// Whether two origin access control configurations ask for the same thing, which is what
/// decides whether a create describes a control the account already holds or a new one.
///
/// The same comparison [`same_config`] makes, for the same reasons: every member a caller
/// can submit takes part — down to the members a request left out — and comparing the
/// DESERIALIZED configurations is what makes it insensitive to the request document's
/// whitespace and member order.
pub fn same_origin_access_control_config(
    left: &OriginAccessControlConfig,
    right: &OriginAccessControlConfig,
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

/// The `InvalidViewerCertificate` message for a configuration whose alternate domain
/// names no certificate IT names can serve, or `None` when the two agree.
///
/// `ViewerCertificate` states the rule for both directions: "If the distribution doesn't
/// use `Aliases` (also known as alternate domain names or CNAMEs) … set
/// `CloudFrontDefaultCertificate` to `true` and leave all other fields empty", and "If you
/// want viewers to use HTTPS with an alternate domain name … you must choose one of
/// `ACMCertificateArn` or `IAMCertificateId`" — the certificate has to validate the
/// authorization to serve the domain name, which CloudFront's own
/// `*.cloudfront.net` certificate cannot do for a name that is not under it. So a
/// configuration that declares at least one alias has to name exactly ONE custom
/// certificate, and a configuration that names none of them — the default certificate, by
/// itself or by omission — may declare no alias.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, so no stored
/// distribution ever carries an alias its certificate does not cover: the condition is a
/// property of the submitted configuration alone, which is why it lives here beside
/// [`config_quantity_mismatch`] rather than in one handler.
pub fn alias_certificate_mismatch(config: &DistributionConfig) -> Option<String> {
    let aliases: &[String] = config
        .aliases
        .as_ref()
        .and_then(|aliases| aliases.items.as_deref())
        .unwrap_or_default();
    if aliases.is_empty() {
        return None;
    }
    let certificate = config.viewer_certificate.as_ref();
    let (acm, iam) = custom_certificate_members(certificate);
    let default = certificate
        .is_none_or(|certificate| certificate.cloud_front_default_certificate == Some(true));
    let aliases = aliases.join(", ");
    match (acm, iam) {
        (false, false) => Some(format!(
            "This distribution states the alternate domain names [{aliases}] and no viewer \
             certificate that covers them: a distribution with an alternate domain name has \
             to name one of ViewerCertificate.ACMCertificateArn or \
             ViewerCertificate.IAMCertificateId, since CloudFront's default certificate \
             validates no domain name of yours"
        )),
        (true, true) => Some(format!(
            "This distribution states the alternate domain names [{aliases}] with both \
             ViewerCertificate.ACMCertificateArn and ViewerCertificate.IAMCertificateId: \
             exactly one of them names the certificate that covers them"
        )),
        _ if default => Some(format!(
            "This distribution states the alternate domain names [{aliases}] with \
             ViewerCertificate.CloudFrontDefaultCertificate set: the default certificate \
             serves only the distribution's own CloudFront domain name, so it cannot be \
             combined with a custom certificate for an alternate domain name"
        )),
        _ => None,
    }
}

/// The `InvalidLambdaFunctionAssociation` message for a configuration carrying a
/// Lambda@Edge association whose ARN names no published function version, or `None` when
/// every association it carries names one.
///
/// `LambdaFunctionAssociation$LambdaFunctionARN` states the rule: "You must specify the ARN
/// of a function version; you can't specify an alias or $LATEST." A function version is
/// addressed by the function's ARN carrying that version's NUMBER as its qualifier, so an
/// association whose value is no Lambda function ARN, carries no qualifier at all, or
/// carries one that is not a published version number — the `$LATEST` of a function nobody
/// published, or an alias name — names no version at all.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, and for EVERY
/// cache behaviour of it (the default one and each ordered one), so no stored distribution
/// ever carries an edge association that names nothing invocable: the condition is a
/// property of the submitted configuration alone, which is why it lives here beside
/// [`alias_certificate_mismatch`] rather than in one handler.
///
/// The message names the FIRST association that breaks the rule, read in the order the
/// configuration states them — the default cache behaviour, then the ordered ones — and
/// echoes that entry's `LambdaFunctionARN` as submitted, since that value is what tells a
/// caller which of a configuration's associations was refused
/// ([`invalid_lambda_function_association_message`]).
pub fn lambda_association_without_version(config: &DistributionConfig) -> Option<String> {
    behavior_views(config)
        .iter()
        .filter_map(|behavior| behavior.lambda_function_associations)
        .flat_map(|associations| associations.items.as_deref().unwrap_or_default())
        .find(|association| !names_lambda_function_version(&association.lambda_function_arn))
        .map(|association| {
            invalid_lambda_function_association_message(&association.lambda_function_arn)
        })
}

/// Whether an origin of `config` names BOTH an origin access control and an origin access
/// identity, which is the configuration `IllegalOriginAccessConfiguration` refuses: "An
/// origin cannot contain both an origin access control (OAC) and an origin access identity
/// (OAI)."
///
/// The two are the two ways ONE origin reaches a private Amazon S3 bucket — `Origin$
/// OriginAccessControlId` names the control whose credentials sign the origin request, and
/// `S3OriginConfig$OriginAccessIdentity` names the identity CloudFront reaches the bucket
/// as — so an origin naming both says nothing about which of them signs it, and the
/// configuration is refused rather than stored. Either member is documented EMPTY for an
/// origin that names none ("specify an empty `OriginAccessIdentity` element"), so an
/// absent or empty value is not a reference and an origin carrying one of them is fine.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, and for EVERY
/// origin of it, so no stored origin ever carries both: the condition is a property of the
/// submitted configuration alone, which is why it lives here beside
/// [`alias_certificate_mismatch`] and [`lambda_association_without_version`] rather than
/// in one handler.
pub fn origin_with_both_access_mechanisms(config: &DistributionConfig) -> bool {
    config.origins.items.iter().any(|origin| {
        let control = origin
            .origin_access_control_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
        let identity = origin
            .s3_origin_config
            .as_ref()
            .is_some_and(|s3| !s3.origin_access_identity.trim().is_empty());
        control && identity
    })
}

/// Whether `arn` names a PUBLISHED Lambda function version.
///
/// A version is addressed by the function's own ARN —
/// `arn:<partition>:lambda:<region>:<account>:function:<name>` — qualified with the version,
/// and a published version's qualifier is the number that was assigned when it was
/// published, counting from 1. So every other value names no version: one that is not a
/// Lambda function ARN at all, the unqualified function ARN, the `$LATEST` qualifier of the
/// unpublished function, and an alias name.
fn names_lambda_function_version(arn: &str) -> bool {
    // The eight segments of a function-version ARN, read in order and without collecting
    // them: a value with fewer segments leaves a `None` behind, and one with more is
    // refused by the final `is_none`.
    let mut segments = arn.split(':');
    let prefix = segments.next();
    let partition = segments.next();
    let service = segments.next();
    let region = segments.next();
    let account = segments.next();
    let resource = segments.next();
    let name = segments.next();
    let qualifier = segments.next();
    let named = |segment: Option<&str>| segment.is_some_and(|segment| !segment.is_empty());
    prefix == Some("arn")
        && service == Some("lambda")
        && resource == Some("function")
        && named(partition)
        && named(region)
        && named(account)
        && named(name)
        && qualifier.is_some_and(is_published_version)
        && segments.next().is_none()
}

/// Whether `qualifier` is a published version number: a decimal number counting from 1,
/// so an empty qualifier, one carrying any non-digit (`$LATEST` and every alias name do)
/// and one written with a leading zero are all not one.
fn is_published_version(qualifier: &str) -> bool {
    !qualifier.is_empty()
        && !qualifier.starts_with('0')
        && qualifier.bytes().all(|byte| byte.is_ascii_digit())
}

/// Which of the two CUSTOM certificate members a `ViewerCertificate` names: the ACM
/// certificate ARN and the IAM certificate id, in that order.
///
/// An empty member names no certificate: it is "left empty" exactly as the
/// default-certificate wording asks for. ONE reading of the pair, so the rule that binds a
/// distribution's aliases to its certificate ([`alias_certificate_mismatch`]) and the
/// question of whether a distribution carries a custom certificate at all
/// ([`names_custom_certificate`]) cannot disagree about one configuration.
fn custom_certificate_members(certificate: Option<&ViewerCertificate>) -> (bool, bool) {
    let named = |value: Option<&String>| value.is_some_and(|value| !value.is_empty());
    (
        named(certificate.and_then(|certificate| certificate.acm_certificate_arn.as_ref())),
        named(certificate.and_then(|certificate| certificate.iam_certificate_id.as_ref())),
    )
}

/// Whether `certificate` names a CUSTOM certificate — one kept in ACM or in IAM — rather
/// than CloudFront's own default certificate.
///
/// `ViewerCertificate` is the certificate a distribution serves its viewers over, and a
/// custom one is what "validates the authorization to serve the domain name": CloudFront's
/// own default certificate "serves only the distribution's own CloudFront domain name", so
/// it covers no alternate domain name at all. Which names a certificate kept in ACM or IAM
/// covers is no record of this service's, so a custom one counts for the domain asked
/// about and the default certificate for none — the same reading
/// [`StoredDistributionTenant::certificate_covers_domain`] gives a tenant's certificate
/// ARN.
pub fn names_custom_certificate(certificate: Option<&ViewerCertificate>) -> bool {
    let (acm, iam) = custom_certificate_members(certificate);
    acm || iam
}

/// Whether anything a distribution-tenant call has to hand provides a TLS certificate for the
/// domains that tenant claims: a certificate its `Customizations` name, or a custom
/// certificate on the multi-tenant distribution it sits under.
///
/// This is the question the sentence for a domain nothing proves ENUMERATES
/// ([`MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE`] — "at least one certificate in the distribution
/// tenant or distribution, or … a managed certificate request"), so it is what decides WHICH
/// of the two refusals that condition carries: with none of the three present the caller is
/// told the requirement it has not met, and with one of them present the refusal is about the
/// DOMAIN, which the certificate it holds validates nothing for.
pub fn names_a_certificate(
    customizations: Option<&Customizations>,
    distribution_certificate: Option<&ViewerCertificate>,
) -> bool {
    customizations.is_some_and(|customizations| customizations.certificate.is_some())
        || names_custom_certificate(distribution_certificate)
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
///
/// The sentence names the WRAPPER MEMBER whose `Quantity` disagreed — `Headers`, `Cookies`,
/// `QueryStrings` — rather than the path to it, as every other wrapper this service reports
/// is named (`Origins`, `Aliases`, `Paths`, `ContentTypeProfiles`): recorded behaviour for
/// this operation's refusal.
pub fn origin_request_policy_quantity_mismatch(
    config: &OriginRequestPolicyConfig,
) -> Option<String> {
    let mut counts: Vec<(&'static str, i32, usize)> = Vec::new();

    if let Some(headers) = &config.headers_config.headers {
        counts.push(("Headers", headers.quantity, items_len(&headers.items)));
    }
    if let Some(cookies) = &config.cookies_config.cookies {
        counts.push(("Cookies", cookies.quantity, items_len(&cookies.items)));
    }
    if let Some(query_strings) = &config.query_strings_config.query_strings {
        counts.push((
            "QueryStrings",
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

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// field-level encryption profile configuration whose `Quantity` disagrees with the length
/// of its `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`config_quantity_mismatch`], [`cache_policy_quantity_mismatch`] and the
/// other wrapper walks apply: every CloudFront list travels in a wrapper stating its own
/// length beside the list itself, and the service refuses a request where the two disagree.
/// A `FieldLevelEncryptionProfileConfig` reaches its required `EncryptionEntities` wrapper
/// and the `FieldPatterns` wrapper each entity in it carries, so the walk covers the nested
/// ones too rather than only the outermost.
pub fn field_level_encryption_profile_quantity_mismatch(
    config: &FieldLevelEncryptionProfileConfig,
) -> Option<String> {
    let entities = &config.encryption_entities;
    let mut counts: Vec<(String, i32, usize)> = vec![(
        "EncryptionEntities".to_string(),
        entities.quantity,
        items_len(&entities.items),
    )];
    for entity in entities.items.as_deref().unwrap_or_default() {
        counts.push((
            format!("EncryptionEntities.Items[{}].FieldPatterns", entity.public_key_id),
            entity.field_patterns.quantity,
            items_len(&entity.field_patterns.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The modelled `InconsistentQuantities` message for the first list wrapper in a
/// field-level encryption CONFIGURATION whose `Quantity` disagrees with the length of its
/// `Items`, or `None` when every wrapper agrees.
///
/// The SAME rule [`field_level_encryption_profile_quantity_mismatch`] and the other wrapper
/// walks apply: every CloudFront list travels in a wrapper stating its own length beside the
/// list itself, and the service refuses a request where the two disagree. A
/// `FieldLevelEncryptionConfig` reaches two such wrappers — the `QueryArgProfiles` of its
/// `QueryArgProfileConfig` and the `ContentTypeProfiles` of its `ContentTypeProfileConfig` —
/// each of them optional, so a wrapper the request leaves out states no quantity to
/// disagree with.
pub fn field_level_encryption_config_quantity_mismatch(
    config: &FieldLevelEncryptionConfig,
) -> Option<String> {
    let mut counts: Vec<(&str, i32, usize)> = Vec::new();
    if let Some(profiles) = config
        .query_arg_profile_config
        .as_ref()
        .and_then(|query_arg| query_arg.query_arg_profiles.as_ref())
    {
        counts.push((
            "QueryArgProfiles",
            profiles.quantity,
            items_len(&profiles.items),
        ));
    }
    if let Some(profiles) = config
        .content_type_profile_config
        .as_ref()
        .and_then(|content_type| content_type.content_type_profiles.as_ref())
    {
        counts.push((
            "ContentTypeProfiles",
            profiles.quantity,
            items_len(&profiles.items),
        ));
    }

    counts
        .iter()
        .find_map(|(label, quantity, len)| quantity_disagreement(label, *quantity, *len))
}

/// The sentence CloudFront refuses a cache configuration whose three TTLs are not ordered
/// with: it states the ORDERING RULE over all three members — which is what attributes the
/// refusal, since `InvalidArgument` is this service's catch-all — and nothing from the
/// request, so a `DefaultTTL` that breaks the chain is attributable from the same sentence a
/// `MaxTTL` below `MinTTL` is.
///
/// The rule in the words the service states it in: "less than or equal to" rather than a
/// symbol, and the three members named in the order they are written.
pub const TTL_ORDER_MESSAGE: &str =
    "MinTTL, MaxTTL and DefaultTTL should follow order MinTTL less than or equal to \
     DefaultTTL less than or equal to MaxTTL";

/// Why a cache policy configuration is unusable, as the sentence the modelled
/// `InvalidArgument` carries — or `None` when every member is within what the model and
/// the documentation allow.
///
/// Two rules, each one the documentation states. The TTL ordering is reported as the RULE it
/// broke ([`TTL_ORDER_MESSAGE`], fixed text); the remaining sentences name the member and the
/// constraint it broke, which is the one thing the caller cannot work out for itself.
///
/// * The three TTLs are ORDERED: each is "The {minimum,default,maximum} amount of time, in
///   seconds, that objects stay in the CloudFront cache", so the minimum cannot exceed the
///   default or the maximum and the default cannot exceed the maximum. Only the members
///   the request SUPPLIES are compared: a member the request leaves out carries
///   CloudFront's own documented default for it (86400 or `MinTTL`, whichever is larger,
///   for `DefaultTTL`; 31536000 or `DefaultTTL`, whichever is larger, for `MaxTTL`), and
///   refusing a configuration whose only disorder comes from that default would refuse
///   what the service accepts. Every break of the chain is the one ordering sentence
///   ([`TTL_ORDER_MESSAGE`]), which states the rule rather than the pair that exposed it.
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
    // supplies both members of it. Every break of the chain is the ONE ordering sentence
    // ([`TTL_ORDER_MESSAGE`]): the rule is over all three members, so which pair exposed
    // the break is no part of what the refusal states.
    let pairs: [(Option<i64>, Option<i64>); 3] = [
        (Some(config.min_ttl), config.default_ttl),
        (Some(config.min_ttl), config.max_ttl),
        (config.default_ttl, config.max_ttl),
    ];
    for (lower, upper) in pairs {
        let (Some(lower), Some(upper)) = (lower, upper) else {
            continue;
        };
        if lower > upper {
            return Some(TTL_ORDER_MESSAGE.to_string());
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

/// `NoSuchContinuousDeploymentPolicy`'s message for an id no stored policy answers to, on
/// the wire.
///
/// The WIRE text — what the recorded service answers this code with, in the form this
/// service states that condition in for every other named resource
/// ([`NO_SUCH_CACHE_POLICY_MESSAGE`], [`NO_SUCH_DISTRIBUTION_MESSAGE`],
/// [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`]): "The specified <resource> does not exist."
/// It differs from the sentence the MODEL's documentation of the error shape uses for the
/// same rule ("The continuous deployment policy doesn't exist."), which documents the
/// condition rather than stating what is served, so do not "correct" this constant to it.
pub const NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE: &str =
    "The specified continuous deployment policy does not exist.";

/// The modelled `NoSuchContinuousDeploymentPolicy` for a policy id the store does not
/// hold. Fixed service text, for the same reason [`no_such_distribution`] is.
pub fn no_such_continuous_deployment_policy() -> crate::ops::types::NoSuchContinuousDeploymentPolicy
{
    crate::ops::types::NoSuchContinuousDeploymentPolicy {
        message: Some(NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE.to_string()),
    }
}

/// `ContinuousDeploymentPolicyInUse`'s message: the sentence the model states on the shape
/// itself, worded as the operation documents the condition — the policy is attached to a
/// primary distribution, and the caller is told to detach it first.
///
/// A property of the CONDITION rather than of the request, so nothing from the request or
/// from the store belongs in it and every refusal of it reads the same, exactly as
/// [`CACHE_POLICY_IN_USE_MESSAGE`] words the same condition for a cache policy.
pub const CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE: &str =
    "You cannot delete a continuous deployment policy that is associated with a primary \
     distribution. First update your distribution to remove the continuous deployment \
     policy, then you can delete the policy.";

/// The modelled `ContinuousDeploymentPolicyInUse` (HTTP 409) for a delete of a policy a
/// stored distribution still attaches.
pub fn continuous_deployment_policy_in_use()
-> crate::ops::types::ContinuousDeploymentPolicyInUse {
    crate::ops::types::ContinuousDeploymentPolicyInUse {
        message: Some(CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE.to_string()),
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

/// The modelled `EntityNotFound` (HTTP 404) for an invalidation id that is not in the scope
/// of the distribution tenant the request named.
///
/// The CODE is the tenant surface's own — the family every tenant-scoped read resolves its
/// tenant under — while the SENTENCE is the one this condition carries whichever code reports
/// it ([`NO_SUCH_INVALIDATION_MESSAGE`], which the distribution-scoped read answers with):
/// what it states is that the store holds no such invalidation, which is a property of the
/// store rather than of the operation asking.
pub fn invalidation_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_INVALIDATION_MESSAGE.to_string()),
    }
}

/// `BatchTooLarge`'s message: the sentence the model states on the shape itself,
/// "Invalidation batch specified is too large."
///
/// A property of the batch the request carried rather than of its contents, so the paths it
/// named and the bound it broke are no part of the text.
pub const BATCH_TOO_LARGE_MESSAGE: &str = "Invalidation batch specified is too large.";

/// The modelled `BatchTooLarge` (HTTP 413) for an invalidation batch naming more objects than
/// one request may — the per-batch bound ([`MAX_INVALIDATION_PATHS`]).
///
/// The DISTRIBUTION-scoped create answers this code for that condition; the tenant-scoped one
/// answers the invalidation quota's own code for it
/// ([`too_many_invalidations_in_progress`]), which is what each surface's recorded behaviour
/// carries. Do not unify them.
pub fn batch_too_large() -> crate::ops::types::BatchTooLarge {
    crate::ops::types::BatchTooLarge {
        message: Some(BATCH_TOO_LARGE_MESSAGE.to_string()),
    }
}

/// `TooManyInvalidationsInProgress`'s message, as CloudFront sends it: "Processing your
/// request will cause you to exceed the maximum number of in-progress invalidations."
///
/// The model's doc text for the shape states the CONDITION ("You have exceeded the maximum
/// number of allowable InProgress invalidation batch requests, or invalidation objects.");
/// this is the sentence the service puts on the wire. Fixed service text that names no
/// request value, for the same reason [`PRECONDITION_FAILED_MESSAGE`] is: the quota is the
/// service's own, and a caller learns what it asked for from the request it sent.
pub const TOO_MANY_INVALIDATIONS_MESSAGE: &str =
    "Processing your request will cause you to exceed the maximum number of in-progress \
     invalidations.";

/// The modelled `TooManyInvalidationsInProgress` for a request the invalidation quota
/// cannot admit: one batch naming more invalidation objects than a request may, or one that
/// would take the invalidations held for a resource past what this emulator keeps.
pub fn too_many_invalidations_in_progress() -> crate::ops::types::TooManyInvalidationsInProgress {
    crate::ops::types::TooManyInvalidationsInProgress {
        message: Some(TOO_MANY_INVALIDATIONS_MESSAGE.to_string()),
    }
}

/// `PreconditionFailed`'s message, as CloudFront sends it on the resource-DOCUMENT surface:
/// "The request failed because it didn't meet the preconditions in one or more
/// request-header fields."
///
/// The model's doc text for the shape states the condition ("The precondition in one or
/// more of the request fields evaluated to `false`."); this is the sentence the service
/// puts on the wire. Either way it is fixed service text that names no request value: a
/// caller learns WHICH precondition failed from the `If-Match` it sent, not from a
/// version token echoed back to it.
///
/// The family this one serves is the resource DOCUMENT a caller conditionally rewrites: a
/// distribution, an origin access identity, a function, a field-level encryption
/// configuration or profile, a public key, a key value store, an Anycast IP list, a
/// continuous deployment policy. The two siblings below word the same condition for the
/// other two families.
pub const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

/// `PreconditionFailed`'s message on the key group surface: see
/// [`PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE`], which the key group surface shares
/// with the policy surface.
/// `PreconditionFailed`'s message on the POLICY surface, as CloudFront sends it: "The
/// request failed because it didn't meet the preconditions for this operation."
///
/// CloudFront words this one refusal per API FAMILY, and the sentence is fixed text that
/// names no request value either way. The reusable configuration objects a distribution
/// references by id — the cache policies, the origin request policies and the key groups —
/// report the precondition of the OPERATION, which is the sentence below; the resource
/// DOCUMENTS a caller conditionally rewrites (a distribution, an identity, a function, a
/// field-level encryption configuration, a key value store) report it of the request's header
/// fields ([`PRECONDITION_FAILED_MESSAGE`]), and the distribution-tenant surface reports the
/// request as missing a requirement
/// ([`PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE`]). Recorded behaviour decides which
/// family an operation belongs to: do not move one between them.
pub const PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions for this operation.";

/// `PreconditionFailed`'s message on the DISTRIBUTION-TENANT surface — the tenants, the
/// connection groups and connection functions that route them, and the web-ACL associations
/// of both — as CloudFront sends it: "Your request is missing requirements for this
/// operation. Update your request and try again."
///
/// The THIRD wording of the one condition, because CloudFront words this refusal per API
/// FAMILY: this family reports the request as missing a requirement rather than reporting the
/// precondition at all. Fixed text that names no request value either way, for the reason
/// [`PRECONDITION_FAILED_MESSAGE`] names none.
pub const PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE: &str =
    "Your request is missing requirements for this operation. Update your request and try \
     again.";

/// `InvalidIfMatchVersion`'s message, as CloudFront sends it: "The If-Match version is
/// missing or not valid for the resource."
///
/// ONE constant for every operation that evaluates an `If-Match`, because the condition
/// is a property of the HEADER the request carried rather than of the resource it names:
/// a distribution, a policy or a key group refused for it learns the same sentence, and
/// nothing about the request belongs in the text.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. It deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("The If-Match version
/// is missing or not valid.").
pub const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";

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

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version token
/// but not the current one, worded as the POLICY surface words it
/// ([`PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE`]).
pub fn precondition_failed_for_this_operation() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_FOR_THIS_OPERATION_MESSAGE.to_string()),
    }
}

/// The modelled `PreconditionFailed` for an `If-Match` that is a well-formed version token
/// but not the current one, worded as the DISTRIBUTION-TENANT surface words it
/// ([`PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE`]).
pub fn precondition_failed_missing_requirements() -> crate::ops::types::PreconditionFailed {
    crate::ops::types::PreconditionFailed {
        message: Some(PRECONDITION_FAILED_MISSING_REQUIREMENTS_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidIfMatchVersion` for an `If-Match` that is missing where the
/// operation requires one, or carries something that is no version token at all.
///
/// Fixed service text ([`INVALID_IF_MATCH_VERSION_MESSAGE`]), for the same reason
/// [`precondition_failed`] is: the caller learns which header failed from the one it
/// sent, never from a value echoed back to it.
pub fn invalid_if_match_version() -> crate::ops::types::InvalidIfMatchVersion {
    crate::ops::types::InvalidIfMatchVersion {
        message: Some(INVALID_IF_MATCH_VERSION_MESSAGE.to_string()),
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

/// The sentence CloudFront refuses a path identifier that is not in the shape its resource
/// type's ids take with — the ONE producer of that sentence in this crate.
///
/// `member` is the MODEL's name for the request member that carried the identifier, which
/// is the only variable part of the text: an operation that addresses its resource by `Id`
/// answers "Id format is invalid", while `GetInvalidationForDistributionTenant` — whose
/// tenant arrives in `DistributionTenantId` — names that member instead. Nothing the
/// request SUPPLIED reaches the wire; the offending value is never echoed back.
///
/// [`RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE`] is this sentence's `Id` form as a
/// `&'static str`, for the suites that assert it in a const table; nothing builds a refusal
/// from it any more.
pub fn id_format_invalid_message(member: &str) -> String {
    format!("{member} format is invalid")
}

/// The modelled `InvalidArgument` (HTTP 400) for a path identifier outside the shape its
/// resource type's ids take, naming the member that carried it
/// ([`id_format_invalid_message`]).
///
/// The shape of an identifier is a property of the REQUEST, so an operation decides it
/// before it resolves the identifier against the store: a value that was never an id of
/// that resource kind is an argument CloudFront cannot read, not a resource that is
/// absent.
pub fn invalid_id_format(member: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(id_format_invalid_message(member)),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for the same argument defect
/// [`invalid_id_format`] reports, on an operation whose model serves no `InvalidArgument`:
/// the code and the status are then the operation's own, and the MESSAGE is all that tells
/// an unreadable identifier from a resource that is simply not there.
pub fn entity_not_found_id_format(member: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(id_format_invalid_message(member)),
    }
}

/// The modelled `NoSuchResource` (HTTP 404) an operation that ADDRESSES a key group by `Id`
/// answers when no stored group answers to it.
///
/// `GetKeyGroup`'s model declares this one error, so one code and one status cover both
/// outcomes and the MESSAGE is what tells them apart — the same split
/// [`unresolved_response_headers_policy_id`] makes: an `Id` outside the shape every key
/// group id takes ([`is_key_group_id`]) is refused as an unreadable identifier, while a
/// well-formed id the store holds no group for is the absent-group sentence
/// ([`no_such_key_group`]). Both are fixed text.
pub fn unresolved_key_group_id(id: &str) -> NoSuchResource {
    if is_key_group_id(id) {
        no_such_key_group()
    } else {
        NoSuchResource {
            message: Some(id_format_invalid_message("Id")),
        }
    }
}

/// The sentence a read of a key value store the store does not hold answers with: the
/// CONDITION, carrying nothing from the request, as every other not-found sentence of this
/// service states its own condition. CloudFront names the resource by its MODELLED shape
/// name (`KeyValueStore`) in this one, as the recorded contract carries it, rather than in
/// the prose form the other sentences use.
pub const KEY_VALUE_STORE_NOT_FOUND_MESSAGE: &str = "The specified KeyValueStore does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for a key value store name the store does
/// not hold — "The entity was not found."
///
/// Takes no identifier: what the refusal states is that the store holds no such resource,
/// which is a property of the store rather than of the value submitted, so the name asked
/// about is no part of the sentence.
pub fn key_value_store_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(KEY_VALUE_STORE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `CannotDeleteEntityWhileInUse` (HTTP 409) for a delete of a resource
/// something this store holds still names.
///
/// FIXED service text ([`ENTITY_IN_USE_MESSAGE`]), as the model states it on the shape
/// itself and for the reason [`function_in_use`] carries one: what the refusal states is a
/// property of the CONDITION — the resource is still in use — rather than of the request,
/// so nothing from the request or from the store belongs in the sentence and every delete
/// this condition refuses reads the same.
pub fn entity_in_use() -> crate::ops::types::CannotDeleteEntityWhileInUse {
    crate::ops::types::CannotDeleteEntityWhileInUse {
        message: Some(ENTITY_IN_USE_MESSAGE.to_string()),
    }
}

/// `CannotDeleteEntityWhileInUse`'s message, as the model documents the shape: "The entity
/// cannot be deleted while it is in use."
pub const ENTITY_IN_USE_MESSAGE: &str = "The entity cannot be deleted while it is in use.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a key value store name that is
/// already taken.
///
/// Carries NO message: the recorded service answers this conflict with the code and the
/// status alone, so there is no sentence to put on the wire and none is invented. The model
/// documents the shape ("The entity already exists. You must provide a unique entity."); that
/// prose states the condition and is not served.
pub fn key_value_store_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists { message: None }
}

/// The modelled `CannotDeleteEntityWhileInUse` (HTTP 409) for a delete of a key value store
/// a function still associates.
///
/// The sentence CloudFront serves for THIS resource kind names the store the request
/// addressed — its one variable part, taken from the request — and nothing else: no function
/// and no ARN, so a caller learns no identifier the request did not already carry. The
/// neighbouring kinds whose conflict is the same code serve the fixed [`ENTITY_IN_USE_MESSAGE`]
/// instead.
pub fn key_value_store_in_use(name: &str) -> crate::ops::types::CannotDeleteEntityWhileInUse {
    crate::ops::types::CannotDeleteEntityWhileInUse {
        message: Some(format!(
            "Cannot delete KeyValueStore {name} because it is associated with a function"
        )),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for a `ListKeyValueStores` `Marker` that is
/// no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is
/// one: what the refusal states is the CONSTRAINT on the parameter — a marker is a key
/// value store name — which is a property of the parameter rather than of the value
/// submitted, so nothing from the request belongs in the text.
pub fn invalid_key_value_store_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_KEY_VALUE_STORE_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListKeyValueStores` refuses an unusable `Marker` with: the constraint on
/// the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the same
/// constraint on the other listing's marker.
pub const INVALID_KEY_VALUE_STORE_MARKER_MESSAGE: &str =
    "Marker must be a valid CF key value store Name";

/// The modelled `EntityNotFound` (HTTP 404) for a distribution id no stored distribution
/// carries, for the operations whose model names `EntityNotFound` rather than
/// `NoSuchDistribution` for that condition.
///
/// The same sentence [`NO_SUCH_DISTRIBUTION_MESSAGE`] states, and for the same reason: the
/// condition is the one both codes report, and it carries no part of the request.
pub fn distribution_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(NO_SUCH_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an ARN, an id or a name —
/// no stored distribution tenant answers to.
///
/// CloudFront's wording states the CONDITION and echoes no part of the request, so it
/// takes no identifier: the same sentence answers an unknown ARN, an unknown id and an
/// unknown name, which is also why every operation that resolves a tenant shares it.
/// Recorded behaviour — the model states only `EntityNotFound`'s own "The entity was not
/// found."
pub fn distribution_tenant_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some("The specified distribution tenant doesn't exist.".to_string()),
    }
}

/// The sentence CloudFront answers `ResourceNotDisabled` with for a distribution tenant,
/// on the wire.
///
/// The WIRE text — what the recorded service answers this code with for THIS resource
/// kind, and what this emulator therefore serves. Like
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from the sentence the MODEL's
/// documentation of the error shape uses ("The specified CloudFront resource hasn't been
/// disabled yet."): that one documents the condition for every resource kind the shape
/// serves, this one is what a tenant's delete answers. Do not "correct" this constant to
/// the documentation sentence.
///
/// A property of the resource's own state rather than of the request, so nothing from the
/// request belongs in it and every refusal of it reads the same.
pub const DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE: &str =
    "You must disable the distribution tenant before you can delete it.";

/// The modelled `ResourceNotDisabled` (HTTP 409) for a delete of a distribution tenant that
/// is still enabled: "To delete a distribution tenant, you must first disable the
/// distribution tenant by using the UpdateDistributionTenant API operation."
pub fn distribution_tenant_not_disabled() -> crate::ops::types::ResourceNotDisabled {
    crate::ops::types::ResourceNotDisabled {
        message: Some(DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a certificate request whose domain ownership it could
/// not verify with: the condition is a property of the domain's DNS rather than of any
/// value the caller sent, so the sentence names none of them — no domain, no tenant name
/// and no id appears in it. Recorded behaviour.
pub const DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE: &str =
    "The provided Domain Name is not valid. Could not verify Domain Name ownership. It may \
     not be pointing to a valid CloudFront resource.";

/// The modelled `InvalidArgument` (HTTP 400) for a CloudFront managed ACM certificate
/// request that asks CloudFront itself to serve the HTTP validation token of a domain whose
/// ownership this service cannot verify
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::unverifiable_certificate_domain`]).
pub fn domain_ownership_unverified() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(DOMAIN_OWNERSHIP_UNVERIFIED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a domain no certificate of this account covers with: what
/// the refusal states is the REQUIREMENT the caller has to satisfy — a certificate on the
/// tenant or on the distribution, or a managed certificate request to mint one — rather than
/// anything about the domain string submitted, so the sentence names no domain, no
/// certificate ARN and no tenant.
///
/// The one sentence for that condition wherever the tenant surface reaches it: a tenant
/// create or update claiming a domain nothing proves, and a domain-conflict listing whose
/// domain control validation resource carries no covering certificate.
pub const MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE: &str =
    "There must be at least one certificate in the distribution tenant or distribution, or \
     there must be a managed certificate request";

/// The modelled `InvalidArgument` (HTTP 400) for a domain no certificate of this account
/// covers ([`MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE`]).
pub fn missing_certificate_for_domain() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(MISSING_CERTIFICATE_FOR_DOMAIN_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses a value that is no domain name at all with: the condition
/// is a property of the value submitted rather than of the account's certificates, so the
/// sentence states that and names the value nowhere. Recorded behaviour.
pub const INVALID_DOMAIN_NAME_MESSAGE: &str = "The request contains an invalid domain name.";

/// The modelled `InvalidArgument` (HTTP 400) for a value that is no domain name at all.
pub fn invalid_domain_name() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_DOMAIN_NAME_MESSAGE.to_string()),
    }
}

/// The modelled `EntityNotFound` (HTTP 404) for a distribution tenant that carries no
/// CloudFront managed ACM certificate: the tenant exists, but no certificate was ever
/// requested for it, so there is no certificate for a read of one to answer from.
pub fn managed_certificate_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(
            "The specified distribution tenant has no CloudFront managed certificate."
                .to_string(),
        ),
    }
}

/// The sentence a read of a connection group this store does not hold answers with, as the
/// recorded `GetConnectionGroupByRoutingEndpoint` contract carries it: the whole sentence
/// is fixed, and no identifier of the group asked about appears in it.
pub const CONNECTION_GROUP_NOT_FOUND_MESSAGE: &str =
    "The specified connection group does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an id, a name, an ARN or a
/// routing endpoint — no connection group this store holds answers to.
///
/// Takes no identifier, so the same sentence answers every form the lookup admits: what
/// the refusal states is that the store holds no such group.
pub fn connection_group_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(CONNECTION_GROUP_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `ResourceNotDisabled` with for a connection group, on
/// the wire.
///
/// The WIRE text — what the recorded service answers this code with for THIS resource
/// kind, and what this emulator therefore serves. `ResourceNotDisabled` is shared by
/// several resource kinds and the served sentence names the kind the caller has to
/// disable, so each kind carries its own constant
/// ([`DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE`] is the tenant's); the sentence the
/// MODEL's documentation of the shape uses ("The specified CloudFront resource hasn't
/// been disabled yet.") documents the condition for all of them and is not served. Do not
/// "correct" this constant to the documentation sentence.
pub const CONNECTION_GROUP_NOT_DISABLED_MESSAGE: &str =
    "You must disable the specified connection group before you can delete it.";

/// The modelled `ResourceNotDisabled` (HTTP 409) for a delete of a connection group that
/// is still enabled.
///
/// FIXED service text ([`CONNECTION_GROUP_NOT_DISABLED_MESSAGE`]), for the reason
/// [`entity_in_use`]'s is fixed: what the refusal states is a property of the CONDITION —
/// the resource has not been disabled — rather than of the request, so nothing from the
/// request or from the store belongs in the sentence and every delete this condition
/// refuses reads the same.
pub fn connection_group_not_disabled() -> crate::ops::types::ResourceNotDisabled {
    crate::ops::types::ResourceNotDisabled {
        message: Some(CONNECTION_GROUP_NOT_DISABLED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a connection group name that is already taken with, on
/// the wire.
///
/// The WIRE text — what the recorded service answers `EntityAlreadyExists` with for THIS
/// resource kind, and what this emulator therefore serves. `EntityAlreadyExists` is shared
/// by several resource kinds and the served sentence names the kind whose name is taken, so
/// each kind carries its own constant ([`DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE`] is
/// the tenant's); the sentence the MODEL's documentation of the shape uses ("The entity
/// already exists. You must provide a unique entity.") documents the condition for all of
/// them and is not served. Do not "correct" this constant to the documentation sentence.
///
/// The WHOLE sentence is fixed: what the refusal states is that the account already holds a
/// group of that name, which is a property of the CONDITION rather than of the request, so
/// neither the submitted name nor the id of the group holding it is any part of it.
pub const CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "A connection group with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a connection group name that is
/// already taken: the name is "unique within your Amazon Web Services account".
///
/// Takes no name: the sentence is fixed service text
/// ([`CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`]), so every refusal of this condition reads
/// the same.
pub fn connection_group_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message CloudFront answers an `AssociationFilter` whose id it cannot use with, on
/// the wire: one CONSTANT sentence for the condition, carrying nothing from the request —
/// the recorded `ListConnectionGroups` contract pins this text, and it is a property of
/// the filter rather than of the value submitted.
pub const INVALID_ASSOCIATION_FILTER_MESSAGE: &str = "The association filter id is invalid.";

/// The modelled `InvalidArgument` (HTTP 400) for an `AssociationFilter` naming an id this
/// service cannot resolve — "An argument is invalid."
pub fn invalid_association_filter() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_ASSOCIATION_FILTER_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for an `AnycastIpListId` that names no
/// Anycast static IP list this service holds — "An argument is invalid."
///
/// The recorded `ListConnectionGroups` contract answers this condition with
/// `InvalidArgument` rather than `EntityNotFound`: the id is an argument CloudFront cannot
/// resolve, so every operation that takes one reports it the same way. Where the argument
/// arrives on an `AssociationFilter`, the sentence is that filter's own
/// ([`invalid_association_filter`]); elsewhere the member it arrived on is named, which a
/// caller cannot work out for itself.
pub fn invalid_anycast_ip_list(id: &str) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: AnycastIpListId {id} is not an Anycast static IP list of \
             this account"
        )),
    }
}

/// The sentence a read of an Anycast IP list this store does not hold answers with, as the
/// recorded `GetAnycastIpList` contract carries it — down to the resource's wording in it
/// and to the absent full stop: the whole sentence is fixed, and the id asked about is no
/// part of it.
pub const ANYCAST_IP_LIST_NOT_FOUND_MESSAGE: &str = "The specified Anycast IP list does not exist";

/// The modelled `EntityNotFound` (HTTP 404) for an Anycast static IP list id no stored
/// list carries — "The entity was not found."
///
/// Takes no id: what the refusal states is that the store holds no such list, which is a
/// property of the store rather than of the value submitted.
pub fn anycast_ip_list_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(ANYCAST_IP_LIST_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `EntityAlreadyExists` (HTTP 409) for an Anycast static IP list name that is
/// already taken: a name identifies ONE list for its whole life, worded as every other
/// already-taken name of this service is.
pub fn anycast_ip_list_already_exists(name: &str) -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(format!(
            "An Anycast static IP list with the name {name} already exists in this account"
        )),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for an `IpCount` no Anycast static IP list can
/// be allocated for.
///
/// What the refusal states is the CONSTRAINT on the parameter — the value set the member
/// documents ([`ANYCAST_IP_COUNTS`]) — which a caller cannot work out from the model alone,
/// since the member carries no `@range`. The value submitted is named beside it, as every
/// other `InvalidArgument` here names the argument it refused, so a caller that sent
/// several learns which one broke.
pub fn invalid_anycast_ip_count(ip_count: i32) -> crate::ops::types::InvalidArgument {
    let admitted: Vec<String> = ANYCAST_IP_COUNTS.iter().map(i32::to_string).collect();
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: IpCount must be one of {}, not {ip_count}",
            admitted.join(", ")
        )),
    }
}

/// The modelled `EntityLimitExceeded` (HTTP 400) for a create there are no Anycast static
/// IP addresses left to allocate for.
///
/// CloudFront words a quota refusal as the condition the request would cause — its
/// `TooManyDistributions` is "Processing your request would cause you to exceed the maximum
/// number of distributions allowed" — so this states the same thing about what the create
/// could not be served: the static IP ADDRESSES it asked to have allocated, which is the
/// condition the caller actually hit. WHY the allocation runs out here is
/// [`ANYCAST_IP_POOL`]'s doc, which a client never reads.
pub fn anycast_ip_pool_exhausted() -> crate::ops::types::EntityLimitExceeded {
    crate::ops::types::EntityLimitExceeded {
        message: Some(
            "Processing your request would cause you to exceed the maximum number of \
             Anycast static IP addresses allowed"
                .to_string(),
        ),
    }
}

/// The sentence CloudFront answers a distribution tenant name that is already taken with,
/// on the wire: the WIRE text for `EntityAlreadyExists` on THIS resource kind, worded as
/// [`CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`] words it for the neighbouring one, and fixed
/// whole for the reason that one is — nothing from the request or from the store belongs in
/// it.
pub const DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE: &str =
    "A distribution tenant with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a distribution tenant name that is
/// already taken: the name is "unique within your Amazon Web Services account".
///
/// Takes no name, for the reason [`connection_group_already_exists`] takes none.
pub fn distribution_tenant_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(DISTRIBUTION_TENANT_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The sentence `CNAMEAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with — and the WHOLE of it:
/// CloudFront states the condition without naming any of the names the request submitted,
/// so nothing from the request is appended to it. Like
/// [`NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE`] it differs from the sentence the MODEL's
/// documentation of the error shape uses for the same rule ("The CNAME specified is already
/// defined for CloudFront."); do not "correct" this constant to the documentation sentence.
pub const CNAME_ALREADY_EXISTS_MESSAGE: &str =
    "One or more of the CNAMEs you provided are already associated with a different resource.";

/// The modelled `CNAMEAlreadyExists` (HTTP 409) for a domain another CloudFront resource
/// already serves.
pub fn cname_already_exists() -> CNAMEAlreadyExists {
    CNAMEAlreadyExists {
        message: Some(CNAME_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message `NoSuchCloudFrontOriginAccessIdentity` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE`], it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("The specified origin access identity does not exist."): the served sentence names the
/// resource as the API does, a CLOUDFRONT origin access identity, while the documentation
/// states the condition in the shorter form. Do not "correct" this constant to the
/// documentation sentence.
pub const NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE: &str =
    "The specified CloudFront origin access identity does not exist.";

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

/// The sentence CloudFront answers `CloudFrontOriginAccessIdentityInUse` with, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from
/// the sentence the MODEL's documentation of the error shape uses for the same condition
/// ("The Origin Access Identity specified is already in use."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
///
/// The sentence names the resource KIND and nothing else: the condition is that something
/// still references the identity, which is a property of the store rather than of the
/// request, and the referencing distribution's own id is NOT part of it — the same shape
/// [`ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE`] takes for the other way an origin reaches a
/// bucket.
pub const ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE: &str =
    "The CloudFront origin access identity is still being used.";

/// The modelled `CloudFrontOriginAccessIdentityInUse` (HTTP 409) for a delete of an
/// identity an Amazon S3 origin still names.
///
/// FIXED service text ([`ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE`]), so every refusal of this
/// condition reads the same and none of them leaks the identifier of the resource holding
/// the reference.
pub fn origin_access_identity_in_use() -> crate::ops::types::CloudFrontOriginAccessIdentityInUse {
    crate::ops::types::CloudFrontOriginAccessIdentityInUse {
        message: Some(ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE.to_string()),
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

/// The modelled `InvalidArgument` for a `ListStreamingDistributions` `Marker` that is no
/// resume point this service ever handed out.
///
/// States the CONSTRAINT on the parameter rather than the value that broke it, the way
/// [`invalid_origin_access_identity_marker`] and [`invalid_public_key_marker`] do for the
/// listings of their own resource kinds.
pub fn invalid_streaming_distribution_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE.to_string()),
    }
}

/// The sentence [`invalid_streaming_distribution_marker`] carries: a `Marker` for this
/// listing is a streaming distribution id.
///
/// The WIRE text, as the recorded service answers this refusal with — the same shape
/// [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] takes for its own listing, down to naming the
/// resource kind the id belongs to.
pub const INVALID_STREAMING_DISTRIBUTION_MARKER_MESSAGE: &str =
    "Marker must be a valid streaming distribution Id";

/// The modelled `NoSuchStreamingDistribution` for a streaming distribution id the store
/// does not hold.
///
/// The id is NOT in the message, for the same reason it is not in
/// [`no_such_distribution`]'s: the sentence is fixed service text, and the caller already
/// knows which distribution it asked for.
pub fn no_such_streaming_distribution() -> crate::ops::types::NoSuchStreamingDistribution {
    crate::ops::types::NoSuchStreamingDistribution {
        message: Some(NO_SUCH_STREAMING_DISTRIBUTION_MESSAGE.to_string()),
    }
}

/// `NoSuchStreamingDistribution`'s message: the sentence the model states on the shape
/// itself, "The specified streaming distribution does not exist."
pub const NO_SUCH_STREAMING_DISTRIBUTION_MESSAGE: &str =
    "The specified streaming distribution does not exist.";

/// The sentence `StreamingDistributionAlreadyExists` carries, up to the streaming
/// distribution the caller reference is already associated with, which is named at the end
/// — the one piece of request-derived detail in it, and the one a caller cannot work out
/// for itself. The same shape [`distribution_already_exists_message`] takes, for the same
/// rule applied to the other resource.
pub fn streaming_distribution_already_exists_message(existing_id: &str) -> String {
    format!(
        "The caller reference that you are using to create a streaming distribution is \
         associated with another streaming distribution. Already exists: {existing_id}"
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

/// The modelled `NoSuchResponseHeadersPolicy` (HTTP 404) for an id neither source of
/// response headers policies holds one for.
///
/// This is the refusal of the STORE: it states that no policy answers to the id submitted,
/// and it says nothing about the id's shape. It is what an operation that resolves such an
/// id as a lookup KEY — one whose own resource is something else, so it never addresses a
/// policy by that id — answers for every id no readable policy exists for, whatever shape
/// the id takes; the sibling policy families answer their own one sentence the same way
/// ([`no_such_cache_policy`], [`no_such_origin_request_policy`]). The shape of the
/// identifier is examined only where an operation ADDRESSES the policy resource itself by
/// it, which is [`unresolved_response_headers_policy_id`].
pub fn no_such_response_headers_policy() -> NoSuchResponseHeadersPolicy {
    NoSuchResponseHeadersPolicy {
        message: Some(NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchResponseHeadersPolicy` (HTTP 404) an operation that ADDRESSES the
/// policy resource by `id` answers when no readable policy answers to it.
///
/// Such an operation checks the SHAPE of the identifier before it looks anything up, and ONE
/// code (and one status) covers both outcomes, so the MESSAGE is what tells them apart: an
/// `id` outside the shape every response headers policy id takes ([`is_minted_uuid`]) is
/// refused as an unusable identifier ([`RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE`]), while
/// a well-formed id that names no policy — neither one this account created nor one AWS
/// published — is the absent-policy sentence ([`no_such_response_headers_policy`]). Both are
/// fixed text: each states a property of the identifier or of the store rather than echoing
/// the value submitted, so nothing from the request reaches the wire.
pub fn unresolved_response_headers_policy_id(id: &str) -> NoSuchResponseHeadersPolicy {
    if is_minted_uuid(id) {
        no_such_response_headers_policy()
    } else {
        NoSuchResponseHeadersPolicy {
            message: Some(id_format_invalid_message("Id")),
        }
    }
}

/// The message `NoSuchResponseHeadersPolicy` carries for an identifier that is not in the
/// shape a response headers policy id takes at all.
///
/// The WIRE text CloudFront answers such a request with: it states that the identifier
/// SUBMITTED is unusable, which is a different condition from a well-formed id that names no
/// policy ([`NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE`]) — the two share the code and the
/// status, so the message is all a caller has to tell them apart. Fixed text; it interpolates
/// nothing, and in particular never echoes the id back.
///
/// Only an operation that ADDRESSES the policy resource by the identifier serves it, through
/// [`unresolved_response_headers_policy_id`]: the precheck belongs to that operation, not to
/// the error code.
///
/// The one PRODUCER of the sentence is [`id_format_invalid_message`], which spells it for any
/// member name; this constant is its `Id` form, kept for the suites that assert the sentence
/// as a `&'static str`, and `id_format_invalid_message_matches_the_constant_form` below pins
/// the two together so neither can drift.
pub const RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE: &str = "Id format is invalid";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListResponseHeadersPolicies` `Marker`
/// that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_cache_policy_marker`] is
/// one: what the refusal states is the CONSTRAINT on the parameter — a marker is a response
/// headers policy id — which is a property of the parameter rather than of the value
/// submitted, so nothing from the request belongs in the text.
pub fn invalid_response_headers_policy_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_RESPONSE_HEADERS_POLICY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListResponseHeadersPolicies` refuses an unusable `Marker` with, in the form
/// this service states the constraint in ([`INVALID_CACHE_POLICY_MARKER_MESSAGE`] states the
/// same one for the cache policy family).
pub const INVALID_RESPONSE_HEADERS_POLICY_MARKER_MESSAGE: &str =
    "Marker must be a valid CF response headers policy Id";

/// The modelled `IllegalUpdate` (HTTP 400) for an update that would move a resource's
/// `CallerReference` onto another value.
///
/// A `CallerReference` "uniquely identifies a request to create a resource" and names that
/// resource for its whole life, so no update changes one — whichever resource is being
/// updated. The sentence states that rule and NOTHING from the request or the stored
/// record: it is the one fixed sentence the service refuses this modification with, so
/// every refusal of it reads the same.
pub fn illegal_update_caller_reference() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for an update that would change a `CallerReference`, on the
/// wire.
pub const ILLEGAL_UPDATE_CALLER_REFERENCE_MESSAGE: &str =
    "You cannot update the value of CallerReference.";

/// The modelled `IllegalUpdate` (HTTP 400) for an update that would move a DISTRIBUTION's
/// `CallerReference` onto another value.
///
/// The same RULE [`illegal_update_caller_reference`] states — a caller reference names its
/// resource for the resource's whole life, so no update moves one — reported in the sentence
/// the DISTRIBUTION surface words it with
/// ([`ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE`]). The wording of a refusal is
/// the service's own per surface, as [`DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE`] and the
/// other wire sentences here are; the sentence takes nothing from the request or the stored
/// record, so every refusal of it reads the same.
pub fn illegal_update_distribution_caller_reference() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for an update that would change a DISTRIBUTION's
/// `CallerReference`, on the wire.
pub const ILLEGAL_UPDATE_DISTRIBUTION_CALLER_REFERENCE_MESSAGE: &str =
    "The caller reference cannot be modified.";

/// The modelled `MonitoringSubscriptionAlreadyExists` (HTTP 409) for a distribution that
/// already carries a monitoring subscription.
///
/// A property of the distribution's state rather than of the request, so nothing from the
/// request belongs in the text, exactly as in [`no_such_distribution`]: one CONSTANT per
/// error code.
pub fn monitoring_subscription_already_exists()
-> crate::ops::types::MonitoringSubscriptionAlreadyExists {
    crate::ops::types::MonitoringSubscriptionAlreadyExists {
        message: Some(MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message `MonitoringSubscriptionAlreadyExists` carries, on the wire.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. Like
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`], it deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("A monitoring
/// subscription already exists for the specified distribution."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
pub const MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE: &str =
    "A monitoring subscription already exists for this distribution";

/// The modelled `NoSuchMonitoringSubscription` (HTTP 404) for a distribution that carries
/// no monitoring subscription. Fixed service text, for the same reason
/// [`monitoring_subscription_already_exists`]'s is.
pub fn no_such_monitoring_subscription() -> NoSuchMonitoringSubscription {
    NoSuchMonitoringSubscription {
        message: Some(NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE.to_string()),
    }
}

/// The message `NoSuchMonitoringSubscription` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE`] and
/// [`DISTRIBUTION_NOT_DISABLED_MESSAGE`], it differs from the sentence the MODEL's
/// documentation of the error shape uses for the same rule ("A monitoring subscription does
/// not exist for the specified distribution."): that one documents the condition, this one
/// is served. Do not "correct" this constant to the documentation sentence.
pub const NO_SUCH_MONITORING_SUBSCRIPTION_MESSAGE: &str =
    "The monitoring subscription does not exist.";

/// The message `NoSuchResponseHeadersPolicy` carries for an id no readable policy answers
/// to, on the wire.
///
/// The WIRE text CloudFront answers this code with when the store holds no policy for the id
/// submitted, in the form this service states that condition in for every other named
/// resource ([`NO_SUCH_CACHE_POLICY_MESSAGE`], [`NO_SUCH_DISTRIBUTION_MESSAGE`],
/// [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`]): "The specified <resource> does not exist."
/// An operation that addresses the policy resource by the identifier serves it for a
/// well-formed id only, since it prechecks the shape
/// ([`unresolved_response_headers_policy_id`]); one that resolves the id as a lookup key
/// serves it for every unknown id ([`no_such_response_headers_policy`]).
/// It differs from the sentence the MODEL's documentation of the error shape uses for the
/// same rule ("The response headers policy does not exist."), which documents the condition
/// rather than stating what is served, so do not "correct" this constant to it.
pub const NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE: &str =
    "The specified response headers policy does not exist.";

/// The modelled `NoSuchOriginAccessControl` (HTTP 404) for an origin access control id
/// the store does not hold.
///
/// The sentence is a property of the store rather than of the request, so nothing from
/// the request belongs in the text.
pub fn no_such_origin_access_control() -> NoSuchOriginAccessControl {
    NoSuchOriginAccessControl {
        message: Some(NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string()),
    }
}

/// `NoSuchOriginAccessControl`'s message, in the form CloudFront answers a `NoSuch…` with
/// for a named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_DISTRIBUTION_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_IDENTITY_MESSAGE`] carry
/// it. Each fills `<resource>` with the resource as the API names it, which for the origin
/// access identity is the longer "CloudFront origin access identity". The recorded service
/// answers this code with that sentence, which the shape's own documentation states in its
/// shorter form.
pub const NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE: &str =
    "The specified origin access control does not exist.";

/// The modelled `InvalidOriginAccessControl` (HTTP 400) for a distribution configuration
/// whose origin names an origin access control this account does not hold.
///
/// The sentence states the condition the configuration broke — the control it names is not
/// one that can be resolved — which is a property of the store rather than of the request,
/// so nothing from the request belongs in the text.
pub fn invalid_origin_access_control() -> crate::ops::types::InvalidOriginAccessControl {
    crate::ops::types::InvalidOriginAccessControl {
        message: Some(INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE.to_string()),
    }
}

/// The message `InvalidOriginAccessControl` carries for an origin that names a control the
/// store cannot resolve, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("The origin access control is not valid."): that one documents the condition, this one
/// is served. Do not "correct" this constant to the documentation sentence.
pub const INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE: &str =
    "The specified origin access control does not exist or is not valid.";

/// The modelled `IllegalOriginAccessConfiguration` (HTTP 400) for a configuration whose
/// origin names BOTH an origin access control and an origin access identity.
///
/// The sentence states the condition the configuration broke — the two mechanisms are
/// mutually exclusive on one origin — which is a property of the configuration's shape
/// rather than of any value in it, so no origin id, control id or identity id belongs in
/// the text.
pub fn illegal_origin_access_configuration() -> crate::ops::types::IllegalOriginAccessConfiguration
{
    crate::ops::types::IllegalOriginAccessConfiguration {
        message: Some(ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE.to_string()),
    }
}

/// The message `IllegalOriginAccessConfiguration` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE`] it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same rule
/// ("An origin cannot contain both an origin access control (OAC) and an origin access
/// identity (OAI)."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const ILLEGAL_ORIGIN_ACCESS_CONFIGURATION_MESSAGE: &str =
    "Cannot use both Origin Access Control and Origin Access Identity on an origin";

/// The message `InvalidLambdaFunctionAssociation` carries for the association whose ARN
/// names no published function version: the fixed sentence stating the FORM the ARN has to
/// have — the function ARN qualified with the number of a published version — and then the
/// value the request supplied for that association, echoed verbatim after " ARN: ".
///
/// A configuration may carry an association on the default cache behaviour and on every
/// ordered one, so the sentence alone does not say WHICH entry was refused; the ARN named
/// at the end is the only thing in the response that identifies it, and it is the request's
/// own string (its partition, Region, account, function name and qualifier all come from
/// the request, never from a constant here). Like [`INVALID_ORIGIN_ACCESS_CONTROL_MESSAGE`]
/// the sentence is the SERVED text, which differs from the one the model's documentation of
/// the error shape uses for the same rule ("The specified Lambda@Edge function association
/// is invalid."): that one describes the condition, this one is sent.
pub fn invalid_lambda_function_association_message(lambda_function_arn: &str) -> String {
    format!("{INVALID_LAMBDA_FUNCTION_ASSOCIATION_PREFIX}{lambda_function_arn}")
}

/// The fixed part of [`invalid_lambda_function_association_message`], up to and including
/// the " ARN: " separator the request's value follows.
pub const INVALID_LAMBDA_FUNCTION_ASSOCIATION_PREFIX: &str =
    "The function ARN must reference a specific function version. (The ARN must end with \
     the version number.) ARN: ";

/// The sentence `OriginAccessControlAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. Like [`CNAME_ALREADY_EXISTS_MESSAGE`] it differs from the sentence the
/// MODEL's documentation of the error shape uses for the same condition ("An origin access
/// control with the specified parameters already exists."): that one documents the
/// condition, this one is served. Do not "correct" this constant to the documentation
/// sentence.
///
/// The WHOLE sentence is fixed: what the refusal states is that the account already holds
/// such a control, so neither the submitted configuration nor the id of the control that
/// holds it is any part of it.
///
/// This constant is the SENTENCE the refusal carries, not the predicate that raises it:
/// what makes a submitted configuration a conflict is `origin_access_control_with_config`'s
/// own rule, which this does not decide. The recorded service words the sentence around the
/// control's name, so a predicate that admitted a second control under a name already taken
/// would never serve it — a question about the predicate, answered where the predicate
/// lives.
pub const ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE: &str =
    "An origin access control with the same name already exists.";

/// The modelled `OriginAccessControlAlreadyExists` (HTTP 409) for a create or an update
/// whose configuration a stored control already holds.
///
/// Takes no identifier: the sentence is fixed service text
/// ([`ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE`]), so both entry points into the
/// uniqueness rule refuse this condition with the one text and neither leaks the id of the
/// control that holds the configuration.
pub fn origin_access_control_already_exists() -> crate::ops::types::OriginAccessControlAlreadyExists
{
    crate::ops::types::OriginAccessControlAlreadyExists {
        message: Some(ORIGIN_ACCESS_CONTROL_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `OriginAccessControlInUse` with, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs from
/// the sentence the MODEL's documentation of the error shape uses for the same condition
/// ("Cannot delete the origin access control because it's in use by one or more
/// distributions."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
///
/// The sentence names the resource KIND and nothing else, for the reason
/// [`ORIGIN_ACCESS_IDENTITY_IN_USE_MESSAGE`] does: the condition is that something still
/// references the control, and the referencing distribution's own id is no part of it.
pub const ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE: &str =
    "The CloudFront origin access control is still being used.";

/// The modelled `OriginAccessControlInUse` (HTTP 409) for a delete of a control an origin
/// still names.
///
/// FIXED service text ([`ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE`]), so every refusal of this
/// condition reads the same and none of them leaks the identifier of the resource holding
/// the reference.
pub fn origin_access_control_in_use() -> crate::ops::types::OriginAccessControlInUse {
    crate::ops::types::OriginAccessControlInUse {
        message: Some(ORIGIN_ACCESS_CONTROL_IN_USE_MESSAGE.to_string()),
    }
}

/// The modelled `InvalidArgument` for an `OriginAccessControlConfig$Name` longer than the
/// documented bound.
///
/// The message states the CONSTRAINT the request broke and the length it submitted, which
/// is the rule the refusal is built from rather than text echoed out of the request.
pub fn origin_access_control_name_too_long(length: usize) -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(format!(
            "An argument is invalid: the origin access control name cannot be longer than \
             {MAX_ORIGIN_ACCESS_CONTROL_NAME} characters but the request submits {length}"
        )),
    }
}

/// The sentence `ResponseHeadersPolicyAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the error
/// shape uses for the same rule ("A response headers policy with this name already exists.
/// You must provide a unique name. To modify an existing response headers policy, use
/// UpdateResponseHeadersPolicy."): that one documents the condition and tells the caller
/// what to do instead, this one is served. Do not "correct" this constant to the
/// documentation sentence.
///
/// The WHOLE sentence is fixed, as [`CACHE_POLICY_ALREADY_EXISTS_MESSAGE`] and
/// [`ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE`] are for the same rule over the other two
/// policy families: nothing from the request or from the store belongs in it.
pub const RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another response headers policy with the same name already exists within the aws account.";

/// The modelled `ResponseHeadersPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken.
///
/// Takes no name: the sentence is fixed service text
/// ([`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`]), so the create and the update refuse
/// a taken name with the one text.
pub fn response_headers_policy_already_exists() -> ResponseHeadersPolicyAlreadyExists {
    ResponseHeadersPolicyAlreadyExists {
        message: Some(RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The modelled `NoSuchOriginRequestPolicy` (HTTP 404) for an origin request policy id the
/// store does not hold.
///
/// The sentence states a property of the store rather than of the request, so nothing from
/// the request belongs in the text.
pub fn no_such_origin_request_policy() -> NoSuchOriginRequestPolicy {
    NoSuchOriginRequestPolicy {
        message: Some(NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE.to_string()),
    }
}

/// `NoSuchOriginRequestPolicy`'s message, in the form CloudFront answers a `NoSuch…` with for
/// a named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_CACHE_POLICY_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] carry it.
/// The recorded service answers this code with that sentence — which is what a replay of the
/// origin request policy surface holds it to — and the shape's own documentation states it in
/// its shorter form.
pub const NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE: &str =
    "The specified origin request policy does not exist.";

/// The sentence `OriginRequestPolicyAlreadyExists` carries, on the wire: the WIRE text for
/// this code, worded as [`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`] words the same
/// rule for the other policy family, and fixed whole for the reason that one is. It differs
/// from the sentence the MODEL's documentation of the shape uses ("An origin request policy
/// with this name already exists. You must provide a unique name. To modify an existing
/// origin request policy, use UpdateOriginRequestPolicy."); do not "correct" this constant
/// to the documentation sentence.
pub const ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another origin request policy with the same name already exists within the aws account.";

/// The modelled `OriginRequestPolicyAlreadyExists` (HTTP 409) for a policy name that is
/// already taken.
///
/// Takes no name, for the reason [`response_headers_policy_already_exists`] takes none.
pub fn origin_request_policy_already_exists() -> OriginRequestPolicyAlreadyExists {
    OriginRequestPolicyAlreadyExists {
        message: Some(ORIGIN_REQUEST_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
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

/// `NoSuchCachePolicy`'s message, in the form CloudFront answers a `NoSuch…` with for a
/// named resource — "The specified <resource> does not exist.", as
/// [`NO_SUCH_DISTRIBUTION_MESSAGE`] and [`NO_SUCH_ORIGIN_ACCESS_CONTROL_MESSAGE`] carry it.
/// The recorded service answers this code with that sentence — which is what a replay of
/// the cache policy surface holds it to — and the shape's own documentation states it in its
/// shorter form.
pub const NO_SUCH_CACHE_POLICY_MESSAGE: &str = "The specified cache policy does not exist.";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListCachePolicies` `Marker` that is no
/// resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is a cache policy
/// id — which is a property of the parameter rather than of the value submitted, so nothing
/// from the request belongs in the text.
pub fn invalid_cache_policy_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_CACHE_POLICY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListCachePolicies` refuses an unusable `Marker` with: the constraint on the
/// parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the same constraint on
/// the other listing's marker.
pub const INVALID_CACHE_POLICY_MARKER_MESSAGE: &str = "Marker must be a valid CF cache policy Id";

/// The modelled `CachePolicyInUse` (HTTP 409) for a delete of a policy a cache behaviour
/// still attaches.
///
/// The sentence is a property of the CONDITION rather than of the request — the policy is
/// associated with a cache behaviour, and the caller is told to disassociate it first — so
/// nothing from the request or from the store belongs in it, and every refusal of this
/// condition reads the same.
pub fn cache_policy_in_use() -> crate::ops::types::CachePolicyInUse {
    crate::ops::types::CachePolicyInUse {
        message: Some(CACHE_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// `CachePolicyInUse`'s message: what CloudFront answers a delete of a cache policy that a
/// cache behaviour still associates.
pub const CACHE_POLICY_IN_USE_MESSAGE: &str =
    "The specified cache policy is currently associated with a cache behavior. Please \
     disassociate the policy before deleting.";

/// The modelled `OriginRequestPolicyInUse` (HTTP 409) for a delete of a policy a cache
/// behaviour still attaches.
///
/// The SAME rule [`cache_policy_in_use`] states for the other policy a cache behaviour
/// attaches: the sentence is a property of the CONDITION rather than of the request — the
/// policy is associated with a cache behaviour, and the caller is told to disassociate it
/// first — so nothing from the request or from the store belongs in it, and every refusal of
/// this condition reads the same.
pub fn origin_request_policy_in_use() -> crate::ops::types::OriginRequestPolicyInUse {
    crate::ops::types::OriginRequestPolicyInUse {
        message: Some(ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE.to_string()),
    }
}

/// `OriginRequestPolicyInUse`'s message: what CloudFront answers a delete of an origin
/// request policy that a cache behaviour still associates, worded as
/// [`CACHE_POLICY_IN_USE_MESSAGE`] words the same condition for the other policy.
pub const ORIGIN_REQUEST_POLICY_IN_USE_MESSAGE: &str =
    "The specified origin request policy is currently associated with a cache behavior. \
     Please disassociate the policy before deleting.";

/// The modelled `IllegalDelete` (HTTP 400) for a delete whose target is a policy AWS
/// published rather than one this account created.
///
/// A published policy is READ-ONLY to the account that reads it — nothing a caller does
/// creates, changes or removes one — so a delete of one is refused for what the REQUEST
/// asked for rather than answered "does not exist": the resource is there, and a caller
/// told it is missing would try to create it. The sentence states that condition and names
/// nothing from the request, so every refusal of it reads the same.
pub fn illegal_delete_managed_policy() -> crate::ops::types::IllegalDelete {
    crate::ops::types::IllegalDelete {
        message: Some(ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE.to_string()),
    }
}

/// `IllegalDelete`'s message, in the form CloudFront answers a delete of a policy AWS owns
/// with: delete is not allowed for it.
pub const ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE: &str =
    "The request failed because delete is not allowed for this policy.";

/// The modelled `IllegalUpdate` (HTTP 400) for an update whose target is a policy AWS
/// published rather than one this account created.
///
/// The SAME rule [`illegal_delete_managed_policy`] states, over the other mutation a policy
/// takes: a published policy is read-only to this account, so the update is refused for
/// what the request asked for and the policy keeps the configuration AWS gave it. The
/// sentence is that one worded for this mutation.
pub fn illegal_update_managed_policy() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE.to_string()),
    }
}

/// `IllegalUpdate`'s message for a policy AWS owns: update is not allowed for it.
pub const ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE: &str =
    "The request failed because update is not allowed for this policy.";

/// The sentence `CachePolicyAlreadyExists` carries, on the wire: the WIRE text for this
/// code, worded as [`RESPONSE_HEADERS_POLICY_ALREADY_EXISTS_MESSAGE`] words the same rule
/// for the other policy family, and fixed whole for the reason that one is. It differs from
/// the sentence the MODEL's documentation of the shape uses ("A cache policy with this name
/// already exists. You must provide a unique name. To modify an existing cache policy, use
/// UpdateCachePolicy."); do not "correct" this constant to the documentation sentence.
pub const CACHE_POLICY_ALREADY_EXISTS_MESSAGE: &str =
    "Another cache policy with the same name already exists within the aws account.";

/// The modelled `CachePolicyAlreadyExists` (HTTP 409) for a policy name that is already
/// taken.
///
/// Takes no name, for the reason [`response_headers_policy_already_exists`] takes none.
pub fn cache_policy_already_exists() -> CachePolicyAlreadyExists {
    CachePolicyAlreadyExists {
        message: Some(CACHE_POLICY_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The longest comment a public key carries.
///
/// `PublicKeyConfig$Comment`: "A comment to describe the public key. The comment cannot be
/// longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_PUBLIC_KEY_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchPublicKey` (HTTP 404) for a public key id the store does not hold.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// public key — so nothing from the request belongs in the text and every path that raises
/// this code reads the same.
pub fn no_such_public_key() -> crate::ops::types::NoSuchPublicKey {
    crate::ops::types::NoSuchPublicKey {
        message: Some(NO_SUCH_PUBLIC_KEY_MESSAGE.to_string()),
    }
}

/// `NoSuchPublicKey`'s message, as CloudFront sends it on the wire — observed in recorded
/// CloudFront responses to this refusal, which is the source this wording comes from. The
/// service names the resource "CloudFront public key" and words the condition "does not
/// exist", as [`NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE`] words the same condition for
/// another resource of this service, rather than in the model's documentation prose ("The
/// specified public key doesn't exist."), and the wire wording is what a client reads. Do not
/// "correct" this constant to the documentation sentence.
pub const NO_SUCH_PUBLIC_KEY_MESSAGE: &str = "The specified CloudFront public key does not exist.";

/// The modelled `PublicKeyInUse` (HTTP 409) for a delete of a public key a stored record
/// still references.
///
/// The SAME rule [`cache_policy_in_use`] states for a policy a cache behaviour attaches: the
/// sentence is a property of the CONDITION rather than of the request — the key is still
/// referenced — so nothing from the request or from the store belongs in it, and every
/// refusal of this condition reads the same.
pub fn public_key_in_use() -> crate::ops::types::PublicKeyInUse {
    crate::ops::types::PublicKeyInUse {
        message: Some(PUBLIC_KEY_IN_USE_MESSAGE.to_string()),
    }
}

/// `PublicKeyInUse`'s message, as CloudFront sends it on the wire — observed in a recorded
/// CloudFront response to this refusal, which is the source this wording comes from: the key
/// is associated with either a key group or a field-level encryption profile and the caller is
/// told to disassociate it before deleting, worded as
/// [`FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE`] and [`CACHE_POLICY_IN_USE_MESSAGE`] word
/// the same condition for the resources they gate, rather than in the model's documentation
/// prose ("The specified public key is in use."). Do not "correct" this constant to the
/// documentation sentence.
pub const PUBLIC_KEY_IN_USE_MESSAGE: &str =
    "The Cloudfront public key is currently associated with either Key Group or a field level \
     encryption profile. Please disassociate the key before deleting.";

/// The longest comment a field-level encryption profile carries.
///
/// `FieldLevelEncryptionProfileConfig$Comment`: "An optional comment for the field-level
/// encryption profile. The comment cannot be longer than 128 characters." The SAME bound,
/// stated in the same words, as [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that
/// constant rather than a second literal, and a comment past it is refused with the one
/// [`comment_too_long`] sentence.
pub const MAX_FIELD_LEVEL_ENCRYPTION_PROFILE_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchFieldLevelEncryptionProfile` (HTTP 404) for a profile id the store
/// does not hold.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// profile — so nothing from the request belongs in the text and every path that raises this
/// code reads the same.
pub fn no_such_field_level_encryption_profile() -> crate::ops::types::NoSuchFieldLevelEncryptionProfile
{
    crate::ops::types::NoSuchFieldLevelEncryptionProfile {
        message: Some(NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE.to_string()),
    }
}

/// `NoSuchFieldLevelEncryptionProfile`'s message, as CloudFront sends it on the wire —
/// observed in a recorded CloudFront response to this refusal, which is the source this
/// wording comes from. The service words it "field level encryption profile does not exist",
/// as [`NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE`] words the same condition for the
/// other field-level encryption resource, rather than in the model's documentation prose
/// ("The specified profile for field-level encryption doesn't exist."), and the wire wording
/// is what a client reads. Do not "correct" this constant to the documentation sentence.
pub const NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE: &str =
    "The specified field level encryption profile does not exist.";

/// The modelled `NoSuchFieldLevelEncryptionProfile` (HTTP 404) a READ of a profile answers
/// an `Id` it cannot resolve.
///
/// `GetFieldLevelEncryptionProfile` models this one not-found code, so one code and one
/// status cover two conditions and the MESSAGE is what tells them apart — the same split
/// [`unresolved_key_group_id`] makes for a key group. An `Id` that is a minted id of ANOTHER
/// resource kind of this service ([`is_minted_entity_id_of_another_kind`]) was never a
/// profile id: the namespaces are told apart by the letter or the form they are minted
/// under, which is a property of the value alone, so CloudFront reports it as the identifier
/// it cannot read ([`id_format_invalid_message`]). Every other `Id` the store holds no
/// profile for is the absent-profile sentence
/// ([`NO_SUCH_FIELD_LEVEL_ENCRYPTION_PROFILE_MESSAGE`]) — a name, a caller reference or a
/// profile id that was simply never minted is a profile that is not there. Both are fixed
/// text.
pub fn unresolved_field_level_encryption_profile_id(
    id: &str,
) -> crate::ops::types::NoSuchFieldLevelEncryptionProfile {
    if is_minted_entity_id_of_another_kind(id) {
        crate::ops::types::NoSuchFieldLevelEncryptionProfile {
            message: Some(id_format_invalid_message("Id")),
        }
    } else {
        no_such_field_level_encryption_profile()
    }
}

/// Whether `value` is a minted id of a resource kind OTHER than a field-level encryption
/// profile: an id of the sibling field-level encryption CONFIG, a public key, a distribution,
/// or one of the UUID-keyed policy kinds.
///
/// Each of those namespaces is disjoint from a profile id's
/// ([`is_field_level_encryption_profile_id`]), so a value in one of them could never have
/// been a profile id — which is what separates an identifier of the WRONG RESOURCE KIND from
/// a profile id, a name or a caller reference the store simply holds nothing for.
fn is_minted_entity_id_of_another_kind(value: &str) -> bool {
    is_field_level_encryption_config_id(value)
        || is_public_key_id(value)
        || is_distribution_id(value)
        || is_minted_uuid(value)
}

/// The modelled `FieldLevelEncryptionProfileInUse` (HTTP 409) for a delete of a profile a
/// stored field-level encryption configuration still names.
///
/// The SAME rule [`field_level_encryption_config_in_use`] states for the configuration a
/// cache behaviour names: the sentence is a property of the CONDITION rather than of the
/// request — the profile is still referenced — so nothing from the request or from the store
/// belongs in it, and every refusal of this condition reads the same.
pub fn field_level_encryption_profile_in_use(
) -> crate::ops::types::FieldLevelEncryptionProfileInUse {
    crate::ops::types::FieldLevelEncryptionProfileInUse {
        message: Some(FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE.to_string()),
    }
}

/// `FieldLevelEncryptionProfileInUse`'s message, as CloudFront sends it on the wire —
/// observed in a recorded CloudFront response to this refusal, which is the source this
/// wording comes from: the profile is associated with a field-level encryption
/// configuration and the caller is told to disassociate it before deleting, worded as
/// [`FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE`] and [`CACHE_POLICY_IN_USE_MESSAGE`] word
/// the same condition for the resources they gate, rather than in the model's documentation
/// prose ("The specified profile for field-level encryption is in use."). Do not "correct"
/// this constant to the documentation sentence.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_IN_USE_MESSAGE: &str =
    "The field level encryption profile is currently associated with a field level encryption \
     config. Please disassociate the profile before deleting.";

/// The sentence `FieldLevelEncryptionProfileAlreadyExists` carries — "The specified profile
/// for field-level encryption already exists." — up to the profile the caller reference is
/// already associated with, which it names at the end: the one piece of store-derived detail
/// in it, and the one a caller cannot work out for itself. The same shape
/// [`origin_access_identity_already_exists_message`] takes, for the same rule applied to the
/// other resource.
pub fn field_level_encryption_profile_already_exists_message(existing_id: &str) -> String {
    format!(
        "The specified profile for field-level encryption already exists: {existing_id}"
    )
}

/// The modelled `TooManyFieldLevelEncryptionProfiles` (HTTP 400) for a create that would
/// take the store past [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`].
///
/// The sentence is the one the model states on the shape itself — "The maximum number of
/// profiles for field-level encryption have been created." — a property of the account's
/// stored profiles rather than of the request, so nothing from the request belongs in the
/// text; it lives here beside every other modelled error text of this service rather than in
/// the handler.
pub fn too_many_field_level_encryption_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionProfiles`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_PROFILES_MESSAGE: &str =
    "The maximum number of profiles for field-level encryption have been created.";

/// The longest comment a field-level encryption configuration carries.
///
/// `FieldLevelEncryptionConfig$Comment`: "An optional comment about the configuration. The
/// comment cannot be longer than 128 characters." The SAME bound, stated in the same words,
/// as [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONFIG_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `NoSuchFieldLevelEncryptionConfig` (HTTP 404) for a configuration id the
/// store does not hold.
///
/// The sentence states a property of the store rather than of the request — the id names no
/// configuration — so nothing from the request belongs in the text and every refusal of this
/// condition reads the same.
pub fn no_such_field_level_encryption_config() -> crate::ops::types::NoSuchFieldLevelEncryptionConfig
{
    crate::ops::types::NoSuchFieldLevelEncryptionConfig {
        message: Some(NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE.to_string()),
    }
}

/// `NoSuchFieldLevelEncryptionConfig`'s message, as CloudFront sends it on the wire — observed
/// in a recorded CloudFront response to this refusal, which is the source this wording comes
/// from. The service words it "config does not exist" rather than in the model's documentation
/// prose ("The specified configuration for field-level encryption doesn't exist."), and the
/// wire wording is what a client reads.
pub const NO_SUCH_FIELD_LEVEL_ENCRYPTION_CONFIG_MESSAGE: &str =
    "The specified field level encryption config does not exist.";

/// The modelled `FieldLevelEncryptionConfigInUse` (HTTP 409) for a delete of a configuration
/// a cache behaviour still names.
///
/// The SAME rule [`cache_policy_in_use`] states for a policy a cache behaviour attaches: the
/// sentence is a property of the CONDITION rather than of the request — the configuration is
/// associated with a cache behaviour, and the caller is told to disassociate it first — so
/// nothing from the request or from the store belongs in it, and every refusal of this
/// condition reads the same.
pub fn field_level_encryption_config_in_use(
) -> crate::ops::types::FieldLevelEncryptionConfigInUse {
    crate::ops::types::FieldLevelEncryptionConfigInUse {
        message: Some(FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE.to_string()),
    }
}

/// `FieldLevelEncryptionConfigInUse`'s message, as CloudFront sends it on the wire — observed
/// in a recorded CloudFront response to this refusal, which is the source this wording comes
/// from: the configuration is associated with a cache behaviour and the caller is told to
/// disassociate it before deleting, worded as [`CACHE_POLICY_IN_USE_MESSAGE`] words the same
/// condition for a policy a cache behaviour attaches, rather than in the model's documentation
/// prose ("The specified configuration for field-level encryption is in use.").
pub const FIELD_LEVEL_ENCRYPTION_CONFIG_IN_USE_MESSAGE: &str =
    "The field level encryption config is currently associated with a cache behavior. Please \
     disassociate the config before deleting.";

/// The sentence `FieldLevelEncryptionConfigAlreadyExists` carries — "The specified
/// configuration for field-level encryption already exists." — up to the configuration the
/// caller reference is already associated with, which it names at the end: the one piece of
/// store-derived detail in it, and the one a caller cannot work out for itself. The same
/// shape [`field_level_encryption_profile_already_exists_message`] takes, for the same rule
/// applied to the other resource.
pub fn field_level_encryption_config_already_exists_message(existing_id: &str) -> String {
    format!("The specified configuration for field-level encryption already exists: {existing_id}")
}

/// The modelled `TooManyFieldLevelEncryptionConfigs` (HTTP 400) for a create that would take
/// the store past [`MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS`].
///
/// The sentence is the one the model states on the shape itself — "The maximum number of
/// configurations for field-level encryption have been created." — a property of the
/// account's stored configurations rather than of the request, so nothing from the request
/// belongs in the text.
pub fn too_many_field_level_encryption_configs(
) -> crate::ops::types::TooManyFieldLevelEncryptionConfigs {
    crate::ops::types::TooManyFieldLevelEncryptionConfigs {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONFIGS_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionConfigs`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONFIGS_MESSAGE: &str =
    "The maximum number of configurations for field-level encryption have been created.";

/// The modelled `TooManyFieldLevelEncryptionQueryArgProfiles` (HTTP 400) for a configuration
/// naming more query argument-profile mappings than
/// [`MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES`].
///
/// The sentence is the one the model states on the shape itself, a property of the bound
/// rather than of the request.
pub fn too_many_field_level_encryption_query_arg_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionQueryArgProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionQueryArgProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionQueryArgProfiles`'s message, as the model documents the shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES_MESSAGE: &str =
    "The maximum number of query arg profiles for field-level encryption have been created.";

/// The modelled `TooManyFieldLevelEncryptionContentTypeProfiles` (HTTP 400) for a
/// configuration naming more content type-profile mappings than
/// [`MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES`].
///
/// The sentence is the one the model states on the shape itself, a property of the bound
/// rather than of the request.
pub fn too_many_field_level_encryption_content_type_profiles(
) -> crate::ops::types::TooManyFieldLevelEncryptionContentTypeProfiles {
    crate::ops::types::TooManyFieldLevelEncryptionContentTypeProfiles {
        message: Some(TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES_MESSAGE.to_string()),
    }
}

/// `TooManyFieldLevelEncryptionContentTypeProfiles`'s message, as the model documents the
/// shape.
pub const TOO_MANY_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES_MESSAGE: &str =
    "The maximum number of content type profiles for field-level encryption have been created.";

/// The modelled `QueryArgProfileEmpty` (HTTP 400) for a query argument-profile mapping that
/// names no profile to encrypt the matching fields with.
///
/// The sentence is the one the model states on the shape itself — "No profile specified for
/// the field-level encryption query argument." — a property of the mapping rather than of
/// any one value in it, so nothing from the request belongs in the text.
pub fn query_arg_profile_empty() -> crate::ops::types::QueryArgProfileEmpty {
    crate::ops::types::QueryArgProfileEmpty {
        message: Some(QUERY_ARG_PROFILE_EMPTY_MESSAGE.to_string()),
    }
}

/// `QueryArgProfileEmpty`'s message, as the model documents the shape.
pub const QUERY_ARG_PROFILE_EMPTY_MESSAGE: &str =
    "No profile specified for the field-level encryption query argument.";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListPublicKeys` `Marker` that is no
/// resume point this service handed out.
///
/// ONE CONSTANT for the condition rather than a sentence built from the request: what the
/// refusal states is the CONSTRAINT on the parameter — a marker is a public key id — which
/// is a property of the parameter and not of the value submitted, so nothing from the
/// request belongs in the text. The value below is the WIRE text, what the recorded
/// service answers this code with, exactly as
/// [`MONITORING_SUBSCRIPTION_ALREADY_EXISTS_MESSAGE`] is; do not reword it into a sentence
/// about the submitted value.
pub fn invalid_public_key_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_PUBLIC_KEY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListPublicKeys` refuses an unusable `Marker` with, on the wire.
pub const INVALID_PUBLIC_KEY_MARKER_MESSAGE: &str = "Marker must be a valid CF public key Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListFieldLevelEncryptionConfigs` `Marker`
/// that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is a field-level
/// encryption configuration id — which is a property of the parameter rather than of the
/// value submitted, so nothing from the request belongs in the text.
pub fn invalid_field_level_encryption_config_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListFieldLevelEncryptionConfigs` refuses an unusable `Marker` with: the
/// constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the
/// same constraint on the other listing's marker, naming this listing's own resource.
pub const INVALID_FIELD_LEVEL_ENCRYPTION_CONFIG_MARKER_MESSAGE: &str =
    "Marker must be a valid CF field-level encryption config Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListFieldLevelEncryptionProfiles`
/// `Marker` that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is a field-level
/// encryption profile id — which is a property of the parameter rather than of the value
/// submitted, so nothing from the request belongs in the text.
pub fn invalid_field_level_encryption_profile_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListFieldLevelEncryptionProfiles` refuses an unusable `Marker` with: the
/// constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words the
/// same constraint on the other listing's marker, naming this listing's own resource.
pub const INVALID_FIELD_LEVEL_ENCRYPTION_PROFILE_MARKER_MESSAGE: &str =
    "Marker must be a valid CF field-level encryption profile Id";

/// The modelled `InvalidArgument` (HTTP 400) for a `ListCloudFrontOriginAccessIdentities`
/// `Marker` that is no resume point this service handed out.
///
/// ONE CONSTANT for the condition, for the same reason [`invalid_public_key_marker`] is one:
/// what the refusal states is the CONSTRAINT on the parameter — a marker is an origin access
/// identity id — which is a property of the parameter rather than of the value submitted, so
/// nothing from the request belongs in the text.
pub fn invalid_origin_access_identity_marker() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE.to_string()),
    }
}

/// The message `ListCloudFrontOriginAccessIdentities` refuses an unusable `Marker` with:
/// the constraint on the parameter, worded as [`INVALID_PUBLIC_KEY_MARKER_MESSAGE`] words
/// the same constraint on the other listing's marker.
pub const INVALID_ORIGIN_ACCESS_IDENTITY_MARKER_MESSAGE: &str =
    "Marker must be a valid CF origin access identity Id";

/// The modelled `TooManyPublicKeys` (HTTP 400) for a create that would take the store past
/// [`MAX_PUBLIC_KEYS`] — "The maximum number of public keys for field-level encryption have
/// been created. To create a new public key, delete one of the existing keys."
///
/// The sentence states the CONSTRAINT and the number this emulator holds to, which is the
/// rule the refusal is built from rather than text echoed out of the request; it lives here
/// beside every other modelled error text of this service rather than in the handler.
pub fn too_many_public_keys() -> crate::ops::types::TooManyPublicKeys {
    crate::ops::types::TooManyPublicKeys {
        message: Some(format!(
            "The maximum number of public keys have been created ({MAX_PUBLIC_KEYS}). To \
             create a new public key, delete one of the existing keys."
        )),
    }
}

/// The sentence `PublicKeyAlreadyExists` carries, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape uses
/// ("The specified public key already exists."): that one documents the condition, this one
/// states what the condition IS — the caller reference the request carried already names a
/// key of this account. Do not "correct" this constant to the documentation sentence.
///
/// The WHOLE sentence is fixed: the conflict is over the reference the REQUEST submitted, so
/// the caller already holds it, and the identifier of the key that holds it is no part of the
/// text.
pub const PUBLIC_KEY_ALREADY_EXISTS_MESSAGE: &str =
    "The caller reference that you are using to create a CloudFront public key is associated \
     with another public key in your account.";

/// The longest comment a key group carries.
///
/// `KeyGroupConfig$Comment`: "A comment to describe the key group. The comment cannot be
/// longer than 128 characters." The SAME bound, stated in the same words, as
/// [`MAX_ORIGIN_ACCESS_IDENTITY_COMMENT`] — so it is that constant rather than a second
/// literal, and a comment past it is refused with the one [`comment_too_long`] sentence.
pub const MAX_KEY_GROUP_COMMENT: usize = MAX_ORIGIN_ACCESS_IDENTITY_COMMENT;

/// The modelled `KeyGroupAlreadyExists` (HTTP 409) for a key group name that is already
/// taken.
///
/// The sentence is fixed service text ([`KEY_GROUP_ALREADY_EXISTS_MESSAGE`]) and it names
/// no part of the request: what it reports is that the account already holds a group of
/// that name, which is a property of the store rather than of the value submitted.
pub fn key_group_already_exists() -> crate::ops::types::KeyGroupAlreadyExists {
    crate::ops::types::KeyGroupAlreadyExists {
        message: Some(KEY_GROUP_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// `KeyGroupAlreadyExists`'s message, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this emulator
/// therefore serves. It differs from the sentence the MODEL's documentation of the shape
/// uses for the same rule ("A key group with this name already exists. You must provide a
/// unique name. To modify an existing key group, use UpdateKeyGroup."): that one documents
/// the condition and tells the caller what to do instead, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const KEY_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "Another key group with the same name already exists within the aws account.";

/// The modelled `NoSuchResource` (HTTP 404) for a key group id the store does not hold.
///
/// `NoSuchResource` is the one error shape every operation of the key group surface models
/// for a missing resource, and CloudFront words it for the resource that operation
/// ADDRESSES — a key group, here — rather than with the generic sentence the tagging
/// surface answers for an arbitrary ARN ([`no_such_resource`]). The sentence states a
/// property of the store, so nothing from the request belongs in it.
pub fn no_such_key_group() -> NoSuchResource {
    NoSuchResource {
        message: Some(NO_SUCH_KEY_GROUP_MESSAGE.to_string()),
    }
}

/// The key group surface's `NoSuchResource` message, in the form CloudFront answers a
/// missing named resource with — "The specified <resource> does not exist.", as
/// [`NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE`] and [`NO_SUCH_CACHE_POLICY_MESSAGE`] carry it.
pub const NO_SUCH_KEY_GROUP_MESSAGE: &str = "The specified key group does not exist.";

/// The modelled `TooManyKeyGroups` (HTTP 400) for a create that would take the store past
/// [`MAX_KEY_GROUPS`].
///
/// The sentence is the one the model states on the shape itself — a property of the
/// account's stored key groups rather than of the request, so nothing from the request
/// belongs in the text; it lives here beside every other modelled error text of this
/// service rather than in the handler.
pub fn too_many_key_groups() -> crate::ops::types::TooManyKeyGroups {
    crate::ops::types::TooManyKeyGroups {
        message: Some(TOO_MANY_KEY_GROUPS_MESSAGE.to_string()),
    }
}

/// `TooManyKeyGroups`'s message, as the model documents the shape.
pub const TOO_MANY_KEY_GROUPS_MESSAGE: &str =
    "You have reached the maximum number of key groups for this Amazon Web Services \
     account. For more information, see Quotas (formerly known as limits) in the Amazon \
     CloudFront Developer Guide.";

/// The modelled `TooManyPublicKeysInKeyGroup` (HTTP 400) for a configuration that names
/// more public keys than [`MAX_PUBLIC_KEYS_IN_KEY_GROUP`].
///
/// The sentence is the one the model states on the shape itself: what it reports is the
/// published quota on one group's key list, which is a property of the parameter rather
/// than of the value submitted.
pub fn too_many_public_keys_in_key_group() -> crate::ops::types::TooManyPublicKeysInKeyGroup {
    crate::ops::types::TooManyPublicKeysInKeyGroup {
        message: Some(TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE.to_string()),
    }
}

/// `TooManyPublicKeysInKeyGroup`'s message, as the model documents the shape.
pub const TOO_MANY_PUBLIC_KEYS_IN_KEY_GROUP_MESSAGE: &str =
    "The number of public keys in this key group is more than the maximum allowed. For \
     more information, see Quotas (formerly known as limits) in the Amazon CloudFront \
     Developer Guide.";

/// The modelled `InvalidArgument` (HTTP 400) for a key group configuration that names no
/// public key at all.
///
/// `CreateKeyGroup` documents the requirement — "To create a key group, you must specify
/// at least one public key for the key group" — and the model's `PublicKeyIdList` carries
/// no length constraint, so the bound is not refused before a handler runs and the create
/// path checks it itself. The sentence states the CONSTRAINT rather than echoing the
/// request, as [`comment_too_long`] does for the other documented bound on a
/// configuration.
pub fn key_group_without_a_public_key() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE.to_string()),
    }
}

/// The message a key group create naming no public key is refused with.
pub const KEY_GROUP_WITHOUT_A_PUBLIC_KEY_MESSAGE: &str =
    "An argument is invalid: to create a key group, you must specify at least one public \
     key for the key group";

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

    /// The one producer of the identifier-shape sentence and the `&'static str` form the
    /// test suites assert cannot drift apart: both spell the same text for `Id`, and the
    /// producer spells it for any other member name the same way.
    #[test]
    fn id_format_invalid_message_matches_the_constant_form() {
        assert_eq!(
            id_format_invalid_message("Id"),
            RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
            "case Id: expected the constant form {RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE:?}, \
             got {:?}",
            id_format_invalid_message("Id")
        );
        assert_eq!(
            id_format_invalid_message("DistributionTenantId"),
            "DistributionTenantId format is invalid",
            "case DistributionTenantId: expected the member's own sentence, got {:?}",
            id_format_invalid_message("DistributionTenantId")
        );
    }

    /// Each MINT's predicate admits exactly what this service mints for that kind — the
    /// resume points a listing issues are its own ids — while the form an operation
    /// ADDRESSES a resource with ([`is_namespaced_resource_id`]) admits every namespace an
    /// opaque id is written under, whatever label, width and alphabet it takes, and nothing
    /// that carries no namespace at all.
    #[test]
    fn an_id_predicate_admits_only_the_shape_its_mint_renders() {
        let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
        for _ in 0..100 {
            let list = state.allocate_anycast_ip_list_id();
            assert!(
                is_namespaced_resource_id(&list),
                "case allocate_anycast_ip_list_id: expected {list:?} to be an addressable id"
            );
            assert!(
                !is_namespaced_resource_id(&list.to_uppercase()),
                "case upper-cased: expected {list:?} upper-cased not to be an addressable id, \
                 since a namespace label is lowercase"
            );
            assert!(
                is_namespaced_resource_id(&list[..list.len() - 1]),
                "case one digit short: expected {list:?} truncated to still be an addressable id, \
                 since the width is one observed id's rather than the rule"
            );

            let tenant = state.allocate_distribution_tenant_id();
            assert!(
                is_distribution_tenant_id(&tenant),
                "case allocate_distribution_tenant_id: expected {tenant:?} to be a tenant id"
            );
            assert!(
                !is_distribution_tenant_id(&tenant.to_lowercase()),
                "case lower-cased: expected {tenant:?} lower-cased not to be a minted tenant id"
            );
            assert!(
                is_namespaced_resource_id(&tenant)
                    && is_namespaced_resource_id(&tenant.to_lowercase()),
                "case addressing {tenant:?}: expected both cases of the body to be addressable, \
                 since the body's alphabet is this mint's rather than the rule"
            );

            let group = state.allocate_key_group_id();
            assert!(
                is_key_group_id(&group),
                "case allocate_key_group_id: expected {group:?} to be a key group id"
            );
            assert!(
                !is_key_group_id(&group.to_uppercase()),
                "case upper-cased: expected {group:?} upper-cased not to be a key group id"
            );

            let policy = state.allocate_continuous_deployment_policy_id();
            assert!(
                is_continuous_deployment_policy_id(&policy),
                "case allocate_continuous_deployment_policy_id: expected {policy:?} to be a \
                 policy id"
            );
        }

        // Across the kinds: no id of one kind is read as an id of another, and no bare word
        // is read as an id at all.
        for value in ["EDFDVBD6EXAMPLE", "nope", "-", ""] {
            assert!(
                !is_namespaced_resource_id(value),
                "case {value:?}: expected it not to be an addressable namespaced id"
            );
            assert!(
                !is_distribution_tenant_id(value),
                "case {value:?}: expected it not to be a distribution tenant id"
            );
            assert!(
                !is_key_group_id(value),
                "case {value:?}: expected it not to be a key group id"
            );
        }

        // The addressable form spans the namespaces CloudFront writes an opaque id under:
        // another label, another body width, another body alphabet and another separator are
        // each an id the store decides on.
        for value in [
            "dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa",
            "aip_1A2B3C4D5E6F7G8H9J0K1L",
            "ipl-notaminted-id",
            "cg_ZZZZZZZZZZZZZ",
            "zz_0",
        ] {
            assert!(
                is_namespaced_resource_id(value),
                "case {value:?}: expected a namespaced id to be addressable"
            );
        }

        // And it refuses what carries no namespace: no separator at all, an empty or
        // over-long label, an upper-case label, a body that is empty or outside the
        // identifier alphabet.
        let grossly_malformed = "A".repeat(512);
        for value in [
            grossly_malformed.as_str(),
            "iplist0123456789",
            "-ipl-0",
            "present-list",
            "IPL-0123456789abcdef0",
            "dt_",
            "dt_token with a space",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e",
        ] {
            assert!(
                !is_namespaced_resource_id(value),
                "case {value:?}: expected a value of no namespace not to be addressable"
            );
        }
    }

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
