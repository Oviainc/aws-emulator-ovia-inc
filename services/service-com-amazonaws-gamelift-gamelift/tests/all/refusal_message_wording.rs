// SPDX-License-Identifier: CC0-1.0

//! The WORDING of three refusals, over the wire: the `Message` member a client logs,
//! matches on and shows an operator.
//!
//! A refusal's message is the service's own text for that error class: its fixed part is
//! the same for every request that fails that way, and its variable parts are only the
//! ones the text itself has a slot for — a value the request supplied, a value the
//! service's own context holds, or a bound the member documents. Nothing of the
//! implementation that happened to detect the failure belongs in it, and no value is
//! appended where the text has no slot: a message that moves with the rejected value
//! cannot be matched on at all.
//!
//! Each suite below asserts that over a TABLE spanning its class — the forms a body can
//! be unreadable in, the values a bound can be missed by, the shapes an identifier can
//! take — and keeps a control row that is accepted, so the refusals are about the request
//! and not a blanket answer.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, …) is the one `hosting_loop`
//! drives the production dispatch path with.

use serde_json::json;

use crate::hosting_loop::{error_code, error_message, GameLift};

/// The text the service answers a rule set body it cannot read the rule language version
/// out of with, whatever the body is: fixed, with the position reached in the document
/// ("top level", nothing having been read) as its only variable part.
const VERSION_EXTRACTION_FAILURE: &str =
    "Encountered IOException attempting to extract version; location: top level";

/// The documented bounds of a queue's `TimeoutInSeconds`
/// (`smithy_query doc shape=CreateGameSessionQueueInput member=TimeoutInSeconds`: "The
/// minimum value is 10 and the maximum value is 600.").
const MIN_TIMEOUT: i64 = 10;
/// See [`MIN_TIMEOUT`].
const MAX_TIMEOUT: i64 = 600;

/// The sentence a `TimeoutInSeconds` outside those bounds is refused with: the member and
/// the two documented bounds, and no slot for the value that missed them.
const TIMEOUT_OUT_OF_BOUNDS: &str = "TimeoutInSeconds must be between 10 and 600 seconds";

