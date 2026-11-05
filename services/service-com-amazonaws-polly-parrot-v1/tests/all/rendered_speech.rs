// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the audio `SynthesizeSpeech` answers with carries.
//!
//! RULE: the stream is RENDERED from what the request asks to be spoken — the text, the
//! lexemes of the lexicons it names and the SSML it carries — so the samples are a signal
//! that follows the text, every modelled `OutputFormat` carries that signal in its own
//! encoding, and every modelled `SpeechMarkType` describes the same utterance. A change
//! to what is spoken is a change to what comes back.
//!
//! Each test checks the rule over a TABLE spanning the dimension it generalises over:
//! several texts, every sample rate a format documents, both voice genders, the four mark
//! types, and the SSML controls that change how long the speech lasts. The one request an
//! issue quotes is one row.

use std::collections::BTreeSet;

use crate::lexicons_and_speech::{header, json, Polly};

/// The PLS namespace every lexicon document below is bound to.
const PLS: &str = "http://www.w3.org/2005/01/pronunciation-lexicon";

/// A lexicon document aliasing `grapheme` to `replacement`.
fn alias_lexicon(grapheme: &str, replacement: &str) -> String {
    format!(
        "<lexicon version=\"1.0\" xmlns=\"{PLS}\" alphabet=\"ipa\" xml:lang=\"en-US\">\
         <lexeme><grapheme>{grapheme}</grapheme><alias>{replacement}</alias></lexeme></lexicon>"
    )
}

/// One synthesis request, as a JSON body.
fn request(text: &str, format: &str, extra: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Text": text,
        "VoiceId": "Joanna",
        "OutputFormat": format,
    });
    for (member, value) in extra {
        body[*member] = value.clone();
    }
    body
}

/// The bytes one request answers with, asserting it was served.
async fn audio(polly: &Polly, case: &str, body: &serde_json::Value) -> Vec<u8> {
    let response = polly.synthesize(body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: expected audio, got {:?}",
        json(&response)
    );
    response.body().to_vec()
}

/// The length of the `pcm` stream `text` is spoken as with the lexicons `names`.
async fn lexicon_length(polly: &Polly, text: &str, names: &[&str]) -> usize {
    let extra: Vec<(&str, serde_json::Value)> = if names.is_empty() {
        Vec::new()
    } else {
        vec![("LexiconNames", serde_json::json!(names))]
    };
    audio(
        polly,
        &format!("text {text:?} with lexicons {names:?}"),
        &request(text, "pcm", &extra),
    )
    .await
    .len()
}

/// The length of the 16 kHz `pcm` stream the SSML document `markup` is spoken as.
async fn ssml_length(polly: &Polly, markup: &str) -> usize {
    audio(polly, markup, &ssml_request(markup)).await.len()
}

/// One `ssml` synthesis request at 16 kHz `pcm`, where the stream's length is a direct
/// reading of how long the speech lasts.
fn ssml_request(markup: &str) -> serde_json::Value {
    request(
        markup,
        "pcm",
        &[
            ("TextType", serde_json::json!("ssml")),
            ("SampleRate", serde_json::json!("16000")),
        ],
    )
}

