// SPDX-License-Identifier: CC0-1.0

//! A cluster subnet group's network is the one the ACCOUNT really has, resolved
//! against the service that owns subnets, end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: a cluster subnet group's network identity is
//! READ OFF the subnets the request names, as Amazon EC2 — the service that owns
//! them — reports those subnets, and never derived from the group's name or the
//! Region. Concretely:
//!
//! * `ClusterSubnetGroup$VpcId` is the VPC that CONTAINS the named subnets ("The
//!   VPC ID of the cluster subnet group");
//! * there is one `Subnet` per supplied `SubnetIdentifier`, in the order the
//!   request listed them, and its `SubnetAvailabilityZone.Name` is THAT subnet's
//!   own zone;
//! * a `Subnet` carries exactly the three members the model gives it —
//!   `SubnetIdentifier`, `SubnetAvailabilityZone`, `SubnetStatus`
//!   (`smithy_query(query="shape", shape="Subnet")`) — and the zone its name
//!   alone, so nothing reports a subnet's supported platforms;
//! * a `SubnetIds` entry the account has no subnet for is refused and NOTHING is
//!   stored, and a list spanning two VPCs is refused as well ("The requested
//!   subnet is not valid, or not all of the subnets are in the same VPC" —
//!   `InvalidSubnet`);
//! * every read of the group reports what the write stored, including the
//!   account's ambient `default` group, which covers its default VPC's subnets.
//!
//! EC2 is mounted beside Redshift on ONE engine (the shared harness) and driven
//! through the AWS SDK over the same in-process callback this service's own
//! resolution reaches it through. Nothing here reaches into either store: a subnet
//! exists only because EC2 answered for it, and a subnet group only because a
//! Redshift read reported it. Every id a row names was MINTED by EC2 (or is
//! deliberately absent), so no id this suite writes can pass itself off as one the
//! account holds.
//!
//! The rows differ along what the rule generalises over: how many subnets a group
//! is made of (one, several, the documented maximum), which VPC they belong to
//! (the account's default one or a VPC the client minted), how many zones they
//! cover, the case of the group's name, and — for the refusals — whether the
//! offending identifier was never minted or was minted and deleted, how many of
//! them there are and where in the list they sit.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};

/// Identifiers of the `subnet-` form EC2 mints no subnet for, so each reaches the
/// handler's resolution and resolves to nothing.
const ABSENT_SUBNET: &str = "subnet-0ffffffffffffffff";
const SECOND_ABSENT_SUBNET: &str = "subnet-0eeeeeeeeeeeeeeee";

/// The sentences Amazon Redshift words these refusals with, spelled out here so a
/// change to the fixed TEXT fails a test rather than passing one computed from the
/// same code under test. The variable parts — the offending identifiers, the
/// requested name — come from the request.
const ABSENT_SUBNET_REFUSAL: &str = "Some input subnets in :[subnet-0ffffffffffffffff] are invalid.";
const UNKNOWN_GROUP_READ_REFUSAL: &str = "Cluster Subnet group 'no-such-subnet-group' not found.";
const UNKNOWN_GROUP_DEREFERENCE_REFUSAL: &str =
    "Cluster subnet group 'no-such-group' does not exist.";
const GROUP_IN_USE_REFUSAL: &str =
    "The cluster subnet group cannot be deleted because it is in use.";
const UNCONFIRMED_SUBNETS_REFUSAL: &str =
    "Your request cannot be completed because a dependent internal service is temporarily \
     unavailable. Wait 30 to 60 seconds and try again.";

/// One subnet of the account as EC2 reports it: the VPC that contains it and the
/// zone it lies in. ASKED, never assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placement {
    vpc_id: String,
    zone: String,
}

/// Where each of `subnet_ids` is, as EC2 answers — in the order asked.
async fn placements(emulator: &Emulator, case: &str, subnet_ids: &[String]) -> Vec<Placement> {
    let described = emulator
        .ec2()
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: EC2 must report the subnets it minted: {error:?}")
        });
    subnet_ids
        .iter()
        .map(|subnet_id| {
            let reported = described
                .subnets()
                .iter()
                .find(|subnet| subnet.subnet_id() == Some(subnet_id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: EC2 must report the subnet {subnet_id}")
                });
            Placement {
                vpc_id: reported.vpc_id().unwrap_or_default().to_string(),
                zone: reported.availability_zone().unwrap_or_default().to_string(),
            }
        })
        .collect()
}

