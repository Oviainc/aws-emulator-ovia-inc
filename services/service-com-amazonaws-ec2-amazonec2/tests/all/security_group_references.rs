// SPDX-License-Identifier: CC0-1.0

//! `DescribeSecurityGroupReferences`, end to end over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The RULE under test: the report names, for each security group the request names, every
//! OTHER VPC that holds a security group rule referencing it — "describes the VPCs on the
//! other side of a VPC peering or Transit Gateway connection that are referencing the
//! security groups you've specified in this request"
//! (`DescribeSecurityGroupReferences`) — once per referencing VPC and connection, with "the
//! ID of the VPC peering connection (if applicable)"
//! (`SecurityGroupReference$VpcPeeringConnectionId`) taken from the referencing rule. A
//! reference inside the named group's own VPC crosses no connection and is not reported, a
//! group the request does not name is not reported on, and a group the account does not have
//! is refused rather than reported empty.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the VPC the
//! referencing rule sits in, the direction of that rule, the protocol and port range it
//! carries, whether it records a peering connection, how a reference names its group (by id
//! or by name), how many rules of one VPC name the same group, and how many groups the
//! request asks about.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::ops::types::{IpPermission, IpRange, UserIdGroupPair};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupSpec};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// One `DescribeSecurityGroupReferences` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "DescribeSecurityGroupReferences", params).await
}

/// The group name both a group of the VPC under test and a group of a referencing VPC
/// carry, so a reference written by NAME has two candidates and must resolve to the one in
/// the VPC that holds the rule.
const SHARED_NAME: &str = "shared-name";

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// One reported reference. A reference is a RECORD, not one element, so it is read with a
/// parser rather than by finding a tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReferenceRow {
    group_id: String,
    referencing_vpc_id: Option<String>,
    vpc_peering_connection_id: Option<String>,
    transit_gateway_id: Option<String>,
}

/// The references a response reports, in the order it reports them.
fn references(case: &str, body: &str) -> Vec<ReferenceRow> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "securityGroupReferenceSet")
        .into_iter()
        .map(|item| ReferenceRow {
            group_id: child_text(item, "groupId").unwrap_or_default(),
            referencing_vpc_id: child_text(item, "referencingVpcId"),
            vpc_peering_connection_id: child_text(item, "vpcPeeringConnectionId"),
            transit_gateway_id: child_text(item, "transitGatewayId"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Seeding the store
// ---------------------------------------------------------------------------

/// Which VPC a row's referencing group sits in: the one that holds the groups the request
/// names, or one of two others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vpc {
    /// The VPC the named groups are in.
    Named,
    /// A second VPC of the account.
    Other,
    /// A third VPC of the account.
    Third,
}

/// Which of the named security groups a row's rule references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    First,
    Second,
    /// The named group whose name a group of [`Vpc::Other`] also carries.
    Shared,
}

/// What a row's rule names as its source or destination.
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// One of the named groups, by its id — the only form a reference across VPCs can take
    /// ("for a security group in a nondefault VPC, use the security group ID",
    /// `UserIdGroupPair$GroupName`).
    ById(Target),
    /// A group by NAME, resolved in the VPC that holds the rule.
    ByName(&'static str),
    /// No group at all: an IPv4 block.
    Cidr(&'static str),
}

/// One row of the table: a security group of `vpc` holding one rule.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    vpc: Vpc,
    /// Whether the rule is an outbound one.
    egress: bool,
    protocol: &'static str,
    ports: Option<(i32, i32)>,
    reference: Reference,
    /// The VPC peering connection the rule records, if it records one.
    peering: Option<&'static str>,
}

/// The store every test here reads: the VPC holding the groups a request names, two other
/// VPCs that may reference them, and the groups themselves.
struct World {
    /// The VPC the named groups are in.
    named_vpc: String,
    /// A second VPC of the account.
    other_vpc: String,
    /// A third VPC of the account.
    third_vpc: String,
    /// The named group rows reference as [`Target::First`].
    first: String,
    /// The named group rows reference as [`Target::Second`].
    second: String,
    /// The named group called [`SHARED_NAME`], in [`World::named_vpc`].
    shared: String,
    /// A named group nothing references.
    unreferenced: String,
}

impl World {
    /// The id of the VPC `vpc` names.
    fn vpc_id(&self, vpc: Vpc) -> &str {
        match vpc {
            Vpc::Named => &self.named_vpc,
            Vpc::Other => &self.other_vpc,
            Vpc::Third => &self.third_vpc,
        }
    }

    /// The id of the named group `target` names.
    fn target_id(&self, target: Target) -> &str {
        match target {
            Target::First => &self.first,
            Target::Second => &self.second,
            Target::Shared => &self.shared,
        }
    }
}

/// Write the three VPCs and the groups the rows reference.
fn world(engine: &engine::InMemoryEngine) -> World {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let mut vpc = |cidr: &str| {
        let vpc_id = store.insert_vpc(cidr, false, "default", "available", REGION, Vec::new());
        // Every VPC comes with its default group, whose inbound rule references the group
        // ITSELF — a reference inside its own VPC, so it is never a reference to report.
        store.insert_default_security_group(&vpc_id, REGION);
        vpc_id
    };
    let named_vpc = vpc("10.0.0.0/16");
    let other_vpc = vpc("10.1.0.0/16");
    let third_vpc = vpc("10.2.0.0/16");

    let mut group = |vpc_id: &str, name: &str| {
        store.insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.to_string(),
            group_name: name.to_string(),
            description: format!("the group {name}"),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: Vec::new(),
            ip_permissions_egress: Vec::new(),
        })
    };
    let first = group(&named_vpc, "first-referenced");
    let second = group(&named_vpc, "second-referenced");
    let shared = group(&named_vpc, SHARED_NAME);
    let unreferenced = group(&named_vpc, "referenced-by-nobody");
    // A group of the OTHER VPC carrying the SAME name as one of the named groups: a rule of
    // that VPC naming the name resolves HERE, not across the VPC boundary.
    group(&other_vpc, SHARED_NAME);

    World {
        named_vpc,
        other_vpc,
        third_vpc,
        first,
        second,
        shared,
        unreferenced,
    }
}

