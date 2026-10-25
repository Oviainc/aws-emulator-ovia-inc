// SPDX-License-Identifier: CC0-1.0

//! @op DescribePublisher
//! Reports the extension-publisher registration this store holds
//! ([`crate::state::publisher`]), which `RegisterPublisher` is what writes.
//!
//! ## What the operation reports
//!
//! "Returns information about a CloudFormation extension publisher. If you don't supply a
//! PublisherId, and you have registered as an extension publisher, DescribePublisher
//! returns information about your own publisher account." So a request either names the
//! publisher it wants or means this account's own, and the answer comes off the stored
//! registration: its id, its status — "currently, all registered publishers are verified"
//! — and the identity provider the registration was verified by.
//!
//! A registration the store does not hold is answered with the registry's own modelled
//! `CFNRegistryException` (400) rather than a publisher nothing stands behind: an account
//! that has not registered has no record, and a `PublisherId` that is not the one it
//! registered under names none either. Nothing is written: this is a reporting operation.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only, and
//! this operation needs none of it: a publisher id "applies to your account in all Amazon
//! Web Services Regions", so the Region the request was signed for neither selects nor
//! scopes the record it reads.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribePublisher`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Returns information about a CloudFormation extension publisher. If you don't supply a
//! PublisherId, and you have registered as an extension publisher, DescribePublisher
//! returns information about your own publisher account. For more information about
//! registering as a publisher, see: RegisterPublisher Publishing extensions to make them
//! available for public use in the CloudFormation Command Line Interface (CLI) User Guide
//!
//! ## Input
//!
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The ID of the
//!   extension publisher. If you don't supply a PublisherId, and you have registered as an
//!   extension publisher, DescribePublisher returns information about your own publisher
//!   account.
//!
//! ## Output
//!
//! * `IdentityProvider` (IdentityProvider, one of: AWS_Marketplace | Bitbucket | GitHub) —
//!   The type of account used as the identity provider when registering this publisher with
//!   CloudFormation.
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The ID of the
//!   extension publisher.
//! * `PublisherProfile` (PublisherProfile, len 0..1024, pattern ^(http:|https:)+[^\s]+[\w]$)
//!   — The URL to the publisher's profile with the identity provider.
//! * `PublisherStatus` (PublisherStatus, one of: UNVERIFIED | VERIFIED) — Whether the
//!   publisher is verified. Currently, all registered publishers are verified.
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
// `DescribePublisherError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribePublisherError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#DescribePublisher")]
#[derive(Debug)]
pub enum DescribePublisherErrorModeled {
    CFNRegistryException(CFNRegistryException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_publisher::DescribePublisherError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribePublisherError = crate::scaffolded::Scaffolded<DescribePublisherErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribePublisherErrorModeled> for DescribePublisherError {
    fn from(modeled: DescribePublisherErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribePublisher` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribePublisher<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribePublisher<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribePublisher<Eng>
{
    type Input = DescribePublisherInput;
    type Output = DescribePublisherOutput;
    type Error = DescribePublisherError;

    const NAME: &'static str = "DescribePublisher";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PUBLISHER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PUBLISHER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A publisher id "applies to your account in all Amazon Web Services Regions", so
        // the Region the request was signed for selects nothing here.
        let _ = ctx;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no store holding a registration, so
            // this answers 501 rather than a truthful-looking "no such publisher".
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "DescribePublisher",
            ));
        };
        // A pure report: nothing here writes, and there is no `settle` call either, because
        // a publisher registration carries no status the clock owes.
        let store = state.lock();
        let wanted = input.publisher_id.as_deref();
        let registered = store
            .publisher(wanted)
            .ok_or_else(|| publisher_not_found(wanted))?;

        Ok(DescribePublisherOutput {
            // "The ID of the extension publisher", off the record rather than off the
            // request: a request that named none is answered with this account's own id.
            publisher_id: Some(registered.publisher_id.clone()),
            // "Whether the publisher is verified."
            publisher_status: Some(registered.status.clone()),
            // "The type of account used as the identity provider when registering this
            // publisher with CloudFormation", which the registration determines (see
            // [`crate::state::publisher::StoredPublisher::identity_provider`]).
            identity_provider: registered.identity_provider().map(str::to_string),
            // "The URL to the publisher's profile with the identity provider": a page held
            // BY that provider, which a registration does not carry, so nothing invents one
            // here.
            publisher_profile: None,
        })
    }
}

/// The registry's own modelled refusal, for a publisher this store holds no registration
/// for: HTTP 400 `CFNRegistryException`, "an error occurred during a CloudFormation
/// registry operation".
fn publisher_not_found(publisher_id: Option<&str>) -> DescribePublisherError {
    let message = match publisher_id {
        Some(publisher_id) => format!(
            "Publisher {publisher_id} is not registered as a CloudFormation extension \
             publisher"
        ),
        None => "This account is not registered as a CloudFormation extension publisher: \
                 register it with RegisterPublisher, or name a PublisherId"
            .to_string(),
    };
    DescribePublisherErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}
