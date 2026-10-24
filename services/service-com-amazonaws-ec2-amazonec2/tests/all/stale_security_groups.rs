// SPDX-License-Identifier: CC0-1.0

//! `DescribeStaleSecurityGroups`, end to end over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The RULE under test: a security group rule is stale exactly while the security group it
//! references cannot be resolved to a live group reachable from the VPC that holds the
//! rule — "rules are stale when they reference a deleted security group … [or] reference a
//! security group in a peer VPC for which the VPC peering connection has been deleted …
//! or if a security group VPC association has been disassociated"
//! (`DescribeStaleSecurityGroups`). A group of the named VPC is reported only when at
//! least one of its rules is stale, with only the stale references of that rule, and a VPC
//! the account does not have is refused rather than reported empty.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the KIND
//! of reference a rule names (a live group by id, that group by name, a deleted group, a
//! name no group of the VPC has, a group of another VPC), the direction the rule sits in,
//! how many references one rule mixes, the protocol and port range it carries, the page
//! size, and the form of a `VpcId` or a `NextToken` that resolves to nothing.

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::ops::types::{IpPermission, IpRange, UserIdGroupPair};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupSpec};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
// The roxmltree readers every suite of this crate shares: one parse that names the case,
// and the child/child-text pair a record is read with.
use super::wire::{child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// One `DescribeStaleSecurityGroups` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "DescribeStaleSecurityGroups", params).await
}

fn p(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_string(), value.into())
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// One reported stale security group, read out of the ec2Query XML. A stale group is a
/// RECORD, not one element, so it is read with a parser rather than by finding a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaleGroupRow {
    group_id: String,
    group_name: Option<String>,
    description: Option<String>,
    vpc_id: Option<String>,
    inbound: Vec<StaleRuleRow>,
    egress: Vec<StaleRuleRow>,
}

/// One reported stale rule of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaleRuleRow {
    protocol: Option<String>,
    from_port: Option<String>,
    to_port: Option<String>,
    /// The references the rule reports as stale, each as [`Reference::identity`] renders
    /// it, in document order.
    references: Vec<String>,
}

/// The `<item>` elements of the `set` member of `node` — a stale group set, a stale rule
/// set or the pair set of one rule.
fn items<'a, 'i>(node: roxmltree::Node<'a, 'i>, set: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    super::wire::child(node, set)
        .map(|set| set.children().filter(roxmltree::Node::is_element).collect())
        .unwrap_or_default()
}

/// The stale groups a response reports, in the order it reports them.
fn stale_groups(case: &str, body: &str) -> Vec<StaleGroupRow> {
    let document = document(case, body);
    items(document.root_element(), "staleSecurityGroupSet")
        .into_iter()
        .map(|group| StaleGroupRow {
            group_id: child_text(group, "groupId").unwrap_or_default(),
            group_name: child_text(group, "groupName"),
            description: child_text(group, "description"),
            vpc_id: child_text(group, "vpcId"),
            inbound: stale_rules(group, "staleIpPermissions"),
            egress: stale_rules(group, "staleIpPermissionsEgress"),
        })
        .collect()
}

/// The stale rules of one direction of one reported group.
fn stale_rules(group: roxmltree::Node<'_, '_>, direction: &str) -> Vec<StaleRuleRow> {
    items(group, direction)
        .into_iter()
        .map(|rule| StaleRuleRow {
            protocol: child_text(rule, "ipProtocol"),
            from_port: child_text(rule, "fromPort"),
            to_port: child_text(rule, "toPort"),
            references: items(rule, "groups")
                .into_iter()
                .map(|pair| match child_text(pair, "groupId") {
                    Some(group_id) => group_id,
                    None => format!(
                        "name:{}",
                        child_text(pair, "groupName").unwrap_or_default()
                    ),
                })
                .collect(),
        })
        .collect()
}

/// The `nextToken` of a response, if it carries one.
fn next_token(case: &str, body: &str) -> Option<String> {
    let document = document(case, body);
    child_text(document.root_element(), "nextToken").filter(|token| !token.is_empty())
}

// ---------------------------------------------------------------------------
// Seeding the store
// ---------------------------------------------------------------------------