/// Write one row's group and its rule, and answer the `GroupId` it was given.
fn seed_row(engine: &engine::InMemoryEngine, world: &World, index: usize, row: &Row) -> String {
    let account_id = engine::emulator_account_id().to_string();
    let (pairs, ranges) = match row.reference {
        Reference::ById(target) => (
            vec![UserIdGroupPair {
                group_id: Some(world.target_id(target).to_string()),
                user_id: Some(account_id),
                vpc_peering_connection_id: row.peering.map(str::to_string),
                ..Default::default()
            }],
            Vec::new(),
        ),
        Reference::ByName(name) => (
            vec![UserIdGroupPair {
                group_name: Some(name.to_string()),
                user_id: Some(account_id),
                vpc_peering_connection_id: row.peering.map(str::to_string),
                ..Default::default()
            }],
            Vec::new(),
        ),
        Reference::Cidr(cidr) => (
            Vec::new(),
            vec![IpRange {
                cidr_ip: Some(cidr.to_string()),
                ..Default::default()
            }],
        ),
    };
    let permission = IpPermission {
        ip_protocol: Some(row.protocol.to_string()),
        from_port: row.ports.map(|(from, _)| from),
        to_port: row.ports.map(|(_, to)| to),
        user_id_group_pairs: Some(pairs),
        ip_ranges: Some(ranges),
        ..Default::default()
    };
    let (ingress, egress) = if row.egress {
        (Vec::new(), vec![permission])
    } else {
        (vec![permission], Vec::new())
    };
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    store.insert_security_group(SecurityGroupSpec {
        vpc_id: world.vpc_id(row.vpc).to_string(),
        group_name: format!("referencing-{index}"),
        description: format!("the group of row {index}"),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: ingress,
        ip_permissions_egress: egress,
    })
}

/// A well-formed `VpcPeeringConnectionId` one set of rows records.
const PEERING: &str = "pcx-0123456789abcdef0";

/// A second one, so one VPC can reach the same group over two connections.
const OTHER_PEERING: &str = "pcx-0abcdef0123456789";

