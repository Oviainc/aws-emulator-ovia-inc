// SPDX-License-Identifier: CC0-1.0

//! @op CreateMicrovmAuthToken
//!
//! Mints the authentication token a client presents to reach a MicroVM's endpoint,
//! for the MicroVM the `microvmIdentifier` names. The token's digest, the ports it
//! grants and its expiry are recorded, so the MicroVM endpoint this emulator serves
//! (`crate::state::microvm_endpoint`) accepts exactly the tokens this operation
//! minted.
//!
//! HAND-AUTHORED, not generated: the scaffold op-stub generator declined this
//! operation (`ops/SKIPPED_STUBS.md`: "shape missing from model: smithy.api#Unit" —
//! `PortSpecification$allPorts` targets the Smithy prelude's `Unit`, which the
//! on-disk model does not carry as a shape), which left a MODELLED operation out of
//! the router altogether. The file is therefore written out in the shape every
//! generated sibling has — `//! @op` header, modelled error enum, error-channel
//! alias, `From` impl, operation struct, `impl Operation` — because `build.rs`
//! discovers operations by scanning `src/ops/*.rs` for that header, so the file IS
//! the registration. Its input and output types live in
//! `types/create_microvm_auth_token_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back.
//!
//! ---
//!
//! # Modeled contract for `CreateMicrovmAuthToken`
//!
//! From this service's Smithy model
//! (`smithy_query(query="closure",operation="CreateMicrovmAuthToken",model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`).
//!
//! Creates an authentication token for accessing a running MicroVM. The token grants
//! access to the specified ports on the MicroVM endpoint.
//!
//! `POST /2025-09-09/microvms/{microvmIdentifier}/auth-token`, success code 200.
//!
//! ## Input
//!
//! * `microvmIdentifier` (MicrovmIdentifier, len 1..256, `@httpLabel`) **required** —
//!   The ID of the MicroVM to create an authentication token for.
//! * `expirationInMinutes` (PositiveInteger, range 1..) **required** — The duration in
//!   minutes before the authentication token expires.
//! * `allowedPorts` (ListOfPortSpecification, len 1..) **required** — The ports on the
//!   MicroVM endpoint the token grants access to. Each entry is a `PortSpecification`
//!   union: `port`, `range` (`startPort`/`endPort`) or `allPorts`.
//!
//! ## Output
//!
//! * `authToken` (TokenParts, len 1..10) **required** — "A map containing the
//!   authentication token. Use the value at key \"X-aws-proxy-auth\" as the header
//!   value when connecting to the MicroVM endpoint."
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The request could not be completed due to a
//!   conflict with the current state of the resource.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request
//!   throttling.
//! * `ValidationException` (HTTP 400) — The input does not satisfy the constraints
//!   specified by the service.

#[allow(unused_imports)]
use super::types::*;

