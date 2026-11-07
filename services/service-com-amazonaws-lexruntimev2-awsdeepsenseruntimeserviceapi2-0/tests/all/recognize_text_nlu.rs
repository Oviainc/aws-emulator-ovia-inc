// SPDX-License-Identifier: CC0-1.0

//! What the Lex V2 runtime makes of an utterance: the intent it recognises, the
//! slot values it resolves, the dialog it drives, and the refusal a locale with
//! no built model earns.
//!
//! Every bot here is AUTHORED the way a client authors one — `CreateBot`,
//! `CreateBotLocale`, `CreateSlotType`, `CreateIntent`, `CreateSlot`,
//! `UpdateIntent`, `BuildBotLocale` over the Lex Models V2 control plane mounted
//! beside this runtime on the same engine — and then talked to over
//! `RecognizeText`, so what is asserted is the whole path from the bot a client
//! built to the turn it gets back, through the wire on both halves.
//!
//! The rules under test, each asserted over a TABLE of utterances rather than
//! one recorded conversation:
//!
//! 1. An utterance is interpreted against the sample utterances the built
//!    locale's intents declare: the best match at or above the locale's own
//!    `nluIntentConfidenceThreshold` is the top interpretation, and
//!    `FallbackIntent` is the answer only when nothing of the locale matches.
//! 2. The words a sample utterance puts in a slot's position — and the whole of
//!    an utterance answering a slot elicitation — are resolved against the slot
//!    type's declared values under its `resolutionStrategy`: `TopResolution`
//!    maps a synonym onto the canonical sample value and resolves nothing for a
//!    word the slot type does not declare, while the default strategy keeps what
//!    the user said.
//! 3. While a `Required` slot of the intent has no value the turn answers
//!    `ElicitSlot` for it, with the prompt authored on the slot and the intent
//!    `InProgress`; once every required slot has one the intent is
//!    `ReadyForFulfillment` and the dialog closes. What a turn filled is session
//!    state, so `GetSession` reports it back.
//! 4. A turn addressed to a bot, alias or locale with no built model is refused
//!    with the modelled `ResourceNotFoundException`, and starts no session.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::Emulator;

/// An IAM role ARN satisfying the modelled `RoleArn` shape
/// (len 32..2048, pattern `^arn:aws:iam::[0-9]{12}:role/.*$`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/lex-runtime-suite";

/// The prompt authored on the slot every conversation here elicits.
pub(crate) const SIZE_PROMPT: &str = "What size pizza would you like?";

/// The intent every conversation here is about, and its slot.
pub(crate) const ORDER_PIZZA: &str = "OrderPizza";
/// See [`ORDER_PIZZA`].
pub(crate) const PIZZA_SIZE: &str = "PizzaSize";

/// The sample utterances authored on [`ORDER_PIZZA`]: one that names no slot
/// value and one that names the slot in the position its value is said in.
const UTTERANCES: [&str; 2] = ["I want to order a pizza", "I want a {PizzaSize} pizza"];

/// The locale the conversations are held in, and its own confidence threshold:
/// inside the modelled `ConfidenceThreshold` range 0..1.
const LOCALE: &str = "en_US";
/// See [`LOCALE`].
pub(crate) const THRESHOLD: f64 = 0.4;

/// `TopResolution` and the two other members of the modelled
/// `SlotValueResolutionStrategy` enum
/// (`smithy_query(query="shape",shape="SlotValueResolutionStrategy")`:
/// OriginalValue | TopResolution | Concatenation).
const TOP_RESOLUTION: &str = "TopResolution";
/// See [`TOP_RESOLUTION`].
const ORIGINAL_VALUE: &str = "OriginalValue";

/// The bot a conversation is held with, as the control plane minted it.
pub(crate) struct Bot {
    pub(crate) bot_id: String,
    pub(crate) bot_alias_id: String,
    pub(crate) locale_id: String,
}

