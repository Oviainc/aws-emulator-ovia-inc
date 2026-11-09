// SPDX-License-Identifier: CC0-1.0

//! The key MATERIAL a key pair is made of: a real key of the type the request asked for,
//! the fingerprint that type is fingerprinted with, and the private-key file format the
//! request asked for it in.
//!
//! "Creates an ED25519 or 2048-bit RSA key pair with the specified name and in the
//! specified format" (`CreateKeyPair`), so every byte here is a real key: the RSA key comes
//! from `aws-lc-rs`, the Ed25519 key and the OpenSSH encodings from `ssh-key`, and the
//! PKCS#1 structure of an RSA private key from `pkcs1`. Nothing is synthesised to look like
//! key material — a caller can parse what it is handed and use it as the private half of
//! the public key this service stored.
//!
//! The two fingerprint rules are the key TYPE's, not the operation's: "for RSA key pairs,
//! the key fingerprint is the SHA-1 digest of the DER encoded private key. For ED25519 key
//! pairs, the key fingerprint is the base64-encoded SHA-256 digest, which is the default
//! for OpenSSH, starting with OpenSSH 6.8" (`KeyPair$KeyFingerprint`).
//!
//! The STORE a generated key pair's public half is kept in is [`super::key_pairs`]; this
//! file is the material.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use ::aws_lc_rs::encoding::AsDer;
use ::hmac::Mac;
use ::pkcs1::der::pem::LineEnding;
use ::pkcs1::der::{Decode, EncodePem};
use ::sha1::Digest;
use ::ssh_encoding::Encode;

/// The modelled `KeyType` member an RSA key pair is created as.
pub const KEY_TYPE_RSA: &str = "rsa";

/// The modelled `KeyType` member an Ed25519 key pair is created as.
pub const KEY_TYPE_ED25519: &str = "ed25519";

/// The modelled `KeyFormat` member a PEM private key is asked for with: "Default: pem"
/// (`CreateKeyPairRequest$KeyFormat`), so this is also the format a request naming none
/// gets.
pub const KEY_FORMAT_PEM: &str = "pem";

/// The modelled `KeyFormat` member a PuTTY private key is asked for with: "an unencrypted
/// PPK formatted private key for use with PuTTY" (`CreateKeyPair`).
pub const KEY_FORMAT_PPK: &str = "ppk";

/// The modulus length of the RSA key pairs this service generates: "an ED25519 or 2048-bit
/// RSA key pair" (`CreateKeyPair`).
pub const RSA_KEY_PAIR_MODULUS_BITS: u32 = 2048;

/// The first line of a PuTTY private key file, which also states the file format's own
/// version: version 2, the format PuTTY documents for an unencrypted key and every PuTTY
/// since 0.52 loads.
const PUTTY_KEY_FILE_HEADER: &str = "PuTTY-User-Key-File-2";

/// The `Encryption` a PuTTY file written for an unencrypted private key names — the one
/// kind this service writes, the key material being "unencrypted" (`KeyPair$KeyMaterial`).
const PUTTY_ENCRYPTION_NONE: &str = "none";

/// The fixed string PuTTY derives a version-2 file's MAC key from, with the (here empty)
/// passphrase appended.
const PUTTY_MAC_KEY_SALT: &str = "putty-private-key-file-mac-key";

/// How many base64 characters a line of a PuTTY file's key blobs carries.
const PUTTY_BASE64_LINE_CHARS: usize = 64;

/// The material one `CreateKeyPair` generated: what the caller is handed, and what the
/// service keeps.
///
/// The private key is in this value and in the response, and NOWHERE else: "Amazon EC2
/// stores the public key and displays the private key for you to save to a file"
/// (`CreateKeyPair`).
#[derive(Clone)]
pub struct GeneratedKeyPair {
    /// "An unencrypted PEM encoded RSA or ED25519 private key" (`KeyPair$KeyMaterial`), or
    /// the PuTTY file when that is the format the request asked for.
    pub key_material: String,
    /// The public half, as the single-line OpenSSH public key its algorithm names it with —
    /// what the service stores and what `DescribeKeyPairs` reports as "the public key
    /// material".
    pub public_key: String,
    /// The fingerprint of this key, as its own type is fingerprinted
    /// (`KeyPair$KeyFingerprint`).
    pub key_fingerprint: String,
}

