// SPDX-License-Identifier: CC0-1.0

//! EC2 as a REGIONAL service, end to end over the wire it speaks (`ec2Query`, through the
//! shared [`super::ec2_query`] harness, which carries the Region in the SigV4 `Credential=`
//! scope of every call).
//!
//! The rules under test:
//!
//! 1. A resource EC2 stores in a Region is visible only to a request signed for that
//!    Region: a reporting operation answers a stored resource only when the Region the
//!    resource was created in is the Region the request reached, and an id naming a resource
//!    of another Region names nothing — the kind's own not-found refusal.
//! 2. An account has at most ONE default VPC per Region, which follows from rule 1: every
//!    Region seeds its own, and none of them is reported in another.
//! 3. A Region the account has not enabled serves it nothing: a request signed for one is
//!    refused `AuthFailure` rather than answered with an empty result.
//! 4. Not every Availability Zone of a Region offers every instance type, and a launch into
//!    a zone that does not offer the requested type is refused `Unsupported`.
//! 5. A TERMINATED instance has released its network: it reports no private address and
//!    drops out of every filter that selects on the network it used to occupy.
//!
//! Every table row is a distinct input along what the rule generalises over — which Region,
//! which resource kind, which filter, which instance type, which zone — so no single
//! recorded request is the specification.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Regions rule 1 is a table over: three Regions of the commercial partition that the
/// account can use without opting in, of differing code shapes. Each is checked against
/// every other, so "visible in its own Region only" is asserted in both directions.
const REGIONS: [&str; 3] = ["us-east-1", "us-west-2", "eu-west-1"];

/// The image every launch here names; which image an instance came from has no bearing on
/// which Region holds it.
const AMI: &str = "ami-0123456789abcdef0";

/// A PREVIOUS-generation instance type, which every Availability Zone of a Region offers —
/// AWS's "Previous generation instances" page is what classifies it
/// (`ec2::state::offered_instance_types`).
const EVERYWHERE_TYPE: &str = "m1.small";

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action} in {region}, got {status} with body \
         {body}"
    );
    body
}

/// The one element `name` of a successful response, or a panic naming the case.
fn only(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{name}> in {body}"))
}

/// The network one Region holds for these tables: a VPC, a subnet of it, a security group of
/// it, and one instance launched into that subnet.
#[derive(Debug, Clone)]
struct RegionalNetwork {
    region: &'static str,
    vpc_id: String,
    subnet_id: String,
    group_id: String,
    instance_id: String,
    private_ip: String,
    zone: String,
}

/// Create one network in `region`, through the operations a client would use.
async fn create_network(
    router: &MultiServiceRouter,
    region: &'static str,
    cidr: &str,
) -> RegionalNetwork {
    let case = region;
    let vpc_id = only(
        case,
        &ok(
            router,
            case,
            region,
            "CreateVpc",
            &[("CidrBlock".to_string(), cidr.to_string())],
        )
        .await,
        "vpcId",
    );
    // The first zone of the Region, so the launch below is in a zone that offers every
    // instance type (see rule 4).
    let zone = ec2::state::availability_zones_in_region(region)[0]
        .zone_name
        .clone();
    let subnet_cidr = cidr.replace("/16", "/24");
    let subnet_id = only(
        case,
        &ok(
            router,
            case,
            region,
            "CreateSubnet",
            &[
                ("VpcId".to_string(), vpc_id.clone()),
                ("CidrBlock".to_string(), subnet_cidr),
                ("AvailabilityZone".to_string(), zone.clone()),
            ],
        )
        .await,
        "subnetId",
    );
    let group_id = only(
        case,
        &ok(
            router,
            case,
            region,
            "CreateSecurityGroup",
            &[
                ("VpcId".to_string(), vpc_id.clone()),
                ("GroupName".to_string(), format!("group-{region}")),
                ("GroupDescription".to_string(), "regional".to_string()),
            ],
        )
        .await,
        "groupId",
    );
    let launched = ok(
        router,
        case,
        region,
        "RunInstances",
        &[
            ("ImageId".to_string(), AMI.to_string()),
            ("MinCount".to_string(), "1".to_string()),
            ("MaxCount".to_string(), "1".to_string()),
            ("InstanceType".to_string(), EVERYWHERE_TYPE.to_string()),
            ("SubnetId".to_string(), subnet_id.clone()),
        ],
    )
    .await;
    RegionalNetwork {
        region,
        vpc_id,
        subnet_id,
        group_id,
        instance_id: only(case, &launched, "instanceId"),
        private_ip: only(case, &launched, "privateIpAddress"),
        zone,
    }
}

