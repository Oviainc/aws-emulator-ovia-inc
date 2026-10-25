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
//!   starts tearing down ([`Probe::start_teardown`]), which leaves the record in
//!   place until the clock has carried its teardown through. There is no second
//!   probe index: `GetMonitor`'s `probes` list is the stored map, so a probe can
//!   never appear in one place and not the other.
//!
//! Those records are held PER REGION
//! ([`ComAmazonawsNetworkmonitorNetworkmonitorState`]): the service answers on a
//! per-region endpoint and every ARN it mints names one region, so a request
//! reaches only the records of the region it was signed for
//! ([`request_region`]).
//!
//! Every value a handler reports is read back out of these records; nothing is
//! echoed straight from a request without being stored first.
//!
//! ## How a refusal this service authors is stated
//!
//! A rule the model states in prose but its constraints cannot express is
//! applied by the service, so the service also has to STATE the refusal. It
//! states it the way AWS states a constraint failure — the
//! `N validation error(s) detected: Value '<submitted>' at '<member path>'
//! failed to satisfy constraint: <requirement>` construction, built from the
//! request — and it builds it through the framework's OWN clause renderer
//! ([`stated_violation`], [`stated_requirement`]), so a refusal a handler raises
//! reads exactly like the one the boundary validator renders for a modelled
//! constraint. A refusal whose AWS sentence is known as a whole
//! ([`TCP_PORT_REQUIRED_MESSAGE`], [`aggregation_period_message`],
//! [`monitor_not_found_message`]) keeps that sentence instead.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_schema_server::constraints::{envelope, path, Violation, ViolationKind};

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
/// `ProbeState` of a probe whose teardown has been accepted and is under way
/// (model enum `ProbeState`). SERVICE-ASSIGNED and TRANSIENT: an accepted
/// `DeleteProbe` puts the probe here and the clock owes it
/// [`PROBE_STATE_DELETED`] ([`PROBE_TEARDOWN_STAGE`]).
pub const PROBE_STATE_DELETING: &str = "DELETING";
/// `ProbeState` of a probe whose teardown has finished. SERVICE-ASSIGNED and
/// TRANSIENT: the record is still readable, and the clock owes its removal one
/// more [`PROBE_TEARDOWN_STAGE`], after which a read of the probe reports it
/// not found.
pub const PROBE_STATE_DELETED: &str = "DELETED";

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

/// What a probe `destination` has to be, as a refusal's clause states the
/// requirement: the model documents `ProbeInput$destination` as "The destination
/// IP address. This must be either `IPV4` or `IPV6`" and
/// `UpdateProbeInput$destination` as "The updated IP address for the probe
/// destination. This must be either an IPv4 or IPv6 address", and the shape
/// carries only `@length(1..255)`, so the format is the service's to check.
const DESTINATION_REQUIREMENT: &str = "Member must be an IPv4 or IPv6 address";

/// How long after the call that adds the first probe to a monitor the clock owes
/// that monitor, and that probe, the `PENDING` -> `ACTIVE` transition.
///
/// A CHOSEN span, not a documented one: AWS takes as long as provisioning the
/// probe's network path needs. It is deliberately non-zero, so a caller that
/// reads the monitor straight after creating it still observes `PENDING`, and
/// the due instant is stored on the record ([`Monitor::activates_at`],
/// [`Probe::activates_at`]) so no handler ever sleeps for it.
pub const ACTIVATION_AFTER: Duration = Duration::from_millis(500);

/// How long one stage of a probe's TEARDOWN lasts: the span from the accepted
/// delete to the `DELETING` -> `DELETED` transition, and the span from there to
/// the record being forgotten.
///
/// A CHOSEN span, not a documented one: AWS takes as long as releasing the
/// probe's network path needs. It is deliberately non-zero, so a caller that
/// reads the probe straight after deleting it still observes the teardown
/// states, and the due instant is stored on the record ([`Probe::deletes_at`])
/// so no handler ever sleeps for it.
pub const PROBE_TEARDOWN_STAGE: Duration = Duration::from_millis(300);

