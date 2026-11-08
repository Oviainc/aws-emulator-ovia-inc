// SPDX-License-Identifier: CC0-1.0

//! The access control an Amplify app configures on its branches: what this service KEEPS
//! for a basic-authorization credential, what it REPORTS for it, and how the hosting
//! edge verifies the credentials a visitor presents.
//!
//! `App$basicAuthCredentials` ("the basic authorization credentials for branches for the
//! Amplify app … base64-encode the authorization credentials and provide them in the
//! format `user:password`") and `Branch$basicAuthCredentials` (the same sentence for one
//! branch) are both `@sensitive`
//! (`smithy_query(query="shape",shape="BasicAuthCredentials")`), and that is the whole
//! shape of this module:
//!
//! * A credential the caller configures is kept only as a VERIFIER — the digest of the
//!   `user:password` bytes it carries — so nothing in this process holds the password
//!   and no reader of the API can be answered with it.
//! * What a reader IS answered with is a stable, non-reversible REFERENCE to that
//!   credential ([`BasicAuthCredentials::reported`]): it says a credential is configured,
//!   it changes when the credential changes, and it discloses nothing about it.
//! * The hosting edge verifies the `Authorization: Basic` credentials a visitor presents
//!   against the verifier ([`BasicAuthCredentials::accepts`]), which is the only thing
//!   the kept digest can be used for.

use ::sha2::{Digest, Sha256};

use crate::ops::types::AutoBranchCreationConfig;

/// One configured basic-authorization credential, as this service keeps it: enough to
/// verify a credential a visitor presents, and never enough to reproduce one.
///
/// `Clone`/`PartialEq` because the record carrying it is cloned by every reader; the
/// [`Debug`] impl is written by hand so no formatter — a log line, a panic message, a
/// derived `Debug` further up — can print the digest of a caller's password.
#[derive(Clone, PartialEq, Eq)]
pub struct BasicAuthCredentials {
    /// SHA-256 of the credential's `user:password` bytes.
    digest: [u8; 32],
}

impl ::std::fmt::Debug for BasicAuthCredentials {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.write_str("BasicAuthCredentials(<configured>)")
    }
}

impl BasicAuthCredentials {
    /// The credential a request configured, read as the member documents it: base64 of
    /// `user:password`.
    ///
    /// The DECODED bytes are what is digested, so the same credential configured with a
    /// differently written base64 token (padding, surrounding whitespace) verifies the
    /// same visitor. A value that is not base64 at all encodes no `user:password` pair,
    /// so its own bytes are digested instead: it is kept — a credential was configured —
    /// and no presented pair can ever match it.
    pub fn configured(submitted: &str) -> Self {
        let pair = ::aws_smithy_types::base64::decode(submitted.trim())
            .unwrap_or_else(|_| submitted.as_bytes().to_vec());
        Self::of_pair(&pair)
    }

    /// Whether `presented` — the decoded `user:password` bytes an `Authorization: Basic`
    /// header carried — is this credential.
    pub fn accepts(&self, presented: &[u8]) -> bool {
        Self::of_pair(presented) == *self
    }

    /// What every reader of the modelled member is answered with: a reference to this
    /// credential that cannot be turned back into it.
    ///
    /// It is the digest, base64-encoded — the same encoding the member is documented to
    /// carry, so a client that decodes it gets bytes rather than a parse failure, and
    /// those bytes are a digest and not a `user:password` pair. The value is derived from
    /// the credential alone, so it is stable across reads and changes exactly when the
    /// configured credential changes.
    pub fn reported(&self) -> String {
        ::aws_smithy_types::base64::encode(self.digest)
    }

    /// The credential whose `user:password` bytes are `pair`.
    fn of_pair(pair: &[u8]) -> Self {
        Self {
            digest: Sha256::digest(pair).into(),
        }
    }
}

/// Ingest the `basicAuthCredentials` a request supplied: keep the verifier, and answer
/// what the record reports for the member.
///
/// `None` is answered for a request that supplied no credential — the member is then left
/// exactly as it was, which is what makes an update partial
/// ([`crate::state::apply_optional`]). The EMPTY value is a credential removed: nothing is
/// kept, so nothing can authenticate, and the member reports the empty value the request
/// set.
pub fn ingest_basic_auth(
    kept: &mut Option<BasicAuthCredentials>,
    submitted: Option<String>,
) -> Option<String> {
    let submitted = submitted?;
    if submitted.is_empty() {
        *kept = None;
        return Some(submitted);
    }
    let credentials = BasicAuthCredentials::configured(&submitted);
    let reported = credentials.reported();
    *kept = Some(credentials);
    Some(reported)
}

/// Ingest the credentials inside an auto-branch-creation configuration, by the same rule.
///
/// `AutoBranchCreationConfig$basicAuthCredentials` is the `@sensitive` credential a branch
/// Amplify creates automatically is given, so a reader must not be answered with it
/// either: the member is replaced by its reference before the configuration is stored. No
/// verifier is kept for it, because it protects no site of this emulator — nothing here
/// creates a branch automatically, so there is nothing to verify against; the credential a
/// branch IS served under is the one configured on the branch or on its app.
pub fn ingest_auto_branch_basic_auth(config: &mut Option<AutoBranchCreationConfig>) {
    let Some(config) = config.as_mut() else {
        return;
    };
    let mut unkept = None;
    if let Some(reported) = ingest_basic_auth(&mut unkept, config.basic_auth_credentials.take()) {
        config.basic_auth_credentials = Some(reported);
    }
}

