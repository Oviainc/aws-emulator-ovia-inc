// SPDX-License-Identifier: CC0-1.0

//! How this service classifies a DELETE it refuses because the resource still owns
//! dependents the caller has to remove first.
//!
//! The rule: a delete blocked by remaining child resources is a `ValidationException`
//! (HTTP 400) — the request is invalid against the stored resource tree and no retry of
//! THAT call can make it valid — while `ConflictException` (HTTP 409) stays for a conflict
//! with the resource's own concurrent state (an idempotency token reused for a different
//! request), which a retry can clear. Both are modelled for every delete of this service
//! (`smithy_query operation=DeleteGlobalNetwork,DeleteSite`), so the classification is
//! behaviour, not shape availability.
//!
//! The tables below span the dependency class: which child kinds are held, how many of
//! each, and which parent is being deleted. A delete addressed at a CONTAINER (the global
//! network itself) names every kind that blocks it and no kind that does not, as its
//! recorded refusal does; a delete addressed at one of the global network's own OBJECTS (a
//! site here) names that object by its kind and the id the request addressed it with, and no
//! dependent kind at all. Either way the refusal must leave the resource tree untouched and
//! must stop blocking once the dependents are gone.

use serde_json::{json, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// One row of the `DeleteGlobalNetwork` table: the children to put in a fresh global
/// network, and the kinds the refusal must then name.
struct NetworkCase {
    name: &'static str,
    /// Sites to create in the global network.
    sites: usize,
    /// Links to create under the first site (so a link blocks through its own site).
    links: usize,
    /// Devices to create in the global network; the first is assigned to the first site
    /// when there is one, the rest are unassigned.
    devices: usize,
    /// Whether to create the global network's one core network (the per-network quota is
    /// one, `state::MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK`).
    core_network: bool,
    /// The dependent kinds the refusal must name, in no particular order.
    blocking: &'static [&'static str],
}

/// The rows differ along which child kinds are held, how many of each, and whether a child
/// is reached directly or through another child (a link through its site) — everything the
/// classification rule generalises over. The last row holds nothing, so it is the control:
/// the same call succeeds.
fn network_cases() -> Vec<NetworkCase> {
    vec![
        NetworkCase {
            name: "one site",
            sites: 1,
            links: 0,
            devices: 0,
            core_network: false,
            blocking: &["site"],
        },
        NetworkCase {
            name: "three sites",
            sites: 3,
            links: 0,
            devices: 0,
            core_network: false,
            blocking: &["site"],
        },
        NetworkCase {
            name: "a site holding two links",
            sites: 1,
            links: 2,
            devices: 0,
            core_network: false,
            blocking: &["site"],
        },
        NetworkCase {
            name: "one unassigned device",
            sites: 0,
            links: 0,
            devices: 1,
            core_network: false,
            blocking: &["device"],
        },
        NetworkCase {
            name: "two devices, one of them assigned to a site",
            sites: 1,
            links: 0,
            devices: 2,
            core_network: false,
            blocking: &["device", "site"],
        },
        NetworkCase {
            name: "one core network",
            sites: 0,
            links: 0,
            devices: 0,
            core_network: true,
            blocking: &["core network"],
        },
        NetworkCase {
            name: "a core network beside a site and a device",
            sites: 1,
            links: 1,
            devices: 1,
            core_network: true,
            blocking: &["device", "site", "core network"],
        },
        NetworkCase {
            name: "nothing at all",
            sites: 0,
            links: 0,
            devices: 0,
            core_network: false,
            blocking: &[],
        },
    ]
}

/// Build the global network a row describes, answering its id and its site ids.
async fn build_network(emulator: &Emulator, case: &NetworkCase) -> (String, Vec<String>) {
    let name = case.name;
    let network = emulator.create_global_network(name).await;
    let mut sites: Vec<String> = Vec::new();
    for index in 0..case.sites {
        sites.push(emulator.create_site(&network, &format!("{name}-{index}")).await);
    }
    for _ in 0..case.links {
        let site = sites
            .first()
            .unwrap_or_else(|| panic!("case {name:?}: a link needs a site"));
        create_link_ok(emulator, &network, site, &plain_link()).await;
    }
    for index in 0..case.devices {
        let body = match (index, sites.first()) {
            (0, Some(site)) => json!({ "SiteId": site }),
            _ => json!({}),
        };
        create_device_ok(emulator, &network, body).await;
    }
    if case.core_network {
        let (status, created) =
            create_core_network(emulator, json!({ "GlobalNetworkId": network })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateCoreNetwork expected 200, got {status} with {created}"
        );
    }
    (network, sites)
}