/// The HTTP status a successful TAG MUTATION answers with.
///
/// Both tag mutations leave their success code defaulted in the model (`@http` on
/// `TagResource` and on `UntagResource` carries no `code:`, i.e. `codeDefaulted`), so
/// Smithy's default of 200 is not a statement about this service: the success status is
/// a behavioural fact and the recorded AWS responses are what state it. A tag mutation
/// writes only the tag set of the resource its `resourceArn` names and reports nothing
/// back about it — `TagResourceOutput` and `UntagResourceOutput` declare no members —
/// so there is no entity for a caller to parse and the answer is 204 No Content. A 200
/// there promises a body the response does not carry, and every client path gated on
/// the no-content status (conditional body parsing, caches and proxies that key on it)
/// takes the wrong branch. The two operations share the `/tags/{resourceArn}` route and
/// this one status, so the rule lives here rather than twice in the handlers.
///
/// This does NOT extend to every output structure that happens to declare no members:
/// `DeleteMonitor` and `DeleteProbe` report nothing either, and the recorded AWS
/// responses answer those with the defaulted 200, which they keep.
pub const TAG_MUTATION_SUCCESS_STATUS: u16 = 204;

/// The page size `ListMonitors` uses when the caller names no `maxResults`: the
/// model's own `MaxResults` maximum (`@range(min: 1, max: 25)`), so one
/// unpaginated call returns as much as a paginated one could.
pub const MAX_PAGE_SIZE: usize = 25;

/// The most monitors this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every request can add to it, so the service answers the modelled
/// `ServiceQuotaExceededException` past this point rather than growing without
/// bound. It bounds the store as a whole, across every region it holds. AWS
/// enforces a per-account, per-region monitor quota of its own and answers that
/// same modelled error.
pub const MAX_MONITORS: usize = 500;

/// The most probes this emulator keeps in ONE monitor. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_MONITORS`]; AWS caps the probes of a
/// monitor too. Like every probe-count ceiling this service applies it is
/// refused as a `ValidationException` (see [`PROBES_PER_SUBNET_MESSAGE`]).
pub const MAX_PROBES_PER_MONITOR: usize = 100;

/// The most probes one monitor holds in ONE source subnet: AWS's Network
/// Monitor quota "Number of probes per subnet for each monitor", whose default
/// value this is. The limit is per (monitor, source subnet) pair, so probes
/// spread over separate subnets of the same monitor do not share it.
pub const MAX_PROBES_PER_SUBNET: usize = 4;

/// What this service states when a request would leave a monitor holding more
/// than [`MAX_PROBES_PER_SUBNET`] probes in one source subnet.
///
/// A whole sentence rather than a constraint clause: the refusal is the one AWS
/// states for this limit, and it names neither the member nor the count, so it
/// reads the same whichever operation added the probes. The class is the
/// `ValidationException` AWS answers here, not the `ServiceQuotaExceededException`
/// the model also declares.
pub const PROBES_PER_SUBNET_MESSAGE: &str = "Maximum probes per subnet limit exceeded.";

/// The same, for the probes of a monitor as a whole
/// ([`MAX_PROBES_PER_MONITOR`]): one refusal class and one sentence shape for
/// every probe-count ceiling, so a caller catching the refusal of one catches
/// the refusal of the other.
pub const PROBES_PER_MONITOR_MESSAGE: &str = "Maximum probes per monitor limit exceeded.";

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
/// hold in the region the request was signed for, built from the request and the
/// caller identity: the account the request was made under, and the
/// `monitorName` it named.
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

/// The marker a cursor this service vended carries ahead of the key it
/// encodes.
///
/// A `@paginated` token is opaque, so the CALLER cannot tell a cursor apart
/// from text of its own; the marker is what lets the SERVICE tell them apart,
/// so a string nothing ever vended is still refused while every cursor this
/// service did vend is honoured. The version digit keeps a cursor minted in
/// some other format from being read as a key.
const PAGE_TOKEN_MARKER: &str = "monitor-page/1:";