/// `CreateClusterSubnetGroup` over `subnet_ids`.
async fn create_group(emulator: &Emulator, name: &str, subnet_ids: &[String]) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterSubnetGroupName".to_string(), name.to_string()),
        ("Description".to_string(), format!("subnets of {name}")),
    ];
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet_id.clone(),
        ));
    }
    call(emulator, "CreateClusterSubnetGroup", &params).await
}

/// `ModifyClusterSubnetGroup` over `subnet_ids`, with `description` when given.
async fn modify_group(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    subnet_ids: &[String],
) -> Answer {
    let mut params: Vec<(String, String)> =
        vec![("ClusterSubnetGroupName".to_string(), name.to_string())];
    if let Some(description) = description {
        params.push(("Description".to_string(), description.to_string()));
    }
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet_id.clone(),
        ));
    }
    call(emulator, "ModifyClusterSubnetGroup", &params).await
}

/// `DescribeClusterSubnetGroups` for one group by name.
async fn describe_group(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeClusterSubnetGroups",
            &[("ClusterSubnetGroupName", name)],
        )
        .await
}

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// Assert one answer is a refusal with this status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert `answer` reports a group over exactly `subnet_ids`, placed where EC2
/// says those subnets are.
fn assert_network(case: &str, answer: &Answer, subnet_ids: &[String], placed: &[Placement]) {
    assert_eq!(
        answer.text("VpcId").as_deref(),
        Some(placed[0].vpc_id.as_str()),
        "case {case}: expected the VPC its subnets are in {:?}, got {:?}",
        placed[0].vpc_id,
        answer.text("VpcId")
    );
    assert_eq!(
        all_text(&answer.body, "SubnetIdentifier"),
        subnet_ids,
        "case {case}: expected one entry per supplied subnet, in request order, got body {}",
        answer.body
    );
    let expected_zones: Vec<String> = placed.iter().map(|place| place.zone.clone()).collect();
    assert_eq!(
        all_text(&answer.body, "Name"),
        expected_zones,
        "case {case}: expected each subnet's own Availability Zone {expected_zones:?}, got body {}",
        answer.body
    );
    assert!(
        !answer.body.contains("SupportedPlatform"),
        "case {case}: expected a Subnet to carry only its three modelled members, got body {}",
        answer.body
    );
    assert_eq!(
        all_text(&answer.body, "SubnetStatus"),
        vec![state::SUBNET_STATUS_ACTIVE.to_string(); subnet_ids.len()],
        "case {case}: expected every subnet {}, got body {}",
        state::SUBNET_STATUS_ACTIVE,
        answer.body
    );
    assert_eq!(
        answer.text("SubnetGroupStatus").as_deref(),
        Some(state::SUBNET_GROUP_STATUS_COMPLETE),
        "case {case}: expected SubnetGroupStatus={}, got {:?}",
        state::SUBNET_GROUP_STATUS_COMPLETE,
        answer.text("SubnetGroupStatus")
    );
}

