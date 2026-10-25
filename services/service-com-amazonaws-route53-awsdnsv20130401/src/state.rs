// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53-awsdnsv20130401 service.
//!
//! Route 53 is a control plane over the hosted zone and, inside it, the
//! resource record set, plus the traffic policy a record set can be generated
//! from and the health check a record set can be routed by. They all live here,
//! in ONE store, so every operation reads the records
//! another operation wrote. The helpers in this module are the
//! shared vocabulary the handlers in `src/ops/` use for them — DNS name
//! normalization, the sort key `ListResourceRecordSets` orders by, the
//! `/hostedzone/` and `/change/` id prefixes the API wraps ids in, and the id
//! minting every create needs.
//!
//! Beside those caller-owned resources this store also holds the one
//! SERVICE-owned record the API reports: the CIDR blocks Route 53's health
//! checkers send their checks from ([`CHECKER_IP_RANGES`]). No operation
//! creates, changes or deletes one — the set belongs to the service, not to the
//! account — so it is built into this struct's [`Default`], which the engine
//! constructs the first time a handler reaches the store. Seeding it there,
//! rather than on the first read, is what keeps `GetCheckerIpRanges` a pure
//! reporting operation: it reads the record and writes nothing, so a read can
//! never change what a later caller sees.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write as _;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::get_change_types::NoSuchChange;
use crate::ops::types::shared_types::{
    ChangeInfo, DelegationSet, HealthCheck, HealthCheckConfig, HostedZone, HostedZoneConfig,
    NoSuchHealthCheck, NoSuchHostedZone, NoSuchTrafficPolicy, ResourceRecordSet, Tag, TrafficPolicy,
    VPC, R_R_TYPE_VALUES, V_P_C_REGION_VALUES,
};

/// How long after a change is submitted it reaches `INSYNC`.
///
/// `CreateHostedZone`'s own model documentation: "When you submit a
/// CreateHostedZone request, the initial status of the hosted zone is PENDING. …
/// When the NS and SOA records are available, the status of the zone changes to
/// INSYNC." Real propagation takes up to a minute; the emulator owes the same
/// transition, so the span is short but NEVER zero — a caller that reads the
/// change straight back still observes `PENDING`, exactly as it would against
/// Route 53.
pub const CHANGE_PROPAGATION: Duration = Duration::from_millis(500);

/// The `Status` of a change that has not propagated yet (`ChangeStatus` enum).
pub const STATUS_PENDING: &str = "PENDING";
/// The `Status` of a change that has propagated (`ChangeStatus` enum).
pub const STATUS_INSYNC: &str = "INSYNC";

/// The most hosted zones the account may hold.
///
/// A zone is created with a caller-chosen `CallerReference`, so without a cap a
/// caller could add zones without end, each of them allowed to reach
/// [`MAX_RECORD_SETS_PER_ZONE`]. This is Route 53's own documented account
/// quota ("Hosted zones: 500 per account"), and the model declares
/// `TooManyHostedZones` for exactly this refusal: "This operation can't be
/// completed either because the current account has reached the limit on the
/// number of hosted zones".
pub const MAX_HOSTED_ZONES: usize = 500;

/// The most changes one `ChangeBatch` may carry.
///
/// The model puts no length constraint on `Changes`, so nothing upstream of the
/// handler bounds it. This is Route 53's own documented service quota — "Maximum
/// number of changes in a single ChangeResourceRecordSets request: 1,000" — so
/// the work one call can ask of the handler is bounded by the same number the
/// real service bounds it by.
pub const MAX_CHANGES_PER_BATCH: usize = 1_000;

/// The most record sets one hosted zone may hold.
///
/// Route 53's documented per-zone quota ("Records per hosted zone: 10,000"), so
/// repeated CREATE/UPSERT cannot grow a zone without end.
pub const MAX_RECORD_SETS_PER_ZONE: usize = 10_000;

/// The most `InvalidChangeBatch` messages one refusal reports.
///
/// A refusal names the changes that were wrong, and a batch may be a thousand
/// changes long, so the message list is capped and the refusal says how many
/// more there were: the error body cannot be amplified by the batch size.
pub const MAX_REPORTED_CHANGE_PROBLEMS: usize = 32;

/// The most changes the ledger retains.
///
/// Every mutating operation appends one, and no API operation deletes one, so
/// without a cap the ledger would grow for the life of the process. Past this
/// many the oldest are evicted (see [`Route53State::retain_recent_changes`]);
/// the cap is far above any single client session's change count, so a change a
/// caller is polling is never the one evicted.
const MAX_RETAINED_CHANGES: usize = 10_000;

/// How long a change stays readable once it has reached `INSYNC`.
///
/// A change is interesting until it has propagated and the caller has seen that
/// it did; an hour past `INSYNC` is far longer than any client waits, and it
/// gives the ledger a retention answer rather than unbounded growth.
const CHANGE_RETENTION: Duration = Duration::from_secs(3_600);

/// The most tags one taggable resource may carry.
///
/// `ChangeTagsForResource$AddTags`'s own documentation: "You can add a maximum
/// of 10 tags to a health check or a hosted zone", and the modelled `TagList`
/// behind `ResourceTagSet$Tags` is `len 1..10`, so a resource holding more than
/// ten tags could not even be reported on the wire.
pub const MAX_TAGS_PER_RESOURCE: usize = 10;

/// The most health checks the account may hold.
///
/// Route 53's documented account quota ("Health checks: 200 per account"), and
/// the model declares `TooManyHealthChecks` for the refusal that enforces it.
pub const MAX_HEALTH_CHECKS: usize = 200;

/// The `HealthCheckType` values that check an endpoint of the caller's, rather
/// than reading a CloudWatch alarm, combining other health checks or following a
/// routing control (`HealthCheckType` enum).
///
/// These are the types whose `HealthCheckConfig` is about reaching an endpoint,
/// so they are the ones that need an endpoint to reach and that carry the
/// `RequestInterval` / `FailureThreshold` defaults below.
pub const ENDPOINT_HEALTH_CHECK_TYPES: &[&str] =
    &["HTTP", "HTTPS", "HTTP_STR_MATCH", "HTTPS_STR_MATCH", "TCP"];

/// The `HealthCheckType` values that search the response body for a string
/// (`HealthCheckType` enum).
pub const STRING_MATCH_HEALTH_CHECK_TYPES: &[&str] = &["HTTP_STR_MATCH", "HTTPS_STR_MATCH"];

/// The `HealthCheckType` that reads a CloudWatch alarm (`HealthCheckType` enum).
pub const HEALTH_CHECK_TYPE_CLOUDWATCH_METRIC: &str = "CLOUDWATCH_METRIC";
/// The `HealthCheckType` that combines other health checks (`HealthCheckType` enum).
pub const HEALTH_CHECK_TYPE_CALCULATED: &str = "CALCULATED";
/// The `HealthCheckType` that follows a routing control (`HealthCheckType` enum).
pub const HEALTH_CHECK_TYPE_RECOVERY_CONTROL: &str = "RECOVERY_CONTROL";

/// The `RequestInterval`, in seconds, a health check that asks for none gets.
///
/// `HealthCheckConfig$RequestInterval`'s own documentation: "If you don't specify
/// a value for RequestInterval, the default value is 30 seconds."
pub const DEFAULT_REQUEST_INTERVAL: i32 = 30;

/// The `FailureThreshold` a health check that asks for none gets.
///
/// `HealthCheckConfig$FailureThreshold`'s own documentation: "if you don't
/// specify a value for FailureThreshold, the default value is three health
/// checks."
pub const DEFAULT_FAILURE_THRESHOLD: i32 = 3;

/// The version a newly created health check carries.
///
/// `HealthCheckVersion` is `range 1..`, and `UpdateHealthCheck` moves it on, so a
/// health check starts at the bottom of that range.
pub const FIRST_HEALTH_CHECK_VERSION: i64 = 1;

/// The most Amazon VPCs one private hosted zone may be associated with.
///
/// Route 53's documented per-zone quota ("Amazon VPCs that you can associate
/// with a private hosted zone: 100"), which is the limit
/// `HostedZoneLimitType`'s `MAX_VPCS_ASSOCIATED_BY_ZONE` reports and which
/// `AssociateVPCWithHostedZone`'s modelled `LimitsExceeded` refuses past, so
/// repeated associations cannot grow one zone's association list without end.
pub const MAX_VPCS_PER_HOSTED_ZONE: usize = 100;