impl Bot {
    /// The session address of one conversation with this bot.
    pub(crate) fn session(&self, session_id: &str) -> String {
        format!(
            "/bots/{}/botAliases/{}/botLocales/{}/sessions/{session_id}",
            self.bot_id, self.bot_alias_id, self.locale_id
        )
    }
}

/// Dispatch one control-plane call and assert the status it answered with.
pub(crate) async fn control(
    emulator: &Emulator,
    method: &str,
    uri: &str,
    body: Value,
    expected: u16,
    what: &str,
) -> Value {
    let reply = emulator.send(method, uri, Some(body)).await;
    assert_eq!(
        reply.status,
        expected,
        "{what}: expected HTTP {expected}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()
}

/// A response member as a string, for the ids the control plane mints.
pub(crate) fn id(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the response must carry {member}, got {body:?}"))
        .to_string()
}

/// Author the pizza bot: one locale, one custom slot type resolving under
/// `strategy` (or under the control plane's default when it is `None`), one
/// intent carrying [`UTTERANCES`], and one `Required` slot of that type with
/// [`SIZE_PROMPT`] authored on it and a priority ordering it.
///
/// `built` decides whether `BuildBotLocale` is run and waited out: a locale is
/// servable only once it is `Built`. `closing` is the plain text of the intent's
/// `intentClosingSetting` closing response, or `None` for a bot that authors
/// none.
pub(crate) async fn pizza_bot(
    emulator: &Emulator,
    bot_name: &str,
    strategy: Option<&str>,
    built: bool,
    closing: Option<&str>,
) -> Bot {
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

    // The slot type's declared values, each with the synonyms that resolve to
    // it.
    let mut value_selection = json!({});
    if let Some(strategy) = strategy {
        value_selection = json!({"valueSelectionSetting": {"resolutionStrategy": strategy}});
    }
    let mut slot_type_body = json!({
        "slotTypeName": PIZZA_SIZE,
        "slotTypeValues": [
            {"sampleValue": {"value": "large"}, "synonyms": [{"value": "big"}]},
            {"sampleValue": {"value": "small"}, "synonyms": [{"value": "little"}]},
        ],
    });
    for (key, value) in value_selection
        .as_object()
        .expect("the selection setting is an object")
    {
        slot_type_body
            .as_object_mut()
            .expect("the body is an object")
            .insert(key.clone(), value.clone());
    }
    let slot_type = control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/slottypes"),
        slot_type_body,
        200,
        "CreateSlotType",
    )
    .await;
    let slot_type_id = id(&slot_type, "slotTypeId");

    let sample_utterances: Vec<Value> = UTTERANCES
        .iter()
        .map(|utterance| json!({"utterance": utterance}))
        .collect();
    // The statement Amazon Lex conveys "when the intent is successfully
    // fulfilled" (`IntentClosingSetting$closingResponse`), for the bots whose
    // author wrote one.
    let closing_setting = closing.map(|text| {
        json!({
            "closingResponse": {
                "messageGroups": [{"message": {"plainTextMessage": {"value": text}}}],
            },
        })
    });
    let mut create_intent = json!({
        "intentName": ORDER_PIZZA,
        "sampleUtterances": sample_utterances.clone(),
    });
    if let Some(setting) = closing_setting.clone() {
        create_intent["intentClosingSetting"] = setting;
    }
    let intent = control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents"),
        create_intent,
        200,
        "CreateIntent",
    )
    .await;
    let intent_id = id(&intent, "intentId");

    let slot = control(
        emulator,
        "PUT",
        &format!(
            "/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents/{intent_id}/slots"
        ),
        json!({
            "slotName": PIZZA_SIZE,
            "slotTypeId": slot_type_id,
            "valueElicitationSetting": {
                "slotConstraint": "Required",
                "promptSpecification": {
                    "messageGroups": [{"message": {"plainTextMessage": {"value": SIZE_PROMPT}}}],
                    "maxRetries": 2,
                },
            },
        }),
        200,
        "CreateSlot",
    )
    .await;
    let slot_id = id(&slot, "slotId");

    // The elicitation order of an intent's slots is its `slotPriorities`, which
    // a build requires every slot of the intent to appear in. An update replaces
    // the intent, so everything the create gave it goes with the priorities.
    let mut update_intent = json!({
        "intentName": ORDER_PIZZA,
        "sampleUtterances": sample_utterances,
        "slotPriorities": [{"priority": 1, "slotId": slot_id}],
    });
    if let Some(setting) = closing_setting {
        update_intent["intentClosingSetting"] = setting;
    }
    control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents/{intent_id}"),
        update_intent,
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

    if built {
        build(emulator, &bot_id).await;
    }

    Bot {
        bot_id,
        bot_alias_id,
        locale_id: LOCALE.to_string(),
    }
}