/// Every dependent kind a `DeleteGlobalNetwork` refusal can name, as the refusal spells it
/// (the ARN resource type read as words). Used to assert that a refusal names the kinds that
/// block it and NONE of the others.
pub(crate) const DEPENDENT_LABELS: [&str; 5] = [
    "device",
    "link",
    "site",
    "transit gateway registration",
    "core network",
];

/// Assert `body` is the envelope every dependency refusal of this service carries: HTTP 400,
/// `ValidationException`, the catch-all reason, no field at fault.
fn assert_dependency_envelope(case: &str, status: u16, body: &Value) {
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
}

/// Assert `body` is the dependency refusal of a delete addressed at a CONTAINER: the
/// envelope above, naming every kind in `blocking` and no other dependent kind.
pub(crate) fn assert_dependency_refusal(
    case: &str,
    parent_label: &str,
    status: u16,
    body: &Value,
    blocking: &[&str],
) {
    assert_dependency_envelope(case, status, body);
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {body}"));
    assert!(
        message.contains(parent_label),
        "case {case:?}: expected the message to name the resource being deleted ({parent_label}), got {message:?}"
    );
    for label in blocking {
        assert!(
            message.contains(label),
            "case {case:?}: expected the message to name the blocking {label}, got {message:?}"
        );
    }
    for label in DEPENDENT_LABELS {
        if !blocking.contains(&label) {
            assert!(
                !message.contains(label),
                "case {case:?}: no {label} blocks this delete, so the message must not name one, got {message:?}"
            );
        }
    }
}

/// Assert `body` is the dependency refusal of a delete addressed at one of a global
/// network's own OBJECTS: the envelope above, and the whole message, which names the object
/// by its kind and the id the request addressed it with and no dependent kind.
pub(crate) fn assert_object_dependency_refusal(
    case: &str,
    object_label: &str,
    object_id: &str,
    status: u16,
    body: &Value,
) {
    assert_dependency_envelope(case, status, body);
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {body}"));
    let expected = crate::delete_live_reference_refusal::refusal_message(object_label, object_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn delete_global_network_blocked_by_dependents_is_a_validation_failure_for_every_kind_held() {
    for case in network_cases() {
        let emulator = Emulator::new();
        let name = case.name;
        let (network, sites) = build_network(&emulator, &case).await;

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

        assert_dependency_refusal(name, "Global Network", status, &body, case.blocking);

        // The refusal changed nothing: the global network and its children are still there.
        let (status, described) = emulator.get("/global-networks").await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeGlobalNetworks expected 200, got {status} with {described}"
        );
        assert!(
            ids_of(&described, "GlobalNetworks", "GlobalNetworkId").contains(&network),
            "case {name:?}: a refused delete must leave the global network stored, got {described}"
        );
        let (_, listed_sites) = emulator
            .get(&format!("/global-networks/{network}/sites"))
            .await;
        assert_eq!(
            ids_of(&listed_sites, "Sites", "SiteId").len(),
            sites.len(),
            "case {name:?}: a refused delete must leave the sites alone, got {listed_sites}"
        );

        // Once the only blocking kind is gone the identical call succeeds, so the refusal
        // tracked stored state rather than the shape of the request. Only the rows whose
        // children can all be removed today can show it: `DeleteLink`, `DeleteDevice` and
        // `DeleteCoreNetwork` are still scaffold stubs, so a row holding one of those
        // cannot be emptied through the API.
        if case.blocking == ["site"] && case.links == 0 {
            for site in &sites {
                let (status, deleted) = emulator
                    .delete(&format!("/global-networks/{network}/sites/{site}"))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: DeleteSite expected 200, got {status} with {deleted}"
                );
            }
            let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: with the sites gone the same delete must succeed, got {status} with {deleted}"
            );
        }
    }
}

/// One row of the `DeleteSite` table: the children to hang off the site, and the kinds the
/// refusal must then name.
struct SiteCase {
    name: &'static str,
    links: usize,
    devices: usize,
    blocking: &'static [&'static str],
}

