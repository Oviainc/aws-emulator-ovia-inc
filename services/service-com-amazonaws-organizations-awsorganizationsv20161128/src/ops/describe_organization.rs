// SPDX-License-Identifier: CC0-1.0

//! @op DescribeOrganization
//! Reports the persisted organization. This is the ONLY file to touch for this
//! operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`). The input, output, error enum and schemas a generated
//! stub would have carried are therefore written out below, in the same shape as a
//! rendered sibling (`create_organization.rs`); `build.rs` picks the operation up from
//! the `@op` line above and wires it into the service's routing table.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach
//! the store through [`crate::state::service_state`] and read it through the guard it
//! hands back.
//!
//! ---
//!
//! # Modeled contract for `DescribeOrganization`
//!
//! Retrieves information about the organization that the user's account belongs to.
//! This operation can be called from any account in the organization.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `Organization` (Organization) — A structure that contains information about the
//!   organization.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403)
//! * `AWSOrganizationsNotInUseException` (HTTP 404) — Your account isn't a member of an
//!   organization.
//! * `ConcurrentModificationException` (HTTP 409)
//! * `ServiceException` (HTTP 500)
//! * `TooManyRequestsException` (HTTP 429)
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-organizations-awsorganizationsv20161128/smithy-model/com-amazonaws-organizations-awsorganizationsv20161128.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-organizations-awsorganizationsv20161128.json");

/// `DescribeOrganization` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeOrganizationInput;

#[smithy_macros::smithy(shape = "com.amazonaws.organizations#DescribeOrganizationResponse")]
#[derive(Debug, Clone, Default)]
pub struct DescribeOrganizationOutput {
    pub organization: Option<Organization>,
}

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.organizations#DescribeOrganization")]
#[derive(Debug)]
pub enum DescribeOrganizationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    AWSOrganizationsNotInUseException(AWSOrganizationsNotInUseException),
    ConcurrentModificationException(ConcurrentModificationException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_organization::DescribeOrganizationError`).
pub type DescribeOrganizationError = crate::scaffolded::Scaffolded<DescribeOrganizationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeOrganizationErrorModeled> for DescribeOrganizationError {
    fn from(modeled: DescribeOrganizationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeOrganization` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DescribeOrganization<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeOrganization<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeOrganization<Eng>
{
    type Input = DescribeOrganizationInput;
    type Output = DescribeOrganizationOutput;
    type Error = DescribeOrganizationError;

    const NAME: &'static str = "DescribeOrganization";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DESCRIBE_ORGANIZATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DESCRIBE_ORGANIZATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = (&input, ctx);
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;

        // A pure read: it reports the stored organization and writes nothing.
        let store = state.lock();
        let organization = store.organization.as_ref().ok_or_else(|| {
            DescribeOrganizationError::from(
                DescribeOrganizationErrorModeled::AWSOrganizationsNotInUseException(
                    AWSOrganizationsNotInUseException {
                        message: Some(crate::state::NOT_IN_USE_MESSAGE.to_string()),
                    },
                ),
            )
        })?;
        Ok(DescribeOrganizationOutput {
            organization: Some(organization.to_modelled()),
        })
    }
}
