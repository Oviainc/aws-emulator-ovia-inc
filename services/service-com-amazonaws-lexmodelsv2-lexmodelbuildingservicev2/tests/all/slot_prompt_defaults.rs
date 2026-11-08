// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the server-side defaults Amazon Lex V2 materializes onto a
//! `PromptSpecification` when it stores one.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the intent helpers are the other suites', reused rather than copied.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: a prompt
//! specification is stored with the service's defaults filled in — the message selection
//! strategy, and one advanced per-attempt entry for every attempt the prompt has (the
//! initial attempt plus one per retry) — and every operation that reports a resource
//! holding one reports what was stored, while any member the request did supply is
//! reported exactly as it supplied it.
//!
//! The rows differ in the number of retries the prompt allows (every value the modelled
//! `maxRetries` range admits, plus one past each bound), in which of the two defaulted
//! members the request supplies, and in which resource holds the prompt — a slot, a
//! constituent sub slot of a composite slot, and an intent's confirmation prompt.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, text};
use crate::intents_and_builds::{create_intent, intent_body, intent_uri, intents_uri};
use crate::slot_type::host_locale;
use crate::slots::{listed_slot_names, slot_uri, slots_uri};

/// Every attempt of a prompt, in the order the attempts happen. These are the members of
/// the modelled `PromptAttempt` enum, which are the keys of
/// `PromptAttemptsSpecificationMap`
/// (`smithy_query(query="shape",shape="PromptAttempt")`).
const PROMPT_ATTEMPTS: [&str; 6] = ["Initial", "Retry1", "Retry2", "Retry3", "Retry4", "Retry5"];

/// The strategy a stored prompt that named none is reported with.
const DEFAULT_STRATEGY: &str = "Random";

/// The advanced settings the service materializes for one attempt of a prompt nobody
/// customized: both input types allowed, the user free to interrupt, and the default
/// audio, DTMF and text timeouts. Written out here rather than read from the crate, so
/// the suite pins the document a client sees.
fn default_attempt() -> Value {
    json!({
        "allowInterrupt": true,
        "allowedInputTypes": { "allowAudioInput": true, "allowDTMFInput": true },
        "audioAndDTMFInputSpecification": {
            "startTimeoutMs": 4000,
            "audioSpecification": { "maxLengthMs": 15000, "endTimeoutMs": 640 },
            "dtmfSpecification": {
                "maxLength": 513,
                "endTimeoutMs": 5000,
                "deletionCharacter": "*",
                "endCharacter": "#",
            },
        },
        "textInputSpecification": { "startTimeoutMs": 30000 },
    })
}

/// The `promptAttemptsSpecification` a prompt allowing `max_retries` retries is stored
/// with: one default entry per attempt, which is the initial attempt plus one per retry.
fn expected_attempts(max_retries: usize) -> Value {
    let mut map = Map::new();
    for attempt in PROMPT_ATTEMPTS.iter().take(max_retries + 1) {
        map.insert((*attempt).to_string(), default_attempt());
    }
    Value::Object(map)
}