/// The most VPC association authorizations one hosted zone may hold.
///
/// Route 53's documented per-zone quota ("Authorizations for associating VPCs
/// with a private hosted zone: 1,000 per hosted zone"), and the model declares
/// `TooManyVPCAssociationAuthorizations` for exactly this refusal: "You've
/// created the maximum number of authorizations that can be created for the
/// specified hosted zone. To authorize another VPC to be associated with the
/// hosted zone, submit a DeleteVPCAssociationAuthorization request to remove an
/// existing authorization." Without it a caller could grow one zone's
/// authorization list without end.
pub const MAX_VPC_ASSOCIATION_AUTHORIZATIONS: usize = 1_000;

/// The most VPC association authorizations one response carries.
///
/// `ListVPCAssociationAuthorizations$MaxResults`'s own documentation: "If you
/// don't specify a value for MaxResults, Route 53 returns up to 50 VPCs per
/// page."
pub const MAX_VPC_AUTHORIZATIONS_PER_PAGE: i32 = 50;

/// The most reusable delegation sets the account may hold.
///
/// Route 53's documented account quota ("Reusable delegation sets: 100 per
/// account"), which the model's `LimitsExceeded` refusal reports.
pub const MAX_REUSABLE_DELEGATION_SETS: usize = 100;

/// The most traffic policies the account may hold.
///
/// Route 53's documented account quota ("Traffic policies: 50 per account"),
/// and the model declares `TooManyTrafficPolicies` for the refusal that
/// enforces it.
pub const MAX_TRAFFIC_POLICIES: usize = 50;

/// The most traffic policy instances the account may hold.
///
/// Route 53's documented account quota ("Traffic policy records: 50 per
/// account"), and the model declares `TooManyTrafficPolicyInstances` for the
/// refusal that enforces it.
pub const MAX_TRAFFIC_POLICY_INSTANCES: usize = 50;

/// The `AccountLimitType` naming the hosted-zone quota (`AccountLimitType` enum).
pub const ACCOUNT_LIMIT_MAX_HOSTED_ZONES: &str = "MAX_HOSTED_ZONES_BY_OWNER";
/// The `AccountLimitType` naming the health-check quota (`AccountLimitType` enum).
pub const ACCOUNT_LIMIT_MAX_HEALTH_CHECKS: &str = "MAX_HEALTH_CHECKS_BY_OWNER";
/// The `AccountLimitType` naming the reusable-delegation-set quota
/// (`AccountLimitType` enum).
pub const ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS: &str =
    "MAX_REUSABLE_DELEGATION_SETS_BY_OWNER";
/// The `AccountLimitType` naming the traffic-policy quota (`AccountLimitType` enum).
pub const ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES: &str = "MAX_TRAFFIC_POLICIES_BY_OWNER";
/// The `AccountLimitType` naming the traffic-policy-instance quota
/// (`AccountLimitType` enum).
pub const ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES: &str =
    "MAX_TRAFFIC_POLICY_INSTANCES_BY_OWNER";

/// The `TagResourceType` naming a hosted zone (`TagResourceType` enum).
pub const TAG_RESOURCE_TYPE_HOSTED_ZONE: &str = "hostedzone";
/// The `TagResourceType` naming a health check (`TagResourceType` enum).
pub const TAG_RESOURCE_TYPE_HEALTH_CHECK: &str = "healthcheck";

/// The refusal text `CreateHostedZone` answers a `CallerReference` that already
/// names a zone with (`HostedZoneAlreadyExists`, HTTP 409).
///
/// A CONSTANT, naming neither the caller reference nor the zone the reference
/// already names: the shape's own documentation states the rule without either
/// ("Amazon Route 53 returns this error when a hosted zone has already been
/// created with the specified CallerReference"), and the zone's id is
/// server-generated, so a refusal quoting it would differ on every run and tell
/// a caller nothing it can act on. The zone the reference names is in state,
/// where `ListHostedZones` reads it.
pub const HOSTED_ZONE_ALREADY_EXISTS_MESSAGE: &str =
    "A hosted zone has already been created with the specified caller reference.";

/// The refusal text `DeleteHostedZone` answers a zone that still holds records
/// of the caller's with (`HostedZoneNotEmpty`, HTTP 400).
///
/// A CONSTANT naming no zone, and it ends with what the caller has to do next —
/// the operation's own documentation states both halves ("You can delete a
/// hosted zone only if it contains only the default SOA and NS records" / "If
/// the hosted zone contains other resource record sets, you must delete them
/// before you can delete the hosted zone").
pub const HOSTED_ZONE_NOT_EMPTY_MESSAGE: &str =
    "The specified hosted zone contains resource record sets other than the default SOA and NS \
     records and so cannot be deleted. Delete all non-default records and try again.";

/// The version number a traffic policy's first version carries.
///
/// `TrafficPolicy$Version`'s own documentation: "For a new traffic policy, the
/// value of Version is always 1."
pub const FIRST_TRAFFIC_POLICY_VERSION: i32 = 1;

/// The `AWSPolicyFormatVersion` a traffic policy document declares.
///
/// The "Traffic Policy Document Format" reference this operation's own
/// documentation points at publishes exactly one format version, so a document
/// declaring any other names a format this API version does not define.
pub const TRAFFIC_POLICY_FORMAT_VERSION: &str = "2015-10-01";

/// The IPv4 CIDR blocks Route 53's health checkers send their health checks
/// from, as the service publishes them.
///
/// `GetCheckerIpRanges`'s own model documentation points at the published
/// source: "we recommend that you download ip-ranges.json, which includes IP
/// address ranges for all Amazon Web Services services" — these are the blocks
/// that file publishes under the Route 53 health-checks service, one `/26` per
/// health-checking region (several regions publish more than one). They are
/// spelled out here, in no particular order, because the ORDER the wire carries
/// is computed by [`checker_ip_ranges`] rather than being a property of this
/// table; duplicates in it collapse there too.
///
/// A caller reads this set to allow health-check traffic through a firewall, so
/// what matters about it is that every entry is a well-formed IPv4 CIDR block
/// and that two reads of it agree — never that a particular block is in it.
pub const CHECKER_IP_RANGES: &[&str] = &[
    "54.183.255.128/26",
    "54.228.16.0/26",
    "54.232.40.64/26",
    "54.241.32.64/26",
    "54.243.31.192/26",
    "54.244.52.192/26",
    "54.245.168.0/26",
    "54.248.220.0/26",
    "54.250.253.192/26",
    "54.251.31.128/26",
    "54.252.79.128/26",
    "54.252.254.192/26",
    "54.255.254.192/26",
    "107.23.255.0/26",
    "176.34.159.192/26",
    "177.71.207.128/26",
    "13.57.226.0/26",
    "13.124.199.0/26",
    "13.210.244.128/26",
];

/// The health-checker CIDR blocks in the order the API reports them, with any
/// duplicate collapsed.
///
/// `CheckerIpRanges`'s own documentation: "A complex type that contains sorted
/// list of IP ranges in CIDR format". A CIDR block's order is its numeric
/// position in the address space, not the spelling of its text — `107.23.255.0`
/// comes after `54.255.254.192`, where a string comparison would put it first —
/// so the list is ordered by network address and then by prefix length.
pub fn checker_ip_ranges() -> Vec<String> {
    let mut ranges: Vec<&str> = CHECKER_IP_RANGES.to_vec();
    ranges.sort_unstable_by_key(|cidr| cidr_sort_key(cidr));
    ranges.dedup();
    ranges.into_iter().map(str::to_string).collect()
}

/// `(network address, prefix length)` for an `a.b.c.d/nn` block: the pair a CIDR
/// block's numeric position in the address space is, and so the key
/// [`checker_ip_ranges`] orders by.
///
/// A block that is not a well-formed IPv4 CIDR sorts last rather than
/// panicking, which keeps the order total whatever the table holds.
fn cidr_sort_key(cidr: &str) -> (u32, u8) {
    let (address, prefix) = cidr.split_once('/').unwrap_or((cidr, "32"));
    (
        address.parse::<Ipv4Addr>().map_or(u32::MAX, u32::from),
        prefix.parse::<u8>().unwrap_or(u8::MAX),
    )
}

