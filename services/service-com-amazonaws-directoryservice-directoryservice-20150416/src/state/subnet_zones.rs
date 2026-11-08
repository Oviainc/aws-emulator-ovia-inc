// SPDX-License-Identifier: CC0-1.0

//! THE TWO-ZONE PLACEMENT RULE, decided over what the service that owns subnets answered.
//!
//! `DirectoryVpcSettings$SubnetIds`' own documentation states it: "The identifiers of the
//! subnets for the directory servers. The two subnets must be in different Availability
//! Zones. Directory Service creates a directory server and a DNS server in each of these
//! subnets." `DirectoryConnectSettings$SubnetIds` places an AD Connector's servers the same
//! way. The rule therefore has two halves, and they are decided in two different places:
//!
//! * the half that is a fact about the REQUEST — two DISTINCT subnet ids, and no more of
//!   them than a placement may name — is [`super::subnet_pair_defect`], which every
//!   placement runs before anything is resolved or stored;
//! * the half that is a fact about the account's NETWORK — whether those subnets sit in
//!   different zones — is [`single_zone_defect`] below, decided over the subnets
//!   [`super::directory_network::resolve_subnet_networks`] already asked Amazon EC2 about
//!   for the same create. No hop of its own: a create resolves its placement ONCE, and that
//!   one answer serves both this refusal and the record the create stores.
//!
//! Nothing here reaches another service, holds a lock or keeps state: it is the decision
//! alone, so what refuses a placement is only ever what EC2 reported.

use std::collections::BTreeMap;

use super::directory_network::SubnetNetwork;

/// Why the subnets a placement names cannot carry a directory's servers, given what EC2
/// reported for them, or `None` when they can.
///
/// The refusal this grounds is the resource-semantics one (`ClientException`), a different
/// class from the validation refusal a MALFORMED subnet id gets and from the
/// `InvalidParameterException` the request-level half ([`super::subnet_pair_defect`])
/// answers.
///
/// REFUSES ONLY WHAT EC2 GROUNDED: the placement is refused when EC2 reported a zone for
/// EVERY subnet the request named and all of those zones are the SAME. A subnet EC2 reported
/// nothing for, or reported without a zone, leaves the topology unknown — and an unknown
/// topology refuses nothing, since this service keeps no zone of its own to decide it with,
/// which is the reading `super::directory_network` states for that hop (it gates no create
/// on silence) and the one `services/common/src/network.rs` assigns to a service that only
/// GATES on what the owner says.
#[must_use]
pub fn single_zone_defect(
    resolved: &BTreeMap<String, SubnetNetwork>,
    subnet_ids: &[String],
) -> Option<String> {
    let mut zones: Vec<&str> = Vec::new();
    for subnet_id in subnet_ids {
        let zone = resolved
            .get(subnet_id)
            .and_then(|subnet| subnet.availability_zone.as_deref())?;
        if !zones.contains(&zone) {
            zones.push(zone);
        }
    }
    // ONE zone across every subnet the request named — and at least one resolved, so a
    // placement that named nothing says nothing.
    (zones.len() == 1).then(|| SUBNETS_IN_ONE_ZONE.to_string())
}

/// The diagnostic Directory Service refuses a placement whose subnets all sit in ONE
/// Availability Zone with.
///
/// FIXED text, the rule the topology broke: it quotes neither the subnets nor the zone, so
/// every caller that breaks it reads the same sentence. It is a different sentence from
/// [`super::INVALID_SUBNET_IDS`], the one a settings block that names no two distinct subnets
/// gets. [`super::refusal`] appends the request id, once.
pub const SUBNETS_IN_ONE_ZONE: &str =
    "Invalid subnet ID(s). The two subnets must be in different Availability Zones.";
