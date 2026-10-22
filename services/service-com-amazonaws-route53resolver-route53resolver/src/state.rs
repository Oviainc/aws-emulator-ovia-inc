// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53resolver-route53resolver service.
//!
//! Besides the state struct this module owns the small pieces the Resolver
//! endpoint operations share, so no two handlers grow their own copy: id
//! minting, the RFC 3339 clock read, the clock-owed status transition, the
//! opaque pagination cursor the two `@paginated` list operations mint and
//! verify, the record -> modelled-shape projections, and the RSLVR-coded
//! exception message every endpoint operation reports a missing endpoint with.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{IpAddressResponse, ResolverEndpoint, ResourceNotFoundException, Tag};

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "route53resolver";

/// `ResolverEndpointStatus` values this slice reports. The model's enum
/// (`smithy_query(query="shape",shape="ResolverEndpointStatus")`) also carries
/// `AUTO_RECOVERING` and `ACTION_NEEDED`, which describe a degraded endpoint the
/// emulator never produces.
pub const ENDPOINT_STATUS_CREATING: &str = "CREATING";
pub const ENDPOINT_STATUS_OPERATIONAL: &str = "OPERATIONAL";
pub const ENDPOINT_STATUS_UPDATING: &str = "UPDATING";
pub const ENDPOINT_STATUS_DELETING: &str = "DELETING";

/// `IpAddressStatus` values this slice reports, out of the model's enum
/// (`smithy_query(query="shape",shape="IpAddressStatus")`); the rest describe
/// failures the emulator never produces.
pub const IP_STATUS_CREATING: &str = "CREATING";
pub const IP_STATUS_ATTACHED: &str = "ATTACHED";
pub const IP_STATUS_UPDATING: &str = "UPDATING";

/// The protocol an endpoint gets when the request names none.
///
/// `CreateResolverEndpoint`'s `Protocols` doc: "None, which is treated as
/// Do53." `Do53` is a member of the model's `Protocol` enum.
pub const DEFAULT_PROTOCOL: &str = "Do53";

/// How long a Resolver endpoint (and each IP address being attached) reports
/// its transient status before the clock owes the move to the settled one.
///
/// AWS answers `CreateResolverEndpoint` with `Status: CREATING` and takes
/// minutes to reach `OPERATIONAL`; an emulator must not make a caller wait that
/// long, so the transition is owed after this span instead. It is deliberately
/// not zero: a caller that reads straight after the create still observes the
/// transient status, exactly as it would against AWS.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// Page size applied when a request omits `MaxResults`: the maximum the model's
/// `MaxResults` range (1..100) allows, so an unpaged caller sees everything it
/// can in one page.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most Resolver endpoints this store holds.
///
/// `CreateResolverEndpoint` admits any new `CreatorRequestId`, so without this
/// bound a caller looping creates would grow one emulator process without
/// limit (each record carries up to 20 IP addresses plus its tags and security
/// groups). A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_endpoint`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record, and a delete frees a slot. Bounding the endpoints bounds
/// [`ComAmazonawsRoute53resolverRoute53resolverState::creator_request_id_index`]
/// too: it only ever holds one entry per stored endpoint.
pub const MAX_STORED_ENDPOINTS: usize = 1_000;

/// The most `Filters` one `ListResolverEndpoints` request may carry, and the
/// most filter values it may carry across all of them.
///
/// The model constrains neither `Filters` nor `Filter$Values`, so without these
/// the only ceiling is the transport body cap — and the scan's cost is the
/// number of stored endpoints MULTIPLIED by the filter terms, so an uncapped
/// request would turn one call into unbounded CPU while the store lock is held.
/// The documented catalog has 7 members, so ten filters is already more than any
/// meaningful query needs. A request past either cap is refused with the
/// `InvalidParameterException` the operation models, before the store is locked.
pub const MAX_FILTERS: usize = 10;

/// See [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 100;

/// The most `SecurityGroupIds` one endpoint may be created with.
///
/// The model constrains the list's length no more than it does `Filters`, and a
/// stored endpoint's groups are walked once per filtered scan, so this bounds
/// both what one record can hold and the per-record cost of filtering on
/// `SecurityGroupIds`. A create past it is refused with the
/// `LimitExceededException` the operation models.
pub const MAX_SECURITY_GROUP_IDS: usize = 100;