/// A prompt specification carrying the two members the model requires, plus `extra`.
fn prompt(max_retries: i64, extra: Value) -> Value {
    let mut body = json!({
        "messageGroups": [{ "message": { "plainTextMessage": { "value": "Which size?" } } }],
        "maxRetries": max_retries,
    });
    let object = body.as_object_mut().expect("the prompt is a JSON object");
    for (key, value) in extra.as_object().expect("extras are a JSON object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// A `CreateSlot` body eliciting a value with `prompt_specification`.
fn slot_with_prompt(name: &str, prompt_specification: Value) -> Value {
    json!({
        "slotName": name,
        "valueElicitationSetting": {
            "slotConstraint": "Required",
            "promptSpecification": prompt_specification,
        },
    })
}

/// The `promptSpecification` of a reported slot's `valueElicitationSetting`.
fn reported_prompt(slot: &Value) -> Value {
    member(&member(slot, "valueElicitationSetting"), "promptSpecification")
}

/// Create a slot and return the response body, asserting the create was accepted.
async fn create_slot(
    router: &MultiServiceRouter,
    bot_id: &str,
    intent_id: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, created) = call(
        router,
        "PUT",
        &slots_uri(bot_id, "DRAFT", "en_US", intent_id),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateSlot to answer 200, got {status} {created:?}"
    );
    created
}

/// Read one slot back and return the response body, asserting the read was served.
async fn describe_slot(
    router: &MultiServiceRouter,
    bot_id: &str,
    intent_id: &str,
    slot_id: &str,
    case: &str,
) -> Value {
    let (status, described) = call(
        router,
        "GET",
        &slot_uri(bot_id, "DRAFT", "en_US", intent_id, slot_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeSlot to answer 200, got {status} {described:?}"
    );
    described
}

/// The `valueElicitationPromptSpecification` `ListSlots` reports for one slot of an
/// intent.
async fn listed_prompt(
    router: &MultiServiceRouter,
    bot_id: &str,
    intent_id: &str,
    slot_name: &str,
) -> Value {
    let (status, listed) = call(
        router,
        "POST",
        &slots_uri(bot_id, "DRAFT", "en_US", intent_id),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected ListSlots to answer 200, got {status} {listed:?}"
    );
    member(&listed, "slotSummaries")
        .as_array()
        .expect("ListSlots reports a slotSummaries list")
        .iter()
        .find(|entry| text(entry, "slotName") == slot_name)
        .map(|entry| member(entry, "valueElicitationPromptSpecification"))
        .unwrap_or(Value::Null)
}

/// A host bot with one `en_US` locale holding one custom intent, and that intent's id.
async fn host_intent(router: &MultiServiceRouter, bot_name: &str) -> (String, String) {
    let bot_id = host_locale(router, bot_name, "en_US").await;
    let intent_id = create_intent(
        router,
        &bot_id,
        "en_US",
        &intent_body(
            "OrderPizza",
            json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
        ),
    )
    .await;
    (bot_id, intent_id)
}

#[tokio::test]
async fn a_stored_prompt_carries_one_default_attempt_per_attempt_it_has() {
    let router = emulator();
    let (bot_id, intent_id) = host_intent(&router, "PromptDefaultsBot").await;

    // One row per value the modelled `maxRetries` range admits (0..5 —
    // `smithy_query(query="shape",shape="PromptSpecification")`), since the number of
    // attempt entries follows it.
    for max_retries in 0..=5usize {
        let case = format!("maxRetries {max_retries}");
        let slot_name = format!("Size{max_retries}");
        let created = create_slot(
            &router,
            &bot_id,
            &intent_id,
            slot_with_prompt(&slot_name, prompt(max_retries as i64, json!({}))),
            &case,
        )
        .await;
        let reported = reported_prompt(&created);
        assert_eq!(
            member(&reported, "messageSelectionStrategy"),
            json!(DEFAULT_STRATEGY),
            "case {case:?}: CreateSlot must report the default message selection \
             strategy, got {reported:?}"
        );
        assert_eq!(
            member(&reported, "promptAttemptsSpecification"),
            expected_attempts(max_retries),
            "case {case:?}: CreateSlot must report one default entry per attempt, got \
             {reported:?}"
        );
        // The members the request supplied are reported as supplied.
        assert_eq!(
            member(&reported, "maxRetries"),
            json!(max_retries),
            "case {case:?}: the stored prompt keeps the retries asked for, got {reported:?}"
        );

        // READ BACK — the defaults are part of the stored record, so every reporting
        // operation reports the same prompt.
        let slot_id = text(&created, "slotId");
        let described = describe_slot(&router, &bot_id, &intent_id, &slot_id, &case).await;
        assert_eq!(
            reported_prompt(&described),
            reported,
            "case {case:?}: DescribeSlot must report the prompt CreateSlot stored, got \
             {described:?}"
        );
        let listed = listed_prompt(&router, &bot_id, &intent_id, &slot_name).await;
        assert_eq!(
            listed, reported,
            "case {case:?}: ListSlots must report the prompt CreateSlot stored, got {listed:?}"
        );
    }

    // One past each published bound of `maxRetries` is refused by the modelled range, so
    // there is no prompt to default.
    for (case, max_retries) in [("one below the minimum", -1i64), ("one above the maximum", 6)] {
        let (status, refused) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(slot_with_prompt("OutOfRange", prompt(max_retries, json!({})))),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateSlot to refuse maxRetries {max_retries}, got \
             {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let listed = listed_slot_names(&router, &bot_id, "en_US", &intent_id).await;
        assert!(
            !listed.iter().any(|name| name == "OutOfRange"),
            "case {case:?}: a refused create must store no slot, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_prompt_keeps_every_defaulted_member_the_request_supplied() {
    let router = emulator();
    let (bot_id, intent_id) = host_intent(&router, "PromptSuppliedBot").await;

    // A per-attempt document that is nothing like the service's defaults, so a row that
    // supplies it cannot be confused with one that was defaulted.
    let custom_attempt = json!({
        "allowInterrupt": false,
        "allowedInputTypes": { "allowAudioInput": false, "allowDTMFInput": true },
        "audioAndDTMFInputSpecification": {
            "startTimeoutMs": 1000,
            "dtmfSpecification": {
                "maxLength": 4,
                "endTimeoutMs": 2000,
                "deletionCharacter": "*",
                "endCharacter": "#",
            },
        },
        "textInputSpecification": { "startTimeoutMs": 5000 },
    });

    // Each row: what the request supplies beside the required members, and the strategy
    // and attempt map the stored prompt must then carry.
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the other strategy the enum admits, supplied",
            json!({ "messageSelectionStrategy": "Ordered" }),
            json!("Ordered"),
            expected_attempts(2),
        ),
        (
            "the service's own strategy, supplied explicitly",
            json!({ "messageSelectionStrategy": "Random" }),
            json!("Random"),
            expected_attempts(2),
        ),
        (
            "one customized attempt, fewer than the prompt has",
            json!({ "promptAttemptsSpecification": { "Initial": custom_attempt.clone() } }),
            json!(DEFAULT_STRATEGY),
            json!({ "Initial": custom_attempt.clone() }),
        ),
        (
            "both members supplied",
            json!({
                "messageSelectionStrategy": "Ordered",
                "promptAttemptsSpecification": {
                    "Initial": custom_attempt.clone(),
                    "Retry1": default_attempt(),
                },
            }),
            json!("Ordered"),
            json!({ "Initial": custom_attempt.clone(), "Retry1": default_attempt() }),
        ),
    ];

    for (index, (case, supplied, strategy, attempts)) in rows.into_iter().enumerate() {
        let slot_name = format!("Supplied{index}");
        let created = create_slot(
            &router,
            &bot_id,
            &intent_id,
            slot_with_prompt(&slot_name, prompt(2, supplied)),
            case,
        )
        .await;
        let reported = reported_prompt(&created);
        assert_eq!(
            member(&reported, "messageSelectionStrategy"),
            strategy,
            "case {case:?}: CreateSlot must report the strategy the request settled, got \
             {reported:?}"
        );
        assert_eq!(
            member(&reported, "promptAttemptsSpecification"),
            attempts,
            "case {case:?}: CreateSlot must report the attempts the request settled, got \
             {reported:?}"
        );

        let slot_id = text(&created, "slotId");
        let described = describe_slot(&router, &bot_id, &intent_id, &slot_id, case).await;
        assert_eq!(
            reported_prompt(&described),
            reported,
            "case {case:?}: DescribeSlot must report the prompt CreateSlot stored, got \
             {described:?}"
        );
    }
}

#[tokio::test]
async fn every_resource_that_stores_a_prompt_carries_the_defaults() {
    let router = emulator();
    let (bot_id, intent_id) = host_intent(&router, "PromptEverywhereBot").await;

    // A composite slot's constituent sub slots each elicit their value with a prompt
    // specification of their own
    // (`smithy_query(query="shape",shape="SubSlotValueElicitationSetting")`), stored by
    // the same call that stores the slot.
    let case = "a constituent sub slot of a composite slot";
    let mut body = slot_with_prompt("Pizza", prompt(1, json!({})));
    body.as_object_mut()
        .expect("the body is a JSON object")
        .insert(
            "subSlotSetting".to_string(),
            json!({
                "expression": "Crust AND Toppings",
                "slotSpecifications": {
                    "Crust": {
                        "slotTypeId": "AMAZON.AlphaNumeric",
                        "valueElicitationSetting": {
                            "promptSpecification": prompt(0, json!({})),
                        },
                    },
                    "Toppings": {
                        "slotTypeId": "AMAZON.AlphaNumeric",
                        "valueElicitationSetting": {
                            "promptSpecification": prompt(3, json!({ "allowInterrupt": false })),
                        },
                    },
                },
            }),
        );
    let created = create_slot(&router, &bot_id, &intent_id, body, case).await;
    let slot_id = text(&created, "slotId");
    let described = describe_slot(&router, &bot_id, &intent_id, &slot_id, case).await;
    for reader in [&created, &described] {
        let specifications = member(&member(reader, "subSlotSetting"), "slotSpecifications");
        for (sub_slot, retries) in [("Crust", 0usize), ("Toppings", 3)] {
            let sub_prompt = member(
                &member(&member(&specifications, sub_slot), "valueElicitationSetting"),
                "promptSpecification",
            );
            assert_eq!(
                member(&sub_prompt, "messageSelectionStrategy"),
                json!(DEFAULT_STRATEGY),
                "case {case:?}: sub slot {sub_slot} must carry the default strategy, got \
                 {sub_prompt:?}"
            );
            assert_eq!(
                member(&sub_prompt, "promptAttemptsSpecification"),
                expected_attempts(retries),
                "case {case:?}: sub slot {sub_slot} must carry one default entry per \
                 attempt, got {sub_prompt:?}"
            );
        }
    }

    // An intent's confirmation prompt is a prompt specification the same service stores,
    // through the create and the update that write it
    // (`smithy_query(query="shape",shape="IntentConfirmationSetting")`).
    let confirmation = |retries: i64| {
        json!({
            "intentConfirmationSetting": {
                "promptSpecification": prompt(retries, json!({})),
                "declinationResponse": {
                    "messageGroups": [{
                        "message": { "plainTextMessage": { "value": "Cancelled." } },
                    }],
                },
            },
        })
    };
    let case = "an intent's confirmation prompt, created";
    let (status, confirmed) = call(
        &router,
        "PUT",
        &intents_uri(&bot_id, "DRAFT", "en_US"),
        Some(intent_body("ConfirmOrder", confirmation(1))),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateIntent to answer 200, got {status} {confirmed:?}"
    );
    let confirm_intent_id = text(&confirmed, "intentId");
    let confirmation_prompt = |intent: &Value| {
        member(
            &member(intent, "intentConfirmationSetting"),
            "promptSpecification",
        )
    };
    let prompt_document = confirmation_prompt(&confirmed);
    assert_eq!(
        member(&prompt_document, "messageSelectionStrategy"),
        json!(DEFAULT_STRATEGY),
        "case {case:?}: the confirmation prompt must carry the default strategy, got \
         {prompt_document:?}"
    );
    assert_eq!(
        member(&prompt_document, "promptAttemptsSpecification"),
        expected_attempts(1),
        "case {case:?}: the confirmation prompt must carry one default entry per attempt, \
         got {prompt_document:?}"
    );

    let case = "an intent's confirmation prompt, read back";
    let (status, read_back) = call(
        &router,
        "GET",
        &intent_uri(&bot_id, "DRAFT", "en_US", &confirm_intent_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeIntent to answer 200, got {status} {read_back:?}"
    );
    assert_eq!(
        confirmation_prompt(&read_back),
        confirmation_prompt(&confirmed),
        "case {case:?}: DescribeIntent must report the prompt CreateIntent stored, got \
         {read_back:?}"
    );

    let case = "an intent's confirmation prompt, replaced by an update";
    let (status, updated) = call(
        &router,
        "PUT",
        &intent_uri(&bot_id, "DRAFT", "en_US", &confirm_intent_id),
        Some(intent_body("ConfirmOrder", confirmation(4))),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateIntent to answer 200, got {status} {updated:?}"
    );
    let updated_prompt = confirmation_prompt(&updated);
    assert_eq!(
        member(&updated_prompt, "promptAttemptsSpecification"),
        expected_attempts(4),
        "case {case:?}: the replacement prompt must carry one default entry per attempt, \
         got {updated_prompt:?}"
    );
    let (status, read_back) = call(
        &router,
        "GET",
        &intent_uri(&bot_id, "DRAFT", "en_US", &confirm_intent_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeIntent to answer 200, got {status} {read_back:?}"
    );
    assert_eq!(
        confirmation_prompt(&read_back),
        updated_prompt,
        "case {case:?}: DescribeIntent must report the prompt UpdateIntent stored, got \
         {read_back:?}"
    );
}