/// A third, recorded by a rule of the third VPC.
const THIRD_PEERING: &str = "pcx-00000000000000abc";

/// The table the rule is asserted over: the rows differ in the VPC that holds the
/// referencing rule, the direction of that rule, its protocol and port range, the
/// connection it records, and how it names the group it references.
fn rows() -> Vec<Row> {
    let row = |case: &'static str,
               vpc: Vpc,
               egress: bool,
               protocol: &'static str,
               ports: Option<(i32, i32)>,
               reference: Reference,
               peering: Option<&'static str>| Row {
        case,
        vpc,
        egress,
        protocol,
        ports,
        reference,
        peering,
    };
    vec![
        row(
            "another VPC's inbound rule names the group by id, over a peering connection",
            Vpc::Other,
            false,
            "tcp",
            Some((22, 22)),
            Reference::ById(Target::First),
            Some(PEERING),
        ),
        row(
            "the same VPC and connection again, from an OUTBOUND rule: one reference",
            Vpc::Other,
            true,
            "-1",
            None,
            Reference::ById(Target::First),
            Some(PEERING),
        ),
        row(
            "the same VPC over a SECOND connection: a reference of its own",
            Vpc::Other,
            false,
            "udp",
            Some((53, 53)),
            Reference::ById(Target::First),
            Some(OTHER_PEERING),
        ),
        row(
            "a third VPC names the same group with no connection recorded",
            Vpc::Third,
            false,
            "icmp",
            Some((-1, -1)),
            Reference::ById(Target::First),
            None,
        ),
        row(
            "a third VPC names the second group over a connection",
            Vpc::Third,
            true,
            "tcp",
            Some((1024, 65535)),
            Reference::ById(Target::Second),
            Some(THIRD_PEERING),
        ),
        row(
            "a rule of the named group's OWN VPC names it by id",
            Vpc::Named,
            false,
            "tcp",
            Some((443, 443)),
            Reference::ById(Target::First),
            None,
        ),
        row(
            "a rule of the named group's OWN VPC names it by name",
            Vpc::Named,
            false,
            "tcp",
            Some((8080, 8080)),
            Reference::ByName(SHARED_NAME),
            None,
        ),
        row(
            "another VPC names that same group NAME, which it has a group of its own for",
            Vpc::Other,
            false,
            "tcp",
            Some((3306, 3306)),
            Reference::ByName(SHARED_NAME),
            None,
        ),
        row(
            "another VPC's rule names a group name nothing answers to",
            Vpc::Other,
            true,
            "tcp",
            Some((9000, 9000)),
            Reference::ByName("never-created"),
            None,
        ),
        row(
            "another VPC's rule names an IPv4 block rather than a group",
            Vpc::Other,
            false,
            "tcp",
            Some((80, 80)),
            Reference::Cidr("198.51.100.0/24"),
            None,
        ),
    ]
}

/// The references the table must produce for `target`: one per VPC and connection among the
/// rows that name it by id from a VPC other than its own.
fn expected(world: &World, rows: &[Row], target: Target) -> BTreeSet<ReferenceRow> {
    rows.iter()
        .filter(|row| matches!(row.reference, Reference::ById(named) if named == target))
        .filter(|row| row.crosses_a_vpc_boundary(world))
        .map(|row| ReferenceRow {
            group_id: world.target_id(target).to_string(),
            referencing_vpc_id: Some(world.vpc_id(row.vpc).to_string()),
            vpc_peering_connection_id: row.peering.map(str::to_string),
            transit_gateway_id: None,
        })
        .collect()
}

impl Row {
    /// Whether this row's rule sits in a VPC other than the one holding the named groups —
    /// the only way its reference reaches one of them across a connection.
    fn crosses_a_vpc_boundary(&self, world: &World) -> bool {
        world.vpc_id(self.vpc) != world.named_vpc
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every other VPC whose rule references a named group is reported, once per VPC and
/// connection, and nothing else is.
#[tokio::test]
async fn every_other_vpc_whose_rule_names_the_group_is_reported_once_per_connection() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let table = rows();
    for (index, row) in table.iter().enumerate() {
        seed_row(&engine, &world, index, row);
    }

    // The request names three groups, in an order of its own: the report is grouped in that
    // order.
    let asked = [Target::Second, Target::First, Target::Shared];
    let params: Vec<(String, String)> = asked
        .iter()
        .enumerate()
        .map(|(index, target)| {
            param(
                &format!("GroupId.{}", index + 1),
                world.target_id(*target),
            )
        })
        .collect();
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "describing three named groups: got {body}");
    let reported = references("three named groups", &body);

