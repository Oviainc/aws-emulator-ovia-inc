// SPDX-License-Identifier: CC0-1.0

//! In which ORDER a global network that holds connections can be emptied, driven over HTTP
//! through the real router.
//!
//! The rule under test: a delete of a resource of this service is refused with the
//! dependency `ValidationException` (HTTP 400) while any stored record of the same global
//! network still names it — a connection names two devices and, optionally, a link at
//! either end ("Creates a connection between two devices", `smithy_query query="doc"
//! shape="CreateConnection"`) — so a record can only be removed after every record naming
//! it is gone, and the parent is only removed after its children are. Together that means no
//! sequence of deletes a client can issue strands a stored record: while a connection
//! stands, every id it reports is answered by that resource's own reader and the connection
//! itself stays addressable through its ARN, and once the connections are gone the same
//! calls that were refused succeed, in the order the references allow (connection, then its
//! links and devices, then the site, then the global network).
//!
//! `crate::delete_live_reference_refusal` pins WHICH kinds one such refusal names; what the
//! table here spans is the whole TEARDOWN, which is the only thing that shows a connection
//! can never be orphaned under a global network the store no longer holds. The rows differ
//! along how many connections the global network holds, how many devices they share, which
//! of the two device members name one device, and whether a link is named at either end.
//!
//! Nothing reaches into `crate::state`: every resource is created, read and removed through
//! an operation, and a connection is only ever read back through `GetConnections` and
//! `ListTagsForResource`.

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, get_connections, id_of, ids_in};
use crate::delete_dependency_refusal::{assert_dependency_refusal, assert_object_dependency_refusal};
use crate::delete_live_reference_refusal::assert_every_reference_resolves;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::list_tags;

/// How a refusal spells the resource being deleted: every word of its ARN resource type
/// capitalised, as `crate::delete_dependency_refusal` pins for the parents it exercises.
const GLOBAL_NETWORK_PARENT: &str = "Global Network";
/// See [`GLOBAL_NETWORK_PARENT`].
const SITE_PARENT: &str = "Site";
/// See [`GLOBAL_NETWORK_PARENT`].
const DEVICE_PARENT: &str = "Device";
/// See [`GLOBAL_NETWORK_PARENT`].
const LINK_PARENT: &str = "Link";

/// How a refusal spells each kind of record that can block one of the deletes below: the
/// resource kind read as words.
const DEVICE: &str = "device";
/// See [`DEVICE`].
const SITE: &str = "site";

/// One connection of a row: which of the row's devices it ties together, and which of the
/// row's links each of its two ends is reached over (`None` for an end naming no link).
struct Wiring {
    first_device: usize,
    second_device: usize,
    first_link: Option<usize>,
    second_link: Option<usize>,
}

/// One row of the teardown table: the resources to put in a fresh global network, and the
/// connections to wire over them.
struct TeardownCase {
    name: &'static str,
    /// Devices the connections are wired over, each assigned to the row's one site.
    devices: usize,
    /// Links the connections can be reached over, each created at that site.
    links: usize,
    connections: &'static [Wiring],
}

/// The rows differ along how many connections stand at once, how many devices they share,
/// whether both ends of one connection are reached over the same link, and whether an end is
/// reached over a link at all — everything the reference rule generalises over. The first row
/// holds no connection at all, so it is the control: the identical teardown runs without a
/// single refusal on the way.
const TEARDOWN_CASES: [TeardownCase; 6] = [
    TeardownCase {
        name: "no connection at all",
        devices: 2,
        links: 1,
        connections: &[],
    },
    TeardownCase {
        name: "one connection over two devices, naming no link",
        devices: 2,
        links: 0,
        connections: &[Wiring {
            first_device: 0,
            second_device: 1,
            first_link: None,
            second_link: None,
        }],
    },
    TeardownCase {
        name: "one connection naming a link at each end",
        devices: 2,
        links: 2,
        connections: &[Wiring {
            first_device: 0,
            second_device: 1,
            first_link: Some(0),
            second_link: Some(1),
        }],
    },
    TeardownCase {
        name: "one connection reached over the same link at both ends",
        devices: 2,
        links: 1,
        connections: &[Wiring {
            first_device: 0,
            second_device: 1,
            first_link: Some(0),
            second_link: Some(0),
        }],
    },
    TeardownCase {
        name: "two connections between the same pair of devices",
        devices: 2,
        links: 0,
        connections: &[
            Wiring {
                first_device: 0,
                second_device: 1,
                first_link: None,
                second_link: None,
            },
            Wiring {
                first_device: 1,
                second_device: 0,
                first_link: None,
                second_link: None,
            },
        ],
    },
    TeardownCase {
        name: "three connections sharing three devices, one over a link",
        devices: 3,
        links: 1,
        connections: &[
            Wiring {
                first_device: 0,
                second_device: 1,
                first_link: None,
                second_link: None,
            },
            Wiring {
                first_device: 1,
                second_device: 2,
                first_link: Some(0),
                second_link: None,
            },
            Wiring {
                first_device: 2,
                second_device: 0,
                first_link: None,
                second_link: None,
            },
        ],
    },
];

