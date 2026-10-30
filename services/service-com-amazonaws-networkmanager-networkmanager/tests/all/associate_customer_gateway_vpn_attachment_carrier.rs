// SPDX-License-Identifier: CC0-1.0

//! `AssociateCustomerGateway` pinned against the precondition its own documentation states
//! on WHICH customer gateways a global network may be associated with, driven over HTTP.
//!
//! The rule under test: "You can only associate customer gateways that are connected to a
//! VPN attachment on a transit gateway or core network registered in your global network"
//! (`smithy_query query="doc" shape="AssociateCustomerGateway"`). A VPN connection is a
//! Regional resource, so the transit gateway it attaches to — and the core network EDGE a
//! site-to-site VPN attachment of it sits at — live in the Region the customer gateway
//! itself lives in, which only the gateway's ARN names. So the associate is served exactly
//! where the global network carries, IN THAT REGION, a registered transit gateway or a core
//! network whose policy in force configures an edge, and is otherwise refused with the
//! modelled `ValidationException` (400) having stored nothing. Because the Region decides,
//! an ARN that names none is refused on the member that carried it.
//!
//! The table below spans what the rule generalises over: which KIND of carrier the global
//! network holds (a registered transit gateway, a core network edge, a core network with no
//! policy at all, none), whether that carrier is in the gateway's Region or in another one,
//! how many carriers are held, and whether the gateway belongs to the account the global
//! network does — the account is not part of the rule, since a VPN connection belongs to
//! the account that owns the customer gateway while the carrier belongs to the global
//! network's.
//!
//! `customer_gateway_association_lifecycle` covers the slice's round trip and its
//! field-level refusals of the ARN's FORM; what this suite adds is the stored state the
//! associate stands on. Every call goes through the real `MultiServiceRouter`, and nothing
//! reaches into `crate::state`: a tie is read back only through
//! `GetCustomerGatewayAssociations`.

use serde_json::json;

use crate::attachment_lifecycle::policy_document;
use crate::core_network_lifecycle::create_core_network;
use crate::customer_gateway_association_lifecycle::{
    associate, get_associations, read_one, register_vpn_carrier,
};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_association_lifecycle::associate_ok as associate_link_ok;
use crate::link_lifecycle::{create_link_ok, plain_link};

/// The Region the customer gateway of every row lives in: one no other resource of the
/// fixture sits in, so only the carriers a row asks for can satisfy the precondition.
const GATEWAY_REGION: &str = "eu-central-1";

/// A Region that is NOT the gateway's, for the rows whose carrier sits in the wrong one.
const OTHER_REGION: &str = "ap-southeast-4";

/// The account the customer gateways of most rows belong to, which is NOT the account this
/// emulator owns its own resources in (`RegisterTransitGateway` accepts a gateway of the
/// global network's account only, so every carrier below is the emulator's).
const FOREIGN_ACCOUNT: &str = "123456789012";