/// A created group's VPC and per-subnet zones are the ones EC2 holds for the
/// subnets the request named, and a later read reports the same.
#[tokio::test]
async fn a_created_group_reports_the_network_its_subnets_are_in() {
    let emulator = Emulator::new();

    // (case, requested name, how many subnets, whether they are the account's
    // default VPC's own subnets). The documented maximum is "A maximum of 20
    // subnets can be modified in a single request"
    // (`CreateClusterSubnetGroupMessage$SubnetIds`).
    let rows: Vec<(&str, &str, usize, bool)> = vec![
        ("one subnet of the default VPC", "net-default-one", 1, true),
        (
            "every subnet of the default VPC, spanning its zones",
            "net-default-all",
            0,
            true,
        ),
        ("one subnet of a minted VPC", "net-minted-one", 1, false),
        (
            "two subnets of a minted VPC, in two zones",
            "net-minted-two",
            2,
            false,
        ),
        (
            "the documented maximum number of subnets",
            "net-minted-max",
            state::SUBNETS_PER_SUBNET_GROUP_QUOTA,
            false,
        ),
        (
            // "Amazon Redshift stores the value as a lowercase string."
            "a mixed-case name over a minted VPC",
            "Net-Minted-Mixed",
            2,
            false,
        ),
    ];

    for (case, requested, count, in_default_vpc) in rows {
        let subnet_ids = if in_default_vpc {
            let held = emulator.account_subnets().await;
            assert!(
                held.len() >= 2,
                "case {case}: the account's default VPC must have at least two subnets, got \
                 {held:?}"
            );
            // `0` means "all of them", which is what makes a group span every
            // zone the default VPC covers.
            if count == 0 {
                held
            } else {
                held[..count].to_vec()
            }
        } else {
            emulator.subnet_ids(case, count).await
        };
        let placed = placements(&emulator, case, &subnet_ids).await;

        let created = create_group(&emulator, requested, &subnet_ids).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );
        assert_network(case, &created, &subnet_ids, &placed);

        let stored = requested.to_ascii_lowercase();
        let described = describe_group(&emulator, &stored).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected the group readable, got {} with body {}",
            described.status, described.body
        );
        assert_network(&format!("{case}, read back"), &described, &subnet_ids, &placed);

        // The VPC the group reports is one EC2 itself holds: a client that takes
        // it to place a security group or an endpoint addresses a real VPC.
        let vpc = described.text("VpcId").unwrap_or_default();
        let known = emulator
            .ec2()
            .describe_vpcs()
            .vpc_ids(&vpc)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case}: EC2 must resolve the VPC {vpc} the group reports: {error:?}")
            });
        assert_eq!(
            known
                .vpcs()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Vpc::vpc_id)
                .collect::<Vec<&str>>(),
            vec![vpc.as_str()],
            "case {case}: expected EC2 to report exactly the VPC {vpc:?} the group named"
        );
    }
}

/// A `SubnetIds` entry the account has no subnet for is refused with the
/// service's own wording, and no group is created.
#[tokio::test]
async fn a_subnet_the_account_does_not_have_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let held = emulator.subnet_ids("the subnets the rows mix in", 2).await;

    // One subnet EC2 minted and then DELETED: an id the account HELD and holds no
    // longer, which is a different thing from one it never had.
    let deleted = emulator.subnet_ids("a subnet minted and deleted", 1).await;
    emulator
        .ec2()
        .delete_subnet()
        .subnet_id(&deleted[0])
        .send()
        .await
        .expect("EC2 must delete the subnet it minted");

    // (case, group name, the SubnetIds the request carries, the ids the refusal
    // must name — those that resolve to nothing, in request order)
    let rows: Vec<(&str, &str, Vec<String>, Vec<String>)> = vec![
        (
            "an identifier EC2 minted no subnet for, alone",
            "bad-alone",
            vec![ABSENT_SUBNET.to_string()],
            vec![ABSENT_SUBNET.to_string()],
        ),
        (
            "an absent identifier FIRST, before real ones",
            "bad-first",
            vec![ABSENT_SUBNET.to_string(), held[0].clone(), held[1].clone()],
            vec![ABSENT_SUBNET.to_string()],
        ),
        (
            "an absent identifier LAST, after real ones",
            "bad-last",
            vec![held[0].clone(), ABSENT_SUBNET.to_string()],
            vec![ABSENT_SUBNET.to_string()],
        ),
        (
            "two absent identifiers, both named in the refusal",
            "bad-two",
            vec![
                ABSENT_SUBNET.to_string(),
                held[0].clone(),
                SECOND_ABSENT_SUBNET.to_string(),
            ],
            vec![ABSENT_SUBNET.to_string(), SECOND_ABSENT_SUBNET.to_string()],
        ),
        (
            "a subnet EC2 minted and then deleted",
            "bad-deleted",
            vec![deleted[0].clone(), held[1].clone()],
            vec![deleted[0].clone()],
        ),
        (
            "the same absent identifier twice, named once",
            "bad-repeated",
            vec![ABSENT_SUBNET.to_string(), ABSENT_SUBNET.to_string()],
            vec![ABSENT_SUBNET.to_string()],
        ),
    ];

    for (case, name, subnet_ids, offending) in rows {
        let answer = create_group(&emulator, name, &subnet_ids).await;
        assert_refused(case, &answer, 400, "InvalidParameterValue");
        let expected = state::invalid_subnets_message(&offending);
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the refusal {expected:?}, got {:?}",
            answer.text("Message")
        );
        // The sentence itself, spelled out, for the row whose offending list is
        // exactly the one identifier: the fixed text is a requirement, not
        // whatever the helper happens to render.
        if offending == vec![ABSENT_SUBNET.to_string()] {
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(ABSENT_SUBNET_REFUSAL),
                "case {case}: expected {ABSENT_SUBNET_REFUSAL:?}, got {:?}",
                answer.text("Message")
            );
        }

        // NOTHING was stored: the name the refused create carried names no group.
        let described = describe_group(&emulator, name).await;
        assert_refused(
            &format!("{case}, read back"),
            &described,
            400,
            "ClusterSubnetGroupNotFoundFault",
        );
    }
}

