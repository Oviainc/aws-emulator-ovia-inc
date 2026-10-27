// SPDX-License-Identifier: CC0-1.0

//! The part of SNS's message-filter grammar that compares an ADDRESS rather than a value: the
//! `cidr` condition a filter policy matches an IP address with.
//!
//! "IP address matching" is one of the matching forms a subscription filter policy may use
//! (Amazon SNS Developer Guide, "Amazon SNS subscription filter policy"): a condition
//! `{"cidr": "<network>/<prefix>"}` holds for a value that names an address inside that
//! network. It lives beside [`super::filter_policy`], which reads every other form of the
//! grammar, because an address is not text compared byte by byte — it is a number in a
//! network, and whether one lies in the other is decided by the network, not by the spelling.

use serde_json::Value;

/// Whether the value `held` names an address that lies in the network `block` names.
///
/// Both sides are read by the tree's IP-network parser rather than by splitting the text here,
/// so an address and a network of either IP version are compared the way the network they
/// describe defines it: a value whose version differs from the block's lies outside it. A
/// condition whose argument is not a network, and a value that is not an address — a number, a
/// list, text that is not an address — lies in nothing, so the condition does not hold.
pub fn matches_cidr(held: &Value, block: &Value) -> bool {
    let (Some(held), Some(block)) = (held.as_str(), block.as_str()) else {
        return false;
    };
    let Ok(network) = block.parse::<::ipnet::IpNet>() else {
        return false;
    };
    match held.parse::<::std::net::IpAddr>() {
        Ok(address) => network.contains(&address),
        Err(_) => false,
    }
}
