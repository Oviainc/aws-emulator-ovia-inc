// SPDX-License-Identifier: CC0-1.0

//! @op DescribeTerminationPolicyTypes
//!
//! Reports the catalog of termination policy TYPE NAMES a group's
//! `TerminationPolicies` may name, which is what a client validates a
//! `CreateAutoScalingGroup`/`UpdateAutoScalingGroup` argument against.
//!
//! The catalog is SERVICE data rather than per-account state — the same answer for
//! every caller, in every region, whatever the store holds — so this handler takes
//! no lock and writes nothing. Its membership and ordering live in
//! [`crate::state::TERMINATION_POLICY_TYPES`], which names the model member that
//! publishes them; it is the same list those operations validate against, plus
//! [`crate::state::TERMINATION_POLICY_TYPE_LAMBDA`] for the custom-policy form,
//! which a group names by a Lambda function ARN rather than by the type name.
//!
//! HAND-AUTHORED. The op-stub generator declined to emit a stub for this operation
//! because the model binds it to Smithy's `Unit` input, which it cannot type yet
//! (see `src/ops/SKIPPED_STUBS.md`): it had no handler file, so it was absent from
//! the generated `operations(...)` allow-list and every well-formed call fell
//! through to the framework's unroutable-operation path — an HTTP 400 that is not
//! one of this operation's modelled outcomes. `build.rs` derives that allow-list
//! from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what
//! wires it. Its input and output shapes live in
//! `types/describe_termination_policy_types_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]; never construct an
//! engine in a handler. This operation needs no store, so it touches neither.
//!
//! ---
//!
//! # Modeled contract for `DescribeTerminationPolicyTypes`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DescribeTerminationPolicyTypes",model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`):
//!
//! Describes the termination policies supported by Amazon EC2 Auto Scaling.
//!
//! ## Input
//!
//! (no members — `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `TerminationPolicyTypes` (TerminationPolicies) — The termination policies
//!   supported by Amazon EC2 Auto Scaling.
//!
//! ## Errors
//!
//! * `ResourceContentionFault` (HTTP 500) — You already have a pending update to an
//!   Amazon EC2 Auto Scaling resource (for example, an Auto Scaling group, instance,
//!   or load balancer).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// The operation's MODELED errors. `#[operation_error]` accepts only variants the
/// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.autoscaling#DescribeTerminationPolicyTypes")]
#[derive(Debug)]
pub enum DescribeTerminationPolicyTypesErrorModeled {
    ResourceContentionFault(ResourceContentionFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_termination_policy_types::DescribeTerminationPolicyTypesError`).
pub type DescribeTerminationPolicyTypesError =
    crate::scaffolded::Scaffolded<DescribeTerminationPolicyTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeTerminationPolicyTypesErrorModeled> for DescribeTerminationPolicyTypesError {
    fn from(modeled: DescribeTerminationPolicyTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeTerminationPolicyTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct DescribeTerminationPolicyTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeTerminationPolicyTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeTerminationPolicyTypes<Eng>
{
    type Input = DescribeTerminationPolicyTypesInput;
    type Output = DescribeTerminationPolicyTypesOutput;
    type Error = DescribeTerminationPolicyTypesError;

    const NAME: &'static str = "DescribeTerminationPolicyTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_TERMINATION_POLICY_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_TERMINATION_POLICY_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it, and
        // the catalog is the same for every caller: no request metadata and no store
        // entry takes part in the answer.
        let _ = (&input, ctx, &self.engine);

        Ok(DescribeTerminationPolicyTypesOutput {
            termination_policy_types: Some(
                crate::state::TERMINATION_POLICY_TYPES
                    .iter()
                    .map(|policy| (*policy).to_string())
                    .collect(),
            ),
        })
    }
}
