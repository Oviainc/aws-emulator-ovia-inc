// SPDX-License-Identifier: CC0-1.0

//! The `ogg_opus` path: the stream the encoders module writes for the format whose Opus
//! encoder is unavailable.
//!
//! Every Opus binding published for Rust either links a libopus this image does not
//! carry or is a raw FFI binding this emulator may not call —
//! [`super::super::legacy_audio`] names each one and how it fails — so the stream is that
//! module's conformant Ogg Opus pages of silence, as long as the speech would have been.
//! The length is taken from the samples that WERE rendered, so this format too follows
//! the utterance in duration.

/// The silent Ogg Opus stream lasting as long as `samples` do at `sample_rate`.
///
/// `None` only if the in-memory writer fails, which the caller answers with the modelled
/// server error.
pub fn ogg_opus_silence_for(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    super::super::legacy_audio::ogg_opus_silence(
        sample_rate,
        super::legacy_audio::duration_ms(samples, sample_rate),
    )
}