    for target in asked {
        let case = format!("{target:?}");
        let expected = expected(&world, &table, target);
        let group_id = world.target_id(target);
        let found: BTreeSet<ReferenceRow> = reported
            .iter()
            .filter(|reference| reference.group_id == group_id)
            .cloned()
            .collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the references {expected:?}, got {found:?}"
        );
    }

    // Each row of the table says for itself what the report must do with it: a rule naming a
    // named group from ANOTHER VPC is in the report, and one that names no group across a VPC
    // boundary contributes no reference from its VPC over its connection.
    let reported_set: BTreeSet<ReferenceRow> = reported.iter().cloned().collect();
    for row in &table {
        let case = row.case;
        let contributed = match row.reference {
            Reference::ById(target) if row.crosses_a_vpc_boundary(&world) => {
                Some(ReferenceRow {
                    group_id: world.target_id(target).to_string(),
                    referencing_vpc_id: Some(world.vpc_id(row.vpc).to_string()),
                    vpc_peering_connection_id: row.peering.map(str::to_string),
                    transit_gateway_id: None,
                })
            }
            _ => None,
        };
        match contributed {
            Some(wanted) => assert!(
                reported_set.contains(&wanted),
                "case {case:?}: expected {wanted:?} among the references, got {reported_set:?}"
            ),
            None => assert!(
                !reported_set.iter().any(|reference| {
                    reference.referencing_vpc_id.as_deref() == Some(world.vpc_id(row.vpc))
                        && reference.vpc_peering_connection_id.as_deref() == row.peering
                }),
                "case {case:?}: this rule reaches no named group across a VPC boundary, yet a \
                 reference names its VPC and connection: {reported_set:?}"
            ),
        }
    }

    // "The ID of the transit gateway (if applicable)"
    // (`SecurityGroupReference$TransitGatewayId`): no operation of this crate stores a VPC
    // attachment to a transit gateway, so nothing the store holds names one.
    for reference in &reported {
        assert!(
            reference.transit_gateway_id.is_none(),
            "no stored reference names a transit gateway, got {reference:?}"
        );
    }

    // The references of one group are reported together, in the order the request named the
    // groups.
    let mut order: Vec<&str> = Vec::new();
    for reference in &reported {
        if order.last() != Some(&reference.group_id.as_str()) {
            order.push(reference.group_id.as_str());
        }
    }
    let wanted: Vec<&str> = asked
        .iter()
        .map(|target| world.target_id(*target))
        .filter(|group_id| reported.iter().any(|row| row.group_id == *group_id))
        .collect();
    assert_eq!(
        order, wanted,
        "the report is grouped in the order the request named the groups, got {order:?}"
    );
}

/// The report is about the groups the request names: a group it does not name is never in
/// it, and a group nothing references is reported on with nothing.
#[tokio::test]
async fn only_the_named_groups_are_reported_on() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let table = rows();
    for (index, row) in table.iter().enumerate() {
        seed_row(&engine, &world, index, row);
    }

    // Rows: one group at a time, a group nothing references, and the same group named
    // twice in one request.
    let cases: Vec<(&str, Vec<Target>)> = vec![
        ("the first group alone", vec![Target::First]),
        ("the second group alone", vec![Target::Second]),
        ("the group nothing references", Vec::new()),
        (
            "the first group named twice",
            vec![Target::First, Target::First],
        ),
    ];
    for (case, targets) in cases {
        let ids: Vec<String> = if targets.is_empty() {
            vec![world.unreferenced.clone()]
        } else {
            targets
                .iter()
                .map(|target| world.target_id(*target).to_string())
                .collect()
        };
        let params: Vec<(String, String)> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| param(&format!("GroupId.{}", index + 1), id))
            .collect();
        let (status, body) = describe(&router, &params).await;
        assert_eq!(status, 200, "case {case:?}: got {body}");
        let reported = references(case, &body);

        let mut wanted: BTreeSet<ReferenceRow> = BTreeSet::new();
        for target in &targets {
            wanted.extend(expected(&world, &table, *target));
        }
        let found: BTreeSet<ReferenceRow> = reported.iter().cloned().collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the references {wanted:?}, got {found:?}"
        );
        // A group named twice is answered once, not twice over.
        assert_eq!(
            reported.len(),
            found.len(),
            "case {case:?}: expected no reference reported twice, got {reported:?}"
        );
    }
}

