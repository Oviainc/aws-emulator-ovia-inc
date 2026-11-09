// SPDX-License-Identifier: CC0-1.0

//! What the service accepts as an `SshPublicKeyBody`, and the refusal for what it does
//! not.
//!
//! The member's MODELLED pattern
//! (`^\s*(ssh|ecdsa)-[a-z0-9-]+[ \t]+(([A-Za-z0-9+/]{4})*([A-Za-z0-9+/]{1,3})?(={0,3})?)(\s*|[ \t]+[\S \t]*\s*)$`,
//! from `smithy_query(query="operation",operation="ImportSshPublicKey")`) admits far
//! more than the service stores: every `ssh-<name>` and `ecdsa-<name>` label, and a
//! payload group whose every part is optional — so an absent payload and a single
//! base64 character both reach the handler. Two things are therefore the SERVICE's to
//! check, and both are checked here, in the one place every write that takes a key body
//! consults:
//!
//! 1. the ALGORITHM is one of the key types the member documents the service accepting
//!    ("Transfer Family accepts RSA, ECDSA, and ED25519 keys", `smithy_query(query="doc",
//!    shape="ImportSshPublicKeyRequest",member="SshPublicKeyBody")`), under the OpenSSH
//!    names those types carry — see [`SUPPORTED_ALGORITHMS`] and [`ECDSA_FAMILY`];
//! 2. the PAYLOAD after it is present and is base64 that decodes to whole bytes.
//!
//! Nothing beyond that is checked: the key MATERIAL is stored verbatim without being
//! parsed as a key, so a payload truncated mid-modulus, or one whose bytes belong to
//! another algorithm than its label names, is accepted exactly as the service accepts
//! it. The decoder is therefore the most permissive reading of base64 there is — padding
//! optional and the trailing bits of a partial group unconstrained — so only a payload
//! that cannot be bytes AT ALL is refused.

use ::base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use ::base64::Engine as _;

/// The OpenSSH algorithm names of the key types the service documents accepting that
/// are not the ECDSA family: RSA and ED25519. (RSA's SHA-2 signature names,
/// `rsa-sha2-256`/`rsa-sha2-512`, are not key types and the member's modelled pattern
/// admits neither, since it requires an `ssh-` or `ecdsa-` prefix.)
const SUPPORTED_ALGORITHMS: [&str; 2] = ["ssh-rsa", "ssh-ed25519"];

/// The prefix every OpenSSH name of an ECDSA key type carries (`ecdsa-sha2-nistp256`,
/// `ecdsa-sha2-nistp384`, `ecdsa-sha2-nistp521`, …): the documented acceptance is of
/// ECDSA keys as a family, not of one curve, so the family is what is accepted.
const ECDSA_FAMILY: &str = "ecdsa-";

/// The refusal text for a key body the service does not accept, covering both
/// conditions: the algorithm it names and the payload it carries are the two things
/// that make a body an SSH public key the service can store, and neither is quoted back
/// to the caller.
pub const UNSUPPORTED_SSH_PUBLIC_KEY: &str = "Unsupported or invalid SSH public key format";

/// The one base64 reading of a key payload: the standard alphabet, with padding
/// OPTIONAL and the trailing bits of a final partial group unconstrained.
///
/// `ssh-keygen` writes a padded payload, but a body a caller assembled, or truncated,
/// need not carry the padding, and a payload cut mid-key ends in a partial group whose
/// spare bits are whatever the key's bytes were. Both are key material the service
/// stores, so neither is a decoding failure here; what IS one is a group of a single
/// character, which carries six bits and so cannot be any whole byte.
const PAYLOAD_BASE64: GeneralPurpose = GeneralPurpose::new(
    &::base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// `Some(message)` when `body` is not an SSH public key this service accepts — the
/// `Message` the modelled `InvalidRequestException` (HTTP 400) carries — and `None`
/// when it is.
///
/// Called by every write that takes an `SshPublicKeyBody` (`CreateUser`,
/// `ImportSshPublicKey`), so one rule decides what a user's key set may hold however
/// the key arrives.
pub fn unsupported_key_body(body: &str) -> Option<&'static str> {
    // The modelled pattern admits leading whitespace, so the body's first FIELD is the
    // algorithm and its second is the payload; everything after them is the comment the
    // member admits, which says nothing about the key.
    let mut fields = body.split_whitespace();
    let Some(algorithm) = fields.next() else {
        // No field at all names no algorithm and no material; the member's modelled
        // pattern already refuses such a body at the boundary, so this is only the
        // reading staying total.
        return Some(UNSUPPORTED_SSH_PUBLIC_KEY);
    };
    if !is_supported_algorithm(algorithm) {
        return Some(UNSUPPORTED_SSH_PUBLIC_KEY);
    }
    // An absent payload names no key material at all, and one that is not base64 names
    // no bytes: neither can ever authenticate a session.
    let payload = fields.next().filter(|payload| !payload.is_empty());
    let decodes = payload.is_some_and(|payload| PAYLOAD_BASE64.decode(payload).is_ok());
    (!decodes).then_some(UNSUPPORTED_SSH_PUBLIC_KEY)
}

/// Whether `algorithm` names one of the key types the service accepts, compared
/// case-insensitively as the OpenSSH names are written in lower case.
fn is_supported_algorithm(algorithm: &str) -> bool {
    let algorithm = algorithm.to_ascii_lowercase();
    SUPPORTED_ALGORITHMS.contains(&algorithm.as_str()) || algorithm.starts_with(ECDSA_FAMILY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two conditions the service checks, over the class each one generalises over:
    /// the algorithm name (each accepted type, an unaccepted one the modelled pattern
    /// admits) and the payload (absent, undecodable, decodable with and without
    /// padding, truncated, and one belonging to another algorithm than the label).
    #[test]
    fn a_key_body_is_accepted_for_a_supported_algorithm_with_decodable_material() {
        // (case, body, accepted)
        let rows: &[(&str, &str, bool)] = &[
            ("an RSA key", "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==", true),
            (
                "an ED25519 key, whose payload carries no padding",
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab",
                true,
            ),
            (
                "an ECDSA key on the P-256 curve",
                "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=",
                true,
            ),
            (
                "an ECDSA key on the P-521 curve",
                "ecdsa-sha2-nistp521 AAAAE2VjZHNhLXNoYTItbmlzdHA1MjE=",
                true,
            ),
            (
                "a key whose material belongs to another algorithm than its label names",
                "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                true,
            ),
            (
                "a payload truncated mid-key, which is still bytes",
                "ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB",
                true,
            ),
            (
                "a key with a comment after its material",
                "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ== user@host",
                true,
            ),
            (
                "a key body with the leading whitespace the modelled pattern admits",
                "\n  ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==\n",
                true,
            ),
            ("a DSA key, which the service does not accept", "ssh-dss AAAAB3NzaC1kc3M=", false),
            (
                "an ssh- label naming no key type at all",
                "ssh-foo AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
                false,
            ),
            ("an accepted algorithm with no payload", "ssh-rsa ", false),
            (
                "an accepted algorithm with a payload of one base64 character",
                "ssh-rsa A",
                false,
            ),
            (
                "a payload that is not base64 at all",
                "ssh-rsa not-base64!!",
                false,
            ),
            (
                "a body that is nothing but whitespace, which the modelled pattern refuses first",
                "   ",
                false,
            ),
        ];
        for (case, body, accepted) in rows {
            let refusal = unsupported_key_body(body);
            assert_eq!(
                refusal.is_none(),
                *accepted,
                "case {case:?}: expected accepted={accepted}, got {refusal:?}"
            );
        }
    }
}
