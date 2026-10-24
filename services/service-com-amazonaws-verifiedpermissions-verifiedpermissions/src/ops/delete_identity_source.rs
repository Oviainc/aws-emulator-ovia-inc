// SPDX-License-Identifier: CC0-1.0

//! @op DeleteIdentitySource
//!
//! REMOVES an identity source from its policy store, so
//! [`crate::ops::get_identity_source`] answers 404 for it and
//! [`crate::ops::list_identity_sources`] stops reporting it.
//!
//! A reference that names no live resource — an unknown store, or an unknown
//! identity source inside a store that does exist — is the 404
//! `ResourceNotFoundException` this operation models, as it is on every other
//! reader and deleter of this service: nothing in the operation's documentation
//! makes a delete of a source that is not there a success.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteIdentitySource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Deletes an identity source that references an identity provider (IdP) such as Amazon
//! Cognito. After you delete the identity source, you can no longer use tokens for
//! identities from that identity source to represent principals in authorization queries
//! made using IsAuthorizedWithToken. operations.
//!
//! ## Input
//!
//! * `identitySourceId` (IdentitySourceId, len 1..200, pattern ^[a-zA-Z0-9-]*$) **required**
//!   — Specifies the ID of the identity source that you want to delete.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the identity source that you want to
//!   delete. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — The request failed because another request to modify a
//!   resource occurred at the same time.
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteIdentitySourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteIdentitySourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#DeleteIdentitySource",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum DeleteIdentitySourceErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_identity_source::DeleteIdentitySourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteIdentitySourceError = crate::scaffolded::Scaffolded<DeleteIdentitySourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteIdentitySourceErrorModeled> for DeleteIdentitySourceError {
    fn from(modeled: DeleteIdentitySourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteIdentitySource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteIdentitySource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteIdentitySource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteIdentitySource<Eng>
{
    type Input = DeleteIdentitySourceInput;
    type Output = DeleteIdentitySourceOutput;
    type Error = DeleteIdentitySourceError;

    const NAME: &'static str = "DeleteIdentitySource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IDENTITY_SOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IDENTITY_SOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            DeleteIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let source_not_found = || {
            DeleteIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::ops::resource_not_found(
                    &input.identity_source_id,
                    crate::state::RESOURCE_TYPE_IDENTITY_SOURCE,
                ),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteIdentitySourceErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        // The source has to be IN the store before it is removed, so a reference
        // that names none is the modelled 404 rather than a silent success.
        store
            .identity_sources
            .remove(&input.identity_source_id)
            .ok_or_else(source_not_found)?;
        crate::ops::audit(
            ctx,
            "DeleteIdentitySource",
            &input.identity_source_id,
            "deleted",
        );

        Ok(DeleteIdentitySourceOutput {})
    }
}