/// The `/hostedzone/` prefix the API wraps a hosted-zone id in.
pub const HOSTED_ZONE_PREFIX: &str = "/hostedzone/";
/// The `/change/` prefix the API wraps a change id in.
pub const CHANGE_PREFIX: &str = "/change/";

/// A change request, as `GetChange` reports it.
///
/// The id is stored BARE (`C…`); [`StoredChange::to_change_info`] adds the
/// `/change/` prefix the wire carries.
#[derive(Debug, Clone)]
pub struct StoredChange {
    /// The bare change id, which is also this record's key in [`Route53State::changes`].
    pub id: String,
    /// When the change was submitted, in milliseconds since the Unix epoch.
    pub submitted_at_millis: i64,
    /// The instant the change becomes `INSYNC`; see [`CHANGE_PROPAGATION`].
    pub insync_at: Instant,
    /// The `ChangeBatch` comment, when the submitter gave one.
    pub comment: Option<String>,
}

impl StoredChange {
    /// `PENDING` until [`StoredChange::insync_at`], `INSYNC` from then on.
    ///
    /// Derived from the clock rather than stored, so no caller has to poll the
    /// change for it to make progress.
    pub fn status(&self) -> &'static str {
        if Instant::now() < self.insync_at {
            STATUS_PENDING
        } else {
            STATUS_INSYNC
        }
    }

    /// The modelled `ChangeInfo` for this change.
    pub fn to_change_info(&self) -> ChangeInfo {
        ChangeInfo {
            id: format!("{CHANGE_PREFIX}{}", self.id),
            status: self.status().to_string(),
            submitted_at: ::aws_smithy_types::DateTime::from_millis(self.submitted_at_millis),
            comment: self.comment.clone(),
        }
    }
}

/// The identity of a resource record set within a zone.
///
/// Route 53 keys a record set by (name, type, set identifier), and
/// `ListResourceRecordSets` returns them "sorted first by DNS name with the
/// labels reversed … When multiple records have the same DNS name,
/// ListResourceRecordSets sorts results by the record type" (the operation's own
/// documentation). Deriving `Ord` over the fields in that order makes a
/// `BTreeMap` keyed by this type give both the uniqueness rule and the list
/// order for free.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RecordKey {
    /// The record name's octets with its labels reversed and a trailing dot,
    /// lowercased: `www.example.com` sorts as `com.example.www.`. See
    /// [`DnsNameKey`]: every spelling of one name gives one key, so this is
    /// the name half of a record set's identity as well as its sort position.
    pub name_sort_key: DnsNameKey,
    /// The `RRType` of the record set.
    pub rr_type: String,
    /// The `SetIdentifier`, which distinguishes the members of a routing group.
    pub set_identifier: Option<String>,
}

impl RecordKey {
    /// The key for a record set, as the wire presents it.
    pub fn of(name: &str, rr_type: &str, set_identifier: Option<&str>) -> Self {
        Self {
            name_sort_key: dns_sort_key(name),
            rr_type: rr_type.to_string(),
            set_identifier: set_identifier.map(str::to_string),
        }
    }

    /// The key for an already-built record set.
    pub fn of_record(record: &ResourceRecordSet) -> Self {
        Self::of(
            &record.name,
            &record.r#type,
            record.set_identifier.as_deref(),
        )
    }
}

/// A hosted zone and the record sets it holds.
#[derive(Debug, Clone)]
pub struct StoredZone {
    /// The bare zone id (`Z…`), which is also this zone's key in [`Route53State::zones`].
    pub id: String,
    /// The domain name in the canonical form [`canonical_dns_name`] gives.
    pub name: String,
    /// The `CallerReference` the zone was created with.
    pub caller_reference: String,
    /// The `Comment` from `HostedZoneConfig`, when one was given.
    pub comment: Option<String>,
    /// Whether this is a private hosted zone.
    pub private_zone: bool,
    /// The four authoritative name servers Route 53 assigned to this zone.
    pub name_servers: Vec<String>,
    /// The VPCs associated with a private hosted zone.
    pub vpcs: Vec<VPC>,
    /// The VPCs another account has been authorized to associate with this
    /// zone, in the order they were authorized — the list
    /// `ListVPCAssociationAuthorizations` pages over and
    /// `DeleteVPCAssociationAuthorization` removes from.
    ///
    /// An authorization is held ON the zone, like the association list beside
    /// it, so deleting the zone takes its authorizations with it and the two
    /// lists cannot disagree about which zone they belong to. It is SEPARATE
    /// from `vpcs`: an authorization only permits a later
    /// `AssociateVPCWithHostedZone`, and associating the VPC does not consume
    /// it (Route 53 keeps it listed until it is deleted). Bounded by
    /// [`MAX_VPC_ASSOCIATION_AUTHORIZATIONS`].
    pub vpc_authorizations: Vec<VPC>,
    /// This zone's record sets, keyed (and therefore ordered) by [`RecordKey`].
    pub records: BTreeMap<RecordKey, ResourceRecordSet>,
    /// The zone's tags, keyed by tag key — the set `ChangeTagsForResource`
    /// edits and `ListTagsForResource` reports. Held ON the zone, so deleting
    /// the zone takes its tags with it and no second map can disagree about
    /// them. Bounded by [`MAX_TAGS_PER_RESOURCE`].
    pub tags: BTreeMap<String, String>,
    /// Whether deletion protection is enabled for this zone, as a hosted-zone
    /// read reports it.
    ///
    /// A zone is created WITHOUT deletion protection, and this API version
    /// (2013-04-01) carries no operation that turns it on, so every zone this
    /// store holds reports it off — exactly as Route 53 reports a zone created
    /// without it. It is held on the zone rather than answered as a constant by
    /// the read, so the day an operation can set it, the read already reports
    /// what was set.
    pub deletion_protection_enabled: bool,
}

impl StoredZone {
    /// The modelled `HostedZone`, with a LIVE `ResourceRecordSetCount`.
    ///
    /// `Config` is ALWAYS present: `HostedZoneConfig$PrivateZone` is a boolean
    /// with `@default false`, so the zone's private flag always has a value to
    /// report, and Route 53 answers every hosted-zone read with the `Config`
    /// container carrying it — including for a public zone created with no
    /// `HostedZoneConfig` at all. `Comment`, which has no default, is reported
    /// only when the zone holds one; that is the member the `HostedZone$Config`
    /// documentation ("the Config and Comment elements don't appear in the
    /// response") is about.
    pub fn to_hosted_zone(&self) -> HostedZone {
        let config = Some(HostedZoneConfig {
            comment: self.comment.clone(),
            private_zone: self.private_zone,
        });
        HostedZone {
            id: format!("{HOSTED_ZONE_PREFIX}{}", self.id),
            name: self.name.clone(),
            caller_reference: self.caller_reference.clone(),
            config,
            resource_record_set_count: Some(self.records.len() as i64),
            linked_service: None,
            features: None,
        }
    }

    /// The zone's name servers as a `DelegationSet`.
    ///
    /// `Id` and `CallerReference` are absent: they belong to a REUSABLE
    /// delegation set (`CreateReusableDelegationSet`), and this zone has none.
    pub fn to_delegation_set(&self) -> DelegationSet {
        DelegationSet {
            id: None,
            caller_reference: None,
            name_servers: self.name_servers.clone(),
        }
    }

    /// This zone's tag set as `ResourceTagSet$Tags` carries it; see
    /// [`tags_to_list`].
    pub fn to_tag_list(&self) -> Vec<Tag> {
        tags_to_list(&self.tags)
    }

    /// Whether the zone still holds a record other than the default SOA and NS
    /// records at its apex.
    ///
    /// `DeleteHostedZone`'s documentation: "You can delete a hosted zone only if
    /// it contains only the default SOA and NS records."
    pub fn holds_user_records(&self) -> bool {
        let apex = dns_sort_key(&self.name);
        self.records.keys().any(|key| {
            key.name_sort_key != apex
                || key.set_identifier.is_some()
                || !matches!(key.rr_type.as_str(), "SOA" | "NS")
        })
    }