/// RULE: a `pcm` stream is signed 16-bit mono samples of a SIGNAL — not silence — so its
/// samples are not all zero and they take many different values.
#[tokio::test]
async fn pcm_carries_a_signal_rather_than_silence() {
    let polly = Polly::new();
    // The rows span the text (one word, a sentence, several sentences, a number, a
    // non-ASCII word), the documented `pcm` rates and both voice genders, because the
    // rule is about every request rather than one.
    let texts = [
        "Testing raw pcm output from Amazon Polly.",
        "Hi.",
        "One. Two. Three.",
        "Call 1234 today",
        "Übersetze dieses Wort",
    ];
    // "Valid values for pcm are \"8000\" and \"16000\"" (`SynthesizeSpeechInput$SampleRate`).
    for rate in ["8000", "16000"] {
        for voice in ["Joanna", "Matthew"] {
            for text in texts {
                let case = format!("text {text:?} at {rate} Hz in {voice}");
                let body = audio(
                    &polly,
                    &case,
                    &request(
                        text,
                        "pcm",
                        &[
                            ("SampleRate", serde_json::json!(rate)),
                            ("VoiceId", serde_json::json!(voice)),
                        ],
                    ),
                )
                .await;
                assert!(
                    body.len() % 2 == 0 && !body.is_empty(),
                    "case {case:?}: pcm carries whole 16-bit samples, got {} bytes",
                    body.len()
                );
                let samples: Vec<i16> = body
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| i16::from_le_bytes(*pair))
                    .collect();
                assert!(
                    samples.iter().any(|sample| *sample != 0),
                    "case {case:?}: the payload is all-zero silence, not rendered speech"
                );
                let distinct: BTreeSet<i16> = samples.iter().copied().collect();
                assert!(
                    distinct.len() > 100,
                    "case {case:?}: a rendered waveform takes many sample values, got {} \
                     across {} samples",
                    distinct.len(),
                    samples.len()
                );
                let peak = samples
                    .iter()
                    .map(|sample| i32::from(*sample).abs())
                    .max()
                    .unwrap_or_default();
                assert!(
                    peak > i32::from(i16::MAX) / 4,
                    "case {case:?}: the signal must use the format's range, peak {peak}"
                );
            }
        }
    }
}

/// RULE: the companded formats carry the same signal through their own law, so their
/// bytes take many values — and the two laws are different codes, so one is not the
/// other's bytes.
#[tokio::test]
async fn the_companded_formats_carry_the_signal_through_their_own_law() {
    let polly = Polly::new();
    let texts = ["Testing mulaw and alaw output.", "Go!", "Seven, eight, nine."];
    for text in texts {
        let mut encoded: Vec<(&str, Vec<u8>)> = Vec::new();
        // "Valid value for mu-law and a-law is \"8000\"".
        for format in ["mulaw", "alaw"] {
            let case = format!("text {text:?} as {format}");
            let body = audio(
                &polly,
                &case,
                &request(text, format, &[("SampleRate", serde_json::json!("8000"))]),
            )
            .await;
            let distinct: BTreeSet<u8> = body.iter().copied().collect();
            assert!(
                distinct.len() > 10,
                "case {case:?}: companded speech takes many byte values, got {:?}",
                distinct
            );
            // Silence is one repeated idle code in both laws, which is exactly what a
            // placeholder payload looks like.
            assert!(
                distinct.len() > 1,
                "case {case:?}: the payload is one repeated byte, so it is silence"
            );
            encoded.push((format, body));
        }
        assert_eq!(
            encoded[0].1.len(),
            encoded[1].1.len(),
            "text {text:?}: both laws carry one byte a sample, so the streams are as long \
             as each other"
        );
        assert_ne!(
            encoded[0].1, encoded[1].1,
            "text {text:?}: mu-law and A-law are different codes, so their bytes must \
             differ"
        );
    }
}

