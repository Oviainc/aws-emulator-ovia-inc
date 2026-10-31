// SPDX-License-Identifier: CC0-1.0

//! What `CreateNetworkAcl` needs beyond the network ACL store itself.
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
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, ComAmazonawsEc2Amazonec2State,
    DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, RULE_ACTION_DENY,
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

/// Refuse a request naming an entry the ACL does not hold: HTTP 400
/// [`INVALID_NETWORK_ACL_ENTRY_NOT_FOUND`], naming the rule number the request sent.
pub fn invalid_network_acl_entry_not_found<E>(rule_number: i32) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_ACL_ENTRY_NOT_FOUND,
        400,
        format!("The network acl entry identified by {rule_number} does not exist."),
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