    /// Where this zone's association list holds the VPC named by `vpc_id`, if it
    /// holds it at all.
    ///
    /// An Amazon VPC id identifies the VPC on its own, so it alone is the
    /// identity of an association; the region is carried ALONGSIDE it (it is
    /// what a reader needs to find the VPC again), never part of that identity.
    /// A zone therefore holds at most one association per VPC id, which is what
    /// makes associating a VPC a second time a no-op rather than a duplicate.
    pub fn vpc_position(&self, vpc_id: &str) -> Option<usize> {
        vpc_position_in(&self.vpcs, vpc_id)
    }

    /// Where this zone's authorization list holds the VPC named by `vpc_id`, if
    /// it holds it at all.
    ///
    /// Keyed by VPC id for the same reason [`StoredZone::vpc_position`] is: the
    /// id identifies the VPC on its own, so a zone holds at most one
    /// authorization per VPC and authorizing the same VPC again is a no-op
    /// rather than a duplicate.
    pub fn vpc_authorization_position(&self, vpc_id: &str) -> Option<usize> {
        vpc_position_in(&self.vpc_authorizations, vpc_id)
    }
}

/// Where `vpcs` holds the VPC named by `vpc_id`, if it holds it at all.
///
/// The one lookup both of a zone's VPC lists use, so the association list and
/// the authorization list cannot drift apart on what identifies a VPC.
pub fn vpc_position_in(vpcs: &[VPC], vpc_id: &str) -> Option<usize> {
    vpcs.iter()
        .position(|vpc| vpc.vpc_id.as_deref() == Some(vpc_id))
}

/// A health check of the account's, as `GetHealthCheck` reports it.
///
/// The configuration is held as the modelled `HealthCheckConfig`, with the
/// defaults the service fills in already applied (see
/// [`health_check_config_with_defaults`]), so the create that persisted it and
/// every later read report the same configuration.
#[derive(Debug, Clone)]
pub struct StoredHealthCheck {
    /// The health check id, which is also this record's key in
    /// [`ComAmazonawsRoute53Awsdnsv20130401State::health_checks`].
    pub id: String,
    /// The `CallerReference` the health check was created with.
    pub caller_reference: String,
    /// The health check's configuration, as the service holds it.
    pub config: HealthCheckConfig,
    /// The `HealthCheckVersion`, starting at [`FIRST_HEALTH_CHECK_VERSION`].
    pub version: i64,
    /// The tags on this health check, keyed by tag key so one key carries one
    /// value (`ChangeTagsForResource`).
    pub tags: BTreeMap<String, String>,
}

impl StoredHealthCheck {
    /// The modelled `HealthCheck` for this record.
    ///
    /// The ONE projection every health-check response is built from, so a create
    /// and a later read cannot report one health check differently.
    pub fn to_health_check(&self) -> HealthCheck {
        HealthCheck {
            id: self.id.clone(),
            caller_reference: self.caller_reference.clone(),
            // `LinkedService` is "If the health check was created by another
            // service, the service that created the health check": a health
            // check this API created for the account was created by no other
            // service, so the member is absent.
            linked_service: None,
            health_check_config: self.config.clone(),
            health_check_version: self.version,
            // The alarm's own configuration belongs to CloudWatch, which is
            // where Route 53 reads it from; this store holds the alarm's
            // identity (`HealthCheckConfig$AlarmIdentifier`), not its settings.
            cloud_watch_alarm_configuration: None,
        }
    }

    /// This health check's tags as the modelled `Tag` list.
    pub fn to_tag_list(&self) -> Vec<Tag> {
        tags_to_list(&self.tags)
    }
}

/// `config` with the values the service fills in for a member the caller left
/// out, so what the store holds is what the service would report.
///
/// Only the endpoint-based types take these two members —
/// `HealthCheckConfig$RequestInterval` and `$FailureThreshold` both state that
/// they are "not supported when you specify a value for Type of
/// RECOVERY_CONTROL", and the other two types read a CloudWatch alarm or combine
/// child health checks rather than probing an endpoint — so a check of another
/// type keeps exactly the configuration it was created with.
pub fn health_check_config_with_defaults(config: &HealthCheckConfig) -> HealthCheckConfig {
    let mut filled = config.clone();
    if ENDPOINT_HEALTH_CHECK_TYPES.contains(&filled.r#type.as_str()) {
        filled.request_interval = Some(filled.request_interval.unwrap_or(DEFAULT_REQUEST_INTERVAL));
        filled.failure_threshold =
            Some(filled.failure_threshold.unwrap_or(DEFAULT_FAILURE_THRESHOLD));
    }
    filled
}

/// One version of a traffic policy: the document that version holds, and the
/// values read off it.
///
/// A version's document is immutable once created — only its comment is
/// editable (`UpdateTrafficPolicyComment`) — so the `RRType` the document names
/// is read off it once, when the version is created, rather than on every read.
#[derive(Debug, Clone)]
pub struct StoredTrafficPolicyVersion {
    /// The `RRType` the document's `RecordType` names, as
    /// [`traffic_policy_record_type`] read it off the document.
    pub record_type: String,
    /// The traffic policy document, exactly as the caller sent it.
    pub document: String,
    /// This version's comment, when it was created with one.
    pub comment: Option<String>,
}

/// A traffic policy and every version of it.
///
/// A traffic policy is addressed by `(Id, Version)` — `GetTrafficPolicy` takes
/// both — so the versions live ON the policy, keyed by version number: one
/// policy's id and name cannot then disagree between two of its versions, and
/// the highest key is its latest version.
#[derive(Debug, Clone)]
pub struct StoredTrafficPolicy {
    /// The policy id, which is also this record's key in
    /// [`ComAmazonawsRoute53Awsdnsv20130401State::traffic_policies`].
    pub id: String,
    /// The `Name` the policy was created with, shared by every version of it.
    pub name: String,
    /// This policy's versions, keyed (and therefore ordered) by version number.
    pub versions: BTreeMap<i32, StoredTrafficPolicyVersion>,
}

impl StoredTrafficPolicy {
    /// The modelled `TrafficPolicy` for one version of this policy, or `None`
    /// when the policy holds no such version.
    ///
    /// The ONE projection every traffic-policy response is built from, so a
    /// create and a later read cannot report one policy version differently.
    pub fn to_traffic_policy(&self, version: i32) -> Option<TrafficPolicy> {
        let stored = self.versions.get(&version)?;
        Some(TrafficPolicy {
            id: self.id.clone(),
            version,
            name: self.name.clone(),
            r#type: stored.record_type.clone(),
            document: stored.document.clone(),
            comment: stored.comment.clone(),
        })
    }
}

/// Per-service state for com-amazonaws-route53-awsdnsv20130401. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsRoute53Awsdnsv20130401State {
    /// Every hosted zone, keyed by its bare id. A `BTreeMap` so
    /// `ListHostedZones` pages over a stable total order of zone ids. Bounded by
    /// [`MAX_HOSTED_ZONES`].
    pub zones: BTreeMap<String, StoredZone>,
    /// `CallerReference` -> bare zone id, the index `CreateHostedZone` answers
    /// `HostedZoneAlreadyExists` from.
    pub zone_by_caller_reference: HashMap<String, String>,
    /// The retained changes, keyed by bare id, as `GetChange` reads them. Bounded
    /// by [`MAX_RETAINED_CHANGES`] and [`CHANGE_RETENTION`].
    pub changes: HashMap<String, StoredChange>,
    /// The ids in [`ComAmazonawsRoute53Awsdnsv20130401State::changes`] in
    /// submission order, so the oldest can be evicted first.
    change_order: VecDeque<String>,
    /// Every traffic policy, keyed by its id. A `BTreeMap` so a listing pages
    /// over a stable total order of policy ids. Bounded by
    /// [`MAX_TRAFFIC_POLICIES`].
    pub traffic_policies: BTreeMap<String, StoredTrafficPolicy>,
    /// Every health check of the account's, keyed by its id. A `BTreeMap` so a
    /// listing pages over a stable total order of health-check ids. Bounded by
    /// [`MAX_HEALTH_CHECKS`].
    pub health_checks: BTreeMap<String, StoredHealthCheck>,
    /// `CallerReference` -> health-check id, the index `CreateHealthCheck`
    /// recognises a repeated request by.
    pub health_check_by_caller_reference: HashMap<String, String>,
    /// The id generator, kept here so ids stay unique across calls.
    ids: IdMinter,
    /// The health-checker CIDR blocks `GetCheckerIpRanges` reports, in the order
    /// it reports them. SERVICE-owned: seeded from [`CHECKER_IP_RANGES`] when
    /// this store is created and only ever read afterwards, so every call
    /// answers the same list.
    checker_ip_ranges: Vec<String>,
}