/// Redacts the private half: a derived `Debug` would carry the generated key into any
/// formatter this value reaches, including a log line.
impl std::fmt::Debug for GeneratedKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedKeyPair")
            .field("public_key", &self.public_key)
            .field("key_fingerprint", &self.key_fingerprint)
            .finish_non_exhaustive()
    }
}

/// The modelled `KeyType` member `requested` names, or `None` when it names none.
///
/// The catalog is the generated [`crate::ops::types::KEY_TYPE_VALUES`], so this crate holds
/// no second copy of it. Answering a `&'static str` out of that catalog is what keeps
/// request text out of the store.
pub fn key_pair_type(requested: &str) -> Option<&'static str> {
    crate::ops::types::KEY_TYPE_VALUES
        .iter()
        .copied()
        .find(|key_type| *key_type == requested)
}

/// The modelled `KeyFormat` member `requested` names, or `None` when it names none, read
/// off the generated [`crate::ops::types::KEY_FORMAT_VALUES`] for the reason
/// [`key_pair_type`] gives.
pub fn key_pair_format(requested: &str) -> Option<&'static str> {
    crate::ops::types::KEY_FORMAT_VALUES
        .iter()
        .copied()
        .find(|key_format| *key_format == requested)
}

/// A fresh key pair of type `key_type`, with its private half written in `key_format` and
/// named `comment` where the format carries a name for it.
///
/// `None` only when the key does not generate or encode, which a caller reports as a server
/// error rather than answering key material nothing produced. `key_type` and `key_format`
/// are members [`key_pair_type`] and [`key_pair_format`] resolved.
pub fn generate_key_pair(
    key_type: &str,
    key_format: &str,
    comment: &str,
) -> Option<GeneratedKeyPair> {
    match key_type {
        KEY_TYPE_ED25519 => generate_ed25519_key_pair(key_format, comment),
        // Every other member of the catalog is RSA: "Default: rsa"
        // (`CreateKeyPairRequest$KeyType`).
        _ => generate_rsa_key_pair(key_format, comment),
    }
}

/// A fresh [`RSA_KEY_PAIR_MODULUS_BITS`]-bit RSA key pair.
///
/// `aws-lc-rs` encodes a generated private key as PKCS#8, which is both the DER the
/// fingerprint is taken over and — its `privateKey` octet string being the PKCS#1
/// `RSAPrivateKey` itself (RFC 5958 section 2) — where the PKCS#1 structure the PEM private
/// key carries is read out of, rather than rebuilt from parts.
fn generate_rsa_key_pair(key_format: &str, comment: &str) -> Option<GeneratedKeyPair> {
    let generated =
        ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(::aws_lc_rs::rsa::KeySize::Rsa2048)
            .ok()?;
    let pkcs8: Vec<u8> = generated.as_der().ok()?.as_ref().to_vec();
    // "For RSA key pairs, the key fingerprint is the SHA-1 digest of the DER encoded
    // private key" (`KeyPair$KeyFingerprint`), shown as the colon-separated hex pairs EC2
    // publishes a key fingerprint in.
    let key_fingerprint = colon_separated_hex(&::sha1::Sha1::digest(&pkcs8));
    let private_key_info = ::pkcs8::PrivateKeyInfo::try_from(pkcs8.as_slice()).ok()?;
    let pkcs1_key = ::pkcs1::RsaPrivateKey::from_der(private_key_info.private_key).ok()?;
    let public_key = rsa_public_key(&pkcs1_key)?;
    let key_material = match key_format {
        KEY_FORMAT_PPK => {
            putty_private_key(&public_key, &rsa_putty_private_blob(&pkcs1_key)?, comment)?
        }
        // "An unencrypted PEM encoded PKCS#1 private key" (`CreateKeyPair`), which is the
        // `RSA PRIVATE KEY` PEM block `pkcs1` writes for this structure.
        _ => pkcs1_key.to_pem(LineEnding::LF).ok()?.to_string(),
    };
    Some(GeneratedKeyPair {
        key_material,
        public_key: openssh_public_key_line(&public_key)?,
        key_fingerprint,
    })
}