/// The kind of security group reference a seeded rule names. Which kinds are STALE is the
/// rule under test, so each kind answers for itself ([`Reference::is_stale`]) and the
/// expected answer is derived from that rather than written out per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// A live group of the VPC under test, named by id.
    Live,
    /// That same live group, named by name instead — "[Default VPC] the name of the
    /// security group" (`UserIdGroupPair$GroupName`).
    LiveByName,
    /// A group of the VPC under test that was created and then DELETED.
    Deleted,
    /// A group name no group of the VPC has.
    MissingName,
    /// A live group of ANOTHER VPC: reachable only across a VPC peering connection, a
    /// transit gateway or a security group VPC association, and the account holds none.
    Peer,
}

impl Reference {
    /// Whether a rule naming this reference is stale: the reference resolves to no live
    /// group of the VPC that holds the rule.
    fn is_stale(self) -> bool {
        match self {
            Self::Live | Self::LiveByName => false,
            Self::Deleted | Self::MissingName | Self::Peer => true,
        }
    }

    /// The pair a rule naming this reference carries.
    fn pair(self, world: &World) -> UserIdGroupPair {
        let by_id = |group_id: &str| UserIdGroupPair {
            group_id: Some(group_id.to_string()),
            user_id: Some(world.account_id.clone()),
            ..Default::default()
        };
        match self {
            Self::Live => by_id(&world.live),
            Self::Deleted => by_id(&world.deleted),
            Self::Peer => by_id(&world.peer),
            Self::LiveByName => UserIdGroupPair {
                group_name: Some(LIVE_GROUP_NAME.to_string()),
                user_id: Some(world.account_id.clone()),
                ..Default::default()
            },
            Self::MissingName => UserIdGroupPair {
                group_name: Some(MISSING_GROUP_NAME.to_string()),
                user_id: Some(world.account_id.clone()),
                ..Default::default()
            },
        }
    }

    /// How a reported pair for this reference reads, as [`stale_rules`] renders one.
    fn identity(self, world: &World) -> String {
        match self {
            Self::Live => world.live.clone(),
            Self::Deleted => world.deleted.clone(),
            Self::Peer => world.peer.clone(),
            Self::LiveByName => format!("name:{LIVE_GROUP_NAME}"),
            Self::MissingName => format!("name:{MISSING_GROUP_NAME}"),
        }
    }
}

/// The name of the live group every row can reference.
const LIVE_GROUP_NAME: &str = "live-reference";

/// A group name no group of the VPC under test has.
const MISSING_GROUP_NAME: &str = "never-created";

/// One rule a seeded group carries.
#[derive(Debug, Clone)]
struct RuleSpec {
    protocol: &'static str,
    /// The port range, for the protocols that have one.
    ports: Option<(i32, i32)>,
    /// The security groups the rule references.
    references: Vec<Reference>,
    /// The IPv4 ranges the rule allows — never stale, so never reported.
    cidrs: Vec<&'static str>,
}

