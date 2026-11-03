// SPDX-License-Identifier: CC0-1.0

//! How a DELETE of a global network OBJECT behaves while some other stored record still
//! NAMES the object being deleted, driven over HTTP through the real router.
//!
//! The rule under test: a delete is refused with this service's dependency
//! `ValidationException` (HTTP 400) while ANY stored record of the same global network
//! still holds a reference to the resource being deleted — whichever member carries it,
//! whether the referring record requires it or only names the one the request supplied,
//! and whether or not that record is a resource with an id of its own — so no delete ever
//! leaves a stored record reporting an id no reader resolves. The refusal names the
//! resource being deleted by its kind and its id, and nothing else; it changes nothing;
//! and the identical call succeeds once the references are gone.
//!
//! `crate::delete_link` and `crate::delete_device` already pin the two kinds of association
//! each operation's own documentation names. What the tables here span is the rest of the
//! class: a connection, which is a resource of its own and needs no association at all to
//! name a link or a device ("Creates a connection between two devices", `smithy_query
//! query="doc" shape="CreateConnection"`, with an optional link at either end), beside a
//! transit gateway Connect peer association carried over the link. The rows differ along
//! WHICH member carries the reference, how MANY records carry one, which KINDS carry one at
//! once, and — as controls — a reference held to a sibling resource, which must not block
//! anything.
//!
//! Nothing reaches into `crate::state`: every resource is created, read back and removed
//! through an operation, and the invariant is asserted through the readers a client has
//! (`GetConnections`, `GetLinks`, `GetDevices`).

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, get_connections, id_of};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_association_lifecycle::{
    associate_ok as associate_link_ok, disassociate as disassociate_link,
};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::transit_gateway_connect_peer_association_lifecycle::{
    associate as associate_connect_peer, connect_peer_arn,
    disassociate as disassociate_connect_peer, register_gateway,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// How a refusal spells the resource being deleted: every word of the ARN resource type
/// capitalised, as `crate::delete_dependency_refusal` pins for the parents it exercises.
const LINK_PARENT: &str = "Link";
/// See [`LINK_PARENT`].
const DEVICE_PARENT: &str = "Device";

/// How a refusal spells each kind of record that can still name a link or a device: the
/// resource kind read as words.
const LINK_ASSOCIATION: &str = "link association";
/// See [`LINK_ASSOCIATION`].
const CONNECTION: &str = "connection";
/// See [`LINK_ASSOCIATION`].
const TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION: &str = "transit gateway connect peer association";

/// The message this service's dependency refusal carries: the resource being deleted, by
/// kind and by the id the request addressed it with, and then the fixed statement that an
/// existing association blocks the delete — pinned here so the rows assert the whole
/// message rather than the presence of a word.
pub(crate) fn refusal_message(parent: &str, parent_id: &str) -> String {
    format!("{parent} with ID {parent_id} cannot be deleted due to existing association.")
}

/// Assert `body` is the dependency refusal for the `parent` resource `parent_id`: HTTP
/// 400, `ValidationException`, the catch-all reason, no field at fault.
fn assert_reference_refusal(
    case: &str,
    parent: &str,
    parent_id: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: a dependency refusal is HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("Other"),
        "case {case:?}: no input field is at fault, so the reason is the modelled Other, got {body}"
    );
    assert_eq!(
        body["Fields"],
        Value::Null,
        "case {case:?}: a dependency refusal blames no field, got {body}"
    );
    let expected = refusal_message(parent, parent_id);
    assert_eq!(
        body["Message"].as_str(),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {body}"
    );
}

/// `DeleteLink` for `link` of `network`.
async fn delete_link(emulator: &Emulator, network: &str, link: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/links/{link}"))
        .await
}

/// `DeleteDevice` for `device` of `network`.
async fn delete_device(emulator: &Emulator, network: &str, device: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/devices/{device}"))
        .await
}

