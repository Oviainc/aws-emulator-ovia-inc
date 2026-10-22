// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-globalaccelerator-globalaccelerator-v20180706 service.
//!
//! The accelerator control plane this emulator serves: accelerators, their
//! listeners, their attributes, and the bookkeeping the create paths need
//! (idempotency tokens and the static-IP pool). Every handler computes its
//! response from these records; nothing is canned.
//!
//! Shared helpers live here rather than in a new top-level module because
//! `src/lib.rs` is generated and declares the module list.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime};

use crate::ops::types::{Accelerator, IpSet, Listener, PortRange, Tag};

/// `AcceleratorStatus` values, from the model enum of the same name
/// (`ACCELERATOR_STATUS_VALUES` in `ops::types`).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_IN_PROGRESS`].
pub const STATUS_DEPLOYED: &str = "DEPLOYED";

/// `IpAddressType` values, from the model enum of the same name
/// (`IP_ADDRESS_TYPE_VALUES` in `ops::types`).
pub const IP_ADDRESS_TYPE_IPV4: &str = "IPV4";
/// See [`IP_ADDRESS_TYPE_IPV4`].
pub const IP_ADDRESS_TYPE_DUAL_STACK: &str = "DUAL_STACK";

/// `IpAddressFamily` values, from the model enum of the same name
/// (`IP_ADDRESS_FAMILY_VALUES` in `ops::types`).
pub const IP_FAMILY_IPV4: &str = "IPv4";
/// See [`IP_FAMILY_IPV4`].
pub const IP_FAMILY_IPV6: &str = "IPv6";

/// `ClientAffinity`'s documented default: "The default value is `NONE`"
/// (`Listener$ClientAffinity` doc).
pub const CLIENT_AFFINITY_NONE: &str = "NONE";

/// The documented maximum length of an accelerator name: "The name can have a
/// maximum of 64 characters" (`CreateAccelerator$Name` doc).
pub const ACCELERATOR_NAME_MAX_LEN: usize = 64;

/// Accelerators per Amazon Web Services account: the service's documented default
/// quota ("Accelerators per account: 20", AWS Global Accelerator service quotas).
/// A create past it is refused with the modelled `LimitExceededException` — which is
/// also what bounds this store's resident size, so a caller cannot grow it without
/// limit.
pub const MAX_ACCELERATORS_PER_ACCOUNT: usize = 20;

/// Listeners per accelerator: the service's documented default quota ("Listeners per
/// accelerator: 10", AWS Global Accelerator service quotas). Refused the same way.
pub const MAX_LISTENERS_PER_ACCELERATOR: usize = 10;

/// The most idempotency records the store can hold: one per resource that can exist.
/// [`ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State::prune_idempotency`]
/// keeps the invariant that every record points at a LIVE resource, so the map is
/// bounded by the two quotas above rather than by how many tokens a caller has ever
/// sent.
pub const MAX_IDEMPOTENCY_RECORDS: usize =
    MAX_ACCELERATORS_PER_ACCOUNT * (1 + MAX_LISTENERS_PER_ACCELERATOR);

/// The largest page any list returns, and the page it returns when the caller names
/// no `MaxResults`: the model's own `MaxResults` upper bound (range 1..100). A request
/// without the member therefore cannot amplify into a response proportional to the
/// whole store.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// How long after the call that starts a deployment an accelerator stops being
/// reported `IN_PROGRESS` and becomes `DEPLOYED`.
///
/// CHOSEN, not documented: the real service publishes no settling time. It is
/// deliberately non-zero so a caller acting straight after `CreateAccelerator`
/// or `UpdateAccelerator` still observes the transient status, and it is stored
/// as a due instant on the record rather than slept for.
pub const DEPLOYMENT_SETTLE: Duration = Duration::from_millis(500);