impl RuleSpec {
    fn permission(&self, world: &World) -> IpPermission {
        IpPermission {
            ip_protocol: Some(self.protocol.to_string()),
            from_port: self.ports.map(|(from, _)| from),
            to_port: self.ports.map(|(_, to)| to),
            user_id_group_pairs: Some(
                self.references
                    .iter()
                    .map(|reference| reference.pair(world))
                    .collect(),
            ),
            ip_ranges: Some(
                self.cidrs
                    .iter()
                    .map(|cidr| IpRange {
                        cidr_ip: Some((*cidr).to_string()),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    /// The report this rule must produce, or `None` when nothing about it is stale.
    fn expected(&self, world: &World) -> Option<StaleRuleRow> {
        let references: Vec<String> = self
            .references
            .iter()
            .filter(|reference| reference.is_stale())
            .map(|reference| reference.identity(world))
            .collect();
        (!references.is_empty()).then(|| StaleRuleRow {
            protocol: Some(self.protocol.to_string()),
            from_port: self.ports.map(|(from, _)| from.to_string()),
            to_port: self.ports.map(|(_, to)| to.to_string()),
            references,
        })
    }
}

/// One row of the staleness table: a security group of the VPC under test, as its rules.
#[derive(Debug, Clone)]
struct GroupSpec {
    case: &'static str,
    group_name: &'static str,
    inbound: Vec<RuleSpec>,
    egress: Vec<RuleSpec>,
}

/// The store every test here reads: a VPC under test beside another VPC, a live group, a
/// deleted group, and a group in the other VPC.
struct World {
    /// The VPC the request names.
    vpc: String,
    /// A second VPC of the same account, holding [`World::peer`].
    other_vpc: String,
    /// A live group of [`World::vpc`], named [`LIVE_GROUP_NAME`].
    live: String,
    /// A group of [`World::vpc`] that was created and then removed from the store.
    deleted: String,
    /// A live group of [`World::other_vpc`].
    peer: String,
    account_id: String,
}

/// Write the two VPCs and the three referenced groups, and answer what a row references.
fn world(engine: &engine::InMemoryEngine) -> World {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let vpc = store.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    // Every VPC comes with its default group, whose inbound rule references the group
    // ITSELF: a live reference, so that group must never be reported.
    store.insert_default_security_group(&vpc, REGION);
    let other_vpc =
        store.insert_vpc("10.1.0.0/16", false, "default", "available", REGION, Vec::new());
    store.insert_default_security_group(&other_vpc, REGION);

    let mut group = |vpc_id: &str, name: &str, description: &str| {
        store.insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.to_string(),
            group_name: name.to_string(),
            description: description.to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: Vec::new(),
            ip_permissions_egress: Vec::new(),
        })
    };
    let live = group(&vpc, LIVE_GROUP_NAME, "a live group of the VPC under test");
    let peer = group(&other_vpc, "peer-group", "a live group of another VPC");
    let deleted = group(&vpc, "deleted-group", "a group that gets deleted");
    store
        .remove_security_group(&deleted)
        .expect("the group just inserted is removable");

    World {
        vpc,
        other_vpc,
        live,
        deleted,
        peer,
        account_id: engine::emulator_account_id().to_string(),
    }
}

/// Write one row's group into the VPC under test, and answer its minted `GroupId`.
fn seed_group(engine: &engine::InMemoryEngine, world: &World, row: &GroupSpec) -> String {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    store.insert_security_group(SecurityGroupSpec {
        vpc_id: world.vpc.clone(),
        group_name: row.group_name.to_string(),
        description: format!("{} rules", row.group_name),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: row.inbound.iter().map(|rule| rule.permission(world)).collect(),
        ip_permissions_egress: row.egress.iter().map(|rule| rule.permission(world)).collect(),
    })
}

/// The table the staleness rule is asserted over: the rows differ along the KIND of
/// reference a rule names, the direction it sits in, how many references one rule mixes,
/// the protocol and the port range.
fn group_specs() -> Vec<GroupSpec> {
    let rule = |protocol: &'static str,
                ports: Option<(i32, i32)>,
                references: Vec<Reference>,
                cidrs: Vec<&'static str>| RuleSpec {
        protocol,
        ports,
        references,
        cidrs,
    };
    vec![
        GroupSpec {
            case: "every reference is live, by id and by name",
            group_name: "all-live",
            inbound: vec![rule("tcp", Some((22, 22)), vec![Reference::Live], Vec::new())],
            egress: vec![rule("-1", None, vec![Reference::LiveByName], Vec::new())],
        },
        GroupSpec {
            case: "an inbound rule references a deleted group",
            group_name: "inbound-deleted",
            inbound: vec![rule(
                "tcp",
                Some((443, 443)),
                vec![Reference::Deleted],
                Vec::new(),
            )],
            egress: vec![rule("-1", None, Vec::new(), vec!["0.0.0.0/0"])],
        },
        GroupSpec {
            case: "an inbound rule references a group of another VPC",
            group_name: "inbound-peer",
            inbound: vec![rule("udp", Some((53, 53)), vec![Reference::Peer], Vec::new())],
            egress: Vec::new(),
        },
        GroupSpec {
            case: "only the outbound rule is stale, and it names two references",
            group_name: "egress-only",
            inbound: vec![rule("tcp", Some((80, 80)), vec![Reference::Live], Vec::new())],
            egress: vec![rule(
                "tcp",
                Some((1024, 65535)),
                vec![Reference::Deleted, Reference::Peer],
                Vec::new(),
            )],
        },
        GroupSpec {
            case: "one rule mixes a live reference with two stale ones",
            group_name: "mixed-rule",
            inbound: vec![rule(
                "icmp",
                Some((-1, -1)),
                vec![Reference::Live, Reference::Deleted, Reference::Peer],
                vec!["10.0.0.0/8"],
            )],
            egress: Vec::new(),
        },
        GroupSpec {
            case: "a rule references a group name the VPC has no group for",
            group_name: "missing-name",
            inbound: vec![rule(
                "tcp",
                Some((3306, 3306)),
                vec![Reference::MissingName],
                Vec::new(),
            )],
            egress: Vec::new(),
        },
        GroupSpec {
            case: "a rule references nothing but CIDR blocks",
            group_name: "cidr-only",
            inbound: vec![rule(
                "tcp",
                Some((8080, 8080)),
                Vec::new(),
                vec!["192.168.0.0/16", "172.16.0.0/12"],
            )],
            egress: vec![rule("-1", None, Vec::new(), vec!["0.0.0.0/0"])],
        },
        GroupSpec {
            case: "two stale rules at different protocols and port ranges",
            group_name: "two-stale-rules",
            inbound: vec![
                rule("tcp", Some((22, 22)), vec![Reference::Deleted], Vec::new()),
                rule("udp", Some((123, 123)), vec![Reference::Peer], Vec::new()),
            ],
            egress: Vec::new(),
        },
    ]
}

// ---------------------------------------------------------------------------
// The staleness rule
// ---------------------------------------------------------------------------

/// A group of the named VPC is reported exactly when one of its rules references a
/// security group that resolves to no live group of that VPC, and it is reported with only
/// those references.
#[tokio::test]
async fn a_rule_is_stale_exactly_while_its_reference_resolves_to_no_live_group_of_the_vpc() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let rows = group_specs();
    let seeded: Vec<(GroupSpec, String)> = rows
        .iter()
        .map(|row| (row.clone(), seed_group(&engine, &world, row)))
        .collect();

    let (status, body) = describe(&router, &[p("VpcId", world.vpc.clone())]).await;
    assert_eq!(status, 200, "describing a VPC of the account: got {body}");
    let reported = stale_groups("the whole VPC", &body);

    for (row, group_id) in &seeded {
        let case = row.case;
        let expected_inbound: Vec<StaleRuleRow> = row
            .inbound
            .iter()
            .filter_map(|rule| rule.expected(&world))
            .collect();
        let expected_egress: Vec<StaleRuleRow> = row
            .egress
            .iter()
            .filter_map(|rule| rule.expected(&world))
            .collect();
        let found = reported.iter().find(|group| &group.group_id == group_id);
        if expected_inbound.is_empty() && expected_egress.is_empty() {
            assert!(
                found.is_none(),
                "case {case:?}: a group with no stale rule must not be reported, got {found:?}"
            );
            continue;
        }
        let found = found.unwrap_or_else(|| {
            panic!("case {case:?}: expected {group_id} among the stale groups, got {reported:?}")
        });
        assert_eq!(
            found.inbound, expected_inbound,
            "case {case:?}: expected the stale inbound rules {expected_inbound:?}, got {:?}",
            found.inbound
        );
        assert_eq!(
            found.egress, expected_egress,
            "case {case:?}: expected the stale outbound rules {expected_egress:?}, got {:?}",
            found.egress
        );
        assert_eq!(
            found.vpc_id.as_deref(),
            Some(world.vpc.as_str()),
            "case {case:?}: a stale group reports the VPC that holds it, got {:?}",
            found.vpc_id
        );
        assert_eq!(
            found.group_name.as_deref(),
            Some(row.group_name),
            "case {case:?}: a stale group reports its name, got {:?}",
            found.group_name
        );
        assert_eq!(
            found.description.as_deref(),
            Some(format!("{} rules", row.group_name).as_str()),
            "case {case:?}: a stale group reports its description, got {:?}",
            found.description
        );
    }

    // "The IP ranges. Not applicable for stale security group rules" / "The prefix list
    // IDs. Not applicable" (`StaleIpPermission$IpRanges`, `$PrefixListIds`): the rows above
    // carry CIDR blocks, and no stale rule reports one.
    for member in ["ipRanges", "prefixListIds"] {
        assert!(
            !body.contains(&format!("<{member}>")),
            "case {member:?}: not applicable for stale rules, yet the answer carries one: {body}"
        );
    }

    // The groups whose references are all live — the row above, the live group itself and
    // the VPC's default group, whose inbound rule references the group itself.
    let reported_ids: Vec<&str> = reported
        .iter()
        .map(|group| group.group_id.as_str())
        .collect();
    assert!(
        !reported_ids.contains(&world.live.as_str()),
        "a group with no rules at all is not stale, got {reported_ids:?}"
    );
}

/// The report is about the VPC the request names: a group of another VPC is never in it,
/// even when the other VPC has stale groups of its own.
#[tokio::test]
async fn the_report_covers_only_the_groups_of_the_named_vpc() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let rows = group_specs();
    let seeded: Vec<(GroupSpec, String)> = rows
        .iter()
        .map(|row| (row.clone(), seed_group(&engine, &world, row)))
        .collect();