/// `DeleteConnection` for `connection` of `network`, which must succeed.
async fn delete_connection_ok(emulator: &Emulator, network: &str, connection: &str) {
    let (status, body) = emulator
        .delete(&format!(
            "/global-networks/{network}/connections/{connection}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DeleteConnection({connection}): expected 200, got {status} with {body}"
    );
}

/// The `LinkId`s `GetLinks` lists for `network`.
async fn link_ids(emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/links?maxResults=50"))
        .await;
    assert_eq!(
        status, 200,
        "GetLinks({network}): expected 200, got {status} with {body}"
    );
    ids_of(&body, "Links", "LinkId")
}

/// The `DeviceId`s `GetDevices` lists for `network`.
async fn device_ids(emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/devices?maxResults=50"))
        .await;
    assert_eq!(
        status, 200,
        "GetDevices({network}): expected 200, got {status} with {body}"
    );
    ids_of(&body, "Devices", "DeviceId")
}

/// Assert the invariant the refusal exists for: every link and device a stored connection
/// of `network` reports is still answered by the reader of that resource, so no reader
/// reports an id another reader cannot resolve.
pub(crate) async fn assert_every_reference_resolves(
    case: &str,
    emulator: &Emulator,
    network: &str,
) {
    let (status, connections) = get_connections(emulator, network, "maxResults=50").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnections expected 200, got {status} with {connections}"
    );
    let links = link_ids(emulator, network).await;
    let devices = device_ids(emulator, network).await;
    let listed = connections["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Connections array in {connections}"))
        .clone();
    for connection in &listed {
        let connection_id = id_of(connection);
        // The empty string names NO resource for every reference member of this service
        // (`crate::state::optional_reference`), so it resolves nothing and is skipped.
        for member in ["LinkId", "ConnectedLinkId"] {
            if let Some(named) = connection[member].as_str().filter(|value| !value.is_empty()) {
                assert!(
                    links.iter().any(|link| link == named),
                    "case {case:?}: connection {connection_id} reports {member} {named:?} that \
                     GetLinks does not answer, got {links:?}"
                );
            }
        }
        for member in ["DeviceId", "ConnectedDeviceId"] {
            if let Some(named) = connection[member].as_str().filter(|value| !value.is_empty()) {
                assert!(
                    devices.iter().any(|device| device == named),
                    "case {case:?}: connection {connection_id} reports {member} {named:?} that \
                     GetDevices does not answer, got {devices:?}"
                );
            }
        }
    }
}

/// Which of a connection's two optional link members names the resource under test.
#[derive(Clone, Copy, Debug)]
enum End {
    /// The member of the connection's first end (`LinkId` / `DeviceId`).
    First,
    /// The member of its second end (`ConnectedLinkId` / `ConnectedDeviceId`).
    Second,
    /// Both members, so one record holds two references to the one resource. A LINK may be
    /// named at both ends of one connection; a DEVICE may not (`CreateConnection` refuses
    /// the two required device members naming one device), so no device row uses this.
    Both,
}

/// One row of the link table: what is left naming the link, and the kinds the refusal must
/// then name.
struct LinkCase {
    name: &'static str,
    /// Devices associated with the link through `AssociateLink`.
    associations: usize,
    /// One entry per connection created over the link, naming which of its link members
    /// point at it.
    connections: &'static [End],
    /// Whether a transit gateway Connect peer association is carried over the link, with
    /// the device association it had to be made over.
    peer_association: bool,
    /// Whether a SIBLING link of the same site is named by its own connection, which must
    /// block nothing here.
    sibling_connection: bool,
    /// The kinds of record left naming the link, which the refusal does NOT enumerate;
    /// empty when the delete has to succeed.
    blocking: &'static [&'static str],
}

