// SPDX-License-Identifier: CC0-1.0

//! What a `LanguageCode` does to a synthesis request, and what it does not.
//!
//! RULE: `LanguageCode` selects which language a BILINGUAL voice speaks and states no
//! requirement on the voice — "Optional language code … This is only necessary if using
//! a bilingual voice, such as Aditi, which can be used for either Indian English
//! (en-IN) or Hindi (hi-IN)" (`smithy_query(query="doc",shape="SynthesizeSpeechInput",
//! member="LanguageCode")`, and the same prose on the task and stream inputs). So any
//! value the model's enum admits is SERVED whatever voice the request names: the
//! request is synthesized with the voice it named, and a language that voice does not
//! speak is not a refusal. Only a value OUTSIDE the enum is refused, by the request
//! boundary (see `crate::stated_enum_value_set`).
//!
//! The tables span the class the rule covers: every modelled `LanguageCode` rather
//! than the one an issue quotes, a monolingual and a bilingual voice, each of the three
//! operations that take the member, and — for the asynchronous one — the value read
//! back off the task record.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::start_speech_synthesis_stream::{
    MODELED_PATH as STREAM_PATH, STREAMING_ENGINE,
};
use service_com_amazonaws_polly_parrot_v1::ops::types::LANGUAGE_CODE_VALUES;

use crate::lexicons_and_speech::{header, json, Polly};

/// The region every call here is signed for.
const REGION: &str = "us-east-1";

/// The bucket the asynchronous path writes its output to.
const OUTPUT_BUCKET: &str = "polly-output";

/// Voices the seeded catalog records, with the language each one speaks: a monolingual
/// voice, the bilingual voice the model's own documentation names, and a voice of a
/// third language. Source: the catalog `DescribeVoices` serves
/// (`crate::voice_catalog` walks it in full).
const VOICES: [(&str, &str); 3] = [
    ("Joanna", "en-US"),
    ("Aditi", "en-IN"),
    ("Marlene", "de-DE"),
];

/// One `SynthesizeSpeech` call naming `voice` and `language`.
async fn synthesize(polly: &Polly, voice: &str, language: &str) -> http::Response<Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": "The language only chooses a bilingual voice's language.",
            "OutputFormat": "mp3",
            "VoiceId": voice,
            "LanguageCode": language,
        }))
        .await
}

/// RULE, for the synchronous path: every modelled `LanguageCode` is served for every
/// voice, whether or not the catalog records that voice as speaking it.
#[tokio::test]
async fn synthesize_speech_serves_every_modelled_language_for_every_voice() {
    let polly = Polly::new();
    for (voice, own_language) in VOICES {
        for language in LANGUAGE_CODE_VALUES {
            let case = format!("voice {voice:?} ({own_language}) asked for {language:?}");
            let response = synthesize(&polly, voice, language).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: a modelled language is served, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            // The documented media type of the `OutputFormat` the request named, so
            // what comes back is that format's audio rather than an empty success.
            assert_eq!(
                header(&response, "content-type"),
                "audio/mpeg",
                "case {case:?}: wrong ContentType, got {:?}",
                header(&response, "content-type")
            );
            assert!(
                !response.body().is_empty(),
                "case {case:?}: the answer carries the synthesized audio"
            );
        }
    }
}

/// RULE, for the asynchronous path: the same languages are accepted, and the task
/// record reports the language the request named — the member is recorded, not checked.
#[tokio::test]
async fn a_task_records_the_language_it_was_started_with() {
    let polly = Polly::new();
    // The rows differ along the voice and along which language it is asked for: its
    // own, a bilingual voice's additional language, and languages the voice does not
    // speak at all (including one whose only voice is another).
    let rows = [
        ("Joanna", "en-US"),
        ("Joanna", "hi-IN"),
        ("Joanna", "arb"),
        ("Aditi", "en-IN"),
        ("Aditi", "hi-IN"),
        ("Aditi", "de-DE"),
        ("Marlene", "en-SG"),
        ("Marlene", "cmn-CN"),
    ];
    for (voice, language) in rows {
        let case = format!("task for voice {voice:?} in {language:?}");
        let started = polly
            .call_in_region(
                REGION,
                "POST",
                "/v1/synthesisTasks",
                &serde_json::json!({
                    "Text": "A task carries the language it was started with.",
                    "OutputFormat": "mp3",
                    "VoiceId": voice,
                    "LanguageCode": language,
                    "OutputS3BucketName": OUTPUT_BUCKET,
                })
                .to_string(),
            )
            .await;
        assert_eq!(
            started.status().as_u16(),
            200,
            "case {case:?}: a modelled language is accepted, got {} {:?}",
            started.status(),
            String::from_utf8_lossy(started.body())
        );
        let task = json(&started);
        assert_eq!(
            task["SynthesisTask"]["LanguageCode"].as_str(),
            Some(language),
            "case {case:?}: the task must report the language it was started with, got {task}"
        );
        let task_id = task["SynthesisTask"]["TaskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the started task has an id, got {task}"))
            .to_string();

        // Read back through the implemented reader: the record, not the request, is
        // what a later caller sees.
        let read_back = polly
            .call("GET", &format!("/v1/synthesisTasks/{task_id}"), "")
            .await;
        assert_eq!(
            read_back.status().as_u16(),
            200,
            "case {case:?}: the task must read back, got {} {:?}",
            read_back.status(),
            String::from_utf8_lossy(read_back.body())
        );
        let reported = json(&read_back);
        assert_eq!(
            (
                reported["SynthesisTask"]["LanguageCode"].as_str(),
                reported["SynthesisTask"]["VoiceId"].as_str()
            ),
            (Some(language), Some(voice)),
            "case {case:?}: the read must report the recorded voice and language, got {reported}"
        );
    }
}

/// RULE, for the streaming path: its own copy of the member carries the same reading,
/// so a language the chosen voice does not speak opens the stream too.
#[tokio::test]
async fn a_stream_opens_for_a_language_its_voice_does_not_speak() {
    let polly = Polly::new();
    // `Joanna` is a voice the catalog records on the streaming engine; the languages
    // below are hers and three she does not speak.
    for language in ["en-US", "hi-IN", "de-DE", "en-SG"] {
        let case = format!("stream for Joanna in {language:?}");
        let headers: Vec<(&str, &str)> = vec![
            ("content-type", "application/vnd.amazon.eventstream"),
            ("x-amzn-Engine", STREAMING_ENGINE),
            ("x-amzn-OutputFormat", "pcm"),
            ("x-amzn-VoiceId", "Joanna"),
            ("x-amzn-LanguageCode", language),
        ];
        // No action event: the header half is what this rule is about, and an empty
        // input stream is not a refusal.
        let response = polly
            .call_bytes_in_region(REGION, "POST", STREAM_PATH, &headers, Vec::new())
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a modelled language opens the stream, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "content-type"),
            "application/vnd.amazon.eventstream",
            "case {case:?}: the answer is an event stream, got {:?}",
            header(&response, "content-type")
        );
        // The stream it opened closes in band, which is the frame an SDK decoder
        // reads: a refusal carries no such frame.
        assert!(
            String::from_utf8_lossy(response.body()).contains("StreamClosedEvent"),
            "case {case:?}: the served stream must close in band, got {:?}",
            String::from_utf8_lossy(response.body())
        );
    }
}
