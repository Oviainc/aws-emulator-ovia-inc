// SPDX-License-Identifier: CC0-1.0

//! What `initialVlans.hcxNetworkAclId` may NAME: a network access control list the
//! caller's own Amazon EC2 account already holds, in the VPC the environment is deployed
//! into.
//!
//! It is "A unique ID for a network access control list that the HCX VLAN uses"
//! (`smithy_query(query="shape",shape="InitialVlans")`, member `hcxNetworkAclId`), so the
//! id names an Amazon EC2 resource of the caller's account that Amazon EVS does not create
//! — every VPC comes with its default list and `CreateNetworkAcl` mints the rest — and a
//! network ACL belongs to ONE VPC (`NetworkAcl$VpcId` in Amazon EC2's model), which is the
//! VPC whose subnets it governs. The rule under test, one level above the example, is the
//! one `CreateEnvironment` already follows for every other member of the request that names
//! an existing Amazon EC2 resource of the caller's network — `vpcId`,
//! `serviceAccessSubnetId` and each `serviceAccessSecurityGroups.securityGroups` entry: a
//! member naming an Amazon EC2 resource the account's own EC2 store does not hold in that
//! VPC is refused with the operation's modelled `ValidationException`, whose `fieldList`
//! names that member and whose clause names the id the request gave, and the refused
//! request creates nothing. So:
//!
//! * a create naming a list the account holds in that VPC is served, and the HCX VLAN
//!   reports that id back on every later read;
//! * a create naming an id the account holds no list for — or a list of some OTHER VPC of
//!   the same account — is refused, and no environment is created;
//! * the question is asked wherever the member is SUPPLIED, public HCX VLAN or not, because
//!   the id is stored on the HCX VLAN and reported as `Vlan$networkAclId` either way;
//! * the id the HCX VLAN reports is one Amazon EC2's own `DescribeNetworkAcls` resolves,
//!   which is what makes it a reference to a resource of the account rather than an opaque
//!   string.
//!
//! The rows differ along what the rule generalises over: WHERE the account holds a list
//! (the environment's VPC, another VPC, nowhere), whether the HCX VLAN is public, and the
//! LENGTH and character classes of the id the member carries — the modelled `NetworkAclId`
//! (`smithy_query(query="shape",shape="NetworkAclId")`) is a string of length 4..21 over
//! the pattern `^acl-[a-zA-Z0-9_-]+$`, so the table carries ids at both published bounds
//! beside the ids Amazon EC2's own writers mint.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{
    call, call_in_region, emulator, error_type, register_network_acl, settle, vlans_with_prefix,
    PAST_THE_TRANSITION, REGION,
};
use crate::hcx_eip_associations::hcx_vlan;
use crate::vlan_subnet_propagation::{call_ec2, element, emulator as evs_with_ec2};

/// The input member under test, as the refusal names it in `fieldList`.
const MEMBER: &str = "initialVlans.hcxNetworkAclId";