/// The rows differ along which member of a connection names the link, how many connections
/// name it, which kinds name it at once, and whether the only reference is held to a
/// sibling — everything the rule generalises over. The rows that hold nothing of the link
/// are the controls: the same call succeeds.
const LINK_CASES: [LinkCase; 8] = [
    LinkCase {
        name: "no record names the link",
        associations: 0,
        connections: &[],
        peer_association: false,
        sibling_connection: false,
        blocking: &[],
    },
    LinkCase {
        name: "a connection reached over the link at its first end",
        associations: 0,
        connections: &[End::First],
        peer_association: false,
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    LinkCase {
        name: "a connection reached over the link at its second end",
        associations: 0,
        connections: &[End::Second],
        peer_association: false,
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    LinkCase {
        name: "one connection naming the link at both of its ends",
        associations: 0,
        connections: &[End::Both],
        peer_association: false,
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    LinkCase {
        name: "two connections, one over each end of the link",
        associations: 0,
        connections: &[End::First, End::Second],
        peer_association: false,
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    LinkCase {
        name: "a connection beside two device associations",
        associations: 2,
        connections: &[End::First],
        peer_association: false,
        sibling_connection: false,
        blocking: &[LINK_ASSOCIATION, CONNECTION],
    },
    LinkCase {
        name: "a transit gateway Connect peer association carried over the link",
        associations: 0,
        connections: &[],
        peer_association: true,
        sibling_connection: false,
        blocking: &[LINK_ASSOCIATION, TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION],
    },
    LinkCase {
        name: "only a sibling link is named by a connection",
        associations: 0,
        connections: &[],
        peer_association: false,
        sibling_connection: true,
        blocking: &[],
    },
];

#[tokio::test]
async fn delete_link_is_refused_while_any_stored_record_still_names_the_link() {
    for (row, case) in LINK_CASES.iter().enumerate() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;
        let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
        let sibling = create_link_ok(&emulator, &network, &site, &plain_link()).await;
        let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        let connected_device =
            create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;

        let mut associated: Vec<String> = Vec::new();
        for _ in 0..case.associations {
            let member = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
            associate_link_ok(&emulator, &network, &member, &link).await;
            associated.push(member);
        }

        let mut connections: Vec<String> = Vec::new();
        for end in case.connections {
            let mut body = json!({
                "DeviceId": device,
                "ConnectedDeviceId": connected_device,
            });
            match end {
                End::First => body["LinkId"] = json!(link),
                End::Second => body["ConnectedLinkId"] = json!(link),
                End::Both => {
                    body["LinkId"] = json!(link);
                    body["ConnectedLinkId"] = json!(link);
                }
            }
            connections.push(id_of(
                &create_connection_ok(&emulator, &network, body).await,
            ));
        }
        if case.sibling_connection {
            connections.push(id_of(
                &create_connection_ok(
                    &emulator,
                    &network,
                    json!({
                        "DeviceId": device,
                        "ConnectedDeviceId": connected_device,
                        "LinkId": sibling,
                        "ConnectedLinkId": sibling,
                    }),
                )
                .await,
            ));
        }

        // A Connect peer is created on a transit gateway registered with the global network,
        // and can only be carried over a link its device is already associated with, so the
        // row that holds one holds that device association too.
        let peer = if case.peer_association {
            let account = own_account();
            register_gateway(
                &emulator,
                &network,
                &transit_gateway_arn("aws", "us-west-2", &account, row + 1, 17),
            )
            .await;
            let arn = connect_peer_arn("aws", "us-west-2", &account, row + 1, 17);
            associate_link_ok(&emulator, &network, &device, &link).await;
            associated.push(device.clone());
            let (status, associated_peer) = associate_connect_peer(
                &emulator,
                &network,
                json!({
                    "TransitGatewayConnectPeerArn": arn,
                    "DeviceId": device,
                    "LinkId": link,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: AssociateTransitGatewayConnectPeer expected 200, got {status} \
                 with {associated_peer}"
            );
            Some(arn)
        } else {
            None
        };

        let (status, body) = delete_link(&emulator, &network, &link).await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a link no stored record names deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Link"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            assert!(
                !link_ids(&emulator, &network).await.contains(&link),
                "case {name:?}: GetLinks still lists the deleted link {link}"
            );
            assert_every_reference_resolves(name, &emulator, &network).await;
            continue;
        }

        assert_reference_refusal(name, LINK_PARENT, &link, status, &body);

        // The refusal changed nothing: the link is still readable, the records naming it
        // still report it, and the two readers still agree.
        assert!(
            link_ids(&emulator, &network).await.contains(&link),
            "case {name:?}: a refused delete removed the link {link}"
        );
        assert_every_reference_resolves(name, &emulator, &network).await;

        // Removing all but the last reference still blocks: the refusal tracks what is
        // stored now, not how many references there once were.
        let mut remaining = connections.len() - usize::from(case.sibling_connection)
            + associated.len()
            + usize::from(peer.is_some());
        for connection in &connections {
            delete_connection_ok(&emulator, &network, connection).await;
            remaining = remaining.saturating_sub(1);
            if remaining > 0 {
                let (status, blocked) = delete_link(&emulator, &network, &link).await;
                assert_eq!(
                    status, 400,
                    "case {name:?}: {remaining} record(s) still name the link, got {status} with \
                     {blocked}"
                );
            }
        }
        // A Connect peer association stands on the device association it was made over, so
        // it goes first; the link association that is left still blocks the delete.
        if let Some(arn) = &peer {
            let (status, untied) = disassociate_connect_peer(&emulator, &network, arn).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DisassociateTransitGatewayConnectPeer expected 200, got {status} \
                 with {untied}"
            );
            remaining -= 1;
            if remaining > 0 {
                let (status, blocked) = delete_link(&emulator, &network, &link).await;
                assert_eq!(
                    status, 400,
                    "case {name:?}: {remaining} record(s) still name the link, got {status} with \
                     {blocked}"
                );
            }
        }
        for member in &associated {
            let (status, untied) = disassociate_link(&emulator, &network, member, &link).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DisassociateLink({member}) expected 200, got {status} with {untied}"
            );
            remaining -= 1;
            if remaining > 0 {
                let (status, blocked) = delete_link(&emulator, &network, &link).await;
                assert_eq!(
                    status, 400,
                    "case {name:?}: {remaining} record(s) still name the link, got {status} with \
                     {blocked}"
                );
            }
        }

        // With the references gone the identical call succeeds, so the refusal tracked
        // stored state rather than the shape of the request.
        let (status, deleted) = delete_link(&emulator, &network, &link).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with nothing naming it the same delete must succeed, got {status} \
             with {deleted}"
        );
        assert_eq!(
            deleted["Link"]["LinkId"],
            json!(link.clone()),
            "case {name:?}: the delete must answer the link it removed, got {deleted}"
        );
        assert!(
            !link_ids(&emulator, &network).await.contains(&link),
            "case {name:?}: GetLinks still lists the deleted link {link}"
        );
        assert!(
            link_ids(&emulator, &network).await.contains(&sibling),
            "case {name:?}: the delete must leave the sibling link {sibling} alone"
        );
        assert_every_reference_resolves(name, &emulator, &network).await;
    }
}

