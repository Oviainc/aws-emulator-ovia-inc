// SPDX-License-Identifier: CC0-1.0

//! Whether an error document CARRIES its `message` member at all.
//!
//! RULE: every error document this service answers with carries the error shape's
//! `message` member — a JSON string when the code carries a message, and JSON `null`
//! when it carries none — so the key is never simply absent and a client that
//! distinguishes "no message supplied" from "an empty message" reads the same thing
//! from the emulator as from Amazon Polly. (An SDK substitutes the empty string for an
//! absent key, which is why dropping it is visible: botocore reports
//! `Error.Message == ''` for an absent key and `None` for a key whose value is null.)
//!
//! The rule is checked over a TABLE of requests that spans the class it covers: every
//! error code these operations can answer rather than one, several requests per code,
//! each operation that can answer a given code, and the scaffold channel's own 501
//! beside the modelled failures. The one request the issue quoted — a well-formed
//! `TaskId` that names no task — is one row.
//!
//! The rule is about the ERROR document only: the last test here pins that a SUCCESS
//! body still omits the optional members it has no value for, which is what every AWS
//! JSON response body does.

use std::time::SystemTime;

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::service::errors;
use service_com_amazonaws_polly_parrot_v1::state::mint_task_id;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{
    fixture_task_id, get_task, message_slot, pls_with_replacement, seed_task, stored_task,
};

/// Assert one error document: the status and code it carries, that its `message`
/// member is PRESENT, and that the member is exactly what this service words that code
/// with — the string for a code with a sentence, JSON null for a code with none.
fn assert_message_member(
    case: &str,
    response: &http::Response<Bytes>,
    expected_status: u16,
    expected_code: &str,
) {
    assert_eq!(
        (response.status().as_u16(), error_code(response)),
        (expected_status, expected_code.to_string()),
        "case {case:?}: wrong refusal, body {:?}",
        json(response)
    );
    // The expectation is read from the service's own table rather than written per
    // row, so this suite checks the rendering of whatever that code is worded with.
    let expected = errors::CANONICAL_MESSAGES
        .iter()
        .find(|(code, _)| *code == expected_code)
        .map(|(_, message)| *message)
        .unwrap_or_else(|| panic!("case {case:?}: {expected_code} is not in CANONICAL_MESSAGES"));
    let slot = message_slot(response);
    let Some(slot) = slot else {
        panic!(
            "case {case:?}: the error document omits the message key entirely, body {:?}",
            json(response)
        )
    };
    match expected {
        None => assert!(
            slot.is_null(),
            "case {case:?}: {expected_code} carries no message, so the member must be \
             JSON null, got {slot:?}"
        ),
        Some(sentence) => assert_eq!(
            slot.as_str(),
            Some(sentence),
            "case {case:?}: expected the message {sentence:?}, got {slot:?}"
        ),
    }
}

/// RULE: the error document of the code Amazon Polly answers with NO message carries
/// the `message` member as JSON null — for every identifier of the form a `TaskId` is
/// minted in, and whatever else is in the store.
#[tokio::test]
async fn the_not_found_task_error_carries_a_null_message_member() {
    let polly = Polly::new();
    // The rows span the minted identifier form: identifiers from the service's own
    // minter, the all-zero one, and one spelled with upper-case hex digits. None of
    // them names a stored task.
    let unknown = [
        mint_task_id(),
        mint_task_id(),
        fixture_task_id("never stored"),
        "00000000-0000-0000-0000-000000000000".to_string(),
        "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C".to_string(),
    ];
    for id in &unknown {
        let response = get_task(&polly, id).await;
        assert_message_member(
            &format!("TaskId {id:?} with an empty store"),
            &response,
            400,
            "SynthesisTaskNotFoundException",
        );
    }

    // A store that HOLDS a task answers the same way for every id that is not it: the
    // member's presence is a property of the code, not of the store's contents.
    seed_task(
        &polly,
        &stored_task(&fixture_task_id("stored"), SystemTime::now()),
    );
    for id in &unknown {
        let response = get_task(&polly, id).await;
        assert_message_member(
            &format!("TaskId {id:?} beside a stored task"),
            &response,
            400,
            "SynthesisTaskNotFoundException",
        );
    }
}

