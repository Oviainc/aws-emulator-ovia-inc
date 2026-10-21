// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! AWS credentials in the [`CredentialStore`](crate::CredentialStore): the access key and session
//! kinds SigV4 verifies, and the names of their secrets.

use crate::credential_store::{CredentialKind, MintedCredential, Secrets, random_from, sealed};

pub const SIGV4_SCHEME: &str = "aws.auth#sigv4";
pub const SECRET_ACCESS_KEY: &str = "secret_access_key";
pub const SESSION_TOKEN: &str = "session_token";

/// Key ids are 20 characters, as observed against AWS
/// (<https://docs.aws.amazon.com/IAM/latest/APIReference/API_AccessKey.html> allows 16 to 128).
/// They start `EMULATOR` rather than AWS's `AKIA`/`ASIA`, as the published key does, so secret
/// scanners never mistake one for an AWS key.
const KEY_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Secret access keys are 40 characters, as observed against AWS.
const SECRET_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// Session tokens are opaque, and their size varies with what they carry (compressed policies
/// and tags); STS caps the assembled token at 4,096 bytes:
/// <https://aws.amazon.com/blogs/security/aws-sts-simplifies-session-token-size-limits-and-adds-session-token-size-monitoring/>.
/// The emulator mints 712 characters, the length of an `AssumeRole` token observed against AWS.
const SESSION_TOKEN_LENGTH: usize = 712;

/// Long-term access keys, for `CredentialStore::create`.
pub static AWS_ACCESS_KEY: AwsAccessKey = AwsAccessKey;
/// Session credentials, for `CredentialStore::create`.
pub static AWS_SESSION: AwsSession = AwsSession;

/// A long-term access key, as IAM `CreateAccessKey` returns:
/// <https://docs.aws.amazon.com/IAM/latest/APIReference/API_CreateAccessKey.html>.
pub struct AwsAccessKey;

/// Session credentials with a session token, as STS returns:
/// <https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_temp_use-resources.html>.
pub struct AwsSession;

impl sealed::Sealed for AwsAccessKey {}
impl sealed::Sealed for AwsSession {}

impl CredentialKind for AwsAccessKey {
    fn scheme(&self) -> &'static str {
        SIGV4_SCHEME
    }

    fn mint(&self) -> MintedCredential {
        aws_key_with_secret("EMULATORAK")
    }
}

impl CredentialKind for AwsSession {
    fn scheme(&self) -> &'static str {
        SIGV4_SCHEME
    }

    fn mint(&self) -> MintedCredential {
        let mut minted = aws_key_with_secret("EMULATORAS");
        let token = random_from(SECRET_ALPHABET, SESSION_TOKEN_LENGTH);
        minted.secrets.insert(SESSION_TOKEN, token);
        minted
    }
}

fn aws_key_with_secret(prefix: &str) -> MintedCredential {
    let id = format!(
        "{prefix}{}",
        random_from(KEY_ID_ALPHABET, 20 - prefix.len())
    );
    let secret = random_from(SECRET_ALPHABET, 40);
    let secrets = Secrets::from([(SECRET_ACCESS_KEY, secret)]);
    MintedCredential { id, secrets }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_keys_take_the_aws_shapes() {
        for (kind, prefix, secrets) in [
            (&AwsAccessKey as &dyn CredentialKind, "EMULATORAK", 1),
            (&AwsSession, "EMULATORAS", 2),
        ] {
            let MintedCredential {
                id,
                secrets: minted,
            } = kind.mint();
            let suffix = id.strip_prefix(prefix).expect(prefix);
            assert!(id.len() == 20 && suffix.bytes().all(|b| KEY_ID_ALPHABET.contains(&b)));
            assert_eq!(
                (minted.len(), minted[SECRET_ACCESS_KEY].len()),
                (secrets, 40)
            );
        }
        assert_eq!(
            AwsSession.mint().secrets[SESSION_TOKEN].len(),
            SESSION_TOKEN_LENGTH
        );
        assert_ne!(AwsAccessKey.mint().id, AwsAccessKey.mint().id);
    }
}