/// Where the owner of subnets cannot be ASKED at all, no group is created and the
/// refusal says so: the request is not what is wrong.
#[tokio::test]
async fn a_group_is_not_created_over_subnets_nobody_can_confirm() {
    // Redshift alone: nothing in this process can answer for a subnet.
    let emulator = Emulator::without_subnet_owner();

    // The rows differ along what the identifiers are: ones EC2 would hold in a
    // deployment that mounted it, and ones it would not. Neither resolves here,
    // and neither is the caller's fault.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "one identifier, nobody to ask",
            "unconfirmed-one",
            vec!["subnet-00000000000000001".to_string()],
        ),
        (
            "several identifiers, nobody to ask",
            "unconfirmed-many",
            vec![
                "subnet-00000000000000002".to_string(),
                ABSENT_SUBNET.to_string(),
            ],
        ),
    ];
    for (case, name, subnet_ids) in rows {
        let answer = create_group(&emulator, name, &subnet_ids).await;
        assert_refused(case, &answer, 400, "DependentServiceUnavailableFault");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(UNCONFIRMED_SUBNETS_REFUSAL),
            "case {case}: expected {UNCONFIRMED_SUBNETS_REFUSAL:?}, got {:?}",
            answer.text("Message")
        );
        // Nothing was stored, and the account has no default group here either:
        // a group over subnets nobody could confirm is not one this service
        // reports.
        assert_refused(
            &format!("{case}, read back"),
            &describe_group(&emulator, name).await,
            400,
            "ClusterSubnetGroupNotFoundFault",
        );
        assert_refused(
            &format!("{case}, the default group"),
            &describe_group(&emulator, state::DEFAULT_CLUSTER_SUBNET_GROUP).await,
            400,
            "ClusterSubnetGroupNotFoundFault",
        );
    }
}

/// Subnets of two VPCs cannot make one group: a group has ONE `VpcId`.
#[tokio::test]
async fn subnets_of_two_vpcs_cannot_make_one_group() {
    let emulator = Emulator::new();
    let here = emulator.subnet_ids("the first VPC", 2).await;
    let elsewhere = emulator.subnet_ids("the second VPC", 1).await;
    let mixed = vec![here[0].clone(), elsewhere[0].clone(), here[1].clone()];

    let answer = create_group(&emulator, "two-vpcs", &mixed).await;
    assert_refused("subnets of two VPCs", &answer, 400, "InvalidSubnet");
    let described = describe_group(&emulator, "two-vpcs").await;
    assert_refused(
        "subnets of two VPCs, read back",
        &described,
        400,
        "ClusterSubnetGroupNotFoundFault",
    );
}

