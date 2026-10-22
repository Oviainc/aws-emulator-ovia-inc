// SPDX-License-Identifier: CC0-1.0

//! @op ListAvailableSolutionStacks
//! Reports this service's solution-stack catalog. This is the ONLY file to touch
//! for this operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet
//! (see `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired
//! and the framework answered it from the unroutable-operation path — a query
//! `InvalidAction`, which is not an answer any AWS client can work with. The input,
//! output, error enum and schemas a generated stub would have carried are written
//! out below in the same shape as a rendered sibling
//! (`disassociate_environment_operations_role.rs`); `build.rs` picks the operation
//! up from the `@op` line above and wires it into the service's routing table.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]. The catalog this
//! operation reports is service reference data, not per-account state, so it comes
//! from [`crate::state::SOLUTION_STACKS`] — the one table every operation that
//! names a solution stack reads.
//!
//! ---
//!
//! # Modeled contract for `ListAvailableSolutionStacks`
//!
//! Returns a list of the available solution stack names, with the public version
//! first and then in reverse chronological order.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `SolutionStacks` (AvailableSolutionStackNamesList) — A list of available
//!   solution stacks.
//! * `SolutionStackDetails` (AvailableSolutionStackDetailsList) — A list of available
//!   solution stacks and their `SolutionStackDescription`.
//!
//! ## Errors
//!
//! None modelled.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json");

/// `ListAvailableSolutionStacks` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct ListAvailableSolutionStacksInput;

/// One entry of `SolutionStackDetails`. Declared here rather than in `types/`
/// because this is the only operation in the service that reaches the shape.
#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#SolutionStackDescription")]
#[derive(Debug, Clone, Default)]
pub struct SolutionStackDescription {
    pub solution_stack_name: Option<String>,
    pub permitted_file_types: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#ListAvailableSolutionStacksResultMessage")]
#[derive(Debug, Clone, Default)]
pub struct ListAvailableSolutionStacksOutput {
    pub solution_stacks: Option<Vec<String>>,
    pub solution_stack_details: Option<Vec<SolutionStackDescription>>,
}

// The operation's MODELED errors. The model declares none for this operation, so
// this enum is UNINHABITED, which is correct: there is nothing to construct until
// the model says otherwise. The wrapper below is still what carries the errors the
// SERVICE names without a shape.
#[smithy_macros::operation_error("com.amazonaws.elasticbeanstalk#ListAvailableSolutionStacks")]
#[derive(Debug)]
pub enum ListAvailableSolutionStacksErrorModeled {}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_available_solution_stacks::ListAvailableSolutionStacksError`).
pub type ListAvailableSolutionStacksError =
    crate::scaffolded::Scaffolded<ListAvailableSolutionStacksErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListAvailableSolutionStacksErrorModeled> for ListAvailableSolutionStacksError {
    fn from(modeled: ListAvailableSolutionStacksErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAvailableSolutionStacks` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct ListAvailableSolutionStacks<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAvailableSolutionStacks<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAvailableSolutionStacks<Eng>
{
    type Input = ListAvailableSolutionStacksInput;
    type Output = ListAvailableSolutionStacksOutput;
    type Error = ListAvailableSolutionStacksError;

    const NAME: &'static str = "ListAvailableSolutionStacks";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        LIST_AVAILABLE_SOLUTION_STACKS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        LIST_AVAILABLE_SOLUTION_STACKS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A pure read of the service's platform catalog: it takes no input, writes
        // nothing, and reports the same stacks to every caller. Both members come
        // from the ONE table, so the names and the details cannot disagree, and
        // each name is derived from the platform branch the stack belongs to, so
        // this catalog cannot disagree with `ListPlatformBranches` either.
        let _ = (&input, ctx, &self.engine);
        Ok(ListAvailableSolutionStacksOutput {
            solution_stacks: Some(
                crate::state::SOLUTION_STACKS
                    .iter()
                    .map(crate::state::SolutionStack::name)
                    .collect(),
            ),
            solution_stack_details: Some(
                crate::state::SOLUTION_STACKS
                    .iter()
                    .map(|stack| SolutionStackDescription {
                        solution_stack_name: Some(stack.name()),
                        permitted_file_types: Some(
                            stack
                                .permitted_file_types
                                .iter()
                                .map(|extension| (*extension).to_string())
                                .collect(),
                        ),
                    })
                    .collect(),
            ),
        })
    }
}