/// A fresh Ed25519 key pair, from the crate that implements the OpenSSH key formats.
///
/// An Ed25519 private key has no PKCS#1 form, so the PEM private key is the OpenSSH one —
/// the `OPENSSH PRIVATE KEY` block `ssh-keygen` itself writes for an Ed25519 key, which is
/// PEM encoded and unencrypted.
fn generate_ed25519_key_pair(key_format: &str, comment: &str) -> Option<GeneratedKeyPair> {
    let keypair =
        ::ssh_key::private::Ed25519Keypair::random(&mut ::ssh_key::rand_core::OsRng);
    let private_scalar = keypair.private.to_bytes();
    let private_key = ::ssh_key::PrivateKey::from(keypair);
    let public_key = private_key.public_key().clone();
    // "For ED25519 key pairs, the key fingerprint is the base64-encoded SHA-256 digest,
    // which is the default for OpenSSH" (`KeyPair$KeyFingerprint`) — the `SHA256:<base64>`
    // form `ssh-keygen -l` prints, which is what that default IS.
    let key_fingerprint = public_key.fingerprint(::ssh_key::HashAlg::Sha256).to_string();
    let key_material = match key_format {
        KEY_FORMAT_PPK => putty_private_key(
            &public_key,
            &ed25519_putty_private_blob(&private_scalar)?,
            comment,
        )?,
        _ => private_key
            .to_openssh(::ssh_key::LineEnding::LF)
            .ok()?
            .to_string(),
    };
    Some(GeneratedKeyPair {
        key_material,
        public_key: openssh_public_key_line(&public_key)?,
        key_fingerprint,
    })
}

/// The public half of the RSA key `key`, as an SSH public key.
///
/// Built from the modulus and public exponent of the key that was generated, so the public
/// key this service stores is the public half OF that private key rather than a second key.
fn rsa_public_key(key: &::pkcs1::RsaPrivateKey<'_>) -> Option<::ssh_key::PublicKey> {
    let rsa = ::ssh_key::public::RsaPublicKey {
        e: ::ssh_key::Mpint::from_positive_bytes(key.public_exponent.as_bytes()).ok()?,
        n: ::ssh_key::Mpint::from_positive_bytes(key.modulus.as_bytes()).ok()?,
    };
    Some(::ssh_key::PublicKey::new(
        ::ssh_key::public::KeyData::Rsa(rsa),
        // No comment: the public key material EC2 reports is the key, and a comment would
        // be a name nothing published.
        String::new(),
    ))
}

/// `public_key` as the one-line OpenSSH public key `<algorithm> <base64 blob>` a
/// `known_hosts` or `authorized_keys` entry carries — the form "the public key material"
/// (`KeyPairInfo$PublicKey`) takes.
fn openssh_public_key_line(public_key: &::ssh_key::PublicKey) -> Option<String> {
    Some(public_key.to_openssh().ok()?.trim_end().to_string())
}

/// The private half of the RSA key `key`, as the blob a PuTTY file carries: the four values
/// PuTTY's `ssh-rsa` private blob is made of — the private exponent, the two primes, and
/// the inverse of the second prime modulo the first — each as an SSH `mpint`.
fn rsa_putty_private_blob(key: &::pkcs1::RsaPrivateKey<'_>) -> Option<Vec<u8>> {
    let mut blob = Vec::new();
    for value in [
        key.private_exponent.as_bytes(),
        key.prime1.as_bytes(),
        key.prime2.as_bytes(),
        key.coefficient.as_bytes(),
    ] {
        ::ssh_key::Mpint::from_positive_bytes(value)
            .ok()?
            .encode(&mut blob)
            .ok()?;
    }
    Some(blob)
}

