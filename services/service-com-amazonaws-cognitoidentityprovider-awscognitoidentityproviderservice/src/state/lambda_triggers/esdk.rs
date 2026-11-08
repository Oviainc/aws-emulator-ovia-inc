// SPDX-License-Identifier: CC0-1.0

//! The encrypted message a custom sender trigger's code travels in: the AWS Encryption SDK
//! message format, written here and read back here.
//!
//! A pool that routes its messages to a custom sender hands the function the code "encrypted
//! with the key that you provided in the `KMSKeyID` parameter" (`LambdaConfigType$KMSKeyID`,
//! this service's model, documents that key as the one "Amazon Cognito uses […] to encrypt
//! codes and temporary passwords"), and the function decrypts it with a client of that same
//! format. So the bytes are not an opaque blob of this emulator's own choosing: they are the
//! published format a decrypting client parses, and this module is the ONE place they are
//! laid out.
//!
//! What is written:
//!
//! * message format VERSION 2, which is the version of every message whose algorithm suite
//!   commits to its data key — the suites a current client accepts by default;
//! * algorithm suite [`ALGORITHM_SUITE_ID`]: AES-256-GCM content encryption, keys derived
//!   with HKDF-SHA512, a key commitment and NO trailing signature, so a message carries no
//!   footer and a reader needs no public key beside it;
//! * one encrypted data key, whose provider is [`KEY_PROVIDER_ID`] and whose provider info
//!   is the key the pool configured — that is what tells the reader which key service call
//!   recovers the data key;
//! * framed content, one frame, because a verification code is far shorter than a frame.
//!
//! The two directions are each other's inverse ([`seal`] and [`parse`] + [`Message::open`]),
//! which is what the round-trip tests below pin; nothing else in this crate reads a message.

use ::ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ::ring::hkdf::{Salt, HKDF_SHA512};

/// The message format this module writes: the version every committing algorithm suite's
/// message carries.
const MESSAGE_FORMAT_VERSION: u8 = 0x02;

/// The algorithm suite every message written here uses: AES-256-GCM with keys derived by
/// HKDF-SHA512 and a key commitment, and no trailing signature.
pub const ALGORITHM_SUITE_ID: u16 = 0x0478;

/// The key provider of an encrypted data key the key service holds the wrapping key for.
pub const KEY_PROVIDER_ID: &str = "aws-kms";

/// The content type of a message whose body is carried in frames.
const CONTENT_TYPE_FRAMED: u8 = 0x02;

/// The length of the data key the content key is derived from, which is the key length of
/// [`ALGORITHM_SUITE_ID`].
pub const DATA_KEY_LENGTH: usize = 32;

/// The length of a message id, of a commitment value and of a derived key under this suite.
pub const MESSAGE_ID_LENGTH: usize = 32;

/// The length of an AES-GCM authentication tag.
const TAG_LENGTH: usize = 16;

/// The frame length a written message declares. One frame holds the whole code.
const FRAME_LENGTH: u32 = 4096;

/// The label that separates the content key from the commitment value derived from the same
/// pseudo-random key.
const DERIVE_KEY_LABEL: &[u8] = b"DERIVEKEY";

/// The label of the commitment value, which a reader recomputes to check that the data key
/// it recovered is the one the writer used.
const COMMIT_KEY_LABEL: &[u8] = b"COMMITKEY";

/// The additional data that binds a frame to its place in the message: the last frame of a
/// framed body says so.
const FINAL_FRAME_LABEL: &[u8] = b"AWSKMSEncryptionClient Final Frame";

/// The marker that precedes the sequence number of a framed body's last frame.
const FINAL_FRAME_MARKER: u32 = 0xFFFF_FFFF;

/// The sequence number of the first frame, which is the only frame a code needs.
const FIRST_FRAME: u32 = 1;

/// Why a message could not be written or read.
///
/// It carries the sentence the caller reports, because what a failure here leaves an
/// operation to answer is the operation's own to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFault(String);

