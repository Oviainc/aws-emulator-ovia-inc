// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteConnection`, driven over HTTP through the real router.
//!
//! The rule these suites pin: a delete of a resource addressed through its parent removes
//! EXACTLY the record the path names from stored state and answers that record in the
//! `DELETING` state — so the reader no longer finds it, a repeated delete is the
//! missing-resource answer (404 `ResourceNotFoundException`), and nothing the path did not
//! name changes: its siblings, the records of other parents, and the devices and links the
//! removed record merely REFERENCED all stay. A path that addresses no stored record (an
//! unknown parent, an unknown resource, or one stored under a different parent) is that
//! same missing-resource answer and removes nothing.
//!
//! The tables below span the class the rule generalises over: which optional members the
//! deleted connection carries, whether the `PENDING` -> `AVAILABLE` transition the create
//! owes is already past, and how many sibling connections sit beside it. The harness and
//! the connection helpers are the ones `global_network_lifecycle` and
//! `connection_lifecycle` define — a connection is only ever read back through
//! `GetConnections`.

use serde_json::{json, Value};

use crate::connection_lifecycle::{
    base_tags, create_connection_ok, create_seeded_connection, fixture, get_connections, id_of,
    ids_in, read_connection, without_state, BASE_DESCRIPTION, CONNECTION, GLOBAL_NETWORK,
    NOT_FOUND_MESSAGE,
};
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};

/// `DeleteConnection` on `connection` of `network`.
async fn delete_connection(emulator: &Emulator, network: &str, connection: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/connections/{connection}"
        ))
        .await
}

/// Assert `body` is this service's missing-resource answer for `resource_id` of
/// `resource_type`.
fn assert_not_found(case: &str, status: u16, body: &Value, resource_id: &str, resource_type: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: wrong error code in {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(NOT_FOUND_MESSAGE),
        "case {case:?}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {body}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: expected ResourceId {resource_id:?}, got {body}"
    );
    assert_eq!(
        body["ResourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected ResourceType {resource_type:?}, got {body}"
    );
}

/// One row of the delete table: the members the connection to delete carries, whether to
/// wait out the settle span the create owes before deleting, and how many sibling
/// connections share its global network.
struct DeleteCase {
    case: &'static str,
    /// The create body, minus the two required device members the harness adds.
    body: Value,
    /// Whether the `PENDING` -> `AVAILABLE` transition is past before the delete.
    settled: bool,
    siblings: usize,
}

/// The rows differ along which optional members the deleted record holds (both links, one,
/// neither, a description of several forms, tags), along the lifecycle state it is in when
/// the delete arrives, and along how many siblings the same global network holds — the
/// dimensions the removal rule must be indifferent to.
fn delete_cases(seed: &crate::connection_lifecycle::Fixture) -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "both links, a description and tags, settled, no sibling",
            body: json!({
                "Description": BASE_DESCRIPTION,
                "LinkId": seed.link,
                "ConnectedLinkId": seed.connected_link,
                "Tags": base_tags(),
            }),
            settled: true,
            siblings: 0,
        },
        DeleteCase {
            case: "no optional member at all",
            body: json!({}),
            settled: true,
            siblings: 0,
        },
        DeleteCase {
            case: "still PENDING when the delete arrives",
            body: json!({ "Description": "deleted before it settles" }),
            settled: false,
            siblings: 0,
        },
        DeleteCase {
            case: "one sibling in the same global network",
            body: json!({ "LinkId": seed.link }),
            settled: true,
            siblings: 1,
        },
        DeleteCase {
            case: "three siblings in the same global network",
            body: json!({ "ConnectedLinkId": seed.connected_link }),
            settled: false,
            siblings: 3,
        },
        DeleteCase {
            case: "an empty description",
            body: json!({ "Description": "" }),
            settled: true,
            siblings: 2,
        },
        DeleteCase {
            case: "a description at the modelled maximum of 256 characters",
            body: json!({ "Description": "d".repeat(256), "Tags": base_tags() }),
            settled: false,
            siblings: 1,
        },
        DeleteCase {
            case: "a multi-line, non-ascii description over the same link at both ends",
            body: json!({
                "Description": "line one\nline two \u{1f310}",
                "LinkId": seed.link,
                "ConnectedLinkId": seed.link,
            }),
            settled: true,
            siblings: 0,
        },
    ]
}

