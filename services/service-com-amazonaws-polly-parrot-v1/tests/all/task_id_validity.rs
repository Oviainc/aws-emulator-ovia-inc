// SPDX-License-Identifier: CC0-1.0

//! Which of the two `TaskId` refusals `GetSpeechSynthesisTask` answers with.
//!
//! RULE: a read keyed by a service-minted identifier judges the identifier's FORM
//! before it looks anything up — a value the model's pattern admits but that is not of
//! the form the service mints is refused with the modelled invalid-identifier error for
//! that resource (`InvalidTaskIdException`, HTTP 400), and only an identifier of the
//! minted form is looked up, answering the modelled not-found error
//! (`SynthesisTaskNotFoundException`, HTTP 400) when it names nothing and the resource
//! when it does.
//!
//! Each test checks the rule over a TABLE (or over identifiers taken from the service's
//! own minter) rather than over the one request an issue quoted: the rows differ along
//! what the rule generalises over — the length, the character classes and the shape of
//! the value, and whether the store holds a task beside it.

use std::collections::BTreeSet;
use std::time::SystemTime;

use service_com_amazonaws_polly_parrot_v1::service::errors;
use service_com_amazonaws_polly_parrot_v1::state::mint_task_id;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, get_task, message_slot, seed_task, stored_task};

/// How many minted identifiers the not-found rows span. Bounded so the suite stays
/// quick; each identifier comes from the service's own minter, and a failure prints the
/// identifier it failed on.
const MINTED_CASES: usize = 32;

/// Values the model's `TaskId` pattern (`^[a-zA-Z0-9_-]{1,100}$`) admits that are NOT
/// identifiers this service could have minted. The rows span the pattern's lower and
/// upper length bounds, each character class it allows, and the near-misses of the
/// minted UUID form (the 32-digit spelling, a group of the wrong length, a non-hex
/// digit, hyphens in the wrong places).
fn non_identifiers() -> Vec<(&'static str, String)> {
    vec![
        ("a word", "notataskid".to_string()),
        ("the pattern's lower bound", "a".to_string()),
        ("the pattern's upper bound", "a".repeat(100)),
        ("digits only", "0123456789".to_string()),
        ("underscores and hyphens", "Task_1-2".to_string()),
        ("mixed case", "ZZ-_zz99".to_string()),
        ("a 36-character non-identifier", "a".repeat(36)),
        (
            "a uuid without its hyphens",
            "6f3a1b2c9d4e4f5a8b6c7d8e9f0a1b2c".to_string(),
        ),
        (
            "a uuid missing one digit",
            "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2".to_string(),
        ),
        (
            "a uuid with a non-hex digit",
            "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2z".to_string(),
        ),
        (
            "a uuid with its hyphens misplaced",
            "6f3a1b2c9-d4e-4f5a-8b6c-7d8e9f0a1b2c".to_string(),
        ),
        (
            "a uuid with a trailing suffix",
            "6f3a1b2c-9d4e-4f5a-8b6c-7d8e9f0a1b2c-1".to_string(),
        ),
    ]
}

/// RULE: a `TaskId` the model's pattern admits but which is not an identifier this
/// service mints is `InvalidTaskIdException` (HTTP 400), carrying the fixed message for
/// that code — never the not-found refusal, and whatever the store holds.
#[tokio::test]
async fn a_task_id_that_is_no_identifier_is_refused_as_invalid() {
    let polly = Polly::new();
    for store in ["an empty store", "a store holding a task"] {
        if store == "a store holding a task" {
            seed_task(
                &polly,
                &stored_task(&fixture_task_id("neighbour"), SystemTime::now()),
            );
        }
        for (case, value) in non_identifiers() {
            let case = format!("{case} ({value:?}) with {store}");
            let response = get_task(&polly, &value).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidTaskIdException".to_string()),
                "case {case:?}: expected the invalid-identifier refusal, got body {:?}",
                json(&response)
            );
            let slot = message_slot(&response);
            assert_eq!(
                slot.as_ref().and_then(serde_json::Value::as_str),
                Some(errors::INVALID_TASK_ID),
                "case {case:?}: expected the fixed message for the code, got {slot:?}"
            );
            // The sentence is a property of the code, so it never echoes the request.
            // Checked for the values distinctive enough for a substring test to mean
            // anything: a one-character value occurs in any English sentence.
            let message = slot
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if value.len() >= 4 {
                assert!(
                    !message.contains(value.as_str()),
                    "case {case:?}: message {message:?} echoes the request value {value:?}"
                );
            }
        }
    }
}

