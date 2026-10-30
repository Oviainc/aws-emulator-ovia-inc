// SPDX-License-Identifier: CC0-1.0

//! What `DecodeAuthorizationMessage` READS out of an `EncodedMessage`, over the class of
//! blobs a client can send.
//!
//! RULE: an encoded authorization message is base64, and the service decodes it before it
//! judges it — with the latitude a DECODER has rather than the strictness an encoder is
//! held to. The padding carries no data, so it is optional; the bits left over in a final
//! group that is not a whole number of bytes carry no data either, so they are discarded
//! rather than required to be zero. Both readings name the same bytes, which is what the
//! refusal of a message this account did not issue reports — so a blob and its canonical
//! re-encoding are refused with the same byte listing, and a blob that is not base64 at
//! all is the only kind whose own characters are reported.
//!
//! The rows span what the rule generalises over: the length of the final group (a whole
//! group, three characters, two characters), whether that group's unused bits are zero,
//! whether the blob carries its padding, which alphabet it is written in (standard,
//! URL-safe), and whether it is base64 at all.
//!
//! Every call goes over the wire as the `application/x-www-form-urlencoded` awsQuery POST
//! a client makes, through the same router the emulator serves with.
//!
//! The expected bytes of every row come from [`decoded_bytes`] below, a decoder written
//! out here on purpose: asserting with the engine the service itself decodes with would
//! only restate the implementation, so this file reads the alphabet, the padding and the
//! final quantum's unused bits itself (RFC 4648 §§3.5, 4 and 5) and the two readings have
//! to agree. Do NOT replace it with the production codec.

use crate::sts_sessions::{call, element, emulator, member, LONG_TERM_KEY};

/// The base64 alphabet, in value order (RFC 4648 §4). The URL-safe alphabet (§5) differs
/// only in the last two symbols, which is why a blob written in it is read by mapping
/// those two back.
const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The bytes a blob names, decoded here independently of the service.
///
/// `None` when the blob is not base64: a character outside either alphabet, a padding
/// character inside the data, padding that is neither absent nor the amount the final
/// group is short of, or a final group of a single character, which names no byte at all.
/// A final group of two or three characters names one or two bytes and leaves 4 or 2 bits
/// over, and those bits are dropped, which is what "the trailing bits are not data" means
/// (RFC 4648 §3.5 calls them the final quantum's unused bits).
fn decoded_bytes(blob: &str) -> Option<Vec<u8>> {
    let standard: String = blob
        .chars()
        .map(|character| match character {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    let data = standard.trim_end_matches('=');
    let padding = standard.len() - data.len();
    // Padding is absent, or exactly what the final group is short of.
    if padding != 0 && padding != (4 - data.len() % 4) % 4 {
        return None;
    }
    if data.contains('=') || data.len() % 4 == 1 {
        return None;
    }
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut bytes = Vec::new();
    for character in data.chars() {
        let value = u32::try_from(ALPHABET.find(character)?).expect("a 6-bit value");
        bits = (bits << 6) | value;
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push(u8::try_from((bits >> held) & 0xff).expect("a masked byte"));
        }
    }
    Some(bytes)
}

/// The refusal a message this account did not issue carries: the sentence, then the bytes
/// it named as signed 8-bit values.
fn expected_refusal(bytes: &[u8]) -> String {
    let rendered: Vec<String> = bytes
        .iter()
        .map(|byte| (i16::from(*byte) - if *byte > 127 { 256 } else { 0 }).to_string())
        .collect();
    format!("Message is not valid: [{}]", rendered.join(", "))
}

/// The blob that names `bytes` canonically, in the standard alphabet with its padding.
fn canonical(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

/// The same blob with the unused bits of its final group SET rather than zero: the final
/// data character is replaced by the one whose value carries those bits too, so the blob
/// names the same bytes without being their canonical encoding.
///
/// `None` for a byte count that fills whole groups, which has no final group with unused
/// bits to set.
fn trailing_bits_set(bytes: &[u8]) -> Option<String> {
    // Two characters name one byte and leave 4 bits over, three name two bytes and leave
    // 2 bits over; a whole group leaves none.
    let spare = match bytes.len() % 3 {
        1 => 0b1111,
        2 => 0b11,
        _ => return None,
    };
    let encoded = canonical(bytes);
    let data: String = encoded.trim_end_matches('=').to_string();
    let last = data.chars().next_back()?;
    let value = ALPHABET.find(last)? | spare;
    let raised = ALPHABET.chars().nth(value)?;
    let mut blob: String = data[..data.len() - last.len_utf8()].to_string();
    blob.push(raised);
    Some(blob)
}

/// The same blob in the URL-safe alphabet, which is how an encoded authorization message
/// travels.
fn url_safe(blob: &str) -> String {
    blob.replace('+', "-").replace('/', "_")
}

/// `count` bytes from a seeded linear congruential generator (the constants are Numerical
/// Recipes'), so a blob is reproducible from the seed an assertion prints.
fn pseudo_random_bytes(seed: u64, count: usize) -> Vec<u8> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            u8::try_from((state >> 24) & 0xff).expect("a masked byte fits in a u8")
        })
        .collect()
}

