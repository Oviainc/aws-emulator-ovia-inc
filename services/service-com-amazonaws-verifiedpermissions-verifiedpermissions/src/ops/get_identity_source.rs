// SPDX-License-Identifier: CC0-1.0

//! @op GetIdentitySource
//!
//! Retrieves the details about the specified identity source, READ out of
//! [`crate::state`] — the identity source
//! [`crate::ops::create_identity_source`] persisted. `@readonly`: this handler
//! writes nothing.
//!
//! The reported `issuer` and the deprecated `details.discoveryUrl` are COMPUTED
//! from the stored user pool ARN, in the forms the model documents
//! (`https://cognito-idp.<region>.amazonaws.com/<user-pool-id>` and that URL
//! plus `/.well-known/openid-configuration`); see
//! [`crate::state::CognitoUserPoolSource`].
//!
//! ## Input
//!
//! * `identitySourceId` (IdentitySourceId, len 1..200, pattern ^[a-zA-Z0-9-]*$) **required**
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   the store's id, or `policy-store-alias/<name>` naming it.
//!
//! ## Output
//!
//! * `configuration` (ConfigurationDetail) — the provider this source points at.
//! * `createdDate`, `identitySourceId`, `lastUpdatedDate`, `policyStoreId`,
//!   `principalEntityType` — all **required**.
//! * `details` (IdentitySourceDetails) — DEPRECATED ("replaced by
//!   configuration.cognitoUserPoolConfiguration"), so it is reported only for a
//!   Cognito source, whose fields it describes.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — no such policy store, or no such
//!   identity source in it.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetIdentitySourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`; it is the honest answer when the service's state
/// store cannot be reached.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#GetIdentitySource",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum GetIdentitySourceErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_identity_source::GetIdentitySourceError`).
pub type GetIdentitySourceError = crate::scaffolded::Scaffolded<GetIdentitySourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<GetIdentitySourceErrorModeled> for GetIdentitySourceError {
    fn from(modeled: GetIdentitySourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetIdentitySource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetIdentitySource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetIdentitySource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// A persisted configuration as the modeled `ConfigurationDetail`.
pub(crate) fn configuration_detail(
    configuration: &crate::state::IdentitySourceConfiguration,
) -> ConfigurationDetail {
    match configuration {
        crate::state::IdentitySourceConfiguration::CognitoUserPool(cognito) => {
            ConfigurationDetail::cognitoUserPoolConfiguration(CognitoUserPoolConfigurationDetail {
                user_pool_arn: cognito.user_pool_arn.clone(),
                client_ids: cognito.client_ids.clone(),
                issuer: cognito.issuer(),
                group_configuration: cognito.group_entity_type.as_ref().map(|entity_type| {
                    CognitoGroupConfigurationDetail {
                        group_entity_type: Some(entity_type.clone()),
                    }
                }),
            })
        }
        crate::state::IdentitySourceConfiguration::OpenIdConnect(oidc) => {
            ConfigurationDetail::openIdConnectConfiguration(OpenIdConnectConfigurationDetail {
                issuer: oidc.issuer.clone(),
                entity_id_prefix: oidc.entity_id_prefix.clone(),
                group_configuration: oidc.group_configuration.as_ref().map(|groups| {
                    OpenIdConnectGroupConfigurationDetail {
                        group_claim: groups.group_claim.clone(),
                        group_entity_type: groups.group_entity_type.clone(),
                    }
                }),
                token_selection: match &oidc.token_selection {
                    crate::state::OpenIdConnectTokens::AccessToken {
                        principal_id_claim,
                        audiences,
                    } => OpenIdConnectTokenSelectionDetail::accessTokenOnly(
                        OpenIdConnectAccessTokenConfigurationDetail {
                            principal_id_claim: principal_id_claim.clone(),
                            audiences: (!audiences.is_empty()).then(|| audiences.clone()),
                        },
                    ),
                    crate::state::OpenIdConnectTokens::IdentityToken {
                        principal_id_claim,
                        client_ids,
                    } => OpenIdConnectTokenSelectionDetail::identityTokenOnly(
                        OpenIdConnectIdentityTokenConfigurationDetail {
                            principal_id_claim: principal_id_claim.clone(),
                            client_ids: (!client_ids.is_empty()).then(|| client_ids.clone()),
                        },
                    ),
                },
            })
        }
    }
}

/// The DEPRECATED `details` view of a persisted configuration. Its members
/// describe a Cognito user pool ("replaced by
/// configuration.cognitoUserPoolConfiguration"), so an OIDC source reports none.
///
/// `pub(crate)` so [`crate::ops::list_identity_sources`] reports the same view of
/// the same stored source: `IdentitySourceItemDetails` is member-identical to
/// `IdentitySourceDetails`, so the list operation re-types this result rather
/// than deciding a second time what the deprecated view contains.
pub(crate) fn details_for(
    configuration: &crate::state::IdentitySourceConfiguration,
) -> Option<IdentitySourceDetails> {
    match configuration {
        crate::state::IdentitySourceConfiguration::CognitoUserPool(cognito) => {
            Some(IdentitySourceDetails {
                client_ids: Some(cognito.client_ids.clone()),
                user_pool_arn: Some(cognito.user_pool_arn.clone()),
                discovery_url: Some(cognito.discovery_url()),
                // `OpenIdIssuer` models exactly one value.
                open_id_issuer: Some("COGNITO".to_string()),
            })
        }
        crate::state::IdentitySourceConfiguration::OpenIdConnect(_) => None,
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetIdentitySource<Eng>
{
    type Input = GetIdentitySourceInput;
    type Output = GetIdentitySourceOutput;
    type Error = GetIdentitySourceError;

    const NAME: &'static str = "GetIdentitySource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IDENTITY_SOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_IDENTITY_SOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let store_not_found = || {
            GetIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let source_not_found = || {
            GetIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::ops::resource_not_found(&input.identity_source_id, "IDENTITY_SOURCE"),
            )
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetIdentitySourceErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(store_not_found)?;
        let store = state
            .policy_stores
            .get(&store_id)
            .ok_or_else(store_not_found)?;
        let source = store
            .identity_sources
            .get(&input.identity_source_id)
            .ok_or_else(source_not_found)?;

        Ok(GetIdentitySourceOutput {
            policy_store_id: source.policy_store_id.clone(),
            identity_source_id: source.identity_source_id.clone(),
            principal_entity_type: source.principal_entity_type.clone(),
            created_date: source.created_date,
            last_updated_date: source.last_updated_date,
            configuration: Some(configuration_detail(&source.configuration)),
            details: details_for(&source.configuration),
        })
    }
}
