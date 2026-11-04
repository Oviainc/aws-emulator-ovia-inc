// SPDX-License-Identifier: CC0-1.0

//! The host a load balancer publishes as its `DNSName`, over the wire.
//!
//! One rule: the host is composed in the form Elastic Load Balancing publishes for the
//! FAMILY the load balancer belongs to, and the family decides all three ways the two
//! forms differ — the label order, the id beside the name, and whether an internal load
//! balancer's scheme shows in it:
//!
//! * an Application Load Balancer publishes
//!   `[internal-]<name>-<decimal id>.<region>.elb.amazonaws.com`
//!   (`my-load-balancer-1234567890.us-west-2.elb.amazonaws.com`), whose id is minted for
//!   the host alone — its ARN carries another;
//! * a Network or Gateway Load Balancer publishes
//!   `<name>-<hexadecimal id>.elb.<region>.amazonaws.com`
//!   (`my-network-load-balancer-1234567890abcdef.elb.us-west-2.amazonaws.com`), whose id
//!   is the very one its ARN carries, and whose host states its scheme nowhere.
//!
//! The rows span what the rule generalises over: every family the model's
//! `LoadBalancerTypeEnum` names, both schemes its `LoadBalancerSchemeEnum` names, names of
//! different shapes, and two Regions — the Region and the family are exactly what the form
//! is keyed on. Every assertion is made on what a later `DescribeLoadBalancers` reports as
//! well as on what the create answered, and no row asserts a literal id: the id is minted,
//! so what is under test is its FORM and that the two reads agree on it.
//!
//! The wire harness (`emulator`, `call_in_region`, `action`, `p`) comes from
//! `provisioning_loop`; nothing here restates it.

use crate::provisioning_loop::{action, call_in_region, emulator, p, Reply};
use smithy_server_runtime::MultiServiceRouter;

/// The two Regions the rows run in: the one every other suite signs for, and a second one,
/// so a host that hard-coded a Region would fail a row.
const REGIONS: [&str; 2] = ["us-west-2", "eu-west-1"];

/// How many decimal digits an Application Load Balancer's published id has
/// (`my-load-balancer-1234567890.us-west-2.elb.amazonaws.com`).
const DECIMAL_ID_DIGITS: usize = 10;

/// How many hexadecimal digits the id a Network or Gateway Load Balancer publishes has: the
/// width of the id its own ARN carries.
const HEX_ID_DIGITS: usize = 16;

/// One row: the case name, the load balancer's `Name`, the `Type` and `Scheme` it is
/// created with, and how many subnets it is placed in.
struct Row {
    case: &'static str,
    name: &'static str,
    lb_type: &'static str,
    scheme: &'static str,
    subnets: usize,
}

/// Every family against both schemes, with names of different shapes and lengths — the
/// name is carried into the host verbatim, so a row with hyphens and digits in it is a
/// different input from a plain one.
const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer",
        name: "alb-public",
        lb_type: "application",
        scheme: "internet-facing",
        subnets: 2,
    },
    Row {
        case: "an internal application load balancer",
        name: "alb-private",
        lb_type: "application",
        scheme: "internal",
        subnets: 2,
    },
    Row {
        case: "an application load balancer with digits and hyphens in its name",
        name: "alb-7-edge-01",
        lb_type: "application",
        scheme: "internet-facing",
        subnets: 3,
    },
    Row {
        case: "an application load balancer with a one-character name",
        name: "a",
        lb_type: "application",
        scheme: "internal",
        subnets: 2,
    },
    Row {
        case: "an internet-facing network load balancer",
        name: "nlb-public",
        lb_type: "network",
        scheme: "internet-facing",
        subnets: 1,
    },
    Row {
        case: "an internal network load balancer",
        name: "nlb-private",
        lb_type: "network",
        scheme: "internal",
        subnets: 3,
    },
    Row {
        case: "an internet-facing gateway load balancer",
        name: "gwlb-public",
        lb_type: "gateway",
        scheme: "internet-facing",
        subnets: 1,
    },
    Row {
        case: "an internal gateway load balancer",
        name: "gwlb-private",
        lb_type: "gateway",
        scheme: "internal",
        subnets: 2,
    },
];

/// The host the published form says a load balancer of `row` in `region` must have, split
/// into the text before the id, the id's own rule, and the text after it.
struct Published {
    /// Everything the host carries before the minted id, including the trailing hyphen.
    head: String,
    /// Everything it carries after the id.
    tail: String,
    /// How many characters the id has.
    digits: usize,
    /// Whether the id is hexadecimal (a Network or Gateway Load Balancer's) rather than
    /// decimal (an Application Load Balancer's).
    hexadecimal: bool,
}

