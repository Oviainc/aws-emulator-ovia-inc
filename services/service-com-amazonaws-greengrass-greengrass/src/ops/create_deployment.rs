// SPDX-License-Identifier: CC0-1.0

//! @op CreateDeployment
//!
//! Raises a deployment against a stored group: mints the deployment id, derives the
//! deployment ARN from the group's stored ARN, and persists the record against the
//! group so `GetDeploymentStatus` reports it back. A group version the caller names
//! must be one the group actually holds, whichever deployment type is used; a
//! `Redeployment` that names none inherits the version of the deployment it replays.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), reached through
//! [`crate::state::service_state`]. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreateDeployment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-greengrass-greengrass/smithy-model/com-amazonaws-greengrass-greengrass.json")
//! ```
//!
//! Creates a deployment. ''CreateDeployment'' requests are idempotent with respect to the
//! ''X-Amzn-Client-Token'' token and the request parameters.
//!
//! ## Input
//!
//! * `AmznClientToken` (__string) — A client token used to correlate requests and responses.
//! * `DeploymentId` (__string) — The ID of the deployment if you wish to redeploy a previous
//!   deployment.
//! * `DeploymentType` (DeploymentType, one of: ForceResetDeployment | NewDeployment |
//!   Redeployment | ResetDeployment) **required** — The type of deployment. When used for
//!   ''CreateDeployment'', only ''NewDeployment'' and ''Redeployment'' are valid.
//! * `GroupId` (__string) **required** — The ID of the Greengrass group.
//! * `GroupVersionId` (__string) — The ID of the group version to be deployed.
//!
//! ## Output
//!
//! * `DeploymentArn` (__string) — The ARN of the deployment.
//! * `DeploymentId` (__string) — The ID of the deployment.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — General error information.

#[allow(unused_imports)]
use super::types::*;

use crate::state::DeploymentRecord;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateDeploymentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-greengrass-greengrass.json");

