// SPDX-License-Identifier: CC0-1.0

//! The encoders behind `OutputFormat`: the rendered waveform, written in the encoding
//! the request asked for.
//!
//! Every function here takes the SAME samples — the waveform [`super::render`] produced
//! from the request's text — and writes them in one encoding, so a caller that asks for
//! the same text in two formats gets the same speech twice:
//!
//! * `pcm` is signed 16-bit mono little-endian, the representation the member documents;
//! * `mulaw` and `alaw` are ITU-T G.711 companded bytes, computed with that
//!   Recommendation's own segment tables;
//! * `ogg_vorbis` is a real Vorbis stream in an Ogg container, from the `vorbis_rs`
//!   encoder.
//!
//! Two formats are NOT written from the samples — `mp3`, which needs an MPEG Layer III
//! encoder, and `ogg_opus`, which needs an Opus encoder; neither is available to this
//! image for the reasons [`super::legacy_audio`] states. Each carries its own real
//! framing around silence, lasting as long as the speech would have, and lives in that
//! module rather than being dressed up as one of the encoders here.

use super::phonemes;

/// The two formats whose samples are not written, so every `OutputFormat` is written
/// through this one module: each child takes the duration from the samples that WERE
/// rendered, and the writers and the reasons live in [`super::legacy_audio`]. The
/// `#[path]` is for the same reason as the declarations in the operation module: this
/// file is reached through one, so a bare `mod` resolves beside it.
#[path = "audio/legacy_audio.rs"]
mod legacy_audio;
#[path = "audio/unused_opus.rs"]
mod unused_opus;
pub use legacy_audio::mp3_silence_for;
pub use unused_opus::ogg_opus_silence_for;

/// Signed 16-bit mono little-endian samples — the representation
/// `SynthesizeSpeechInput$OutputFormat` documents for `pcm` ("a signed 16-bit, 1 channel
/// (mono), little-endian format").
pub fn pcm(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        out.extend_from_slice(&to_i16(*sample).to_le_bytes());
    }
    out
}

/// ITU-T G.711 μ-law bytes.
pub fn mulaw(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| linear_to_mulaw(to_i16(*sample)))
        .collect()
}

/// ITU-T G.711 A-law bytes.
pub fn alaw(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| linear_to_alaw(to_i16(*sample)))
        .collect()
}

/// One sample as the signed 16-bit integer it is carried by.
fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// The segment ends of the μ-law characteristic (ITU-T G.711, Table 2a).
const MULAW_SEGMENT_ENDS: [i32; 8] = [0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF];
/// The segment ends of the A-law characteristic (ITU-T G.711, Table 1a).
const ALAW_SEGMENT_ENDS: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

/// Which segment of the characteristic `value` falls in.
fn segment(value: i32, ends: &[i32; 8]) -> usize {
    ends.iter()
        .position(|end| value <= *end)
        .unwrap_or(ends.len())
}

/// One sample, μ-law companded (ITU-T G.711): the 14 most significant bits of the
/// sample, biased, then split into the segment and the step within it, and inverted.
fn linear_to_mulaw(sample: i16) -> u8 {
    /// The largest magnitude the characteristic represents, in its own 14-bit scale.
    const CLIP: i32 = 8159;
    /// The bias the characteristic adds before the segment is found.
    const BIAS: i32 = 0x84;
    let mut magnitude = i32::from(sample) >> 2;
    let mask: u8 = if magnitude < 0 {
        magnitude = -magnitude;
        0x7F
    } else {
        0xFF
    };
    magnitude = magnitude.min(CLIP) + (BIAS >> 2);
    let segment = segment(magnitude, &MULAW_SEGMENT_ENDS);
    if segment >= MULAW_SEGMENT_ENDS.len() {
        return 0x7F ^ mask;
    }
    let step = ((magnitude >> (segment + 1)) & 0x0F) as u8;
    (((segment as u8) << 4) | step) ^ mask
}

/// One sample, A-law companded (ITU-T G.711): the 13 most significant bits of the
/// sample split into the segment and the step within it, with the alternate-bit mask
/// the Recommendation applies.
fn linear_to_alaw(sample: i16) -> u8 {
    let mut magnitude = i32::from(sample) >> 3;
    let mask: u8 = if magnitude >= 0 {
        0xD5
    } else {
        magnitude = -magnitude - 1;
        0x55
    };
    let segment = segment(magnitude, &ALAW_SEGMENT_ENDS);
    if segment >= ALAW_SEGMENT_ENDS.len() {
        return 0x7F ^ mask;
    }
    let step = if segment < 2 {
        ((magnitude >> 1) & 0x0F) as u8
    } else {
        ((magnitude >> segment) & 0x0F) as u8
    };
    (((segment as u8) << 4) | step) ^ mask
}

