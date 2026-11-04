// SPDX-License-Identifier: CC0-1.0

//! Asymmetric encryption: a ciphertext produced OUTSIDE KMS, under the public half
//! this service publishes, and the plaintext its private half recovers.
//!
//! One concern of [`super`]'s store, in its own file: the helpers here are reached as
//! `crate::state::…` exactly as they were before, because the parent re-exports them.

use super::*;

/// Why a ciphertext presented for an asymmetric KMS key could not be opened with the
/// private half this store holds, or why a plaintext could not be sealed to its public
/// half.
#[derive(Debug)]
pub enum AsymmetricRefusal {
    /// No library reachable from here performs the encryption scheme this key pair and
    /// algorithm name, so there is no plaintext to answer with. The KMS key is still
    /// real — the caller is told the capability is not written yet rather than handed
    /// bytes that are not the plaintext of its ciphertext.
    Unsupported,
    /// The bytes are not a ciphertext this key pair's private half opens under those
    /// parameters: they were produced for another key, under another algorithm, or
    /// they have been altered.
    NotThisCiphertext,
    /// The library implements the scheme and refused this particular operation, with the
    /// request already judged against every rule the scheme states. Nothing about the
    /// request explains it, so it is a server fault rather than a refusal.
    Failed,
}

/// The plaintext of `ciphertext`, which was encrypted under the PUBLIC half of `key`'s
/// key pair — the bytes `GetPublicKey` publishes ([`public_key_spki`]) — with
/// `encryption_algorithm`.
///
/// This is the one-way path an asymmetric encryption KMS key exists for: a party
/// holding only the published public key encrypts without calling KMS at all, and only
/// the private half held here opens the result. Nothing about the ciphertext says which
/// key or algorithm produced it, so both come from the request; the ciphertext carries
/// no additional authenticated data either, since "the standard asymmetric encryption
/// algorithms ... that KMS uses do not support an encryption context"
/// (`smithy_query(query="doc",shape="DecryptRequest",member="EncryptionContext")`).
///
/// The RSA-OAEP decryption comes from `aws-lc-rs` (`aws_lc_rs::rsa`), under the private
/// key this store holds, with no label — the parameter set
/// [`rsa_oaep_algorithm`] names for the algorithm. Nothing cryptographic is composed
/// here.
pub fn decrypt_asymmetric(
    key: &Cmk,
    encryption_algorithm: &str,
    ciphertext: &[u8],
) -> Result<Vec<u8>, AsymmetricRefusal> {
    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(AsymmetricRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let oaep_algorithm =
                rsa_oaep_algorithm(encryption_algorithm).ok_or(AsymmetricRefusal::Unsupported)?;
            let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            let key_size_bytes = private_key.key_size_bytes();
            let private_key = ::aws_lc_rs::rsa::OaepPrivateDecryptingKey::new(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            // An OAEP plaintext is shorter than the modulus the ciphertext occupies,
            // so one modulus of room always holds it.
            let mut plaintext = vec![0u8; key_size_bytes];
            let len = private_key
                .decrypt(oaep_algorithm, ciphertext, &mut plaintext, None)
                .map_err(|_| AsymmetricRefusal::NotThisCiphertext)?
                .len();
            plaintext.truncate(len);
            Ok(plaintext)
        }
        // SM2PKE, the public-key encryption of GM/T 0003-2012, is the one encryption
        // scheme no library reachable from here performs; the remaining key pairs are
        // signing and key-agreement pairs, whose specs advertise no encryption
        // algorithm at all, so a request naming one is refused before it reaches here.
        KeyPairKind::Sm2 | KeyPairKind::Ecdsa(_) | KeyPairKind::Ed25519 => {
            Err(AsymmetricRefusal::Unsupported)
        }
    }
}

/// The RSA-OAEP parameters an `EncryptionAlgorithmSpec` names: the OAEP scheme of RFC
/// 8017 with the digest the algorithm's own name carries, used both as the message
/// digest and as the hash of MGF1.
///
/// `None` for an algorithm that is not one an RSA key pair encrypts under
/// ([`RSA_ENCRYPTION_ALGORITHMS`] is that set).
fn rsa_oaep_algorithm(
    encryption_algorithm: &str,
) -> Option<&'static ::aws_lc_rs::rsa::OaepAlgorithm> {
    match encryption_algorithm {
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1 => Some(&::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1),
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256 => Some(&::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256),
        _ => None,
    }
}

