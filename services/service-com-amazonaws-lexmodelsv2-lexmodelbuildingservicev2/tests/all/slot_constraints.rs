// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rules `CreateSlot` enforces on the request before it
//! stores a slot, over tables of inputs rather than one case each.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator serves
//! on its socket, so the restJson1 routing, the modelled constraints and the error
//! envelopes are exercised exactly as a client sees them. The harness, the bot/locale
//! setup and the intent and slot helpers are the other suites', reused rather than
//! copied.
//!
//! The rules under test:
//!
//! * a slot returning multiple values is accepted only in the one locale the
//!   `multipleValuesSetting` member documents it for; in every other supported locale the
//!   request is refused as invalid and nothing is stored, while the same slot without
//!   multiple values is accepted in all of them;
//! * an intent holds no more slots than the per-intent quota: a create is accepted up to
//!   it and refused with the modelled quota error past it, and the bound is per intent;
//! * a member outside a published bound or the modelled pattern is refused and stores
//!   nothing, while the bounds themselves are accepted.

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, text};
use crate::bot_locale::{host_bot, locale_body, locales_uri};
use crate::intents_and_builds::{create_intent, intent_body};
use crate::slot_type::{host_locale, name_of_length};
use crate::slots::{slot_body, slots_uri};

/// A `CreateIntent` body for an intent a slot can hang off.
fn utterance_intent(name: &str) -> Value {
    intent_body(
        name,
        json!({ "sampleUtterances": [{ "utterance": "I want to order a pizza" }] }),
    )
}

/// How many slots `ListSlots` reports for one intent of one locale.
async fn slot_count(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    intent_id: &str,
) -> usize {
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
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_multi_value_slot_is_accepted_only_in_the_locale_the_member_documents() {
    let router = emulator();
    let bot_id = host_bot(&router, "MultiValueSlotBot").await;

    // Every locale this service supports, so the rule is asserted over the whole catalog
    // rather than one locale the issue happened to name. The catalog is
    // `state::SUPPORTED_LOCALES`, and the restricted locale is the one the
    // `multipleValuesSetting` member names.
    for (locale_id, _) in state::SUPPORTED_LOCALES {
        let (status, created) = call(
            &router,
            "PUT",
            &locales_uri(&bot_id, "DRAFT"),
            Some(locale_body(locale_id, 0.4, json!({}))),
        )
        .await;
        assert_eq!(
            status, 202,
            "locale {locale_id}: expected CreateBotLocale to answer 202, got {status} {created:?}"
        );
        let intent_id = create_intent(&router, &bot_id, locale_id, &utterance_intent("OrderPizza"))
            .await;

        // A slot that does NOT return multiple values is accepted wherever the intent is,
        // whether the setting is absent or explicitly off.
        let accepted: [(&str, Value); 2] = [
            ("the setting absent", json!({ "slotTypeId": "AMAZON.Number" })),
            (
                "the setting explicitly off",
                json!({
                    "slotTypeId": "AMAZON.Number",
                    "multipleValuesSetting": { "allowMultipleValues": false },
                }),
            ),
        ];
        for (index, (case, extra)) in accepted.iter().enumerate() {
            let (status, answered) = call(
                &router,
                "PUT",
                &slots_uri(&bot_id, "DRAFT", locale_id, &intent_id),
                Some(slot_body(&format!("Single{index}"), extra.clone())),
            )
            .await;
            assert_eq!(
                status, 200,
                "locale {locale_id} case {case:?}: expected CreateSlot to answer 200, got \
                 {status} {answered:?}"
            );
            assert_eq!(
                member(&answered, "multipleValuesSetting"),
                member(extra, "multipleValuesSetting"),
                "locale {locale_id} case {case:?}: CreateSlot must report the stored setting, \
                 got {answered:?}"
            );
        }

        let before = slot_count(&router, &bot_id, locale_id, &intent_id).await;
        let (status, answered) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", locale_id, &intent_id),
            Some(slot_body(
                "Toppings",
                json!({
                    "slotTypeId": "AMAZON.AlphaNumeric",
                    "multipleValuesSetting": { "allowMultipleValues": true },
                }),
            )),
        )
        .await;
        let after = slot_count(&router, &bot_id, locale_id, &intent_id).await;

        if locale_id == state::MULTIPLE_VALUES_LOCALE_ID {
            assert_eq!(
                status, 200,
                "locale {locale_id}: a multi-value slot must be accepted here, got {status} \
                 {answered:?}"
            );
            assert_eq!(
                member(&answered, "multipleValuesSetting"),
                json!({ "allowMultipleValues": true }),
                "locale {locale_id}: CreateSlot must report the stored setting, got {answered:?}"
            );
            assert_eq!(
                after,
                before + 1,
                "locale {locale_id}: the accepted slot must be stored, got {after} slots"
            );
        } else {
            assert_eq!(
                status, 400,
                "locale {locale_id}: a multi-value slot must be refused here, got {status} \
                 {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "ValidationException",
                "locale {locale_id}: expected ValidationException, got {answered:?}"
            );
            let message = text(&answered, "message");
            assert!(
                message.contains(state::MULTIPLE_VALUES_LOCALE_ID),
                "locale {locale_id}: the refusal must name the locale that allows it, got \
                 {message:?}"
            );
            assert_eq!(
                after, before,
                "locale {locale_id}: a refused create must store nothing, got {after} slots \
                 where {before} were held"
            );
        }
    }
}

