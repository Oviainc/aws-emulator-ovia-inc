// SPDX-License-Identifier: CC0-1.0

//! The netmask a VLAN subnet Amazon EVS provisions PUBLIC has to carry.
//!
//! The rule under test, one level above the example: a VLAN subnet an environment
//! provisions public must have exactly a /28 netmask — the public case narrows the
//! /24../28 range every VLAN subnet block is otherwise allowed, to its single smallest
//! value — while a VLAN subnet that is not public keeps the whole range. "If you plan
//! to use a public HCX VLAN subnet, the following requirements must be met: Must have a
//! /28 netmask and be allocated from the IPAM public pool."
//! (`smithy_query(query="doc", shape="InitialVlans", member="hcx")`), against "Amazon
//! EVS VLAN subnets have a minimum CIDR block size of /28 and a maximum size of /24"
//! (`InitialVlans` doc) for the general range.
//!
//! A request that breaks it is refused with the operation's `ValidationException`, whose
//! `fieldList` names the `initialVlans` member whose block is wrong, and creates
//! nothing — exactly as the general range refusal and the `hcxNetworkAclId` refusal do.
//!
//! The rows below differ along what the rule generalises over: whether the VLAN is
//! provisioned public, and the netmask of its block across and past the published range
//! (each published bound, /24 and /28, one past the small end, /29, and one past the
//! large end, /23, plus the sizes in between). An accepted row is read back through
//! `ListEnvironmentVlans`, so what is asserted is the block the environment really
//! provisioned and not just the status code.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{call, emulator, error_type, settle, vlans_with_prefix};

/// The `InitialVlans` member whose VLAN can be provisioned public, and so the member
/// every row varies: "Only the HCX VLAN can be public" — it is the one `isHcxPublic`
/// speaks for.
const PUBLIC_CAPABLE_MEMBER: &str = "hcx";

/// The `fieldList` entry a refused block is reported under.
const PUBLIC_CAPABLE_PATH: &str = "initialVlans.hcx";

/// The network ACL id a public HCX VLAN needs ("Required when isHcxPublic is set to
/// true", `hcxNetworkAclId` doc), in the form Amazon EC2's own id minter gives one. The
/// id also has to name a network ACL the account holds in the environment's VPC, so
/// every public row registers it there (`Emulator::register_hcx_network_acl`) and the
/// only failure such a row can draw is the netmask under test.
const HCX_NETWORK_ACL_ID: &str = "acl-0123456789abcdef0";

/// The first address of the block every row gives the HCX VLAN: its own `/24` of the
/// test VPC's `10.0.0.0/16`, clear of the blocks [`vlans_with_prefix`] hands the other
/// nine VLANs and of the service access subnet — so whatever netmask a row asks for,
/// the block overlaps nothing and the netmask is the only thing under test.
const HCX_BLOCK_BASE: &str = "10.0.14.0";

/// One row: a netmask for the HCX VLAN's block, whether that VLAN is provisioned
/// public, and whether the rule admits the combination.
struct Row {
    case: &'static str,
    public: bool,
    prefix_len: u8,
    accepted: bool,
}

/// The `initialVlans` of a row: the nine other VLANs at the published minimum, and the
/// HCX VLAN's own block at the row's netmask.
fn initial_vlans(row: &Row) -> Value {
    let mut vlans = vlans_with_prefix(28);
    vlans[PUBLIC_CAPABLE_MEMBER] = json!({
        "cidr": format!("{HCX_BLOCK_BASE}/{}", row.prefix_len),
    });
    if row.public {
        vlans["isHcxPublic"] = json!(true);
        vlans["hcxNetworkAclId"] = json!(HCX_NETWORK_ACL_ID);
    }
    vlans
}