/// The account HAS a `default` cluster subnet group, over its default VPC's own
/// subnets, before anybody creates one — and a name that names no group is the
/// read's own refusal, worded as Amazon Redshift words it.
#[tokio::test]
async fn the_account_has_a_default_subnet_group_over_its_default_vpc() {
    let emulator = Emulator::new();

    // Asked of EC2 first: which VPC is the account's default one, and which
    // subnets belong to it.
    let default_vpc = emulator
        .ec2()
        .describe_vpcs()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name("is-default")
                .values("true")
                .build(),
        )
        .send()
        .await
        .expect("EC2 must report the account's default VPC")
        .vpcs()
        .iter()
        .find_map(|vpc| vpc.vpc_id().map(str::to_string))
        .expect("an account has a default VPC in every Region");
    let default_subnets = emulator.account_subnets().await;
    let placed = placements(&emulator, "the default subnet group", &default_subnets).await;

    let described = describe_group(&emulator, state::DEFAULT_CLUSTER_SUBNET_GROUP).await;
    assert_eq!(
        described.status, 200,
        "case the default subnet group: expected 200, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.text("VpcId").as_deref(),
        Some(default_vpc.as_str()),
        "case the default subnet group: expected the account's default VPC {default_vpc:?}, got \
         {:?}",
        described.text("VpcId")
    );
    let mut reported_subnets = all_text(&described.body, "SubnetIdentifier");
    let mut expected_subnets = default_subnets.clone();
    reported_subnets.sort();
    expected_subnets.sort();
    assert_eq!(
        reported_subnets, expected_subnets,
        "case the default subnet group: expected the default VPC's own subnets \
         {expected_subnets:?}, got {reported_subnets:?}"
    );
    let mut reported_zones = all_text(&described.body, "Name");
    let mut expected_zones: Vec<String> = placed.iter().map(|place| place.zone.clone()).collect();
    reported_zones.sort();
    expected_zones.sort();
    assert_eq!(
        reported_zones, expected_zones,
        "case the default subnet group: expected each subnet's own zone {expected_zones:?}, got \
         {reported_zones:?}"
    );
    assert!(
        !described.body.contains("SupportedPlatform"),
        "case the default subnet group: expected only the modelled Subnet members, got body {}",
        described.body
    );

    // A name that resolves to nothing: the read's own refusal, over rows that
    // differ in spelling, length (both documented bounds) and case.
    let longest: String = std::iter::once('g').chain(std::iter::repeat_n('x', 254)).collect();
    let unknown: Vec<(&str, String)> = vec![
        ("a plain unknown name", "no-such-subnet-group".to_string()),
        ("the shortest unknown name", "g".to_string()),
        ("the longest unknown name", longest),
        ("an unknown name in mixed case", "No-Such-Group".to_string()),
        (
            "a name one character off the default group's",
            "defaul".to_string(),
        ),
    ];
    for (case, name) in unknown {
        let answer = describe_group(&emulator, &name).await;
        assert_refused(case, &answer, 400, "ClusterSubnetGroupNotFoundFault");
        let expected = state::subnet_group_not_found_message(
            state::SubnetGroupNotFoundWording::NotFound,
            &name,
        );
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the refusal {expected:?}, got {:?}",
            answer.text("Message")
        );
        // The sentence itself, spelled out, for the row that names it: the read's
        // own wording, capital `S` and "not found." included.
        if name == "no-such-subnet-group" {
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(UNKNOWN_GROUP_READ_REFUSAL),
                "case {case}: expected {UNKNOWN_GROUP_READ_REFUSAL:?}, got {:?}",
                answer.text("Message")
            );
        }
    }
}

