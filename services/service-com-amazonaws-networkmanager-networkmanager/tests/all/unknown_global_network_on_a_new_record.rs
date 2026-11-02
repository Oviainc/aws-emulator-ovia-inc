// SPDX-License-Identifier: CC0-1.0

//! One refusal for the global network a request stores a NEW record under, over every
//! operation of this service that stores one there.
//!
//! An operation that stores a new record under the global network its URI addresses — a
//! resource of its own, a job, or a tie that carries no id (an association, a registration)
//! — validates that parent reference before anything is stored, so an id that names no
//! stored global network is a FIELD failure on the member that carried it: HTTP 400 with
//! `x-amzn-errortype: ValidationException`, `Reason` = `FieldValidationFailed`, one `Fields`
//! entry whose `Name` is that member in this service's own lowerCamelCase field naming and
//! whose `Message` is the top-level `Message` — "Global network not found." for the global
//! network kind. An operation addressed AT a resource that already exists (a read, an
//! update, a delete, a disassociation) keeps answering the absent resource with
//! `ResourceNotFoundException` (404); that half of the rule is pinned by
//! `crate::resource_not_found_message`, and `crate::unknown_parent_reference` pins this half
//! over the creates and the other reference members each one carries.
//!
//! The rows differ along what the rule generalises over: the operation storing the record
//! (the four associations, the registration, the analysis, and two creates that already
//! answered this way, so the whole family is asserted to agree), the kind of record it
//! stores, the form of the id that names no global network (one never minted, one minted and
//! then deleted, and a string that is not an id of this service at all), and the rest of the
//! request's content, which is the same whether the refusal is reached or not.
//!
//! The control rows drive the identical requests against a global network that IS stored:
//! each is answered past this check — accepted, or refused for something else entirely — so
//! the refusals above turn on the parent reference resolving to nothing and nothing wider.
//! Between the two, the live global network is read back through the reporting operation of
//! each record kind: a refused request stores nothing.

use serde_json::{json, Value};

use crate::customer_gateway_association_lifecycle::plain_cgw_arn;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::transit_gateway_connect_peer_association_lifecycle::connect_peer_arn;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The field name this service reports for a global-network reference that resolves to
/// nothing, in the lowerCamelCase spelling of the request member, and the prose it carries:
/// the spelling and the sentence the recorded refusals of a create and of an association
/// under an unknown global network both answer.
const FIELD_GLOBAL_NETWORK_ID: &str = "globalNetworkId";
/// See [`FIELD_GLOBAL_NETWORK_ID`].
const GLOBAL_NETWORK_NOT_FOUND: &str = "Global network not found.";
/// The modelled `Reason` a refusal of one input member carries (`smithy_query
/// shape=ValidationExceptionReason`: `UnknownOperation`, `CannotParse`,
/// `FieldValidationFailed`, `Other`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// An id of the shape this service mints that was never minted here, for the resources a
/// request names inside the global network it addresses.
const UNKNOWN_DEVICE: &str = "device-0000000000000beef";
/// See [`UNKNOWN_DEVICE`].
const UNKNOWN_LINK: &str = "link-0000000000000beef";
/// See [`UNKNOWN_DEVICE`]: the modelled `ConnectPeerId` pattern is
/// `^connect-peer-([0-9a-f]{8,17})$` (`smithy_query shape=ConnectPeerId`), so a row's id has
/// to satisfy it to reach the handler at all.
const UNKNOWN_CONNECT_PEER: &str = "connect-peer-0000000000000beef";

/// An Amazon VPC transit gateway attachment ARN, the kind of ARN a route analysis endpoint
/// names ("The ARN of the transit gateway attachment", `smithy_query query="doc"
/// shape="RouteAnalysisEndpointOptions" member="TransitGatewayAttachmentArn"`).
fn transit_gateway_attachment_arn(index: usize) -> String {
    format!(
        "arn:aws:ec2:us-west-2:{}:transit-gateway-attachment/tgw-attach-{index:017x}",
        own_account()
    )
}

