// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `SynthesizeSpeech` accepts as SSML.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs that
//! spans the class the rule covers — every published tag rather than one, an
//! out-of-domain value for every attribute that publishes a value set rather than one,
//! and both ends of the one published numeric bound plus a value past it. The single
//! request an issue happens to quote is one row.
//!
//! The accepted and refused tags, attributes and values come from the service's
//! transcription of Amazon Polly's supported-SSML-tags documentation
//! (`ops::synthesize_speech::ssml::SUPPORTED_ELEMENTS`), which the first test below
//! walks so no published tag can go untested.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::synthesize_speech::ssml;
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// One `SynthesizeSpeech` call with `text` as SSML.
async fn synthesize_ssml(polly: &Polly, text: &str) -> http::Response<Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": text,
            "TextType": "ssml",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        }))
        .await
}

/// The `message` an error document carries.
fn error_message(response: &http::Response<Bytes>) -> String {
    let body = json(response);
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(|m| m.as_str()) {
            return message.to_string();
        }
    }
    "<no message>".to_string()
}

/// Assert that `text` is refused as SSML: HTTP 400 `InvalidSsmlException`, carrying the
/// one message that error answers with.
async fn assert_refused(polly: &Polly, case: &str, text: &str) {
    let response = synthesize_ssml(polly, text).await;
    assert_eq!(
        (
            response.status().as_u16(),
            error_code(&response),
            error_message(&response)
        ),
        (
            400,
            "InvalidSsmlException".to_string(),
            errors::INVALID_SSML.to_string()
        ),
        "case {case:?} ({text:?}): expected a 400 InvalidSsmlException, got status {} body {:?}",
        response.status().as_u16(),
        String::from_utf8_lossy(response.body())
    );
}

/// Assert that `text` is synthesized, billing `billed` characters.
async fn assert_accepted(polly: &Polly, case: &str, text: &str, billed: usize) {
    let response = synthesize_ssml(polly, text).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?} ({text:?}): expected success, got body {:?}",
        String::from_utf8_lossy(response.body())
    );
    let actual = response
        .headers()
        .get("x-amzn-requestcharacters")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        actual,
        billed.to_string(),
        "case {case:?} ({text:?}): expected {billed} billed characters, got {actual:?}"
    );
}

/// RULE: every tag Amazon Polly publishes is accepted, with the attributes and values
/// that tag publishes, and the markup is not billed — only the text content is.
#[tokio::test]
async fn every_published_ssml_tag_is_accepted() {
    let polly = Polly::new();
    // One row per published tag, each using that tag's documented attributes. The
    // billed count is the text content, since "SSML tags are not counted as billed
    // characters".
    let table: &[(&str, &str, usize)] = &[
        ("speak", "<speak>hello</speak>", 5),
        (
            "speak declarations",
            "<speak version=\"1.1\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
             xml:lang=\"en-US\">hello</speak>",
            5,
        ),
        (
            "break",
            "<speak>a<break strength=\"x-strong\" time=\"500ms\"/>b</speak>",
            2,
        ),
        (
            "emphasis",
            "<speak><emphasis level=\"reduced\">hi</emphasis></speak>",
            2,
        ),
        (
            "lang",
            "<speak><lang xml:lang=\"fr-FR\">bonjour</lang></speak>",
            7,
        ),
        ("mark", "<speak>a<mark name=\"here\"/>b</speak>", 2),
        ("p", "<speak><p>hi</p></speak>", 2),
        (
            "phoneme",
            "<speak><phoneme alphabet=\"x-sampa\" ph=\"pI.kAn\">pecan</phoneme></speak>",
            5,
        ),
        (
            "prosody",
            "<speak><prosody volume=\"+6dB\" rate=\"80%\" pitch=\"x-low\" \
             amazon:max-duration=\"2500ms\">hi</prosody></speak>",
            2,
        ),
        ("s", "<speak><s>hi</s></speak>", 2),
        (
            "say-as",
            "<speak><say-as interpret-as=\"date\" format=\"ymd\">20260101</say-as></speak>",
            8,
        ),
        (
            "sub",
            "<speak><sub alias=\"magnesium\">Mg</sub></speak>",
            2,
        ),
        ("w", "<speak><w role=\"amazon:VBD\">read</w></speak>", 4),
        (
            "audio",
            "<speak><audio src=\"https://example.com/a.mp3\"/>hi</speak>",
            2,
        ),
        (
            "amazon:effect",
            "<speak><amazon:effect name=\"whispered\" phonation=\"soft\" \
             vocal-tract-length=\"+15%\">hi</amazon:effect></speak>",
            2,
        ),
        (
            "amazon:domain",
            "<speak><amazon:domain name=\"news\">hi</amazon:domain></speak>",
            2,
        ),
        (
            "amazon:breath",
            "<speak>a<amazon:breath duration=\"x-long\" volume=\"x-soft\"/>b</speak>",
            2,
        ),
        (
            "amazon:auto-breaths",
            "<speak><amazon:auto-breaths duration=\"short\" volume=\"loud\" \
             frequency=\"x-high\">hi</amazon:auto-breaths></speak>",
            2,
        ),
    ];
    for (case, text, billed) in table {
        assert_accepted(&polly, case, text, *billed).await;
    }

    // The table must span the whole vocabulary, so a tag added to it later cannot go
    // untested.
    for element in ssml::SUPPORTED_ELEMENTS {
        assert!(
            table.iter().any(|(case, _, _)| *case == element.name),
            "case {:?}: the published tag has no row in this table",
            element.name
        );
    }
}

