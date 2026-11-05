// SPDX-License-Identifier: CC0-1.0

//! What `CreateNetworkAcl` and the operations that write a network ACL's entries need beyond
//! the network ACL store itself.
//!
//! The store, the id form, the byte charge, the default ACL every VPC comes with and the
//! subnet associations live in the "Network ACLs" section of [`crate::state`], beside the
//! other stores; this module holds only what a CALLER-created ACL adds to them: the
//! `ResourceType` its tag specification may name, the entries it is created with, the quota
//! refusal for one ACL too many, and the `ClientToken` map that makes the create idempotent.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use super::{
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, CidrProblem, ComAmazonawsEc2Amazonec2State,
    DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, MAX_ICMP_TYPE, MAX_PORT, MAX_PROTOCOL_NUMBER,
    RULE_ACTION_DENY, canonical_ipv4_cidr, canonical_ipv6_cidr, invalid_parameter_combination,
    invalid_parameter_value, missing_parameter,
};
use crate::ops::types::{IcmpTypeCode, NetworkAclEntry, PortRange};
use crate::scaffolded::Scaffolded;

/// The one `ResourceType` a `CreateNetworkAcl` tag specification may name: "The tags to
/// assign to the network ACL" (`CreateNetworkAclRequest$TagSpecifications`), spelled as the
/// modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const NETWORK_ACL_TAGGABLE_RESOURCE: &str = "network-acl";

/// EC2's quota code for one network ACL too many, named the way every other per-resource EC2
/// quota code is (EC2 API "Error codes": `NetworkAclLimitExceeded`).
pub const NETWORK_ACL_LIMIT_EXCEEDED: &str = "NetworkAclLimitExceeded";

/// EC2's quota code for one entry too many in a network ACL (EC2 API "Error codes":
/// `NetworkAclEntryLimitExceeded`).
pub const NETWORK_ACL_ENTRY_LIMIT_EXCEEDED: &str = "NetworkAclEntryLimitExceeded";

/// EC2's code for an entry a network ACL already holds at that rule number in that
/// direction (EC2 API "Error codes": `NetworkAclEntryAlreadyExists`) — "After you add an
/// entry, you can't modify it; you must either replace it, or create an entry and delete the
/// old one" (`CreateNetworkAclEntry`), so a create never overwrites one.
pub const NETWORK_ACL_ENTRY_ALREADY_EXISTS: &str = "NetworkAclEntryAlreadyExists";

/// EC2's code for an entry the network ACL does not hold (EC2 API "Error codes":
/// `InvalidNetworkAclEntry.NotFound`).
pub const INVALID_NETWORK_ACL_ENTRY_NOT_FOUND: &str = "InvalidNetworkAclEntry.NotFound";

/// The rule numbers a CALLER-created network ACL entry may take: "Constraints: Positive
/// integer from 1 to 32766. The range 32767 to 65535 is reserved for internal use"
/// (`CreateNetworkAclEntryRequest$RuleNumber`), which is why the catch-all deny every ACL
/// ends with sits at [`DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER`], one past this range.
pub const NETWORK_ACL_ENTRY_RULE_NUMBERS: ::std::ops::RangeInclusive<i32> = 1..=32766;

/// How many entries one network ACL holds before a create must refuse.
///
/// "Rules per network ACL" is 20 by default and adjustable up to a maximum of 40 (Amazon VPC
/// quotas), so 40 is the most any account's ACL can hold. Counted over the entries inside
/// [`NETWORK_ACL_ENTRY_RULE_NUMBERS`] — the catch-all deny every ACL ends with sits one past
/// that range and is not a rule a caller wrote, so it is not counted; a default ACL's own
/// allow-everything rule is at [`super::DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER`], inside the
/// range, and counts like any other rule of that ACL.
pub const MAX_NETWORK_ACL_ENTRIES: usize = 40;

/// The protocol number of TCP, one of the two protocols a network ACL entry must carry a
/// port range for: "Required if specifying protocol 6 (TCP) or 17 (UDP)"
/// (`CreateNetworkAclEntryRequest$PortRange`).
pub const NETWORK_ACL_PROTOCOL_TCP: i32 = 6;