/// The store a fresh engine starts with: no zones and no changes, and the
/// service-owned health-checker ranges already in place (see the module docs).
impl Default for ComAmazonawsRoute53Awsdnsv20130401State {
    fn default() -> Self {
        Self {
            zones: BTreeMap::new(),
            zone_by_caller_reference: HashMap::new(),
            changes: HashMap::new(),
            change_order: VecDeque::new(),
            traffic_policies: BTreeMap::new(),
            health_checks: BTreeMap::new(),
            health_check_by_caller_reference: HashMap::new(),
            ids: IdMinter::default(),
            checker_ip_ranges: checker_ip_ranges(),
        }
    }
}

impl ComAmazonawsRoute53Awsdnsv20130401State {
    /// Mint a hosted-zone id: `Z` plus 14 uppercase alphanumerics, the shape
    /// Route 53 returns (`Z1D633PJN98FT9`). Retried until it is unused.
    pub fn mint_zone_id(&mut self) -> String {
        loop {
            let id = self.ids.mint('Z', ID_BODY_LEN);
            if !self.zones.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a traffic policy id: 8-4-4-4-12 lowercase hex, the form Route 53
    /// returns for one (`TrafficPolicyId` is `len 1..36`, which is that form's
    /// length). Retried until it is unused.
    pub fn mint_traffic_policy_id(&mut self) -> String {
        loop {
            let id = self.ids.mint_uuid();
            if !self.traffic_policies.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a health-check id: 8-4-4-4-12 lowercase hex, the form Route 53
    /// returns for one. Retried until it is unused.
    pub fn mint_health_check_id(&mut self) -> String {
        loop {
            let id = self.ids.mint_uuid();
            if !self.health_checks.contains_key(&id) {
                return id;
            }
        }
    }

    /// The traffic policy carrying `name`, if the account holds one.
    ///
    /// `TrafficPolicyAlreadyExists`'s own documentation — "A traffic policy that
    /// has the same value for Name already exists" — is about the POLICY, not
    /// one of its versions, so the lookup is over the policies, and it is
    /// searched off the store rather than tracked beside it: a policy that is
    /// deleted frees its name by itself.
    pub fn traffic_policy_with_name(&self, name: &str) -> Option<&StoredTrafficPolicy> {
        self.traffic_policies
            .values()
            .find(|policy| policy.name == name)
    }

    /// Record a newly submitted change and return it, so the caller can render
    /// the `ChangeInfo` it just persisted. The id is `C` plus 14 uppercase
    /// alphanumerics, the shape Route 53 returns (`C2682N5HXP0BZ4`).
    pub fn submit_change(&mut self, comment: Option<String>) -> StoredChange {
        let id = loop {
            let id = self.ids.mint('C', ID_BODY_LEN);
            if !self.changes.contains_key(&id) {
                break id;
            }
        };
        let change = StoredChange {
            id: id.clone(),
            submitted_at_millis: now_millis(),
            insync_at: Instant::now() + CHANGE_PROPAGATION,
            comment,
        };
        self.change_order.push_back(id.clone());
        self.changes.insert(id, change.clone());
        self.retain_recent_changes();
        change
    }

    /// Drop the changes the ledger no longer retains: the oldest past
    /// [`MAX_RETAINED_CHANGES`], and any that reached `INSYNC` more than
    /// [`CHANGE_RETENTION`] ago.
    ///
    /// Called from the mutating path that appends a change, never from a read,
    /// so reading a change never changes what a later caller sees.
    fn retain_recent_changes(&mut self) {
        let now = Instant::now();
        while let Some(oldest) = self.change_order.front().cloned() {
            let over_cap = self.change_order.len() > MAX_RETAINED_CHANGES;
            let expired = self.changes.get(&oldest).is_none_or(|change| {
                now.saturating_duration_since(change.insync_at) > CHANGE_RETENTION
            });
            if !over_cap && !expired {
                break;
            }
            self.change_order.pop_front();
            self.changes.remove(&oldest);
        }
    }

    /// The account quota for one `AccountLimitType` and how much of it this
    /// store currently holds, as `(limit value, current count)`. `None` for a
    /// value outside `AccountLimitType`.
    ///
    /// The count is COUNTED off the store on every call rather than tracked
    /// beside it, so it cannot disagree with the resources the account actually
    /// holds: creating or deleting one moves the count by itself, and no read
    /// has to be told about it. The quota is the same constant the create path
    /// refuses at, so a count that has reached the reported limit is exactly
    /// the point at which the next create is refused.
    pub fn account_limit_usage(&self, limit_type: &str) -> Option<(i64, i64)> {
        let (limit, count) = match limit_type {
            ACCOUNT_LIMIT_MAX_HOSTED_ZONES => (MAX_HOSTED_ZONES, self.zones.len()),
            ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES => {
                (MAX_TRAFFIC_POLICIES, self.traffic_policies.len())
            }
            ACCOUNT_LIMIT_MAX_HEALTH_CHECKS => (MAX_HEALTH_CHECKS, self.health_checks.len()),
            // The entity kinds below have no collection in this store yet —
            // `CreateReusableDelegationSet` and `CreateTrafficPolicyInstance`
            // are both still scaffolds, so nothing can bring one into being and
            // the account holds none of them. Each gets its count from the
            // collection this struct grows when its create is implemented.
            ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS => (MAX_REUSABLE_DELEGATION_SETS, 0),
            ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES => (MAX_TRAFFIC_POLICY_INSTANCES, 0),
            _ => return None,
        };
        Some((count_as_i64(limit), count_as_i64(count)))
    }

    /// How many hosted zones the account holds right now, public and private
    /// together, as the `long` a count shape carries.
    ///
    /// COUNTED off the store on every call rather than tracked beside it, for
    /// the same reason [`Self::account_limit_usage`] counts: creating or
    /// deleting a zone moves the number by itself, so no reporting operation
    /// has to be told about it and none can disagree with the zones a listing
    /// returns. A borrow, not a mutation — reporting the count leaves the store
    /// exactly as it was.
    pub fn hosted_zone_count(&self) -> i64 {
        count_as_i64(self.zones.len())
    }

    /// The zone OTHER than `excluding_zone_id` that carries the same DNS name
    /// as `zone_name` and is already associated with `vpc_id`, if there is one.
    ///
    /// `ConflictingDomainExists`'s own documentation, for the case "Associate
    /// VPCs with a private hosted zone": "The VPC that you specified is already
    /// associated with another hosted zone that has the same name." One VPC
    /// resolving one name through two zones has no answer, so the association
    /// that would create the ambiguity is the one refused.
    pub fn zone_with_name_associated_to_vpc(
        &self,
        zone_name: &str,
        excluding_zone_id: &str,
        vpc_id: &str,
    ) -> Option<&StoredZone> {
        let name_key = dns_sort_key(zone_name);
        self.zones.values().find(|zone| {
            zone.id != excluding_zone_id
                && dns_sort_key(&zone.name) == name_key
                && zone.vpc_position(vpc_id).is_some()
        })
    }

    /// The health-checker CIDR blocks this store holds, in the order
    /// `GetCheckerIpRanges` reports them.
    ///
    /// A borrow, not a mutation: the set is service-owned, so reporting it
    /// leaves the store exactly as it was.
    pub fn checker_ip_ranges(&self) -> &[String] {
        &self.checker_ip_ranges
    }

    /// Mint the four authoritative name servers for a new hosted zone.
    ///
    /// The Route 53 Developer Guide ("NS and SOA Records that Route 53 Creates
    /// for a Hosted Zone") describes the delegation set as four name servers,
    /// one in each of the `com`, `net`, `org` and `co.uk` top-level domains, so
    /// a resolver failure in one TLD cannot take all four out.
    pub fn mint_name_servers(&mut self) -> Vec<String> {
        const TLDS: [&str; 4] = ["com", "net", "org", "co.uk"];
        TLDS.iter()
            .map(|tld| {
                let host = self.ids.next_u64() % 2048 + 1;
                let set = self.ids.next_u64() % 64;
                format!("ns-{host}.awsdns-{set:02}.{tld}")
            })
            .collect()
    }
}

/// The number of characters after the single-letter prefix in a Route 53
/// resource id (`Z1D633PJN98FT9`, `C2682N5HXP0BZ4`).
const ID_BODY_LEN: usize = 14;

/// A count or a quota as the `long` the count and limit shapes carry
/// (`HealthCheckCount` and `HostedZoneCount` are `long`, `UsageCount` is
/// `range 0..`, `LimitValue` is `range 1..`), saturating rather than wrapping on
/// a count no store this size can reach.
pub fn count_as_i64(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Milliseconds since the Unix epoch, for a `SubmittedAt` timestamp.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The octets of a DNS name, one `Vec<u8>` per label, with every presentation
/// escape resolved and the root label dropped.
///
/// A name reaches the API in PRESENTATION form, where an octet may be spelled
/// either literally, as `\DDD` (three octal digits — `\052` is `*`, octet 42),
/// or as a backslash in front of the octet itself (`\.` is a dot INSIDE a label
/// rather than a label separator). Resolving all three spellings down to octets
/// is what makes two spellings of one name the same name, so it is the basis of
/// both [`canonical_dns_name`] and [`dns_sort_key`].
///
/// An empty name has no labels, so it keys and renders as the empty name rather
/// than as the root.
fn dns_labels(name: &str) -> Vec<Vec<u8>> {
    let bytes = name.trim().as_bytes();
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut labels: Vec<Vec<u8>> = Vec::new();
    let mut label: Vec<u8> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'.' => {
                labels.push(std::mem::take(&mut label));
                index += 1;
            }
            b'\\' => {
                if let Some(octet) = octal_escape_at(bytes, index) {
                    label.push(octet);
                    index += 4;
                } else if let Some(&escaped) =
                    bytes.get(index + 1).filter(|byte| !byte.is_ascii_digit())
                {
                    label.push(escaped);
                    index += 2;
                } else {
                    // Either nothing follows, or what follows is a digit run
                    // that is no escape code of this form (`\777` is above one
                    // octet, `\8` is not octal): the backslash is then an octet
                    // of its own, and the digits are ordinary octets after it.
                    label.push(b'\\');
                    index += 1;
                }
            }
            octet => {
                label.push(octet);
                index += 1;
            }
        }
    }
    labels.push(label);
    // A trailing dot spells the root label, which names nothing of its own.
    if labels.len() > 1 && labels.last().is_some_and(Vec::is_empty) {
        labels.pop();
    }
    labels
}

/// The octet a `\DDD` escape at `index` stands for, if one starts there.
///
/// Route 53 writes these codes in OCTAL ("the escape code \\three-digit octal
/// code" of the DNS domain-name format), so `\052` is the asterisk, octet 42.
/// A code that is not three octal digits, or that is above one octet, is not an
/// escape of this kind.
fn octal_escape_at(bytes: &[u8], index: usize) -> Option<u8> {
    let digits = bytes.get(index + 1..index + 4)?;
    if !digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0u32, |value, digit| value * 8 + u32::from(digit - b'0'));
    u8::try_from(value).ok()
}

/// Whether this octet is one a DNS name carries literally in Route 53's
/// presentation of it.
///
/// The characters a name is spelled with are `a-z`, `0-9`, `-` (hyphen) and `_`
/// (underscore); every other octet is presented as an octal escape code (the
/// DNS domain-name format the `Name` member's documentation points at).
fn is_plain_dns_octet(octet: u8) -> bool {
    octet.is_ascii_lowercase() || octet.is_ascii_digit() || octet == b'-' || octet == b'_'
}

/// A DNS name in the ONE canonical form this service stores and returns it in:
/// lower-cased, every octet outside the plain set escaped as `\DDD`, and
/// terminated by the root dot.
///
/// Two parts of the same documented rule. The trailing dot: "The trailing dot
/// is optional; Amazon Route 53 assumes that the domain name is fully
/// qualified. This means that Route 53 treats www.example.com (without a
/// trailing dot) and www.example.com. (with a trailing dot) as identical."
/// The spelling: a name may only be written with `a-z`, `0-9`, `-` and `_`, so
/// any other octet — the `*` of a wildcard label above all — comes back as its
/// octal escape code, and a name's letters come back folded to lower case
/// however they were sent.
///
/// Because the form is canonical, EVERY spelling of a name maps onto it: the
/// escaped and the literal spelling of an octet, and either case of a letter,
/// all name one resource.
pub fn canonical_dns_name(name: &str) -> String {
    let mut canonical = String::new();
    for label in dns_labels(name) {
        for octet in label {
            let octet = octet.to_ascii_lowercase();
            if is_plain_dns_octet(octet) {
                canonical.push(char::from(octet));
            } else {
                let _ = write!(canonical, "\\{octet:03o}");
            }
        }
        canonical.push('.');
    }
    canonical
}

/// A name's labels, lower-cased, as [`dns_labels`] resolves them.
///
/// DNS names are case-insensitive, so folding here is what makes `WWW.example.com`
/// and `www.example.com` one name.
fn dns_lower_labels(name: &str) -> Vec<Vec<u8>> {
    let mut labels = dns_labels(name);
    for label in &mut labels {
        label.make_ascii_lowercase();
    }
    labels
}

/// The order key `ListResourceRecordSets` sorts by, which is also a record
/// set's name IDENTITY.
///
/// The sort position, from the operation's documentation: "ListResourceRecordSets
/// sorts results first by DNS name with the labels reversed, for example:
/// `com.example.www.` Note the trailing dot, which can change the sort order
/// when the record name contains characters that appear before . (decimal 46) in
/// the ASCII table. These characters include the following: ! " # $ % & ' ( ) *
/// + , -". So `dotted` is the reversed labels' OCTETS, not their escaped
/// spelling, joined by dots: those characters then sort where the documentation
/// says they do. It is a byte string because an octet of a DNS name need not be
/// text at all.
///
/// A name has ONE canonical spelling, so `WWW.example.com`, `www.example.com.`
/// and `\167ww.example.com` give the same key and name the same record set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DnsNameKey {
    /// The reversed labels' octets joined by `.` — the documented sort position.
    dotted: Vec<u8>,
    /// How long each of those reversed labels is, which the dotted form alone
    /// does not pin down: a label may CONTAIN a dot octet (`a\.b` is one label),
    /// so `a\.b.example.com` and `b.a.example.com` share a sort position while
    /// being different names. Compared only after `dotted`, so it breaks exactly
    /// those ties and leaves the documented order alone.
    label_lengths: Vec<usize>,
}

/// The [`DnsNameKey`] of a name.
pub fn dns_sort_key(name: &str) -> DnsNameKey {
    let labels = dns_lower_labels(name);
    let mut dotted = Vec::with_capacity(name.len() + 1);
    let mut label_lengths = Vec::with_capacity(labels.len());
    for label in labels.iter().rev() {
        dotted.extend_from_slice(label);
        dotted.push(b'.');
        label_lengths.push(label.len());
    }
    DnsNameKey {
        dotted,
        label_lengths,
    }
}

/// The most octets one DNS label may carry.
///
/// The DNS domain name format every `DNSName` member's documentation points at:
/// "The maximum length of each label is 63 characters." The octets are counted
/// AFTER the presentation escapes are resolved, because an escape code spells
/// one octet however many characters it is written with.
pub const MAX_DNS_LABEL_OCTETS: usize = 63;

/// The most octets a whole DNS name may carry, counting the dot after each
/// label.
///
/// The same documented format: "The total DNS name can't exceed 255
/// characters", dots included.
pub const MAX_DNS_NAME_OCTETS: usize = 255;

/// Why `name` is no DNS name Route 53 can hold, or `None` when it is one.
///
/// The modelled `@length` on a `DNSName` member bounds the REQUEST field (up to
/// 1024 characters), which is far looser than what DNS itself allows, so a name
/// that satisfies every modelled constraint can still be unusable as a domain
/// name — and that is the refusal `InvalidDomainName` names. The two bounds are
/// the documented label and whole-name lengths; both are measured in resolved
/// OCTETS, so a name spelled with escape codes is judged by the name it spells.
pub fn dns_name_problem(name: &str) -> Option<String> {
    let labels = dns_labels(name);
    if let Some(label) = labels.iter().find(|label| label.len() > MAX_DNS_LABEL_OCTETS) {
        return Some(format!(
            "a label may carry at most {MAX_DNS_LABEL_OCTETS} octets, and this name has one of {}",
            label.len()
        ));
    }
    // Every label is followed by the dot that ends it, the root dot included,
    // which is the canonical form the name is stored and compared in.
    let octets: usize = labels.iter().map(|label| label.len() + 1).sum();
    if octets > MAX_DNS_NAME_OCTETS {
        return Some(format!(
            "a domain name may carry at most {MAX_DNS_NAME_OCTETS} octets including the dots, \
             and this one carries {octets}"
        ));
    }
    None
}

/// Whether `name` is inside `zone_name` — the same name, or a subdomain of it.
///
/// `ChangeResourceRecordSets` only accepts records for the zone it is called
/// on, so this is what separates a record the zone owns from one it does not.
/// Compared LABEL by label, so a name is in the zone only when the zone's whole
/// labels are its suffix: `evil-example.com` is not inside `example.com`.
pub fn name_is_in_zone(name: &str, zone_name: &str) -> bool {
    let name_labels = dns_lower_labels(name);
    let zone_labels = dns_lower_labels(zone_name);
    let Some(offset) = name_labels.len().checked_sub(zone_labels.len()) else {
        return false;
    };
    name_labels[offset..] == zone_labels[..]
}

/// A resource's tag set as the wire carries it, in tag-key order.
///
/// The list is the resource's WHOLE tag set, and it is empty when the resource
/// carries no tags: a tag read reports the `Tags` container unconditionally, so
/// a client walking `ResourceTagSet/Tags` to enumerate tags finds an empty
/// collection rather than a missing node once every tag has been removed. (The
/// `@length min 1` on `TagList` bounds what a REQUEST may send — the modelled
/// constraints are enforced on input only — not what a tag set may hold.)
pub fn tags_to_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// Apply one `ChangeTagsForResource` batch to a resource's tag set.
///
/// A tag set is a MAP from key to value, which is what makes the operation
/// "adds, edits, or deletes tags" (its own documentation): adding a key the
/// resource already carries edits that key's value, and removing a key it does
/// not carry leaves the set as it was. `RemoveTagKeys` is applied FIRST, so a
/// key named in both lists ends up with the value `AddTags` gives it rather
/// than being dropped.
///
/// The batch is applied to a COPY and committed only once it is wholly valid,
/// so a refused batch leaves the resource's tags exactly as they were. `Err`
/// carries the refusal text for the caller's `InvalidInput`.
pub fn apply_tag_changes(
    tags: &mut BTreeMap<String, String>,
    add_tags: Option<&[Tag]>,
    remove_tag_keys: Option<&[String]>,
) -> Result<(), String> {
    if add_tags.is_none() && remove_tag_keys.is_none() {
        return Err(
            "the request must specify AddTags, RemoveTagKeys, or both: it asks for no change"
                .to_string(),
        );
    }

    let mut updated = tags.clone();
    for key in remove_tag_keys.unwrap_or_default() {
        updated.remove(key);
    }
    for tag in add_tags.unwrap_or_default() {
        // `Tag$Key` is not required by the model, but a tag with no key names
        // nothing to add, edit or delete.
        let key = tag
            .key
            .clone()
            .ok_or_else(|| "every Tag in AddTags must carry a Key".to_string())?;
        updated.insert(key, tag.value.clone().unwrap_or_default());
    }

    if updated.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a resource can carry at most {MAX_TAGS_PER_RESOURCE} tags; this request would \
             leave it with {}",
            updated.len()
        ));
    }

    *tags = updated;
    Ok(())
}

