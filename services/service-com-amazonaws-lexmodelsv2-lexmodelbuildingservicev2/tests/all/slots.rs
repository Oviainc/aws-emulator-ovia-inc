// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 slot slice: `CreateSlot`, `DescribeSlot`
//! and `ListSlots`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the slot type and intent helpers are the other suites', reused rather than
//! copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create resolves every resource it references against the store BEFORE it writes
//!   anything — the intent the slot hangs off and the slot type it elicits values for —
//!   and refuses a reference that resolves to nothing with the service's one
//!   precondition sentence, leaving the intent holding exactly what it held;
//! * a reference that does resolve — a custom slot type of that locale, or a built-in
//!   one Amazon Lex supplies — is accepted, and the slot is then read back through
//!   `DescribeSlot` and listed by `ListSlots`;
//! * a slot is reachable only under the intent of the locale of the bot version that
//!   holds it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, seconds, text};
use crate::intents_and_builds::{create_intent, intent_body};
use crate::slot_type::{create_slot_type, host_locale, slot_type_body};

/// `CreateSlot`'s URI:
/// `PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots`.
/// `ListSlots` is the same URI under `POST`.
pub(crate) fn slots_uri(
    bot_id: &str,
    bot_version: &str,
    locale_id: &str,
    intent_id: &str,
) -> String {
    format!(
        "/bots/{bot_id}/botversions/{bot_version}/botlocales/{locale_id}/intents/{intent_id}/slots"
    )
}

/// `DescribeSlot`'s URI, which adds the `slotId` to [`slots_uri`].
pub(crate) fn slot_uri(
    bot_id: &str,
    bot_version: &str,
    locale_id: &str,
    intent_id: &str,
    slot_id: &str,
) -> String {
    format!(
        "{}/{slot_id}",
        slots_uri(bot_id, bot_version, locale_id, intent_id)
    )
}