/// One network per Region of [`REGIONS`], each with its own CIDR block so nothing about the
/// addresses can make two Regions indistinguishable.
async fn create_networks(router: &MultiServiceRouter) -> Vec<RegionalNetwork> {
    let mut networks = Vec::new();
    for (index, region) in REGIONS.iter().enumerate() {
        networks.push(create_network(router, region, &format!("10.{index}.0.0/16")).await);
    }
    networks
}

// ---------------------------------------------------------------------------
// Rule 1: a resource is visible only in its own Region
// ---------------------------------------------------------------------------

/// One resource kind of the table: the describe that reports it, the element its id arrives
/// under, the request member that narrows the describe to one id, the id of the resource in
/// a given network, and the error code the describe answers for an id it cannot see.
struct Kind {
    describe: &'static str,
    id_element: &'static str,
    id_member: &'static str,
    id_of: fn(&RegionalNetwork) -> String,
    absent_code: &'static str,
}

/// Every resource kind this suite asserts rule 1 over: the VPC itself and each kind that
/// belongs to one, plus the instance, whose Region is its Availability Zone's.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            describe: "DescribeVpcs",
            id_element: "vpcId",
            id_member: "VpcId",
            id_of: |network| network.vpc_id.clone(),
            absent_code: "InvalidVpcID.NotFound",
        },
        Kind {
            describe: "DescribeSubnets",
            id_element: "subnetId",
            id_member: "SubnetId",
            id_of: |network| network.subnet_id.clone(),
            absent_code: "InvalidSubnetID.NotFound",
        },
        Kind {
            describe: "DescribeSecurityGroups",
            id_element: "groupId",
            id_member: "GroupId",
            id_of: |network| network.group_id.clone(),
            absent_code: "InvalidGroup.NotFound",
        },
        Kind {
            describe: "DescribeInstances",
            id_element: "instanceId",
            id_member: "InstanceId",
            id_of: |network| network.instance_id.clone(),
            absent_code: "InvalidInstanceID.NotFound",
        },
    ]
}

/// An unfiltered describe reports the resources of the Region it was signed for and of no
/// other, for every kind and every pair of Regions.
#[tokio::test]
async fn a_describe_reports_only_the_resources_of_its_own_region() {
    let (router, _engine) = super::ec2_query::emulator();
    let networks = create_networks(&router).await;

    for kind in kinds() {
        for asking in &networks {
            let case = format!("{} in {}", kind.describe, asking.region);
            let reported: BTreeSet<String> =
                elements(&ok(&router, &case, asking.region, kind.describe, &[]).await, kind.id_element)
                    .into_iter()
                    .collect();
            for created in &networks {
                let id = (kind.id_of)(created);
                let expected = created.region == asking.region;
                assert_eq!(
                    reported.contains(&id),
                    expected,
                    "case {case:?}: {id} belongs to {}, so it must {} be reported; got \
                     {reported:?}",
                    created.region,
                    if expected { "" } else { "not" }
                );
            }
        }
    }
}

/// Naming the id of a resource of ANOTHER Region is the kind's own not-found refusal, not an
/// absent row: the describe cannot see it, so the id names nothing there.
#[tokio::test]
async fn an_id_of_another_regions_resource_names_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let networks = create_networks(&router).await;

    for kind in kinds() {
        for asking in &networks {
            for created in &networks {
                let id = (kind.id_of)(created);
                let case = format!("{} in {} for {id}", kind.describe, asking.region);
                let (status, body) = call(
                    &router,
                    asking.region,
                    kind.describe,
                    &indexed(kind.id_member, &[id.as_str()]),
                )
                .await;
                if created.region == asking.region {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a resource of this Region must resolve, got {status} \
                         with body {body}"
                    );
                    assert_eq!(
                        elements(&body, kind.id_element),
                        vec![id.clone()],
                        "case {case:?}: expected exactly {id}, got {body}"
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: a resource of another Region names nothing, so the \
                         request is refused; got {status} with body {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        kind.absent_code,
                        "case {case:?}: expected {}, got {body}",
                        kind.absent_code
                    );
                }
            }
        }
    }
}

