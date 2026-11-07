// SPDX-License-Identifier: CC0-1.0

//! What one dialog turn REPORTS: who completed the intent, which slots the
//! intent names, how the turns of one conversation are correlated, how a
//! misaddressed turn is refused, and what a text response carries as a body.
//!
//! Every bot here is AUTHORED the way a client authors one — `CreateBot`,
//! `CreateBotLocale`, `CreateSlotType`, `CreateIntent`, `CreateSlot`,
//! `UpdateIntent`, `BuildBotLocale` over the Lex Models V2 control plane mounted
//! beside this runtime — and then talked to over `RecognizeText` and
//! `RecognizeUtterance`, so what is asserted is the whole path from the bot a
//! client built to the turn it gets back.
//!
//! The rules under test, each over a TABLE of inputs:
//!
//! 1. A turn that closes the dialog reports `Intent.state` `Fulfilled` when the
//!    BOT completed the intent itself — it played a closing response of its own
//!    and has no fulfilment code hook to hand the intent to — and
//!    `ReadyForFulfillment` when the fulfilment is somebody else's.
//! 2. `Intent.slots` is always reported, naming EVERY slot the intent declares,
//!    with the documented `null` for each slot that has no value: "a map of all
//!    of the slots for the intent … if a slot has not been filled, the value is
//!    null" (`Intent$slots`).
//! 3. A `botId`/`botAliasId` pair that resolves to no alias of that bot is
//!    refused with `ResourceNotFoundException` naming both ids, which is a
//!    different sentence from the one a locale with no built model earns.
//! 4. `RecognizeUtterance` synthesizes no speech for a text `responseContentType`,
//!    so its `audioStream` body is empty and the bot's messages travel only in
//!    the `x-amz-lex-messages` header.
//!
//! `SessionState.originatingRequestId` — the third divergence this suite's issue
//! reports — is pinned by `bot_locale_dialog.rs`'s own rule 3, which this crate
//! gained for a sibling issue while this one was in review; there is one suite
//! for it and not two.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::harness::Emulator;
use crate::recognize_text_nlu::{at, build, control, id, say, Bot, LOCALE, ROLE_ARN, THRESHOLD};

/// The intent every conversation here is about, its two slots, and what the
/// author wrote on them.
const ORDER_SHAKE: &str = "OrderShake";
/// See [`ORDER_SHAKE`]: the `Required` slot the dialog elicits.
const FLAVOR: &str = "Flavor";
/// See [`ORDER_SHAKE`]: an `Optional` slot, which the dialog never elicits and
/// the slot map still names.
const TOPPING: &str = "Topping";
/// The prompt authored on [`FLAVOR`].
const FLAVOR_PROMPT: &str = "What flavor would you like?";
/// The `intentClosingSetting` closing response authored on [`ORDER_SHAKE`].
const CLOSING_RESPONSE: &str = "Your shake is on the way.";

/// The sample utterances authored on [`ORDER_SHAKE`]: one that names the slot in
/// the position its value is said in, and two that name no value.
const UTTERANCES: [&str; 3] = [
    "i would like a {Flavor} shake",
    "order a shake",
    "i would like a shake",
];

/// `Fulfilled` and `ReadyForFulfillment`, two of the modelled `IntentState`
/// members (`smithy_query(query="shape",shape="IntentState")`: Failed,
/// Fulfilled, InProgress, ReadyForFulfillment, Waiting, FulfillmentInProgress).
const FULFILLED: &str = "Fulfilled";
/// See [`FULFILLED`].
const READY_FOR_FULFILLMENT: &str = "ReadyForFulfillment";

/// The text `responseContentType` the model lists ("if the value is
/// `text/plain;charset=utf-8`, Amazon Lex V2 returns text in the response"), and
/// the content type a turn reports when the caller asks for none.
const TEXT_CONTENT_TYPE: &str = "text/plain;charset=utf-8";

/// The `requestContentType` a text input stream carries ("Text format —
/// `text/plain; charset=utf-8`").
const TEXT_REQUEST_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// How the shake bot's intent is closed and fulfilled: the two authored settings
/// rule 1 reads.
#[derive(Debug, Clone, Default)]
struct Fulfilment {
    /// The `intentClosingSetting` to author, or `None` for an intent with none.
    closing: Option<Value>,
    /// The `fulfillmentCodeHook` to author, or `None` for an intent with none.
    code_hook: Option<Value>,
}