/// A `CreateSlot` request body carrying the two required members — the name and the
/// elicitation setting — plus `extra`.
pub(crate) fn slot_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "slotName": name,
        "valueElicitationSetting": {
            "slotConstraint": "Required",
            "promptSpecification": {
                "messageGroups": [{
                    "message": { "plainTextMessage": { "value": "What size?" } },
                }],
                "maxRetries": 2,
            },
        },
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// The `slotSummaries` `ListSlots` reports for one intent, by slot name.
pub(crate) async fn listed_slot_names(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    intent_id: &str,
) -> Vec<String> {
    let (status, listed) = call(
        router,
        "POST",
        &slots_uri(bot_id, "DRAFT", locale_id, intent_id),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListSlots to answer 200, got {status} {listed:?}"
    );
    member(&listed, "slotSummaries")
        .as_array()
        .map(|entries| entries.iter().map(|entry| text(entry, "slotName")).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_refuses_every_reference_the_locale_cannot_resolve() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotReferenceBot", "en_US").await;
    let other_bot_id = host_locale(&router, "OtherSlotReferenceBot", "en_US").await;
    // A second locale of the same bot, so a slot type defined there is a reference that
    // exists in the store but not where the slot is being created.
    let (status, created) = call(
        &router,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales"),
        Some(json!({ "localeId": "de_DE", "nluIntentConfidenceThreshold": 0.4 })),
    )
    .await;
    assert_eq!(status, 202, "the second locale must be created, got {created:?}");

    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body(
            "OrderPizza",
            json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
        ),
    )
    .await;
    let elsewhere = create_slot_type(
        &router,
        &bot_id,
        "de_DE",
        &slot_type_body("PizzaSize", json!({})),
    )
    .await;
    let other_bots = create_slot_type(
        &router,
        &other_bot_id,
        "en_US",
        &slot_type_body("PizzaSize", json!({})),
    )
    .await;

    // Each row is a reference that satisfies every modelled constraint and still names
    // nothing the locale can resolve.
    let rows: Vec<(&str, String)> = vec![
        ("an id no slot type was ever minted under", "ZZZZZZZZZZ".to_string()),
        (
            "a one-character id, the modelled minimum length",
            "Z".to_string(),
        ),
        (
            "the id of a slot type of another locale of the same bot",
            elsewhere.clone(),
        ),
        (
            "the id of a slot type of another bot",
            other_bots.clone(),
        ),
        (
            "an id that resolves to a resource of another kind",
            intent_id.clone(),
        ),
    ];

    for (case, slot_type_id) in rows {
        let (status, refused) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(slot_body(
                "Doomed",
                json!({ "slotTypeId": slot_type_id }),
            )),
        )
        .await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected CreateSlot to refuse with 412, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {refused:?}"
        );
        let message = text(&refused, "message");
        assert_eq!(
            message,
            state::PARENT_RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one precondition sentence, got {message:?}"
        );
        // The sentence has no variable part, so nothing the request carried reaches it.
        assert!(
            !message.contains(&slot_type_id) && !message.contains("Doomed"),
            "case {case:?}: the sentence names no identifier, got {message:?}"
        );
        // The refusal created nothing.
        let listed = listed_slot_names(&router, &bot_id, "en_US", &intent_id).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused create must leave the intent with no slot, got \
             {listed:?}"
        );
    }

    // The intent the slot hangs off is resolved the same way.
    let misses: Vec<(&str, String)> = vec![
        (
            "an intent id never minted",
            slots_uri(&bot_id, "DRAFT", "en_US", "ABCDEFGHIJ"),
        ),
        (
            "an intent of another locale of the same bot",
            slots_uri(&bot_id, "DRAFT", "de_DE", &intent_id),
        ),
        (
            "an intent of another bot",
            slots_uri(&other_bot_id, "DRAFT", "en_US", &intent_id),
        ),
        (
            "a locale the bot does not have",
            slots_uri(&bot_id, "DRAFT", "nl_NL", &intent_id),
        ),
    ];
    for (case, uri) in misses {
        let (status, refused) = call(&router, "PUT", &uri, Some(slot_body("Doomed", json!({})))).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected CreateSlot to refuse with 412, got {status} {refused:?}"
        );
        assert_eq!(
            text(&refused, "message"),
            state::PARENT_RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one precondition sentence, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_created_slot_is_stored_and_read_back_through_its_own_intent() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotRoundTripBot", "en_US").await;
    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body(
            "OrderPizza",
            json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
        ),
    )
    .await;
    let slot_type_id = create_slot_type(
        &router,
        &bot_id,
        "en_US",
        &slot_type_body("PizzaSize", json!({})),
    )
    .await;

    // The members a create reports back and a describe reports again, unchanged.
    const ECHOED_BY_DESCRIBE: [&str; 6] = [
        "slotName",
        "description",
        "slotTypeId",
        "valueElicitationSetting",
        "obfuscationSetting",
        "multipleValuesSetting",
    ];

    // Rows differ in which reference the slot carries (a custom slot type of the locale,
    // a built-in one Amazon Lex supplies, none at all) and in which optional members are
    // set.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a custom slot type of the same locale",
            slot_body("Size", json!({ "slotTypeId": slot_type_id })),
        ),
        (
            "a built-in slot type, with a description",
            slot_body(
                "Quantity",
                json!({ "slotTypeId": "AMAZON.Number", "description": "how many" }),
            ),
        ),
        (
            "another built-in slot type, with values obfuscated in the logs",
            slot_body(
                "Street",
                json!({
                    "slotTypeId": "AMAZON.AlphaNumeric",
                    "obfuscationSetting": { "obfuscationSettingType": "DefaultObfuscation" },
                }),
            ),
        ),
        (
            "no slot type reference at all",
            slot_body("Freeform", json!({})),
        ),
        (
            "a multi-value slot, which the en_US locale allows",
            slot_body(
                "Toppings",
                json!({
                    "slotTypeId": "AMAZON.AlphaNumeric",
                    "multipleValuesSetting": { "allowMultipleValues": true },
                }),
            ),
        ),
        (
            "an optional slot, which elicits nothing",
            json!({
                "slotName": "Note",
                "valueElicitationSetting": { "slotConstraint": "Optional" },
            }),
        ),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, request) in rows {
        let (status, answered) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateSlot to answer 200, got {status} {answered:?}"
        );
        let slot_id = text(&answered, "slotId");
        // The modelled `Id` shape: exactly 10 characters from `[0-9a-zA-Z]`.
        assert!(
            slot_id.len() == 10 && slot_id.chars().all(char::is_alphanumeric),
            "case {case:?}: expected a 10-character alphanumeric slotId, got {slot_id:?}"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&answered, name),
                member(&request, name),
                "case {case:?}: CreateSlot must report the stored {name}, got {answered:?}"
            );
        }
        assert_eq!(
            text(&answered, "intentId"),
            intent_id,
            "case {case:?}: CreateSlot must report the intent it was called on, got {answered:?}"
        );
        let created_at = seconds(&answered, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {answered:?}"
        );

        // READ BACK — the stored record, member for member.
        let (status, described) = call(
            &router,
            "GET",
            &slot_uri(&bot_id, "DRAFT", "en_US", &intent_id, &slot_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSlot to answer 200, got {status} {described:?}"
        );
        for name in ECHOED_BY_DESCRIBE {
            assert_eq!(
                member(&described, name),
                member(&request, name),
                "case {case:?}: DescribeSlot must report the stored {name}, got {described:?}"
            );
        }
        assert_eq!(
            text(&described, "slotId"),
            slot_id,
            "case {case:?}: DescribeSlot must report the id it was asked about, got {described:?}"
        );
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );
        assert!(
            seconds(&described, "lastUpdatedDateTime") >= created_at,
            "case {case:?}: lastUpdatedDateTime must not precede creationDateTime, got \
             {described:?}"
        );

        created.push((slot_id, text(&request, "slotName")));
    }

    // Every id minted is distinct, and the listing is built from the same records.
    let ids: BTreeSet<&str> = created.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "every CreateSlot must mint its own slotId, got {created:?}"
    );
    let listed = listed_slot_names(&router, &bot_id, "en_US", &intent_id).await;
    let mut expected: Vec<String> = created.iter().map(|(_, name)| name.clone()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "ListSlots must report every stored slot of the intent, by name"
    );

    // A second slot under a name the intent already carries is refused, and the stored
    // slot is left as it was.
    let (status, refused) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(slot_body("Size", json!({ "slotTypeId": "AMAZON.Number" }))),
    )
    .await;
    assert_eq!(
        status, 400,
        "a duplicate slotName must be refused, got {status} {refused:?}"
    );
    assert_eq!(
        text(&refused, "message"),
        state::name_already_exists(state::SLOT_RESOURCE_LABEL, "Size"),
        "expected the service's name-in-use sentence, got {refused:?}"
    );
    assert_eq!(
        listed_slot_names(&router, &bot_id, "en_US", &intent_id).await,
        expected,
        "a refused create must leave the listing as it was"
    );
}

