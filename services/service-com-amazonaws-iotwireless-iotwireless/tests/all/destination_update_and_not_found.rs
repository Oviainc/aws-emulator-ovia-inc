// SPDX-License-Identifier: CC0-1.0

//! `UpdateDestination` and the destination not-found refusal, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * an update REPLACES every member the request carries on the stored destination and
//!   leaves every member it omits alone, so a later read reports the destination's
//!   CURRENT routing target, description and role rather than its create-time one;
//! * an `ExpressionType` outside the modelled enum is refused and changes nothing;
//! * every operation that addresses a destination by name refuses a name the account
//!   does not hold with 404 `ResourceNotFoundException`, whose `Message` is this
//!   service's fixed sentence — no part of it taken from the request — and which carries
//!   neither of the two optional members the model permits.
//!
//! `tests/all/*.rs` are modules of ONE test target, so the wire harness (`router`,
//! `call`, `error_code`) is shared from the destination/device suite rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// `ExpressionType`'s modelled values — the model-generated constant the handlers
/// validate against, so the suite and the service cannot drift apart.
const EXPRESSION_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::EXPRESSION_TYPE_VALUES;

/// `Expression`'s published length ceiling.
/// Source: `smithy_query(query="operation", operation="UpdateDestination")` — `Expression`
/// carries length 0..2048.
const EXPRESSION_MAX_LEN: usize = 2048;

/// The sentence this service's 404 carries for a destination the account does not hold.
/// Fixed text: no part of it comes from the request or from the store.
const NOT_FOUND_MESSAGE: &str = "Destination doesn't exist";

/// What every case's destination is created with, so an assertion about an omitted
/// member is an assertion that the stored value survived the update.
const SEED_EXPRESSION: &str = "seed-rule";
const SEED_EXPRESSION_TYPE: &str = "RuleName";
const SEED_DESCRIPTION: &str = "seed description";
const SEED_ROLE: &str = "arn:aws:iam::000000000000:role/seed-destination-role";

/// Create the case's destination, panicking on anything but 201.
async fn seed(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": SEED_EXPRESSION_TYPE,
            "Expression": SEED_EXPRESSION,
            "Description": SEED_DESCRIPTION,
            "RoleArn": SEED_ROLE,
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seed {name}: expected 201 from CreateDestination, got {status} {body}"
    );
}

/// Read one destination back, returning its status and body.
async fn get(router: &MultiServiceRouter, name: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/destinations/{name}"), None).await
}

/// An update's four updatable members, as `GetDestination` reports them.
const UPDATABLE_MEMBERS: [&str; 4] = ["Expression", "ExpressionType", "Description", "RoleArn"];