/// The protocol number of UDP, the other one (same member).
pub const NETWORK_ACL_PROTOCOL_UDP: i32 = 17;

/// The protocol number of ICMP, one of the two protocols a network ACL entry must carry an
/// ICMP type and code for: "Required if specifying protocol 1 (ICMP) or protocol 58 (ICMPv6)
/// with an IPv6 CIDR block" (`CreateNetworkAclEntryRequest$IcmpTypeCode`).
pub const NETWORK_ACL_PROTOCOL_ICMP: i32 = 1;

/// The ICMP type and code numbers a network ACL entry may carry: one byte each, since ICMP
/// carries both the type and the code in one byte of its header (RFC 792) — the bound
/// [`MAX_ICMP_TYPE`] already states for this crate — and `-1` beside them, which is the only
/// other value either member documents: "The ICMP type. A value of -1 means all types"
/// (`IcmpTypeCode$Type`) and "The ICMP code. A value of -1 means all codes for the specified
/// ICMP type" (`$Code`,
/// `smithy_query(query="shape",shape="IcmpTypeCode")`).
pub const NETWORK_ACL_ICMP_NUMBERS: ::std::ops::RangeInclusive<i32> = -1..=MAX_ICMP_TYPE;

/// The protocol number of ICMPv6, which needs a type and code only "with an IPv6 CIDR
/// block": "If you specify protocol 58 (ICMPv6) and specify an IPv4 CIDR block, traffic for
/// all ICMP types and codes allowed" (`CreateNetworkAclEntryRequest$Protocol`).
pub const NETWORK_ACL_PROTOCOL_ICMPV6: i32 = 58;

/// The caller-chosen bytes ONE stored network ACL entry holds: the text of its CIDR blocks,
/// its protocol and its action. The rule number, the direction and the port or ICMP numbers
/// are fixed-size.
///
/// The ONE definition of that charge: [`super::network_acl_payload_bytes`] sums it over an
/// ACL's entries for a create, [`ComAmazonawsEc2Amazonec2State::insert_network_acl_entry`]
/// charges exactly it for an entry added to a stored ACL, and
/// [`ComAmazonawsEc2Amazonec2State::remove_network_acl_entry`] gives back exactly it.
pub fn network_acl_entry_payload_bytes(entry: &NetworkAclEntry) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    text(entry.cidr_block.as_deref())
        .saturating_add(text(entry.ipv6_cidr_block.as_deref()))
        .saturating_add(text(entry.protocol.as_deref()))
        .saturating_add(text(entry.rule_action.as_deref()))
}

/// Where one entry sits in the order Amazon EC2 reports a network ACL's entries in: the
/// egress rules first, then the ingress rules, each by ascending rule number — the order an
/// ACL's `entrySet` comes back in, so an entry added to a stored ACL takes its place among
/// the ones already there rather than being appended after them.
///
/// The ONE definition of that order: every writer sorts the entries it stores by it
/// ([`created_network_acl_entries`], [`super::default_network_acl_entries`],
/// [`ComAmazonawsEc2Amazonec2State::insert_network_acl_entry`]), so a reader reports an ACL
/// exactly as it is stored.
pub fn network_acl_entry_report_order(entry: &NetworkAclEntry) -> (bool, i32) {
    (
        // `false` sorts first, so the egress rules lead.
        !entry.egress.unwrap_or(false),
        entry.rule_number.unwrap_or_default(),
    )
}

/// Refuse an entry the ACL already holds at that rule number and direction: HTTP 400
/// [`NETWORK_ACL_ENTRY_ALREADY_EXISTS`], naming the rule number the request sent.
pub fn network_acl_entry_already_exists<E>(rule_number: i32) -> Scaffolded<E> {
    Scaffolded::service_error(
        NETWORK_ACL_ENTRY_ALREADY_EXISTS,
        400,
        format!("The network acl entry identified by {rule_number} already exists."),
    )
}

/// The word AWS names a network ACL rule's direction with: an `egress` rule or an `ingress`
/// rule — "Each network ACL has a set of numbered ingress rules and a separate set of
/// numbered egress rules" (`CreateNetworkAclEntry`), and a refusal about one entry names
/// which of the two sets it looked in.
pub fn network_acl_entry_direction(egress: bool) -> &'static str {
    if egress { "egress" } else { "ingress" }
}

