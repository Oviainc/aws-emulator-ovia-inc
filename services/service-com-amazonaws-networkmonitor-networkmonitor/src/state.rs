// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmonitor-networkmonitor service.
//!
//! CloudWatch Network Monitor's primary resource is the MONITOR, and a monitor
//! CONTAINS its probes, so the store holds exactly one record per monitor with
//! its probes nested inside it:
//!
//! * [`Monitor`] — what `CreateMonitor` inserts, `GetMonitor` / `ListMonitors`
//!   report and `DeleteMonitor` removes.
//! * [`Probe`] — what `CreateProbe` attaches to a monitor and `DeleteProbe`
//!   detaches. There is no second probe index: `GetMonitor`'s `probes` list is
//!   the stored map, so a probe can never appear in one place and not the other.
//!
//! Every value a handler reports is read back out of these records; nothing is
//! echoed straight from a request without being stored first.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `networkmonitor`").
const ARN_NAMESPACE: &str = "networkmonitor";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `MonitorState` of a monitor that is provisioning the probes it holds and is
/// not aggregating metrics yet (model enum `MonitorState`). TRANSIENT: the clock
/// owes such a monitor the [`MONITOR_STATE_ACTIVE`] transition
/// ([`ACTIVATION_AFTER`]).
pub const MONITOR_STATE_PENDING: &str = "PENDING";
/// `MonitorState` of a monitor whose probes are live.
pub const MONITOR_STATE_ACTIVE: &str = "ACTIVE";
/// `MonitorState` of a monitor that holds no probe.
///
/// A monitor aggregates what its probes measure, so one with no probe has
/// nothing to activate: it is reported `INACTIVE` straight away and stays there
/// — nothing is due — until a probe is added. This is the state a monitor
/// created with an empty (or absent) `probes` list is in, and the one a monitor
/// returns to when its last probe is deleted.
pub const MONITOR_STATE_INACTIVE: &str = "INACTIVE";

/// `ProbeState` of a probe that is not sending packets yet (model enum
/// `ProbeState`). SERVICE-ASSIGNED: the service puts a probe here while it
/// provisions, so a caller cannot ask for it ([`SETTABLE_PROBE_STATES`]).
pub const PROBE_STATE_PENDING: &str = "PENDING";
/// `ProbeState` of a live probe.
pub const PROBE_STATE_ACTIVE: &str = "ACTIVE";
/// `ProbeState` of a probe a caller has switched off: it keeps its
/// configuration but sends no packets.
pub const PROBE_STATE_INACTIVE: &str = "INACTIVE";

/// The `ProbeState` values a CALLER may ask a probe to be put into.
///
/// The model's `ProbeState` enum is the catalog of states a probe can be
/// REPORTED in (`PENDING | ACTIVE | INACTIVE | ERROR | DELETING | DELETED`),
/// and most of those the service assigns itself over a probe's lifecycle: it
/// provisions through `PENDING`, records its own failure as `ERROR` and tears a
/// probe down through `DELETING` / `DELETED`. The only states a request names
/// are the two halves of the documented activate/deactivate toggle, so
/// `UpdateProbeInput$state` ("The state of the probe update") admits exactly
/// these; see [`validate_settable_probe_state`].
pub const SETTABLE_PROBE_STATES: &[&str] = &[PROBE_STATE_ACTIVE, PROBE_STATE_INACTIVE];

/// `AddressFamily` of an IPv4 destination (model enum `AddressFamily`).
pub const ADDRESS_FAMILY_IPV4: &str = "IPV4";
/// `AddressFamily` of an IPv6 destination.
pub const ADDRESS_FAMILY_IPV6: &str = "IPV6";

/// The `Protocol` that makes `destinationPort` required, as the model documents
/// for `ProbeInput$destinationPort`: "This is required only if the `protocol` is
/// `TCP` and must be a number between `1` and `65536`."
pub const PROTOCOL_TCP: &str = "TCP";

/// The `aggregationPeriod` values the service accepts, from the model doc for
/// `CreateMonitorInput$aggregationPeriod`: "Valid values are either `30` or
/// `60`. `60` is the default if no period is chosen." The shape's own
/// `@range(min: 30)` admits more, so the handler applies this list.
pub const AGGREGATION_PERIODS: &[i64] = &[30, 60];

