// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateGateway` (`CreateGateway` -> `UpdateGateway` ->
//! `DescribeGateway` / `ListGateways`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the update REPLACES the addressed gateway's name with the one the request
//!   carries and persists it, so every later reader answers with the new name and
//!   nothing else about the gateway (its platform, version, capabilities,
//!   `creationDate`) moves, while `lastUpdateDate` does;
//! * a gateway name is taken by at most one gateway, so a rename asking for a name
//!   another gateway holds is the modelled 409 naming that gateway, a rename to the
//!   gateway's own current name is accepted, and a name a rename frees is available
//!   again;
//! * an id that resolves to no stored gateway is a modelled 404, and the refusal
//!   invents no gateway;
//! * a name outside one of the member's modelled constraints is refused as client
//!   input and leaves the stored name exactly as it was.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) and the
//! gateway create helpers are the ones the asset, gateway and capability suites
//! already have; this suite reuses them rather than growing a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router};
use crate::gateway_capability_configuration::create_gateway;
use crate::gateway_lifecycle::{greengrass, greengrass_v2, siemens_ie, GATEWAYS};

/// `PUT /20200301/gateways/{gatewayId}` — the route `UpdateGateway` is bound to
/// (smithy_query query="operation" operation="UpdateGateway").
fn update_uri(gateway_id: &str) -> String {
    format!("{GATEWAYS}/{gateway_id}")
}

/// Ask for `name` on the gateway `gateway_id` and return `(status, body)`.
async fn rename(
    router: &MultiServiceRouter,
    gateway_id: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &update_uri(gateway_id),
        Some(json!({ "gatewayName": name })),
    )
    .await
}

/// What `DescribeGateway` reports for `gateway_id`.
async fn describe(router: &MultiServiceRouter, gateway_id: &str) -> Value {
    let (status, described) = call(router, "GET", &update_uri(gateway_id), None).await;
    assert_eq!(
        status, 200,
        "DescribeGateway {gateway_id:?}: expected 200, got {status} {described}"
    );
    described
}

/// The name `ListGateways` reports for `gateway_id`, or `None` when it lists no
/// such gateway.
async fn listed_name(router: &MultiServiceRouter, gateway_id: &str) -> Option<String> {
    let (status, body) = call(router, "GET", &format!("{GATEWAYS}?maxResults=250"), None).await;
    assert_eq!(status, 200, "ListGateways expected 200, got {status} {body}");
    body["gatewaySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("gatewaySummaries must be a list, got {body}"))
        .iter()
        .find(|summary| summary["gatewayId"].as_str() == Some(gateway_id))
        .and_then(|summary| summary["gatewayName"].as_str().map(str::to_string))
}

/// A `gatewayName` of exactly `length` characters that the member's pattern
/// `^[^\u0000-\u001F\u007F]+$` admits (smithy_query query="operation"
/// operation="UpdateGateway": `gatewayName` targets `GatewayName`, len 1..256).
fn name_of_length(length: usize) -> String {
    "g".repeat(length)
}

#[tokio::test]
async fn a_rename_persists_and_every_reader_answers_with_the_new_name() {
    let router = router();
    let gateway = create_gateway(&router, "renamed").await;
    let before = describe(&router, &gateway).await;
    let creation = before["creationDate"].as_f64().unwrap_or(0.0);
    let mut previous_update = before["lastUpdateDate"].as_f64().unwrap_or(0.0);
    assert!(
        creation > 0.0 && creation == previous_update,
        "an untouched gateway's lastUpdateDate is its creationDate, got {before}"
    );

    // Rows: the name forms the member's constraints admit, differing in length
    // (its minimum of 1, an ordinary name, its maximum of 256), in script
    // (non-ASCII), in the characters outside `[A-Za-z0-9-]` the pattern still
    // admits (spaces, punctuation, digits) — and, last, the name the gateway
    // already carries, which asks for the state it is already in.
    let cases: Vec<(&str, String)> = vec![
        ("an ordinary name", "windfarm-north".to_string()),
        ("a name at the bottom of its length", name_of_length(1)),
        ("a name at the top of its length", name_of_length(256)),
        ("a name with spaces and punctuation", "Boiler House #2 (east)".to_string()),
        ("a name of digits only", "20200301".to_string()),
        ("a non-ASCII name", "passerelle-été-日本".to_string()),
        ("a name with an inner space", "pump station".to_string()),
        ("the name the gateway already carries", "pump station".to_string()),
    ];

    for (case, name) in cases {
        // The clock must have moved between the writes for the timestamp
        // comparison below to say anything.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (status, answer) = rename(&router, &gateway, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rename expected 200, got {status} {answer}"
        );

        let described = describe(&router, &gateway).await;
        assert_eq!(
            described["gatewayName"].as_str().unwrap_or(""),
            name,
            "case {case:?}: DescribeGateway must report the new name, got {described}"
        );
        assert_eq!(
            listed_name(&router, &gateway).await.as_deref(),
            Some(name.as_str()),
            "case {case:?}: ListGateways must report the new name too"
        );

        // Only the name moved: the gateway's identity, platform, version and
        // capabilities are not in this request, and its creationDate stays where
        // the create put it.
        for member in [
            "gatewayId",
            "gatewayArn",
            "gatewayPlatform",
            "gatewayVersion",
            "gatewayCapabilitySummaries",
            "creationDate",
        ] {
            assert_eq!(
                described[member], before[member],
                "case {case:?}: the rename must leave {member} alone, got {described}"
            );
        }
        let last_update = described["lastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            last_update > previous_update,
            "case {case:?}: the rename must move lastUpdateDate past {previous_update}, \
             got {described}"
        );
        previous_update = last_update;

        // A READ does not move it: only the write does.
        let again = describe(&router, &gateway).await;
        assert_eq!(
            again, described,
            "case {case:?}: DescribeGateway answered differently on the second read: {again}"
        );
    }
}

