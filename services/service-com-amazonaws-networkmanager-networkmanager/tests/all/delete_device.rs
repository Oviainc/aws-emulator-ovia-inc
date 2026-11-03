// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteDevice`, driven over HTTP.
//!
//! The rule under test: a delete of a device removes the stored record the path addresses
//! and answers THAT record with `DELETING` for its state, so a later read no longer finds
//! it and a sibling is untouched; a device that is not reachable through the global
//! network in the path — unknown, already deleted, or stored under another global network
//! — is the not-found refusal (404), and "You must first disassociate the device from any
//! links and customer gateways" (`smithy_query query="doc" shape="DeleteDevice"`) makes a
//! device that still holds an association of either kind the dependency
//! `ValidationException` (400) every delete of this service raises, until the associations
//! are gone.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`: a device is only ever read back through `GetDevices`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::connection_lifecycle::without_state;
use crate::customer_gateway_association_lifecycle::register_vpn_carrier;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;

/// The `ResourceType` a not-found refusal reports for each kind this suite addresses: the
/// ARN resource type this service mints.
const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
const DEVICE: &str = "device";

/// A subnet ARN of the form `AWSLocation$SubnetArn` accepts: its modelled pattern is
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:subnet\/subnet-[0-9a-f]{8,17}$`
/// (`smithy_query shape="AWSLocation"`).
const SUBNET_ARN: &str = "arn:aws:ec2:us-west-2:123456789012:subnet/subnet-0123456789abcdef0";

/// `DeleteDevice` for `device` of `network`.
async fn delete_device(emulator: &Emulator, network: &str, device: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/devices/{device}"))
        .await
}

/// The `DeviceId`s `GetDevices` lists for `network`.
async fn device_ids(emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/devices"))
        .await;
    assert_eq!(
        status, 200,
        "GetDevices({network}): expected 200, got {status} with {body}"
    );
    ids_of(&body, "Devices", "DeviceId")
}

/// The one `Device` `GetDevices` answers for `device`, or `None` once it is gone.
async fn read_device(emulator: &Emulator, network: &str, device: &str) -> Option<Value> {
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/devices?deviceIds={device}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetDevices({device}): expected 200, got {status} with {body}"
    );
    body["Devices"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Devices array in {body}"))
        .first()
        .cloned()
}

/// One row of the delete table: the `CreateDevice` body, whether the row assigns the
/// device to a site, and whether the row waits out the settle span the create owes before
/// deleting.
struct DeleteCase {
    name: &'static str,
    sited: bool,
    /// Members beside `SiteId` the create carries.
    body: Value,
    /// Whether the row sleeps past the `PENDING` -> `AVAILABLE` transition first, so the
    /// delete is exercised on a device in either lifecycle state.
    settled: bool,
    /// How many other devices the global network holds, which the delete must leave alone.
    siblings: usize,
}

/// The rows differ along which members the stored record carries (none, a site, the
/// Amazon Web Services location, a physical location, the free-form descriptors, tags),
/// along the lifecycle state the device is in when the delete arrives, and along how many
/// siblings the global network holds — everything the removal rule generalises over.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "a bare unsited device, deleted while still PENDING",
            sited: false,
            body: json!({}),
            settled: false,
            siblings: 0,
        },
        DeleteCase {
            name: "a sited device, deleted once AVAILABLE",
            sited: true,
            body: json!({ "Description": "branch router", "Type": "router" }),
            settled: true,
            siblings: 2,
        },
        DeleteCase {
            name: "every member, beside one sibling",
            sited: true,
            body: json!({
                "AWSLocation": { "Zone": "us-west-2a", "SubnetArn": SUBNET_ARN },
                "Location": { "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" },
                "Description": "core switch",
                "Type": "switch",
                "Vendor": "AnyCompany",
                "Model": "AC-1000",
                "SerialNumber": "SN-0001",
                "Tags": [{ "Key": "env", "Value": "test" }, { "Key": "tier", "Value": "edge" }],
            }),
            settled: false,
            siblings: 1,
        },
        DeleteCase {
            name: "an unsited device carrying only tags, beside three siblings",
            sited: false,
            body: json!({ "Tags": [{ "Key": "owner", "Value": "net-eng" }] }),
            settled: true,
            siblings: 3,
        },
    ]
}

#[tokio::test]
async fn delete_device_removes_the_stored_record_it_reports_for_every_form_of_device() {
    for case in delete_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;

        let mut body = case.body.clone();
        if case.sited {
            body["SiteId"] = json!(site);
        }
        let device = create_device_ok(&emulator, &network, body).await;
        let mut siblings: Vec<String> = Vec::new();
        for index in 0..case.siblings {
            siblings.push(
                create_device_ok(&emulator, &network, json!({ "Description": format!("sibling {index}") }))
                    .await,
            );
        }
        if case.settled {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // What the reader answers for the device before the delete: the delete must report
        // THIS record rather than a shape built from the request.
        let stored = read_device(&emulator, &network, &device)
            .await
            .unwrap_or_else(|| panic!("case {name:?}: GetDevices did not list the created device"));

        let (status, deleted) = delete_device(&emulator, &network, &device).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteDevice expected 200, got {status} with {deleted}"
        );
        assert_eq!(
            without_state(&deleted["Device"]),
            without_state(&stored),
            "case {name:?}: the delete must answer the stored record, got {deleted} for stored {stored}"
        );
        assert_eq!(
            deleted["Device"]["State"],
            json!("DELETING"),
            "case {name:?}: the removed record reports DELETING, got {deleted}"
        );
        // The whole-record comparison above already covers the ARN's text; what the delete
        // must not do is answer an ARN that names some other device, whatever form the
        // service mints ARNs in.
        let arn = deleted["Device"]["DeviceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no DeviceArn in {deleted}"));
        assert!(
            arn.ends_with(&device),
            "case {name:?}: the removed record's ARN must name the device {device}, got {arn:?}"
        );

        // The removal is in stored state: the reader no longer finds it, every sibling is
        // still listed, and a second delete of the same id is the not-found refusal.
        assert!(
            read_device(&emulator, &network, &device).await.is_none(),
            "case {name:?}: GetDevices still lists the deleted device {device}"
        );
        let listed = device_ids(&emulator, &network).await;
        assert_eq!(
            listed, siblings,
            "case {name:?}: expected only the siblings {siblings:?} to remain, got {listed:?}"
        );
        let (status, again) = delete_device(&emulator, &network, &device).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a second DeleteDevice expected 404, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {again}"
        );
    }
}

/// One row of the dependency table: how many links and how many customer gateways the
/// device is associated with before the delete is attempted, and the kinds the refusal
/// must then name.
struct AssociationCase {
    name: &'static str,
    links: usize,
    gateways: usize,
    /// The dependent kinds the refusal must name, as it spells them (the kind read as
    /// words), in no particular order.
    blocking: &'static [&'static str],
}

/// "You must first disassociate the device from any links and customer gateways."
/// (`smithy_query query="doc" shape="DeleteDevice"`) — the rows differ along WHICH of the
/// two kinds of association the device holds and HOW MANY of each, with a control row
/// holding neither.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            name: "no association at all",
            links: 0,
            gateways: 0,
            blocking: &[],
        },
        AssociationCase {
            name: "one link",
            links: 1,
            gateways: 0,
            blocking: &["link association"],
        },
        AssociationCase {
            name: "four links",
            links: 4,
            gateways: 0,
            blocking: &["link association"],
        },
        AssociationCase {
            name: "one customer gateway",
            links: 0,
            gateways: 1,
            blocking: &["customer gateway association"],
        },
        AssociationCase {
            name: "two links and two customer gateways",
            links: 2,
            gateways: 2,
            blocking: &["link association", "customer gateway association"],
        },
    ]
}

/// A customer gateway ARN naming the gateway id `cgw-{hex}`: a customer gateway is an
/// Amazon VPC resource, so the form is EC2's, as `customer_gateway_association_lifecycle`
/// builds it.
fn cgw_arn(hex: &str) -> String {
    format!("arn:aws:ec2:{CUSTOMER_GATEWAY_REGION}:123456789012:customer-gateway/cgw-{hex}")
}

/// The Region the gateways above live in, which is therefore the Region their carrier is
/// registered in ([`register_vpn_carrier`]).
const CUSTOMER_GATEWAY_REGION: &str = "us-west-2";

/// `AssociateCustomerGateway` tying `arn` to `device` of `network`, which must succeed.
///
/// A customer gateway is associated only where the global network carries, in the gateway's
/// Region, a transit gateway or core network edge its VPN attachment could be on, so the
/// carrier for the Region [`cgw_arn`] names is registered first.
async fn associate_gateway_ok(emulator: &Emulator, network: &str, device: &str, arn: &str) {
    register_vpn_carrier(emulator, network, CUSTOMER_GATEWAY_REGION).await;
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/customer-gateway-associations"),
            json!({ "CustomerGatewayArn": arn, "DeviceId": device }),
        )
        .await;
    assert_eq!(
        status, 200,
        "AssociateCustomerGateway({arn}): expected 200, got {status} with {body}"
    );
}

/// `DisassociateLink` for the `device`/`link` pair of `network`.
async fn disassociate_link(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: &str,
) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await
}

/// `DisassociateCustomerGateway` for `arn` on `network`. The ARN travels in a non-greedy
/// `@httpLabel`, so its `:` and `/` go percent-encoded.
async fn disassociate_gateway(emulator: &Emulator, network: &str, arn: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/customer-gateway-associations/{}",
            escape(arn)
        ))
        .await
}

#[tokio::test]
async fn a_device_still_associated_with_a_link_or_a_gateway_is_refused_until_it_is_disassociated() {
    for case in association_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;
        let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;

        let mut links: Vec<String> = Vec::new();
        for _ in 0..case.links {
            let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
            let (status, associated) = emulator
                .post(
                    &format!("/global-networks/{network}/link-associations"),
                    json!({ "DeviceId": device, "LinkId": link }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: AssociateLink expected 200, got {status} with {associated}"
            );
            links.push(link);
        }
        let mut gateways: Vec<String> = Vec::new();
        for index in 0..case.gateways {
            let arn = cgw_arn(&format!("0123456789abcdef{index}"));
            associate_gateway_ok(&emulator, &network, &device, &arn).await;
            gateways.push(arn);
        }

        let (status, body) = delete_device(&emulator, &network, &device).await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a device holding no association deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Device"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {name:?}: a dependency refusal is HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Reason"],
            json!("Other"),
            "case {name:?}: no input field is at fault, so the reason is the modelled Other, got {body}"
        );
        assert_eq!(
            body["Fields"],
            Value::Null,
            "case {name:?}: a dependency refusal blames no field, got {body}"
        );
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no Message in {body}"));
        let expected =
            crate::delete_live_reference_refusal::refusal_message("Device", &device);
        assert_eq!(
            message, expected,
            "case {name:?}: expected the message {expected:?}, got {message:?}"
        );
        // The refusal names the resource being deleted and NOT which kind of record still
        // names it, whichever kinds those are.
        for label in ["link association", "customer gateway association"] {
            assert!(
                !message.contains(label),
                "case {name:?}: the refusal must not name a {label}, got {message:?}"
            );
        }

        // The refusal changed nothing: the device is still readable, with the state the
        // clock gives it rather than DELETING.
        let still_there = read_device(&emulator, &network, &device)
            .await
            .unwrap_or_else(|| panic!("case {name:?}: a refused delete removed the device"));
        assert_ne!(
            still_there["State"],
            json!("DELETING"),
            "case {name:?}: a refused delete must leave the device's own state alone, got {still_there}"
        );

        // Removing all but the last association still blocks: the refusal tracks what is
        // left, not how many there once were.
        let mut remaining: Vec<(&str, String)> = links
            .iter()
            .map(|link| ("link", link.clone()))
            .chain(gateways.iter().map(|arn| ("gateway", arn.clone())))
            .collect();
        let last = remaining
            .pop()
            .unwrap_or_else(|| panic!("case {name:?}: expected at least one association"));
        for (kind, id) in &remaining {
            let (status, removed) = match *kind {
                "link" => disassociate_link(&emulator, &network, &device, id).await,
                _ => disassociate_gateway(&emulator, &network, id).await,
            };
            assert_eq!(
                status, 200,
                "case {name:?}: disassociating the {kind} {id} expected 200, got {status} with {removed}"
            );
            let (status, blocked) = delete_device(&emulator, &network, &device).await;
            assert_eq!(
                status, 400,
                "case {name:?}: one association still blocks the delete, got {status} with {blocked}"
            );
        }

        // With the last one gone the identical call succeeds, so the refusal tracked stored
        // state rather than the shape of the request.
        let (kind, id) = &last;
        let (status, removed) = match *kind {
            "link" => disassociate_link(&emulator, &network, &device, id).await,
            _ => disassociate_gateway(&emulator, &network, id).await,
        };
        assert_eq!(
            status, 200,
            "case {name:?}: disassociating the last {kind} expected 200, got {status} with {removed}"
        );
        let (status, deleted) = delete_device(&emulator, &network, &device).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with the associations gone the same delete must succeed, got {status} with {deleted}"
        );
        assert_eq!(
            deleted["Device"]["DeviceId"],
            json!(device),
            "case {name:?}: the delete must answer the device it removed, got {deleted}"
        );
        assert!(
            read_device(&emulator, &network, &device).await.is_none(),
            "case {name:?}: GetDevices still lists the deleted device"
        );
    }
}

/// One row of the not-found table: the ids the request addresses, and the resource the
/// refusal has to identify.
struct NotFoundCase {
    name: &'static str,
    /// Built from the ids the test mints, so each row is a closure over them.
    network: String,
    device: String,
    resource_id: String,
    resource_type: &'static str,
}

#[tokio::test]
async fn a_device_the_path_does_not_reach_is_the_not_found_refusal() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("devices").await;
    let other = emulator.create_global_network("other").await;
    let device = create_device_ok(&emulator, &network, json!({})).await;
    let elsewhere = create_device_ok(&emulator, &other, json!({})).await;

    // A device that was created and then deleted, so its id is well-formed and names
    // nothing.
    let (status, deleted) = delete_device(&emulator, &network, &device).await;
    assert_eq!(
        status, 200,
        "DeleteDevice expected 200, got {status} with {deleted}"
    );

    // Ids that were never minted, in the form this service mints.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_device = "device-00000000000000099".to_string();

    // The rows differ along WHICH half of the path is unreachable: the global network, the
    // device, or the pairing of the two (a device of another global network).
    let rows = vec![
        NotFoundCase {
            name: "a global network that never existed",
            network: unknown_network.clone(),
            device: unknown_device.clone(),
            resource_id: unknown_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            name: "a device that never existed",
            network: network.clone(),
            device: unknown_device.clone(),
            resource_id: unknown_device.clone(),
            resource_type: DEVICE,
        },
        NotFoundCase {
            name: "a device that was already deleted",
            network: network.clone(),
            device: device.clone(),
            resource_id: device.clone(),
            resource_type: DEVICE,
        },
        NotFoundCase {
            name: "a device of another global network",
            network: network.clone(),
            device: elsewhere.clone(),
            resource_id: elsewhere.clone(),
            resource_type: DEVICE,
        },
    ];

    for row in &rows {
        let name = row.name;
        let (status, body) = delete_device(&emulator, &row.network, &row.device).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(row.resource_id.as_str()),
            "case {name:?}: expected ResourceId {:?}, got {body}",
            row.resource_id
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {name:?}: expected ResourceType {:?}, got {body}",
            row.resource_type
        );
    }

    // The refused rows left the device of the other global network alone, so it is still
    // deletable through ITS own path.
    let (status, deleted) = delete_device(&emulator, &other, &elsewhere).await;
    assert_eq!(
        status, 200,
        "the device of the other global network must still delete, got {status} with {deleted}"
    );
}

#[tokio::test]
async fn deleting_the_devices_unblocks_the_deletes_of_their_site_and_global_network() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("teardown").await;
    let site = emulator.create_site(&network, "hq").await;
    let sited = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let unsited = create_device_ok(&emulator, &network, json!({})).await;

    // While the site holds a device its delete is refused, and so is the global network's.
    let (status, blocked) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 400,
        "DeleteSite with a device at the site expected 400, got {status} with {blocked}"
    );
    let (status, blocked) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 400,
        "DeleteGlobalNetwork with devices expected 400, got {status} with {blocked}"
    );

    for device in [&sited, &unsited] {
        let (status, deleted) = delete_device(&emulator, &network, device).await;
        assert_eq!(
            status, 200,
            "DeleteDevice({device}) expected 200, got {status} with {deleted}"
        );
    }
    assert!(
        device_ids(&emulator, &network).await.is_empty(),
        "every device of the global network was deleted, so GetDevices lists none"
    );

    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 200,
        "with the device gone DeleteSite must succeed, got {status} with {deleted}"
    );
    let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "with the site and devices gone DeleteGlobalNetwork must succeed, got {status} with {deleted}"
    );
}
