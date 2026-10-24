// SPDX-License-Identifier: CC0-1.0

//! @op UpdateIdentitySource
//!
//! REPLACES the identity provider an identity source points at, in
//! [`crate::state`], so [`crate::ops::get_identity_source`] and
//! [`crate::ops::list_identity_sources`] report the NEW configuration.
//!
//! `updateConfiguration` is a complete `UpdateConfiguration` union value — every
//! member its arm needs is on it — so the stored configuration is REPLACED by
//! what the request names rather than merged into: a member the new
//! configuration does not carry (an `entityIdPrefix`, a group configuration) is
//! absent afterwards, exactly as it would be on a create carrying the same
//! configuration. `principalEntityType` is a request member beside it and keeps
//! its own rule: the request's value when it carries one, otherwise the stored
//! one, because every read of an identity source requires it.
//!
//! A Cognito `userPoolArn` is resolved exactly as the create resolves it
//! ([`crate::state::resolve_user_pool_arn`]), so a pool this emulator cannot
//! resolve is the same 400 `ValidationException` here as there and nothing is
//! written.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateIdentitySource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Updates the specified identity source to use a new identity provider (IdP), or to change
//! the mapping of identities from the IdP to a different principal entity type. Verified
//! Permissions is eventually consistent . It can take a few seconds for a new or changed
//! element to propagate through the service and be visible in the results of other Verified
//! Permissions operations.
//!
//! ## Input
//!
//! * `identitySourceId` (IdentitySourceId, len 1..200, pattern ^[a-zA-Z0-9-]*$) **required**
//!   — Specifies the ID of the identity source that you want to update.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the identity source that you want to
//!   update. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//! * `principalEntityType` (PrincipalEntityType, len 1..200, pattern ^.*$) — Specifies the
//!   data type of principals generated for identities authenticated by the identity source.
//! * `updateConfiguration` (UpdateConfiguration) **required** — Specifies the details
//!   required to communicate with the identity provider (IdP) associated with this identity
//!   source.
//!
//! ## Output
//!
//! * `createdDate` (TimestampFormat) **required** — The date and time that the updated
//!   identity source was originally created.
//! * `identitySourceId` (IdentitySourceId, len 1..200, pattern ^[a-zA-Z0-9-]*$) **required**
//!   — The ID of the updated identity source.
//! * `lastUpdatedDate` (TimestampFormat) **required** — The date and time that the identity
//!   source was most recently updated.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   The ID of the policy store that contains the updated identity source.
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
// `UpdateIdentitySourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateIdentitySourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#UpdateIdentitySource",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum UpdateIdentitySourceErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_identity_source::UpdateIdentitySourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateIdentitySourceError = crate::scaffolded::Scaffolded<UpdateIdentitySourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateIdentitySourceErrorModeled> for UpdateIdentitySourceError {
    fn from(modeled: UpdateIdentitySourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateIdentitySource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateIdentitySource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateIdentitySource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Reduce an `UpdateConfiguration` to what state persists, refusing a provider
/// reference this emulator cannot resolve. The `Err` names the member the caller
/// has to fix, for the modelled `ValidationException`'s `fieldList`.
///
/// `UpdateConfiguration` is member-identical to the `Configuration` a create
/// carries, so this translates the same provider description into the same
/// stored shape — and a Cognito pool reference goes through
/// [`crate::state::resolve_user_pool_arn`], the one implementation of that rule,
/// so an update accepts exactly the pools a create accepts.
fn persisted_configuration(
    configuration: &UpdateConfiguration,
    request_region: &str,
) -> Result<crate::state::IdentitySourceConfiguration, crate::ops::InvalidInput> {
    match configuration {
        UpdateConfiguration::cognitoUserPoolConfiguration(cognito) => {
            let (region, user_pool_id) =
                crate::state::resolve_user_pool_arn(&cognito.user_pool_arn, request_region)?;
            Ok(crate::state::IdentitySourceConfiguration::CognitoUserPool(
                crate::state::CognitoUserPoolSource {
                    user_pool_arn: cognito.user_pool_arn.clone(),
                    user_pool_id,
                    region,
                    client_ids: cognito.client_ids.clone().unwrap_or_default(),
                    group_entity_type: cognito
                        .group_configuration
                        .as_ref()
                        .map(|group| group.group_entity_type.clone()),
                },
            ))
        }
        UpdateConfiguration::openIdConnectConfiguration(oidc) => {
            let token_selection = match &oidc.token_selection {
                UpdateOpenIdConnectTokenSelection::accessTokenOnly(access) => {
                    crate::state::OpenIdConnectTokens::AccessToken {
                        principal_id_claim: access.principal_id_claim.clone(),
                        audiences: access.audiences.clone().unwrap_or_default(),
                    }
                }
                UpdateOpenIdConnectTokenSelection::identityTokenOnly(identity) => {
                    crate::state::OpenIdConnectTokens::IdentityToken {
                        principal_id_claim: identity.principal_id_claim.clone(),
                        client_ids: identity.client_ids.clone().unwrap_or_default(),
                    }
                }
            };
            Ok(crate::state::IdentitySourceConfiguration::OpenIdConnect(
                crate::state::OpenIdConnectSource {
                    issuer: oidc.issuer.clone(),
                    entity_id_prefix: oidc.entity_id_prefix.clone(),
                    group_configuration: oidc.group_configuration.as_ref().map(|group| {
                        crate::state::OpenIdConnectGroups {
                            group_claim: group.group_claim.clone(),
                            group_entity_type: group.group_entity_type.clone(),
                        }
                    }),
                    token_selection,
                },
            ))
        }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateIdentitySource<Eng>
{
    type Input = UpdateIdentitySourceInput;
    type Output = UpdateIdentitySourceOutput;
    type Error = UpdateIdentitySourceError;

    const NAME: &'static str = "UpdateIdentitySource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_IDENTITY_SOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_IDENTITY_SOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store_not_found = || {
            UpdateIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let source_not_found = || {
            UpdateIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::ops::resource_not_found(
                    &input.identity_source_id,
                    crate::state::RESOURCE_TYPE_IDENTITY_SOURCE,
                ),
            )
        };
        let unavailable = || {
            UpdateIdentitySourceErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };
        let invalid = |invalid: crate::ops::InvalidInput| -> UpdateIdentitySourceError {
            UpdateIdentitySourceErrorModeled::ValidationException(
                crate::ops::validation_exception([invalid]),
            )
            .into()
        };

        // The provider reference is resolved BEFORE any state is touched: a
        // configuration this service cannot resolve must change nothing.
        let region = crate::state::request_region(ctx.parts());
        let configuration =
            persisted_configuration(&input.update_configuration, &region).map_err(invalid)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get_mut(&store_id)
            .ok_or_else(store_not_found)?;
        // An `identitySourceId` is only ever an id — the model documents no
        // `name/` form for it, as it does for a policy and a template — so the
        // stored source is looked up by the id the request carries.
        let source = store
            .identity_sources
            .get_mut(&input.identity_source_id)
            .ok_or_else(source_not_found)?;

        source.configuration = configuration;
        // "Specifies the data type of principals generated for identities
        // authenticated by the identity source": optional here and REQUIRED on
        // every read, so a request naming none leaves the stored value as it is
        // rather than clearing a member a read must still report.
        if let Some(principal_entity_type) = input.principal_entity_type.clone() {
            source.principal_entity_type = principal_entity_type;
        }
        source.last_updated_date = crate::state::now();

        // Read the response back out of what was just persisted, so a value that
        // did not reach state cannot appear in the response.
        let output = UpdateIdentitySourceOutput {
            policy_store_id: source.policy_store_id.clone(),
            identity_source_id: source.identity_source_id.clone(),
            created_date: source.created_date,
            last_updated_date: source.last_updated_date,
        };
        crate::ops::audit(
            ctx,
            "UpdateIdentitySource",
            &output.identity_source_id,
            "updated",
        );
        Ok(output)
    }
}