/// RULE: a lexeme the request's lexicons define changes what is SPOKEN, so a grapheme
/// aliased to a longer phrase lengthens the stream — and naming several lexicons applies
/// each one's lexemes, so together they lengthen it more than either alone.
#[tokio::test]
async fn a_lexicon_changes_the_audio_it_is_applied_to() {
    let polly = Polly::new();
    for (name, content) in [
        ("LexA", alias_lexicon("AWS", "Amazon Web Services")),
        ("LexB", alias_lexicon("EC2", "Elastic Compute Cloud")),
        // A lexeme whose replacement is a pronunciation rather than other text.
        ("LexC", {
            format!(
                "<lexicon version=\"1.0\" xmlns=\"{PLS}\" alphabet=\"ipa\" xml:lang=\"en-US\">\
                 <lexeme><grapheme>tomato</grapheme><phoneme>təˈmɑːtoʊ</phoneme></lexeme>\
                 </lexicon>"
            )
        }),
    ] {
        assert_eq!(
            polly.put_lexicon(name, &content).await.status().as_u16(),
            200,
            "the fixture lexicon {name} must be stored"
        );
    }

    const TEXT: &str = "Please deploy AWS and EC2 today.";
    let baseline = lexicon_length(&polly, TEXT, &[]).await;
    let only_a = lexicon_length(&polly, TEXT, &["LexA"]).await;
    let only_b = lexicon_length(&polly, TEXT, &["LexB"]).await;
    let both = lexicon_length(&polly, TEXT, &["LexA", "LexB"]).await;
    assert!(
        only_a > baseline,
        "a lexicon whose alias is longer than its grapheme must lengthen the audio: \
         baseline={baseline} only_a={only_a}"
    );
    assert!(
        only_b > baseline,
        "the second lexicon must lengthen it too: baseline={baseline} only_b={only_b}"
    );
    assert!(
        both > only_a && both > only_b,
        "both lexicons together must lengthen it more than either alone: only_a={only_a} \
         only_b={only_b} both={both}"
    );

    // A lexeme that replaces a word with a pronunciation changes the audio without
    // changing the text's own length.
    let plain = audio(&polly, "tomato, no lexicon", &request("tomato", "pcm", &[])).await;
    let pronounced = audio(
        &polly,
        "tomato, with its pronunciation",
        &request(
            "tomato",
            "pcm",
            &[("LexiconNames", serde_json::json!(["LexC"]))],
        ),
    )
    .await;
    assert_ne!(
        plain, pronounced,
        "a pronunciation lexeme must change the audio of the word it defines"
    );

    // The billed characters are the TEXT's own, whatever a lexicon replaces in it.
    let response = polly
        .synthesize(&request(
            TEXT,
            "pcm",
            &[("LexiconNames", serde_json::json!(["LexA"]))],
        ))
        .await;
    assert_eq!(
        header(&response, "x-amzn-requestcharacters"),
        TEXT.chars().count().to_string(),
        "a lexicon changes what is spoken, not what is billed"
    );
}

/// RULE: SSML is APPLIED, so markup that changes how long its content takes to speak
/// changes the length of the stream: a slower rate lengthens it, a faster one shortens
/// it, and a pause adds its own time.
#[tokio::test]
async fn ssml_markup_changes_the_rendered_audio() {
    let polly = Polly::new();
    const SENTENCE: &str = "This sentence is spoken at a different rate.";
    let plain = audio(
        &polly,
        "plain text",
        &request(SENTENCE, "pcm", &[("SampleRate", serde_json::json!("16000"))]),
    )
    .await
    .len();

    // The rate values are `<prosody rate>`'s published domain: the keywords and a
    // percentage. A rate below the voice's own lengthens the speech; one above shortens
    // it; the same sentence with no prosody at all is the plain-text length.
    let rows: [(&str, f64, f64); 6] = [
        // (rate, the smallest multiple of the plain length, the largest)
        ("x-slow", 1.3, 3.0),
        ("slow", 1.1, 2.0),
        ("50%", 1.3, 3.0),
        ("medium", 0.95, 1.05),
        ("fast", 0.6, 0.95),
        ("x-fast", 0.5, 0.95),
    ];
    for (rate, low, high) in rows {
        let length = ssml_length(
            &polly,
            &format!("<speak><prosody rate=\"{rate}\">{SENTENCE}</prosody></speak>"),
        )
        .await;
        let ratio = length as f64 / plain as f64;
        assert!(
            ratio >= low && ratio <= high,
            "rate {rate:?}: expected between {low} and {high} times the plain length \
             ({plain} bytes), got {length} bytes ({ratio:.2}x)"
        );
    }

    // A pause adds its own time at 16 kHz signed 16-bit mono: 32 bytes a millisecond.
    let without = ssml_length(&polly, &format!("<speak>{SENTENCE}</speak>")).await;
    for (markup, expected_ms) in [
        ("<break time=\"1s\"/>", 1000.0f64),
        ("<break time=\"250ms\"/>", 250.0),
        ("<break strength=\"none\"/>", 0.0),
    ] {
        let with = ssml_length(&polly, &format!("<speak>{SENTENCE}{markup}</speak>")).await;
        let added_ms = (with as f64 - without as f64) / 32.0;
        assert!(
            (added_ms - expected_ms).abs() < 60.0,
            "markup {markup:?}: expected about {expected_ms} ms more audio, got {added_ms:.0} \
             ms ({with} bytes against {without})"
        );
    }

    // Markup that changes the voice rather than the timing keeps the length and changes
    // the samples.
    for markup in [
        format!("<speak><prosody pitch=\"x-high\">{SENTENCE}</prosody></speak>"),
        format!("<speak><amazon:effect name=\"whispered\">{SENTENCE}</amazon:effect></speak>"),
        format!("<speak><prosody volume=\"x-soft\">{SENTENCE}</prosody></speak>"),
    ] {
        let case = markup.clone();
        let shaped = audio(&polly, &case, &ssml_request(&markup)).await;
        let bare = audio(
            &polly,
            "bare document",
            &ssml_request(&format!("<speak>{SENTENCE}</speak>")),
        )
        .await;
        assert_ne!(
            shaped, bare,
            "markup {case:?} must change the samples it shapes"
        );
    }
}

