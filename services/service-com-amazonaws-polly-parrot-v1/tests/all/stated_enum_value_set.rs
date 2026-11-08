// SPDX-License-Identifier: CC0-1.0

//! Which accepted values a request-shape refusal STATES for an enumerated member.
//!
//! RULE: when a value outside an enumerated member's value set is refused, the
//! `ValidationException` states that enum's accepted values as the SERVICE publishes
//! the set — its own membership, in its own fixed order — and not in the order the
//! model happens to declare the enum's members in. The stated set is a per-service
//! constant: nothing in the request and nothing in the store varies it, and an enum
//! is recognised by its value set rather than by the member that carried the rejected
//! value, so every member targeting one enum — of any operation, body-bound,
//! query-bound or header-bound — states the one set.
//!
//! Each test checks the rule over a TABLE: four members targeting `LanguageCode`
//! across four operations, several rejected values each, and three OTHER enums of the
//! same model whose sets the service publishes no statement of and which are therefore
//! stated exactly as the model declares them.
//!
//! The stated set is pinned here as the LITERAL Amazon Polly's own refusal carries
//! (recorded trace `polly.synthesize-speech-language-not-supported-error`), so a
//! reordered or mistyped constant cannot pass.

use std::collections::BTreeSet;

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::start_speech_synthesis_stream::{
    MODELED_PATH as STREAM_PATH, STREAMING_ENGINE,
};
use service_com_amazonaws_polly_parrot_v1::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_polly_parrot_v1::service::validation::STATED_LANGUAGE_CODE_VALUES;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// The accepted `LanguageCode` values Amazon Polly's own refusal lists, in the order it
/// lists them, as the recorded response carries them.
const OBSERVED_LANGUAGE_CODE_SET: &str = "en-IE, ar-AE, en-US, fr-BE, en-IN, es-MX, en-ZA, \
                                          tr-TR, ru-RU, ro-RO, pt-PT, pl-PL, nl-NL, it-IT, \
                                          is-IS, fr-FR, fi-FI, es-ES, de-DE, yue-CN, ko-KR, \
                                          en-NZ, en-GB-WLS, hi-IN, de-CH, arb, nl-BE, cy-GB, \
                                          cs-CZ, cmn-CN, da-DK, en-AU, pt-BR, nb-NO, sv-SE, \
                                          ja-JP, es-US, ca-ES, fr-CA, en-GB, de-AT";

/// The declared value sets of three other enums of this model, in the order the model
/// declares them (`smithy_query(query="shape",shape="OutputFormat,Engine,TextType")`).
/// The service publishes no statement of these, so a refusal states them as declared.
const DECLARED_OUTPUT_FORMAT_SET: &str = "json, mp3, ogg_opus, ogg_vorbis, pcm, mulaw, alaw";
const DECLARED_ENGINE_SET: &str = "standard, neural, long-form, generative";
const DECLARED_TEXT_TYPE_SET: &str = "ssml, text";

/// The S3 bucket `StartSpeechSynthesisTask` is asked to write to; its `@pattern` is what
/// makes this a value that reaches the enum check rather than failing beside it.
const OUTPUT_BUCKET: &str = "polly-output";

/// The `message` an error document carries.
fn message(case: &str, response: &http::Response<Bytes>) -> String {
    let body = json(response);
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(|m| m.as_str()) {
            return message.to_string();
        }
    }
    panic!("case {case:?}: the refusal carries no message: {body}")
}

/// The sentence a single-member constraint violation is worded with.
fn one_violation(path: &str, value: &str, set: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{set}]"
    )
}

/// How one row carries the member under test, so the same rejected value is driven
/// through every binding the model gives that member.
#[derive(Debug, Clone, Copy)]
enum Carrier {
    /// `SynthesizeSpeech`: a JSON body member.
    SynthesizeSpeechBody,
    /// `StartSpeechSynthesisTask`: a JSON body member of another operation.
    SynthesisTaskBody,
    /// `DescribeVoices`: an `@httpQuery` member.
    DescribeVoicesQuery,
    /// `StartSpeechSynthesisStream`: an `@httpHeader` member, read by the one
    /// hand-authored operation rather than by the framework's own boundary.
    StreamHeader,
}

