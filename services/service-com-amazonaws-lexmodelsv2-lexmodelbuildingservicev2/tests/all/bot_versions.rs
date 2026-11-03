// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Lex Models V2 bot version slice: `CreateBotVersion` and
//! `DescribeBotVersion`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the intent, slot type and slot helpers are the other suites', reused rather
//! than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a create numbers the version one past the number of versions the bot has had, and
//!   stores it, so `DescribeBotVersion` reports it back carrying the bot settings as they
//!   were when it was created;
//! * a version is an immutable SNAPSHOT of the version each of its locales names as that
//!   locale's source: the locale and every intent, slot type and slot defined in it are
//!   readable under the new version's number by the id they already had, and what the
//!   draft does afterwards does not reach them;
//! * a locale the version would copy has to exist in the version named as its source, and
//!   a create naming one that does not resolve stores nothing;
//! * a version is `Versioning` until the `Available` the clock owes falls due.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{
    call, create_bot, create_body, emulator, error_code, member, seconds, text,
};
use crate::bot_locale::{locale_body, locale_uri, locales_uri};
use crate::intents_and_builds::{create_intent, intent_body, intent_uri};
use crate::slot_type::{create_slot_type, slot_type_body, slot_types_uri};
use crate::slots::{slot_body, slot_uri, slots_uri};

/// `CreateBotVersion`'s URI: `PUT /bots/{botId}/botversions`.
fn versions_uri(bot_id: &str) -> String {
    format!("/bots/{bot_id}/botversions")
}

/// `DescribeBotVersion`'s URI, which adds the version number to [`versions_uri`]:
/// `GET /bots/{botId}/botversions/{botVersion}`.
fn version_uri(bot_id: &str, bot_version: &str) -> String {
    format!("{}/{bot_version}", versions_uri(bot_id))
}

/// `DescribeSlotType`'s URI, which adds the `slotTypeId` to the locale's slot types.
fn slot_type_uri(bot_id: &str, bot_version: &str, locale_id: &str, slot_type_id: &str) -> String {
    format!(
        "{}/{slot_type_id}",
        slot_types_uri(bot_id, bot_version, locale_id)
    )
}

/// A `CreateBotVersion` request body: one `botVersionLocaleSpecification` entry per
/// locale named, each taken from `source`, plus `extra`.
fn version_body(locales: &[&str], source: &str, extra: Value) -> Value {
    let spec: Map<String, Value> = locales
        .iter()
        .map(|locale| {
            (
                (*locale).to_string(),
                json!({ "sourceBotVersion": source }),
            )
        })
        .collect();
    let mut body = json!({ "botVersionLocaleSpecification": Value::Object(spec) });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// Create a version, assert it was accepted, and return the whole response.
async fn create_version(router: &MultiServiceRouter, bot_id: &str, body: &Value) -> Value {
    let (status, created) = call(router, "PUT", &versions_uri(bot_id), Some(body.clone())).await;
    assert_eq!(
        status, 202,
        "CreateBotVersion {body:?}: expected 202, got {status} {created:?}"
    );
    created
}

/// An intent name for one locale, satisfying the modelled `Name` pattern
/// `^([0-9a-zA-Z][_-]?){1,100}$`.
fn intent_name(locale_id: &str) -> String {
    format!("Order_{locale_id}")
}

/// Create a bot carrying `locales` on its `DRAFT` version, each holding one intent of its
/// own, and return the minted `botId` with the minted intent id per locale.
async fn draft_bot(
    router: &MultiServiceRouter,
    bot_name: &str,
    locales: &[&str],
) -> (String, BTreeMap<String, String>) {
    let bot_id = create_bot(router, &create_body(bot_name, 300, json!({}))).await;
    let mut intents = BTreeMap::new();
    for locale_id in locales {
        let (status, created) = call(
            router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(locale_body(locale_id, 0.4, json!({}))),
        )
        .await;
        assert_eq!(
            status, 202,
            "locale {locale_id}: expected CreateBotLocale to answer 202, got {status} {created:?}"
        );
        let intent_id = create_intent(
            router,
            &bot_id,
            locale_id,
            &intent_body(
                &intent_name(locale_id),
                json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
            ),
        )
        .await;
        intents.insert((*locale_id).to_string(), intent_id);
    }
    (bot_id, intents)
}

/// The table the round-trip assertions run over: rows differ in how many locales the bot
/// carries, in how many of them the version is taken over, in which locales those are and
/// in whether the version carries a description, at its published maximum length.
fn version_cases() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>, Value)> {
    vec![
        (
            "one locale and no description",
            vec!["en_US"],
            vec!["en_US"],
            json!({}),
        ),
        (
            "one locale and a description at its published maximum length",
            vec!["de_DE"],
            vec!["de_DE"],
            json!({ "description": "d".repeat(2_000) }),
        ),
        (
            "both locales of a two-locale bot",
            vec!["en_US", "ja_JP"],
            vec!["en_US", "ja_JP"],
            json!({ "description": "both locales" }),
        ),
        (
            "one locale of a two-locale bot",
            vec!["en_US", "fr_FR"],
            vec!["fr_FR"],
            json!({}),
        ),
        (
            "three locales at once",
            vec!["en_US", "es_US", "it_IT"],
            vec!["en_US", "es_US", "it_IT"],
            json!({ "description": "three locales" }),
        ),
    ]
}

