// SPDX-License-Identifier: CC0-1.0

//! Addressing and seeding: which calls this runtime serves at all, and what a
//! seeded dialog state may say.
//!
//! The Lex V2 runtime has no bot of its own — "Amazon Lex V2 then interprets the
//! user input using the machine learning model that it build for the bot" — so
//! every test here creates the bot through the Lex Models V2 control plane
//! mounted on the same engine, exactly as a client would, and then talks to it.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A call that needs the bot's runtime model (`RecognizeText`,
//!    `RecognizeUtterance`, `PutSession`) resolves the `botId`/`botAliasId`
//!    PAIR and the addressed locale's build state first: an alias that is not
//!    an alias of that bot, and a locale with no built model, are refused with
//!    HTTP 404 `ResourceNotFoundException` in AWS's own words — and the refusal
//!    has no effect, so no session comes into existence.
//! 2. A `sessionState` seeded by `PutSession` names resources the resolved
//!    locale defines: an intent it does not declare, or an elicited slot that
//!    intent does not declare, is HTTP 400 `ValidationException` and stores
//!    nothing.
//! 3. The `messages` a `PutSession` reports are the ones the BOT authors for
//!    the dialog state it seeds when the request carries none of its own, and
//!    the caller's own messages when it does.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::harness::{
    Bot, Emulator, CLOSING_MESSAGE, CONFIRMATION_PROMPT, INTENT, SLOT, SLOT_PROMPT, TEST_ALIAS,
};

/// The text `requestContentType` `RecognizeUtterance` accepts.
const TEXT_REQUEST_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// The intent every locale is created with, which the control plane adds to a
/// locale as it creates it.
const FALLBACK_INTENT: &str = "FallbackIntent";

/// What the sentence for a locale that has no built model is: entirely fixed
/// text, so it is the same for every bot and every locale.
const LOCALE_NOT_VALID: &str = "The provided BotId/BotLocale combination is not valid. \
     Please update the BotId/BotLocale combination and retry your request.";

/// The three calls that need the bot's runtime model: `PutSession`, which seeds
/// a session's dialog state, and the two encodings of one turn. Each row of a
/// table below is applied to all three, so what is asserted is the addressing
/// rule they share rather than one operation's spelling of it.
enum Call {
    PutSession,
    RecognizeText,
    RecognizeUtterance,
}

impl Call {
    fn name(&self) -> &'static str {
        match self {
            Self::PutSession => "PutSession",
            Self::RecognizeText => "RecognizeText",
            Self::RecognizeUtterance => "RecognizeUtterance",
        }
    }

    /// Send this call against `path`, with a dialog state the fixture bot's
    /// locale defines, so nothing but the addressing can refuse it.
    async fn send(&self, emulator: &Emulator, path: &str) -> crate::harness::Reply {
        match self {
            Self::PutSession => {
                emulator
                    .send(
                        "POST",
                        path,
                        Some(json!({"sessionState": {"dialogAction": {"type": "Delegate"}}})),
                    )
                    .await
            }
            Self::RecognizeText => {
                emulator
                    .send(
                        "POST",
                        &format!("{path}/text"),
                        Some(json!({"text": "I want to order a pizza"})),
                    )
                    .await
            }
            Self::RecognizeUtterance => {
                emulator
                    .send_raw(
                        "POST",
                        &format!("{path}/utterance"),
                        Bytes::from_static(b"I want to order a pizza"),
                        TEXT_REQUEST_CONTENT_TYPE,
                        &[],
                    )
                    .await
            }
        }
    }
}

/// Every call that needs the runtime model.
fn calls() -> [Call; 3] {
    [Call::PutSession, Call::RecognizeText, Call::RecognizeUtterance]
}

/// The sentence AWS states for a `botId`/`botAliasId` pair that is not one of
/// its bot's: the two ids from the request, then fixed text.
fn alias_not_valid(bot_id: &str, bot_alias_id: &str) -> String {
    format!("{bot_id}/{bot_alias_id} is not a valid BotId/BotAliasId combination.")
}

/// The sentence AWS states for a session that is not there.
fn session_not_found(session_id: &str) -> String {
    format!("The session for user {session_id} doesn\u{2019}t exist or has expired.")
}

