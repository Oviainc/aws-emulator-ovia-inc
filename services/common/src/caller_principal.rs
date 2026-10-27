// SPDX-License-Identifier: CC0-1.0

//! The ONE definition of the IAM principal this emulator's own long-term credential
//! speaks for.
//!
//! The emulator publishes exactly one long-term access key at start, and on AWS a
//! long-term access key belongs to an IAM USER of the account — so that user is a record
//! the account holds from the moment it exists, before any request is served. Two services
//! answer questions about it and must not answer differently:
//!
//! * STS reports it as the calling principal ("Returns details about the IAM user or role
//!   whose credentials are used to call the operation", `GetCallerIdentity`), and
//! * IAM holds it among the account's users, so every operation that resolves a user by
//!   name resolves this one too.
//!
//! Which is why its NAME, its PATH, its ARN and its `UserId` are stated here once rather
//! than in each of them: a client that reads its own identity and then manages that
//! identity's policies addresses one record.
//!
//! Nothing here reaches the network or the clock: every value is a pure function of the
//! account contract, so the principal is the same on every call and across restarts.

/// The friendly name of that user, as `User$UserName` carries it.
///
/// Inside `userNameType` (length 1..64, pattern `^[\w+=,.@-]+$`), so it is a name IAM
/// itself would accept and a value a caller can pass straight back as a `UserName`.
pub const AMBIENT_USER_NAME: &str = "emulator";

/// The path that user sits under: the account root, which is what IAM gives every entity
/// whose creation names no path (`pathType`: "defaults to a slash (/)").
pub const AMBIENT_USER_PATH: &str = "/";

/// The type prefix AWS gives an IAM user's unique id, from the IAM "unique identifier
/// prefixes" table (`AIDA` for a user, `AROA` for a role).
const USER_ID_PREFIX: &str = "AIDA";

/// The alphabet AWS unique ids are drawn from: uppercase letters and digits.
const UNIQUE_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// An AWS unique id is a four-character type prefix plus 17 more characters, 21 in total
/// (`AIDAJ4XQZ7X5YQ2R3TZ4A`) — inside the modelled `idType` (length 16..128, pattern
/// `^[\w]+$`).
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// An AWS unique id: `prefix` plus [`UNIQUE_ID_SUFFIX_LEN`] characters derived from
/// `seed`.
///
/// The characters are a SHA-256 digest of `seed` mapped onto [`UNIQUE_ID_ALPHABET`], so
/// the id is a function of the thing it names — the same ARN always yields the same id,
/// two different ones practically never collide, and nothing about the id depends on when
/// it was asked for. This is NOT base32: the mapping only needs to be stable and well
/// distributed, never decodable.
#[must_use]
pub fn aws_unique_id(prefix: &str, seed: &str) -> String {
    use ::sha2::Digest;
    let digest = ::sha2::Sha256::digest(seed.as_bytes());
    let suffix = digest
        .iter()
        .take(UNIQUE_ID_SUFFIX_LEN)
        .map(|byte| char::from(UNIQUE_ID_ALPHABET[usize::from(*byte) % UNIQUE_ID_ALPHABET.len()]));
    prefix.chars().chain(suffix).collect()
}

/// That user's ARN: `arn:aws:iam::<account>:user/<name>`.
///
/// Built through the account contract's own ARN builder, so the account segment is the
/// canonical one every other ARN this emulator emits carries. IAM is a global service, so
/// the region segment is empty.
#[must_use]
pub fn ambient_user_arn() -> String {
    ::emulator_server::emulator_arn(
        "iam",
        "",
        &format!("user{AMBIENT_USER_PATH}{AMBIENT_USER_NAME}"),
    )
}

/// That user's `UserId` — "the stable and unique string identifying the user".
///
/// Derived from its ARN ([`aws_unique_id`]), so the id is stable across reads and across
/// restarts without anything having to be remembered, and the service that HOLDS the user
/// and the service that REPORTS it as the caller report one value.
#[must_use]
pub fn ambient_user_id() -> String {
    aws_unique_id(USER_ID_PREFIX, &ambient_user_arn())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id and the ARN are pure functions of the account contract: stable across calls,
    /// and of the form the modelled `idType` admits.
    #[test]
    fn the_ambient_principal_is_stable_and_well_formed() {
        let arn = ambient_user_arn();
        assert_eq!(
            arn,
            ambient_user_arn(),
            "case the ARN read twice: expected one value, got {arn:?}"
        );
        assert!(
            arn.ends_with("iam::000000000000:user/emulator"),
            "case the ARN: expected the account contract's IAM user ARN, got {arn:?}"
        );

        let id = ambient_user_id();
        assert_eq!(
            id,
            ambient_user_id(),
            "case the id read twice: expected one value, got {id:?}"
        );
        assert!(
            id.starts_with(USER_ID_PREFIX),
            "case the id: expected the {USER_ID_PREFIX} prefix, got {id:?}"
        );
        assert_eq!(
            id.chars().count(),
            USER_ID_PREFIX.len() + UNIQUE_ID_SUFFIX_LEN,
            "case the id: expected an AWS unique id's length, got {id:?}"
        );
        assert!(
            id.chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()),
            "case the id: expected idType's ^[\\w]+$ alphabet, got {id:?}"
        );
    }
}