/// The modelled `NoSuchTrafficPolicy` (HTTP 404) for an `(Id, Version)` pair
/// that names no traffic policy version in this store.
///
/// The ONE place this service words that refusal, so every operation addressed
/// at a traffic policy answers a missing one identically. The message is
/// REQUEST-DERIVED — it names the id and version the request asked about, which
/// is what tells a caller whether the policy or the version was the missing
/// half.
pub fn no_such_traffic_policy(requested_id: &str, requested_version: i32) -> NoSuchTrafficPolicy {
    NoSuchTrafficPolicy {
        message: Some(format!(
            "No traffic policy exists with ID {requested_id} and version {requested_version}"
        )),
    }
}

/// The `RRType` a traffic policy document records, or the reason the document
/// is not a valid one (`InvalidTrafficPolicyDocument`, HTTP 400).
///
/// The document is the traffic policy's definition, and `TrafficPolicy$Type` is
/// "the DNS type of the resource record sets that Amazon Route 53 creates when
/// you use a traffic policy to create a traffic policy instance" — so the type
/// is READ OFF the document rather than taken from the request, which carries
/// no type at all. The structure checked here is the one the "Traffic Policy
/// Document Format" reference defines: a JSON object declaring its
/// `AWSPolicyFormatVersion`, the `RecordType` it creates, the `Endpoints` it can
/// route to, and exactly one entry point — `StartEndpoint`, naming one of those
/// endpoints, or `StartRule`, naming one of the `Rules`.
///
/// Every refusal names the member that was wrong and what was wrong with it, so
/// the caller can act on it; the document itself is never quoted back.
pub fn traffic_policy_record_type(document: &str) -> Result<String, String> {
    let parsed: serde_json::Value = serde_json::from_str(document)
        .map_err(|error| format!("the traffic policy document is not valid JSON: {error}"))?;
    let Some(root) = parsed.as_object() else {
        return Err("the traffic policy document must be a JSON object".to_string());
    };

    match root
        .get("AWSPolicyFormatVersion")
        .and_then(serde_json::Value::as_str)
    {
        Some(TRAFFIC_POLICY_FORMAT_VERSION) => {}
        Some(other) => {
            return Err(format!(
                "AWSPolicyFormatVersion {other} is not a traffic policy document format version; \
                 the supported version is {TRAFFIC_POLICY_FORMAT_VERSION}"
            ))
        }
        None => {
            return Err(
                "the traffic policy document must carry AWSPolicyFormatVersion as a string"
                    .to_string(),
            )
        }
    }

    let record_type = match root.get("RecordType").and_then(serde_json::Value::as_str) {
        Some(value) if R_R_TYPE_VALUES.contains(&value) => value.to_string(),
        Some(other) => {
            return Err(format!(
                "RecordType {other} is not a DNS record type a traffic policy can create"
            ))
        }
        None => {
            return Err(
                "the traffic policy document must carry RecordType as a string".to_string()
            )
        }
    };

    let endpoints = root
        .get("Endpoints")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "the traffic policy document must carry Endpoints as a JSON object".to_string()
        })?;
    if endpoints.is_empty() {
        return Err("the traffic policy document must define at least one endpoint in Endpoints"
            .to_string());
    }
    if let Some(name) = named_entry_that_is_not_an_object(endpoints) {
        return Err(format!("the endpoint {name} in Endpoints must be a JSON object"));
    }

    let start_endpoint = root
        .get("StartEndpoint")
        .and_then(serde_json::Value::as_str);
    let start_rule = root.get("StartRule").and_then(serde_json::Value::as_str);
    match (start_endpoint, start_rule) {
        (Some(_), Some(_)) => Err("the traffic policy document must carry exactly one of \
                                   StartEndpoint and StartRule, not both"
            .to_string()),
        (None, None) => Err("the traffic policy document must carry either StartEndpoint or \
                             StartRule as a string"
            .to_string()),
        (Some(endpoint), None) => {
            if endpoints.contains_key(endpoint) {
                Ok(record_type)
            } else {
                Err(format!(
                    "StartEndpoint {endpoint} names no endpoint in Endpoints"
                ))
            }
        }
        (None, Some(rule)) => {
            let rules = root
                .get("Rules")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    "a traffic policy document with a StartRule must carry Rules as a JSON object"
                        .to_string()
                })?;
            if let Some(name) = named_entry_that_is_not_an_object(rules) {
                return Err(format!("the rule {name} in Rules must be a JSON object"));
            }
            if rules.contains_key(rule) {
                Ok(record_type)
            } else {
                Err(format!("StartRule {rule} names no rule in Rules"))
            }
        }
    }
}