/// Base of the static-IPv4 pool: `198.18.0.0`, the first address of
/// `198.18.0.0/15`, the RFC 2544 benchmarking range. Non-routable, so an
/// emulated accelerator never hands a caller an address that belongs to
/// somebody.
const IPV4_POOL_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);
/// `198.18.0.0/15` holds 2^17 addresses; allocation wraps inside it.
const IPV4_POOL_LEN: u32 = 1 << 17;

/// Base of the static-IPv6 pool: `2001:db8::`, the RFC 3849 documentation
/// prefix. Non-routable, for the same reason as [`IPV4_POOL_BASE`].
const IPV6_POOL_BASE: u128 = 0x2001_0db8_u128 << 96;

/// One of an accelerator's IP address sets, as persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIpSet {
    /// An `IpAddressFamily` value: [`IP_FAMILY_IPV4`] or [`IP_FAMILY_IPV6`].
    pub ip_address_family: String,
    /// The addresses in this set; an IP address set holds at most two.
    pub ip_addresses: Vec<String>,
}

impl StoredIpSet {
    /// This set as the modelled `IpSet`.
    ///
    /// `IpFamily` carries the same value as `IpAddressFamily`: the model marks it
    /// `@deprecated` with "IpFamily has been replaced by IpAddressFamily", i.e.
    /// it is an alias, not a second datum. `IpAddressDetails` is left unset —
    /// its `NetworkZone` is a datum this emulator has no rule for, and inventing
    /// one would be fabrication.
    fn to_wire(&self) -> IpSet {
        IpSet {
            ip_family: Some(self.ip_address_family.clone()),
            ip_addresses: Some(self.ip_addresses.clone()),
            ip_address_family: Some(self.ip_address_family.clone()),
            ip_address_details: None,
        }
    }
}

/// An accelerator, as persisted.
#[derive(Debug, Clone)]
pub struct StoredAccelerator {
    /// `AcceleratorArn` — also this record's key in [`GlobalAcceleratorState::accelerators`].
    pub arn: String,
    pub name: String,
    /// An `IpAddressType` value: [`IP_ADDRESS_TYPE_IPV4`] or [`IP_ADDRESS_TYPE_DUAL_STACK`].
    pub ip_address_type: String,
    pub ip_sets: Vec<StoredIpSet>,
    pub dns_name: String,
    /// Present only for a dual-stack accelerator.
    pub dual_stack_dns_name: Option<String>,
    pub enabled: bool,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The instant at which the deployment started by the most recent change
    /// becomes visible as [`STATUS_DEPLOYED`]. Stored, never slept for.
    pub deployed_at: SystemTime,
    /// `Tags`, as key/value pairs in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// The ARNs of this accelerator's listeners, in creation order.
    pub listener_arns: Vec<String>,
}

impl StoredAccelerator {
    /// The deployment status at `now`: `IN_PROGRESS` until the stored due
    /// instant passes, `DEPLOYED` from then on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.deployed_at {
            STATUS_IN_PROGRESS
        } else {
            STATUS_DEPLOYED
        }
    }

    /// This record as the modelled `Accelerator`.
    ///
    /// `Events` is left unset: it is the accelerator's change history, which this
    /// slice does not record.
    pub fn to_wire(&self, now: SystemTime) -> Accelerator {
        Accelerator {
            accelerator_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            enabled: Some(self.enabled),
            ip_sets: Some(self.ip_sets.iter().map(StoredIpSet::to_wire).collect()),
            dns_name: Some(self.dns_name.clone()),
            status: Some(self.status(now).to_string()),
            created_time: Some(to_date_time(self.created_time)),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            dual_stack_dns_name: self.dual_stack_dns_name.clone(),
            events: None,
        }
    }
}

/// A listener, as persisted.
#[derive(Debug, Clone)]
pub struct StoredListener {
    /// `ListenerArn` — also this record's key in [`GlobalAcceleratorState::listeners`].
    pub arn: String,
    /// The ARN of the accelerator this listener belongs to.
    pub accelerator_arn: String,
    /// `PortRanges`, as inclusive `(FromPort, ToPort)` pairs.
    pub port_ranges: Vec<(i32, i32)>,
    /// A `Protocol` value: `TCP` or `UDP`.
    pub protocol: String,
    /// A `ClientAffinity` value: `NONE` or `SOURCE_IP`.
    pub client_affinity: String,
}

