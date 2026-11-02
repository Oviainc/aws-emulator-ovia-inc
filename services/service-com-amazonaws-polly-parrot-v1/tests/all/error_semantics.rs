// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Polly's refusal semantics: which status and error code each
//! refusal carries, what message rides on it, and the asynchronous-task read path.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs that
//! spans the class the rule covers — several task ids rather than one, every operation
//! that can answer a given error code rather than one, both published bounds of a
//! quota and one past it. The single request an issue happens to quote is one row.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::MAX_LEXEME_CHARACTERS;
use service_com_amazonaws_polly_parrot_v1::service::errors;
use service_com_amazonaws_polly_parrot_v1::state::{
    mint_task_id, PendingTaskStatus, StoredSynthesisTask, TASK_COMPLETION_DELAY,
};

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{fixture_task_id, pls_with_replacement, seed_task, stored_task};

/// The `message` an error response carries, absent when the error carries none.
fn error_message(response: &http::Response<Bytes>) -> Option<String> {
    let body = json(response);
    // restJson1 renders the modelled `message` member under its member name; the
    // capitalised spelling is accepted too so the helper does not depend on which.
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(|m| m.as_str()) {
            return Some(message.to_string());
        }
    }
    None
}

/// Assert one refusal: its status, its error code, and the message it carries —
/// `expected_message` of `None` meaning the error carries no message at all.
///
/// `forbidden` lists the values the REQUEST carried: a message fixed per error code
/// cannot contain any of them.
fn assert_refusal(
    case: &str,
    response: &http::Response<Bytes>,
    expected_status: u16,
    expected_code: &str,
    expected_message: Option<&str>,
    forbidden: &[&str],
) {
    assert_eq!(
        (response.status().as_u16(), error_code(response)),
        (expected_status, expected_code.to_string()),
        "case {case:?}: wrong refusal, body {:?}",
        json(response)
    );
    let actual = error_message(response);
    assert_eq!(
        actual.as_deref(),
        expected_message,
        "case {case:?}: expected the fixed message for {expected_code}, got {actual:?}"
    );
    if let Some(message) = actual {
        for value in forbidden {
            assert!(
                !message.contains(value),
                "case {case:?}: message {message:?} echoes the request value {value:?}"
            );
        }
    }
}

/// RULE: a read of a resource that does not exist is refused with the modelled
/// not-found error for that resource — for a speech synthesis task,
/// `SynthesisTaskNotFoundException` (HTTP 400) — for every identifier the model's
/// `TaskId` pattern admits; an identifier the pattern does not admit is refused at the
/// request boundary instead, and neither answer is ever a 501 or a 5xx.
/// (`crate::task_id_validity` covers that whole class, on both sides of the pattern.)
#[tokio::test]
async fn an_unknown_synthesis_task_is_refused_with_its_modeled_not_found_error() {
    let polly = Polly::new();
    // The rows span the minted identifier form: identifiers from the service's own
    // minter, the all-zero one, and one spelled with upper-case hex digits.
    let well_formed = [
        mint_task_id(),
        mint_task_id(),
        fixture_task_id("never stored"),
        "00000000-0000-0000-0000-000000000000".to_string(),
        "6F3A1B2C-9D4E-4F5A-8B6C-7D8E9F0A1B2C".to_string(),
    ];
    for id in well_formed {
        let response = polly
            .call("GET", &format!("/v1/synthesisTasks/{id}"), "")
            .await;
        assert_refusal(
            &format!("TaskId {id:?}"),
            &response,
            400,
            "SynthesisTaskNotFoundException",
            // Amazon Polly answers this error with a null message body.
            None,
            &[&id],
        );
    }

    // One past the pattern's upper bound, and two characters outside its class: the
    // framework refuses these before the handler runs, as a request-shape failure.
    for bad in ["a".repeat(101), "bad!id".to_string(), "bad.id".to_string()] {
        let response = polly
            .call("GET", &format!("/v1/synthesisTasks/{bad}"), "")
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "ValidationException".to_string()),
            "TaskId {bad:?}: a malformed id is a request-shape refusal, body {:?}",
            json(&response)
        );
    }
}