/// RULE: the audio one request asks for is BOUNDED. `Text` is capped at the documented
/// character limit, but SSML stretches how long those characters take to speak — a slow
/// `<prosody rate>` multiplies every segment and each `<break>` may add ten seconds — so
/// a document inside the character limit still gets a response of bounded size.
#[tokio::test]
async fn the_audio_one_request_asks_for_is_bounded() {
    let polly = Polly::new();
    // "10s" is the longest pause `<break time>` documents, and a document well inside the
    // 6000-character limit holds two hundred of them: over half an hour of audio if
    // nothing bounded it.
    let packed = format!("<speak>{}</speak>", "<break time=\"10s\"/>".repeat(200));
    let body = audio(&polly, "two hundred ten-second pauses", &ssml_request(&packed)).await;
    // 16 kHz signed 16-bit mono is 32 bytes a millisecond, so the body length IS the
    // duration.
    let seconds = body.len() as f64 / 32_000.0;
    assert!(
        seconds < 500.0,
        "the request asked for {} seconds of pauses; the answer must be bounded, got \
         {seconds:.0} seconds ({} bytes)",
        200 * 10,
        body.len()
    );
    assert!(
        seconds > 60.0,
        "the pauses are still rendered up to that bound, got {seconds:.0} seconds"
    );
}