impl MessageFault {
    /// What went wrong, as a sentence.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl ::std::fmt::Display for MessageFault {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The output length one HKDF expansion asks for.
struct Length(usize);

impl ::ring::hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

/// The content key and the commitment value `data_key` derives for `message_id`.
///
/// Both come from ONE extraction, salted by the message id, so a different message with the
/// same data key encrypts under a different content key; the labels are what keep the two
/// expansions apart, and the suite id in the content key's label is what keeps a key derived
/// for this suite from being usable under another.
fn derived_keys(
    message_id: &[u8],
    data_key: &[u8],
) -> Result<([u8; DATA_KEY_LENGTH], [u8; MESSAGE_ID_LENGTH]), MessageFault> {
    if data_key.len() != DATA_KEY_LENGTH {
        return Err(MessageFault(format!(
            "a data key of {DATA_KEY_LENGTH} bytes is required, got {}",
            data_key.len()
        )));
    }
    let extracted = Salt::new(HKDF_SHA512, message_id).extract(data_key);
    let suite = ALGORITHM_SUITE_ID.to_be_bytes();
    let mut content_key = [0_u8; DATA_KEY_LENGTH];
    extracted
        .expand(&[&suite[..], DERIVE_KEY_LABEL], Length(DATA_KEY_LENGTH))
        .and_then(|derived| derived.fill(&mut content_key))
        .map_err(|_| MessageFault("the content key could not be derived".to_string()))?;
    let mut commitment = [0_u8; MESSAGE_ID_LENGTH];
    extracted
        .expand(&[COMMIT_KEY_LABEL], Length(MESSAGE_ID_LENGTH))
        .and_then(|derived| derived.fill(&mut commitment))
        .map_err(|_| MessageFault("the key commitment could not be derived".to_string()))?;
    Ok((content_key, commitment))
}

/// An AES-256-GCM key over `key`.
fn content_cipher(key: &[u8]) -> Result<LessSafeKey, MessageFault> {
    UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| MessageFault("the content key could not be used".to_string()))
}

/// The initialization vector of the frame numbered `sequence`: the sequence number in the
/// last eight bytes, so no two frames of one message share one.
fn frame_nonce(sequence: u32) -> Nonce {
    let mut iv = [0_u8; NONCE_LEN];
    iv[NONCE_LEN - 8..].copy_from_slice(&u64::from(sequence).to_be_bytes());
    Nonce::assume_unique_for_key(iv)
}

/// The initialization vector the header's own authentication uses, which is fixed because a
/// message has exactly one header.
fn header_nonce() -> Nonce {
    Nonce::assume_unique_for_key([0_u8; NONCE_LEN])
}

/// The additional data that binds the one frame of `message_id` to its content length.
fn frame_aad(message_id: &[u8], sequence: u32, content_length: usize) -> Vec<u8> {
    let mut aad = Vec::with_capacity(message_id.len() + FINAL_FRAME_LABEL.len() + 12);
    aad.extend_from_slice(message_id);
    aad.extend_from_slice(FINAL_FRAME_LABEL);
    aad.extend_from_slice(&sequence.to_be_bytes());
    aad.extend_from_slice(&(content_length as u64).to_be_bytes());
    aad
}

/// A two-byte length prefix and the bytes it measures.
fn length_prefixed(bytes: &[u8]) -> Result<Vec<u8>, MessageFault> {
    let length = u16::try_from(bytes.len())
        .map_err(|_| MessageFault("a message field is longer than its length prefix".to_string()))?;
    let mut field = length.to_be_bytes().to_vec();
    field.extend_from_slice(bytes);
    Ok(field)
}