/// RULE: a reporting read answers from stored state — `GetSpeechSynthesisTask` reports
/// every member of the task the store holds, reports the status transition the clock
/// has already made due, and does not hurry one that is not yet due.
#[tokio::test]
async fn get_speech_synthesis_task_reports_the_stored_task() {
    let polly = Polly::new();
    let created = SystemTime::now() - Duration::from_secs(30);
    let now = SystemTime::now();

    let mut rows: Vec<(&str, StoredSynthesisTask, &str)> = Vec::new();

    // A task already in a terminal status stays there.
    let mut completed = stored_task(&fixture_task_id("completed"), created);
    completed.task_status = "completed".to_string();
    rows.push(("completed", completed, "completed"));

    // A failed task reports its reason beside its status.
    let mut failed = stored_task(&fixture_task_id("failed"), created);
    failed.task_status = "failed".to_string();
    failed.task_status_reason = Some("the output bucket is not writable".to_string());
    rows.push(("failed with a reason", failed, "failed"));

    // The clock has already made the completion due, so the read reports it.
    let mut due = stored_task(&fixture_task_id("due"), created);
    due.pending_status = Some(PendingTaskStatus {
        due_at: now - TASK_COMPLETION_DELAY,
        status: "completed".to_string(),
        reason: None,
    });
    rows.push(("completion already due", due, "completed"));

    // Not yet due: still the status it was stored in, and a read must not advance it.
    let mut pending = stored_task(&fixture_task_id("pending"), created);
    pending.task_status = "scheduled".to_string();
    pending.pending_status = Some(PendingTaskStatus {
        due_at: now + Duration::from_secs(3_600),
        status: "completed".to_string(),
        reason: None,
    });
    rows.push(("completion not yet due", pending, "scheduled"));

    // A task recorded with every optional member set, so none of them is dropped or
    // normalised on the way out.
    let mut full = stored_task(&fixture_task_id("full"), created);
    full.task_status = "completed".to_string();
    full.engine = "neural".to_string();
    full.output_format = "json".to_string();
    full.sample_rate = None;
    full.speech_mark_types = vec!["sentence".to_string(), "word".to_string()];
    full.text_type = "ssml".to_string();
    full.voice_id = "Aditi".to_string();
    full.language_code = Some("hi-IN".to_string());
    full.lexicon_names = vec!["LexA".to_string(), "LexB".to_string()];
    full.sns_topic_arn =
        Some("arn:aws:sns:us-east-1:000000000000:polly-synthesis".to_string());
    full.request_characters = 4_242;
    rows.push(("every optional member set", full, "completed"));

    for (case, task, expected_status) in rows {
        seed_task(&polly, &task);
        let path = format!("/v1/synthesisTasks/{}", task.task_id);
        let response = polly.call("GET", &path, "").await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: expected the task, body {:?}",
            json(&response)
        );
        let body = json(&response);
        let reported = &body["SynthesisTask"];
        assert_eq!(
            reported["TaskId"].as_str(),
            Some(task.task_id.as_str()),
            "case {case:?}: wrong TaskId, got {reported:?}"
        );
        assert_eq!(
            reported["TaskStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: wrong TaskStatus, got {reported:?}"
        );
        assert_eq!(
            reported["TaskStatusReason"].as_str(),
            task.task_status_reason.as_deref(),
            "case {case:?}: wrong TaskStatusReason, got {reported:?}"
        );
        assert_eq!(
            reported["Engine"].as_str(),
            Some(task.engine.as_str()),
            "case {case:?}: wrong Engine, got {reported:?}"
        );
        assert_eq!(
            reported["OutputFormat"].as_str(),
            Some(task.output_format.as_str()),
            "case {case:?}: wrong OutputFormat, got {reported:?}"
        );
        assert_eq!(
            reported["OutputUri"].as_str(),
            Some(task.output_uri.as_str()),
            "case {case:?}: wrong OutputUri, got {reported:?}"
        );
        assert_eq!(
            reported["VoiceId"].as_str(),
            Some(task.voice_id.as_str()),
            "case {case:?}: wrong VoiceId, got {reported:?}"
        );
        assert_eq!(
            reported["TextType"].as_str(),
            Some(task.text_type.as_str()),
            "case {case:?}: wrong TextType, got {reported:?}"
        );
        assert_eq!(
            reported["SampleRate"].as_str(),
            task.sample_rate.as_deref(),
            "case {case:?}: wrong SampleRate, got {reported:?}"
        );
        assert_eq!(
            reported["LanguageCode"].as_str(),
            task.language_code.as_deref(),
            "case {case:?}: wrong LanguageCode, got {reported:?}"
        );
        assert_eq!(
            reported["SnsTopicArn"].as_str(),
            task.sns_topic_arn.as_deref(),
            "case {case:?}: wrong SnsTopicArn, got {reported:?}"
        );
        assert_eq!(
            reported["RequestCharacters"].as_i64(),
            Some(i64::from(task.request_characters)),
            "case {case:?}: wrong RequestCharacters, got {reported:?}"
        );
        // An optional list member is reported only when the task has values for it,
        // so a task that applied no lexicon and asked for no speech marks carries
        // neither key — not an empty array.
        for (member, stored) in [
            ("LexiconNames", &task.lexicon_names),
            ("SpeechMarkTypes", &task.speech_mark_types),
        ] {
            let expected = (!stored.is_empty()).then(|| serde_json::json!(stored));
            assert_eq!(
                reported.get(member).cloned(),
                expected,
                "case {case:?}: wrong {member}, got {reported:?}"
            );
        }
        let reported_creation = reported["CreationTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: CreationTime is a timestamp, got {reported:?}"));
        let stored_creation = task
            .creation_time
            .duration_since(UNIX_EPOCH)
            .expect("the task was created after the epoch")
            .as_secs_f64();
        assert!(
            (reported_creation - stored_creation).abs() < 1.0,
            "case {case:?}: CreationTime {reported_creation} is not the stored \
             {stored_creation}"
        );

        // Reading again reports the same task: a read is not what moves a status.
        let again = json(&polly.call("GET", &path, "").await);
        assert_eq!(
            again["SynthesisTask"]["TaskStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: a second read changed the status"
        );
    }
}

