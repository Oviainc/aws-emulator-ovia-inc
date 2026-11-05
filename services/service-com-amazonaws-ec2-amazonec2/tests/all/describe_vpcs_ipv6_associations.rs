// SPDX-License-Identifier: CC0-1.0

//! What `DescribeVpcs` reports for a VPC's IPv6 ASSOCIATIONS, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: "information about the IPv6 CIDR blocks associated with the VPC"
//! (`Vpc$Ipv6CidrBlockAssociationSet`) is what the member carries, so a report of a VPC that
//! has associated no IPv6 block carries the member not at all, rather than an empty one — a
//! client that tests for its PRESENCE to decide whether a VPC is dual-stack is reading
//! exactly that. A VPC that HAS an association reports it, however the association was made:
//! at create (`AmazonProvidedIpv6CidrBlock`) or afterwards (`AssociateVpcCidrBlock`).
//!
//! The rows differ along what the rule generalises over: whether the VPC has an IPv6
//! association, and whether it got one at create or later. The IPv4 member beside it
//! (`Vpc$CidrBlockAssociationSet`) always carries the block the VPC was created with, which
//! every row asserts, so the omission is of the EMPTY set and not of association sets in
//! general.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, emulator, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// How a row's VPC comes to have — or not have — an IPv6 association.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ipv6 {
    /// No IPv6 block is ever associated with it.
    None,
    /// The create asks for an Amazon-provided block.
    AtCreate,
    /// The VPC is created IPv4-only and the block is associated afterwards.
    Later,
}

/// One row: a VPC's IPv4 block and how it comes by an IPv6 association, if it does.
struct VpcCase {
    case: &'static str,
    cidr_block: &'static str,
    ipv6: Ipv6,
}

/// The rows: an IPv4-only VPC, one dual-stack from the start, and one that became dual-stack
/// after it was created.
fn vpc_cases() -> Vec<VpcCase> {
    vec![
        VpcCase {
            case: "an IPv4-only VPC",
            cidr_block: "10.61.0.0/16",
            ipv6: Ipv6::None,
        },
        VpcCase {
            case: "a VPC created with an Amazon-provided IPv6 block",
            cidr_block: "10.62.0.0/16",
            ipv6: Ipv6::AtCreate,
        },
        VpcCase {
            case: "a VPC that was given an IPv6 block after it was created",
            cidr_block: "10.63.0.0/16",
            ipv6: Ipv6::Later,
        },
    ]
}

/// `DescribeVpcs` carries `ipv6CidrBlockAssociationSet` for exactly the VPCs that have an
/// IPv6 association, and carries the IPv4 association set for every one of them.
#[tokio::test]
async fn a_describe_reports_only_the_ipv6_associations_a_vpc_has() {
    for row in vpc_cases() {
        let case = row.case;
        let (router, _engine) = emulator();

        let mut create = vec![param("CidrBlock", row.cidr_block)];
        if row.ipv6 == Ipv6::AtCreate {
            create.push(param("AmazonProvidedIpv6CidrBlock", "true"));
        }
        let created = ok(case, &router, "CreateVpc", &create).await;
        let vpc_id = member(case, &created, "vpcId");
        if row.ipv6 == Ipv6::Later {
            ok(
                case,
                &router,
                "AssociateVpcCidrBlock",
                &[
                    param("VpcId", &vpc_id),
                    param("AmazonProvidedIpv6CidrBlock", "true"),
                ],
            )
            .await;
        }

        let described = ok(
            case,
            &router,
            "DescribeVpcs",
            &indexed("VpcId", &[vpc_id.as_str()]),
        )
        .await;

        // The IPv4 side is always reported, with the block the VPC was created with.
        let ipv4_set = member(case, &described, "cidrBlockAssociationSet");
        assert!(
            ipv4_set.contains(row.cidr_block),
            "case {case:?}: expected the IPv4 association set to carry {cidr:?}, got {described}",
            cidr = row.cidr_block
        );

        let reported = element(&described, "ipv6CidrBlockAssociationSet");
        match row.ipv6 {
            Ipv6::None => assert!(
                reported.is_none(),
                "case {case:?}: expected no <ipv6CidrBlockAssociationSet> for a VPC with no \
                 IPv6 association, got {reported:?} in {described}"
            ),
            Ipv6::AtCreate | Ipv6::Later => {
                let set = reported.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an <ipv6CidrBlockAssociationSet>, got \
                         {described}"
                    )
                });
                let associated = element(&set, "ipv6CidrBlock").unwrap_or_else(|| {
                    panic!("case {case:?}: expected an associated IPv6 block, got {set}")
                });
                assert!(
                    associated.contains(':'),
                    "case {case:?}: expected an IPv6 block, got {associated:?}"
                );
            }
        }
    }
}
