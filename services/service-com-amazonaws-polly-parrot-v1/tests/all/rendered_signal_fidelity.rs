// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that DECODE what `SynthesizeSpeech` answers with, so each stream is
//! checked as a signal rather than as a length.
//!
//! RULE: the response carries ONE waveform rendered from what the request asks to be
//! spoken — the text, the lexemes of the lexicons it names and the SSML it carries — and
//! each `OutputFormat` is that waveform written in its own encoding, so decoding the
//! stream the encoding's own way recovers the same speech, and the `json` mark stream
//! describes that same utterance on the same timeline.
//!
//! The suites beside this one read the bytes as bytes (a length, a media type, a byte
//! value count). These read them back through the decoder a client would use: the G.711
//! characteristic's own inverse for the companded formats and a Vorbis decoder for
//! `ogg_vorbis`. A placeholder payload survives a length check; it does not survive
//! being decoded and compared with the samples of the same utterance.
//!
//! Each test checks the rule over a TABLE spanning what it generalises over — several
//! texts, both voice genders, the sample rates each format documents and several
//! lexemes — of which the one request an issue quotes is a single row.

use crate::lexicons_and_speech::{header, json, Polly};

/// The PLS namespace a lexicon document is bound to.
const PLS: &str = "http://www.w3.org/2005/01/pronunciation-lexicon";

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

