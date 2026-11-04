// SPDX-License-Identifier: CC0-1.0

//! Message authentication codes: the HMAC an HMAC KMS key's own material produces over
//! a message, and the comparison that verifies one.
//!
//! One concern of [`super`]'s store, in its own file: the helpers here are reached as
//! `crate::state::…` exactly as they were before, because the parent re-exports them.

use super::*;

/// The HMAC algorithm a `MacAlgorithmSpec` names.
///
/// "The HMAC KMS keys and the HMAC algorithms that KMS uses conform to industry
/// standards defined in RFC 2104" (`smithy_query(query="doc",shape="GenerateMac")`),
/// and the spec's own name gives the hash that HMAC is built on, so `HMAC_SHA_384` is
/// HMAC-SHA-384. `None` for a value that is not one of the four the model declares
/// (`smithy_query(query="shape",shape="MacAlgorithmSpec")`), which the modelled enum
/// constraint already refuses before a handler runs.
pub fn mac_hash(mac_algorithm: &str) -> Option<::aws_lc_rs::hmac::Algorithm> {
    use ::aws_lc_rs::hmac::{HMAC_SHA224, HMAC_SHA256, HMAC_SHA384, HMAC_SHA512};

    match mac_algorithm {
        "HMAC_SHA_224" => Some(HMAC_SHA224),
        "HMAC_SHA_256" => Some(HMAC_SHA256),
        "HMAC_SHA_384" => Some(HMAC_SHA384),
        "HMAC_SHA_512" => Some(HMAC_SHA512),
        _ => None,
    }
}

/// The HMAC of `message` under `key`'s own key material with `mac_algorithm`: "the
/// standard, raw HMAC defined in RFC 2104"
/// (`smithy_query(query="doc",shape="GenerateMacResponse",member="Mac")`).
///
/// The tag is produced by the library that implements the algorithm, from the bytes
/// this key holds — which never leave this process — so it is a function of the key,
/// the algorithm and the message and of nothing else: the same three inputs give the
/// same tag, and any change to one of them gives another.
///
/// `None` when the key holds no HMAC key material, or `mac_algorithm` names no HMAC
/// algorithm: there is then no HMAC of the caller's message to answer with.
pub fn generate_mac(key: &Cmk, mac_algorithm: &str, message: &[u8]) -> Option<Vec<u8>> {
    let hmac_key = mac_key(key, mac_algorithm)?;
    Some(::aws_lc_rs::hmac::sign(&hmac_key, message).as_ref().to_vec())
}

/// Whether `mac` is the HMAC of `message` under `key`'s material with `mac_algorithm`.
///
/// "VerifyMac computes an HMAC using the message, HMAC KMS key, and MAC algorithm that
/// you specify, and compares the computed HMAC to the HMAC that you specify"
/// (`smithy_query(query="doc",shape="VerifyMac")`), which is the comparison the library
/// itself performs — in constant time, and false for a tag of the wrong length rather
/// than a panic. A tag from another key, another algorithm or another message therefore
/// does not verify.
pub fn mac_matches(key: &Cmk, mac_algorithm: &str, message: &[u8], mac: &[u8]) -> bool {
    mac_key(key, mac_algorithm)
        .is_some_and(|hmac_key| ::aws_lc_rs::hmac::verify(&hmac_key, message, mac).is_ok())
}

/// `key`'s HMAC key material as a key of the HMAC algorithm `mac_algorithm` names, so
/// generating a MAC and verifying one are the same key over the same bytes.
///
/// The material is the key's CURRENT material ([`Cmk::effective_material`]), which is
/// what a cryptographic operation on the key uses.
fn mac_key(key: &Cmk, mac_algorithm: &str) -> Option<::aws_lc_rs::hmac::Key> {
    let KeyMaterial::Hmac(material) = key.effective_material() else {
        return None;
    };
    Some(::aws_lc_rs::hmac::Key::new(
        mac_hash(mac_algorithm)?,
        material,
    ))
}