    // A group of the OTHER VPC with a rule referencing the deleted group: stale on exactly
    // the same terms, and reported only when its own VPC is the one asked about.
    let across = {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        store.insert_security_group(SecurityGroupSpec {
            vpc_id: world.other_vpc.clone(),
            group_name: "stale-in-the-other-vpc".to_string(),
            description: "a stale group of the other VPC".to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: vec![IpPermission {
                ip_protocol: Some("tcp".to_string()),
                from_port: Some(1),
                to_port: Some(1),
                user_id_group_pairs: Some(vec![UserIdGroupPair {
                    group_id: Some(world.deleted.clone()),
                    ..Default::default()
                }]),
                ..Default::default()
            }],
            ip_permissions_egress: Vec::new(),
        })
    };

    for (case, vpc_id, must_hold, must_not_hold) in [
        (
            "the VPC under test",
            world.vpc.clone(),
            seeded
                .iter()
                .filter(|(row, _)| {
                    row.inbound
                        .iter()
                        .chain(row.egress.iter())
                        .any(|rule| rule.expected(&world).is_some())
                })
                .map(|(_, group_id)| group_id.clone())
                .collect::<Vec<String>>(),
            vec![across.clone()],
        ),
        (
            "the other VPC",
            world.other_vpc.clone(),
            vec![across.clone()],
            seeded
                .iter()
                .map(|(_, group_id)| group_id.clone())
                .collect::<Vec<String>>(),
        ),
    ] {
        let (status, body) = describe(&router, &[p("VpcId", vpc_id.clone())]).await;
        assert_eq!(status, 200, "case {case:?}: got {body}");
        let reported: Vec<String> = stale_groups(case, &body)
            .into_iter()
            .map(|group| group.group_id)
            .collect();
        for group_id in must_hold {
            assert!(
                reported.contains(&group_id),
                "case {case:?}: expected {group_id} in the report, got {reported:?}"
            );
        }
        for group_id in must_not_hold {
            assert!(
                !reported.contains(&group_id),
                "case {case:?}: {group_id} belongs to another VPC, got {reported:?}"
            );
        }
    }
}

