// SPDX-License-Identifier: CC0-1.0

//! `DeleteGlobalNetwork` while the global network still holds transit gateway
//! registrations.
//!
//! The rule under test: a delete refuses with the dependency `ValidationException` (HTTP
//! 400) while the resource it names still holds ANY of the dependents its own
//! documentation requires be removed first, counting a tie that carries no id of its own
//! (a registration, an association) exactly like a child resource — "You must first delete
//! all global network objects (devices, links, and sites), deregister all transit
//! gateways, and delete any core networks" (`smithy_query query="doc"
//! shape="DeleteGlobalNetwork"`). So a registered transit gateway blocks the delete, the
//! refusal names it beside every other kind held, and the delete goes through once
//! `DeregisterTransitGateway` has removed the ties.
//!
//! That refusal is also what keeps a registration reachable: it is keyed by its global
//! network plus the gateway ARN, and every operation that could reach it answers
//! not-found for a global network the store no longer holds, while the gateway stays
//! claimed against every other global network. The second test drives that round trip —
//! deregister, delete, register the same gateway elsewhere.
//!
//! The rows span what the rule generalises over: how many registrations are held, which
//! other dependent kinds are held beside them, and the partition, Region, account and id
//! width of the gateway ARN. Every call goes through the real dispatch; a registration is
//! only ever read back through `GetTransitGatewayRegistrations`.

use serde_json::json;

use crate::core_network_lifecycle::create_core_network;
use crate::delete_dependency_refusal::assert_dependency_refusal;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::transit_gateway_peering::transit_gateway_arn;
use crate::transit_gateway_registration_lifecycle::{
    arns_of, deregister, get_registrations, register,
};

/// How the refusal spells a transit gateway registration: the ARN resource type read as
/// words, the way every dependency refusal of this service names the kinds that block it.
const REGISTRATION_LABEL: &str = "transit gateway registration";

/// How the refusal names the resource being deleted.
const PARENT_LABEL: &str = "Global Network";

/// One row: the gateways to register in a fresh global network, the other children to put
/// beside them, and the kinds the refusal must then name.
struct Case {
    name: &'static str,
    /// The transit gateway ARNs to register, spanning the ARN's partition, Region, account
    /// and id width.
    gateways: Vec<String>,
    /// Sites to create in the global network.
    sites: usize,
    /// Devices to create in the global network (unassigned to any site).
    devices: usize,
    /// Whether to create the global network's one core network.
    core_network: bool,
    /// The dependent kinds the refusal must name, in no particular order.
    blocking: &'static [&'static str],
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one registered transit gateway",
            gateways: vec![transit_gateway_arn("aws", "us-west-2", "123456789012", 1, 8)],
            sites: 0,
            devices: 0,
            core_network: false,
            blocking: &[REGISTRATION_LABEL],
        },
        Case {
            name: "three registrations across partitions, Regions and accounts",
            gateways: vec![
                transit_gateway_arn("aws", "eu-central-1", "123456789012", 2, 17),
                transit_gateway_arn("aws-cn", "cn-north-1", "210987654321", 3, 12),
                transit_gateway_arn(
                    "aws-us-gov",
                    "us-gov-west-1",
                    engine::emulator_account_id(),
                    4,
                    16,
                ),
            ],
            sites: 0,
            devices: 0,
            core_network: false,
            blocking: &[REGISTRATION_LABEL],
        },
        Case {
            name: "a registration beside a site",
            gateways: vec![transit_gateway_arn(
                "aws",
                "ap-southeast-2",
                "210987654321",
                5,
                10,
            )],
            sites: 1,
            devices: 0,
            core_network: false,
            blocking: &["site", REGISTRATION_LABEL],
        },
        Case {
            name: "a registration beside a device and a core network",
            gateways: vec![transit_gateway_arn("aws", "us-east-1", "123456789012", 6, 17)],
            sites: 0,
            devices: 1,
            core_network: true,
            blocking: &["device", REGISTRATION_LABEL, "core network"],
        },
        Case {
            name: "no registration at all",
            gateways: Vec::new(),
            sites: 0,
            devices: 0,
            core_network: false,
            blocking: &[],
        },
    ]
}

