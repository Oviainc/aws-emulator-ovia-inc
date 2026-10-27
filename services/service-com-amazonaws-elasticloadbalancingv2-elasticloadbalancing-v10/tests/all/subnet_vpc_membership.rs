// SPDX-License-Identifier: CC0-1.0

//! Every subnet a load balancer is placed in belongs to ONE VPC — the load balancer's.
//!
//! A load balancer's network interfaces live in the subnets it is placed in and a network
//! interface belongs to exactly one VPC, so a placement naming a subnet of another VPC is a
//! configuration no load balancer can hold: `CreateLoadBalancer` and `SetSubnets` refuse it
//! with the modelled `InvalidConfigurationRequest`.
//!
//! Which VPC a subnet is in is EC2's to say, so this suite drives the two halves this crate
//! owns:
//!
//! * THE RULE ITSELF, tabled directly over what EC2 reports — the VPC the load balancer
//!   already carries, how many subnets the request names, which of them is the foreign one
//!   and in which position, and the cases that are awkward to stand a VPC up for (no EC2 to
//!   ask, a subnet EC2 reports no VPC for).
//! * THE WIRE PATH with no EC2 mounted, where nothing is known about the subnets and so
//!   nothing is refused on them: the placement is served and replaces the previous one,
//!   because a refusal is grounded in what EC2 reported rather than assumed from silence.
//!
//! The whole rule end to end — two VPCs created through EC2, a load balancer in one of
//! them, and each mixed placement refused — needs both services on one engine and is tabled
//! in `core/router/tests/all/elbv2_subnet_vpc_membership.rs`.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) comes from `provisioning_loop` and
//! the load balancer creator from `load_balancer_attributes`; nothing here restates them.

use crate::load_balancer_attributes::create_load_balancer;
use crate::provisioning_loop::{action, call, emulator, members, p};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    check_subnet_vpcs, network::SubnetFacts,
};

/// One subnet as EC2 reports it, reduced to what this rule reads off it: its id and the VPC
/// it is in (`None` for a subnet EC2 reported no VPC for).
fn subnet(subnet_id: &str, vpc_id: Option<&str>) -> SubnetFacts {
    SubnetFacts {
        subnet_id: subnet_id.to_string(),
        vpc_id: vpc_id.map(str::to_string),
        availability_zone: None,
        cidrs: Vec::new(),
    }
}

/// The ids of these subnets, in the order the request named them.
fn ids(subnets: &[SubnetFacts]) -> Vec<String> {
    subnets
        .iter()
        .map(|subnet| subnet.subnet_id.clone())
        .collect()
}

/// The refusal a placement spanning two VPCs earns, for the VPC it had to stay in.
fn refusal(vpc_id: &str) -> String {
    format!("All subnets must belong to the same VPC: '{vpc_id}'")
}

/// One row of the rule table: the case name, the VPC the load balancer already carries, the
/// subnets EC2 reported for the ids the request named — in request order — and the VPC the
/// refusal must name, if any.
type Row<'a> = (
    &'a str,
    Option<&'a str>,
    Vec<SubnetFacts>,
    Option<&'a str>,
);

