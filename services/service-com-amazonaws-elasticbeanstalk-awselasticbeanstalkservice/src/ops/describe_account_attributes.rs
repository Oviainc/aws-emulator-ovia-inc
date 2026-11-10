// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAccountAttributes
//! Implemented: reports the per-resource Elastic Beanstalk quotas this account is
//! held to — the same bounds the create operations enforce. This is the ONLY file
//! to touch for this operation.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DescribeAccountAttributes::new(engine)` and drives it through the centralized request
//! walk: deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAccountAttributes`
//!
//! What AWS documents for this operation, from this service's Smithy model. For the
//! FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json")
//! ```
//!
//! Returns attributes related to Elastic Beanstalk that are associated with the calling
//! Amazon Web Services account. The result currently has one set of attributes — resource
//! quotas.
//!
//! ## Input
//!
//! None: the operation's modelled input is `smithy.api#Unit`.
//!
//! ## Output
//!
//! * `ResourceQuotas` (ResourceQuotas) — The Elastic Beanstalk resource quotas associated
//!   with the calling Amazon Web Services account.
//!
//! ## Errors
//!
//! * `InsufficientPrivilegesException` (HTTP 403) — The specified account does not have
//!   sufficient privileges for one or more Amazon Web Services services.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAccountAttributesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.elasticbeanstalk#DescribeAccountAttributes")]
#[derive(Debug)]
pub enum DescribeAccountAttributesErrorModeled {
    InsufficientPrivilegesException(InsufficientPrivilegesException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_account_attributes::DescribeAccountAttributesError`).
pub type DescribeAccountAttributesError =
    crate::scaffolded::Scaffolded<DescribeAccountAttributesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper.
impl From<DescribeAccountAttributesErrorModeled> for DescribeAccountAttributesError {
    fn from(modeled: DescribeAccountAttributesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The quota value the modelled `ResourceQuota$Maximum` (`BoxedInt`) carries for a
/// bound this service holds as a `usize`.
///
/// Every bound this service enforces is far below `i32::MAX`, so the conversion is
/// exact; `try_into` rather than `as` so a bound that ever grew past the modelled
/// type would be reported as absent instead of silently wrapping to a negative
/// maximum.
fn quota(maximum: usize) -> ResourceQuota {
    ResourceQuota {
        maximum: i32::try_from(maximum).ok(),
    }
}

/// The `DescribeAccountAttributes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAccountAttributes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAccountAttributes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAccountAttributes<Eng>
{
    type Input = DescribeAccountAttributesInput;
    type Output = DescribeAccountAttributesOutput;
    type Error = DescribeAccountAttributesError;

    const NAME: &'static str = "DescribeAccountAttributes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_ACCOUNT_ATTRIBUTES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_ACCOUNT_ATTRIBUTES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input, and reporting the account's quotas reads
        // nothing out of the store and changes nothing in it: the quotas are the
        // service's own — the very bounds the creates refuse a create past
        // (`CreateApplication`, `CreateApplicationVersion`, `CreateEnvironment`,
        // `CreateConfigurationTemplate` and `CreatePlatformVersion`).
        let _ = (&input, ctx, &self.engine);

        Ok(DescribeAccountAttributesOutput {
            resource_quotas: Some(ResourceQuotas {
                application_quota: Some(quota(crate::state::MAX_APPLICATIONS)),
                application_version_quota: Some(quota(crate::state::MAX_APPLICATION_VERSIONS)),
                environment_quota: Some(quota(crate::state::MAX_ENVIRONMENTS)),
                configuration_template_quota: Some(quota(
                    crate::state::MAX_CONFIGURATION_TEMPLATES,
                )),
                custom_platform_quota: Some(quota(crate::state::MAX_PLATFORM_VERSIONS)),
                // RECORDED BEHAVIOUR, not modelled: the wire `ResourceQuotas`
                // carries a sixth quota, for health groups, with its own
                // maximum. A health group is the unit enhanced health reports
                // an ENVIRONMENT's instances under — one per environment it is
                // turned on for — so the number of them this account can hold
                // is the number of environments it can hold, rather than a
                // second bound invented beside it.
                health_group_quota: Some(quota(crate::state::MAX_ENVIRONMENTS)),
            }),
        })
    }
}