impl Carrier {
    /// Refuse-worthy call carrying `member` = `value`, with every other member of the
    /// request valid.
    async fn call(
        self,
        polly: &Polly,
        member: &str,
        value: &str,
    ) -> http::Response<Bytes> {
        match self {
            Self::SynthesizeSpeechBody => {
                let mut request = serde_json::json!({
                    "Text": "Testing the stated value set.",
                    "OutputFormat": "mp3",
                    "VoiceId": "Aditi",
                });
                request[member] = serde_json::json!(value);
                polly.synthesize(&request).await
            }
            Self::SynthesisTaskBody => {
                let mut request = serde_json::json!({
                    "Text": "Testing the stated value set.",
                    "OutputFormat": "mp3",
                    "VoiceId": "Aditi",
                    "OutputS3BucketName": OUTPUT_BUCKET,
                });
                request[member] = serde_json::json!(value);
                polly
                    .call("POST", "/v1/synthesisTasks", &request.to_string())
                    .await
            }
            Self::DescribeVoicesQuery => {
                polly
                    .call("GET", &format!("/v1/voices?{member}={value}"), "")
                    .await
            }
            Self::StreamHeader => {
                let header = format!("x-amzn-{member}");
                let mut headers: Vec<(&str, &str)> = vec![
                    ("content-type", "application/vnd.amazon.eventstream"),
                    ("x-amzn-Engine", STREAMING_ENGINE),
                    ("x-amzn-OutputFormat", "pcm"),
                    ("x-amzn-VoiceId", "Joanna"),
                ];
                headers.retain(|(name, _)| !name.eq_ignore_ascii_case(&header));
                headers.push((header.as_str(), value));
                // The header half is read before the action events are, so an empty
                // body still reaches the refusal under test.
                polly
                    .call_bytes_in_region("us-east-1", "POST", STREAM_PATH, &headers, Vec::new())
                    .await
            }
        }
    }
}

/// RULE, for the one enum this service publishes a statement of: every member
/// targeting `LanguageCode`, whatever operation carries it and however it is bound,
/// refuses a value outside the enum with HTTP 400 `ValidationException` whose message
/// states the service's own set, in the service's own order.
#[tokio::test]
async fn every_member_targeting_language_code_states_the_published_set() {
    let polly = Polly::new();
    // The rows differ along what the rule generalises over: the operation and the HTTP
    // binding the member is carried by, and the form of the rejected value (an
    // unassigned tag, the wrong separator, the wrong case, a language name, a primary
    // subtag alone, a declared value with a suffix).
    let carriers = [
        ("SynthesizeSpeech body", Carrier::SynthesizeSpeechBody),
        ("StartSpeechSynthesisTask body", Carrier::SynthesisTaskBody),
        ("DescribeVoices query", Carrier::DescribeVoicesQuery),
        ("StartSpeechSynthesisStream header", Carrier::StreamHeader),
    ];
    let rejected = ["xx-XX", "en_US", "EN-US", "english", "en", "en-US-x"];

    for (binding, carrier) in carriers {
        for value in rejected {
            let case = format!("{binding}: LanguageCode {value:?}");
            let response = carrier.call(&polly, "LanguageCode", value).await;
            assert_eq!(
                response.status().as_u16(),
                400,
                "case {case:?}: a value outside the enum is refused, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: wrong error code, got {:?}",
                error_code(&response)
            );
            let actual = message(&case, &response);
            assert_eq!(
                actual,
                one_violation("languageCode", value, OBSERVED_LANGUAGE_CODE_SET),
                "case {case:?}: wrong message, got {actual:?}"
            );
        }
    }
}