impl Published {
    /// The form for `row` in `region`.
    fn of(row: &Row, region: &str) -> Self {
        match row.lb_type {
            "application" => {
                // An internal Application Load Balancer's host states its scheme.
                let prefix = if row.scheme == "internal" {
                    "internal-"
                } else {
                    ""
                };
                Self {
                    head: format!("{prefix}{}-", row.name),
                    tail: format!(".{region}.elb.amazonaws.com"),
                    digits: DECIMAL_ID_DIGITS,
                    hexadecimal: false,
                }
            }
            // A Network or Gateway Load Balancer's host states its scheme nowhere, whatever
            // the scheme is.
            _ => Self {
                head: format!("{}-", row.name),
                tail: format!(".elb.{region}.amazonaws.com"),
                digits: HEX_ID_DIGITS,
                hexadecimal: true,
            },
        }
    }

    /// The id `host` carries, or an explanation of how it breaks this form.
    fn id_of(&self, host: &str) -> Result<String, String> {
        let id = host
            .strip_prefix(&self.head)
            .ok_or_else(|| format!("expected the host to start with {:?}", self.head))?
            .strip_suffix(&self.tail)
            .ok_or_else(|| format!("expected the host to end with {:?}", self.tail))?;
        if id.len() != self.digits {
            return Err(format!(
                "expected an id of {} characters, got {id:?}",
                self.digits
            ));
        }
        let well_formed = if self.hexadecimal {
            id.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            // A decimal id of the published width: digits only, and not one the service
            // padded out of a shorter number.
            id.chars().all(|c| c.is_ascii_digit()) && !id.starts_with('0')
        };
        if !well_formed {
            let kind = if self.hexadecimal {
                "hexadecimal"
            } else {
                "decimal"
            };
            return Err(format!("expected a {kind} id, got {id:?}"));
        }
        Ok(id.to_string())
    }
}

/// Create the load balancer `row` describes in `region`, and answer what the create replied.
async fn create(router: &MultiServiceRouter, region: &str, row: &Row) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", row.name));
    form.push(p("Type", row.lb_type));
    form.push(p("Scheme", row.scheme));
    if row.lb_type == "gateway" {
        // A Gateway Load Balancer carries IPv4 addresses alone.
        form.push(p("IpAddressType", "ipv4"));
    }
    for index in 1..=row.subnets {
        form.push(p(
            &format!("Subnets.member.{index}"),
            &format!("subnet-{index}{index}{index}{index}{index}{index}{index}{index}"),
        ));
    }
    let reply = call_in_region(router, region, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {:?} in {region}: expected the create to be served, got {} body {}",
        row.case, reply.status, reply.body
    );
    reply
}

/// The host a load balancer publishes follows the form of its FAMILY, in whichever Region
/// it was created, and a later read reports the same host.
#[tokio::test]
async fn the_published_host_follows_the_form_of_its_family() {
    // Every host seen, so no two load balancers publish the same one.
    let mut seen: Vec<String> = Vec::new();

    for region in REGIONS {
        let (router, _engine) = emulator();
        for row in ROWS {
            let created = create(&router, region, row).await;
            let host = created
                .text("DNSName")
                .unwrap_or_else(|| panic!("case {:?}: no DNSName in {}", row.case, created.body));
            let arn = created.text("LoadBalancerArn").unwrap_or_else(|| {
                panic!("case {:?}: no LoadBalancerArn in {}", row.case, created.body)
            });

            let published = Published::of(row, region);
            let id = published
                .id_of(&host)
                .unwrap_or_else(|why| panic!("case {:?} in {region}: {why}, host {host:?}", row.case));

            // A Network or Gateway Load Balancer publishes the very id its ARN carries; an
            // Application Load Balancer publishes a decimal id of its own, which its ARN
            // does not carry.
            let arn_id = arn.rsplit('/').next().unwrap_or_default().to_string();
            if published.hexadecimal {
                assert_eq!(
                    id, arn_id,
                    "case {:?} in {region}: expected the host to carry the ARN's own id, \
                     host {host:?} arn {arn}",
                    row.case
                );
            } else {
                assert_ne!(
                    id, arn_id,
                    "case {:?} in {region}: an application load balancer's host carries a \
                     decimal id of its own, host {host:?} arn {arn}",
                    row.case
                );
            }

            // The read reports the host the create answered: it is stored, not recomposed.
            let mut form = action("DescribeLoadBalancers");
            form.push(p("LoadBalancerArns.member.1", &arn));
            let read = call_in_region(&router, region, &form).await;
            assert_eq!(
                read.status, 200,
                "case {:?} in {region}: expected 200 from the read, got {} body {}",
                row.case, read.status, read.body
            );
            assert_eq!(
                read.text("DNSName").as_deref(),
                Some(host.as_str()),
                "case {:?} in {region}: a read must report the host the create published, \
                 got body {}",
                row.case,
                read.body
            );

            assert!(
                !seen.contains(&host),
                "case {:?} in {region}: every load balancer publishes its own host, \
                 {host:?} repeated",
                row.case
            );
            seen.push(host);
        }
    }
}
