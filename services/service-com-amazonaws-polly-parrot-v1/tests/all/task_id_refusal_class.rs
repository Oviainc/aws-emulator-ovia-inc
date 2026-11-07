// SPDX-License-Identifier: CC0-1.0

//! Which answer `GetSpeechSynthesisTask` gives for each `TaskId` the model admits,
//! swept across the member's WHOLE pattern rather than over a handful of spellings.
//!
//! RULE: a read keyed by a service-generated identifier answers from the STORE, never
//! from the identifier's spelling — every value the member's pattern admits is looked
//! up, and the answer is the resource when a task is held under that value and the
//! modelled not-found error for the resource (`SynthesisTaskNotFoundException`, HTTP
//! 400, whose `message` is JSON null) when none is. The identifier error the operation
//! also binds (`InvalidTaskIdException`, HTTP 400) is not an answer the read invents for
//! a value the pattern admits, and a value the pattern does NOT admit never reaches the
//! read at all: the request boundary refuses it as a request-shape failure.
//!
//! Source of the class: `smithy_query(operation="GetSpeechSynthesisTask")` — `TaskId` is
//! `@httpLabel @required` with pattern `^[a-zA-Z0-9_-]{1,100}$`, and both client errors
//! the operation declares are HTTP 400, so only the code distinguishes them.
//!
//! `crate::task_id_validity` checks the same rule over named forms; this suite checks it
//! over EVERY length the pattern admits, over an identifier the service itself minted,
//! and as an "served exactly when held" invariant — so no length, character class or
//! near-UUID spelling can be pre-judged without a row failing here.

use std::time::SystemTime;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{get_task, message_slot, seed_task, stored_task};

/// The longest value the member's pattern admits (`{1,100}`).
const MAX_TASK_ID: usize = 100;

/// Every character class the member's pattern admits, in one string: lower-case and
/// upper-case letters, digits, and the two punctuation characters (`_` and `-`). The
/// sweep below walks it, so each generated identifier mixes the classes.
const PATTERN_CHARACTERS: &str = "aZ9_-qB0";

/// One pattern-admitted identifier of exactly `length` characters.
///
/// Deterministic (no fixture needs a seed printed) and injective in `length`: the
/// character the value starts at rotates with the length, so two rows never differ only
/// in how long they are.
fn swept_id(length: usize) -> String {
    let alphabet: Vec<char> = PATTERN_CHARACTERS.chars().collect();
    (0..length)
        .map(|index| alphabet[(index + length) % alphabet.len()])
        .collect()
}

/// RULE: every length the pattern admits reaches the store, so an identifier the service
/// holds no task under is the NOT-FOUND refusal whatever its length or spelling — the
/// canonical-UUID length is not special, and nothing here is refused as an invalid id.
#[tokio::test]
async fn every_length_the_pattern_admits_is_a_store_question() {
    let polly = Polly::new();
    for length in 1..=MAX_TASK_ID {
        let id = swept_id(length);
        let response = get_task(&polly, &id).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "SynthesisTaskNotFoundException".to_string()),
            "case length {length} ({id:?}): expected the not-found refusal, got body {:?}",
            json(&response)
        );
        // Amazon Polly carries no message on this code: the slot is present and null.
        let slot = message_slot(&response);
        assert_eq!(
            slot,
            Some(serde_json::Value::Null),
            "case length {length} ({id:?}): expected a null message member, got {slot:?}"
        );
    }
}

/// RULE (the invariant): across the same sweep, a swept identifier is served EXACTLY
/// when the store holds a task under it. One read answering everything, or one
/// pre-judging a spelling, breaks the equivalence in one direction or the other.
#[tokio::test]
async fn a_swept_identifier_is_served_exactly_when_the_store_holds_it() {
    let polly = Polly::new();
    // Every third length is held, so served and not-found rows interleave along the
    // sweep instead of splitting it into two blocks.
    let held = |length: usize| length % 3 == 1;
    for length in 1..=MAX_TASK_ID {
        if held(length) {
            let mut task = stored_task(&swept_id(length), SystemTime::now());
            task.task_status = "completed".to_string();
            seed_task(&polly, &task);
        }
    }

    for length in 1..=MAX_TASK_ID {
        let id = swept_id(length);
        let response = get_task(&polly, &id).await;
        if held(length) {
            assert_eq!(
                response.status(),
                200,
                "case length {length} ({id:?}): a held task must be served, got body {:?}",
                json(&response)
            );
            assert_eq!(
                json(&response)["SynthesisTask"]["TaskId"].as_str(),
                Some(id.as_str()),
                "case length {length} ({id:?}): the read reported another task, body {:?}",
                json(&response)
            );
        } else {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "SynthesisTaskNotFoundException".to_string()),
                "case length {length} ({id:?}): expected the not-found refusal, got body {:?}",
                json(&response)
            );
        }
    }
}

/// RULE (end to end, through the minter): the identifier `StartSpeechSynthesisTask`
/// mints reads back, and identifiers derived from it that the store does NOT hold — each
/// still inside the member's pattern — are the not-found refusal, not an invalid-id
/// refusal. So the boundary between the two codes is "held" versus "not held", not "is
/// spelled like a minted identifier" versus "is not".
#[tokio::test]
async fn a_minted_identifier_is_served_and_its_unheld_neighbours_are_not() {
    let polly = Polly::new();
    let started = polly
        .call(
            "POST",
            "/v1/synthesisTasks",
            &serde_json::json!({
                "Text": "Hello from the emulator.",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "OutputS3BucketName": "polly-output",
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        started.status(),
        200,
        "the task under test must be creatable, body {:?}",
        json(&started)
    );
    let task_id = json(&started)["SynthesisTask"]["TaskId"]
        .as_str()
        .expect("a started task reports its minted TaskId")
        .to_string();

    let read = get_task(&polly, &task_id).await;
    assert_eq!(
        read.status(),
        200,
        "the minted identifier {task_id:?} must read back, body {:?}",
        json(&read)
    );
    assert_eq!(
        json(&read)["SynthesisTask"]["TaskId"].as_str(),
        Some(task_id.as_str()),
        "the read reported another task for {task_id:?}, body {:?}",
        json(&read)
    );

    // Each neighbour is pattern-admitted and differs from the minted identifier, so the
    // store holds no task under it.
    let neighbours = [
        ("the hyphens removed", task_id.replace('-', "")),
        ("a suffix appended", format!("{task_id}-1")),
        ("a prefix prepended", format!("x{task_id}")),
        ("the last character dropped", task_id[..task_id.len() - 1].to_string()),
        ("underscores for hyphens", task_id.replace('-', "_")),
    ];
    for (case, neighbour) in neighbours {
        assert_ne!(
            neighbour, task_id,
            "case {case:?}: the neighbour must differ from the held identifier"
        );
        let response = get_task(&polly, &neighbour).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "SynthesisTaskNotFoundException".to_string()),
            "case {case:?} ({neighbour:?}): expected the not-found refusal, got body {:?}",
            json(&response)
        );
    }

    // And the held task is still held: the neighbour reads above answered from the store
    // without disturbing it.
    let again = get_task(&polly, &task_id).await;
    assert_eq!(
        again.status(),
        200,
        "the minted identifier {task_id:?} must still be served, body {:?}",
        json(&again)
    );
}