/// The `MaxResults` bounds the model declares for both list operations.
const MAX_RESULTS_RANGE: (i32, i32) = (1, 100);

/// Domain-separation string mixed into a pagination cursor's checksum. NOT a
/// secret and not a security boundary: it exists so a `NextToken` this emulator
/// never minted is detected and refused, instead of being silently read as
/// "start from the beginning".
const PAGE_TOKEN_DOMAIN: &str = "route53resolver/page-token/v1";

/// The Route 53 Resolver service error code reported when a request names a
/// Resolver endpoint id the service holds no endpoint under.
///
/// Route 53 Resolver carries its own `RSLVR-nnnnn` code inside every exception
/// message, because the HTTP status (400 for every modelled Resolver error) and
/// the exception shape name together still do not say which Resolver fault
/// occurred — the code is what operators and tooling key on. The model states
/// no message text, so this code comes from the recorded responses of the
/// endpoint operations that raise `ResourceNotFoundException`.
pub const RSLVR_CODE_ENDPOINT_NOT_FOUND: &str = "RSLVR-00501";

/// `ResourceNotFoundException$ResourceType`, "the type of resource that doesn't
/// exist": the modelled shape name of a Resolver endpoint.
pub const RESOURCE_TYPE_RESOLVER_ENDPOINT: &str = "ResolverEndpoint";

/// Render one Route 53 Resolver exception message: the service error code in
/// square brackets, a space, the sentence that states the fault, and the
/// terminating period.
///
/// `sentence` is passed WITHOUT its final period, so the shape of the message
/// is decided in exactly one place.
pub fn rslvr_message(code: &str, sentence: &str) -> String {
    format!("[{code}] {sentence}.")
}

/// The `ResourceNotFoundException` every Resolver endpoint operation answers
/// for an id it holds no endpoint under.
///
/// Built here rather than in each handler so the four operations that resolve a
/// `ResolverEndpointId` (`GetResolverEndpoint`,
/// `ListResolverEndpointIpAddresses`, `UpdateResolverEndpoint`,
/// `DeleteResolverEndpoint`) cannot drift apart: all of them report the
/// RSLVR-coded message, with the requested id double-quoted inside it, and the
/// resource type the model asks for.
pub fn resolver_endpoint_not_found(resolver_endpoint_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_ENDPOINT_NOT_FOUND,
            &format!(r#"Resolver endpoint with ID "{resolver_endpoint_id}" does not exist"#),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_ENDPOINT.to_string()),
    }
}

/// One IP address of a Resolver endpoint, as stored.
#[derive(Debug, Clone)]
pub struct IpAddressRecord {
    /// The emulator-minted `IpId` (`rni-<16 hex>`).
    pub ip_id: String,
    pub subnet_id: String,
    pub ip: Option<String>,
    pub ipv6: Option<String>,
    /// The transient `IpAddressStatus` this IP reports until [`Self::attached_at`]
    /// falls due.
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `ATTACHED` is owed.
    pub attached_at: Instant,
}

