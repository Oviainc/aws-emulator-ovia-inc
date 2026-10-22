// SPDX-License-Identifier: CC0-1.0

//! @op CreateIdentitySource
//!
//! Adds an identity source to a policy store and INSERTS it into
//! [`crate::state`]: the id and timestamps this returns are the ones
//! [`crate::ops::get_identity_source`] reads back.
//!
//! ## What the provider reference is checked against
//!
//! Verified Permissions resolves the identity provider named by `configuration`
//! before it creates anything, and AWS documents that an Amazon Cognito user
//! pool identity source must name a pool in the same account and Region as the
//! policy store. This emulator presents exactly one account
//! ([`engine::emulator_account_id`]) and serves the region the request was
//! signed for, so a `userPoolArn` naming any other account or region names a
//! pool that does not exist here and is refused with a 400
//! `ValidationException`. [`crate::state::resolve_user_pool_arn`] is the one
//! implementation of that rule.
//!
//! LIMIT OF THAT CHECK, stated plainly: a well-formed ARN in this emulator's
//! account and in the request's own region is accepted even when no
//! `CreateUserPool` ever minted that pool. Whether a pool exists is knowledge
//! the cognito-idp emulator holds in ITS OWN state struct, and per-service state
//! is reached through the engine BY TYPE
//! ([`crate::state::service_state`]) — the type lives in that service's crate,
//! which this crate does not (and by the project's one-crate-per-service layout
//! must not) depend on, and no shared crate publishes a pool registry. So the
//! account, the region and the region the pool id itself names are the whole of
//! what this service can resolve; anything beyond them would be a guess, and a
//! guess is not made.
//!
//! An OIDC `issuer` is a provider OUTSIDE the emulator, and AWS resolves it by
//! fetching `<issuer>/.well-known/openid-configuration`. This emulator makes no
//! outbound connections, so it cannot reproduce that fetch: a syntactically
//! valid issuer (the model's `^https://.*$`, enforced before this handler runs)
//! is accepted as given, and nothing about the provider is invented.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..64, pattern ^[a-zA-Z0-9-]*$, @idempotencyToken)
//!   — "Verified Permissions recognizes a ClientToken for eight hours"; a replay
//!   answers with the identity source the first call created, and the same token
//!   with different parameters is a `ConflictException`.
//! * `configuration` (Configuration) **required** — the Cognito user pool or the
//!   OIDC provider this identity source points at.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   the store's id, or `policy-store-alias/<name>` naming it.
//! * `principalEntityType` (PrincipalEntityType, len 1..200, pattern ^.*$) — the
//!   Cedar entity type of the principals this source generates. Optional here
//!   while required on every read, so an absent value resolves to
//!   [`crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE`] for a Cognito
//!   source; an OIDC source generates principals of a type only the caller can
//!   name, so omitting it there is refused.
//!
//! ## Output
//!
//! * `createdDate`, `identitySourceId`, `lastUpdatedDate`, `policyStoreId` — all
//!   **required**, all read back out of what was just persisted.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — the same `clientToken` was replayed with
//!   different parameters.
//! * `ResourceNotFoundException` (HTTP 404) — no such policy store.
//! * `ServiceQuotaExceededException` (HTTP 402) — the store is at
//!   [`crate::state::MAX_IDENTITY_SOURCES_PER_STORE`].
//! * `ValidationException` (HTTP 400) — the configuration names a provider this
//!   emulator cannot resolve.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateIdentitySourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`; it is the honest answer when the service's state
/// store cannot be reached, which is a server fault and not a bad request.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreateIdentitySource",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum CreateIdentitySourceErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_identity_source::CreateIdentitySourceError`). A provider
/// reference it cannot resolve, and an omitted `principalEntityType` it cannot
/// default, are answered with the MODELLED 400 `ValidationException`, whose
/// `fieldList` names the member — see [`crate::ops::validation_exception`].
pub type CreateIdentitySourceError = crate::scaffolded::Scaffolded<CreateIdentitySourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<CreateIdentitySourceErrorModeled> for CreateIdentitySourceError {
    fn from(modeled: CreateIdentitySourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateIdentitySource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateIdentitySource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateIdentitySource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Reduce the request's `Configuration` to what state persists, refusing a
/// provider reference this emulator cannot resolve. The `Err` names the member
/// the caller has to fix, for the modelled `ValidationException`'s `fieldList`.
pub(crate) fn persisted_configuration(
    configuration: &Configuration,
    request_region: &str,
) -> Result<crate::state::IdentitySourceConfiguration, crate::ops::InvalidInput> {
    match configuration {
        Configuration::cognitoUserPoolConfiguration(cognito) => {
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
        Configuration::openIdConnectConfiguration(oidc) => {
            let token_selection = match &oidc.token_selection {
                OpenIdConnectTokenSelection::accessTokenOnly(access) => {
                    crate::state::OpenIdConnectTokens::AccessToken {
                        principal_id_claim: access.principal_id_claim.clone(),
                        audiences: access.audiences.clone().unwrap_or_default(),
                    }
                }
                OpenIdConnectTokenSelection::identityTokenOnly(identity) => {
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

/// Render a persisted identity source as this operation's output. Every member
/// is read out of state, so a value that never reached state cannot appear on
/// the wire.
fn output_for(source: &crate::state::IdentitySource) -> CreateIdentitySourceOutput {
    CreateIdentitySourceOutput {
        policy_store_id: source.policy_store_id.clone(),
        identity_source_id: source.identity_source_id.clone(),
        created_date: source.created_date,
        last_updated_date: source.last_updated_date,
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateIdentitySource<Eng>
{
    type Input = CreateIdentitySourceInput;
    type Output = CreateIdentitySourceOutput;
    type Error = CreateIdentitySourceError;

    const NAME: &'static str = "CreateIdentitySource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_IDENTITY_SOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_IDENTITY_SOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            CreateIdentitySourceErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };
        let unavailable = || {
            CreateIdentitySourceErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };
        // A member of the request this service refuses on its own checks answers
        // with the model's own `ValidationException`, whose `fieldList` names it.
        let invalid = |invalid: crate::ops::InvalidInput| -> CreateIdentitySourceError {
            CreateIdentitySourceErrorModeled::ValidationException(
                crate::ops::validation_exception([invalid]),
            )
            .into()
        };

        // The provider reference is resolved before any state is touched: a
        // request this service cannot resolve must change nothing.
        let region = crate::state::request_region(ctx.parts());
        let configuration = persisted_configuration(&input.configuration, &region)
            .map_err(invalid)?;

        // `principalEntityType` is optional on the create and REQUIRED on every
        // read (`GetIdentitySourceOutput`, `IdentitySourceItem`), so an omitted
        // value has to resolve to something here rather than at read time.
        //
        // A Cognito source has a service default to resolve to
        // (`AWS::Cognito`). An OIDC source does not: its principals are
        // `Namespace::[Entity type]::[entityIdPrefix]|[attribute]`, and the
        // entity type is a name only the caller's own Cedar schema supplies —
        // nothing in the model, the request or this emulator's state determines
        // it. Rather than SYNTHESISE an entity type that would then be reported
        // as if the caller had chosen it, the request is refused with the 400
        // the model's `ValidationException` carries, naming the member. This is
        // this emulator's refusal, not a recorded AWS behaviour: the model
        // declares the member optional and documents no such requirement, and
        // the alternative (inventing the value a read must report) is the one
        // thing an emulator must not do.
        let principal_entity_type = match (&input.principal_entity_type, &configuration) {
            (Some(entity_type), _) => entity_type.clone(),
            (None, crate::state::IdentitySourceConfiguration::CognitoUserPool(_)) => {
                crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE.to_string()
            }
            (None, crate::state::IdentitySourceConfiguration::OpenIdConnect(_)) => {
                return Err(invalid(crate::ops::InvalidInput::new(
                    crate::ops::paths::PRINCIPAL_ENTITY_TYPE,
                    "an OpenID Connect identity source generates principals of the entity type \
                     the request names, so it cannot be omitted",
                )));
            }
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;

        // `@idempotencyToken` replay: the same token answers with the identity
        // source the first call created, and the same token with different
        // parameters is the `ConflictException` the model documents.
        let parameters = format!(
            "store={store_id};principalEntityType={principal_entity_type};configuration={}",
            configuration.fingerprint()
        );
        if let Some(token) = input.client_token.as_deref() {
            if let Some(record) = state.idempotency_replay("CreateIdentitySource", token) {
                if record.parameters != parameters {
                    // Fixed message; the identity source the token already
                    // created is reported in `resources`.
                    return Err(CreateIdentitySourceErrorModeled::ConflictException(
                        crate::ops::conflict([crate::ops::resource_conflict(
                            &record.resource_id,
                            crate::state::RESOURCE_TYPE_IDENTITY_SOURCE,
                        )]),
                    )
                    .into());
                }
                if let Some(source) = state
                    .policy_stores
                    .get(&store_id)
                    .and_then(|store| store.identity_sources.get(&record.resource_id))
                {
                    return Ok(output_for(source));
                }
                // The token's identity source is no longer in the store (only a
                // delete can do that), so the ledger record is STALE: there is
                // nothing to answer the replay with. The call continues as a
                // fresh create and `record_idempotency` below replaces the
                // stale record, which is deliberate — answering a modelled
                // error here would make a token unusable forever because the
                // resource it once named was deleted.
            }
        }

        let store = state.policy_stores.get_mut(&store_id).ok_or_else(not_found)?;
        // The identity-source map is the one structure this request grows, so the
        // bound is checked before anything is minted.
        if store.identity_sources.len() >= crate::state::MAX_IDENTITY_SOURCES_PER_STORE {
            return Err(
                CreateIdentitySourceErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "policy store {store_id} already holds the maximum of {} identity sources",
                            crate::state::MAX_IDENTITY_SOURCES_PER_STORE
                        ),
                        resource_id: Some(store_id.clone()),
                        resource_type: crate::state::RESOURCE_TYPE_IDENTITY_SOURCE.to_string(),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let identity_source_id = store.mint_identity_source_id();
        let created = crate::state::now();
        store.identity_sources.insert(
            identity_source_id.clone(),
            crate::state::IdentitySource {
                identity_source_id: identity_source_id.clone(),
                policy_store_id: store_id.clone(),
                principal_entity_type,
                created_date: created,
                last_updated_date: created,
                configuration,
            },
        );
        if let Some(token) = input.client_token.as_deref() {
            state.record_idempotency(
                "CreateIdentitySource",
                token,
                &identity_source_id,
                parameters,
            );
        }

        // Read the response back out of what was just persisted, so a field that
        // did not reach state cannot appear in the response.
        let stored = state
            .policy_stores
            .get(&store_id)
            .and_then(|store| store.identity_sources.get(&identity_source_id))
            .ok_or_else(unavailable)?;
        let output = output_for(stored);
        crate::ops::audit(
            ctx,
            "CreateIdentitySource",
            &output.identity_source_id,
            "created",
        );
        Ok(output)
    }
}
