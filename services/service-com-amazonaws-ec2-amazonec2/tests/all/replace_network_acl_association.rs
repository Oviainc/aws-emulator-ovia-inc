// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ReplaceNetworkAclAssociation`: which network ACL a subnet is
//! associated with, changed and read back through `DescribeNetworkAcls`. Driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. "Changes which network ACL a subnet is associated with" (`ReplaceNetworkAclAssociation`):
//!    the subnet of the association the request names stops being associated with the ACL it
//!    was associated with and is associated with the ACL the request names instead — so the
//!    subnet is reported against exactly ONE ACL afterwards, the association the request
//!    named is reported by no ACL at all, and every other subnet's association is left where
//!    it was.
//! 2. "The ID of the new association" (`$NewAssociationId`): the call answers the id of the
//!    association it wrote — an association id of EC2's own form, which the moved-to ACL
//!    reports against that subnet, and which the next request can name in turn.
//! 3. A network ACL governs the traffic of one VPC (`NetworkAcl$VpcId`), so a subnet can only
//!    be associated with an ACL of its own VPC: a request pairing the two across VPCs is
//!    refused and moves nothing.
//! 4. An id that resolves to nothing is a refusal, not an association made out of nothing:
//!    `InvalidAssociationID.NotFound` for an association id no ACL holds,
//!    `InvalidNetworkAclID.NotFound` for an ACL id the account holds no ACL for, and
//!    `InvalidNetworkAclID.Malformed` for a value that is not a network ACL id at all.
//! 5. A `DryRun` answers `DryRunOperation` and moves nothing; a request with two faults is
//!    refused for the one read first — the ACL id's FORM before the store is read at all,
//!    and the association before the ACL.
//!
//! Each table row is a distinct input along what those rules generalise over — which subnet
//! moves, whether it starts on the ACL its VPC came with or on one the caller created,
//! whether the target is a created ACL, the VPC's own, or the ACL the subnet is ALREADY
//! associated with, and how many subnets the target ACL already holds — so no single
//! recorded request is the specification.

use ec2::state::NETWORK_ACL_ASSOCIATION_ID_PREFIX;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code};
use super::wire::{child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for. A network ACL carries no Region member, and the
/// default VPC a fresh account already has is seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// One association between a network ACL and a subnet, as `DescribeNetworkAcls` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedAssociation {
    association_id: String,
    network_acl_id: String,
    subnet_id: String,
}

/// One reported network ACL, down to the associations it holds.
#[derive(Debug, Clone)]
struct ReportedAcl {
    network_acl_id: String,
    vpc_id: String,
    is_default: bool,
    associations: Vec<ReportedAssociation>,
}

