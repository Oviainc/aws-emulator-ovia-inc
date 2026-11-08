// SPDX-License-Identifier: CC0-1.0

//! @op DescribeDomain
//!
//! A pure read: it reports the ONE stored domain record named by `name` — its info
//! (including the ARN minted at registration) and its retention configuration — and
//! writes nothing. It is the reader that `RegisterDomain` round-trips through.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeDomain`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Returns information about the specified domain, including description and status. Access
//! Control You can use IAM policies to control this action's access to Amazon SWF resources
//! as follows: Use a Resource element with the domain name to limit the action to only
//! specified domains. Use an Action element to allow or deny permission to call this
//! action. You cannot use an IAM policy to constrain this action's parameters. If the
//! caller doesn't have sufficient permissions to invoke the action, or the parameter values
//! fall outside the specified constraints, the action fails. The associated event
//! attribute's cause parameter is set to OPERATION_NOT_PERMITTED. For details and example
//! IAM policies, see Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF
//! Developer Guide.
//!
//! ## Input
//!
//! * `name` (DomainName, len 1..256) **required** — The name of the domain to describe.
//!
//! ## Output
//!
//! * `configuration` (DomainConfiguration) **required** — The domain configuration.
//!   Currently, this includes only the domain's retention period.
//! * `domainInfo` (DomainInfo) **required** — The basic information about a domain, such as
//!   its name, status, and description.
//!
//! ## Errors
//!
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `UnknownResourceFault` (HTTP 400) — Returned when the named resource cannot be found
//!   with in the scope of this operation (region or domain). This could happen if the named
//!   resource was never created or is no longer available for this operation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeDomainError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeDomainError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#DescribeDomain")]
#[derive(Debug)]
pub enum DescribeDomainErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_domain::DescribeDomainError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeDomainError = crate::scaffolded::Scaffolded<DescribeDomainErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeDomainErrorModeled> for DescribeDomainError {
    fn from(modeled: DescribeDomainErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeDomain` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeDomain<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeDomain<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeDomain<Eng>
{
    type Input = DescribeDomainInput;
    type Output = DescribeDomainOutput;
    type Error = DescribeDomainError;

    const NAME: &'static str = "DescribeDomain";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_DOMAIN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_DOMAIN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeDomainErrorModeled::OperationNotPermittedFault(OperationNotPermittedFault {
                message: Some("the SWF domain store is unavailable".to_string()),
            })
        })?;

        let guard = state.lock();
        let Some(domain) = guard.domains.get(&input.name) else {
            return Err(DescribeDomainErrorModeled::UnknownResourceFault(
                UnknownResourceFault {
                    message: Some(crate::state::unknown_domain_message(&input.name)),
                },
            )
            .into());
        };

        Ok(DescribeDomainOutput {
            domain_info: domain.info(),
            configuration: domain.configuration(),
        })
    }
}
