// SPDX-License-Identifier: CC0-1.0

//! The two `OutputFormat` values whose streams are silence rather than the rendered
//! waveform, and why.
//!
//! Every other format carries the speech [`super::render`] produced. These two cannot,
//! and neither is answered with something else or refused. Each blocker is named with the
//! crate and the failure, so a later attempt can re-check them:
//!
//! * `mp3` would need an MPEG Layer III encoder. The only one published for Rust is
//!   `mp3lame-encoder`, which binds libmp3lame and declares LGPL-3.0 — a licence this
//!   project cannot take.
//! * `ogg_opus` would need an Opus encoder. `audiopus_sys` 0.1 runs the bundled libopus
//!   `autogen.sh` and fails for want of `autoreconf`; `opus` 0.3 with `audiopus_sys` 0.2
//!   links a system libopus this image does not carry, so the link step fails; and
//!   `opusic-sys`, which does build its own copy, is a raw FFI binding this emulator may
//!   not call, because it writes no `unsafe` code.
//!
//! So each of them carries its own real framing — the requested sample rate, conformant
//! frames or pages, the length the speech would have had — around a signal of zero
//! amplitude. That is silence; it is said so here, in [`super::audio`] and in the
//! operation's own module documentation, rather than dressed up as speech, and the
//! integration suite asserts it (`tests/all/rendered_speech.rs`:
//! `the_two_formats_without_an_encoder_carry_silence`), so nothing here can quietly start
//! claiming otherwise. Both are encoded to the length of the SAME utterance the other
//! formats render, so a lexicon or an SSML change still changes what they carry.

/// A run of conformant MPEG Layer III frames, as long as `duration_ms`, whose main data
/// is zero.
///
/// The frame header is built per ISO/IEC 11172-3 (MPEG-1) and 13818-3 (MPEG-2), plus the
/// MPEG-2.5 extension for 8 kHz: 11 sync bits, the version and layer, the bitrate and
/// sample-rate indexes, and single-channel mode. `None` for a sample rate MPEG Layer III
/// cannot carry, which the caller has already refused as an invalid sample rate.
pub fn mp3_silence(sample_rate: u32, duration_ms: u64) -> Option<Vec<u8>> {
    // (version bits, sample-rate index, samples per frame, frame-size coefficient,
    // bitrate index for 64 kb/s). MPEG-1 = 0b11, MPEG-2 = 0b10, MPEG-2.5 = 0b00;
    // Layer III carries 1152 samples per MPEG-1 frame and 576 per MPEG-2/2.5 frame.
    let (version, rate_index, samples_per_frame, coefficient, bitrate_index) = match sample_rate {
        44100 => (0b11u8, 0u8, 1152u64, 144u64, 5u8),
        48000 => (0b11, 1, 1152, 144, 5),
        32000 => (0b11, 2, 1152, 144, 5),
        22050 => (0b10, 0, 576, 72, 8),
        24000 => (0b10, 1, 576, 72, 8),
        16000 => (0b10, 2, 576, 72, 8),
        8000 => (0b00, 2, 576, 72, 8),
        _ => return None,
    };
    /// The bitrate every frame declares.
    const BITRATE: u64 = 64_000;
    // Layer III frame length is `coefficient * bitrate / sample_rate`, which is rarely a
    // whole number; the header's padding bit is what carries the remainder, so a
    // constant-bitrate stream alternates padded and unpadded frames. Tracking the
    // remainder frame by frame keeps the stream's real bitrate equal to the one its
    // header declares.
    let numerator = coefficient.saturating_mul(BITRATE);
    let divisor = u64::from(sample_rate);
    let base_bytes = usize::try_from(numerator / divisor).unwrap_or(usize::MAX);
    let remainder = numerator % divisor;
    let frames = usize::try_from(
        (u64::from(sample_rate).saturating_mul(duration_ms) / 1000)
            .div_ceil(samples_per_frame)
            .max(1),
    )
    .unwrap_or(usize::MAX);

    let mut out: Vec<u8> = Vec::with_capacity(frames.saturating_mul(base_bytes + 1));
    let mut carry = 0u64;
    for _ in 0..frames {
        carry += remainder;
        let padded = carry >= divisor;
        if padded {
            carry -= divisor;
        }
        let header = [
            0xFF,
            // sync (3 more bits) | version | Layer III (0b01) | no CRC (1)
            0xE0 | (version << 3) | (0b01 << 1) | 0x01,
            (bitrate_index << 4) | (rate_index << 2) | if padded { 0x02 } else { 0x00 },
            // single channel, no mode extension, no copyright/original, no emphasis
            0xC0,
        ];
        out.extend_from_slice(&header);
        // The rest of the frame is main data, and zero main data is silence.
        let main_data = base_bytes.saturating_sub(header.len()) + usize::from(padded);
        out.resize(out.len() + main_data, 0);
    }
    Some(out)
}