/// RULE: every modelled exception this service answers with carries Amazon Polly's
/// FIXED message for its error code — one sentence per code, the same for every
/// request, never built from the request and never echoing a value the caller sent.
#[tokio::test]
async fn every_modeled_error_carries_the_fixed_message_for_its_code() {
    let polly = Polly::new();

    // LexiconNotFoundException: three operations, several names, including one that
    // was stored and then deleted.
    assert_eq!(polly.put_lexicon("WasStored", &pls_with_replacement("alias", 4)).await.status(), 200);
    assert_eq!(polly.delete_lexicon("WasStored").await.status(), 200);
    for name in ["Missing", "ltgtfe7c3e80084a", "WasStored", "X"] {
        assert_refusal(
            &format!("GetLexicon {name:?}"),
            &polly.get_lexicon(name).await,
            404,
            "LexiconNotFoundException",
            Some(errors::LEXICON_NOT_FOUND),
            &[name],
        );
        assert_refusal(
            &format!("DeleteLexicon {name:?}"),
            &polly.delete_lexicon(name).await,
            404,
            "LexiconNotFoundException",
            Some(errors::LEXICON_NOT_FOUND),
            &[name],
        );
        let synthesis = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "LexiconNames": [name],
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech with lexicon {name:?}"),
            &synthesis,
            404,
            "LexiconNotFoundException",
            Some(errors::LEXICON_NOT_FOUND),
            &[name],
        );
    }

    // InvalidNextTokenException: both paginated reads, several foreign tokens.
    for token in [
        "not-a-real-pagination-token",
        "this-was-never-issued-by-polly==",
        "zzzz",
    ] {
        assert_refusal(
            &format!("ListLexicons NextToken {token:?}"),
            &polly.list_lexicons(&format!("?NextToken={token}")).await,
            400,
            "InvalidNextTokenException",
            Some(errors::INVALID_NEXT_TOKEN),
            &[token],
        );
        assert_refusal(
            &format!("DescribeVoices NextToken {token:?}"),
            &polly.describe_voices(&format!("?NextToken={token}")).await,
            400,
            "InvalidNextTokenException",
            Some(errors::INVALID_NEXT_TOKEN),
            &[token],
        );
    }

    // The PutLexicon refusals, each over more than one document.
    for length in [5_000usize, 40_078] {
        let content = pls_with_replacement("grapheme", length);
        assert_refusal(
            &format!("PutLexicon of {length} characters"),
            &polly.put_lexicon("TooBig", &content).await,
            400,
            "LexiconSizeExceededException",
            Some(errors::LEXICON_SIZE_EXCEEDED),
            &[&length.to_string()],
        );
    }
    for content in ["<not-a-lexicon/>", "<speak>hello</speak>", "nonsense <"] {
        assert_refusal(
            &format!("PutLexicon content {content:?}"),
            &polly.put_lexicon("NotPls", content).await,
            400,
            "InvalidLexiconException",
            Some(errors::INVALID_LEXICON),
            &["not-a-lexicon", "speak", "nonsense"],
        );
    }
    for language in ["xx-ZZ", "klingon", "zz"] {
        let content = format!(
            "<lexicon version=\"1.0\" \
             xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             alphabet=\"ipa\" xml:lang=\"{language}\"/>"
        );
        assert_refusal(
            &format!("PutLexicon language {language:?}"),
            &polly.put_lexicon("BadLanguage", &content).await,
            400,
            "UnsupportedPlsLanguageException",
            Some(errors::UNSUPPORTED_PLS_LANGUAGE),
            &[language],
        );
    }
    for length in [MAX_LEXEME_CHARACTERS + 1, 150] {
        let content = pls_with_replacement("alias", length);
        assert_refusal(
            &format!("PutLexicon alias of {length} characters"),
            &polly.put_lexicon("LongLexeme", &content).await,
            400,
            "MaxLexemeLengthExceededException",
            Some(errors::MAX_LEXEME_LENGTH_EXCEEDED),
            &[&length.to_string()],
        );
    }

    // The SynthesizeSpeech refusals, each over more than one request.
    for (format, rate) in [("mp3", "99999"), ("pcm", "7"), ("ogg_opus", "44100")] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": format,
                "SampleRate": rate,
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech {format} at {rate}"),
            &response,
            400,
            "InvalidSampleRateException",
            Some(errors::INVALID_SAMPLE_RATE),
            &[rate, format],
        );
    }
    for text in ["<speak>unclosed", "not ssml at all <", "<speak>a</speak"] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech SSML {text:?}"),
            &response,
            400,
            "InvalidSsmlException",
            Some(errors::INVALID_SSML),
            &["speak", "unclosed"],
        );
    }
    for format in ["mp3", "pcm", "ogg_opus"] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": format,
                "SpeechMarkTypes": ["word"],
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech marks with {format}"),
            &response,
            400,
            "MarksNotSupportedForFormatException",
            Some(errors::MARKS_NOT_SUPPORTED_FOR_FORMAT),
            &[format],
        );
    }
    for marks in [
        serde_json::json!(["ssml"]),
        serde_json::json!(["ssml", "word"]),
    ] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "TextType": "text",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": marks,
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech SSML marks {marks:?} over plain text"),
            &response,
            400,
            "SsmlMarksNotSupportedForTextTypeException",
            Some(errors::SSML_MARKS_NOT_SUPPORTED_FOR_TEXT_TYPE),
            &["ssml", "word"],
        );
    }
    for length in [3_001usize, 6_001] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "a".repeat(length),
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_refusal(
            &format!("SynthesizeSpeech text of {length} characters"),
            &response,
            400,
            "TextLengthExceededException",
            Some(errors::TEXT_LENGTH_EXCEEDED),
            &[&length.to_string()],
        );
    }

    // And the voice/engine mismatch, which Amazon Polly answers as a request-shape
    // failure rather than with the modelled EngineNotSupportedException. Its message
    // IS built from the request, so only the status and code are pinned here.
    for (voice, engine) in [("Geraint", "neural"), ("Kevin", "standard"), ("Nicole", "generative")] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": voice,
                "Engine": engine,
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "ValidationException".to_string()),
            "voice {voice:?} on engine {engine:?}: expected a request-shape refusal, \
             body {:?}",
            json(&response)
        );
    }
}

