// SPDX-License-Identifier: CC0-1.0

//! The subnets the ACCOUNT has, as the service that owns them reports them, for the one
//! resource of this service that is placed in them: a `VpcLink`.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. A client builds its
//! network with `ec2:CreateVpc`/`ec2:CreateSubnet`, or uses the default VPC EC2
//! materialises for every Region of the account, and then names those subnets in
//! `CreateVpcLink$SubnetIds` ("A list of subnet IDs to include in the VPC link"). So EC2
//! is the ONE answer to "which subnets does the account have", and an identifier a
//! request names is resolved there and nowhere else — reached the way any emulated
//! service reaches another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! A VPC link puts one elastic network interface in each subnet it names, and those
//! interfaces live in ONE VPC, so a create has two things to resolve: that the account
//! HAS every subnet named, and that the subnets named agree on the VPC the link's
//! interfaces go in.
//!
//! This service keeps NOTHING of a subnet — a `VpcLink` reports no VPC and no
//! Availability Zone of its own — so it only GATES on what EC2 answered, and the reading
//! `services/common/network.rs` gives that case is the one applied here: a hop that could
//! not be made at all is evidence of nothing, so it refuses nothing and says so at `warn`,
//! and the refusals that ARE served are the ones EC2's answer grounds. Nothing is cached:
//! a subnet created a moment ago resolves at once, and one that was deleted stops
//! resolving at once.
//!
//! ## Why the client the questions below go over is built in this crate
//!
//! `services/common/src/network.rs` states this contract for every service that places a
//! resource in a subnet, and it is DELIBERATELY documentation-only: "it exports nothing,
//! because the one source it names is another emulated SERVICE rather than a function this
//! crate could hold". A per-crate client is this tree's convention — every service that asks
//! EC2 anything builds its own (`fsx`'s `src/state/ec2_subnets.rs`,
//! `licensemanagerusersubscriptions`'s `src/state/ec2_fleet.rs`, `kafka`'s
//! `src/state/network.rs`, `eks`, `opensearch`, `elasticloadbalancingv2`, …) — because each
//! asks a different question and takes its own reading of silence: one that PLACES a resource
//! and reports the subnet's VPC and zone fails closed, while one that only gates, as this
//! module does, refuses nothing on silence. Hoisting the builder would also put the EC2 SDK
//! and the server runtime behind `services/common`, which every service crate links. What is
//! shared is already shared: the Region-label grammar (`service_common::arn::region_label`)
//! and the contract document itself. This crate's own copy of it lives beside this module, in
//! [`super::network`], so the questions below read as questions rather than as client
//! configuration.

use std::collections::{HashMap, HashSet};

use super::network::ec2;

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `SubnetIds`: an identifier the account
/// has no subnet for is then an absent row instead of a failure of the whole call, so
/// every identifier the request named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter.
///
/// The owner evaluates a filter against every subnet the account stores, so a hop costs
/// (identifiers asked x subnets stored): this caps the first factor, so ONE hop costs a
/// constant times what the account stores rather than something the request chooses.
const SUBNET_IDS_PER_HOP: usize = 64;

/// The most DISTINCT subnet identifiers one create resolves at all, which caps how many hops
/// of [`SUBNET_IDS_PER_HOP`] a request can ask for.
///
/// `SubnetIdList` is an unconstrained list in the model
/// (`smithy_query(query="shape", shape="SubnetIdList")`), so without this the PRODUCT a
/// create costs — identifiers resolved x subnets the account stores, one in-process hop per
/// [`SUBNET_IDS_PER_HOP`] identifiers — would be bounded only by how large a body the caller
/// sends. Both factors are therefore capped by constants BEFORE the first hop is made: the
/// reading `licensemanagerusersubscriptions`'s `src/state/ec2_fleet.rs` applies to the same
/// question, down to the same bound.
///
/// It is THIS EMULATOR's own server-side bound on the work one call costs, not an AWS quota,
/// and it is orders of magnitude above any network a VPC link is placed in, which holds one
/// subnet per Availability Zone.
pub const MAX_SUBNET_IDS_RESOLVED: usize = 1_000;

/// How a refusal names a subnet the account has none of under that id.
///
/// Built from the id the REQUEST sent, in the sentence the service that OWNS subnets
/// states for an id it does not hold, so a caller reads the same fact however it reached
/// the lookup.
pub fn subnet_does_not_exist_message(subnet_id: &str) -> String {
    format!("The subnet ID '{subnet_id}' does not exist")
}

/// The sentence a create is refused with when the subnets it names do not agree on a VPC.
///
/// A VPC link's network interfaces are created in one VPC, so a list spanning two of them
/// describes no placement at all; this is API Gateway's own sentence for that refusal.
pub const SUBNETS_NOT_IN_ONE_VPC_MESSAGE: &str = "All subnets provided should be in the same VPC";

/// How a request naming more distinct subnets than [`MAX_SUBNET_IDS_RESOLVED`] is refused:
/// the member and the bound it broke, in this service's own "Invalid value for …" grammar.
/// The bound is the emulator's own — see [`MAX_SUBNET_IDS_RESOLVED`].
fn too_many_subnets_message(named: usize) -> String {
    format!(
        "Invalid value for SubnetIds: at most {MAX_SUBNET_IDS_RESOLVED} subnets may be \
         specified in one request, got {named}."
    )
}

/// What the owner of subnets answered about the identifiers one request named.
enum Answer {
    /// EC2 could not be asked at all — evidence of nothing.
    NotAsked,
    /// Every subnet EC2 holds of those asked about, by identifier, with the VPC it
    /// reports for each. An identifier it holds no subnet for is simply absent, and a
    /// subnet it reports without a VPC of its own carries `None`.
    Held(HashMap<String, Option<String>>),
}