#[tokio::test]
async fn delete_connection_removes_only_the_addressed_record_and_answers_it_deleting() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "delete-connection").await;

    // A connection of ANOTHER global network, which no delete in this suite names: it must
    // survive every row.
    let elsewhere = fixture(&emulator, "delete-connection-other").await;
    let untouched = create_seeded_connection(&emulator, &elsewhere).await;

    for row in delete_cases(&seed) {
        let case = row.case;

        let mut body = row.body.clone();
        let object = body
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {case:?}: the row body must be an object"));
        object.insert("DeviceId".to_string(), json!(seed.device));
        object.insert(
            "ConnectedDeviceId".to_string(),
            json!(seed.connected_device),
        );

        let connection_id = id_of(&create_connection_ok(&emulator, &seed.network, body).await);
        let mut siblings = Vec::new();
        for _ in 0..row.siblings {
            siblings.push(create_seeded_connection(&emulator, &seed).await);
        }
        if row.settled {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // The record as stored, so the delete's answer can be compared against it member
        // for member rather than against the request that created it.
        let before = read_connection(&emulator, &seed.network, &connection_id).await;

        let (status, answer) = delete_connection(&emulator, &seed.network, &connection_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer}"
        );
        let deleted = &answer["Connection"];
        assert_eq!(
            without_state(deleted),
            without_state(&before),
            "case {case:?}: the delete must answer the record it removed, got {deleted}"
        );
        assert_eq!(
            deleted["State"],
            json!("DELETING"),
            "case {case:?}: the removed record reports DELETING, got {deleted}"
        );

        // The removal is in stored state: the reader no longer finds it.
        let (status, listed) = get_connections(
            &emulator,
            &seed.network,
            &format!("connectionIds={connection_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConnections expected 200, got {status} with {listed}"
        );
        assert_eq!(
            ids_in(&listed),
            Vec::<String>::new(),
            "case {case:?}: the deleted connection must no longer be listed, got {listed}"
        );

        // Only the addressed record went: the siblings of the same global network are all
        // still there, and nothing else is.
        let mut expected = siblings.clone();
        expected.sort();
        let (_, remaining) = get_connections(&emulator, &seed.network, "").await;
        assert_eq!(
            ids_in(&remaining),
            expected,
            "case {case:?}: expected exactly the siblings {expected:?} to remain, got {remaining}"
        );

        // A second, identical delete addresses nothing and removes nothing.
        let (status, again) = delete_connection(&emulator, &seed.network, &connection_id).await;
        assert_not_found(case, status, &again, &connection_id, CONNECTION);

        // The devices and links the removed connection referenced are its parents, not its
        // children: they outlive it.
        let (_, devices) = emulator
            .get(&format!("/global-networks/{}/devices", seed.network))
            .await;
        let device_ids = ids_of(&devices, "Devices", "DeviceId");
        for device in [&seed.device, &seed.connected_device] {
            assert!(
                device_ids.contains(device),
                "case {case:?}: the referenced device {device} must outlive the connection, got {devices}"
            );
        }
        let (_, links) = emulator
            .get(&format!("/global-networks/{}/links", seed.network))
            .await;
        let link_ids = ids_of(&links, "Links", "LinkId");
        for link in [&seed.link, &seed.connected_link] {
            assert!(
                link_ids.contains(link),
                "case {case:?}: the referenced link {link} must outlive the connection, got {links}"
            );
        }

        // Leave the global network as the next row expects to find it.
        for sibling in &siblings {
            let (status, removed) = delete_connection(&emulator, &seed.network, sibling).await;
            assert_eq!(
                status, 200,
                "case {case:?}: clearing the sibling {sibling} expected 200, got {status} with {removed}"
            );
        }

        // No row reached the connection of the other global network.
        let survivor = read_connection(&emulator, &elsewhere.network, &untouched).await;
        assert_eq!(
            survivor["Description"],
            json!(BASE_DESCRIPTION),
            "case {case:?}: a delete in one global network must leave another's records alone, got {survivor}"
        );
    }
}

#[tokio::test]
async fn delete_connection_refuses_a_path_that_addresses_no_stored_record_and_removes_nothing() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "delete-connection-not-found").await;
    let other = fixture(&emulator, "delete-connection-not-found-other").await;
    let connection_id = create_seeded_connection(&emulator, &seed).await;

    // Well-formed ids the store never minted, plus one it minted and then removed.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_connection = "connection-00000000000000099".to_string();
    let removed = create_seeded_connection(&emulator, &seed).await;
    let (status, answer) = delete_connection(&emulator, &seed.network, &removed).await;
    assert_eq!(
        status, 200,
        "seeding a removed connection: expected 200, got {status} with {answer}"
    );

    // The rows differ along WHICH part of the path addresses nothing: the parent, the
    // resource, the pairing of the two, and a resource that existed and no longer does.
    let rows = vec![
        (
            "a global network that never existed",
            unknown_network.clone(),
            connection_id.clone(),
            unknown_network.clone(),
            GLOBAL_NETWORK,
        ),
        (
            "a connection that never existed",
            seed.network.clone(),
            unknown_connection.clone(),
            unknown_connection.clone(),
            CONNECTION,
        ),
        (
            "a live connection addressed through another global network",
            other.network.clone(),
            connection_id.clone(),
            connection_id.clone(),
            CONNECTION,
        ),
        (
            "a connection this store minted and already removed",
            seed.network.clone(),
            removed.clone(),
            removed.clone(),
            CONNECTION,
        ),
        (
            "neither the global network nor the connection exists",
            unknown_network.clone(),
            unknown_connection.clone(),
            unknown_network.clone(),
            GLOBAL_NETWORK,
        ),
    ];

    for (case, network, connection, resource_id, resource_type) in rows {
        let (status, answer) = delete_connection(&emulator, &network, &connection).await;
        assert_not_found(case, status, &answer, &resource_id, resource_type);
    }

    // Every refusal above removed nothing: the live connection is still the record it was.
    let stored = read_connection(&emulator, &seed.network, &connection_id).await;
    assert_eq!(
        stored["Description"],
        json!(BASE_DESCRIPTION),
        "case refused deletes: the live connection must keep its stored Description, got {stored}"
    );
    assert_eq!(
        ids_in(&get_connections(&emulator, &seed.network, "").await.1),
        vec![connection_id.clone()],
        "case refused deletes: the global network must still hold exactly its one connection"
    );

    // And the delete that does address it succeeds, so the refusals tracked stored state
    // rather than the shape of the request.
    let (status, answer) = delete_connection(&emulator, &seed.network, &connection_id).await;
    assert_eq!(
        status, 200,
        "case the addressed connection: expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["Connection"]["ConnectionId"],
        json!(connection_id),
        "case the addressed connection: the answer must name the removed record, got {answer}"
    );
}