/// RULE: the two formats this emulator has no encoder for carry SILENCE, and say so: the
/// framing is real and as long as the speech, the signal inside it is zero. Nothing about
/// their response may suggest otherwise, so this is asserted rather than left to a doc
/// comment — and the formats that DO carry speech are asserted beside them, so the two
/// classes cannot drift into each other.
#[tokio::test]
async fn the_two_formats_without_an_encoder_carry_silence() {
    let polly = Polly::new();
    const TEXT: &str = "Testing which formats carry the signal.";
    // mp3 at a rate whose frames are 1152 samples: the payload is Layer III frames whose
    // main data is zero, so every byte after each four-byte header is zero.
    let mp3 = audio(
        &polly,
        "mp3",
        &request(
            TEXT,
            "mp3",
            &[("SampleRate", serde_json::json!("44100"))],
        ),
    )
    .await;
    assert_eq!(mp3[0], 0xFF, "an MPEG frame starts with the sync byte");
    // Four bytes in every frame are its header; everything else is main data, and the
    // main data of a silent stream is zero, so nearly the whole payload is zero bytes.
    let zero_bytes = mp3.iter().filter(|byte| **byte == 0).count();
    assert!(
        zero_bytes * 10 > mp3.len() * 9,
        "the mp3 payload is framing around silence, so almost every byte is zero: {} of \
         {} bytes",
        zero_bytes,
        mp3.len()
    );
    // ogg_opus: the pages are real and name the codec, and every audio packet is the
    // one-byte "no transmission" packet, so the stream is far smaller than the speech it
    // stands for would be.
    let opus = audio(&polly, "ogg_opus", &request(TEXT, "ogg_opus", &[])).await;
    assert_eq!(&opus[..4], b"OggS", "the Opus stream is Ogg-framed");
    assert!(
        opus.windows(8).any(|window| window == b"OpusHead"),
        "the Opus stream names its codec"
    );
    // The same utterance as real samples at the same rate would be two bytes a sample:
    // a silent Opus stream is a tiny fraction of that.
    let pcm = audio(
        &polly,
        "pcm for the comparison",
        &request(
            TEXT,
            "pcm",
            &[("SampleRate", serde_json::json!("16000"))],
        ),
    )
    .await;
    assert!(
        opus.len() * 10 < pcm.len(),
        "a silent Opus stream carries no waveform, so it is far smaller than the samples \
         it stands for: {} against {} bytes",
        opus.len(),
        pcm.len()
    );
    // And the formats that DO carry the waveform are not silent: their samples vary.
    for format in ["pcm", "mulaw", "alaw"] {
        let body = audio(&polly, format, &request(TEXT, format, &[])).await;
        let distinct: BTreeSet<u8> = body.iter().copied().collect();
        assert!(
            distinct.len() > 20,
            "format {format:?}: a rendered stream varies, got {} byte values",
            distinct.len()
        );
    }
}

