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
use crate::ops::types::NetworkAclEntry;
use crate::scaffolded::Scaffolded;

/// The one `ResourceType` a `CreateNetworkAcl` tag specification may name: "The tags to
/// assign to the network ACL" (`CreateNetworkAclRequest$TagSpecifications`), spelled as the
/// modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const NETWORK_ACL_TAGGABLE_RESOURCE: &str = "network-acl";

/// EC2's quota code for one network ACL too many, named the way every other per-resource EC2
/// quota code is (EC2 API "Error codes": `NetworkAclLimitExceeded`).
pub const NETWORK_ACL_LIMIT_EXCEEDED: &str = "NetworkAclLimitExceeded";

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
    [false, true]
        .into_iter()
        .map(|egress| NetworkAclEntry {
            cidr_block: Some(ALL_IPV4_ADDRESSES.to_string()),
            egress: Some(egress),
            // ICMP type and code describe an entry whose protocol is ICMP, and a port range
            // one whose protocol is TCP or UDP; this entry's protocol is every protocol, so
            // it carries neither.
            icmp_type_code: None,
            ipv6_cidr_block: None,
            port_range: None,
            protocol: Some(ALL_IP_PROTOCOLS.to_string()),
            rule_action: Some(RULE_ACTION_DENY.to_string()),
            rule_number: Some(DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER),
        })
        .collect()
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