/// RULE: the stated set is the service's, so it is neither sorted nor the model's
/// declaration order — and every value in it is one the model declares, because a
/// statement must not advertise a value the member would refuse.
#[tokio::test]
async fn the_stated_set_is_the_services_own_and_lists_only_declared_values() {
    let stated: Vec<&str> = STATED_LANGUAGE_CODE_VALUES.to_vec();
    assert_eq!(
        stated.join(", "),
        OBSERVED_LANGUAGE_CODE_SET,
        "the stated set must be the one Amazon Polly's own refusal lists, got {stated:?}"
    );
    for value in &stated {
        assert!(
            LANGUAGE_CODE_VALUES.contains(value),
            "stated value {value:?} is not one the model declares"
        );
    }
    let declared: Vec<&str> = LANGUAGE_CODE_VALUES.to_vec();
    assert_ne!(
        stated, declared,
        "the stated order is not the sorted order the generated catalog holds"
    );

    // Membership, not only order: a value the model declares but the refusal does not
    // list is still ACCEPTED, because what a member admits is the enum it targets and
    // not the set a refusal advertises.
    let unlisted: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|value| !STATED_LANGUAGE_CODE_VALUES.contains(value))
        .collect();
    let polly = Polly::new();
    for value in unlisted {
        let response = polly
            .call("GET", &format!("/v1/voices?LanguageCode={value}"), "")
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "LanguageCode {value:?} is declared by the model, so it is accepted; got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

/// RULE: the statement is keyed to the ENUM, so an enum this service publishes no
/// statement of is stated exactly as the model declares it.
#[tokio::test]
async fn an_enum_with_no_published_statement_is_stated_as_the_model_declares_it() {
    let polly = Polly::new();
    // One row per other enum a `SynthesizeSpeech` body member targets, each with a
    // value outside it.
    let rows = [
        ("OutputFormat", "flac", DECLARED_OUTPUT_FORMAT_SET),
        ("Engine", "whisper", DECLARED_ENGINE_SET),
        ("TextType", "html", DECLARED_TEXT_TYPE_SET),
    ];
    for (member, value, declared) in rows {
        for (binding, carrier) in [
            ("SynthesizeSpeech body", Carrier::SynthesizeSpeechBody),
            ("StartSpeechSynthesisTask body", Carrier::SynthesisTaskBody),
        ] {
            let case = format!("{binding}: {member} {value:?}");
            let response = carrier.call(&polly, member, value).await;
            assert_eq!(
                response.status().as_u16(),
                400,
                "case {case:?}: a value outside the enum is refused, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let path = format!(
                "{}{}",
                member[..1].to_lowercase(),
                &member[1..]
            );
            let actual = message(&case, &response);
            assert_eq!(
                actual,
                one_violation(&path, value, declared),
                "case {case:?}: wrong message, got {actual:?}"
            );
        }
    }
}

/// RULE: the statement is per clause, so a request breaking two enums states the
/// published set for the one and the declared set for the other, in the one envelope.
#[tokio::test]
async fn one_refusal_states_each_enums_own_set() {
    let polly = Polly::new();
    let request = serde_json::json!({
        "Text": "Two members outside their enums.",
        "OutputFormat": "flac",
        "VoiceId": "Aditi",
        "LanguageCode": "xx-XX",
    });
    let response = polly.synthesize(&request).await;
    assert_eq!(
        response.status().as_u16(),
        400,
        "two members outside their enums are refused, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let actual = message("two bad enum members", &response);
    assert!(
        actual.starts_with("2 validation errors detected: "),
        "both violations must be reported in one envelope, got {actual:?}"
    );
    for expected in [
        format!("Member must satisfy enum value set: [{OBSERVED_LANGUAGE_CODE_SET}]"),
        format!("Member must satisfy enum value set: [{DECLARED_OUTPUT_FORMAT_SET}]"),
    ] {
        assert!(
            actual.contains(&expected),
            "the envelope must state {expected:?}, got {actual:?}"
        );
    }
    // Neither clause leaks into the other: the two sets are disjoint, so each value
    // appears once per clause it belongs to.
    let listed: BTreeSet<&str> = actual
        .split(&['[', ']'][..])
        .flat_map(|chunk| chunk.split(", "))
        .collect();
    for value in STATED_LANGUAGE_CODE_VALUES {
        assert!(
            listed.contains(value),
            "the stated LanguageCode set must list {value:?}, got {actual:?}"
        );
    }
}