impl StoredListener {
    /// This record as the modelled `Listener`.
    pub fn to_wire(&self) -> Listener {
        Listener {
            listener_arn: Some(self.arn.clone()),
            port_ranges: Some(
                self.port_ranges
                    .iter()
                    .map(|&(from, to)| PortRange {
                        from_port: Some(from),
                        to_port: Some(to),
                    })
                    .collect(),
            ),
            protocol: Some(self.protocol.clone()),
            client_affinity: Some(self.client_affinity.clone()),
        }
    }
}

/// An accelerator's flow-log attributes, as persisted. Created with defaults
/// alongside the accelerator, so `Describe`/`UpdateAcceleratorAttributes` can be
/// implemented without a state change.
#[derive(Debug, Clone, Default)]
pub struct StoredAcceleratorAttributes {
    pub flow_logs_enabled: bool,
    pub flow_logs_s3_bucket: Option<String>,
    pub flow_logs_s3_prefix: Option<String>,
}

/// Per-service state for com-amazonaws-globalaccelerator-globalaccelerator-v20180706. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State {
    /// Accelerators by `AcceleratorArn`. `BTreeMap` so list pagination has a
    /// stable total order to resume in.
    pub accelerators: BTreeMap<String, StoredAccelerator>,
    /// Listeners by `ListenerArn`; the parent's `listener_arns` holds the
    /// per-accelerator order.
    pub listeners: BTreeMap<String, StoredListener>,
    /// Flow-log attributes by `AcceleratorArn`.
    pub accelerator_attributes: BTreeMap<String, StoredAcceleratorAttributes>,
    /// `<kind>:<scope>:<IdempotencyToken>` -> the ARN that token already minted, so a
    /// repeated create returns the same resource instead of a second one. The scope is
    /// what the token identifies a create request WITHIN: an accelerator's token is
    /// account-scoped (`accelerator:<token>`), a listener's is scoped to its parent
    /// accelerator (`listener:<AcceleratorArn>:<token>`), so one token cannot replay a
    /// resource that hangs off a different parent.
    pub idempotency: HashMap<String, String>,
    /// Next offset into the static-IPv4 pool, so allocated addresses are unique.
    pub next_ipv4: u32,
    /// Next offset into the static-IPv6 pool.
    pub next_ipv6: u32,
    /// Next sequence number for [`Self::mint_uuid`], so every identifier this store
    /// mints is built from a distinct input.
    pub next_id: u64,
}

/// Short alias for this service's state struct, whose generated name is long.
pub type GlobalAcceleratorState = ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State;

impl ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State {
    /// Allocate the next static IPv4 address from the pool.
    pub fn allocate_ipv4(&mut self) -> String {
        let offset = self.next_ipv4 % IPV4_POOL_LEN;
        self.next_ipv4 = self.next_ipv4.wrapping_add(1);
        Ipv4Addr::from(IPV4_POOL_BASE + offset).to_string()
    }

    /// Allocate the next static IPv6 address from the pool.
    pub fn allocate_ipv6(&mut self) -> String {
        let offset = u128::from(self.next_ipv6);
        self.next_ipv6 = self.next_ipv6.wrapping_add(1);
        Ipv6Addr::from(IPV6_POOL_BASE + offset).to_string()
    }

    /// Mint a fresh identifier in the `8-4-4-4-12` UUID text form AWS puts in an
    /// accelerator ARN.
    ///
    /// Built from this store's own monotonic counter mixed with the current instant
    /// through the standard library's hasher, so each call yields a distinct, opaque
    /// value carrying the version-4 nibble and the RFC 4122 variant bits — a client
    /// that parses an accelerator id out of its ARN sees a well-formed UUID, and the
    /// id needs no identifier source beyond the store and the clock.
    pub fn mint_uuid(&mut self) -> String {
        let sequence = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        format_uuid(mix(sequence, nanos, 0), mix(sequence, nanos, 1))
    }