#[tokio::test]
async fn a_registered_transit_gateway_blocks_the_global_network_delete_until_it_is_deregistered() {
    for case in cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;

        for arn in &case.gateways {
            let (status, created) = register(&emulator, &network, arn).await;
            assert_eq!(
                status, 200,
                "case {name:?}: RegisterTransitGateway({arn}) expected 200, got {status} with {created}"
            );
        }
        let mut sites = Vec::new();
        for index in 0..case.sites {
            sites.push(
                emulator
                    .create_site(&network, &format!("{name}-{index}"))
                    .await,
            );
        }
        let mut devices = Vec::new();
        for _ in 0..case.devices {
            devices.push(create_device_ok(&emulator, &network, json!({})).await);
        }
        let mut core_network = None;
        if case.core_network {
            let (status, created) =
                create_core_network(&emulator, json!({ "GlobalNetworkId": network })).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateCoreNetwork expected 200, got {status} with {created}"
            );
            core_network = Some(
                created["CoreNetwork"]["CoreNetworkId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: no CoreNetworkId in {created}"))
                    .to_string(),
            );
        }

        let (status, body) = emulator.delete(&format!("/global-networks/{network}")).await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a global network holding nothing deletes, got {status} with {body}"
            );
            assert_eq!(
                body["GlobalNetwork"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            continue;
        }

        assert_dependency_refusal(name, PARENT_LABEL, status, &body, case.blocking);

        // The refusal changed nothing: the global network is still described, and every
        // registration it holds is still readable through its own reader.
        let (status, described) = emulator.get("/global-networks").await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeGlobalNetworks expected 200, got {status} with {described}"
        );
        assert!(
            ids_of(&described, "GlobalNetworks", "GlobalNetworkId").contains(&network),
            "case {name:?}: a refused delete must leave the global network stored, got {described}"
        );
        let (status, listed) = get_registrations(&emulator, &network, "").await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetTransitGatewayRegistrations expected 200, got {status} with {listed}"
        );
        let mut expected = case.gateways.clone();
        expected.sort();
        assert_eq!(
            arns_of(&listed),
            expected,
            "case {name:?}: a refused delete must leave every registration readable, got {listed}"
        );

        // Deregistering every gateway removes exactly that clause: a global network still
        // holding other children is refused again, and the refusal no longer names a
        // registration.
        for arn in &case.gateways {
            let (status, removed) = deregister(&emulator, &network, arn).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeregisterTransitGateway({arn}) expected 200, got {status} with {removed}"
            );
        }
        let remaining: Vec<&str> = case
            .blocking
            .iter()
            .copied()
            .filter(|label| *label != REGISTRATION_LABEL)
            .collect();
        if !remaining.is_empty() {
            let (status, again) = emulator.delete(&format!("/global-networks/{network}")).await;
            assert_dependency_refusal(name, PARENT_LABEL, status, &again, &remaining);
        }

        // With the other children gone too, the identical call succeeds: the refusal
        // tracked stored state, not the shape of the request.
        for site in &sites {
            let (status, deleted) = emulator
                .delete(&format!("/global-networks/{network}/sites/{site}"))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteSite expected 200, got {status} with {deleted}"
            );
        }
        for device in &devices {
            let (status, deleted) = emulator
                .delete(&format!("/global-networks/{network}/devices/{device}"))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteDevice expected 200, got {status} with {deleted}"
            );
        }
        if let Some(core_network) = &core_network {
            let (status, deleted) = emulator
                .delete(&format!("/core-networks/{core_network}"))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteCoreNetwork expected 200, got {status} with {deleted}"
            );
        }
        let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with every dependent gone the same delete must succeed, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["GlobalNetwork"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );
    }
}

#[tokio::test]
async fn a_transit_gateway_survives_the_teardown_of_the_global_network_it_was_registered_with() {
    for (index, arn) in [
        transit_gateway_arn("aws", "us-west-2", "123456789012", 7, 8),
        transit_gateway_arn("aws-cn", "cn-northwest-1", "210987654321", 8, 17),
    ]
    .iter()
    .enumerate()
    {
        let case = format!("gateway {index}");
        let emulator = Emulator::new();
        let first = emulator.create_global_network(&case).await;
        let (status, created) = register(&emulator, &first, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: RegisterTransitGateway expected 200, got {status} with {created}"
        );

        // The delete is refused while the registration stands, so the registration never
        // becomes unreachable.
        let (status, blocked) = emulator.delete(&format!("/global-networks/{first}")).await;
        assert_dependency_refusal(&case, PARENT_LABEL, status, &blocked, &[REGISTRATION_LABEL]);

        let (status, removed) = deregister(&emulator, &first, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeregisterTransitGateway expected 200, got {status} with {removed}"
        );
        let (status, deleted) = emulator.delete(&format!("/global-networks/{first}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: with the registration gone the delete must succeed, got {status} with {deleted}"
        );

        // Nothing is left claiming the gateway, so another global network registers it.
        let second = emulator.create_global_network(&format!("{case} again")).await;
        let (status, again) = register(&emulator, &second, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the gateway must be registrable again, got {status} with {again}"
        );
        assert_eq!(
            again["TransitGatewayRegistration"]["GlobalNetworkId"],
            json!(second),
            "case {case:?}: the registration must name the new global network, got {again}"
        );

        // And the deleted global network is gone for every registration operation.
        let (status, orphan) = get_registrations(&emulator, &first, "").await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted global network must be unknown, got {status} with {orphan}"
        );
        assert_eq!(
            error_code(&orphan),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {orphan}"
        );
    }
}
