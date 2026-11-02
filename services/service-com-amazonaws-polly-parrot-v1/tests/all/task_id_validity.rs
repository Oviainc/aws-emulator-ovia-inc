// SPDX-License-Identifier: CC0-1.0

//! Which refusal `GetSpeechSynthesisTask` answers for a `TaskId` it holds no task
//! under.
//!
//! RULE: a read keyed by a service-generated identifier does not narrow that identifier
//! beyond what the model says a request may carry — every value the member's pattern
//! admits reaches the store and is answered by the STORE: the resource when a task is
//! held under it, and the modelled not-found error for that resource
//! (`SynthesisTaskNotFoundException`, HTTP 400) when none is. A value the pattern does
//! not admit never reaches the handler at all: the request boundary refuses it as a
//! request-shape failure (`ValidationException`, HTTP 400).
//!
//! Each test checks the rule over a TABLE (or over identifiers taken from the service's
//! own minter) rather than over the one request an issue quoted: the rows differ along
//! what the rule generalises over — the length, the character classes and the shape of
//! the value, whether the store holds a task beside it, and which side of the modelled
//! pattern the value falls on.

use std::collections::BTreeSet;
use std::time::SystemTime;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, get_task, message_slot, seed_task, stored_task};
use service_com_amazonaws_polly_parrot_v1::state::mint_task_id;

/// How many minted identifiers the not-found rows span. Bounded so the suite stays
/// quick; each identifier comes from the service's own minter, and a failure prints the
/// identifier it failed on.
const MINTED_CASES: usize = 32;

/// Values the model's `TaskId` pattern (`^[a-zA-Z0-9_-]{1,100}$`) admits that are NOT
/// in the form this service mints an identifier in. The rows span the pattern's lower
/// and upper length bounds, each character class it allows, and the near-misses of the
/// minted UUID form (the 32-digit spelling, a group of the wrong length, a non-hex
/// digit, hyphens in the wrong places).
fn unminted_forms() -> Vec<(&'static str, String)> {
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

/// RULE: a `TaskId` the model's pattern admits that names no held task is the NOT-FOUND
/// refusal — whatever form the value takes, and whatever else the store holds. The
/// identifier's spelling is not what the answer turns on; the store is.
#[tokio::test]
async fn a_well_formed_task_id_that_names_no_task_is_refused_as_not_found() {
    let polly = Polly::new();
    for store in ["an empty store", "a store holding a task"] {
        if store == "a store holding a task" {
            seed_task(
                &polly,
                &stored_task(&fixture_task_id("neighbour"), SystemTime::now()),
            );
        }
        for (case, value) in unminted_forms() {
            let case = format!("{case} ({value:?}) with {store}");
            let response = get_task(&polly, &value).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "SynthesisTaskNotFoundException".to_string()),
                "case {case:?}: expected the not-found refusal, got body {:?}",
                json(&response)
            );
            // Amazon Polly answers this code with no message, rendered as JSON null —
            // so the document never echoes the identifier the caller sent either.
            let slot = message_slot(&response);
            assert_eq!(
                slot,
                Some(serde_json::Value::Null),
                "case {case:?}: expected a null message member, got {slot:?}"
            );
        }
    }
}

/// RULE (the same answer from the other end of the class): an identifier of the form
/// the service mints that names no task is the not-found refusal too, for every
/// identifier the minter produces.
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
    }
}

/// RULE (the served case): EVERY identifier the model's pattern admits is looked up, so
/// a task the store holds reads back whatever form its identifier takes — and the ones
/// beside it are still not found. The read is a function of the store, not of the
/// identifier's spelling.
#[tokio::test]
async fn any_well_formed_identifier_the_store_holds_is_served() {
    let polly = Polly::new();
    // Rows spanning the pattern: minted identifiers, and the unminted forms above — a
    // task held under one of those is still a held task.
    let mut stored: Vec<String> = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(fixture_task_id)
        .collect();
    stored.extend(
        unminted_forms()
            .into_iter()
            .map(|(_, value)| value)
            // The pattern's upper bound is 100 characters, which the rows above stay
            // within, so every one of them is a value a request can carry.
            .collect::<Vec<String>>(),
    );
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

    // Beside those: identifiers of both forms that the store does NOT hold are refused
    // as not found, so the served rows above are not passing on a read that answers
    // everything.
    for unknown in [mint_task_id(), "notheldatall".to_string()] {
        let response = get_task(&polly, &unknown).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "SynthesisTaskNotFoundException".to_string()),
            "unknown {unknown:?}: expected the not-found refusal, got body {:?}",
            json(&response)
        );
    }
}

/// RULE (the other side of the pattern): a `TaskId` the model's pattern does NOT admit
/// is refused at the request boundary as a request-shape failure, not by the store —
/// one past the length bound, and one per character class the pattern excludes.
#[tokio::test]
async fn a_task_id_outside_the_modeled_pattern_is_refused_at_the_boundary() {
    let polly = Polly::new();
    for (case, value) in [
        ("one past the upper length bound", "a".repeat(101)),
        ("many past the upper length bound", "a".repeat(400)),
        ("a punctuation character", "bad!id".to_string()),
        ("a dot", "bad.id".to_string()),
        ("a tilde", "bad~id".to_string()),
    ] {
        let response = get_task(&polly, &value).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "ValidationException".to_string()),
            "case {case:?} ({value:?}): a malformed id is a request-shape refusal, \
             got body {:?}",
            json(&response)
        );
    }
}