/// The name of the first entry of a traffic policy document's `Endpoints` or
/// `Rules` block that is not itself a JSON object, if there is one: both blocks
/// map a name to a definition, so an entry that is not an object defines
/// nothing.
fn named_entry_that_is_not_an_object(
    entries: &serde_json::Map<String, serde_json::Value>,
) -> Option<&String> {
    entries
        .iter()
        .find(|(_, definition)| !definition.is_object())
        .map(|(name, _)| name)
}

/// The endpoint host a `Location` URL falls back to when the request carried no
/// `Host` header to build one from.
const DEFAULT_ENDPOINT_HOST: &str = "route53.amazonaws.com";

/// The absolute URL of a resource a create answers with, as its `Location`
/// carries it.
///
/// Built from the endpoint the caller actually addressed, so the URL it gets
/// back is one it can follow; `path` is the resource's path BELOW the API
/// version prefix (`hostedzone/<id>`, `trafficpolicy/<id>/<version>`) — the
/// same path the operation that reads the resource back is bound to. One
/// helper, so two creates cannot word the same URL differently.
pub fn resource_location(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    path: &str,
) -> String {
    let host = ctx
        .parts()
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ENDPOINT_HOST);
    format!("https://{host}/2013-04-01/{path}")
}

/// The modelled `NoSuchHostedZone` (HTTP 404) for a zone id that names no zone
/// in this store.
///
/// The ONE place this service words that refusal, so every zone-addressed
/// operation answers a missing zone identically. The message is
/// REQUEST-DERIVED — it names the id the request asked about, as the request
/// wrote it — which is what lets a caller tell which of a batch of ids was the
/// missing one: `No hosted zone found with ID: <the requested id>`.
pub fn no_such_hosted_zone(requested_id: &str) -> NoSuchHostedZone {
    NoSuchHostedZone {
        message: Some(format!("No hosted zone found with ID: {requested_id}")),
    }
}