/// A connection that still stands, with the resources it names, so a row can assert the
/// refusal for exactly those.
struct Standing {
    id: String,
    arn: String,
    /// The devices its `DeviceId` / `ConnectedDeviceId` report, deduplicated.
    devices: Vec<String>,
    /// The links its `LinkId` / `ConnectedLinkId` report, deduplicated.
    links: Vec<String>,
}

/// `DeleteDevice` for `device` of `network`.
async fn delete_device(emulator: &Emulator, network: &str, device: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/devices/{device}"))
        .await
}

/// `DeleteLink` for `link` of `network`.
async fn delete_link(emulator: &Emulator, network: &str, link: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/links/{link}"))
        .await
}

/// `DeleteSite` for `site` of `network`.
async fn delete_site(emulator: &Emulator, network: &str, site: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await
}

/// The item of `items` at `index`, or a failure naming the case rather than an index panic.
fn pick(case: &str, what: &str, items: &[String], index: usize) -> String {
    items
        .get(index)
        .unwrap_or_else(|| panic!("case {case:?}: the row names {what} {index}, which it does not create"))
        .clone()
}

/// The `ConnectionId`s `GetConnections` lists for `network`, sorted so a row compares sets.
async fn listed_connection_ids(case: &str, emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, body) = get_connections(emulator, network, "maxResults=50").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnections expected 200, got {status} with {body}"
    );
    let mut ids = ids_in(&body);
    ids.sort();
    ids
}

/// Add `value` to `into` unless it is already there, so a connection naming one resource
/// twice reports it once.
fn push_unique(into: &mut Vec<String>, value: String) {
    if !into.contains(&value) {
        into.push(value);
    }
}