// The operation's MODELED errors. `InternalServerErrorException` is the service's own
// modelled server error (HTTP 500), widened in through `extra_errors` so the one
// unreachable-store path has a modelled answer.
#[smithy_macros::operation_error(
    "com.amazonaws.greengrass#CreateDeployment",
    extra_errors = ["InternalServerErrorException"]
)]
#[derive(Debug)]
pub enum CreateDeploymentErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_deployment::CreateDeploymentError`).
pub type CreateDeploymentError = crate::scaffolded::Scaffolded<CreateDeploymentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateDeploymentErrorModeled> for CreateDeploymentError {
    fn from(modeled: CreateDeploymentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The deployment types `CreateDeployment` itself accepts. The `DeploymentType` enum
/// also carries `ResetDeployment` and `ForceResetDeployment`, but the model documents
/// of that member: "When used for ''CreateDeployment'', only ''NewDeployment'' and
/// ''Redeployment'' are valid" — the reset types go through `ResetDeployments`.
const CREATE_DEPLOYMENT_TYPES: &[&str] = &["NewDeployment", "Redeployment"];

/// The `CreateDeployment` operation.
pub struct CreateDeployment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateDeployment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// `BadRequestException` carrying `message`, lifted into this operation's channel.
fn bad_request(message: impl Into<String>) -> CreateDeploymentError {
    CreateDeploymentErrorModeled::BadRequestException(crate::state::bad_request(message)).into()
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateDeployment<Eng>
{
    type Input = CreateDeploymentInput;
    type Output = CreateDeploymentOutput;
    type Error = CreateDeploymentError;

    const NAME: &'static str = "CreateDeployment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_DEPLOYMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_DEPLOYMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateDeploymentErrorModeled::InternalServerErrorException(
                crate::state::store_unreachable(),
            )
        })?;

        if !CREATE_DEPLOYMENT_TYPES.contains(&input.deployment_type.as_str()) {
            return Err(bad_request(format!(
                "DeploymentType must be one of {} for CreateDeployment, got '{}'",
                CREATE_DEPLOYMENT_TYPES.join(", "),
                input.deployment_type
            )));
        }

        let token_key = input
            .amzn_client_token
            .as_deref()
            .filter(|token| !token.is_empty())
            // Scoped to the parent group: the same token against another group names
            // another deployment.
            .map(|token| crate::state::client_token_key(Self::NAME, &input.group_id, token));

        let mut store = state.lock();
        let group = store.groups.get(&input.group_id).ok_or_else(|| {
            CreateDeploymentErrorModeled::BadRequestException(crate::state::bad_request(format!(
                "That Group Definition does not exist: {}",
                input.group_id
            )))
        })?;

        // A retried create carrying the same client token answers with the deployment
        // the first attempt stored, rather than raising a second one.
        let replayed = token_key
            .as_deref()
            .and_then(|key| store.client_tokens.resource_for(key))
            .cloned();
        if let Some(existing) =
            replayed.and_then(|id| group.deployments.iter().find(|d| d.deployment_id == id))
        {
            return Ok(CreateDeploymentOutput {
                deployment_arn: Some(existing.deployment_arn.clone()),
                deployment_id: Some(existing.deployment_id.clone()),
            });
        }

        // A group's deployment history is bounded: an emulator is a long-lived
        // process, so a caller cannot grow one group without limit.
        if group.deployments.len() >= crate::state::MAX_DEPLOYMENTS_PER_GROUP {
            return Err(bad_request(format!(
                "Group {} already holds the maximum of {} deployments",
                input.group_id,
                crate::state::MAX_DEPLOYMENTS_PER_GROUP
            )));
        }

        // Which group version this deployment carries depends on the type: a new
        // deployment names it directly, a redeployment replays an earlier deployment
        // and inherits its version unless the caller names one. A `GroupVersionId`
        // the caller DID supply is never ignored: it must be a version this group
        // holds, whichever type is being created.
        let requested_version = input
            .group_version_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        if let Some(version_id) = requested_version.as_deref() {
            if group.version(version_id).is_none() {
                return Err(bad_request(format!(
                    "Version {version_id} of Group Definition {} does not exist",
                    input.group_id
                )));
            }
        }
        let group_version_id = match input.deployment_type.as_str() {
            "Redeployment" => {
                let previous_id = input.deployment_id.as_deref().filter(|id| !id.is_empty());
                let previous_id = previous_id.ok_or_else(|| {
                    CreateDeploymentErrorModeled::BadRequestException(crate::state::bad_request(
                        "DeploymentId is required for a Redeployment".to_string(),
                    ))
                })?;
                let previous = group
                    .deployments
                    .iter()
                    .find(|d| d.deployment_id == previous_id)
                    .ok_or_else(|| {
                        CreateDeploymentErrorModeled::BadRequestException(
                            crate::state::bad_request(format!(
                                "Deployment {previous_id} of group {} does not exist",
                                input.group_id
                            )),
                        )
                    })?;
                // The caller's version wins when it named one; otherwise the replayed
                // deployment's version is what gets deployed again.
                requested_version.or_else(|| previous.group_version_id.clone())
            }
            // `NewDeployment`: the version is required, and was checked above.
            "NewDeployment" => Some(requested_version.ok_or_else(|| {
                CreateDeploymentErrorModeled::BadRequestException(crate::state::bad_request(
                    "GroupVersionId is required for a NewDeployment".to_string(),
                ))
            })?),
            // Unreachable while `CREATE_DEPLOYMENT_TYPES` holds exactly the two types
            // above, and a refusal rather than a silent `NewDeployment` if a third is
            // ever added to that list without being given its own arm here.
            other => {
                return Err(bad_request(format!(
                    "DeploymentType '{other}' is not supported by CreateDeployment"
                )))
            }
        };

        let deployment_id = crate::state::new_id();
        let deployment_arn = crate::state::deployment_arn(&group.arn, &deployment_id);
        let record = DeploymentRecord::accepted(
            deployment_id.clone(),
            deployment_arn.clone(),
            input.deployment_type.clone(),
            group_version_id,
            crate::state::now_timestamp(),
        );

        let group = store
            .groups
            .get_mut(&input.group_id)
            .expect("group presence checked above");
        group.deployments.push(record);

        if let Some(key) = token_key {
            store.client_tokens.remember(key, deployment_id.clone());
        }

        Ok(CreateDeploymentOutput {
            deployment_arn: Some(deployment_arn),
            deployment_id: Some(deployment_id),
        })
    }
}
