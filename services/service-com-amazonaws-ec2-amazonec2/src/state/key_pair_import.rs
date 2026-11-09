// SPDX-License-Identifier: CC0-1.0

//! The PUBLIC key material an `ImportKeyPair` carries: what counts as a key EC2 can take,
//! which `KeyType` it is, and the fingerprint EC2 publishes for an IMPORTED key of that
//! type.
//!
//! "Imports the public key from an RSA or ED25519 key pair that you created using a
//! third-party tool. You give Amazon Web Services only the public key. The private key is
//! never transferred between you and Amazon Web Services" (`ImportKeyPair`) — so nothing
//! here reads, derives or holds a private key, and the two algorithms that sentence names
//! are the only ones a key can be read as.
//!
//! The material is read by [`ssh_key`], the crate that implements the OpenSSH key formats,
//! rather than by matching on the text: what counts as a key is the format itself. The two
//! public-key forms the import requirements name are both read — the
//! `key-type key-body [comment]` line `ssh-keygen` writes and the armoured "SSH public key
//! file format as specified in RFC 4716".
//!
//! An IMPORTED key pair's fingerprint rule is not the GENERATED one
//! ([`super::key_pair_material`], which fingerprints the private key EC2 made): "if you used
//! `ImportKeyPair` to provide Amazon Web Services the public key: for RSA key pairs, the key
//! fingerprint is the MD5 public key fingerprint as specified in section 4 of RFC4716. For
//! ED25519 key pairs, the key fingerprint is the base64-encoded SHA-256 digest, which is the
//! default for OpenSSH" (`KeyPairInfo$KeyFingerprint`). The STORE both kinds of key pair are
//! kept in is [`super::key_pairs`].
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use ::md5::Digest as _;

use super::{colon_separated_hex, openssh_public_key_line, KEY_TYPE_ED25519, KEY_TYPE_RSA};

/// The RSA key lengths an import admits, in bits: "supported lengths: 1024, 2048, and 4096"
/// (Amazon EC2 User Guide, "Create a key pair and import the public key to Amazon EC2" —
/// the key requirements `ImportKeyPair` points a caller at).
pub const IMPORTABLE_RSA_KEY_BITS: [usize; 3] = [1024, 2048, 4096];

/// The most bytes of public key material an import reads.
///
/// The longest key those requirements admit is a 4096-bit RSA key, whose OpenSSH line is
/// under a kilobyte and whose armoured file is not much longer, so this is well above every
/// key that can be imported and bounds the work a caller can ask the reader for. Material
/// above it is not a key in a supported format ([`read_public_key_material`]).
pub const MAX_PUBLIC_KEY_MATERIAL_BYTES: usize = 4096;

/// The code EC2 answers for public key material it cannot import: "the format of the public
/// key you are attempting to upload is not valid" (AWS's `InvalidKeyPair.Format` error-code
/// entry).
pub const INVALID_KEY_PAIR_FORMAT: &str = "InvalidKeyPair.Format";

/// Refuse an import whose material is not a public key EC2 can take: HTTP 400
/// [`INVALID_KEY_PAIR_FORMAT`].
///
/// ONE sentence for every way the material fails [`read_public_key_material`] — not a key at
/// all, a key of an algorithm an import does not take, an RSA key of a length the
/// requirements do not list, material longer than any of them — because what EC2 reports is
/// that the key it was given is not one it can read: the sentence names neither which clause
/// failed nor the material itself, and carries no variable part.
pub fn invalid_key_pair_format<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_KEY_PAIR_FORMAT,
        400,
        UNREADABLE_PUBLIC_KEY_MESSAGE,
    )
}

/// EC2's sentence for public key material it cannot read as a key.
pub const UNREADABLE_PUBLIC_KEY_MESSAGE: &str = "Key is not in valid OpenSSH public key format";