#[test]
fn a_placement_belongs_to_one_vpc() {
    let rows: Vec<Row<'_>> = vec![
        (
            "two subnets of the load balancer's own VPC",
            Some("vpc-a"),
            vec![
                subnet("subnet-1", Some("vpc-a")),
                subnet("subnet-2", Some("vpc-a")),
            ],
            None,
        ),
        (
            "one own subnet and one of another VPC",
            Some("vpc-a"),
            vec![
                subnet("subnet-1", Some("vpc-a")),
                subnet("subnet-9", Some("vpc-b")),
            ],
            Some("vpc-a"),
        ),
        (
            // The VPC the refusal names is the load balancer's, whatever the request
            // listed first.
            "the subnet of another VPC named first",
            Some("vpc-a"),
            vec![
                subnet("subnet-9", Some("vpc-b")),
                subnet("subnet-1", Some("vpc-a")),
            ],
            Some("vpc-a"),
        ),
        (
            "every named subnet of another VPC",
            Some("vpc-a"),
            vec![
                subnet("subnet-9", Some("vpc-b")),
                subnet("subnet-8", Some("vpc-b")),
            ],
            Some("vpc-a"),
        ),
        (
            "one subnet, of another VPC",
            Some("vpc-a"),
            vec![subnet("subnet-9", Some("vpc-b"))],
            Some("vpc-a"),
        ),
        (
            "one subnet, of the own VPC",
            Some("vpc-a"),
            vec![subnet("subnet-1", Some("vpc-a"))],
            None,
        ),
        (
            "three subnets, the last of another VPC",
            Some("vpc-a"),
            vec![
                subnet("subnet-1", Some("vpc-a")),
                subnet("subnet-2", Some("vpc-a")),
                subnet("subnet-9", Some("vpc-c")),
            ],
            Some("vpc-a"),
        ),
        (
            "three subnets, each of a different VPC",
            Some("vpc-a"),
            vec![
                subnet("subnet-1", Some("vpc-a")),
                subnet("subnet-9", Some("vpc-b")),
                subnet("subnet-8", Some("vpc-c")),
            ],
            Some("vpc-a"),
        ),
        (
            // A create carries no VPC yet, so the placement is judged against the VPC of
            // the FIRST subnet it names — the VPC it would put the load balancer in.
            "a create in subnets of one VPC",
            None,
            vec![
                subnet("subnet-1", Some("vpc-b")),
                subnet("subnet-2", Some("vpc-b")),
            ],
            None,
        ),
        (
            "a create spanning two VPCs",
            None,
            vec![
                subnet("subnet-1", Some("vpc-b")),
                subnet("subnet-9", Some("vpc-c")),
            ],
            Some("vpc-b"),
        ),
        (
            "a create spanning two VPCs, the other one named first",
            None,
            vec![
                subnet("subnet-9", Some("vpc-c")),
                subnet("subnet-1", Some("vpc-b")),
            ],
            Some("vpc-c"),
        ),
        (
            // Nothing is known about a subnet EC2 reported no VPC for, so nothing is
            // refused on it.
            "a subnet EC2 reports no VPC for, beside the own VPC's",
            Some("vpc-a"),
            vec![
                subnet("subnet-1", Some("vpc-a")),
                subnet("subnet-2", None),
            ],
            None,
        ),
        (
            "every named subnet reported without a VPC, on a create",
            None,
            vec![subnet("subnet-1", None), subnet("subnet-2", None)],
            None,
        ),
    ];
    for (case, carried, subnets, expected) in rows {
        let judged = check_subnet_vpcs(carried, &ids(&subnets), Some(&subnets));
        let want: Result<(), String> = match expected {
            Some(vpc_id) => Err(refusal(vpc_id)),
            None => Ok(()),
        };
        assert_eq!(
            judged, want,
            "case {case:?}: expected {want:?}, got {judged:?}"
        );
    }
}

#[test]
fn with_no_ec2_to_ask_a_placement_is_refused_on_nothing() {
    // No EC2 answered, so the subnets resolve to no facts at all and the rule has nothing
    // to compare: the placement is served, whichever VPC its subnets may really be in.
    for (case, carried) in [
        ("a load balancer carrying a VPC", Some("vpc-a")),
        ("a create, which carries none", None),
    ] {
        let judged = check_subnet_vpcs(carried, &["subnet-1".to_string()], None);
        assert_eq!(
            judged,
            Ok(()),
            "case {case:?}: expected the placement to be served, got {judged:?}"
        );
    }
}

#[tokio::test]
async fn set_subnets_replaces_the_placement_when_no_ec2_resolves_it() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(
        &router,
        "vpc-membership-lb",
        &[
            ("Scheme", "internal"),
            ("Subnets.member.1", "subnet-aaaa1111"),
            ("Subnets.member.2", "subnet-aaaa2222"),
        ],
    )
    .await;

    // Each row replaces the previous placement; with no EC2 mounted every subnet id is one
    // nothing is known about, so the rule refuses none of them and the load balancer ends
    // up placed in exactly what the request named.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("the subnets it was created in", vec!["subnet-aaaa1111", "subnet-aaaa2222"]),
        ("two other subnets", vec!["subnet-bbbb1111", "subnet-bbbb2222"]),
        (
            "three subnets",
            vec!["subnet-cccc1111", "subnet-cccc2222", "subnet-cccc3333"],
        ),
    ];
    for (case, subnets) in rows {
        let mut form = action("SetSubnets");
        form.push(p("LoadBalancerArn", &arn));
        for (index, subnet_id) in subnets.iter().enumerate() {
            form.push(p(&format!("Subnets.member.{}", index + 1), subnet_id));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        let mut read = action("DescribeLoadBalancers");
        read.push(p("LoadBalancerArns.member.1", &arn));
        let described = call(&router, &read).await;
        let mut placed: Vec<String> = members(&described.body, "AvailabilityZones")
            .iter()
            .filter_map(|zone| crate::provisioning_loop::first_text(zone, "SubnetId"))
            .collect();
        placed.sort();
        let mut expected: Vec<String> = subnets.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            placed, expected,
            "case {case:?}: expected the load balancer to be placed in {expected:?}, got \
             {placed:?}"
        );
    }
}