/// RULE, over the codes several operations can answer: the `message` member is present
/// on EVERY error document — carrying the code's sentence where it has one, and null
/// where it has none — so one code's rendering is not a special case of another's.
#[tokio::test]
async fn every_error_document_carries_its_message_member() {
    let polly = Polly::new();
    let mut rows: Vec<(String, http::Response<Bytes>, u16, &str)> = Vec::new();

    // The code with no message and the codes with one, from several operations: an
    // identifier that names no task, in both the minted form and a form the service
    // does not mint, since neither is held.
    for id in [
        mint_task_id(),
        fixture_task_id("absent"),
        "b".to_string(),
        "unknown-task-id".to_string(),
        "z".repeat(100),
    ] {
        rows.push((
            format!("GetSpeechSynthesisTask {id:?}"),
            get_task(&polly, &id).await,
            400,
            "SynthesisTaskNotFoundException",
        ));
    }

    // LexiconNotFoundException: three operations reach it, over several names — one of
    // them stored and then deleted.
    assert_eq!(
        polly
            .put_lexicon("WasStored", &pls_with_replacement("alias", 4))
            .await
            .status(),
        200
    );
    assert_eq!(polly.delete_lexicon("WasStored").await.status(), 200);
    for name in ["Missing", "WasStored", "X"] {
        rows.push((
            format!("GetLexicon {name:?}"),
            polly.get_lexicon(name).await,
            404,
            "LexiconNotFoundException",
        ));
        rows.push((
            format!("DeleteLexicon {name:?}"),
            polly.delete_lexicon(name).await,
            404,
            "LexiconNotFoundException",
        ));
        rows.push((
            format!("SynthesizeSpeech with lexicon {name:?}"),
            polly
                .synthesize(&serde_json::json!({
                    "Text": "hello world",
                    "VoiceId": "Joanna",
                    "OutputFormat": "mp3",
                    "LexiconNames": [name],
                }))
                .await,
            404,
            "LexiconNotFoundException",
        ));
    }

    // InvalidNextTokenException: both paginated reads, several foreign tokens.
    for token in ["not-a-real-token", "zzzz"] {
        rows.push((
            format!("ListLexicons NextToken {token:?}"),
            polly.list_lexicons(&format!("?NextToken={token}")).await,
            400,
            "InvalidNextTokenException",
        ));
        rows.push((
            format!("DescribeVoices NextToken {token:?}"),
            polly.describe_voices(&format!("?NextToken={token}")).await,
            400,
            "InvalidNextTokenException",
        ));
    }

    // The PutLexicon and SynthesizeSpeech refusals, one row per code.
    for content in ["<not-a-lexicon/>", "nonsense <"] {
        rows.push((
            format!("PutLexicon content {content:?}"),
            polly.put_lexicon("NotPls", content).await,
            400,
            "InvalidLexiconException",
        ));
    }
    rows.push((
        "PutLexicon past the size quota".to_string(),
        polly
            .put_lexicon("TooBig", &pls_with_replacement("grapheme", 40_078))
            .await,
        400,
        "LexiconSizeExceededException",
    ));
    rows.push((
        "PutLexicon past the lexeme quota".to_string(),
        polly
            .put_lexicon("LongLexeme", &pls_with_replacement("alias", 150))
            .await,
        400,
        "MaxLexemeLengthExceededException",
    ));
    rows.push((
        "PutLexicon in an unsupported language".to_string(),
        polly
            .put_lexicon(
                "BadLanguage",
                "<lexicon version=\"1.0\" \
                 xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
                 alphabet=\"ipa\" xml:lang=\"xx-ZZ\"/>",
            )
            .await,
        400,
        "UnsupportedPlsLanguageException",
    ));
    for (format, rate) in [("mp3", "99999"), ("pcm", "7")] {
        rows.push((
            format!("SynthesizeSpeech {format} at {rate}"),
            polly
                .synthesize(&serde_json::json!({
                    "Text": "hello world",
                    "VoiceId": "Joanna",
                    "OutputFormat": format,
                    "SampleRate": rate,
                }))
                .await,
            400,
            "InvalidSampleRateException",
        ));
    }
    rows.push((
        "SynthesizeSpeech asking for marks with an audio format".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "SpeechMarkTypes": ["word"],
            }))
            .await,
        400,
        "MarksNotSupportedForFormatException",
    ));
    rows.push((
        "SynthesizeSpeech asking for SSML marks over plain text".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "TextType": "text",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": ["ssml"],
            }))
            .await,
        400,
        "SsmlMarksNotSupportedForTextTypeException",
    ));
    rows.push((
        "SynthesizeSpeech with text past the limit".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "a".repeat(6_001),
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await,
        400,
        "TextLengthExceededException",
    ));
    rows.push((
        "SynthesizeSpeech with malformed SSML".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "<speak>unclosed",
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await,
        400,
        "InvalidSsmlException",
    ));

    // Every code these requests reach, and at least the one that carries no message.
    let mut codes: Vec<&str> = rows.iter().map(|(_, _, _, code)| *code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert!(
        codes.len() >= 8 && codes.contains(&"SynthesisTaskNotFoundException"),
        "the table must span the error codes, got {codes:?}"
    );

    for (case, response, status, code) in &rows {
        assert_message_member(case, response, *status, code);
    }
}