/// Build the locale and wait for the `Built` the clock owes, asserting
/// `DescribeBotLocale` reports it.
pub(crate) async fn build(emulator: &Emulator, bot_id: &str) {
    control(
        emulator,
        "POST",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}"),
        json!({}),
        202,
        "BuildBotLocale",
    )
    .await;
    let mut status = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let described = control(
            emulator,
            "GET",
            &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}"),
            json!({}),
            200,
            "DescribeBotLocale",
        )
        .await;
        status = id(&described, "botLocaleStatus");
        if status == "Built" {
            return;
        }
    }
    panic!("the locale must reach botLocaleStatus Built, last saw {status:?}");
}

/// One `RecognizeText` turn.
pub(crate) async fn say(emulator: &Emulator, bot: &Bot, session_id: &str, text: &str) -> Value {
    let path = format!("{}/text", bot.session(session_id));
    let reply = emulator.send("POST", &path, Some(json!({"text": text}))).await;
    assert_eq!(
        reply.status,
        200,
        "RecognizeText {text:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()
}

/// A string at a JSON pointer, or `None` when nothing is there.
fn at(body: &Value, pointer: &str) -> Option<String> {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `resolvedValues` of one slot of the session's intent — the list a client
/// reads a resolution off — or `None` when the slot holds no value.
fn resolved(body: &Value, slot: &str) -> Option<Vec<String>> {
    let values = body
        .pointer(&format!("/sessionState/intent/slots/{slot}/value/resolvedValues"))?
        .as_array()?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    Some(values)
}

/// The `PlainText` contents of a turn's `messages`, in order.
pub(crate) fn messages(body: &Value) -> Vec<String> {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| at(message, "/content"))
                .collect()
        })
        .unwrap_or_default()
}

/// What one turn must answer with.
#[derive(Debug, Default, Clone)]
struct Expected<'a> {
    case: &'a str,
    text: &'a str,
    /// The name of the top interpretation, which is the intent the session is
    /// then on.
    intent: &'a str,
    /// `sessionState.dialogAction.type`.
    dialog_action: &'a str,
    /// `sessionState.dialogAction.slotToElicit`.
    slot_to_elicit: Option<&'a str>,
    /// `sessionState.intent.state`.
    state: &'a str,
    /// The slot's `originalValue` / `interpretedValue` / `resolvedValues`, or
    /// `None` for a slot the turn left unfilled.
    original: Option<&'a str>,
    interpreted: Option<&'a str>,
    resolved: Option<&'a [&'a str]>,
    /// The messages the turn sends.
    messages: &'a [&'a str],
    /// Does the top interpretation carry a confidence at or above the locale's
    /// threshold?
    scored: bool,
}

