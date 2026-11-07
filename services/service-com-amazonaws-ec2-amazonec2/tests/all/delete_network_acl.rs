// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteNetworkAcl`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteNetworkAcl`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A delete takes EXACTLY the one network ACL its `NetworkAclId` names out of the store
//!    and leaves every other ACL — its entries, its subnet associations and its tags — as
//!    the readers reported it: afterwards the id names nothing (`InvalidNetworkAclID.NotFound`
//!    from every read of it), it is absent from the unnarrowed report, and each remaining ACL
//!    reads back unchanged.
//! 2. A delete the operation's own documented blockers forbid — "You can't delete the ACL if
//!    it's associated with any subnets. You can't delete the default network ACL"
//!    (`DeleteNetworkAcl`) — and one whose id this service cannot resolve, are refused with
//!    the code that names the reason, and the store is left exactly as it was: a refused
//!    delete removes nothing.
//! 3. A `DryRun` of a delete that would have succeeded answers `DryRunOperation` and removes
//!    nothing either, since the flag asks whether the action WOULD succeed rather than for it
//!    (`DeleteNetworkAclRequest$DryRun`).
//!
//! Every table row is a distinct input along what those rules generalise over — how many ACLs
//! the VPC holds, which of them the request names, how many entries and tags the named ACL
//! carries, whether a subnet association was ever recorded against it, and which form an
//! unresolvable id takes — so no single recorded request is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, RULE_ACTION_ALLOW, RULE_ACTION_DENY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child_text, document, param, set_items};

/// The Region every call here is signed for. A network ACL carries no Region member, and the
/// default VPC a fresh account already has is seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// One network ACL as the report carries it, in the members a delete has to leave alone: the
/// ACL's identity, its entries by rule number and direction, the subnets associated with it
/// and its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotAcl {
    network_acl_id: String,
    vpc_id: String,
    is_default: String,
    /// Each entry as `(ruleNumber, egress)`, in the order the report listed them.
    entries: Vec<(String, String)>,
    /// The subnets associated with the ACL, in the order the report listed them.
    associated_subnets: Vec<String>,
    /// Each tag as `(key, value)`.
    tags: Vec<(String, String)>,
}

/// One `DeleteNetworkAcl` call for `network_acl_id`, as its HTTP status and XML body.
async fn delete_acl(
    router: &MultiServiceRouter,
    network_acl_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![param("NetworkAclId", network_acl_id)];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "DeleteNetworkAcl", &params).await
}

/// One `DescribeNetworkAcls` call naming `network_acl_id`, as its status and body.
async fn describe_one(router: &MultiServiceRouter, network_acl_id: &str) -> (u16, String) {
    let params = indexed("NetworkAclId", &[network_acl_id]);
    super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &params).await
}