/// The private half of an Ed25519 key, as the blob a PuTTY file carries: the private scalar
/// as an SSH `mpint`.
fn ed25519_putty_private_blob(private_scalar: &[u8]) -> Option<Vec<u8>> {
    let mut blob = Vec::new();
    ::ssh_key::Mpint::from_positive_bytes(private_scalar)
        .ok()?
        .encode(&mut blob)
        .ok()?;
    Some(blob)
}

/// The PuTTY private key file holding `public_key` and `private_blob`, named `comment`.
///
/// PuTTY's own format, which no maintained crate writes: the key's algorithm, the (absent)
/// encryption, the comment, the public and private blobs as base64 lines, and an integrity
/// line over all five. The blobs themselves are the SSH wire encoding `ssh-encoding`
/// produces, so only the file's layout is written here.
fn putty_private_key(
    public_key: &::ssh_key::PublicKey,
    private_blob: &[u8],
    comment: &str,
) -> Option<String> {
    let algorithm = public_key.algorithm().as_str().to_string();
    let mut public_blob = Vec::new();
    public_key.key_data().encode(&mut public_blob).ok()?;
    let private_mac = putty_private_mac(&algorithm, comment, &public_blob, private_blob)?;

    let mut file = String::new();
    file.push_str(&format!("{PUTTY_KEY_FILE_HEADER}: {algorithm}\n"));
    file.push_str(&format!("Encryption: {PUTTY_ENCRYPTION_NONE}\n"));
    file.push_str(&format!("Comment: {comment}\n"));
    for (header, blob) in [
        ("Public-Lines", public_blob.as_slice()),
        ("Private-Lines", private_blob),
    ] {
        let lines = base64_lines(blob);
        file.push_str(&format!("{header}: {count}\n", count = lines.len()));
        for line in lines {
            file.push_str(&line);
            file.push('\n');
        }
    }
    file.push_str(&format!("Private-MAC: {private_mac}\n"));
    Some(file)
}

/// The integrity line of a PuTTY version-2 file: HMAC-SHA-1, over the algorithm, the
/// encryption, the comment and the two blobs — each as a length-prefixed SSH string — keyed
/// with the SHA-1 digest of [`PUTTY_MAC_KEY_SALT`] and the file's passphrase, which for an
/// unencrypted file is empty.
fn putty_private_mac(
    algorithm: &str,
    comment: &str,
    public_blob: &[u8],
    private_blob: &[u8],
) -> Option<String> {
    let mut data = Vec::new();
    algorithm.encode(&mut data).ok()?;
    PUTTY_ENCRYPTION_NONE.encode(&mut data).ok()?;
    comment.encode(&mut data).ok()?;
    public_blob.encode(&mut data).ok()?;
    private_blob.encode(&mut data).ok()?;

    let mac_key = ::sha1::Sha1::digest(PUTTY_MAC_KEY_SALT.as_bytes());
    let mut mac = ::hmac::Hmac::<::sha1::Sha1>::new_from_slice(&mac_key).ok()?;
    mac.update(&data);
    Some(::hex::encode(mac.finalize().into_bytes()))
}

/// `blob` as the base64 lines a PuTTY file holds it in, at most
/// [`PUTTY_BASE64_LINE_CHARS`] characters each.
fn base64_lines(blob: &[u8]) -> Vec<String> {
    use ::base64::Engine;
    let encoded = ::base64::engine::general_purpose::STANDARD.encode(blob);
    encoded
        .as_bytes()
        .chunks(PUTTY_BASE64_LINE_CHARS)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

/// `bytes` as the colon-separated lowercase hex pairs EC2 publishes an RSA key
/// fingerprint in.
fn colon_separated_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<String>>()
        .join(":")
}
