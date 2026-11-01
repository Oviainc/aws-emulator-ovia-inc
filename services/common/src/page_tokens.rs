// SPDX-License-Identifier: CC0-1.0

//! The ONE way a service tells a continuation token it ISSUED from one a caller built.
//!
//! A paginated read hands back an opaque `NextToken` and is handed one back on the next
//! call. The position the token names travels INSIDE it — there is no page kept on the
//! side to look up — so a read that only checks the token's SHAPE honours any value of
//! that shape, and a caller can name a position the service never offered: the first
//! page of a report it was paging past, a position far in the future, a position under
//! another filter. What the services' APIs document for such a value is a refusal (an
//! invalid-token error), not a page.
//!
//! So a token carries an AUTHENTICATOR over its own cursor, computed under a secret the
//! issuing store draws from the system's randomness and never reveals ([`PageTokenKey`]):
//! the cursor stays readable by the service that minted it, and no caller can produce a
//! cursor the key authenticates. A token minted by another store — another emulator
//! process, a restarted one — is refused for the same reason.
//!
//! The authenticator is HMAC-SHA256 from the cryptography library this workspace already
//! uses, not a keyed digest written here; the module computes over data in process and
//! reads nothing off the host. How a service RENDERS the authenticated bytes as the
//! string on the wire is the service's own (each one already has an opaque spelling for
//! its tokens), so this module hands back bytes and takes bytes.

use ring::hmac;
use ring::rand::SecureRandom;

/// How many bytes of authenticator [`PageTokenKey::mint`] puts ahead of the cursor: the
/// width of an HMAC-SHA256 tag, carried whole rather than truncated.
///
/// Public so a test can build a value with the SHAPE of a minted token — the case that
/// matters is the one only the authenticator can refuse — without restating the width.
pub const AUTHENTICATOR_BYTES: usize = 32;

/// The width of the secret: SHA-256's block-aligned key length, so the key is used as
/// HMAC's key without being hashed down or padded out.
const KEY_BYTES: usize = 32;

/// The secret one store's page tokens are authenticated under.
///
/// Drawn from the system's randomness when the store is created, held only in memory,
/// never derived from a request, never reported and never logged (its [`std::fmt::Debug`]
/// renders nothing of it). `Copy`, so a read can take the key off its store and go on to
/// borrow the records it is about to report in the same call.
#[derive(Clone, Copy)]
pub struct PageTokenKey([u8; KEY_BYTES]);

impl PageTokenKey {
    /// A fresh key from the system's random generator.
    ///
    /// Explicit at the call site rather than hidden behind a `Default`, because what this
    /// answers is new randomness and not a zero value.
    pub fn new() -> Self {
        let mut key = [0_u8; KEY_BYTES];
        ::ring::rand::SystemRandom::new()
            .fill(&mut key)
            .expect("the system random generator fills a page-token key");
        Self(key)
    }

    /// The bytes a `NextToken` carries for `cursor`: the authenticator over the cursor,
    /// then the cursor itself.
    ///
    /// The cursor is not hidden — a continuation token is opaque, not confidential — and
    /// nothing about it has to be: what a caller cannot do is produce the authenticator
    /// for a cursor of its own.
    pub fn mint(&self, cursor: &str) -> Vec<u8> {
        let authenticator = hmac::sign(&self.key(), cursor.as_bytes());
        let mut token = Vec::with_capacity(AUTHENTICATOR_BYTES + cursor.len());
        token.extend_from_slice(authenticator.as_ref());
        token.extend_from_slice(cursor.as_bytes());
        token
    }

    /// The cursor `token` carries, or `None` for any value THIS key did not mint: one too
    /// short to carry an authenticator, one whose authenticator does not match the cursor
    /// it travels with, and one whose cursor is not UTF-8.
    ///
    /// `hmac::verify` is the constant-time comparison, so a refusal tells the caller
    /// nothing about how close its value was.
    pub fn cursor_of(&self, token: &[u8]) -> Option<String> {
        if token.len() < AUTHENTICATOR_BYTES {
            return None;
        }
        let (authenticator, cursor) = token.split_at(AUTHENTICATOR_BYTES);
        hmac::verify(&self.key(), cursor, authenticator).ok()?;
        String::from_utf8(cursor.to_vec()).ok()
    }

