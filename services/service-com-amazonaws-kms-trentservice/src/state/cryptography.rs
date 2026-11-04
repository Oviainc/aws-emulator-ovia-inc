// SPDX-License-Identifier: CC0-1.0

//! The ciphertext envelope a symmetric KMS key seals a plaintext in: what `Encrypt`
//! writes, what `Decrypt` reads, and the encryption context both of them authenticate.
//!
//! One concern of [`super`]'s store, in its own file: the records and helpers here are
//! reached as `crate::state::…` exactly as they were before, because the parent
//! re-exports them. The AEAD and the CSPRNG come from `ring` — this module composes the
//! envelope, it implements no cryptography of its own.

use super::*;

/// The envelope version this build writes. It is the first byte of every
/// ciphertext, so a blob from another scheme is rejected rather than mis-parsed.
const ENVELOPE_VERSION: u8 = 1;

/// AES-GCM's nonce length, as `ring::aead` defines it for `AES_256_GCM`.
const NONCE_LEN_BYTES: usize = NONCE_LEN;

/// AES-GCM's authentication tag length (`AES_256_GCM.tag_len()`).
const TAG_LEN: usize = 16;

/// A parsed ciphertext envelope: which key sealed it, and the sealed bytes.
pub struct Envelope {
    /// The key id of the KMS key whose material sealed this ciphertext.
    pub key_id: String,
    nonce: [u8; NONCE_LEN_BYTES],
    body: Vec<u8>,
}

/// `material` as an AES-256-GCM key, or `None` when it is not AES material and so
/// has nothing to seal or open with.
fn cipher(material: &KeyMaterial) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, material.aes256()?)
        .ok()
        .map(LessSafeKey::new)
}

/// Seal `plaintext` under `key`'s material, binding `context` into the ciphertext
/// as additional authenticated data.
///
/// Returns `None` only if the AEAD itself refuses the input (a plaintext larger
/// than AES-GCM can carry), which the modelled 4096-byte bound already excludes.
pub fn seal(
    key: &Cmk,
    plaintext: &[u8],
    context: Option<&HashMap<String, String>>,
) -> Option<Vec<u8>> {
    let cipher = cipher(key.effective_material())?;
    let mut nonce = [0u8; NONCE_LEN_BYTES];
    SystemRandom::new().fill(&mut nonce).ok()?;
    let aad = encryption_context_aad(context);

    // `seal_in_place_append_tag` leaves `body` holding the ciphertext followed by
    // the 16-byte authentication tag.
    let mut body = plaintext.to_vec();
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_slice()),
            &mut body,
        )
        .ok()?;

    let id = key.key_id.as_bytes();
    let mut blob = Vec::with_capacity(2 + id.len() + NONCE_LEN_BYTES + body.len());
    blob.push(ENVELOPE_VERSION);
    blob.push(u8::try_from(id.len()).ok()?);
    blob.extend_from_slice(id);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&body);
    Some(blob)
}

/// Read the envelope a `CiphertextBlob` carries, or `None` when the bytes are not
/// one — which is what makes a foreign or truncated blob an
/// `InvalidCiphertextException` instead of a panic.
pub fn parse_envelope(blob: &[u8]) -> Option<Envelope> {
    let (&version, rest) = blob.split_first()?;
    if version != ENVELOPE_VERSION {
        return None;
    }
    let (&id_len, rest) = rest.split_first()?;
    let id_len = usize::from(id_len);
    if id_len == 0 || rest.len() < id_len + NONCE_LEN_BYTES + TAG_LEN {
        return None;
    }
    let (id, rest) = rest.split_at(id_len);
    let key_id = std::str::from_utf8(id).ok()?.to_string();
    let (nonce, body) = rest.split_at(NONCE_LEN_BYTES);
    Some(Envelope {
        key_id,
        nonce: nonce.try_into().ok()?,
        body: body.to_vec(),
    })
}

/// What opening a ciphertext envelope yielded: the plaintext, and which version of
/// the key's material opened it.
pub struct Opened {
    /// The recovered plaintext.
    pub plaintext: Vec<u8>,
    /// The `KeyMaterialId` of the material that opened the ciphertext — the
    /// material that sealed it, which after a rotation need not be the key's
    /// current one.
    pub key_material_id: Option<String>,
}

/// Open an envelope with `key`'s material under `context`.
///
/// Every version of the key's material is tried: the one current now, then the ones
/// rotation replaced, newest first. A KMS key keeps the material it has encrypted
/// under, so a ciphertext sealed before a rotation still decrypts after it, and the
/// authentication tag is what identifies the version that sealed it.
///
/// `None` means no version verified the tag: the ciphertext was sealed by another
/// key, under a different encryption context, or has been altered.
pub fn unseal(
    key: &Cmk,
    envelope: &Envelope,
    context: Option<&HashMap<String, String>>,
) -> Option<Opened> {
    let aad = encryption_context_aad(context);
    key.open_candidates()
        .into_iter()
        .find_map(|material| {
            let cipher = cipher(material)?;
            let mut body = envelope.body.clone();
            let plaintext = cipher
                .open_in_place(
                    Nonce::assume_unique_for_key(envelope.nonce),
                    Aad::from(aad.as_slice()),
                    &mut body,
                )
                .ok()?;
            Some(Opened {
                plaintext: plaintext.to_vec(),
                key_material_id: material.id(),
            })
        })
}

/// The encryption context as additional authenticated data.
///
/// The context is an unordered map, so it is canonicalized before it is
/// authenticated: entries sorted by key, each key and value length-prefixed so no
/// two different maps can encode to the same bytes. An absent context and an empty
/// one authenticate identically, which is what makes them interchangeable.
fn encryption_context_aad(context: Option<&HashMap<String, String>>) -> Vec<u8> {
    let mut entries: Vec<(&str, &str)> = context
        .map(|context| {
            context
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_unstable();

    let mut aad = Vec::new();
    for (key, value) in entries {
        for part in [key, value] {
            let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
            aad.extend_from_slice(&len.to_be_bytes());
            aad.extend_from_slice(part.as_bytes());
        }
    }
    aad
}