/// A VPC the account does not have is refused with `InvalidVpcID.NotFound`, whatever form
/// the id a caller sent takes.
#[tokio::test]
async fn a_vpc_the_account_does_not_have_is_refused() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    seed_group(&engine, &world, &group_specs()[1]);

    // A VPC that was created and then removed from the store, so its id is one the account
    // genuinely had.
    let removed = {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let vpc_id =
            store.insert_vpc("10.9.0.0/16", false, "default", "available", REGION, Vec::new());
        store.remove_vpc(&vpc_id).expect("the VPC is removable");
        vpc_id
    };

    // Rows: the long and the short id form, a well-formed id of another resource type, an
    // id of a resource the account HAS but which is not a VPC, and a VPC the account had.
    let cases = [
        ("a long-form VPC id nothing minted", "vpc-0123456789abcdef0".to_string()),
        ("a short-form VPC id", "vpc-12345678".to_string()),
        ("a subnet id in the VpcId member", "subnet-0123456789abcdef0".to_string()),
        ("the id of a live security group", world.live.clone()),
        ("a VPC the account no longer has", removed),
    ];
    for (case, vpc_id) in cases {
        let (status, body) = describe(&router, &[p("VpcId", vpc_id.clone())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {vpc_id}, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "InvalidVpcID.NotFound",
            "case {case:?}: expected InvalidVpcID.NotFound for {vpc_id}, got {body}"
        );
    }
}