/// Where the account holds a network access control list, for one row of the table.
#[derive(Debug, Clone, Copy)]
enum Holding {
    /// The environment's own VPC holds one, at the id the request names.
    EnvironmentVpcAtNamedId,
    /// The environment's own VPC holds one, but at this OTHER id.
    EnvironmentVpcAtOtherId(&'static str),
    /// Another VPC of the same account holds one, at the id the request names.
    OtherVpcAtNamedId,
    /// The account holds no network ACL of that id anywhere.
    Nowhere,
}

/// One row: what it is, the id the member carries (none for a request that omits it),
/// where the account holds a list, whether the HCX VLAN is public, and whether the rule
/// refuses the create.
struct Row {
    case: &'static str,
    named: Option<&'static str>,
    holding: Holding,
    public: bool,
    refused: bool,
}

/// The id Amazon EC2's own `CreateNetworkAcl` would mint shapes like: `acl-` and 17
/// lowercase hex digits, the 21 characters the modelled `NetworkAclId` admits at most.
const MINTED_SHAPE_ID: &str = "acl-0123456789abcdef0";

/// The shortest value the modelled `NetworkAclId` admits: length 4..21 over
/// `^acl-[a-zA-Z0-9_-]+$`, so one character after the prefix.
const SHORTEST_ID: &str = "acl-a";

/// The longest value it admits: 21 characters, the published maximum.
const LONGEST_ID: &str = "acl-00112233445566778";

/// A value one character past that maximum, which the boundary validator refuses before
/// the handler sees it — the member is a reference either way.
const PAST_LONGEST_ID: &str = "acl-001122334455667788";

#[tokio::test]
async fn an_hcx_network_acl_the_account_does_not_hold_in_the_vpc_is_refused() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a list of the environment's VPC, public HCX VLAN",
            named: Some(MINTED_SHAPE_ID),
            holding: Holding::EnvironmentVpcAtNamedId,
            public: true,
            refused: false,
        },
        Row {
            case: "a list of the environment's VPC, private HCX VLAN",
            named: Some(MINTED_SHAPE_ID),
            holding: Holding::EnvironmentVpcAtNamedId,
            public: false,
            refused: false,
        },
        Row {
            case: "no list of that id anywhere, public HCX VLAN",
            named: Some(MINTED_SHAPE_ID),
            holding: Holding::Nowhere,
            public: true,
            refused: true,
        },
        Row {
            case: "no list of that id anywhere, private HCX VLAN",
            named: Some(MINTED_SHAPE_ID),
            holding: Holding::Nowhere,
            public: false,
            refused: true,
        },
        Row {
            case: "a list of ANOTHER VPC of the same account",
            named: Some(MINTED_SHAPE_ID),
            holding: Holding::OtherVpcAtNamedId,
            public: true,
            refused: true,
        },
        Row {
            // The id is the key the store answers by, so an id differing only in case
            // names no list of the account.
            case: "an id differing from the held list's in its case alone",
            named: Some("acl-0123456789ABCDEF0"),
            holding: Holding::EnvironmentVpcAtOtherId(MINTED_SHAPE_ID),
            public: true,
            refused: true,
        },
        Row {
            case: "the shortest id the model admits, held in the environment's VPC",
            named: Some(SHORTEST_ID),
            holding: Holding::EnvironmentVpcAtNamedId,
            public: true,
            refused: false,
        },
        Row {
            case: "the shortest id the model admits, held nowhere",
            named: Some(SHORTEST_ID),
            holding: Holding::Nowhere,
            public: true,
            refused: true,
        },
        Row {
            case: "the longest id the model admits, held in the environment's VPC",
            named: Some(LONGEST_ID),
            holding: Holding::EnvironmentVpcAtNamedId,
            public: false,
            refused: false,
        },
        Row {
            case: "the longest id the model admits, held nowhere",
            named: Some(LONGEST_ID),
            holding: Holding::Nowhere,
            public: false,
            refused: true,
        },
        Row {
            case: "one character past the longest id the model admits",
            named: Some(PAST_LONGEST_ID),
            holding: Holding::Nowhere,
            public: false,
            refused: true,
        },
        Row {
            // Nothing is named, so nothing is resolved: a private HCX VLAN needs no list.
            case: "no network ACL id at all, private HCX VLAN",
            named: None,
            holding: Holding::Nowhere,
            public: false,
            refused: false,
        },
        Row {
            // "Required when isHcxPublic is set to true": the presence rule refuses the
            // same member, so a public VLAN with no id draws one clause and not two.
            case: "no network ACL id at all, public HCX VLAN",
            named: None,
            holding: Holding::Nowhere,
            public: true,
            refused: true,
        },
    ];

    for row in rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let mut request = router.create_request(Some("hcx-acl"));
        let vpc_id = request["vpcId"]
            .as_str()
            .expect("a create request names the VPC it is deployed into")
            .to_string();
        let mut vlans = vlans_with_prefix(28);
        if row.public {
            vlans["isHcxPublic"] = json!(true);
        }
        if let Some(named) = row.named {
            vlans["hcxNetworkAclId"] = json!(named);
        }
        request["initialVlans"] = vlans;

        let case = row.case;
        match row.holding {
            Holding::EnvironmentVpcAtNamedId => {
                let named = row.named.expect("the row names the list it holds");
                register_network_acl(&router.engine, &vpc_id, named);
            }
            Holding::EnvironmentVpcAtOtherId(other) => {
                register_network_acl(&router.engine, &vpc_id, other);
            }
            Holding::OtherVpcAtNamedId => {
                let named = row.named.expect("the row names the list it holds");
                let (other_vpc_id, _) = router.fresh_network();
                register_network_acl(&router.engine, &other_vpc_id, named);
            }
            Holding::Nowhere => {}
        }

        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if row.refused {
            // A value past the modelled length is refused at the boundary, which reports
            // the same member without the handler's own clause — and creates nothing
            // either.
            if row.named == Some(PAST_LONGEST_ID) {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
                    error_type(&body)
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(MEMBER),
                    "case {case:?}: expected the refusal to name {MEMBER}, got {message:?}"
                );
                assert_eq!(
                    environment_count(&router).await,
                    0,
                    "case {case:?}: a refused create must not persist an environment"
                );
                continue;
            }
            assert_aggregated_refusal(case, status, &body, &[MEMBER]);
            if let Some(named) = row.named {
                // The clause is built from the request and the store: it names the id the
                // member carried and the VPC the environment is deployed into.
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(named),
                    "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
                );
                assert!(
                    message.contains(&vpc_id),
                    "case {case:?}: expected the refusal to name VPC {vpc_id}, got {message:?}"
                );
            }
            assert_eq!(
                environment_count(&router).await,
                0,
                "case {case:?}: a refused create must not persist an environment"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment_id = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string();

        // The stored HCX VLAN reports the list the create resolved — and reports none when
        // the request named none.
        settle(&router);
        let vlan = hcx_vlan(&router, &environment_id).await;
        let expected = match row.named {
            Some(named) => json!(named),
            None => Value::Null,
        };
        assert_eq!(
            vlan["networkAclId"], expected,
            "case {case:?}: expected the HCX VLAN to report {expected}, got {}",
            vlan["networkAclId"]
        );
    }
}

