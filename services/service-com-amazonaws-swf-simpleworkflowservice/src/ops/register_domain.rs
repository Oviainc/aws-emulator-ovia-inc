// SPDX-License-Identifier: CC0-1.0

//! @op RegisterDomain
//!
//! Inserts a domain record into [`crate::state`] with status `REGISTERED`, and answers
//! the modelled `DomainAlreadyExistsFault` for a second registration of the same name.
//! The output body is empty (the operation's output shape is `Unit`), so the ONLY
//! observable effect is the stored record every other domain-scoped operation reads.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RegisterDomain`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-swf-simpleworkflowservice/smithy-model/com-amazonaws-swf-simpleworkflowservice.json")
//! ```
//!
//! Registers a new domain. Access Control You can use IAM policies to control this action's
//! access to Amazon SWF resources as follows: You cannot use an IAM policy to control
//! domain access for this action. The name of the domain being registered is available as
//! the resource of this action. Use an Action element to allow or deny permission to call
//! this action. You cannot use an IAM policy to constrain this action's parameters. If the
//! caller doesn't have sufficient permissions to invoke the action, or the parameter values
//! fall outside the specified constraints, the action fails. The associated event
//! attribute's cause parameter is set to OPERATION_NOT_PERMITTED. For details and example
//! IAM policies, see Using IAM to Manage Access to Amazon SWF Workflows in the Amazon SWF
//! Developer Guide.
//!
//! ## Input
//!
//! * `description` (Description, len 0..1024) — A text description of the domain.
//! * `name` (DomainName, len 1..256) **required** — Name of the domain to register. The name
//!   must be unique in the region that the domain is registered in. The specified string must
//!   not start or end with whitespace. It must not contain a : (colon), / (slash), |
//!   (vertical bar), or any control characters (\u0000-\u001f | \u007f-\u009f). Also, it must
//!   not be the literal string arn.
//! * `tags` (ResourceTagList) — Tags to be added when registering a domain. Tags may only
//!   contain unicode letters, digits, whitespace, or these symbols: _ . : / = + - @.
//! * `workflowExecutionRetentionPeriodInDays` (DurationInDays, len 1..8) **required** — The
//!   duration (in days) that records and histories of workflow executions on the domain
//!   should be kept by the service. After the retention period, the workflow execution isn't
//!   available in the results of visibility calls. If you pass the value NONE or 0 (zero),
//!   then the workflow execution history isn't retained. As soon as the workflow execution
//!   completes, the execution record and its history are deleted. The maximum workflow
//!   execution retention period is 90 days. For more information about Amazon SWF service
//!   limits, see: Amazon SWF Service Limits in the Amazon SWF Developer Guide.
//!
//! ## Errors
//!
//! * `DomainAlreadyExistsFault` (HTTP 400) — Returned if the domain already exists. You may
//!   get this fault if you are registering a domain that is either already registered or
//!   deprecated, or if you undeprecate a domain that is currently registered.
//! * `LimitExceededFault` (HTTP 400) — Returned by any operation if a system imposed
//!   limitation has been reached. To address this fault you should either clean up unused
//!   resources or increase the limit by contacting AWS.
//! * `OperationNotPermittedFault` (HTTP 400) — Returned when the caller doesn't have
//!   sufficient permissions to invoke the action.
//! * `TooManyTagsFault` (HTTP 400) — You've exceeded the number of tags allowed for a domain.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RegisterDomainError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-swf-simpleworkflowservice.json");

#[derive(Debug, Clone, Default)]
pub struct RegisterDomainOutput;

impl RegisterDomainOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static REGISTER_DOMAIN_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &RegisterDomainOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for RegisterDomainOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RegisterDomainError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.swf#RegisterDomain")]
#[derive(Debug)]
pub enum RegisterDomainErrorModeled {
    DomainAlreadyExistsFault(DomainAlreadyExistsFault),
    LimitExceededFault(LimitExceededFault),
    OperationNotPermittedFault(OperationNotPermittedFault),
    TooManyTagsFault(TooManyTagsFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::register_domain::RegisterDomainError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RegisterDomainError = crate::scaffolded::Scaffolded<RegisterDomainErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RegisterDomainErrorModeled> for RegisterDomainError {
    fn from(modeled: RegisterDomainErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RegisterDomain` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RegisterDomain<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RegisterDomain<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RegisterDomain<Eng>
{
    type Input = RegisterDomainInput;
    type Output = RegisterDomainOutput;
    type Error = RegisterDomainError;

    const NAME: &'static str = "RegisterDomain";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REGISTER_DOMAIN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = REGISTER_DOMAIN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            // The model declares no server fault for this operation, and
            // `OperationNotPermittedFault` is this service's common error: the request
            // cannot be served, and saying so with a modelled code beats a 501, which
            // would claim the operation is unwritten.
            RegisterDomainErrorModeled::OperationNotPermittedFault(OperationNotPermittedFault {
                message: Some("the SWF domain store is unavailable".to_string()),
            })
        })?;

        crate::state::validate_name("name", &input.name)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        crate::state::validate_retention_period(&input.workflow_execution_retention_period_in_days)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        let tags = input.tags.clone().unwrap_or_default();
        for tag in &tags {
            crate::state::validate_tag(&tag.key, tag.value.as_deref())
                .map_err(crate::scaffolded::Scaffolded::validation)?;
        }
        if tags.len() > crate::state::MAX_TAGS_PER_DOMAIN {
            return Err(RegisterDomainErrorModeled::TooManyTagsFault(TooManyTagsFault {
                // The same fixed sentence `TagResource` refuses the same bound with:
                // the fault names neither the count nor the resource.
                message: Some(crate::state::TOO_MANY_TAGS_MESSAGE.to_string()),
            })
            .into());
        }

        let region = crate::state::request_region(ctx);
        let mut guard = state.lock();

        // A name that is already registered (or deprecated) is a conflict, not a
        // silent re-registration: the modelled fault says "You may get this fault if
        // you are registering a domain that is either already registered or
        // deprecated".
        if guard.domains.contains_key(&input.name) {
            return Err(RegisterDomainErrorModeled::DomainAlreadyExistsFault(
                DomainAlreadyExistsFault {
                    // The fault IDENTIFIES the domain that already exists: its whole
                    // message is the requested name, with no text around it, which is
                    // what lets a caller read the message as the resource identifier.
                    message: Some(input.name.clone()),
                },
            )
            .into());
        }

        if guard.domains.len() >= crate::state::MAX_DOMAINS {
            return Err(RegisterDomainErrorModeled::LimitExceededFault(
                LimitExceededFault {
                    message: Some(format!(
                        "this emulator holds at most {} domains",
                        crate::state::MAX_DOMAINS
                    )),
                },
            )
            .into());
        }

        let mut domain = crate::state::Domain::registered(
            &input.name,
            &region,
            &input.workflow_execution_retention_period_in_days,
        );
        domain.description = input.description.clone();
        domain.tags = tags;
        guard.domains.insert(input.name.clone(), domain);

        Ok(RegisterDomainOutput)
    }
}