/// One row of the device table: what is left naming the device, and the kinds the refusal
/// must then name.
struct DeviceCase {
    name: &'static str,
    /// Whether the device is associated with a link of its site through `AssociateLink`.
    associated: bool,
    /// One entry per connection created over the device, naming which of its device members
    /// point at it.
    connections: &'static [End],
    /// Whether a connection names only the OTHER two devices, which must block nothing
    /// here.
    sibling_connection: bool,
    blocking: &'static [&'static str],
}

/// The rows differ along which member of a connection names the device, how many
/// connections name it, and which kinds name it at once; the rows that hold nothing of the
/// device are the controls. A device member of a connection is REQUIRED, unlike the link
/// members above, which is the other half of what the rule generalises over.
const DEVICE_CASES: [DeviceCase; 5] = [
    DeviceCase {
        name: "no record names the device",
        associated: false,
        connections: &[],
        sibling_connection: false,
        blocking: &[],
    },
    DeviceCase {
        name: "a connection holding the device at its first end",
        associated: false,
        connections: &[End::First],
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    DeviceCase {
        name: "a connection holding the device at its second end",
        associated: false,
        connections: &[End::Second],
        sibling_connection: false,
        blocking: &[CONNECTION],
    },
    DeviceCase {
        name: "two connections beside a link association",
        associated: true,
        connections: &[End::First, End::Second],
        sibling_connection: false,
        blocking: &[LINK_ASSOCIATION, CONNECTION],
    },
    DeviceCase {
        name: "only the other devices are named by a connection",
        associated: false,
        connections: &[],
        sibling_connection: true,
        blocking: &[],
    },
];

#[tokio::test]
async fn delete_device_is_refused_while_any_stored_record_still_names_the_device() {
    for case in DEVICE_CASES.iter() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;
        let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
        let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        let partner = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        let bystander = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;

        if case.associated {
            associate_link_ok(&emulator, &network, &device, &link).await;
        }

        let mut connections: Vec<String> = Vec::new();
        for end in case.connections {
            let body = match end {
                End::First => json!({ "DeviceId": device, "ConnectedDeviceId": partner }),
                End::Second => json!({ "DeviceId": partner, "ConnectedDeviceId": device }),
                // A connection between one device and itself cannot be created, so no
                // device row asks for one.
                End::Both => panic!(
                    "case {name:?}: a connection names two different devices, never one twice"
                ),
            };
            connections.push(id_of(
                &create_connection_ok(&emulator, &network, body).await,
            ));
        }
        if case.sibling_connection {
            connections.push(id_of(
                &create_connection_ok(
                    &emulator,
                    &network,
                    json!({ "DeviceId": partner, "ConnectedDeviceId": bystander }),
                )
                .await,
            ));
        }

        let (status, body) = delete_device(&emulator, &network, &device).await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a device no stored record names deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Device"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            assert!(
                !device_ids(&emulator, &network).await.contains(&device),
                "case {name:?}: GetDevices still lists the deleted device {device}"
            );
            assert_every_reference_resolves(name, &emulator, &network).await;
            continue;
        }

        assert_reference_refusal(name, DEVICE_PARENT, &device, status, &body);
        assert!(
            device_ids(&emulator, &network).await.contains(&device),
            "case {name:?}: a refused delete removed the device {device}"
        );
        assert_every_reference_resolves(name, &emulator, &network).await;

        let mut remaining =
            connections.len() - usize::from(case.sibling_connection) + usize::from(case.associated);
        for connection in &connections {
            delete_connection_ok(&emulator, &network, connection).await;
            remaining = remaining.saturating_sub(1);
            if remaining > 0 {
                let (status, blocked) = delete_device(&emulator, &network, &device).await;
                assert_eq!(
                    status, 400,
                    "case {name:?}: {remaining} record(s) still name the device, got {status} \
                     with {blocked}"
                );
            }
        }
        if case.associated {
            let (status, untied) = disassociate_link(&emulator, &network, &device, &link).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DisassociateLink expected 200, got {status} with {untied}"
            );
        }

        let (status, deleted) = delete_device(&emulator, &network, &device).await;
        assert_eq!(
            status, 200,
            "case {name:?}: with nothing naming it the same delete must succeed, got {status} \
             with {deleted}"
        );
        assert_eq!(
            deleted["Device"]["DeviceId"],
            json!(device.clone()),
            "case {name:?}: the delete must answer the device it removed, got {deleted}"
        );
        assert!(
            !device_ids(&emulator, &network).await.contains(&device),
            "case {name:?}: GetDevices still lists the deleted device {device}"
        );
        assert!(
            device_ids(&emulator, &network).await.contains(&partner),
            "case {name:?}: the delete must leave the other device {partner} alone"
        );
        assert_every_reference_resolves(name, &emulator, &network).await;
    }
}