    /// Drop every idempotency record whose resource is gone.
    ///
    /// This is the release path for [`Self::idempotency`]: a record is only useful
    /// while the resource its token minted still exists, so after this call the map
    /// holds at most one entry per live resource and cannot exceed
    /// [`MAX_IDEMPOTENCY_RECORDS`]. Called on the create paths (before a new record is
    /// added) and on the delete paths (where a record's resource disappears), so a
    /// caller looping create/delete with fresh tokens reclaims the memory each time.
    pub fn prune_idempotency(&mut self) {
        let Self {
            accelerators,
            listeners,
            idempotency,
            ..
        } = self;
        idempotency.retain(|_, minted| {
            accelerators.contains_key(minted) || listeners.contains_key(minted)
        });
        debug_assert!(idempotency.len() <= MAX_IDEMPOTENCY_RECORDS);
    }
}

/// One 64-bit lane of a minted identifier: the standard library's hasher over the
/// sequence number, the instant and the lane index.
fn mix(sequence: u64, nanos: u128, lane: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_u64(sequence);
    hasher.write_u128(nanos);
    hasher.write_u64(lane);
    hasher.finish()
}

/// Render 128 bits as UUID text, with the version-4 nibble and the RFC 4122 variant
/// bits set so the result is a well-formed UUID.
fn format_uuid(high: u64, low: u64) -> String {
    let time_low = (high >> 32) as u32;
    let time_mid = ((high >> 16) & 0xffff) as u16;
    let time_hi_and_version = 0x4000_u16 | (high & 0x0fff) as u16;
    let clock_seq = 0x8000_u16 | ((low >> 48) & 0x3fff) as u16;
    let node = low & 0x0000_ffff_ffff_ffff;
    format!("{time_low:08x}-{time_mid:04x}-{time_hi_and_version:04x}-{clock_seq:04x}-{node:012x}")
}

/// The index of the IPv4 set in an accelerator's `IpSets`, if it has one.
pub fn ipv4_set_index(ip_sets: &[StoredIpSet]) -> Option<usize> {
    ip_sets
        .iter()
        .position(|set| set.ip_address_family == IP_FAMILY_IPV4)
}

/// An `IdempotencyToken` key for the accelerator creates.
pub fn accelerator_idempotency_key(token: &str) -> String {
    format!("accelerator:{token}")
}

/// An `IdempotencyToken` key for the listener creates.
///
/// Scoped by the PARENT accelerator: a token identifies one create request, and the
/// same token sent against a different accelerator is a different request, so it must
/// not replay a listener that hangs off another accelerator.
pub fn listener_idempotency_key(accelerator_arn: &str, token: &str) -> String {
    format!("listener:{accelerator_arn}:{token}")
}

/// Mint an accelerator ARN and the DNS label that goes with it.
///
/// The DNS naming convention is the model's own (`Accelerator$DnsName`): "A
/// lowercase letter a, followed by a 16-bit random hex string, followed by
/// .awsglobalaccelerator.com", with the documented example
/// `a1234567890abcdef.awsglobalaccelerator.com` showing 16 hex digits. The label
/// is taken from the accelerator's own freshly minted id (see
/// [`ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State::mint_uuid`]), so
/// it is unique and stable for the life of the record.
pub fn mint_accelerator(uuid: &str) -> (String, String) {
    let arn = ::engine::emulator_arn("globalaccelerator", "", &format!("accelerator/{uuid}"));
    let hex: String = uuid.chars().filter(|c| *c != '-').take(16).collect();
    (arn, format!("a{hex}"))
}

/// The `DnsName` for a DNS label minted by [`mint_accelerator`].
pub fn dns_name(label: &str) -> String {
    format!("{label}.awsglobalaccelerator.com")
}