/// The waveform as a Vorbis stream in an Ogg container.
///
/// The encoder is `vorbis_rs`: a conformant Vorbis stream needs the codebooks of a real
/// encoder's setup header, which is exactly what this provides. `None` only when the
/// encoder refuses the stream, which the caller answers with the modelled server error.
pub fn ogg_vorbis(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    use std::num::{NonZeroU32, NonZeroU8};

    /// The Ogg logical-stream serial number this service writes: one response is
    /// exactly one logical stream, so it is fixed rather than drawn at random.
    const OGG_SERIAL: i32 = 0x506f_6c6c;
    /// How many samples are handed to the encoder at a time.
    const BLOCK: usize = 2048;

    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut encoder = vorbis_rs::VorbisEncoderBuilder::new_with_serial(
            NonZeroU32::new(sample_rate)?,
            NonZeroU8::new(1)?,
            &mut buffer,
            OGG_SERIAL,
        )
        .build()
        .ok()?;
        for block in samples.chunks(BLOCK) {
            encoder.encode_audio_block([block]).ok()?;
        }
        encoder.finish().ok()?;
    }
    Some(buffer)
}

/// The speech-mark stream a `json` request carries: one JSON object per line.
///
/// A mark reports `start` and `end` only where it covers a stretch of the text — a
/// `sentence` or a `word` does, a `viseme` and an `ssml` mark are points in time — which
/// is the shape Amazon Polly's `application/x-json-stream` carries.
pub fn speech_marks_stream(marks: &[super::SpeechMark]) -> Vec<u8> {
    let mut out = Vec::new();
    for mark in marks {
        let mut line = ::serde_json::json!({
            "time": mark.time,
            "type": mark.mark_type,
            "value": mark.value,
        });
        if let (Some((start, end)), Some(object)) = (mark.span, line.as_object_mut()) {
            object.insert("start".to_string(), ::serde_json::json!(start));
            object.insert("end".to_string(), ::serde_json::json!(end));
        }
        out.extend_from_slice(line.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

/// Whether `viseme` is one of the published viseme values, which is what a `viseme` mark
/// carries.
pub fn is_published_viseme(viseme: &str) -> bool {
    viseme == phonemes::SILENCE_VISEME
        || phonemes::INVENTORY
            .iter()
            .any(|phoneme| phoneme.viseme == viseme)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A waveform that sweeps the whole range, for the companding tables.
    fn ramp(count: usize) -> Vec<f32> {
        (0..count)
            .map(|index| (index as f32 / count as f32) * 2.0 - 1.0)
            .collect()
    }

    /// RULE: G.711 companding is reversible to within its own step size, and silence
    /// encodes to the Recommendation's own idle codes.
    #[test]
    fn companding_follows_g711() {
        assert_eq!(
            linear_to_mulaw(0),
            0xFF,
            "μ-law silence is the idle code 0xFF"
        );
        assert_eq!(linear_to_alaw(0), 0xD5, "A-law silence is the idle code 0xD5");
        // A sweep has to take many different code values in both laws: a companded
        // signal that collapses to one byte is not a signal.
        let sweep = ramp(4096);
        for (law, encoded) in [("mulaw", mulaw(&sweep)), ("alaw", alaw(&sweep))] {
            let distinct: std::collections::BTreeSet<u8> = encoded.iter().copied().collect();
            assert!(
                distinct.len() > 200,
                "law {law}: expected most of the code space, got {} values",
                distinct.len()
            );
        }
    }

    /// RULE: `pcm` is signed 16-bit little-endian mono, so each sample is two bytes.
    #[test]
    fn pcm_is_two_bytes_a_sample() {
        let samples = ramp(100);
        let encoded = pcm(&samples);
        assert_eq!(encoded.len(), samples.len() * 2, "two bytes a sample");
        let first = i16::from_le_bytes([encoded[0], encoded[1]]);
        assert!(first < 0, "the ramp starts negative, got {first}");
    }

    /// RULE: the Ogg Vorbis format carries a real stream of the samples, in an Ogg
    /// container.
    #[test]
    fn the_vorbis_stream_carries_the_samples() {
        let samples = ramp(48_000);
        let encoded = ogg_vorbis(&samples, 48_000).expect("the Vorbis encoder accepts mono f32");
        assert_eq!(&encoded[..4], b"OggS", "expected an Ogg container");
        assert!(
            encoded.windows(6).any(|window| window == b"vorbis"),
            "the stream must name its codec"
        );
        assert!(
            encoded.len() > 1000,
            "expected a stream of the samples, got {} bytes",
            encoded.len()
        );
        let shorter = ogg_vorbis(&ramp(8_000), 48_000).expect("a shorter stream encodes too");
        assert!(
            shorter.len() < encoded.len(),
            "less audio must be fewer bytes: {} vs {}",
            shorter.len(),
            encoded.len()
        );
    }
}
