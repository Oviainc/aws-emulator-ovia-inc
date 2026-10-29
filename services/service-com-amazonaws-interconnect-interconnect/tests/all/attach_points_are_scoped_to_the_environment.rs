// SPDX-License-Identifier: CC0-1.0

//! `ListAttachPoints` is a listing FOR an Environment, not a listing of the account.
//!
//! The operation reports "all Attach Points the caller has access to that are valid for
//! the specified Environment", and `ListAttachPointsRequest$environmentId` is "the
//! identifier of the Environment for which to list valid Attach Points" — so the member
//! has to have an effect on the page: an Environment that can carry no Connection has no
//! attach point valid for it, whatever gateways the account holds, because an attach
//! point is where a Connection on THAT Environment "will logically connect"
//! (`CreateConnection$attachPoint`).
//!
//! Every row below is built from the LIVE Environment catalog and the LIVE gateways of
//! the account rather than from a copy of either, and the rows differ in the
//! Environment's state, its provider kind, its location, the bandwidths it offers and
//! how many gateways the account holds.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emulator::{
    available_bandwidths, carries_connections, code, creatable_environments, identifiers, message,
    remote_account_for, text, Emulator,
};

/// The gateway rows the account is stocked with: the name's shape and the
/// `amazonSideAsn` differ per row (absent, and one from each private range
/// `CreateDirectConnectGatewayRequest$amazonSideAsn` documents), so no assertion is
/// keyed to one form of gateway.
const GATEWAY_ROWS: [(&str, Option<i64>); 3] = [
    ("ic-scoped-default-asn", None),
    ("ic scoped spaced name", Some(64_512)),
    ("ic-scoped-32bit-asn", Some(4_200_000_000)),
];

/// RULE: the attach points listed for an Environment are those valid FOR it — the
/// account's ready gateways on an Environment that can carry a Connection, and none at
/// all on one that cannot — so two environments of the same account can report different
/// pages.
#[tokio::test]
async fn the_listing_is_scoped_to_the_environment_it_is_asked_for() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let owned: BTreeSet<String> = emulator
        .ready_gateways(&GATEWAY_ROWS)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        owned.len(),
        GATEWAY_ROWS.len(),
        "setup: expected one gateway per row, got {owned:?}"
    );

    // One row per seeded Environment: they differ in state, provider kind, location and
    // the bandwidths they offer.
    let mut carrying = 0;
    let mut not_carrying = 0;
    for environment in &catalog {
        let id = text(environment, "environmentId");
        let listed = emulator.attach_points(&id).await;
        let case = format!("{id} / state {}", text(environment, "state"));
        if carries_connections(environment) {
            carrying += 1;
            assert_eq!(
                identifiers(&listed),
                owned,
                "case {case:?}: expected the account's ready gateways to be valid here, got {listed:?}"
            );
        } else {
            not_carrying += 1;
            assert!(
                listed.is_empty(),
                "case {case:?}: expected no attach point valid for an environment that carries no connection, got {listed:?}"
            );
        }
    }
    assert!(
        carrying >= 1 && not_carrying >= 1,
        "case \"catalog\": expected the catalog to span environments that do and do not carry connections, got {carrying} and {not_carrying}"
    );

    // The member is therefore NOT ignored: the two pages differ, which is what lets a
    // caller tell the attach points one environment accepts from another's.
    let carrying_id = text(&creatable_environments(&catalog)[0], "environmentId");
    let barren_id = catalog
        .iter()
        .find(|environment| !carries_connections(environment))
        .map(|environment| text(environment, "environmentId"))
        .expect("the catalog holds an environment that carries no connection");
    let carrying_page = emulator.attach_points(&carrying_id).await;
    let barren_page = emulator.attach_points(&barren_id).await;
    assert_ne!(
        carrying_page, barren_page,
        "case \"{carrying_id} vs {barren_id}\": expected environmentId to scope the page, got {carrying_page:?} twice"
    );

    // Reading either page changes nothing a later caller sees.
    for id in [&carrying_id, &barren_id] {
        let before = emulator.attach_points(id).await;
        let again = emulator.attach_points(id).await;
        assert_eq!(
            again, before,
            "case {id:?}: expected a repeated read to report the same page, got {again:?}"
        );
    }
}

