// SPDX-License-Identifier: CC0-1.0

//! @op CreateAppBundle
//!
//! Mints an app bundle and PERSISTS it: the response is built from the stored
//! record (see [`crate::state::AppBundleRecord::to_app_bundle`]), so
//! `GetAppBundle` and `ListAppBundles` answer from the same bytes this call
//! wrote.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreateAppBundle`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json")
//! ```
//!
//! Creates an app bundle to collect data from an application using AppFabric.
//!
//! ## Input
//!
//! * `clientToken` (UUID, pattern
//!   ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$, @idempotencyToken) —
//!   Specifies a unique, case-sensitive identifier that you provide to ensure the idempotency
//!   of the request. This lets you safely retry the request without accidentally performing
//!   the same operation a second time. Passing the same value to a later call to an operation
//!   requires that you also pass the same value for all other parameters. We recommend that
//!   you use a UUID type of value. If you don't provide this value, then Amazon Web Services
//!   generates a random one for you. If you retry the operation with the same ClientToken,
//!   but with different parameters, the retry fails with an IdempotentParameterMismatch
//!   error.
//! * `customerManagedKeyIdentifier` (Identifier, len 1..1011, pattern
//!   ^arn:.+$|^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$) — The Amazon
//!   Resource Name (ARN) of the Key Management Service (KMS) key to use to encrypt the
//!   application data. If this is not specified, an Amazon Web Services owned key is used for
//!   encryption.
//! * `tags` (TagList, len 0..50) — A map of the key-value pairs of the tag or tags to assign
//!   to the resource.
//!
//! ## Output
//!
//! * `appBundle` (AppBundle) **required** — Contains information about an app bundle.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You are not authorized to perform this operation.
//! * `ConflictException` (HTTP 409) — The request has created a conflict. Check the request
//!   parameters and try again.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception, or failure with an internal server.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request exceeds a service quota.
//! * `ThrottlingException` (HTTP 429) — The request rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — The request has invalid or missing parameters.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateAppBundleError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.appfabric#CreateAppBundle")]
#[derive(Debug)]
pub enum CreateAppBundleErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_app_bundle::CreateAppBundleError`). The wrapper stays so the
/// handler can still refuse a request this operation's model has no error for.
pub type CreateAppBundleError = crate::scaffolded::Scaffolded<CreateAppBundleErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateAppBundleErrorModeled> for CreateAppBundleError {
    fn from(modeled: CreateAppBundleErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateAppBundle` operation.
pub struct CreateAppBundle<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateAppBundle<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateAppBundle<Eng>
{
    type Input = CreateAppBundleInput;
    type Output = CreateAppBundleOutput;
    type Error = CreateAppBundleError;

    const NAME: &'static str = "CreateAppBundle";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APP_BUNDLE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APP_BUNDLE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateAppBundleErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the AppFabric emulator could not reach its state store".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };

        let region = crate::state::request_region(ctx);
        // The response member is an ARN, so a bare key id is expanded into one.
        let customer_managed_key_arn = input
            .customer_managed_key_identifier
            .as_deref()
            .map(|identifier| crate::state::customer_managed_key_arn(identifier, &region));
        let fingerprint = crate::state::request_fingerprint(
            &[(
                "customerManagedKeyArn",
                customer_managed_key_arn.as_deref().unwrap_or(""),
            )],
            input.tags.as_deref(),
        );

        let mut store = state.lock();

        // @idempotencyToken: the same token replays the resource the first call
        // created; the same token with different parameters is the conflict AWS
        // documents as IdempotentParameterMismatch.
        if let Some(token) = input.client_token.as_deref() {
            let key = crate::state::client_token_key(Self::NAME, token);
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                if spent.fingerprint != fingerprint {
                    return Err(CreateAppBundleErrorModeled::ConflictException(
                        crate::state::idempotent_parameter_mismatch(token, "AppBundle"),
                    )
                    .into());
                }
                if let Some(bundle) = store.resolve_bundle(&spent.arn) {
                    return Ok(CreateAppBundleOutput {
                        app_bundle: bundle.to_app_bundle(),
                    });
                }
                // The replayed resource was deleted since; the token is spent on
                // nothing, so this call creates a new bundle under a new token.
                store.client_tokens.remove(&key);
            }
        }

        let id = crate::state::new_uuid();
        // A RESOURCE GUARD on the store, answered with the quota error this
        // operation models. Checked AFTER the idempotent replay above, so a retry
        // of a bundle that already exists is never refused by it.
        if store.app_bundles.len() >= crate::state::MAX_APP_BUNDLES {
            return Err(CreateAppBundleErrorModeled::ServiceQuotaExceededException(
                crate::state::quota_exceeded(
                    ::engine::emulator_account_id(),
                    "AppBundle",
                    "AppBundlesPerAccount",
                    crate::state::MAX_APP_BUNDLES,
                ),
            )
            .into());
        }
        let arn = crate::state::appfabric_arn(&region, &format!("appbundle/{id}"));
        let sequence = store.next_sequence();
        let record = crate::state::AppBundleRecord {
            id: id.clone(),
            arn: arn.clone(),
            customer_managed_key_arn,
            tags: input.tags.unwrap_or_default(),
            created_at: ::std::time::SystemTime::now(),
            sequence,
            ingestions: ::std::collections::HashMap::new(),
            app_authorizations: ::std::collections::HashMap::new(),
            user_access_tasks: ::std::collections::HashMap::new(),
        };
        store.app_bundles.insert(id.clone(), record);
        if let Some(token) = input.client_token.as_deref() {
            store.remember_client_token(
                crate::state::client_token_key(Self::NAME, token),
                arn,
                fingerprint,
            );
        }

        // Read the response back out of the store, so what the caller sees is
        // exactly what a later Get or List will see.
        let Some(stored) = store.app_bundles.get(&id) else {
            return Err(CreateAppBundleErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the created app bundle could not be read back".to_string(),
                    retry_after_seconds: None,
                },
            )
            .into());
        };
        let created = CreateAppBundleOutput {
            app_bundle: stored.to_app_bundle(),
        };
        crate::state::audit_resource_change(ctx.parts(), Self::NAME, "AppBundle", &id, "created");
        Ok(created)
    }
}