/// A conformant Ogg Opus stream of silence (RFC 7845 headers, RFC 6716 packets), as long
/// as `duration_ms`.
///
/// The Ogg layer — pages, lacing and the page checksum — is the `ogg` crate's; only the
/// two codec headers and the packet payloads are built here, because those are Opus, not
/// Ogg. A one-byte packet is RFC 6716's "no transmission" (DTX) packet, which a decoder
/// renders as silence, so the stream a decoder accepts is as long as the speech and
/// silent throughout. `None` only if the in-memory writer fails, which the caller answers
/// with the modelled server error rather than a panic.
pub fn ogg_opus_silence(input_sample_rate: u32, duration_ms: u64) -> Option<Vec<u8>> {
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};

    /// Every Opus frame this emits is 20 ms long.
    const FRAME_MS: u64 = 20;
    /// An Opus granule position counts 48 kHz samples, whatever the input rate.
    const FRAME_GRANULES: u64 = 960;
    /// Frames per audio page, so a long stream is paged rather than one huge page.
    const FRAMES_PER_PAGE: u64 = 50;
    /// A one-byte packet is a TOC byte with a zero-length frame: RFC 6716's
    /// "no transmission" (DTX) packet. The TOC selects SILK narrowband, 20 ms, mono, one
    /// frame per packet.
    const DTX_PACKET: [u8; 1] = [0x08];
    /// The Ogg logical-stream serial number this service uses. Fixed, because one
    /// response is always exactly one logical stream.
    const OGG_SERIAL: u32 = 0x506f_6c6c;

    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut writer = PacketWriter::new(&mut buffer);
        // RFC 7845 §3: the identification and comment headers each occupy their own
        // page, and the audio packets follow.
        writer
            .write_packet(
                opus_head(input_sample_rate),
                OGG_SERIAL,
                PacketWriteEndInfo::EndPage,
                0,
            )
            .ok()?;
        writer
            .write_packet(opus_tags(), OGG_SERIAL, PacketWriteEndInfo::EndPage, 0)
            .ok()?;

        let frames = duration_ms.div_ceil(FRAME_MS).max(1);
        for frame in 1..=frames {
            let end = if frame == frames {
                PacketWriteEndInfo::EndStream
            } else if frame % FRAMES_PER_PAGE == 0 {
                PacketWriteEndInfo::EndPage
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            writer
                .write_packet(
                    DTX_PACKET.to_vec(),
                    OGG_SERIAL,
                    end,
                    frame.saturating_mul(FRAME_GRANULES),
                )
                .ok()?;
        }
    }
    Some(buffer)
}

/// RFC 7845 §5.1 `OpusHead`: mono, no pre-skip, no output gain, mapping family 0.
fn opus_head(input_sample_rate: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1); // version
    head.push(1); // channel count
    head.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
    head.extend_from_slice(&input_sample_rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    head
}

/// RFC 7845 §5.2 `OpusTags`: a vendor string and no user comments. The vendor string
/// names the producer honestly — this stream comes from an emulator.
fn opus_tags() -> Vec<u8> {
    const VENDOR: &[u8] = b"aws-emulator polly";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&u32::try_from(VENDOR.len()).unwrap_or(0).to_le_bytes());
    tags.extend_from_slice(VENDOR);
    tags.extend_from_slice(&0u32.to_le_bytes()); // user comment count
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every MPEG frame declares the stream it is part of — the sync word, Layer
    /// III and the bitrate its length matches — for every rate the format documents.
    #[test]
    fn mp3_frames_carry_a_valid_header_for_every_documented_rate() {
        for spelling in super::super::valid_sample_rates("mp3") {
            let rate = super::super::resolve_sample_rate("mp3", spelling)
                .unwrap_or_else(|| panic!("rate {spelling:?} is one of mp3's documented values"));
            let bytes = mp3_silence(rate, 1_000).expect("mp3 supports this rate");
            assert!(
                !bytes.is_empty(),
                "rate {rate}: expected frames, got an empty stream"
            );
            assert_eq!(
                bytes[0], 0xFF,
                "rate {rate}: the frame must start with the MPEG sync byte, got {:#04x}",
                bytes[0]
            );
            assert_eq!(
                bytes[1] & 0xE0,
                0xE0,
                "rate {rate}: the remaining sync bits are missing, got {:#04x}",
                bytes[1]
            );
            // Layer III is 0b01 in bits 2..1 of the second header byte.
            assert_eq!(
                (bytes[1] >> 1) & 0b11,
                0b01,
                "rate {rate}: the frame does not declare Layer III"
            );
            // One second at the declared 64 kb/s is 8000 bytes; the stream rounds up to
            // a whole frame, so it must land within one frame of that. A stream that
            // ignored the padding bit would drift away from its own header.
            assert!(
                (7_600..=8_600).contains(&bytes.len()),
                "rate {rate}: one second at 64 kb/s is ~8000 bytes, got {}",
                bytes.len()
            );
        }
        assert!(
            mp3_silence(11_025, 1_000).is_none(),
            "a rate Layer III cannot carry is not encodable"
        );
    }

    /// RULE: the Opus stream is Ogg-framed, names its codec and lasts as long as the
    /// duration asked for.
    #[test]
    fn the_opus_stream_is_ogg_framed_and_names_its_codec() {
        let opus = ogg_opus_silence(48000, 200).expect("the in-memory writer cannot fail");
        assert_eq!(&opus[..4], b"OggS", "an Ogg page starts with OggS");
        for header in [&b"OpusHead"[..], &b"OpusTags"[..]] {
            assert!(
                opus.windows(header.len()).any(|window| window == header),
                "the Opus stream must carry {:?}",
                String::from_utf8_lossy(header)
            );
        }
        let longer = ogg_opus_silence(48000, 2_000).expect("writer cannot fail");
        assert!(
            longer.len() > opus.len(),
            "a longer duration must carry more packets: {} vs {}",
            longer.len(),
            opus.len()
        );
    }
}