/// RULE: the listing and the attach-point refusal are the same judgement — an identifier
/// the listing reports for an Environment creates a Connection on it, and one it does
/// not report is refused with the modelled `ResourceNotFoundException` (404) naming the
/// identifier and the Environment it was offered for.
#[tokio::test]
async fn a_create_accepts_exactly_what_the_environments_listing_offers() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;

    // Rows differ in the environment and, for each, in the gateway and the bandwidth
    // used — and the environments differ in whether they can carry a Connection at all.
    let mut rows = 0;
    for (index, environment) in catalog.iter().enumerate() {
        let id = text(environment, "environmentId");
        let listed = identifiers(&emulator.attach_points(&id).await);
        for gateway in &gateways {
            let bandwidths = available_bandwidths(environment);
            // An environment offering no bandwidth refuses every value for the required
            // member, so the one sent there is the first bandwidth the catalog offers
            // anywhere: what is under test is the attach point, and the refusal order is
            // asserted by the state checks the create makes first.
            let bandwidth = bandwidths
                .first()
                .cloned()
                .unwrap_or_else(|| available_bandwidths(&creatable_environments(&catalog)[0])[0].clone());
            let attach_point = json!({ "directConnectGateway": gateway });
            let mut request = serde_json::Map::new();
            request.insert("environmentId".to_string(), json!(id));
            request.insert("bandwidth".to_string(), json!(bandwidth));
            request.insert("attachPoint".to_string(), attach_point.clone());
            request.insert(
                "description".to_string(),
                json!(format!("scoped case {index} {rows}")),
            );
            if let Some(remote_account) = remote_account_for(environment) {
                request.insert("remoteAccount".to_string(), remote_account);
            }
            let (status, body) = emulator
                .ic("CreateConnection", Value::Object(request))
                .await;
            let case = format!("{id} / {gateway} / {bandwidth}");

            if listed.contains(gateway) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a listed attach point to be accepted, got {status} with {body:?}"
                );
                assert_eq!(
                    body["connection"]["attachPoint"], attach_point,
                    "case {case:?}: expected the connection to carry the attach point it was created on, got {:?}",
                    body["connection"]["attachPoint"]
                );
            } else {
                assert_ne!(
                    status, 200,
                    "case {case:?}: expected an attach point the listing does not offer to be refused, got {body:?}"
                );
            }
            rows += 1;
        }
    }
    assert!(
        rows >= 4,
        "case \"coverage\": expected the table to span at least four rows, got {rows}"
    );
}

/// RULE: an attach point an Environment's listing does not offer is never accepted for
/// it — an identifier the account does not hold is the modelled `ResourceNotFoundException`
/// (404), worded from the identifier the request carried and the Environment it was
/// offered for, and an Environment that carries no Connection refuses the request by its
/// own state (`InterconnectClientException`, 400) before an attach point is judged at all.
#[tokio::test]
async fn an_attach_point_outside_the_environments_listing_is_never_accepted() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let owned = emulator
        .ready_gateways(&[("ic-scoped-refusal", None)])
        .await
        .remove(0);
    let carrying = creatable_environments(&catalog);
    let barren: Vec<Value> = catalog
        .iter()
        .filter(|environment| !carries_connections(environment))
        .cloned()
        .collect();
    assert!(
        !barren.is_empty(),
        "case \"catalog\": expected an environment that carries no connection, got {catalog:?}"
    );

    // Rows differ in WHY the attach point is not valid, and each row names the refusal
    // it expects: an identifier the account does not hold at all, offered for an
    // environment that carries connections, is the attach-point refusal itself (the
    // modelled `ResourceNotFoundException`, 404, in the shared wording); a gateway the
    // account DOES hold, offered for an environment that carries none, is refused by that
    // environment's own state first (`InterconnectClientException`, 400), which is where
    // the request stops before an attach point is judged at all.
    let unowned = "00000000-0000-4000-8000-000000000000".to_string();
    let rows: Vec<(Value, String, u16, &str)> = carrying
        .iter()
        .map(|environment| {
            (
                environment.clone(),
                unowned.clone(),
                404,
                "ResourceNotFoundException",
            )
        })
        .chain(barren.iter().map(|environment| {
            (
                environment.clone(),
                owned.clone(),
                400,
                "InterconnectClientException",
            )
        }))
        .collect();

    for (environment, identifier, expected_status, expected_code) in rows {
        let environment_id = text(&environment, "environmentId");
        let listed = identifiers(&emulator.attach_points(&environment_id).await);
        assert!(
            !listed.contains(&identifier),
            "case \"{environment_id} / {identifier}\": expected the listing not to offer this attach point, got {listed:?}"
        );

        // A bandwidth the environment offers where it offers any, so the row is refused
        // for the reason it names and not for its bandwidth; an environment offering none
        // refuses every value anyway, and is refused by its state before the bandwidth.
        let bandwidth = available_bandwidths(&environment)
            .first()
            .cloned()
            .unwrap_or_else(|| available_bandwidths(&carrying[0])[0].clone());
        let mut request = serde_json::Map::new();
        request.insert("environmentId".to_string(), json!(environment_id));
        request.insert("bandwidth".to_string(), json!(bandwidth));
        request.insert(
            "attachPoint".to_string(),
            json!({ "directConnectGateway": identifier }),
        );
        if let Some(remote_account) = remote_account_for(&environment) {
            request.insert("remoteAccount".to_string(), remote_account);
        }
        let (status, body) = emulator
            .ic("CreateConnection", Value::Object(request))
            .await;
        let case = format!("{environment_id} / {identifier}");
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for an attach point outside the listing, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(expected_code),
            "case {case:?}: expected {expected_code}, got {body:?}"
        );
        // The refusal names the Environment the request was for, and the attach-point
        // refusal names the identifier it judged as well — one wording, built from the
        // request rather than from a stored value.
        assert!(
            message(&body).contains(&environment_id),
            "case {case:?}: expected the refusal to name the environment, got {:?}",
            message(&body)
        );
        if expected_code == "ResourceNotFoundException" {
            assert!(
                message(&body).contains(&identifier),
                "case {case:?}: expected the refusal to name the identifier it judged, got {:?}",
                message(&body)
            );
        }
    }
}