/// A filter is narrowed by the Region too: the private address of an instance in one Region
/// selects nothing in another, whichever of the two documented address filters is used.
#[tokio::test]
async fn an_address_filter_matches_only_in_the_region_that_holds_the_address() {
    let (router, _engine) = super::ec2_query::emulator();
    let networks = create_networks(&router).await;

    // The two documented filters that select on an instance's private address: its own
    // primary, and the addresses of the interfaces attached to it
    // (`DescribeInstancesRequest$Filters`).
    for filter in [
        "private-ip-address",
        "network-interface.addresses.private-ip-address",
    ] {
        for asking in &networks {
            for created in &networks {
                let case = format!("{filter} {} in {}", created.private_ip, asking.region);
                let body = ok(
                    &router,
                    &case,
                    asking.region,
                    "DescribeInstances",
                    &filters(&[(filter, &[created.private_ip.as_str()])]),
                )
                .await;
                let expected: Vec<String> = if created.region == asking.region {
                    vec![created.instance_id.clone()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    elements(&body, "instanceId"),
                    expected,
                    "case {case:?}: expected {expected:?}, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: one default VPC per Region
// ---------------------------------------------------------------------------

/// Each Region reports exactly one default VPC, under either spelling EC2 serves the filter
/// with, and no two Regions report the same one.
///
/// The rows are the Regions of [`REGIONS`] crossed with the two filter spellings, driven
/// against a store that has already been asked about every one of those Regions — which is
/// exactly the account baseline a multi-Region client reads.
#[tokio::test]
async fn each_region_reports_exactly_one_default_vpc() {
    let (router, _engine) = super::ec2_query::emulator();
    // Ask in every Region first, so each Region's ambient network exists before any of the
    // assertions below: a second Region's default VPC must not show up in the first's.
    for region in REGIONS {
        ok(&router, region, region, "DescribeVpcs", &[]).await;
    }

    let mut defaults: BTreeSet<String> = BTreeSet::new();
    for region in REGIONS {
        // "is-default" is the spelling the model documents; "isDefault" is the legacy
        // camelCase one EC2 also serves.
        for spelling in ["is-default", "isDefault"] {
            let case = format!("{spelling} in {region}");
            let body = ok(
                &router,
                &case,
                region,
                "DescribeVpcs",
                &filters(&[(spelling, &["true"])]),
            )
            .await;
            let reported = elements(&body, "vpcId");
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: an account has at most one default VPC per Region, got \
                 {reported:?}"
            );
            defaults.insert(reported[0].clone());
        }
    }
    assert_eq!(
        defaults.len(),
        REGIONS.len(),
        "each Region's default VPC is its own, got {defaults:?} for {REGIONS:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a Region the account has not enabled
// ---------------------------------------------------------------------------

/// A request signed for a Region that requires an opt-in the account has not made is
/// refused `AuthFailure`, whichever operation it names — never answered with an empty set,
/// which is the answer an enabled but empty Region gives.
///
/// The rows are commercial Regions outside AWS's "Regions that are enabled by default"
/// table, crossed with a read and a write.
#[tokio::test]
async fn a_region_the_account_has_not_enabled_refuses_every_request() {
    let (router, _engine) = super::ec2_query::emulator();
    let enabled: BTreeSet<&str> = ec2::state::REGIONS_ENABLED_BY_DEFAULT.iter().copied().collect();

    for region in ["af-south-1", "ap-east-1", "eu-south-1", "me-south-1"] {
        assert!(
            !enabled.contains(region),
            "case {region:?}: the row must name a Region the account has to opt in to"
        );
        for (action, params) in [
            ("DescribeVpcs", Vec::new()),
            ("DescribeInstances", Vec::new()),
            (
                "CreateVpc",
                vec![("CidrBlock".to_string(), "10.99.0.0/16".to_string())],
            ),
        ] {
            let (status, body) = call(&router, region, action, &params).await;
            assert_eq!(
                status, 400,
                "case {region:?}/{action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "AuthFailure",
                "case {region:?}/{action}: expected AuthFailure, got {body}"
            );
            assert!(
                elements(&body, "vpcId").is_empty() && elements(&body, "instanceId").is_empty(),
                "case {region:?}/{action}: a refused request reports no resource, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: per-zone instance type offerings
// ---------------------------------------------------------------------------

/// `DescribeInstanceTypeOfferings(LocationType='availability-zone')` reports a STRICT SUBSET
/// of a Region's zones for a current-generation instance type, and every zone for a previous
/// generation one — so the offerings boundary a capacity-aware client falls back over
/// exists.
///
/// The rows cross the Regions of [`REGIONS`] with instance types of both generations, none
/// of which shares a family with another.
#[tokio::test]
async fn an_instance_type_is_offered_in_some_zones_of_a_region_and_not_others() {
    let (router, _engine) = super::ec2_query::emulator();

    // (the instance type, whether every zone of a Region offers it)
    let rows: [(&str, bool); 5] = [
        // Current generation, so the Region's last zone does not offer it.
        ("t3.micro", false),
        ("m5.large", false),
        ("c6g.medium", false),
        // Previous generation, offered in every zone.
        (EVERYWHERE_TYPE, true),
        ("c3.large", true),
    ];

    for region in REGIONS {
        let zones: Vec<String> = ec2::state::availability_zones_in_region(region)
            .into_iter()
            .map(|zone| zone.zone_name)
            .collect();
        for (instance_type, everywhere) in rows {
            let case = format!("{instance_type} in {region}");
            let mut params = vec![(
                "LocationType".to_string(),
                "availability-zone".to_string(),
            )];
            params.extend(filters(&[("instance-type", &[instance_type])]));
            let body = ok(&router, &case, region, "DescribeInstanceTypeOfferings", &params).await;
            let offered = elements(&body, "location");
            if everywhere {
                assert_eq!(
                    offered, zones,
                    "case {case:?}: a previous-generation type is offered in every zone, got \
                     {offered:?}"
                );
            } else {
                assert!(
                    !offered.is_empty() && offered.len() < zones.len(),
                    "case {case:?}: expected a strict subset of {zones:?}, got {offered:?}"
                );
                assert!(
                    offered.iter().all(|zone| zones.contains(zone)),
                    "case {case:?}: every offering is in a zone of the Region, got {offered:?}"
                );
            }
        }
    }
}

/// A launch into a zone that does not offer the requested instance type is refused
/// `Unsupported`, and the same launch into a zone that does offer it succeeds — the
/// offerings a client enumerates and the launch it then makes answer from one fact.
#[tokio::test]
async fn a_launch_into_a_zone_that_does_not_offer_the_type_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    for region in REGIONS {
        for instance_type in ["t3.micro", "m5.large", EVERYWHERE_TYPE] {
            let offering: BTreeSet<String> =
                ec2::state::zones_offering_instance_type(region, instance_type)
                    .into_iter()
                    .collect();
            for zone in ec2::state::availability_zones_in_region(region) {
                let case = format!("{instance_type} into {}", zone.zone_name);
                let params = vec![
                    ("ImageId".to_string(), AMI.to_string()),
                    ("MinCount".to_string(), "1".to_string()),
                    ("MaxCount".to_string(), "1".to_string()),
                    ("InstanceType".to_string(), instance_type.to_string()),
                    (
                        "Placement.AvailabilityZone".to_string(),
                        zone.zone_name.clone(),
                    ),
                ];
                let (status, body) = call(&router, region, "RunInstances", &params).await;
                if offering.contains(&zone.zone_name) {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a zone that offers the type must serve the launch, got \
                         {status} with body {body}"
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: a zone that does not offer the type must refuse the \
                         launch, got {status} with body {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "Unsupported",
                        "case {case:?}: expected Unsupported, got {body}"
                    );
                    let message = unescaped(&body);
                    assert!(
                        message.contains(instance_type) && message.contains(&zone.zone_name),
                        "case {case:?}: the refusal must name the type and the zone, got {body}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a terminated instance has released its network
// ---------------------------------------------------------------------------

/// One row of the terminated-instance filter table: the filter name, the value it carries,
/// and whether the STOPPED instance must still match it.
type NetworkFilterRow = (&'static str, String, bool);

/// A STOPPED instance keeps its private address and its place in the network; a TERMINATED
/// one reports no address at all and drops out of every filter that selects on the network
/// it used to occupy.
///
/// The rows are the filters an instance's network membership is read through, each applied
/// to the stopped instance (which must still match) and to the terminated one (which must
/// not).
#[tokio::test]
async fn a_terminated_instance_releases_its_network() {
    let (router, _engine) = super::ec2_query::emulator();
    let network = create_network(&router, REGIONS[0], "10.61.0.0/16").await;
    let region = network.region;
    let stopped = network.instance_id.clone();
    let stopped_ip = network.private_ip.clone();

    let doomed = ok(
        &router,
        "the instance to terminate",
        region,
        "RunInstances",
        &[
            ("ImageId".to_string(), AMI.to_string()),
            ("MinCount".to_string(), "1".to_string()),
            ("MaxCount".to_string(), "1".to_string()),
            ("InstanceType".to_string(), EVERYWHERE_TYPE.to_string()),
            ("SubnetId".to_string(), network.subnet_id.clone()),
        ],
    )
    .await;
    let terminated = only("the instance to terminate", &doomed, "instanceId");
    let terminated_ip = only("the instance to terminate", &doomed, "privateIpAddress");
    assert_ne!(
        stopped_ip, terminated_ip,
        "the two instances must hold different addresses, got {stopped_ip} twice"
    );

    ok(
        &router,
        "stop",
        region,
        "StopInstances",
        &indexed("InstanceId", &[stopped.as_str()]),
    )
    .await;
    ok(
        &router,
        "terminate",
        region,
        "TerminateInstances",
        &indexed("InstanceId", &[terminated.as_str()]),
    )
    .await;
    // Both clock-owed moves are due one transition span after the calls that started them;
    // nothing sleeps for them in the service, so the test waits them out.
    tokio::time::sleep(ec2::state::TRANSITION_DELAY * 3).await;
    for (case, instance_id, expected) in [
        ("the stopped instance", &stopped, "stopped"),
        ("the terminated instance", &terminated, "terminated"),
    ] {
        let state =
            super::ec2_query::described_instance_state(&router, region, instance_id).await;
        assert_eq!(
            state, expected,
            "case {case:?}: expected the instance to reach {expected}, got {state:?}"
        );
    }

    // The terminated instance reports no private address, while the stopped one still does.
    let reported = ok(
        &router,
        "the private addresses reported",
        region,
        "DescribeInstances",
        &indexed("InstanceId", &[stopped.as_str(), terminated.as_str()]),
    )
    .await;
    let addresses: BTreeSet<String> = elements(&reported, "privateIpAddress").into_iter().collect();
    assert!(
        addresses.contains(&stopped_ip),
        "a stopped instance keeps its private address {stopped_ip}, got {addresses:?}"
    );
    assert!(
        !addresses.contains(&terminated_ip),
        "a terminated instance reports no private address, got {terminated_ip} among \
         {addresses:?}"
    );

    // And it is out of every filter that selects on the network it used to occupy. Each row
    // carries whether the STOPPED instance must still match it: the two address filters name
    // the terminated instance's own address, which no other instance holds, while the
    // network filters name the VPC and subnet the stopped instance still occupies.
    let rows: [NetworkFilterRow; 5] = [
        ("private-ip-address", terminated_ip.clone(), false),
        (
            "network-interface.addresses.private-ip-address",
            terminated_ip.clone(),
            false,
        ),
        ("vpc-id", network.vpc_id.clone(), true),
        ("subnet-id", network.subnet_id.clone(), true),
        // The zone is a fact of the launch rather than of the network, so it still names the
        // terminated instance's placement — the one row where the exclusion does NOT hold,
        // which is what keeps the rows above about the network and not about the instance
        // disappearing from every report.
        ("availability-zone", network.zone.clone(), true),
    ];
    for (filter, value, expect_stopped) in rows {
        let case = format!("{filter}={value}");
        let body = ok(
            &router,
            &case,
            region,
            "DescribeInstances",
            &filters(&[(filter, &[value.as_str()])]),
        )
        .await;
        let matched = elements(&body, "instanceId");
        let expect_terminated = filter == "availability-zone";
        assert_eq!(
            matched.contains(&terminated),
            expect_terminated,
            "case {case:?}: a terminated instance holds no network, so it must {}match a \
             network filter; got {matched:?}",
            if expect_terminated { "" } else { "not " }
        );
        assert_eq!(
            matched.contains(&stopped),
            expect_stopped,
            "case {case:?}: the stopped instance still occupies the network, got {matched:?}"
        );
    }

    // The stopped instance still matches its OWN address under both address filters: the
    // exclusions above are about termination, not about filtering by address.
    for filter in [
        "private-ip-address",
        "network-interface.addresses.private-ip-address",
    ] {
        let case = format!("{filter}={stopped_ip}");
        let body = ok(
            &router,
            &case,
            region,
            "DescribeInstances",
            &filters(&[(filter, &[stopped_ip.as_str()])]),
        )
        .await;
        assert_eq!(
            elements(&body, "instanceId"),
            vec![stopped.clone()],
            "case {case:?}: a stopped instance keeps its address, got {body}"
        );
    }
}

/// The private address a completed termination releases can be given to a NEW launch, and
/// that launch KEEPS it: the release is owed once, at the instant the termination completed,
/// so a later describe — which materialises every clock-owed move — cannot free it again.
///
/// The rows are the two ways an address is handed out: a launch that NAMES the freed address,
/// and the next launch into the same subnet, which must be given a different one because the
/// freed address is held again.
#[tokio::test]
async fn an_address_a_termination_released_stays_with_its_new_holder() {
    let (router, _engine) = super::ec2_query::emulator();
    let network = create_network(&router, REGIONS[0], "10.62.0.0/16").await;
    let region = network.region;
    let freed = network.private_ip.clone();

    ok(
        &router,
        "terminate the first holder",
        region,
        "TerminateInstances",
        &indexed("InstanceId", &[network.instance_id.as_str()]),
    )
    .await;
    tokio::time::sleep(ec2::state::TRANSITION_DELAY * 3).await;
    let state =
        super::ec2_query::described_instance_state(&router, region, &network.instance_id).await;
    assert_eq!(
        state, "terminated",
        "the first holder must reach terminated before its address is free, got {state:?}"
    );

    // A launch that names the freed address is served, which is what "released" means.
    let reused = ok(
        &router,
        "the new holder",
        region,
        "RunInstances",
        &[
            ("ImageId".to_string(), AMI.to_string()),
            ("MinCount".to_string(), "1".to_string()),
            ("MaxCount".to_string(), "1".to_string()),
            ("InstanceType".to_string(), EVERYWHERE_TYPE.to_string()),
            ("SubnetId".to_string(), network.subnet_id.clone()),
            ("PrivateIpAddress".to_string(), freed.clone()),
        ],
    )
    .await;
    let new_holder = only("the new holder", &reused, "instanceId");
    assert_eq!(
        only("the new holder", &reused, "privateIpAddress"),
        freed,
        "the new holder must be given the address it asked for, got {reused}"
    );

    // Several describes later — each one materialising the clock-owed moves — the new holder
    // still reports the address, and the next launch into the same subnet is given another.
    for round in 0..3 {
        let case = format!("round {round}");
        let body = ok(
            &router,
            &case,
            region,
            "DescribeInstances",
            &indexed("InstanceId", &[new_holder.as_str()]),
        )
        .await;
        let reported: BTreeSet<String> =
            elements(&body, "privateIpAddress").into_iter().collect();
        assert_eq!(
            reported,
            BTreeSet::from([freed.clone()]),
            "case {case:?}: the new holder keeps the address it was given, got {body}"
        );
    }
    let next = ok(
        &router,
        "the launch after the reuse",
        region,
        "RunInstances",
        &[
            ("ImageId".to_string(), AMI.to_string()),
            ("MinCount".to_string(), "1".to_string()),
            ("MaxCount".to_string(), "1".to_string()),
            ("InstanceType".to_string(), EVERYWHERE_TYPE.to_string()),
            ("SubnetId".to_string(), network.subnet_id.clone()),
        ],
    )
    .await;
    assert_ne!(
        only("the launch after the reuse", &next, "privateIpAddress"),
        freed,
        "an address a live instance holds must not be handed out again, got {next}"
    );
}

// ---------------------------------------------------------------------------
// DescribeInstances: the filter surface and the page floor
// ---------------------------------------------------------------------------

/// A filter name EC2 does not document is refused with `InvalidParameterValue` and the
/// sentence that names it, rather than with prose of this emulator's own.
///
/// The rows are near-misses of documented names, a misspelling of each of the two address
/// filters, the tag filter without a key, and an empty name.
#[tokio::test]
async fn an_undocumented_describe_instances_filter_is_refused_naming_it() {
    let (router, _engine) = super::ec2_query::emulator();

    for name in [
        "private-ip-addres",
        "network-interface.addresses.private-ip-addres",
        "privateIpAddress",
        "instance_type",
        "Instance-Type",
        "tag:",
        "",
    ] {
        let (status, body) = call(
            &router,
            REGIONS[0],
            "DescribeInstances",
            &filters(&[(name, &["x"])]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

/// `MaxResults` below the smallest page `DescribeInstances` serves is refused rather than
/// answered with an empty set, and the published minimum itself is served.
///
/// The rows are the published bound, one past it on each side, a value well below it and the
/// two that ask for no page at all.
#[tokio::test]
async fn a_page_size_below_the_published_minimum_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    // AWS publishes "Valid Range: Minimum value of 5" for
    // `DescribeInstancesRequest$MaxResults`; the model declares no `@range`, so the bound
    // lives in the handler.
    const MINIMUM: i64 = 5;

    for max_results in [MINIMUM, MINIMUM + 1, 1000] {
        let case = format!("MaxResults={max_results}");
        ok(
            &router,
            &case,
            REGIONS[0],
            "DescribeInstances",
            &[("MaxResults".to_string(), max_results.to_string())],
        )
        .await;
    }
    for max_results in [MINIMUM - 1, 1, 0, -1] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = call(
            &router,
            REGIONS[0],
            "DescribeInstances",
            &[("MaxResults".to_string(), max_results.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, "instanceId").is_empty(),
            "case {case:?}: a refused page reports no instance, got {body}"
        );
    }

    // A request that gets BOTH wrong is told about the filter: the filter name is checked
    // first, so the answer does not depend on which of the two mistakes the caller notices.
    let mut both = filters(&[("bogus-filter-name", &["x"])]);
    both.push(("MaxResults".to_string(), "1".to_string()));
    let (status, body) = call(&router, REGIONS[0], "DescribeInstances", &both).await;
    assert_eq!(
        status, 400,
        "case \"a bad filter beside a bad page size\": expected HTTP 400, got {status} with \
         body {body}"
    );
    assert!(
        unescaped(&body).contains("The filter 'bogus-filter-name' is invalid"),
        "case \"a bad filter beside a bad page size\": the filter name is refused first, got \
         {body}"
    );
}

/// A long-form instance id this service never issued is `InvalidInstanceID.Malformed`
/// before any lookup — the describe decodes the obfuscated identifier first — while a
/// short-form id it holds no instance for is `InvalidInstanceID.NotFound`.
///
/// The rows are long-form ids of differing identifiers and both published id forms.
#[tokio::test]
async fn a_long_form_instance_id_this_service_never_issued_is_malformed() {
    let (router, _engine) = super::ec2_query::emulator();
    let network = create_network(&router, REGIONS[0], "10.70.0.0/16").await;

    let rows: [(&str, &str); 5] = [
        ("i-0123456789abcdef0", "InvalidInstanceID.Malformed"),
        ("i-00000000000000999", "InvalidInstanceID.Malformed"),
        ("i-0ffffffffffffffff", "InvalidInstanceID.Malformed"),
        // The retired short form carries no identifier to decode, so it is looked up.
        ("i-0abc1234", "InvalidInstanceID.NotFound"),
        ("i-9f3c71de", "InvalidInstanceID.NotFound"),
    ];
    for (instance_id, code) in rows {
        let (status, body) = call(
            &router,
            network.region,
            "DescribeInstances",
            &indexed("InstanceId", &[instance_id]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {instance_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {instance_id:?}: expected {code}, got {body}"
        );
    }

    // The instance the launch really minted still resolves, so the refusals above are about
    // the ids rather than about the operation.
    let body = ok(
        &router,
        "the launched instance",
        network.region,
        "DescribeInstances",
        &indexed("InstanceId", &[network.instance_id.as_str()]),
    )
    .await;
    assert_eq!(
        elements(&body, "instanceId"),
        vec![network.instance_id.clone()],
        "the launched instance must still resolve, got {body}"
    );
}