/// RULE: an element Amazon Polly does not publish is outside the SSML it accepts, so a
/// document carrying one is `InvalidSsmlException` — whatever else is well-formed about
/// it.
#[tokio::test]
async fn an_unpublished_element_is_refused() {
    let polly = Polly::new();
    let table: &[(&str, &str)] = &[
        // W3C SSML defines `<voice>`; Amazon Polly does not publish it.
        ("voice", "<speak><voice name=\"Kevin\">hello</voice></speak>"),
        ("invented", "<speak><bogus>hello</bogus></speak>"),
        ("empty invented", "<speak><bogus/>hello</speak>"),
        ("misspelled", "<speak><brek time=\"1s\"/>hello</speak>"),
        // A vendor-prefixed tag is published WITH its prefix, so only the published
        // ones count.
        (
            "invented amazon tag",
            "<speak><amazon:whisper>hello</amazon:whisper></speak>",
        ),
        (
            "unprefixed vendor tag",
            "<speak><effect name=\"whispered\">hello</effect></speak>",
        ),
        ("nested deep", "<speak><p><s>a <b>bold</b></s></p></speak>"),
        // Not `<speak>` at all: the document element rule, still in force.
        ("wrong document element", "<voice>hello</voice>"),
    ];
    for (case, text) in table {
        assert_refused(&polly, case, text).await;
    }
}

/// RULE: a published tag accepts only the attributes published for it; any other
/// attribute on it is `InvalidSsmlException`.
#[tokio::test]
async fn an_unpublished_attribute_is_refused() {
    let polly = Polly::new();
    let table: &[(&str, &str)] = &[
        ("break", "<speak><break pause=\"1s\"/>hi</speak>"),
        ("p", "<speak><p align=\"left\">hi</p></speak>"),
        ("s", "<speak><s level=\"strong\">hi</s></speak>"),
        (
            "say-as",
            "<speak><say-as interpret-as=\"date\" style=\"long\">20260101</say-as></speak>",
        ),
        (
            "sub",
            "<speak><sub alias=\"magnesium\" lang=\"en-US\">Mg</sub></speak>",
        ),
        (
            "prosody",
            "<speak><prosody contour=\"(0%,+20Hz)\">hi</prosody></speak>",
        ),
        // An attribute published for ANOTHER tag is still not published for this one.
        (
            "borrowed from another tag",
            "<speak><emphasis strength=\"strong\">hi</emphasis></speak>",
        ),
        (
            "amazon:domain",
            "<speak><amazon:domain name=\"news\" level=\"strong\">hi</amazon:domain></speak>",
        ),
    ];
    for (case, text) in table {
        assert_refused(&polly, case, text).await;
    }
}

/// RULE: an attribute whose published value set is a list of keywords accepts exactly
/// those keywords; a value outside the list is `InvalidSsmlException`.
#[tokio::test]
async fn a_value_outside_an_attributes_published_set_is_refused() {
    let polly = Polly::new();
    // One refused value per keyword attribute in the vocabulary, including the tags
    // the issue did not cite.
    let table: &[(&str, &str)] = &[
        (
            "prosody rate",
            "<speak><prosody rate=\"supersonic\">hello</prosody></speak>",
        ),
        (
            "prosody volume",
            "<speak><prosody volume=\"deafening\">hello</prosody></speak>",
        ),
        (
            "prosody pitch",
            "<speak><prosody pitch=\"squeaky\">hello</prosody></speak>",
        ),
        (
            "break strength",
            "<speak><break strength=\"xx-strong\"/>hello</speak>",
        ),
        (
            "emphasis level",
            "<speak><emphasis level=\"extreme\">hello</emphasis></speak>",
        ),
        (
            "phoneme alphabet",
            "<speak><phoneme alphabet=\"klingon\" ph=\"x\">hello</phoneme></speak>",
        ),
        (
            "say-as interpret-as",
            "<speak><say-as interpret-as=\"runes\">hello</say-as></speak>",
        ),
        (
            "say-as format",
            "<speak><say-as interpret-as=\"date\" format=\"dmyy\">010126</say-as></speak>",
        ),
        ("w role", "<speak><w role=\"amazon:ZZ\">read</w></speak>"),
        (
            "amazon:effect name",
            "<speak><amazon:effect name=\"shouted\">hello</amazon:effect></speak>",
        ),
        (
            "amazon:effect phonation",
            "<speak><amazon:effect phonation=\"hard\">hello</amazon:effect></speak>",
        ),
        (
            "amazon:domain name",
            "<speak><amazon:domain name=\"sports\">hello</amazon:domain></speak>",
        ),
        (
            "amazon:breath duration",
            "<speak><amazon:breath duration=\"eternal\"/>hello</speak>",
        ),
        (
            "amazon:breath volume",
            "<speak><amazon:breath volume=\"xx-loud\"/>hello</speak>",
        ),
        (
            "amazon:auto-breaths frequency",
            "<speak><amazon:auto-breaths frequency=\"constant\">hi</amazon:auto-breaths></speak>",
        ),
        (
            "lang xml:lang",
            "<speak><lang xml:lang=\"not a language\">hello</lang></speak>",
        ),
    ];
    for (case, text) in table {
        assert_refused(&polly, case, text).await;
    }
}