#[tokio::test]
async fn an_update_that_drops_the_reference_unblocks_the_delete_of_the_resource_it_named() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("reference moved").await;
    let site = emulator.create_site(&network, "hq").await;
    let first = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let second = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let connected_device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let connection = id_of(
        &create_connection_ok(
            &emulator,
            &network,
            json!({
                "DeviceId": device,
                "ConnectedDeviceId": connected_device,
                "LinkId": first,
            }),
        )
        .await,
    );

    let (status, blocked) = delete_link(&emulator, &network, &first).await;
    assert_reference_refusal(
        "the link the connection names",
        LINK_PARENT,
        &first,
        status,
        &blocked,
    );
    // The link the connection does NOT name is deletable the whole time, so the refusal is
    // about the reference rather than about the operation or the global network.
    let (status, deleted) = delete_link(&emulator, &network, &second).await;
    assert_eq!(
        status, 200,
        "an unreferenced link of the same global network must still delete, got {status} with \
         {deleted}"
    );

    // "To remove information for any of the parameters, specify an empty string"
    // (`smithy_query query="doc" shape="UpdateConnection"`): the update moves the reference
    // off the link without removing the connection, and the delete that was refused now
    // succeeds.
    let (status, updated) = emulator
        .patch(
            &format!("/global-networks/{network}/connections/{connection}"),
            json!({ "LinkId": "" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateConnection expected 200, got {status} with {updated}"
    );
    assert_ne!(
        updated["Connection"]["LinkId"],
        json!(first.clone()),
        "the update must leave the connection naming the link no more, got {updated}"
    );

    let (status, deleted) = delete_link(&emulator, &network, &first).await;
    assert_eq!(
        status, 200,
        "with the reference dropped the same delete must succeed, got {status} with {deleted}"
    );
    assert_every_reference_resolves("reference moved", &emulator, &network).await;
}
