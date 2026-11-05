// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT a `NotifyRecommendationsReceived` per-entry failure
//! carries, driven over HTTP through the same router the production emulator mounts.
//!
//! The rule under test: an operation that reports per-item failures INSIDE a successful
//! answer words every entry with the service's ONE fixed sentence for that failure class
//! — the sentence states neither a reason nor anything taken from the request, so it is
//! byte-for-byte the same for every entry of every call — and the item the entry is about
//! is named only in the entry's own identifier member
//! (`smithy_query(query="shape",shape="NotifyRecommendationsReceivedError")`:
//! `recommendationId` is "The identifier of the recommendation that is in error",
//! `message` is "A recommendation is causing an error").
//!
//! The rows differ along every axis the rule generalises over — how many ids the call
//! fails on (one, several, the model's `@length(0..25)` maximum), what FORM those ids
//! take (a uuid, an upper-case uuid, a bare token, an ARN-shaped label, a long opaque
//! string), whether the session's feed holds anything at all, where the failing ids sit
//! among ids the feed does hold, and whether the same id is named twice in one call — and
//! every row asserts the entry's whole message, not a substring.
//!
//! Only the recommendations a session holds are put there through the state seam: real
//! Amazon Q in Connect makes a recommendation available out of band, and no operation of
//! this model publishes one. Everything else goes over the wire, so a response that is
//! not computed from persisted state cannot pass. The harness is the one this surface is
//! already driven with (`idempotency_contents_and_refusal_fidelity.rs`).

use serde_json::{json, Value};

use super::idempotency_contents_and_refusal_fidelity::Emulator;

/// The one sentence this endpoint words a recommendation it could not acknowledge with,
/// as the recorded wire answers of the endpoint word it: fixed text that names no reason
/// and quotes nothing from the request.
const MESSAGE_RECOMMENDATION_NOT_PROCESSED: &str = "Could not process recommendation";

/// The most ids one call may name — the `@length(0..25)` of `RecommendationIdList`
/// (`smithy_query(query="shape",shape="RecommendationIdList")`), so the widest row is the
/// widest call the model admits.
const MAX_RECOMMENDATION_IDS: usize = 25;

/// The longest one id may be — the `@length(1..2048)` of that list's `RecommendationId`
/// member — so the longest row's id is the longest the model admits.
const MAX_RECOMMENDATION_ID_LENGTH: usize = 2048;

/// One row: the ids made available on the session's feed first, the ids the call names,
/// and what the answer must report in each of its two lists.
struct Case {
    case: &'static str,
    published: Vec<String>,
    requested: Vec<String>,
    acknowledged: Vec<String>,
    failed: Vec<String>,
}

/// A uuid-shaped id, so a row can name an id of the form this service mints without
/// minting one.
fn uuid(nibble: char) -> String {
    let run = |count: usize| std::iter::repeat_n(nibble, count).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        run(8),
        run(4),
        run(4),
        run(4),
        run(12)
    )
}