#[tokio::test]
async fn an_unresolved_network_acl_is_reported_beside_every_other_failed_precondition() {
    // One refusal reports every precondition the request broke, so the network ACL clause
    // travels with the others rather than short-circuiting them.
    let router = emulator();
    let mut request = router.create_request(Some("hcx-acl-aggregated"));
    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    vlans["hcxNetworkAclId"] = json!(MINTED_SHAPE_ID);
    request["initialVlans"] = vlans;
    request["termsAccepted"] = json!(false);

    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_aggregated_refusal(
        "an unheld network ACL together with unaccepted terms",
        status,
        &body,
        &["termsAccepted", MEMBER],
    );
    assert_eq!(
        environment_count(&router).await,
        0,
        "a refused create must not persist an environment"
    );
}

#[tokio::test]
async fn the_list_the_hcx_vlan_reports_is_one_amazon_ec2_resolves() {
    // Both services on ONE shared engine, every call over the wire a client makes it on:
    // the network and its lists are Amazon EC2's own, created through Amazon EC2 itself.
    let router = evs_with_ec2();

    let (status, xml) = call_ec2(&router, REGION, "CreateVpc", &[("CidrBlock", "10.0.0.0/16")]).await;
    assert_eq!(status, 200, "CreateVpc expected 200, got {status} {xml}");
    let vpc_id = element(&xml, "vpcId").unwrap_or_else(|| panic!("no vpcId in {xml}"));

    let (status, xml) = call_ec2(
        &router,
        REGION,
        "CreateSubnet",
        &[
            ("VpcId", &vpc_id),
            ("CidrBlock", "10.0.240.0/24"),
            ("AvailabilityZone", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateSubnet expected 200, got {status} {xml}");
    let subnet_id = element(&xml, "subnetId").unwrap_or_else(|| panic!("no subnetId in {xml}"));

    // The list the VPC itself came with: "Your VPC automatically comes with a modifiable
    // default network ACL", so the account holds one for this VPC without creating any.
    let (status, xml) = call_ec2(
        &router,
        REGION,
        "DescribeNetworkAcls",
        &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeNetworkAcls in {vpc_id}: expected 200, got {status} {xml}"
    );
    let default_acl_id =
        element(&xml, "networkAclId").unwrap_or_else(|| panic!("no networkAclId in {xml}"));

    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    vlans["hcxNetworkAclId"] = json!(default_acl_id);
    let (status, body) = call_in_region(
        &router,
        REGION,
        "CreateEnvironment",
        &json!({
            "environmentName": "hcx-acl-ec2",
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": "SELF_DEPLOYED",
            "termsAccepted": true,
            "initialVlans": vlans,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the VPC's own default network ACL: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment_id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no environmentId in {body}"))
        .to_string();

    // Wait out the transition the clock owes the new environment: a list addressed to one
    // that is still initializing is refused (`tests/all/list_while_initializing.rs`).
    std::thread::sleep(PAST_THE_TRANSITION);
    let (status, listed) = call_in_region(
        &router,
        REGION,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListEnvironmentVlans expected 200, got {status} {listed}"
    );
    let reported = listed["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("no environmentVlans in {listed}"))
        .iter()
        .find(|vlan| vlan["functionName"] == json!("hcx"))
        .cloned()
        .unwrap_or_else(|| panic!("no hcx VLAN in {listed}"));
    assert_eq!(
        reported["networkAclId"],
        json!(default_acl_id),
        "expected the HCX VLAN to report {default_acl_id}, got {}",
        reported["networkAclId"]
    );

    // And Amazon EC2 resolves exactly that id: the VLAN names a list the account holds.
    let network_acl_id = reported["networkAclId"]
        .as_str()
        .unwrap_or_else(|| panic!("the HCX VLAN reports no networkAclId: {reported}"));
    let (status, xml) = call_ec2(
        &router,
        REGION,
        "DescribeNetworkAcls",
        &[("NetworkAclId.1", network_acl_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeNetworkAcls {network_acl_id}: expected 200, got {status} {xml}"
    );
    assert_eq!(
        element(&xml, "networkAclId").as_deref(),
        Some(network_acl_id),
        "expected Amazon EC2 to report the list the HCX VLAN names, got {xml}"
    );

    // An id Amazon EC2 cannot resolve is an id the create refuses: a second network, so
    // the only precondition the request breaks is the one under test.
    let (status, xml) = call_ec2(&router, REGION, "CreateVpc", &[("CidrBlock", "10.1.0.0/16")]).await;
    assert_eq!(status, 200, "second CreateVpc expected 200, got {status} {xml}");
    let other_vpc_id = element(&xml, "vpcId").unwrap_or_else(|| panic!("no vpcId in {xml}"));
    let (status, xml) = call_ec2(
        &router,
        REGION,
        "CreateSubnet",
        &[
            ("VpcId", &other_vpc_id),
            ("CidrBlock", "10.1.240.0/24"),
            ("AvailabilityZone", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "second CreateSubnet expected 200, got {status} {xml}"
    );
    let other_subnet_id = element(&xml, "subnetId").unwrap_or_else(|| panic!("no subnetId in {xml}"));

    let (status, xml) = call_ec2(
        &router,
        REGION,
        "DescribeNetworkAcls",
        &[("NetworkAclId.1", MINTED_SHAPE_ID)],
    )
    .await;
    assert_eq!(
        status, 400,
        "DescribeNetworkAcls {MINTED_SHAPE_ID}: the account holds no such list, got \
         {status} {xml}"
    );

    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    vlans["hcxNetworkAclId"] = json!(MINTED_SHAPE_ID);
    let mut other_vlans = serde_json::Map::new();
    for (member, value) in vlans.as_object().expect("the VLAN plan is an object") {
        // The second VPC's own address space, so every block is a block of it.
        let moved = match value.get("cidr").and_then(Value::as_str) {
            Some(cidr) => json!({ "cidr": cidr.replace("10.0.", "10.1.") }),
            None => value.clone(),
        };
        other_vlans.insert(member.clone(), moved);
    }
    let (status, body) = call_in_region(
        &router,
        REGION,
        "CreateEnvironment",
        &json!({
            "environmentName": "hcx-acl-ec2-absent",
            "vpcId": other_vpc_id,
            "serviceAccessSubnetId": other_subnet_id,
            "vcfVersion": "SELF_DEPLOYED",
            "termsAccepted": true,
            "initialVlans": Value::Object(other_vlans),
        }),
    )
    .await;
    assert_aggregated_refusal(
        "an id Amazon EC2 holds no network ACL for",
        status,
        &body,
        &[MEMBER],
    );
}
