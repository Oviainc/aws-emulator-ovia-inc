// SPDX-License-Identifier: CC0-1.0

//! Which IAM user a request that carried no `UserName` addresses, resolved from the
//! access key id it was signed with.
//!
//! Every operation of this service whose `UserName` is optional resolves an absent one
//! "implicitly based on the Amazon Web Services access key ID used to sign the request"
//! (`GetUser`), and a long-term access key belongs to an IAM USER of the account — so the
//! resolution is stated ONCE here and the whole family reads it:
//!
//! * a key the account MINTED, read out of
//!   [`IamState::access_keys`](super::IamState::access_keys) — the registry
//!   `iam:CreateAccessKey` writes — resolves to the user it was minted for; and
//! * the long-term key pair this emulator PUBLISHES at start resolves to the user that
//!   pair belongs to, the record the account holds from the moment it exists
//!   ([`::service_common::caller_principal`]) — the same principal
//!   `sts:GetCallerIdentity` reports for that credential, so a client that reads its own
//!   identity and then manages it addresses one record.
//!
//! Every other credential belongs to no IAM user of this account: a temporary credential
//! of a role session speaks for a role, and a key id nothing issued speaks for nobody.
//! Such a call resolves to no user and is refused
//! ([`crate::state::must_specify_user_name`]) rather than served for an invented
//! identity.
//!
//! Resolving is a pure READ: it consults the store and the credential the request already
//! presented, and writes nothing.

use crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State as IamState;

/// The IAM user a request with no `UserName` addresses, keyed as
/// [`crate::state::name_key`] keys the account's users, or `None` for a credential that
/// belongs to no user of this account.
pub fn implicit_user_key(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<String> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    if let Some(record) = store.access_keys.get(&access_key_id) {
        return Some(record.user_key.clone());
    }
    published_credential_of(store, access_key_id).map(|credential| credential.user_key)
}

/// The long-term credential a request authenticates with when the account holds no
/// `iam:CreateAccessKey` record for it: the key pair this emulator publishes at start.
#[derive(Debug, Clone)]
pub struct PublishedCredential {
    /// The access key id the request presented, which is that pair's id — the id of a
    /// long-term key of the user below, and the only part of the pair this service ever
    /// reads (the secret half is the authentication layer's and is never touched here).
    pub access_key_id: String,
    /// [`crate::state::name_key`] of the IAM user the credential belongs to.
    pub user_key: String,
}

/// The published credential this request is authenticating with, or `None` when it
/// presents some other credential — one the account minted, a role session's temporary
/// one, or a key id nothing issued.
///
/// `None` as well when the account no longer holds the user that pair belongs to: a
/// credential whose user was deleted belongs to no IAM user, which is exactly the caller
/// the family's refusal is for.
pub fn published_credential(
    store: &IamState,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<PublishedCredential> {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    if store.access_keys.contains_key(&access_key_id) {
        return None;
    }
    published_credential_of(store, access_key_id)
}

/// [`published_credential`] for an access key id the caller has already read out of the
/// request and found no key record for, so neither question is asked twice.
fn published_credential_of(store: &IamState, access_key_id: String) -> Option<PublishedCredential> {
    if !is_published(&access_key_id) {
        return None;
    }
    let user_key = crate::state::name_key(::service_common::caller_principal::AMBIENT_USER_NAME);
    store
        .users
        .contains_key(&user_key)
        .then_some(PublishedCredential {
            access_key_id,
            user_key,
        })
}

/// Whether `access_key_id` is the id of the long-term key pair this emulator process
/// publishes at start.
///
/// Asked of the authentication layer that OWNS that pair rather than of a copy, so the
/// answer cannot drift from the credential requests are actually verified against. A
/// process that published no pair at all (authentication off, so nothing was minted)
/// answers `false`: there is no published credential for a request to be signed with.
fn is_published(access_key_id: &str) -> bool {
    ::emulator_server::SigV4Authenticator::minted_credentials()
        .is_some_and(|published| published.access_key_id == access_key_id)
}