/// One `DecodeAuthorizationMessage` call.
async fn decode(
    router: &smithy_server_runtime::MultiServiceRouter,
    encoded_message: &str,
) -> (u16, String) {
    call(
        router,
        LONG_TERM_KEY,
        "DecodeAuthorizationMessage",
        &[("EncodedMessage", encoded_message.to_string())],
    )
    .await
}

/// RULE: the refusal reports the bytes the blob NAMES, read with a decoder's latitude —
/// so a blob whose padding is missing, or whose final group's unused bits are set, is
/// read as the bytes it names and not as its own characters. Only a blob that is not
/// base64 at all reports its own characters.
#[tokio::test]
async fn an_undecodable_message_is_refused_reporting_the_bytes_it_names() {
    let (router, _engine) = emulator();

    // Byte counts that fill a whole group, leave one byte over, and leave two over, so
    // every length of final group appears.
    let whole = pseudo_random_bytes(0x5eed_1001, 24);
    let one_over = pseudo_random_bytes(0x5eed_1002, 25);
    let two_over = pseudo_random_bytes(0x5eed_1003, 26);

    let mut rows: Vec<(String, String)> = Vec::new();
    for (bytes, length) in [
        (&whole, "a whole final group"),
        (&one_over, "one byte over"),
        (&two_over, "two bytes over"),
    ] {
        rows.push((
            format!("canonical, padded, standard alphabet, {length}"),
            canonical(bytes),
        ));
        rows.push((
            format!("canonical with its padding stripped, {length}"),
            canonical(bytes).trim_end_matches('=').to_string(),
        ));
        rows.push((
            format!("the URL-safe alphabet with no padding, {length}"),
            url_safe(canonical(bytes).trim_end_matches('=')),
        ));
        if let Some(raised) = trailing_bits_set(bytes) {
            rows.push((
                format!("the final group's unused bits set, unpadded, {length}"),
                raised.clone(),
            ));
            rows.push((
                format!("the same, in the URL-safe alphabet, {length}"),
                url_safe(&raised),
            ));
        }
    }
    // A word-like blob: base64 all the same, in the URL-safe alphabet, unpadded, and no
    // canonical encoding of the bytes it names — the class a client sends when it passes
    // something that is not an encoded authorization message at all.
    rows.push((
        "a word-like blob in the URL-safe alphabet".to_string(),
        "emulator-probe-not-an-authorization-message".to_string(),
    ));
    // Not base64: the only class whose own characters are reported.
    rows.push((
        "a character outside both alphabets".to_string(),
        "abcd*efg".to_string(),
    ));
    rows.push((
        "a final group of a single character".to_string(),
        "abcde".to_string(),
    ));
    rows.push((
        "a padding character inside the data".to_string(),
        "ab=dabcd".to_string(),
    ));
    rows.push((
        "a blob carrying a line break".to_string(),
        "abcdefgh\nijkl".to_string(),
    ));
    rows.push((
        "the shortest message the model allows".to_string(),
        "A".to_string(),
    ));

    for (case, blob) in &rows {
        let named = decoded_bytes(blob);
        let expected = named.clone().unwrap_or_else(|| blob.as_bytes().to_vec());
        let (status, body) = decode(&router, blob).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidAuthorizationMessageException")),
            "case {case:?}: expected a 400 InvalidAuthorizationMessageException for {blob:?}, \
             got {status} {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message,
            expected_refusal(&expected),
            "case {case:?}: the refusal must report the {} bytes {blob:?} names, got {message:?}",
            expected.len()
        );
    }
}

/// INVARIANT: two blobs that name the same bytes are refused with the same byte listing,
/// whichever alphabet each is written in, whether or not it is padded, and whether or not
/// its final group's unused bits are zero — because what the service reads is the bytes,
/// not the characters.
///
/// A bounded sweep over byte counts, so each row has a different final group; the seed is
/// printed by every assertion.
#[tokio::test]
async fn blobs_naming_the_same_bytes_are_refused_alike() {
    let (router, _engine) = emulator();
    let seed: u64 = 0x5eed_2a11;

    for case in 0..9_u64 {
        let length = usize::try_from(case + 1).expect("a small length");
        let bytes = pseudo_random_bytes(seed.wrapping_add(case), length);
        let padded = canonical(&bytes);
        let mut spellings = vec![
            padded.clone(),
            padded.trim_end_matches('=').to_string(),
            url_safe(padded.trim_end_matches('=')),
        ];
        if let Some(raised) = trailing_bits_set(&bytes) {
            spellings.push(raised.clone());
            spellings.push(url_safe(&raised));
        }

        let expected = expected_refusal(&bytes);
        for blob in &spellings {
            let (status, body) = decode(&router, blob).await;
            assert_eq!(
                status, 400,
                "case seed {seed:#x}+{case} ({length} bytes, blob {blob:?}): expected a 400 \
                 refusal, got {status} {body}"
            );
            let message = member("same bytes", &body, "Message");
            assert_eq!(
                message, expected,
                "case seed {seed:#x}+{case} ({length} bytes, blob {blob:?}): every spelling of \
                 the same bytes must be refused alike, got {message:?}"
            );
        }
    }
}