#[tokio::test]
async fn a_slot_is_reachable_only_under_the_intent_that_holds_it() {
    let router = emulator();
    let bot_id = host_locale(&router, "SlotScopeBot", "en_US").await;
    let other_bot_id = host_locale(&router, "OtherSlotScopeBot", "en_US").await;
    let utterance = json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] });
    let intent_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body("OrderPizza", utterance.clone()),
    )
    .await;
    let sibling_id = create_intent(
        &router,
        &bot_id,
        "en_US",
        &intent_body("CancelOrder", utterance.clone()),
    )
    .await;
    let other_intent_id = create_intent(
        &router,
        &other_bot_id,
        "en_US",
        &intent_body("OrderPizza", utterance),
    )
    .await;

    let (status, created) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(slot_body("Size", json!({ "slotTypeId": "AMAZON.Number" }))),
    )
    .await;
    assert_eq!(status, 200, "the slot must be created, got {status} {created:?}");
    let slot_id = text(&created, "slotId");

    // The same name in ANOTHER intent is a different slot, and each intent lists only
    // its own.
    let (status, sibling) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &sibling_id),
        Some(slot_body("Size", json!({ "slotTypeId": "AMAZON.Number" }))),
    )
    .await;
    assert_eq!(
        status, 200,
        "the same slotName in another intent must be accepted, got {status} {sibling:?}"
    );
    assert_ne!(
        text(&sibling, "slotId"),
        slot_id,
        "a slot of another intent is its own record, got {sibling:?}"
    );

    // Every scope but its own misses.
    let misses: Vec<(&str, String)> = vec![
        (
            "another intent of the same locale",
            slot_uri(&bot_id, "DRAFT", "en_US", &sibling_id, &slot_id),
        ),
        (
            "another bot",
            slot_uri(&other_bot_id, "DRAFT", "en_US", &other_intent_id, &slot_id),
        ),
        (
            "a slot id never minted",
            slot_uri(&bot_id, "DRAFT", "en_US", &intent_id, "ABCDEFGHIJ"),
        ),
    ];
    for (case, uri) in misses {
        let (status, response) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeSlot to answer 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
        assert_eq!(
            text(&response, "message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one not-found sentence, got {response:?}"
        );
    }

    // A slot is defined inside its bot, so deleting the bot takes it too.
    let (status, _) = call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(status, 202, "deleting the host bot must succeed");
    let (status, response) = call(
        &router,
        "GET",
        &slot_uri(&bot_id, "DRAFT", "en_US", &intent_id, &slot_id),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted bot's slot must be gone, got {status} {response:?}"
    );
}
