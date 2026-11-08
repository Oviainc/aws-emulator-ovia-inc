// SPDX-License-Identifier: CC0-1.0

//! @op DescribeWorkflowType
//!
//! A pure read: it reports the ONE stored workflow-type record addressed by
//! `(domain, name, version)` — its info and its registration-time defaults — and
//! writes nothing. It is the reader that `RegisterWorkflowType` round-trips through.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeWorkflowType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Returns information about the specified workflow type. This includes configuration
//! settings specified when the type was registered and other information such as creation
//! date, current status, etc. Access Control You can use IAM policies to control this
//! action's access to Amazon SWF resources as follows: Use a Resource element with the
//! domain name to limit the action to only specified domains. Use an Action element to
//! allow or deny permission to call this action. Constrain the following parameters by
//! using a Condition element with the appropriate keys. workflowType.name: String
//! constraint. The key is swf:workflowType.name. workflowType.version: String constraint.
//! The key is swf:workflowType.version. If the caller doesn't have sufficient permissions
//! to invoke the action, or the parameter values fall outside the specified constraints,
//! the action fails. The associated event attribute's cause parameter is set to
//! OPERATION_NOT_PERMITTED. For details and example IAM policies, see Using IAM to Manage
//! Access to Amazon SWF Workflows in the Amazon SWF Developer Guide.
//!
//! ## Input
//!
//! * `domain` (DomainName, len 1..256) **required** — The name of the domain in which this
//!   workflow type is registered.
//! * `workflowType` (WorkflowType) **required** — The workflow type to describe.
//!
//! ## Output
//!
//! * `configuration` (WorkflowTypeConfiguration) **required** — Configuration settings of the
//!   workflow type registered through RegisterWorkflowType
//! * `typeInfo` (WorkflowTypeInfo) **required** — General information about the workflow
//!   type. The status of the workflow type (returned in the WorkflowTypeInfo structure) can
//!   be one of the following. REGISTERED – The type is registered and available. Workers
//!   supporting this type should be running. DEPRECATED – The type was deprecated using
//!   DeprecateWorkflowType, but is still in use. You should keep workers supporting this type
//!   running. You cannot create new workflow executions of this type.
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
// `DescribeWorkflowTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeWorkflowTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#DescribeWorkflowType")]
#[derive(Debug)]
pub enum DescribeWorkflowTypeErrorModeled {
    OperationNotPermittedFault(OperationNotPermittedFault),
    UnknownResourceFault(UnknownResourceFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_workflow_type::DescribeWorkflowTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeWorkflowTypeError = crate::scaffolded::Scaffolded<DescribeWorkflowTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeWorkflowTypeErrorModeled> for DescribeWorkflowTypeError {
    fn from(modeled: DescribeWorkflowTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeWorkflowType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeWorkflowType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeWorkflowType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeWorkflowType<Eng>
{
    type Input = DescribeWorkflowTypeInput;
    type Output = DescribeWorkflowTypeOutput;
    type Error = DescribeWorkflowTypeError;

    const NAME: &'static str = "DescribeWorkflowType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_WORKFLOW_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_WORKFLOW_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeWorkflowTypeErrorModeled::OperationNotPermittedFault(
                OperationNotPermittedFault {
                    message: Some("the SWF domain store is unavailable".to_string()),
                },
            )
        })?;

        let guard = state.lock();
        // An `UnknownResourceFault` message names the resource the REQUEST asked for.
        // This operation addresses one workflow type, so the type is what the fault
        // names — in the `WorkflowType=[name=.., version=..]` form every operation of
        // this service that reports a missing workflow type uses — whether the lookup
        // missed because the domain holds no such type or because this service holds no
        // such domain: either way the type the request named is what could not be
        // resolved, and the domain the lookup was scoped to is never part of the clause.
        let unknown_type = || {
            DescribeWorkflowTypeErrorModeled::UnknownResourceFault(UnknownResourceFault {
                message: Some(crate::state::unknown_workflow_type_message(
                    &input.workflow_type.name,
                    &input.workflow_type.version,
                )),
            })
        };
        let domain = guard.domains.get(&input.domain).ok_or_else(unknown_type)?;
        let workflow_type = domain
            .workflow_types
            .get(&(
                input.workflow_type.name.clone(),
                input.workflow_type.version.clone(),
            ))
            .ok_or_else(unknown_type)?;

        Ok(DescribeWorkflowTypeOutput {
            type_info: workflow_type.info(),
            configuration: workflow_type.configuration(),
        })
    }
}