/// Encode a pagination cursor: the key of the first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as a `@paginated` token is
/// meant to be, and safe to carry in a `@httpQuery` parameter.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_MARKER}{key}"))
}

/// The key a cursor produced by [`encode_page_token`] encodes; `None` when the
/// token was not produced by this service.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_MARKER).map(str::to_owned)
}

/// Where a listing resumes from a cursor: the index of the first item of
/// `ordered` whose key does not sort before `cursor`.
///
/// A cursor names a POSITION in the key order, not a membership. The item it
/// names can be gone by the time the caller resumes — deleted, or no longer
/// matching the filter the listing was made with — and the items after it are
/// still owed to the caller, so the resume falls forward to the next key at or
/// after the cursor instead of requiring the cursor's own item to still be in
/// the set. `ordered.len()` (an empty final page) when every key sorts before
/// the cursor.
///
/// `ordered` must be sorted by `key`, which the `monitorName`-keyed
/// [`BTreeMap`] walk over one region's monitors
/// ([`ComAmazonawsNetworkmonitorNetworkmonitorState::monitors_in`]) already is.
pub fn resume_index<T>(ordered: &[T], cursor: &str, key: impl Fn(&T) -> &str) -> usize {
    ordered.partition_point(|item| key(item) < cursor)
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

/// The `aggregationPeriod` a create or update request asks for, checked against
/// the documented value list.
///
/// `None` means the request named none, which is the documented default
/// ([`DEFAULT_AGGREGATION_PERIOD`]). `Err` carries the sentence the service
/// states for this rule ([`aggregation_period_message`]): the rule is an
/// allow-list the model publishes, so what the refusal states is that list and
/// nothing of the value that missed it.
pub fn resolve_aggregation_period(requested: Option<i64>) -> Result<i64, String> {
    let Some(period) = requested else {
        return Ok(DEFAULT_AGGREGATION_PERIOD);
    };
    if AGGREGATION_PERIODS.contains(&period) {
        return Ok(period);
    }
    Err(aggregation_period_message())
}

/// The text that separates a constraint clause's subject from the requirement
/// it states, in the framework's own wording ([`Violation::message`]). SPLIT on,
/// never written: see [`stated_requirement`].
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// The longest submitted value a refusal echoes back before the rest is elided.
///
/// The cap and the elision text are the framework's own (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`), so a long value refused here reads
/// exactly as the same value refused at the boundary: a clause interpolates a
/// value whose length the CALLER chooses, so an unbounded echo would let a small
/// request name an arbitrarily large response. The checks themselves always read
/// the whole value; this is rendering only.
const MAX_ECHOED_VALUE: usize = 256;

/// A value the caller submitted, as a clause echoes it: itself, or its first
/// [`MAX_ECHOED_VALUE`] bytes followed by the framework's own elision.
fn echoed(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The dotted path of a top-level member of a request, spelled through the
/// framework's own path rule so a clause this service raises names a member
/// exactly as the boundary validator names it.
pub fn member_path(member: &str) -> String {
    path::member(member)
}

/// The dotted path of the `position`-th (1-BASED) element of a list member —
/// `probes.1.member` for the first entry of `CreateMonitor`'s `probes`.
pub fn element_path(member: &str, position: usize) -> String {
    path::join(&[path::member(member), path::element(position)])
}

/// AWS's constraint-violation message for ONE rule this service's own code
/// applies, rendered by the framework's own clause builder.
///
/// `member_path` is where the offending member sat in the REQUEST (the operation
/// that holds the request supplies it, since only it knows that); `submitted` is
/// the value the caller sent, echoed single-quoted and capped ([`echoed`]), or
/// `None` for a member that carried none; `kind` is the constraint class the
/// value broke. The count envelope, the clause and the requirement for `kind`
/// are all [`envelope`]'s and [`Violation::message`]'s, so this cannot drift
/// from the wording a modelled constraint is refused with — and nothing is
/// rendered through Rust's `Debug`, which quotes a string and is a language
/// artefact no AWS service emits.
pub fn stated_violation(member_path: &str, submitted: Option<&str>, kind: ViolationKind) -> String {
    let violation = Violation {
        path: member_path.to_string(),
        kind,
        value: submitted.map(echoed),
    };
    envelope(std::slice::from_ref(&violation)).unwrap_or_else(|| violation.message())
}

/// The same message for a requirement the framework carries no
/// [`ViolationKind`] for — a rule only this service can state, such as which
/// cursors a listing vends, or a format the model documents in prose.
///
/// [`ViolationKind`] is `#[non_exhaustive]` and lives in the framework crate, so
/// such a rule cannot be expressed as one. The envelope and the clause are still
/// the framework's: a clause is rendered for this member and value and only the
/// text after [`CONSTRAINT_MARKER`] is this service's, so the two refusal paths
/// cannot drift into two sentence shapes.
pub fn stated_requirement(
    member_path: &str,
    submitted: Option<&str>,
    requirement: &str,
) -> String {
    // Every class renders the same clause HEAD; this one is simply the class no
    // member of this service can break, so the requirement being replaced is
    // never one a caller could have been shown.
    let rendered = stated_violation(member_path, submitted, ViolationKind::UniqueItems);
    match rendered.rsplit_once(CONSTRAINT_MARKER) {
        Some((head, _)) => format!("{head}{CONSTRAINT_MARKER}{requirement}"),
        None => rendered,
    }
}

/// The requirement an enumerated value set states, over the values as AWS
/// renders them: BARE and SORTED, rather than in the order the model declares
/// them or in any language's quoting.
pub fn enum_value_set(values: &[&str]) -> ViolationKind {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort_unstable();
    ViolationKind::EnumValue(sorted)
}

/// The refusal of a value outside an enumerated type's value set, for a member
/// the model targets a plain `string` with — so the boundary validator admits
/// any text and the operation applies the enum itself.
pub fn enum_value_refusal(member: &str, allowed: &[&str], submitted: &str) -> String {
    stated_violation(
        &member_path(member),
        Some(submitted),
        enum_value_set(allowed),
    )
}

/// The refusal of a pagination cursor `operation` never vended.
///
/// A cursor is opaque — [`encode_page_token`] decides what one is — so the rule
/// a caller has to satisfy is WHERE the value came from: a token that same
/// listing handed back. The member and the rejected token are the request's, so
/// the clause is built from it like any other.
pub fn unknown_page_token_refusal(member: &str, operation: &str, submitted: &str) -> String {
    stated_requirement(
        &member_path(member),
        Some(submitted),
        &format!("Member must be a {member} returned by a previous {operation} call"),
    )
}

/// What the service states when an `aggregationPeriod` is outside the documented
/// allow-list: the list itself, rendered from [`AGGREGATION_PERIODS`] so the
/// sentence cannot drift from the values the handlers actually admit.
///
/// One sentence for the whole rule, shared by every operation that applies it
/// (`CreateMonitor`, `UpdateMonitor`), rather than one per handler. The list is
/// joined explicitly rather than `Debug`-formatted, so the sentence's shape is
/// this function's and not a formatting detail of the constant's type.
pub fn aggregation_period_message() -> String {
    let values: Vec<String> = AGGREGATION_PERIODS.iter().map(i64::to_string).collect();
    format!("The aggregation period must be one of [{}].", values.join(", "))
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
///
/// `probe_path` is where this probe sits in the REQUEST, for the clause a
/// refusal states; see [`validate_probe_spec`].
pub fn build_probe(
    spec: ProbeSpec,
    probe_id: String,
    region: &str,
    now: SystemTime,
    probe_path: &str,
) -> Result<Probe, String> {
    let address_family = validate_probe_spec(&spec, probe_path)?;
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
        deletes_at: None,
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
///
/// `probe_path` is the dotted path of the probe WITHIN the request this spec was
/// built from, which is what a refusal's clause names the member by: `probe` for
/// `CreateProbe`, `probes.<position>.member` for one entry of `CreateMonitor`'s
/// list ([`element_path`]), and the EMPTY path for `UpdateProbe`, which binds a
/// probe's members at the top level of its own input. Only the operation holding
/// the request knows where its probe sat, so it supplies it.
pub fn validate_probe_spec(spec: &ProbeSpec, probe_path: &str) -> Result<&'static str, String> {
    if subnet_id_of(&spec.source_arn).is_none() {
        return Err(format!(
            "The specified subnet {} does not exist or could not be found.",
            arn_resource_id(&spec.source_arn)
        ));
    }
    let Some(address_family) = address_family_of(&spec.destination) else {
        return Err(stated_requirement(
            &probe_member_path(probe_path, "destination"),
            Some(&spec.destination),
            DESTINATION_REQUIREMENT,
        ));
    };
    if spec.protocol == PROTOCOL_TCP && spec.destination_port.is_none() {
        return Err(TCP_PORT_REQUIRED_MESSAGE.to_string());
    }
    if let Some(port) = spec.destination_port {
        if port < MIN_DESTINATION_PORT {
            // The documented floor, stated in the framework's own wording for a
            // numeric bound: the model's `@range(min: 0)` admits `0`, so this
            // bound is the service's but the clause is the one a `@range`
            // refusal already reads like.
            return Err(stated_violation(
                &probe_member_path(probe_path, "destinationPort"),
                Some(&port.to_string()),
                ViolationKind::MinValue(MIN_DESTINATION_PORT.to_string()),
            ));
        }
    }
    Ok(address_family)
}

/// Check the per-subnet probe limit for a request that would ADD probes to a
/// monitor ([`MAX_PROBES_PER_SUBNET`]).
///
/// The limit is counted per (monitor, source subnet) pair, so the probes
/// `existing` already holds are grouped by their `sourceArn` and each requested
/// `sourceArn` is counted on top of that group, in request order: one call that
/// adds several probes to one subnet is bounded exactly as several calls adding
/// one each are. `existing` is `None` for a monitor the request is creating,
/// which holds no probe yet.
///
/// Shared by every operation that ADDS a probe to a monitor (`CreateProbe`, and
/// `CreateMonitor`'s inline `probes` list), so the limit cannot be enforced on
/// one path and not the other. `Err` carries
/// [`PROBES_PER_SUBNET_MESSAGE`].
pub fn check_probes_per_subnet<'a>(
    existing: Option<&'a Monitor>,
    requested: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut per_subnet: BTreeMap<&str, usize> = BTreeMap::new();
    if let Some(monitor) = existing {
        // Every probe the monitor holds occupies a slot in its own subnet,
        // including one whose teardown is still draining: such a probe is still
        // attached, and the store drops it once the teardown has run its course
        // ([`Probe::is_forgotten_at`]), which frees the slot.
        for probe in monitor.probes.values() {
            *per_subnet.entry(probe.source_arn.as_str()).or_default() += 1;
        }
    }
    for source_arn in requested {
        let count = per_subnet.entry(source_arn).or_default();
        *count += 1;
        if *count > MAX_PROBES_PER_SUBNET {
            return Err(PROBES_PER_SUBNET_MESSAGE.to_string());
        }
    }
    Ok(())
}

/// The dotted path of one member of the probe sitting at `probe_path`, which is
/// empty for an operation that binds a probe's members at the top level of its
/// own input (`UpdateProbe`).
fn probe_member_path(probe_path: &str, member: &str) -> String {
    if probe_path.is_empty() {
        return member_path(member);
    }
    path::join(&[probe_path.to_string(), path::member(member)])
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
    /// When the clock owes this probe its `DELETING` -> `DELETED` transition,
    /// set by the accepted `DeleteProbe` that started its teardown; `None` for a
    /// probe nothing has asked to delete. A probe that carries one is being torn
    /// down ([`Probe::is_being_torn_down`]) and leaves the store one
    /// [`PROBE_TEARDOWN_STAGE`] after that instant
    /// ([`Probe::is_forgotten_at`]).
    pub deletes_at: Option<SystemTime>,
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

    /// Accept this probe's deletion: tearing a probe down is asynchronous, so
    /// the record STAYS, moves into the teardown half of its lifecycle
    /// ([`PROBE_STATE_DELETING`]) and the clock owes it the rest
    /// ([`PROBE_TEARDOWN_STAGE`]).
    ///
    /// Idempotent, as the modelled operation that calls it is: a probe already
    /// being torn down keeps the instants the first accepted delete set, so a
    /// repeat neither postpones the teardown nor restarts it.
    pub fn start_teardown(&mut self, now: SystemTime) {
        if self.is_being_torn_down() {
            return;
        }
        self.state = PROBE_STATE_DELETING.to_string();
        self.deletes_at = Some(now.checked_add(PROBE_TEARDOWN_STAGE).unwrap_or(now));
    }

    /// Whether this probe's teardown has been accepted. Such a probe is still
    /// reported, in its teardown state, but it is on its way out: its teardown
    /// is irreversible, so no later call puts it back into service.
    pub fn is_being_torn_down(&self) -> bool {
        self.deletes_at.is_some()
    }

    /// Whether this probe's teardown finished long enough ago that the record is
    /// no longer readable at `now` — from then on a read of it is not-found.
    pub fn is_forgotten_at(&self, now: SystemTime) -> bool {
        self.deletes_at.is_some_and(|deletes_at| {
            deletes_at
                .checked_add(PROBE_TEARDOWN_STAGE)
                .is_some_and(|forgotten_at| now >= forgotten_at)
        })
    }

    /// Materialise the lifecycle transition the clock has already made due for
    /// THIS probe: `PENDING` -> `ACTIVE` once its activation instant has passed,
    /// and `DELETING` -> `DELETED` once its teardown instant has. A probe being
    /// torn down owes nothing else: the teardown outranks the activation, so a
    /// probe deleted while still provisioning never becomes `ACTIVE`.
    fn settle(&mut self, now: SystemTime) {
        if let Some(deletes_at) = self.deletes_at {
            if now >= deletes_at {
                self.state = PROBE_STATE_DELETED.to_string();
            }
            return;
        }
        if self.state == PROBE_STATE_PENDING && now >= self.activates_at {
            self.state = PROBE_STATE_ACTIVE.to_string();
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
    /// A monitor, by the region that holds it and its `monitorName`.
    Monitor {
        region: String,
        monitor_name: String,
    },
    /// A probe, by the region and monitor that hold it and its own `probeId`.
    Probe {
        region: String,
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
///
/// A monitor is a REGIONAL resource: the service has one endpoint per region
/// (`https://networkmonitor.<region>.<dnsSuffix>`) and every ARN it mints names
/// that region, so one `monitorName` identifies a different resource in each
/// region, and a request reaches only the records of the region its SigV4
/// credential scope names ([`request_region`]). The store is therefore keyed by
/// region first, and each region's own map by `monitorName`.
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// Region -> (`monitorName` -> record), each record holding its own probes.
    /// Bounded by [`MAX_MONITORS`] across every region; a region whose last
    /// monitor is deleted keeps no empty map behind
    /// ([`Self::remove_monitor`]).
    monitors: BTreeMap<String, BTreeMap<String, Monitor>>,
    /// (region, `clientToken`) -> what it replays. A token is scoped to the
    /// region it was sent to, like the monitor it minted. Bounded by
    /// [`MAX_CLIENT_TOKENS`] and [`CLIENT_TOKEN_TTL`].
    ///
    /// The ONLY record of which token minted which resource: a monitor and a
    /// probe keep no token of their own, so there is nothing for this map to
    /// disagree with once a record is forgotten (TTL, ceiling, or the delete of
    /// the monitor it named).
    client_tokens: BTreeMap<(String, String), ClientTokenRecord>,
    /// Monotonic counter behind [`Self::next_probe_id`]; never reused within a
    /// run.
    probe_seq: u64,
}

impl ComAmazonawsNetworkmonitorNetworkmonitorState {
    /// The monitors of `region`, in `monitorName` order; `None` when that region
    /// holds none.
    pub fn monitors_in(&self, region: &str) -> Option<&BTreeMap<String, Monitor>> {
        self.monitors.get(region)
    }

    /// One region's monitor, for reporting.
    pub fn monitor(&self, region: &str, monitor_name: &str) -> Option<&Monitor> {
        self.monitors.get(region)?.get(monitor_name)
    }

    /// One region's monitor, for mutation. Does NOT create the region's map, so
    /// a request naming a monitor nothing stored leaves the store as it found
    /// it.
    pub fn monitor_mut(&mut self, region: &str, monitor_name: &str) -> Option<&mut Monitor> {
        self.monitors.get_mut(region)?.get_mut(monitor_name)
    }

    /// Store a monitor in `region`, opening that region's map if this is the
    /// first record kept there. For a create path only, AFTER the request is
    /// known to be acceptable.
    pub fn insert_monitor(&mut self, region: &str, monitor: Monitor) {
        self.monitors
            .entry(region.to_string())
            .or_default()
            .insert(monitor.monitor_name.clone(), monitor);
    }

    /// Remove one region's monitor, with the probes it holds. A region left
    /// holding no monitor is dropped with it, so an empty partition never
    /// lingers.
    pub fn remove_monitor(&mut self, region: &str, monitor_name: &str) -> Option<Monitor> {
        let region_monitors = self.monitors.get_mut(region)?;
        let removed = region_monitors.remove(monitor_name);
        if region_monitors.is_empty() {
            self.monitors.remove(region);
        }
        removed
    }

    /// How many monitors this store holds across every region — what
    /// [`MAX_MONITORS`] bounds, since the ceiling exists to bound the store's
    /// memory.
    pub fn monitor_count(&self) -> usize {
        self.monitors.values().map(BTreeMap::len).sum()
    }

    /// What a `clientToken` sent to `region` replays, when that region holds a
    /// record for it.
    ///
    /// Private: which token minted which resource is recorded in ONE place, and
    /// [`Self::monitor_minted_by_token`] / [`Self::probe_minted_by_token`] are
    /// the whole of what a create asks of it, so no caller outside this module
    /// re-derives the retry decision from the raw record.
    fn client_token_record(&self, region: &str, token: &str) -> Option<&ClientTokenRecord> {
        self.client_tokens
            .get(&(region.to_string(), token.to_string()))
    }

    /// The monitor a `clientToken` sent to `region` already minted under
    /// `monitor_name`, when that token makes the repeat a RETRY of the create
    /// that minted it: this region holds a record for the token, that record was
    /// made by a monitor create (it names no probe) and it names this monitor,
    /// and the monitor it names is still stored.
    ///
    /// `None` for every other token — one this region never saw, one the store
    /// has since forgotten, one that minted a probe, one that minted a
    /// differently named monitor, or one whose monitor is gone — so the repeat
    /// is judged on its own, as a request carrying no token is.
    pub fn monitor_minted_by_token(
        &self,
        region: &str,
        token: &str,
        monitor_name: &str,
    ) -> Option<&Monitor> {
        let record = self.client_token_record(region, token)?;
        if record.probe_id.is_some() || record.monitor_name != monitor_name {
            return None;
        }
        self.monitor(region, monitor_name)
    }

    /// The probe a `clientToken` sent to `region` already minted on
    /// `monitor_name`, under the same rule as
    /// [`Self::monitor_minted_by_token`]: the record has to name a probe of this
    /// monitor, and that probe has to be stored still.
    pub fn probe_minted_by_token(
        &self,
        region: &str,
        token: &str,
        monitor_name: &str,
    ) -> Option<&Probe> {
        let record = self.client_token_record(region, token)?;
        if record.monitor_name != monitor_name {
            return None;
        }
        let probe_id = record.probe_id.as_deref()?;
        self.monitor(region, monitor_name)?.probes.get(probe_id)
    }

    /// Record what a `clientToken` sent to `region` replays.
    pub fn record_client_token(&mut self, region: &str, token: String, record: ClientTokenRecord) {
        self.client_tokens
            .insert((region.to_string(), token), record);
    }

    /// Forget every `clientToken` of `region` that named this monitor: once the
    /// monitor is gone those records replay nothing.
    pub fn forget_client_tokens_of_monitor(&mut self, region: &str, monitor_name: &str) {
        self.client_tokens.retain(|(token_region, _), record| {
            token_region != region || record.monitor_name != monitor_name
        });
    }

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

    /// Materialise the transitions the clock has already made due, for ONE
    /// monitor and its probes.
    ///
    /// These are the ONLY writes a reporting operation performs: a monitor whose
    /// `activates_at` has passed was owed its activation at that instant whether
    /// or not anybody called, and a probe whose teardown instants have passed
    /// was owed its `DELETED` state, and then its removal, at those instants, so
    /// a read settles them before reporting. Nothing else about the monitor
    /// changes, and `modified_at` is NOT touched: the caller modified nothing.
    ///
    /// The order is removal first, then the per-probe transition: a probe whose
    /// teardown finished long enough ago is not reported at all, and one still
    /// within its teardown is reported in the state that teardown has reached.
    pub fn settle_monitor(&mut self, region: &str, monitor_name: &str, now: SystemTime) {
        let Some(monitor) = self.monitor_mut(region, monitor_name) else {
            return;
        };
        monitor.probes.retain(|_, probe| !probe.is_forgotten_at(now));
        for probe in monitor.probes.values_mut() {
            probe.settle(now);
        }
        // A monitor left holding no probe has nothing to aggregate, so the
        // teardown of its last probe takes its activation away with it — the
        // same state a monitor created without probes is in.
        if monitor.probes.is_empty() {
            if monitor.state != MONITOR_STATE_INACTIVE || monitor.activates_at.is_some() {
                monitor.clear_activation();
            }
            return;
        }
        if monitor.state == MONITOR_STATE_PENDING
            && monitor
                .activates_at
                .is_some_and(|activates_at| now >= activates_at)
        {
            monitor.state = MONITOR_STATE_ACTIVE.to_string();
        }
    }

    /// The same, for every monitor of ONE region: what `ListMonitors` reports is
    /// the state each monitor is in at the instant of the call, so that region's
    /// whole set is settled before the page is built. Bounded by
    /// [`MAX_MONITORS`].
    pub fn settle_monitors_in(&mut self, region: &str, now: SystemTime) {
        let names: Vec<String> = self
            .monitors_in(region)
            .map(|monitors| monitors.keys().cloned().collect())
            .unwrap_or_default();
        for name in names {
            self.settle_monitor(region, &name, now);
        }
    }

    /// Which stored resource carries `resource_arn`, or `None` when nothing this
    /// service holds does.
    ///
    /// The ARN is compared against the one STORED on each record — the ARN the
    /// service minted and reports — so a tagging call reaches exactly the
    /// resource a read of that ARN reports, and an ARN for an account this store
    /// never minted resolves to nothing. Only `region`'s own records are
    /// considered, so an ARN naming another region's resource resolves to
    /// nothing here, just as that resource is unreachable through this endpoint.
    pub fn resolve_tagged_resource(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<TaggedResource> {
        for monitor in self.monitors_in(region)?.values() {
            if monitor.monitor_arn == resource_arn {
                return Some(TaggedResource::Monitor {
                    region: region.to_string(),
                    monitor_name: monitor.monitor_name.clone(),
                });
            }
            for probe in monitor.probes.values() {
                if probe.probe_arn == resource_arn {
                    return Some(TaggedResource::Probe {
                        region: region.to_string(),
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
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => self
                .monitor(region, monitor_name)
                .map(|monitor| &monitor.tags),
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => self
                .monitor(region, monitor_name)
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
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => self
                .monitor_mut(region, monitor_name)
                .map(|monitor| &mut monitor.tags),
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => self
                .monitor_mut(region, monitor_name)
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
            TaggedResource::Monitor {
                region,
                monitor_name,
            } => {
                if let Some(monitor) = self.monitor_mut(region, monitor_name) {
                    monitor.modified_at = now;
                }
            }
            TaggedResource::Probe {
                region,
                monitor_name,
                probe_id,
            } => {
                if let Some(probe) = self
                    .monitor_mut(region, monitor_name)
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
                .map(|(key, _)| key.clone());
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
