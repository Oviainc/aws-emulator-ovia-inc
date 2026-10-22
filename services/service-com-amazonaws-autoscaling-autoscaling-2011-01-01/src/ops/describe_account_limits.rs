// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAccountLimits
//!
//! Reports this account's Amazon EC2 Auto Scaling quotas and how much of them is
//! used: the maxima this service enforces on Auto Scaling groups and launch
//! configurations, beside the number of each the store currently holds. A
//! REPORTER: the only state it writes is the transitions the clock already made
//! due, through [`crate::state::AutoScalingState::materialize_due`].
//!
//! The maxima are the quotas the CREATE operations enforce
//! ([`crate::state::MAX_GROUPS`] and
//! [`crate::state::MAX_LAUNCH_CONFIGURATIONS`], which `CreateAutoScalingGroup`
//! and `CreateLaunchConfiguration` answer `LimitExceededFault` past), so what
//! this operation reports and what the service lets a caller create are one
//! number. The counts are computed from the store, never remembered separately,
//! so creating or deleting either resource moves them with no bookkeeping.
//!
//! HAND-AUTHORED. The op-stub generator declined to emit a stub for this
//! operation because the model binds it to Smithy's `Unit` input, which it cannot
//! type yet (see `src/ops/SKIPPED_STUBS.md`): it had no handler file, so it was
//! absent from the generated `operations(...)` allow-list and every well-formed
//! call fell through to the framework's unroutable-operation path — an HTTP 400
//! that is not one of this operation's modelled outcomes. `build.rs` derives that
//! allow-list from the `//! @op <Name>` header of every `src/ops/*.rs`, so this
//! file is what wires it. Its input and output shapes live in
//! `types/describe_account_limits_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]; never construct an
//! engine in a handler. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DescribeAccountLimits`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DescribeAccountLimits",model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`):
//!
//! Describes the current Amazon EC2 Auto Scaling resource quotas for your account.
//! When you create an Auto Scaling group and when you create a launch
//! configuration, Amazon EC2 Auto Scaling checks that your request does not exceed
//! the quotas.
//!
//! ## Input
//!
//! (no members — `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `MaxNumberOfAutoScalingGroups` (integer) — The maximum number of groups allowed
//!   for your account.
//! * `MaxNumberOfLaunchConfigurations` (integer) — The maximum number of launch
//!   configurations allowed for your account.
//! * `NumberOfAutoScalingGroups` (integer) — The current number of groups for your
//!   account.
//! * `NumberOfLaunchConfigurations` (integer) — The current number of launch
//!   configurations for your account.
//!
//! The model declares no `ThrottlingLimits` member on
//! `DescribeAccountLimitsAnswer`, so this operation reports none: a member the
//! model does not carry has no place on the wire.
//!
//! ## Errors
//!
//! * `ResourceContentionFault` (HTTP 500) — You already have a pending update to an
//!   Amazon EC2 Auto Scaling resource (for example, an Auto Scaling group, instance,
//!   or load balancer).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAccountLimitsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// The operation's MODELED errors. `#[operation_error]` accepts only variants the
/// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.autoscaling#DescribeAccountLimits")]
#[derive(Debug)]
pub enum DescribeAccountLimitsErrorModeled {
    ResourceContentionFault(ResourceContentionFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_account_limits::DescribeAccountLimitsError`).
pub type DescribeAccountLimitsError =
    crate::scaffolded::Scaffolded<DescribeAccountLimitsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeAccountLimitsErrorModeled> for DescribeAccountLimitsError {
    fn from(modeled: DescribeAccountLimitsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAccountLimits` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct DescribeAccountLimits<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAccountLimits<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAccountLimits<Eng>
{
    type Input = DescribeAccountLimitsInput;
    type Output = DescribeAccountLimitsOutput;
    type Error = DescribeAccountLimitsError;

    const NAME: &'static str = "DescribeAccountLimits";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_ACCOUNT_LIMITS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_ACCOUNT_LIMITS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input has no members and `ctx` carries request metadata only; the
        // store is reached through `self.engine`.
        let _ = (&input, ctx);

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribeAccountLimitsErrorModeled::ResourceContentionFault(
                ResourceContentionFault {
                    message: Some(
                        "the Auto Scaling store is not mounted on this engine".to_string(),
                    ),
                },
            )
            .into());
        };
        let mut state = state.lock();
        state.materialize_due(::std::time::SystemTime::now());

        // `i32` is the modelled width of every member here, and both counts are
        // bounded by the maxima beside them, so neither conversion can be lossy.
        Ok(DescribeAccountLimitsOutput {
            max_number_of_auto_scaling_groups: Some(crate::state::MAX_GROUPS as i32),
            max_number_of_launch_configurations: Some(
                crate::state::MAX_LAUNCH_CONFIGURATIONS as i32,
            ),
            number_of_auto_scaling_groups: Some(state.groups.len() as i32),
            number_of_launch_configurations: Some(state.launch_configurations.len() as i32),
        })
    }
}