/// Rule 1, on the alias: the pair is resolved, so an alias id the bot does not
/// have — however well formed, and even when it is another bot's — is the
/// modelled 404, and the refused call leaves no session behind.
#[tokio::test]
async fn a_call_is_refused_unless_the_bot_alias_pair_resolves() {
    let emulator = Emulator::new();
    let bot = emulator.built_bot("AddressedBot", &["en_US"]).await;
    let other = emulator.built_bot("OtherBot", &["en_US"]).await;
    // An alias that IS an alias — of the other bot, not of this one.
    let other_alias = emulator.second_alias(&other, "OTHERALIAS");

    // (case, botId, botAliasId, resolves)
    //
    // The rows differ along what the pair can be wrong in: the alias id (a
    // well-formed id of no alias, an id that is an alias of another bot, the
    // test alias id spelled in another case) and the bot id (one no bot was
    // created under).
    let rows: Vec<(&str, String, String, bool)> = vec![
        (
            "the bot's own alias",
            bot.bot_id.clone(),
            bot.alias_id.clone(),
            true,
        ),
        (
            "a well-formed alias id the bot does not have",
            bot.bot_id.clone(),
            "XXXXXXXXXX".to_string(),
            false,
        ),
        (
            "an alias of another bot",
            bot.bot_id.clone(),
            other_alias.alias_id.clone(),
            false,
        ),
        (
            "the test alias id in lower case",
            bot.bot_id.clone(),
            bot.alias_id.to_lowercase(),
            false,
        ),
        (
            "a botId no bot was created under",
            "0123456789".to_string(),
            bot.alias_id.clone(),
            false,
        ),
        (
            "the other bot addressed through its own alias",
            other.bot_id.clone(),
            other_alias.alias_id.clone(),
            true,
        ),
    ];

    for (index, (case, bot_id, bot_alias_id, resolves)) in rows.iter().enumerate() {
        for call in calls() {
            let session_id = format!("alias-{index}-{}", call.name());
            let path = format!(
                "/bots/{bot_id}/botAliases/{bot_alias_id}/botLocales/{}/sessions/{session_id}",
                bot.locale()
            );
            let reply = call.send(&emulator, &path).await;
            if *resolves {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?} on {}: the pair resolves, so the call is served, got {} {:?}",
                    call.name(),
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                continue;
            }
            assert_eq!(
                reply.status,
                404,
                "case {case:?} on {}: expected HTTP 404, got {} {:?}",
                call.name(),
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_type().ends_with("ResourceNotFoundException"),
                "case {case:?} on {}: expected ResourceNotFoundException, got {:?}",
                call.name(),
                reply.error_type()
            );
            let message = reply.error_message();
            assert_eq!(
                message,
                alias_not_valid(bot_id, bot_alias_id),
                "case {case:?} on {}: expected the pair's own sentence, got {message:?}",
                call.name()
            );

            // The refusal had no effect: the same session id on the bot's REAL
            // alias is still absent.
            let read = emulator
                .send("GET", &bot.session_path(&session_id), None)
                .await;
            assert_eq!(
                read.status,
                404,
                "case {case:?} on {}: the refused call must create no session, got {} {:?}",
                call.name(),
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                read.error_message(),
                session_not_found(&session_id),
                "case {case:?} on {}: an absent session is stated as AWS states it",
                call.name()
            );
        }
    }
}