/// The `DualStackDnsName` for a DNS label minted by [`mint_accelerator`]:
/// the same label with `.dualstack` inserted, per `Accelerator$DualStackDnsName`.
pub fn dual_stack_dns_name(label: &str) -> String {
    format!("{label}.dualstack.awsglobalaccelerator.com")
}

/// Mint a listener ARN under its parent accelerator, the form
/// `<accelerator arn>/listener/<id>`.
pub fn mint_listener_arn(accelerator_arn: &str, uuid: &str) -> String {
    let id: String = uuid.chars().filter(|c| *c != '-').take(8).collect();
    format!("{accelerator_arn}/listener/{id}")
}

/// Refuse a name the real service refuses.
///
/// The rule is the model's own doc for `CreateAccelerator$Name` /
/// `Accelerator$Name`: "The name can have a maximum of 64 characters, must
/// contain only alphanumeric characters, periods (.), or hyphens (-), and must
/// not begin or end with a hyphen or period." Returns the message to put on an
/// `InvalidArgumentException`.
pub fn validate_accelerator_name(name: &str) -> Result<(), String> {
    let bad = |reason: &str| {
        Err(format!(
            "Name {reason}: a name can have a maximum of {ACCELERATOR_NAME_MAX_LEN} characters, \
             must contain only alphanumeric characters, periods (.), or hyphens (-), and must not \
             begin or end with a hyphen or period"
        ))
    };
    if name.chars().count() > ACCELERATOR_NAME_MAX_LEN {
        return bad("is too long");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return bad("contains an unsupported character");
    }
    let edge = |c: char| c == '-' || c == '.';
    if name.starts_with(edge) || name.ends_with(edge) {
        return bad("begins or ends with a hyphen or period");
    }
    Ok(())
}

/// A single `PortRange`, validated.
///
/// A port range must carry both ends and run forwards: `FromPort` and `ToPort`
/// are the inclusive bounds of one range (`PortRange` doc), so a range missing an
/// end, or with `FromPort > ToPort`, names no ports at all. The model's own
/// `PortNumber` range (1..65535) is refused by the boundary validator before a
/// handler runs; it is re-checked here so the rule holds wherever this helper is
/// called from.
pub fn validate_port_range(range: &PortRange) -> Result<(i32, i32), String> {
    let (Some(from), Some(to)) = (range.from_port, range.to_port) else {
        return Err("a PortRange must carry both FromPort and ToPort".to_string());
    };
    if !(1..=65535).contains(&from) || !(1..=65535).contains(&to) {
        return Err(format!(
            "PortRange {from}-{to} is outside the supported port range 1-65535"
        ));
    }
    if from > to {
        return Err(format!(
            "PortRange {from}-{to} is invalid: FromPort must not be greater than ToPort"
        ));
    }
    Ok((from, to))
}

/// Validate a caller-supplied static IPv4 address (the BYOIP path of
/// `CreateAccelerator$IpAddresses`). Returns the message for an
/// `InvalidArgumentException`.
pub fn validate_static_ipv4(address: &str) -> Result<(), String> {
    if address.parse::<Ipv4Addr>().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "IpAddresses entry {address:?} is not a valid IPv4 address"
        ))
    }
}

/// The whole caller-supplied `IpAddresses` list, validated.
///
/// One accelerator has exactly two static IPv4 addresses, so a caller may name at
/// most two and may not name one twice; each entry must be an IPv4 literal. The
/// single place the rule lives, so the create and the update path cannot drift.
/// Returns the message for an `InvalidArgumentException`.
pub fn validate_static_ipv4_addresses(addresses: &[String]) -> Result<(), String> {
    if addresses.len() > 2 {
        return Err(format!(
            "IpAddresses holds {} addresses; an accelerator has at most two static IPv4 \
             addresses",
            addresses.len()
        ));
    }
    for (index, address) in addresses.iter().enumerate() {
        validate_static_ipv4(address)?;
        if addresses[..index].contains(address) {
            return Err(format!(
                "IpAddresses names {address:?} more than once; an accelerator's static \
                 IPv4 addresses are distinct"
            ));
        }
    }
    Ok(())
}