/// Refuse a request naming an entry the ACL does not hold: HTTP 400
/// [`INVALID_NETWORK_ACL_ENTRY_NOT_FOUND`].
///
/// The sentence is the one the wire corpus records this condition being answered with — "no
/// `<direction>` entry with number `<rule number>` in network ACL `<id>`" — and it names the
/// facts that say WHICH entry was looked for and did not answer: there is one such entry per
/// rule number per direction per ACL, so none of the three can be left out of a message a
/// caller recovers from. Each is taken from the REQUEST.
pub fn invalid_network_acl_entry_not_found<E>(
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_ACL_ENTRY_NOT_FOUND,
        400,
        format!(
            "no {direction} entry with number {rule_number} in network ACL {network_acl_id}",
            direction = network_acl_entry_direction(egress),
        ),
    )
}

/// Refuse a create the ACL has no room for: HTTP 400 [`NETWORK_ACL_ENTRY_LIMIT_EXCEEDED`].
pub fn network_acl_entry_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(NETWORK_ACL_ENTRY_LIMIT_EXCEEDED, 400, message)
}

/// `entry` as a reader reports it.
///
/// A reported entry carries BOTH "the ICMP type and code" (`NetworkAclEntry$IcmpTypeCode`)
/// and "the range of ports the rule applies to" (`$PortRange`): a rule whose protocol has no
/// ICMP numbers or no ports carries them EMPTY rather than not at all, which is how an ACL's
/// `entrySet` comes back on the wire.
///
/// The ONE place that is applied, so every writer of a stored entry
/// ([`created_network_acl_entries`], [`super::default_network_acl_entries`],
/// [`ComAmazonawsEc2Amazonec2State::insert_network_acl_entry`]) stores the same shape and no
/// two readers report one entry differently.
pub fn reported_network_acl_entry(mut entry: NetworkAclEntry) -> NetworkAclEntry {
    entry.icmp_type_code.get_or_insert_with(IcmpTypeCode::default);
    entry.port_range.get_or_insert_with(PortRange::default);
    entry
}

/// The entries a network ACL a CALLER created comes with: the catch-all deny in each
/// direction, and nothing else.
///
/// A custom network ACL "denies all inbound and outbound traffic until you add rules", and
/// every ACL carries the rule "whose rule number is an asterisk" that denies what no other
/// rule matched (Amazon VPC User Guide, "Control subnet traffic with network access control
/// lists"). So this is [`super::default_network_acl_entries`] without the allow-everything
/// pair a VPC's own default ACL carries, at the same
/// [`DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER`] and over the same every-protocol,
/// every-IPv4-address match.
///
/// The ONE definition of those entries, so a created ACL cannot be written two ways.
pub fn created_network_acl_entries() -> Vec<NetworkAclEntry> {
    let mut entries: Vec<NetworkAclEntry> = [false, true]
        .into_iter()
        .map(|egress| {
            reported_network_acl_entry(NetworkAclEntry {
                cidr_block: Some(ALL_IPV4_ADDRESSES.to_string()),
                egress: Some(egress),
                // ICMP type and code describe an entry whose protocol is ICMP, and a port
                // range one whose protocol is TCP or UDP; this entry's protocol is every
                // protocol, so it carries neither of them filled in.
                icmp_type_code: None,
                ipv6_cidr_block: None,
                port_range: None,
                protocol: Some(ALL_IP_PROTOCOLS.to_string()),
                rule_action: Some(RULE_ACTION_DENY.to_string()),
                rule_number: Some(DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER),
            })
        })
        .collect();
    // Stored in the order an ACL's entries are reported in, through the one definition of
    // that order.
    entries.sort_by_key(network_acl_entry_report_order);
    entries
}