/// A public key an import can take: the one-line OpenSSH form it is stored as, the `KeyType`
/// it reports, and the fingerprint EC2 publishes for an imported key of that type.
#[derive(Debug, Clone)]
pub struct ImportedPublicKey {
    /// The key as the single-line OpenSSH public key, which is the form "the public key
    /// material" (`KeyPairInfo$PublicKey`) takes — written by the key crate from the key it
    /// parsed rather than copied out of the request, so one key reaches the store in one
    /// spelling however the caller armoured it.
    pub public_key: String,
    /// The pair's `KeyType`: [`KEY_TYPE_RSA`] or [`KEY_TYPE_ED25519`], a word out of the
    /// modelled catalog rather than anything the request said.
    pub key_type: &'static str,
    /// The pair's `KeyFingerprint`, as [`imported_key_fingerprint`] derives it from the key
    /// material and from nothing else.
    pub key_fingerprint: String,
}

/// The public key `material` carries, or `None` when the material is not one an import can
/// take.
///
/// Exactly two algorithms are read, because `ImportKeyPair` imports "the public key from an
/// RSA or ED25519 key pair", and an RSA key's length must be one the requirements list
/// ([`IMPORTABLE_RSA_KEY_BITS`]). Anything else — material that is not UTF-8, is longer than
/// [`MAX_PUBLIC_KEY_MATERIAL_BYTES`], is no key at all, is a key of another algorithm, or is
/// an RSA key of an unlisted length — is not a key this operation can take, and its caller
/// answers [`invalid_key_pair_format`].
pub fn read_public_key_material(material: &[u8]) -> Option<ImportedPublicKey> {
    if material.len() > MAX_PUBLIC_KEY_MATERIAL_BYTES {
        return None;
    }
    let text = ::std::str::from_utf8(material).ok()?;
    let key = parse_public_key(text.trim())?;
    let key_type = match key.key_data() {
        ::ssh_key::public::KeyData::Rsa(rsa) => {
            if !IMPORTABLE_RSA_KEY_BITS.contains(&integer_bits(rsa.n.as_positive_bytes()?)) {
                return None;
            }
            KEY_TYPE_RSA
        }
        ::ssh_key::public::KeyData::Ed25519(_) => KEY_TYPE_ED25519,
        // Every other algorithm the OpenSSH formats carry (ECDSA, DSA, the security-key
        // variants): a readable key, but not one of the two an import takes.
        _ => return None,
    };
    Some(ImportedPublicKey {
        // The key crate writes the stored spelling, so one key reaches the store in one
        // form however the caller armoured it — through the same helper a GENERATED pair's
        // public key is written with.
        public_key: openssh_public_key_line(&key)?,
        key_type,
        key_fingerprint: imported_key_fingerprint(&key)?,
    })
}

/// The `KeyFingerprint` EC2 publishes for the IMPORTED key `key`.
///
/// * RSA — "the MD5 public key fingerprint as specified in section 4 of RFC4716"
///   (`KeyPairInfo$KeyFingerprint`): the MD5 digest of the key's DER
///   `SubjectPublicKeyInfo`, as the colon-separated lowercase hex pairs EC2 publishes a hex
///   fingerprint in. That is the value `openssl rsa -in key -pubout -outform DER | openssl
///   md5 -c` prints, which is how the User Guide has a caller verify an imported RSA pair's
///   fingerprint.
/// * ED25519 — "the base64-encoded SHA-256 digest, which is the default for OpenSSH": the
///   `SHA256:<base64>` form `ssh-keygen -l` prints, which is what that default IS, and the
///   same rendering a generated Ed25519 key pair's fingerprint takes
///   ([`super::key_pair_material`]) — one sentence of the model, one rendering.
///
/// Both are computed from the key material ALONE, so one key always fingerprints to one
/// string and every later read of the pair answers that one value.
fn imported_key_fingerprint(key: &::ssh_key::PublicKey) -> Option<String> {
    match key.key_data() {
        ::ssh_key::public::KeyData::Rsa(rsa) => Some(colon_separated_hex(&::md5::Md5::digest(
            rsa_subject_public_key_info(rsa)?,
        ))),
        _ => Some(key.fingerprint(::ssh_key::HashAlg::Sha256).to_string()),
    }
}