/// RULE: the per-Region lexicon count quota is enforced, and its refusal carries the
/// same fixed message as every other error code's.
#[tokio::test]
async fn the_lexicon_count_quota_refusal_carries_its_fixed_message() {
    let polly = Polly::new();
    let content = pls_with_replacement("alias", 4);
    for index in 0..100 {
        let response = polly.put_lexicon(&format!("Lex{index:04}"), &content).await;
        assert_eq!(
            response.status(),
            200,
            "lexicon {index} is within the quota, body {:?}",
            json(&response)
        );
    }
    assert_refusal(
        "one lexicon past the count quota",
        &polly.put_lexicon("OneTooMany", &content).await,
        400,
        "MaxLexiconsNumberExceededException",
        Some(errors::MAX_LEXICONS_NUMBER_EXCEEDED),
        &["OneTooMany", "100"],
    );
}

/// RULE (its recorded half): the fixed sentence a code carries is the one Amazon
/// Polly's own responses carry. The literals below are the OBSERVED text, pinned in
/// ONE table so a reworded or mistyped constant cannot pass unnoticed; the tests above
/// then check that the wire really carries that text, for every request and every
/// operation that reaches the code.
#[test]
fn the_fixed_messages_are_the_sentences_amazon_polly_sends() {
    let observed = [
        ("InvalidLexiconException", Some("Invalid PLS Lexicon")),
        ("InvalidNextTokenException", Some("Invalid NextToken parameter")),
        ("InvalidSampleRateException", Some("Invalid SampleRate parameter")),
        ("InvalidSsmlException", Some("Invalid SSML request")),
        ("LexiconNotFoundException", Some("Lexicon not found")),
        (
            "LexiconSizeExceededException",
            Some("Maximum lexicons size has been exceeded"),
        ),
        (
            "MarksNotSupportedForFormatException",
            Some("Speech marks are not supported for this format"),
        ),
        (
            "MaxLexemeLengthExceededException",
            Some("Lexeme rule length has been exceeded"),
        ),
        (
            "MaxLexiconsNumberExceededException",
            Some("Max lexicon number has been exceeded"),
        ),
        (
            "SsmlMarksNotSupportedForTextTypeException",
            Some("SSML Speech marks are not supported for this text type"),
        ),
        // Amazon Polly answers this one with a null message body.
        ("SynthesisTaskNotFoundException", None),
        (
            "TextLengthExceededException",
            Some("Maximum text length has been exceeded"),
        ),
        (
            "UnsupportedPlsLanguageException",
            Some("Unsupported PLS language"),
        ),
    ];
    for (code, sentence) in observed {
        let entry = errors::CANONICAL_MESSAGES
            .iter()
            .find(|(table_code, _)| *table_code == code)
            .unwrap_or_else(|| panic!("code {code:?} is not in the service's message table"));
        assert_eq!(
            entry.1, sentence,
            "code {code:?}: the service's message is not the observed sentence"
        );
    }
}