/// RULE, for the document the framework itself renders: a refusal carries a `message`
/// member too, and it is always a non-empty string, because it is built from the request.
///
/// The scaffold channel's 501 has no row here any more: every operation this service
/// models is implemented, so no request reaches it.
#[tokio::test]
async fn the_framework_documents_carry_a_string_message_member() {
    let polly = Polly::new();
    let mut rows: Vec<(String, http::Response<Bytes>, u16, &str)> = Vec::new();

    // A `TaskId` outside the model's pattern never reaches the handler: the boundary
    // refuses it, with a message built from the request.
    for bad in ["a".repeat(101), "bad!id".to_string(), "bad.id".to_string()] {
        rows.push((
            format!("GetSpeechSynthesisTask {bad:?}"),
            get_task(&polly, &bad).await,
            400,
            "ValidationException",
        ));
    }

    // A voice that does not support the engine the request names is the other document
    // the framework renders: a refusal whose message names the engine.
    rows.push((
        "SynthesizeSpeech with an engine the voice does not support".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "Engine": "long-form",
                "OutputFormat": "pcm",
            }))
            .await,
        400,
        "ValidationException",
    ));

    for (case, response, status, code) in &rows {
        assert_eq!(
            (response.status().as_u16(), error_code(response)),
            (*status, code.to_string()),
            "case {case:?}: wrong refusal, body {:?}",
            json(response)
        );
        let slot = message_slot(response);
        let Some(slot) = slot else {
            panic!(
                "case {case:?}: the error document omits the message key entirely, \
                 body {:?}",
                json(response)
            )
        };
        let text = slot.as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a string message member, got {slot:?}")
        });
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty message, got {text:?}"
        );
    }
}

/// RULE: the always-present member is a property of the ERROR document. A SUCCESS body
/// still omits every optional member the resource has no value for, which is what an
/// AWS JSON response body does.
#[tokio::test]
async fn a_success_body_omits_the_members_it_has_no_value_for() {
    let polly = Polly::new();
    let mut task = stored_task(&fixture_task_id("sparse"), SystemTime::now());
    task.task_status = "completed".to_string();
    task.sns_topic_arn = None;
    task.language_code = None;
    task.sample_rate = None;
    task.task_status_reason = None;
    seed_task(&polly, &task);

    let response = get_task(&polly, &task.task_id).await;
    assert_eq!(
        response.status(),
        200,
        "the seeded task must read back, body {:?}",
        json(&response)
    );
    let body = json(&response);
    let reported = &body["SynthesisTask"];
    for member in [
        "SnsTopicArn",
        "LanguageCode",
        "SampleRate",
        "TaskStatusReason",
    ] {
        assert!(
            reported.get(member).is_none(),
            "member {member:?}: an absent optional member stays out of a success body, \
             got {:?}",
            reported.get(member)
        );
    }
    // And the members it DOES have a value for are there, so the check above is not
    // passing on an empty body.
    assert_eq!(
        reported["TaskId"].as_str(),
        Some(task.task_id.as_str()),
        "the task's own members must still be reported, got {reported:?}"
    );
}
