// SPDX-License-Identifier: CC0-1.0

//! @op CreateResourceGroup
//!
//! Mints a resource group from the tags the caller sent and PERSISTS it under a generated
//! `…:resourcegroup/0-xxxxxxxx` ARN, so `DescribeResourceGroups` hands the same tags and
//! creation time back. The ARN is the whole of the response, so it is stored before it is
//! returned — never minted and discarded.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateResourceGroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-inspector-inspectorservice/smithy-model/com-amazonaws-inspector-inspectorservice.json")
//! ```
//!
//! Creates a resource group using the specified set of tags (key and value pairs) that are
//! used to select the EC2 instances to be included in an Amazon Inspector assessment
//! target. The created resource group is then used to create an Amazon Inspector assessment
//! target. For more information, see CreateAssessmentTarget.
//!
//! ## Input
//!
//! * `resourceGroupTags` (ResourceGroupTags, len 1..10) **required** — A collection of keys
//!   and an array of possible values,
//!   '[{"key":"key1","values":["Value1","Value2"]},{"key":"Key2","values":["Value3"]}]'. For
//!   example,'[{"key":"Name","values":["TestEC2Instance"]}]'.
//!
//! ## Output
//!
//! * `resourceGroupArn` (Arn, len 1..300) **required** — The ARN that specifies the resource
//!   group that is created.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have required permissions to access the
//!   requested resource.
//! * `InternalException` (HTTP 500) — Internal server error.
//! * `InvalidInputException` (HTTP 400) — The request was rejected because an invalid or
//!   out-of-range value was supplied for an input parameter.
//! * `LimitExceededException` (HTTP 400) — The request was rejected because it attempted to
//!   create resources beyond the current AWS account limits. The error code describes the
//!   limit exceeded.
//! * `ServiceTemporarilyUnavailableException` (HTTP 503) — The serice is temporary
//!   unavailable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateResourceGroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-inspector-inspectorservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateResourceGroupError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.inspector#CreateResourceGroup")]
#[derive(Debug)]
pub enum CreateResourceGroupErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalException(InternalException),
    InvalidInputException(InvalidInputException),
    LimitExceededException(LimitExceededException),
    ServiceTemporarilyUnavailableException(ServiceTemporarilyUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_resource_group::CreateResourceGroupError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateResourceGroupError = crate::scaffolded::Scaffolded<CreateResourceGroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateResourceGroupErrorModeled> for CreateResourceGroupError {
    fn from(modeled: CreateResourceGroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateResourceGroup` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateResourceGroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateResourceGroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateResourceGroup<Eng>
{
    type Input = CreateResourceGroupInput;
    type Output = CreateResourceGroupOutput;
    type Error = CreateResourceGroupError;

    const NAME: &'static str = "CreateResourceGroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESOURCE_GROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESOURCE_GROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The modelled 1..10 bound on `resourceGroupTags` is refused before this handler
        // runs, so the tags here already satisfy it.
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateResourceGroupErrorModeled::InternalException(InternalException {
                message: "the Inspector state store is unavailable".to_string(),
                can_retry: true,
            })
            .into());
        };
        let region = crate::state::request_region(ctx);
        let created_at = crate::state::date_time(::std::time::SystemTime::now());

        let mut store = state.lock();
        if store.resource_groups.len() >= crate::state::MAX_RESOURCE_GROUPS {
            return Err(CreateResourceGroupErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: format!(
                        "The account already holds the maximum of {} resource groups",
                        crate::state::MAX_RESOURCE_GROUPS
                    ),
                    error_code: "RESOURCE_GROUP_LIMIT_EXCEEDED".to_string(),
                    can_retry: false,
                },
            )
            .into());
        }
        let id = store.mint_id();
        let arn = crate::state::inspector_arn(&region, &format!("resourcegroup/{id}"));
        store.resource_groups.insert(
            arn.clone(),
            ResourceGroup {
                arn: arn.clone(),
                tags: input.resource_group_tags,
                created_at,
            },
        );

        Ok(CreateResourceGroupOutput {
            resource_group_arn: arn,
        })
    }
}
