// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the wording of this service's "that name is already in
//! use" refusals: `CreateSlotType`, `CreateBot` and `UpdateBot`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the error envelope a client parses is what is asserted
//! here. The harness and the bot/locale/slot-type setup are the other suites', reused
//! rather than copied.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: when the
//! name a request asks for is already in use where that name has to be unique, the
//! refusal's message names the kind of resource and the requested name and nothing
//! else — the scope the name is unique in (the account, the bot version, the locale) is
//! not part of the text, so the message is the same whatever the scope.
//!
//! Rows differ in the shape and length of the requested name (both published bounds of
//! the modelled `Name` shape, the separators its pattern allows, digits only), in the
//! locale and bot the name is taken in, and in what else the refused request carries.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, create_body, create_bot, emulator, error_code, text};
use crate::slot_type::{
    create_slot_type, host_locale, name_of_length, slot_type_body, slot_types_uri,
};

/// How the service names a slot type in a message about its name.
const SLOT_TYPE: &str = "Slot Type";

/// How the service names a bot in a message about its name.
const BOT: &str = "Bot";

/// The message a name-already-in-use refusal carries, written out here rather than read
/// off the crate so that the test pins the wording independently of the code under
/// test: the fixed text, with the requested name as its ONE substitution.
fn name_taken_message(resource_label: &str, name: &str) -> String {
    format!("{resource_label} with name {name} already exists. Use another name.")
}

/// The `message` member of an error envelope.
fn message(body: &Value) -> String {
    text(body, "message")
}

/// One `slotTypeValues` entry, so a duplicate request can differ from the stored one.
fn sample(value: &str) -> Value {
    json!({ "sampleValue": { "value": value } })
}

/// A `UpdateBot` request body renaming the bot to `name`.
fn rename_body(name: &str) -> Value {
    create_body(name, 300, json!({}))
}

/// Create the slot type `name` in `locale_id` of `bot_id`'s DRAFT version, then ask for
/// the same name again with `second` as the request body, and return the refusal.
async fn create_twice(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    name: &str,
    second: Value,
) -> (u16, Value) {
    create_slot_type(router, bot_id, locale_id, &slot_type_body(name, json!({}))).await;
    call(
        router,
        "PUT",
        &slot_types_uri(bot_id, "DRAFT", locale_id),
        Some(slot_type_body(name, second)),
    )
    .await
}

#[tokio::test]
async fn a_taken_slot_type_name_is_refused_with_the_name_taken_message() {
    // (case, slotTypeName, localeId, what else the refused request carries)
    let rows: Vec<(&str, String, &str, Value)> = vec![
        (
            "a name re-used with a different definition",
            "PizzaSize".to_string(),
            "en_US",
            json!({
                "slotTypeValues": [sample("gigantic")],
                "valueSelectionSetting": { "resolutionStrategy": "OriginalValue" },
            }),
        ),
        (
            "a name at the published minimum length, re-used bare",
            "A".to_string(),
            "de_DE",
            json!({}),
        ),
        (
            "a name at the published maximum length",
            name_of_length(100),
            "fr_FR",
            json!({ "description": "the losing slot type" }),
        ),
        (
            "a name carrying the separators the pattern allows",
            "Pizza_Size-2".to_string(),
            "ja_JP",
            json!({ "slotTypeValues": [sample("large"), sample("small")] }),
        ),
        (
            "a name of digits only",
            "0123456789".to_string(),
            "es_419",
            json!({ "parentSlotTypeSignature": "AMAZON.AlphaNumeric" }),
        ),
    ];

    for (index, (case, name, locale_id, second)) in rows.iter().enumerate() {
        let router = emulator();
        let bot_id = host_locale(&router, &format!("NameTakenBot{index}"), locale_id).await;
        let (status, response) = create_twice(&router, &bot_id, locale_id, name, second.clone()).await;

        assert_eq!(
            status, 400,
            "case {case:?}: a taken slot type name must be refused with 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert_eq!(
            message(&response),
            name_taken_message(SLOT_TYPE, name),
            "case {case:?}: expected the name-taken message, got {:?}",
            message(&response)
        );
    }
}

#[tokio::test]
async fn the_slot_type_name_taken_message_does_not_depend_on_the_scope() {
    let router = emulator();
    let name = "City";

    // The same name, taken in three different scopes: two locales of one bot, and a
    // locale of another bot. The refusal text must not vary with the scope.
    let first_bot = host_locale(&router, "ScopeOneBot", "en_US").await;
    let second_bot = host_locale(&router, "ScopeTwoBot", "en_US").await;
    let scopes: [(&str, &str); 3] = [
        (first_bot.as_str(), "en_US"),
        (first_bot.as_str(), "de_DE"),
        (second_bot.as_str(), "en_US"),
    ];

    // The second locale on the first bot, so that bot carries the name twice over.
    let (status, created) = call(
        &router,
        "PUT",
        &crate::bot_locale::locales_uri(&first_bot, "DRAFT"),
        Some(crate::bot_locale::locale_body("de_DE", 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "expected CreateBotLocale to answer 202, got {status} {created:?}"
    );

    let expected = name_taken_message(SLOT_TYPE, name);
    for (bot_id, locale_id) in scopes {
        let case = format!("bot {bot_id} locale {locale_id}");
        let (status, response) = create_twice(&router, bot_id, locale_id, name, json!({})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a taken slot type name must be refused with 400, got {status} {response:?}"
        );
        assert_eq!(
            message(&response),
            expected,
            "case {case:?}: the refusal must not carry the scope, got {:?}",
            message(&response)
        );
    }
}

#[tokio::test]
async fn a_taken_bot_name_is_refused_with_the_name_taken_message() {
    // The same rule over the other resource whose name has to be unique: rows differ in
    // the shape and length of the bot name.
    let names: [String; 4] = [
        "TakenName".to_string(),
        "B".to_string(),
        "Pizza_Bot-2".to_string(),
        name_of_length(100),
    ];

    for name in &names {
        let router = emulator();
        let held = create_bot(&router, &create_body(name, 300, json!({}))).await;
        let other = create_bot(&router, &create_body("SomeOtherBot", 300, json!({}))).await;
        assert_ne!(held, other, "the two bots must be distinct records");

        // A create under the held name is refused...
        let (status, response) = call(
            &router,
            "PUT",
            "/bots",
            Some(create_body(name, 600, json!({ "botType": "Bot" }))),
        )
        .await;
        assert_eq!(
            status, 409,
            "name {name:?}: a taken botName must be refused with 409, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "name {name:?}: expected ConflictException, got {response:?}"
        );
        assert_eq!(
            message(&response),
            name_taken_message(BOT, name),
            "name {name:?}: expected the name-taken message, got {:?}",
            message(&response)
        );

        // ...and so is a rename onto it, with the same text.
        let (status, response) = call(
            &router,
            "PUT",
            &format!("/bots/{other}"),
            Some(rename_body(name)),
        )
        .await;
        assert_eq!(
            status, 409,
            "name {name:?}: a rename onto a taken botName must be refused, got {status} {response:?}"
        );
        assert_eq!(
            message(&response),
            name_taken_message(BOT, name),
            "name {name:?}: a rename must carry the same name-taken message, got {:?}",
            message(&response)
        );
    }
}