/// The ciphertext of `plaintext` under the PUBLIC half of `key`'s key pair, with
/// `encryption_algorithm` — the other direction of [`decrypt_asymmetric`], for the caller
/// that asks KMS to encrypt rather than holding the published public key itself.
///
/// The public half is COMPUTED from the private half this store holds, by the same
/// library that implements the scheme, so what seals a plaintext here is the public key
/// `GetPublicKey` publishes ([`public_key_spki`]) and nothing else can open the result.
/// The ciphertext carries no additional authenticated data: "the standard asymmetric
/// encryption algorithms ... that KMS uses do not support an encryption context"
/// (`smithy_query(query="doc",shape="DecryptRequest",member="EncryptionContext")`).
///
/// The plaintext must already be within the scheme's ceiling
/// ([`max_asymmetric_plaintext_len`]), which is a documented property of the key spec and
/// the algorithm and so is a REFUSAL the handler answers, not a failure here.
pub fn encrypt_asymmetric(
    key: &Cmk,
    encryption_algorithm: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, AsymmetricRefusal> {
    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(AsymmetricRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let oaep_algorithm =
                rsa_oaep_algorithm(encryption_algorithm).ok_or(AsymmetricRefusal::Unsupported)?;
            let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            // An RSA ciphertext is exactly one modulus wide.
            let mut ciphertext = vec![0u8; private_key.key_size_bytes()];
            let public_key = ::aws_lc_rs::rsa::OaepPublicEncryptingKey::new(private_key.public_key())
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            let len = public_key
                .encrypt(oaep_algorithm, plaintext, &mut ciphertext, None)
                .map_err(|_| AsymmetricRefusal::Failed)?
                .len();
            ciphertext.truncate(len);
            Ok(ciphertext)
        }
        // The same scheme coverage [`decrypt_asymmetric`] has: SM2PKE is the one
        // encryption scheme no library reachable from here performs, and the remaining
        // key pairs advertise no encryption algorithm at all.
        KeyPairKind::Sm2 | KeyPairKind::Ecdsa(_) | KeyPairKind::Ed25519 => {
            Err(AsymmetricRefusal::Unsupported)
        }
    }
}

/// The largest plaintext an asymmetric KMS key of `key_spec` encrypts under
/// `encryption_algorithm`, in bytes, or `None` when the pair names no scheme with a
/// ceiling this computes.
///
/// For RSAES-OAEP the ceiling is the scheme's own: a message of at most `k - 2*hLen - 2`
/// bytes, where `k` is the modulus in bytes and `hLen` the digest's output length (RFC
/// 8017 §7.1.1). It is therefore derived from the KEY SPEC and the ALGORITHM rather than
/// listed, and it reproduces every ceiling the operation documents — "RSA_2048
/// RSAES_OAEP_SHA_1: 214 bytes RSAES_OAEP_SHA_256: 190 bytes RSA_3072 RSAES_OAEP_SHA_1:
/// 342 bytes RSAES_OAEP_SHA_256: 318 bytes RSA_4096 RSAES_OAEP_SHA_1: 470 bytes
/// RSAES_OAEP_SHA_256: 446 bytes" (`smithy_query(query="doc",shape="Encrypt")`).
pub fn max_asymmetric_plaintext_len(key_spec: &str, encryption_algorithm: &str) -> Option<usize> {
    let KeyPairKind::Rsa(modulus_bits) = key_pair_kind(key_spec)? else {
        return None;
    };
    let digest_len = match encryption_algorithm {
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1 => ::aws_lc_rs::digest::SHA1_OUTPUT_LEN,
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256 => ::aws_lc_rs::digest::SHA256_OUTPUT_LEN,
        _ => return None,
    };
    (modulus_bits / 8).checked_sub(2 * digest_len + 2)
}