#[tokio::test]
async fn a_rename_cannot_take_a_name_another_gateway_holds() {
    let router = router();

    // Rows: the gateway whose name is asked for, each on a platform of its own —
    // the conflict is the NAME, whichever platform either gateway runs on
    // (`GatewayPlatform`: greengrass, greengrassV2, siemensIE).
    let holders: Vec<(&str, &str, Value)> = vec![
        ("greengrassV2 holder", "held-by-greengrass-v2", greengrass_v2("conflict-core-device", None)),
        ("siemensIE holder", "held-by-siemens", siemens_ie("conflict-siemens-thing")),
        ("greengrass holder", "held-by-greengrass", greengrass("conflict-group")),
    ];

    let renamed = create_gateway(&router, "the-renamed-gateway").await;
    let mut current = "the-renamed-gateway".to_string();

    for (case, held_name, platform) in holders {
        let (status, created) = call(
            &router,
            "POST",
            GATEWAYS,
            Some(json!({ "gatewayName": held_name, "gatewayPlatform": platform })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the holder expected 201, got {status} {created}"
        );
        let holder_id = created["gatewayId"].as_str().unwrap_or("").to_string();
        let holder_arn = created["gatewayArn"].as_str().unwrap_or("").to_string();

        let (status, conflict) = rename(&router, &renamed, held_name).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a name another gateway holds expected 409, got {status} {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictingOperationException",
            "case {case:?}: unexpected error code in {conflict}"
        );
        assert_eq!(
            conflict["resourceId"].as_str().unwrap_or(""),
            holder_id,
            "case {case:?}: the conflict must name the gateway holding the name, got {conflict}"
        );
        assert_eq!(
            conflict["resourceArn"].as_str().unwrap_or(""),
            holder_arn,
            "case {case:?}: the conflict must carry the holder's own ARN, got {conflict}"
        );

        // The refused rename changed nothing: both gateways keep their names.
        let described = describe(&router, &renamed).await;
        assert_eq!(
            described["gatewayName"].as_str().unwrap_or(""),
            current,
            "case {case:?}: a refused rename must leave the name alone, got {described}"
        );
        let holder = describe(&router, &holder_id).await;
        assert_eq!(
            holder["gatewayName"].as_str().unwrap_or(""),
            held_name,
            "case {case:?}: the holder must keep its name, got {holder}"
        );

        // Renaming the HOLDER away frees the name, which the other gateway may
        // then take: the rule is one name per gateway, not a name reserved forever.
        let freed = format!("{held_name}-moved-on");
        let (status, answer) = rename(&router, &holder_id, &freed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: renaming the holder expected 200, got {status} {answer}"
        );
        let (status, answer) = rename(&router, &renamed, held_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the freed name must be available, got {status} {answer}"
        );
        current = held_name.to_string();
        let described = describe(&router, &renamed).await;
        assert_eq!(
            described["gatewayName"].as_str().unwrap_or(""),
            current,
            "case {case:?}: the freed name must have been taken, got {described}"
        );
    }

    // A gateway asking for the name it already carries is accepted: it asks for
    // the state it is already in, so there is no other gateway to conflict with.
    let (status, answer) = rename(&router, &renamed, &current).await;
    assert_eq!(
        status, 200,
        "renaming a gateway to its own name expected 200, got {status} {answer}"
    );
    let described = describe(&router, &renamed).await;
    assert_eq!(
        described["gatewayName"].as_str().unwrap_or(""),
        current,
        "the gateway must keep the name it asked for, got {described}"
    );
}

#[tokio::test]
async fn renaming_a_gateway_that_was_never_created_is_refused() {
    let router = router();
    let gateway = create_gateway(&router, "the-only-gateway").await;

    // Well-formed ids no create ever minted: one all-f, and one differing from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = gateway.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    for case in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        let (status, body) = rename(&router, case, "a-name-for-nothing").await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        // The refusal names the id the request supplied, so a caller can see which
        // gateway was missing.
        assert!(
            body["message"].as_str().unwrap_or("").contains(case),
            "case {case:?}: the refusal must name the id the request supplied, got {body}"
        );
        // The refusal invented no gateway, and the name it asked for is still free.
        let (status, listed) = call(&router, "GET", &format!("{GATEWAYS}?maxResults=250"), None).await;
        assert_eq!(status, 200, "case {case:?}: ListGateways expected 200, got {listed}");
        let summaries = listed["gatewaySummaries"].as_array().map(Vec::len).unwrap_or(0);
        assert_eq!(
            summaries, 1,
            "case {case:?}: the refusal must invent no gateway, got {listed}"
        );
    }

    let (status, answer) = rename(&router, &gateway, "a-name-for-nothing").await;
    assert_eq!(
        status, 200,
        "the stored gateway must still be renameable, got {status} {answer}"
    );
}