/// The `aggregationPeriod` a monitor gets when the create request named none —
/// the documented default (same doc sentence as [`AGGREGATION_PERIODS`]).
pub const DEFAULT_AGGREGATION_PERIOD: i64 = 60;

/// The refusal message for the cross-member rule the model states for both the
/// create and the update path: `ProbeInput$destinationPort` is "required only if
/// the `protocol` is `TCP`", and `UpdateProbeInput$protocol` — "If the protocol
/// is `TCP`, then `port` is also required".
///
/// A FIXED sentence determined by that rule rather than by the request: it names
/// no value the caller sent, so every operation that applies the rule
/// ([`validate_probe_spec`], reached by `CreateMonitor`, `CreateProbe` and
/// `UpdateProbe`) emits this one sentence.
pub const TCP_PORT_REQUIRED_MESSAGE: &str = "Destination port must be provided for TCP protocol.";

/// The smallest `destinationPort` the service accepts, from the model doc for
/// `ProbeInput$destinationPort` ("must be a number between `1` and `65536`").
/// The shape's `@range(min: 0, max: 65536)` admits `0`, so the handler applies
/// the documented floor.
pub const MIN_DESTINATION_PORT: i32 = 1;

/// How long after the call that adds the first probe to a monitor the clock owes
/// that monitor, and that probe, the `PENDING` -> `ACTIVE` transition.
///
/// A CHOSEN span, not a documented one: AWS takes as long as provisioning the
/// probe's network path needs. It is deliberately non-zero, so a caller that
/// reads the monitor straight after creating it still observes `PENDING`, and
/// the due instant is stored on the record ([`Monitor::activates_at`],
/// [`Probe::activates_at`]) so no handler ever sleeps for it.
pub const ACTIVATION_AFTER: Duration = Duration::from_millis(500);

/// The page size `ListMonitors` uses when the caller names no `maxResults`: the
/// model's own `MaxResults` maximum (`@range(min: 1, max: 25)`), so one
/// unpaginated call returns as much as a paginated one could.
pub const MAX_PAGE_SIZE: usize = 25;

/// The most monitors this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every request can add to it, so the service answers the modelled
/// `ServiceQuotaExceededException` past this point rather than growing without
/// bound. AWS enforces a per-account monitor quota of its own and answers that
/// same modelled error.
pub const MAX_MONITORS: usize = 500;

/// The most probes this emulator keeps in ONE monitor. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_MONITORS`]; AWS caps probes per
/// monitor too and answers `ServiceQuotaExceededException`.
pub const MAX_PROBES_PER_MONITOR: usize = 100;

/// The most `clientToken` records this emulator keeps. An EMULATOR-CHOSEN
/// ceiling: the key is a string the CALLER picks, so this is the one store whose
/// keys a caller controls. Past the ceiling the oldest record is dropped — a
/// dropped token simply stops replaying, which is the same answer a caller gets
/// for a token it never sent.
pub const MAX_CLIENT_TOKENS: usize = 1_000;