/// The members of a request that writes ONE entry (rule) onto a network ACL, as both
/// operations that write one send them: `CreateNetworkAclEntry` and `ReplaceNetworkAclEntry`
/// carry the same rule members, and "you must either replace it, or create an entry and
/// delete the old one" (`CreateNetworkAclEntry`), so a rule that may be created is exactly a
/// rule that may replace one.
#[derive(Debug, Clone)]
pub struct RequestedNetworkAclEntry<'a> {
    /// `$RuleNumber`: which of the ACL's numbered rules, within the direction below.
    pub rule_number: i32,
    /// `$Egress`: which of the ACL's two sets of numbered rules the entry belongs to.
    pub egress: bool,
    /// `$Protocol`: the protocol number the rule matches, or `-1` for all protocols.
    pub protocol: &'a str,
    /// `$RuleAction`: `allow` or `deny`, already held to the modelled enum by the boundary.
    pub rule_action: &'a str,
    /// `$CidrBlock`: the IPv4 range the rule matches, as the request spelled it.
    pub cidr_block: Option<&'a str>,
    /// `$Ipv6CidrBlock`: the IPv6 range the rule matches, as the request spelled it.
    pub ipv6_cidr_block: Option<&'a str>,
    /// `$IcmpTypeCode`: the ICMP or ICMPv6 type and code, for the protocols that need one.
    pub icmp_type_code: Option<IcmpTypeCode>,
    /// `$PortRange`: the range of ports, for the protocols that have ports.
    pub port_range: Option<PortRange>,
}

