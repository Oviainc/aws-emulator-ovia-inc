// SPDX-License-Identifier: CC0-1.0

//! The `mp3` path: the stream the encoders module writes for the one format whose samples
//! it cannot.
//!
//! MPEG Layer III needs a Layer III encoder, which is not available here for the reason
//! [`super::super::legacy_audio`] states, so the stream is that module's conformant
//! frames of silence — as long as the speech would have been. The length is taken from
//! the samples that WERE rendered, so the one `OutputFormat` whose signal is missing
//! still follows the utterance in duration, and both operations that answer with audio
//! take it from here rather than computing that duration twice.

/// The silent MPEG Layer III stream lasting as long as `samples` do at `sample_rate`.
///
/// `None` for a sample rate MPEG Layer III cannot carry, which the caller has already
/// refused as an invalid sample rate.
pub fn mp3_silence_for(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    super::super::legacy_audio::mp3_silence(sample_rate, duration_ms(samples, sample_rate))
}

/// How long `samples` last at `sample_rate`, in milliseconds.
pub fn duration_ms(samples: &[f32], sample_rate: u32) -> u64 {
    (samples.len() as u64).saturating_mul(1000) / u64::from(sample_rate.max(1))
}