/// A rule set body that satisfies every element the service requires, so the rows that
/// are refused below are refused for what they are missing and not for being a rule set.
fn conformant_body() -> String {
    json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// A body the service cannot read a rule set out of is refused with the version-extraction
/// text, the same string for every such body — on the create path and on the validate
/// path alike, since both read the one document the same way.
///
/// The rows span the ways a document can be unreadable: unparseable text (truncated,
/// unquoted, empty of content, with trailing matter) and parseable JSON that is not a
/// document with elements to read (a list, a string, a number, a boolean, a null). The
/// control rows are readable documents that are *not* rule sets, which name the element
/// they miss instead, and a conformant body, which is created.
#[tokio::test]
async fn an_unreadable_rule_set_body_reports_the_version_extraction_failure() {
    let gamelift = GameLift::new();

    let unreadable: Vec<(&str, String)> = vec![
        ("an object that is never closed", "{not valid json at all".to_string()),
        ("a bare opening brace", "{".to_string()),
        ("whitespace only", "   ".to_string()),
        ("a single word", "ruleset".to_string()),
        (
            "an object with an unquoted key",
            "{ruleLanguageVersion: \"1.0\"}".to_string(),
        ),
        (
            "an object with a trailing comma",
            "{\"ruleLanguageVersion\": \"1.0\",}".to_string(),
        ),
        (
            "a document followed by trailing matter",
            format!("{} trailing", conformant_body()),
        ),
        (
            "a list of rule set objects rather than one",
            json!([{"ruleLanguageVersion": "1.0", "teams": []}]).to_string(),
        ),
        ("an empty list", json!([]).to_string()),
        ("a bare string", json!("ruleLanguageVersion").to_string()),
        ("a bare number", json!(1.0).to_string()),
        ("a bare boolean", json!(true).to_string()),
        ("a bare null", json!(null).to_string()),
    ];

    for (index, (label, body)) in unreadable.iter().enumerate() {
        // The validate path reports on the body alone...
        let (status, parsed) = gamelift
            .call("ValidateMatchmakingRuleSet", &json!({"RuleSetBody": body}))
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?} on ValidateMatchmakingRuleSet: expected 400 \
             InvalidRequestException, got {status} {code} with body {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, VERSION_EXTRACTION_FAILURE,
            "case {label:?} on ValidateMatchmakingRuleSet: expected Message \
             {VERSION_EXTRACTION_FAILURE:?}, got {message:?}"
        );

        // ...and the create path reads the same document, so it refuses it the same way.
        let name = format!("unreadable-row-{index}");
        let (status, parsed) = gamelift
            .call(
                "CreateMatchmakingRuleSet",
                &json!({"Name": &name, "RuleSetBody": body}),
            )
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?} on CreateMatchmakingRuleSet: expected 400 \
             InvalidRequestException, got {status} {code} with body {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, VERSION_EXTRACTION_FAILURE,
            "case {label:?} on CreateMatchmakingRuleSet: expected Message \
             {VERSION_EXTRACTION_FAILURE:?}, got {message:?}"
        );
    }

    // A body that IS a readable document but is not a rule set is a different failure, so
    // it does NOT carry the extraction text — and still carries nothing of whatever read
    // it: no line, no column, no expected token.
    let readable_non_rule_sets: Vec<(&str, String)> = vec![
        (
            "a document with no teams",
            json!({"ruleLanguageVersion": "1.0"}).to_string(),
        ),
        (
            "a document with no ruleLanguageVersion",
            json!({"teams": [{"name": "solo", "maxPlayers": 2}]}).to_string(),
        ),
        (
            "a team with no name",
            json!({"ruleLanguageVersion": "1.0", "teams": [{"maxPlayers": 2}]}).to_string(),
        ),
    ];
    for (label, body) in &readable_non_rule_sets {
        let (status, parsed) = gamelift
            .call("ValidateMatchmakingRuleSet", &json!({"RuleSetBody": body}))
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: expected 400 InvalidRequestException, got {status} {code} with \
             body {parsed}"
        );
        let message = error_message(&parsed);
        assert_ne!(
            message, VERSION_EXTRACTION_FAILURE,
            "case {label:?}: a document the version WAS read out of must not report the \
             extraction failure, got {message:?}"
        );
        for leak in ["line ", "column ", "expected "] {
            assert!(
                !message.contains(leak),
                "case {label:?}: the message must carry nothing of whatever read the body, \
                 found {leak:?} in {message:?}"
            );
        }
    }

    // The control: a conformant body is created, so none of the above is a blanket refusal.
    let created = gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({"Name": "readable-rule-set", "RuleSetBody": conformant_body()}),
        )
        .await;
    assert_eq!(
        created["RuleSet"]["RuleSetName"],
        json!("readable-rule-set"),
        "a conformant body must be created, got {created}"
    );
}