    /// This key as the HMAC algorithm's own key type. Built per call, which costs one
    /// key schedule and keeps [`PageTokenKey`] a plain `Copy` value a store can hold.
    fn key(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &self.0)
    }
}

impl Default for PageTokenKey {
    fn default() -> Self {
        Self::new()
    }
}

/// Redacted: the key authenticates a store's page tokens, so it belongs in no log line
/// and in no debug dump of the state that holds it.
impl std::fmt::Debug for PageTokenKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PageTokenKey(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cursor round-trips under the key that minted it; a value that key did not mint
    /// names no cursor at all, however exactly it has a minted token's shape.
    #[test]
    fn a_cursor_round_trips_and_only_under_the_key_that_minted_it() {
        let key = PageTokenKey::new();
        let other = PageTokenKey::new();
        // Cursors spanning the forms a paginated read carries: empty, one character, a
        // name, a composite several-field position, and one with a separator byte.
        for cursor in [
            "",
            "A",
            "MyLexicon",
            "1761000000000\u{1f}task-1\u{1f}completed",
            "\u{1f}",
        ] {
            let token = key.mint(cursor);
            assert_eq!(
                key.cursor_of(&token).as_deref(),
                Some(cursor),
                "cursor {cursor:?}: did not round-trip"
            );
            assert_eq!(
                other.cursor_of(&token),
                None,
                "cursor {cursor:?}: must not decode under another key"
            );

            // The same cursor behind an authenticator the caller made up.
            for (case, authenticator) in [
                ("all-zero", vec![0_u8; AUTHENTICATOR_BYTES]),
                ("all-ones", vec![0xff_u8; AUTHENTICATOR_BYTES]),
            ] {
                let forged = [authenticator.as_slice(), cursor.as_bytes()].concat();
                assert_eq!(
                    key.cursor_of(&forged),
                    None,
                    "cursor {cursor:?}: a {case} authenticator must not decode"
                );
            }

            // Every single-byte change to a minted token, in the authenticator and in
            // the cursor alike, plus one truncated and one extended.
            for index in 0..token.len() {
                let mut tampered = token.clone();
                tampered[index] ^= 0x01;
                assert_eq!(
                    key.cursor_of(&tampered),
                    None,
                    "cursor {cursor:?}: byte {index} changed must not decode"
                );
            }
            assert_eq!(
                key.cursor_of(&token[..token.len() - 1]),
                None,
                "cursor {cursor:?}: a token with its last byte dropped must not decode"
            );
            assert_eq!(
                key.cursor_of(&[token.as_slice(), b"x"].concat()),
                None,
                "cursor {cursor:?}: a token with a byte appended must not decode"
            );
        }

        // Too short to carry an authenticator, and bytes that are not UTF-8 behind one.
        for (case, token) in [
            ("empty", Vec::new()),
            ("one byte", vec![0_u8]),
            ("one byte short", vec![0_u8; AUTHENTICATOR_BYTES - 1]),
            ("not UTF-8", vec![0xff_u8; AUTHENTICATOR_BYTES + 2]),
        ] {
            assert_eq!(
                key.cursor_of(&token),
                None,
                "case {case:?}: expected no cursor"
            );
        }
    }

    /// Two stores draw two different secrets, and the secret is in no rendering of the
    /// key.
    #[test]
    fn keys_differ_and_are_not_rendered() {
        let key = PageTokenKey::new();
        let other = PageTokenKey::new();
        assert_ne!(
            key.0, other.0,
            "two keys drawn from the system's randomness must differ"
        );
        assert_eq!(
            format!("{key:?}"),
            "PageTokenKey(..)",
            "the key must render redacted"
        );
        assert!(
            !format!("{key:?}").contains(&format!("{:?}", key.0)),
            "the key material must not be in its rendering"
        );
    }
}
