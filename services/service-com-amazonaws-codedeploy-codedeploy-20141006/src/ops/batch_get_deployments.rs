// SPDX-License-Identifier: CC0-1.0

//! @op BatchGetDeployments
//! Answers the stored `DeploymentInfo` of each deployment id the request names.
//!
//! This is the ONLY file to touch for this operation.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `BatchGetDeployments::new(engine)` and drives it through the centralized request walk:
//! deserialize -> validate -> `handle` -> serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request, so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::with_state`], which also settles every transition the clock already
//! owes before this operation reads a record.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only -
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchGetDeployments`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json")
//! ```
//!
//! Gets information about one or more deployments. The maximum number of deployments that
//! can be returned is 25.
//!
//! ## Input
//!
//! * `deploymentIds` (DeploymentsList) **required** - A list of deployment IDs, separated by
//!   spaces. The maximum number of deployment IDs you can specify is 25.
//!
//! ## Output
//!
//! * `deploymentsInfo` (DeploymentsInfoList) - Information about the deployments.
//!
//! ## Errors
//!
//! * `BatchLimitExceededException` (HTTP 400) - The maximum number of names or IDs allowed
//!   for this request (100) was exceeded.
//! * `DeploymentIdRequiredException` (HTTP 400) - At least one deployment ID must be
//!   specified.
//! * `InvalidDeploymentIdException` (HTTP 400) - At least one of the deployment IDs was
//!   specified in an invalid format.

#[allow(unused_imports)]
use super::types::*;

/// How many deployment ids one `BatchGetDeployments` request may carry.
///
/// The model documents the bound on the member itself:
/// `smithy_query(query="doc",shape="BatchGetDeploymentsInput",member="deploymentIds")`
/// reads "A list of deployment IDs, separated by spaces. The maximum number of
/// deployment IDs you can specify is 25.", so this is CodeDeploy's own published
/// limit for THIS operation rather than a chosen one. Exceeding it is what
/// `BatchLimitExceededException` states; the "(100)" in that shape's own doc is
/// the generic ceiling of a shape several batch operations share, and a batch
/// that exceeds 100 exceeds this bound too. It lives here, beside the one
/// operation that enforces it, so no other operation can drift from it.
const MAX_BATCH_DEPLOYMENT_IDS: usize = 25;

/// Whether `value` has the form of a CodeDeploy deployment id: delegated to
/// `crate::state::deployment_id_is_well_formed`, the crate's one gate for that
/// format, so this reader and the single-id readers
/// (`GetDeployment`, `StopDeployment`, …) cannot disagree about which value is
/// malformed and which simply names no stored deployment.
fn is_deployment_id(value: &str) -> bool {
    crate::state::deployment_id_is_well_formed(value)
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchGetDeploymentsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.codedeploy#BatchGetDeployments")]
#[derive(Debug)]
pub enum BatchGetDeploymentsErrorModeled {
    BatchLimitExceededException(BatchLimitExceededException),
    DeploymentIdRequiredException(DeploymentIdRequiredException),
    InvalidDeploymentIdException(InvalidDeploymentIdException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_get_deployments::BatchGetDeploymentsError`). The wrapper also carries
/// the refusal channel this handler uses for an engine that is not the shared store.
pub type BatchGetDeploymentsError = crate::scaffolded::Scaffolded<BatchGetDeploymentsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<BatchGetDeploymentsErrorModeled> for BatchGetDeploymentsError {
    fn from(modeled: BatchGetDeploymentsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchGetDeployments` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchGetDeployments<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchGetDeployments<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE - the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchGetDeployments<Eng>
{
    type Input = BatchGetDeploymentsInput;
    type Output = BatchGetDeploymentsOutput;
    type Error = BatchGetDeploymentsError;

    const NAME: &'static str = "BatchGetDeployments";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_DEPLOYMENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_DEPLOYMENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let deployment_ids = input.deployment_ids;

        // The batch bound is a property of the REQUEST as a whole, so it is
        // answered before any id in it is examined: a caller that sent too many
        // ids learns that, rather than learning that one of them is malformed.
        if deployment_ids.len() > MAX_BATCH_DEPLOYMENT_IDS {
            return Err(BatchGetDeploymentsErrorModeled::BatchLimitExceededException(
                BatchLimitExceededException {
                    message: Some(format!(
                        "The maximum number of deployment IDs allowed for this request ({}) was \
                         exceeded: {} were specified",
                        MAX_BATCH_DEPLOYMENT_IDS,
                        deployment_ids.len()
                    )),
                },
            )
            .into());
        }

        // `deploymentIds` is @required but carries no modelled length, so an empty
        // list - and an empty id inside a non-empty list - both reach the handler,
        // and both are what `DeploymentIdRequiredException` is for.
        // An empty or whitespace-only id is "you did not give me this id", and a
        // value that is not shaped like one the service issues is a FORMAT refusal
        // — the same two refusals, in the same order, as the single-id reader
        // `GetDeployment` answers for its `deploymentId`.
        if deployment_ids.is_empty() || deployment_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(
                BatchGetDeploymentsErrorModeled::DeploymentIdRequiredException(
                    DeploymentIdRequiredException {
                        message: Some("At least one deployment ID must be specified".to_string()),
                    },
                )
                .into(),
            );
        }

        // A value that is not shaped like a deployment id names no deployment in
        // any account: that is a format refusal, not an absence from this store.
        // The wording comes from the crate's one builder, so every operation that
        // gates this format refuses it in the same words.
        if let Some(malformed) = deployment_ids.iter().find(|id| !is_deployment_id(id)) {
            return Err(
                BatchGetDeploymentsErrorModeled::InvalidDeploymentIdException(
                    InvalidDeploymentIdException {
                        message: Some(crate::state::malformed_deployment_id_message(malformed)),
                    },
                )
                .into(),
            );
        }

        // One entry per id this store holds, in the order the request named them.
        // A well-formed id the store does not hold is simply absent from the
        // answer: this operation models no does-not-exist error, so an unknown id
        // cannot be reported as one. The statuses reported come from the stored
        // records, which `with_state` has already brought up to the instants the
        // clock owed them.
        let deployments_info = crate::state::with_state(&*self.engine, |state| {
            deployment_ids
                .iter()
                .filter_map(|id| state.deployments.get(id))
                .map(crate::state::deployment_info)
                .collect::<Vec<_>>()
        })
        .ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(crate::state::STATE_UNAVAILABLE)
        })?;

        Ok(BatchGetDeploymentsOutput {
            deployments_info: Some(deployments_info),
        })
    }
}
