// SPDX-License-Identifier: CC0-1.0

//! @op CreateContainerServiceRegistryLogin
//! Creates ONE temporary set of log in credentials for the container image
//! registry of this account, and answers it together with the address those
//! credentials log in at.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## What one call creates
//!
//! * the `username` and the `password` are MINTED BY THE EMULATOR'S CREDENTIAL
//!   STORE, the one component that generates credential material: the username
//!   is the id it minted and the password the secret it minted, never a value
//!   this handler made up. They log a client in to a container image registry
//!   and are NOT a SigV4 credential of this emulator, so the credential store
//!   forgets them again at once (as `GetAuthorizationToken` does in the ECR
//!   emulator) and what this service keeps is the digest a presented password is
//!   checked against — never a readable copy of the password;
//! * `expiresAt` is 12 hours after the call, the validity the modelled
//!   `expiresAt` member documents (`crate::state::REGISTRY_LOGIN_LIFETIME_MS`),
//!   and the clock forgets the set at that instant;
//! * `registry` is the address the credentials log in at, built from the
//!   endpoint THIS request reached the emulator on
//!   (`crate::state::container_image_registry_url`) — a field that promises "the
//!   address to use to push container images" has to name something the caller
//!   can reach, and this emulator owns no DNS zone. A request that names no host
//!   names no endpoint to announce, and then the field is absent rather than a
//!   literal nothing answers on.
//!
//! The set is recorded in this service's store
//! (`ComAmazonawsLightsailLightsail20161128State::registry_logins`), so a client
//! that logs in at the announced address with the credentials it was handed is
//! recognised there — and one that presents anything else is not. This emulator
//! serves no container image registry protocol behind that address, which is why
//! an authenticated request to it answers 501 (see `crate::service`) and why
//! `RegisterContainerImage` finds no pushed digest to register.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateContainerServiceRegistryLogin::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::with_state`], which applies every clock-owed transition — here,
//! forgetting the sets of credentials that have expired — before the body runs.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`,
//! plus the credential store this operation mints from. The engine is NOT reached
//! through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateContainerServiceRegistryLogin`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json")
//! ```
//!
//! Creates a temporary set of log in credentials that you can use to log in to the Docker
//! process on your local machine. After you're logged in, you can use the native Docker
//! commands to push your local container images to the container image registry of your
//! Amazon Lightsail account so that you can use them with your Lightsail container service.
//! The log in credentials expire 12 hours after they are created, at which point you will
//! need to create a new set of log in credentials. You can only push container images to
//! the container service registry of your Lightsail account. You cannot pull container
//! images or perform any other container image management actions on the container service
//! registry. After you push your container images to the container image registry of your
//! Lightsail account, use the RegisterContainerImage action to register the pushed images
//! to a specific Lightsail container service. This action is not required if you install
//! and use the Lightsail Control (lightsailctl) plugin to push container images to your
//! Lightsail container service. For more information, see Pushing and managing container
//! images on your Amazon Lightsail container services in the Amazon Lightsail Developer
//! Guide.
//!
//! ## Output
//!
//! * `registryLogin` (ContainerServiceRegistryLogin) — An object that describes the log in
//!   information for the container service registry of your Lightsail account.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Lightsail throws this exception when the user
//!   cannot be authenticated or uses invalid credentials to access a resource.
//! * `InvalidInputException` (HTTP 400) — Lightsail throws this exception when user input
//!   does not conform to the validation rules of an input field. Domain and distribution APIs
//!   are only available in the N. Virginia (us-east-1) Amazon Web Services Region. Please set
//!   your Amazon Web Services Region configuration to us-east-1 to create, view, or edit
//!   these resources.
//! * `NotFoundException` (HTTP 404) — Lightsail throws this exception when it cannot find a
//!   resource.
//! * `RegionSetupInProgressException` (HTTP 428) — Lightsail throws this exception when an
//!   operation is performed on resources in an opt-in Region that is currently being set up.
//! * `ServiceException` (HTTP 500) — A general service exception.
//! * `UnauthenticatedException` (HTTP 401) — Lightsail throws this exception when the user
//!   has not been authenticated.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateContainerServiceRegistryLoginError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lightsail-lightsail-20161128.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateContainerServiceRegistryLoginError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lightsail#CreateContainerServiceRegistryLogin")]
#[derive(Debug)]
pub enum CreateContainerServiceRegistryLoginErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidInputException(InvalidInputException),
    NotFoundException(NotFoundException),
    RegionSetupInProgressException(RegionSetupInProgressException),
    ServiceException(ServiceException),
    UnauthenticatedException(UnauthenticatedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_container_service_registry_login::CreateContainerServiceRegistryLoginError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateContainerServiceRegistryLoginError = crate::scaffolded::Scaffolded<CreateContainerServiceRegistryLoginErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateContainerServiceRegistryLoginErrorModeled> for CreateContainerServiceRegistryLoginError {
    fn from(modeled: CreateContainerServiceRegistryLoginErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateContainerServiceRegistryLogin` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateContainerServiceRegistryLogin<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateContainerServiceRegistryLogin<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateContainerServiceRegistryLogin<Eng>
{
    type Input = CreateContainerServiceRegistryLoginInput;
    type Output = CreateContainerServiceRegistryLoginOutput;
    type Error = CreateContainerServiceRegistryLoginError;

    const NAME: &'static str = "CreateContainerServiceRegistryLogin";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CONTAINER_SERVICE_REGISTRY_LOGIN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CONTAINER_SERVICE_REGISTRY_LOGIN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use ::emulator_server::aws_credentials::{AWS_SESSION, SESSION_TOKEN};

        // The modelled request carries no member at all, so there is nothing to
        // read off it: what the credentials are created for is the account the
        // request is signed by, and where they are presented is the region it is
        // signed for.
        let CreateContainerServiceRegistryLoginInput {} = input;
        let region = crate::state::request_region(ctx.parts());
        let now = crate::state::now_ms();

        // The credential material comes from the emulator's credential store,
        // the only component that generates ids and secrets. It is minted BEFORE
        // this service's store is locked, so no other component's lock is taken
        // while that guard is held.
        let credentials = ctx.credential_store();
        let minted = credentials.create(&AWS_SESSION);
        let password = minted.secret(SESSION_TOKEN).map(str::to_string);

        // These credentials log a client in to a container image registry; they
        // are NOT a SigV4 credential of this emulator, so the emulator must not
        // go on accepting them as one and the store forgets them again at once.
        // A set it could not forget would stay signable, which is not what this
        // operation promises, so the request is refused rather than answered
        // with it.
        if let Err(error) = credentials.delete(&minted.id) {
            ::tracing::warn!(
                operation = Self::NAME,
                username = %minted.id,
                reason = ?error,
                "a credential minted for container image registry log in credentials could \
                 not be withdrawn from the credential store"
            );
            return Err(service_exception(
                "the container image registry log in credentials could not be created",
            ));
        }
        // A set without its password cannot be logged in with, and no later call
        // can supply it: the password lives in this response alone.
        let Some(password) = password else {
            return Err(service_exception(
                "the container image registry log in credentials were minted without a password",
            ));
        };

        // What the store keeps is the username and the digest a presented
        // password is checked against, so the announced address can recognise
        // this set until it expires — and nothing can read the password back out
        // of the store.
        //
        // And nothing else: no entry is appended to the service's operation log.
        // An entry carries a `resourceType` from the modelled `ResourceType`
        // enum and the `resourceName` of the resource it acted on, and a set of
        // log in credentials is neither — it has no name and the enum has no
        // member for it, so an entry for it could only name a resource this call
        // did not touch. The modelled output carries no `operations` member
        // either, which is what sets this apart from `CreateBucketAccessKey`:
        // that call acts on a named `Bucket` and ANSWERS the log entry it wrote.
        let login = crate::state::StoredRegistryLogin::new(minted.id.clone(), &password, now);
        let expires_at_ms = login.expires_at_ms;
        crate::state::with_state(&*self.engine, |store| {
            store.insert_registry_login(&region, login);
        })
        .ok_or_else(unavailable)?;

        // Where the credentials are presented: the registry of this region,
        // beneath the endpoint THIS request reached the emulator on. A request
        // that names no host names no endpoint to announce, and an address
        // nothing answers on is worse than none — the credentials it was handed
        // are valid either way.
        let endpoint = crate::state::request_endpoint(ctx.parts());
        let registry = endpoint
            .as_deref()
            .map(|endpoint| crate::state::container_image_registry_url(endpoint, &region));

        // Audit record for a credential-creating call: who asked (the access key
        // id the request presented and the endpoint it arrived at), the region,
        // the server-minted username and when the set expires — NEVER the
        // password, which only the caller gets.
        ::tracing::info!(
            audit = true,
            operation = Self::NAME,
            outcome = "created",
            region = %region,
            presented_access_key_id = %::engine::extract_akid(&ctx.parts().headers),
            source_endpoint = endpoint.as_deref().unwrap_or("<none>"),
            username = %minted.id,
            expires_at_epoch_millis = expires_at_ms,
            "container image registry log in credentials created"
        );

        Ok(CreateContainerServiceRegistryLoginOutput {
            registry_login: Some(ContainerServiceRegistryLogin {
                username: Some(minted.id),
                password: Some(password),
                expires_at: Some(crate::state::timestamp(expires_at_ms)),
                registry,
            }),
        })
    }
}

/// The modelled server fault for a set of credentials this emulator could not
/// create. `message` says what could not be done, and carries nothing from the
/// request.
fn service_exception(message: &str) -> CreateContainerServiceRegistryLoginError {
    CreateContainerServiceRegistryLoginErrorModeled::ServiceException(ServiceException {
        message: Some(message.to_string()),
        ..Default::default()
    })
    .into()
}

/// The store is unreachable — the engine is not the shared in-memory one. A
/// modelled server fault, never a fresh engine.
fn unavailable() -> CreateContainerServiceRegistryLoginError {
    service_exception("the Lightsail container image registry store is unavailable")
}
