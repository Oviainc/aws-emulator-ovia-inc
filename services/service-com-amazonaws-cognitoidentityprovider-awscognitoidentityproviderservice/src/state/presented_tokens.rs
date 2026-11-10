// SPDX-License-Identifier: CC0-1.0

//! What a pool of this service says about a token ANOTHER service was presented:
//! the server-side validation an integrated identity pool asks a user pool for.

use super::*;

/// What the user pool a presented token names says about the USER that token
/// stands for.
///
/// The answer to the one question a server-side token validation asks a user pool:
/// an integrated identity pool "will check with the integrated user pools to make
/// sure that the user has not been globally signed out or deleted before the
/// identity pool provides an OIDC token or Amazon Web Services credentials for the
/// user" (`CognitoIdentityProvider$ServerSideTokenCheck`, the Amazon Cognito
/// Identity model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentedTokenUser {
    /// The pool resolves the token to a user that still signs in with it.
    Active,
    /// It resolves the token to no user that does: one it has signed out — every
    /// token of a deactivated user is revoked, as is every token a revocation or a
    /// global sign-out named — one it holds deactivated, and one it no longer
    /// holds at all.
    Inactive,
    /// This service holds no pool of that id, so it validates nothing about the
    /// token: what such a token says is the pool's to decide, wherever that pool
    /// is held.
    Unknown,
}

/// The claim naming "the principal that is the subject of the JWT" (RFC 7519
/// §4.1.2), which the tokens a pool issues carry the stored user's immutable id in
/// (`sub`, written by this service's own minting).
const SUBJECT_CLAIM: &str = "sub";

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// What the pool `user_pool_id` says about `token`, presented to another service
    /// as the login of one of this pool's users.
    ///
    /// Two things can resolve the token to a user, and the stored ISSUANCE comes
    /// first: a token this pool issued is held beside the user it was issued to and
    /// beside whether the pool has revoked it since, so a sign-out, a deactivation
    /// and a token revocation each leave that record resolving to no active user,
    /// whatever the token's own claims still say. Any other value is resolved by the
    /// subject it claims ([`SUBJECT_CLAIM`]), so a token whose issuance record the
    /// store has already swept is still answered for the user it names, and one this
    /// pool holds no user for stands for no user of it.
    ///
    /// A PURE read: nothing is swept, revoked or expired here, so asking changes
    /// nothing a later caller sees.
    pub fn presented_token_user(&self, user_pool_id: &str, token: &str) -> PresentedTokenUser {
        let Some(pool) = self.pool(user_pool_id) else {
            return PresentedTokenUser::Unknown;
        };
        if let Some(record) = self
            .issued_tokens
            .get(token)
            .filter(|record| record.user_pool_id == user_pool_id)
        {
            if record.revoked {
                return PresentedTokenUser::Inactive;
            }
            // The record names its user by name, and a name can be taken again, so
            // the stored user it finds is the one this token was issued to only while
            // it still carries the same id: a token outliving its user resolves to no
            // user rather than to a later namesake.
            return match pool.user(&record.username) {
                Some(user) if user.sub == record.sub => user_activity(user),
                _ => PresentedTokenUser::Inactive,
            };
        }
        match claimed_subject(token).and_then(|subject| pool.user(&subject)) {
            Some(user) => user_activity(user),
            None => PresentedTokenUser::Inactive,
        }
    }
}

/// Whether the stored user `user` still signs in: "a deactivated user can't sign
/// in" (`AdminDisableUser` in this service's model), so a user whose profile is
/// deactivated stands behind no token of its own.
fn user_activity(user: &User) -> PresentedTokenUser {
    if user.enabled {
        PresentedTokenUser::Active
    } else {
        PresentedTokenUser::Inactive
    }
}

/// The subject `token` claims, when it is a readable JSON Web Token carrying one.
///
/// The reading is [`::service_common::jwt::read_unsecured`], the claim-set reading
/// that leaves the signature judgement out: the claim is used to RESOLVE a stored
/// user, and what that user's state is then decides the answer, so a value nothing
/// signed resolves a user no more easily than it would otherwise.
fn claimed_subject(token: &str) -> Option<String> {
    ::service_common::jwt::read_unsecured(token)
        .ok()
        .and_then(|read| read.claim_str(SUBJECT_CLAIM).map(ToString::to_string))
}
