// SPDX-License-Identifier: CC0-1.0

//! One session `CreateSession` vended, and what it is scoped to.
//!
//! A session is two things at once, and they travel in two different places on a Zonal
//! endpoint request:
//!
//! * the KEY PAIR the request is SIGNED with — an access key id and a secret access key the
//!   emulator's credential store minted, which is what makes the signature verify at all;
//!   and
//! * the session TOKEN, which "you also apply … to the `x-amz-s3session-token` request
//!   header for authorization" (`CreateSession`'s own documentation) — not the
//!   `x-amz-security-token` header an STS temporary credential travels in. It is not
//!   signed with and it is not a SigV4 credential: S3 authorizes it, against the bucket the
//!   session was created for, because "temporary security credentials are scoped to the
//!   bucket and expire after 5 minutes".
//!
//! So the store keeps, per session, the expiry both halves share and the scope the token is
//! judged against.

use super::*;

/// What a presented session token authorizes, as
/// [`ComAmazonawsS3Amazons3State::session_token_verdict`] answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTokenVerdict {
    /// The token names a live session created for the bucket the request addressed.
    Authorized,
    /// No session this service vended carries this token.
    Unknown,
    /// A session carries it, and the clock has passed its expiry: "after the expiration
    /// time, any calls that you make with those credentials will fail".
    Expired,
    /// A live session carries it, and it was created for ANOTHER bucket — "temporary
    /// security credentials are scoped to the bucket".
    OtherBucket,
}

/// One session this service vended and has not released.
#[derive(Debug, Clone)]
pub struct VendedSession {
    /// The instant the session's credentials stop being accepted: "temporary security
    /// credentials are scoped to the bucket and expire after 5 minutes".
    pub expires_at: SystemTime,
    /// What the session is scoped to, for a session `CreateSession` vended; `None` for a
    /// record made about the COUNT of sessions alone
    /// ([`ComAmazonawsS3Amazons3State::record_vended_session`]).
    pub scope: Option<SessionScope>,
}

/// What one session is scoped to: the directory bucket it was created for and the token a
/// Zonal endpoint request presents it with.
#[derive(Debug, Clone)]
pub struct SessionScope {
    /// The directory bucket `CreateSession` was called on. A Zonal endpoint request
    /// presenting this session's token against any OTHER bucket is presenting a session
    /// that does not cover it.
    pub bucket: String,
    /// The session token the caller was handed, as it is presented in
    /// `x-amz-s3session-token`.
    pub token: String,
}