/// The message for a refusal that names an accelerator the store does not hold.
///
/// A refusal identifies the resource the request named, so the caller — commonly a
/// cleanup loop over many accelerators — can attribute it: the ARN is interpolated
/// from the request rather than left out of a constant sentence. One place for the
/// wording so every operation that answers `AcceleratorNotFoundException` states it
/// alike.
pub fn accelerator_not_found_message(accelerator_arn: &str) -> String {
    format!("Accelerator {accelerator_arn} not found.")
}

/// The message for a refusal that names a listener the store does not hold. The same
/// rule as [`accelerator_not_found_message`], one level down.
pub fn listener_not_found_message(listener_arn: &str) -> String {
    format!("Listener {listener_arn} not found.")
}

/// The message for deleting an accelerator that is still enabled.
///
/// "Before you can delete an accelerator, you must disable it" (`DeleteAccelerator`
/// doc), so the refusal tells the caller which accelerator to disable, by the ARN the
/// request carried.
pub fn accelerator_not_disabled_message(accelerator_arn: &str) -> String {
    format!("Please disable accelerator {accelerator_arn} before deleting it.")
}

/// The message for deleting an accelerator that still has a listener.
///
/// "You must remove all dependent resources from an accelerator before you can delete
/// it" (`AssociatedListenerFoundException` doc) — stated over the request's own ARN,
/// for the same reason as [`accelerator_not_disabled_message`].
pub fn associated_listener_found_message(accelerator_arn: &str) -> String {
    format!(
        "Accelerator {accelerator_arn} has a listener associated with it; delete its \
         listeners before deleting it."
    )
}

/// One page of keys, plus the token that resumes immediately after it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The keys on this page, in the collection's order.
    pub keys: Vec<String>,
    /// The `NextToken` to resume with, absent on the last page.
    pub next_token: Option<String>,
}

/// The caller's `NextToken` did not decode, or named a position the collection no
/// longer holds — an `InvalidNextTokenException`.
#[derive(Debug, Clone, Copy)]
pub struct InvalidPageToken;

/// Encode a `NextToken` that resumes at `key`.
///
/// Hex rather than the bare key: `NextToken` is an opaque continuation token, so
/// a caller must not be able to read or hand-build a position out of it, and a
/// token that is not hex is recognisably not one this service issued.
pub fn encode_next_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a `NextToken` resumes at, or `None` when the token is not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Cut one page out of `all`.
///
/// `max_results` truncates the page; the returned token names the first key of
/// the following page, so resuming with it continues exactly where this page
/// ended, and it is absent on the last page. A token that does not decode, or
/// that names a key the collection no longer holds, is refused.
///
/// The page is ALWAYS clamped to [`DEFAULT_MAX_RESULTS`], which is also the page a
/// caller that names no `MaxResults` gets, so one small request can never answer with
/// the whole store. A non-positive `max_results` is treated as absent: the model's own
/// `MaxResults` range (1..100) makes it unreachable from the wire, and a zero-length
/// page carrying a continuation token would be a page that never makes progress.
pub fn paginate(
    all: &[String],
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page, InvalidPageToken> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let key = decode_next_token(token).ok_or(InvalidPageToken)?;
            all.iter()
                .position(|candidate| *candidate == key)
                .ok_or(InvalidPageToken)?
        }
    };
    let limit = max_results
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(DEFAULT_MAX_RESULTS) as usize;
    let end = start.saturating_add(limit).min(all.len());
    Ok(Page {
        keys: all[start..end].to_vec(),
        next_token: (end < all.len()).then(|| encode_next_token(&all[end])),
    })
}

/// A `Tags` list as the key/value pairs the store holds.
pub fn tags_to_pairs(tags: Option<Vec<Tag>>) -> Vec<(String, String)> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// A `SystemTime` as the modelled `Timestamp`, at millisecond resolution.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlobalacceleratorGlobalacceleratorV20180706State>)
}