/// Rule 1, on the locale: a locale with no built model has no runtime model, so
/// the call is refused until the build settles — and the session the refused
/// call would have started does not exist, before or after the build.
#[tokio::test]
async fn a_call_is_refused_until_the_locale_is_built() {
    let emulator = Emulator::new();
    let bot_id = emulator.create_bot("UnbuiltBot").await;
    // Two locales: one that will be built, one left as created.
    for locale_id in ["en_US", "en_GB"] {
        emulator.create_locale(&bot_id, locale_id).await;
        emulator.create_intent_with_slot(&bot_id, locale_id).await;
    }
    emulator.settle_control_plane();
    for locale_id in ["en_US", "en_GB"] {
        let status = emulator.bot_locale_status(&bot_id, locale_id).await;
        assert_eq!(
            status, "NotBuilt",
            "locale {locale_id:?}: a created locale is not built yet, got {status:?}"
        );
    }

    let bot = Bot {
        bot_id: bot_id.clone(),
        // Every bot is created with the alias Amazon Lex keeps for testing it,
        // so that is the alias this one is addressed through.
        alias_id: TEST_ALIAS.to_string(),
        locales: vec!["en_US".to_string()],
    };

    // (case, localeId) — every row is a locale with no built model: one created
    // but never built, one the bot has no locale for at all, and one outside the
    // locales the service supports.
    let unbuilt: [(&str, &str); 3] = [
        ("a created locale that was never built", "en_US"),
        ("a locale the bot has no record for", "de_DE"),
        ("a locale the service does not support", "xx_XX"),
    ];

    for (case, locale_id) in unbuilt {
        for call in calls() {
            let session_id = format!("locale-{}", call.name());
            let path = format!(
                "/bots/{bot_id}/botAliases/{}/botLocales/{locale_id}/sessions/{session_id}",
                bot.alias_id
            );
            let reply = call.send(&emulator, &path).await;
            assert_eq!(
                reply.status,
                404,
                "case {case:?} on {}: expected HTTP 404, got {} {:?}",
                call.name(),
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert!(
                reply.error_type().ends_with("ResourceNotFoundException"),
                "case {case:?} on {}: expected ResourceNotFoundException, got {:?}",
                call.name(),
                reply.error_type()
            );
            let message = reply.error_message();
            assert_eq!(
                message,
                LOCALE_NOT_VALID,
                "case {case:?} on {}: expected the locale's own sentence, got {message:?}",
                call.name()
            );
        }
    }

    // Building the locale is what makes the call servable — and the sessions the
    // refused calls would have started are still absent afterwards.
    emulator.build_locale(&bot_id, "en_US").await;
    let status = emulator.bot_locale_status(&bot_id, "en_US").await;
    assert_eq!(
        status, "Built",
        "the submitted build must have settled, got {status:?}"
    );
    for call in calls() {
        let session_id = format!("locale-{}", call.name());
        let read = emulator
            .send("GET", &bot.session_path(&session_id), None)
            .await;
        assert_eq!(
            read.status,
            404,
            "on {}: the refused call must have created no session, got {} {:?}",
            call.name(),
            read.status,
            String::from_utf8_lossy(&read.body)
        );

        let served = call
            .send(&emulator, &bot.session_path(&format!("built-{}", call.name())))
            .await;
        assert_eq!(
            served.status,
            200,
            "on {}: the built locale serves the call, got {} {:?}",
            call.name(),
            served.status,
            String::from_utf8_lossy(&served.body)
        );
    }

    // The locale that was never built is still refused: building one locale
    // builds no other.
    let reply = Call::RecognizeText
        .send(
            &emulator,
            &format!(
                "/bots/{bot_id}/botAliases/{}/botLocales/en_GB/sessions/still-unbuilt",
                bot.alias_id
            ),
        )
        .await;
    assert_eq!(
        reply.status,
        404,
        "the locale that was not built is still refused, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// Rule 2: a seeded dialog state names resources the locale defines.
#[tokio::test]
async fn a_seeded_session_state_must_name_what_the_locale_defines() {
    let (emulator, bot) = Emulator::with_bot().await;

    // (case, sessionState, the refusal owed — None where the seed is stored)
    //
    // The rows differ along what the seed can name: an intent (one the locale
    // declares, the one it was created with, one it does not declare, one
    // differing only in case) and the slot an `ElicitSlot` names (one of that
    // intent, one of no intent), plus a seed that names neither.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "no intent named at all",
            json!({"dialogAction": {"type": "Delegate"}}),
            None,
        ),
        (
            "an intent the locale declares",
            json!({"intent": {"name": INTENT, "state": "InProgress"}}),
            None,
        ),
        (
            "the intent the locale was created with",
            json!({"intent": {"name": FALLBACK_INTENT}}),
            None,
        ),
        (
            "an intent the locale does not declare",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": {"name": "NoSuchIntent", "state": "InProgress"},
            }),
            Some("The intent is invalid."),
        ),
        (
            "an intent differing from a declared one only in case",
            json!({"intent": {"name": INTENT.to_lowercase()}}),
            Some("The intent is invalid."),
        ),
        (
            "eliciting a slot of that intent",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            None,
        ),
        (
            "eliciting a slot the intent does not declare",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": "Flavor"},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            Some("The slot is invalid."),
        ),
    ];

    for (index, (case, state, refusal)) in rows.iter().enumerate() {
        let session_id = format!("seed-{index}");
        let path = bot.session_path(&session_id);
        let reply = emulator
            .send("POST", &path, Some(json!({"sessionState": state})))
            .await;
        match refusal {
            None => {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?}: the seed names what the locale defines, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                assert_eq!(
                    emulator.send("GET", &path, None).await.status,
                    200,
                    "case {case:?}: the stored seed must be readable"
                );
            }
            Some(expected) => {
                assert_eq!(
                    reply.status,
                    400,
                    "case {case:?}: expected HTTP 400, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                assert!(
                    reply.error_type().ends_with("ValidationException"),
                    "case {case:?}: expected ValidationException, got {:?}",
                    reply.error_type()
                );
                let message = reply.error_message();
                assert_eq!(
                    message, *expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                // The refused seed stored nothing.
                let read = emulator.send("GET", &path, None).await;
                assert_eq!(
                    read.status,
                    404,
                    "case {case:?}: a refused seed must create no session, got {} {:?}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    read.error_message(),
                    session_not_found(&session_id),
                    "case {case:?}: an absent session is stated as AWS states it"
                );
            }
        }
    }
}

