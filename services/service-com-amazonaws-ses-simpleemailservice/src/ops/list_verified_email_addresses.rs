// SPDX-License-Identifier: CC0-1.0

//! @op ListVerifiedEmailAddresses
//!
//! Lists the verified email-address identities of this account.
//!
//! HAND-AUTHORED, like `get_send_quota.rs`: the scaffold op-stub generator
//! declined `ListVerifiedEmailAddresses` because the model binds it to the
//! `smithy.api#Unit` input shape, which the generator does not emit yet. Without
//! this file the operation was modeled but UNWIRED — absent from the service's
//! `operations(...)` allow-list, so the framework answered a caller addressing it
//! through the unroutable-operation path (`400 InvalidAction`) instead of serving
//! it. The `//! @op ListVerifiedEmailAddresses` line above is what puts it back in
//! that allow-list, and `types/list_verified_email_addresses_types.rs` carries its
//! two shapes.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListVerifiedEmailAddresses::new(engine)` and drives it through the centralized
//! request walk: deserialize → validate → `handle` → serialize. You write ONLY the
//! `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `ListVerifiedEmailAddresses`
//!
//! Deprecated. Use the `ListIdentities` operation to list the email addresses and
//! domains associated with your account.
//!
//! ## Input
//!
//! (no members: the modeled input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `VerifiedEmailAddresses` (list<string>) — A list of email addresses that have
//!   been verified.
//!
//! ## Errors
//!
//! (none)

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListVerifiedEmailAddressesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ses-simpleemailservice.json");

// The operation's MODELED errors. `ListVerifiedEmailAddresses` declares none, so
// this enum is UNINHABITED, which is correct and expected: there is nothing to
// construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ses#ListVerifiedEmailAddresses")]
#[derive(Debug)]
pub enum ListVerifiedEmailAddressesErrorModeled {}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_verified_email_addresses::ListVerifiedEmailAddressesError`).
/// The wrapper is also the only place an operation whose model declares no error at
/// all can refuse a request.
pub type ListVerifiedEmailAddressesError =
    crate::scaffolded::Scaffolded<ListVerifiedEmailAddressesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<ListVerifiedEmailAddressesErrorModeled> for ListVerifiedEmailAddressesError {
    fn from(modeled: ListVerifiedEmailAddressesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListVerifiedEmailAddresses` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct ListVerifiedEmailAddresses<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListVerifiedEmailAddresses<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListVerifiedEmailAddresses<Eng>
{
    type Input = ListVerifiedEmailAddressesInput;
    type Output = ListVerifiedEmailAddressesOutput;
    type Error = ListVerifiedEmailAddressesError;

    const NAME: &'static str = "ListVerifiedEmailAddresses";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_VERIFIED_EMAIL_ADDRESSES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_VERIFIED_EMAIL_ADDRESSES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The modeled input has no members, and `ctx` carries only request
        // metadata; this operation reads neither.
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // See `verify_email_identity.rs`: an engine with no store for this
            // service cannot serve the operation at all, and it models no server
            // error of its own.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListVerifiedEmailAddresses",
            ));
        };
        let mut state = state.lock();

        // The verification move the clock already owes each identity,
        // materialised against one instant before the list is read: the move was
        // due whether or not anybody asked, and this listing reports only the
        // addresses whose verification has gone through.
        state.settle_identities(std::time::SystemTime::now());

        // Reporting only, out of the ONE identity list every identity operation
        // reads and writes: this listing is the email-address view of it, so an
        // address verified through either the deprecated `VerifyEmailAddress` or
        // `VerifyEmailIdentity` shows up here, and one removed through either
        // `DeleteVerifiedEmailAddress` or `DeleteIdentity` stops showing up.
        Ok(ListVerifiedEmailAddressesOutput {
            // Present even when empty: an account with no verified address has an
            // empty list, which is a different answer from an absent member.
            verified_email_addresses: Some(state.verified_email_addresses()),
        })
    }
}