/// Ask EC2 about `subnet_ids`, in hops of at most [`SUBNET_IDS_PER_HOP`] identifiers.
async fn account_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> Answer {
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return Answer::NotAsked;
    };
    let mut held: HashMap<String, Option<String>> = HashMap::new();
    for chunk in subnet_ids.chunks(SUBNET_IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(chunk.iter().map(|id| (*id).to_string()).collect()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => held.extend(output.subnets().iter().filter_map(|subnet| {
                Some((
                    subnet.subnet_id()?.to_string(),
                    subnet.vpc_id().map(str::to_string),
                ))
            })),
            Err(error) => {
                // At `warn`: this gate refuses nothing on silence, so a deployment where
                // the hop fails admits a VPC link the account's network does not support,
                // and that is worth seeing. One failed hop ends the question: a partial
                // answer would refuse an identifier for not being in a page nobody read.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a VPC link named could not be resolved against EC2"
                );
                return Answer::NotAsked;
            }
        }
    }
    Answer::Held(held)
}

/// Resolve every `SubnetIds` identifier a `CreateVpcLink` names against the subnets the
/// account has in `region`, and require them to agree on one VPC.
///
/// `Ok(())` for a list every one of whose subnets the account has in one VPC — and for one
/// EC2 could not be asked about at all, which is evidence of nothing (see this module's
/// documentation). `Err` carries the message for the operation's `BadRequestException`:
/// the FIRST identifier the account has no subnet for, in the order the request lists them,
/// so the answer does not depend on how many of them are wrong, or — when every identifier
/// resolved — that the subnets span more than one VPC.
///
/// Deduplicated, so naming one subnet twice costs one resolution and is one subnet. BOUNDED
/// BY CONSTANTS, not by the request: at most [`MAX_SUBNET_IDS_RESOLVED`] distinct identifiers
/// are resolved at all, in hops of at most [`SUBNET_IDS_PER_HOP`] each, so both factors of
/// the owner's matching cost are capped before the first hop is made, and a request past that
/// cap is refused instead of turned into hops.
pub async fn resolve_vpc_link_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Result<(), String> {
    let mut distinct: Vec<&str> = Vec::with_capacity(subnet_ids.len());
    let mut seen: HashSet<&str> = HashSet::new();
    for subnet_id in subnet_ids {
        if seen.insert(subnet_id.as_str()) {
            distinct.push(subnet_id.as_str());
        }
    }
    if distinct.len() > MAX_SUBNET_IDS_RESOLVED {
        return Err(too_many_subnets_message(distinct.len()));
    }
    if distinct.is_empty() {
        return Ok(());
    }
    let Answer::Held(held) = account_subnets(ctx, region, &distinct).await else {
        return Ok(());
    };

    // Existence first, in the order the request named them: an identifier that names no
    // subnet of the account cannot be placed in, whatever the rest of the list says.
    let mut vpcs: Vec<Option<&str>> = Vec::with_capacity(distinct.len());
    for subnet_id in &distinct {
        let Some(holder) = held.get(*subnet_id) else {
            return Err(subnet_does_not_exist_message(subnet_id));
        };
        vpcs.push(holder.as_deref());
    }
    agree_on_one_vpc(&vpcs)
}

/// `Ok(())` when every subnet EC2 reported a VPC for reports the SAME one.
///
/// A subnet EC2 reports without a VPC of its own cannot disagree with anything, so it
/// decides nothing here.
fn agree_on_one_vpc(vpcs: &[Option<&str>]) -> Result<(), String> {
    let mut agreed: Option<&str> = None;
    for holder in vpcs.iter().filter_map(|holder| *holder) {
        match agreed {
            None => agreed = Some(holder),
            Some(expected) if expected == holder => {}
            Some(_) => return Err(SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{agree_on_one_vpc, SUBNETS_NOT_IN_ONE_VPC_MESSAGE};

    #[test]
    fn a_set_of_subnets_agrees_on_one_vpc_only_when_no_two_name_different_ones() {
        // (case, the VPC EC2 reported for each subnet in the order named, accepted?)
        let rows: Vec<(&str, Vec<Option<&str>>, bool)> = vec![
            ("no subnet at all", vec![], true),
            ("one subnet", vec![Some("vpc-0a1b2c3d")], true),
            (
                "three subnets of one VPC",
                vec![
                    Some("vpc-0a1b2c3d"),
                    Some("vpc-0a1b2c3d"),
                    Some("vpc-0a1b2c3d"),
                ],
                true,
            ),
            (
                "a subnet EC2 reports no VPC for, beside one it does",
                vec![None, Some("vpc-0a1b2c3d")],
                true,
            ),
            (
                "two subnets of different VPCs",
                vec![Some("vpc-0a1b2c3d"), Some("vpc-00112233")],
                false,
            ),
            (
                "the disagreement in the last position of a longer list",
                vec![
                    Some("vpc-0a1b2c3d"),
                    Some("vpc-0a1b2c3d"),
                    Some("vpc-00112233"),
                ],
                false,
            ),
        ];
        for (case, vpcs, accepted) in rows {
            let resolved = agree_on_one_vpc(&vpcs);
            assert_eq!(
                resolved.is_ok(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {resolved:?}"
            );
            if !accepted {
                assert_eq!(
                    resolved,
                    Err(SUBNETS_NOT_IN_ONE_VPC_MESSAGE.to_string()),
                    "case {case:?}: expected the one-VPC sentence"
                );
            }
        }
    }
}