/// An update replaces what it carries and leaves what it omits: asserted over the member
/// subsets a caller can send (each member alone, every member at once, none at all), both
/// `ExpressionType` values, and an `Expression` at its published length ceiling.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_the_rest() {
    let router = router();
    let long_expression = "a".repeat(EXPRESSION_MAX_LEN);
    let updated_role = "arn:aws:iam::000000000000:role/updated-destination-role";

    // (case, the PATCH body, what every later read must report for the four members)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a rule target re-pointed to an MQTT topic",
            json!({ "Expression": "sim/uplink/topic", "ExpressionType": "MqttTopic" }),
            json!({
                "Expression": "sim/uplink/topic",
                "ExpressionType": "MqttTopic",
                "Description": SEED_DESCRIPTION,
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "the expression alone, leaving the type",
            json!({ "Expression": "another-rule" }),
            json!({
                "Expression": "another-rule",
                "ExpressionType": SEED_EXPRESSION_TYPE,
                "Description": SEED_DESCRIPTION,
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "the expression type alone, leaving the expression",
            json!({ "ExpressionType": "MqttTopic" }),
            json!({
                "Expression": SEED_EXPRESSION,
                "ExpressionType": "MqttTopic",
                "Description": SEED_DESCRIPTION,
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "the description alone",
            json!({ "Description": "a new description" }),
            json!({
                "Expression": SEED_EXPRESSION,
                "ExpressionType": SEED_EXPRESSION_TYPE,
                "Description": "a new description",
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "the description replaced with empty text",
            json!({ "Description": "" }),
            json!({
                "Expression": SEED_EXPRESSION,
                "ExpressionType": SEED_EXPRESSION_TYPE,
                "Description": "",
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "the role alone",
            json!({ "RoleArn": updated_role }),
            json!({
                "Expression": SEED_EXPRESSION,
                "ExpressionType": SEED_EXPRESSION_TYPE,
                "Description": SEED_DESCRIPTION,
                "RoleArn": updated_role,
            }),
        ),
        (
            "every updatable member at once",
            json!({
                "Expression": "sim/all/members",
                "ExpressionType": "MqttTopic",
                "Description": "all four replaced",
                "RoleArn": updated_role,
            }),
            json!({
                "Expression": "sim/all/members",
                "ExpressionType": "MqttTopic",
                "Description": "all four replaced",
                "RoleArn": updated_role,
            }),
        ),
        (
            "no updatable member at all",
            json!({}),
            json!({
                "Expression": SEED_EXPRESSION,
                "ExpressionType": SEED_EXPRESSION_TYPE,
                "Description": SEED_DESCRIPTION,
                "RoleArn": SEED_ROLE,
            }),
        ),
        (
            "an expression at its published length ceiling",
            json!({ "Expression": long_expression, "ExpressionType": "MqttTopic" }),
            json!({
                "Expression": long_expression,
                "ExpressionType": "MqttTopic",
                "Description": SEED_DESCRIPTION,
                "RoleArn": SEED_ROLE,
            }),
        ),
    ];

    for (index, (case, patch, expected)) in cases.iter().enumerate() {
        let name = format!("UpdateCase{index}");
        seed(&router, &name).await;

        let (status, body) = call(
            &router,
            Method::PATCH,
            &format!("/destinations/{name}"),
            Some(patch.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from UpdateDestination, got {status} {body}"
        );

        let (status, fetched) = get(&router, &name).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from GetDestination, got {status} {fetched}"
        );
        for member in UPDATABLE_MEMBERS {
            assert_eq!(
                fetched[member], expected[member],
                "case {case:?}: expected {member} {}, got {}",
                expected[member], fetched[member]
            );
        }

        // An update re-points the destination; it does not re-identify it.
        assert_eq!(
            fetched["Name"],
            json!(name),
            "case {case:?}: expected Name {name:?}, got {fetched}"
        );
        let arn = fetched["Arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.ends_with(&format!("Destination/{name}")),
            "case {case:?}: expected an ARN naming the destination, got {arn:?}"
        );

        // Reading does not change what the next caller sees, and the update is applied
        // once rather than again per read.
        let (_, again) = get(&router, &name).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: a second read must report the same record, got {again}"
        );
    }
}

/// The whole `ExpressionType` catalog is updatable, in both directions: a destination
/// created with one value and updated to any other reports the updated pair.
#[tokio::test]
async fn every_modelled_expression_type_can_be_updated_to_and_from() {
    let router = router();

    for from in EXPRESSION_TYPES {
        for to in EXPRESSION_TYPES {
            let case = format!("{from} -> {to}");
            let name = format!("Catalog{from}To{to}");
            let (status, created) = call(
                &router,
                Method::POST,
                "/destinations",
                Some(json!({
                    "Name": name,
                    "ExpressionType": from,
                    "Expression": format!("created-for-{from}"),
                    "RoleArn": SEED_ROLE,
                })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201 from CreateDestination, got {status} {created}"
            );

            let target = format!("updated-for-{to}");
            let (status, body) = call(
                &router,
                Method::PATCH,
                &format!("/destinations/{name}"),
                Some(json!({ "ExpressionType": to, "Expression": target })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?}: expected 204 from UpdateDestination, got {status} {body}"
            );

            let (status, fetched) = get(&router, &name).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: expected 200 from GetDestination, got {status} {fetched}"
            );
            assert_eq!(
                fetched["ExpressionType"],
                json!(to),
                "case {case:?}: expected ExpressionType {to:?}, got {fetched}"
            );
            assert_eq!(
                fetched["Expression"],
                json!(target),
                "case {case:?}: expected Expression {target:?}, got {fetched}"
            );
        }
    }
}

/// An `ExpressionType` outside the modelled enum is refused by the update, and the
/// destination keeps every value it held.
#[tokio::test]
async fn an_unmodelled_expression_type_is_refused_by_the_update_and_changes_nothing() {
    let router = router();

    for (index, bad) in ["mqtttopic", "Mqtt Topic", "SqsQueue", ""].iter().enumerate() {
        let name = format!("BadUpdate{index}");
        seed(&router, &name).await;

        let (status, body) = call(
            &router,
            Method::PATCH,
            &format!("/destinations/{name}"),
            Some(json!({ "ExpressionType": bad, "Expression": "must-not-be-stored" })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {bad:?}: expected 400 from UpdateDestination, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {bad:?}: expected ValidationException, got {body}"
        );

        let (_, fetched) = get(&router, &name).await;
        assert_eq!(
            fetched["Expression"],
            json!(SEED_EXPRESSION),
            "case {bad:?}: a refused update must change nothing, got {fetched}"
        );
        assert_eq!(
            fetched["ExpressionType"],
            json!(SEED_EXPRESSION_TYPE),
            "case {bad:?}: a refused update must change nothing, got {fetched}"
        );
    }
}

/// Every call that addresses a destination by name refuses a name the account does not
/// hold the same way: 404 `ResourceNotFoundException`, the service's fixed sentence, and
/// neither optional member of the error shape set.
#[tokio::test]
async fn a_destination_the_account_does_not_hold_is_refused_with_one_fixed_sentence() {
    let router = router();
    seed(&router, "HeldDestination").await;

    for absent in ["Absent", "helddestination", "nope", "a-very_long-9999-name"] {
        // (the operation, its method, its URI, its body) — every call whose subject is a
        // destination named by the request.
        let calls: Vec<(&str, Method, String, Option<Value>)> = vec![
            (
                "GetDestination",
                Method::GET,
                format!("/destinations/{absent}"),
                None,
            ),
            (
                "UpdateDestination",
                Method::PATCH,
                format!("/destinations/{absent}"),
                Some(json!({ "Expression": "any-rule" })),
            ),
            (
                "DeleteDestination",
                Method::DELETE,
                format!("/destinations/{absent}"),
                None,
            ),
            (
                "CreateWirelessDevice",
                Method::POST,
                "/wireless-devices".to_string(),
                Some(json!({ "Type": "LoRaWAN", "DestinationName": absent })),
            ),
        ];

        for (operation, method, uri, body) in calls {
            let case = format!("{operation} {absent}");
            let (status, response) = call(&router, method, &uri, body).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "case {case:?}: expected 404, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                response["Message"],
                json!(NOT_FOUND_MESSAGE),
                "case {case:?}: expected Message {NOT_FOUND_MESSAGE:?}, got {response}"
            );
            let message = response["Message"].as_str().unwrap_or_default().to_string();
            assert!(
                !message.contains(absent),
                "case {case:?}: the refusal must take nothing from the request, got {message:?}"
            );
            assert!(
                response.get("ResourceId").is_none(),
                "case {case:?}: expected no ResourceId member, got {response}"
            );
            assert!(
                response.get("ResourceType").is_none(),
                "case {case:?}: expected no ResourceType member, got {response}"
            );
        }
    }

    // None of the refusals touched the destination the account does hold.
    let (status, fetched) = get(&router, "HeldDestination").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the held destination must still be readable, got {status} {fetched}"
    );
    assert_eq!(
        fetched["Expression"],
        json!(SEED_EXPRESSION),
        "the held destination must be unchanged, got {fetched}"
    );
}