/// RULE (the other half): an identifier OF the minted form that names no task is
/// `SynthesisTaskNotFoundException` (HTTP 400) — the error about the store, not about
/// the identifier — for every identifier this service's minter produces.
#[tokio::test]
async fn a_minted_identifier_that_names_no_task_is_refused_as_not_found() {
    let polly = Polly::new();
    let mut identifiers: Vec<(String, String)> = (0..MINTED_CASES)
        .map(|index| (format!("minted identifier {index}"), mint_task_id()))
        .collect();
    // Two identifiers of the minted form that the minter is unlikely to produce: the
    // all-zero UUID, and one spelled with upper-case hex digits.
    identifiers.push((
        "the nil identifier".to_string(),
        "00000000-0000-0000-0000-000000000000".to_string(),
    ));
    identifiers.push((
        "upper-case hex digits".to_string(),
        "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C".to_string(),
    ));

    // The minter must not be answering with the same identifier twice, or the rows
    // below would all be one case.
    let unique: BTreeSet<&String> = identifiers.iter().map(|(_, id)| id).collect();
    assert_eq!(
        unique.len(),
        identifiers.len(),
        "the minted identifiers must all differ, got {identifiers:?}"
    );

    for (case, id) in &identifiers {
        let response = get_task(&polly, id).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "SynthesisTaskNotFoundException".to_string()),
            "case {case:?} ({id:?}): expected the not-found refusal, got body {:?}",
            json(&response)
        );
        // Amazon Polly answers this code with no message, rendered as JSON null.
        let slot = message_slot(&response);
        assert_eq!(
            slot,
            Some(serde_json::Value::Null),
            "case {case:?} ({id:?}): expected a null message member, got {slot:?}"
        );
    }
}

/// RULE (the served case): an identifier of the minted form that names a stored task is
/// answered with that task, while every other identifier beside it keeps the refusal
/// its own form earns — so the two refusals are a function of the identifier and the
/// store, not of one another.
#[tokio::test]
async fn a_minted_identifier_that_names_a_task_is_served() {
    let polly = Polly::new();
    let stored: Vec<String> = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(fixture_task_id)
        .collect();
    for id in &stored {
        let mut task = stored_task(id, SystemTime::now());
        task.task_status = "completed".to_string();
        seed_task(&polly, &task);
    }

    for id in &stored {
        let response = get_task(&polly, id).await;
        assert_eq!(
            response.status(),
            200,
            "task {id:?}: the stored task must read back, body {:?}",
            json(&response)
        );
        assert_eq!(
            json(&response)["SynthesisTask"]["TaskId"].as_str(),
            Some(id.as_str()),
            "task {id:?}: the read reported another task, body {:?}",
            json(&response)
        );
    }

    // Beside those three: another minted identifier is not found, and a value that is
    // no identifier at all is invalid. Both are 400, and the CODES differ — which is
    // what a client branching on the code reads.
    let unknown = get_task(&polly, &mint_task_id()).await;
    let invalid = get_task(&polly, "notataskid").await;
    assert_eq!(
        (
            unknown.status().as_u16(),
            error_code(&unknown),
            invalid.status().as_u16(),
            error_code(&invalid),
        ),
        (
            400,
            "SynthesisTaskNotFoundException".to_string(),
            400,
            "InvalidTaskIdException".to_string(),
        ),
        "the two refusals must stay distinct, got {:?} and {:?}",
        json(&unknown),
        json(&invalid)
    );
}