/// RULE: every value of `SpeechMarkType` describes the utterance that was rendered —
/// `sentence` and `word` with the text they cover, `viseme` with the mouth shape of each
/// phoneme, `ssml` with each `<mark>` the document placed — and the marks are in stream
/// order.
#[tokio::test]
async fn every_speech_mark_type_describes_the_utterance() {
    let polly = Polly::new();
    // The viseme value set Amazon Polly's speech-mark documentation publishes.
    const VISEMES: [&str; 17] = [
        "sil", "p", "t", "S", "T", "f", "k", "i", "r", "s", "u", "@", "a", "e", "E", "o", "O",
    ];
    const SSML: &str = "<speak>Hello there. <mark name=\"middle\"/>How are you?</speak>";
    let rows: [(&str, &str, &str, &[&str]); 5] = [
        ("viseme over ssml", SSML, "ssml", &["viseme"]),
        ("ssml marks", SSML, "ssml", &["ssml"]),
        (
            "every type at once",
            SSML,
            "ssml",
            &["sentence", "word", "viseme", "ssml"],
        ),
        (
            "viseme over plain text",
            "Hello there. How are you?",
            "text",
            &["viseme"],
        ),
        (
            "sentence and viseme over plain text",
            "Hello there. How are you?",
            "text",
            &["sentence", "viseme"],
        ),
    ];
    for (case, text, text_type, wanted) in rows {
        let response = polly
            .synthesize(&request(
                text,
                "json",
                &[
                    ("TextType", serde_json::json!(text_type)),
                    ("SpeechMarkTypes", serde_json::json!(wanted)),
                ],
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected a mark stream, got {:?}",
            json(&response)
        );
        assert_eq!(
            header(&response, "content-type"),
            "application/x-json-stream",
            "case {case:?}: wrong media type"
        );
        let marks: Vec<serde_json::Value> = String::from_utf8_lossy(response.body())
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
            .collect();
        assert!(
            !marks.is_empty(),
            "case {case:?}: expected marks, got an empty stream"
        );
        let types: Vec<&str> = marks
            .iter()
            .map(|mark| mark["type"].as_str().unwrap_or_default())
            .collect();
        for mark_type in &types {
            assert!(
                wanted.contains(mark_type),
                "case {case:?}: expected only {wanted:?}, got {mark_type:?}"
            );
        }
        for requested in wanted {
            assert!(
                types.contains(requested),
                "case {case:?}: expected a {requested:?} mark, got {types:?}"
            );
        }
        let times: Vec<u64> = marks
            .iter()
            .map(|mark| mark["time"].as_u64().unwrap_or_default())
            .collect();
        assert!(
            times.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: the marks must be in stream order, got {times:?}"
        );

        let visemes: Vec<&str> = marks
            .iter()
            .filter(|mark| mark["type"].as_str() == Some("viseme"))
            .map(|mark| mark["value"].as_str().unwrap_or_default())
            .collect();
        if wanted.contains(&"viseme") {
            for viseme in &visemes {
                assert!(
                    VISEMES.contains(viseme),
                    "case {case:?}: {viseme:?} is not a published viseme value"
                );
            }
            assert_eq!(
                visemes.last().copied(),
                Some("sil"),
                "case {case:?}: the viseme stream ends with the closed mouth, got {visemes:?}"
            );
            assert!(
                visemes.len() > 5,
                "case {case:?}: expected one viseme a phoneme, got {visemes:?}"
            );
            // A viseme is a point in time, so it reports no offsets into the text.
            for mark in marks
                .iter()
                .filter(|mark| mark["type"].as_str() == Some("viseme"))
            {
                assert!(
                    mark.get("start").is_none() && mark.get("end").is_none(),
                    "case {case:?}: a viseme covers no stretch of the text, got {mark}"
                );
            }
        }
        if wanted.contains(&"ssml") {
            let named: Vec<&str> = marks
                .iter()
                .filter(|mark| mark["type"].as_str() == Some("ssml"))
                .map(|mark| mark["value"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                named,
                vec!["middle"],
                "case {case:?}: an ssml mark carries the name the document gave it"
            );
        }
    }
}

/// RULE: every modelled `OutputFormat` carries the SAME utterance in its own encoding, so
/// asking for one text in two formats answers the same speech twice — and a different
/// text answers different bytes in every one of them.
#[tokio::test]
async fn every_format_carries_the_same_utterance() {
    let polly = Polly::new();
    // (format, its media type, the framing its stream starts with)
    let table: [(&str, &str, Option<&[u8]>); 6] = [
        ("pcm", "audio/pcm", None),
        ("mulaw", "audio/mulaw", None),
        ("alaw", "audio/alaw", None),
        ("mp3", "audio/mpeg", Some(&[0xFF])),
        ("ogg_vorbis", "audio/ogg", Some(b"OggS")),
        ("ogg_opus", "audio/ogg", Some(b"OggS")),
    ];
    for (format, media_type, framing) in table {
        let mut answers: Vec<Vec<u8>> = Vec::new();
        for text in ["A short sentence.", "A different, rather longer sentence."] {
            let case = format!("format {format:?} text {text:?}");
            let response = polly.synthesize(&request(text, format, &[])).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: expected audio, got {:?}",
                json(&response)
            );
            assert_eq!(
                header(&response, "content-type"),
                media_type,
                "case {case:?}: wrong Content-Type"
            );
            let body = response.body().to_vec();
            if let Some(framing) = framing {
                assert_eq!(
                    &body[..framing.len()],
                    framing,
                    "case {case:?}: the stream does not carry the format's framing"
                );
            }
            // Asking twice for the same text answers the same bytes: the stream is
            // computed from the request, not drawn at random.
            let again = polly.synthesize(&request(text, format, &[])).await;
            assert_eq!(
                again.body().to_vec(),
                body,
                "case {case:?}: the same request must answer the same bytes"
            );
            answers.push(body);
        }
        assert!(
            answers[1].len() > answers[0].len(),
            "format {format:?}: more speech must be a longer stream, got {} and {} bytes",
            answers[0].len(),
            answers[1].len()
        );
    }
}