/// The `Code` and the `Message` of a refusal, read off the TREE of the error envelope
/// (`<Response><Errors><Error><Code/><Message/></Error></Errors></Response>`).
///
/// Parsed rather than sliced out of the bytes: the message names the value the REQUEST sent,
/// so a value carrying XML metacharacters must reach the caller as a well-formed document
/// whose text decodes back to what was sent — a substring read would answer the same for a
/// body that is not a document at all. Every text child is concatenated, since a parser is
/// free to split an element's text around a resolved entity.
fn refusal(case: &str, body: &str) -> (String, String) {
    let parsed = document(case, body);
    let error = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Error")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <Error> in {body}"));
    let text_of = |name: &str| -> String {
        error
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == name)
            .map(|element| {
                element
                    .children()
                    .filter(roxmltree::Node::is_text)
                    .filter_map(|node| node.text())
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    (text_of("Code"), text_of("Message"))
}

/// Every network ACL the account holds, read back through the unnarrowed describe and sorted
/// by id — the store as a caller sees it, which is what a delete may change in exactly one
/// place.
///
/// Read off the TREE: an association carries a `networkAclId` of its own, so only the direct
/// child of a reported ACL is that ACL's id.
async fn snapshot(router: &MultiServiceRouter, case: &str) -> Vec<SnapshotAcl> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reporting every network ACL, got {status} with \
         body {body}"
    );
    let parsed = document(case, &body);
    let mut acls: Vec<SnapshotAcl> = set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .map(|item| SnapshotAcl {
            network_acl_id: child_text(item, "networkAclId").unwrap_or_default(),
            vpc_id: child_text(item, "vpcId").unwrap_or_default(),
            is_default: child_text(item, "default").unwrap_or_default(),
            entries: set_items(item, "entrySet")
                .into_iter()
                .map(|entry| {
                    (
                        child_text(entry, "ruleNumber").unwrap_or_default(),
                        child_text(entry, "egress").unwrap_or_default(),
                    )
                })
                .collect(),
            associated_subnets: set_items(item, "associationSet")
                .into_iter()
                .map(|association| child_text(association, "subnetId").unwrap_or_default())
                .collect(),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect();
    acls.sort_by(|left, right| left.network_acl_id.cmp(&right.network_acl_id));
    acls
}

/// A VPC created over the wire, by id.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateVpc", &[param("CidrBlock", cidr_block)])
            .await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// A subnet of `vpc_id` created over the wire, by id.
async fn create_subnet(router: &MultiServiceRouter, vpc_id: &str, cidr_block: &str) -> String {
    let params = [param("VpcId", vpc_id), param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateSubnet", &params).await;
    assert_eq!(
        status, 200,
        "CreateSubnet {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "subnetId").unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// A network ACL created over the wire in `vpc_id`, carrying `tags`, by id. A created ACL is
/// associated with no subnet, which is what makes it deletable at all.
async fn create_acl(
    router: &MultiServiceRouter,
    vpc_id: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![param("VpcId", vpc_id)];
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "network-acl"));
        for (index, (key, value)) in tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{position}.Value"),
                value,
            ));
        }
    }
    let (status, body) = super::ec2_query::call(router, REGION, "CreateNetworkAcl", &params).await;
    assert_eq!(
        status, 200,
        "CreateNetworkAcl in {vpc_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("CreateNetworkAcl answered no networkAclId: {body}"))
}

/// One entry on `network_acl_id`: the rule number and direction a row names, allowing or
/// denying every protocol from every IPv4 address — the members the delete rule does not
/// generalise over are held fixed.
async fn create_entry(
    router: &MultiServiceRouter,
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
) {
    let rule_action = if egress { RULE_ACTION_DENY } else { RULE_ACTION_ALLOW };
    let params = [
        param("NetworkAclId", network_acl_id),
        param("RuleNumber", &rule_number.to_string()),
        param("Egress", &egress.to_string()),
        param("RuleAction", rule_action),
        param("Protocol", "-1"),
        param("CidrBlock", "0.0.0.0/0"),
    ];
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateNetworkAclEntry", &params).await;
    assert_eq!(
        status, 200,
        "CreateNetworkAclEntry {rule_number} (egress {egress}) on {network_acl_id}: expected \
         HTTP 200, got {status} with body {body}"
    );
}

/// The default network ACL of `vpc_id`, read back through the describe's `default` filter.
async fn default_acl_of(router: &MultiServiceRouter, vpc_id: &str) -> String {
    let params = super::ec2_query::filters(&[("vpc-id", &[vpc_id]), ("default", &["true"])]);
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &params).await;
    assert_eq!(
        status, 200,
        "describing the default ACL of {vpc_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("expected the default network ACL of {vpc_id} in {body}"))
}

/// One row of the delete table: a population of ACLs in one VPC, and which of them the
/// request names.
#[derive(Debug, Clone)]
struct DeleteCase {
    case: &'static str,
    vpc_cidr: &'static str,
    /// How many ACLs the test creates in that VPC, beside the default one it comes with.
    created_acls: usize,
    /// Which of those created ACLs the delete names, counting from 0.
    target: usize,
    /// The entries the named ACL carries, as `(ruleNumber, egress)`.
    entries: &'static [(i32, bool)],
    tags: &'static [(&'static str, &'static str)],
    /// Whether a subnet was associated with the named ACL and then moved off it again: an
    /// ACL that HELD an association is deletable once it holds none.
    association_cleared: bool,
}

