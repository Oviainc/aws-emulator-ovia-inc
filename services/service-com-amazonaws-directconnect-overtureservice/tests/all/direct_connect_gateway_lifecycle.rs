// SPDX-License-Identifier: CC0-1.0

//! The Direct Connect gateway lifecycle, end to end over the wire.
//!
//! Every assertion goes through `POST /` with `X-Amz-Target: OvertureService.<Op>`
//! (awsJson1_1), the same path a client takes, and every expectation is about what
//! the SERVICE STORED: a `CreateDirectConnectGateway` is proved by reading the
//! gateway back through `DescribeDirectConnectGateways`.
//!
//! The tables span the class each rule generalises over (ASN range and bound, tag
//! count, name shape, page size, filter), and the ASN tables are BUILT from the
//! service's own `PRIVATE_AMAZON_SIDE_ASN_RANGES` — the ranges
//! `CreateDirectConnectGatewayRequest$amazonSideAsn` documents — so every published
//! bound and one value past it is covered rather than a chosen few.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_directconnect_overtureservice::state::{
    DEFAULT_AMAZON_SIDE_ASN, MAX_RESULT_SET_SIZE, PRIVATE_AMAZON_SIDE_ASN_RANGES,
};

use crate::connection_and_virtual_interface_lifecycle::{error_code, Emulator};

/// Long enough for the `pending` span the service owes the clock to fall due —
/// its own `TRANSITION_DELAY` plus slack.
///
/// Awaited with `std::thread::sleep` rather than `tokio::time::sleep`: the
/// workspace enables tokio's `macros`/`rt` features only, so `tokio::time` is not
/// available here. Nothing else is pending on the test's runtime.
const SETTLE: Duration = Duration::from_millis(900);

/// The group lengths of a UUID (`8-4-4-4-12`), the form AWS reports a
/// `directConnectGatewayId` in.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// One row of the create table: case, name, `amazonSideAsn`, tags.
type GatewayRow<'a> = (&'a str, &'a str, Option<i64>, &'a [(&'a str, &'a str)]);

