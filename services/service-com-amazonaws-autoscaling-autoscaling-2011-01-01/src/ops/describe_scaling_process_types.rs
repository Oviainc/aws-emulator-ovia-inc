// SPDX-License-Identifier: CC0-1.0

//! @op DescribeScalingProcessTypes
//!
//! Reports the catalog of scaling processes a group can suspend and resume: one
//! `ProcessType` per process name the service recognises, which is what a client
//! validates a `SuspendProcesses`/`ResumeProcesses` argument against.
//!
//! The catalog is SERVICE data rather than per-account state — the same answer for
//! every caller, in every region, whatever the store holds — so this handler takes
//! no lock and writes nothing. Its membership and ordering live in
//! [`crate::state::SCALING_PROCESS_TYPES`], which names the model member that
//! publishes them.
//!
//! HAND-AUTHORED. The op-stub generator declined to emit a stub for this operation
//! because the model binds it to Smithy's `Unit` input, which it cannot type yet
//! (see `src/ops/SKIPPED_STUBS.md`): it had no handler file, so it was absent from
//! the generated `operations(...)` allow-list and every well-formed call fell
//! through to the framework's unroutable-operation path — an HTTP 400 that is not
//! one of this operation's modelled outcomes. `build.rs` derives that allow-list
//! from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what
//! wires it. Its input and output shapes live in
//! `types/describe_scaling_process_types_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]; never construct an
//! engine in a handler. This operation needs no store, so it touches neither.
//!
//! ---
//!
//! # Modeled contract for `DescribeScalingProcessTypes`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DescribeScalingProcessTypes",model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`):
//!
//! Describes the scaling process types for use with the `ResumeProcesses` and
//! `SuspendProcesses` APIs.
//!
//! ## Input
//!
//! (no members — `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Processes` (Processes) — The names of the process types.
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
#[smithy_macros::operation_error("com.amazonaws.autoscaling#DescribeScalingProcessTypes")]
#[derive(Debug)]
pub enum DescribeScalingProcessTypesErrorModeled {
    ResourceContentionFault(ResourceContentionFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_scaling_process_types::DescribeScalingProcessTypesError`).
pub type DescribeScalingProcessTypesError =
    crate::scaffolded::Scaffolded<DescribeScalingProcessTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeScalingProcessTypesErrorModeled> for DescribeScalingProcessTypesError {
    fn from(modeled: DescribeScalingProcessTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeScalingProcessTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct DescribeScalingProcessTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeScalingProcessTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeScalingProcessTypes<Eng>
{
    type Input = DescribeScalingProcessTypesInput;
    type Output = DescribeScalingProcessTypesOutput;
    type Error = DescribeScalingProcessTypesError;

    const NAME: &'static str = "DescribeScalingProcessTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_SCALING_PROCESS_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_SCALING_PROCESS_TYPES_OUTPUT_SCHEMA;
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

        Ok(DescribeScalingProcessTypesOutput {
            processes: Some(
                crate::state::SCALING_PROCESS_TYPES
                    .iter()
                    .map(|process_name| ProcessType {
                        process_name: (*process_name).to_string(),
                    })
                    .collect(),
            ),
        })
    }
}