/// Write the message that carries `plaintext` under `data_key`.
///
/// `encrypted_data_key` is that same data key as the key service wrapped it, and
/// `key_provider_info` names the wrapping key, so a reader knows which key to ask to unwrap
/// it. `message_id` is this message's own, which salts the key derivation and binds every
/// frame to this message; it must be fresh per message, and it is an input so a test can
/// pin the bytes a given message becomes.
pub fn seal(
    data_key: &[u8],
    encrypted_data_key: &[u8],
    key_provider_info: &str,
    message_id: &[u8; MESSAGE_ID_LENGTH],
    plaintext: &[u8],
) -> Result<Vec<u8>, MessageFault> {
    if plaintext.len() >= FRAME_LENGTH as usize {
        return Err(MessageFault(format!(
            "a message of {} bytes does not fit one frame",
            plaintext.len()
        )));
    }
    let (content_key, commitment) = derived_keys(message_id, data_key)?;
    let cipher = content_cipher(&content_key)?;

    let mut header = Vec::new();
    header.push(MESSAGE_FORMAT_VERSION);
    header.extend_from_slice(&ALGORITHM_SUITE_ID.to_be_bytes());
    header.extend_from_slice(message_id);
    // No encryption context travels with the message, so the field that would carry one is
    // empty and nothing follows its length.
    header.extend_from_slice(&0_u16.to_be_bytes());
    header.extend_from_slice(&1_u16.to_be_bytes());
    header.extend_from_slice(&length_prefixed(KEY_PROVIDER_ID.as_bytes())?);
    header.extend_from_slice(&length_prefixed(key_provider_info.as_bytes())?);
    header.extend_from_slice(&length_prefixed(encrypted_data_key)?);
    header.push(CONTENT_TYPE_FRAMED);
    header.extend_from_slice(&FRAME_LENGTH.to_be_bytes());
    header.extend_from_slice(&commitment);

    // The header is authenticated but not encrypted: every reader needs to read it before
    // it can recover a key, and the tag is what says it was not altered.
    let mut nothing: [u8; 0] = [];
    let header_tag = cipher
        .seal_in_place_separate_tag(header_nonce(), Aad::from(&header), &mut nothing)
        .map_err(|_| MessageFault("the message header could not be authenticated".to_string()))?;

    let mut body = plaintext.to_vec();
    let body_tag = cipher
        .seal_in_place_separate_tag(
            frame_nonce(FIRST_FRAME),
            Aad::from(frame_aad(message_id, FIRST_FRAME, plaintext.len())),
            &mut body,
        )
        .map_err(|_| MessageFault("the message body could not be encrypted".to_string()))?;

    let mut message = header;
    message.extend_from_slice(header_tag.as_ref());
    message.extend_from_slice(&FINAL_FRAME_MARKER.to_be_bytes());
    message.extend_from_slice(&FIRST_FRAME.to_be_bytes());
    message.extend_from_slice(frame_nonce(FIRST_FRAME).as_ref());
    message.extend_from_slice(
        &u32::try_from(plaintext.len())
            .map_err(|_| MessageFault("the message is too long to frame".to_string()))?
            .to_be_bytes(),
    );
    message.extend_from_slice(&body);
    message.extend_from_slice(body_tag.as_ref());
    Ok(message)
}

/// One written message, read back: what a reader learns before it holds any key, plus the
/// parts [`Message::open`] needs once it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The key provider of the one encrypted data key, which decides who can unwrap it.
    pub key_provider_id: String,
    /// The wrapping key, as the writer named it.
    pub key_provider_info: String,
    /// The data key as the key service wrapped it.
    pub encrypted_data_key: Vec<u8>,
    /// This message's own id, which salts the key derivation.
    message_id: [u8; MESSAGE_ID_LENGTH],
    /// The commitment the header carries, which the recovered data key must reproduce.
    commitment: [u8; MESSAGE_ID_LENGTH],
    /// The header bytes, which the header's tag is over.
    header: Vec<u8>,
    /// The header's authentication tag.
    header_tag: Vec<u8>,
    /// The encrypted content of the one frame, with its tag.
    frame: Vec<u8>,
    /// The content length that frame declares.
    content_length: usize,
    /// The sequence number of that frame.
    sequence: u32,
}

/// A cursor over the bytes of a message being read.
struct Reader<'bytes> {
    bytes: &'bytes [u8],
    at: usize,
}

