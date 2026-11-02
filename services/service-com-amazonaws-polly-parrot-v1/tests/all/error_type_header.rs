// SPDX-License-Identifier: CC0-1.0

//! How an error answer NAMES its error shape.
//!
//! RULE: the `x-amzn-errortype` header of an error this service's model declares names
//! the error shape QUALIFIED by the service's own error namespace — the shape name,
//! then a colon, then the namespace URI the service's model carries — so the shape name
//! is the only part of that value which varies with the error that occurred. A client,
//! an SDK or a proxy matching the whole value reads what AWS sends.
//!
//! The rule is checked over a TABLE of refusals rather than the one the issue quoted:
//! the rows span the error codes of this service, several operations, both status
//! classes a refusal takes (400 and 404) and both renderings of the error document (a
//! code with a message and the one with none), so the header value cannot be a property
//! of any single code or operation.

use bytes::Bytes;

use crate::lexicons_and_speech::{error_code, header, json, Polly};
use crate::support::{get_task, pls_with_replacement};
use service_com_amazonaws_polly_parrot_v1::state::mint_task_id;

/// The header an error answer names its shape in.
const ERRORTYPE_HEADER: &str = "x-amzn-errortype";

/// The namespace the shape name is qualified by.
///
/// Source: this service's model declares `xmlNamespace: {uri:
/// "http://polly.amazonaws.com/doc/v1"}` on the service shape
/// (`smithy_query(query="service_info")`), which is the error namespace Amazon Polly's
/// own answers name their shapes in.
const ERROR_NAMESPACE: &str = "http://polly.amazonaws.com/doc/v1";

/// What the errortype header must read for `code`.
fn qualified(code: &str) -> String {
    format!("{code}:{ERROR_NAMESPACE}")
}

/// RULE: every modelled refusal names its shape qualified — one row per error code
/// these operations reach, over several operations and both refusal statuses.
#[tokio::test]
async fn every_modeled_error_names_its_shape_qualified() {
    let polly = Polly::new();
    let mut rows: Vec<(String, http::Response<Bytes>, u16, &str)> = Vec::new();

    // The code that carries NO message, and so renders its document differently: a
    // `TaskId` that names no task, in both the minted form and another the pattern
    // admits.
    for id in [mint_task_id(), "unknown-task-id".to_string()] {
        rows.push((
            format!("GetSpeechSynthesisTask {id:?}"),
            get_task(&polly, &id).await,
            400,
            "SynthesisTaskNotFoundException",
        ));
    }

    // A 404 refusal, from three operations that reach the same code.
    rows.push((
        "GetLexicon of a name nothing stored".to_string(),
        polly.get_lexicon("Missing").await,
        404,
        "LexiconNotFoundException",
    ));
    rows.push((
        "DeleteLexicon of a name nothing stored".to_string(),
        polly.delete_lexicon("Missing").await,
        404,
        "LexiconNotFoundException",
    ));
    rows.push((
        "SynthesizeSpeech naming a lexicon nothing stored".to_string(),
        polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "LexiconNames": ["Missing"],
            }))
            .await,
        404,
        "LexiconNotFoundException",
    ));

    // Both paginated reads, on a token this service did not mint.
    rows.push((
        "ListLexicons with a foreign NextToken".to_string(),
        polly.list_lexicons("?NextToken=not-a-real-token").await,
        400,
        "InvalidNextTokenException",
    ));
    rows.push((
        "ListSpeechSynthesisTasks with a foreign NextToken".to_string(),
        polly
            .call("GET", "/v1/synthesisTasks?NextToken=not-a-real-token", "")
            .await,
        400,
        "InvalidNextTokenException",
    ));

    // The write path and the two synthesis paths, one row per code.
    rows.push((
        "PutLexicon of a document that is no lexicon".to_string(),
        polly.put_lexicon("NotPls", "<not-a-lexicon/>").await,
        400,
        "InvalidLexiconException",
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
        "StartSpeechSynthesisTask asking for marks with an audio format".to_string(),
        polly
            .call(
                "POST",
                "/v1/synthesisTasks",
                &serde_json::json!({
                    "Text": "hello world",
                    "VoiceId": "Joanna",
                    "OutputFormat": "mp3",
                    "OutputS3BucketName": "polly-output",
                    "SpeechMarkTypes": ["word"],
                })
                .to_string(),
            )
            .await,
        400,
        "MarksNotSupportedForFormatException",
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

    // The table must span the codes, or one row's header would be the whole evidence.
    let mut codes: Vec<&str> = rows.iter().map(|(_, _, _, code)| *code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert!(
        codes.len() >= 6,
        "the table must span the error codes, got {codes:?}"
    );

    for (case, response, status, code) in &rows {
        assert_eq!(
            (response.status().as_u16(), error_code(response)),
            (*status, (*code).to_string()),
            "case {case:?}: wrong refusal, body {:?}",
            json(response)
        );
        let actual = header(response, ERRORTYPE_HEADER);
        assert_eq!(
            actual,
            qualified(code),
            "case {case:?}: expected the qualified errortype header, got {actual:?}"
        );
    }
}

/// RULE (what the qualifier is NOT): the namespace is fixed text and the shape name is
/// the variable part, so the header's prefix is the code the document carries and the
/// value is qualified exactly once — never twice, and never with another service's
/// namespace.
#[tokio::test]
async fn the_qualified_header_names_the_code_exactly_once() {
    let polly = Polly::new();
    for id in [
        mint_task_id(),
        "b".to_string(),
        "z".repeat(100),
        "Task_1-2".to_string(),
    ] {
        let response = get_task(&polly, &id).await;
        let actual = header(&response, ERRORTYPE_HEADER);
        let code = error_code(&response);
        assert!(
            actual.starts_with(&code),
            "TaskId {id:?}: the header {actual:?} must begin with the code {code:?}"
        );
        assert_eq!(
            actual.matches(ERROR_NAMESPACE).count(),
            1,
            "TaskId {id:?}: the namespace must appear exactly once, got {actual:?}"
        );
        assert_eq!(
            actual,
            qualified(&code),
            "TaskId {id:?}: wrong errortype header, got {actual:?}"
        );
    }
}