/// Assert one turn against its row.
fn assert_turn(turn: &Value, expected: &Expected<'_>) {
    let case = expected.case;
    assert_eq!(
        at(turn, "/interpretations/0/intent/name").as_deref(),
        Some(expected.intent),
        "case {case:?}: the top interpretation must be the authored intent, got {turn:?}"
    );
    assert_eq!(
        at(turn, "/sessionState/intent/name").as_deref(),
        Some(expected.intent),
        "case {case:?}: the session must be on that intent, got {turn:?}"
    );
    assert_eq!(
        at(turn, "/sessionState/dialogAction/type").as_deref(),
        Some(expected.dialog_action),
        "case {case:?}: the next dialog action, got {turn:?}"
    );
    assert_eq!(
        at(turn, "/sessionState/dialogAction/slotToElicit").as_deref(),
        expected.slot_to_elicit,
        "case {case:?}: the slot being elicited, got {turn:?}"
    );
    assert_eq!(
        at(turn, "/sessionState/intent/state").as_deref(),
        Some(expected.state),
        "case {case:?}: the intent's state, got {turn:?}"
    );
    assert_eq!(
        at(
            turn,
            &format!("/sessionState/intent/slots/{PIZZA_SIZE}/value/originalValue")
        )
        .as_deref(),
        expected.original,
        "case {case:?}: the slot's originalValue, got {turn:?}"
    );
    assert_eq!(
        at(
            turn,
            &format!("/sessionState/intent/slots/{PIZZA_SIZE}/value/interpretedValue")
        )
        .as_deref(),
        expected.interpreted,
        "case {case:?}: the slot's interpretedValue, got {turn:?}"
    );
    let actual_resolved = resolved(turn, PIZZA_SIZE);
    assert_eq!(
        actual_resolved.as_deref().map(<[String]>::to_vec),
        expected
            .resolved
            .map(|values| values.iter().map(|value| (*value).to_string()).collect()),
        "case {case:?}: the slot's resolvedValues, got {actual_resolved:?} in {turn:?}"
    );
    let actual_messages = messages(turn);
    assert_eq!(
        actual_messages,
        expected
            .messages
            .iter()
            .map(|message| (*message).to_string())
            .collect::<Vec<_>>(),
        "case {case:?}: the messages the turn sends, got {actual_messages:?}"
    );
    let score = turn
        .pointer("/interpretations/0/nluConfidence/score")
        .and_then(Value::as_f64);
    if expected.scored {
        let score = score.unwrap_or_else(|| {
            panic!("case {case:?}: a recognised intent carries a confidence, got {turn:?}")
        });
        assert!(
            (THRESHOLD..=1.0).contains(&score),
            "case {case:?}: the confidence must be in [{THRESHOLD}, 1.00], got {score}"
        );
    } else {
        assert_eq!(
            score, None,
            "case {case:?}: nothing of the locale matched, so no confidence is reported, \
             got {turn:?}"
        );
    }
}

/// Rules 1, 2 and 3 on a single-turn utterance: what the locale makes of a whole
/// sentence, over a table spanning the sample utterances it declares, the
/// declared values and synonyms of its slot type, the case and punctuation the
/// words are said with, a value the slot type declares nowhere, and a sentence
/// the locale has nothing for.
#[tokio::test]
async fn an_utterance_is_interpreted_against_the_built_locale() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "SingleTurnPizzaBot", Some(TOP_RESOLUTION), true, None).await;

    let large: &[&str] = &["large"];
    let small: &[&str] = &["small"];
    let prompt: &[&str] = &[SIZE_PROMPT];
    let rows: Vec<Expected<'_>> = vec![
        Expected {
            case: "a declared value in the slot's position",
            text: "I want a large pizza",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("large"),
            interpreted: Some("large"),
            resolved: Some(large),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "a synonym of a declared value",
            text: "I want a big pizza",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("big"),
            interpreted: Some("large"),
            resolved: Some(large),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "the other declared value",
            text: "I want a small pizza",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("small"),
            interpreted: Some("small"),
            resolved: Some(small),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "the other value's synonym",
            text: "I want a little pizza",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("little"),
            interpreted: Some("small"),
            resolved: Some(small),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "the same sentence shouted",
            text: "I WANT A LARGE PIZZA",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("LARGE"),
            interpreted: Some("large"),
            resolved: Some(large),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "the same sentence punctuated",
            text: "I want a large pizza!",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("large"),
            interpreted: Some("large"),
            resolved: Some(large),
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "a word the slot type declares nowhere",
            text: "I want a gigantic pizza",
            intent: ORDER_PIZZA,
            dialog_action: "ElicitSlot",
            slot_to_elicit: Some(PIZZA_SIZE),
            state: "InProgress",
            messages: prompt,
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "the sample utterance that names no value",
            text: "I want to order a pizza",
            intent: ORDER_PIZZA,
            dialog_action: "ElicitSlot",
            slot_to_elicit: Some(PIZZA_SIZE),
            state: "InProgress",
            messages: prompt,
            scored: true,
            ..Expected::default()
        },
        Expected {
            case: "a sentence the locale has no intent for",
            text: "what is the weather like tomorrow",
            intent: "FallbackIntent",
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            ..Expected::default()
        },
    ];

    for (index, expected) in rows.iter().enumerate() {
        // A fresh session per row, so each utterance is the opening turn of its
        // own conversation.
        let turn = say(&emulator, &bot, &format!("single-{index}"), expected.text).await;
        assert_turn(&turn, expected);
    }
}