/// RULE: `PutLexicon` refuses a document whose `<lexeme>` replacement is longer than
/// Amazon Polly's published lexeme quota, with `MaxLexemeLengthExceededException`
/// (HTTP 400), and stores nothing; a replacement at or under the quota is stored.
#[tokio::test]
async fn put_lexicon_enforces_the_lexeme_length_quota() {
    // AWS's published quotas for Amazon Polly cap a lexeme at 100 characters, which
    // is what the handler's constant carries.
    assert_eq!(
        MAX_LEXEME_CHARACTERS, 100,
        "the published per-lexeme quota is 100 characters"
    );
    let polly = Polly::new();
    // PLS defines two replacement forms, and the quota is on both.
    for element in ["alias", "phoneme"] {
        for (case, length, refused) in [
            ("one character", 1, false),
            ("at the quota", MAX_LEXEME_CHARACTERS, false),
            ("one past the quota", MAX_LEXEME_CHARACTERS + 1, true),
            ("far past the quota", 150, true),
        ] {
            let name = format!("Lex{element}{length}");
            let response = polly
                .put_lexicon(&name, &pls_with_replacement(element, length))
                .await;
            let read_back = polly.get_lexicon(&name).await;
            if refused {
                assert_refusal(
                    &format!("<{element}> {case}"),
                    &response,
                    400,
                    "MaxLexemeLengthExceededException",
                    Some(errors::MAX_LEXEME_LENGTH_EXCEEDED),
                    &[&length.to_string(), element],
                );
                assert_eq!(
                    (read_back.status().as_u16(), error_code(&read_back)),
                    (404, "LexiconNotFoundException".to_string()),
                    "<{element}> {case}: a refused put must store nothing"
                );
            } else {
                assert_eq!(
                    response.status(),
                    200,
                    "<{element}> {case}: expected the put to be stored, body {:?}",
                    json(&response)
                );
                assert_eq!(
                    read_back.status(),
                    200,
                    "<{element}> {case}: the stored lexicon must read back"
                );
            }
        }
    }

    // The quota is on the REPLACEMENT: a `<grapheme>` longer than it, in a document
    // within the lexicon size quota, is stored.
    let long_grapheme = pls_with_replacement("grapheme", MAX_LEXEME_CHARACTERS * 3);
    assert_eq!(
        polly.put_lexicon("LongGrapheme", &long_grapheme).await.status(),
        200,
        "a long grapheme is not a long lexeme replacement"
    );
    assert_eq!(
        polly.get_lexicon("LongGrapheme").await.status(),
        200,
        "the lexicon with the long grapheme must read back"
    );
}