#[tokio::test]
async fn an_intent_holds_no_more_slots_than_its_quota() {
    let router = emulator();
    let bot_id = host_locale(&router, "QuotaSlotBot", "en_US").await;
    let intent_id = create_intent(&router, &bot_id, "en_US", &utterance_intent("OrderPizza")).await;
    let sibling_id = create_intent(&router, &bot_id, "en_US", &utterance_intent("CancelOrder")).await;

    // Fill the intent to its quota, each slot under a name of its own: the published
    // bound is accepted.
    for index in 0..state::SLOTS_PER_INTENT_QUOTA {
        let (status, created) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(slot_body(
                &format!("Slot{index}"),
                json!({ "slotTypeId": "AMAZON.Number" }),
            )),
        )
        .await;
        assert_eq!(
            status, 200,
            "slot {index}: expected 200 up to the quota, got {status} {created:?}"
        );
    }

    // One past it is refused with the modelled quota error, and stores nothing.
    let (status, refused) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
        Some(slot_body(
            "OnePastTheQuota",
            json!({ "slotTypeId": "AMAZON.Number" }),
        )),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the quota must be refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException past the quota, got {refused:?}"
    );
    assert_eq!(
        slot_count(&router, &bot_id, "en_US", &intent_id).await,
        state::SLOTS_PER_INTENT_QUOTA,
        "a refused create must store nothing"
    );

    // The quota bounds one intent, so a sibling intent of the same locale still takes a
    // slot of its own.
    let (status, created) = call(
        &router,
        "PUT",
        &slots_uri(&bot_id, "DRAFT", "en_US", &sibling_id),
        Some(slot_body("Slot0", json!({ "slotTypeId": "AMAZON.Number" }))),
    )
    .await;
    assert_eq!(
        status, 200,
        "another intent of the same locale must still accept a slot, got {status} {created:?}"
    );
    assert_eq!(
        slot_count(&router, &bot_id, "en_US", &sibling_id).await,
        1,
        "the sibling intent must hold exactly the slot it was given"
    );
}

#[tokio::test]
async fn a_member_outside_its_published_bound_is_refused_and_stores_nothing() {
    let router = emulator();
    let bot_id = host_locale(&router, "ConstrainedSlotBot", "en_US").await;
    let intent_id = create_intent(&router, &bot_id, "en_US", &utterance_intent("OrderPizza")).await;

    // The bounds themselves, from the modelled shapes of `CreateSlot`'s members:
    // `slotName` is `Name` (len 1..100, pattern `^([0-9a-zA-Z][_-]?){1,100}$`) and
    // `description` is `Description` (len 0..2000).
    let accepted: Vec<(&str, Value)> = vec![
        (
            "a name of the minimum length",
            slot_body(&name_of_length(1), json!({})),
        ),
        (
            "a name of the maximum length",
            slot_body(&name_of_length(100), json!({})),
        ),
        (
            "an empty description, the minimum length",
            slot_body("EmptyDescription", json!({ "description": "" })),
        ),
        (
            "a description of the maximum length",
            slot_body(
                "LongDescription",
                json!({ "description": "d".repeat(2_000) }),
            ),
        ),
    ];
    for (case, request) in &accepted {
        let (status, answered) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value on the published bound must be accepted, got {status} \
             {answered:?}"
        );
    }
    let held = slot_count(&router, &bot_id, "en_US", &intent_id).await;
    assert_eq!(
        held,
        accepted.len(),
        "every accepted create must be stored, got {held} slots"
    );

    // One step outside each bound, plus a name the modelled pattern rejects and a slot
    // type reference longer than its own published maximum.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a name one character past the maximum",
            slot_body(&name_of_length(101), json!({})),
        ),
        (
            "an empty name, one character short of the minimum",
            slot_body("", json!({})),
        ),
        (
            "a name the modelled pattern rejects",
            slot_body("Pizza Size", json!({})),
        ),
        (
            "a description one character past the maximum",
            slot_body(
                "TooDescribed",
                json!({ "description": "d".repeat(2_001) }),
            ),
        ),
        (
            "a slot type reference past its published maximum",
            slot_body(
                "TooLongReference",
                json!({ "slotTypeId": "A".repeat(26) }),
            ),
        ),
    ];
    for (case, request) in &refused {
        let (status, response) = call(
            &router,
            "PUT",
            &slots_uri(&bot_id, "DRAFT", "en_US", &intent_id),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }
    let after = slot_count(&router, &bot_id, "en_US", &intent_id).await;
    assert_eq!(
        after, held,
        "a refused create must store nothing, got {after} slots where {held} were held"
    );
}
