// SPDX-License-Identifier: CC0-1.0

//! @op RegisterPublisher
//! Registers this ACCOUNT as a publisher of public extensions, and answers the publisher
//! id the registration is addressed by ([`crate::state::publisher`]).
//!
//! ## What the operation does
//!
//! "Registers your account as a publisher of public extensions in the CloudFormation
//! registry... This publisher ID applies to your account in all Amazon Web Services
//! Regions." So the call WRITES one account-wide record — the spec says this call
//! registers, so minting the id it hands back is what it is for — and the id is minted
//! once: the operation is `@idempotent`, so a second registration re-verifies the account
//! and answers the id it already has rather than a second one.
//!
//! Two things reach the record, and only one of them is stored. The caller's acceptance of
//! the terms and conditions is a condition of registering at all — "you must accept the
//! terms and conditions in order to register to publish public extensions to the
//! CloudFormation registry. The default is false" — so a request that does not accept them
//! is refused with the registry's own modelled `CFNRegistryException` (400) and writes
//! nothing. The `ConnectionArn` is the identity the publisher was verified by, and that is
//! what the record keeps; `DescribePublisher` reports the identity provider computed from
//! it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only, and
//! this operation needs none of it: the registration is the ACCOUNT's, so the Region the
//! request was signed for neither selects nor scopes it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterPublisher`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Registers your account as a publisher of public extensions in the CloudFormation
//! registry. Public extensions are available for use by all CloudFormation users. This
//! publisher ID applies to your account in all Amazon Web Services Regions. For information
//! about requirements for registering as a public extension publisher, see Prerequisite:
//! Registering your account to publish CloudFormation extensions in the CloudFormation
//! Command Line Interface (CLI) User Guide.
//!
//! ## Input
//!
//! * `AcceptTermsAndConditions` (AcceptTermsAndConditions) — Whether you accept the Terms and
//!   Conditions for publishing extensions in the CloudFormation registry. You must accept the
//!   terms and conditions in order to register to publish public extensions to the
//!   CloudFormation registry. The default is false.
//! * `ConnectionArn` (ConnectionArn, len 1..256, pattern
//!   ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$) — If you are using a Bitbucket or GitHub account
//!   for identity verification, the Amazon Resource Name (ARN) for your connection to that
//!   account. For more information, see Prerequisite: Registering your account to publish
//!   CloudFormation extensions in the CloudFormation Command Line Interface (CLI) User Guide.
//!
//! ## Output
//!
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The ID assigned
//!   this account by CloudFormation for publishing extensions.
//!
//! ## Errors
//!
//! * `CFNRegistryException` (HTTP 400) — An error occurred during a CloudFormation registry
//!   operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterPublisherError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterPublisherError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#RegisterPublisher")]
#[derive(Debug)]
pub enum RegisterPublisherErrorModeled {
    CFNRegistryException(CFNRegistryException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_publisher::RegisterPublisherError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterPublisherError = crate::scaffolded::Scaffolded<RegisterPublisherErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterPublisherErrorModeled> for RegisterPublisherError {
    fn from(modeled: RegisterPublisherErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterPublisher` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterPublisher<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterPublisher<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterPublisher<Eng>
{
    type Input = RegisterPublisherInput;
    type Output = RegisterPublisherOutput;
    type Error = RegisterPublisherError;

    const NAME: &'static str = "RegisterPublisher";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_PUBLISHER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_PUBLISHER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The registration is this account's, in every Region at once: nothing here is
        // selected or scoped by the Region the request was signed for.
        let _ = ctx;

        // "You must accept the terms and conditions in order to register to publish public
        // extensions to the CloudFormation registry. The default is false." A request that
        // does not accept them is refused, and writes nothing.
        if !input.accept_terms_and_conditions.unwrap_or(false) {
            return Err(registry_exception(
                "AcceptTermsAndConditions must be true: the terms and conditions for \
                 publishing extensions in the CloudFormation registry have to be accepted \
                 to register as a publisher of public extensions",
            ));
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no store to register in, so this
            // answers 501 rather than a publisher id nothing stands behind.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "RegisterPublisher",
            ));
        };
        // No `settle` call: a publisher registration carries no status the clock owes, and
        // this operation neither reads nor writes a stack.
        let publisher_id = state.lock().register_publisher(input.connection_arn);

        Ok(RegisterPublisherOutput {
            publisher_id: Some(publisher_id),
        })
    }
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: &str) -> RegisterPublisherError {
    RegisterPublisherErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message.to_string()),
    })
    .into()
}