#[tokio::test]
async fn a_global_network_holding_connections_empties_only_in_the_order_its_references_allow() {
    for case in TEARDOWN_CASES.iter() {
        let name = case.name;
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;

        let mut links: Vec<String> = Vec::new();
        for _ in 0..case.links {
            links.push(create_link_ok(&emulator, &network, &site, &plain_link()).await);
        }
        let mut devices: Vec<String> = Vec::new();
        for index in 0..case.devices {
            devices.push(
                create_device_ok(
                    &emulator,
                    &network,
                    json!({ "SiteId": site, "Description": format!("device {index}") }),
                )
                .await,
            );
        }
        // A device no connection names, which has to stay deletable while the connections
        // stand: the refusal is about the reference held, not about the global network
        // holding connections at all.
        let bystander = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;

        let mut standing: Vec<Standing> = Vec::new();
        for (index, wiring) in case.connections.iter().enumerate() {
            let first_device = pick(name, "device", &devices, wiring.first_device);
            let second_device = pick(name, "device", &devices, wiring.second_device);
            let mut body = json!({
                "DeviceId": first_device,
                "ConnectedDeviceId": second_device,
                "Description": format!("connection {index}"),
                "Tags": [{ "Key": "case", "Value": name }],
            });
            let mut named_links: Vec<String> = Vec::new();
            if let Some(link) = wiring.first_link {
                let link = pick(name, "link", &links, link);
                body["LinkId"] = json!(link);
                push_unique(&mut named_links, link);
            }
            if let Some(link) = wiring.second_link {
                let link = pick(name, "link", &links, link);
                body["ConnectedLinkId"] = json!(link);
                push_unique(&mut named_links, link);
            }
            let created = create_connection_ok(&emulator, &network, body).await;
            let mut named_devices: Vec<String> = Vec::new();
            push_unique(&mut named_devices, first_device);
            push_unique(&mut named_devices, second_device);
            standing.push(Standing {
                id: id_of(&created),
                arn: created["ConnectionArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: no ConnectionArn in {created}"))
                    .to_string(),
                devices: named_devices,
                links: named_links,
            });
        }

        // The unreferenced device deletes straight away, whatever the connections hold.
        let (status, deleted) = delete_device(&emulator, &network, &bystander).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a device no connection names must still delete, got {status} with \
             {deleted}"
        );

        while !standing.is_empty() {
            // Nothing a standing connection reports is unreadable, whatever has been
            // deleted so far.
            assert_every_reference_resolves(name, &emulator, &network).await;

            // The parent chain above the connections is refused: the global network still
            // holds devices and its site, and the site still holds those devices (and, when
            // the row has them, its links).
            let (status, body) = emulator.delete(&format!("/global-networks/{network}")).await;
            assert_dependency_refusal(name, GLOBAL_NETWORK_PARENT, status, &body, &[DEVICE, SITE]);
            let (status, body) = delete_site(&emulator, &network, &site).await;
            assert_object_dependency_refusal(name, SITE_PARENT, &site, status, &body);

            // Every device and link a standing connection names is refused, and the refusal
            // names the connection as what blocks it.
            let mut blocked_devices: Vec<String> = Vec::new();
            let mut blocked_links: Vec<String> = Vec::new();
            for connection in &standing {
                for device in &connection.devices {
                    push_unique(&mut blocked_devices, device.clone());
                }
                for link in &connection.links {
                    push_unique(&mut blocked_links, link.clone());
                }
            }
            for device in &blocked_devices {
                let (status, body) = delete_device(&emulator, &network, device).await;
                assert_object_dependency_refusal(name, DEVICE_PARENT, device, status, &body);
            }
            for link in &blocked_links {
                let (status, body) = delete_link(&emulator, &network, link).await;
                assert_object_dependency_refusal(name, LINK_PARENT, link, status, &body);
            }

            // Every standing connection is still addressable: the reader lists exactly
            // those, and its ARN still reaches its tags.
            let mut expected: Vec<String> =
                standing.iter().map(|holder| holder.id.clone()).collect();
            expected.sort();
            let listed = listed_connection_ids(name, &emulator, &network).await;
            assert_eq!(
                listed, expected,
                "case {name:?}: expected GetConnections to list {expected:?}, got {listed:?}"
            );
            for connection in &standing {
                let (status, tags) = list_tags(&emulator, &connection.arn).await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: ListTagsForResource({}) expected 200, got {status} with {tags}",
                    connection.arn
                );
                assert_eq!(
                    tags,
                    json!([{ "Key": "case", "Value": name }]),
                    "case {name:?}: expected the stored tags of {}, got {tags}",
                    connection.id
                );
            }

            // Removing the connection is what makes what it named deletable in turn, so the
            // teardown peels one connection and goes round again.
            let removed = standing.remove(0);
            let (status, deleted) = emulator
                .delete(&format!(
                    "/global-networks/{network}/connections/{}",
                    removed.id
                ))
                .await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteConnection({}) expected 200, got {status} with {deleted}",
                removed.id
            );
            assert_eq!(
                deleted["Connection"]["ConnectionId"],
                json!(removed.id.clone()),
                "case {name:?}: the delete must answer the connection it removed, got {deleted}"
            );
        }

        // With every connection gone the whole tree comes apart, in the order the
        // references allow and with no call refused.
        let listed = listed_connection_ids(name, &emulator, &network).await;
        assert!(
            listed.is_empty(),
            "case {name:?}: every connection was deleted, so GetConnections lists none, got \
             {listed:?}"
        );
        for link in &links {
            let (status, deleted) = delete_link(&emulator, &network, link).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteLink({link}) expected 200, got {status} with {deleted}"
            );
        }
        for device in &devices {
            let (status, deleted) = delete_device(&emulator, &network, device).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DeleteDevice({device}) expected 200, got {status} with {deleted}"
            );
        }
        let (status, deleted) = delete_site(&emulator, &network, &site).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteSite expected 200, got {status} with {deleted}"
        );
        let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteGlobalNetwork expected 200, got {status} with {deleted}"
        );

        // The global network is gone and so is everything that was scoped to it, so no
        // record is left naming a parent no reader resolves: its connections read as the
        // missing-resource answer rather than as an orphaned listing.
        let (status, body) = get_connections(&emulator, &network, "").await;
        assert_eq!(
            status, 404,
            "case {name:?}: GetConnections on the deleted global network expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}