impl IpAddressRecord {
    /// This IP's `IpAddressStatus` as of `now`: the transient status until the
    /// attach the clock owes falls due, `ATTACHED` after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.attached_at {
            &self.status
        } else {
            IP_STATUS_ATTACHED
        }
    }

    /// Project the record onto the modelled `IpAddressResponse`.
    pub fn to_response(&self, now: Instant) -> IpAddressResponse {
        IpAddressResponse {
            ip_id: Some(self.ip_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            ip: self.ip.clone(),
            ipv6: self.ipv6.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }
}

/// A Resolver endpoint, as stored. `IpAddressCount` is DERIVED from
/// [`Self::ip_addresses`] and never held separately, so the count and the
/// addresses cannot disagree.
#[derive(Debug, Clone)]
pub struct ResolverEndpointRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::endpoint_id_seq`]. It
    /// is the list order and the source of the minted ids.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub creator_request_id: String,
    pub security_group_ids: Vec<String>,
    pub direction: String,
    pub host_vpc_id: Option<String>,
    /// The transient `ResolverEndpointStatus` reported until [`Self::ready_at`]
    /// falls due (`CREATING` after a create, `UPDATING` after an update).
    pub status: String,
    pub status_message: Option<String>,
    pub endpoint_type: Option<String>,
    pub protocols: Vec<String>,
    pub outpost_arn: Option<String>,
    pub preferred_instance_type: Option<String>,
    pub rni_enhanced_metrics_enabled: Option<bool>,
    pub target_name_server_metrics_enabled: Option<bool>,
    pub dns64_enabled: Option<bool>,
    pub ipv6_internet_access_enabled: Option<bool>,
    pub ip_addresses: Vec<IpAddressRecord>,
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `OPERATIONAL` is owed.
    pub ready_at: Instant,
}

impl ResolverEndpointRecord {
    /// This endpoint's `ResolverEndpointStatus` as of `now`: the transient
    /// status until the transition the clock owes falls due, `OPERATIONAL`
    /// after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.ready_at {
            &self.status
        } else {
            ENDPOINT_STATUS_OPERATIONAL
        }
    }

    /// `IpAddressCount`: the number of IP addresses stored for this endpoint.
    pub fn ip_address_count(&self) -> i32 {
        i32::try_from(self.ip_addresses.len()).unwrap_or(i32::MAX)
    }

    /// Project the record onto the modelled `ResolverEndpoint`.
    pub fn to_endpoint(&self, now: Instant) -> ResolverEndpoint {
        ResolverEndpoint {
            id: Some(self.id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            security_group_ids: Some(self.security_group_ids.clone()),
            direction: Some(self.direction.clone()),
            ip_address_count: Some(self.ip_address_count()),
            host_vpc_id: self.host_vpc_id.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            outpost_arn: self.outpost_arn.clone(),
            preferred_instance_type: self.preferred_instance_type.clone(),
            resolver_endpoint_type: self.endpoint_type.clone(),
            protocols: Some(self.protocols.clone()),
            rni_enhanced_metrics_enabled: self.rni_enhanced_metrics_enabled,
            target_name_server_metrics_enabled: self.target_name_server_metrics_enabled,
            dns64_enabled: self.dns64_enabled,
            ipv6_internet_access_enabled: self.ipv6_internet_access_enabled,
        }
    }

    /// Whether this endpoint satisfies one `ListResolverEndpoints` filter: does
    /// any value it presents for `key` appear in `values`?
    ///
    /// A member that holds several values (`SecurityGroupIds`) matches when ANY
    /// of them is among the filter values. The test allocates nothing per record
    /// (bar the fixed-size decimal rendering of `IpAddressCount`) and is a set
    /// lookup rather than a scan of the filter values, so the cost of a filtered
    /// page is O(stored endpoints x filter keys) — not multiplied by how many
    /// values the request carried.
    pub fn matches_filter(&self, key: FilterKey, values: &HashSet<String>, now: Instant) -> bool {
        match key {
            FilterKey::CreatorRequestId => values.contains(self.creator_request_id.as_str()),
            FilterKey::Direction => values.contains(self.direction.as_str()),
            FilterKey::HostVpcId => self
                .host_vpc_id
                .as_deref()
                .is_some_and(|id| values.contains(id)),
            FilterKey::IpAddressCount => values.contains(&self.ip_address_count().to_string()),
            FilterKey::Name => self.name.as_deref().is_some_and(|name| values.contains(name)),
            FilterKey::SecurityGroupIds => self
                .security_group_ids
                .iter()
                .any(|group| values.contains(group.as_str())),
            FilterKey::Status => values.contains(self.status_at(now)),
        }
    }
}

/// The member a `ListResolverEndpoints` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverEndpoints**): `CreatorRequestId`, `Direction`, `HostVPCId`,
/// `IpAddressCount`, `Name`, `SecurityGroupIds`, `Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKey {
    CreatorRequestId,
    Direction,
    HostVpcId,
    IpAddressCount,
    Name,
    SecurityGroupIds,
    Status,
}

impl FilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreatorRequestId => "CreatorRequestId",
            Self::Direction => "Direction",
            Self::HostVpcId => "HostVPCId",
            Self::IpAddressCount => "IpAddressCount",
            Self::Name => "Name",
            Self::SecurityGroupIds => "SecurityGroupIds",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog.
    ///
    /// The same doc note says the early, upper-snake spellings
    /// (`CREATOR_REQUEST_ID` for `CreatorRequestId`) "are still supported", so
    /// the name is folded to upper case with `_` removed before matching —
    /// which accepts both spellings and nothing else.
    pub fn parse(name: &str) -> Option<Self> {
        let folded: String = name
            .chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_uppercase)
            .collect();
        match folded.as_str() {
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DIRECTION" => Some(Self::Direction),
            "HOSTVPCID" => Some(Self::HostVpcId),
            "IPADDRESSCOUNT" => Some(Self::IpAddressCount),
            "NAME" => Some(Self::Name),
            "SECURITYGROUPIDS" => Some(Self::SecurityGroupIds),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// Per-service state for com-amazonaws-route53resolver-route53resolver. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53resolverRoute53resolverState {
    /// Every stored Resolver endpoint, keyed by its `Id`.
    pub resolver_endpoints: HashMap<String, ResolverEndpointRecord>,
    /// `CreatorRequestId` -> endpoint id, so a retried create with the same
    /// `CreatorRequestId` resolves to the endpoint it already made instead of a
    /// duplicate.
    pub creator_request_id_index: HashMap<String, String>,
    /// Monotonic counter behind every minted endpoint and IP id, so ids are
    /// reproducible within a run and never collide.
    pub endpoint_id_seq: u64,
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Whether one more endpoint may be stored, i.e. the store is still below
    /// [`MAX_STORED_ENDPOINTS`]. A create that this refuses answers
    /// `LimitExceededException` and writes nothing.
    pub fn can_store_another_endpoint(&self) -> bool {
        self.resolver_endpoints.len() < MAX_STORED_ENDPOINTS
    }

    /// Take the next value of the id sequence.
    pub fn next_seq(&mut self) -> u64 {
        self.endpoint_id_seq = self.endpoint_id_seq.saturating_add(1);
        self.endpoint_id_seq
    }

    /// Every stored endpoint in creation order — the order both the list
    /// operation's pages and its cursor are defined over.
    pub fn endpoints_in_creation_order(&self) -> Vec<&ResolverEndpointRecord> {
        let mut all: Vec<&ResolverEndpointRecord> = self.resolver_endpoints.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53resolverRoute53resolverState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53resolverRoute53resolverState>)
}

/// The wall clock, as the `Rfc3339TimeString` the model's time members declare
/// (length 20..40).
pub fn now_rfc3339() -> String {
    aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The region the request was signed for, which is the region of every ARN the
/// request mints. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope.
pub fn request_region(headers: &http::HeaderMap) -> String {
    emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Spread a counter value over 64 bits, so a minted id reads like the opaque hex
/// AWS returns while staying a pure function of the counter. splitmix64's
/// finaliser is a bijection on `u64`, so distinct counter values never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Mint the `Id` of a Resolver endpoint from the id sequence.
///
/// AWS names an inbound endpoint `rslvr-in-<hex>` and an outbound one
/// `rslvr-out-<hex>`; `INBOUND_DELEGATION` is an inbound direction, so it takes
/// the inbound infix. Every result fits `ResourceId` (length 1..64).
pub fn mint_endpoint_id(direction: &str, seq: u64) -> String {
    let infix = if direction == "OUTBOUND" { "out" } else { "in" };
    format!("rslvr-{infix}-{:016x}", scramble(seq))
}

/// Mint the `IpId` of one endpoint IP address from the id sequence. AWS names a
/// Resolver network interface `rni-<hex>`.
pub fn mint_ip_id(seq: u64) -> String {
    format!("rni-{:016x}", scramble(seq))
}

/// The `HostVPCId` of an endpoint placed in `subnet_ids`.
///
/// `Filter`'s doc for `HostVPCId` says "In a CreateResolverEndpoint request,
/// SubnetId indirectly identifies the VPC", and the emulator holds no VPC
/// inventory to look the subnets up in, so the id is derived from the subnets
/// themselves: the same set of subnets always yields the same VPC id, and a
/// second endpoint in those subnets reports the same `HostVPCId` as the first.
pub fn host_vpc_id(subnet_ids: &[String]) -> String {
    let mut distinct: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
    distinct.sort_unstable();
    distinct.dedup();
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/host-vpc/v1");
    for subnet in distinct {
        hasher.update(b":");
        hasher.update(subnet.as_bytes());
    }
    let digest = hasher.finalize();
    format!("vpc-{}", hex::encode(&digest[..8]))
}

/// Feed a length-prefixed field into a digest, so two different sequences of
/// fields cannot hash to the same byte stream whatever the fields contain.
fn feed_field(hasher: &mut Sha256, field: &[u8]) {
    hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(field);
}

/// The cursor scope of one `ListResolverEndpoints` query.
///
/// The offset a cursor carries is an index into the FILTERED collection, so a
/// cursor only means anything for the filter set it was minted under. Folding
/// that set into the scope (canonically, so the order the filters and their
/// values arrive in does not matter) means resuming with a different `Filters`
/// is answered with `InvalidNextTokenException` rather than with a page cut at
/// the wrong position.
pub fn endpoint_list_scope(filters: &[(FilterKey, HashSet<String>)]) -> String {
    let mut described: Vec<(&'static str, Vec<&str>)> = filters
        .iter()
        .map(|(key, values)| {
            let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
            values.sort_unstable();
            (key.canonical_name(), values)
        })
        .collect();
    described.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"ListResolverEndpoints/filters/v1");
    for (name, values) in &described {
        feed_field(&mut hasher, name.as_bytes());
        for value in values {
            feed_field(&mut hasher, value.as_bytes());
        }
    }
    format!(
        "ListResolverEndpoints/{}",
        hex::encode(&hasher.finalize()[..8])
    )
}

/// One page of a `@paginated` list operation.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The cursor that resumes where this page stopped, or `None` on the last
    /// page.
    pub next_token: Option<String>,
}

/// The checksum carried inside a pagination cursor.
fn page_token_digest(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN.as_bytes());
    hasher.update(b":");
    hasher.update(payload.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Mint the opaque cursor that resumes `scope`'s collection at `offset`.
///
/// `scope` names the collection being paged (it carries the endpoint id for a
/// per-endpoint collection, and the filter set for a filtered one), so a cursor
/// minted for one collection is not accepted for another.
pub fn mint_page_token(scope: &str, offset: usize) -> String {
    let payload = format!("{scope}:{offset}");
    let digest = page_token_digest(&payload);
    hex::encode(format!("{payload}:{digest}"))
}

/// Read a cursor this emulator minted for `scope`, or `None` for one it did not
/// — which a list operation answers with `InvalidNextTokenException`.
pub fn parse_page_token(scope: &str, token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (payload, digest) = decoded.rsplit_once(':')?;
    if digest != page_token_digest(payload) {
        return None;
    }
    let (token_scope, offset) = payload.rsplit_once(':')?;
    if token_scope != scope {
        return None;
    }
    offset.parse().ok()
}

/// Cut one page out of `all`, honouring the `@paginated` contract: resume at the
/// cursor, truncate at `MaxResults`, and mint a cursor only when items remain.
///
/// `None` means `next_token` is not a cursor this emulator minted for `scope`.
pub fn paginate<T>(
    all: Vec<T>,
    scope: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page<T>> {
    let offset = match next_token {
        Some(token) => parse_page_token(scope, token)?,
        None => 0,
    };
    let page_size = max_results
        .map(|requested| requested.clamp(MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1))
        .and_then(|bounded| usize::try_from(bounded).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE);

    // One item past the page is enough to know whether a cursor is owed, so the
    // allocation is bounded by the page size rather than by the collection.
    let mut items: Vec<T> = all
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let next_token = if items.len() > page_size {
        items.truncate(page_size);
        Some(mint_page_token(scope, offset.saturating_add(page_size)))
    } else {
        None
    };
    Some(Page { items, next_token })
}
