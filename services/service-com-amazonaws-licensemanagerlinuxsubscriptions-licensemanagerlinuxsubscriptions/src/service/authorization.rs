// SPDX-License-Identifier: CC0-1.0

//! The ONE authorization pre-condition this service's tagging surface applies, ahead of
//! every input check and every lookup.
//!
//! The tag APIs (`/tags/{resourceArn}`) are the only operations of this service that take an
//! ARN of "the Amazon Web Services resource" from the caller rather than a `SubscriptionProviderArn`
//! this account minted, so they are the only ones whose request can name a resource of
//! ANOTHER account. The account field of that ARN settles authorization on its own: no
//! document of the account a request is served for can grant anything over a resource in a
//! different account, so there is nothing to look up and nothing a lookup could change about
//! the verdict. That is why the gate is stated here once and applied by all three handlers
//! before they touch the store: a denial must not depend on whether this account happens to
//! hold a resource under the same id, and a refusal of absence must never stand in for one
//! of authorization — a client distinguishes the two by status, treating 403 as permanent and
//! 404 as a resource it may create or ignore.
//!
//! Nothing here reads the store or the clock: the decision is a function of the request's own
//! ARN and of the account contract this emulator serves, so it is the same on every call.

/// The account every request to this emulator is served for, and therefore the account a
/// resource has to belong to for the caller to hold anything over it.
///
/// Read from the account contract (the same source every ARN this emulator mints carries in
/// its account field), never from the request.
#[must_use]
pub fn caller_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The IAM principal every request to this emulator speaks for — the identity the account
/// holds and `sts:GetCallerIdentity` reports — which is the principal an authorization
/// refusal names.
///
/// Read from the shared definition of that identity, so the principal this service refuses in
/// the name of is the one every other service of this emulator names.
#[must_use]
pub fn caller_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The authorization verdict for a tag request addressed at `resource_arn`.
///
/// `Ok(())` when nothing about the ARN refuses the request — its account field is this
/// emulator's account, or it names no account at all (an account-less ARN belongs to no other
/// account). Otherwise the denial, ready to be returned from any tag operation's error
/// channel: HTTP 403 with the principal whose authorization was evaluated and nothing about
/// what it asked for, so a caller cannot learn from it what exists in an account that is not
/// its own.
///
/// An ARN this service cannot parse is NOT denied here: it names no account to decide on, and
/// the surface already refuses it as a malformed `resourceArn`
/// ([`crate::scaffolded::UnmodeledRefusal::invalid_resource_arn`], stated at the request
/// boundary by the modelled `@pattern` on the label).
pub fn authorize_resource_arn<E>(
    resource_arn: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let Some(parsed) = ::service_common::arn::parse(resource_arn) else {
        return Ok(());
    };
    if parsed.account.is_empty() || parsed.account == caller_account() {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::access_denied(
        &caller_principal(),
    ))
}
