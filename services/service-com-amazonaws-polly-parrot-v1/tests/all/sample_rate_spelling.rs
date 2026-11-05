// SPDX-License-Identifier: CC0-1.0

//! Which `SampleRate` strings `SynthesizeSpeech` accepts.
//!
//! RULE: `SampleRate` is a STRING member whose domain the model enumerates per
//! `OutputFormat` as quoted spellings, so a request is accepted only when its
//! `SampleRate` is one of those spellings CHARACTER FOR CHARACTER; any other string —
//! including one that merely denotes the same number (a sign, leading zeros,
//! surrounding whitespace, a decimal point, a non-decimal notation) — is
//! `InvalidSampleRateException` (HTTP 400).
//!
//! Each test checks the rule over a TABLE rather than over one request: the rows are
//! the cross product of every audio `OutputFormat`, every value that format documents
//! and every way of respelling a number, so the accepted set is pinned from both
//! sides. The documented value sets come from the crate's transcription of
//! `SynthesizeSpeechInput$SampleRate` (`ops::synthesize_speech::valid_sample_rates`),
//! which quotes that member's documentation.

use service_com_amazonaws_polly_parrot_v1::ops::synthesize_speech::{
    resolve_sample_rate, valid_sample_rates,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, Polly};
use crate::support::message_slot;

/// The audio formats whose streams this emulator encodes, so a documented rate for one
/// of them reaches a 200. `json` carries speech marks rather than audio and documents no
/// rate, so it has no accepted-rate row here.
const ENCODED_FORMATS: [&str; 6] = ["mp3", "ogg_vorbis", "pcm", "ogg_opus", "mulaw", "alaw"];

/// Every audio format: a `SampleRate` is settled before the output encoding is, so each
/// format refuses a bad spelling with the same 400.
const AUDIO_FORMATS: [&str; 6] = ["mp3", "ogg_vorbis", "pcm", "ogg_opus", "mulaw", "alaw"];

/// The text every request below synthesises, and its billed character count.
const TEXT: &str = "hello";

/// Strings that are NOT the documented spelling `value` but denote the same number, or
/// are `value` with something around it. The rows span the respellings a numeric parser
/// typically accepts (a sign, leading zeros), the ones a lenient reader might trim
/// (whitespace, a newline), other notations for the same quantity (a decimal point, an
/// exponent, hexadecimal, a digit group separator) and a non-ASCII digit.
fn respellings(value: &str) -> Vec<(&'static str, String)> {
    let number: u64 = value
        .parse()
        .unwrap_or_else(|e| panic!("documented value {value:?} is a decimal number: {e}"));
    let (first, rest) = value.split_at(1);
    let fullwidth: String = value
        .chars()
        .map(|digit| {
            // U+FF10 FULLWIDTH DIGIT ZERO is the fullwidth block's '0'.
            char::from_u32(0xFF10 + u32::from(digit as u8 - b'0')).expect("a fullwidth digit")
        })
        .collect();
    vec![
        ("a leading plus", format!("+{value}")),
        ("a leading minus", format!("-{value}")),
        ("one leading zero", format!("0{value}")),
        ("three leading zeros", format!("000{value}")),
        ("a leading space", format!(" {value}")),
        ("a trailing space", format!("{value} ")),
        ("a trailing newline", format!("{value}\n")),
        ("a decimal point", format!("{value}.0")),
        ("an exponent", format!("{value}e0")),
        ("an underscore separator", format!("{first}_{rest}")),
        ("a comma separator", format!("{first},{rest}")),
        ("hexadecimal", format!("0x{number:x}")),
        ("fullwidth digits", fullwidth),
        ("the value twice", format!("{value}{value}")),
    ]
}