/// Every network ACL the account holds, read off the TREE: an association's members are
/// children of its own `<item>`, so scanning the body for an element name would mix two
/// associations together.
async fn described_acls(router: &MultiServiceRouter, case: &str) -> Vec<ReportedAcl> {
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the network ACLs back, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .map(|acl| ReportedAcl {
            network_acl_id: child_text(acl, "networkAclId").unwrap_or_default(),
            vpc_id: child_text(acl, "vpcId").unwrap_or_default(),
            is_default: child_text(acl, "default").as_deref() == Some("true"),
            associations: set_items(acl, "associationSet")
                .into_iter()
                .map(|association| ReportedAssociation {
                    association_id: child_text(association, "networkAclAssociationId")
                        .unwrap_or_default(),
                    network_acl_id: child_text(association, "networkAclId").unwrap_or_default(),
                    subnet_id: child_text(association, "subnetId").unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}

/// Every association the account's ACLs report, whichever ACL holds it.
fn all_associations(acls: &[ReportedAcl]) -> Vec<ReportedAssociation> {
    acls.iter()
        .flat_map(|acl| acl.associations.iter().cloned())
        .collect()
}

/// The associations reported for `subnet_id` — a LIST, so a move that left the subnet on two
/// ACLs at once is caught rather than hidden by a search that stops at the first match.
fn associations_of(acls: &[ReportedAcl], subnet_id: &str) -> Vec<ReportedAssociation> {
    all_associations(acls)
        .into_iter()
        .filter(|association| association.subnet_id == subnet_id)
        .collect()
}

/// The one association reported for `subnet_id`, or a panic naming the case: every subnet of
/// a VPC is associated with exactly one network ACL.
fn association_of(acls: &[ReportedAcl], case: &str, subnet_id: &str) -> ReportedAssociation {
    let found = associations_of(acls, subnet_id);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected {subnet_id} to be associated with exactly one network ACL, \
         got {found:?}"
    );
    found.into_iter().next().unwrap_or_else(|| {
        panic!("case {case:?}: a list of one has a first element: {subnet_id}")
    })
}

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// A subnet of `vpc_id` created over the wire, by id. A created subnet is associated with the
/// ACL its VPC came with, which is the association every move below starts from.
async fn create_subnet(router: &MultiServiceRouter, vpc_id: &str, cidr_block: &str) -> String {
    let params = [param("VpcId", vpc_id), param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateSubnet", &params).await;
    assert_eq!(
        status, 200,
        "CreateSubnet {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "subnetId")
        .unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// A network ACL created over the wire in `vpc_id`, by id.
async fn create_acl(router: &MultiServiceRouter, vpc_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateNetworkAcl",
        &[param("VpcId", vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNetworkAcl in {vpc_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("CreateNetworkAcl answered no networkAclId: {body}"))
}

/// The id of the ACL the VPC `vpc_id` came with, as the report names it.
async fn default_acl_of(router: &MultiServiceRouter, vpc_id: &str) -> String {
    let acls = described_acls(router, "the VPC's own network ACL").await;
    acls.iter()
        .find(|acl| acl.vpc_id == vpc_id && acl.is_default)
        .map(|acl| acl.network_acl_id.clone())
        .unwrap_or_else(|| panic!("the VPC {vpc_id} has no default network ACL: {acls:?}"))
}

/// One `ReplaceNetworkAclAssociation` call naming `association_id` and `network_acl_id`, with
/// `extra` carrying whatever else the row under test sends.
async fn replace_association(
    router: &MultiServiceRouter,
    association_id: &str,
    network_acl_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![
        param("AssociationId", association_id),
        param("NetworkAclId", network_acl_id),
    ];
    params.extend_from_slice(extra);
    super::ec2_query::call(router, REGION, "ReplaceNetworkAclAssociation", &params).await
}

/// An id of the same FORM as `id` that the account holds no resource for: a stored id with
/// its last hexadecimal digit changed, so it is well formed and resolves to nothing.
fn unheld_id(id: &str) -> String {
    let mut digits = id.to_string();
    let last = digits
        .pop()
        .unwrap_or_else(|| panic!("a stored id is not empty: {id:?}"));
    digits.push(if last == '0' { '1' } else { '0' });
    digits
}

/// The VPC the moves below happen in: three subnets, the ACL the VPC came with and two ACLs
/// the caller created in it — plus a second VPC with an ACL of its own, which no subnet of
/// the first VPC may be associated with.
struct Fixture {
    vpc_id: String,
    default_acl_id: String,
    created_acl_ids: [String; 2],
    subnet_ids: [String; 3],
    other_vpc_acl_id: String,
}

/// The fixture, built over the wire.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let vpc_id = create_vpc(router, "10.0.0.0/16").await;
    let subnet_ids = [
        create_subnet(router, &vpc_id, "10.0.1.0/24").await,
        create_subnet(router, &vpc_id, "10.0.2.0/24").await,
        create_subnet(router, &vpc_id, "10.0.3.0/24").await,
    ];
    let default_acl_id = default_acl_of(router, &vpc_id).await;
    let created_acl_ids = [
        create_acl(router, &vpc_id).await,
        create_acl(router, &vpc_id).await,
    ];
    let other_vpc_id = create_vpc(router, "10.9.0.0/16").await;
    let other_vpc_acl_id = create_acl(router, &other_vpc_id).await;
    Fixture {
        vpc_id,
        default_acl_id,
        created_acl_ids,
        subnet_ids,
        other_vpc_acl_id,
    }
}

/// Which ACL a row moves a subnet onto.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The ACL the VPC came with.
    VpcDefault,
    /// One of the two ACLs the caller created in the VPC.
    Created(usize),
}

impl Target {
    /// The id the target names in `fixture`.
    fn id(self, fixture: &Fixture) -> String {
        match self {
            Target::VpcDefault => fixture.default_acl_id.clone(),
            Target::Created(index) => fixture.created_acl_ids[index].clone(),
        }
    }
}

/// The move table: which subnet moves and onto which ACL, spanning what a move can be — off
/// the ACL the VPC came with, between two created ACLs, onto an ACL another subnet already
/// uses, back onto the VPC's own, onto the ACL the subnet is ALREADY associated with, and a
/// subnet that has not moved before.
fn move_cases() -> Vec<(&'static str, usize, Target)> {
    vec![
        (
            "off the ACL its VPC came with, onto a created one",
            0,
            Target::Created(0),
        ),
        ("from one created ACL to another", 0, Target::Created(1)),
        (
            "a second subnet onto the created ACL the first already uses",
            1,
            Target::Created(1),
        ),
        (
            "back onto the ACL its VPC came with",
            0,
            Target::VpcDefault,
        ),
        (
            "onto the ACL the subnet is already associated with",
            1,
            Target::Created(1),
        ),
        (
            "a subnet that has not moved before, onto a created ACL",
            2,
            Target::Created(0),
        ),
    ]
}

/// A move takes the subnet off the ACL it was associated with and puts it on the one the
/// request names: one association afterwards, carrying the answered id, and every other
/// subnet left where it was.
#[tokio::test]
async fn a_replacement_moves_the_subnet_onto_the_named_acl() {
    let (router, _engine) = super::ec2_query::emulator();
    let fixture = fixture(&router).await;

    // Where each subnet starts: on the ACL its VPC came with, which is what a created subnet
    // is associated with.
    let acls = described_acls(&router, "before any move").await;
    let mut expected: Vec<String> = fixture
        .subnet_ids
        .iter()
        .map(|subnet_id| {
            let association = association_of(&acls, "before any move", subnet_id);
            assert_eq!(
                association.network_acl_id, fixture.default_acl_id,
                "case {case:?}: expected {subnet_id} to start on the ACL its VPC came with, \
                 got {association:?}",
                case = "before any move"
            );
            fixture.default_acl_id.clone()
        })
        .collect();

    for (case, subnet_index, target) in move_cases() {
        let subnet_id = &fixture.subnet_ids[subnet_index];
        let target_acl_id = target.id(&fixture);
        let acls = described_acls(&router, case).await;
        let before = association_of(&acls, case, subnet_id);

        let (status, body) =
            replace_association(&router, &before.association_id, &target_acl_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let new_association_id = element(&body, "newAssociationId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a newAssociationId, got {body}")
        });
        assert!(
            is_long_id(NETWORK_ACL_ASSOCIATION_ID_PREFIX, &new_association_id),
            "case {case:?}: expected an association id of EC2's own form, got \
             {new_association_id:?}"
        );
        assert_ne!(
            new_association_id, before.association_id,
            "case {case:?}: expected the id of a NEW association, got the one the request \
             named ({new_association_id:?})"
        );

        expected[subnet_index] = target_acl_id.clone();
        let acls = described_acls(&router, case).await;

        // The subnet is associated with the named ACL, once, under the answered id.
        let after = association_of(&acls, case, subnet_id);
        assert_eq!(
            after,
            ReportedAssociation {
                association_id: new_association_id.clone(),
                network_acl_id: target_acl_id.clone(),
                subnet_id: subnet_id.clone(),
            },
            "case {case:?}: expected the subnet on {target_acl_id} under \
             {new_association_id}, got {after:?}"
        );

        // The association the request named is reported by no ACL at all.
        let lingering: Vec<ReportedAssociation> = all_associations(&acls)
            .into_iter()
            .filter(|association| association.association_id == before.association_id)
            .collect();
        assert!(
            lingering.is_empty(),
            "case {case:?}: expected the replaced association {replaced} to be gone, got \
             {lingering:?}",
            replaced = before.association_id
        );

        // Every subnet is still on exactly one ACL — the one it was last moved onto.
        for (index, subnet) in fixture.subnet_ids.iter().enumerate() {
            let association = association_of(&acls, case, subnet);
            assert_eq!(
                association.network_acl_id, expected[index],
                "case {case:?}: expected {subnet} on {want}, got {association:?}",
                want = expected[index]
            );
        }
    }
}

/// The answered id is the one the NEXT request names: a chain of moves driven entirely off
/// what each call answered leaves the subnet on the last ACL of the chain, and only there.
#[tokio::test]
async fn the_answered_id_names_the_association_the_next_move_replaces() {
    let (router, _engine) = super::ec2_query::emulator();
    let fixture = fixture(&router).await;
    let subnet_id = &fixture.subnet_ids[0];

    let acls = described_acls(&router, "the start of the chain").await;
    let mut association_id = association_of(&acls, "the start of the chain", subnet_id).association_id;

    // Each hop names the id the hop before it answered, so nothing here is read back out of
    // the report between moves.
    let chain = [
        ("hop onto the first created ACL", Target::Created(0)),
        ("hop onto the second created ACL", Target::Created(1)),
        ("hop back onto the VPC's own ACL", Target::VpcDefault),
        ("hop onto the first created ACL again", Target::Created(0)),
    ];
    for (case, target) in chain {
        let target_acl_id = target.id(&fixture);
        let (status, body) =
            replace_association(&router, &association_id, &target_acl_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        association_id = element(&body, "newAssociationId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a newAssociationId, got {body}")
        });
    }

    let acls = described_acls(&router, "the end of the chain").await;
    let after = association_of(&acls, "the end of the chain", subnet_id);
    assert_eq!(
        after,
        ReportedAssociation {
            association_id: association_id.clone(),
            network_acl_id: fixture.created_acl_ids[0].clone(),
            subnet_id: subnet_id.clone(),
        },
        "case {case:?}: expected the chain to end on {acl} under {association_id}, got \
         {after:?}",
        case = "the end of the chain",
        acl = fixture.created_acl_ids[0]
    );
}

/// What a replacement refuses, and the fact that a refused request moves nothing.
#[tokio::test]
async fn a_request_that_cannot_be_served_moves_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let fixture = fixture(&router).await;
    let subnet_id = &fixture.subnet_ids[0];
    let acls = described_acls(&router, "before the refusals").await;
    let held = association_of(&acls, "before the refusals", subnet_id);

    // The rows span what a request can get wrong: either id resolving to nothing, an ACL id
    // that is not one at all, and an ACL of another VPC — each with the error code EC2 words
    // that class with.
    let rows: Vec<(&'static str, String, String, &'static str)> = vec![
        (
            "an association id no ACL holds",
            unheld_id(&held.association_id),
            fixture.created_acl_ids[0].clone(),
            "InvalidAssociationID.NotFound",
        ),
        (
            "an ACL id the account holds no ACL for",
            held.association_id.clone(),
            unheld_id(&fixture.created_acl_ids[0]),
            "InvalidNetworkAclID.NotFound",
        ),
        (
            "a value that is not a network ACL id at all",
            held.association_id.clone(),
            "not-a-network-acl".to_string(),
            "InvalidNetworkAclID.Malformed",
        ),
        (
            "an ACL of another VPC",
            held.association_id.clone(),
            fixture.other_vpc_acl_id.clone(),
            "InvalidParameterValue",
        ),
        (
            // Two faults: the ACL id's FORM is read before the store is read at all, so a
            // request that also names an association id no ACL holds is refused for the form.
            "a malformed ACL id beside an association id no ACL holds",
            unheld_id(&held.association_id),
            "acl".to_string(),
            "InvalidNetworkAclID.Malformed",
        ),
        (
            // Two faults: the association is resolved before the ACL, so a request that gets
            // both wrong is refused for the association.
            "an unknown association id beside an unknown ACL id",
            unheld_id(&held.association_id),
            unheld_id(&fixture.created_acl_ids[1]),
            "InvalidAssociationID.NotFound",
        ),
    ];

    for (case, association_id, network_acl_id, want_code) in rows {
        let (status, body) =
            replace_association(&router, &association_id, &network_acl_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, want_code,
            "case {case:?}: expected {want_code}, got {code} with body {body}"
        );

        let acls = described_acls(&router, case).await;
        let after = association_of(&acls, case, subnet_id);
        assert_eq!(
            after, held,
            "case {case:?}: expected a refused request to move nothing, got {after:?}"
        );
    }
}

/// A dry run of a request that would have succeeded answers `DryRunOperation` and leaves the
/// association exactly as it was.
#[tokio::test]
async fn a_dry_run_moves_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let fixture = fixture(&router).await;

    // Both targets a dry run can name: a created ACL and the one the VPC came with — the
    // second being the ACL the subnet is already associated with.
    let targets = [
        ("a dry run onto a created ACL", Target::Created(0)),
        ("a dry run onto the ACL the subnet already uses", Target::VpcDefault),
    ];
    for (case, target) in targets {
        let subnet_id = &fixture.subnet_ids[0];
        let acls = described_acls(&router, case).await;
        let held = association_of(&acls, case, subnet_id);

        let (status, body) = replace_association(
            &router,
            &held.association_id,
            &target.id(&fixture),
            &[param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {code} with body {body}"
        );

        let acls = described_acls(&router, case).await;
        let after = association_of(&acls, case, subnet_id);
        assert_eq!(
            after, held,
            "case {case:?}: expected the dry run to move nothing, got {after:?}"
        );
        assert_eq!(
            after.network_acl_id, fixture.default_acl_id,
            "case {case:?}: expected the subnet to be left on {acl}, got {after:?}",
            acl = fixture.default_acl_id
        );
    }
    // The VPC itself is untouched by a dry run, so the fixture's VPC id is still the one
    // every association above belongs to.
    let acls = described_acls(&router, "after the dry runs").await;
    assert!(
        acls.iter().any(|acl| acl.vpc_id == fixture.vpc_id),
        "case {case:?}: expected the VPC's ACLs to still be reported, got {acls:?}",
        case = "after the dry runs"
    );
}