/// A customer gateway ARN of `region` and `account`. The form is the EC2 one this model
/// spells out for the EC2 ARNs it does type
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`,
/// `smithy_query shape=VpnConnectionArn`), with the `customer-gateway` resource type and a
/// `cgw-` id, so every row satisfies the modelled pattern and reaches the handler.
fn cgw_arn(region: &str, account: &str, hex: &str) -> String {
    format!("arn:aws:ec2:{region}:{account}:customer-gateway/cgw-{hex}")
}

/// One thing a global network can carry that a VPN connection could be attached to.
#[derive(Clone, Copy, Debug)]
enum Carrier {
    /// A transit gateway registered with the global network, living in this Region.
    RegisteredTransitGateway(&'static str),
    /// A core network of the global network whose policy in force configures one edge, in
    /// this Region.
    CoreNetworkEdge(&'static str),
    /// A core network of the global network created with NO policy document, so it
    /// configures no edge anywhere.
    CoreNetworkWithoutPolicy,
}

/// One row: what the global network carries, the customer gateway being associated, and
/// whether the associate is served.
struct Row {
    /// Names the case in every assertion message.
    name: &'static str,
    carriers: &'static [Carrier],
    gateway_region: &'static str,
    /// The account the customer gateway belongs to, or `None` for the emulator's own — the
    /// account every carrier a global network holds belongs to.
    gateway_account: Option<&'static str>,
    accepted: bool,
}

/// The account of a row's customer gateway.
fn account_of(row: &Row) -> String {
    row.gateway_account
        .map_or_else(|| engine::emulator_account_id().to_string(), str::to_string)
}

/// The table. A global network holds at most one core network
/// (`state::MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK`), so no row asks for two.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "nothing registered and no core network",
            carriers: &[],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: false,
        },
        Row {
            name: "a transit gateway registered in the gateway's Region",
            carriers: &[Carrier::RegisteredTransitGateway(GATEWAY_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: true,
        },
        Row {
            name: "a transit gateway registered only in another Region",
            carriers: &[Carrier::RegisteredTransitGateway(OTHER_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: false,
        },
        Row {
            name: "a core network configuring an edge in the gateway's Region",
            carriers: &[Carrier::CoreNetworkEdge(GATEWAY_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: true,
        },
        Row {
            name: "a core network configuring an edge only in another Region",
            carriers: &[Carrier::CoreNetworkEdge(OTHER_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: false,
        },
        Row {
            name: "a core network with no policy document at all",
            carriers: &[Carrier::CoreNetworkWithoutPolicy],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: false,
        },
        Row {
            name: "a core network edge elsewhere beside a registration in the Region",
            carriers: &[
                Carrier::CoreNetworkEdge(OTHER_REGION),
                Carrier::RegisteredTransitGateway(GATEWAY_REGION),
            ],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: true,
        },
        Row {
            name: "two registrations, neither in the gateway's Region",
            carriers: &[
                Carrier::RegisteredTransitGateway(OTHER_REGION),
                Carrier::RegisteredTransitGateway("us-west-2"),
            ],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some(FOREIGN_ACCOUNT),
            accepted: false,
        },
        Row {
            name: "a gateway of the global network's own account, carried in its Region",
            carriers: &[Carrier::RegisteredTransitGateway(GATEWAY_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: None,
            accepted: true,
        },
        Row {
            name: "a gateway in the carrier's Region, owned by a third account",
            carriers: &[Carrier::CoreNetworkEdge(GATEWAY_REGION)],
            gateway_region: GATEWAY_REGION,
            gateway_account: Some("210987654321"),
            accepted: true,
        },
    ]
}

/// A device of `network` at a fresh site, with a link of that site associated with it: what
/// an associate needs BESIDE the precondition under test, so a refusal here can only be
/// about the carriers.
struct Subject {
    device: String,
    link: String,
}

/// Build the global network of one row: the device and link above, plus every carrier the
/// row asks for.
async fn fixture(emulator: &Emulator, row: &Row) -> (String, Subject) {
    let network = emulator.create_global_network(row.name).await;
    let site = emulator.create_site(&network, row.name).await;
    let device = create_device_ok(emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(emulator, &network, &site, &plain_link()).await;
    associate_link_ok(emulator, &network, &device, &link).await;

    for carrier in row.carriers {
        match carrier {
            Carrier::RegisteredTransitGateway(region) => {
                register_vpn_carrier(emulator, &network, region).await;
            }
            Carrier::CoreNetworkEdge(region) => {
                let (status, created) = create_core_network(
                    emulator,
                    json!({
                        "GlobalNetworkId": network,
                        "Description": row.name,
                        "PolicyDocument": policy_document(&[region]),
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: CreateCoreNetwork expected 200, got {status} with {created}",
                    row.name
                );
            }
            Carrier::CoreNetworkWithoutPolicy => {
                let (status, created) = create_core_network(
                    emulator,
                    json!({ "GlobalNetworkId": network, "Description": row.name }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: CreateCoreNetwork expected 200, got {status} with {created}",
                    row.name
                );
            }
        }
    }

    (network, Subject { device, link })
}

#[tokio::test]
async fn only_a_global_network_carrying_a_vpn_attachment_in_the_gateways_region_associates_it() {
    for row in rows() {
        let case = row.name;
        let emulator = Emulator::new();
        let (network, subject) = fixture(&emulator, &row).await;
        let arn = cgw_arn(row.gateway_region, &account_of(&row), "0123456789abcdef1");

        let (status, answer) = associate(
            &emulator,
            &network,
            json!({
                "CustomerGatewayArn": arn,
                "DeviceId": subject.device,
                "LinkId": subject.link,
            }),
        )
        .await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from AssociateCustomerGateway, got {status} with {answer}"
            );
            // ROUND TRIP: the stored tie, not the answer above, is what the reader answers.
            let stored = read_one(&emulator, &network, &arn)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the association must be stored"));
            assert_eq!(
                stored["DeviceId"],
                json!(subject.device),
                "case {case:?}: the stored tie must name the device, got {stored}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from AssociateCustomerGateway, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        // No input member is at fault: it is the stored state that carries no VPN
        // attachment the gateway could be connected to, so the refusal carries the
        // catch-all reason and no field entry.
        assert_eq!(
            answer["Reason"],
            json!("Other"),
            "case {case:?}: expected the catch-all Reason, got {answer}"
        );
        assert!(
            answer["Fields"].is_null(),
            "case {case:?}: a state-level refusal names no field, got {answer}"
        );
        let (status, page) = get_associations(&emulator, &network, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCustomerGatewayAssociations expected 200, got {status} with {page}"
        );
        assert_eq!(
            page["CustomerGatewayAssociations"],
            json!([]),
            "case {case:?}: a refused associate must store nothing, got {page}"
        );
    }
}

#[tokio::test]
async fn registering_the_carrier_is_what_turns_the_refusal_into_the_stored_tie() {
    let emulator = Emulator::new();
    let row = Row {
        name: "carrier registered after the refusal",
        carriers: &[],
        gateway_region: GATEWAY_REGION,
        gateway_account: Some(FOREIGN_ACCOUNT),
        accepted: false,
    };
    let case = row.name;
    let (network, subject) = fixture(&emulator, &row).await;
    let arn = cgw_arn(GATEWAY_REGION, FOREIGN_ACCOUNT, "0a1b2c3d4");
    let request = json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device });

    let (status, refused) = associate(&emulator, &network, request.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 while the global network carries nothing, got {status} with {refused}"
    );

    // The one change the refusal was about: a carrier in the gateway's Region. The identical
    // request is then served, which is what shows the refusal stood on stored state alone.
    register_vpn_carrier(&emulator, &network, GATEWAY_REGION).await;
    let (status, created) = associate(&emulator, &network, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 once the carrier is registered, got {status} with {created}"
    );
    let stored = read_one(&emulator, &network, &arn)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the association must be stored"));
    assert_eq!(
        stored["CustomerGatewayArn"],
        json!(arn),
        "case {case:?}: the stored tie must name the gateway asked for, got {stored}"
    );
}

#[tokio::test]
async fn a_customer_gateway_arn_naming_no_region_is_refused_on_that_member() {
    let emulator = Emulator::new();
    let row = Row {
        name: "an ARN naming no Region",
        // Every carrier form at once as far as one global network can hold them, so the
        // refusal cannot be read as "this global network carries nothing".
        carriers: &[
            Carrier::RegisteredTransitGateway(GATEWAY_REGION),
            Carrier::CoreNetworkEdge(GATEWAY_REGION),
        ],
        gateway_region: GATEWAY_REGION,
        gateway_account: Some(FOREIGN_ACCOUNT),
        accepted: false,
    };
    let case = row.name;
    let (network, subject) = fixture(&emulator, &row).await;

    // The Region segment is the only difference between the two ARNs: the EC2 form admits an
    // empty one, and the rule is about the Region, so the member that carried it is at fault.
    let regionless = cgw_arn("", FOREIGN_ACCOUNT, "0123abcd");
    let (status, refused) = associate(
        &emulator,
        &network,
        json!({ "CustomerGatewayArn": regionless, "DeviceId": subject.device }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["Fields"][0]["Name"],
        json!("customerGatewayArn"),
        "case {case:?}: the refusal must name the ARN member, got {refused}"
    );
    assert_eq!(
        refused["Reason"],
        json!("FieldValidationFailed"),
        "case {case:?}: a member at fault is the field reason, got {refused}"
    );

    let (status, page) = get_associations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCustomerGatewayAssociations expected 200, got {status} with {page}"
    );
    assert_eq!(
        page["CustomerGatewayAssociations"],
        json!([]),
        "case {case:?}: a refused associate must store nothing, got {page}"
    );

    // The same ARN WITH a Region the global network carries is served, so the refusal is
    // about the missing Region and nothing else.
    let located = cgw_arn(GATEWAY_REGION, FOREIGN_ACCOUNT, "0123abcd");
    let (status, created) = associate(
        &emulator,
        &network,
        json!({ "CustomerGatewayArn": located, "DeviceId": subject.device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the same ARN carrying a Region must be served, got {status} with {created}"
    );
}