/// The `intentClosingSetting` carrying [`CLOSING_RESPONSE`], optionally switched
/// off by its own `active` flag.
fn closing_setting(active: Option<bool>) -> Value {
    let mut setting = json!({
        "closingResponse": {
            "messageGroups": [
                {"message": {"plainTextMessage": {"value": CLOSING_RESPONSE}}}
            ],
        },
    });
    if let Some(active) = active {
        setting["active"] = json!(active);
    }
    setting
}

/// Author the shake bot: one built `en_US` locale, a custom `Flavor` slot type,
/// an intent carrying [`UTTERANCES`], a `Required` `Flavor` slot with
/// [`FLAVOR_PROMPT`] and an `Optional` `Topping` slot, closed and fulfilled as
/// `fulfilment` says.
async fn shake_bot(emulator: &Emulator, bot_name: &str, fulfilment: &Fulfilment) -> Bot {
    let created = control(
        emulator,
        "PUT",
        "/bots",
        json!({
            "botName": bot_name,
            "roleArn": ROLE_ARN,
            "dataPrivacy": {"childDirected": false},
            "idleSessionTTLInSeconds": 300,
            "botType": "Bot",
        }),
        202,
        "CreateBot",
    )
    .await;
    let bot_id = id(&created, "botId");

    control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales"),
        json!({"localeId": LOCALE, "nluIntentConfidenceThreshold": THRESHOLD}),
        202,
        "CreateBotLocale",
    )
    .await;

    let slot_type = control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/slottypes"),
        json!({
            "slotTypeName": FLAVOR,
            "slotTypeValues": [
                {"sampleValue": {"value": "chocolate"}},
                {"sampleValue": {"value": "vanilla"}},
                {"sampleValue": {"value": "strawberry"}},
            ],
            "valueSelectionSetting": {"resolutionStrategy": "TopResolution"},
        }),
        200,
        "CreateSlotType",
    )
    .await;
    let slot_type_id = id(&slot_type, "slotTypeId");

    let sample_utterances: Vec<Value> = UTTERANCES
        .iter()
        .map(|utterance| json!({"utterance": utterance}))
        .collect();
    let mut intent_body = json!({
        "intentName": ORDER_SHAKE,
        "sampleUtterances": sample_utterances.clone(),
    });
    if let Some(closing) = &fulfilment.closing {
        intent_body["intentClosingSetting"] = closing.clone();
    }
    if let Some(code_hook) = &fulfilment.code_hook {
        intent_body["fulfillmentCodeHook"] = code_hook.clone();
    }
    let intent = control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents"),
        intent_body.clone(),
        200,
        "CreateIntent",
    )
    .await;
    let intent_id = id(&intent, "intentId");

    // (the slot, its `slotConstraint`, the prompt authored on it)
    let mut priorities: Vec<Value> = Vec::new();
    for (priority, (slot_name, constraint, prompt)) in [
        (FLAVOR, "Required", Some(FLAVOR_PROMPT)),
        (TOPPING, "Optional", None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut elicitation = json!({"slotConstraint": constraint});
        if let Some(prompt) = prompt {
            elicitation["promptSpecification"] = json!({
                "messageGroups": [{"message": {"plainTextMessage": {"value": prompt}}}],
                "maxRetries": 2,
            });
        }
        let slot = control(
            emulator,
            "PUT",
            &format!(
                "/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents/{intent_id}/slots"
            ),
            json!({
                "slotName": slot_name,
                "slotTypeId": slot_type_id,
                "valueElicitationSetting": elicitation,
            }),
            200,
            "CreateSlot",
        )
        .await;
        priorities.push(json!({"priority": priority + 1, "slotId": id(&slot, "slotId")}));
    }

    // A build requires every slot of the intent to appear in `slotPriorities`,
    // which is also the order the dialog elicits them in.
    intent_body["slotPriorities"] = json!(priorities);
    control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents/{intent_id}"),
        intent_body,
        200,
        "UpdateIntent",
    )
    .await;

    // The alias the runtime is addressed by is the one Amazon Lex keeps for
    // testing the bot, read off the control plane rather than assumed.
    let aliases = control(
        emulator,
        "POST",
        &format!("/bots/{bot_id}/botaliases"),
        json!({}),
        200,
        "ListBotAliases",
    )
    .await;
    let bot_alias_id = aliases
        .get("botAliasSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .map(|summary| id(summary, "botAliasId"))
        .unwrap_or_else(|| panic!("the bot must have the test alias, got {aliases:?}"));

    build(emulator, &bot_id).await;

    Bot {
        bot_id,
        bot_alias_id,
        locale_id: LOCALE.to_string(),
    }
}