/// One operation that stores a new record under the global network in its URI: how a row
/// names it, the collection it posts to under that global network, and the body it carries.
struct Storing {
    operation: &'static str,
    collection: &'static str,
    body: Value,
}

/// Every such operation of this service, with a request whose other members are well formed,
/// so each row reaches the parent-reference check rather than an earlier refusal.
fn storing_operations() -> Vec<Storing> {
    vec![
        Storing {
            operation: "AssociateLink",
            collection: "link-associations",
            body: json!({ "DeviceId": UNKNOWN_DEVICE, "LinkId": UNKNOWN_LINK }),
        },
        Storing {
            operation: "AssociateCustomerGateway",
            collection: "customer-gateway-associations",
            body: json!({
                "CustomerGatewayArn": plain_cgw_arn("0123456789abcdef1"),
                "DeviceId": UNKNOWN_DEVICE,
            }),
        },
        Storing {
            operation: "AssociateConnectPeer",
            collection: "connect-peer-associations",
            body: json!({ "ConnectPeerId": UNKNOWN_CONNECT_PEER, "DeviceId": UNKNOWN_DEVICE }),
        },
        Storing {
            operation: "AssociateTransitGatewayConnectPeer",
            collection: "transit-gateway-connect-peer-associations",
            body: json!({
                "TransitGatewayConnectPeerArn":
                    connect_peer_arn("aws", "us-west-2", &own_account(), 9, 17),
                "DeviceId": UNKNOWN_DEVICE,
            }),
        },
        Storing {
            operation: "RegisterTransitGateway",
            collection: "transit-gateway-registrations",
            body: json!({
                "TransitGatewayArn": transit_gateway_arn("aws", "us-west-2", &own_account(), 9, 17),
            }),
        },
        Storing {
            operation: "StartRouteAnalysis",
            collection: "route-analyses",
            body: json!({
                "Source": { "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x31) },
                "Destination": {
                    "TransitGatewayAttachmentArn": transit_gateway_attachment_arn(0x32),
                },
            }),
        },
        // The two creates that already answered this way, asserted beside the rest so the
        // whole family is pinned to ONE refusal: one carrying the reference as a URI path
        // label, one as a JSON body member.
        Storing {
            operation: "CreateSite",
            collection: "sites",
            body: json!({ "Description": "orphan" }),
        },
        Storing {
            operation: "CreateConnection",
            collection: "connections",
            body: json!({ "DeviceId": UNKNOWN_DEVICE, "ConnectedDeviceId": UNKNOWN_DEVICE }),
        },
    ]
}

#[tokio::test]
async fn a_global_network_that_is_not_stored_is_a_field_failure_on_every_operation_storing_under_it()
{
    let emulator = Emulator::new();

    // A global network minted and then deleted: a well-formed id that named a record and
    // names none now.
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, answer) = emulator
        .delete(&format!("/global-networks/{deleted}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {answer}"
    );
    // A live global network for the control rows.
    let live = emulator.create_global_network("kept").await;

    let unresolvable: [(&str, String); 3] = [
        ("never minted", "global-network-00000000000000099".to_string()),
        ("minted and deleted", deleted.clone()),
        ("not an id of this service", "nope".to_string()),
    ];

    let mut answers: Vec<(String, Value)> = Vec::new();
    for row in storing_operations() {
        for (form, id) in &unresolvable {
            let case = format!("{} under a global network {form}", row.operation);
            let (status, refused) = emulator
                .post(
                    &format!("/global-networks/{id}/{}", row.collection),
                    row.body.clone(),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case}: expected ValidationException, got {refused}"
            );
            assert_eq!(
                refused["Reason"].as_str(),
                Some(REASON_FIELD_VALIDATION_FAILED),
                "case {case}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {refused}"
            );
            assert_eq!(
                refused["Message"].as_str(),
                Some(GLOBAL_NETWORK_NOT_FOUND),
                "case {case}: expected Message {GLOBAL_NETWORK_NOT_FOUND:?}, got {refused}"
            );
            assert_eq!(
                refused["Fields"],
                json!([{ "Name": FIELD_GLOBAL_NETWORK_ID, "Message": GLOBAL_NETWORK_NOT_FOUND }]),
                "case {case}: expected one Fields entry naming {FIELD_GLOBAL_NETWORK_ID}, got {refused}"
            );
            answers.push((case, refused));
        }
    }

    // Every one of them is the SAME refusal, whichever operation and whichever form of
    // unresolvable id produced it.
    let (first_case, first) = answers[0].clone();
    for (case, answer) in &answers[1..] {
        assert_eq!(
            (answer["Message"].clone(), answer["Fields"].clone()),
            (first["Message"].clone(), first["Fields"].clone()),
            "case {case}: its Message and Fields must match {first_case}, got {answer}"
        );
    }

    // Not one refused request stored anything: every record kind the rows above asked for
    // is absent from the live global network, which no request has been accepted against
    // yet. A route analysis is read back one id at a time (`GetRouteAnalysis` takes the
    // analysis's own id and this service mints none for a refused start), so the refused
    // starts are read back below instead, through the id a start would have been reported
    // under.
    for (case, uri, list) in [
        (
            "link associations",
            format!("/global-networks/{live}/link-associations?deviceId={UNKNOWN_DEVICE}"),
            "LinkAssociations",
        ),
        (
            "customer gateway associations",
            format!("/global-networks/{live}/customer-gateway-associations"),
            "CustomerGatewayAssociations",
        ),
        (
            "Connect peer associations",
            format!("/global-networks/{live}/connect-peer-associations"),
            "ConnectPeerAssociations",
        ),
        (
            "transit gateway Connect peer associations",
            format!("/global-networks/{live}/transit-gateway-connect-peer-associations"),
            "TransitGatewayConnectPeerAssociations",
        ),
        (
            "transit gateway registrations",
            format!("/global-networks/{live}/transit-gateway-registrations"),
            "TransitGatewayRegistrations",
        ),
        (
            "connections",
            format!("/global-networks/{live}/connections"),
            "Connections",
        ),
        (
            "sites",
            format!("/global-networks/{live}/sites"),
            "Sites",
        ),
    ] {
        let (status, listed) = emulator.get(&uri).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 reading them back, got {status} with {listed}"
        );
        assert_eq!(
            listed[list],
            json!([]),
            "case {case}: a refused request stores nothing, got {listed}"
        );
    }

    // The route analyses: a refused start minted no id, so the first id this service would
    // have issued names no analysis of the live global network either.
    let (status, read) = emulator
        .get(&format!(
            "/global-networks/{live}/route-analyses/route-analysis-00000000000000001"
        ))
        .await;
    assert_eq!(
        status, 404,
        "case route analyses: a refused start stores nothing, so the first id it would have been reported under must name none, got {status} with {read}"
    );
    assert_eq!(
        error_code(&read),
        "ResourceNotFoundException",
        "case route analyses: expected ResourceNotFoundException, got {read}"
    );

    // The control: the identical requests against a global network that IS stored get past
    // this check — accepted, or refused for something else — so none of them is answered
    // with the parent-reference refusal above.
    for row in storing_operations() {
        let case = format!("{} under a live global network", row.operation);
        let (status, answer) = emulator
            .post(
                &format!("/global-networks/{live}/{}", row.collection),
                row.body.clone(),
            )
            .await;
        assert_ne!(
            answer["Message"].as_str(),
            Some(GLOBAL_NETWORK_NOT_FOUND),
            "case {case}: expected the request to get past the parent-reference check, got {status} with {answer}"
        );
        assert_ne!(
            answer["Fields"][0]["Name"].as_str(),
            Some(FIELD_GLOBAL_NETWORK_ID),
            "case {case}: expected no refusal naming {FIELD_GLOBAL_NETWORK_ID}, got {status} with {answer}"
        );
    }
}