/// How long a recorded `clientToken` stays replayable. An EMULATOR-CHOSEN span:
/// AWS documents no lifetime for Network Monitor's idempotency tokens, and an
/// in-memory store that never forgets a caller-chosen key grows with the request
/// count.
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The most tags one monitor or probe keeps: an EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_MONITORS`] — every tagging write can add to the
/// store, so it is bounded rather than left to grow with the request count.
///
/// Its VALUE is the modelled `TagMap`'s own `@length(min: 0, max: 200)`, which
/// is the most pairs any response can carry: a resource holding more could not
/// be reported. AWS enforces a tag quota of its own, which may differ; a write
/// past this ceiling is refused with the modelled `ValidationException` naming
/// the bound, since `TagResource` models no quota error.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The AWS service namespace a probe's `sourceArn` must name.
///
/// The model documents `ProbeInput$sourceArn` as "The ARN of the subnet", and a
/// VPC subnet is an Amazon EC2 resource, so the ARN's service segment is `ec2`.
/// The shape's own `@pattern("^arn:.*$")` cannot express that, so the service
/// checks it.
pub const SUBNET_ARN_SERVICE: &str = "ec2";

/// The resource type a VPC subnet ARN names:
/// `arn:aws:ec2:<region>:<account>:subnet/<subnetId>`.
pub const SUBNET_ARN_RESOURCE_TYPE: &str = "subnet";

/// The id prefix every VPC subnet carries (`subnet-0123456789abcdef0`).
pub const SUBNET_ID_PREFIX: &str = "subnet-";

/// The lengths of the id part that follows an EC2 resource-id prefix: AWS's
/// "Resource IDs" documentation gives EC2 ids in the short (8 character) and
/// long (17 character) forms, and nothing else.
pub const EC2_ID_SUFFIX_LENGTHS: &[usize] = &[8, 17];

/// The `ResourceNotFoundException` message for a monitor this service does not
/// hold, built from the request and the caller identity: the account the request
/// was made under, and the `monitorName` it named.
pub fn monitor_not_found_message(monitor_name: &str) -> String {
    format!(
        "No existing monitor found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a `CreateMonitor` naming a monitor this
/// service already holds — the same two facts as
/// [`monitor_not_found_message`], stated as a conflict.
///
/// It names nothing the service generated (no ARN): a message built from the
/// request and the caller identity reads the same in every environment.
pub fn monitor_conflict_message(monitor_name: &str) -> String {
    format!(
        "Existing monitor found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The `ResourceNotFoundException` message for a probe a stored monitor does not
/// hold — the probe's id beside the same account and `monitorName` as
/// [`monitor_not_found_message`].
pub fn probe_not_found_message(probe_id: &str, monitor_name: &str) -> String {
    format!(
        "No existing probe {probe_id} found for account {} and monitorName {monitor_name}.",
        ::engine::emulator_account_id()
    )
}

/// The resource an ARN names, as a message states it: the last `/`- or
/// `:`-separated segment (`subnet-0123…` of a subnet ARN, `my-topic` of an SNS
/// topic ARN). The whole string when it carries no separator.
pub fn arn_resource_id(arn: &str) -> &str {
    arn.rsplit(['/', ':']).next().unwrap_or(arn)
}

/// The subnet id a probe's `sourceArn` names, when it names a VPC subnet of this
/// account at all; `None` for anything else.
///
/// AWS resolves `sourceArn` against the caller's VPC subnets and refuses a probe
/// whose source is not one, so this is the structural half of that lookup an
/// emulator holding no EC2 inventory can perform: the ARN must be an
/// [`SUBNET_ARN_SERVICE`] ARN of this emulator's account
/// ([`::engine::emulator_account_id`]) whose resource is a
/// [`SUBNET_ARN_RESOURCE_TYPE`] with a well-formed EC2 id.
pub fn subnet_id_of(source_arn: &str) -> Option<&str> {
    // arn : partition : service : region : account : resource
    let mut segments = source_arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let _partition = segments.next()?;
    if segments.next()? != SUBNET_ARN_SERVICE {
        return None;
    }
    let _region = segments.next()?;
    if segments.next()? != ::engine::emulator_account_id() {
        return None;
    }
    let (resource_type, subnet_id) = segments.next()?.split_once('/')?;
    if resource_type != SUBNET_ARN_RESOURCE_TYPE {
        return None;
    }
    let suffix = subnet_id.strip_prefix(SUBNET_ID_PREFIX)?;
    if !EC2_ID_SUFFIX_LENGTHS.contains(&suffix.len())
        || !suffix.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(subnet_id)
}

/// The state a monitor is in the instant it is stored: `INACTIVE` when it holds
/// no probe (nothing to aggregate, so nothing is due) and `PENDING` when it
/// holds at least one (the clock owes it [`MONITOR_STATE_ACTIVE`]).
pub fn initial_monitor_state(has_probes: bool) -> &'static str {
    if has_probes {
        MONITOR_STATE_PENDING
    } else {
        MONITOR_STATE_INACTIVE
    }
}

/// The ARN of a monitor: `arn:aws:networkmonitor:<region>:<account>:monitor/<name>`.
pub fn monitor_arn(region: &str, monitor_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("monitor/{monitor_name}"))
}

/// The ARN of a probe: `arn:aws:networkmonitor:<region>:<account>:probe/<probeId>`.
pub fn probe_arn(region: &str, probe_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("probe/{probe_id}"))
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode a pagination cursor: the key of the first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as a `@paginated` token is
/// meant to be, and safe to carry in a `@httpQuery` parameter.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
pub fn decode_page_token(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// The `AddressFamily` of a probe destination, derived from the address itself.
///
/// The model documents `ProbeInput$destination` as "The destination IP address.
/// This must be either `IPV4` or `IPV6`", so the family is a property of the
/// address the caller gave, not a separate input: an IPv4 literal is `IPV4` and
/// an IPv6 literal is `IPV6`. `None` means the destination is not an IP address
/// at all, which the handler refuses.
pub fn address_family_of(destination: &str) -> Option<&'static str> {
    match destination.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Some(ADDRESS_FAMILY_IPV4),
        Ok(IpAddr::V6(_)) => Some(ADDRESS_FAMILY_IPV6),
        Err(_) => None,
    }
}

/// The `aggregationPeriod` a create request asks for, checked against the
/// documented value list.
///
/// `None` means the request named none, which is the documented default
/// ([`DEFAULT_AGGREGATION_PERIOD`]). `Err` carries a message naming the member
/// and the values it admits, for the caller to fix the request from.
pub fn resolve_aggregation_period(requested: Option<i64>) -> Result<i64, String> {
    let Some(period) = requested else {
        return Ok(DEFAULT_AGGREGATION_PERIOD);
    };
    if AGGREGATION_PERIODS.contains(&period) {
        return Ok(period);
    }
    Err(format!(
        "aggregationPeriod must be one of {AGGREGATION_PERIODS:?}, got {period}"
    ))
}

/// What a caller asks for when it asks for a probe: the members
/// `CreateMonitorProbeInput` and `ProbeInput` share, with the tags the request
/// assigns to the probe already resolved.
#[derive(Debug, Clone)]
pub struct ProbeSpec {
    pub source_arn: String,
    pub destination: String,
    pub destination_port: Option<i32>,
    pub protocol: String,
    pub packet_size: Option<i32>,
    pub tags: BTreeMap<String, String>,
}

/// Turn a [`ProbeSpec`] into the record the store keeps, applying the two rules
/// the model documents but its constraints cannot express.
///
/// * `destination` is "the destination IP address … either `IPV4` or `IPV6`", so
///   a destination that is not an IP literal is refused and the stored
///   `addressFamily` is derived from the one that is.
/// * `destinationPort` "is required only if the `protocol` is `TCP` and must be
///   a number between `1` and `65536`", so TCP without a port, and a port of
///   `0` (which the shape's `@range(min: 0)` admits), are refused.
///
/// `Err` carries a message naming the offending member.
/// * `sourceArn` is "The ARN of the subnet", so an ARN that names anything other
///   than a VPC subnet of this account ([`subnet_id_of`]) is refused — the
///   shape's `@pattern("^arn:.*$")` admits any ARN at all.
pub fn build_probe(
    spec: ProbeSpec,
    probe_id: String,
    region: &str,
    now: SystemTime,
) -> Result<Probe, String> {
    let address_family = validate_probe_spec(&spec)?;
    Ok(Probe {
        probe_arn: probe_arn(region, &probe_id),
        probe_id,
        source_arn: spec.source_arn,
        destination: spec.destination,
        destination_port: spec.destination_port,
        protocol: spec.protocol,
        packet_size: spec.packet_size,
        address_family: address_family.to_string(),
        vpc_id: None,
        state: PROBE_STATE_PENDING.to_string(),
        activates_at: now
            .checked_add(ACTIVATION_AFTER)
            .unwrap_or(now),
        tags: spec.tags,
        created_at: now,
        modified_at: now,
    })
}

/// Check the rules every stored probe satisfies — the ones the model documents
/// in prose but its constraints cannot express — and answer the
/// `addressFamily` the spec's `destination` implies.
///
/// Shared by the operations that WRITE a probe record ([`build_probe`] for a
/// create, and `UpdateProbe` on the record it has merged the request into), so a
/// probe an update leaves behind satisfies exactly what a create would have
/// refused. `Err` carries a message naming the offending member.
pub fn validate_probe_spec(spec: &ProbeSpec) -> Result<&'static str, String> {
    if subnet_id_of(&spec.source_arn).is_none() {
        return Err(format!(
            "The specified subnet {} does not exist or could not be found.",
            arn_resource_id(&spec.source_arn)
        ));
    }
    let Some(address_family) = address_family_of(&spec.destination) else {
        return Err(format!(
            "destination must be an IPv4 or IPv6 address, got {:?}",
            spec.destination
        ));
    };
    if spec.protocol == PROTOCOL_TCP && spec.destination_port.is_none() {
        return Err(TCP_PORT_REQUIRED_MESSAGE.to_string());
    }
    if let Some(port) = spec.destination_port {
        if port < MIN_DESTINATION_PORT {
            return Err(format!(
                "destinationPort must be at least {MIN_DESTINATION_PORT}, got {port}"
            ));
        }
    }
    Ok(address_family)
}

/// Check that `requested` is a state a CALLER may put a probe into.
///
/// The modelled `ProbeState` enum carries the service's own lifecycle values
/// beside the settable ones, so the enum alone cannot express this: the
/// allow-list ([`SETTABLE_PROBE_STATES`]) is enforced by the operation that
/// takes the member. `Err` carries the message that names the rejected status,
/// which is what the caller has to fix in its request.
pub fn validate_settable_probe_state(requested: &str) -> Result<(), String> {
    if SETTABLE_PROBE_STATES.contains(&requested) {
        return Ok(());
    }
    Err(format!("{requested} is not a valid probe status."))
}

/// A probe: one source-subnet-to-destination path inside a monitor.
#[derive(Debug, Clone)]
pub struct Probe {
    pub probe_id: String,
    pub probe_arn: String,
    pub source_arn: String,
    pub destination: String,
    pub destination_port: Option<i32>,
    pub protocol: String,
    pub packet_size: Option<i32>,
    /// Derived from `destination` by [`address_family_of`].
    pub address_family: String,
    /// The VPC of the source subnet. AWS reads it from the subnet named by
    /// `source_arn`; a subnet ARN does not carry its VPC id and this emulator
    /// holds no EC2 subnets, so no stored probe reports one rather than
    /// reporting a VPC that nothing created.
    pub vpc_id: Option<String>,
    pub state: String,
    /// When the clock owes this probe its `PENDING` -> `ACTIVE` transition. See
    /// [`ACTIVATION_AFTER`].
    pub activates_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

impl Probe {
    /// What this probe asks for, as the shared [`ProbeSpec`] states it: the
    /// starting point an update merges a request into, so a stored probe and a
    /// requested one go through the same [`validate_probe_spec`].
    pub fn spec(&self) -> ProbeSpec {
        ProbeSpec {
            source_arn: self.source_arn.clone(),
            destination: self.destination.clone(),
            destination_port: self.destination_port,
            protocol: self.protocol.clone(),
            packet_size: self.packet_size,
            tags: self.tags.clone(),
        }
    }

}

/// A monitor: the container `CreateMonitor` inserts, holding its own probes.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub monitor_name: String,
    pub monitor_arn: String,
    pub state: String,
    pub aggregation_period: i64,
    pub tags: BTreeMap<String, String>,
    /// Probes by `probeId`. A `BTreeMap`, so `GetMonitor` reports them in a
    /// stable order across reads.
    pub probes: BTreeMap<String, Probe>,
    /// When the clock owes this monitor its `PENDING` -> `ACTIVE` transition,
    /// set by the call that gave it its first probe. `None` while the monitor
    /// holds no probes: it has nothing to aggregate, so it stays
    /// [`MONITOR_STATE_INACTIVE`].
    pub activates_at: Option<SystemTime>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// The `clientToken` of the `CreateMonitor` that made it, when one was
    /// given — what makes a repeat of that exact request a retry rather than a
    /// conflict.
    pub client_token: Option<String>,
}

impl Monitor {
    /// Record that this monitor now holds at least one probe, so the clock owes
    /// it the `PENDING` -> `ACTIVE` transition [`ACTIVATION_AFTER`] from `now`.
    ///
    /// Called only by the mutating operations that add a probe. A monitor that
    /// already has an activation due keeps the earlier instant: adding a second
    /// probe does not postpone the first one's activation.
    ///
    /// A monitor that was `INACTIVE` — nothing to aggregate — is provisioning
    /// the new probe, so it moves to `PENDING` and the clock owes it `ACTIVE`.
    pub fn schedule_activation(&mut self, now: SystemTime) {
        if self.activates_at.is_none() {
            self.activates_at = now.checked_add(ACTIVATION_AFTER);
            if self.state == MONITOR_STATE_INACTIVE {
                self.state = MONITOR_STATE_PENDING.to_string();
            }
        }
    }

    /// Give up the activation the clock owed: a monitor with no probes has
    /// nothing to aggregate, so it goes back to `INACTIVE` with nothing due.
    pub fn clear_activation(&mut self) {
        self.activates_at = None;
        self.state = MONITOR_STATE_INACTIVE.to_string();
    }
}

/// Which stored resource the `resourceArn` of a tagging request names.
///
/// The tagging operations address their target by ARN rather than by name
/// (`TagResource`, `UntagResource` and `ListTagsForResource` all bind
/// `resourceArn` to `/tags/{resourceArn}`), and the model documents that target
/// as "a monitor or probe", so an ARN resolves to one of these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A monitor, by `monitorName`.
    Monitor { monitor_name: String },
    /// A probe, by the monitor that holds it and its own `probeId`.
    Probe {
        monitor_name: String,
        probe_id: String,
    },
}

/// What a recorded `clientToken` replays, and when it was recorded so it can be
/// forgotten after [`CLIENT_TOKEN_TTL`].
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    pub monitor_name: String,
    /// The probe the token minted, for a `CreateProbe` token; `None` for a
    /// `CreateMonitor` token.
    pub probe_id: Option<String>,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-networkmonitor-networkmonitor. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// Monitors by `monitorName`, each holding its own probes. Bounded by
    /// [`MAX_MONITORS`].
    pub monitors: BTreeMap<String, Monitor>,
    /// `clientToken` -> what it replays. Bounded by [`MAX_CLIENT_TOKENS`] and
    /// [`CLIENT_TOKEN_TTL`].
    pub client_tokens: BTreeMap<String, ClientTokenRecord>,
    /// Monotonic counter behind [`Self::next_probe_id`]; never reused within a
    /// run.
    probe_seq: u64,
}

impl ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// A fresh, unique `probeId` in the form the model's `ProbeId` pattern
    /// requires: `^probe-[a-z0-9A-Z-]{21,64}$`, here 24 lowercase hex digits
    /// after the prefix.
    ///
    /// Uniqueness comes from `probe_seq`, a per-engine counter, so two ids
    /// minted within one nanosecond still differ; the creation instant supplies
    /// the high bits so ids from separate runs do not collide.
    pub fn next_probe_id(&mut self, now: SystemTime) -> String {
        self.probe_seq = self.probe_seq.wrapping_add(1);
        let nanos = u64::try_from(
            now.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        // The low 32 bits of the counter render as exactly 8 hex digits, so the
        // id is always 24 characters after the prefix — inside the pattern's
        // 21..64.
        let seq = self.probe_seq & 0xffff_ffff;
        format!("probe-{nanos:016x}{seq:08x}")
    }

    /// Materialise the transition the clock has already made due, for ONE
    /// monitor and its probes.
    ///
    /// This is the ONLY write a reporting operation performs: a monitor whose
    /// `activates_at` has passed was owed its activation at that instant whether
    /// or not anybody called, so a read settles it before reporting. Nothing
    /// else about the monitor changes, and `modified_at` is NOT touched: the
    /// caller modified nothing.
    pub fn settle_monitor(&mut self, monitor_name: &str, now: SystemTime) {
        let Some(monitor) = self.monitors.get_mut(monitor_name) else {
            return;
        };
        for probe in monitor.probes.values_mut() {
            if probe.state == PROBE_STATE_PENDING && now >= probe.activates_at {
                probe.state = PROBE_STATE_ACTIVE.to_string();
            }
        }
        if monitor.state == MONITOR_STATE_PENDING
            && monitor
                .activates_at
                .is_some_and(|activates_at| now >= activates_at)
        {
            monitor.state = MONITOR_STATE_ACTIVE.to_string();
        }
    }

    /// The same, for every stored monitor: what `ListMonitors` reports is the
    /// state each monitor is in at the instant of the call, so the whole set is
    /// settled before the page is built. Bounded by [`MAX_MONITORS`].
    pub fn settle_all_monitors(&mut self, now: SystemTime) {
        let names: Vec<String> = self.monitors.keys().cloned().collect();
        for name in names {
            self.settle_monitor(&name, now);
        }
    }

    /// Which stored resource carries `resource_arn`, or `None` when nothing this
    /// service holds does.
    ///
    /// The ARN is compared against the one STORED on each record — the ARN the
    /// service minted and reports — so a tagging call reaches exactly the
    /// resource a read of that ARN reports, and an ARN for a region or an
    /// account this store never minted resolves to nothing.
    pub fn resolve_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        for monitor in self.monitors.values() {
            if monitor.monitor_arn == resource_arn {
                return Some(TaggedResource::Monitor {
                    monitor_name: monitor.monitor_name.clone(),
                });
            }
            for probe in monitor.probes.values() {
                if probe.probe_arn == resource_arn {
                    return Some(TaggedResource::Probe {
                        monitor_name: monitor.monitor_name.clone(),
                        probe_id: probe.probe_id.clone(),
                    });
                }
            }
        }
        None
    }

    /// The tag map stored on one resolved resource, for reporting.
    ///
    /// The same single map [`Self::tags_mut`] hands a tagging write, so what
    /// `ListTagsForResource` reports and what `GetMonitor` reports for the same
    /// resource can never disagree.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::Monitor { monitor_name } => {
                self.monitors.get(monitor_name).map(|monitor| &monitor.tags)
            }
            TaggedResource::Probe {
                monitor_name,
                probe_id,
            } => self
                .monitors
                .get(monitor_name)
                .and_then(|monitor| monitor.probes.get(probe_id))
                .map(|probe| &probe.tags),
        }
    }

    /// The tag map stored on one resolved resource, for writing.
    ///
    /// There is ONE tag map per resource — the same one `GetMonitor` reports —
    /// so a tagging write and a read can never disagree.
    pub fn tags_mut(&mut self, resource: &TaggedResource) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::Monitor { monitor_name } => self
                .monitors
                .get_mut(monitor_name)
                .map(|monitor| &mut monitor.tags),
            TaggedResource::Probe {
                monitor_name,
                probe_id,
            } => self
                .monitors
                .get_mut(monitor_name)
                .and_then(|monitor| monitor.probes.get_mut(probe_id))
                .map(|probe| &mut probe.tags),
        }
    }

    /// Record that a call CHANGED one resolved resource, moving its
    /// `modifiedAt` to `now`.
    ///
    /// Tags are part of the resource every read reports, so a call that removes
    /// or adds one modified the resource. Called only when the stored tag map
    /// actually changed: a tagging call that is a no-op modified nothing.
    pub fn mark_modified(&mut self, resource: &TaggedResource, now: SystemTime) {
        match resource {
            TaggedResource::Monitor { monitor_name } => {
                if let Some(monitor) = self.monitors.get_mut(monitor_name) {
                    monitor.modified_at = now;
                }
            }
            TaggedResource::Probe {
                monitor_name,
                probe_id,
            } => {
                if let Some(probe) = self
                    .monitors
                    .get_mut(monitor_name)
                    .and_then(|monitor| monitor.probes.get_mut(probe_id))
                {
                    probe.modified_at = now;
                }
            }
        }
    }

    /// Forget every `clientToken` record past [`CLIENT_TOKEN_TTL`], then the
    /// oldest records beyond [`MAX_CLIENT_TOKENS`].
    ///
    /// This is the one store whose KEYS the caller chooses, so it is pruned on
    /// every create rather than left to grow with the request count.
    pub fn prune_client_tokens(&mut self, now: SystemTime) {
        self.client_tokens.retain(|_, record| {
            record
                .recorded_at
                .checked_add(CLIENT_TOKEN_TTL)
                .is_none_or(|expires_at| now < expires_at)
        });
        while self.client_tokens.len() > MAX_CLIENT_TOKENS {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            self.client_tokens.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmonitorNetworkmonitorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmonitorNetworkmonitorState>)
}