/// The VLAN of `environment_id` whose `functionName` is `function`, read back through
/// `ListEnvironmentVlans` — the provisioned VLAN, not the request's own words.
async fn vlan_of(
    router: &crate::environment_lifecycle::Emulator,
    case: &str,
    environment_id: &str,
    function: &str,
) -> Value {
    let (status, body) = call(
        router,
        "ListEnvironmentVlans",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEnvironmentVlans expected 200, got {status} {body}"
    );
    body["environmentVlans"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentVlans in {body}"))
        .iter()
        .find(|vlan| vlan["functionName"] == json!(function))
        .unwrap_or_else(|| panic!("case {case:?}: no {function} VLAN in {body}"))
        .clone()
}

#[tokio::test]
async fn a_public_vlan_subnet_is_a_slash_28_and_a_private_one_the_whole_range() {
    let rows = [
        Row {
            case: "a public HCX VLAN on a /28 is the one netmask the public case allows",
            public: true,
            prefix_len: 28,
            accepted: true,
        },
        Row {
            case: "a public HCX VLAN on a /27",
            public: true,
            prefix_len: 27,
            accepted: false,
        },
        Row {
            case: "a public HCX VLAN on a /26",
            public: true,
            prefix_len: 26,
            accepted: false,
        },
        Row {
            case: "a public HCX VLAN on a /25",
            public: true,
            prefix_len: 25,
            accepted: false,
        },
        Row {
            case: "a public HCX VLAN on a /24, the general maximum",
            public: true,
            prefix_len: 24,
            accepted: false,
        },
        Row {
            case: "a public HCX VLAN on a /29, one past the general minimum",
            public: true,
            prefix_len: 29,
            accepted: false,
        },
        Row {
            case: "a private HCX VLAN on a /28, the general minimum",
            public: false,
            prefix_len: 28,
            accepted: true,
        },
        Row {
            case: "a private HCX VLAN on a /27",
            public: false,
            prefix_len: 27,
            accepted: true,
        },
        Row {
            case: "a private HCX VLAN on a /26",
            public: false,
            prefix_len: 26,
            accepted: true,
        },
        Row {
            case: "a private HCX VLAN on a /24, the general maximum",
            public: false,
            prefix_len: 24,
            accepted: true,
        },
        Row {
            case: "a private HCX VLAN on a /29, one past the general minimum",
            public: false,
            prefix_len: 29,
            accepted: false,
        },
        Row {
            case: "a private HCX VLAN on a /23, one past the general maximum",
            public: false,
            prefix_len: 23,
            accepted: false,
        },
    ];

    for row in &rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let mut request = router.create_request(None);
        request["initialVlans"] = initial_vlans(row);
        // A public row names a network ACL of the account: the create resolves it in
        // Amazon EC2's own store, so the one it names is registered there first and the
        // netmask stays the only thing under test.
        router.register_hcx_network_acl(&request);
        let case = row.case;

        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if !row.accepted {
            assert_aggregated_refusal(case, status, &body, &[PUBLIC_CAPABLE_PATH]);
            let message = body["message"].as_str().unwrap_or_default();
            if row.public {
                // The public case's own requirement: the one netmask it allows, and the
                // netmask the request gave instead.
                assert!(
                    message.contains("/28 netmask"),
                    "case {case:?}: expected the clause to state the /28 netmask a \
                     public VLAN subnet must have, got {message:?}"
                );
            } else {
                assert!(
                    message.contains("between /24 and /28"),
                    "case {case:?}: expected the clause to state the general /24../28 \
                     range, got {message:?}"
                );
            }
            assert!(
                message.contains(&format!("was /{}", row.prefix_len)),
                "case {case:?}: expected the clause to name the netmask the request \
                 gave, got {message:?}"
            );
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

        // The VLAN the environment really provisioned carries the block the request
        // asked for, and is public exactly when the request asked for that.
        settle(&router);
        let vlan = vlan_of(&router, case, &environment_id, PUBLIC_CAPABLE_MEMBER).await;
        let expected_cidr = format!("{HCX_BLOCK_BASE}/{}", row.prefix_len);
        assert_eq!(
            vlan["cidr"],
            json!(expected_cidr),
            "case {case:?}: expected the HCX VLAN to report {expected_cidr}, got {}",
            vlan["cidr"]
        );
        assert_eq!(
            vlan["isPublic"],
            json!(row.public),
            "case {case:?}: expected the HCX VLAN's isPublic to be {}, got {}",
            row.public,
            vlan["isPublic"]
        );
    }
}

#[tokio::test]
async fn the_public_narrowing_reaches_only_the_vlan_that_is_public() {
    // The narrowing belongs to the PUBLIC provisioning of one VLAN subnet, so the other
    // nine VLANs of the very same request keep the whole published /24../28 range: a
    // create whose HCX VLAN is public on its /28 and whose other blocks are larger is
    // accepted, and every one of those blocks is provisioned as asked.
    let router = emulator();
    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    vlans["hcxNetworkAclId"] = json!(HCX_NETWORK_ACL_ID);
    vlans[PUBLIC_CAPABLE_MEMBER] = json!({ "cidr": format!("{HCX_BLOCK_BASE}/28") });
    // Each larger block stays inside the `/24` its member already had, so none of them
    // overlaps another.
    // Each member's name is also the `functionName` its VLAN carries, so one table
    // serves both the request and the read-back.
    let larger = [
        ("vmManagement", "10.0.2.0/26"),
        ("vSan", "10.0.6.0/24"),
        ("nsxUplink", "10.0.12.0/27"),
    ];
    for (member, cidr) in &larger {
        vlans[*member] = json!({ "cidr": cidr });
    }

    let case = "a public HCX /28 beside larger private blocks";
    let mut request = router.create_request(Some("public-hcx-mixed-sizes"));
    request["initialVlans"] = vlans;
    router.register_hcx_network_acl(&request);
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment_id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
        .to_string();

    settle(&router);
    for (function, cidr) in &larger {
        let vlan = vlan_of(&router, case, &environment_id, function).await;
        assert_eq!(
            vlan["cidr"],
            json!(cidr),
            "case {case:?}: expected the {function} VLAN to report {cidr}, got {}",
            vlan["cidr"]
        );
        assert_eq!(
            vlan["isPublic"],
            json!(false),
            "case {case:?}: expected the {function} VLAN to be private, got {}",
            vlan["isPublic"]
        );
    }
    let hcx = vlan_of(&router, case, &environment_id, PUBLIC_CAPABLE_MEMBER).await;
    assert_eq!(
        hcx["isPublic"],
        json!(true),
        "case {case:?}: expected the HCX VLAN to be public, got {}",
        hcx["isPublic"]
    );

    // And the same request with the public VLAN's own block one size larger is refused,
    // which is what makes the acceptance above a statement about the public block.
    let router = emulator();
    let mut vlans = vlans_with_prefix(28);
    vlans["isHcxPublic"] = json!(true);
    vlans["hcxNetworkAclId"] = json!(HCX_NETWORK_ACL_ID);
    vlans[PUBLIC_CAPABLE_MEMBER] = json!({ "cidr": format!("{HCX_BLOCK_BASE}/27") });
    let mut request = router.create_request(Some("public-hcx-too-large"));
    request["initialVlans"] = vlans;
    router.register_hcx_network_acl(&request);
    let (status, body) = call(&router, "CreateEnvironment", &request).await;
    assert_aggregated_refusal(
        "a public HCX /27 beside correct private blocks",
        status,
        &body,
        &[PUBLIC_CAPABLE_PATH],
    );
}