/// `requested` as a STORED network ACL entry, or the refusal the request earns.
///
/// The ONE definition of what a rule a caller may write looks like, so the two operations
/// that write one — `CreateNetworkAclEntry` and `ReplaceNetworkAclEntry` — cannot hold the
/// same request to different rules or refuse it for different faults. Every member is read
/// in a FIXED order, before the store is asked anything, so a request with two faults is
/// refused for the same one whichever operation it was sent to.
pub fn validated_network_acl_entry<E>(
    requested: &RequestedNetworkAclEntry<'_>,
) -> Result<NetworkAclEntry, Scaffolded<E>> {
    // "The rule number for the entry… Constraints: Positive integer from 1 to 32766. The
    // range 32767 to 65535 is reserved for internal use"
    // (`CreateNetworkAclEntryRequest$RuleNumber`). The model puts no `@range` on the member,
    // so this is the only place that bound can be enforced, and it is enforced before the
    // ACL is looked up.
    if !NETWORK_ACL_ENTRY_RULE_NUMBERS.contains(&requested.rule_number) {
        return Err(invalid_parameter_value(format!(
            "Value ({number}) for parameter ruleNumber is invalid. Rulenumber must be \
             in range {first}..{last}",
            number = requested.rule_number,
            first = NETWORK_ACL_ENTRY_RULE_NUMBERS.start(),
            last = NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
        )));
    }

    // "The protocol number. A value of `-1` means all protocols" (`$Protocol`) — a NUMBER,
    // not one of the protocol names a security group rule may carry, so a value outside the
    // single byte a protocol number occupies is refused rather than stored.
    let protocol = requested.protocol.trim();
    let protocol_number = if protocol == ALL_IP_PROTOCOLS {
        None
    } else {
        match protocol.parse::<i32>() {
            Ok(number) if (0..=MAX_PROTOCOL_NUMBER).contains(&number) => Some(number),
            _ => {
                return Err(invalid_parameter_value(format!(
                    "Value ({protocol}) for parameter protocol is invalid. The protocol \
                     number must be between 0 and {max}, or {all} for all protocols",
                    max = MAX_PROTOCOL_NUMBER,
                    all = ALL_IP_PROTOCOLS,
                )))
            }
        }
    };

    // "The IPv4 network range to allow or deny, in CIDR notation" (`$CidrBlock`) and "The
    // IPv6 network range" (`$Ipv6CidrBlock`): a rule matches a range of ONE family, so
    // exactly one of them says which traffic the rule applies to. "We modify the specified
    // CIDR block to its canonical form; for example, if you specify 100.68.0.18/18, we
    // modify it to 100.68.0.0/18" (`$CidrBlock`), so the stored range is the canonical one,
    // whichever of a range's equivalent spellings the request used.
    let requested_ipv4 = requested.cidr_block.filter(|value| !value.is_empty());
    let requested_ipv6 = requested.ipv6_cidr_block.filter(|value| !value.is_empty());
    let (cidr_block, ipv6_cidr_block) = match (requested_ipv4, requested_ipv6) {
        (None, None) => {
            return Err(missing_parameter(
                "The request must contain the parameter cidrBlock or ipv6CidrBlock",
            ))
        }
        (Some(_), Some(_)) => {
            return Err(invalid_parameter_combination(
                "Only one of cidrBlock and ipv6CidrBlock may be specified: a network \
                 ACL entry matches a range of one address family",
            ))
        }
        (Some(value), None) => (
            Some(
                canonical_ipv4_cidr(value).map_err(|_: CidrProblem| {
                    invalid_parameter_value(format!(
                        "Value ({value}) for parameter cidrBlock is invalid. This is \
                         not a valid CIDR block."
                    ))
                })?,
            ),
            None,
        ),
        (None, Some(value)) => (
            None,
            Some(
                canonical_ipv6_cidr(value).map_err(|_: CidrProblem| {
                    invalid_parameter_value(format!(
                        "Value ({value}) for parameter ipv6CidrBlock is invalid. \
                         This is not a valid CIDR block."
                    ))
                })?,
            ),
        ),
    };

    // "TCP or UDP protocols: The range of ports the rule applies to. Required if specifying
    // protocol 6 (TCP) or 17 (UDP)" (`$PortRange`). A range that IS given has to be a range
    // of ports, whatever the protocol: "The first port in the range" (`PortRange$From`) and
    // the last, in order.
    let port_range = requested
        .port_range
        .clone()
        .filter(|range| range.from.is_some() || range.to.is_some());
    let needs_port_range = matches!(
        protocol_number,
        Some(NETWORK_ACL_PROTOCOL_TCP) | Some(NETWORK_ACL_PROTOCOL_UDP)
    );
    if needs_port_range && port_range.is_none() {
        return Err(missing_parameter(format!(
            "The request must contain the parameter portRange for protocol {protocol}"
        )));
    }
    if let Some(range) = port_range.as_ref() {
        // A range is both of its ports: a request that sends one of them has not named a
        // range at all, which is the member that is missing rather than a range whose ports
        // are in the wrong order.
        let (Some(from), Some(to)) = (range.from, range.to) else {
            return Err(missing_parameter(format!(
                "The request must contain the parameter portRange.{half}",
                half = if range.from.is_none() { "from" } else { "to" },
            )));
        };
        if !(0..=MAX_PORT).contains(&from) || !(0..=MAX_PORT).contains(&to) {
            return Err(invalid_parameter_value(format!(
                "Value ({from}, {to}) for parameter portRange is invalid. A port is \
                 between 0 and {max}",
                max = MAX_PORT,
            )));
        }
        if from > to {
            return Err(invalid_parameter_value(format!(
                "Value ({from}, {to}) for parameter portRange is invalid. The first \
                 port of the range must not be above the last"
            )));
        }
    }

    // "ICMP protocol: The ICMP or ICMPv6 type and code. Required if specifying protocol 1
    // (ICMP) or protocol 58 (ICMPv6) with an IPv6 CIDR block" (`$IcmpTypeCode`) — ICMPv6
    // over an IPv4 range needs none, because then "traffic for all ICMP types and codes
    // allowed, regardless of any that you specify" (`$Protocol`).
    let needs_icmp_type_code = protocol_number == Some(NETWORK_ACL_PROTOCOL_ICMP)
        || (protocol_number == Some(NETWORK_ACL_PROTOCOL_ICMPV6) && ipv6_cidr_block.is_some());
    if needs_icmp_type_code && requested.icmp_type_code.is_none() {
        return Err(missing_parameter(format!(
            "The request must contain the parameter icmpTypeCode for protocol {protocol}"
        )));
    }
    // A type and code that ARE given have to be an ICMP type and code, whatever the
    // protocol, the way a port range that is given has to be a range of ports: each is one
    // byte of the ICMP header, or the `-1` that stands for all of them
    // ([`NETWORK_ACL_ICMP_NUMBERS`]). Read here rather than in either handler, so the two
    // writers of a rule hold the member to one bound.
    if let Some(icmp_type_code) = requested.icmp_type_code.as_ref() {
        for (member, value) in [
            ("type", icmp_type_code.r#type),
            ("code", icmp_type_code.code),
        ] {
            if let Some(value) = value {
                if !NETWORK_ACL_ICMP_NUMBERS.contains(&value) {
                    return Err(invalid_parameter_value(format!(
                        "Value ({value}) for parameter icmpTypeCode.{member} is invalid. An \
                         ICMP {member} is between 0 and {max}, or {all} for all of them",
                        max = MAX_ICMP_TYPE,
                        all = NETWORK_ACL_ICMP_NUMBERS.start(),
                    )));
                }
            }
        }
    }

    // The entry the request describes, built out of the validated members so nothing the
    // request sent is stored unchecked. "Indicates whether this is an egress rule"
    // (`$Egress`) is what separates the ACL's two sets of numbered rules, and `RuleAction`
    // targets a modelled enum, so the boundary validator has already refused any word but
    // `allow` and `deny`.
    Ok(NetworkAclEntry {
        cidr_block,
        egress: Some(requested.egress),
        icmp_type_code: requested.icmp_type_code.clone(),
        ipv6_cidr_block,
        port_range,
        protocol: Some(
            // The number's own spelling, so a request that wrote `06` and one that wrote `6`
            // store and report the same rule, the way a CIDR block is stored canonicalised.
            protocol_number
                .map_or_else(|| ALL_IP_PROTOCOLS.to_string(), |number| number.to_string()),
        ),
        rule_action: Some(requested.rule_action.to_string()),
        rule_number: Some(requested.rule_number),
    })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the network ACL `network_acl_id` already holds an entry at `rule_number` in
    /// the direction `egress` names.
    ///
    /// "Each network ACL has a set of numbered ingress rules and a separate set of numbered
    /// egress rules" (`CreateNetworkAclEntry`), so one rule number identifies one entry
    /// WITHIN a direction and the same number may be used in both.
    pub fn network_acl_holds_entry(
        &self,
        network_acl_id: &str,
        rule_number: i32,
        egress: bool,
    ) -> bool {
        self.network_acls
            .get(network_acl_id)
            .and_then(|network_acl| network_acl.entries.as_deref())
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.rule_number == Some(rule_number)
                        && entry.egress.unwrap_or(false) == egress
                })
            })
    }

    /// How many entries of the network ACL `network_acl_id` count against
    /// [`MAX_NETWORK_ACL_ENTRIES`]: the ones whose rule number is inside
    /// [`NETWORK_ACL_ENTRY_RULE_NUMBERS`], which is every rule but the catch-all deny each
    /// direction ends with.
    pub fn network_acl_entry_count(&self, network_acl_id: &str) -> usize {
        self.network_acls
            .get(network_acl_id)
            .and_then(|network_acl| network_acl.entries.as_deref())
            .map_or(0, |entries| {
                entries
                    .iter()
                    .filter(|entry| {
                        entry
                            .rule_number
                            .is_some_and(|number| NETWORK_ACL_ENTRY_RULE_NUMBERS.contains(&number))
                    })
                    .count()
            })
    }

    /// What storing `entry` on the network ACL `network_acl_id` costs the store's byte
    /// budget: [`network_acl_entry_payload_bytes`] for a caller-created ACL, and nothing for
    /// a DEFAULT one, on exactly the terms [`Self::insert_network_acl`] charges an ACL's own
    /// entries.
    ///
    /// The ONE definition of that charge, so the gate a handler asks before writing and the
    /// charge [`Self::insert_network_acl_entry`] takes cannot differ.
    pub fn network_acl_entry_charge(&self, network_acl_id: &str, entry: &NetworkAclEntry) -> usize {
        match self.network_acls.get(network_acl_id) {
            Some(network_acl) if network_acl.is_default != Some(true) => {
                network_acl_entry_payload_bytes(entry)
            }
            _ => 0,
        }
    }

    /// Store `entry` on the network ACL `network_acl_id`, and answer whether it was stored.
    ///
    /// The one place an entry enters a stored ACL, so `CreateNetworkAclEntry` and every
    /// reader of `NetworkAcl$Entries` agree on where it sits
    /// ([`network_acl_entry_report_order`]) and on what it costs the store's byte budget
    /// ([`Self::network_acl_entry_charge`]). `false`, with NOTHING written, for an ACL the
    /// store does not hold or when the entry's bytes would cross [`super::MAX_STORED_BYTES`]
    /// — the bound a caller must map onto the quota refusal its operation answers.
    pub fn insert_network_acl_entry(
        &mut self,
        network_acl_id: &str,
        entry: NetworkAclEntry,
    ) -> bool {
        if !self.network_acls.contains_key(network_acl_id) {
            return false;
        }
        let charge = self.network_acl_entry_charge(network_acl_id, &entry);
        if self.would_exceed_stored_bytes(charge) {
            return false;
        }
        let Some(network_acl) = self.network_acls.get_mut(network_acl_id) else {
            return false;
        };
        let entries = network_acl.entries.get_or_insert_with(Vec::new);
        entries.push(reported_network_acl_entry(entry));
        entries.sort_by_key(network_acl_entry_report_order);
        self.add_stored_bytes(charge);
        true
    }

    /// Remove the entry the network ACL `network_acl_id` holds at `rule_number` in the
    /// direction `egress` names, and answer it as it was last reported, or `None` when the
    /// ACL holds no such entry.
    ///
    /// The bytes the entry held go back to the store's budget on exactly the terms
    /// [`Self::insert_network_acl_entry`] took them on.
    pub fn remove_network_acl_entry(
        &mut self,
        network_acl_id: &str,
        rule_number: i32,
        egress: bool,
    ) -> Option<NetworkAclEntry> {
        let is_default = self
            .network_acls
            .get(network_acl_id)
            .is_some_and(|network_acl| network_acl.is_default == Some(true));
        let removed = {
            let entries = self
                .network_acls
                .get_mut(network_acl_id)?
                .entries
                .as_mut()?;
            let position = entries.iter().position(|entry| {
                entry.rule_number == Some(rule_number) && entry.egress.unwrap_or(false) == egress
            })?;
            entries.remove(position)
        };
        if !is_default {
            self.release_stored_bytes(network_acl_entry_payload_bytes(&removed));
        }
        Some(removed)
    }

    /// What the store would give back for the entry the network ACL `network_acl_id` holds
    /// at `rule_number` in the direction `egress` names: the charge that entry was taken on,
    /// and nothing when the ACL holds no such entry.
    ///
    /// Asked through [`Self::network_acl_entry_charge`], so the refund a replacement counts
    /// on cannot differ from the charge the stored entry was taken on.
    pub fn network_acl_entry_refund(
        &self,
        network_acl_id: &str,
        rule_number: i32,
        egress: bool,
    ) -> usize {
        self.network_acls
            .get(network_acl_id)
            .and_then(|network_acl| network_acl.entries.as_deref())
            .and_then(|entries| {
                entries.iter().find(|entry| {
                    entry.rule_number == Some(rule_number)
                        && entry.egress.unwrap_or(false) == egress
                })
            })
            .map_or(0, |entry| {
                self.network_acl_entry_charge(network_acl_id, entry)
            })
    }

    /// What replacing the entry the network ACL `network_acl_id` holds at `entry`'s own rule
    /// number and direction with `entry` would ADD to the store's byte budget: the
    /// replacement's charge less what the displaced entry was charged, and nothing when the
    /// replacement is no larger than what it displaces.
    ///
    /// The ONE definition of that growth, so the gate a handler asks before writing and the
    /// gate [`Self::replace_network_acl_entry`] applies cannot differ: a replacement stores no
    /// further entry, so only the DIFFERENCE in the ranges a caller chose the length of can
    /// grow the store.
    pub fn network_acl_entry_replacement_growth(
        &self,
        network_acl_id: &str,
        entry: &NetworkAclEntry,
    ) -> usize {
        let Some(rule_number) = entry.rule_number else {
            return self.network_acl_entry_charge(network_acl_id, entry);
        };
        let egress = entry.egress.unwrap_or(false);
        self.network_acl_entry_charge(network_acl_id, entry)
            .saturating_sub(self.network_acl_entry_refund(network_acl_id, rule_number, egress))
    }

    /// Put `entry` in the place of the entry the network ACL `network_acl_id` already holds
    /// at the same rule number and direction, and answer the entry it displaced — or `None`,
    /// with nothing written, when the ACL holds no entry there or when the replacement's own
    /// bytes would cross [`super::MAX_STORED_BYTES`], the bound a caller must map onto the
    /// quota refusal its operation answers.
    ///
    /// The one place a stored entry is overwritten, so `ReplaceNetworkAclEntry` and every
    /// reader of `NetworkAcl$Entries` agree on where the rule sits
    /// ([`network_acl_entry_report_order`]) and on what it costs the store's byte budget
    /// ([`Self::network_acl_entry_replacement_growth`]): the displaced entry's bytes go back
    /// and the new entry's are taken, so a replacement grows the store only by the
    /// difference. The replacement keeps the rule number and direction of the entry it
    /// displaces — those are what name it — so the ACL reports the same number of rules in
    /// the same order.
    pub fn replace_network_acl_entry(
        &mut self,
        network_acl_id: &str,
        entry: NetworkAclEntry,
    ) -> Option<NetworkAclEntry> {
        let rule_number = entry.rule_number?;
        let egress = entry.egress.unwrap_or(false);
        // The same gate [`Self::insert_network_acl_entry`] applies to an entry that enters a
        // stored ACL, asked about what a REPLACEMENT can actually grow.
        if self.would_exceed_stored_bytes(
            self.network_acl_entry_replacement_growth(network_acl_id, &entry),
        ) {
            return None;
        }
        let charge = self.network_acl_entry_charge(network_acl_id, &entry);
        let refund = self.network_acl_entry_refund(network_acl_id, rule_number, egress);
        let entries = self.network_acls.get_mut(network_acl_id)?.entries.as_mut()?;
        let position = entries.iter().position(|stored| {
            stored.rule_number == Some(rule_number) && stored.egress.unwrap_or(false) == egress
        })?;
        let displaced = ::std::mem::replace(
            entries.get_mut(position)?,
            reported_network_acl_entry(entry),
        );
        // No re-sort: the replacement carries the rule number and direction of the entry it
        // displaced — those are what named it — and those two are the whole report order
        // ([`network_acl_entry_report_order`]), so the ACL's entries are still in it.
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        Some(displaced)
    }
}