/// The credentials an `Authorization` header presents to the hosting edge: the decoded
/// bytes of its base64 `user:password` token, or `None` when the request presents no HTTP
/// Basic credentials at all.
///
/// The scheme name is compared case-insensitively and separated from the token on any run
/// of whitespace, as RFC 9110's `credentials = scheme 1*SP token68` admits, and the token
/// is base64 with the standard alphabet — the same codec the wire serializer uses for
/// every Smithy blob.
pub fn presented_credentials(authorization: Option<&str>) -> Option<Vec<u8>> {
    let value = authorization?.trim();
    let (scheme, token) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case(BASIC_SCHEME) {
        return None;
    }
    ::aws_smithy_types::base64::decode(token.trim()).ok()
}

/// The authentication scheme a protected branch's site is read with — the one the
/// credentials member is documented in terms of (`user:password`, base64-encoded).
pub const BASIC_SCHEME: &str = "Basic";

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a configured credential is kept as a verifier and reported as a reference —
    /// the reported value is never the submitted one, never decodes to the password, is
    /// stable across reads, and differs for a different credential.
    #[test]
    fn a_configured_credential_is_reported_as_a_reference_and_never_echoed() {
        // (case, the submitted base64 credential, the password inside it)
        let rows: [(&str, &str, &str); 4] = [
            ("the documented user:password form", "dXNlcjpwYXNzd29yZA==", "password"),
            ("a long password", "cHJvYmU6czNjcmV0LXBhc3N3b3JkLTAxMjM0", "s3cret-password-01234"),
            ("a password carrying a colon", "dXNlcjphOmI6Yw==", "a:b:c"),
            ("a value that is not base64 at all", "not+base64+%%%", "%%%"),
        ];
        for (case, submitted, password) in rows {
            let credentials = BasicAuthCredentials::configured(submitted);
            let reported = credentials.reported();
            assert!(
                !reported.is_empty() && reported != submitted,
                "case {case:?}: the reported value must be non-empty and not the \
                 submitted credential, got {reported:?}"
            );
            let decoded = ::aws_smithy_types::base64::decode(&reported)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                .unwrap_or_default();
            assert!(
                !decoded.contains(password),
                "case {case:?}: the reported value must not decode to the configured \
                 password, got {decoded:?}"
            );
            assert_eq!(
                reported,
                BasicAuthCredentials::configured(submitted).reported(),
                "case {case:?}: the reported value must be stable across reads"
            );
            assert_ne!(
                reported,
                BasicAuthCredentials::configured("ZGlmZmVyZW50OmNyZWRz").reported(),
                "case {case:?}: a different credential must report a different reference"
            );
        }
    }

    /// RULE: the verifier accepts exactly the configured `user:password` pair, however
    /// the base64 token presenting it was written, and nothing else.
    #[test]
    fn the_verifier_accepts_only_the_configured_pair() {
        let credentials = BasicAuthCredentials::configured("cHJvYmU6c2VjcmV0");
        // (case, the Authorization header value, accepted?)
        let rows: [(&str, &str, bool); 7] = [
            ("the configured pair", "Basic cHJvYmU6c2VjcmV0", true),
            ("the scheme in lower case", "basic cHJvYmU6c2VjcmV0", true),
            ("extra separating space", "Basic   cHJvYmU6c2VjcmV0", true),
            ("the wrong password", "Basic cHJvYmU6d3Jvbmc=", false),
            ("the wrong user", "Basic b3RoZXI6c2VjcmV0", false),
            ("a bearer token", "Bearer cHJvYmU6c2VjcmV0", false),
            ("no credentials at all", "", false),
        ];
        for (case, header, expected) in rows {
            let accepted = presented_credentials(Some(header))
                .is_some_and(|presented| credentials.accepts(&presented));
            assert_eq!(
                accepted, expected,
                "case {case:?}: expected accepted {expected}, got {accepted}"
            );
        }
        assert!(
            presented_credentials(None).is_none(),
            "a request with no Authorization header presents no credentials"
        );
    }

    /// RULE: an ingested credential fills the member with the reference; an omitted
    /// member keeps what was configured; the empty value removes it.
    #[test]
    fn ingestion_keeps_a_verifier_only_while_a_credential_is_configured() {
        let mut kept = None;
        let reported = ingest_basic_auth(&mut kept, Some("dXNlcjpwYXNz".to_string()));
        assert!(
            kept.is_some() && reported.is_some_and(|value| value != "dXNlcjpwYXNz"),
            "case \"configured\": a credential is kept and reported as a reference, got \
             {kept:?}"
        );

        assert!(
            ingest_basic_auth(&mut kept, None).is_none() && kept.is_some(),
            "case \"omitted\": an omitted member leaves the configured credential alone, \
             got {kept:?}"
        );

        assert_eq!(
            (ingest_basic_auth(&mut kept, Some(String::new())), kept.is_some()),
            (Some(String::new()), false),
            "case \"emptied\": the empty value removes the credential"
        );
    }
}