/// A `TimeoutInSeconds` outside its documented bounds is refused with the one sentence
/// naming the member and those bounds — identical for every rejected value, which is what
/// lets a client match on it. Both published bounds are accepted, and one past each is
/// refused.
#[tokio::test]
async fn a_queue_timeout_outside_its_bounds_reports_the_documented_bounds() {
    let gamelift = GameLift::new();

    // Rows differ along how the value misses the range: below it (including the modelled
    // floor of `WholeNumber`, zero), one step either side of a published bound, and far
    // above it.
    let refused: Vec<(&str, i64)> = vec![
        ("zero, the modelled floor", 0),
        ("one second", 1),
        ("one below the documented minimum", MIN_TIMEOUT - 1),
        ("one above the documented maximum", MAX_TIMEOUT + 1),
        ("an hour", 3_600),
        ("the largest whole number a request can carry", i64::from(i32::MAX)),
    ];
    for (index, (label, timeout)) in refused.iter().enumerate() {
        let name = format!("out-of-bounds-{index}");
        let (status, parsed) = gamelift
            .call(
                "CreateGameSessionQueue",
                &json!({"Name": &name, "TimeoutInSeconds": timeout}),
            )
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: expected 400 InvalidRequestException, got {status} {code} with \
             body {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, TIMEOUT_OUT_OF_BOUNDS,
            "case {label:?}: expected Message {TIMEOUT_OUT_OF_BOUNDS:?}, got {message:?}"
        );

        // The refused create stored nothing, so the name is still free.
        let described = gamelift
            .ok("DescribeGameSessionQueues", json!({}))
            .await;
        let stored: Vec<&str> = described["GameSessionQueues"]
            .as_array()
            .map(|queues| {
                queues
                    .iter()
                    .map(|queue| queue["Name"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !stored.contains(&name.as_str()),
            "case {label:?}: a refused create must store nothing, found {name} in {described}"
        );
    }

    // Each published bound, and a value between them, is accepted and stored as supplied.
    for (index, timeout) in [MIN_TIMEOUT, MAX_TIMEOUT, 300].into_iter().enumerate() {
        let name = format!("in-bounds-{index}");
        let created = gamelift
            .ok(
                "CreateGameSessionQueue",
                json!({"Name": &name, "TimeoutInSeconds": timeout}),
            )
            .await;
        assert_eq!(
            created["GameSessionQueue"]["TimeoutInSeconds"],
            json!(timeout),
            "case timeout {timeout}: a value within the documented bounds must be stored as \
             supplied, got {created}"
        );
    }
}

/// `DescribeGameSessionPlacement` for an id no placement is stored under reports the
/// reservation it could not find and the account it looked in: fixed text with those two
/// slots, the id taken from the request verbatim.
///
/// The rows span the identifiers `IdStringModel` (len 1..48, `^[a-zA-Z0-9-]+$`) allows,
/// including both of its length bounds, and the control rows are the placement that IS
/// stored, which is still reported.
#[tokio::test]
async fn describing_an_unstored_placement_reports_the_reservation_and_account() {
    let gamelift = GameLift::new();
    gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({"Name": "wording-queue", "TimeoutInSeconds": 60}),
        )
        .await;
    let stored_id = "stored-placement";
    gamelift
        .ok(
            "StartGameSessionPlacement",
            json!({
                "PlacementId": stored_id,
                "GameSessionQueueName": "wording-queue",
                "MaximumPlayerSessionCount": 4,
            }),
        )
        .await;

    let account = engine::emulator_account_id();
    let unstored: Vec<(&str, String)> = vec![
        ("one character, the shortest id the model allows", "p".to_string()),
        ("a hyphenated id", "unknown-placement".to_string()),
        ("digits only", "20261029".to_string()),
        ("mixed case", "AbSeNtPlacement".to_string()),
        (
            "the stored id with one more character",
            format!("{stored_id}-2"),
        ),
        (
            "48 characters, the longest id the model allows",
            "p".repeat(48),
        ),
    ];

    for (label, placement_id) in &unstored {
        let (status, parsed) = gamelift
            .call(
                "DescribeGameSessionPlacement",
                &json!({"PlacementId": placement_id}),
            )
            .await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: expected 400 NotFoundException, got {status} {code} with body \
             {parsed}"
        );
        let expected = format!(
            "No GameSessionReservation found for reservation {placement_id} accountId {account}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, expected,
            "case {label:?}: expected Message {expected:?}, got {message:?}"
        );
    }

    // The control: the placement that IS stored is still reported, so the refusals above
    // are about the id the request named.
    let described = gamelift
        .ok(
            "DescribeGameSessionPlacement",
            json!({"PlacementId": stored_id}),
        )
        .await;
    assert_eq!(
        described["GameSessionPlacement"]["PlacementId"],
        json!(stored_id),
        "the stored placement must survive the refused reads, got {described}"
    );
}