/// Refuse a create the store has no room for: HTTP 400 [`NETWORK_ACL_LIMIT_EXCEEDED`], the
/// answer for a [`ComAmazonawsEc2Amazonec2State::insert_network_acl`] that stored nothing.
pub fn network_acl_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(NETWORK_ACL_LIMIT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The network ACL the `CreateNetworkAcl` `ClientToken` `client_token` already minted, if
    /// one did.
    ///
    /// A READ: a token that has minted nothing is not recorded here. The id is answered even
    /// when the ACL it named has since been deleted — the token map is kept in step by
    /// [`Self::forget_network_acl_client_tokens_of`], so that cannot happen — and the caller
    /// resolves it against [`Self::network_acls`] either way.
    pub fn network_acl_minted_by_client_token(&self, client_token: &str) -> Option<String> {
        self.network_acl_client_tokens.get(client_token).cloned()
    }

    /// Record that the `ClientToken` `client_token` minted the network ACL `network_acl_id`,
    /// so a replay of that request reports that ACL instead of creating a second one.
    ///
    /// The one writer of [`Self::network_acl_client_tokens`]. The token's own bytes are
    /// bounded by [`super::MAX_CLIENT_TOKEN_CHARS`], which the create holds it to, and the
    /// map holds at most one entry per stored ACL.
    pub fn record_network_acl_client_token(&mut self, client_token: String, network_acl_id: &str) {
        self.network_acl_client_tokens
            .insert(client_token, network_acl_id.to_string());
    }

    /// Drop every `ClientToken` entry that names the network ACL `network_acl_id` — what the
    /// removal of an ACL owes, since a token that names an ACL the store no longer holds
    /// could otherwise replay into a dangling id.
    pub fn forget_network_acl_client_tokens_of(&mut self, network_acl_id: &str) {
        self.network_acl_client_tokens
            .retain(|_, minted| minted.as_str() != network_acl_id);
    }
}