/// The slot map of the turn's intent.
fn slots(turn: &Value) -> Value {
    turn.pointer("/sessionState/intent/slots")
        .cloned()
        .unwrap_or_else(|| panic!("the intent must report a slot map, got {turn:?}"))
}

/// Rule 1: who completed the intent decides the state the closing turn reports,
/// over the authored settings that decide it — and it is the same answer whether
/// the dialog closed on one fully specified utterance or on the answer to an
/// elicitation.
#[tokio::test]
async fn the_closing_turn_reports_who_completed_the_intent() {
    // (case, the authored fulfilment, the state the closing turn reports)
    let rows: Vec<(&str, Fulfilment, &str)> = vec![
        (
            "a closing response and no fulfilment hook: the bot finished it",
            Fulfilment {
                closing: Some(closing_setting(None)),
                code_hook: None,
            },
            FULFILLED,
        ),
        (
            "a closing response the author switched off, so the bot says nothing",
            Fulfilment {
                closing: Some(closing_setting(Some(false))),
                code_hook: None,
            },
            READY_FOR_FULFILLMENT,
        ),
        (
            "no closing response at all: the caller fulfils the intent",
            Fulfilment::default(),
            READY_FOR_FULFILLMENT,
        ),
        (
            "a closing response and an enabled fulfilment hook: the hook fulfils it",
            Fulfilment {
                closing: Some(closing_setting(None)),
                code_hook: Some(json!({"enabled": true})),
            },
            READY_FOR_FULFILLMENT,
        ),
        (
            "a closing response and a hook its own active flag switched off",
            Fulfilment {
                closing: Some(closing_setting(None)),
                code_hook: Some(json!({"enabled": true, "active": false})),
            },
            FULFILLED,
        ),
        (
            "a closing response and a hook that is not enabled",
            Fulfilment {
                closing: Some(closing_setting(None)),
                code_hook: Some(json!({"enabled": false})),
            },
            FULFILLED,
        ),
    ];

    for (index, (case, fulfilment, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let bot = shake_bot(&emulator, &format!("ShakeBot{index}"), fulfilment).await;

        // The same close reached two ways: one fully specified utterance, and
        // the turn that answers an elicitation.
        let single = say(&emulator, &bot, "single", "i would like a chocolate shake").await;
        say(&emulator, &bot, "elicited", "order a shake").await;
        let answered = say(&emulator, &bot, "elicited", "vanilla").await;

        for (how, turn) in [("one utterance", &single), ("an elicited answer", &answered)] {
            assert_eq!(
                at(turn, "/sessionState/dialogAction/type").as_deref(),
                Some("Close"),
                "case {case:?} by {how}: the dialog must close, got {turn:?}"
            );
            assert_eq!(
                at(turn, "/sessionState/intent/state").as_deref(),
                Some(*expected),
                "case {case:?} by {how}: the intent's state, got {turn:?}"
            );
            // The closing response the author wrote is what the bot plays, and
            // a setting that is not active plays nothing.
            let plays_closing = fulfilment
                .closing
                .as_ref()
                .is_some_and(|closing| closing.get("active") != Some(&json!(false)));
            let expected_messages = if plays_closing {
                vec![CLOSING_RESPONSE.to_string()]
            } else {
                Vec::new()
            };
            let actual: Vec<String> = turn
                .get("messages")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter_map(|message| at(message, "/content"))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                actual, expected_messages,
                "case {case:?} by {how}: the messages the closing turn sends, got {turn:?}"
            );
        }
    }
}