#[tokio::test]
async fn a_created_version_snapshots_the_locales_it_names_and_is_read_back() {
    let router = emulator();

    for (index, (case, bot_locales, versioned, extra)) in version_cases().into_iter().enumerate() {
        let (bot_id, draft_intents) =
            draft_bot(&router, &format!("VersionRoundTripBot{index}"), &bot_locales).await;
        let (_, bot) = call(&router, "GET", &format!("/bots/{bot_id}"), None).await;

        let request = version_body(&versioned, "DRAFT", extra);
        let created = create_version(&router, &bot_id, &request).await;

        // "When you create the first version of a bot, Amazon Lex sets the version
        // number to 1."
        assert_eq!(
            text(&created, "botVersion"),
            "1",
            "case {case:?}: the first version of a bot is number 1, got {created:?}"
        );
        assert_eq!(
            text(&created, "botId"),
            bot_id,
            "case {case:?}: CreateBotVersion must report the bot it was called on, got {created:?}"
        );
        assert_eq!(
            text(&created, "botStatus"),
            state::BOT_STATUS_VERSIONING,
            "case {case:?}: a version being created reports the transient status, got {created:?}"
        );
        assert_eq!(
            member(&created, "description"),
            member(&request, "description"),
            "case {case:?}: CreateBotVersion must report the stored description, got {created:?}"
        );
        assert_eq!(
            member(&created, "botVersionLocaleSpecification"),
            member(&request, "botVersionLocaleSpecification"),
            "case {case:?}: CreateBotVersion must report the source version of every locale \
             it was given, got {created:?}"
        );
        let created_at = seconds(&created, "creationDateTime");
        assert!(
            created_at > 0.0,
            "case {case:?}: creationDateTime must be recorded, got {created:?}"
        );

        // READ BACK — the stored version, member for member. A version is immutable, so
        // it carries the bot settings as they were when it was created.
        let (status, described) = call(&router, "GET", &version_uri(&bot_id, "1"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBotVersion to answer 200, got {status} {described:?}"
        );
        for name in [
            "botName",
            "roleArn",
            "dataPrivacy",
            "idleSessionTTLInSeconds",
            "botType",
            "botMembers",
        ] {
            assert_eq!(
                member(&described, name),
                member(&bot, name),
                "case {case:?}: the version must report the bot's {name}, got {described:?}"
            );
        }
        assert_eq!(
            text(&described, "botVersion"),
            "1",
            "case {case:?}: DescribeBotVersion must report the version it was asked about, \
             got {described:?}"
        );
        assert_eq!(
            member(&described, "description"),
            member(&request, "description"),
            "case {case:?}: DescribeBotVersion must report the stored description, got \
             {described:?}"
        );
        assert_eq!(
            text(&described, "botStatus"),
            state::BOT_STATUS_VERSIONING,
            "case {case:?}: a version read back immediately is still being created, got \
             {described:?}"
        );
        assert!(
            (seconds(&described, "creationDateTime") - created_at).abs() < 1e-3,
            "case {case:?}: creationDateTime must be the stored one, got {described:?}"
        );

        // Every locale the create named is in the version, holding the intent the draft
        // held under the very id the draft minted for it.
        for locale_id in versioned.iter().copied() {
            let (status, locale) = call(&router, "GET", &locale_uri(&bot_id, "1", locale_id), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: locale {locale_id} must be in the version, got {status} \
                 {locale:?}"
            );
            assert_eq!(
                text(&locale, "botVersion"),
                "1",
                "case {case:?}: the copied locale belongs to the new version, got {locale:?}"
            );
            let intent_id = &draft_intents[locale_id];
            let (status, intent) = call(
                &router,
                "GET",
                &intent_uri(&bot_id, "1", locale_id, intent_id),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: intent {intent_id} of {locale_id} must be in the version under \
                 the id the draft minted, got {status} {intent:?}"
            );
            assert_eq!(
                text(&intent, "intentName"),
                intent_name(locale_id),
                "case {case:?}: the copied intent keeps its definition, got {intent:?}"
            );
            let (_, draft_intent) = call(
                &router,
                "GET",
                &intent_uri(&bot_id, "DRAFT", locale_id, intent_id),
                None,
            )
            .await;
            assert_eq!(
                member(&intent, "sampleUtterances"),
                member(&draft_intent, "sampleUtterances"),
                "case {case:?}: the copied intent carries what the draft intent carries, got \
                 {intent:?}"
            );
        }

        // A locale the create did NOT name is not in the version, and neither is its
        // intent: only the locale data the request asked for is copied.
        for locale_id in bot_locales
            .iter()
            .copied()
            .filter(|locale| !versioned.contains(locale))
        {
            let (status, response) =
                call(&router, "GET", &locale_uri(&bot_id, "1", locale_id), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: locale {locale_id} was not named, so the version has none, got \
                 {status} {response:?}"
            );
            let intent_id = &draft_intents[locale_id];
            let (status, response) = call(
                &router,
                "GET",
                &intent_uri(&bot_id, "1", locale_id, intent_id),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: an intent of a locale the version has not got cannot be read, \
                 got {status} {response:?}"
            );
        }

        // Past the due instant the transition the clock owed is materialised.
        tokio::time::sleep(state::CREATION_SETTLES_AFTER + Duration::from_millis(150)).await;
        let (_, described) = call(&router, "GET", &version_uri(&bot_id, "1"), None).await;
        assert_eq!(
            text(&described, "botStatus"),
            state::BOT_STATUS_AVAILABLE,
            "case {case:?}: a version past its due instant is ready for use, got {described:?}"
        );
    }
}

#[tokio::test]
async fn versions_are_numbered_in_sequence_and_each_one_is_an_immutable_snapshot() {
    let router = emulator();
    let (bot_id, draft_intents) = draft_bot(&router, "VersionSequenceBot", &["en_US"]).await;
    let intent_id = draft_intents["en_US"].clone();

    // The draft locale is given a slot type and a slot too, so what the snapshot copies
    // spans every kind of resource a locale holds.
    let slot_type_id = create_slot_type(
        &router,
        &bot_id,
        "en_US",
        &slot_type_body(
            "PizzaSize",
            json!({ "slotTypeValues": [{ "sampleValue": { "value": "large" } }] }),
        ),
    )
    .await;
    let (status, slot) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(slot_body(
            "PizzaSize_Slot",
            json!({ "slotTypeId": slot_type_id }),
        )),
    )
    .await;
    assert_eq!(status, 200, "the draft slot must be created, got {slot:?}");
    let slot_id = text(&slot, "slotId");

    // Each create is numbered one past the versions the bot has had, whether it is taken
    // from the draft or from a version already published.
    let sources = [
        ("1", "DRAFT", "the first version"),
        ("2", "DRAFT", "the second version"),
        ("3", "1", "a version of a version"),
    ];
    let mut created_order: Vec<String> = Vec::new();
    let mut later_intent_id = String::new();
    for (expected, source, description) in sources {
        // Between the first and the second version the draft moves on: the intent is
        // redefined and another intent is added to the same locale.
        if expected == "2" {
            let (status, updated) = call(
                &router,
                "PUT",
                &intent_uri(&bot_id, "DRAFT", "en_US", &intent_id),
                Some(intent_body(
                    &intent_name("en_US"),
                    json!({ "description": "the redefined draft intent" }),
                )),
            )
            .await;
            assert_eq!(
                status, 200,
                "the draft intent must be updated, got {status} {updated:?}"
            );
            later_intent_id = create_intent(
                &router,
                &bot_id,
                "en_US",
                &intent_body("CancelOrder", json!({})),
            )
            .await;
        }
        let created = create_version(
            &router,
            &bot_id,
            &version_body(&["en_US"], source, json!({ "description": description })),
        )
        .await;
        assert_eq!(
            text(&created, "botVersion"),
            expected,
            "source {source:?}: versions increase in an increment of 1, got {created:?}"
        );
        created_order.push(text(&created, "botVersion"));
    }
    assert_eq!(
        created_order,
        vec!["1".to_string(), "2".to_string(), "3".to_string()],
        "every create must hand out its own number, got {created_order:?}"
    );

    // What each version holds is what its source held WHEN it was taken, not what the
    // draft holds now: version 1 and the version taken from it carry the first
    // definition, version 2 the redefined one.
    for (case, bot_version, expected) in [
        ("the version taken from the draft first", "1", Value::Null),
        (
            "the version taken from the draft after it was redefined",
            "2",
            Value::String("the redefined draft intent".to_string()),
        ),
        ("the version taken from version 1", "3", Value::Null),
    ] {
        let (status, intent) = call(
            &router,
            "GET",
            &intent_uri(&bot_id, bot_version, "en_US", &intent_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the copied intent must be readable, got {status} {intent:?}"
        );
        assert_eq!(
            member(&intent, "description"),
            expected,
            "case {case:?}: version {bot_version} must report the definition it snapshotted, \
             got {intent:?}"
        );
    }

    // The intent added to the draft after version 1 was taken is in version 2 only, and
    // the slot type and the slot the locale held are in every version that copied it.
    for (case, bot_version, holds_the_later_intent) in [
        ("the version taken before the intent was added", "1", false),
        ("the version taken after it was added", "2", true),
        ("the version taken from version 1", "3", false),
    ] {
        let (status, slot_type) = call(
            &router,
            "GET",
            &slot_type_uri(&bot_id, bot_version, "en_US", &slot_type_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the copied slot type must be readable, got {status} {slot_type:?}"
        );
        assert_eq!(
            text(&slot_type, "slotTypeName"),
            "PizzaSize",
            "case {case:?}: the copied slot type keeps its definition, got {slot_type:?}"
        );
        let (status, slot) = call(
            &router,
            "GET",
            &slot_uri(&bot_id, bot_version, "en_US", &intent_id, &slot_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the copied slot must be readable, got {status} {slot:?}"
        );
        assert_eq!(
            text(&slot, "slotTypeId"),
            slot_type_id,
            "case {case:?}: the copied slot still elicits values for the copied slot type, \
             got {slot:?}"
        );

        let (status, described) = call(&router, "GET", &version_uri(&bot_id, bot_version), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: every version stays readable under its own number, got {status} \
             {described:?}"
        );
        assert_eq!(
            text(&described, "botVersion"),
            bot_version,
            "case {case:?}: DescribeBotVersion must report the version asked about, got \
             {described:?}"
        );

        // The intent added to the draft between version 1 and version 2 is only in the
        // version whose source already held it.
        let (status, later) = call(
            &router,
            "GET",
            &intent_uri(&bot_id, bot_version, "en_US", &later_intent_id),
            None,
        )
        .await;
        let expected_status = if holds_the_later_intent { 200 } else { 404 };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} for the intent added after version 1, \
             got {status} {later:?}"
        );
    }

    // The descriptions are the ones each create named, so no version took another's.
    for (bot_version, description) in [
        ("1", "the first version"),
        ("2", "the second version"),
        ("3", "a version of a version"),
    ] {
        let (_, described) = call(&router, "GET", &version_uri(&bot_id, bot_version), None).await;
        assert_eq!(
            text(&described, "description"),
            description,
            "version {bot_version}: expected the description its create named, got {described:?}"
        );
    }

    // A listing reports the highest number a version of the bot was created under.
    let (status, listed) = call(&router, "POST", "/bots", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "expected ListBots to answer 200, got {status} {listed:?}"
    );
    let summary = member(&listed, "botSummaries")
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| text(entry, "botId") == bot_id)
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_eq!(
        text(&summary, "latestBotVersion"),
        "3",
        "the bot's summary must report the latest numbered version, got {summary:?}"
    );

    // A number no version was created under is not a version.
    let (status, response) = call(&router, "GET", &version_uri(&bot_id, "4"), None).await;
    assert_eq!(
        status, 404,
        "a version number never handed out cannot be read, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response:?}"
    );
    assert_eq!(
        text(&response, "message"),
        state::RESOURCE_NOT_FOUND_MESSAGE,
        "expected the service's one not-found sentence, got {response:?}"
    );
}