/// A delete removes the one ACL its id names and nothing else: the id names nothing
/// afterwards, and every other ACL of the account — the siblings in the same VPC, the VPC's
/// own default ACL and the default VPC's — reads back exactly as it did before.
#[tokio::test]
async fn a_delete_removes_only_the_network_acl_its_id_names() {
    // The rows differ along what the rule generalises over: how many ACLs share the VPC,
    // which one of them is named, and how much the named ACL carries. `RuleNumber`'s
    // published bounds ("Positive integer from 1 to 32766") are among the entry rows.
    let cases = [
        DeleteCase {
            case: "the only created ACL of a VPC, carrying nothing",
            vpc_cidr: "10.10.0.0/16",
            created_acls: 1,
            target: 0,
            entries: &[],
            tags: &[],
            association_cleared: false,
        },
        DeleteCase {
            case: "the first of three ACLs sharing one VPC",
            vpc_cidr: "10.11.0.0/16",
            created_acls: 3,
            target: 0,
            entries: &[(1, false)],
            tags: &[],
            association_cleared: false,
        },
        DeleteCase {
            case: "the last of three ACLs sharing one VPC, with entries in both directions",
            vpc_cidr: "10.12.0.0/16",
            created_acls: 3,
            target: 2,
            entries: &[(100, false), (32766, true)],
            tags: &[],
            association_cleared: false,
        },
        DeleteCase {
            case: "an ACL carrying tags",
            vpc_cidr: "10.13.0.0/16",
            created_acls: 2,
            target: 1,
            entries: &[],
            tags: &[("Name", "edge"), ("tier", "public")],
            association_cleared: false,
        },
        DeleteCase {
            case: "an ACL a subnet was associated with and then moved off",
            vpc_cidr: "10.14.0.0/16",
            created_acls: 2,
            target: 0,
            entries: &[(200, true)],
            tags: &[],
            association_cleared: true,
        },
    ];

    for row in cases {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = create_vpc(&router, row.vpc_cidr).await;
        let mut created = Vec::new();
        for index in 0..row.created_acls {
            let tags = if index == row.target { row.tags } else { &[] };
            created.push(create_acl(&router, &vpc_id, tags).await);
        }
        let target_id = created[row.target].clone();

        // The named ACL carries the row's entries; every sibling carries one of its own, so
        // "the siblings are untouched" is asserted about ACLs that hold caller-written state
        // rather than only the entries a create comes with.
        for (rule_number, egress) in row.entries {
            create_entry(&router, &target_id, *rule_number, *egress).await;
        }
        for (index, acl_id) in created.iter().enumerate() {
            if index != row.target {
                create_entry(&router, acl_id, 150 + index as i32, false).await;
            }
        }

        if row.association_cleared {
            // A subnet is associated with exactly one ACL, so moving it onto the named ACL
            // and back is two store writes. Seeded through the store because the operation
            // that moves an association between ACLs (`ReplaceNetworkAclAssociation`) is
            // still a scaffold with its own open issue.
            let subnet_id = create_subnet(&router, &vpc_id, "10.14.1.0/24").await;
            {
                let state = engine.state::<Ec2State>();
                let mut store = state.lock();
                store.disassociate_subnet_from_network_acls(&subnet_id);
                store
                    .associate_subnet_with_network_acl(&subnet_id, &target_id)
                    .unwrap_or_else(|| panic!("case {case:?}: the store associates the subnet"));
                store.disassociate_subnet_from_network_acls(&subnet_id);
                store
                    .associate_subnet_with_default_network_acl(&subnet_id, &vpc_id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the store associates the subnet with the default ACL")
                    });
            }
        }

        let before = snapshot(&router, case).await;
        assert!(
            before.iter().any(|acl| acl.network_acl_id == target_id),
            "case {case:?}: expected {target_id} among the reported ACLs, got {before:?}"
        );

        let (status, body) = delete_acl(&router, &target_id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {target_id}, got {status} with body {body}"
        );

        // Exactly the named ACL left the store: the rest of the report is byte-for-byte the
        // same rows it carried before.
        let after = snapshot(&router, case).await;
        let expected: Vec<SnapshotAcl> = before
            .iter()
            .filter(|acl| acl.network_acl_id != target_id)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected every ACL but {target_id} to read back unchanged, got \
             {after:?}"
        );

        // And the id names nothing: every read of it answers the not-found refusal, worded
        // with the id the REQUEST sent.
        let (status, body) = describe_one(&router, &target_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 reading {target_id} back, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkAclID.NotFound",
            "case {case:?}: expected InvalidNetworkAclID.NotFound reading {target_id} back, \
             got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The networkAcl ID '{target_id}' does not exist"),
            "case {case:?}: expected the not-found message to name {target_id}, got {body}"
        );

        // A second delete of the same id is refused the same way, so a caller can tell the
        // ACL is gone rather than reading a success for a removal that did nothing.
        let (status, body) = delete_acl(&router, &target_id, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a second delete of {target_id}, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkAclID.NotFound",
            "case {case:?}: expected InvalidNetworkAclID.NotFound from a second delete, got {body}"
        );

        // The VPC the ACL belonged to is untouched: a delete of one of its ACLs is not a
        // delete of the network.
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DescribeVpcs",
            &indexed("VpcId", &[vpc_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the VPC {vpc_id} still readable, got {status} with \
             body {body}"
        );
    }
}