/// Rule 2: the slot map names every slot the intent declares, with `null` for
/// each one that has no value — over the turns of a conversation, a turn that
/// fills a slot outright, and an intent that declares no slots at all. What a
/// turn reported is session state, so `GetSession` reports the same map.
#[tokio::test]
async fn the_intent_names_every_slot_it_declares() {
    let emulator = Emulator::new();
    let bot = shake_bot(
        &emulator,
        "SlotMapShakeBot",
        &Fulfilment {
            closing: Some(closing_setting(None)),
            code_hook: None,
        },
    )
    .await;

    // (case, session, the utterances to say in it, the slot map the last turn
    // must report)
    let rows: Vec<(&str, &str, Vec<&str>, Value)> = vec![
        (
            "an opening turn that filled nothing",
            "elicit",
            vec!["order a shake"],
            json!({FLAVOR: null, TOPPING: null}),
        ),
        (
            "the answer that fills the elicited slot, the optional one still open",
            "elicit",
            vec!["chocolate"],
            json!({
                FLAVOR: {
                    "value": {
                        "originalValue": "chocolate",
                        "interpretedValue": "chocolate",
                        "resolvedValues": ["chocolate"],
                    },
                    "shape": "Scalar",
                },
                TOPPING: null,
            }),
        ),
        (
            "one utterance that fills the slot outright",
            "single",
            vec!["i would like a strawberry shake"],
            json!({
                FLAVOR: {
                    "value": {
                        "originalValue": "strawberry",
                        "interpretedValue": "strawberry",
                        "resolvedValues": ["strawberry"],
                    },
                    "shape": "Scalar",
                },
                TOPPING: null,
            }),
        ),
        (
            "an intent of the locale that declares no slots",
            "fallback",
            vec!["what is the weather like tomorrow"],
            json!({}),
        ),
    ];

    for (case, session, utterances, expected) in &rows {
        let mut turn = Value::Null;
        for text in utterances {
            turn = say(&emulator, &bot, session, text).await;
        }
        let actual = slots(&turn);
        assert_eq!(
            &actual, expected,
            "case {case:?}: the slot map the turn reports, got {turn:?}"
        );
        // The unfilled slots are the documented JSON `null`, not an empty
        // object: a client has to be able to tell the two apart.
        for (name, value) in actual.as_object().expect("the slot map is an object") {
            if value.is_null() {
                continue;
            }
            assert!(
                value.get("value").is_some(),
                "case {case:?}: slot {name:?} is neither null nor filled, got {value:?}"
            );
        }

        let read = emulator.send("GET", &bot.session(session), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the conversation must be readable, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let stored = read.json();
        assert_eq!(
            slots(&stored),
            actual,
            "case {case:?}: GetSession must report the same slot map, got {stored:?}"
        );
    }
}

/// Rule 3: the refusal names the condition actually hit — an unresolvable
/// `botId`/`botAliasId` pair carries both ids from the request, while a locale
/// with no built model carries the fixed BotId/BotLocale sentence — and both
/// operations that take a turn answer the same way.
#[tokio::test]
async fn a_misaddressed_turn_is_refused_for_the_condition_it_hit() {
    /// What Amazon Lex V2 says when the locale a turn addresses has no model it
    /// can serve. Fixed text: it names neither the bot nor the locale.
    const NOT_VALID_LOCALE: &str = "The provided BotId/BotLocale combination is not valid. \
                                    Please update the BotId/BotLocale combination and retry \
                                    your request.";

    let emulator = Emulator::new();
    let bot = shake_bot(
        &emulator,
        "AddressedShakeBot",
        &Fulfilment {
            closing: Some(closing_setting(None)),
            code_hook: None,
        },
    )
    .await;

    // (case, botId, botAliasId, localeId, the message the refusal carries) —
    // the alias ids are well formed and simply name no alias of the bot, and the
    // locale rows address a locale the bot has no built model for.
    let rows: Vec<(&str, String, String, String, String)> = vec![
        (
            "an alias id of the right shape that is not an alias of the bot",
            bot.bot_id.clone(),
            "XXXXXXXXXX".to_string(),
            bot.locale_id.clone(),
            format!(
                "{}/XXXXXXXXXX is not a valid BotId/BotAliasId combination.",
                bot.bot_id
            ),
        ),
        (
            "another alias id the bot does not have",
            bot.bot_id.clone(),
            "A1b2C3".to_string(),
            bot.locale_id.clone(),
            format!(
                "{}/A1b2C3 is not a valid BotId/BotAliasId combination.",
                bot.bot_id
            ),
        ),
        (
            "a bot id nothing was ever created for",
            "ZZZZZZZZZZ".to_string(),
            bot.bot_alias_id.clone(),
            bot.locale_id.clone(),
            format!(
                "ZZZZZZZZZZ/{} is not a valid BotId/BotAliasId combination.",
                bot.bot_alias_id
            ),
        ),
        (
            "the bot's own alias, for a locale it has none of",
            bot.bot_id.clone(),
            bot.bot_alias_id.clone(),
            "de_DE".to_string(),
            NOT_VALID_LOCALE.to_string(),
        ),
        (
            "the bot's own alias, for another locale it has none of",
            bot.bot_id.clone(),
            bot.bot_alias_id.clone(),
            "ja_JP".to_string(),
            NOT_VALID_LOCALE.to_string(),
        ),
    ];

    for (case, bot_id, bot_alias_id, locale_id, expected) in &rows {
        let session = format!(
            "/bots/{bot_id}/botAliases/{bot_alias_id}/botLocales/{locale_id}/sessions/refused"
        );

        let by_text = emulator
            .send(
                "POST",
                &format!("{session}/text"),
                Some(json!({"text": "order a shake"})),
            )
            .await;
        let by_utterance = emulator
            .send_raw(
                "POST",
                &format!("{session}/utterance"),
                Bytes::from_static(b"order a shake"),
                TEXT_REQUEST_CONTENT_TYPE,
                &[],
            )
            .await;

        for (operation, refused) in [
            ("RecognizeText", &by_text),
            ("RecognizeUtterance", &by_utterance),
        ] {
            assert_eq!(
                refused.status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {} {:?}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_type(),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: the refusal names the modelled error"
            );
            assert_eq!(
                at(&refused.json(), "/message").as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {operation}: the refusal is worded for the condition it hit, \
                 got {:?}",
                refused.json()
            );
        }
    }
}

/// Rule 4: a text `responseContentType` suppresses speech synthesis, so the
/// `audioStream` body is empty whatever the bot had to say — over the whitespace
/// and case variants of the media type, and the default a request that names
/// none gets.
#[tokio::test]
async fn a_text_response_carries_no_audio_stream() {
    let emulator = Emulator::new();
    let bot = shake_bot(
        &emulator,
        "SpokenShakeBot",
        &Fulfilment {
            closing: Some(closing_setting(None)),
            code_hook: None,
        },
    )
    .await;

    // (case, the responseContentType the request names, the contentType the
    // response reports back)
    let rows: [(&str, Option<&str>, &str); 4] = [
        ("no responseContentType at all", None, TEXT_CONTENT_TYPE),
        (
            "the media type as the model writes it",
            Some(TEXT_CONTENT_TYPE),
            TEXT_CONTENT_TYPE,
        ),
        (
            "the same media type with a space before the parameter",
            Some("text/plain; charset=utf-8"),
            "text/plain; charset=utf-8",
        ),
        (
            "the same media type in upper case",
            Some("TEXT/PLAIN; CHARSET=UTF-8"),
            "TEXT/PLAIN; CHARSET=UTF-8",
        ),
    ];

    for (index, (case, requested, reported)) in rows.iter().enumerate() {
        let session = format!("spoken-{index}");
        let headers: Vec<(&str, &str)> = requested
            .map(|value| vec![("Response-Content-Type", value)])
            .unwrap_or_default();
        let reply = emulator
            .send_raw(
                "POST",
                &format!("{}/utterance", bot.session(&session)),
                Bytes::from_static(b"i would like a strawberry shake"),
                TEXT_REQUEST_CONTENT_TYPE,
                &headers,
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: RecognizeUtterance must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.body.is_empty(),
            "case {case:?}: asking for text must suppress speech synthesis, got {} bytes ({:?})",
            reply.body.len(),
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.header("content-type").as_deref(),
            Some(*reported),
            "case {case:?}: the response reports the content type that was asked for"
        );
        // The bot's message is not lost: it travels in the messages header, in
        // full, which is where a client reads it.
        assert_eq!(
            reply.lex_header("x-amz-lex-messages"),
            Some(json!([{"content": CLOSING_RESPONSE, "contentType": "PlainText"}])),
            "case {case:?}: the closing response travels in the messages header"
        );
        // And the turn itself is the same turn the JSON-bodied sibling takes.
        let by_text = say(
            &emulator,
            &bot,
            &format!("spoken-text-{index}"),
            "i would like a strawberry shake",
        )
        .await;
        assert_eq!(
            reply
                .lex_header("x-amz-lex-session-state")
                .and_then(|state| state.pointer("/intent").cloned()),
            by_text.pointer("/sessionState/intent").cloned(),
            "case {case:?}: the turn agrees with the same turn through RecognizeText"
        );
    }
}