/// A reference written through the API is reported by this read: the rules
/// `AuthorizeSecurityGroupIngress` and `AuthorizeSecurityGroupEgress` store are the ones
/// the report is computed from.
#[tokio::test]
async fn a_reference_authorized_over_the_wire_is_reported() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);

    // Rows: an inbound grant recording the peering connection it reaches the group over,
    // and an outbound grant from another VPC recording none.
    let cases = [
        (
            "an inbound grant over a peering connection",
            "AuthorizeSecurityGroupIngress",
            Vpc::Other,
            Some(PEERING),
            "tcp",
            5432,
        ),
        (
            "an outbound grant with no connection recorded",
            "AuthorizeSecurityGroupEgress",
            Vpc::Third,
            None,
            "udp",
            5433,
        ),
    ];

    let mut wanted: BTreeSet<ReferenceRow> = BTreeSet::new();
    for (case, action, vpc, peering, protocol, port) in cases {
        let vpc_id = world.vpc_id(vpc).to_string();
        let (status, body) = ec2_query::call(
            &router,
            REGION,
            "CreateSecurityGroup",
            &[
                param("GroupName", &format!("wire-{port}")),
                param("GroupDescription", "a group created over the wire"),
                param("VpcId", &vpc_id),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: the create must succeed: {body}");
        let group_id = ec2_query::element(&body, "groupId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateSecurityGroup reports its group"));

        let mut params = vec![
            param("GroupId", &group_id),
            param("IpPermissions.1.IpProtocol", protocol),
            param("IpPermissions.1.FromPort", &port.to_string()),
            param("IpPermissions.1.ToPort", &port.to_string()),
            param("IpPermissions.1.Groups.1.GroupId", &world.first),
            param("IpPermissions.1.Groups.1.VpcId", &world.named_vpc),
        ];
        if let Some(peering) = peering {
            params.push(param(
                "IpPermissions.1.Groups.1.VpcPeeringConnectionId",
                peering,
            ));
        }
        let (status, body) = ec2_query::call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the authorize must succeed: {body}"
        );

        wanted.insert(ReferenceRow {
            group_id: world.first.clone(),
            referencing_vpc_id: Some(vpc_id),
            vpc_peering_connection_id: peering.map(str::to_string),
            transit_gateway_id: None,
        });

        let (status, body) = describe(&router, &[param("GroupId.1", &world.first)]).await;
        assert_eq!(status, 200, "case {case:?}: the report must succeed: {body}");
        let found: BTreeSet<ReferenceRow> = references(case, &body).into_iter().collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the references {wanted:?}, got {found:?}"
        );
    }
}