/// The `authToken` key the response member documents: "Use the value at key
/// \"X-aws-proxy-auth\" as the header value when connecting to the MicroVM endpoint."
/// The endpoint reads the token from that same header
/// (`crate::state::microvm_endpoint::AUTH_TOKEN_HEADER`).
const AUTH_TOKEN_KEY: &str = crate::state::microvm_endpoint::AUTH_TOKEN_HEADER;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.lambdamicrovms#CreateMicrovmAuthToken")]
#[derive(Debug)]
pub enum CreateMicrovmAuthTokenErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_microvm_auth_token::CreateMicrovmAuthTokenError`).
pub type CreateMicrovmAuthTokenError = crate::scaffolded::Scaffolded<CreateMicrovmAuthTokenErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateMicrovmAuthTokenErrorModeled> for CreateMicrovmAuthTokenError {
    fn from(modeled: CreateMicrovmAuthTokenErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateMicrovmAuthToken` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`].
pub struct CreateMicrovmAuthToken<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateMicrovmAuthToken<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateMicrovmAuthToken<Eng>
{
    type Input = CreateMicrovmAuthTokenInput;
    type Output = CreateMicrovmAuthTokenOutput;
    type Error = CreateMicrovmAuthTokenError;

    const NAME: &'static str = "CreateMicrovmAuthToken";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_MICROVM_AUTH_TOKEN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_MICROVM_AUTH_TOKEN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(unavailable("the MicroVM store is unavailable"));
        };

        // A `range` entry has to be a range: the model constrains each endpoint to
        // 1..65535 but cannot say that the start does not pass the end, so the refusal
        // names the member and the bound it broke.
        for spec in &input.allowed_ports {
            if let PortSpecification::Range(range) = spec {
                if range.start_port > range.end_port {
                    return Err(CreateMicrovmAuthTokenErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(format!(
                                "allowedPorts range startPort {} must not be greater than endPort {}",
                                range.start_port, range.end_port
                            )),
                        },
                    )
                    .into());
                }
            }
        }

        let mut store = state.lock();
        let now = ::std::time::Instant::now();
        store.materialize_due(now);

        let region = crate::state::request_region(ctx);
        let Some(vm) = store.microvm(&region, &input.microvm_identifier) else {
            // The not-found envelope this service puts on the wire: a request-derived
            // `message`, with the optional `resourceType`/`resourceId` unset.
            return Err(CreateMicrovmAuthTokenErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::microvm_not_found_message(&input.microvm_identifier),
                    resource_type: None,
                    resource_id: None,
                },
            )
            .into());
        };

        // "an authentication token for accessing a running MicroVM": a MicroVM on its
        // way out, or already gone, accepts no further connection, so there is nothing
        // for a token to grant access to — the modelled conflict with the resource's
        // current state.
        if !crate::state::is_connectable_microvm_state(&vm.state) {
            let message = format!(
                "MicroVM {} is {} and accepts no connection",
                vm.microvm_id, vm.state
            );
            return Err(CreateMicrovmAuthTokenErrorModeled::ConflictException(
                ConflictException {
                    message: Some(message),
                    resource_id: Some(vm.microvm_id.clone()),
                    resource_type: Some("Microvm".to_string()),
                },
            )
            .into());
        }
        // Nothing of the stored record is needed past this point, and the token is
        // minted outside the lock.
        let microvm_id = vm.microvm_id.clone();
        drop(store);

        // `expirationInMinutes` and `allowedPorts` scope what the token GRANTS at the
        // MicroVM endpoint; the response models the token alone, so they are recorded
        // with the token rather than reported, and the endpoint enforces them
        // (`crate::state::microvm_endpoint`).
        //
        // Nothing in a service constructs a secret: the emulator's credential store
        // mints it (see `crate::state::mint_microvm_auth_token`).
        let Some(token) = crate::state::mint_microvm_auth_token(ctx.credential_store()) else {
            return Err(unavailable(
                "no authentication token could be minted for the MicroVM",
            ));
        };

        // Only the DIGEST of the token is kept, so the caller holds the one copy of the
        // secret and the endpoint can still recognise it. The record carries the REGION
        // the mint was authorised in beside the `microvmId`, the same compound identity
        // the lookup above resolved through, so a token can never authenticate a MicroVM
        // of another region's partition.
        state.lock().record_auth_token(
            &region,
            &microvm_id,
            &token,
            input.expiration_in_minutes,
            input.allowed_ports,
            now,
        );

        let mut auth_token = ::std::collections::HashMap::new();
        auth_token.insert(AUTH_TOKEN_KEY.to_string(), token);
        Ok(CreateMicrovmAuthTokenOutput { auth_token })
    }
}

/// The modelled server error for work this handler could not complete.
fn unavailable(message: &str) -> CreateMicrovmAuthTokenError {
    CreateMicrovmAuthTokenErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.to_string()),
        retry_after_seconds: None,
    })
    .into()
}