/// Rules 2 and 3 over the turns of a conversation: the opening turn elicits the
/// required slot with its authored prompt, the answer to that elicitation is
/// resolved against the slot type, an answer that resolves to nothing leaves the
/// dialog eliciting the same slot, and what is filled is session state —
/// `GetSession` reports it back.
#[tokio::test]
async fn an_elicited_slot_is_filled_from_the_answer_and_read_back() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "MultiTurnPizzaBot", Some(TOP_RESOLUTION), true, None).await;

    let large: &[&str] = &["large"];
    let small: &[&str] = &["small"];
    let prompt: &[&str] = &[SIZE_PROMPT];
    // (case, the answer to the elicitation, what the answer must leave behind)
    let answers: Vec<Expected<'_>> = vec![
        Expected {
            case: "a declared value",
            text: "large",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("large"),
            interpreted: Some("large"),
            resolved: Some(large),
            ..Expected::default()
        },
        Expected {
            case: "a synonym",
            text: "big",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("big"),
            interpreted: Some("large"),
            resolved: Some(large),
            ..Expected::default()
        },
        Expected {
            case: "the other value's synonym, shouted",
            text: "LITTLE",
            intent: ORDER_PIZZA,
            dialog_action: "Close",
            state: "ReadyForFulfillment",
            original: Some("LITTLE"),
            interpreted: Some("small"),
            resolved: Some(small),
            ..Expected::default()
        },
        Expected {
            case: "a word the slot type declares nowhere",
            text: "gigantic",
            intent: ORDER_PIZZA,
            dialog_action: "ElicitSlot",
            slot_to_elicit: Some(PIZZA_SIZE),
            state: "InProgress",
            messages: prompt,
            ..Expected::default()
        },
    ];

    for (index, expected) in answers.iter().enumerate() {
        let session = format!("multi-{index}");

        // The opening turn: the intent is recognised, nothing is filled, and the
        // required slot is elicited with the prompt authored on it.
        let opening = say(&emulator, &bot, &session, UTTERANCES[0]).await;
        assert_turn(
            &opening,
            &Expected {
                case: "the opening turn",
                text: UTTERANCES[0],
                intent: ORDER_PIZZA,
                dialog_action: "ElicitSlot",
                slot_to_elicit: Some(PIZZA_SIZE),
                state: "InProgress",
                messages: prompt,
                scored: true,
                ..Expected::default()
            },
        );

        // The answer, which names no intent of its own.
        let answered = say(&emulator, &bot, &session, expected.text).await;
        assert_turn(&answered, expected);

        // What the turn filled is session state.
        let read = emulator.send("GET", &bot.session(&session), None).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the conversation must be readable, got {} {:?}",
            expected.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let stored = read.json();
        assert_eq!(
            resolved(&stored, PIZZA_SIZE),
            resolved(&answered, PIZZA_SIZE),
            "case {:?}: GetSession must report the slot the turn filled, got {stored:?}",
            expected.case
        );
        assert_eq!(
            at(&stored, "/sessionState/dialogAction/type"),
            at(&answered, "/sessionState/dialogAction/type"),
            "case {:?}: and the dialog the turn left it in, got {stored:?}",
            expected.case
        );

        // A conversation still eliciting the slot takes an answer that does
        // resolve, and closes.
        if expected.dialog_action == "ElicitSlot" {
            let retried = say(&emulator, &bot, &session, "big").await;
            assert_turn(
                &retried,
                &Expected {
                    case: "the retry after an unresolvable answer",
                    text: "big",
                    intent: ORDER_PIZZA,
                    dialog_action: "Close",
                    state: "ReadyForFulfillment",
                    original: Some("big"),
                    interpreted: Some("large"),
                    resolved: Some(large),
                    ..Expected::default()
                },
            );
        }
    }
}