/// One row of the refusal table: the id the delete names, whether it is a dry run, and the
/// answer it must get.
#[derive(Debug, Clone)]
struct RefusalCase {
    case: &'static str,
    /// The `NetworkAclId` to send, or which ACL of the population to name.
    id: RefusedId,
    dry_run: bool,
    status: u16,
    code: &'static str,
}

/// Which id a refusal row sends.
#[derive(Debug, Clone, Copy)]
enum RefusedId {
    /// A literal value, for the ids no ACL of the account could have.
    Literal(&'static str),
    /// The default network ACL of the population's VPC.
    DefaultOfVpc,
    /// The population's ACL a subnet is associated with.
    Associated,
    /// The population's ACL nothing is associated with — deletable, so a dry run of it is a
    /// dry run of a delete that WOULD have succeeded.
    Deletable,
}

/// Every refused delete leaves the store exactly as it was, and is refused with the code
/// that names the reason.
#[tokio::test]
async fn a_refused_delete_leaves_every_network_acl_where_it_was() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.20.0.0/16").await;
    let associated_acl = create_acl(&router, &vpc_id, &[("Name", "associated")]).await;
    let deletable_acl = create_acl(&router, &vpc_id, &[]).await;
    create_entry(&router, &deletable_acl, 120, false).await;
    let default_acl = default_acl_of(&router, &vpc_id).await;
    let subnet_id = create_subnet(&router, &vpc_id, "10.20.1.0/24").await;
    {
        // The subnet is associated with one ACL: moved off the default ACL it was stored
        // against and onto `associated_acl`, which is what makes that ACL's delete a
        // dependency violation. Seeded through the store because
        // `ReplaceNetworkAclAssociation` is still a scaffold with its own open issue.
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        store.disassociate_subnet_from_network_acls(&subnet_id);
        store
            .associate_subnet_with_network_acl(&subnet_id, &associated_acl)
            .expect("the store associates the subnet with the ACL under test");
    }

    let before = snapshot(&router, "the population").await;