#[tokio::test]
async fn a_version_is_refused_when_what_it_would_copy_is_not_there() {
    let router = emulator();
    let (bot_id, _) = draft_bot(&router, "RefusedVersionBot", &["en_US"]).await;

    // Each row satisfies every modelled constraint and still names locale data the store
    // cannot resolve.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "a bot id no bot was ever minted under",
            versions_uri("ZZZZZZZZZZ"),
            version_body(&["en_US"], "DRAFT", json!({})),
        ),
        (
            "a locale the bot has not got",
            versions_uri(&bot_id),
            version_body(&["ko_KR"], "DRAFT", json!({})),
        ),
        (
            "a source version that was never created",
            versions_uri(&bot_id),
            version_body(&["en_US"], "7", json!({})),
        ),
        (
            "one locale that resolves beside one that does not",
            versions_uri(&bot_id),
            version_body(&["en_US", "it_IT"], "DRAFT", json!({})),
        ),
    ];

    for (case, uri, body) in rows {
        let (status, refused) = call(&router, "PUT", &uri, Some(body)).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected CreateBotVersion to refuse with 412, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {refused:?}"
        );
        assert_eq!(
            text(&refused, "message"),
            state::PARENT_RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one precondition sentence, got {refused:?}"
        );
    }

    // None of the refusals stored anything: there is no version to read, and the next
    // version the bot does get is still number 1.
    let (status, response) = call(&router, "GET", &version_uri(&bot_id, "1"), None).await;
    assert_eq!(
        status, 404,
        "a refused create must leave no version behind, got {status} {response:?}"
    );
    let created = create_version(
        &router,
        &bot_id,
        &version_body(&["en_US"], "DRAFT", json!({})),
    )
    .await;
    assert_eq!(
        text(&created, "botVersion"),
        "1",
        "a refused create must not have consumed a version number, got {created:?}"
    );

    // A version is a version OF a bot, so deleting the bot takes it with it.
    let (status, deleted) = call(&router, "DELETE", &format!("/bots/{bot_id}"), None).await;
    assert_eq!(
        status, 202,
        "deleting the host bot must succeed, got {status} {deleted:?}"
    );
    let (status, response) = call(&router, "GET", &version_uri(&bot_id, "1"), None).await;
    assert_eq!(
        status, 404,
        "a deleted bot's version must be gone, got {status} {response:?}"
    );
}
