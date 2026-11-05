// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH characters of an SSML document `SynthesizeSpeech` bills.
//!
//! RULE: the billed characters of an SSML document are the characters of its text
//! content — everything between the tags and nothing of the markup — and a character is
//! text content whichever spelling it arrives in, so one written as an entity reference
//! (`&amp;`) or as a numeric character reference (`&#120;`, `&#x78;`) bills as the one
//! character it stands for, exactly as a literal one does. `RequestCharacters` reports
//! that count, the documented billed-character limit is measured on it, and a document
//! whose text content is spoken comes back with audio of that speech.
//!
//! The tests check that over a TABLE whose rows differ along the spelling (literal,
//! predefined entity, decimal reference, hexadecimal reference, CDATA, a reference for a
//! markup character, a doubly escaped reference), where the characters sit (flat, around
//! an empty element, inside a nested element, in an attribute value rather than the text),
//! how many there are (none, one, many) and whether the total lands below, on or past the
//! billed limit. Each row's expected count is also checked against the SAME characters
//! sent as plain text, which bills every character it carries — so the expectation is
//! independent of the SSML reader under test.

use service_com_amazonaws_polly_parrot_v1::ops::synthesize_speech::{
    MAX_BILLED_CHARACTERS, MAX_TOTAL_CHARACTERS,
};

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// The `pcm` rate the stream-length rows use; `pcm` carries a fixed number of bytes per
/// millisecond, so its body length is a direct reading of the duration the count implies.
const PCM_RATE: &str = "16000";

/// One spelling of some text content.
///
/// `inner` is the markup between `<speak>` and `</speak>`; `plain` is the same
/// characters as plain text, which bills every character it carries; `billed` is how
/// many characters of text content the markup holds, counted by hand.
struct Row {
    case: &'static str,
    inner: String,
    plain: String,
    billed: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "flat literal text",
            inner: "hello world".to_string(),
            plain: "hello world".to_string(),
            billed: 11,
        },
        Row {
            case: "no text content at all",
            inner: String::new(),
            plain: String::new(),
            billed: 0,
        },
        Row {
            case: "a predefined entity reference between literal characters",
            inner: "a&amp;b".to_string(),
            plain: "a&b".to_string(),
            billed: 3,
        },
        Row {
            case: "nothing but entity references",
            inner: "&amp;&amp;&amp;".to_string(),
            plain: "&&&".to_string(),
            billed: 3,
        },
        Row {
            case: "decimal character references",
            inner: "&#120;&#120;&#120;&#120;&#120;".to_string(),
            plain: "xxxxx".to_string(),
            billed: 5,
        },
        Row {
            case: "hexadecimal character references",
            inner: "&#x78;&#x79;".to_string(),
            plain: "xy".to_string(),
            billed: 2,
        },
        Row {
            case: "a character reference outside ASCII",
            inner: "&#233;t&#233;".to_string(),
            plain: "été".to_string(),
            billed: 3,
        },
        Row {
            case: "references standing for the markup delimiters",
            inner: "&lt;tag&gt;".to_string(),
            plain: "<tag>".to_string(),
            billed: 5,
        },
        Row {
            case: "a doubly escaped reference spells out its own text",
            inner: "&amp;amp;".to_string(),
            plain: "&amp;".to_string(),
            billed: 5,
        },
        Row {
            case: "references on both sides of an empty element",
            inner: "&lt;<break time=\"1s\"/>&gt;".to_string(),
            plain: "<>".to_string(),
            billed: 2,
        },
        Row {
            case: "a reference inside a nested element, with text after it",
            inner: "<prosody rate=\"slow\">&amp;x</prosody>y".to_string(),
            plain: "&xy".to_string(),
            billed: 3,
        },
        Row {
            case: "a reference beside a CDATA section, which carries its characters verbatim",
            inner: "&amp;<![CDATA[&amp;]]>".to_string(),
            plain: "&&amp;".to_string(),
            billed: 6,
        },
        Row {
            // An attribute value is markup, not text content, so a reference in one
            // bills nothing however it is spelled.
            case: "a reference in an attribute value rather than in the text",
            inner: "<sub alias=\"a&amp;b\">x</sub>".to_string(),
            plain: "x".to_string(),
            billed: 1,
        },
        Row {
            case: "many references, so the implied duration is well past the floor",
            inner: "&#120;".repeat(75),
            plain: "x".repeat(75),
            billed: 75,
        },
    ]
}

/// One `pcm` synthesis of `text`.
async fn synthesize(polly: &Polly, text: &str, text_type: &str) -> http::Response<bytes::Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": text,
            "TextType": text_type,
            "VoiceId": "Joanna",
            "OutputFormat": "pcm",
            "SampleRate": PCM_RATE,
        }))
        .await
}