/// RULE: a `SampleRate` spelled exactly as its format documents it is accepted, and the
/// stream is encoded at that frequency.
#[tokio::test]
async fn every_documented_spelling_is_accepted_at_its_own_frequency() {
    let polly = Polly::new();
    // The `pcm` streams, by the frequency each was asked for.
    let mut pcm_lengths: Vec<(u32, usize)> = Vec::new();
    for format in ENCODED_FORMATS {
        let documented = valid_sample_rates(format);
        assert!(
            !documented.is_empty(),
            "format {format:?}: the documented value set must not be empty"
        );
        for spelling in documented {
            let case = format!("format {format:?} rate {spelling:?}");
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": TEXT,
                    "VoiceId": "Joanna",
                    "OutputFormat": format,
                    "SampleRate": spelling,
                }))
                .await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: expected success, got body {:?}",
                String::from_utf8_lossy(response.body())
            );
            assert_eq!(
                header(&response, "x-amzn-requestcharacters"),
                TEXT.len().to_string(),
                "case {case:?}: wrong RequestCharacters"
            );

            // `pcm` is signed 16-bit mono, so its byte count fixes the frequency the
            // handler resolved: a spelling must be read as the value it IS, not as some
            // neighbouring rate. The utterance is the same one at every rate, so the
            // stream's length is the frequency times that duration — which is what the
            // ratio between two rates pins, whatever the duration happens to be.
            if format == "pcm" {
                let hz = resolve_sample_rate(format, spelling)
                    .unwrap_or_else(|| panic!("case {case:?}: documented value not resolved"));
                pcm_lengths.push((hz, response.body().len()));
            }
        }
    }
    // Every pcm rate rendered the same text, so the byte counts stand in the ratio of
    // the frequencies: a rate read as a neighbouring value would break it.
    let (base_hz, base_len) = pcm_lengths
        .first()
        .copied()
        .expect("pcm has documented rates");
    for (hz, len) in &pcm_lengths {
        let expected = (base_len as f64) * f64::from(*hz) / f64::from(base_hz);
        assert!(
            ((*len as f64) - expected).abs() <= expected * 0.01 + 8.0,
            "rate {hz} Hz: a {base_hz} Hz stream of {base_len} bytes makes about {expected} \
             bytes here, got {len}"
        );
        assert!(
            *len > 0 && len % 2 == 0,
            "rate {hz} Hz: pcm carries whole 16-bit samples, got {len} bytes"
        );
    }
}

/// RULE: a `SampleRate` that is not one of the format's documented spellings is
/// `InvalidSampleRateException` (HTTP 400) carrying that code's message — including a
/// respelling of a value that format DOES document.
#[tokio::test]
async fn a_respelling_of_a_documented_value_is_refused() {
    let polly = Polly::new();
    let expected_message = errors::invalid_sample_rate().message;
    for format in AUDIO_FORMATS {
        for value in valid_sample_rates(format) {
            for (respelling, rate) in respellings(value) {
                let case = format!("format {format:?} rate {rate:?} ({respelling})");
                let response = polly
                    .synthesize(&serde_json::json!({
                        "Text": TEXT,
                        "VoiceId": "Joanna",
                        "OutputFormat": format,
                        "SampleRate": rate,
                    }))
                    .await;
                assert_eq!(
                    (response.status().as_u16(), error_code(&response)),
                    (400, "InvalidSampleRateException".to_string()),
                    "case {case:?}: expected the invalid-sample-rate refusal, got body {:?}",
                    json(&response)
                );
                assert_eq!(
                    message_slot(&response).and_then(|slot| slot.as_str().map(str::to_string)),
                    expected_message.clone(),
                    "case {case:?}: wrong message"
                );
            }
        }
    }
}

/// RULE: the same respellings are refused for a value the format does NOT document, so
/// nothing about the refusal depends on the number being a documented one, and the
/// empty string is refused too.
#[tokio::test]
async fn a_respelling_of_an_undocumented_value_is_also_refused() {
    let polly = Polly::new();
    // 11025 is a sample rate no `OutputFormat` documents, so neither it nor any
    // respelling of it is in any format's value set.
    let mut rates: Vec<(String, String)> = respellings("11025")
        .into_iter()
        .map(|(respelling, rate)| (respelling.to_string(), rate))
        .collect();
    // 44100 is documented for mp3 and ogg_vorbis only, so pcm refuses it.
    rates.push((
        "the documented value of another format".to_string(),
        "44100".to_string(),
    ));
    rates.push(("the empty string".to_string(), String::new()));
    for (respelling, rate) in rates {
        let case = format!("rate {rate:?} ({respelling})");
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": TEXT,
                "VoiceId": "Joanna",
                "OutputFormat": "pcm",
                "SampleRate": rate,
            }))
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidSampleRateException".to_string()),
            "case {case:?}: expected the invalid-sample-rate refusal, got body {:?}",
            json(&response)
        );
    }
}

/// RULE: `resolve_sample_rate` answers a frequency only for a documented spelling, and
/// that frequency is the number the spelling names — the same rule the wire tests above
/// observe, checked directly over every format the model declares.
#[test]
fn resolution_accepts_exactly_the_documented_spellings() {
    for format in ["mp3", "ogg_vorbis", "pcm", "ogg_opus", "mulaw", "alaw", "json"] {
        for value in valid_sample_rates(format) {
            assert_eq!(
                resolve_sample_rate(format, value).map(|hz| hz.to_string()),
                Some((*value).to_string()),
                "format {format:?} rate {value:?}: expected the frequency it names"
            );
            for (respelling, rate) in respellings(value) {
                assert_eq!(
                    resolve_sample_rate(format, &rate),
                    None,
                    "format {format:?} rate {rate:?} ({respelling}): expected no frequency"
                );
            }
        }
    }
    // `json` documents no rate at all, so its value set is empty and nothing resolves
    // for it.
    assert!(
        valid_sample_rates("json").is_empty(),
        "json documents no SampleRate"
    );
    assert_eq!(
        resolve_sample_rate("json", "16000"),
        None,
        "json has no documented rate to resolve"
    );
}