/// `MaxResults` cuts the report into pages in `GroupId` order, the token resumes from where
/// the page ended, and the walk reports every stale group exactly once.
#[tokio::test]
async fn max_results_and_next_token_page_the_report_in_group_id_order() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);

    // Seven stale groups: more than the smallest page the model admits, so a page size at
    // that bound takes more than one request.
    let mut expected: Vec<String> = (0..7)
        .map(|index| {
            let state = engine.state::<Ec2State>();
            let mut store = state.lock();
            store.insert_security_group(SecurityGroupSpec {
                vpc_id: world.vpc.clone(),
                group_name: format!("stale-{index}"),
                description: "one of a VPC's stale groups".to_string(),
                region: REGION.to_string(),
                tags: Vec::new(),
                ip_permissions: vec![IpPermission {
                    ip_protocol: Some("tcp".to_string()),
                    from_port: Some(1000 + index),
                    to_port: Some(1000 + index),
                    user_id_group_pairs: Some(vec![UserIdGroupPair {
                        group_id: Some(world.deleted.clone()),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }],
                ip_permissions_egress: Vec::new(),
            })
        })
        .collect();
    expected.sort();

    // Rows: the published bounds of `DescribeStaleSecurityGroupsMaxResults` (range 5..255)
    // and a request that names no page size at all.
    for page_size in [Some(5), Some(255), None] {
        let case = format!("MaxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..expected.len() + 1 {
            let mut params = vec![p("VpcId", world.vpc.clone())];
            if let Some(size) = page_size {
                params.push(p("MaxResults", size.to_string()));
            }
            if let Some(cursor) = &token {
                params.push(p("NextToken", cursor.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(status, 200, "case {case:?}: got {body}");
            let page: Vec<String> = stale_groups(&case, &body)
                .into_iter()
                .map(|group| group.group_id)
                .collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= usize::try_from(size).expect("a positive page size"),
                    "case {case:?}: a page may not exceed MaxResults, got {page:?}"
                );
            }
            let mut sorted = page.clone();
            sorted.sort();
            assert_eq!(
                page, sorted,
                "case {case:?}: a page is reported in GroupId order, got {page:?}"
            );
            walked.extend(page);
            token = next_token(&case, &body);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end without a token, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk reports every stale group exactly once, in GroupId order, \
             got {walked:?}"
        );
    }
}

/// `MaxResults` is accepted at the bounds the model publishes and refused past them.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    seed_group(&engine, &world, &group_specs()[1]);

    // `DescribeStaleSecurityGroupsMaxResults` is modelled `range 5..255`, so the rows are
    // its published bounds and one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (255, 200), (256, 400)] {
        let params = vec![
            p("VpcId", world.vpc.clone()),
            p("MaxResults", max_results.to_string()),
        ];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    let group_id = seed_group(&engine, &world, &group_specs()[1]);

    // Rows: free text, punctuation, hex this service never minted, the bare group id a
    // caller might guess the cursor is, that id hex-encoded, and a forged token carrying
    // this service's marker with a checksum that does not match its cursor.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        group_id.clone(),
        ::hex::encode(&group_id),
        ::hex::encode(format!("ec2-1:0000000000000000:{group_id}")),
    ];
    for token in cases {
        let params = vec![p("VpcId", world.vpc.clone()), p("NextToken", token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// A dry run reports the outcome and reads nothing; describing twice reports the same
/// thing, so this report cannot change what a later caller sees.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_a_report_changes_nothing() {
    let (router, engine) = ec2_query::emulator();
    let world = world(&engine);
    for row in group_specs() {
        seed_group(&engine, &world, &row);
    }

    let (status, body) = describe(&router, &[p("VpcId", world.vpc.clone())]).await;
    assert_eq!(status, 200, "the first report: got {body}");
    let first = stale_groups("the first report", &body);
    assert!(
        !first.is_empty(),
        "the seeded VPC holds stale groups, got {first:?}"
    );
    let held_groups = engine.state::<Ec2State>().lock().security_groups.len();

    // Rows: a dry run on a VPC the account has, and one on a VPC it does not — "if you
    // have the required permissions, the error response is DryRunOperation"
    // (`DescribeStaleSecurityGroupsRequest$DryRun`), and every caller of this emulator has
    // them.
    for (case, vpc_id) in [
        ("a VPC of the account", world.vpc.clone()),
        ("a VPC the account does not have", "vpc-0123456789abcdef0".to_string()),
    ] {
        let params = vec![p("VpcId", vpc_id), p("DryRun", "true")];
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

    let (status, body) = describe(&router, &[p("VpcId", world.vpc.clone())]).await;
    assert_eq!(status, 200, "the second report: got {body}");
    let second = stale_groups("the second report", &body);
    assert_eq!(
        second, first,
        "a report is a read: expected {first:?}, got {second:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().security_groups.len(),
        held_groups,
        "a report writes nothing: expected {held_groups} groups in the store"
    );
}