impl<'bytes> Reader<'bytes> {
    fn new(bytes: &'bytes [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'bytes [u8], MessageFault> {
        let end = self.at.checked_add(count).ok_or_else(truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or_else(truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, MessageFault> {
        let bytes: [u8; 1] = self.take(1)?.try_into().map_err(|_| truncated())?;
        Ok(u8::from_be_bytes(bytes))
    }

    fn be_u16(&mut self) -> Result<u16, MessageFault> {
        let bytes: [u8; 2] = self.take(2)?.try_into().map_err(|_| truncated())?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn be_u32(&mut self) -> Result<u32, MessageFault> {
        let bytes: [u8; 4] = self.take(4)?.try_into().map_err(|_| truncated())?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn field(&mut self) -> Result<&'bytes [u8], MessageFault> {
        let length = self.be_u16()? as usize;
        self.take(length)
    }

    fn rest(&mut self) -> &'bytes [u8] {
        let rest = &self.bytes[self.at.min(self.bytes.len())..];
        self.at = self.bytes.len();
        rest
    }
}

/// The fault of a message that ends before a field it declares.
fn truncated() -> MessageFault {
    MessageFault("the message ends inside a field it declares".to_string())
}

/// Read `bytes` as a message, or report why they are not one.
pub fn parse(bytes: &[u8]) -> Result<Message, MessageFault> {
    let mut reader = Reader::new(bytes);
    let version = reader.byte()?;
    if version != MESSAGE_FORMAT_VERSION {
        return Err(MessageFault(format!(
            "message format version {MESSAGE_FORMAT_VERSION} is required, got {version}"
        )));
    }
    let suite = reader.be_u16()?;
    if suite != ALGORITHM_SUITE_ID {
        return Err(MessageFault(format!(
            "algorithm suite {ALGORITHM_SUITE_ID:#06x} is required, got {suite:#06x}"
        )));
    }
    let mut message_id = [0_u8; MESSAGE_ID_LENGTH];
    message_id.copy_from_slice(reader.take(MESSAGE_ID_LENGTH)?);
    let context = reader.field()?;
    if !context.is_empty() {
        return Err(MessageFault(
            "a message written here carries no encryption context".to_string(),
        ));
    }
    let keys = reader.be_u16()?;
    if keys != 1 {
        return Err(MessageFault(format!(
            "one encrypted data key is required, got {keys}"
        )));
    }
    let key_provider_id = String::from_utf8(reader.field()?.to_vec())
        .map_err(|_| MessageFault("the key provider is not text".to_string()))?;
    let key_provider_info = String::from_utf8(reader.field()?.to_vec())
        .map_err(|_| MessageFault("the key provider info is not text".to_string()))?;
    let encrypted_data_key = reader.field()?.to_vec();
    let content_type = reader.byte()?;
    if content_type != CONTENT_TYPE_FRAMED {
        return Err(MessageFault(format!(
            "a framed body is required, got content type {content_type}"
        )));
    }
    let _frame_length = reader.be_u32()?;
    let mut commitment = [0_u8; MESSAGE_ID_LENGTH];
    commitment.copy_from_slice(reader.take(MESSAGE_ID_LENGTH)?);
    let header = bytes
        .get(..reader.at)
        .ok_or_else(truncated)?
        .to_vec();
    let header_tag = reader.take(TAG_LENGTH)?.to_vec();
    let marker = reader.be_u32()?;
    if marker != FINAL_FRAME_MARKER {
        return Err(MessageFault(
            "the body's one frame must be a final frame".to_string(),
        ));
    }
    let sequence = reader.be_u32()?;
    let _iv = reader.take(NONCE_LEN)?;
    let content_length = reader.be_u32()? as usize;
    let frame = reader.rest().to_vec();
    if frame.len() != content_length + TAG_LENGTH {
        return Err(MessageFault(format!(
            "the frame declares {content_length} bytes of content, and carries {}",
            frame.len().saturating_sub(TAG_LENGTH)
        )));
    }
    Ok(Message {
        key_provider_id,
        key_provider_info,
        encrypted_data_key,
        message_id,
        commitment,
        header,
        header_tag,
        frame,
        content_length,
        sequence,
    })
}

impl Message {
    /// The content of this message, under the `data_key` the key service unwrapped.
    ///
    /// Every check the format provides is made: the data key must reproduce the commitment
    /// the header carries, the header must authenticate under the key derived from it, and
    /// the frame must authenticate under its own place in the message. A data key that is
    /// not the one the message was written under fails the first of them.
    pub fn open(&self, data_key: &[u8]) -> Result<Vec<u8>, MessageFault> {
        let (content_key, commitment) = derived_keys(&self.message_id, data_key)?;
        if commitment != self.commitment {
            return Err(MessageFault(
                "the data key is not the one this message commits to".to_string(),
            ));
        }
        let cipher = content_cipher(&content_key)?;
        let mut header_tag = self.header_tag.clone();
        cipher
            .open_in_place(
                header_nonce(),
                Aad::from(&self.header),
                &mut header_tag,
            )
            .map_err(|_| MessageFault("the message header is not authentic".to_string()))?;
        let mut frame = self.frame.clone();
        let content = cipher
            .open_in_place(
                frame_nonce(self.sequence),
                Aad::from(frame_aad(&self.message_id, self.sequence, self.content_length)),
                &mut frame,
            )
            .map_err(|_| MessageFault("the message body is not authentic".to_string()))?;
        Ok(content.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data key of 32 distinct bytes.
    fn data_key(seed: u8) -> Vec<u8> {
        (0..DATA_KEY_LENGTH)
            .map(|index| seed.wrapping_add(index as u8))
            .collect()
    }

    /// A message id of 32 distinct bytes.
    fn message_id(seed: u8) -> [u8; MESSAGE_ID_LENGTH] {
        let mut id = [0_u8; MESSAGE_ID_LENGTH];
        for (index, byte) in id.iter_mut().enumerate() {
            *byte = seed.wrapping_mul(3).wrapping_add(index as u8);
        }
        id
    }

    /// RULE: a message written here reads back as what was written, under the data key it
    /// was written with, whatever the content and whoever wrapped the key.
    ///
    /// The rows span what the format generalises over: the content (a six-digit code, an
    /// eight-digit one, one byte, bytes that are not text, the longest content one frame
    /// takes), the wrapped key's bytes and the key the provider info names.
    #[test]
    fn a_written_message_reads_back_as_what_was_written() {
        let longest = vec![b'9'; FRAME_LENGTH as usize - 1];
        let cases: [(&str, &[u8], &str); 5] = [
            ("a six-digit code", b"123456", "arn:aws:kms:us-east-1:000000000000:key/one"),
            ("an eight-digit code", b"87654321", "arn:aws:kms:eu-west-1:000000000000:key/two"),
            ("one byte", b"7", "alias/sender"),
            ("bytes that are not text", &[0, 159, 146, 150], "mrk-0123456789"),
            ("the longest content one frame takes", &longest, "arn:aws:kms:ap-south-1:000000000000:key/three"),
        ];
        for (index, (case, content, key)) in cases.into_iter().enumerate() {
            let key_bytes = data_key(index as u8);
            let wrapped = format!("wrapped-{index}").into_bytes();
            let written = seal(&key_bytes, &wrapped, key, &message_id(index as u8), content)
                .unwrap_or_else(|fault| panic!("case {case:?}: a message is written: {fault}"));
            let read = parse(&written)
                .unwrap_or_else(|fault| panic!("case {case:?}: the message parses: {fault}"));
            assert_eq!(
                (read.key_provider_id.as_str(), read.key_provider_info.as_str()),
                (KEY_PROVIDER_ID, key),
                "case {case:?}: expected the provider and the key the writer named, got {read:?}"
            );
            assert_eq!(
                read.encrypted_data_key, wrapped,
                "case {case:?}: expected the wrapped data key, got {:?}",
                read.encrypted_data_key
            );
            let opened = read
                .open(&key_bytes)
                .unwrap_or_else(|fault| panic!("case {case:?}: the message opens: {fault}"));
            assert_eq!(
                opened, content,
                "case {case:?}: expected the content that was written, got {opened:?}"
            );
        }
    }

    /// RULE: the header states the format a reader dispatches on before it holds any key.
    #[test]
    fn the_header_states_the_format_and_the_suite() {
        let written = seal(&data_key(5), b"wrapped", "alias/key", &message_id(5), b"135790")
            .expect("a message is written");
        assert_eq!(
            written[0], MESSAGE_FORMAT_VERSION,
            "expected the message format version first, got {:?}",
            written.first()
        );
        assert_eq!(
            u16::from_be_bytes([written[1], written[2]]),
            ALGORITHM_SUITE_ID,
            "expected the committing algorithm suite, got {:#06x}",
            u16::from_be_bytes([written[1], written[2]])
        );
    }

    /// RULE: every key but the one a message was written under is refused, and so is a
    /// message a byte of which changed.
    #[test]
    fn a_message_is_refused_under_any_other_key_or_altered_bytes() {
        let written = seal(&data_key(9), b"wrapped", "alias/key", &message_id(9), b"246810")
            .expect("a message is written");
        let read = parse(&written).expect("the message parses");
        for (case, key) in [
            ("another data key", data_key(10)),
            ("a data key of the wrong length", vec![7; 16]),
        ] {
            let opened = read.open(&key);
            assert!(
                opened.is_err(),
                "case {case:?}: expected the message to be refused, got {opened:?}"
            );
        }
        for altered in [0_usize, 1, 40, written.len() - 1] {
            let mut bytes = written.clone();
            bytes[altered] ^= 0xFF;
            let refused = parse(&bytes).and_then(|message| message.open(&data_key(9)));
            assert!(
                refused.is_err(),
                "case \"byte {altered} altered\": expected the message to be refused, got \
                 {refused:?}"
            );
        }
    }

    /// RULE: content that does not fit one frame is not written at all.
    #[test]
    fn content_longer_than_one_frame_is_not_written() {
        let too_long = vec![b'0'; FRAME_LENGTH as usize];
        let written = seal(&data_key(1), b"wrapped", "alias/key", &message_id(1), &too_long);
        assert!(
            written.is_err(),
            "expected content of a whole frame to be refused, got {} bytes",
            written.map(|bytes| bytes.len()).unwrap_or_default()
        );
    }

    /// RULE: bytes that are not a message of this format are REPORTED as such — the reader
    /// is total, so no input ends a read any way but with a fault.
    ///
    /// The rows span the ways bytes fail to be a message: nothing at all, a prefix cut at
    /// every field boundary the header declares, a version and a suite this reader does not
    /// write, a message carrying trailing bytes, and bytes far larger than any message.
    #[test]
    fn bytes_that_are_not_a_message_are_reported_rather_than_read() {
        let written = seal(&data_key(3), b"wrapped", "alias/key", &message_id(3), b"024680")
            .expect("a message is written");
        let mut cases: Vec<(String, Vec<u8>)> = vec![
            ("no bytes at all".to_string(), Vec::new()),
            ("one byte".to_string(), vec![MESSAGE_FORMAT_VERSION]),
            (
                "another message format version".to_string(),
                [vec![0x01], written[1..].to_vec()].concat(),
            ),
            (
                "another algorithm suite".to_string(),
                [vec![written[0], 0x00, 0x14], written[3..].to_vec()].concat(),
            ),
            (
                "a message with trailing bytes".to_string(),
                [written.clone(), vec![0x00]].concat(),
            ),
            (
                "far more bytes than any message".to_string(),
                vec![0xAB; 1 << 20],
            ),
        ];
        // Every truncation of a message that IS well formed: each one ends inside a field
        // the header declares, and none of them may be read as a message.
        for cut in [1_usize, 2, 3, 20, 35, 40, 60, 90, written.len() - 1] {
            cases.push((format!("truncated at {cut} bytes"), written[..cut].to_vec()));
        }
        for (case, bytes) in cases {
            let read = parse(&bytes).and_then(|message| message.open(&data_key(3)));
            assert!(
                read.is_err(),
                "case {case:?}: expected the bytes to be refused, got {read:?}"
            );
        }
    }
}