/// The modelled `NoSuchChange` (HTTP 404) for a change id that names no change
/// in this store.
///
/// The ONE place this service words that refusal, beside
/// [`no_such_hosted_zone`]. Route 53 reports an id that resolves to no change
/// through its generic resource-lookup wording, built FROM the request — it
/// names the id the request asked about, as the request wrote it:
/// `Could not find resource with ID: <the requested id>`.
pub fn no_such_change(requested_id: &str) -> NoSuchChange {
    NoSuchChange {
        message: Some(format!("Could not find resource with ID: {requested_id}")),
    }
}

/// The modelled `NoSuchHealthCheck` (HTTP 404) for a health-check id that names
/// no health check in this store.
///
/// The ONE place this service words that refusal, beside
/// [`no_such_hosted_zone`] and [`no_such_change`], so every health-check-addressed
/// operation answers a missing health check identically. The message is
/// REQUEST-DERIVED: it names the id the request asked about, as the request wrote
/// it, which is what lets a caller tell which of a batch of ids was the missing
/// one. The shape's own documentation states the rule it reports: "No health
/// check exists with the specified ID."
pub fn no_such_health_check(requested_id: &str) -> NoSuchHealthCheck {
    NoSuchHealthCheck {
        message: Some(format!(
            "no health check exists with the ID {requested_id}"
        )),
    }
}

/// Mint the request id ONE response names the request it answers with — the
/// `RequestId` element of this service's `<ErrorResponse>` envelope (see
/// [`crate::service::envelope`]).
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is
/// per-response and is not a requirement: nothing can predict it and no caller
/// may depend on a particular one. What it must do is name ONE response, so it
/// is drawn from a per-process counter mixed with the instant the response was
/// answered through the maintained `sha2` digest — the counter is what makes two
/// ids minted in the same nanosecond differ, and the instant is what keeps two
/// runs of this emulator from minting the same sequence. It is an identifier for
/// correlation, never a secret: nothing authenticates with it.
///
/// A free function rather than a mint on the store: a failure the request
/// boundary refused never reaches a handler, so the envelope that answers it has
/// no state to reach.
pub fn new_request_id() -> String {
    use sha2::{Digest, Sha256};

    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    let mut digest = Sha256::new();
    digest.update(counter.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let hex: String = digest.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Whether `vpc_id` has the form of an Amazon VPC id.
///
/// A VPC id is the literal `vpc-` followed by the resource's lowercase
/// hexadecimal identifier — 8 characters in the original form, 17 in the long
/// form issued today — and nothing else names a VPC. `InvalidVPCId`'s own
/// documentation is the refusal for anything else: "The VPC ID that you
/// specified either isn't a valid ID or the current account is not authorized
/// to access this VPC."
pub fn vpc_id_is_well_formed(vpc_id: &str) -> bool {
    let Some(body) = vpc_id.strip_prefix("vpc-") else {
        return false;
    };
    matches!(body.len(), 8 | 17)
        && body
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `region` is one of the regions the model's `VPCRegion` enum names.
///
/// `VPC$VPCRegion` targets that enum, so a value outside it names no region a
/// VPC can have been created in.
pub fn vpc_region_is_modelled(region: &str) -> bool {
    V_P_C_REGION_VALUES.contains(&region)
}

/// A hosted-zone id with the `/hostedzone/` prefix stripped, so a caller may
/// pass either the bare id or the prefixed `Id` a response handed it back.
pub fn normalize_zone_id(id: &str) -> &str {
    strip_id_prefix(id, "hostedzone")
}

/// A change id with the `/change/` prefix stripped, for the same reason
/// [`normalize_zone_id`] strips `/hostedzone/`.
pub fn normalize_change_id(id: &str) -> &str {
    strip_id_prefix(id, "change")
}

/// Strip a leading `/`, then a leading `<collection>/`, from a resource id.
fn strip_id_prefix<'a>(id: &'a str, collection: &str) -> &'a str {
    let without_slash = id.strip_prefix('/').unwrap_or(id);
    without_slash
        .strip_prefix(collection)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(without_slash)
}

/// The id generator behind every minted zone and change id.
///
/// A plain xorshift64 sequence, seeded from the wall clock on first use: ids
/// have to be unique and opaque, not cryptographically unpredictable, and
/// keeping the generator in the state struct is what makes them unique ACROSS
/// calls rather than per handler.
#[derive(Debug, Default)]
struct IdMinter {
    seed: u64,
}

impl IdMinter {
    /// The alphabet Route 53 ids are drawn from: uppercase letters and digits.
    const ALPHABET: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    fn next_u64(&mut self) -> u64 {
        if self.seed == 0 {
            // Any non-zero seed will do; the clock keeps two emulator runs from
            // minting the same first id. The constant is the fallback for a
            // clock that reads before the epoch.
            self.seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0x2545_F491_4F6C_DD1D, |d| d.as_nanos() as u64)
                | 1;
        }
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }

    /// 8-4-4-4-12 lowercase hex, the form of the ids Route 53 mints for the
    /// resources it does not give a letter-prefixed id (a traffic policy).
    fn mint_uuid(&mut self) -> String {
        let hex = format!("{:016x}{:016x}", self.next_u64(), self.next_u64());
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// `prefix` followed by `len` characters from [`IdMinter::ALPHABET`].
    fn mint(&mut self, prefix: char, len: usize) -> String {
        let mut id = String::with_capacity(len + 1);
        id.push(prefix);
        for _ in 0..len {
            let pick = (self.next_u64() % Self::ALPHABET.len() as u64) as usize;
            id.push(char::from(Self::ALPHABET[pick]));
        }
        id
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53Awsdnsv20130401State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53Awsdnsv20130401State>)
}