/// Rule 3: the messages of a seeded dialog state are the bot's own, and a
/// request's own messages win over them.
#[tokio::test]
async fn a_seeded_dialog_state_answers_with_the_bots_authored_messages() {
    let (emulator, bot) = Emulator::with_bot().await;

    // (case, sessionState, messages in the request, the contents owed)
    //
    // The rows span the dialog states a bot authors messages for — eliciting a
    // slot, confirming an intent, closing one — plus the states it authors none
    // for and the case where the request carries its own.
    let rows: Vec<(&str, Value, Option<Value>, Vec<&str>)> = vec![
        (
            "eliciting the intent's prompted slot",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": {"name": INTENT, "state": "InProgress"},
                "sessionAttributes": {"seededBy": "put-session"},
            }),
            None,
            vec![SLOT_PROMPT],
        ),
        (
            "confirming the intent",
            json!({
                "dialogAction": {"type": "ConfirmIntent"},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            None,
            vec![CONFIRMATION_PROMPT],
        ),
        (
            "closing the intent",
            json!({
                "dialogAction": {"type": "Close"},
                "intent": {"name": INTENT, "state": "ReadyForFulfillment"},
            }),
            None,
            vec![CLOSING_MESSAGE],
        ),
        (
            "delegating the next step, which the bot authors no message for",
            json!({
                "dialogAction": {"type": "Delegate"},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            None,
            vec![],
        ),
        (
            "eliciting an intent, with none named",
            json!({"dialogAction": {"type": "ElicitIntent"}}),
            None,
            vec![],
        ),
        (
            "the caller's own messages, which are what was last sent",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            Some(json!([
                {"content": "Which size?", "contentType": "PlainText"},
                {"content": "Small, medium or large?", "contentType": "PlainText"},
            ])),
            vec!["Which size?", "Small, medium or large?"],
        ),
        (
            "an empty message list, which says the caller sent none of its own",
            json!({
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": {"name": INTENT, "state": "InProgress"},
            }),
            Some(json!([])),
            vec![],
        ),
    ];

    for (index, (case, state, messages, expected)) in rows.iter().enumerate() {
        let path = bot.session_path(&format!("messages-{index}"));
        let mut body = json!({"sessionState": state});
        if let Some(messages) = messages {
            body["messages"] = messages.clone();
        }
        let put = emulator.send("POST", &path, Some(body)).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: PutSession must succeed, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let header = put.lex_header("x-amz-lex-messages");
        let actual = contents(header.as_ref());
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected the messages {expected:?}, got {actual:?}"
        );
        // Every message reported carries the type the bot authored it in.
        if let Some(reported) = header.as_ref().and_then(Value::as_array) {
            for message in reported {
                assert_eq!(
                    message.get("contentType").and_then(Value::as_str),
                    Some("PlainText"),
                    "case {case:?}: a plain-text message is reported as PlainText, \
                     got {message:?}"
                );
            }
        }

        // What was last sent to the user is session state: a read reports the
        // same messages.
        let stored = emulator.send("GET", &path, None).await.json();
        let read_back = contents(stored.get("messages"));
        assert_eq!(
            read_back, *expected,
            "case {case:?}: GetSession must report the same messages, got {stored:?}"
        );
    }
}

/// The `content` of each message of a `messages` list, in order.
fn contents(messages: Option<&Value>) -> Vec<&str> {
    messages
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|message| message.get("content").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}