/// RULE: `RequestCharacters` is the count of the document's text content, whichever
/// spelling each character arrives in, and a document whose text content is spoken comes
/// back with the audio of that speech.
#[tokio::test]
async fn references_bill_as_the_characters_they_stand_for() {
    let polly = Polly::new();
    // Every stream ends in a short silence, so even a document with nothing to say
    // carries a few bytes; what distinguishes a spoken document is that it carries much
    // more than that.
    let empty = synthesize(&polly, "<speak></speak>", "ssml").await;
    assert_eq!(
        empty.status().as_u16(),
        200,
        "an empty document is still synthesized, got {:?}",
        json(&empty)
    );
    let floor_bytes = empty.body().len();
    for row in rows() {
        let ssml = format!("<speak>{}</speak>", row.inner);
        let case = row.case;
        let response = synthesize(&polly, &ssml, "ssml").await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected the document to be synthesized, got {:?}",
            json(&response)
        );
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            row.billed.to_string(),
            "case {case:?}: expected {} billed characters for {ssml:?}",
            row.billed
        );

        // The same characters as plain text bill the same.
        let as_plain = synthesize(&polly, &row.plain, "text").await;
        assert_eq!(
            (
                as_plain.status().as_u16(),
                header(&as_plain, "x-amzn-requestcharacters")
            ),
            (200, row.billed.to_string()),
            "case {case:?}: the plain-text form {:?} must bill the same {} characters",
            row.plain,
            row.billed
        );
        // A document whose text content carries words comes back with their speech; one
        // whose text content is nothing, or carries no word at all, has nothing to say
        // and stays at the closing silence. Which characters are BILLED and which are
        // SPOKEN are different questions: punctuation bills and says nothing.
        let body = response.body().len();
        let speakable = row.plain.chars().any(char::is_alphanumeric);
        let silent_markup = row.inner.chars().all(|ch| ch != '<');
        if speakable {
            assert!(
                body > floor_bytes,
                "case {case:?}: the words of {:?} must carry more than the \
                 {floor_bytes}-byte floor, got {body}",
                row.plain
            );
            assert!(
                body % 2 == 0,
                "case {case:?}: pcm carries whole 16-bit samples, got {body} bytes"
            );
        } else if silent_markup {
            assert_eq!(
                body, floor_bytes,
                "case {case:?}: {:?} carries no word to speak, so it is the \
                 {floor_bytes}-byte floor, got {body}",
                row.plain
            );
        }
    }
}

/// RULE: the documented billed-character limit is measured on the characters the text
/// content resolves to, so a document passes it exactly when those characters pass it —
/// the spelling neither hides them nor adds any.
#[tokio::test]
async fn the_billed_limit_is_measured_on_the_resolved_characters() {
    let polly = Polly::new();
    // "the limit for input text is a maximum of 6000 characters total, of which no more
    // than 3000 can be billed characters" (`TextLengthExceededException`).
    assert_eq!(
        (MAX_TOTAL_CHARACTERS, MAX_BILLED_CHARACTERS),
        (6000, 3000),
        "the documented limits are 6000 total and 3000 billed characters"
    );
    let quota = MAX_BILLED_CHARACTERS;
    // case, the markup between the tags, the characters of text content it holds.
    // Every row stays well inside the 6000-character total so that it is the BILLED
    // limit each one lands against.
    let table: [(&str, String, usize); 6] = [
        (
            "literal text one under the billed limit",
            "x".repeat(quota - 1),
            quota - 1,
        ),
        ("literal text at the billed limit", "x".repeat(quota), quota),
        (
            "literal text one past the billed limit",
            "x".repeat(quota + 1),
            quota + 1,
        ),
        (
            "literal text plus two entity references, one under the limit",
            format!("{}{}", "x".repeat(quota - 3), "&amp;".repeat(2)),
            quota - 1,
        ),
        (
            "literal text plus two entity references, at the limit",
            format!("{}{}", "x".repeat(quota - 2), "&amp;".repeat(2)),
            quota,
        ),
        (
            "literal text plus two entity references, one past the limit",
            format!("{}{}", "x".repeat(quota - 1), "&#120;".repeat(2)),
            quota + 1,
        ),
    ];
    for (case, inner, billed) in table {
        let ssml = format!("<speak>{inner}</speak>");
        assert!(
            ssml.chars().count() <= MAX_TOTAL_CHARACTERS,
            "case {case:?}: the row must stay inside the total-character limit, got {}",
            ssml.chars().count()
        );
        // `mp3` rather than `pcm`: the rows near the limit imply minutes of audio, and
        // mp3 carries them in a fraction of the bytes.
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": ssml,
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        if billed > MAX_BILLED_CHARACTERS {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "TextLengthExceededException".to_string()),
                "case {case:?} ({billed} billed characters): expected the length refusal, \
                 got {:?}",
                json(&response)
            );
        } else {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?} ({billed} billed characters): expected success, got {:?}",
                json(&response)
            );
            assert_eq!(
                header(&response, "x-amzn-requestcharacters"),
                billed.to_string(),
                "case {case:?}: expected {billed} billed characters"
            );
        }
    }
}

/// RULE (the same rule, where a reference stands for nothing): a reference the document
/// defines no replacement for is not well-formed text content, so the document is
/// `InvalidSsmlException` rather than a document whose unresolvable characters bill as
/// zero.
#[tokio::test]
async fn a_reference_standing_for_nothing_is_invalid_ssml() {
    let polly = Polly::new();
    // case, the markup between the tags, whether Amazon Polly can read it.
    let table: [(&str, &str, bool); 6] = [
        ("a resolvable entity reference", "a&amp;b", true),
        ("a resolvable character reference", "a&#98;c", true),
        ("an entity nothing declares", "a&nosuchentity;b", false),
        ("an entity nothing declares, alone", "&undefined;", false),
        ("a character reference with no digits", "a&#;b", false),
        ("a character reference past the code-point range", "a&#xFFFFFF;b", false),
    ];
    for (case, inner, readable) in table {
        let ssml = format!("<speak>{inner}</speak>");
        let response = synthesize(&polly, &ssml, "ssml").await;
        if readable {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: expected success, got {:?}",
                json(&response)
            );
        } else {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidSsmlException".to_string()),
                "case {case:?}: expected the SSML refusal, got {:?}",
                json(&response)
            );
        }
    }
}