/// Whether `id` is a server-minted gateway id: lowercase hex in UUID groups.
fn is_minted_gateway_id(id: &Value) -> bool {
    let Some(id) = id.as_str() else { return false };
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == length
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// A `CreateDirectConnectGateway` body.
fn create_gateway_body(
    name: &str,
    amazon_side_asn: Option<i64>,
    tags: &[(&str, &str)],
) -> Value {
    let mut body = Map::new();
    body.insert("directConnectGatewayName".to_string(), json!(name));
    if let Some(asn) = amazon_side_asn {
        body.insert("amazonSideAsn".to_string(), json!(asn));
    }
    if !tags.is_empty() {
        body.insert(
            "tags".to_string(),
            Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// Create one gateway and return its id, failing the test if it was refused.
async fn gateway(dx: &Emulator, name: &str) -> String {
    let (status, body) = dx
        .call(
            "CreateDirectConnectGateway",
            create_gateway_body(name, None, &[]),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateDirectConnectGateway to succeed, got {status} with {body}"
    );
    body["directConnectGateway"]["directConnectGatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The gateways `DescribeDirectConnectGateways` answers for `body`, with the
/// response's `nextToken`.
async fn describe(dx: &Emulator, body: Value) -> (Vec<Value>, Value) {
    let (status, response) = dx.call("DescribeDirectConnectGateways", body).await;
    assert_eq!(
        status, 200,
        "expected DescribeDirectConnectGateways 200, got {status} with {response}"
    );
    (
        response["directConnectGateways"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        response["nextToken"].clone(),
    )
}

/// `gateway` without its state, so a round-trip comparison does not race the
/// `pending` -> `available` transition the clock owes.
fn without_state(gateway: &Value) -> Value {
    let mut copy = gateway.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("directConnectGatewayState");
    }
    copy
}

#[tokio::test]
async fn a_created_gateway_is_minted_stored_and_read_back_field_for_field() {
    let dx = Emulator::new();

    // (case, name, amazonSideAsn, tags). The ASNs span both private ranges the
    // service's PRIVATE_AMAZON_SIDE_ASN_RANGES names, including each bound.
    let rows: &[GatewayRow<'_>] = &[
        ("no amazonSideAsn, so the documented default", "dxgw-default", None, &[]),
        (
            "the low bound of the two-byte private range",
            "Gateway With Spaces",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[0].0),
            &[("env", "prod")],
        ),
        (
            "the high bound of the two-byte private range",
            "gw.with.dots",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[0].1),
            &[("env", "test"), ("team", "network")],
        ),
        (
            "the low bound of the four-byte private range",
            "x",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[1].0),
            &[],
        ),
        (
            "the high bound of the four-byte private range",
            "UPPER-and-lower_99",
            Some(PRIVATE_AMAZON_SIDE_ASN_RANGES[1].1),
            &[("only", "one-tag")],
        ),
        (
            "a value inside the four-byte private range",
            "mid-range-asn",
            Some(4_250_000_000),
            &[],
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, name, amazon_side_asn, tags) in rows {
        let (status, response) = dx
            .call(
                "CreateDirectConnectGateway",
                create_gateway_body(name, *amazon_side_asn, tags),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        let created = response["directConnectGateway"].clone();
        assert!(
            is_minted_gateway_id(&created["directConnectGatewayId"]),
            "case {case:?}: expected a server-minted UUID id, got {:?}",
            created["directConnectGatewayId"]
        );
        assert_eq!(
            created["directConnectGatewayName"],
            json!(name),
            "case {case:?}: expected the requested name, got {:?}",
            created["directConnectGatewayName"]
        );
        assert_eq!(
            created["directConnectGatewayState"],
            json!("pending"),
            "case {case:?}: pending is the initial state after the create, got {:?}",
            created["directConnectGatewayState"]
        );
        assert_eq!(
            created["amazonSideAsn"],
            json!(amazon_side_asn.unwrap_or(DEFAULT_AMAZON_SIDE_ASN)),
            "case {case:?}: expected the requested ASN or the documented default, got {:?}",
            created["amazonSideAsn"]
        );
        assert_eq!(
            created["ownerAccount"],
            json!(engine::EMULATOR_ACCOUNT_ID),
            "case {case:?}: expected the emulator account, got {:?}",
            created["ownerAccount"]
        );
        // A brand new gateway has no attachments, so no route prefixes are
        // allocated to it.
        assert_eq!(
            created["totalPrefixPoolAllocations"],
            json!(0),
            "case {case:?}: a gateway with no attachments allocates no prefixes, got {:?}",
            created["totalPrefixPoolAllocations"]
        );
        assert!(
            created["stateChangeError"].is_null(),
            "case {case:?}: no state advance has failed, got {:?}",
            created["stateChangeError"]
        );
        assert_eq!(
            created["tags"].as_array().map_or(0, Vec::len),
            tags.len(),
            "case {case:?}: expected {} tags, got {:?}",
            tags.len(),
            created["tags"]
        );

        // The stored record, read back: field for field what the create returned.
        let id = created["directConnectGatewayId"].as_str().unwrap_or_default();
        let (described, next_token) = describe(&dx, json!({"directConnectGatewayId": id})).await;
        assert_eq!(
            described.len(),
            1,
            "case {case:?}: expected exactly the named gateway, got {described:?}"
        );
        assert!(
            next_token.is_null(),
            "case {case:?}: a single-item page is the last one, got {next_token:?}"
        );
        assert_eq!(
            without_state(&described[0]),
            without_state(&created),
            "case {case:?}: the describe must render the stored record, got {:?}",
            described[0]
        );

        minted.push(id.to_string());
    }

    // Ids are unique, and a describe with no filter answers every stored one in
    // creation order.
    let (all, _) = describe(&dx, json!({})).await;
    let listed: Vec<String> = all
        .iter()
        .map(|gateway| {
            gateway["directConnectGatewayId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, minted,
        "expected every stored gateway in creation order, got {listed:?}"
    );
}

#[tokio::test]
async fn a_pending_gateway_becomes_available_when_the_clock_makes_it_due() {
    let dx = Emulator::new();
    let id = gateway(&dx, "clock-owed").await;

    // The span is never zero, so a caller reading straight back still observes
    // the transient state the model documents.
    let (described, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
    assert_eq!(
        described[0]["directConnectGatewayState"],
        json!("pending"),
        "case {id:?}: a gateway read straight back is still pending, got {:?}",
        described[0]["directConnectGatewayState"]
    );

    std::thread::sleep(SETTLE);

    let (described, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
    assert_eq!(
        described[0]["directConnectGatewayState"],
        json!("available"),
        "case {id:?}: the pending span has elapsed, so the gateway is available, got {:?}",
        described[0]["directConnectGatewayState"]
    );
    assert_eq!(
        described[0]["directConnectGatewayId"],
        json!(&id),
        "case {id:?}: the id must not change when the state advances, got {:?}",
        described[0]["directConnectGatewayId"]
    );
    // Reading it again answers the same state: the read materialised only what
    // the clock already owed.
    let (again, _) = describe(&dx, json!({"directConnectGatewayId": &id})).await;
    assert_eq!(
        again[0], described[0],
        "case {id:?}: a second read must answer the same record, got {:?}",
        again[0]
    );
}

#[tokio::test]
async fn an_amazon_side_asn_outside_the_documented_private_ranges_is_refused() {
    let dx = Emulator::new();

    // One value past each published bound of each documented private range, plus
    // values far outside them.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for (low, high) in PRIVATE_AMAZON_SIDE_ASN_RANGES {
        rows.push((format!("one below the low bound {low}"), json!(low - 1)));
        rows.push((format!("one above the high bound {high}"), json!(high + 1)));
    }
    rows.push(("a public two-byte ASN".to_string(), json!(7_224)));
    rows.push(("zero".to_string(), json!(0)));
    rows.push(("negative".to_string(), json!(-1)));

    for (case, asn) in &rows {
        let mut body = Map::new();
        body.insert("directConnectGatewayName".to_string(), json!("asn-check"));
        body.insert("amazonSideAsn".to_string(), asn.clone());
        let (status, response) = dx
            .call("CreateDirectConnectGateway", Value::Object(body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
    }

    // A refused create stores nothing.
    let (stored, _) = describe(&dx, json!({})).await;
    assert!(
        stored.is_empty(),
        "a refused create must store no gateway, got {stored:?}"
    );
}

#[tokio::test]
async fn a_request_that_names_nothing_is_refused() {
    let dx = Emulator::new();

    // (case, body, the error code expected)
    let rows: &[(&str, Value, &str)] = &[
        (
            "an empty gateway name",
            json!({"directConnectGatewayName": ""}),
            "DirectConnectClientException",
        ),
        (
            "a gateway name of only whitespace",
            json!({"directConnectGatewayName": "   "}),
            "DirectConnectClientException",
        ),
        (
            "a gateway id naming nothing",
            json!({"directConnectGatewayId": "00000000-0000-4000-8000-000000000000"}),
            "DirectConnectClientException",
        ),
        (
            "a pagination token naming nothing",
            json!({"nextToken": "00000000-0000-4000-8000-000000000000"}),
            "DirectConnectClientException",
        ),
    ];

    for (case, body, expected) in rows {
        let target = if body.get("directConnectGatewayName").is_some() {
            "CreateDirectConnectGateway"
        } else {
            "DescribeDirectConnectGateways"
        };
        let (status, response) = dx.call(target, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains(expected),
            "case {case:?}: expected {expected}, got {:?}",
            error_code(&response)
        );
    }
}

#[tokio::test]
async fn gateways_page_with_max_results_and_next_token() {
    let dx = Emulator::new();
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(gateway(&dx, &format!("page-{n}")).await);
    }

    // (case, page size, expected page lengths)
    let rows: &[(&str, i64, &[usize])] = &[
        ("one at a time", 1, &[1, 1, 1, 1, 1]),
        ("two at a time", 2, &[2, 2, 1]),
        ("exactly the whole set", 5, &[5]),
        ("more than the whole set", 50, &[5]),
    ];

    for (case, page_size, expected_lengths) in rows {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        for (index, expected_length) in expected_lengths.iter().enumerate() {
            let mut body = Map::new();
            body.insert("maxResults".to_string(), json!(page_size));
            if let Some(token) = &token {
                body.insert("nextToken".to_string(), json!(token));
            }
            let (page, next_token) = describe(&dx, Value::Object(body)).await;
            assert_eq!(
                page.len(),
                *expected_length,
                "case {case:?} page {index}: expected {expected_length} items, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|gateway| {
                gateway["directConnectGatewayId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }));
            let is_last = index + 1 == expected_lengths.len();
            assert_eq!(
                next_token.is_null(),
                is_last,
                "case {case:?} page {index}: nextToken must be null only on the last page, got {next_token:?}"
            );
            token = next_token.as_str().map(ToString::to_string);
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must visit every stored gateway exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_max_results_above_the_documented_ceiling_is_capped() {
    let dx = Emulator::new();
    // One more than the documented ceiling, so a capped page is observable.
    let total = usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive") + 1;
    for n in 0..total {
        gateway(&dx, &format!("capped-{n}")).await;
    }

    // "If MaxResults is given a value larger than 100, only 100 results are
    // returned."
    for asked in [MAX_RESULT_SET_SIZE + 1, 250, 100_000] {
        let (page, next_token) = describe(&dx, json!({"maxResults": asked})).await;
        assert_eq!(
            page.len(),
            usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive"),
            "case {asked:?}: a page is capped at {MAX_RESULT_SET_SIZE}, got {}",
            page.len()
        );
        assert!(
            next_token.is_string(),
            "case {asked:?}: a capped page is not the last one, got {next_token:?}"
        );

        let (rest, rest_token) =
            describe(&dx, json!({"maxResults": asked, "nextToken": next_token})).await;
        assert_eq!(
            rest.len(),
            total - usize::try_from(MAX_RESULT_SET_SIZE).expect("the ceiling is positive"),
            "case {asked:?}: the next page holds what the cap left, got {}",
            rest.len()
        );
        assert!(
            rest_token.is_null(),
            "case {asked:?}: the last page carries no token, got {rest_token:?}"
        );
    }
}

#[tokio::test]
async fn a_private_virtual_interface_terminates_on_a_stored_gateway() {
    let dx = Emulator::new();
    let (status, connection) = dx
        .call(
            "CreateConnection",
            json!({"connectionName": "gw-host", "location": "EqDC2", "bandwidth": "1Gbps"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConnection to succeed, got {status} with {connection}"
    );
    let connection_id = connection["connectionId"].as_str().unwrap_or_default();

    let asn_rows = [
        PRIVATE_AMAZON_SIDE_ASN_RANGES[0].0,
        PRIVATE_AMAZON_SIDE_ASN_RANGES[1].1,
    ];
    for (index, amazon_side_asn) in asn_rows.into_iter().enumerate() {
        let (status, created) = dx
            .call(
                "CreateDirectConnectGateway",
                create_gateway_body(&format!("vif-gateway-{index}"), Some(amazon_side_asn), &[]),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {amazon_side_asn:?}: expected the gateway create to succeed, got {status} with {created}"
        );
        let gateway_id = created["directConnectGateway"]["directConnectGatewayId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, vif) = dx
            .call(
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": connection_id,
                    "newPrivateVirtualInterface": {
                        "virtualInterfaceName": format!("vif-{index}"),
                        "vlan": 200 + i64::try_from(index).unwrap_or(0),
                        "asn": 65_000,
                        "directConnectGatewayId": &gateway_id,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {amazon_side_asn:?}: expected the virtual interface create to succeed, got {status} with {vif}"
        );
        assert_eq!(
            vif["directConnectGatewayId"],
            json!(&gateway_id),
            "case {amazon_side_asn:?}: expected the gateway the caller named, got {:?}",
            vif["directConnectGatewayId"]
        );
        // The Amazon side of a peering that terminates on a Direct Connect
        // gateway is that gateway's ASN.
        assert_eq!(
            vif["amazonSideAsn"],
            json!(amazon_side_asn),
            "case {amazon_side_asn:?}: expected the gateway's ASN, got {:?}",
            vif["amazonSideAsn"]
        );
        assert!(
            vif["virtualGatewayId"].is_null(),
            "case {amazon_side_asn:?}: a gateway-terminated interface names no virtual private gateway, got {:?}",
            vif["virtualGatewayId"]
        );
    }

    // (case, the newPrivateVirtualInterface members under test)
    let refusals: &[(&str, Value)] = &[
        (
            "a Direct Connect gateway id naming nothing",
            json!({
                "virtualInterfaceName": "unknown-gw",
                "vlan": 301,
                "asn": 65_000,
                "directConnectGatewayId": "00000000-0000-4000-8000-000000000000",
            }),
        ),
        (
            "both gateway kinds at once",
            json!({
                "virtualInterfaceName": "both-gw",
                "vlan": 302,
                "asn": 65_000,
                "directConnectGatewayId": "00000000-0000-4000-8000-000000000000",
                "virtualGatewayId": "vgw-0123456789abcdef0",
            }),
        ),
        (
            "neither gateway kind",
            json!({
                "virtualInterfaceName": "no-gw",
                "vlan": 303,
                "asn": 65_000,
            }),
        ),
    ];

    for (case, requested) in refusals {
        let (status, response) = dx
            .call(
                "CreatePrivateVirtualInterface",
                json!({
                    "connectionId": connection_id,
                    "newPrivateVirtualInterface": requested.clone(),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {response}"
        );
        assert!(
            error_code(&response).contains("DirectConnectClientException"),
            "case {case:?}: expected DirectConnectClientException, got {:?}",
            error_code(&response)
        );
    }
}