/// A modify replaces the group's subnet list — and with it the network the group
/// reports — and a delete leaves no group for a later read to find.
#[tokio::test]
async fn a_modify_replaces_the_network_and_a_delete_leaves_nothing() {
    let emulator = Emulator::new();
    let first = emulator.subnet_ids("the group's first network", 2).await;
    let second = emulator.subnet_ids("the group's second network", 3).await;

    let created = create_group(&emulator, "moved-subnets", &first).await;
    assert_eq!(
        created.status, 200,
        "case create over the first network: expected 200, got {} with body {}",
        created.status, created.body
    );
    assert_network(
        "create over the first network",
        &created,
        &first,
        &placements(&emulator, "the first network", &first).await,
    );

    // (case, the new subnet list, the description the request carries)
    let rows: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        (
            "a modify into another VPC's subnets",
            second.clone(),
            Some("the group's second description"),
        ),
        (
            "a modify back to one subnet of the first VPC",
            first[..1].to_vec(),
            None,
        ),
    ];
    let mut description = created
        .text("Description")
        .expect("the create reports the description it stored");
    for (case, subnet_ids, requested_description) in rows {
        let placed = placements(&emulator, case, &subnet_ids).await;
        let modified = modify_group(
            &emulator,
            "MOVED-SUBNETS",
            requested_description,
            &subnet_ids,
        )
        .await;
        assert_eq!(
            modified.status, 200,
            "case {case}: expected 200, got {} with body {}",
            modified.status, modified.body
        );
        assert_network(case, &modified, &subnet_ids, &placed);

        // "A text description of the subnet group to be modified" is optional, so
        // a request that carries none leaves the one the group had.
        description = requested_description
            .map(str::to_string)
            .unwrap_or(description);
        assert_eq!(
            modified.text("Description").as_deref(),
            Some(description.as_str()),
            "case {case}: expected the description {description:?}, got {:?}",
            modified.text("Description")
        );

        let described = describe_group(&emulator, "moved-subnets").await;
        assert_eq!(
            described.status, 200,
            "case {case}, read back: expected 200, got {} with body {}",
            described.status, described.body
        );
        assert_network(&format!("{case}, read back"), &described, &subnet_ids, &placed);
    }

    // A modify naming a group the account does not hold changes nothing and is
    // that group's own refusal — worded as every operation that DEREFERENCES a
    // group words it.
    let absent = modify_group(&emulator, "no-such-group", None, &first).await;
    assert_refused(
        "a modify naming an absent group",
        &absent,
        400,
        "ClusterSubnetGroupNotFoundFault",
    );
    assert_eq!(
        absent.text("Message").as_deref(),
        Some(UNKNOWN_GROUP_DEREFERENCE_REFUSAL),
        "case a modify naming an absent group: expected {UNKNOWN_GROUP_DEREFERENCE_REFUSAL:?}, \
         got {:?}",
        absent.text("Message")
    );

    // A group a cluster was launched into is IN USE, so it cannot be deleted
    // while that cluster exists.
    let in_use = create_group(
        &emulator,
        "in-use-subnets",
        &emulator.subnet_ids("the group a cluster joins", 1).await,
    )
    .await;
    assert_eq!(
        in_use.status, 200,
        "case seed the in-use group: expected 200, got {} with body {}",
        in_use.status, in_use.body
    );
    let cluster = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", "subnet-group-user"),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("ClusterSubnetGroupName", "in-use-subnets"),
            ],
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "case seed the cluster in the group: expected 200, got {} with body {}",
        cluster.status, cluster.body
    );
    let refused = emulator
        .call(
            "DeleteClusterSubnetGroup",
            &[("ClusterSubnetGroupName", "in-use-subnets")],
        )
        .await;
    assert_refused(
        "a delete of a group a cluster is in",
        &refused,
        400,
        "InvalidClusterSubnetGroupStateFault",
    );
    assert_eq!(
        refused.text("Message").as_deref(),
        Some(GROUP_IN_USE_REFUSAL),
        "case a delete of a group a cluster is in: expected {GROUP_IN_USE_REFUSAL:?}, got {:?}",
        refused.text("Message")
    );
    assert_eq!(
        describe_group(&emulator, "in-use-subnets").await.status,
        200,
        "case a delete of a group a cluster is in: expected the group to stay"
    );

    // The account's own default group is in use by the account itself.
    let default_refused = emulator
        .call(
            "DeleteClusterSubnetGroup",
            &[(
                "ClusterSubnetGroupName",
                state::DEFAULT_CLUSTER_SUBNET_GROUP,
            )],
        )
        .await;
    assert_refused(
        "a delete of the account's default group",
        &default_refused,
        400,
        "InvalidClusterSubnetGroupStateFault",
    );

    // The delete, over the group nothing uses: gone from every later read, and a
    // second delete finds nothing either.
    let deleted = emulator
        .call(
            "DeleteClusterSubnetGroup",
            &[("ClusterSubnetGroupName", "Moved-Subnets")],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case the delete: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    for case in ["after the delete", "after a second delete"] {
        let described = describe_group(&emulator, "moved-subnets").await;
        assert_refused(case, &described, 400, "ClusterSubnetGroupNotFoundFault");
        let again = emulator
            .call(
                "DeleteClusterSubnetGroup",
                &[("ClusterSubnetGroupName", "moved-subnets")],
            )
            .await;
        assert_refused(
            &format!("{case}, the delete itself"),
            &again,
            400,
            "ClusterSubnetGroupNotFoundFault",
        );
    }

    // An unfiltered read still reports the groups the account does hold, so the
    // refusals above are the precondition and not a blanket refusal.
    let listed = emulator.call("DescribeClusterSubnetGroups", &[]).await;
    assert_eq!(
        listed.status, 200,
        "case an unfiltered read: expected 200, got {} with body {}",
        listed.status, listed.body
    );
    let names = all_text(&listed.body, "ClusterSubnetGroupName");
    assert!(
        names.contains(&state::DEFAULT_CLUSTER_SUBNET_GROUP.to_string())
            && names.contains(&"in-use-subnets".to_string())
            && !names.contains(&"moved-subnets".to_string()),
        "case an unfiltered read: expected the groups the account holds in {REGION}, got {names:?}"
    );
}