/// The DER `SubjectPublicKeyInfo` of the RSA public key `rsa` — the X.509 public key
/// `openssl rsa -pubout -outform DER` writes, which an imported RSA pair's MD5 fingerprint
/// is taken over.
///
/// The structure is written by the crates that implement the standards it comes from: the
/// PKCS#1 `RSAPublicKey` body by [`pkcs1`] (RFC 8017), under the `rsaEncryption` algorithm
/// identifier that crate publishes, and the `SubjectPublicKeyInfo` that carries it by
/// [`spki`] (RFC 5280).
fn rsa_subject_public_key_info(rsa: &::ssh_key::public::RsaPublicKey) -> Option<Vec<u8>> {
    use ::pkcs1::der::Encode as _;

    let body = ::pkcs1::RsaPublicKey {
        modulus: ::pkcs1::der::asn1::UintRef::new(rsa.n.as_positive_bytes()?).ok()?,
        public_exponent: ::pkcs1::der::asn1::UintRef::new(rsa.e.as_positive_bytes()?).ok()?,
    }
    .to_der()
    .ok()?;
    ::spki::SubjectPublicKeyInfo {
        algorithm: ::spki::AlgorithmIdentifier {
            oid: ::pkcs1::ALGORITHM_OID,
            parameters: Some(::pkcs1::der::asn1::AnyRef::NULL),
        },
        subject_public_key: ::pkcs1::der::asn1::BitStringRef::new(0, &body).ok()?,
    }
    .to_der()
    .ok()
}

/// How many bits the big-endian unsigned integer `bytes` holds — an RSA modulus's key
/// length, which is what the import requirements list.
fn integer_bits(bytes: &[u8]) -> usize {
    let Some(leading) = bytes.iter().position(|byte| *byte != 0) else {
        return 0;
    };
    let Some(top) = bytes.get(leading) else {
        return 0;
    };
    bytes
        .len()
        .saturating_sub(leading)
        .saturating_mul(8)
        .saturating_sub(top.leading_zeros() as usize)
}

/// The opening line of the armoured public key file format RFC 4716 defines, which the
/// import requirements name as a supported form.
const RFC4716_BEGIN: &str = "---- BEGIN SSH2 PUBLIC KEY ----";

/// Its closing line.
const RFC4716_END: &str = "---- END SSH2 PUBLIC KEY ----";

/// `text` as a public key, in either of the two public-key forms the import requirements
/// name: the single-line OpenSSH form first, then the RFC 4716 armoured file.
fn parse_public_key(text: &str) -> Option<::ssh_key::PublicKey> {
    if let Ok(key) = ::ssh_key::PublicKey::from_openssh(text) {
        return Some(key);
    }
    ::ssh_key::PublicKey::from_bytes(&rfc4716_body(text)?).ok()
}

/// The binary key blob an RFC 4716 armoured public key file carries, or `None` when `text`
/// is not one.
///
/// The file is the begin and end lines around header fields and the base64 body (RFC 4716
/// §3.2): a header is a `Name: value` line, continued while a line ends in a backslash, and
/// the base64 alphabet carries no colon, so a body line is never read as a header. A flat
/// loop over at most [`MAX_PUBLIC_KEY_MATERIAL_BYTES`] of text — no recursion, and no work
/// proportional to anything but the material's own length.
fn rfc4716_body(text: &str) -> Option<Vec<u8>> {
    use ::base64::Engine as _;

    let inner = text
        .strip_prefix(RFC4716_BEGIN)?
        .trim_end()
        .strip_suffix(RFC4716_END)?;
    let mut body = String::new();
    let mut continued = false;
    for line in inner.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if continued || line.contains(':') {
            continued = line.ends_with('\\');
            continue;
        }
        body.push_str(line);
    }
    ::base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()
}
