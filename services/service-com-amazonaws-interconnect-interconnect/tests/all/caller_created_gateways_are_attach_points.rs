// SPDX-License-Identifier: CC0-1.0

//! A Direct Connect gateway the CALLER created is what `ListAttachPoints` reports.
//!
//! An attach point is not a catalog this service publishes: `AttachPointType`'s only
//! modelled value is `DirectConnectGateway` and `AttachPoint$directConnectGateway`
//! identifies one "by DirectConnectGatewayID", so the listing is the account's own Direct
//! Connect gateways — state of the service that owns them. These suites drive both
//! services on one engine (the `crate::emulator` harness) and assert the rule over
//! tables whose rows differ in how many gateways the account holds, in each gateway's
//! name and `amazonSideAsn`, in the environment the listing is asked for, and in the
//! gateway's own lifecycle state.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emulator::{
    available_bandwidths, creatable_environments, text, Emulator, ATTACH_POINT_TYPE,
};

/// A well-formed `DirectConnectGatewayAttachPoint` (36 characters in the UUID groups the
/// member's pattern requires) that names no gateway of the account, so it must never be
/// reported as an attach point.
const UNOWNED_GATEWAY_UUID: &str = "00000000-0000-4000-8000-000000000000";

/// The `identifier`s of a listing, as a set.
fn identifiers(listed: &[Value]) -> BTreeSet<String> {
    listed
        .iter()
        .map(|descriptor| text(descriptor, "identifier"))
        .collect()
}

/// RULE: the attach points an Environment offers are exactly the Direct Connect gateways
/// the account holds — so creating one makes it listable, and the listing grows with the
/// account rather than with the environment id.
#[tokio::test]
async fn a_created_gateway_becomes_a_listed_attach_point() {
    let emulator = Emulator::new();
    let environments: Vec<String> = emulator
        .environments()
        .await
        .iter()
        .map(|environment| text(environment, "environmentId"))
        .collect();
    assert!(
        !environments.is_empty(),
        "case \"catalog\": expected the seeded Environment catalog to be non-empty, got {environments:?}"
    );

    // Rows differ in how many gateways the account holds (none, one, then one more each
    // time) and in each gateway's name shape and `amazonSideAsn` — absent, and one from
    // each private range `CreateDirectConnectGatewayRequest$amazonSideAsn` documents.
    let rows: [(&str, Option<i64>); 4] = [
        ("ic-owned-default-asn", None),
        ("ic owned spaced name", Some(64_512)),
        ("ic-owned-high-16bit-asn", Some(65_534)),
        ("ic-owned-32bit-asn", Some(4_294_967_294)),
    ];

    let mut owned: BTreeSet<String> = BTreeSet::new();
    for (case, name, asn) in std::iter::once(("no gateway at all", None, None)).chain(
        rows.iter()
            .map(|(name, asn)| ("one more gateway", Some(*name), *asn)),
    ) {
        if let Some(name) = name {
            let ids = emulator.ready_gateways(&[(name, asn)]).await;
            owned.extend(ids);
        }
        // EVERY environment of the catalog reports the same account-owned set: the
        // gateways are account state, so the environment the caller names does not
        // change which ones exist.
        for environment_id in &environments {
            let listed = emulator.attach_points(environment_id).await;
            assert_eq!(
                identifiers(&listed),
                owned,
                "case {case:?} ({} owned) / {environment_id:?}: expected the account's gateways, got {listed:?}",
                owned.len()
            );
            for descriptor in &listed {
                assert_eq!(
                    text(descriptor, "type"),
                    ATTACH_POINT_TYPE,
                    "case {case:?} / {environment_id:?}: expected the only modelled AttachPointType, got {:?}",
                    descriptor["type"]
                );
                assert!(
                    !text(descriptor, "name").is_empty(),
                    "case {case:?} / {environment_id:?}: expected a descriptive name, got {:?}",
                    descriptor["name"]
                );
            }
            assert!(
                !identifiers(&listed).contains(UNOWNED_GATEWAY_UUID),
                "case {case:?} / {environment_id:?}: expected no descriptor for a gateway the account does not hold, got {listed:?}"
            );
        }
    }
}