/// One row of the resolution-strategy table: the case, the strategy authored on
/// the slot type (or none at all), the answer said for the slot, and the
/// `interpretedValue` and `resolvedValues` it must resolve to.
type StrategyRow<'a> = (
    &'a str,
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<Vec<&'a str>>,
);

/// Rule 2 over the strategies a slot type resolves under: the same answer, on
/// the same declared values, resolves differently under `TopResolution` than
/// under the strategy a create that names no `valueSelectionSetting` gets.
#[tokio::test]
async fn the_resolution_strategy_decides_what_an_answer_resolves_to() {
    let emulator = Emulator::new();

    // (case, the authored strategy, the answer, interpretedValue, resolvedValues)
    let rows: Vec<StrategyRow<'_>> = vec![
        (
            "TopResolution maps a synonym onto the canonical value",
            Some(TOP_RESOLUTION),
            "big",
            Some("large"),
            Some(vec!["large"]),
        ),
        (
            "TopResolution resolves a declared value to itself",
            Some(TOP_RESOLUTION),
            "small",
            Some("small"),
            Some(vec!["small"]),
        ),
        (
            "TopResolution resolves nothing outside the declared values",
            Some(TOP_RESOLUTION),
            "gigantic",
            None,
            None,
        ),
        (
            "OriginalValue keeps the word and reports the resolution beside it",
            Some(ORIGINAL_VALUE),
            "big",
            Some("big"),
            Some(vec!["large"]),
        ),
        (
            "OriginalValue keeps a word outside the declared values",
            Some(ORIGINAL_VALUE),
            "gigantic",
            Some("gigantic"),
            None,
        ),
        (
            "a slot type with no selection setting resolves as OriginalValue does",
            None,
            "gigantic",
            Some("gigantic"),
            None,
        ),
    ];

    for (index, (case, strategy, answer, interpreted, resolved_values)) in rows.iter().enumerate() {
        let bot =
            pizza_bot(&emulator, &format!("StrategyBot{index}"), *strategy, true, None).await;
        let session = format!("strategy-{index}");
        say(&emulator, &bot, &session, UTTERANCES[0]).await;
        let answered = say(&emulator, &bot, &session, answer).await;

        assert_eq!(
            at(
                &answered,
                &format!("/sessionState/intent/slots/{PIZZA_SIZE}/value/interpretedValue")
            )
            .as_deref(),
            *interpreted,
            "case {case:?}: the interpretedValue of {answer:?}, got {answered:?}"
        );
        let actual = resolved(&answered, PIZZA_SIZE);
        assert_eq!(
            actual,
            resolved_values
                .as_ref()
                .map(|values| values.iter().map(|value| (*value).to_string()).collect()),
            "case {case:?}: the resolvedValues of {answer:?}, got {actual:?}"
        );
        // A slot nothing resolved for is still being elicited; one that resolved
        // completes the intent.
        let expected_action = if interpreted.is_some() {
            "Close"
        } else {
            "ElicitSlot"
        };
        assert_eq!(
            at(&answered, "/sessionState/dialogAction/type").as_deref(),
            Some(expected_action),
            "case {case:?}: the dialog after {answer:?}, got {answered:?}"
        );
    }
}

