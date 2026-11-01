// SPDX-License-Identifier: CC0-1.0

//! The media type this service announces on its responses.
//!
//! Every reply of the Lex Models V2 model-building front end — whatever the status, and
//! whichever layer of the request walk produced it — is labelled with the awsJson `1.1`
//! media type, not the `application/json` the `restJson1` protocol trait its model
//! declares implies. A client that keys its parsing or its content negotiation off the
//! media type reads that label, so it is part of the wire contract.
//!
//! The rule is asserted over a TABLE spanning two dimensions, because the media type is
//! the SERVICE's and so belongs to neither one layer nor one operation:
//!
//! * the layers that can answer a request — a handler's success, a handler's modelled
//!   error, the boundary refusal the framework renders inside the walk, an operation
//!   nobody has written yet, and a request that names no modelled operation at all;
//! * the operations that answer them, across the bot, bot locale and slot type resources
//!   of this front end, on each status those operations reach here (200, 202, 400, 404,
//!   409, 412, 501).
//!
//! The pair itself is named once in `src/service.rs` (`RESPONSE_MEDIA_TYPE`) and read from
//! there, so this suite cannot drift from the service.

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::service::RESPONSE_MEDIA_TYPE;

use crate::bot_lifecycle::{call_announcing, create_body, emulator};
use crate::bot_locale::{locale_body, locale_uri, locales_uri};
use crate::slot_type::{create_slot_type, host_locale, slot_type_body, slot_types_uri};

/// One row of the table: how the request is made, and the status the layer under test
/// answers with.
struct Answered {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
}

#[tokio::test]
async fn every_response_announces_the_media_type_this_service_answers_under() {
    let router = emulator();

    // A bot and one of its locales, so the rows below can hit a handler that succeeds
    // and one that refuses from stored state.
    let (status, _, created) = call_announcing(
        &router,
        "PUT",
        "/bots",
        Some(create_body("MediaTypeBot", 300, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the host bot must be created, got {created:?}");
    let bot_id = created
        .get("botId")
        .and_then(Value::as_str)
        .expect("CreateBot reports the id it minted")
        .to_string();

    // A second bot with a locale of its own, carrying one slot type already, so the
    // slot-type rows below can read a stored record back, collide with a name the locale
    // holds, and ask for one no record carries.
    let slot_bot = host_locale(&router, "MediaTypeSlotTypeBot", "en_US").await;
    let stored_slot_type =
        create_slot_type(&router, &slot_bot, "en_US", &slot_type_body("City", json!({}))).await;
    let slot_types = slot_types_uri(&slot_bot, "DRAFT", "en_US");

    let rows = vec![
        Answered {
            case: "a create a handler served",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 0.4, json!({}))),
            status: 202,
        },
        Answered {
            case: "a read a handler served",
            method: "GET",
            uri: locale_uri(&bot_id, "DRAFT", "en_US"),
            body: None,
            status: 200,
        },
        Answered {
            case: "a modelled error a handler raised",
            method: "GET",
            uri: "/bots/ABCDEFGHIJ".to_string(),
            body: None,
            status: 404,
        },
        Answered {
            case: "a locale a handler refused as a conflict",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 0.4, json!({}))),
            status: 409,
        },
        Answered {
            case: "a precondition a handler refused",
            method: "POST",
            uri: locale_uri(&bot_id, "DRAFT", "nl_NL"),
            body: Some(json!({})),
            status: 412,
        },
        Answered {
            case: "a constraint the framework refused inside the walk",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(locale_body("en_US", 2.5, json!({}))),
            status: 400,
        },
        Answered {
            case: "an absent required member the framework refused",
            method: "PUT",
            uri: locales_uri(&bot_id, "DRAFT"),
            body: Some(json!({ "localeId": "de_DE" })),
            status: 400,
        },
        Answered {
            case: "an operation nobody has written yet",
            method: "POST",
            uri: format!("{}/intents", locale_uri(&bot_id, "DRAFT", "en_US")),
            body: Some(json!({})),
            status: 501,
        },
        Answered {
            case: "a request that names no modelled operation",
            method: "GET",
            uri: "/policy".to_string(),
            body: None,
            status: 501,
        },
        Answered {
            case: "a slot type a handler created",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("Size", json!({ "description": "a size" }))),
            status: 200,
        },
        Answered {
            case: "a slot type a handler read back",
            method: "GET",
            uri: format!("{slot_types}/{stored_slot_type}"),
            body: None,
            status: 200,
        },
        Answered {
            case: "a slot type listing a handler served",
            method: "POST",
            uri: slot_types.clone(),
            body: Some(json!({})),
            status: 200,
        },
        Answered {
            case: "a slot type name a handler refused as already taken",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("City", json!({}))),
            status: 400,
        },
        Answered {
            case: "a slot type for a locale a handler refused",
            method: "PUT",
            uri: slot_types_uri(&slot_bot, "DRAFT", "nl_NL"),
            body: Some(slot_type_body("Orphan", json!({}))),
            status: 412,
        },
        Answered {
            case: "a slot type id no record carries",
            method: "GET",
            uri: format!("{slot_types}/ZZZZZZZZZZ"),
            body: None,
            status: 404,
        },
        Answered {
            case: "a slot type name outside its modelled pattern, refused inside the walk",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body("not a slot type name!", json!({}))),
            status: 400,
        },
        Answered {
            case: "a slot type whose parent signature the model does not support",
            method: "PUT",
            uri: slot_types.clone(),
            body: Some(slot_type_body(
                "Parented",
                json!({ "parentSlotTypeSignature": "AMAZON.Number" }),
            )),
            status: 400,
        },
        Answered {
            case: "a slot type update nobody has written yet",
            method: "PUT",
            uri: format!("{slot_types}/{stored_slot_type}"),
            body: Some(slot_type_body("City", json!({}))),
            status: 501,
        },
    ];

    for row in rows {
        let (status, announced, body) =
            call_announcing(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} {body:?}",
            row.case, row.status
        );
        assert_eq!(
            announced.as_deref(),
            Some(RESPONSE_MEDIA_TYPE.announced()),
            "case {:?} (HTTP {status}): expected content-type {}, got {announced:?}",
            row.case,
            RESPONSE_MEDIA_TYPE.announced()
        );
        assert_ne!(
            announced.as_deref(),
            Some(RESPONSE_MEDIA_TYPE.implied()),
            "case {:?}: the protocol-implied media type must not reach a client",
            row.case
        );
    }
}