/// RULE: a gateway is an attach point once Direct Connect reports it ready for use
/// (`available`); while it is still being created (`pending`) it is not one yet.
#[tokio::test]
async fn a_gateway_is_an_attach_point_once_direct_connect_reports_it_available() {
    let emulator = Emulator::new();
    let environment_id = text(&emulator.environments().await[0], "environmentId");

    // Rows differ in the gateway's name and ASN; each is read straight back (the
    // transient state the clock owes is never zero-length) and then after it settles.
    for (name, asn) in [("ic-pending-a", None), ("ic-pending-b", Some(64_512))] {
        let id = emulator.create_gateway(name, asn).await;
        let state = emulator.gateway_state(&id).await;
        assert_eq!(
            state, "pending",
            "case {name:?}: expected a gateway read straight back to be pending, got {state:?}"
        );
        let pending = emulator.attach_points(&environment_id).await;
        assert!(
            !identifiers(&pending).contains(&id),
            "case {name:?}: expected a {state} gateway not to be an attach point yet, got {pending:?}"
        );

        // Once the clock has made the transition due, the same gateway is listed.
        let started = std::time::Instant::now();
        let available = loop {
            if emulator.gateway_state(&id).await == "available" {
                break emulator.attach_points(&environment_id).await;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "case {name:?}: expected the gateway to become available within 5s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(
            identifiers(&available).contains(&id),
            "case {name:?}: expected an available gateway to be an attach point, got {available:?}"
        );
    }
}

/// RULE: an attach point the listing reports is usable as `CreateConnection$attachPoint`,
/// and the connection created on it reports that gateway back through `GetConnection` —
/// so the path from "I own a gateway" to "I have a Connection on it" closes.
#[tokio::test]
async fn a_listed_gateway_carries_a_connection_through_its_whole_lifecycle() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator
        .ready_gateways(&[("ic-lifecycle-a", None), ("ic-lifecycle-b", Some(64_512))])
        .await;
    assert_eq!(
        gateways.len(),
        2,
        "setup: expected two gateways, got {gateways:?}"
    );

    // Rows differ in the environment (and so in the provider kind and the
    // `ConnectionId` prefix), in the bandwidth, and in the gateway attached to.
    let mut rows = 0;
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let listed = emulator.attach_points(&environment_id).await;
        let owned: BTreeSet<String> = gateways.iter().cloned().collect();
        assert_eq!(
            identifiers(&listed),
            owned,
            "case {environment_id:?}: expected the created gateways to be listed, got {listed:?}"
        );

        for (index, bandwidth) in available_bandwidths(&environment).iter().enumerate() {
            let gateway = &gateways[index % gateways.len()];
            let case = format!("{environment_id} / {bandwidth} / {gateway}");
            let connection = emulator
                .create_connection(&environment_id, bandwidth, gateway, "on an owned gateway")
                .await;
            let connection_id = text(&connection, "id");
            assert_eq!(
                connection["attachPoint"],
                json!({ "directConnectGateway": gateway }),
                "case {case:?}: expected the connection to carry the gateway it was created on, got {:?}",
                connection["attachPoint"]
            );

            let (status, fetched) = emulator.get_connection(&connection_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the connection to be readable, got {status} with {fetched:?}"
            );
            assert_eq!(
                fetched["connection"]["attachPoint"],
                json!({ "directConnectGateway": gateway }),
                "case {case:?}: expected GetConnection to report the same attach point, got {:?}",
                fetched["connection"]["attachPoint"]
            );

            // The connection is deleted on the same gateway, and the listing is
            // untouched by either call: a Connection does not consume an attach point.
            let deleted = emulator
                .ic_ok("DeleteConnection", json!({ "identifier": connection_id }))
                .await;
            assert!(
                ["deleting", "deleted"]
                    .contains(&text(&deleted["connection"], "state").as_str()),
                "case {case:?}: expected the delete to report a tear-down state, got {:?}",
                deleted["connection"]["state"]
            );
            let after = emulator.attach_points(&environment_id).await;
            assert_eq!(
                after, listed,
                "case {case:?}: expected the attach-point listing to be unchanged, got {after:?}"
            );
            rows += 1;
        }
    }
    assert!(
        rows >= 2,
        "case \"coverage\": expected the table to span at least two rows, got {rows}"
    );
}