/// A group the request names that the account does not have, or that is not a security
/// group id at all, is refused rather than reported on.
#[tokio::test]
async fn a_group_the_account_does_not_have_is_refused() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    seed_row(&engine, &world, 0, &rows()[0]);

    // A group that was created and then deleted, so its id is one the account genuinely
    // had.
    let removed = {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let group_id = store.insert_security_group(SecurityGroupSpec {
            vpc_id: world.named_vpc.clone(),
            group_name: "deleted-group".to_string(),
            description: "a group that gets deleted".to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: Vec::new(),
            ip_permissions_egress: Vec::new(),
        });
        store
            .remove_security_group(&group_id)
            .expect("the group just inserted is removable");
        group_id
    };

    // Rows: the long and the short id form of a group nothing minted, a group the account
    // had, and — for the malformed refusal — free text, another resource's id, and a group
    // id with a digit too few.
    let not_found = [
        ("a long-form group id nothing minted", "sg-0123456789abcdef0".to_string()),
        ("a short-form group id", "sg-12345678".to_string()),
        ("a group the account no longer has", removed),
    ];
    for (case, group_id) in not_found {
        for params in [
            vec![param("GroupId.1", &group_id)],
            // The refusal does not depend on the position the bad id sits at.
            vec![param("GroupId.1", &world.first), param("GroupId.2", &group_id)],
        ] {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {group_id}, got {status} with body {body}"
            );
            assert_eq!(
                ec2_query::error_code(&body),
                "InvalidGroup.NotFound",
                "case {case:?}: expected InvalidGroup.NotFound for {group_id}, got {body}"
            );
            assert_eq!(
                ec2_query::error_message(&body),
                format!("The security group '{group_id}' does not exist"),
                "case {case:?}: expected the refusal to name {group_id}, got {body}"
            );
        }
    }

    let malformed = [
        ("free text", "not-a-group".to_string()),
        ("a VPC id in the GroupId member", world.named_vpc.clone()),
        ("a group id a digit short", "sg-1234567".to_string()),
        ("a group id with a non-hex digit", "sg-0123456789abcdefg".to_string()),
    ];
    for (case, group_id) in malformed {
        let (status, body) = describe(&router, &[param("GroupId.1", &group_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {group_id}, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "InvalidGroupId.Malformed",
            "case {case:?}: expected InvalidGroupId.Malformed for {group_id}, got {body}"
        );
    }
}

/// A request that names no security group is refused rather than reported on: `GroupId` is
/// a required member, so a request that leaves it out never reaches the handler's report.
#[tokio::test]
async fn a_request_naming_no_group_is_refused() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    seed_row(&engine, &world, 0, &rows()[0]);

    // Rows: no `GroupId` field at all, and one carrying an empty value at no list position —
    // "the IDs of the security groups in your account" (`$GroupId`, required) names a group
    // in neither, so the required member is absent either way and the refusal names it.
    for (case, params) in [
        ("no GroupId field", Vec::new()),
        ("a GroupId field at no position", vec![param("GroupId", "")]),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert!(
            ec2_query::error_message(&body).contains("groupId"),
            "case {case:?}: expected the refusal to name the groupId member, got {body}"
        );
        assert!(
            references(case, &body).is_empty(),
            "case {case:?}: a refused request reports no reference, got {body}"
        );
    }
}

/// A dry run reports the outcome and reads nothing; reporting twice reports the same thing,
/// so this read cannot change what a later caller sees.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_a_report_changes_nothing() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let table = rows();
    for (index, row) in table.iter().enumerate() {
        seed_row(&engine, &world, index, row);
    }

    let asked = vec![
        param("GroupId.1", &world.first),
        param("GroupId.2", &world.second),
    ];
    let (status, body) = describe(&router, &asked).await;
    assert_eq!(status, 200, "the first report: got {body}");
    let first = references("the first report", &body);
    assert!(
        !first.is_empty(),
        "the seeded groups are referenced, got {first:?}"
    );
    let held = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        (guard.security_groups.len(), guard.security_group_rules.len())
    };

    // Rows: a dry run on a group the account has, on one it does not, and on a value that
    // is not a group id — "if you have the required permissions, the error response is
    // DryRunOperation" (`$DryRun`), and every caller of this emulator has them, so the
    // outcome is reported before the request's ids are resolved.
    for (case, group_id) in [
        ("a group of the account", world.first.clone()),
        ("a group the account does not have", "sg-0123456789abcdef0".to_string()),
        ("a value that is not a group id", "not-a-group".to_string()),
    ] {
        let params = vec![param("GroupId.1", &group_id), param("DryRun", "true")];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }

    let (status, body) = describe(&router, &asked).await;
    assert_eq!(status, 200, "the second report: got {body}");
    let second = references("the second report", &body);
    assert_eq!(
        second, first,
        "a report is a read: expected {first:?}, got {second:?}"
    );
    let after = {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        (guard.security_groups.len(), guard.security_group_rules.len())
    };
    assert_eq!(
        after, held,
        "a report writes nothing: expected {held:?} groups and rules, got {after:?}"
    );
}