/// Rule 4: a turn is refused until the locale it addresses has a built model,
/// the refusal leaves no session behind, and the same turn is served once the
/// locale is built.
#[tokio::test]
async fn a_turn_addressed_to_a_locale_with_no_built_model_is_refused() {
    /// What Amazon Lex V2 says when the bot and locale a turn addresses have no
    /// model it can serve. Fixed text: it names neither the bot nor the locale.
    const NOT_VALID: &str = "The provided BotId/BotLocale combination is not valid. Please \
                             update the BotId/BotLocale combination and retry your request.";

    let emulator = Emulator::new();
    // Authored but NOT built: a locale leaves `Creating` for `NotBuilt`, the
    // status AWS documents for one that "was saved but not built".
    let bot = pizza_bot(&emulator, "UnbuiltPizzaBot", Some(TOP_RESOLUTION), false, None).await;
    let locale_uri = format!(
        "/bots/{}/botversions/DRAFT/botlocales/{}",
        bot.bot_id, bot.locale_id
    );
    let mut described = Value::Null;
    for _ in 0..50 {
        described = control(
            &emulator,
            "GET",
            &locale_uri,
            json!({}),
            200,
            "DescribeBotLocale",
        )
        .await;
        if id(&described, "botLocaleStatus") == "NotBuilt" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        id(&described, "botLocaleStatus"),
        "NotBuilt",
        "the locale under test must be one that was never built, got {described:?}"
    );

    // (case, the labels the turn addresses) — one row per way the addressed
    // combination can have no built model.
    let rows: Vec<(&str, String)> = vec![
        (
            "the bot's own locale, never built",
            format!(
                "/bots/{}/botAliases/{}/botLocales/{}/sessions/unbuilt",
                bot.bot_id, bot.bot_alias_id, bot.locale_id
            ),
        ),
        (
            "a bot id nothing was ever created for",
            format!(
                "/bots/ZZZZZZZZZZ/botAliases/{}/botLocales/{}/sessions/unbuilt",
                bot.bot_alias_id, bot.locale_id
            ),
        ),
        (
            "an alias id the bot does not have",
            format!(
                "/bots/{}/botAliases/NOSUCHALIAS/botLocales/{}/sessions/unbuilt",
                bot.bot_id, bot.locale_id
            ),
        ),
        (
            "a locale id the bot has no locale for",
            format!(
                "/bots/{}/botAliases/{}/botLocales/de_DE/sessions/unbuilt",
                bot.bot_id, bot.bot_alias_id
            ),
        ),
    ];

    for (case, path) in &rows {
        let refused = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({"text": "I want a large pizza"})),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_type(),
            "ResourceNotFoundException",
            "case {case:?}: the refusal names the modelled error"
        );
        assert_eq!(
            at(&refused.json(), "/message").as_deref(),
            Some(NOT_VALID),
            "case {case:?}: the refusal is worded as the service words it, got {:?}",
            refused.json()
        );

        // The refusal has no effect: no session came into existence.
        let read = emulator.send("GET", path, None).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused turn must start no session, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            at(&read.json(), "/message").as_deref(),
            Some("The session for user unbuilt doesn\u{2019}t exist or has expired."),
            "case {case:?}: the absent session is worded as the service words it, got {:?}",
            read.json()
        );
    }

    // Built, and the very same turn is served.
    build(&emulator, &bot.bot_id).await;
    let turn = say(&emulator, &bot, "unbuilt", "I want a large pizza").await;
    assert_eq!(
        at(&turn, "/interpretations/0/intent/name").as_deref(),
        Some(ORDER_PIZZA),
        "once the locale is built the turn is interpreted against it, got {turn:?}"
    );
}