    // The malformed rows span the forms an EC2 resource id is read against: an id is
    // `<prefix>-<8 or 17 lowercase hex digits>`, so a missing separator, an empty suffix, a
    // non-hex digit, a suffix one short of the long form, an upper-case prefix and another
    // resource's id are each "not a network ACL id at all" rather than "no such ACL". One row
    // sends a value carrying XML metacharacters: the refusal names the value the request
    // sent, so the response must still be a well-formed document (the reads below parse it)
    // with the value readable back exactly as it was sent.
    let cases = [
        RefusalCase {
            case: "the prefix with no separator",
            id: RefusedId::Literal("acl"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "the prefix with an empty suffix",
            id: RefusedId::Literal("acl-"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "a suffix that is not hexadecimal",
            id: RefusedId::Literal("acl-0123456789abcdefg"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "a suffix one digit short of the long id form",
            id: RefusedId::Literal("acl-0123456789abcdef"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "an upper-case prefix",
            id: RefusedId::Literal("ACL-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "another resource's id",
            id: RefusedId::Literal("vpc-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "a value carrying XML metacharacters",
            id: RefusedId::Literal("acl-<&\"']]>"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.Malformed",
        },
        RefusalCase {
            case: "a well-formed long id of no ACL",
            id: RefusedId::Literal("acl-0123456789abcdef0"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.NotFound",
        },
        RefusalCase {
            case: "a well-formed short id of no ACL",
            id: RefusedId::Literal("acl-12345678"),
            dry_run: false,
            status: 400,
            code: "InvalidNetworkAclID.NotFound",
        },
        RefusalCase {
            case: "the default ACL of a VPC",
            id: RefusedId::DefaultOfVpc,
            dry_run: false,
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "the default ACL of a VPC, as a dry run",
            id: RefusedId::DefaultOfVpc,
            dry_run: true,
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an ACL a subnet is associated with",
            id: RefusedId::Associated,
            dry_run: false,
            status: 400,
            code: "DependencyViolation",
        },
        RefusalCase {
            case: "an ACL a subnet is associated with, as a dry run",
            id: RefusedId::Associated,
            dry_run: true,
            status: 400,
            code: "DependencyViolation",
        },
        RefusalCase {
            case: "a dry run of a delete that would have succeeded",
            id: RefusedId::Deletable,
            dry_run: true,
            status: 412,
            code: "DryRunOperation",
        },
    ];

    for row in cases {
        let case = row.case;
        let network_acl_id = match row.id {
            RefusedId::Literal(value) => value.to_string(),
            RefusedId::DefaultOfVpc => default_acl.clone(),
            RefusedId::Associated => associated_acl.clone(),
            RefusedId::Deletable => deletable_acl.clone(),
        };
        let (status, body) = delete_acl(&router, &network_acl_id, row.dry_run).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {} for {network_acl_id}, got {status} with body {body}",
            row.status
        );
        let (code, message) = refusal(case, &body);
        assert_eq!(
            code, row.code,
            "case {case:?}: expected {} for {network_acl_id}, got {body}",
            row.code
        );
        let expected_message = match row.id {
            // One wording for every unresolvable id, naming the value the request sent.
            RefusedId::Literal(value) if row.code == "InvalidNetworkAclID.Malformed" => {
                format!("Invalid id: \"{value}\"")
            }
            RefusedId::Literal(value) => format!("The networkAcl ID '{value}' does not exist"),
            // AWS's own sentence for this refusal: lower-case start, with the ACL id the
            // request named appended bare.
            RefusedId::DefaultOfVpc => {
                format!("cannot delete default network ACL {network_acl_id}")
            }
            RefusedId::Associated => format!(
                "The networkAcl '{network_acl_id}' has dependencies and cannot be deleted."
            ),
            RefusedId::Deletable => {
                "Request would have succeeded, but DryRun flag is set.".to_string()
            }
        };
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got {body}"
        );

        let after = snapshot(&router, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must remove nothing, got {after:?}"
        );
    }

    // The ACL the rows above only ever dry-ran is still deletable for real, which is what
    // makes that row a dry run of a delete that WOULD have succeeded.
    let (status, body) = delete_acl(&router, &deletable_acl, false).await;
    assert_eq!(
        status, 200,
        "the deletable ACL: expected HTTP 200, got {status} with body {body}"
    );
    let remaining: Vec<String> = snapshot(&router, "after the delete")
        .await
        .into_iter()
        .map(|acl| acl.network_acl_id)
        .collect();
    assert!(
        !remaining.contains(&deletable_acl),
        "the deletable ACL: expected {deletable_acl} gone, got {remaining:?}"
    );
}