/// "Deletes an existing site. The site cannot be associated with any device or link."
/// (`smithy_query query="doc" shape="DeleteSite"`) — the rows differ along which of the
/// two kinds is associated and how many, and the last row holds neither.
fn site_cases() -> Vec<SiteCase> {
    vec![
        SiteCase {
            name: "one link",
            links: 1,
            devices: 0,
            blocking: &["link"],
        },
        SiteCase {
            name: "three links",
            links: 3,
            devices: 0,
            blocking: &["link"],
        },
        SiteCase {
            name: "one device",
            links: 0,
            devices: 1,
            blocking: &["device"],
        },
        SiteCase {
            name: "two devices and two links",
            links: 2,
            devices: 2,
            blocking: &["device", "link"],
        },
        SiteCase {
            name: "neither",
            links: 0,
            devices: 0,
            blocking: &[],
        },
    ]
}

#[tokio::test]
async fn delete_site_blocked_by_its_devices_or_links_is_the_same_validation_failure() {
    for case in site_cases() {
        let emulator = Emulator::new();
        let name = case.name;
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;
        for _ in 0..case.links {
            create_link_ok(&emulator, &network, &site, &plain_link()).await;
        }
        for _ in 0..case.devices {
            create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        }

        let (status, body) = emulator
            .delete(&format!("/global-networks/{network}/sites/{site}"))
            .await;

        if case.blocking.is_empty() {
            assert_eq!(
                status, 200,
                "case {name:?}: a site holding neither deletes, got {status} with {body}"
            );
            assert_eq!(
                body["Site"]["State"],
                json!("DELETING"),
                "case {name:?}: the removed record reports DELETING, got {body}"
            );
            continue;
        }

        assert_object_dependency_refusal(name, "Site", &site, status, &body);

        // A sibling site holding nothing still deletes, so the refusal is about the
        // dependents of the site named, not about the operation.
        let empty = emulator.create_site(&network, "empty").await;
        let (status, deleted) = emulator
            .delete(&format!("/global-networks/{network}/sites/{empty}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: an empty sibling site must still delete, got {status} with {deleted}"
        );
    }
}

#[tokio::test]
async fn deleting_the_core_network_unblocks_the_global_network_delete() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("core network teardown").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({ "GlobalNetworkId": network, "Description": "blocking child" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();

    let (status, blocked) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_dependency_refusal(
        "a core network",
        "Global Network",
        status,
        &blocked,
        &["core network"],
    );

    // DeleteCoreNetwork removes the record and reports it as DELETING.
    let (status, deleted) = emulator
        .delete(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["CoreNetwork"]["CoreNetworkId"],
        json!(core_network),
        "DeleteCoreNetwork must answer the record it removed, got {deleted}"
    );
    assert_eq!(
        deleted["CoreNetwork"]["State"],
        json!("DELETING"),
        "DeleteCoreNetwork must report the removed record as DELETING, got {deleted}"
    );

    // The removal is in stored state: the reader no longer finds it, and neither does a
    // second delete.
    let (status, read) = emulator.get(&format!("/core-networks/{core_network}")).await;
    assert_eq!(
        status, 404,
        "GetCoreNetwork after the delete expected 404, got {status} with {read}"
    );
    assert_eq!(
        error_code(&read),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {read}"
    );
    let (status, again) = emulator
        .delete(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 404,
        "a second DeleteCoreNetwork expected 404, got {status} with {again}"
    );

    // And the identical DeleteGlobalNetwork that was refused now succeeds.
    let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "with the core network gone the same delete must succeed, got {status} with {deleted}"
    );
    assert_eq!(
        deleted["GlobalNetwork"]["State"],
        json!("DELETING"),
        "the removed global network reports DELETING, got {deleted}"
    );
}

#[tokio::test]
async fn a_conflict_with_the_resources_own_state_is_still_the_409_conflict_exception() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("token reuse").await;
    let request = json!({
        "GlobalNetworkId": network,
        "Description": "first",
        "ClientToken": "token-boundary",
    });
    let (status, created) = create_core_network(&emulator, request.clone()).await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );

    // The same idempotency token asking for something else conflicts with what the token
    // already created: the caller can clear it by changing the token, so it stays the 409
    // ConflictException the dependency refusals no longer use.
    let mut different = request;
    different["Description"] = json!("second");
    let (status, conflict) = create_core_network(&emulator, different).await;
    assert_eq!(
        status, 409,
        "a reused client token with a different request is HTTP 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );
}