fn cases() -> Vec<Case> {
    let held: Vec<String> = ['1', '2', '3'].into_iter().map(uuid).collect();
    let absent = uuid('a');

    // Every id form the list's `RecommendationId` member admits: it constrains length
    // only, so the list is not restricted to uuids.
    let forms: Vec<String> = vec![
        uuid('b'),
        uuid('c').to_uppercase(),
        "recommendation-7".to_string(),
        format!(
            "arn:aws:wisdom:eu-central-1:000000000000:session/{}",
            uuid('d')
        ),
        "x".repeat(MAX_RECOMMENDATION_ID_LENGTH),
    ];

    vec![
        Case {
            case: "one id, on a session whose feed holds nothing",
            published: Vec::new(),
            requested: vec![absent.clone()],
            acknowledged: Vec::new(),
            failed: vec![absent.clone()],
        },
        Case {
            case: "ids of every form the list admits, none of them held",
            published: Vec::new(),
            requested: forms.clone(),
            acknowledged: Vec::new(),
            failed: forms.clone(),
        },
        Case {
            case: "one unheld id between two the feed holds",
            published: held.clone(),
            requested: vec![held[0].clone(), absent.clone(), held[1].clone()],
            acknowledged: vec![held[0].clone(), held[1].clone()],
            failed: vec![absent.clone()],
        },
        Case {
            case: "an unheld id first and last, around every id the feed holds",
            published: held.clone(),
            requested: vec![
                absent.clone(),
                held[0].clone(),
                held[1].clone(),
                held[2].clone(),
                forms[2].clone(),
            ],
            acknowledged: held.clone(),
            failed: vec![absent.clone(), forms[2].clone()],
        },
        Case {
            case: "the same id named twice: the second mention names nothing held",
            published: vec![held[0].clone()],
            requested: vec![held[0].clone(), held[0].clone()],
            acknowledged: vec![held[0].clone()],
            failed: vec![held[0].clone()],
        },
        Case {
            case: "an unheld id named twice: one entry per mention",
            published: Vec::new(),
            requested: vec![absent.clone(), absent.clone()],
            acknowledged: Vec::new(),
            failed: vec![absent.clone(), absent.clone()],
        },
        Case {
            case: "the model's maximum of 25 ids, none of them held",
            published: Vec::new(),
            requested: (0..MAX_RECOMMENDATION_IDS)
                .map(|index| format!("unheld-{index}-{absent}"))
                .collect(),
            acknowledged: Vec::new(),
            failed: (0..MAX_RECOMMENDATION_IDS)
                .map(|index| format!("unheld-{index}-{absent}"))
                .collect(),
        },
    ]
}

/// RULE: every per-entry failure of this operation carries the service's one fixed
/// sentence for the class, and names the recommendation it is about only in the entry's
/// own `recommendationId`.
#[tokio::test]
async fn every_unacknowledged_recommendation_carries_the_services_fixed_sentence() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("notify-message-assistant").await;
    // Every sentence this suite reads, so the last assertion can show the whole surface
    // words the class alike rather than per row.
    let mut sentences: Vec<String> = Vec::new();

    for (row, case) in cases().into_iter().enumerate() {
        let (session_id, _) = emulator
            .session(&assistant_id, &format!("notify-message-session-{row}"))
            .await;
        for recommendation_id in &case.published {
            emulator.publish(&assistant_id, &session_id, recommendation_id);
        }

        let label = case.case;
        let reply = emulator
            .send(
                "POST",
                &format!(
                    "/assistants/{assistant_id}/sessions/{session_id}/recommendations/notify"
                ),
                Some(json!({ "recommendationIds": case.requested })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: a per-entry failure is reported inside a success, so the \
             answer must be HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply
                .array("/recommendationIds")
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            case.acknowledged,
            "case {label:?}: expected the acknowledged ids {:?}, got {}",
            case.acknowledged,
            reply.body
        );

        let errors = reply.array("/errors");
        assert_eq!(
            errors.len(),
            case.failed.len(),
            "case {label:?}: expected {} per-entry error(s), got {}",
            case.failed.len(),
            reply.body
        );
        for (position, (entry, expected_id)) in errors.iter().zip(&case.failed).enumerate() {
            let reported_id = entry
                .pointer("/recommendationId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                reported_id, expected_id,
                "case {label:?}: entry {position} must name the recommendation it is \
                 about, expected {expected_id:?}, got {reported_id:?}"
            );
            let message = entry
                .pointer("/message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, MESSAGE_RECOMMENDATION_NOT_PROCESSED,
                "case {label:?}: entry {position} must carry the service's fixed sentence, \
                 expected {MESSAGE_RECOMMENDATION_NOT_PROCESSED:?}, got {message:?}"
            );
            assert!(
                !message.contains(expected_id),
                "case {label:?}: entry {position} must not quote the request's id in its \
                 message, got {message:?}"
            );
            sentences.push(message.to_string());
        }
    }

    sentences.sort();
    sentences.dedup();
    assert_eq!(
        sentences,
        vec![MESSAGE_RECOMMENDATION_NOT_PROCESSED.to_string()],
        "case \"every row\": the class has ONE sentence across every entry of every call, \
         got {sentences:?}"
    );
}