/// The samples of the `pcm` stream `body` asks for, as the signed 16-bit mono
/// little-endian values the member documents.
async fn pcm_samples(polly: &Polly, case: &str, body: &serde_json::Value) -> Vec<i16> {
    let bytes = audio(polly, case, body).await;
    assert!(
        bytes.len() % 2 == 0 && !bytes.is_empty(),
        "case {case:?}: pcm carries whole 16-bit samples, got {} bytes",
        bytes.len()
    );
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

/// One sample decoded from its μ-law byte: the inverse of the ITU-T G.711 μ-law
/// characteristic — the sign bit, the segment and the step within it, with the
/// characteristic's bias removed — which is what any client's G.711 decoder computes.
fn mulaw_to_linear(byte: u8) -> i16 {
    /// The bias the characteristic adds before the segment is found.
    const BIAS: i32 = 0x84;
    let code = !byte;
    // The step within the segment, with the segment's implicit leading bit and the bias
    // restored, then scaled by the segment.
    let magnitude = ((i32::from(code & 0x0F) << 3) + BIAS) << ((code & 0x70) >> 4);
    let value = if code & 0x80 != 0 {
        BIAS - magnitude
    } else {
        magnitude - BIAS
    };
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// One sample decoded from its A-law byte: the inverse of the ITU-T G.711 A-law
/// characteristic, including the alternate-bit mask the Recommendation applies.
fn alaw_to_linear(byte: u8) -> i16 {
    let code = byte ^ 0x55;
    let segment = (code & 0x70) >> 4;
    let step = i32::from(code & 0x0F) << 4;
    let magnitude = match segment {
        0 => step + 8,
        1 => step + 0x108,
        _ => (step + 0x108) << (segment - 1),
    };
    let value = if code & 0x80 != 0 { magnitude } else { -magnitude };
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// How closely two signals track each other: Pearson correlation over the samples, which
/// is 1.0 for the same waveform and 0.0 for unrelated ones.
fn correlation(left: &[i16], right: &[i16]) -> f64 {
    assert_eq!(
        left.len(),
        right.len(),
        "a correlation compares two signals of the same length"
    );
    let count = left.len() as f64;
    let mean = |signal: &[i16]| signal.iter().map(|s| f64::from(*s)).sum::<f64>() / count;
    let (left_mean, right_mean) = (mean(left), mean(right));
    let mut covariance = 0.0f64;
    let mut left_energy = 0.0f64;
    let mut right_energy = 0.0f64;
    for (a, b) in left.iter().zip(right.iter()) {
        let (a, b) = (f64::from(*a) - left_mean, f64::from(*b) - right_mean);
        covariance += a * b;
        left_energy += a * a;
        right_energy += b * b;
    }
    if left_energy == 0.0 || right_energy == 0.0 {
        return 0.0;
    }
    covariance / (left_energy.sqrt() * right_energy.sqrt())
}

/// RULE: the companded formats carry the rendered waveform through the G.711
/// characteristic, so decoding their bytes with that characteristic's inverse recovers
/// the same signal the `pcm` stream of the same request carries, sample for sample, to
/// within the companding error.
#[tokio::test]
async fn companded_streams_decode_back_to_the_rendered_waveform() {
    let polly = Polly::new();
    // The rows span the text (a sentence, one word, several sentences, digits) and both
    // voice genders, because the rule is about the signal of every request.
    let texts = [
        "Testing mu-law and a-law output from Amazon Polly.",
        "Go!",
        "One. Two. Three.",
        "Call 1234 today",
    ];
    for voice in ["Joanna", "Matthew"] {
        for text in texts {
            // "Valid values for pcm are \"8000\" and \"16000\" … Valid value for mu-law
            // and a-law is \"8000\"" (`SynthesizeSpeechInput$SampleRate`), so 8000 is the
            // one rate at which the three formats describe the same waveform.
            let extra = |format: &str| {
                vec![
                    ("SampleRate", serde_json::json!("8000")),
                    ("VoiceId", serde_json::json!(voice)),
                    ("OutputFormat", serde_json::json!(format)),
                ]
            };
            let case = format!("text {text:?} in {voice}");
            let reference = pcm_samples(&polly, &case, &request(text, "pcm", &extra("pcm"))).await;
            for (format, decode) in [
                ("mulaw", mulaw_to_linear as fn(u8) -> i16),
                ("alaw", alaw_to_linear as fn(u8) -> i16),
            ] {
                let case = format!("{case} as {format}");
                let encoded = audio(&polly, &case, &request(text, format, &extra(format))).await;
                assert_eq!(
                    encoded.len(),
                    reference.len(),
                    "case {case:?}: a companded stream is one byte a sample, so it is as \
                     long as the {} samples of the same utterance, got {} bytes",
                    reference.len(),
                    encoded.len()
                );
                let decoded: Vec<i16> = encoded.iter().copied().map(decode).collect();
                // The companding error is bounded by the step of the segment a sample
                // falls in, which grows with the sample: a proportional allowance plus
                // the smallest segment's own step.
                let worst = decoded
                    .iter()
                    .zip(reference.iter())
                    .map(|(got, want)| {
                        let error = (i32::from(*got) - i32::from(*want)).abs();
                        let allowed = i32::from(*want).abs() / 8 + 300;
                        (error - allowed, error, *want, *got)
                    })
                    .max_by_key(|(excess, _, _, _)| *excess)
                    .expect("a rendered utterance has samples");
                assert!(
                    worst.0 <= 0,
                    "case {case:?}: a decoded sample must track the waveform to within the \
                     companding error, got {} for {} (error {})",
                    worst.3,
                    worst.2,
                    worst.1
                );
                let tracking = correlation(&decoded, &reference);
                assert!(
                    tracking > 0.99,
                    "case {case:?}: the decoded signal must be the same waveform, \
                     correlation {tracking:.4}"
                );
            }
        }
    }
}

/// RULE: `ogg_vorbis` is the same waveform written as a Vorbis stream in an Ogg
/// container, so a Vorbis decoder reads it back as mono audio at the sample rate the
/// request asked for, lasting as long as the speech and carrying a signal.
#[tokio::test]
async fn the_vorbis_stream_decodes_to_the_rendered_waveform() {
    let polly = Polly::new();
    // Every row is a rate `SynthesizeSpeechInput$SampleRate` documents for ogg_vorbis
    // ("8000", "16000", "22050", "24000", "44100" and "48000"), against a text of its
    // own, so the decoded duration is checked across the whole documented domain.
    let rows: [(&str, &str); 6] = [
        ("8000", "Testing the vorbis stream."),
        ("16000", "A second, rather longer sentence to speak."),
        ("22050", "Hello."),
        ("24000", "One. Two. Three. Four."),
        ("44100", "Please deploy today."),
        ("48000", "Counting 1234 out loud."),
    ];
    for (rate, text) in rows {
        let case = format!("text {text:?} as ogg_vorbis at {rate} Hz");
        let body = audio(
            &polly,
            &case,
            &request(
                text,
                "ogg_vorbis",
                &[("SampleRate", serde_json::json!(rate))],
            ),
        )
        .await;
        let frequency: u32 = rate.parse().expect("a documented rate is a number");
        let mut decoder = vorbis_rs::VorbisDecoder::new(std::io::Cursor::new(body))
            .unwrap_or_else(|error| panic!("case {case:?}: a Vorbis decoder must read it: {error}"));
        assert_eq!(
            decoder.sampling_frequency().get(),
            frequency,
            "case {case:?}: the stream must declare the rate the request asked for"
        );
        assert_eq!(
            decoder.channels().get(),
            1,
            "case {case:?}: synthesized speech is one channel"
        );
        let mut samples: Vec<f32> = Vec::new();
        while let Some(block) = decoder
            .decode_audio_block()
            .unwrap_or_else(|error| panic!("case {case:?}: every block must decode: {error}"))
        {
            samples.extend_from_slice(block.samples()[0]);
        }
        assert!(
            !samples.is_empty(),
            "case {case:?}: the decoded stream carries no samples"
        );
        let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak > 0.1,
            "case {case:?}: the decoded signal must use the format's range, peak {peak:.4}"
        );
        // The same utterance as pcm is two bytes a sample at a documented pcm rate, so
        // its length IS the duration the Vorbis stream must also last.
        let reference = pcm_samples(
            &polly,
            &case,
            &request(text, "pcm", &[("SampleRate", serde_json::json!("16000"))]),
        )
        .await;
        let expected_seconds = reference.len() as f64 / 16_000.0;
        let decoded_seconds = samples.len() as f64 / f64::from(frequency);
        assert!(
            (decoded_seconds - expected_seconds).abs() < 0.1 * expected_seconds + 0.05,
            "case {case:?}: expected about {expected_seconds:.3} seconds of speech, decoded \
             {decoded_seconds:.3}"
        );
    }
}

/// RULE: an `<alias>` lexeme makes the grapheme it matches SPOKEN as the text it names,
/// so the audio of a text the lexicon applies to is the audio of that text with the
/// replacement written out — and nothing else about the utterance changes.
#[tokio::test]
async fn a_lexicon_alias_is_spoken_as_the_text_it_replaces() {
    let polly = Polly::new();
    // (grapheme, alias, the text it appears in): a longer replacement, a shorter one and
    // one that appears twice, so the rule is checked wherever the grapheme sits rather
    // than only where the issue's example put it.
    let rows: [(&str, &str, &str); 4] = [
        ("AWS", "Amazon Web Services", "Please deploy AWS today."),
        ("EC2", "Elastic Compute Cloud", "EC2 starts the instance."),
        ("kilometre", "km", "It is one kilometre away."),
        ("AWS", "Amazon Web Services", "AWS and AWS again."),
    ];
    for (index, (grapheme, alias, text)) in rows.iter().enumerate() {
        let name = format!("AliasLex{index}");
        let document = format!(
            "<lexicon version=\"1.0\" xmlns=\"{PLS}\" alphabet=\"ipa\" xml:lang=\"en-US\">\
             <lexeme><grapheme>{grapheme}</grapheme><alias>{alias}</alias></lexeme></lexicon>"
        );
        assert_eq!(
            polly.put_lexicon(&name, &document).await.status().as_u16(),
            200,
            "the fixture lexicon {name} must be stored"
        );
        let case = format!("grapheme {grapheme:?} in {text:?}");
        let applied = audio(
            &polly,
            &case,
            &request(
                text,
                "pcm",
                &[
                    ("SampleRate", serde_json::json!("16000")),
                    ("LexiconNames", serde_json::json!([name])),
                ],
            ),
        )
        .await;
        let written_out = text.replace(grapheme, alias);
        let spelled = audio(
            &polly,
            &format!("{case}, written out"),
            &request(
                &written_out,
                "pcm",
                &[("SampleRate", serde_json::json!("16000"))],
            ),
        )
        .await;
        assert_eq!(
            applied.len(),
            spelled.len(),
            "case {case:?}: the lexeme is spoken as {alias:?}, so the stream lasts as long \
             as speaking {written_out:?} does: {} against {} bytes",
            applied.len(),
            spelled.len()
        );
        assert!(
            applied == spelled,
            "case {case:?}: the lexeme is spoken as {alias:?}, so the samples are those of \
             {written_out:?}"
        );
        let plain = audio(
            &polly,
            &format!("{case}, no lexicon"),
            &request(
                text,
                "pcm",
                &[("SampleRate", serde_json::json!("16000"))],
            ),
        )
        .await;
        assert!(
            plain != applied,
            "case {case:?}: applying the lexicon must change the audio of {text:?}"
        );
        // The billed characters are the request's own text, whatever the lexeme replaces.
        let response = polly
            .synthesize(&request(
                text,
                "pcm",
                &[("LexiconNames", serde_json::json!([name]))],
            ))
            .await;
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            text.chars().count().to_string(),
            "case {case:?}: a lexicon changes what is spoken, not what is billed"
        );
    }
}

/// RULE: the `json` mark stream describes the utterance that the audio formats carry, on
/// the same timeline, so every mark falls inside the duration of the audio the same
/// request renders — and markup that stretches the speech moves the marks with it.
#[tokio::test]
async fn the_speech_mark_timeline_matches_the_audio_it_describes() {
    let polly = Polly::new();
    /// Signed 16-bit mono at 16 kHz is 32 samples a millisecond, so a `pcm` sample count
    /// is a duration.
    const SAMPLES_PER_MS: f64 = 16.0;
    const SENTENCE: &str = "This sentence is spoken at a measurable rate.";
    // (case, the text, its TextType)
    let rows: [(&str, String, &str); 5] = [
        ("plain text", SENTENCE.to_string(), "text"),
        ("several sentences", "One. Two. Three.".to_string(), "text"),
        (
            "a bare document",
            format!("<speak>{SENTENCE}</speak>"),
            "ssml",
        ),
        (
            "a slowed document",
            format!("<speak><prosody rate=\"x-slow\">{SENTENCE}</prosody></speak>"),
            "ssml",
        ),
        (
            "a document with a pause",
            "<speak>One <break time=\"1s\"/>two</speak>".to_string(),
            "ssml",
        ),
    ];
    for (case, text, text_type) in &rows {
        let samples = pcm_samples(
            &polly,
            case,
            &request(
                text,
                "pcm",
                &[
                    ("SampleRate", serde_json::json!("16000")),
                    ("TextType", serde_json::json!(text_type)),
                ],
            ),
        )
        .await;
        let duration_ms = samples.len() as f64 / SAMPLES_PER_MS;
        let response = polly
            .synthesize(&request(
                text,
                "json",
                &[
                    ("TextType", serde_json::json!(text_type)),
                    (
                        "SpeechMarkTypes",
                        serde_json::json!(["sentence", "word", "viseme"]),
                    ),
                ],
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected a mark stream, got {:?}",
            json(&response)
        );
        let times: Vec<f64> = String::from_utf8_lossy(response.body())
            .lines()
            .map(|line| {
                let mark: serde_json::Value =
                    serde_json::from_str(line).expect("each line is one JSON object");
                mark["time"].as_f64().unwrap_or_default()
            })
            .collect();
        assert!(
            !times.is_empty(),
            "case {case:?}: expected marks for an utterance that rendered {duration_ms:.0} ms"
        );
        let last = times.iter().copied().fold(0.0f64, f64::max);
        assert!(
            last <= duration_ms,
            "case {case:?}: a mark is a point in the audio, so the last one at {last:.0} ms \
             must fall inside the {duration_ms:.0} ms the request rendered"
        );
        // The marks span the utterance rather than crowding its start: the last one is in
        // its final stretch.
        assert!(
            last > duration_ms * 0.5,
            "case {case:?}: the marks must describe the whole utterance, last at {last:.0} ms \
             of {duration_ms:.0} ms"
        );
    }

    // The two documents above differ only in the rate they are spoken at, so the slowed
    // one's marks are pushed later by about as much as its audio is longer.
    let last_mark = |rows: &[(&str, String, &str)], case: &str| {
        let (_, text, text_type) = rows
            .iter()
            .find(|(name, _, _)| *name == case)
            .expect("a row of this table");
        (text.clone(), (*text_type).to_string())
    };
    let (bare, bare_type) = last_mark(&rows, "a bare document");
    let (slow, slow_type) = last_mark(&rows, "a slowed document");
    let word_marks = |text: String, text_type: String| {
        let polly = &polly;
        async move {
            let response = polly
                .synthesize(&request(
                    &text,
                    "json",
                    &[
                        ("TextType", serde_json::json!(text_type)),
                        ("SpeechMarkTypes", serde_json::json!(["word"])),
                    ],
                ))
                .await;
            String::from_utf8_lossy(response.body())
                .lines()
                .map(|line| {
                    let mark: serde_json::Value =
                        serde_json::from_str(line).expect("each line is one JSON object");
                    mark["time"].as_f64().unwrap_or_default()
                })
                .fold(0.0f64, f64::max)
        }
    };
    let bare_last = word_marks(bare, bare_type).await;
    let slow_last = word_marks(slow, slow_type).await;
    assert!(
        slow_last > bare_last * 1.3,
        "a slower rate stretches the timeline the marks sit on: {bare_last:.0} ms against \
         {slow_last:.0} ms"
    );
}