#[tokio::test]
async fn a_name_outside_a_modelled_constraint_is_refused_and_persists_nothing() {
    let router = router();
    let gateway = create_gateway(&router, "bounds").await;
    let mut stored = "bounds".to_string();

    // (case, name, accepted). Each row walks one modelled bound of `gatewayName`
    // (smithy_query query="operation" operation="UpdateGateway": len 1..256,
    // pattern `^[^\u0000-\u001F\u007F]+$`): the value AT it, which must be
    // accepted, and the value just outside it, which must not.
    let cases: Vec<(&str, String, bool)> = vec![
        ("a name at the bottom of its length", name_of_length(1), true),
        ("a name below its minimum length", String::new(), false),
        ("a name at the top of its length", name_of_length(256), true),
        (
            "a name one character past its length",
            name_of_length(257),
            false,
        ),
        (
            "a name carrying a control character the pattern excludes",
            "edge\u{0001}gateway".to_string(),
            false,
        ),
        (
            "a name carrying the delete character the pattern excludes",
            "edge\u{007F}gateway".to_string(),
            false,
        ),
        (
            "a name carrying a newline the pattern excludes",
            "edge\ngateway".to_string(),
            false,
        ),
        ("a printable name the pattern admits", "edge~gateway".to_string(), true),
    ];

    for (case, name, accepted) in cases {
        let (status, answer) = rename(&router, &gateway, &name).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a value inside the bound is accepted, got {status} {answer}"
            );
            stored = name.clone();
        } else {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a value outside the bound must be refused, got {status} {answer}"
            );
        }
        let described = describe(&router, &gateway).await;
        assert_eq!(
            described["gatewayName"].as_str().unwrap_or(""),
            stored,
            "case {case:?}: expected the stored name {stored:?}, got {described}"
        );
    }
}