/// RULE: an attribute whose published value set is numeric accepts a value of the
/// published form, inside the published bound, and refuses anything else — `<break
/// time>` up to its published ten-second cap, `<prosody rate>` at or above its
/// published minimum percentage.
#[tokio::test]
async fn a_numeric_attribute_is_bounded_where_the_documentation_bounds_it() {
    let polly = Polly::new();

    // `<break time>`: both spellings of the bound, the values inside it, and the
    // first values past it.
    let accepted_breaks: &[&str] = &["0ms", "1ms", "500ms", "1s", "1.5s", "10s", "10000ms"];
    for time in accepted_breaks {
        assert_accepted(
            &polly,
            &format!("break time {time}"),
            &format!("<speak>a<break time=\"{time}\"/>b</speak>"),
            2,
        )
        .await;
    }
    assert_eq!(
        ssml::MAX_BREAK_MS,
        10_000.0,
        "the published break cap must be ten seconds"
    );
    let refused_breaks: &[&str] = &[
        // Past the cap, in both spellings, including the first value past it.
        "10001ms", "10.5s", "30s", // Not a duration at all: no unit, a wrong unit, prose.
        "1", "500", "1sec", "1 s", "x-long", "",
    ];
    for time in refused_breaks {
        assert_refused(
            &polly,
            &format!("break time {time}"),
            &format!("<speak>a<break time=\"{time}\"/>b</speak>"),
        )
        .await;
    }

    // `<prosody rate>`: the published minimum, values above it, and the values below.
    assert_eq!(
        ssml::MIN_RATE_PERCENT,
        20.0,
        "the published rate minimum must be twenty percent"
    );
    for rate in ["20%", "21%", "100%", "250%"] {
        assert_accepted(
            &polly,
            &format!("prosody rate {rate}"),
            &format!("<speak><prosody rate=\"{rate}\">hi</prosody></speak>"),
            2,
        )
        .await;
    }
    for rate in ["19%", "0%", "100", "%", "100 %"] {
        assert_refused(
            &polly,
            &format!("prosody rate {rate}"),
            &format!("<speak><prosody rate=\"{rate}\">hi</prosody></speak>"),
        )
        .await;
    }

    // `<prosody volume>`: a decibel offset is the published alternative to a keyword.
    for volume in ["+6dB", "-6dB", "6dB"] {
        assert_accepted(
            &polly,
            &format!("prosody volume {volume}"),
            &format!("<speak><prosody volume=\"{volume}\">hi</prosody></speak>"),
            2,
        )
        .await;
    }
    for volume in ["6", "6db", "+6 dB", "loudest"] {
        assert_refused(
            &polly,
            &format!("prosody volume {volume}"),
            &format!("<speak><prosody volume=\"{volume}\">hi</prosody></speak>"),
        )
        .await;
    }
}

/// RULE: the vocabulary check is a refusal of the REQUEST, so it happens whatever else
/// the request asks for, and plain text is not SSML and is never checked against it.
#[tokio::test]
async fn the_vocabulary_check_is_independent_of_the_rest_of_the_request() {
    let polly = Polly::new();
    const OUTSIDE: &str = "<speak><voice name=\"Kevin\">hello</voice></speak>";

    for format in ["mp3", "pcm", "ogg_opus", "mulaw", "alaw"] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": OUTSIDE,
                "TextType": "ssml",
                "VoiceId": "Matthew",
                "OutputFormat": format,
            }))
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidSsmlException".to_string()),
            "format {format:?}: expected the SSML refusal, got body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }

    // The same text as PLAIN TEXT is just text: it is billed whole and synthesized,
    // because the vocabulary applies to `ssml` input only.
    for text_type in [serde_json::json!("text"), serde_json::Value::Null] {
        let mut request = serde_json::json!({
            "Text": OUTSIDE,
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        });
        if !text_type.is_null() {
            request["TextType"] = text_type.clone();
        }
        let response = polly.synthesize(&request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "TextType {text_type:?}: plain text must not be checked as SSML, got body {:?}",
            String::from_utf8_lossy(response.body())
        );
        let billed = response
            .headers()
            .get("x-amzn-requestcharacters")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            billed,
            OUTSIDE.chars().count().to_string(),
            "TextType {text_type:?}: plain text bills every character, got {billed:?}"
        );
    }
}
