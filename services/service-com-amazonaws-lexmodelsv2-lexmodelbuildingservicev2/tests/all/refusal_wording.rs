// SPDX-License-Identifier: CC0-1.0

//! How this service words the boundary refusal of a request, and the one sentence it
//! states for a read of a resource it does not hold.
//!
//! Two rules, each asserted over a TABLE of inputs rather than one case:
//!
//! * a constraint refusal names the member and what it had to satisfy and states NO
//!   value — not the one submitted and not the `null` of an absent member — quotes a
//!   `@pattern` requirement as the model declares it, anchors included, and states a
//!   member's expression requirement before that member's other requirements;
//! * a read of a resource the service does not hold answers one fixed sentence, which
//!   names neither the kind of resource nor the identifier the request carried.
//!
//! The rows span the operations and the constraint classes of this service's model — a
//! numeric `range`, a string `@length` and `@pattern`, an absent required member, a value
//! outside a modelled enum and a member bound to the URI — so what is asserted is the
//! rule rather than one recorded message.

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;

use crate::bot_lifecycle::{call, create_body, emulator, error_code, member, text};
use crate::bot_locale::{host_bot, locale_body, locale_uri, locales_uri};

/// The envelope the framework states a constraint refusal in, which this service keeps.
fn envelope(count: usize) -> String {
    let plural = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {plural} detected: ")
}

/// The clause a refusal states for one member: no value half, the member path, and the
/// requirement.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

#[tokio::test]
async fn a_constraint_refusal_states_the_member_and_the_requirement_and_no_value() {
    let router = emulator();
    let bot_id = host_bot(&router, "RefusalWordingBot").await;

    // Each row is one request, the clauses the refusal must state, in order.
    let cases: Vec<(&str, &str, String, Value, Vec<String>)> = vec![
        (
            "a double above its modelled range maximum",
            "PUT",
            locales_uri(&bot_id, "DRAFT"),
            locale_body("en_US", 2.5, json!({})),
            vec![clause(
                "nluIntentConfidenceThreshold",
                "Member must have value less than or equal to 1",
            )],
        ),
        (
            "a double below its modelled range minimum",
            "PUT",
            locales_uri(&bot_id, "DRAFT"),
            locale_body("en_US", -1.0, json!({})),
            vec![clause(
                "nluIntentConfidenceThreshold",
                "Member must have value greater than or equal to 0",
            )],
        ),
        (
            "a required member the request left out",
            "PUT",
            locales_uri(&bot_id, "DRAFT"),
            json!({ "localeId": "en_US" }),
            vec![clause(
                "nluIntentConfidenceThreshold",
                "Member must not be null",
            )],
        ),
        (
            "a string breaking its pattern and its minimum length at once",
            "PUT",
            format!("{}/intents", locale_uri(&bot_id, "DRAFT", "en_US")),
            json!({ "intentName": "" }),
            vec![
                // The EXPRESSION requirement comes first, quoted as the model declares
                // it (`smithy_query(query="shape",shape="Name")`), then the length one.
                clause(
                    "intentName",
                    r"Member must satisfy regular expression pattern: ^([0-9a-zA-Z][_-]?){1,100}$",
                ),
                clause(
                    "intentName",
                    "Member must have length greater than or equal to 1",
                ),
            ],
        ),
        (
            "a string over its modelled maximum length",
            "PUT",
            "/bots".to_string(),
            // 101 characters that still satisfy the `Name` pattern (fifty `a-` groups
            // and a final `a`), so the LENGTH is the only requirement broken.
            create_body(&format!("{}a", "a-".repeat(50)), 300, json!({})),
            vec![clause(
                "botName",
                "Member must have length less than or equal to 100",
            )],
        ),
        (
            "a value outside a modelled enum",
            "PUT",
            locales_uri(&bot_id, "DRAFT"),
            locale_body(
                "en_US",
                0.4,
                json!({ "speechDetectionSensitivity": "Highest" }),
            ),
            vec![clause(
                "speechDetectionSensitivity",
                "Member must satisfy enum value set: [Default, HighNoiseTolerance, \
                 MaximumNoiseTolerance]",
            )],
        ),
        (
            "a member bound to the URI breaking its pattern and its maximum length",
            "GET",
            locale_uri(&bot_id, "not-a-version", "en_US"),
            Value::Null,
            vec![
                clause(
                    "botVersion",
                    r"Member must satisfy regular expression pattern: ^(DRAFT|[0-9]+)$",
                ),
                clause(
                    "botVersion",
                    "Member must have length less than or equal to 5",
                ),
            ],
        ),
    ];

    for (case, method, uri, body, clauses) in cases {
        let request = (body != Value::Null).then_some(body);
        let (status, refused) = call(&router, method, &uri, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        let message = text(&refused, "message");
        let expected = format!("{}{}", envelope(clauses.len()), clauses.join("; "));
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // No clause may carry a value half, whichever one the framework would have
        // written: the submitted bytes or the `null` of an absent member.
        assert!(
            !message.contains("Value '") && !message.contains("Value null"),
            "case {case:?}: a clause must state no value at all, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_read_of_a_resource_the_service_does_not_hold_states_one_sentence() {
    let router = emulator();
    let bot_id = host_bot(&router, "NotFoundWordingBot").await;
    let (status, created) = call(
        &router,
        "PUT",
        &locales_uri(&bot_id, "DRAFT"),
        Some(locale_body("en_US", 0.4, json!({}))),
    )
    .await;
    assert_eq!(status, 202, "the host locale must be created, got {created:?}");

    // One row per read this slice serves, each naming a resource the store holds nothing
    // under: the sentence is the same whatever the resource and whatever identified it.
    let reads: Vec<(&str, String)> = vec![
        ("a bot", "/bots/ABCDEFGHIJ".to_string()),
        (
            "a bot locale",
            locale_uri(&bot_id, "DRAFT", "nl_NL"),
        ),
        (
            "a slot type",
            format!(
                "{}/slottypes/ABCDEFGHIJ",
                locale_uri(&bot_id, "DRAFT", "en_US")
            ),
        ),
        (
            "an intent",
            format!(
                "{}/intents/ABCDEFGHIJ",
                locale_uri(&bot_id, "DRAFT", "en_US")
            ),
        ),
    ];

    for (case, uri) in reads {
        let (status, response) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
        let message = text(&response, "message");
        assert_eq!(
            message,
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's one not-found sentence, got {message:?}"
        );
        // The sentence has no variable part, so nothing the request carried reaches it.
        assert!(
            !message.contains("ABCDEFGHIJ") && !message.contains(&bot_id),
            "case {case:?}: the sentence names no identifier, got {message:?}"
        );
        assert_eq!(
            member(&response, "message"),
            Value::String(state::RESOURCE_NOT_FOUND_MESSAGE.to_string()),
            "case {case:?}: the message is stated on every one of these reads"
        );
    }
}
